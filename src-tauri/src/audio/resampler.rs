use anyhow::Result;
use rubato::{
    FastFixedIn, FastFixedOut, PolynomialDegree, Resampler, SincFixedIn, SincFixedOut,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::audio::processor::{DFN_HOP_SIZE, DFN_SAMPLE_RATE};

fn low_latency_sinc_params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 64, // 32-sample group delay (< 0.7ms @ 48kHz) with strong ultrasonic anti-aliasing
        f_cutoff: 0.92,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    }
}

/// Proportional-Integral (PI) clock drift controller.
/// Tracks output ring buffer fill level relative to target latency and computes
/// a smooth relative resampling ratio adjustment around 1.0 (e.g. 0.9998 .. 1.0002)
/// to absorb crystal oscillator mismatch between physical mic and virtual audio cable.
pub struct DriftController {
    target_samples: f64,
    max_integrand_samples: f64,
    smoothed_error: f64,
    integral_error: f64,
    kp: f64,
    ki: f64,
    current_rel_ratio: f64,
}

impl DriftController {
    pub fn new(target_samples: usize, sample_rate_out: u32) -> Self {
        let sr = sample_rate_out.max(8_000) as f64;
        // Normalize error by sample rate so error is measured in seconds of drift.
        // Near-critically damped 2nd-order PI loop:
        //   omega_n = sqrt(sr * ki) = sqrt(0.0018) ≈ 0.0424 rad/s
        //   zeta = (sr * kp) / (2 * omega_n) = 0.075 / 0.0849 ≈ 0.884
        // Eliminates the underdamped (-2000..+1000 ppm) ringing after startup/OS hiccups
        // while locking onto steady-state crystal clock drift cleanly without pitch warble.
        Self {
            target_samples: target_samples as f64,
            max_integrand_samples: sr * 0.004, // ±4ms integrand cap prevents windup on OS burst spikes
            smoothed_error: 0.0,
            integral_error: 0.0,
            kp: 0.075 / sr,
            ki: 0.0018 / sr,
            current_rel_ratio: 1.0,
        }
    }

    /// Updates the PI loop once per 10ms frame given the current output ring buffer occupancy (in mono frames).
    /// Returns the relative output resampling ratio in `[0.9985, 1.0015]`.
    pub fn update(&mut self, occupied_frames: usize) -> f64 {
        let raw_error = occupied_frames as f64 - self.target_samples;
        // ~0.66s exponential low-pass filter suppresses 10-40ms OS callback block-size sawtooth
        // while preserving >70 deg phase margin at omega_n = 0.0424 rad/s.
        self.smoothed_error = 0.985 * self.smoothed_error + 0.015 * raw_error;

        // Conditional anti-windup integration: cap per-step integrand to ±4ms so large one-shot
        // OS scheduling bursts are drained by `kp` without winding up `integral_error`.
        let bounded_err = self
            .smoothed_error
            .clamp(-self.max_integrand_samples, self.max_integrand_samples);
        self.integral_error = (self.integral_error + bounded_err * 0.01)
            .clamp(-0.0008 / self.ki.max(1e-12), 0.0008 / self.ki.max(1e-12));

        // Positive error => buffer has MORE samples than target => produce slightly FEWER samples (ratio < 1.0)
        let adjustment = -(self.kp * self.smoothed_error + self.ki * self.integral_error);
        let rel_ratio = (1.0 + adjustment).clamp(0.9985, 1.0015);
        self.current_rel_ratio = rel_ratio;
        rel_ratio
    }

    /// Raises or updates the target buffer level dynamically (e.g., when the OS audio host uses
    /// larger callback block sizes) without injecting a sharp proportional step spike or
    /// discarding the learned clock-drift `integral_error`.
    pub fn set_target_samples(&mut self, target_samples: usize) {
        let next_target = target_samples as f64;
        let delta = next_target - self.target_samples;
        self.target_samples = next_target;
        // Dampen immediate proportional kick when target rises dynamically at stream startup
        self.smoothed_error = (self.smoothed_error - delta * 0.25).clamp(
            -0.015 / self.kp.max(1e-12),
            0.015 / self.kp.max(1e-12),
        );
    }

    /// Damps transient error spikes (e.g., brief underrun) while keeping the learned oscillator
    /// drift `integral_error` intact so long-call clock lock does not have to reconverge from zero.
    pub fn damp_transient(&mut self) {
        self.smoothed_error *= 0.25;
    }

    pub fn reset(&mut self) {
        self.smoothed_error = 0.0;
        self.integral_error = 0.0;
        self.current_rel_ratio = 1.0;
    }

    /// Estimated clock drift compensation in parts-per-million (ppm).
    pub fn drift_ppm(&self) -> f32 {
        ((self.current_rel_ratio - 1.0) * 1_000_000.0) as f32
    }
}

