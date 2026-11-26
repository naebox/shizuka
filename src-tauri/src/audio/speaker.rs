use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use realfft::{RealFftPlanner, RealToComplex};
use ringbuf::traits::Consumer;
use tract_onnx::prelude::*;

use crate::audio::processor::DFN_HOP_SIZE;
use crate::config::{AppConfig, ConfigSaver};
use crate::diagnostics;

pub const SPEAKER_SAMPLE_RATE: usize = 16_000;
pub const SPEAKER_FRAME_SAMPLES_16K: usize = 160; // 10ms hop at 16kHz (exact 3:1 from 480 @ 48kHz)
pub const SPEAKER_EMBEDDING_DIM: usize = 192;
pub const SPEAKER_FBANK_BINS: usize = 80;
pub const DEFAULT_VOICE_GATE_THRESHOLD: f32 = 0.28;

/// Number of 0.90s near-field speech segments required before initial automatic voiceprint lock-in.
const AUTO_LEARN_MIN_SEGMENTS: usize = 10;
const AUTO_LEARN_FORCE_SEGMENTS: usize = 16;
const AUTO_LEARN_MAX_SEGMENTS: usize = 20;
const AUTO_LEARN_SEG_LEN_FRAMES: usize = 90; // 0.90s voiced window per candidate segment
const AUTO_LEARN_SEG_STRIDE_FRAMES: usize = 45; // 0.45s hop (~4.95s active speech for 10 segments)
const AUTO_LEARN_TOTAL_FRAMES_EST: f32 =
    (AUTO_LEARN_SEG_LEN_FRAMES + (AUTO_LEARN_MIN_SEGMENTS - 1) * AUTO_LEARN_SEG_STRIDE_FRAMES)
        as f32;

const CAMPPLUS_FP16_GZ_BYTES: &[u8] = include_bytes!("../../models/campplus_sv_16k_fp16.onnx.gz");

const FBANK_WIN_LEN: usize = 400; // 25ms @ 16kHz
const FBANK_OVERLAP_LEN: usize = FBANK_WIN_LEN - SPEAKER_FRAME_SAMPLES_16K; // 240 samples (15ms)
const FBANK_FFT_LEN: usize = 512;
const FBANK_NUM_BINS: usize = SPEAKER_FBANK_BINS;
const FBANK_PREEMPH: f32 = 0.97;

type TractRunnable = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

#[inline]
fn store_f32(atom: &AtomicU32, val: f32) {
    atom.store(val.to_bits(), Ordering::Relaxed);
}

#[inline]
fn load_f32(atom: &AtomicU32) -> f32 {
    f32::from_bits(atom.load(Ordering::Relaxed))
}

/// Fixed-size 10ms 16kHz frame sent from `shizuka-rt-dsp` to `shizuka-spk-id` via lock-free SPSC ring buffer.
#[derive(Clone, Copy)]
pub struct SpeakerRtFrame {
    pub samples_16k: [f32; SPEAKER_FRAME_SAMPLES_16K],
    pub speech_active: bool,
    pub mon_active: bool,
    pub lsnr_db: f32,
}

/// Zero-allocation 48 kHz -> 16 kHz (`3:1`) anti-aliased decimator for the real-time DSP thread.
/// Uses a 2nd-order Butterworth low-pass filter (`fc = 7200 Hz` @ `48 kHz`) prior to `3:1` decimation.
pub struct Downsampler48kTo16k {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Default for Downsampler48kTo16k {
    fn default() -> Self {
        Self::new()
    }
}

impl Downsampler48kTo16k {
    pub fn new() -> Self {
        let w0 = 2.0 * std::f64::consts::PI * 7_200.0 / 48_000.0;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;

        let b1 = (1.0 - cos_w0) / a0;
        let b0 = b1 * 0.5;
        let b2 = b0;
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
        }
    }

    #[inline]
    pub fn process_frame(
        &mut self,
        in_480: &[f32; DFN_HOP_SIZE],
        out_160: &mut [f32; SPEAKER_FRAME_SAMPLES_16K],
    ) {
        for (dst, chunk) in out_160.iter_mut().zip(in_480.as_chunks::<3>().0) {
            let mut y = 0.0_f64;
            for &s in chunk {
                let x = if s.is_finite() { s as f64 } else { 0.0 };
                y = self.b0 * x + self.z1;
                self.z1 = self.b1 * x - self.a1 * y + self.z2;
                self.z2 = self.b2 * x - self.a2 * y;
            }
            if self.z1.abs() < 1e-24 {
                self.z1 = 0.0;
            }
            if self.z2.abs() < 1e-24 {
                self.z2 = 0.0;
            }
            *dst = y as f32;
        }
    }
}

/// Lock-free atomic bridge between `shizuka-rt-dsp`, `shizuka-spk-id`, and UI telemetry.
pub struct SpeakerSharedState {
    pub gate_enabled: AtomicBool,
    pub has_profile: AtomicBool,
    pub threshold_bits: AtomicU32,
    pub speaker_pass: AtomicBool,
    pub gate_blocking: AtomicBool,
    pub similarity_bits: AtomicU32,
    pub learn_progress_bits: AtomicU32,
    pub model_ready: AtomicBool,
}

