use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::f32::consts::PI;
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
/// Plays subtle stereo "Shizuka" quartal/quintal harmonic blooms ("和の余白", A-5) to the user's
/// physical speaker/headphones (never the virtual mic) on a dedicated background thread and
/// releases the audio stream immediately after playback.
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
            log::warn!("Cue audio playback skipped: {}", err);
        }
    }
}

#[derive(Clone, Copy)]
struct PartialSpec {
    ratio: f32,
    amp: f32,
    decay: f32,
}

const VELVET_PARTIALS: &[PartialSpec] = &[
    PartialSpec {
        ratio: 1.0,
        amp: 1.0,
        decay: 0.0,
    },
    PartialSpec {
        ratio: 2.0,
        amp: 0.07,
        decay: 5.0,
    },
];

const PURE_PARTIALS: &[PartialSpec] = &[PartialSpec {
    ratio: 1.0,
    amp: 1.0,
    decay: 0.0,
}];

struct VoiceSpec<'a> {
    start_ms: f32,
    f_start: f32,
    f_end: f32,
    glide_rate: f32,
    duration_ms: f32,
    gain: f32,
    decay: f32,
    attack_ms: f32,
    release_ms: f32,
    pan: f32, // -1.0 (left) .. +1.0 (right)
    partials: &'a [PartialSpec],
}

#[inline]
fn cos_attack(i: usize, attack: usize) -> f32 {
    if i < attack {
        0.5 - 0.5 * (PI * i as f32 / attack as f32).cos()
    } else {
        1.0
    }
}

#[inline]
fn cos_release(i: usize, n: usize, release: usize) -> f32 {
    if i + release >= n {
        let rem = (n - 1 - i) as f32 / release as f32;
        0.5 - 0.5 * (PI * rem).cos()
    } else {
        1.0
    }
}

fn add_voice(left: &mut [f32], right: &mut [f32], sr: f32, spec: &VoiceSpec<'_>) {
    let offset = ((sr * spec.start_ms) / 1000.0).round() as usize;
    let n = (((sr * spec.duration_ms) / 1000.0).round() as usize).max(1);
    let attack = (((sr * spec.attack_ms) / 1000.0).round() as usize)
        .max(1)
        .min((n / 2).max(1));
    let release = (((sr * spec.release_ms) / 1000.0).round() as usize)
        .max(1)
        .min((n / 2).max(1));

    let pan_angle = (spec.pan.clamp(-1.0, 1.0) + 1.0) * (PI / 4.0);
    let gain_l = pan_angle.cos();
    let gain_r = pan_angle.sin();

    let mut phases = [0.0f32; 4];
    let p_count = spec.partials.len().min(phases.len());

    for i in 0..n {
        let idx = offset + i;
        if idx >= left.len() {
            break;
        }
        let u = i as f32 / n as f32;
        let freq = if (spec.f_end - spec.f_start).abs() > 1e-4 {
            spec.f_start + (spec.f_end - spec.f_start) * (1.0 - (-spec.glide_rate * u).exp())
        } else {
            spec.f_start
        };

        let mut sample = 0.0f32;
        for (p_idx, partial) in spec.partials.iter().take(p_count).enumerate() {
            phases[p_idx] += (2.0 * PI * freq * partial.ratio) / sr;
            let p_env = if partial.decay > 0.0 {
                (-partial.decay * u).exp()
            } else {
                1.0
            };
            sample += partial.amp * p_env * phases[p_idx].sin();
        }

        let env = cos_attack(i, attack) * (-spec.decay * u).exp() * cos_release(i, n, release);
        let out = sample * spec.gain * env;
        left[idx] += out * gain_l;
        right[idx] += out * gain_r;
    }
}

