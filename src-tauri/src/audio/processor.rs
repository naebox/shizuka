use anyhow::Result;
use deep_filter::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

use crate::config::ModelVariant;

pub const DFN_SAMPLE_RATE: usize = 48_000;
pub const DFN_HOP_SIZE: usize = 480; // 10ms frame at 48kHz

const DFN3_STANDARD_BYTES: &[u8] = include_bytes!("../../models/DeepFilterNet3_onnx.tar.gz");

/// Hysteresis + Hold + Sample-Smoothed Voice Activity Detection (VAD) Noise Gate.
/// Uses DeepFilterNet3's neural local SNR (`lsnr`) and post-filtered frame energy
/// to silence residual keystrokes and room hiss completely between speech phrases
/// without clipping word onsets or trailing syllables.
struct VadGate {
    enabled: bool,
    open_thresh_db: f32,
    is_open: bool,
    hold_frames_left: usize,
    envelope_gain: f32,
}

impl VadGate {
    const HOLD_FRAMES: usize = 18; // 180ms hangover hold after speech drops below close threshold

    fn new(enabled: bool, open_thresh_db: f32) -> Self {
        Self {
            enabled,
            open_thresh_db: open_thresh_db.clamp(-18.0, 12.0),
            is_open: false,
            hold_frames_left: 0,
            envelope_gain: if enabled { 0.0 } else { 1.0 },
        }
    }

    #[inline]
    fn set_params(&mut self, enabled: bool, open_thresh_db: f32) {
        self.enabled = enabled;
        self.open_thresh_db = open_thresh_db.clamp(-18.0, 12.0);
    }

    /// Evaluates speech presence and applies per-sample smooth envelope gain to `enh_480`.
    /// Returns `true` when voice activity is currently detected (including hangover hold).
    fn process(&mut self, lsnr_db: f32, enh_480: &mut [f32], atten_lim_db: f32) -> bool {
        if !self.enabled {
            self.is_open = true;
            // Smoothly ramp back to 1.0 if just disabled
            for s in enh_480.iter_mut() {
                self.envelope_gain += 0.02 * (1.0 - self.envelope_gain);
                *s *= self.envelope_gain;
            }
            return lsnr_db >= self.open_thresh_db;
        }

        // Measure post-DFN frame energy so dead-quiet floor never false-triggers
        let mut sum_sq = 0.0f32;
        for &s in enh_480.iter() {
            sum_sq += s * s;
        }
        let rms = (sum_sq / DFN_HOP_SIZE as f32).sqrt();
        let rms_db = if rms < 1e-6 {
            -120.0
        } else {
            20.0 * rms.log10()
        };

        let speech_active_now = if self.is_open {
            // 3.5 dB hysteresis keeps trailing consonants & quiet sentence endings intact
            lsnr_db >= (self.open_thresh_db - 3.5) && rms_db >= -66.0
        } else {
            (lsnr_db >= self.open_thresh_db && rms_db >= -60.0)
                || (lsnr_db >= self.open_thresh_db + 4.5 && rms_db >= -65.0)
        };

        if speech_active_now {
            self.is_open = true;
            self.hold_frames_left = Self::HOLD_FRAMES;
        } else if self.hold_frames_left > 0 {
            self.hold_frames_left -= 1;
            self.is_open = true;
        } else {
            self.is_open = false;
        }

        // Complete silence (0.0) in Balanced/Max modes; gentle -32 dB floor in Natural mode
        let closed_floor = if atten_lim_db >= 45.0 { 0.0 } else { 0.025 };
        let target_gain = if self.is_open { 1.0 } else { closed_floor };

        // Fast ~3.5ms attack on speech onset; smooth ~110ms exponential release after hold
        let alpha = if target_gain > self.envelope_gain {
            0.018_f32
        } else {
            0.00045_f32
        };

        for s in enh_480.iter_mut() {
            self.envelope_gain += alpha * (target_gain - self.envelope_gain);
            let g = if self.envelope_gain < 1e-4 && target_gain == 0.0 {
                0.0
            } else {
                self.envelope_gain
            };
            *s *= g;
        }

        self.is_open
    }
}