impl SpeakerSharedState {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let has_prof = cfg
            .voice_profile
            .as_ref()
            .is_some_and(|v| is_valid_embedding(v));
        Self {
            gate_enabled: AtomicBool::new(cfg.voice_gate_enabled),
            has_profile: AtomicBool::new(has_prof),
            threshold_bits: AtomicU32::new(cfg.voice_gate_threshold.clamp(0.16, 0.48).to_bits()),
            speaker_pass: AtomicBool::new(true),
            gate_blocking: AtomicBool::new(false),
            similarity_bits: AtomicU32::new(0.0f32.to_bits()),
            learn_progress_bits: AtomicU32::new(if has_prof { 1.0f32 } else { 0.0f32 }.to_bits()),
            model_ready: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn learn_progress(&self) -> f32 {
        load_f32(&self.learn_progress_bits)
    }

    #[inline]
    pub fn similarity(&self) -> f32 {
        load_f32(&self.similarity_bits)
    }

    #[inline]
    pub fn threshold(&self) -> f32 {
        load_f32(&self.threshold_bits)
    }

    #[inline]
    pub fn set_threshold(&self, thresh: f32) {
        store_f32(&self.threshold_bits, thresh.clamp(0.16, 0.48));
    }

    /// Returns `true` on the real-time thread when 16kHz frames should be pushed to `shizuka-spk-id`
    /// (both during automatic initial learning and during active speaker gating).
    #[inline]
    pub fn should_capture_frames(&self, nc_enabled: bool) -> bool {
        nc_enabled && self.gate_enabled.load(Ordering::Relaxed)
    }

    /// Returns `true` once automatic learning has locked onto the user's voiceprint and gating is live.
    #[inline]
    pub fn is_gate_engaged(&self, nc_enabled: bool) -> bool {
        nc_enabled
            && self.gate_enabled.load(Ordering::Relaxed)
            && self.has_profile.load(Ordering::Relaxed)
    }
}

#[inline]
pub fn is_valid_embedding(v: &[f32]) -> bool {
    if v.len() != SPEAKER_EMBEDDING_DIM {
        return false;
    }
    let norm_sq: f32 = v
        .iter()
        .map(|&x| if x.is_finite() { x * x } else { f32::NAN })
        .sum();
    norm_sq.is_finite() && norm_sq > 1e-6
}

#[inline]
pub fn is_valid_fbank_tilt(t: &[f32]) -> bool {
    t.len() == FBANK_NUM_BINS && t.iter().all(|x| x.is_finite() && x.abs() < 40.0)
}

/// Fallback zero-mean average speech spectral tilt (used only when upgrading an older `settings.json`
/// that has `voice_profile` without `voice_fbank_tilt`, until live speech updates it).
fn default_speech_fbank_tilt() -> [f32; FBANK_NUM_BINS] {
    let mut tilt = [0.0f32; FBANK_NUM_BINS];
    // Post-preemphasis average speech log-Mel envelope peaks gently around 400..1500 Hz (bins 12..42)
    // and rolls off gently toward low/high edges.
    let mut sum = 0.0f32;
    for (b, slot) in tilt.iter_mut().enumerate() {
        let x = b as f32 / (FBANK_NUM_BINS as f32);
        let v = 1.4 * (-((x - 0.34) / 0.28).powi(2)).exp() - 0.9 * x;
        *slot = v;
        sum += v;
    }
    let mean = sum / (FBANK_NUM_BINS as f32);
    for slot in &mut tilt {
        *slot -= mean;
    }
    tilt
}

pub fn l2_normalize(v: &mut [f32]) -> bool {
    let norm = v.iter().map(|&x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= 1e-8 {
        return false;
    }
    let inv = 1.0 / norm;
    for x in v.iter_mut() {
        *x *= inv;
    }
    true
}

#[inline]
pub fn round_vec_for_json(v: &[f32]) -> Vec<f32> {
    v.iter()
        .map(|&x| (x * 100_000.0).round() / 100_000.0)
        .collect()
}

#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na * nb).sqrt();
    if denom <= 1e-12 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

#[inline]
fn mel_scale(freq: f32) -> f32 {
    1127.0 * (1.0 + freq / 700.0).ln()
}

struct FbankWindowStats {
    means: [f32; FBANK_NUM_BINS],
    scalar_mean: f32,
    zero_mean_tilt: [f32; FBANK_NUM_BINS],
    /// Mean temporal standard deviation across core vocal-tract bins (bins `8..72`, ~`250..6400 Hz`).
    /// Multi-syllable speech typically yields `1.2 .. 3.0`; a held filler vowel (`"えー"`, `"うーん"`)
    /// yields `0.05 .. 0.55`.
    phonetic_spread: f32,
}

fn compute_fbank_window_stats(frames: &[[f32; FBANK_NUM_BINS]]) -> FbankWindowStats {
    let n = frames.len().max(1);
    let inv_n = 1.0 / (n as f32);

    let mut means = [0.0f32; FBANK_NUM_BINS];
    for fr in frames {
        for (m, &v) in means.iter_mut().zip(fr.iter()) {
            *m += v;
        }
    }
    let mut sum_means = 0.0f32;
    for m in &mut means {
        *m *= inv_n;
        sum_means += *m;
    }
    let scalar_mean = sum_means / (FBANK_NUM_BINS as f32);

    let mut zero_mean_tilt = [0.0f32; FBANK_NUM_BINS];
    for (dst, &m) in zero_mean_tilt.iter_mut().zip(means.iter()) {
        *dst = m - scalar_mean;
    }

    let mut sum_std = 0.0f32;
    const B_START: usize = 8;
    const B_END: usize = 72;
    for (b, &m) in means.iter().enumerate().take(B_END).skip(B_START) {
        let mut var = 0.0f32;
        for fr in frames {
            let d = fr[b] - m;
            var += d * d;
        }
        sum_std += (var * inv_n).sqrt();
    }
    let phonetic_spread = sum_std / ((B_END - B_START) as f32);

    FbankWindowStats {
        means,
        scalar_mean,
        zero_mean_tilt,
        phonetic_spread,
    }
}

/// Streaming Kaldi-compatible 80-bin log-Mel Filterbank extractor (16 kHz, 25ms Povey window, 10ms hop).
pub struct StreamingKaldiFbank80 {
    tail: [f32; FBANK_OVERLAP_LEN],
    povey: [f32; FBANK_WIN_LEN],
    bins: Vec<(usize, Vec<f32>)>,
    r2c: Arc<dyn RealToComplex<f32>>,
    fft_in: Vec<f32>,
    fft_out: Vec<realfft::num_complex::Complex<f32>>,
    fft_scratch: Vec<realfft::num_complex::Complex<f32>>,
}

impl Default for StreamingKaldiFbank80 {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingKaldiFbank80 {
    pub fn new() -> Self {
        let mut povey = [0.0f32; FBANK_WIN_LEN];
        let a = 2.0 * std::f64::consts::PI / ((FBANK_WIN_LEN - 1) as f64);
        for (n, w) in povey.iter_mut().enumerate() {
            *w = (0.5 - 0.5 * (a * n as f64).cos()).powf(0.85) as f32;
        }

        let sr = SPEAKER_SAMPLE_RATE as f32;
        let low_mel = mel_scale(20.0);
        let high_mel = mel_scale(sr * 0.5);
        let mel_delta = (high_mel - low_mel) / ((FBANK_NUM_BINS + 1) as f32);
        let fft_bin_width = sr / (FBANK_FFT_LEN as f32);

        let mut bins = Vec::with_capacity(FBANK_NUM_BINS);
        for b in 0..FBANK_NUM_BINS {
            let left_mel = low_mel + b as f32 * mel_delta;
            let center_mel = low_mel + (b + 1) as f32 * mel_delta;
            let right_mel = low_mel + (b + 2) as f32 * mel_delta;
            let mut first = None;
            let mut last = 0usize;
            let mut this_bin = [0.0f32; FBANK_FFT_LEN / 2];
            for (k, slot) in this_bin.iter_mut().enumerate() {
                let freq = k as f32 * fft_bin_width;
                let mel = mel_scale(freq);
                if mel > left_mel && mel < right_mel {
                    *slot = if mel <= center_mel {
                        (mel - left_mel) / (center_mel - left_mel)
                    } else {
                        (right_mel - mel) / (right_mel - center_mel)
                    };
                    if first.is_none() {
                        first = Some(k);
                    }
                    last = k;
                }
            }
            if let Some(f_idx) = first {
                bins.push((f_idx, this_bin[f_idx..=last].to_vec()));
            } else {
                bins.push((0, Vec::new()));
            }
        }

        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FBANK_FFT_LEN);
        let fft_in = r2c.make_input_vec();
        let fft_out = r2c.make_output_vec();
        let fft_scratch = r2c.make_scratch_vec();

        Self {
            tail: [0.0; FBANK_OVERLAP_LEN],
            povey,
            bins,
            r2c,
            fft_in,
            fft_out,
            fft_scratch,
        }
    }

    pub fn reset(&mut self) {
        self.tail.fill(0.0);
    }

    /// Consumes one 10ms (`160`-sample) 16 kHz frame and returns `(fbank_80, frame_rms_db)`.
    pub fn push_hop_160(
        &mut self,
        hop_160: &[f32; SPEAKER_FRAME_SAMPLES_16K],
    ) -> ([f32; FBANK_NUM_BINS], f32) {
        let mut window_raw = [0.0f32; FBANK_WIN_LEN];
        window_raw[..FBANK_OVERLAP_LEN].copy_from_slice(&self.tail);
        window_raw[FBANK_OVERLAP_LEN..].copy_from_slice(hop_160);

        self.tail
            .copy_from_slice(&window_raw[SPEAKER_FRAME_SAMPLES_16K..]);

        let mean: f32 = window_raw.iter().copied().sum::<f32>() / (FBANK_WIN_LEN as f32);
        let mut sq = 0.0f32;
        self.fft_in.fill(0.0);
        for (i, &raw_s) in window_raw.iter().enumerate() {
            let v = raw_s - mean;
            sq += v * v;
            self.fft_in[i] = v;
        }
        let rms_db = 10.0 * (sq / (FBANK_WIN_LEN as f32)).max(1e-12).log10();

        for i in (1..FBANK_WIN_LEN).rev() {
            self.fft_in[i] -= FBANK_PREEMPH * self.fft_in[i - 1];
        }
        self.fft_in[0] -= FBANK_PREEMPH * self.fft_in[0];
        for i in 0..FBANK_WIN_LEN {
            self.fft_in[i] *= self.povey[i];
        }

        let mut mel_frame = [0.0f32; FBANK_NUM_BINS];
        if self
            .r2c
            .process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.fft_scratch)
            .is_err()
        {
            return (mel_frame, -120.0);
        }

        let mut power = [0.0f32; FBANK_FFT_LEN / 2 + 1];
        for (k, c) in self.fft_out.iter().enumerate() {
            power[k] = c.re * c.re + c.im * c.im;
        }

        for (b, (offset, ws)) in self.bins.iter().enumerate() {
            let mut sum = 0.0f32;
            for (j, &w) in ws.iter().enumerate() {
                sum += w * power[offset + j];
            }
            mel_frame[b] = sum.max(f32::EPSILON).ln();
        }

        (mel_frame, rms_db)
    }
}

/// Speaker embedding extractor wrapping Apache-2.0 `3D-Speaker CAM++` (`192`-d) on `tract-onnx`.
pub struct SpeakerEmbeddingModel {
    runnable: TractRunnable,
}

impl SpeakerEmbeddingModel {
    pub fn load_bundled() -> Result<Self> {
        let mut gz = GzDecoder::new(Cursor::new(CAMPPLUS_FP16_GZ_BYTES));
        let mut proto = tract_onnx::onnx()
            .proto_model_for_read(&mut gz)
            .context("声の識別モデルの展開に失敗しました")?;

        // Unpack FP16-compressed weight initializers (`data_type == 10`) back to IEEE-754 f32 (`1`)
        if let Some(ref mut g) = proto.graph {
            for init in &mut g.initializer {
                if init.data_type == 10 && !init.raw_data.is_empty() {
                    let mut f32_bytes = Vec::with_capacity(init.raw_data.len() * 2);
                    for &b in init.raw_data.as_chunks::<2>().0 {
                        let v = half::f16::from_le_bytes(b).to_f32();
                        f32_bytes.extend_from_slice(&v.to_le_bytes());
                    }
                    init.raw_data = f32_bytes;
                    init.data_type = 1;
                }
            }
        }

        let mut model = tract_onnx::onnx()
            .model_for_proto_model(&proto)
            .context("声の識別モデルのグラフ構築に失敗しました")?;
        let t_sym = model.symbol_table.sym("T");
        model
            .set_input_fact(
                0,
                InferenceFact::dt_shape(
                    f32::datum_type(),
                    tvec!(1.into(), TDim::Sym(t_sym), (FBANK_NUM_BINS as i64).into()),
                ),
            )
            .context("声の識別モデルの入力シェイプ設定に失敗しました")?;

        let runnable = model
            .into_optimized()
            .context("声の識別モデルの最適化に失敗しました")?
            .into_runnable()
            .context("声の識別モデルの実行プラン構築に失敗しました")?;

        Ok(Self { runnable })
    }