/// Zero-allocation input resampler converting arbitrary microphone sample rate -> 48kHz 480-sample chunks.
/// Uses an anti-aliased windowed Sinc resampler whenever `input_sr != 48_000` so 96/192 kHz interfaces
/// do not alias ultrasonic content and 16 kHz Bluetooth HFP does not emit imaging artifacts.
pub struct InputResampler48k {
    resampler: Option<SincFixedOut<f32>>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl InputResampler48k {
    pub fn new(input_sr: u32) -> Result<Self> {
        if input_sr as usize == DFN_SAMPLE_RATE {
            Ok(Self {
                resampler: None,
                in_buf: Vec::new(),
                out_buf: Vec::new(),
            })
        } else {
            let ratio = DFN_SAMPLE_RATE as f64 / input_sr as f64;
            let resampler = SincFixedOut::<f32>::new(
                ratio,
                1.02,
                low_latency_sinc_params(),
                DFN_HOP_SIZE,
                1,
            )?;
            let in_buf = resampler.input_buffer_allocate(true);
            let out_buf = resampler.output_buffer_allocate(true);
            Ok(Self {
                resampler: Some(resampler),
                in_buf,
                out_buf,
            })
        }
    }

    /// Exact number of input mono samples needed from the microphone ring buffer to emit 480 samples @ 48kHz.
    pub fn input_frames_next(&self) -> usize {
        match &self.resampler {
            Some(r) => r.input_frames_next(),
            None => DFN_HOP_SIZE,
        }
    }

    pub fn process(&mut self, input_slice: &[f32], out_480: &mut [f32]) -> Result<()> {
        match &mut self.resampler {
            None => {
                out_480.copy_from_slice(&input_slice[..DFN_HOP_SIZE]);
                Ok(())
            }
            Some(r) => {
                let needed = r.input_frames_next();
                self.in_buf[0].resize(needed, 0.0);
                self.in_buf[0].copy_from_slice(&input_slice[..needed]);
                let (_nbr_in, nbr_out) =
                    r.process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
                debug_assert_eq!(nbr_out, DFN_HOP_SIZE);
                out_480.copy_from_slice(&self.out_buf[0][..DFN_HOP_SIZE]);
                Ok(())
            }
        }
    }
}

enum AdaptiveInputResamplerKind {
    Fast(FastFixedOut<f32>),
    Sinc(SincFixedOut<f32>),
}

/// Zero-allocation adaptive input resampler converting an arbitrary speaker loopback sample rate
/// into exact 48 kHz `480`-sample (10ms) frames while continuously tracking clock drift between
/// the physical speaker DAC and physical microphone ADC.
pub struct AdaptiveInputResampler48k {
    resampler: AdaptiveInputResamplerKind,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl AdaptiveInputResampler48k {
    pub fn new(input_sr: u32) -> Result<Self> {
        let nominal_ratio = DFN_SAMPLE_RATE as f64 / input_sr.max(8_000) as f64;
        if input_sr as usize == DFN_SAMPLE_RATE {
            let resampler = FastFixedOut::<f32>::new(
                nominal_ratio,
                1.02,
                PolynomialDegree::Cubic,
                DFN_HOP_SIZE,
                1,
            )?;
            let in_buf = resampler.input_buffer_allocate(true);
            let out_buf = resampler.output_buffer_allocate(true);
            Ok(Self {
                resampler: AdaptiveInputResamplerKind::Fast(resampler),
                in_buf,
                out_buf,
            })
        } else {
            let resampler = SincFixedOut::<f32>::new(
                nominal_ratio,
                1.02,
                low_latency_sinc_params(),
                DFN_HOP_SIZE,
                1,
            )?;
            let in_buf = resampler.input_buffer_allocate(true);
            let out_buf = resampler.output_buffer_allocate(true);
            Ok(Self {
                resampler: AdaptiveInputResamplerKind::Sinc(resampler),
                in_buf,
                out_buf,
            })
        }
    }

    /// Updates relative resampling ratio (`[0.99, 1.01]`) and returns exact input mono frames needed
    /// to emit 480 samples @ 48 kHz on the next `process()` call.
    pub fn prepare_next(&mut self, relative_ratio: f64) -> Result<usize> {
        let clamped_rel = relative_ratio.clamp(0.99, 1.01);
        match &mut self.resampler {
            AdaptiveInputResamplerKind::Fast(r) => {
                r.set_resample_ratio_relative(clamped_rel, true)?;
                Ok(r.input_frames_next())
            }
            AdaptiveInputResamplerKind::Sinc(r) => {
                r.set_resample_ratio_relative(clamped_rel, true)?;
                Ok(r.input_frames_next())
            }
        }
    }

    pub fn process(&mut self, input_slice: &[f32], out_480: &mut [f32]) -> Result<()> {
        match &mut self.resampler {
            AdaptiveInputResamplerKind::Fast(r) => {
                let needed = r.input_frames_next();
                self.in_buf[0].resize(needed, 0.0);
                self.in_buf[0].copy_from_slice(&input_slice[..needed]);
                let (_nbr_in, nbr_out) =
                    r.process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
                debug_assert_eq!(nbr_out, DFN_HOP_SIZE);
                out_480.copy_from_slice(&self.out_buf[0][..DFN_HOP_SIZE]);
                Ok(())
            }
            AdaptiveInputResamplerKind::Sinc(r) => {
                let needed = r.input_frames_next();
                self.in_buf[0].resize(needed, 0.0);
                self.in_buf[0].copy_from_slice(&input_slice[..needed]);
                let (_nbr_in, nbr_out) =
                    r.process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
                debug_assert_eq!(nbr_out, DFN_HOP_SIZE);
                out_480.copy_from_slice(&self.out_buf[0][..DFN_HOP_SIZE]);
                Ok(())
            }
        }
    }
}

enum OutputResamplerKind {
    Fast(FastFixedIn<f32>),
    Sinc(SincFixedIn<f32>),
}

/// Zero-allocation adaptive output resampler converting 48kHz 480-sample chunks -> output device sample rate
/// while continuously applying small relative ratio changes for clock drift compensation.
pub struct AdaptiveOutputResampler {
    resampler: OutputResamplerKind,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl AdaptiveOutputResampler {
    pub fn new(output_sr: u32) -> Result<Self> {
        let nominal_ratio = output_sr as f64 / DFN_SAMPLE_RATE as f64;
        if output_sr as usize == DFN_SAMPLE_RATE {
            // FastFixedIn cubic interpolation has zero filter group delay and is transparent for
            // small +-0.05% adaptive clock-drift compensation between two 48 kHz clocks.
            let resampler = FastFixedIn::<f32>::new(
                nominal_ratio,
                1.02, // Allows up to ±2% dynamic adjustment without reallocating
                PolynomialDegree::Cubic,
                DFN_HOP_SIZE,
                1,
            )?;
            let in_buf = resampler.input_buffer_allocate(true);
            let out_buf = resampler.output_buffer_allocate(true);
            Ok(Self {
                resampler: OutputResamplerKind::Fast(resampler),
                in_buf,
                out_buf,
            })
        } else {
            // Anti-aliased adaptive Sinc resampling for non-48 kHz outputs (e.g., 44.1 kHz / 96 kHz).
            let resampler = SincFixedIn::<f32>::new(
                nominal_ratio,
                1.02,
                low_latency_sinc_params(),
                DFN_HOP_SIZE,
                1,
            )?;
            let in_buf = resampler.input_buffer_allocate(true);
            let out_buf = resampler.output_buffer_allocate(true);
            Ok(Self {
                resampler: OutputResamplerKind::Sinc(resampler),
                in_buf,
                out_buf,
            })
        }
    }

    /// Processes 480 samples @ 48kHz with smooth ratio ramping (`ramp = true`) and returns a slice of output samples.
    pub fn process(&mut self, in_480: &[f32], relative_ratio: f64) -> Result<&[f32]> {
        let clamped_rel = relative_ratio.clamp(0.99, 1.01);
        self.in_buf[0][..DFN_HOP_SIZE].copy_from_slice(in_480);
        let nbr_out = match &mut self.resampler {
            OutputResamplerKind::Fast(r) => {
                r.set_resample_ratio_relative(clamped_rel, true)?;
                let (_nbr_in, n) = r.process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
                n
            }
            OutputResamplerKind::Sinc(r) => {
                r.set_resample_ratio_relative(clamped_rel, true)?;
                let (_nbr_in, n) = r.process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
                n
            }
        };
        Ok(&self.out_buf[0][..nbr_out])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drift_controller_compensates_clock_mismatch() {
        let sr_out = 48_000u32;
        let target = 1440usize; // 30ms target buffer
        let mut ctrl = DriftController::new(target, sr_out);
        let mut resampler = AdaptiveOutputResampler::new(sr_out).unwrap();

        // Simulate microphone running +120 ppm faster than output device for 30 seconds (3000 frames of 10ms)
        // Output consumes 480.0 samples per 10.0012ms => per mic 10ms frame, output consumes 480 * (1 - 0.00012) = 479.9424 samples
        let mut buffer_level = target as f64;
        let frame_480 = [0.05f32; DFN_HOP_SIZE];

        for _ in 0..3_000 {
            let rel = ctrl.update(buffer_level.round().max(0.0) as usize);
            let out = resampler.process(&frame_480, rel).unwrap();
            buffer_level += out.len() as f64;
            buffer_level -= 480.0 * (1.0 - 0.00012);
        }

        // Without drift compensation, +120 ppm over 30s would add +173 samples and grow unboundedly forever;
        // with PI controller, buffer stays tightly locked near target (1440 ± 80 samples)!
        assert!(
            (buffer_level - target as f64).abs() < 80.0,
            "Buffer drifted too far: {:.1} vs target {}",
            buffer_level,
            target
        );
        assert!(
            ctrl.drift_ppm() < -50.0,
            "Expected negative ppm compensation for faster mic, got {:.1} ppm",
            ctrl.drift_ppm()
        );
    }

    #[test]
    fn test_negative_drift_and_multi_sample_rates() {
        // Negative clock drift (-120 ppm: slower mic than output)
        let sr_out = 48_000u32;
        let target = 1440usize;
        let mut ctrl = DriftController::new(target, sr_out);
        let mut out_rs = AdaptiveOutputResampler::new(sr_out).unwrap();
        let mut level = target as f64;
        let frame = [0.05f32; DFN_HOP_SIZE];
        for _ in 0..3_000 {
            let rel = ctrl.update(level.round().max(0.0) as usize);
            let out = out_rs.process(&frame, rel).unwrap();
            level += out.len() as f64;
            level -= 480.0 * (1.0 + 0.00012);
        }
        assert!((level - target as f64).abs() < 80.0);
        assert!(ctrl.drift_ppm() > 50.0);

        // Input & output resamplers across common hardware/Bluetooth sample rates
        for sr in [16_000u32, 44_100, 48_000, 96_000, 192_000] {
            let mut in_rs = InputResampler48k::new(sr).unwrap();
            let mut out_480 = [0.0f32; DFN_HOP_SIZE];
            for _ in 0..5 {
                let needed = in_rs.input_frames_next();
                let in_chunk = vec![0.1f32; needed];
                in_rs.process(&in_chunk, &mut out_480).unwrap();
                assert!(out_480.iter().all(|s| s.is_finite()));
            }

            let mut out_rs = AdaptiveOutputResampler::new(sr).unwrap();
            let res = out_rs.process(&out_480, 1.0005).unwrap();
            assert!(!res.is_empty());
            assert!(res.iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn test_drift_controller_damps_transient_without_wild_ppm_ringing() {
        let sr_out = 48_000u32;
        let target = 1440usize;
        let mut ctrl = DriftController::new(target, sr_out);
        let mut out_rs = AdaptiveOutputResampler::new(sr_out).unwrap();
        let frame = [0.02f32; DFN_HOP_SIZE];

        // Inject a sudden +960-sample (20ms) OS scheduling burst on top of target, then run 90s (~3.4 time constants)
        let mut level = (target + 960) as f64;
        let mut min_ppm = f32::MAX;
        let mut max_ppm = f32::MIN;
        for step in 0..9_000 {
            // Simulate realistic 512-frame CoreAudio callback sawtooth jitter around `level`
            let sawtooth = ((step % 3) as f64 - 1.0) * 180.0;
            let rel = ctrl.update((level + sawtooth).round().max(0.0) as usize);
            let ppm = ctrl.drift_ppm();
            min_ppm = min_ppm.min(ppm);
            max_ppm = max_ppm.max(ppm);
            let out = out_rs.process(&frame, rel).unwrap();
            level += out.len() as f64 - 480.0;
        }

        // Critically-damped controller must recover smoothly without swinging into +-2000 ppm
        // or ringing into large positive overshoot (> +80 ppm) when draining a positive surplus.
        assert!(
            min_ppm > -1_500.0 && max_ppm < 80.0,
            "Unexpected transient overshoot/ringing: min_ppm={:.1}, max_ppm={:.1}",
            min_ppm,
            max_ppm
        );
        assert!(
            (level - target as f64).abs() < 60.0,
            "Expected tight return to target {}, got {:.1}",
            target,
            level
        );

        // Also verify AdaptiveInputResampler48k across common speaker loopback sample rates
        for sr in [44_100u32, 48_000, 96_000] {
            let mut in_rs = AdaptiveInputResampler48k::new(sr).unwrap();
            let mut out_480 = [0.0f32; DFN_HOP_SIZE];
            for _ in 0..5 {
                let needed = in_rs.prepare_next(0.9997).unwrap();
                let in_chunk = vec![0.08f32; needed];
                in_rs.process(&in_chunk, &mut out_480).unwrap();
                assert!(out_480.iter().all(|s| s.is_finite()));
            }
        }
    }
}