pub struct NoiseProcessor {
    model: DfTract,
    variant: ModelVariant,
    in_frame: Array2<f32>,
    out_frame: Array2<f32>,
    nc_enabled: bool,
    wet_mix: f32,
    input_gain_linear: f32,
    atten_lim_db: f32,
    pf_beta: f32,
    vad_gate: VadGate,
}

impl NoiseProcessor {
    pub fn new(
        variant: ModelVariant,
        atten_lim_db: f32,
        pf_beta: f32,
        min_db_thresh: f32,
        vad_enabled: bool,
        vad_thresh_db: f32,
    ) -> Result<Self> {
        let df_params = match variant {
            ModelVariant::Standard => DfParams::from_bytes(DFN3_STANDARD_BYTES)?,
            ModelVariant::LowLatency => DfParams::default(),
        };

        let mut rp = RuntimeParams::default_with_ch(1);
        rp.atten_lim_db = atten_lim_db;
        rp.post_filter_beta = pf_beta;
        rp.min_db_thresh = min_db_thresh;

        let mut model = DfTract::new(df_params, &rp)?;
        model.set_atten_lim(atten_lim_db);
        model.set_pf_beta(pf_beta);

        let hop = model.hop_size;
        assert_eq!(
            hop, DFN_HOP_SIZE,
            "Expected DeepFilterNet3 hop_size of 480 samples at 48kHz"
        );

        Ok(Self {
            model,
            variant,
            in_frame: Array2::<f32>::zeros((1, hop)),
            out_frame: Array2::<f32>::zeros((1, hop)),
            nc_enabled: true,
            wet_mix: 1.0,
            input_gain_linear: 1.0,
            atten_lim_db,
            pf_beta,
            vad_gate: VadGate::new(vad_enabled, vad_thresh_db),
        })
    }

    pub fn variant(&self) -> ModelVariant {
        self.variant
    }

    /// Algorithmic latency in ms: 10ms hop + lookahead frames (30ms for Standard, 10ms for LowLatency).
    pub fn algorithmic_latency_ms(&self) -> f32 {
        (1 + self.model.lookahead) as f32 * 10.0
    }