    /// Computes an L2-normalized `192`-dim speaker embedding from `frames` using **Phonetic-Spread-Aware CMN**:
    /// - On multi-phoneme speech (`phonetic_spread >= 1.25`), uses standard Kaldi window `global-mean` CMN.
    /// - On sustained stationary vowels (`"えー"`, `"うーん"`, `"あー"`, where `phonetic_spread` is low),
    ///   smoothly anchors CMN to `scalar_mean + baseline_tilt[b]` so subtracting the window mean NEVER
    ///   wipes out the sustained vowel's formants and pitch harmonics!
    fn extract_embedding_with_tilt(
        &self,
        frames: &[[f32; FBANK_NUM_BINS]],
        baseline_tilt: Option<&[f32; FBANK_NUM_BINS]>,
    ) -> Result<([f32; SPEAKER_EMBEDDING_DIM], FbankWindowStats)> {
        if frames.len() < 15 {
            return Err(anyhow!("音声フレームが短すぎます ({} frames)", frames.len()));
        }
        let n = frames.len();
        let stats = compute_fbank_window_stats(frames);

        let mut cmn_mean = [0.0f32; FBANK_NUM_BINS];
        if let Some(lt_tilt) = baseline_tilt {
            let alpha_win = ((stats.phonetic_spread - 0.50) / 0.75).clamp(0.0, 1.0);
            for b in 0..FBANK_NUM_BINS {
                cmn_mean[b] = stats.scalar_mean
                    + alpha_win * stats.zero_mean_tilt[b]
                    + (1.0 - alpha_win) * lt_tilt[b];
            }
        } else {
            cmn_mean = stats.means;
        }

        let mut flat = Vec::with_capacity(n * FBANK_NUM_BINS);
        for fr in frames {
            for (b, &val) in fr.iter().enumerate() {
                flat.push(val - cmn_mean[b]);
            }
        }

        let input = tract_ndarray::Array3::from_shape_vec((1, n, FBANK_NUM_BINS), flat)
            .context("特徴量テンソル生成エラー")?
            .into_tensor();
        let outputs = self
            .runnable
            .run(tvec!(input.into()))
            .context("声の識別推論エラー")?;
        let view = outputs[0]
            .to_array_view::<f32>()
            .context("特徴ベクトル読み取りエラー")?;

        if view.len() != SPEAKER_EMBEDDING_DIM {
            return Err(anyhow!(
                "想定外の特徴ベクトル次元: {} (expected {})",
                view.len(),
                SPEAKER_EMBEDDING_DIM
            ));
        }

        let mut emb = [0.0f32; SPEAKER_EMBEDDING_DIM];
        for (dst, &src) in emb.iter_mut().zip(view.iter()) {
            *dst = if src.is_finite() { src } else { 0.0 };
        }
        if !l2_normalize(&mut emb) {
            return Err(anyhow!("無音のため声の特徴を抽出できませんでした"));
        }
        Ok((emb, stats))
    }
}

#[derive(Clone)]
struct CandidateSegment {
    emb: [f32; SPEAKER_EMBEDDING_DIM],
    tilt: [f32; FBANK_NUM_BINS],
    quality: f32,
}

/// Unsupervised online voiceprint learner with dominant-cluster medoid selection.
/// Collects clear, articulated near-field speech segments during normal conversation and extracts
/// both the primary speaker's `192`-d centroid and `80`-d baseline spectral tilt while rejecting
/// background/TV voices and stationary hums.
struct AutoVoiceLearner {
    frame_buf: VecDeque<([f32; FBANK_NUM_BINS], f32, f32)>,
    frames_since_seg: usize,
    segments: VecDeque<CandidateSegment>,
}

impl AutoVoiceLearner {
    /// Pairwise seed threshold (`E[e_i · e_j] ≈ α² ≈ 0.35..0.45` across disjoint 0.9s phrases
    /// of different words from the same human speaker, vs `~0.05..0.15` for different speakers).
    const SEED_PAIRWISE_SIM: f32 = 0.22;
    /// Centroid inlier threshold (`E[e_j · centroid] ≈ α ≈ 0.55..0.78` for same speaker).
    const CENTROID_INLIER_SIM: f32 = 0.38;
    const MIN_LEARN_PHONETIC_SPREAD: f32 = 0.22;

    fn new() -> Self {
        Self {
            frame_buf: VecDeque::with_capacity(AUTO_LEARN_SEG_LEN_FRAMES + 4),
            frames_since_seg: 0,
            segments: VecDeque::with_capacity(AUTO_LEARN_MAX_SEGMENTS + 2),
        }
    }

    fn reset(&mut self, shared: &SpeakerSharedState) {
        self.frame_buf.clear();
        self.frames_since_seg = 0;
        self.segments.clear();
        store_f32(&shared.learn_progress_bits, 0.0);
    }