/// Applies subtle stereo early-reflection micro-reverb (velvet low-passed intimate room, ~95ms).
fn apply_micro_reverb(left: &[f32], right: &[f32], sr: f32, wet: f32) -> Vec<[f32; 2]> {
    const TAPS_L: [(f32, f32, usize); 6] = [
        (11.3, 0.34, 0),
        (19.7, 0.24, 1),
        (31.1, 0.17, 0),
        (47.3, 0.11, 1),
        (67.9, 0.07, 0),
        (89.1, 0.04, 1),
    ];
    const TAPS_R: [(f32, f32, usize); 6] = [
        (13.7, 0.32, 1),
        (23.1, 0.23, 0),
        (35.9, 0.16, 1),
        (52.7, 0.11, 0),
        (73.3, 0.06, 1),
        (94.7, 0.04, 0),
    ];

    let n = left.len();
    let mut out = vec![[0.0f32; 2]; n];

    let delays_l: [(usize, f32, usize); 6] = TAPS_L
        .map(|(ms, g, ch)| (((sr * ms) / 1000.0).round() as usize, g, ch));
    let delays_r: [(usize, f32, usize); 6] = TAPS_R
        .map(|(ms, g, ch)| (((sr * ms) / 1000.0).round() as usize, g, ch));

    // Sample-rate-invariant ~3.5 kHz 1-pole low-pass on wet reflections (~0.367 at 48 kHz)
    let lp = 1.0 - (-2.0 * PI * 3500.0 / sr).exp();
    let mut filt_l = 0.0f32;
    let mut filt_r = 0.0f32;

    for i in 0..n {
        let mut w_l = 0.0f32;
        for &(d, g, src_ch) in &delays_l {
            if i >= d {
                let s = if src_ch == 0 { left[i - d] } else { right[i - d] };
                w_l += s * g;
            }
        }
        let mut w_r = 0.0f32;
        for &(d, g, src_ch) in &delays_r {
            if i >= d {
                let s = if src_ch == 0 { left[i - d] } else { right[i - d] };
                w_r += s * g;
            }
        }

        filt_l += lp * (w_l - filt_l);
        filt_r += lp * (w_r - filt_r);
        out[i] = [
            (left[i] + filt_l * wet).clamp(-0.95, 0.95),
            (right[i] + filt_r * wet).clamp(-0.95, 0.95),
        ];
    }

    // Smooth 22 ms tail fade-out to exact zero
    let tail_fade = (((sr * 22.0) / 1000.0).round() as usize).max(1).min(n);
    let fade_start = n - tail_fade;
    for (offset, frame) in out[fade_start..].iter_mut().enumerate() {
        let k = (tail_fade - 1 - offset) as f32 / tail_fade as f32;
        frame[0] *= k;
        frame[1] *= k;
    }

    out
}

