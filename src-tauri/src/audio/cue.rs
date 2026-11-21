use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::devices::find_output_device;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    NcOn,
    NcOff,
    MuteOn,
    MuteOff,
}

/// Non-blocking synthesized audio feedback player for global shortcut & tray actions.
/// Plays subtle two-tone chimes to the user's physical speaker/headphones (never the virtual mic)
/// on a dedicated background thread and releases the audio stream immediately after playback.
#[derive(Clone)]
pub struct CuePlayer {
    tx: mpsc::SyncSender<(CueKind, Option<String>)>,
}

impl CuePlayer {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::sync_channel::<(CueKind, Option<String>)>(4);
        let _ = thread::Builder::new()
            .name("shizuka-cue-player".into())
            .spawn(move || cue_worker_loop(rx));
        Self { tx }
    }

    pub fn play(&self, kind: CueKind, monitor_device: Option<String>) {
        // Non-blocking send so global shortcut handlers never stall
        let _ = self.tx.try_send((kind, monitor_device));
    }
}

fn cue_worker_loop(rx: mpsc::Receiver<(CueKind, Option<String>)>) {
    while let Ok(mut pending) = rx.recv() {
        // Coalesce rapid repeated presses so only the latest state chime plays
        while let Ok(newer) = rx.try_recv() {
            pending = newer;
        }
        let (kind, monitor_device) = pending;
        if let Err(err) = play_cue_blocking(kind, monitor_device.as_deref()) {
            log::debug!("Cue audio playback skipped: {}", err);
        }
    }
}

struct ToneSegment {
    freq_hz: f32,
    duration_ms: f32,
    peak_gain: f32,
}

fn cue_segments(kind: CueKind) -> [ToneSegment; 2] {
    match kind {
        // Gentle rising C5 -> E5 chime (NC enabled)
        CueKind::NcOn => [
            ToneSegment {
                freq_hz: 523.25,
                duration_ms: 52.0,
                peak_gain: 0.12,
            },
            ToneSegment {
                freq_hz: 659.25,
                duration_ms: 88.0,
                peak_gain: 0.13,
            },
        ],
        // Gentle falling C5 -> G4 chime (NC bypassed)
        CueKind::NcOff => [
            ToneSegment {
                freq_hz: 523.25,
                duration_ms: 52.0,
                peak_gain: 0.12,
            },
            ToneSegment {
                freq_hz: 392.00,
                duration_ms: 88.0,
                peak_gain: 0.12,
            },
        ],
        // Soft closing A4 -> E4 tone (Mic muted)
        CueKind::MuteOn => [
            ToneSegment {
                freq_hz: 440.00,
                duration_ms: 48.0,
                peak_gain: 0.13,
            },
            ToneSegment {
                freq_hz: 329.63,
                duration_ms: 85.0,
                peak_gain: 0.13,
            },
        ],
        // Crisp opening A4 -> D5 tone (Mic unmuted)
        CueKind::MuteOff => [
            ToneSegment {
                freq_hz: 440.00,
                duration_ms: 48.0,
                peak_gain: 0.12,
            },
            ToneSegment {
                freq_hz: 587.33,
                duration_ms: 88.0,
                peak_gain: 0.13,
            },
        ],
    }
}

/// Synthesizes a subtle, click-free mono sine-chime waveform at `sample_rate`.
pub fn synthesize_cue_samples(kind: CueKind, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate.max(8_000) as f32;
    let segments = cue_segments(kind);
    let gap_samples = ((sr * 0.012).round() as usize).max(1);
    let tail_silence = ((sr * 0.040).round() as usize).max(1);

    let total_est = segments
        .iter()
        .map(|s| (sr * s.duration_ms / 1000.0).ceil() as usize)
        .sum::<usize>()
        + gap_samples
        + tail_silence;
    let mut samples = Vec::with_capacity(total_est);

    for (seg_idx, seg) in segments.iter().enumerate() {
        if seg_idx > 0 {
            samples.resize(samples.len() + gap_samples, 0.0);
        }
        let n = ((sr * seg.duration_ms / 1000.0).round() as usize).max(1);
        let attack = ((sr * 0.004).round() as usize).max(1).min(n / 3);
        let release = ((sr * 0.012).round() as usize).max(1).min(n / 2);

        for i in 0..n {
            let t = i as f32 / sr;
            let phase = 2.0 * std::f32::consts::PI * seg.freq_hz * t;
            // Warm fundamental + tiny 2nd harmonic for gentle acoustic presence on laptop speakers
            let wave = phase.sin() + 0.10 * (2.0 * phase).sin();

            // Smooth raised-cosine attack + exponential body decay + zero-ending release
            let attack_env = if i < attack {
                0.5 - 0.5 * (std::f32::consts::PI * i as f32 / attack as f32).cos()
            } else {
                1.0
            };
            let decay_env = (-3.2 * (i as f32 / n as f32)).exp();
            let release_env = if i + release >= n {
                let rem = (n - 1 - i) as f32 / release as f32;
                0.5 - 0.5 * (std::f32::consts::PI * rem).cos()
            } else {
                1.0
            };

            samples.push(wave * seg.peak_gain * attack_env * decay_env * release_env);
        }
    }

    // Trailing silence ensures the OS mixer flushes the last frame cleanly before stream drop
    samples.resize(samples.len() + tail_silence, 0.0);
    samples
}