    /// Feeds one 10ms frame during initial learning. Returns `Some((profile, baseline_tilt))`
    /// once a clear dominant near-field speaker cluster locks in.
    fn step(
        &mut self,
        fbank: [f32; FBANK_NUM_BINS],
        rms_db: f32,
        lsnr_db: f32,
        speech_active: bool,
        model: &SpeakerEmbeddingModel,
        shared: &SpeakerSharedState,
    ) -> Option<([f32; SPEAKER_EMBEDDING_DIM], [f32; FBANK_NUM_BINS])> {
        if !speech_active || rms_db < -54.0 || lsnr_db < -3.5 {
            return None;
        }

        if self.frame_buf.len() == AUTO_LEARN_SEG_LEN_FRAMES {
            self.frame_buf.pop_front();
        }
        self.frame_buf.push_back((fbank, rms_db, lsnr_db));
        self.frames_since_seg += 1;

        let collected_frames = if self.segments.is_empty() {
            self.frame_buf.len()
        } else {
            AUTO_LEARN_SEG_LEN_FRAMES
                + (self.segments.len() - 1) * AUTO_LEARN_SEG_STRIDE_FRAMES
                + self.frames_since_seg.min(AUTO_LEARN_SEG_STRIDE_FRAMES)
        };
        let raw_prog = collected_frames as f32 / AUTO_LEARN_TOTAL_FRAMES_EST;
        let prog = if raw_prog <= 0.90 {
            raw_prog
        } else {
            // Smooth asymptotic progress 90% -> 99% if gathering a few extra consensus segments
            (0.90 + 0.09 * ((raw_prog - 0.90) / 0.60).clamp(0.0, 1.0)).clamp(0.0, 0.99)
        };
        store_f32(&shared.learn_progress_bits, prog);

        let ready_for_seg = self.frame_buf.len() == AUTO_LEARN_SEG_LEN_FRAMES
            && (self.segments.is_empty() || self.frames_since_seg >= AUTO_LEARN_SEG_STRIDE_FRAMES);
        if !ready_for_seg {
            return None;
        }
        self.frames_since_seg = 0;

        let mut raw_frames = Vec::with_capacity(AUTO_LEARN_SEG_LEN_FRAMES);
        let mut sum_rms = 0.0f32;
        let mut sum_lsnr = 0.0f32;
        for &(ref fb, r, l) in &self.frame_buf {
            raw_frames.push(*fb);
            sum_rms += r;
            sum_lsnr += l;
        }

        let Ok((emb, stats)) = model.extract_embedding_with_tilt(&raw_frames, None) else {
            return None;
        };
        if stats.phonetic_spread < Self::MIN_LEARN_PHONETIC_SPREAD {
            return None;
        }

        let inv = 1.0 / (AUTO_LEARN_SEG_LEN_FRAMES as f32);
        let avg_rms = sum_rms * inv;
        let avg_lsnr = sum_lsnr * inv;
        let quality = ((avg_rms + 55.0).clamp(1.0, 45.0)) + (avg_lsnr.clamp(0.0, 30.0) * 0.8);

        if self.segments.len() == AUTO_LEARN_MAX_SEGMENTS {
            self.segments.pop_front();
        }
        self.segments.push_back(CandidateSegment {
            emb,
            tilt: stats.zero_mean_tilt,
            quality,
        });

        Self::try_extract_dominant_centroid(&self.segments)
    }

    /// Two-stage Medoid-to-Centroid clustering on the unit hypersphere:
    /// 1. For each candidate segment `i`, forms a tentative neighborhood centroid `c_i` from all
    ///    segments `j` with pairwise similarity `>= SEED_PAIRWISE_SIM (0.22)` (since two disjoint
    ///    0.9s phrases of different words have expected pairwise dot product `α² ≈ 0.35`, whereas
    ///    their dot product with the multi-phrase centroid `c_i` jumps up to `α ≈ 0.62+`).
    /// 2. Scores `c_i` by quality-weighted inliers (`sim(e_j, c_i) >= inlier_thresh`), locking in
    ///    once `>= 60%` majority agrees (or guaranteed lock-in on the dominant cluster by
    ///    `AUTO_LEARN_FORCE_SEGMENTS` so learning can never stall at 96%).
    fn try_extract_dominant_centroid(
        segments: &VecDeque<CandidateSegment>,
    ) -> Option<([f32; SPEAKER_EMBEDDING_DIM], [f32; FBANK_NUM_BINS])> {
        let total = segments.len();
        if total < AUTO_LEARN_MIN_SEGMENTS {
            return None;
        }

        let inlier_thresh = if total >= 14 {
            0.32
        } else {
            Self::CENTROID_INLIER_SIM
        };

        let mut best_seed_centroid = [0.0f32; SPEAKER_EMBEDDING_DIM];
        let mut best_inliers = 0usize;
        let mut best_score = -1.0f32;

        for cand_i in segments.iter() {
            // Stage 1: build tentative local centroid around `cand_i`
            let mut seed_c = [0.0f32; SPEAKER_EMBEDDING_DIM];
            for cand_j in segments.iter() {
                let p_sim = cosine_similarity(&cand_i.emb, &cand_j.emb);
                if p_sim >= Self::SEED_PAIRWISE_SIM {
                    let w = cand_j.quality.max(1.0) * p_sim.max(0.25);
                    for (dst, &src) in seed_c.iter_mut().zip(cand_j.emb.iter()) {
                        *dst += src * w;
                    }
                }
            }
            if !l2_normalize(&mut seed_c) {
                continue;
            }

            // Stage 2: evaluate all segments against the tentative multi-phrase centroid `seed_c`
            let mut inliers = 0usize;
            let mut score = 0.0f32;
            for cand_j in segments.iter() {
                let c_sim = cosine_similarity(&seed_c, &cand_j.emb);
                if c_sim >= inlier_thresh {
                    inliers += 1;
                    score += c_sim * cand_j.quality.max(1.0);
                }
            }
            if score > best_score {
                best_score = score;
                best_inliers = inliers;
                best_seed_centroid = seed_c;
            }
        }

        let majority_ok = best_inliers >= 6 && best_inliers * 10 >= total * 6;
        let force_ok = total >= AUTO_LEARN_FORCE_SEGMENTS && best_inliers >= 3;
        if !majority_ok && !force_ok {
            return None;
        }

        let final_thresh = if force_ok && !majority_ok {
            0.26
        } else {
            inlier_thresh
        };

        let mut centroid = [0.0f32; SPEAKER_EMBEDDING_DIM];
        let mut tilt_sum = [0.0f32; FBANK_NUM_BINS];
        let mut weight_total = 0.0f32;

        for cand in segments.iter() {
            let sim = cosine_similarity(&best_seed_centroid, &cand.emb);
            if sim >= final_thresh {
                let w = cand.quality.max(1.0) * sim.max(0.3);
                weight_total += w;
                for (c, &e) in centroid.iter_mut().zip(cand.emb.iter()) {
                    *c += e * w;
                }
                for (t_dst, &t_src) in tilt_sum.iter_mut().zip(cand.tilt.iter()) {
                    *t_dst += t_src * w;
                }
            }
        }

        if !l2_normalize(&mut centroid) || weight_total <= 0.0 {
            return None;
        }

        let inv_w = 1.0 / weight_total;
        let mut tilt_mean_check = 0.0f32;
        for t in &mut tilt_sum {
            *t *= inv_w;
            tilt_mean_check += *t;
        }
        let drift = tilt_mean_check / (FBANK_NUM_BINS as f32);
        for t in &mut tilt_sum {
            *t -= drift;
        }

        Some((centroid, tilt_sum))
    }
}

/// Real-time speech-turn state machine for `周囲の話し声カット`.
/// - Lets speech onsets (`0.0s..0.3s`) pass naturally so the user's first syllable is never clipped.
/// - Once an utterance is confirmed as the user, keeps the gate locked open for the entire unbroken
///   speech turn (`< 450ms` pauses) so casual/soft voice endings or sustained `"えー"` never drop out.
/// - Cuts unmistakable other-speaker voices within `~0.3s`, followed by a brief `0.55s` cooldown guard.
struct GateTracker {
    voiced_ring: VecDeque<[f32; FBANK_NUM_BINS]>,
    turn_confirmed_user: bool,
    silence_frames: usize,
    frames_since_eval: usize,
    consecutive_matches: u32,
    consecutive_rejects: u32,
    guarded_cooldown_frames: usize,
}

impl GateTracker {
    const MAX_RING_FRAMES: usize = 95; // 0.95s rolling context
    const FAST_ONSET_FRAMES: usize = 30; // First verification at 0.30s of active speech
    const PHRASE_SILENCE_RESET_FRAMES: usize = 45; // 450ms pause ends current speech turn
    const POST_REJECT_COOLDOWN_FRAMES: usize = 55; // Brief 0.55s guard ONLY after a clear other-speaker mismatch