/// Synthesizes a subtle, click-free stereo harmonic bloom waveform ("和の余白", A-5) at `sample_rate`.
pub fn synthesize_cue_stereo(kind: CueKind, sample_rate: u32) -> Vec<[f32; 2]> {
    let sr = sample_rate.max(8_000) as f32;
    let body_len = (((sr * 370.0) / 1000.0).round() as usize).max(1);
    let mut left = vec![0.0f32; body_len];
    let mut right = vec![0.0f32; body_len];

    match kind {
        // Pure open quartal/quintal Zen stillness: D4 -> A4 + E5
        CueKind::NcOn => {
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 0.0,
                    f_start: 293.66,
                    f_end: 293.66,
                    glide_rate: 5.0,
                    duration_ms: 175.0,
                    gain: 0.14,
                    decay: 2.5,
                    attack_ms: 15.0,
                    release_ms: 24.0,
                    pan: -0.22,
                    partials: VELVET_PARTIALS,
                },
            );
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 58.0,
                    f_start: 440.0,
                    f_end: 440.0,
                    glide_rate: 5.0,
                    duration_ms: 235.0,
                    gain: 0.15,
                    decay: 2.7,
                    attack_ms: 16.0,
                    release_ms: 24.0,
                    pan: 0.16,
                    partials: VELVET_PARTIALS,
                },
            );
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 72.0,
                    f_start: 659.25,
                    f_end: 659.25,
                    glide_rate: 5.0,
                    duration_ms: 215.0,
                    gain: 0.085,
                    decay: 3.0,
                    attack_ms: 20.0,
                    release_ms: 24.0,
                    pan: 0.26,
                    partials: PURE_PARTIALS,
                },
            );
        }
        // Calm downward release: A4 -> E4
        CueKind::NcOff => {
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 0.0,
                    f_start: 440.0,
                    f_end: 440.0,
                    glide_rate: 5.0,
                    duration_ms: 145.0,
                    gain: 0.14,
                    decay: 2.8,
                    attack_ms: 13.0,
                    release_ms: 24.0,
                    pan: 0.18,
                    partials: VELVET_PARTIALS,
                },
            );
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 58.0,
                    f_start: 329.63,
                    f_end: 329.63,
                    glide_rate: 5.0,
                    duration_ms: 210.0,
                    gain: 0.14,
                    decay: 2.9,
                    attack_ms: 15.0,
                    release_ms: 24.0,
                    pan: -0.18,
                    partials: VELVET_PARTIALS,
                },
            );
        }
        // Single quiet descending 4th glide (unmistakable from 2-step NC)
        CueKind::MuteOn => {
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 0.0,
                    f_start: 349.23,
                    f_end: 261.63,
                    glide_rate: 6.0,
                    duration_ms: 155.0,
                    gain: 0.17,
                    decay: 3.4,
                    attack_ms: 8.0,
                    release_ms: 24.0,
                    pan: 0.0,
                    partials: PURE_PARTIALS,
                },
            );
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 12.0,
                    f_start: 261.63,
                    f_end: 261.63,
                    glide_rate: 5.0,
                    duration_ms: 165.0,
                    gain: 0.08,
                    decay: 3.6,
                    attack_ms: 14.0,
                    release_ms: 24.0,
                    pan: 0.0,
                    partials: PURE_PARTIALS,
                },
            );
        }
        // Calm ascending 5th glide — effortless lift
        CueKind::MuteOff => {
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 0.0,
                    f_start: 329.63,
                    f_end: 493.88,
                    glide_rate: 6.5,
                    duration_ms: 175.0,
                    gain: 0.16,
                    decay: 3.0,
                    attack_ms: 8.0,
                    release_ms: 24.0,
                    pan: 0.0,
                    partials: VELVET_PARTIALS,
                },
            );
            add_voice(
                &mut left,
                &mut right,
                sr,
                &VoiceSpec {
                    start_ms: 18.0,
                    f_start: 493.88,
                    f_end: 493.88,
                    glide_rate: 5.0,
                    duration_ms: 190.0,
                    gain: 0.09,
                    decay: 3.1,
                    attack_ms: 16.0,
                    release_ms: 24.0,
                    pan: 0.12,
                    partials: PURE_PARTIALS,
                },
            );
        }
    }

    let wet_body = apply_micro_reverb(&left, &right, sr, 0.30);

    // Brief leading & trailing silence so WASAPI/CoreAudio buffers open & drain without clipping
    let lead_silence = ((sr * 0.016).round() as usize).max(1);
    let tail_silence = ((sr * 0.080).round() as usize).max(1);
    let mut frames = Vec::with_capacity(lead_silence + wet_body.len() + tail_silence);
    frames.resize(lead_silence, [0.0, 0.0]);
    frames.extend_from_slice(&wet_body);
    frames.resize(frames.len() + tail_silence, [0.0, 0.0]);
    frames
}

/// Mono downmix helper for diagnostics & unit testing.
pub fn synthesize_cue_samples(kind: CueKind, sample_rate: u32) -> Vec<f32> {
    synthesize_cue_stereo(kind, sample_rate)
        .into_iter()
        .map(|[l, r]| 0.5 * (l + r))
        .collect()
}

