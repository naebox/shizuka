use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ringbuf::traits::{Consumer, Observer};
use sonora::config::{
    DownmixMethod, EchoCanceller, EchoCanceller3Config, MaxProcessingRate, Pipeline,
    TransparentModeType,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::audio::processor::{DFN_HOP_SIZE, DFN_SAMPLE_RATE};
use crate::audio::resampler::{AdaptiveInputResampler48k, DriftController};

/// Pre-allocated, lock-free speaker loopback reference consumer attached to the real-time DSP loop.
/// Constructed on the non-RT audio controller thread so attaching/switching speakers never
/// allocates heap memory on `shizuka-rt-dsp`.
pub struct AecRefSink {
    pub cons: ringbuf::HeapCons<f32>,
    pub resampler: AdaptiveInputResampler48k,
    pub drift: DriftController,
    pub target_frames: usize,
    pub sr_ref: u32,
    pub observed_block_frames: Arc<AtomicU32>,
    pub chunk_scratch: Vec<f32>,
    pub streaming: bool,
    pub silence_grace_frames: u8,
}

impl AecRefSink {
    pub fn new(
        cons: ringbuf::HeapCons<f32>,
        sr_ref: u32,
        observed_block_frames: Arc<AtomicU32>,
    ) -> anyhow::Result<Self> {
        let sr = sr_ref.max(8_000);
        // Keep nominal cushion tight (~15ms) so OS loopback reference stays causal ahead of
        // physical speaker-to-mic acoustic propagation, while still absorbing callback jitter.
        let target_frames = ((sr as usize) * 15) / 1000;
        let resampler = AdaptiveInputResampler48k::new(sr)?;
        let drift = DriftController::new(target_frames, sr);
        Ok(Self {
            cons,
            resampler,
            drift,
            target_frames,
            sr_ref: sr,
            observed_block_frames,
            chunk_scratch: vec![0.0f32; 4_096],
            streaming: false,
            silence_grace_frames: 0,
        })
    }

    /// Pulls 10ms (`480` samples @ 48 kHz) of drift-aligned speaker reference audio without blocking.
    /// When the OS loopback endpoint is idle/silent (e.g., Windows WASAPI stops firing callbacks
    /// whenever no app renders audio), seamlessly emits `0.0` silence so the microphone never stalls.
    pub fn pull_frame_48k(&mut self, out_480: &mut [f32]) -> bool {
        debug_assert_eq!(out_480.len(), DFN_HOP_SIZE);

        let obs_blk = self.observed_block_frames.load(Ordering::Relaxed) as usize;
        let min_floor = (obs_blk + (self.sr_ref as usize / 125))
            .max(((self.sr_ref as usize) * 15) / 1000)
            .min(((self.sr_ref as usize) * 60) / 1000);
        if min_floor > self.target_frames {
            self.target_frames = min_floor;
            self.drift.set_target_samples(min_floor);
        }

        let occ = self.cons.occupied_len();
        let cap = self.cons.capacity().get();

        // Discard stale backlog if AEC was previously bypassed or OS resumed from sleep
        if occ > cap * 3 / 4 && occ > self.target_frames {
            let _ = self.cons.skip(occ - self.target_frames);
            self.drift.reset();
        }

        let occ = self.cons.occupied_len();

        if !self.streaming {
            // Wait until `target_frames` accumulate on transition from silence -> active playback
            // so callback block jitter cannot immediately underrun on frame 2.
            if occ < self.target_frames {
                out_480.fill(0.0);
                return false;
            }
            self.streaming = true;
            self.silence_grace_frames = 0;
            self.drift.reset();
        }

        let rel_ratio = self.drift.update(occ);
        let needed = match self.resampler.prepare_next(rel_ratio) {
            Ok(n) => n,
            Err(_) => {
                out_480.fill(0.0);
                return false;
            }
        };

        if needed > self.chunk_scratch.len() {
            // Fallback guard (4096 covers up to 384 kHz -> 48 kHz 10ms frames = 3840 samples)
            out_480.fill(0.0);
            return false;
        }

        if occ < needed {
            // WASAPI loopback paused callbacks because speaker went silent (or brief scheduler jitter)
            self.silence_grace_frames = self.silence_grace_frames.saturating_add(1);
            if self.silence_grace_frames >= 3 {
                // Drain leftover partial tail so old samples don't fire later as a false transient
                if occ > 0 {
                    let _ = self.cons.skip(occ);
                }
                self.streaming = false;
                self.drift.reset();
            } else {
                self.drift.damp_transient();
            }
            out_480.fill(0.0);
            return false;
        }

        self.silence_grace_frames = 0;
        let popped = self.cons.pop_slice(&mut self.chunk_scratch[..needed]);
        if popped < needed {
            out_480.fill(0.0);
            return false;
        }

        if self
            .resampler
            .process(&self.chunk_scratch[..needed], out_480)
            .is_err()
        {
            out_480.fill(0.0);
            return false;
        }

        let mut has_energy = false;
        for s in out_480.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
            } else if s.abs() > 1e-5 {
                has_energy = true;
            }
        }
        has_energy
    }

    pub fn discard_backlog(&mut self) {
        let occ = self.cons.occupied_len();
        if occ > 0 {
            let _ = self.cons.skip(occ);
        }
        self.streaming = false;
        self.silence_grace_frames = 0;
        self.drift.reset();
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AecFrameMetrics {
    /// True when AEC is enabled, not paused for Mic Test, and processing audio.
    pub active: bool,
    /// Echo Return Loss Enhancement (ERLE) in dB estimated by WebRTC AEC3 (`>= 0.0`).
    pub erle_db: f32,
}

/// Real-time Acoustic Echo Canceller (WebRTC AEC3 via `sonora`) operating at 48 kHz mono.
/// Placed after 65Hz HPF + Manual Input Gain and strictly BEFORE `PreDfnNormalizer` + `DeepFilterNet3`
/// so linear/nonlinear acoustic echo is removed while preserving phase coherence.
pub struct AecProcessor {
    apm: AudioProcessing,
    enabled: bool,
    monitor_bypass: bool,
    wet_mix: f32,
    dry_scratch: [f32; DFN_HOP_SIZE],
    wet_scratch: [f32; DFN_HOP_SIZE],
    render_in_scratch: [f32; DFN_HOP_SIZE],
    render_out_scratch: [f32; DFN_HOP_SIZE],
    cached_erle_db: f32,
    stats_tick: u8,
}

impl AecProcessor {
    /// 128 split-band samples @ 16 kHz = 8ms fixed capture delay margin.
    /// Ensures OS loopback reference (which arrives 1 mixer callback after render) remains strictly
    /// causal even on low-latency wired mics placed close to physical speakers, while adding 0.0 ms
    /// latency whenever AEC is disabled.
    const CAPTURE_CAUSAL_MARGIN_SAMPLES_16K: usize = 128;

    pub fn new(enabled: bool) -> Self {
        let stream_cfg = StreamConfig::new(DFN_SAMPLE_RATE as u32, 1);
        let config = Config {
            pipeline: Pipeline {
                maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                multi_channel_render: false,
                multi_channel_capture: false,
                capture_downmix_method: DownmixMethod::AverageChannels,
            },
            echo_canceller: Some(EchoCanceller {
                // Keep Shizuka's own 65Hz Butterworth HPF in charge so 85-120Hz vocal warmth is untouched
                enforce_high_pass_filtering: false,
                transparent_mode: TransparentModeType::Hmm,
            }),
            ..Default::default()
        };

        let mut aec3_cfg = EchoCanceller3Config::default();
        aec3_cfg.delay.fixed_capture_delay_samples = Self::CAPTURE_CAUSAL_MARGIN_SAMPLES_16K;
        let _ = aec3_cfg.validate();

        let apm = AudioProcessing::builder()
            .config(config)
            .echo_canceller3_config(aec3_cfg)
            .capture_config(stream_cfg)
            .render_config(stream_cfg)
            .build();

        Self {
            apm,
            enabled,
            monitor_bypass: false,
            wet_mix: 0.0,
            dry_scratch: [0.0f32; DFN_HOP_SIZE],
            wet_scratch: [0.0f32; DFN_HOP_SIZE],
            render_in_scratch: [0.0f32; DFN_HOP_SIZE],
            render_out_scratch: [0.0f32; DFN_HOP_SIZE],
            cached_erle_db: 0.0,
            stats_tick: 0,
        }
    }

    #[inline]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Automatically bypasses AEC while Shizuka's own "マイクテスト (Live Monitor)" is playing
    /// so the user's monitored voice can never feed back into AEC and cancel itself.
    #[inline]
    pub fn set_monitor_bypass(&mut self, bypass: bool) {
        self.monitor_bypass = bypass;
    }

    /// Extra algorithmic delay contributed when AEC is actively engaged (`8.0 ms`; `0.0 ms` when OFF).
    #[inline]
    pub fn active_latency_ms(&self) -> f32 {
        if self.enabled && !self.monitor_bypass {
            (Self::CAPTURE_CAUSAL_MARGIN_SAMPLES_16K as f32 / 16.0).max(0.0)
        } else {
            0.0
        }
    }

    /// Processes a 10ms (`480`-sample @ 48 kHz) capture frame in place with zero heap allocations.
    /// When `ref_480` is `None` or AEC is disabled/bypassed, smoothly ramps back to bit-exact pass-through.
    pub fn process_frame_48k(
        &mut self,
        ref_480: Option<&[f32]>,
        mic_in_out_480: &mut [f32],
    ) -> AecFrameMetrics {
        debug_assert_eq!(mic_in_out_480.len(), DFN_HOP_SIZE);

        let want_active = self.enabled && !self.monitor_bypass && ref_480.is_some();
        let target_mix = if want_active { 1.0_f32 } else { 0.0_f32 };

        // Fast zero-CPU, zero-latency bypass when AEC is completely inactive
        if !want_active && self.wet_mix <= 1e-4 {
            self.wet_mix = 0.0;
            self.cached_erle_db = 0.0;
            return AecFrameMetrics {
                active: false,
                erle_db: 0.0,
            };
        }

        for (dst, &src) in self.dry_scratch.iter_mut().zip(mic_in_out_480.iter()) {
            *dst = if src.is_finite() { src } else { 0.0 };
        }

        if let Some(r) = ref_480 {
            debug_assert_eq!(r.len(), DFN_HOP_SIZE);
            for (dst, &src) in self.render_in_scratch.iter_mut().zip(r.iter()) {
                *dst = if src.is_finite() { src } else { 0.0 };
            }
        } else {
            self.render_in_scratch.fill(0.0);
        }

        // Feed far-end speaker reference BEFORE capture frame so current-frame echo is causal
        let render_ok = self
            .apm
            .process_render_f32(
                &[&self.render_in_scratch],
                &mut [&mut self.render_out_scratch],
            )
            .is_ok();
        let capture_ok = render_ok
            && self
                .apm
                .process_capture_f32(&[&self.dry_scratch], &mut [&mut self.wet_scratch])
                .is_ok();

        if !capture_ok {
            self.wet_mix = 0.0;
            return AecFrameMetrics {
                active: false,
                erle_db: 0.0,
            };
        }

        if (self.wet_mix - target_mix).abs() <= 1e-4 {
            self.wet_mix = target_mix;
            if want_active {
                for (dst, &w) in mic_in_out_480.iter_mut().zip(self.wet_scratch.iter()) {
                    *dst = if w.is_finite() { w } else { 0.0 };
                }
            }
        } else {
            let step = (target_mix - self.wet_mix) / (DFN_HOP_SIZE as f32);
            let mut m = self.wet_mix;
            for (i, dst) in mic_in_out_480.iter_mut().enumerate().take(DFN_HOP_SIZE) {
                m = (m + step).clamp(0.0, 1.0);
                let dry = self.dry_scratch[i];
                let wet = if self.wet_scratch[i].is_finite() {
                    self.wet_scratch[i]
                } else {
                    dry
                };
                *dst = dry * (1.0 - m) + wet * m;
            }
            self.wet_mix = target_mix;
        }

        if want_active {
            self.stats_tick = self.stats_tick.wrapping_add(1);
            if self.stats_tick >= 10 {
                self.stats_tick = 0;
                let stats = self.apm.statistics();
                if let Some(erle) = stats.echo_return_loss_enhancement {
                    let v = erle as f32;
                    if v.is_finite() {
                        self.cached_erle_db = v.clamp(0.0, 60.0);
                    }
                }
            }
        } else {
            self.cached_erle_db = 0.0;
        }

        AecFrameMetrics {
            active: want_active,
            erle_db: self.cached_erle_db,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    /// Synthesizes speech-like harmonic + broadband excitation so AEC3 delay estimator & adaptive
    /// filters experience realistic wide-band voice signal statistics.
    fn synth_far_end_voice_sample(sample_idx: usize, seed: &mut u32) -> f32 {
        let t = sample_idx as f32 / DFN_SAMPLE_RATE as f32;
        let syllable_env = 0.55 + 0.45 * (2.0 * std::f32::consts::PI * 3.2 * t).sin();
        let f0 = 145.0 + 20.0 * (t * 1.7).sin();
        let mut s = 0.0f32;
        for h in 1..=12 {
            let freq = f0 * h as f32;
            s += (1.0 / (h as f32).sqrt()) * (2.0 * std::f32::consts::PI * freq * t).sin();
        }
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let hiss = (((*seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0) * 0.08;
        (s * 0.09 + hiss) * syllable_env
    }

    #[test]
    fn test_aec_cancels_simulated_speaker_room_echo() {
        let mut aec = AecProcessor::new(true);

        // Simulate a speaker-to-mic room impulse response:
        // direct acoustic path at 32ms (1536 samples @ 48kHz) + two early room reflections
        let max_hist = 4_800usize;
        let mut spk_hist = vec![0.0f32; max_hist];
        let mut wr = 0usize;
        let taps: [(usize, f32); 3] = [(1536, 0.45), (1890, -0.16), (2420, 0.08)];

        let mut seed = 0xCAFE_BABEu32;
        let mut ref_frame = [0.0f32; DFN_HOP_SIZE];
        let mut mic_frame = [0.0f32; DFN_HOP_SIZE];

        let mut echo_in_sq = 0.0f64;
        let mut echo_out_sq = 0.0f64;

        // Run 4.5s (450 frames of 10ms); measure residual echo over the last 1.5s after convergence
        for f in 0..450 {
            for i in 0..DFN_HOP_SIZE {
                let g_idx = f * DFN_HOP_SIZE + i;
                let spk = synth_far_end_voice_sample(g_idx, &mut seed);
                ref_frame[i] = spk;
                spk_hist[wr] = spk;

                let mut echo = 0.0f32;
                for &(delay, gain) in &taps {
                    let r_idx = (wr + max_hist - delay) % max_hist;
                    echo += spk_hist[r_idx] * gain;
                }
                mic_frame[i] = echo;
                wr = (wr + 1) % max_hist;
            }

            let raw_copy = mic_frame;
            let metrics = aec.process_frame_48k(Some(&ref_frame), &mut mic_frame);
            assert!(metrics.active);

            if f >= 300 {
                for i in 0..DFN_HOP_SIZE {
                    echo_in_sq += (raw_copy[i] as f64).powi(2);
                    echo_out_sq += (mic_frame[i] as f64).powi(2);
                }
            }
        }

        let attenuation_db = 10.0 * (echo_in_sq / echo_out_sq.max(1e-18)).log10();
        assert!(
            attenuation_db > 20.0,
            "Expected >20 dB speaker echo cancellation, got {:.1} dB",
            attenuation_db
        );
    }

    #[test]
    fn test_aec_causal_margin_handles_slightly_late_os_loopback() {
        // Simulates tight hardware latency where OS loopback arrives 4ms (192 samples @ 48kHz)
        // *after* the physical mic sample (would be non-causal without CAPTURE_CAUSAL_MARGIN_SAMPLES_16K).
        let mut aec = AecProcessor::new(true);
        let late_samples = 192usize; // 4ms late reference
        let mut delay_line = vec![0.0f32; late_samples];
        let mut dl_idx = 0usize;
        let mut seed = 0x1357_9BDFu32;

        let mut ref_frame = [0.0f32; DFN_HOP_SIZE];
        let mut mic_frame = [0.0f32; DFN_HOP_SIZE];
        let mut in_sq = 0.0f64;
        let mut out_sq = 0.0f64;

        for f in 0..400 {
            for i in 0..DFN_HOP_SIZE {
                let spk = synth_far_end_voice_sample(f * DFN_HOP_SIZE + i, &mut seed);
                // Mic hears `spk * 0.4` immediately, while OS loopback delivers `spk` 4ms later!
                mic_frame[i] = spk * 0.4;
                ref_frame[i] = delay_line[dl_idx];
                delay_line[dl_idx] = spk;
                dl_idx = (dl_idx + 1) % late_samples;
            }
            let before = mic_frame;
            let _ = aec.process_frame_48k(Some(&ref_frame), &mut mic_frame);
            if f >= 250 {
                for i in 0..DFN_HOP_SIZE {
                    in_sq += (before[i] as f64).powi(2);
                    out_sq += (mic_frame[i] as f64).powi(2);
                }
            }
        }

        let atten_db = 10.0 * (in_sq / out_sq.max(1e-18)).log10();
        assert!(
            atten_db > 15.0,
            "Expected causal margin to cancel slightly-late OS loopback (>15 dB), got {:.1} dB",
            atten_db
        );
    }

    #[test]
    fn test_ref_sink_handles_wasapi_silent_idle_and_monitor_bypass() {
        let sr = 48_000u32;
        let rb = HeapRb::<f32>::new(16_384);
        let (mut prod, cons) = rb.split();
        let obs = Arc::new(AtomicU32::new(480));
        let mut sink = AecRefSink::new(cons, sr, obs).unwrap();
        let mut out = [1.0f32; DFN_HOP_SIZE];

        // 1. When WASAPI loopback fires zero callbacks (speaker idle), pull_frame_48k never blocks and outputs 0.0
        assert!(!sink.pull_frame_48k(&mut out));
        assert!(out.iter().all(|&s| s == 0.0));

        // 2. Push 30ms of speaker tone -> sink transitions cleanly to streaming
        let tone: Vec<f32> = (0..1440)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin() * 0.3)
            .collect();
        let _ = prod.push_slice(&tone);
        assert!(sink.pull_frame_48k(&mut out));
        assert!(out.iter().any(|&s| s.abs() > 0.1));

        // 3. Monitor bypass protects near-end voice from self-cancellation
        let mut aec = AecProcessor::new(true);
        aec.set_monitor_bypass(true);
        let orig = out;
        let mut mic = orig;
        let m = aec.process_frame_48k(Some(&orig), &mut mic);
        assert!(!m.active);
        assert_eq!(mic, orig);
    }
}