    fn new() -> Self {
        Self {
            voiced_ring: VecDeque::with_capacity(Self::MAX_RING_FRAMES + 1),
            turn_confirmed_user: false,
            silence_frames: Self::PHRASE_SILENCE_RESET_FRAMES,
            frames_since_eval: 0,
            consecutive_matches: 0,
            consecutive_rejects: 0,
            guarded_cooldown_frames: 0,
        }
    }

    fn reset(&mut self, shared: &SpeakerSharedState) {
        self.voiced_ring.clear();
        self.turn_confirmed_user = false;
        self.silence_frames = Self::PHRASE_SILENCE_RESET_FRAMES;
        self.frames_since_eval = 0;
        self.consecutive_matches = 0;
        self.consecutive_rejects = 0;
        self.guarded_cooldown_frames = 0;
        shared.speaker_pass.store(true, Ordering::Relaxed);
        shared.gate_blocking.store(false, Ordering::Relaxed);
    }

    /// Evaluates one frame and returns `true` if `enrolled` / `baseline_tilt` updated via EMA.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        fbank: [f32; FBANK_NUM_BINS],
        rms_db: f32,
        lsnr_db: f32,
        speech_active: bool,
        mon_active: bool,
        enrolled: &mut [f32; SPEAKER_EMBEDDING_DIM],
        baseline_tilt: &mut [f32; FBANK_NUM_BINS],
        base_thresh: f32,
        model: &SpeakerEmbeddingModel,
        shared: &SpeakerSharedState,
    ) -> bool {
        let is_voiced = speech_active && rms_db > -58.0;

        if !is_voiced {
            self.silence_frames = self.silence_frames.saturating_add(1);
            if self.guarded_cooldown_frames > 0 {
                self.guarded_cooldown_frames -= 1;
            }
            if self.silence_frames >= 18 {
                shared.gate_blocking.store(false, Ordering::Relaxed);
            }
            if self.silence_frames == Self::PHRASE_SILENCE_RESET_FRAMES {
                self.voiced_ring.clear();
                self.turn_confirmed_user = false;
                self.frames_since_eval = 0;
                self.consecutive_matches = 0;
                self.consecutive_rejects = 0;
                if self.guarded_cooldown_frames == 0 {
                    shared.speaker_pass.store(true, Ordering::Relaxed);
                }
            } else if self.silence_frames > Self::PHRASE_SILENCE_RESET_FRAMES
                && self.guarded_cooldown_frames == 0
            {
                shared.speaker_pass.store(true, Ordering::Relaxed);
            }
            return false;
        }

        if self.silence_frames >= Self::PHRASE_SILENCE_RESET_FRAMES {
            self.voiced_ring.clear();
            self.turn_confirmed_user = false;
            self.frames_since_eval = 0;
            self.consecutive_matches = 0;
            self.consecutive_rejects = 0;
            if self.guarded_cooldown_frames == 0 {
                shared.speaker_pass.store(true, Ordering::Relaxed);
                shared.gate_blocking.store(false, Ordering::Relaxed);
            } else {
                shared.gate_blocking.store(true, Ordering::Relaxed);
            }
        }
        self.silence_frames = 0;

        if self.voiced_ring.len() == Self::MAX_RING_FRAMES {
            self.voiced_ring.pop_front();
        }
        self.voiced_ring.push_back(fbank);
        self.frames_since_eval += 1;

        let cur_pass = shared.speaker_pass.load(Ordering::Relaxed);

        // Once the current unbroken speech turn (`< 450ms` pauses) has been verified as the user,
        // keep the gate locked OPEN for the remainder of this speech turn (only checking lazily
        // every 60 frames = 600ms for passive EMA adaptation when Mic Test is off, NEVER cutting mid-sentence!).
        if self.turn_confirmed_user && cur_pass {
            if mon_active || self.frames_since_eval < 60 || self.voiced_ring.len() < 75 {
                return false;
            }
            self.frames_since_eval = 0;
            let eval_frames: Vec<[f32; FBANK_NUM_BINS]> = self.voiced_ring.iter().copied().collect();
            let Ok((cur_emb, eval_stats)) =
                model.extract_embedding_with_tilt(&eval_frames, Some(baseline_tilt))
            else {
                return false;
            };
            let sim = cosine_similarity(enrolled, &cur_emb);
            store_f32(&shared.similarity_bits, sim);

            let adapt_thresh = (base_thresh + 0.24).max(0.55);
            if eval_stats.phonetic_spread >= 0.80
                && rms_db > -48.0
                && lsnr_db > 2.0
                && sim >= adapt_thresh
            {
                for (dst, &src) in enrolled.iter_mut().zip(cur_emb.iter()) {
                    *dst = 0.985 * *dst + 0.015 * src;
                }
                let _ = l2_normalize(enrolled);

                let mut tilt_sum = 0.0f32;
                for (t_dst, &t_src) in baseline_tilt.iter_mut().zip(eval_stats.zero_mean_tilt.iter())
                {
                    *t_dst = 0.98 * *t_dst + 0.02 * t_src;
                    tilt_sum += *t_dst;
                }
                let t_mean = tilt_sum / (FBANK_NUM_BINS as f32);
                for t_dst in baseline_tilt.iter_mut() {
                    *t_dst -= t_mean;
                }
                return true;
            }
            return false;
        }

        let ring_len = self.voiced_ring.len();
        if ring_len < Self::FAST_ONSET_FRAMES {
            return false;
        }

        let eval_interval = if !cur_pass {
            16 // 160ms re-check while blocked so user entry re-opens briskly
        } else {
            18 // 180ms confirmation check
        };

        let should_eval =
            ring_len == Self::FAST_ONSET_FRAMES || self.frames_since_eval >= eval_interval;
        if !should_eval {
            return false;
        }
        self.frames_since_eval = 0;

        let max_win = if !cur_pass { 55 } else { Self::MAX_RING_FRAMES };
        let take_n = ring_len.min(max_win);
        let skip_n = ring_len - take_n;
        let eval_frames: Vec<[f32; FBANK_NUM_BINS]> =
            self.voiced_ring.iter().skip(skip_n).copied().collect();

        let Ok((cur_emb, raw_stats)) =
            model.extract_embedding_with_tilt(&eval_frames, Some(baseline_tilt))
        else {
            return false;
        };

        let sim = cosine_similarity(enrolled, &cur_emb);
        store_f32(&shared.similarity_bits, sim);

        let is_sustained_vowel = raw_stats.phonetic_spread < 0.72;
        let len_relax = if take_n < 42 {
            0.08
        } else if take_n < 68 {
            0.04
        } else {
            0.0
        };
        let vowel_relax = if raw_stats.phonetic_spread < 0.85 {
            ((0.85 - raw_stats.phonetic_spread) / 0.60).clamp(0.0, 1.0) * 0.10
        } else {
            0.0
        };
        // During Live Mic Test (`mon_active`), speaker playback in the room can add comb-filtering;
        // add a small 0.04 cushion so speaker sidetone never causes false self-rejection.
        let mon_relax = if mon_active { 0.04 } else { 0.0 };
        let eff_thresh = (base_thresh - len_relax - vowel_relax - mon_relax).max(0.10);

        if sim >= eff_thresh {
            self.consecutive_matches = self.consecutive_matches.saturating_add(1);
            self.consecutive_rejects = 0;
            self.guarded_cooldown_frames = 0;
            shared.speaker_pass.store(true, Ordering::Relaxed);
            shared.gate_blocking.store(false, Ordering::Relaxed);

            // Lock in this speech turn as confirmed user!
            self.turn_confirmed_user = true;
            false
        } else {
            let clear_stranger_margin = if is_sustained_vowel { 0.12 } else { 0.08 };
            if sim < eff_thresh - clear_stranger_margin {
                // Unmistakable other-speaker voice (e.g. sim < ~0.12) -> mute immediately (~0.30s)
                self.consecutive_rejects = self.consecutive_rejects.saturating_add(1);
                self.consecutive_matches = 0;
                self.guarded_cooldown_frames = Self::POST_REJECT_COOLDOWN_FRAMES;
                shared.speaker_pass.store(false, Ordering::Relaxed);
                shared.gate_blocking.store(true, Ordering::Relaxed);
                while self.voiced_ring.len() > 38 {
                    self.voiced_ring.pop_front();
                }
            } else {
                // Borderline zone: require 2 consecutive misses (~0.48s) before muting, and do NOT
                // set `guarded_cooldown_frames` so the next phrase still gets clean 0ms onset!
                self.consecutive_rejects = self.consecutive_rejects.saturating_add(1);
                self.consecutive_matches = 0;
                let needed_misses = if is_sustained_vowel { 3 } else { 2 };
                if !cur_pass || self.consecutive_rejects >= needed_misses {
                    shared.speaker_pass.store(false, Ordering::Relaxed);
                    shared.gate_blocking.store(true, Ordering::Relaxed);
                }
            }
            false
        }
    }
}