fn play_cue_blocking(kind: CueKind, monitor_device: Option<&str>) -> anyhow::Result<()> {
    // Always target physical speakers/headphones (fallback_to_default=true prefers non-virtual output)
    let dev = find_output_device(monitor_device, true)
        .ok_or_else(|| anyhow::anyhow!("No physical output device available for cue audio"))?;
    let supported = dev.default_output_config()?;
    let fmt = supported.sample_format();
    let cfg: StreamConfig = supported.into();
    let sr = cfg.sample_rate;
    let ch = cfg.channels as usize;

    let frames = Arc::new(synthesize_cue_stereo(kind, sr));
    let expected_ms = ((frames.len() as u64) * 1000 / (sr.max(8_000) as u64)).max(250);
    let cursor = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));

    let stream = build_cue_stream(
        &dev,
        &cfg,
        fmt,
        ch,
        Arc::clone(&frames),
        Arc::clone(&cursor),
        Arc::clone(&done),
    )?;
    stream.play()?;

    let deadline = Instant::now() + Duration::from_millis(expected_ms + 220);
    while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(12));
    }
    // Drain wait so WASAPI / USB / wireless output buffers finish playing the reverb tail cleanly
    thread::sleep(Duration::from_millis(80));
    let _ = stream.pause();
    drop(stream);
    Ok(())
}

fn build_cue_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    frames_buf: Arc<Vec<[f32; 2]>>,
    cursor: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
) -> anyhow::Result<Stream> {
    match format {
        SampleFormat::F32 => {
            build_cue_typed::<f32>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::F64 => {
            build_cue_typed::<f64>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::I8 => {
            build_cue_typed::<i8>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::U8 => {
            build_cue_typed::<u8>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::I16 => {
            build_cue_typed::<i16>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::U16 => {
            build_cue_typed::<u16>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::I24 => {
            build_cue_typed::<cpal::I24>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::U24 => {
            build_cue_typed::<cpal::U24>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::I32 => {
            build_cue_typed::<i32>(device, config, channels, frames_buf, cursor, done)
        }
        SampleFormat::U32 => {
            build_cue_typed::<u32>(device, config, channels, frames_buf, cursor, done)
        }
        other => Err(anyhow::anyhow!("Unsupported cue output format: {:?}", other)),
    }
}

fn build_cue_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    frames_buf: Arc<Vec<[f32; 2]>>,
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
            let out_frames = data.len() / ch;
            let len = frames_buf.len();
            let mut idx = cursor.load(Ordering::Relaxed);

            for f in 0..out_frames {
                let [l, r] = if idx < len {
                    let s = frames_buf[idx];
                    idx += 1;
                    s
                } else {
                    [0.0, 0.0]
                };
                let base = f * ch;
                if ch == 1 {
                    data[base] = T::from_sample(0.5 * (l + r));
                } else {
                    data[base] = T::from_sample(l);
                    data[base + 1] = T::from_sample(r);
                    for c in 2..ch {
                        data[base + c] = zero;
                    }
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
            let stereo = synthesize_cue_stereo(kind, 48_000);
            assert!(
                stereo.len() > 18_000 && stereo.len() < 26_000,
                "Cue {:?} duration out of expected range: {} frames",
                kind,
                stereo.len()
            );
            let mut peak = 0.0f32;
            let mut stereo_diff = 0.0f32;
            for &[l, r] in &stereo {
                assert!(l.is_finite() && r.is_finite(), "Cue sample must be finite");
                peak = peak.max(l.abs()).max(r.abs());
                stereo_diff = stereo_diff.max((l - r).abs());
            }
            assert!(
                peak > 0.08 && peak < 0.35,
                "Cue {:?} peak should be audible yet subtle (0.08..0.35), got {}",
                kind,
                peak
            );
            assert!(
                stereo_diff > 0.005,
                "Cue {:?} should exhibit subtle stereo spatialization, max |L-R|={}",
                kind,
                stereo_diff
            );
            assert_eq!(stereo[0], [0.0, 0.0], "Cue should start cleanly at zero");
            assert_eq!(
                *stereo.last().unwrap(),
                [0.0, 0.0],
                "Cue should end with trailing silence"
            );

            let mono = synthesize_cue_samples(kind, 48_000);
            assert_eq!(mono.len(), stereo.len());
        }
    }
}
