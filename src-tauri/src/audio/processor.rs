use anyhow::Result;
use deep_filter::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

use crate::audio::aec::AecProcessor;
use crate::config::ModelVariant;

pub const DFN_SAMPLE_RATE: usize = 48_000;
pub const DFN_HOP_SIZE: usize = 480; // 10ms frame at 48kHz
pub const HPF_CUTOFF_HZ: f64 = 65.0; // Removes desk rumble & plosives while preserving 85-120Hz voice fundamentals

/// Knee threshold of `soft_limit()` (~ -0.72 dBFS); exceeding this triggers clipping detection.
pub const CLIP_DETECT_THRESH: f32 = 0.92;

const DFN3_STANDARD_BYTES: &[u8] = include_bytes!("../../models/DeepFilterNet3_onnx.tar.gz");

/// Result metrics emitted by `NoiseProcessor::process_frame_48k` per 10ms frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameProcessResult {
    pub lsnr_db: f32,
    pub speech_active_now: bool,
    pub vad_speaking: bool,
    pub voice_gate_blocked: bool,
    pub agc_gain_db: f32,
    pub aec_active: bool,
    pub aec_erle_db: f32,
    pub clipped: bool,
    pub dfn_error: bool,
}

/// Zero-allocation 2nd-order Butterworth High-Pass Filter (~65 Hz @ 48 kHz).
/// Uses `f64` coefficients and Direct Form II Transposed state for precision at low fc/fs ratios.
/// Removes desk thumps, boom-arm vibrations, air-conditioner rumble, plosives, and DC offset
/// before AI inference while keeping low vocal fundamentals (85-120 Hz) intact.
struct HighPassFilter {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
    enabled: bool,
    mix: f32,
}

impl HighPassFilter {
    fn new(sample_rate_hz: f64, cutoff_hz: f64, enabled: bool) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * cutoff_hz / sample_rate_hz;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let q = std::f64::consts::FRAC_1_SQRT_2; // Butterworth Q = 1 / sqrt(2)
        let alpha = sin_w0 / (2.0 * q);
        let a0 = 1.0 + alpha;

        let b0 = ((1.0 + cos_w0) * 0.5) / a0;
        let b1 = -(1.0 + cos_w0) / a0;
        let b2 = ((1.0 + cos_w0) * 0.5) / a0;
        let a1 = (-2.0 * cos_w0) / a0;
        let a2 = (1.0 - alpha) / a0;

        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
            enabled,
            mix: if enabled { 1.0 } else { 0.0 },
        }
    }

    #[inline]
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Processes one sample through the biquad state continuously so toggling HPF never clicks.
    #[inline]
    fn tick_filter(&mut self, sample: f32) -> f32 {
        let x = if sample.is_finite() {
            sample as f64
        } else {
            0.0
        };
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        // Flush denormals to zero to avoid x86 CPU stalls during extended silence
        if self.z1.abs() < 1e-24 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-24 {
            self.z2 = 0.0;
        }
        y as f32
    }

    /// Filters a 480-sample frame into `out_480` with clickless 10ms dry/wet ramping on toggle.
    fn process_frame(&mut self, in_480: &[f32], out_480: &mut [f32]) {
        let target_mix = if self.enabled { 1.0_f32 } else { 0.0_f32 };
        let transitioning = (self.mix - target_mix).abs() > 1e-4;

        if !transitioning {
            self.mix = target_mix;
            if self.enabled {
                for i in 0..DFN_HOP_SIZE {
                    out_480[i] = self.tick_filter(in_480[i]);
                }
            } else {
                for &x in &in_480[..DFN_HOP_SIZE] {
                    let _ = self.tick_filter(x);
                }
                out_480[..DFN_HOP_SIZE].copy_from_slice(&in_480[..DFN_HOP_SIZE]);
            }
        } else {
            let step = (target_mix - self.mix) / (DFN_HOP_SIZE as f32);
            let mut m = self.mix;
            for i in 0..DFN_HOP_SIZE {
                m = (m + step).clamp(0.0, 1.0);
                let dry = in_480[i];
                let wet = self.tick_filter(dry);
                out_480[i] = dry * (1.0 - m) + wet * m;
            }
            self.mix = target_mix;
        }
    }
}

/// Hysteresis + Hold + Sample-Smoothed Voice Activity Detection (VAD) Noise Gate.
/// Uses DeepFilterNet3's neural local SNR (`lsnr`) and pre-AGC post-DFN frame energy
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
            is_open: !enabled,
            hold_frames_left: 0,
            envelope_gain: if enabled { 0.0 } else { 1.0 },
        }
    }

    #[inline]
    fn set_params(&mut self, enabled: bool, open_thresh_db: f32) {
        self.enabled = enabled;
        self.open_thresh_db = open_thresh_db.clamp(-18.0, 12.0);
    }

    /// Evaluates speech presence on pre-AGC post-DFN audio.
    /// Returns `(speech_active_now, gate_open_or_speaking)`:
    /// - `speech_active_now`: strict instantaneous voice presence (used to gate AGC updates so
    ///   AGC never pumps background noise during silence or hangover tails).
    /// - `gate_open_or_speaking`: voice activity status including hysteresis + 180ms hangover hold.
    fn evaluate(&mut self, lsnr_db: f32, pre_agc_rms_db: f32) -> (bool, bool) {
        if !self.enabled {
            self.is_open = true;
            let speech_now = lsnr_db >= self.open_thresh_db && pre_agc_rms_db >= -62.0;
            return (speech_now, speech_now);
        }

        let speech_active_now = if self.is_open {
            // 3.5 dB hysteresis keeps trailing consonants & quiet sentence endings intact
            lsnr_db >= (self.open_thresh_db - 3.5) && pre_agc_rms_db >= -78.0
        } else {
            (lsnr_db >= self.open_thresh_db && pre_agc_rms_db >= -72.0)
                || (lsnr_db >= self.open_thresh_db + 4.5 && pre_agc_rms_db >= -76.0)
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

        (speech_active_now, self.is_open)
    }

    /// Applies per-sample smooth gate envelope gain to `enh_480`.
    fn apply_envelope(&mut self, enh_480: &mut [f32], atten_lim_db: f32) {
        if !self.enabled {
            if (self.envelope_gain - 1.0).abs() < 1e-4 {
                self.envelope_gain = 1.0;
                return;
            }
            for s in enh_480.iter_mut() {
                self.envelope_gain += 0.02 * (1.0 - self.envelope_gain);
                *s *= self.envelope_gain;
            }
            return;
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
            if target_gain == 0.0 && self.envelope_gain < 1e-4 {
                // Snap state directly to 0.0 to avoid x86_64 denormal float stalls in sustained silence
                self.envelope_gain = 0.0;
            } else if (self.envelope_gain - target_gain).abs() < 1e-5 {
                self.envelope_gain = target_gain;
            }
            *s *= self.envelope_gain;
        }
    }
}