enum SpeakerCmd {
    AttachConsumer(Option<ringbuf::HeapCons<SpeakerRtFrame>>),
    SetGateConfig {
        enabled: bool,
        threshold: f32,
    },
    ResetProfile,
    Shutdown(mpsc::SyncSender<()>),
}

#[derive(Clone)]
pub struct SpeakerHandle {
    cmd_tx: mpsc::Sender<SpeakerCmd>,
    worker_thread: thread::Thread,
    pub shared: Arc<SpeakerSharedState>,
}

impl SpeakerHandle {
    pub fn spawn(
        config: Arc<Mutex<AppConfig>>,
        config_saver: Option<ConfigSaver>,
        config_revision: Arc<AtomicU64>,
    ) -> Self {
        let init_cfg = config.lock().map(|g| g.clone()).unwrap_or_default();
        let shared = Arc::new(SpeakerSharedState::from_config(&init_cfg));
        let (cmd_tx, cmd_rx) = mpsc::channel::<SpeakerCmd>();

        let shared_worker = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("shizuka-spk-id".into())
            .spawn(move || {
                speaker_worker_loop(
                    cmd_rx,
                    shared_worker,
                    config,
                    config_saver,
                    config_revision,
                    init_cfg,
                );
            })
            .expect("Failed to spawn speaker verification thread");

        let worker_thread = join.thread().clone();
        Self {
            cmd_tx,
            worker_thread,
            shared,
        }
    }

    #[inline]
    pub fn notify_frames_ready(&self) {
        self.worker_thread.unpark();
    }

    pub fn attach_consumer(&self, cons: Option<ringbuf::HeapCons<SpeakerRtFrame>>) {
        let _ = self.cmd_tx.send(SpeakerCmd::AttachConsumer(cons));
        self.worker_thread.unpark();
    }

    pub fn set_gate_config(&self, enabled: bool, threshold: f32) {
        let clamped = threshold.clamp(0.16, 0.48);
        self.shared.set_threshold(clamped);
        self.shared.gate_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.shared.speaker_pass.store(true, Ordering::Relaxed);
            self.shared.gate_blocking.store(false, Ordering::Relaxed);
        }
        let _ = self.cmd_tx.send(SpeakerCmd::SetGateConfig {
            enabled,
            threshold: clamped,
        });
        self.worker_thread.unpark();
    }

    pub fn reset_profile(&self) {
        self.shared.has_profile.store(false, Ordering::Relaxed);
        self.shared.speaker_pass.store(true, Ordering::Relaxed);
        self.shared.gate_blocking.store(false, Ordering::Relaxed);
        store_f32(&self.shared.similarity_bits, 0.0);
        store_f32(&self.shared.learn_progress_bits, 0.0);
        let _ = self.cmd_tx.send(SpeakerCmd::ResetProfile);
        self.worker_thread.unpark();
    }

    pub fn shutdown(&self, timeout: Duration) {
        let (tx, rx) = mpsc::sync_channel(1);
        if self.cmd_tx.send(SpeakerCmd::Shutdown(tx)).is_ok() {
            self.worker_thread.unpark();
            let _ = rx.recv_timeout(timeout);
        }
    }
}

/// Demotes the background speaker-ID ONNX thread below normal priority so heavy neural matrix
/// ops never preempt real-time `cpal` audio callbacks or `shizuka-rt-dsp`.
fn demote_speaker_worker_priority() {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> *mut core::ffi::c_void;
            fn SetThreadPriority(hThread: *mut core::ffi::c_void, nPriority: i32) -> i32;
        }
        const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe {
            let _ = pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
}