    #[inline]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.nc_enabled = enabled;
    }

    #[inline]
    pub fn set_input_gain_db(&mut self, gain_db: f32) {
        let clamped = gain_db.clamp(-24.0, 24.0);
        self.input_gain_linear = 10.0_f32.powf(clamped / 20.0);
    }

    pub fn set_attenuation_limit(&mut self, db: f32) {
        let clamped = db.clamp(6.0, 100.0);
        if (clamped - self.atten_lim_db).abs() > 0.05 {
            self.atten_lim_db = clamped;
            self.model.set_atten_lim(clamped);
        }
    }

    pub fn set_post_filter_beta(&mut self, beta: f32) {
        let clamped = beta.clamp(0.0, 0.06);
        if (clamped - self.pf_beta).abs() > 0.0005 {
            self.pf_beta = clamped;
            self.model.set_pf_beta(clamped);
        }
    }

    pub fn set_min_db_thresh(&mut self, min_db: f32) {
        self.model.min_db_thresh = min_db.clamp(-20.0, 20.0);
    }

    #[inline]
    pub fn set_vad_gate(&mut self, enabled: bool, thresh_db: f32) {
        self.vad_gate.set_params(enabled, thresh_db);
    }

    /// Processes exactly 480 samples (10ms @ 48kHz) with zero heap allocations.
    /// Writes gain-adjusted raw signal into `raw_out_480`, always-denoised (+ VAD gated) signal into `enh_out_480`,
    /// and final output (denoised or bypassed with smooth crossfade) into `final_out_480`.
    /// Returns `(lsnr_db, vad_speaking)`.
    pub fn process_frame_48k(
        &mut self,
        input_480: &[f32],
        raw_out_480: &mut [f32],
        enh_out_480: &mut [f32],
        final_out_480: &mut [f32],
    ) -> (f32, bool) {
        debug_assert_eq!(input_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(raw_out_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(enh_out_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(final_out_480.len(), DFN_HOP_SIZE);

        let gain = self.input_gain_linear;
        let mut in_slice = self.in_frame.row_mut(0);

        for i in 0..DFN_HOP_SIZE {
            let s = soft_limit(input_480[i] * gain);
            in_slice[i] = s;
            raw_out_480[i] = s;
        }

        let lsnr = self
            .model
            .process(self.in_frame.view(), self.out_frame.view_mut())
            .unwrap_or(-15.0);

        let enh_slice = self.out_frame.row(0);
        for i in 0..DFN_HOP_SIZE {
            enh_out_480[i] = soft_limit(enh_slice[i]);
        }

        // Apply post-DFN VAD Noise Gate to eliminate any residual non-speech artifacts
        let vad_speaking = self
            .vad_gate
            .process(lsnr, enh_out_480, self.atten_lim_db);

        let target_mix = if self.nc_enabled { 1.0_f32 } else { 0.0_f32 };

        if (self.wet_mix - target_mix).abs() < 1e-4 {
            self.wet_mix = target_mix;
            if self.nc_enabled {
                final_out_480.copy_from_slice(enh_out_480);
            } else {
                final_out_480.copy_from_slice(raw_out_480);
            }
        } else {
            let step = (target_mix - self.wet_mix) / (DFN_HOP_SIZE as f32);
            let mut mix = self.wet_mix;
            for i in 0..DFN_HOP_SIZE {
                mix = (mix + step).clamp(0.0, 1.0);
                let dry = raw_out_480[i];
                let wet = enh_out_480[i];
                final_out_480[i] = dry * (1.0 - mix) + wet * mix;
            }
            self.wet_mix = target_mix;
        }

        (lsnr, vad_speaking)
    }
}

#[inline]
pub(crate) fn soft_limit(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let abs = x.abs();
    if abs <= 0.92 {
        x
    } else {
        let sign = x.signum();
        let excess = (abs - 0.92) / 0.08;
        sign * (0.92 + 0.08 * excess.tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_dfn3_and_vad_gate_silence_noise() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::Standard,
            80.0,
            0.02,
            -10.0,
            true,
            -5.0,
        )
        .expect("Failed to load Standard DeepFilterNet3");

        assert!(
            proc.algorithmic_latency_ms() >= 20.0,
            "Standard DFN3 should include lookahead frames"
        );

        let mut raw_out = [0.0f32; DFN_HOP_SIZE];
        let mut enh_out = [0.0f32; DFN_HOP_SIZE];
        let mut clean_out = [0.0f32; DFN_HOP_SIZE];

        let mut seed: u32 = 0x1234_5678;
        let mut in_energy = 0.0f64;
        let mut out_energy = 0.0f64;

        // Run 80 frames (800ms) of stationary noise; measure last 20 frames after VAD gate closes
        for frame_idx in 0..80 {
            let mut noisy = [0.0f32; DFN_HOP_SIZE];
            for s in noisy.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let rand_unit = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
                *s = rand_unit * 0.15;
            }
            let (_lsnr, _speaking) =
                proc.process_frame_48k(&noisy, &mut raw_out, &mut enh_out, &mut clean_out);

            if frame_idx >= 60 {
                for i in 0..DFN_HOP_SIZE {
                    in_energy += (raw_out[i] as f64).powi(2);
                    out_energy += (clean_out[i] as f64).powi(2);
                }
            }
        }

        let in_rms = (in_energy / (20.0 * DFN_HOP_SIZE as f64)).sqrt();
        let out_rms = (out_energy / (20.0 * DFN_HOP_SIZE as f64)).sqrt();
        let reduction_db = 20.0 * (in_rms / out_rms.max(1e-9)).log10();

        assert!(
            reduction_db > 40.0,
            "Expected >40 dB combined DFN3 + VAD suppression on pure noise, got {:.2} dB",
            reduction_db
        );
    }

    #[test]
    fn test_low_latency_dfn3_ll_loads_and_runs() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::LowLatency,
            65.0,
            0.02,
            -10.0,
            false,
            -5.0,
        )
        .expect("Failed to load LowLatency DeepFilterNet3_ll");

        let mut raw_out = [0.0f32; DFN_HOP_SIZE];
        let mut enh_out = [0.0f32; DFN_HOP_SIZE];
        let mut clean_out = [0.0f32; DFN_HOP_SIZE];
        let noisy = [0.02f32; DFN_HOP_SIZE];
        let (_lsnr, _) = proc.process_frame_48k(&noisy, &mut raw_out, &mut enh_out, &mut clean_out);
    }
}