fn play_cue_blocking(kind: CueKind, monitor_device: Option<&str>) -> anyhow::Result<()> {
    // Always target physical speakers/headphones (fallback_to_default=true prefers non-virtual output)
    let dev = find_output_device(monitor_device, true)
        .ok_or_else(|| anyhow::anyhow!("No physical output device available for cue audio"))?;
    let supported = dev.default_output_config()?;
    let fmt = supported.sample_format();
    let cfg: StreamConfig = supported.into();
    let sr = cfg.sample_rate.0;
    let ch = cfg.channels as usize;

    let samples = Arc::new(synthesize_cue_samples(kind, sr));
    let expected_ms = ((samples.len() as u64) * 1000 / (sr.max(8_000) as u64)).max(120);
    let cursor = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));

    let stream = build_cue_stream(
        &dev,
        &cfg,
        fmt,
        ch,
        Arc::clone(&samples),
        Arc::clone(&cursor),
        Arc::clone(&done),
    )?;
    stream.play()?;

    let deadline = Instant::now() + Duration::from_millis(expected_ms + 140);
    while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(12));
    }
    // Brief drain wait so the final hardware buffer plays out cleanly
    thread::sleep(Duration::from_millis(25));
    drop(stream);
    Ok(())
}

fn build_cue_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
) -> anyhow::Result<Stream> {
    match format {
        SampleFormat::F32 => build_cue_typed::<f32>(device, config, channels, samples, cursor, done),
        SampleFormat::F64 => build_cue_typed::<f64>(device, config, channels, samples, cursor, done),
        SampleFormat::I16 => build_cue_typed::<i16>(device, config, channels, samples, cursor, done),
        SampleFormat::U16 => build_cue_typed::<u16>(device, config, channels, samples, cursor, done),
        SampleFormat::I32 => build_cue_typed::<i32>(device, config, channels, samples, cursor, done),
        SampleFormat::U32 => build_cue_typed::<u32>(device, config, channels, samples, cursor, done),
        other => Err(anyhow::anyhow!("Unsupported cue output format: {:?}", other)),
    }
}

fn build_cue_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
) -> anyhow::Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = channels.max(1);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let zero = T::from_sample(0.0);
            let frames = data.len() / ch;
            let len = samples.len();
            let mut idx = cursor.load(Ordering::Relaxed);

            for f in 0..frames {
                let val = if idx < len {
                    let s = samples[idx];
                    idx += 1;
                    s
                } else {
                    0.0
                };
                let converted = T::from_sample(val);
                let base = f * ch;
                for c in 0..ch {
                    data[base + c] = if c < 2 { converted } else { zero };
                }
            }

            cursor.store(idx, Ordering::Relaxed);
            if idx >= len {
                done.store(true, Ordering::Relaxed);
            }
        },
        |err| log::debug!("Cue stream callback error: {}", err),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesize_cue_samples_are_subtle_finite_and_clickless() {
        for kind in [
            CueKind::NcOn,
            CueKind::NcOff,
            CueKind::MuteOn,
            CueKind::MuteOff,
        ] {
            let buf = synthesize_cue_samples(kind, 48_000);
            assert!(
                buf.len() > 6_000 && buf.len() < 16_000,
                "Cue {:?} duration out of expected subtle range: {} samples",
                kind,
                buf.len()
            );
            let mut peak = 0.0f32;
            for &s in &buf {
                assert!(s.is_finite(), "Cue sample must be finite");
                peak = peak.max(s.abs());
            }
            assert!(
                peak > 0.05 && peak < 0.20,
                "Cue {:?} peak should be audible yet subtle (0.05..0.20), got {}",
                kind,
                peak
            );
            assert_eq!(buf[0], 0.0, "Cue should start cleanly at zero");
            assert_eq!(
                *buf.last().unwrap(),
                0.0,
                "Cue should end with trailing silence"
            );
        }
    }
}