fn speaker_worker_loop(
    cmd_rx: mpsc::Receiver<SpeakerCmd>,
    shared: Arc<SpeakerSharedState>,
    config: Arc<Mutex<AppConfig>>,
    config_saver: Option<ConfigSaver>,
    config_revision: Arc<AtomicU64>,
    init_cfg: AppConfig,
) {
    demote_speaker_worker_priority();
    let mut model_opt: Option<SpeakerEmbeddingModel> = None;
    let mut fbank = StreamingKaldiFbank80::new();
    let mut learner = AutoVoiceLearner::new();
    let mut gate_tracker = GateTracker::new();
    let mut cons_opt: Option<ringbuf::HeapCons<SpeakerRtFrame>> = None;

    let mut enrolled_profile: Option<[f32; SPEAKER_EMBEDDING_DIM]> = init_cfg
        .voice_profile
        .filter(|v| is_valid_embedding(v))
        .and_then(|mut v| {
            if l2_normalize(&mut v) {
                v.try_into().ok()
            } else {
                None
            }
        });
    let mut baseline_tilt: [f32; FBANK_NUM_BINS] = init_cfg
        .voice_fbank_tilt
        .filter(|t| is_valid_fbank_tilt(t))
        .and_then(|t| t.try_into().ok())
        .unwrap_or_else(default_speech_fbank_tilt);

    let mut want_gate = init_cfg.voice_gate_enabled;
    let mut last_ema_persist = Instant::now();
    let mut ema_dirty = false;

    let ensure_model = |model_opt: &mut Option<SpeakerEmbeddingModel>,
                        shared: &SpeakerSharedState|
     -> bool {
        if model_opt.is_some() {
            return true;
        }
        diagnostics::info("Loading bundled CAM++ speaker model on background worker");
        match SpeakerEmbeddingModel::load_bundled() {
            Ok(m) => {
                *model_opt = Some(m);
                shared.model_ready.store(true, Ordering::Relaxed);
                diagnostics::info("CAM++ speaker model ready");
                true
            }
            Err(e) => {
                diagnostics::error(format!("Speaker model load failed: {:#}", e));
                false
            }
        }
    };

    let persist_profile = |prof: &[f32; SPEAKER_EMBEDDING_DIM],
                           tilt: &[f32; FBANK_NUM_BINS],
                           immediate: bool| {
        let vec_json = round_vec_for_json(prof);
        let tilt_json = round_vec_for_json(tilt);
        let snap = if let Ok(mut cfg) = config.lock() {
            cfg.voice_profile = Some(vec_json);
            cfg.voice_fbank_tilt = Some(tilt_json);
            Some(cfg.clone())
        } else {
            None
        };
        if let Some(cfg_snap) = snap {
            if let Some(ref saver) = config_saver {
                if immediate {
                    saver.flush_async(cfg_snap);
                } else {
                    saver.schedule(cfg_snap);
                }
            } else {
                let _ = cfg_snap.save();
            }
        }
    };

    if want_gate {
        let _ = ensure_model(&mut model_opt, &shared);
    }

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                SpeakerCmd::AttachConsumer(next_cons) => {
                    cons_opt = next_cons;
                    fbank.reset();
                    gate_tracker.reset(&shared);
                }
                SpeakerCmd::SetGateConfig { enabled, threshold } => {
                    shared.set_threshold(threshold);
                    want_gate = enabled;
                    shared.gate_enabled.store(enabled, Ordering::Relaxed);
                    if enabled {
                        let _ = ensure_model(&mut model_opt, &shared);
                    } else {
                        gate_tracker.reset(&shared);
                    }
                }
                SpeakerCmd::ResetProfile => {
                    enrolled_profile = None;
                    baseline_tilt = default_speech_fbank_tilt();
                    ema_dirty = false;
                    shared.has_profile.store(false, Ordering::Relaxed);
                    learner.reset(&shared);
                    gate_tracker.reset(&shared);
                    if want_gate {
                        let _ = ensure_model(&mut model_opt, &shared);
                    }
                }
                SpeakerCmd::Shutdown(ack) => {
                    if ema_dirty {
                        if let Some(ref prof) = enrolled_profile {
                            persist_profile(prof, &baseline_tilt, true);
                        }
                    }
                    let _ = ack.send(());
                    return;
                }
            }
        }

        let mut processed_any = false;
        if let Some(ref mut cons) = cons_opt {
            while let Some(rt_frame) = cons.try_pop() {
                processed_any = true;
                if !want_gate {
                    continue;
                }
                let Some(ref model) = model_opt else {
                    continue;
                };

                let (fbank_frame, rms_db) = fbank.push_hop_160(&rt_frame.samples_16k);

                if let Some(ref mut enrolled) = enrolled_profile {
                    let base_thresh = shared.threshold();
                    let adapted = gate_tracker.step(
                        fbank_frame,
                        rms_db,
                        rt_frame.lsnr_db,
                        rt_frame.speech_active,
                        rt_frame.mon_active,
                        enrolled,
                        &mut baseline_tilt,
                        base_thresh,
                        model,
                        &shared,
                    );
                    if adapted {
                        ema_dirty = true;
                        if last_ema_persist.elapsed() >= Duration::from_secs(180) {
                            persist_profile(enrolled, &baseline_tilt, false);
                            last_ema_persist = Instant::now();
                            ema_dirty = false;
                        }
                    }
                } else if let Some((learned_centroid, learned_tilt)) = learner.step(
                    fbank_frame,
                    rms_db,
                    rt_frame.lsnr_db,
                    rt_frame.speech_active,
                    model,
                    &shared,
                ) {
                    enrolled_profile = Some(learned_centroid);
                    baseline_tilt = learned_tilt;
                    shared.has_profile.store(true, Ordering::Relaxed);
                    store_f32(&shared.learn_progress_bits, 1.0);
                    gate_tracker.reset(&shared);
                    persist_profile(&learned_centroid, &baseline_tilt, true);
                    last_ema_persist = Instant::now();
                    ema_dirty = false;
                    config_revision.fetch_add(1, Ordering::Relaxed);
                    diagnostics::info(
                        "Primary user voiceprint and spectral tilt learned automatically",
                    );
                }
            }
        }

        if !processed_any {
            thread::park_timeout(Duration::from_millis(80));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_speaker_48k(
        duration_frames_10ms: usize,
        f0_hz: f32,
        glottal_tilt: f32,
        cadence_hz: f32,
        formant_pair: ([f32; 3], [f32; 3]),
        phase_offset_sec: f32,
    ) -> Vec<[f32; DFN_HOP_SIZE]> {
        let sr = 48_000.0f32;
        let mut phase = 0.0f32;
        let mut z_state = [(0.0f32, 0.0f32); 3];
        let mut lcg = 0x9E37_79B9u32;

        let mut out = Vec::with_capacity(duration_frames_10ms);
        let mut sample_idx = 0usize;
        for frame_idx in 0..duration_frames_10ms {
            let t_sec = frame_idx as f32 * 0.01 + phase_offset_sec;
            let blend = if cadence_hz <= 0.0 {
                0.0
            } else {
                0.5 + 0.5 * (2.0 * std::f32::consts::PI * cadence_hz * t_sec).sin()
            };

            let coeffs: [(f32, f32, f32); 3] = std::array::from_fn(|k| {
                let fc = formant_pair.0[k] * (1.0 - blend) + formant_pair.1[k] * blend;
                let bw = 75.0 + 30.0 * (k as f32);
                let r = (-std::f32::consts::PI * bw / sr).exp();
                let a1 = -2.0 * r * (2.0 * std::f32::consts::PI * fc / sr).cos();
                let a2 = r * r;
                let b0 = 1.0 - r;
                (b0, a1, a2)
            });

            let mut frame = [0.0f32; DFN_HOP_SIZE];
            for s in frame.iter_mut() {
                let t = sample_idx as f32 / sr + phase_offset_sec;
                let cur_f0 = if cadence_hz <= 0.0 {
                    // Held monotone filler ("えーーーー") with tiny natural vocal jitter
                    f0_hz * (1.0 + 0.008 * (2.0 * std::f32::consts::PI * 4.8 * t).sin())
                } else {
                    f0_hz * (1.0 + 0.05 * (2.0 * std::f32::consts::PI * cadence_hz * t).cos())
                };
                phase = (phase + cur_f0 / sr).fract();
                let mut glottal = 0.0f32;
                for h in 1..=32 {
                    let hf = h as f32 * cur_f0;
                    if hf < 7_600.0 {
                        glottal += (2.0 * std::f32::consts::PI * (h as f32) * phase).sin()
                            / (h as f32).powf(glottal_tilt);
                    }
                }
                lcg = lcg.wrapping_mul(1664525).wrapping_add(1013904223);
                let breath = ((lcg >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.03;
                let excite = glottal + breath;

                let mut vocal = 0.0f32;
                for (k, &(b0, a1, a2)) in coeffs.iter().enumerate() {
                    let (ref mut z1, ref mut z2) = z_state[k];
                    let y = b0 * excite - a1 * *z1 - a2 * *z2;
                    *z2 = *z1;
                    *z1 = y;
                    vocal += y;
                }
                let env = if cadence_hz <= 0.0 {
                    0.85
                } else {
                    0.65 + 0.35 * (2.0 * std::f32::consts::PI * cadence_hz * 1.5 * t).sin().abs()
                };
                *s = (vocal * env * 0.07).clamp(-0.85, 0.85);
                sample_idx += 1;
            }
            out.push(frame);
        }
        out
    }

    #[test]
    fn test_auto_learner_sustained_ee_filler_preservation_and_other_speaker_cut() {
        let model = SpeakerEmbeddingModel::load_bundled().expect("load bundled CAM++ model");
        let mut ds = Downsampler48kTo16k::new();
        let mut fbank = StreamingKaldiFbank80::new();
        let shared = SpeakerSharedState::from_config(&AppConfig::default());
        shared.gate_enabled.store(true, Ordering::Relaxed);

        let mut learner = AutoVoiceLearner::new();
        let spk_a_formants = ([380.0, 1020.0, 2180.0], [760.0, 1880.0, 2620.0]);
        let spk_a_speech = synth_speaker_48k(750, 110.0, 0.62, 2.6, spk_a_formants, 0.0);

        // Inject 2 outlier candidate segments into AutoVoiceLearner BEFORE Speaker A speaks;
        // medoid majority voting must exclude them!
        let mut outlier_emb = [0.0f32; SPEAKER_EMBEDDING_DIM];
        outlier_emb[0] = 1.0;
        learner.segments.push_back(CandidateSegment {
            emb: outlier_emb,
            tilt: [0.0; FBANK_NUM_BINS],
            quality: 5.0,
        });
        learner.segments.push_back(CandidateSegment {
            emb: outlier_emb,
            tilt: [0.0; FBANK_NUM_BINS],
            quality: 5.0,
        });

        let mut hop_16k = [0.0f32; SPEAKER_FRAME_SAMPLES_16K];
        let mut learned_opt = None;
        for fr48 in &spk_a_speech {
            ds.process_frame(fr48, &mut hop_16k);
            let (fb, rms_db) = fbank.push_hop_160(&hop_16k);
            if let Some(res) = learner.step(fb, rms_db, 16.0, true, &model, &shared) {
                learned_opt = Some(res);
                break;
            }
        }
        let (mut profile_a, mut tilt_a) =
            learned_opt.expect("AutoVoiceLearner should lock in user voiceprint and spectral tilt");
        assert!(cosine_similarity(&profile_a, &outlier_emb).abs() < 0.25);

        shared.has_profile.store(true, Ordering::Relaxed);
        let mut tracker = GateTracker::new();

        // 1. Mid-sentence sustained "えーーーー" (`cadence_hz = 0.0`, static /e/ formants for 2.5s!)
        //    after 0.6s of normal speech: MUST NEVER get muted!
        fbank.reset();
        let spk_a_intro = synth_speaker_48k(60, 112.0, 0.64, 2.5, spk_a_formants, 1.3);
        let ee_vowel_formants = ([530.0, 1840.0, 2480.0], [530.0, 1840.0, 2480.0]);
        let spk_a_held_ee = synth_speaker_48k(250, 110.0, 0.64, 0.0, ee_vowel_formants, 0.0);

        for (idx, fr48) in spk_a_intro.iter().chain(spk_a_held_ee.iter()).enumerate() {
            ds.process_frame(fr48, &mut hop_16k);
            let (fb, rms_db) = fbank.push_hop_160(&hop_16k);
            let _ = tracker.step(
                fb,
                rms_db,
                16.0,
                true,
                false,
                &mut profile_a,
                &mut tilt_a,
                DEFAULT_VOICE_GATE_THRESHOLD,
                &model,
                &shared,
            );
            assert!(
                shared.speaker_pass.load(Ordering::Relaxed),
                "Mid-turn held 'えー' must NEVER be muted (frame {}, sim={:.3})",
                idx,
                shared.similarity()
            );
        }

        // 500ms silence -> new speech turn starting DIRECTLY with a 2.0-second held "えーーーー" from silence!
        let silence_16k = [0.0f32; SPEAKER_FRAME_SAMPLES_16K];
        for _ in 0..50 {
            let (fb, rms_db) = fbank.push_hop_160(&silence_16k);
            let _ = tracker.step(
                fb,
                rms_db,
                -15.0,
                false,
                false,
                &mut profile_a,
                &mut tilt_a,
                DEFAULT_VOICE_GATE_THRESHOLD,
                &model,
                &shared,
            );
        }

        for (idx, fr48) in spk_a_held_ee.iter().take(200).enumerate() {
            ds.process_frame(fr48, &mut hop_16k);
            let (fb, rms_db) = fbank.push_hop_160(&hop_16k);
            let _ = tracker.step(
                fb,
                rms_db,
                16.0,
                true,
                false,
                &mut profile_a,
                &mut tilt_a,
                DEFAULT_VOICE_GATE_THRESHOLD,
                &model,
                &shared,
            );
            assert!(
                shared.speaker_pass.load(Ordering::Relaxed),
                "Turn-initial held 'えー' from silence must NEVER be muted (frame {}, sim={:.3})",
                idx,
                shared.similarity()
            );
        }

        // 500ms silence -> Non-matching speaker enters: still muted promptly at ~0.28s!
        for _ in 0..50 {
            let (fb, rms_db) = fbank.push_hop_160(&silence_16k);
            let _ = tracker.step(
                fb,
                rms_db,
                -15.0,
                false,
                false,
                &mut profile_a,
                &mut tilt_a,
                DEFAULT_VOICE_GATE_THRESHOLD,
                &model,
                &shared,
            );
        }

        let mut profile_other = [0.0f32; SPEAKER_EMBEDDING_DIM];
        for (i, slot) in profile_other.iter_mut().enumerate() {
            *slot = if i % 2 == 0 {
                profile_a[i + 1]
            } else {
                -profile_a[i - 1]
            };
        }
        assert!(l2_normalize(&mut profile_other));

        let mut muted_at_frame = None;
        for (idx, fr48) in spk_a_intro.iter().enumerate() {
            ds.process_frame(fr48, &mut hop_16k);
            let (fb, rms_db) = fbank.push_hop_160(&hop_16k);
            let _ = tracker.step(
                fb,
                rms_db,
                12.0,
                true,
                false,
                &mut profile_other,
                &mut tilt_a,
                DEFAULT_VOICE_GATE_THRESHOLD,
                &model,
                &shared,
            );
            if !shared.speaker_pass.load(Ordering::Relaxed) && muted_at_frame.is_none() {
                muted_at_frame = Some(idx);
            }
        }
        assert_eq!(muted_at_frame, Some(29));
        assert!(shared.gate_blocking.load(Ordering::Relaxed));
    }

    #[test]
    fn test_two_stage_centroid_clustering_never_stalls_at_96pct_on_diverse_phrases() {
        // Reproduces real-world conversational speech where disjoint 0.9s phrases of different
        // words have moderate pairwise similarity (~0.34, below old 0.48 pairwise threshold)
        // while aligning at ~0.60 with the speaker's true centroid.
        let mut true_user = [0.0f32; SPEAKER_EMBEDDING_DIM];
        true_user[0] = 1.0;

        let mut segs = VecDeque::new();
        // 2 early outlier segments from another speaker / TV
        for k in 0..2 {
            let mut tv = [0.0f32; SPEAKER_EMBEDDING_DIM];
            tv[50 + k] = 1.0;
            segs.push_back(CandidateSegment {
                emb: tv,
                tilt: [0.0; FBANK_NUM_BINS],
                quality: 8.0,
            });
        }
        // 10 diverse phrases from the user: each has cos(e_i, true_user) = 0.60 -> pairwise e_i·e_j = 0.36!
        // (Old single-stage pairwise >= 0.48 threshold had 0 neighbors and froze forever at 96%!)
        for i in 0..10 {
            let mut e = [0.0f32; SPEAKER_EMBEDDING_DIM];
            e[0] = 0.60;
            e[2 + i] = 0.80; // orthogonal per-phrase phonetic variation: 0.6^2 + 0.8^2 = 1.0
            segs.push_back(CandidateSegment {
                emb: e,
                tilt: [0.0; FBANK_NUM_BINS],
                quality: 25.0,
            });
        }

        let (centroid, _) = AutoVoiceLearner::try_extract_dominant_centroid(&segs)
            .expect("2-stage centroid clustering must lock in without stalling at 96%");
        assert!(
            cosine_similarity(&centroid, &true_user) > 0.90,
            "Learned centroid should closely recover true speaker direction (got {:.3})",
            cosine_similarity(&centroid, &true_user)
        );
    }
}