/// Lightweight VAD-Gated Automatic Gain Control (AGC).
/// Tracks speech RMS exclusively when `speech_active_now == true` and voice level exceeds the
/// speech floor, holding gain flat during silence/pauses to prevent background noise pumping.
struct VadAgc {
    enabled: bool,
    current_gain_db: f32,
    smoothed_gain_linear: f32,
}

impl VadAgc {
    const TARGET_RMS_DB: f32 = -18.0; // Standard Discord / conferencing voice RMS target
    const MIN_GAIN_DB: f32 = -12.0;
    const MAX_GAIN_DB: f32 = 18.0;
    const MIN_SPEECH_RMS_DB: f32 = -62.0; // Allow boosting quiet dynamic/distant mics while ignoring digital silence
    const ALPHA_DOWN: f32 = 0.08; // ~120ms attack when attenuating loud speech
    const ALPHA_UP: f32 = 0.0125; // ~800ms release when boosting quiet speech
    const ALPHA_BYPASS: f32 = 0.12; // ~80ms smooth return to 0 dB when AGC disabled

    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            current_gain_db: 0.0,
            smoothed_gain_linear: 1.0,
        }
    }

    #[inline]
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Updates gain only on confirmed active speech frames and applies sample-interpolated gain
    /// to `enh_480`. Returns current applied AGC gain in dB.
    fn process(
        &mut self,
        speech_active_now: bool,
        pre_agc_rms_db: f32,
        enh_480: &mut [f32],
    ) -> f32 {
        if !self.enabled {
            self.current_gain_db += Self::ALPHA_BYPASS * (0.0 - self.current_gain_db);
            if self.current_gain_db.abs() < 0.01 {
                self.current_gain_db = 0.0;
            }
        } else if speech_active_now && pre_agc_rms_db >= Self::MIN_SPEECH_RMS_DB {
            let desired_db = (Self::TARGET_RMS_DB - pre_agc_rms_db)
                .clamp(Self::MIN_GAIN_DB, Self::MAX_GAIN_DB);
            let alpha = if desired_db < self.current_gain_db {
                Self::ALPHA_DOWN
            } else {
                Self::ALPHA_UP
            };
            self.current_gain_db += alpha * (desired_db - self.current_gain_db);
        }

        let target_linear = 10.0_f32.powf(self.current_gain_db / 20.0);
        if (target_linear - 1.0).abs() < 1e-4 && (self.smoothed_gain_linear - 1.0).abs() < 1e-4 {
            self.smoothed_gain_linear = 1.0;
            return 0.0;
        }

        let step = (target_linear - self.smoothed_gain_linear) / (DFN_HOP_SIZE as f32);
        let mut g = self.smoothed_gain_linear;
        for s in enh_480.iter_mut() {
            g += step;
            *s *= g;
        }
        self.smoothed_gain_linear = target_linear;

        self.current_gain_db
    }
}

/// Zero-allocation, lookahead-aligned internal operating-level normalizer for DeepFilterNet3.
///
/// DeepFilterNet3's feature extraction (`feat_erb` floor at `1e-10`, `band_unit_norm` scaling
/// as `sqrt(|X|)`, and `rms < 1e-7` silence skip) has a sensitivity dead zone on quiet or
/// distant microphones (~ -40 to -58 dBFS, common on dynamic mics like Shure MV7 on macOS),
/// where genuine human speech collapses to `lsnr = -15 dB` and gets heavily attenuated even
/// with the VAD gate turned off.
///
/// This wrapper smoothly drives quiet inputs into DeepFilterNet3's optimal operating window
/// (~ -12 dBFS peak / -24 dBFS RMS) with a strict anti-clipping ceiling, then divides the
/// denoised output by the exact lookahead-aligned gain so end-to-end gain remains strictly
/// unity (`0.0 dB`).
struct PreDfnNormalizer {
    env_peak: f32,
    cur_gain: f32,
    /// Ring buffer storing `(start_gain, end_gain)` applied to each input frame so the
    /// delayed output frame is restored with exact lookahead alignment.
    history: [(f32, f32); Self::HISTORY_CAP],
    write_pos: usize,
    lookahead_frames: usize,
}

