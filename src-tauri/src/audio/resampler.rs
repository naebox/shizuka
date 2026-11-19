use anyhow::Result;
use rubato::{FastFixedIn, FastFixedOut, PolynomialDegree, Resampler};

use crate::audio::processor::{DFN_HOP_SIZE, DFN_SAMPLE_RATE};

/// Proportional-Integral (PI) clock drift controller.
/// Tracks output ring buffer fill level relative to target latency and computes
/// a smooth relative resampling ratio adjustment around 1.0 (e.g. 0.9998 .. 1.0002)
/// to absorb crystal oscillator mismatch between physical mic and virtual audio cable.
pub struct DriftController {
    target_samples: f64,
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
        // Integral time constant Ti = kp / ki = 5s eliminates steady-state clock offset smoothly without pitch warble.
        Self {
            target_samples: target_samples as f64,
            smoothed_error: 0.0,
            integral_error: 0.0,
            kp: 0.04 / sr,
            ki: 0.008 / sr,
            current_rel_ratio: 1.0,
        }
    }

    /// Updates the PI loop once per 10ms frame given the current output ring buffer occupancy (in mono frames).
    /// Returns the relative output resampling ratio in `[0.995, 1.005]`.
    pub fn update(&mut self, occupied_frames: usize) -> f64 {
        let raw_error = occupied_frames as f64 - self.target_samples;
        // Exponential low-pass filter on buffer occupancy jitter (callback block size jitter)
        self.smoothed_error = 0.92 * self.smoothed_error + 0.08 * raw_error;

        // Anti-windup clamped integral term
        self.integral_error = (self.integral_error + self.smoothed_error * 0.01)
            .clamp(-0.003 / self.ki.max(1e-12), 0.003 / self.ki.max(1e-12));

        // Positive error => buffer has MORE samples than target => produce slightly FEWER samples (ratio < 1.0)
        let adjustment = -(self.kp * self.smoothed_error + self.ki * self.integral_error);
        let rel_ratio = (1.0 + adjustment).clamp(0.995, 1.005);
        self.current_rel_ratio = rel_ratio;
        rel_ratio
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
pub struct InputResampler48k {
    resampler: Option<FastFixedOut<f32>>,
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
            let resampler = FastFixedOut::<f32>::new(
                ratio,
                1.02,
                PolynomialDegree::Cubic,
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

/// Zero-allocation adaptive output resampler converting 48kHz 480-sample chunks -> output device sample rate
/// while continuously applying small relative ratio changes for clock drift compensation.
pub struct AdaptiveOutputResampler {
    resampler: FastFixedIn<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl AdaptiveOutputResampler {
    pub fn new(output_sr: u32) -> Result<Self> {
        let nominal_ratio = output_sr as f64 / DFN_SAMPLE_RATE as f64;
        // Always instantiate FastFixedIn even when output_sr == 48_000 so adaptive clock-drift
        // compensation works seamlessly between two independent 48kHz hardware/virtual clocks!
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
            resampler,
            in_buf,
            out_buf,
        })
    }

    /// Processes 480 samples @ 48kHz with smooth ratio ramping (`ramp = true`) and returns a slice of output samples.
    pub fn process(&mut self, in_480: &[f32], relative_ratio: f64) -> Result<&[f32]> {
        let clamped_rel = relative_ratio.clamp(0.99, 1.01);
        self.resampler.set_resample_ratio_relative(clamped_rel, true)?;
        self.in_buf[0][..DFN_HOP_SIZE].copy_from_slice(in_480);
        let (_nbr_in, nbr_out) =
            self.resampler
                .process_into_buffer(&self.in_buf, &mut self.out_buf, None)?;
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
}