impl PreDfnNormalizer {
    const HISTORY_CAP: usize = 8; // Must exceed max lookahead (Standard = 2, LL = 0)
    const TARGET_PEAK: f32 = 0.55; // ~ -5.2 dBFS peak (~ -18..-22 dBFS speech RMS, DFN3 sweet spot)
    const SAFE_CEILING: f32 = 0.85; // Hard anti-clipping ceiling inside DeepFilterNet3
    const MAX_STEADY_RMS: f32 = 0.14; // ~ -17 dBFS RMS cap so steady fan noise isn't over-driven
    const MIN_DRIVE: f32 = 1.0; // 0 dB (never attenuate normal/loud microphones)
    const MAX_DRIVE: f32 = 32.0; // +30 dB max internal drive (lifts -55 dBFS -> -25 dBFS cleanly)
    const SILENCE_PEAK: f32 = 1e-5; // -100 dBFS digital silence floor

    fn new(lookahead_frames: usize) -> Self {
        Self {
            env_peak: 0.02, // Gently prepared for quiet/distant mics; loud speech clamps immediately
            cur_gain: Self::MIN_DRIVE,
            history: [(Self::MIN_DRIVE, Self::MIN_DRIVE); Self::HISTORY_CAP],
            write_pos: 0,
            lookahead_frames: lookahead_frames.min(Self::HISTORY_CAP - 1),
        }
    }

    /// Drives `in_480` smoothly into DeepFilterNet3's sweet spot and returns the lookahead-aligned
    /// `(restore_start_gain, restore_end_gain)` pair to divide `out_480` by after inference.
    fn drive_input(&mut self, in_480: &mut [f32]) -> (f32, f32) {
        let mut frame_peak = 0.0f32;
        let mut sum_sq = 0.0f32;
        for &s in in_480.iter() {
            let a = s.abs();
            if a > frame_peak {
                frame_peak = a;
            }
            sum_sq += s * s;
        }
        let frame_rms = (sum_sq / DFN_HOP_SIZE as f32).sqrt();

        // Fast attack on louder speech + very slow release (~15s time constant) so drive stays
        // rock-steady across pauses between words/sentences and NEVER compresses speech-to-noise
        // contrast when speaking after a quiet pause.
        if frame_peak > self.env_peak {
            self.env_peak = 0.50 * self.env_peak + 0.50 * frame_peak;
        } else {
            self.env_peak = 0.9993 * self.env_peak + 0.0007 * frame_peak;
            if self.env_peak < 1e-9 {
                self.env_peak = 0.0;
            }
        }

        let desired_from_peak = if self.env_peak > Self::SILENCE_PEAK {
            (Self::TARGET_PEAK / self.env_peak).clamp(Self::MIN_DRIVE, Self::MAX_DRIVE)
        } else {
            Self::MIN_DRIVE
        };

        // Never drive steady broadband noise above MAX_STEADY_RMS (~ -18.4 dBFS)
        let max_from_rms = if frame_rms > Self::SILENCE_PEAK {
            (Self::MAX_STEADY_RMS / frame_rms).clamp(Self::MIN_DRIVE, Self::MAX_DRIVE)
        } else {
            Self::MAX_DRIVE
        };

        // Instantaneous peak ceiling guarantees `in_480 * gain` never clips on sudden shouts
        let max_safe = if frame_peak > Self::SILENCE_PEAK {
            (Self::SAFE_CEILING / frame_peak).clamp(Self::MIN_DRIVE, Self::MAX_DRIVE)
        } else {
            Self::MAX_DRIVE
        };

        let desired = desired_from_peak.min(max_from_rms);
        let prev_gain = self.cur_gain;
        // Climb briskly toward optimal operating level (~400ms), drop smoothly when louder speech arrives
        let alpha = if desired < prev_gain { 0.08 } else { 0.025 };
        let end_gain = (prev_gain + alpha * (desired - prev_gain))
            .clamp(Self::MIN_DRIVE, Self::MAX_DRIVE)
            .min(max_safe);
        // If a sudden loud transient exceeds SAFE_CEILING at prev_gain, clamp start_gain too so sample 0 never clips
        let start_gain = prev_gain.min(max_safe);

        if (start_gain - 1.0).abs() > 1e-4 || (end_gain - 1.0).abs() > 1e-4 {
            let step = (end_gain - start_gain) / (DFN_HOP_SIZE as f32);
            let mut g = start_gain;
            for s in in_480.iter_mut() {
                g += step;
                *s = soft_limit(*s * g);
            }
        }
        self.cur_gain = end_gain;

        // Record current frame's applied gain and read the lookahead-aligned gain for output restore
        self.history[self.write_pos] = (start_gain, end_gain);
        let read_pos =
            (self.write_pos + Self::HISTORY_CAP - self.lookahead_frames) % Self::HISTORY_CAP;
        self.write_pos = (self.write_pos + 1) % Self::HISTORY_CAP;
        self.history[read_pos]
    }

    /// Restores `out_480` back to exact unity gain using the lookahead-aligned `(start_g, end_g)`.
    #[inline]
    fn restore_output(out_480: &mut [f32], restore_gains: (f32, f32)) {
        let (start_g, end_g) = restore_gains;
        if (start_g - 1.0).abs() <= 1e-4 && (end_g - 1.0).abs() <= 1e-4 {
            return;
        }
        let inv_start = 1.0 / start_g.max(1.0);
        let inv_end = 1.0 / end_g.max(1.0);
        let step = (inv_end - inv_start) / (DFN_HOP_SIZE as f32);
        let mut inv = inv_start;
        for s in out_480.iter_mut() {
            inv += step;
            *s *= inv;
        }
    }
}

pub struct NoiseProcessor {
    model: DfTract,
    variant: ModelVariant,
    in_frame: Array2<f32>,
    out_frame: Array2<f32>,
    hpf_scratch: [f32; DFN_HOP_SIZE],
    hpf: HighPassFilter,
    aec: AecProcessor,
    pre_norm: PreDfnNormalizer,
    agc: VadAgc,
    nc_enabled: bool,
    wet_mix: f32,
    mic_muted: bool,
    mute_gain: f32,
    input_gain_target: f32,
    input_gain_smoothed: f32,
    atten_lim_db: f32,
    pf_beta: f32,
    vad_gate: VadGate,
    voice_gate_engaged: bool,
    speaker_allowed: bool,
    speaker_gate_gain: f32,
}

impl NoiseProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        variant: ModelVariant,
        atten_lim_db: f32,
        pf_beta: f32,
        min_db_thresh: f32,
        vad_enabled: bool,
        vad_thresh_db: f32,
        hpf_enabled: bool,
        agc_enabled: bool,
        aec_enabled: bool,
        nc_enabled: bool,
        mic_muted: bool,
        input_gain_db: f32,
    ) -> Result<Self> {
        let df_params = match variant {
            ModelVariant::Standard => DfParams::from_bytes(DFN3_STANDARD_BYTES)?,
            ModelVariant::LowLatency => DfParams::default(),
        };

        let atten_lim_db = atten_lim_db.clamp(6.0, 100.0);
        let pf_beta = pf_beta.clamp(0.0, 0.06);
        let min_db_thresh = min_db_thresh.clamp(-20.0, 20.0);
        let vad_thresh_db = vad_thresh_db.clamp(-18.0, 12.0);
        let input_gain_linear = 10.0_f32.powf(input_gain_db.clamp(-24.0, 24.0) / 20.0);

        let mut rp = RuntimeParams::default_with_ch(1);
        rp.atten_lim_db = atten_lim_db;
        rp.post_filter_beta = pf_beta;
        rp.min_db_thresh = min_db_thresh;
        // Disable abrupt mid-speech stage skipping (default 20 dB DF / 30 dB ERB cutoffs cause
        // sudden volume jumps and phase glitches on high-SNR mics whose lsnr reaches 20..35 dB).
        rp.max_db_erb_thresh = 40.0;
        rp.max_db_df_thresh = 40.0;

        let mut model = DfTract::new(df_params, &rp)?;
        model.set_atten_lim(atten_lim_db);
        model.set_pf_beta(pf_beta);

        let hop = model.hop_size;
        assert_eq!(
            hop, DFN_HOP_SIZE,
            "Expected DeepFilterNet3 hop_size of 480 samples at 48kHz"
        );

        let lookahead = model.lookahead;
        Ok(Self {
            model,
            variant,
            in_frame: Array2::<f32>::zeros((1, hop)),
            out_frame: Array2::<f32>::zeros((1, hop)),
            hpf_scratch: [0.0f32; DFN_HOP_SIZE],
            hpf: HighPassFilter::new(DFN_SAMPLE_RATE as f64, HPF_CUTOFF_HZ, hpf_enabled),
            aec: AecProcessor::new(aec_enabled),
            pre_norm: PreDfnNormalizer::new(lookahead),
            agc: VadAgc::new(agc_enabled),
            nc_enabled,
            wet_mix: if nc_enabled { 1.0 } else { 0.0 },
            mic_muted,
            mute_gain: if mic_muted { 0.0 } else { 1.0 },
            input_gain_target: input_gain_linear,
            input_gain_smoothed: input_gain_linear,
            atten_lim_db,
            pf_beta,
            vad_gate: VadGate::new(vad_enabled, vad_thresh_db),
            voice_gate_engaged: false,
            speaker_allowed: true,
            speaker_gate_gain: 1.0,
        })
    }

    pub fn variant(&self) -> ModelVariant {
        self.variant
    }

    /// Algorithmic latency in ms: 10ms hop + lookahead frames (30ms for Standard, 10ms for LowLatency)
    /// + 8ms causal margin only when AEC is actively engaged.
    pub fn algorithmic_latency_ms(&self) -> f32 {
        (1 + self.model.lookahead) as f32 * 10.0 + self.aec.active_latency_ms()
    }

    #[inline]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.nc_enabled = enabled;
    }

    #[inline]
    pub fn set_mic_muted(&mut self, muted: bool) {
        self.mic_muted = muted;
    }

    #[inline]
    pub fn set_hpf_enabled(&mut self, enabled: bool) {
        self.hpf.set_enabled(enabled);
    }

    #[inline]
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        self.agc.set_enabled(enabled);
    }

    #[inline]
    pub fn set_aec_enabled(&mut self, enabled: bool) {
        self.aec.set_enabled(enabled);
    }

    #[inline]
    pub fn set_aec_monitor_bypass(&mut self, bypass: bool) {
        self.aec.set_monitor_bypass(bypass);
    }

    #[inline]
    pub fn set_input_gain_db(&mut self, gain_db: f32) {
        let clamped = gain_db.clamp(-24.0, 24.0);
        self.input_gain_target = 10.0_f32.powf(clamped / 20.0);
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

    #[inline]
    pub fn set_speaker_gate(&mut self, engaged: bool, speaker_allowed: bool) {
        self.voice_gate_engaged = engaged;
        self.speaker_allowed = speaker_allowed;
    }

    /// Processes exactly 480 samples (10ms @ 48kHz) with zero heap allocations.
    /// - `aec_ref_480`: optional 48 kHz mono speaker loopback reference frame for WebRTC AEC3
    /// - `pre_gate_enh_out_480`: optional tap of unity-gain post-AEC/post-DFN3 speech before gates/AGC
    /// - `raw_out_480`: gain-adjusted raw signal (for A/B live monitor & NC bypass)
    /// - `enh_out_480`: HPF -> AEC3 -> PreDfnNormalizer -> DFN3 -> VAD/Speaker-gated AGC & envelope -> soft-limited signal
    /// - `final_out_480`: active output (denoised or bypassed) with clickless mic-mute fade
    pub fn process_frame_48k(
        &mut self,
        input_480: &[f32],
        aec_ref_480: Option<&[f32]>,
        pre_gate_enh_out_480: Option<&mut [f32; DFN_HOP_SIZE]>,
        raw_out_480: &mut [f32],
        enh_out_480: &mut [f32],
        final_out_480: &mut [f32],
    ) -> FrameProcessResult {
        debug_assert_eq!(input_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(raw_out_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(enh_out_480.len(), DFN_HOP_SIZE);
        debug_assert_eq!(final_out_480.len(), DFN_HOP_SIZE);

        let mut clipped = false;

        // 1. Pre-AI 65Hz Butterworth High-Pass Filter (state continuously ticked for clickless toggle)
        self.hpf.process_frame(input_480, &mut self.hpf_scratch);

        // 2. Apply per-sample smoothed manual input gain & detect pre-limiter clipping
        {
            let target_gain = self.input_gain_target;
            let start_gain = self.input_gain_smoothed;
            let gain_step = (target_gain - start_gain) / (DFN_HOP_SIZE as f32);
            let ramp_gain = (target_gain - start_gain).abs() > 1e-4;
            let mut g = start_gain;

            let mut in_slice = self.in_frame.row_mut(0);
            for i in 0..DFN_HOP_SIZE {
                if ramp_gain {
                    g += gain_step;
                } else {
                    g = target_gain;
                }
                let dry_in = if input_480[i].is_finite() {
                    input_480[i]
                } else {
                    0.0
                };
                let raw_scaled = dry_in * g;
                let hpf_scaled = self.hpf_scratch[i] * g;
                if !self.mic_muted {
                    let active_in = if self.nc_enabled {
                        hpf_scaled
                    } else {
                        raw_scaled
                    };
                    if active_in.abs() >= CLIP_DETECT_THRESH {
                        clipped = true;
                    }
                }
                raw_out_480[i] = soft_limit(raw_scaled);
                in_slice[i] = soft_limit(hpf_scaled);
            }
            self.input_gain_smoothed = target_gain;
        }

        // 2b. Pre-DFN3 WebRTC AEC3 (removes far-end speaker bleed while linear acoustic phase
        //     coherence is still intact, preventing DFN3 / VAD from opening on speaker voices).
        let aec_metrics = {
            let mut in_slice = self.in_frame.row_mut(0);
            let slice = in_slice
                .as_slice_mut()
                .expect("contiguous 1x480 DeepFilterNet3 input frame");
            self.aec.process_frame_48k(aec_ref_480, slice)
        };

        // 3. Transparent unity-gain operating-level normalization + DeepFilterNet3 neural inference
        //    Prevents DFN3's -40..-58 dBFS low-level dead zone on quiet/distant mics (e.g. dynamic mics on macOS).
        let restore_gains = {
            let mut in_slice = self.in_frame.row_mut(0);
            let slice = in_slice
                .as_slice_mut()
                .expect("contiguous 1x480 DeepFilterNet3 input frame");
            self.pre_norm.drive_input(slice)
        };

        let (lsnr, dfn_error) = match self
            .model
            .process(self.in_frame.view(), self.out_frame.view_mut())
        {
            Ok(v) if v.is_finite() => (v, false),
            Ok(_) => (-15.0, false),
            Err(_) => {
                // Clear stale frame so repeated inference failures never buzz old audio
                self.out_frame.fill(0.0);
                (-15.0, true)
            }
        };

        {
            let enh_slice = self.out_frame.row(0);
            for i in 0..DFN_HOP_SIZE {
                let s = enh_slice[i];
                enh_out_480[i] = if s.is_finite() { s } else { 0.0 };
            }
        }
        PreDfnNormalizer::restore_output(enh_out_480, restore_gains);
        if let Some(pre_gate_buf) = pre_gate_enh_out_480 {
            pre_gate_buf.copy_from_slice(&enh_out_480[..DFN_HOP_SIZE]);
        }

        let mut sum_sq = 0.0f32;
        for &s in enh_out_480.iter() {
            sum_sq += s * s;
        }
        let pre_agc_rms = (sum_sq / DFN_HOP_SIZE as f32).sqrt();
        let pre_agc_rms_db = if pre_agc_rms < 1e-6 {
            -120.0
        } else {
            20.0 * pre_agc_rms.log10()
        };

        // 4. Evaluate VAD BEFORE AGC so AGC gain never causes false gate openings
        let (speech_active_now, vad_open) = self.vad_gate.evaluate(lsnr, pre_agc_rms_db);
        let spk_open = !self.voice_gate_engaged || self.speaker_allowed;

        // 5. Apply VAD + Speaker-gated AGC (updates gain only during confirmed user speech)
        let agc_gain_db = self
            .agc
            .process(speech_active_now && spk_open, pre_agc_rms_db, enh_out_480);

        // 6. Apply VAD noise gate envelope + personalized speaker verification gate envelope
        self.vad_gate.apply_envelope(enh_out_480, self.atten_lim_db);

        let target_spk_gain = if spk_open { 1.0_f32 } else { 0.0_f32 };
        if (self.speaker_gate_gain - target_spk_gain).abs() < 1e-4 {
            self.speaker_gate_gain = target_spk_gain;
            if target_spk_gain == 0.0 {
                enh_out_480.fill(0.0);
            }
        } else {
            // Smooth ~30ms soft mute when rejecting non-enrolled voice; ~5ms brisk open
            let alpha = if target_spk_gain > self.speaker_gate_gain {
                0.012_f32
            } else {
                0.0024_f32
            };
            for s in enh_out_480.iter_mut() {
                self.speaker_gate_gain += alpha * (target_spk_gain - self.speaker_gate_gain);
                if target_spk_gain == 0.0 && self.speaker_gate_gain < 1e-4 {
                    self.speaker_gate_gain = 0.0;
                }
                *s *= self.speaker_gate_gain;
            }
        }
        let voice_gate_blocked = vad_open && !spk_open;
        let vad_speaking = vad_open && spk_open;

        // 7. Post-AGC/VAD soft limiter & output clipping detection
        for s in enh_out_480.iter_mut() {
            if self.nc_enabled && !self.mic_muted && s.abs() >= CLIP_DETECT_THRESH {
                clipped = true;
            }
            *s = soft_limit(*s);
        }

        // 8. Smooth NC ON/OFF crossfade
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

        // 9. Clickless 10ms microphone mute fade on final outgoing signal
        let target_mute_gain = if self.mic_muted { 0.0_f32 } else { 1.0_f32 };
        if (self.mute_gain - target_mute_gain).abs() < 1e-4 {
            self.mute_gain = target_mute_gain;
            if self.mic_muted {
                final_out_480.fill(0.0);
            }
        } else {
            let step = (target_mute_gain - self.mute_gain) / (DFN_HOP_SIZE as f32);
            let mut g = self.mute_gain;
            for s in final_out_480.iter_mut() {
                g = (g + step).clamp(0.0, 1.0);
                *s *= g;
            }
            self.mute_gain = target_mute_gain;
        }

        FrameProcessResult {
            lsnr_db: lsnr,
            speech_active_now,
            vad_speaking,
            voice_gate_blocked,
            agc_gain_db,
            aec_active: aec_metrics.active,
            aec_erle_db: aec_metrics.erle_db,
            clipped,
            dfn_error,
        }
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

    fn measure_sine_gain_db(freq_hz: f64) -> f64 {
        let mut hpf = HighPassFilter::new(DFN_SAMPLE_RATE as f64, HPF_CUTOFF_HZ, true);
        let mut in_buf = [0.0f32; DFN_HOP_SIZE];
        let mut out_buf = [0.0f32; DFN_HOP_SIZE];
        let mut in_sq = 0.0f64;
        let mut out_sq = 0.0f64;

        // Run 60 frames (600ms) so low-frequency filter reaches steady state; measure last 20 frames
        for frame in 0..60 {
            for (i, s) in in_buf.iter_mut().enumerate() {
                let n = (frame * DFN_HOP_SIZE + i) as f64;
                *s = (2.0 * std::f64::consts::PI * freq_hz * n / DFN_SAMPLE_RATE as f64)
                    .sin() as f32
                    * 0.5;
            }
            hpf.process_frame(&in_buf, &mut out_buf);
            if frame >= 40 {
                for i in 0..DFN_HOP_SIZE {
                    in_sq += (in_buf[i] as f64).powi(2);
                    out_sq += (out_buf[i] as f64).powi(2);
                }
            }
        }
        10.0 * (out_sq / in_sq.max(1e-18)).log10()
    }

    #[test]
    fn test_hpf_65hz_attenuates_rumble_and_preserves_voice() {
        let gain_20hz = measure_sine_gain_db(20.0);
        let gain_65hz = measure_sine_gain_db(65.0);
        let gain_120hz = measure_sine_gain_db(120.0);
        let gain_300hz = measure_sine_gain_db(300.0);
        let gain_1khz = measure_sine_gain_db(1000.0);

        assert!(
            gain_20hz < -19.0,
            "Expected >19 dB attenuation at 20 Hz rumble, got {:.2} dB",
            gain_20hz
        );
        assert!(
            (gain_65hz - (-3.01)).abs() < 0.25,
            "Expected -3.01 dB Butterworth cutoff at 65 Hz, got {:.2} dB",
            gain_65hz
        );
        assert!(
            gain_120hz > -0.5,
            "Expected low vocal fundamentals at 120 Hz preserved (> -0.5 dB), got {:.2} dB",
            gain_120hz
        );
        assert!(
            gain_300hz > -0.1,
            "Expected flat passband at 300 Hz, got {:.2} dB",
            gain_300hz
        );
        assert!(
            gain_1khz > -0.05,
            "Expected flat passband at 1 kHz, got {:.2} dB",
            gain_1khz
        );
    }

    #[test]
    fn test_vad_agc_tracks_speech_and_holds_during_silence() {
        let mut agc = VadAgc::new(true);
        let mut frame = [0.0f32; DFN_HOP_SIZE];

        // 1. Non-speech low noise (-40 dBFS) for 200 frames (2s): AGC must NOT boost!
        for _ in 0..200 {
            frame.fill(0.01);
            let gain_db = agc.process(false, -40.0, &mut frame);
            assert!(
                gain_db.abs() < 0.01,
                "AGC must not pump gain during silence/non-speech, got {:.2} dB",
                gain_db
            );
        }

        // 2. Quiet active speech (-32 dBFS) for 300 frames (3s): AGC should boost toward -18 dBFS (+14 dB)
        let mut last_gain_db = 0.0f32;
        for _ in 0..300 {
            frame.fill(0.025);
            last_gain_db = agc.process(true, -32.0, &mut frame);
        }
        assert!(
            last_gain_db > 10.0 && last_gain_db <= 14.5,
            "Expected AGC to boost quiet speech toward +14 dB, got {:.2} dB",
            last_gain_db
        );

        // 3. Silence returns: AGC must hold steady without pumping further
        let held_gain = last_gain_db;
        for _ in 0..100 {
            frame.fill(0.001);
            let g = agc.process(false, -60.0, &mut frame);
            assert!(
                (g - held_gain).abs() < 1e-4,
                "AGC should hold gain during silence (expected {:.2}, got {:.2})",
                held_gain,
                g
            );
        }

        // 4. Loud speech (-6 dBFS) for 80 frames (800ms): AGC should attenuate quickly
        for _ in 0..80 {
            frame.fill(0.5);
            last_gain_db = agc.process(true, -6.0, &mut frame);
        }
        assert!(
            last_gain_db < -8.0,
            "Expected AGC to attenuate loud speech quickly, got {:.2} dB",
            last_gain_db
        );
    }

    #[test]
    fn test_standard_dfn3_and_vad_gate_silence_noise() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::Standard,
            80.0,
            0.02,
            -10.0,
            true,
            -5.0,
            true,
            false,
            false,
            true,
            false,
            0.0,
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
            let _res = proc.process_frame_48k(
                &noisy,
                None,
                None,
                &mut raw_out,
                &mut enh_out,
                &mut clean_out,
            );

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
    fn test_low_latency_dfn3_ll_clipping_and_mute() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::LowLatency,
            65.0,
            0.02,
            -10.0,
            false,
            -5.0,
            true,
            false,
            false,
            true,
            false,
            0.0,
        )
        .expect("Failed to load LowLatency DeepFilterNet3_ll");

        let mut raw_out = [0.0f32; DFN_HOP_SIZE];
        let mut enh_out = [0.0f32; DFN_HOP_SIZE];
        let mut final_out = [0.0f32; DFN_HOP_SIZE];

        // Overdriven 440Hz sine (+12 dB gain on 0.5 amplitude -> peak ~2.0 > 0.92 threshold)
        proc.set_input_gain_db(12.0);
        let mut hot = [0.0f32; DFN_HOP_SIZE];
        for (i, s) in hot.iter_mut().enumerate() {
            let t = i as f32 / DFN_SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
        }
        let res_hot = proc.process_frame_48k(
            &hot,
            None,
            None,
            &mut raw_out,
            &mut enh_out,
            &mut final_out,
        );
        assert!(res_hot.clipped, "Expected clipping flag on overdriven input");
        for &s in &final_out {
            assert!(s.abs() <= 1.0, "Soft limiter must keep samples within [-1, 1]");
        }

        // Enable mute: frame 1 fades down, frame 2 is completely silent
        proc.set_mic_muted(true);
        let _ = proc.process_frame_48k(
            &hot,
            None,
            None,
            &mut raw_out,
            &mut enh_out,
            &mut final_out,
        );
        let res_muted = proc.process_frame_48k(
            &hot,
            None,
            None,
            &mut raw_out,
            &mut enh_out,
            &mut final_out,
        );
        assert!(!res_muted.clipped, "Muted state should not emit clipping alerts");
        let max_abs = final_out.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
        assert_eq!(max_abs, 0.0, "Muted output must be completely silent");
    }

    #[test]
    fn test_muted_startup_and_nan_inf_safety() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::LowLatency,
            65.0,
            0.0,
            -16.0,
            true,
            -5.0,
            true,
            false,
            false,
            true,
            true, // Start muted!
            6.0,
        )
        .expect("Failed to load LowLatency DeepFilterNet3_ll");

        let mut raw_out = [0.0f32; DFN_HOP_SIZE];
        let mut enh_out = [0.0f32; DFN_HOP_SIZE];
        let mut final_out = [0.0f32; DFN_HOP_SIZE];

        // Frame 1 with loud sine while muted must leak ZERO samples
        let mut loud = [0.0f32; DFN_HOP_SIZE];
        for (i, s) in loud.iter_mut().enumerate() {
            let t = i as f32 / DFN_SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.6;
        }
        let res1 = proc.process_frame_48k(
            &loud,
            None,
            None,
            &mut raw_out,
            &mut enh_out,
            &mut final_out,
        );
        assert!(!res1.clipped);
        let max_abs = final_out.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
        assert_eq!(
            max_abs, 0.0,
            "Muted processor startup must not leak any audio on frame 1"
        );

        // Unmute and inject NaN / +-Inf samples: output must remain finite and bounded
        proc.set_mic_muted(false);
        let mut bad = [0.0f32; DFN_HOP_SIZE];
        bad[0] = f32::NAN;
        bad[1] = f32::INFINITY;
        bad[2] = f32::NEG_INFINITY;
        let _ = proc.process_frame_48k(
            &bad,
            None,
            None,
            &mut raw_out,
            &mut enh_out,
            &mut final_out,
        );
        for &s in &final_out {
            assert!(s.is_finite() && s.abs() <= 1.0);
        }
    }

    #[test]
    fn test_vad_gate_flushes_denormals_and_gain_smooths() {
        let mut gate = VadGate::new(true, -5.0);
        gate.envelope_gain = 1.0;
        gate.is_open = false;
        let mut buf = [0.5f32; DFN_HOP_SIZE];
        for _ in 0..800 {
            gate.apply_envelope(&mut buf, 65.0);
        }
        assert_eq!(
            gate.envelope_gain, 0.0,
            "VadGate envelope must snap cleanly to 0.0 instead of decaying into denormals"
        );
    }

    #[test]
    fn test_quiet_distant_speech_preserved_without_dead_zone() {
        // Verifies PreDfnNormalizer eliminates DeepFilterNet3's -40..-55 dBFS dead zone
        // (e.g. dynamic microphones like Shure MV7 on macOS) while keeping pure quiet room
        // noise suppressed and preserving unity gain without clipping on sudden loud transients.
        let mut proc = NoiseProcessor::new(
            ModelVariant::Standard,
            24.0,
            0.0,
            -16.0,
            false,
            -5.0,
            true,
            false,
            false,
            true,
            false,
            0.0,
        )
        .expect("Failed to load Standard DeepFilterNet3");

        let speech_rms_db = -45.0f32;
        let speech_amp = 10.0f32.powf((speech_rms_db + 12.2) / 20.0);
        let noise_amp = 10.0f32.powf((speech_rms_db - 20.0 + 4.8) / 20.0);
        let mut seed = 0x1234_5678u32;
        let mut raw = [0.0f32; DFN_HOP_SIZE];
        let mut enh = [0.0f32; DFN_HOP_SIZE];
        let mut out = [0.0f32; DFN_HOP_SIZE];

        // 1. Warm up on 2s of quiet room noise (-65 dBFS)
        for _ in 0..200 {
            let mut nbuf = [0.0f32; DFN_HOP_SIZE];
            for s in nbuf.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let rnd = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
                *s = rnd * noise_amp;
            }
            let _ = proc.process_frame_48k(&nbuf, None, None, &mut raw, &mut enh, &mut out);
        }

        // 2. Feed 2.5s of quiet -45 dBFS speech bursts (without PreDfnNormalizer, DFN3 drops by -24 dB)
        let mut in_sq = 0.0f64;
        let mut out_sq = 0.0f64;
        let mut cnt = 0usize;
        for f in 0..250 {
            let mut buf = [0.0f32; DFN_HOP_SIZE];
            let t_frame = f as f32 * 0.01;
            let cyc = t_frame % 0.9;
            let env = if cyc < 0.6 {
                (std::f32::consts::PI * cyc / 0.6).sin().powf(0.4)
            } else {
                0.0
            };
            let f0 = 135.0 + 25.0 * (t_frame * 2.1).sin();
            for (i, s) in buf.iter_mut().enumerate() {
                let t = (f * DFN_HOP_SIZE + i) as f32 / DFN_SAMPLE_RATE as f32;
                let mut voice = 0.0f32;
                for h in 1..=20 {
                    let freq = f0 * h as f32;
                    let f1 = (-((freq - 520.0) / 180.0).powi(2)).exp();
                    let f2 = 0.7 * (-((freq - 1550.0) / 280.0).powi(2)).exp();
                    let f3 = 0.35 * (-((freq - 2600.0) / 400.0).powi(2)).exp();
                    let w = (0.25 / h as f32) + f1 + f2 + f3;
                    voice += w * (2.0 * std::f32::consts::PI * freq * t).sin();
                }
                voice *= 0.18 * env * speech_amp;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let rnd = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
                *s = voice + rnd * noise_amp;
            }
            let _ = proc.process_frame_48k(&buf, None, None, &mut raw, &mut enh, &mut out);
            if cyc > 0.1 && cyc < 0.55 {
                cnt += 1;
                for i in 0..DFN_HOP_SIZE {
                    in_sq += (raw[i] as f64).powi(2);
                    out_sq += (out[i] as f64).powi(2);
                }
            }
        }
        let in_db = 10.0 * (in_sq / (cnt * DFN_HOP_SIZE) as f64).max(1e-18).log10();
        let out_db = 10.0 * (out_sq / (cnt * DFN_HOP_SIZE) as f64).max(1e-18).log10();
        let delta_db = out_db - in_db;
        assert!(
            delta_db > -13.0,
            "Quiet -45 dBFS speech fell into DFN3 dead zone (delta={:.1} dB; expected > -13 dB)",
            delta_db
        );

        // 3. Sudden loud shout (-6 dBFS) immediately after quiet audio must stay finite & unclipped
        let mut loud = [0.0f32; DFN_HOP_SIZE];
        for (i, s) in loud.iter_mut().enumerate() {
            let t = i as f32 / DFN_SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 0.5;
        }
        for _ in 0..5 {
            let _ = proc.process_frame_48k(&loud, None, None, &mut raw, &mut enh, &mut out);
            assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
    }

    #[test]
    fn test_speaker_gate_smoothly_silences_rejected_voice_and_preserves_pre_gate_tap() {
        let mut proc = NoiseProcessor::new(
            ModelVariant::LowLatency,
            18.0,
            0.0,
            -16.0,
            false,
            -5.0,
            true,
            true, // AGC enabled: must NOT boost rejected voice
            false,
            true,
            false,
            0.0,
        )
        .expect("load LowLatency DFN3");

        let mut pre_tap = [0.0f32; DFN_HOP_SIZE];
        let mut raw = [0.0f32; DFN_HOP_SIZE];
        let mut enh = [0.0f32; DFN_HOP_SIZE];
        let mut out = [0.0f32; DFN_HOP_SIZE];

        // Reject speaker via speaker gate: output fades smoothly to 0.0 while pre-gate tap stays intact
        proc.set_speaker_gate(true, false);
        for f in 0..20 {
            let mut voice = [0.0f32; DFN_HOP_SIZE];
            let f0 = 140.0 + 20.0 * (f as f32 * 0.25).sin();
            for (i, s) in voice.iter_mut().enumerate() {
                let t = (f * DFN_HOP_SIZE + i) as f32 / DFN_SAMPLE_RATE as f32;
                let mut v = 0.0f32;
                for h in 1..=12 {
                    v += (2.0 * std::f32::consts::PI * (f0 * h as f32) * t).sin() / (h as f32);
                }
                *s = v * 0.12;
            }
            let _ = proc.process_frame_48k(
                &voice,
                None,
                Some(&mut pre_tap),
                &mut raw,
                &mut enh,
                &mut out,
            );
        }
        let max_out = out.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        let max_tap = pre_tap.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert_eq!(
            max_out, 0.0,
            "Rejected speaker output must fade cleanly to exact 0.0"
        );
        assert!(
            max_tap > 0.01,
            "Pre-gate tap must keep active audio so enrolled user re-entry can be detected (got {})",
            max_tap
        );
    }
}
