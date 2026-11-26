use anyhow::{anyhow, Context, Result};
use audio_thread_priority::{
    demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use ringbuf::{traits::*, HeapRb};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::aec::AecRefSink;
use crate::audio::devices::{
    default_physical_input_name, default_physical_output_name, device_name, find_input_device,
    find_output_device, list_audio_devices,
};
use crate::audio::loopback::{build_aec_loopback_stream, LoopbackPipeline};
use crate::audio::processor::{soft_limit, NoiseProcessor, DFN_HOP_SIZE, DFN_SAMPLE_RATE};
use crate::audio::resampler::{AdaptiveOutputResampler, DriftController, InputResampler48k};
use crate::audio::speaker::{
    Downsampler48kTo16k, SpeakerHandle, SpeakerRtFrame, SpeakerSharedState,
    SPEAKER_FRAME_SAMPLES_16K,
};
use crate::config::{AppConfig, ConfigSaver, ModelVariant};
use crate::diagnostics;
use crate::i18n::{self, tr, Locale};

#[inline]
fn store_f32(atom: &AtomicU32, val: f32) {
    atom.store(val.to_bits(), Ordering::Relaxed);
}

#[inline]
fn load_f32(atom: &AtomicU32) -> f32 {
    f32::from_bits(atom.load(Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Running,
    WaitingDevice,
    Reconnecting,
    #[default]
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorSource {
    Pipeline,
    VirtualOutput,
    Monitor,
    Aec,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineDeviceStatus {
    pub connection_state: ConnectionState,
    pub waiting_device_name: Option<String>,
    pub active_input_name: Option<String>,
    pub active_output_name: Option<String>,
    pub active_monitor_name: Option<String>,
    pub active_aec_ref_name: Option<String>,
    pub input_sample_rate: u32,
    pub output_sample_rate: u32,
    pub reconnect_count: u32,
    pub input_drop_count: u64,
    pub output_underrun_count: u64,
    pub last_error: Option<String>,
    #[serde(skip)]
    pub(crate) last_error_source: Option<ErrorSource>,
}

#[derive(Debug, Clone, Copy)]
pub struct DspParams {
    pub atten_lim_db: f32,
    pub post_filter_beta: f32,
    pub input_gain_db: f32,
    pub min_db_thresh: f32,
    pub vad_gate_enabled: bool,
    pub vad_threshold_db: f32,
    pub hpf_enabled: bool,
    pub agc_enabled: bool,
    pub aec_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTelemetry {
    pub stream_running: bool,
    pub nc_enabled: bool,
    pub mic_muted: bool,
    pub model_variant: ModelVariant,
    pub vad_gate_enabled: bool,
    pub vad_speaking: bool,
    pub hpf_enabled: bool,
    pub agc_enabled: bool,
    pub agc_gain_db: f32,
    pub aec_enabled: bool,
    pub aec_active: bool,
    pub aec_erle_db: f32,
    pub voice_gate_enabled: bool,
    pub has_voice_profile: bool,
    pub voice_gate_blocking: bool,
    pub voice_gate_similarity: f32,
    pub voice_learn_progress: f32,
    pub clipping_active: bool,
    pub clip_count: u64,
    pub config_revision: u64,
    pub virtual_out_connected: bool,
    pub live_monitor_enabled: bool,
    pub monitor_clean: bool,
    pub monitor_volume: f32,
    pub monitor_rms_db: f32,
    pub input_rms_db: f32,
    pub output_rms_db: f32,
    pub noise_reduced_db: f32,
    pub lsnr_db: f32,
    pub drift_ppm: f32,
    pub latency_ms: f32,
    pub cpu_load_pct: f32,
    pub status: EngineDeviceStatus,
}

pub struct SharedAudioState {
    pub nc_enabled: AtomicBool,
    pub mic_muted: AtomicBool,
    pub model_variant_u8: AtomicU8,
    pub atten_lim_bits: AtomicU32,
    pub pf_beta_bits: AtomicU32,
    pub input_gain_bits: AtomicU32,
    pub min_db_thresh_bits: AtomicU32,
    pub vad_gate_enabled: AtomicBool,
    pub vad_threshold_bits: AtomicU32,
    pub vad_speaking: AtomicBool,
    pub hpf_enabled: AtomicBool,
    pub agc_enabled: AtomicBool,
    pub agc_gain_db_bits: AtomicU32,
    pub aec_enabled: AtomicBool,
    pub aec_active: AtomicBool,
    pub aec_erle_db_bits: AtomicU32,
    pub clipping_active: AtomicBool,
    pub clip_count: AtomicU64,
    pub config_revision: Arc<AtomicU64>,
    pub speaker: Arc<SpeakerSharedState>,
    pub live_monitor_enabled: AtomicBool,
    pub monitor_clean: AtomicBool,
    pub monitor_volume_bits: AtomicU32,

    pub stream_running: AtomicBool,
    pub virtual_out_connected: AtomicBool,
    pub input_rms_db_bits: AtomicU32,
    pub output_rms_db_bits: AtomicU32,
    pub monitor_rms_db_bits: AtomicU32,
    pub lsnr_db_bits: AtomicU32,
    pub drift_ppm_bits: AtomicU32,
    pub latency_ms_bits: AtomicU32,
    pub process_time_us: AtomicU32,

    pub input_callback_frames: AtomicU64,
    pub output_callback_frames: AtomicU64,
    pub input_drop_count: AtomicU64,
    pub output_drop_count: AtomicU64,
    pub output_underrun_count: AtomicU64,
    pub reconnect_count: AtomicU32,
    pub rt_priority_active: AtomicBool,
    pub dfn_error_count: AtomicU64,
    pub resampler_error_count: AtomicU64,
    pub observed_in_block_frames: AtomicU32,
    pub observed_out_block_frames: AtomicU32,

    pub status: Mutex<EngineDeviceStatus>,
}

fn variant_to_u8(v: ModelVariant) -> u8 {
    match v {
        ModelVariant::Standard => 0,
        ModelVariant::LowLatency => 1,
    }
}

fn u8_to_variant(v: u8) -> ModelVariant {
    match v {
        1 => ModelVariant::LowLatency,
        _ => ModelVariant::Standard,
    }
}

impl SharedAudioState {
    pub fn from_config_with_speaker(
        cfg: &AppConfig,
        config_revision: Arc<AtomicU64>,
        speaker: Arc<SpeakerSharedState>,
    ) -> Self {
        Self {
            nc_enabled: AtomicBool::new(cfg.nc_enabled),
            mic_muted: AtomicBool::new(false),
            model_variant_u8: AtomicU8::new(variant_to_u8(cfg.model_variant)),
            atten_lim_bits: AtomicU32::new(cfg.atten_lim_db.to_bits()),
            pf_beta_bits: AtomicU32::new(cfg.post_filter_beta.to_bits()),
            input_gain_bits: AtomicU32::new(cfg.input_gain_db.to_bits()),
            min_db_thresh_bits: AtomicU32::new(cfg.min_db_thresh.to_bits()),
            vad_gate_enabled: AtomicBool::new(cfg.vad_gate_enabled),
            vad_threshold_bits: AtomicU32::new(cfg.vad_threshold_db.to_bits()),
            vad_speaking: AtomicBool::new(false),
            hpf_enabled: AtomicBool::new(cfg.hpf_enabled),
            agc_enabled: AtomicBool::new(cfg.agc_enabled),
            agc_gain_db_bits: AtomicU32::new(0.0f32.to_bits()),
            aec_enabled: AtomicBool::new(cfg.aec_enabled),
            aec_active: AtomicBool::new(false),
            aec_erle_db_bits: AtomicU32::new(0.0f32.to_bits()),
            clipping_active: AtomicBool::new(false),
            clip_count: AtomicU64::new(0),
            config_revision,
            speaker,
            live_monitor_enabled: AtomicBool::new(false),
            monitor_clean: AtomicBool::new(true),
            monitor_volume_bits: AtomicU32::new(cfg.monitor_volume.clamp(0.0, 1.0).to_bits()),

            stream_running: AtomicBool::new(false),
            virtual_out_connected: AtomicBool::new(false),
            input_rms_db_bits: AtomicU32::new((-90.0f32).to_bits()),
            output_rms_db_bits: AtomicU32::new((-90.0f32).to_bits()),
            monitor_rms_db_bits: AtomicU32::new((-90.0f32).to_bits()),
            lsnr_db_bits: AtomicU32::new((-15.0f32).to_bits()),
            drift_ppm_bits: AtomicU32::new(0.0f32.to_bits()),
            latency_ms_bits: AtomicU32::new(0.0f32.to_bits()),
            process_time_us: AtomicU32::new(0),

            input_callback_frames: AtomicU64::new(0),
            output_callback_frames: AtomicU64::new(0),
            input_drop_count: AtomicU64::new(0),
            output_drop_count: AtomicU64::new(0),
            output_underrun_count: AtomicU64::new(0),
            reconnect_count: AtomicU32::new(0),
            rt_priority_active: AtomicBool::new(false),
            dfn_error_count: AtomicU64::new(0),
            resampler_error_count: AtomicU64::new(0),
            observed_in_block_frames: AtomicU32::new(0),
            observed_out_block_frames: AtomicU32::new(0),

            status: Mutex::new(EngineDeviceStatus::default()),
        }
    }

    pub fn apply_dsp_params(&self, params: DspParams) {
        store_f32(&self.atten_lim_bits, params.atten_lim_db);
        store_f32(&self.pf_beta_bits, params.post_filter_beta);
        store_f32(&self.input_gain_bits, params.input_gain_db);
        store_f32(&self.min_db_thresh_bits, params.min_db_thresh);
        self.vad_gate_enabled
            .store(params.vad_gate_enabled, Ordering::Relaxed);
        store_f32(&self.vad_threshold_bits, params.vad_threshold_db);
        self.hpf_enabled
            .store(params.hpf_enabled, Ordering::Relaxed);
        self.agc_enabled
            .store(params.agc_enabled, Ordering::Relaxed);
        self.aec_enabled
            .store(params.aec_enabled, Ordering::Relaxed);
    }

    pub fn apply_config_dsp(&self, cfg: &AppConfig) {
        self.apply_dsp_params(DspParams {
            atten_lim_db: cfg.atten_lim_db,
            post_filter_beta: cfg.post_filter_beta,
            input_gain_db: cfg.input_gain_db,
            min_db_thresh: cfg.min_db_thresh,
            vad_gate_enabled: cfg.vad_gate_enabled,
            vad_threshold_db: cfg.vad_threshold_db,
            hpf_enabled: cfg.hpf_enabled,
            agc_enabled: cfg.agc_enabled,
            aec_enabled: cfg.aec_enabled,
        });
    }

    pub fn snapshot(&self) -> AudioTelemetry {
        let in_db = load_f32(&self.input_rms_db_bits);
        let out_db = load_f32(&self.output_rms_db_bits);
        let nc_enabled = self.nc_enabled.load(Ordering::Relaxed);
        let mic_muted = self.mic_muted.load(Ordering::Relaxed);
        let reduced = if nc_enabled && !mic_muted {
            (in_db - out_db).max(0.0)
        } else {
            0.0
        };
        let proc_us = self.process_time_us.load(Ordering::Relaxed) as f32;
        // 10ms = 10,000 us budget per frame
        let cpu_load_pct = (proc_us / 10_000.0 * 100.0).clamp(0.0, 100.0);
        let mut status = self
            .status
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        status.reconnect_count = self.reconnect_count.load(Ordering::Relaxed);
        status.input_drop_count = self.input_drop_count.load(Ordering::Relaxed);
        status.output_underrun_count = self.output_underrun_count.load(Ordering::Relaxed);

        AudioTelemetry {
            stream_running: self.stream_running.load(Ordering::Relaxed),
            nc_enabled,
            mic_muted,
            model_variant: u8_to_variant(self.model_variant_u8.load(Ordering::Relaxed)),
            vad_gate_enabled: self.vad_gate_enabled.load(Ordering::Relaxed),
            vad_speaking: self.vad_speaking.load(Ordering::Relaxed),
            hpf_enabled: self.hpf_enabled.load(Ordering::Relaxed),
            agc_enabled: self.agc_enabled.load(Ordering::Relaxed),
            agc_gain_db: load_f32(&self.agc_gain_db_bits),
            aec_enabled: self.aec_enabled.load(Ordering::Relaxed),
            aec_active: self.aec_active.load(Ordering::Relaxed),
            aec_erle_db: load_f32(&self.aec_erle_db_bits),
            voice_gate_enabled: self.speaker.gate_enabled.load(Ordering::Relaxed),
            has_voice_profile: self.speaker.has_profile.load(Ordering::Relaxed),
            voice_gate_blocking: self.speaker.gate_blocking.load(Ordering::Relaxed),
            voice_gate_similarity: self.speaker.similarity(),
            voice_learn_progress: self.speaker.learn_progress(),
            clipping_active: self.clipping_active.load(Ordering::Relaxed),
            clip_count: self.clip_count.load(Ordering::Relaxed),
            config_revision: self.config_revision.load(Ordering::Relaxed),
            virtual_out_connected: self.virtual_out_connected.load(Ordering::Relaxed),
            live_monitor_enabled: self.live_monitor_enabled.load(Ordering::Relaxed),
            monitor_clean: self.monitor_clean.load(Ordering::Relaxed),
            monitor_volume: load_f32(&self.monitor_volume_bits),
            monitor_rms_db: load_f32(&self.monitor_rms_db_bits),
            input_rms_db: in_db,
            output_rms_db: out_db,
            noise_reduced_db: reduced,
            lsnr_db: load_f32(&self.lsnr_db_bits),
            drift_ppm: load_f32(&self.drift_ppm_bits),
            latency_ms: load_f32(&self.latency_ms_bits),
            cpu_load_pct,
            status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamKind {
    Input,
    VirtualOutput,
    Monitor,
    AecLoopback,
}

enum PipelineStartError {
    WaitingForDevice(Option<String>),
    Other(anyhow::Error),
}

enum EngineCommand {
    StartOrRestart {
        input_device: Option<String>,
        output_device: Option<String>,
        monitor_device: Option<String>,
        profile_already_applied: bool,
    },
    SetModelVariant(ModelVariant),
    SyncLastProfileMic(Option<String>),
    Stop(Option<mpsc::SyncSender<()>>),
    SetLiveMonitor {
        enabled: bool,
        monitor_device: Option<String>,
    },
    SetAecConfig {
        enabled: bool,
        reference_device: Option<String>,
    },
    StreamError {
        generation: u64,
        kind: StreamKind,
        message: String,
    },
}

#[derive(Clone)]
pub struct AudioEngineHandle {
    cmd_tx: mpsc::Sender<EngineCommand>,
    pub shared: Arc<SharedAudioState>,
    pub speaker: SpeakerHandle,
}

struct MonitorSink {
    prod: ringbuf::HeapProd<f32>,
    resampler: AdaptiveOutputResampler,
    drift: DriftController,
    target_frames: usize,
    sr_mon: u32,
}

struct RunningPipeline {
    stop_flag: Arc<AtomicBool>,
    worker_join: Option<JoinHandle<()>>,
    mon_tx: mpsc::Sender<Option<MonitorSink>>,
    aec_tx: mpsc::Sender<Option<AecRefSink>>,
    speaker: SpeakerHandle,
    active_input_name: String,
    _input_stream: Stream,
    _virtual_output_stream: Option<Stream>,
}

impl RunningPipeline {
    fn stop_inner(&mut self) {
        // Explicitly pause OS audio streams before dropping (belt-and-suspenders against
        // CoreAudio/WASAPI callbacks firing during teardown)
        let _ = self._input_stream.pause();
        if let Some(ref virt) = self._virtual_output_stream {
            let _ = virt.pause();
        }
        let _ = self.mon_tx.send(None);
        let _ = self.aec_tx.send(None);
        self.speaker.attach_consumer(None);
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(jh) = self.worker_join.take() {
            jh.thread().unpark();
            let _ = jh.join();
        }
    }

    fn stop(mut self) {
        self.stop_inner();
    }
}

impl Drop for RunningPipeline {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// RAII guard ensuring the real-time DSP worker thread is always stopped and joined
/// if `build_main_pipeline` fails before constructing `RunningPipeline`.
struct WorkerInitGuard {
    stop_flag: Arc<AtomicBool>,
    worker_join: Option<JoinHandle<()>>,
}

impl WorkerInitGuard {
    fn disarm(&mut self) -> Option<JoinHandle<()>> {
        self.worker_join.take()
    }
}

impl Drop for WorkerInitGuard {
    fn drop(&mut self) {
        if let Some(jh) = self.worker_join.take() {
            self.stop_flag.store(true, Ordering::SeqCst);
            jh.thread().unpark();
            let _ = jh.join();
        }
    }
}

struct MonitorPipeline {
    _stream: Stream,
}

impl Drop for MonitorPipeline {
    fn drop(&mut self) {
        let _ = self._stream.pause();
    }
}

impl AudioEngineHandle {
    pub fn spawn(config: Arc<Mutex<AppConfig>>, config_saver: Option<ConfigSaver>) -> Self {
        let init_cfg = config.lock().map(|g| g.clone()).unwrap_or_default();
        let config_rev = Arc::new(AtomicU64::new(0));
        let speaker = SpeakerHandle::spawn(
            Arc::clone(&config),
            config_saver,
            Arc::clone(&config_rev),
        );
        let shared = Arc::new(SharedAudioState::from_config_with_speaker(
            &init_cfg,
            config_rev,
            Arc::clone(&speaker.shared),
        ));
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>();

        let shared_ctrl = Arc::clone(&shared);
        let speaker_ctrl = speaker.clone();
        let ctrl_tx = cmd_tx.clone();

        thread::Builder::new()
            .name("shizuka-audio-ctrl".into())
            .spawn(move || {
                audio_control_loop(
                    cmd_rx,
                    ctrl_tx,
                    shared_ctrl,
                    speaker_ctrl,
                    config,
                    init_cfg,
                );
            })
            .expect("Failed to spawn audio controller thread");

        Self {
            cmd_tx,
            shared,
            speaker,
        }
    }

    pub fn start_or_restart(
        &self,
        input_device: Option<String>,
        output_device: Option<String>,
        monitor_device: Option<String>,
        profile_already_applied: bool,
    ) {
        let _ = self.cmd_tx.send(EngineCommand::StartOrRestart {
            input_device,
            output_device,
            monitor_device,
            profile_already_applied,
        });
    }

    pub fn sync_last_profile_mic(&self, mic_name: Option<String>) {
        let _ = self.cmd_tx.send(EngineCommand::SyncLastProfileMic(mic_name));
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(EngineCommand::Stop(None));
    }

    /// Stops all audio streams, joins the real-time DSP thread, and waits up to `timeout`
    /// for completion before returning (used for clean app exit & pre-update shutdown).
    pub fn stop_and_wait(&self, timeout: Duration) -> bool {
        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        let ok = if self.cmd_tx.send(EngineCommand::Stop(Some(ack_tx))).is_err() {
            true
        } else {
            ack_rx.recv_timeout(timeout).is_ok()
        };
        self.speaker.shutdown(Duration::from_millis(800));
        ok
    }

    pub fn set_nc_enabled(&self, enabled: bool) {
        self.shared.nc_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.shared.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_params(&self, params: DspParams) {
        self.shared.apply_dsp_params(params);
    }

    pub fn set_model_variant(&self, variant: ModelVariant) {
        let _ = self.cmd_tx.send(EngineCommand::SetModelVariant(variant));
    }

    pub fn set_live_monitor(&self, enabled: bool, monitor_device: Option<String>) {
        let _ = self.cmd_tx.send(EngineCommand::SetLiveMonitor {
            enabled,
            monitor_device,
        });
    }

    pub fn set_monitor_mode(&self, clean: bool) {
        self.shared.monitor_clean.store(clean, Ordering::Relaxed);
    }

    pub fn set_monitor_volume(&self, volume: f32) {
        store_f32(&self.shared.monitor_volume_bits, volume.clamp(0.0, 1.0));
    }

    pub fn set_aec_config(&self, enabled: bool, reference_device: Option<String>) {
        self.shared.aec_enabled.store(enabled, Ordering::Relaxed);
        let _ = self.cmd_tx.send(EngineCommand::SetAecConfig {
            enabled,
            reference_device,
        });
    }
}

#[derive(Clone)]
pub(crate) struct StreamErrorNotifier {
    tx: mpsc::Sender<EngineCommand>,
    generation: u64,
    kind: StreamKind,
    sent: Arc<AtomicBool>,
}

impl StreamErrorNotifier {
    fn new(tx: mpsc::Sender<EngineCommand>, generation: u64, kind: StreamKind) -> Self {
        Self {
            tx,
            generation,
            kind,
            sent: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn notify(&self, err: cpal::StreamError) {
        // Transient xrun notifications in cpal 0.17 should not tear down and restart the pipeline
        if matches!(err, cpal::StreamError::BufferUnderrun) {
            log::warn!(
                "Audio {:?} transient buffer underrun/overrun (gen {})",
                self.kind,
                self.generation
            );
            return;
        }
        log::error!(
            "Audio {:?} stream error (gen {}): {}",
            self.kind,
            self.generation,
            err
        );
        if !self.sent.swap(true, Ordering::Relaxed) {
            let _ = self.tx.send(EngineCommand::StreamError {
                generation: self.generation,
                kind: self.kind,
                message: err.to_string(),
            });
        }
    }
}

fn reconnect_backoff_duration(attempt: u32) -> Duration {
    match attempt {
        0 => Duration::from_millis(600),
        1 => Duration::from_millis(1200),
        2 => Duration::from_millis(2000),
        _ => Duration::from_millis(3000),
    }
}

fn virtual_reattach_backoff_duration(attempt: u32) -> Duration {
    match attempt {
        0 => Duration::from_secs(3),
        1 => Duration::from_secs(6),
        2 => Duration::from_secs(12),
        3 => Duration::from_secs(30),
        _ => Duration::from_secs(60),
    }
}

struct WatchdogState {
    retry_attempt: u32,
    next_retry_at: Option<Instant>,
    virt_reattach_attempt: u32,
    next_virt_reattach_at: Option<Instant>,
    aec_reattach_attempt: u32,
    next_aec_reattach_at: Option<Instant>,
    pipe_started_at: Instant,
    last_seen_in_frames: u64,
    last_seen_out_frames: u64,
    in_stall_ticks: u32,
    out_stall_ticks: u32,
    last_mon_reopen_at: Option<Instant>,
}

impl WatchdogState {
    fn new() -> Self {
        Self {
            retry_attempt: 0,
            next_retry_at: None,
            virt_reattach_attempt: 0,
            next_virt_reattach_at: None,
            aec_reattach_attempt: 0,
            next_aec_reattach_at: None,
            pipe_started_at: Instant::now(),
            last_seen_in_frames: 0,
            last_seen_out_frames: 0,
            in_stall_ticks: 0,
            out_stall_ticks: 0,
            last_mon_reopen_at: None,
        }
    }
}

fn reset_audio_meters(shared: &SharedAudioState) {
    store_f32(&shared.input_rms_db_bits, -90.0);
    store_f32(&shared.output_rms_db_bits, -90.0);
    store_f32(&shared.monitor_rms_db_bits, -90.0);
    store_f32(&shared.agc_gain_db_bits, 0.0);
    store_f32(&shared.aec_erle_db_bits, 0.0);
    shared.aec_active.store(false, Ordering::Relaxed);
    shared.speaker.gate_blocking.store(false, Ordering::Relaxed);
    shared.speaker.speaker_pass.store(true, Ordering::Relaxed);
    shared.vad_speaking.store(false, Ordering::Relaxed);
    shared.clipping_active.store(false, Ordering::Relaxed);
}

fn audio_control_loop(
    cmd_rx: mpsc::Receiver<EngineCommand>,
    cmd_tx: mpsc::Sender<EngineCommand>,
    shared: Arc<SharedAudioState>,
    speaker: SpeakerHandle,
    config: Arc<Mutex<AppConfig>>,
    init_cfg: AppConfig,
) {
    let mut main_pipe: Option<RunningPipeline> = None;
    let mut live_mon_pipe: Option<MonitorPipeline> = None;
    let mut aec_loop_pipe: Option<LoopbackPipeline> = None;

    let mut cur_in_dev = init_cfg.input_device.clone();
    let mut cur_out_dev = init_cfg.output_device.clone();
    let mut cur_mon_dev = init_cfg.monitor_device.clone();
    let mut cur_aec_ref_dev = init_cfg.aec_reference_device.clone();
    let mut want_running = init_cfg.stream_active;
    let mut want_mon = false;
    let mut last_profile_mic: Option<String> = None;

    let mut main_gen: u64 = 0;
    let mut mon_gen: u64 = 0;
    let mut aec_gen: u64 = 0;
    let mut wd = WatchdogState::new();
    let mut watchdog_ticks: u32 = 0;

    // Auto-select recommended virtual cable on first run if none configured
    if cur_out_dev.is_none() {
        let dev_list = list_audio_devices();
        if let Some(rec) = dev_list.recommended_virtual_output {
            cur_out_dev = Some(rec);
        }
    }

    let apply_live_monitor = |enabled: bool,
                              mon_name: Option<String>,
                              mon_gen: &mut u64,
                              main_pipe: &mut Option<RunningPipeline>,
                              live_mon_pipe: &mut Option<MonitorPipeline>| -> bool {
        // Detach previous monitor stream cleanly before opening a new one
        *mon_gen = mon_gen.wrapping_add(1);
        if let Some(pipe) = main_pipe.as_ref() {
            let _ = pipe.mon_tx.send(None);
        }
        let _ = live_mon_pipe.take();

        if !enabled {
            shared.live_monitor_enabled.store(false, Ordering::Relaxed);
            if let Ok(mut st) = shared.status.lock() {
                st.active_monitor_name = None;
            }
            return false;
        }

        if let Some(pipe) = main_pipe.as_ref() {
            let notifier =
                StreamErrorNotifier::new(cmd_tx.clone(), *mon_gen, StreamKind::Monitor);
            match build_live_monitor_stream(mon_name.as_deref(), notifier) {
                Ok((mon_pipe, sink, actual_mon_name)) => {
                    let _ = pipe.mon_tx.send(Some(sink));
                    *live_mon_pipe = Some(mon_pipe);
                    shared.live_monitor_enabled.store(true, Ordering::Relaxed);
                    if let Ok(mut st) = shared.status.lock() {
                        st.active_monitor_name = Some(actual_mon_name);
                        if st.last_error_source == Some(ErrorSource::Monitor) {
                            st.last_error = None;
                            st.last_error_source = None;
                        }
                    }
                    true
                }
                Err(e) => {
                    diagnostics::warn(format!("Live monitor failed to start: {:#}", e));
                    shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                    if let Ok(mut st) = shared.status.lock() {
                        st.active_monitor_name = None;
                        st.last_error = Some(format!("{:#}", e));
                        st.last_error_source = Some(ErrorSource::Monitor);
                    }
                    false
                }
            }
        } else {
            shared.live_monitor_enabled.store(false, Ordering::Relaxed);
            false
        }
    };

    #[allow(clippy::too_many_arguments)]
    let sync_aec_loopback = |force_reopen: bool,
                             want_running: bool,
                             want_mon: bool,
                             aec_ref_name: Option<String>,
                             aec_gen: &mut u64,
                             wd: &mut WatchdogState,
                             main_pipe: &Option<RunningPipeline>,
                             aec_loop_pipe: &mut Option<LoopbackPipeline>| {
        let want_aec = shared.aec_enabled.load(Ordering::Relaxed);
        let should_open = want_running && main_pipe.is_some() && want_aec && !want_mon;

        if !should_open {
            if aec_loop_pipe.is_some() {
                *aec_gen = aec_gen.wrapping_add(1);
                if let Some(pipe) = main_pipe.as_ref() {
                    let _ = pipe.aec_tx.send(None);
                }
                let _ = aec_loop_pipe.take();
            }
            shared.aec_active.store(false, Ordering::Relaxed);
            store_f32(&shared.aec_erle_db_bits, 0.0);
            wd.aec_reattach_attempt = 0;
            wd.next_aec_reattach_at = None;
            if let Ok(mut st) = shared.status.lock() {
                st.active_aec_ref_name = None;
                if !want_aec && st.last_error_source == Some(ErrorSource::Aec) {
                    st.last_error = None;
                    st.last_error_source = None;
                }
            }
            return;
        }

        if aec_loop_pipe.is_some() && !force_reopen {
            return;
        }

        *aec_gen = aec_gen.wrapping_add(1);
        if let Some(pipe) = main_pipe.as_ref() {
            let _ = pipe.aec_tx.send(None);
        }
        let _ = aec_loop_pipe.take();

        if let Some(pipe) = main_pipe.as_ref() {
            let notifier =
                StreamErrorNotifier::new(cmd_tx.clone(), *aec_gen, StreamKind::AecLoopback);
            match build_aec_loopback_stream(aec_ref_name.as_deref(), notifier) {
                Ok((lp, sink, actual_ref_name)) => {
                    let _ = pipe.aec_tx.send(Some(sink));
                    *aec_loop_pipe = Some(lp);
                    wd.aec_reattach_attempt = 0;
                    wd.next_aec_reattach_at = None;
                    diagnostics::info(format!(
                        "AEC speaker reference attached: '{}'",
                        actual_ref_name
                    ));
                    if let Ok(mut st) = shared.status.lock() {
                        st.active_aec_ref_name = Some(actual_ref_name);
                        if st.last_error_source == Some(ErrorSource::Aec) {
                            st.last_error = None;
                            st.last_error_source = None;
                        }
                    }
                }
                Err(e) => {
                    if wd.aec_reattach_attempt == 0 {
                        diagnostics::warn(format!("AEC reference stream unavailable: {:#}", e));
                    }
                    shared.aec_active.store(false, Ordering::Relaxed);
                    store_f32(&shared.aec_erle_db_bits, 0.0);
                    if let Ok(mut st) = shared.status.lock() {
                        st.active_aec_ref_name = None;
                        if wd.aec_reattach_attempt == 0 {
                            st.last_error = Some(format!("{:#}", e));
                            st.last_error_source = Some(ErrorSource::Aec);
                        }
                    }
                    wd.next_aec_reattach_at = Some(
                        Instant::now() + virtual_reattach_backoff_duration(wd.aec_reattach_attempt),
                    );
                    wd.aec_reattach_attempt = wd.aec_reattach_attempt.saturating_add(1);
                }
            }
        }
    };

    #[allow(clippy::too_many_arguments)]
    let start_main = |in_name: Option<String>,
                      out_name: Option<String>,
                      mon_name: Option<String>,
                      aec_ref_name: Option<String>,
                      profile_already_applied: bool,
                      last_profile_mic: &mut Option<String>,
                      want_mon: &mut bool,
                      main_gen: &mut u64,
                      mon_gen: &mut u64,
                      aec_gen: &mut u64,
                      wd: &mut WatchdogState,
                      main_pipe: &mut Option<RunningPipeline>,
                      live_mon_pipe: &mut Option<MonitorPipeline>,
                      aec_loop_pipe: &mut Option<LoopbackPipeline>| {
        *main_gen = main_gen.wrapping_add(1);
        *aec_gen = aec_gen.wrapping_add(1);
        let current_gen = *main_gen;
        if let Some(old) = main_pipe.take() {
            old.stop();
        }
        let _ = live_mon_pipe.take();
        let _ = aec_loop_pipe.take();

        match build_main_pipeline(
            &shared,
            &speaker,
            &config,
            &cmd_tx,
            current_gen,
            in_name.as_deref(),
            out_name.as_deref(),
            last_profile_mic,
            profile_already_applied,
        ) {
            Ok(pipe) => {
                let in_label = pipe.active_input_name.clone();
                let virt_ok = shared.virtual_out_connected.load(Ordering::Relaxed);
                *main_pipe = Some(pipe);
                // Keep retry_attempt until the stream proves healthy for >= 10s so flapping
                // devices cannot trigger an unthrottled ONNX reload storm.
                wd.next_retry_at = None;
                wd.pipe_started_at = Instant::now();
                wd.last_seen_in_frames = shared.input_callback_frames.load(Ordering::Relaxed);
                wd.last_seen_out_frames = shared.output_callback_frames.load(Ordering::Relaxed);
                wd.in_stall_ticks = 0;
                wd.out_stall_ticks = 0;
                if virt_ok {
                    wd.virt_reattach_attempt = 0;
                    wd.next_virt_reattach_at = None;
                } else if out_name.as_deref().is_some_and(|s| !s.is_empty()) {
                    wd.next_virt_reattach_at =
                        Some(Instant::now() + virtual_reattach_backoff_duration(wd.virt_reattach_attempt));
                    wd.virt_reattach_attempt = wd.virt_reattach_attempt.saturating_add(1);
                }
                diagnostics::info(format!(
                    "Pipeline started: mic='{}', virtual_out_connected={}",
                    in_label, virt_ok
                ));
                if *want_mon {
                    *want_mon = apply_live_monitor(true, mon_name, mon_gen, main_pipe, live_mon_pipe);
                }
                wd.aec_reattach_attempt = 0;
                wd.next_aec_reattach_at = None;
                sync_aec_loopback(
                    true,
                    true,
                    *want_mon,
                    aec_ref_name,
                    aec_gen,
                    wd,
                    main_pipe,
                    aec_loop_pipe,
                );
            }
            Err(PipelineStartError::WaitingForDevice(missing_name)) => {
                if wd.retry_attempt == 0 {
                    diagnostics::info(format!(
                        "Waiting for input device: {:?}",
                        missing_name
                    ));
                }
                shared.stream_running.store(false, Ordering::Relaxed);
                shared.virtual_out_connected.store(false, Ordering::Relaxed);
                shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                shared.rt_priority_active.store(false, Ordering::Relaxed);
                reset_audio_meters(&shared);
                if let Ok(mut st) = shared.status.lock() {
                    st.connection_state = ConnectionState::WaitingDevice;
                    st.waiting_device_name = missing_name;
                    st.active_input_name = None;
                    st.active_output_name = None;
                    st.active_monitor_name = None;
                    st.active_aec_ref_name = None;
                    st.last_error = None;
                    st.last_error_source = None;
                }
                wd.next_retry_at = Some(Instant::now() + reconnect_backoff_duration(wd.retry_attempt));
                wd.retry_attempt = wd.retry_attempt.saturating_add(1);
            }
            Err(PipelineStartError::Other(e)) => {
                if wd.retry_attempt == 0 || wd.retry_attempt.is_multiple_of(10) {
                    diagnostics::error(format!(
                        "Pipeline start failed (attempt {}): {:#}",
                        wd.retry_attempt + 1,
                        e
                    ));
                }
                shared.stream_running.store(false, Ordering::Relaxed);
                shared.virtual_out_connected.store(false, Ordering::Relaxed);
                shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                shared.rt_priority_active.store(false, Ordering::Relaxed);
                reset_audio_meters(&shared);
                if let Ok(mut st) = shared.status.lock() {
                    st.connection_state = ConnectionState::Reconnecting;
                    st.waiting_device_name = in_name;
                    st.active_monitor_name = None;
                    st.active_aec_ref_name = None;
                    st.last_error = Some(format!("{:#}", e));
                    st.last_error_source = Some(ErrorSource::Pipeline);
                }
                wd.next_retry_at = Some(Instant::now() + reconnect_backoff_duration(wd.retry_attempt));
                wd.retry_attempt = wd.retry_attempt.saturating_add(1);
            }
        }
    };

    if want_running {
        start_main(
            cur_in_dev.clone(),
            cur_out_dev.clone(),
            cur_mon_dev.clone(),
            cur_aec_ref_dev.clone(),
            false,
            &mut last_profile_mic,
            &mut want_mon,
            &mut main_gen,
            &mut mon_gen,
            &mut aec_gen,
            &mut wd,
            &mut main_pipe,
            &mut live_mon_pipe,
            &mut aec_loop_pipe,
        );
    } else if let Ok(mut st) = shared.status.lock() {
        st.connection_state = ConnectionState::Stopped;
    }

    loop {
        match cmd_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(cmd) => match cmd {
                EngineCommand::StartOrRestart {
                    input_device,
                    output_device,
                    monitor_device,
                    profile_already_applied,
                } => {
                    cur_in_dev = input_device;
                    cur_out_dev = output_device;
                    cur_mon_dev = monitor_device;
                    want_running = true;
                    wd.retry_attempt = 0;
                    wd.virt_reattach_attempt = 0;
                    wd.next_virt_reattach_at = None;
                    wd.aec_reattach_attempt = 0;
                    wd.next_aec_reattach_at = None;
                    start_main(
                        cur_in_dev.clone(),
                        cur_out_dev.clone(),
                        cur_mon_dev.clone(),
                        cur_aec_ref_dev.clone(),
                        profile_already_applied,
                        &mut last_profile_mic,
                        &mut want_mon,
                        &mut main_gen,
                        &mut mon_gen,
                        &mut aec_gen,
                        &mut wd,
                        &mut main_pipe,
                        &mut live_mon_pipe,
                        &mut aec_loop_pipe,
                    );
                }
                EngineCommand::SetModelVariant(variant) => {
                    let prev = u8_to_variant(shared.model_variant_u8.load(Ordering::Relaxed));
                    shared
                        .model_variant_u8
                        .store(variant_to_u8(variant), Ordering::SeqCst);
                    if main_pipe.is_some() && prev != variant {
                        diagnostics::info(format!("Switching AI model variant to {:?}", variant));
                        wd.retry_attempt = 0;
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            cur_aec_ref_dev.clone(),
                            true,
                            &mut last_profile_mic,
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut aec_gen,
                            &mut wd,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                            &mut aec_loop_pipe,
                        );
                    }
                }
                EngineCommand::SyncLastProfileMic(mic_name) => {
                    last_profile_mic = mic_name;
                }
                EngineCommand::Stop(ack) => {
                    diagnostics::info("Stopping audio pipeline");
                    want_running = false;
                    want_mon = false;
                    main_gen = main_gen.wrapping_add(1);
                    mon_gen = mon_gen.wrapping_add(1);
                    aec_gen = aec_gen.wrapping_add(1);
                    wd.next_retry_at = None;
                    wd.retry_attempt = 0;
                    wd.virt_reattach_attempt = 0;
                    wd.next_virt_reattach_at = None;
                    wd.aec_reattach_attempt = 0;
                    wd.next_aec_reattach_at = None;
                    if let Some(old) = main_pipe.take() {
                        old.stop();
                    }
                    let _ = live_mon_pipe.take();
                    let _ = aec_loop_pipe.take();
                    shared.stream_running.store(false, Ordering::Relaxed);
                    shared.virtual_out_connected.store(false, Ordering::Relaxed);
                    shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                    shared.rt_priority_active.store(false, Ordering::Relaxed);
                    reset_audio_meters(&shared);
                    if let Ok(mut st) = shared.status.lock() {
                        st.connection_state = ConnectionState::Stopped;
                        st.waiting_device_name = None;
                        st.active_monitor_name = None;
                        st.active_aec_ref_name = None;
                    }
                    if let Some(tx) = ack {
                        let _ = tx.send(());
                    }
                }
                EngineCommand::SetLiveMonitor {
                    enabled,
                    monitor_device,
                } => {
                    cur_mon_dev = monitor_device;
                    want_mon = enabled;
                    wd.last_mon_reopen_at = None;
                    if enabled && main_pipe.is_none() {
                        want_running = true;
                        wd.retry_attempt = 0;
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            cur_aec_ref_dev.clone(),
                            false,
                            &mut last_profile_mic,
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut aec_gen,
                            &mut wd,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                            &mut aec_loop_pipe,
                        );
                    } else {
                        want_mon = apply_live_monitor(
                            enabled,
                            cur_mon_dev.clone(),
                            &mut mon_gen,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                        // Automatically detach AEC loopback while Live Monitor is active,
                        // and reattach cleanly once Live Monitor stops.
                        wd.aec_reattach_attempt = 0;
                        wd.next_aec_reattach_at = None;
                        sync_aec_loopback(
                            false,
                            want_running,
                            want_mon,
                            cur_aec_ref_dev.clone(),
                            &mut aec_gen,
                            &mut wd,
                            &main_pipe,
                            &mut aec_loop_pipe,
                        );
                    }
                }
                EngineCommand::SetAecConfig {
                    enabled,
                    reference_device,
                } => {
                    let ref_changed = cur_aec_ref_dev != reference_device;
                    cur_aec_ref_dev = reference_device;
                    shared.aec_enabled.store(enabled, Ordering::Relaxed);
                    wd.aec_reattach_attempt = 0;
                    wd.next_aec_reattach_at = None;
                    sync_aec_loopback(
                        ref_changed,
                        want_running,
                        want_mon,
                        cur_aec_ref_dev.clone(),
                        &mut aec_gen,
                        &mut wd,
                        &main_pipe,
                        &mut aec_loop_pipe,
                    );
                }
                EngineCommand::StreamError {
                    generation,
                    kind,
                    message,
                } => match kind {
                    StreamKind::AecLoopback => {
                        if generation == aec_gen && want_running {
                            diagnostics::warn(format!(
                                "AEC speaker reference interrupted ({}); detaching cleanly without dropping mic",
                                message
                            ));
                            aec_gen = aec_gen.wrapping_add(1);
                            if let Some(pipe) = main_pipe.as_ref() {
                                let _ = pipe.aec_tx.send(None);
                            }
                            let _ = aec_loop_pipe.take();
                            shared.aec_active.store(false, Ordering::Relaxed);
                            store_f32(&shared.aec_erle_db_bits, 0.0);
                            if let Ok(mut st) = shared.status.lock() {
                                st.active_aec_ref_name = None;
                            }
                            wd.next_aec_reattach_at = Some(
                                Instant::now() + reconnect_backoff_duration(wd.aec_reattach_attempt),
                            );
                            wd.aec_reattach_attempt = wd.aec_reattach_attempt.saturating_add(1);
                        }
                    }
                    StreamKind::Monitor => {
                        if generation == mon_gen && want_mon {
                            let can_retry = wd
                                .last_mon_reopen_at
                                .is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
                            if can_retry {
                                diagnostics::warn(format!(
                                    "Live monitor interrupted ({}); attempting single reopen",
                                    message
                                ));
                                wd.last_mon_reopen_at = Some(Instant::now());
                                want_mon = apply_live_monitor(
                                    true,
                                    cur_mon_dev.clone(),
                                    &mut mon_gen,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                );
                            } else {
                                diagnostics::warn(format!(
                                    "Live monitor failed repeatedly ({}); disabling monitor to prevent storm",
                                    message
                                ));
                                want_mon = apply_live_monitor(
                                    false,
                                    cur_mon_dev.clone(),
                                    &mut mon_gen,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                );
                                if let Ok(mut st) = shared.status.lock() {
                                    st.last_error = Some(match i18n::current_locale() {
                                        Locale::Ja => format!(
                                            "マイクテスト再生デバイスが切断されました: {}",
                                            message
                                        ),
                                        Locale::En => format!(
                                            "Mic Test playback device disconnected: {}",
                                            message
                                        ),
                                    });
                                    st.last_error_source = Some(ErrorSource::Monitor);
                                }
                                if !want_mon {
                                    wd.aec_reattach_attempt = 0;
                                    wd.next_aec_reattach_at = None;
                                    sync_aec_loopback(
                                        false,
                                        want_running,
                                        want_mon,
                                        cur_aec_ref_dev.clone(),
                                        &mut aec_gen,
                                        &mut wd,
                                        &main_pipe,
                                        &mut aec_loop_pipe,
                                    );
                                }
                            }
                        }
                    }
                    StreamKind::Input | StreamKind::VirtualOutput => {
                        if generation == main_gen && want_running {
                            diagnostics::warn(format!(
                                "Audio {:?} stream interrupted ({}); scheduling auto-reconnect",
                                kind, message
                            ));
                            main_gen = main_gen.wrapping_add(1);
                            aec_gen = aec_gen.wrapping_add(1);
                            if let Some(old) = main_pipe.take() {
                                old.stop();
                            }
                            let _ = live_mon_pipe.take();
                            let _ = aec_loop_pipe.take();
                            shared.stream_running.store(false, Ordering::Relaxed);
                            shared.virtual_out_connected.store(false, Ordering::Relaxed);
                            shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                            shared.rt_priority_active.store(false, Ordering::Relaxed);
                            shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                            reset_audio_meters(&shared);

                            let input_missing = find_input_device(cur_in_dev.as_deref()).is_none();
                            if let Ok(mut st) = shared.status.lock() {
                                st.active_input_name = None;
                                st.active_output_name = None;
                                st.active_monitor_name = None;
                                st.active_aec_ref_name = None;
                                if input_missing {
                                    st.connection_state = ConnectionState::WaitingDevice;
                                    st.waiting_device_name = cur_in_dev.clone();
                                    st.last_error = None;
                                    st.last_error_source = None;
                                } else {
                                    st.connection_state = ConnectionState::Reconnecting;
                                    st.waiting_device_name = cur_in_dev.clone();
                                    st.last_error = Some(match i18n::current_locale() {
                                        Locale::Ja => format!(
                                            "オーディオストリームを再接続しています: {}",
                                            message
                                        ),
                                        Locale::En => {
                                            format!("Reconnecting audio stream: {}", message)
                                        }
                                    });
                                    st.last_error_source = Some(ErrorSource::Pipeline);
                                }
                            }
                            // Preserve exponential backoff on flapping streams (healthy streams already reset retry_attempt after 10s)
                            wd.next_retry_at =
                                Some(Instant::now() + reconnect_backoff_duration(wd.retry_attempt));
                            wd.retry_attempt = wd.retry_attempt.saturating_add(1);
                        }
                    }
                },
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                watchdog_ticks = watchdog_ticks.wrapping_add(1);
                if !want_running {
                    continue;
                }

                if main_pipe.is_some() {
                    // 1. Sleep/wake & silent USB/virtual-output stall watchdog (after 2s startup grace period)
                    if wd.pipe_started_at.elapsed() >= Duration::from_secs(2) {
                        let cur_in_frames = shared.input_callback_frames.load(Ordering::Relaxed);
                        if cur_in_frames == wd.last_seen_in_frames {
                            wd.in_stall_ticks = wd.in_stall_ticks.saturating_add(1);
                            if wd.in_stall_ticks >= 5 {
                                diagnostics::warn(
                                    "Input audio stream stalled for 2.5s (sleep/wake or USB disconnect); reconnecting",
                                );
                                shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                                start_main(
                                    cur_in_dev.clone(),
                                    cur_out_dev.clone(),
                                    cur_mon_dev.clone(),
                                    cur_aec_ref_dev.clone(),
                                    false,
                                    &mut last_profile_mic,
                                    &mut want_mon,
                                    &mut main_gen,
                                    &mut mon_gen,
                                    &mut aec_gen,
                                    &mut wd,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                    &mut aec_loop_pipe,
                                );
                                continue;
                            }
                        } else {
                            wd.last_seen_in_frames = cur_in_frames;
                            wd.in_stall_ticks = 0;
                        }

                        if shared.virtual_out_connected.load(Ordering::Relaxed) {
                            let cur_out_frames =
                                shared.output_callback_frames.load(Ordering::Relaxed);
                            if cur_out_frames == wd.last_seen_out_frames {
                                wd.out_stall_ticks = wd.out_stall_ticks.saturating_add(1);
                                if wd.out_stall_ticks >= 5 {
                                    diagnostics::warn(
                                        "Virtual output audio stream stalled for 2.5s; reconnecting pipeline",
                                    );
                                    shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                                    start_main(
                                        cur_in_dev.clone(),
                                        cur_out_dev.clone(),
                                        cur_mon_dev.clone(),
                                        cur_aec_ref_dev.clone(),
                                        true,
                                        &mut last_profile_mic,
                                        &mut want_mon,
                                        &mut main_gen,
                                        &mut mon_gen,
                                        &mut aec_gen,
                                        &mut wd,
                                        &mut main_pipe,
                                        &mut live_mon_pipe,
                                        &mut aec_loop_pipe,
                                    );
                                    continue;
                                }
                            } else {
                                wd.last_seen_out_frames = cur_out_frames;
                                wd.out_stall_ticks = 0;
                            }
                        }

                        // Reset reconnect backoff only once the pipeline has streamed healthily for >= 10s
                        if wd.retry_attempt > 0
                            && wd.in_stall_ticks == 0
                            && wd.out_stall_ticks == 0
                            && wd.pipe_started_at.elapsed() >= Duration::from_secs(10)
                        {
                            wd.retry_attempt = 0;
                        }
                    }

                    // 2. When set to "auto" (None), follow OS default physical microphone changes (~every 2s)
                    if cur_in_dev.is_none() && watchdog_ticks.is_multiple_of(4) {
                        if let Some(os_default) = default_physical_input_name() {
                            let changed = main_pipe
                                .as_ref()
                                .is_some_and(|p| p.active_input_name != os_default);
                            if changed {
                                diagnostics::info(format!(
                                    "OS default microphone changed to '{}'; switching automatically",
                                    os_default
                                ));
                                shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                                wd.retry_attempt = 0;
                                start_main(
                                    cur_in_dev.clone(),
                                    cur_out_dev.clone(),
                                    cur_mon_dev.clone(),
                                    cur_aec_ref_dev.clone(),
                                    false,
                                    &mut last_profile_mic,
                                    &mut want_mon,
                                    &mut main_gen,
                                    &mut mon_gen,
                                    &mut aec_gen,
                                    &mut wd,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                    &mut aec_loop_pipe,
                                );
                                continue;
                            }
                        }
                    }

                    // 2b. Follow OS default speaker changes (~every 2s) when AEC is ON in auto mode,
                    //     or retry reattaching an unplugged AEC speaker without disturbing the microphone.
                    if shared.aec_enabled.load(Ordering::Relaxed) && !want_mon {
                        if cur_aec_ref_dev.is_none()
                            && aec_loop_pipe.is_some()
                            && watchdog_ticks.is_multiple_of(4)
                        {
                            if let Some(os_def_spk) = default_physical_output_name() {
                                let cur_active_spk = shared
                                    .status
                                    .lock()
                                    .ok()
                                    .and_then(|s| s.active_aec_ref_name.clone());
                                if cur_active_spk.as_deref() != Some(os_def_spk.as_str()) {
                                    diagnostics::info(format!(
                                        "OS default speaker changed to '{}'; switching AEC reference",
                                        os_def_spk
                                    ));
                                    wd.aec_reattach_attempt = 0;
                                    wd.next_aec_reattach_at = None;
                                    sync_aec_loopback(
                                        true,
                                        want_running,
                                        want_mon,
                                        cur_aec_ref_dev.clone(),
                                        &mut aec_gen,
                                        &mut wd,
                                        &main_pipe,
                                        &mut aec_loop_pipe,
                                    );
                                }
                            }
                        } else if aec_loop_pipe.is_none()
                            && wd.next_aec_reattach_at.is_none_or(|t| Instant::now() >= t)
                        {
                            sync_aec_loopback(
                                false,
                                want_running,
                                want_mon,
                                cur_aec_ref_dev.clone(),
                                &mut aec_gen,
                                &mut wd,
                                &main_pipe,
                                &mut aec_loop_pipe,
                            );
                        }
                    } else if aec_loop_pipe.is_some() {
                        sync_aec_loopback(
                            false,
                            want_running,
                            want_mon,
                            cur_aec_ref_dev.clone(),
                            &mut aec_gen,
                            &mut wd,
                            &main_pipe,
                            &mut aec_loop_pipe,
                        );
                    }

                    // 3. Reattach virtual output automatically with exponential backoff (3s -> 60s)
                    // so a broken/locked virtual endpoint never causes an endless 3s DFN3 reload storm
                    if cur_out_dev.is_some()
                        && !shared.virtual_out_connected.load(Ordering::Relaxed)
                        && wd.next_virt_reattach_at.is_none_or(|t| Instant::now() >= t)
                    {
                        if find_output_device(cur_out_dev.as_deref(), false).is_some() {
                            diagnostics::info(
                                "Configured virtual output available; attempting reattach",
                            );
                            shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                            start_main(
                                cur_in_dev.clone(),
                                cur_out_dev.clone(),
                                cur_mon_dev.clone(),
                                cur_aec_ref_dev.clone(),
                                true,
                                &mut last_profile_mic,
                                &mut want_mon,
                                &mut main_gen,
                                &mut mon_gen,
                                &mut aec_gen,
                                &mut wd,
                                &mut main_pipe,
                                &mut live_mon_pipe,
                                &mut aec_loop_pipe,
                            );
                        } else {
                            wd.next_virt_reattach_at = Some(
                                Instant::now()
                                    + virtual_reattach_backoff_duration(wd.virt_reattach_attempt),
                            );
                            wd.virt_reattach_attempt = wd.virt_reattach_attempt.saturating_add(1);
                        }
                    }
                } else if wd.next_retry_at.is_none_or(|t| Instant::now() >= t) {
                    // Waiting for disconnected physical microphone or transient retry
                    if find_input_device(cur_in_dev.as_deref()).is_some() {
                        shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            cur_aec_ref_dev.clone(),
                            false,
                            &mut last_profile_mic,
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut aec_gen,
                            &mut wd,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                            &mut aec_loop_pipe,
                        );
                    } else {
                        if let Ok(mut st) = shared.status.lock() {
                            st.connection_state = ConnectionState::WaitingDevice;
                            st.waiting_device_name = cur_in_dev.clone();
                        }
                        wd.next_retry_at =
                            Some(Instant::now() + reconnect_backoff_duration(wd.retry_attempt));
                        wd.retry_attempt = wd.retry_attempt.saturating_add(1);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(old) = main_pipe.take() {
                    old.stop();
                }
                let _ = live_mon_pipe.take();
                let _ = aec_loop_pipe.take();
                break;
            }
        }
    }
}

fn build_live_monitor_stream(
    mon_name: Option<&str>,
    err_notifier: StreamErrorNotifier,
) -> Result<(MonitorPipeline, MonitorSink, String)> {
    // Only fall back to OS default speakers when user chose "auto" (None/empty);
    // when an explicit headset/monitor is configured and unplugged, fail cleanly instead of
    // switching to laptop speakers and causing acoustic feedback howl.
    let explicit = mon_name.filter(|s| !s.is_empty());
    let mon_dev = find_output_device(explicit, explicit.is_none()).ok_or_else(|| {
        if let Some(name) = explicit {
            match i18n::current_locale() {
                Locale::Ja => anyhow!(
                    "指定された再生デバイス「{}」が見つかりません（スピーカーへの誤出力を防ぐためマイクテストを停止しました）",
                    name
                ),
                Locale::En => anyhow!(
                    "Playback device \"{}\" was not found (Mic Test was stopped to prevent accidental speaker feedback)",
                    name
                ),
            }
        } else {
            anyhow!(
                "{}",
                tr(
                    "マイクテスト用のスピーカー／ヘッドホンが見つかりません",
                    "No speakers or headphones found for Mic Test"
                )
            )
        }
    })?;
    let actual_mon_name = device_name(&mon_dev).unwrap_or_else(|| "Default Output".to_string());
    let supported_mon = mon_dev
        .default_output_config()
        .with_context(|| match i18n::current_locale() {
            Locale::Ja => format!(
                "再生デバイス「{}」のフォーマット取得に失敗しました",
                actual_mon_name
            ),
            Locale::En => format!(
                "Failed to query audio format for playback device \"{}\"",
                actual_mon_name
            ),
        })?;
    let mon_fmt = supported_mon.sample_format();
    let mon_cfg: StreamConfig = supported_mon.into();
    let sr_mon = mon_cfg.sample_rate;
    let ch_mon = mon_cfg.channels as usize;

    let mon_target = ((sr_mon as usize) * 30) / 1000;
    let mon_rb = HeapRb::<f32>::new((sr_mon as usize / 2).max(8_192));
    let (mut mon_prod, mon_cons) = mon_rb.split();
    let _ = mon_prod.push_slice(&vec![0.0f32; mon_target]);

    let resampler = AdaptiveOutputResampler::new(sr_mon)?;
    let drift = DriftController::new(mon_target, sr_mon);
    let stream = build_output_stream_from_rb(
        &mon_dev,
        &mon_cfg,
        mon_fmt,
        ch_mon,
        mon_cons,
        None,
        err_notifier,
    )
    .with_context(|| match i18n::current_locale() {
        Locale::Ja => format!(
            "再生デバイス「{}」のストリーム作成に失敗しました",
            actual_mon_name
        ),
        Locale::En => format!(
            "Failed to create output stream for playback device \"{}\"",
            actual_mon_name
        ),
    })?;
    stream
        .play()
        .with_context(|| match i18n::current_locale() {
            Locale::Ja => format!(
                "再生デバイス「{}」の再生開始に失敗しました",
                actual_mon_name
            ),
            Locale::En => format!("Failed to start playback on device \"{}\"", actual_mon_name),
        })?;

    Ok((
        MonitorPipeline { _stream: stream },
        MonitorSink {
            prod: mon_prod,
            resampler,
            drift,
            target_frames: mon_target,
            sr_mon,
        },
        actual_mon_name,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_main_pipeline(
    shared: &Arc<SharedAudioState>,
    speaker: &SpeakerHandle,
    config: &Arc<Mutex<AppConfig>>,
    cmd_tx: &mpsc::Sender<EngineCommand>,
    generation: u64,
    in_name: Option<&str>,
    out_name: Option<&str>,
    last_profile_mic: &mut Option<String>,
    profile_already_applied: bool,
) -> std::result::Result<RunningPipeline, PipelineStartError> {
    let in_dev = find_input_device(in_name).ok_or_else(|| {
        PipelineStartError::WaitingForDevice(in_name.filter(|s| !s.is_empty()).map(String::from))
    })?;
    let actual_in_name = device_name(&in_dev).unwrap_or_else(|| "Unknown Mic".into());

    // Restore per-microphone profile automatically on startup, OS default mic switch, or reconnect
    // (runs on the non-RT control thread before launching the real-time DSP worker).
    if !profile_already_applied && last_profile_mic.as_deref() != Some(actual_in_name.as_str()) {
        if let Ok(mut cfg) = config.lock() {
            let changed = cfg.switch_to_mic_profile(last_profile_mic.as_deref(), &actual_in_name);
            if changed {
                shared.apply_config_dsp(&cfg);
                let _ = cfg.save();
                shared.config_revision.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    *last_profile_mic = Some(actual_in_name.clone());
    let supported_in = in_dev
        .default_input_config()
        .context(tr(
            "マイクの標準入力フォーマットを取得できません",
            "Failed to query default microphone input format",
        ))
        .map_err(PipelineStartError::Other)?;
    let in_sample_format = supported_in.sample_format();
    let mut in_cfg: StreamConfig = supported_in.into();
    // Request a low-latency buffer (~10ms) when supported by WASAPI/CoreAudio
    in_cfg.buffer_size = cpal::BufferSize::Default;
    let sr_in = in_cfg.sample_rate;
    let ch_in = in_cfg.channels as usize;

    // Optional virtual output device (e.g., CABLE Input on Windows, Shizuka Virtual Mic on macOS)
    let mut actual_out_name: Option<String> = None;
    let mut sr_out = DFN_SAMPLE_RATE as u32;
    let mut virtual_out_warning: Option<String> = None;

    // SPSC RingBuffer for Input (capacity: 500ms of mono samples)
    let in_rb = HeapRb::<f32>::new((sr_in as usize / 2).max(8_192));
    let (in_prod, mut in_cons) = in_rb.split();

    // Optional Virtual Output RingBuffer + Stream
    let mut out_prod_opt = None;
    let mut virtual_stream_opt = None;
    let mut target_out_frames = (sr_out as usize * 30) / 1000; // 30ms target latency

    if let Some(target_out) = out_name.filter(|s| !s.is_empty()) {
        match find_output_device(Some(target_out), false) {
            Some(out_dev) => match out_dev.default_output_config() {
                Ok(supported_out) => {
                    let resolved_out_name =
                        device_name(&out_dev).unwrap_or_else(|| target_out.to_string());
                    let out_fmt = supported_out.sample_format();
                    let out_cfg: StreamConfig = supported_out.into();
                    sr_out = out_cfg.sample_rate;
                    let ch_out = out_cfg.channels as usize;

                    // Target 30ms occupancy in output ring buffer to absorb OS callback jitter cleanly
                    target_out_frames = ((sr_out as usize) * 30) / 1000;
                    let out_rb_cap = (sr_out as usize / 2).max(8_192);
                    let out_rb = HeapRb::<f32>::new(out_rb_cap);
                    let (mut out_prod, out_cons) = out_rb.split();

                    // Pre-fill target_out_frames with silence so output starts right at optimal drift equilibrium
                    let silence = vec![0.0f32; target_out_frames];
                    let _ = out_prod.push_slice(&silence);

                    let out_notifier = StreamErrorNotifier::new(
                        cmd_tx.clone(),
                        generation,
                        StreamKind::VirtualOutput,
                    );
                    match build_output_stream_from_rb(
                        &out_dev,
                        &out_cfg,
                        out_fmt,
                        ch_out,
                        out_cons,
                        Some(Arc::clone(shared)),
                        out_notifier,
                    ) {
                        Ok(stream) => {
                            actual_out_name = Some(resolved_out_name);
                            out_prod_opt = Some(out_prod);
                            virtual_stream_opt = Some(stream);
                        }
                        Err(e) => {
                            virtual_out_warning = Some(match i18n::current_locale() {
                                Locale::Ja => format!(
                                    "仮想出力「{}」のストリーム作成に失敗しました: {:#}",
                                    resolved_out_name, e
                                ),
                                Locale::En => format!(
                                    "Failed to create stream for virtual output \"{}\": {:#}",
                                    resolved_out_name, e
                                ),
                            });
                        }
                    }
                }
                Err(e) => {
                    virtual_out_warning = Some(match i18n::current_locale() {
                        Locale::Ja => format!(
                            "仮想出力「{}」のフォーマット取得に失敗しました: {}",
                            target_out, e
                        ),
                        Locale::En => format!(
                            "Failed to query format for virtual output \"{}\": {}",
                            target_out, e
                        ),
                    });
                }
            },
            None => {
                virtual_out_warning = Some(match i18n::current_locale() {
                    Locale::Ja => format!(
                        "仮想出力「{}」が見つかりません（再接続待ち）",
                        target_out
                    ),
                    Locale::En => format!(
                        "Virtual output \"{}\" not found (waiting to reconnect)",
                        target_out
                    ),
                });
            }
        }
    }

    // Reset observed OS callback block sizes for the new pipeline
    shared.observed_in_block_frames.store(0, Ordering::Relaxed);
    shared.observed_out_block_frames.store(0, Ordering::Relaxed);

    // Initialize Resamplers BEFORE launching real-time thread (DfTract holds Rc<Tensor> so it initializes on the worker thread)
    let init_variant = u8_to_variant(shared.model_variant_u8.load(Ordering::Relaxed));
    let init_atten = load_f32(&shared.atten_lim_bits);
    let init_beta = load_f32(&shared.pf_beta_bits);
    let init_thresh = load_f32(&shared.min_db_thresh_bits);
    let init_vad_en = shared.vad_gate_enabled.load(Ordering::Relaxed);
    let init_vad_thresh = load_f32(&shared.vad_threshold_bits);
    let init_hpf_en = shared.hpf_enabled.load(Ordering::Relaxed);
    let init_agc_en = shared.agc_enabled.load(Ordering::Relaxed);
    let init_aec_en = shared.aec_enabled.load(Ordering::Relaxed);
    let init_nc_en = shared.nc_enabled.load(Ordering::Relaxed);
    let init_mic_muted = shared.mic_muted.load(Ordering::Relaxed);
    let init_gain_db = load_f32(&shared.input_gain_bits);
    let mut in_resampler =
        InputResampler48k::new(sr_in).map_err(PipelineStartError::Other)?;
    let mut out_resampler =
        AdaptiveOutputResampler::new(sr_out).map_err(PipelineStartError::Other)?;
    let mut drift_ctrl = DriftController::new(target_out_frames, sr_out);

    let (mon_tx, mon_rx) = mpsc::channel::<Option<MonitorSink>>();
    let (aec_tx, aec_rx) = mpsc::channel::<Option<AecRefSink>>();
    let spk_rb = HeapRb::<SpeakerRtFrame>::new(256);
    let (mut spk_prod, spk_cons) = spk_rb.split();
    speaker.attach_consumer(Some(spk_cons));
    let spk_worker = speaker.clone();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_worker = Arc::clone(&stop_flag);
    let shared_worker = Arc::clone(shared);
    let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);

    // Spawn Real-Time DSP Worker Thread
    let worker_join = thread::Builder::new()
        .name("shizuka-rt-dsp".into())
        .spawn(move || {
            let mut processor = match NoiseProcessor::new(
                init_variant,
                init_atten,
                init_beta,
                init_thresh,
                init_vad_en,
                init_vad_thresh,
                init_hpf_en,
                init_agc_en,
                init_aec_en,
                init_nc_en,
                init_mic_muted,
                init_gain_db,
            ) {
                Ok(p) => {
                    let _ = init_tx.send(Ok(()));
                    p
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return;
                }
            };

            // Promote worker thread to OS real-time audio priority (Windows MMCSS Pro Audio / macOS CoreAudio RT)
            let rt_handle =
                promote_current_thread_to_real_time(DFN_HOP_SIZE as u32, DFN_SAMPLE_RATE as u32)
                    .ok();
            shared_worker
                .rt_priority_active
                .store(rt_handle.is_some(), Ordering::Relaxed);

            let mut in_chunk_scratch = vec![0.0f32; 4_096];
            let mut frame_48k_in = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_ref = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_pre_gate = [0.0f32; DFN_HOP_SIZE];
            let mut frame_16k_spk = [0.0f32; SPEAKER_FRAME_SAMPLES_16K];
            let mut spk_downsampler = Downsampler48kTo16k::new();
            let mut frame_48k_raw = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_enh = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_out = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_mon = [0.0f32; DFN_HOP_SIZE];
            let mut mon_sink_opt: Option<MonitorSink> = None;
            let mut aec_sink_opt: Option<AecRefSink> = None;

            let mut smooth_in_db = -85.0f32;
            let mut smooth_enh_db = -85.0f32;
            let mut smooth_out_db = -85.0f32;
            let mut smooth_proc_us: f32 = 0.0;
            let mut clip_hold_frames: u32 = 0;
            let mut spk_unpark_counter: u8 = 0;

            let mut mon_clean_mix = if shared_worker.monitor_clean.load(Ordering::Relaxed) {
                1.0f32
            } else {
                0.0f32
            };
            let mut mon_gain =
                monitor_volume_to_gain(load_f32(&shared_worker.monitor_volume_bits));

            while !stop_worker.load(Ordering::Relaxed) {
                let needed_in = in_resampler.input_frames_next();
                if in_cons.occupied_len() < needed_in {
                    if !in_cons.write_is_held() {
                        break;
                    }
                    // Input callback unparks immediately when audio arrives; 50ms timeout avoids
                    // spurious 250 Hz timer wakeups while keeping stop_worker responsive.
                    thread::park_timeout(Duration::from_millis(50));
                    continue;
                }

                // Process all available 10ms frames in queue
                while !stop_worker.load(Ordering::Relaxed) {
                    let needed = in_resampler.input_frames_next();
                    if in_cons.occupied_len() < needed {
                        break;
                    }
                    if in_chunk_scratch.len() < needed {
                        in_chunk_scratch.resize(needed, 0.0);
                    }
                    let popped = in_cons.pop_slice(&mut in_chunk_scratch[..needed]);
                    if popped < needed {
                        break;
                    }

                    let t0 = Instant::now();

                    // Drain any live-monitor or AEC speaker-reference attach/detach updates without blocking
                    while let Ok(next_sink) = mon_rx.try_recv() {
                        mon_sink_opt = next_sink;
                    }
                    while let Ok(next_aec) = aec_rx.try_recv() {
                        aec_sink_opt = next_aec;
                    }

                    // Sync lock-free parameters from UI / shortcuts
                    let nc_en = shared_worker.nc_enabled.load(Ordering::Relaxed);
                    let want_aec = shared_worker.aec_enabled.load(Ordering::Relaxed);
                    let mon_active = mon_sink_opt.is_some();
                    let spk_capture = spk_worker.shared.should_capture_frames(nc_en);
                    let spk_engaged = spk_worker.shared.is_gate_engaged(nc_en);
                    let spk_allowed = spk_worker.shared.speaker_pass.load(Ordering::Relaxed);
                    processor.set_enabled(nc_en);
                    processor.set_mic_muted(shared_worker.mic_muted.load(Ordering::Relaxed));
                    processor.set_hpf_enabled(shared_worker.hpf_enabled.load(Ordering::Relaxed));
                    processor.set_agc_enabled(shared_worker.agc_enabled.load(Ordering::Relaxed));
                    processor.set_aec_enabled(want_aec);
                    processor.set_aec_monitor_bypass(mon_active);
                    processor.set_attenuation_limit(load_f32(&shared_worker.atten_lim_bits));
                    processor.set_post_filter_beta(load_f32(&shared_worker.pf_beta_bits));
                    processor.set_input_gain_db(load_f32(&shared_worker.input_gain_bits));
                    processor.set_min_db_thresh(load_f32(&shared_worker.min_db_thresh_bits));
                    processor.set_vad_gate(
                        shared_worker.vad_gate_enabled.load(Ordering::Relaxed),
                        load_f32(&shared_worker.vad_threshold_bits),
                    );
                    processor.set_speaker_gate(spk_engaged, spk_allowed);
                    let alg_latency_ms = processor.algorithmic_latency_ms();

                    if in_resampler
                        .process(&in_chunk_scratch[..needed], &mut frame_48k_in)
                        .is_err()
                    {
                        shared_worker
                            .resampler_error_count
                            .fetch_add(1, Ordering::Relaxed);
                        continue;
                    }

                    let has_aec_ref = if want_aec && !mon_active {
                        if let Some(ref mut aec_sink) = aec_sink_opt {
                            let _ = aec_sink.pull_frame_48k(&mut frame_48k_ref);
                            true
                        } else {
                            false
                        }
                    } else {
                        if let Some(ref mut aec_sink) = aec_sink_opt {
                            aec_sink.discard_backlog();
                        }
                        false
                    };
                    let aec_ref_slice = if has_aec_ref {
                        Some(&frame_48k_ref[..])
                    } else {
                        None
                    };

                    let res = processor.process_frame_48k(
                        &frame_48k_in,
                        aec_ref_slice,
                        if spk_capture {
                            Some(&mut frame_48k_pre_gate)
                        } else {
                            None
                        },
                        &mut frame_48k_raw,
                        &mut frame_48k_enh,
                        &mut frame_48k_out,
                    );
                    if spk_capture {
                        spk_downsampler.process_frame(&frame_48k_pre_gate, &mut frame_16k_spk);
                        if spk_prod
                            .try_push(SpeakerRtFrame {
                                samples_16k: frame_16k_spk,
                                speech_active: res.speech_active_now,
                                mon_active,
                                lsnr_db: res.lsnr_db,
                            })
                            .is_ok()
                        {
                            // Batch cross-thread unpark syscalls to once every 80ms (8 frames)
                            // so the real-time MMCSS/CoreAudio loop never incurs 100 Hz wakeup jitter.
                            spk_unpark_counter = spk_unpark_counter.wrapping_add(1);
                            if spk_unpark_counter >= 8 {
                                spk_unpark_counter = 0;
                                spk_worker.notify_frames_ready();
                            }
                        }
                    } else {
                        spk_unpark_counter = 0;
                    }
                    if res.dfn_error {
                        shared_worker
                            .dfn_error_count
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    shared_worker
                        .vad_speaking
                        .store(res.vad_speaking, Ordering::Relaxed);
                    shared_worker
                        .aec_active
                        .store(res.aec_active, Ordering::Relaxed);
                    store_f32(&shared_worker.agc_gain_db_bits, res.agc_gain_db);
                    store_f32(&shared_worker.aec_erle_db_bits, res.aec_erle_db);

                    // Hold clipping warning for 1.5s (150 x 10ms frames) so 50ms UI polling never misses peaks
                    if res.clipped {
                        if clip_hold_frames == 0 {
                            shared_worker.clip_count.fetch_add(1, Ordering::Relaxed);
                        }
                        clip_hold_frames = 150;
                    } else {
                        clip_hold_frames = clip_hold_frames.saturating_sub(1);
                    }
                    shared_worker
                        .clipping_active
                        .store(clip_hold_frames > 0, Ordering::Relaxed);

                    let lsnr = res.lsnr_db;

                    // Dynamically adapt output buffer target when OS audio host uses large callback blocks
                    // (e.g., Bluetooth HFP or CoreAudio 1024/2048-frame periods) to avoid recurring underruns
                    let obs_in = shared_worker.observed_in_block_frames.load(Ordering::Relaxed);
                    let obs_out = shared_worker
                        .observed_out_block_frames
                        .load(Ordering::Relaxed);

                    // Push to Virtual Microphone Output with PI Clock Drift Compensation
                    if let Some(out_prod) = &mut out_prod_opt {
                        let in_as_out =
                            ((obs_in as u64 * sr_out as u64) / (sr_in.max(1) as u64)) as usize;
                        let dynamic_floor = (in_as_out + obs_out as usize + (sr_out as usize / 200))
                            .min((sr_out as usize * 120) / 1000);
                        if dynamic_floor > target_out_frames {
                            target_out_frames = dynamic_floor;
                            drift_ctrl.set_target_samples(target_out_frames);
                        }

                        let occ = out_prod.occupied_len();
                        let cap = out_prod.capacity().get();
                        let excess_silence_thresh = target_out_frames + (sr_out as usize * 35) / 1000;

                        // Drop redundant silent frames if an OS/CPU stall caused >35ms excess queue buildup,
                        // restoring crisp 30ms latency immediately between words without any audible click!
                        if !res.vad_speaking && occ > excess_silence_thresh {
                            drift_ctrl.damp_transient();
                        } else {
                            let rel_ratio = if occ > cap * 3 / 4 {
                                drift_ctrl.reset();
                                0.990 // Drain excess smoothly
                            } else if occ > target_out_frames + (sr_out as usize * 55) / 1000 {
                                drift_ctrl.damp_transient();
                                0.975 // Fast drain of transient stall backlog (~25ms/s)
                            } else if occ == 0 {
                                drift_ctrl.damp_transient();
                                1.005 // Smoothly rebuild buffer occupancy while keeping learned drift lock
                            } else {
                                drift_ctrl.update(occ)
                            };

                            match out_resampler.process(&frame_48k_out, rel_ratio) {
                                Ok(resampled_out) => {
                                    let pushed = out_prod.push_slice(resampled_out);
                                    if pushed < resampled_out.len() {
                                        shared_worker.output_drop_count.fetch_add(
                                            (resampled_out.len() - pushed) as u64,
                                            Ordering::Relaxed,
                                        );
                                    }
                                }
                                Err(_) => {
                                    shared_worker
                                        .resampler_error_count
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }

                        store_f32(&shared_worker.drift_ppm_bits, drift_ctrl.drift_ppm());
                        let in_lat_ms =
                            (in_cons.occupied_len() as f32 / sr_in as f32) * 1000.0;
                        let out_lat_ms = (occ as f32 / sr_out as f32) * 1000.0;
                        store_f32(
                            &shared_worker.latency_ms_bits,
                            in_lat_ms + alg_latency_ms + out_lat_ms,
                        );
                    } else {
                        store_f32(&shared_worker.drift_ppm_bits, 0.0);
                        store_f32(&shared_worker.latency_ms_bits, alg_latency_ms);
                    }

                    // Push to Discord-style Live Mic Test Monitor with smooth A/B crossfade & volume gain
                    let want_mon_clean = shared_worker.monitor_clean.load(Ordering::Relaxed);
                    let target_mon_mix = if want_mon_clean { 1.0f32 } else { 0.0f32 };
                    let target_mon_gain =
                        monitor_volume_to_gain(load_f32(&shared_worker.monitor_volume_bits));

                    if let Some(sink) = &mut mon_sink_opt {
                        // Keep Live Mic Test latency tight (<= 35ms target) so speaker monitoring never
                        // develops a >100ms slapback delay that re-triggers the mic in an echo loop!
                        let in_as_mon = ((obs_in as u64 * sink.sr_mon as u64)
                            / (sr_in.max(1) as u64)) as usize;
                        let mon_floor = (in_as_mon + (sink.sr_mon as usize / 65))
                            .min((sink.sr_mon as usize * 35) / 1000);
                        if mon_floor > sink.target_frames {
                            sink.target_frames = mon_floor;
                            sink.drift.set_target_samples(mon_floor);
                        }

                        let mix_step = (target_mon_mix - mon_clean_mix) / (DFN_HOP_SIZE as f32);
                        let gain_step = (target_mon_gain - mon_gain) / (DFN_HOP_SIZE as f32);
                        for i in 0..DFN_HOP_SIZE {
                            mon_clean_mix = (mon_clean_mix + mix_step).clamp(0.0, 1.0);
                            mon_gain = (mon_gain + gain_step).clamp(0.0, 3.0);
                            let sig = frame_48k_raw[i] * (1.0 - mon_clean_mix)
                                + frame_48k_enh[i] * mon_clean_mix;
                            frame_48k_mon[i] = soft_limit(sig * mon_gain);
                        }
                        mon_clean_mix = target_mon_mix;
                        mon_gain = target_mon_gain;

                        let occ = sink.prod.occupied_len();
                        let cap = sink.prod.capacity().get();
                        let mon_excess_silence =
                            sink.target_frames + (sink.sr_mon as usize * 18) / 1000;
                        let mon_hard_cap =
                            sink.target_frames + (sink.sr_mon as usize * 35) / 1000;

                        // Never allow Live Monitor queue to accumulate >35ms excess lag (prevents
                        // delayed speaker playback from re-entering the mic and echoing!)
                        if (!res.vad_speaking && occ > mon_excess_silence) || occ > mon_hard_cap {
                            sink.drift.damp_transient();
                        } else {
                            let rel = if occ > cap * 3 / 4 {
                                sink.drift.reset();
                                0.990
                            } else if occ > mon_excess_silence {
                                sink.drift.damp_transient();
                                0.960 // Rapidly drain any transient monitor lag
                            } else if occ == 0 {
                                sink.drift.damp_transient();
                                1.005
                            } else {
                                sink.drift.update(occ)
                            };
                            if let Ok(mon_samples) = sink.resampler.process(&frame_48k_mon, rel) {
                                let _ = sink.prod.push_slice(mon_samples);
                            }
                        }
                    } else {
                        mon_clean_mix = target_mon_mix;
                        mon_gain = target_mon_gain;
                    }

                    // Level Metering (dBFS with fast attack, smooth release)
                    let in_db = rms_dbfs(&frame_48k_raw);
                    let enh_db = rms_dbfs(&frame_48k_enh);
                    let out_db = rms_dbfs(&frame_48k_out);
                    smooth_in_db = smooth_meter(smooth_in_db, in_db);
                    smooth_enh_db = smooth_meter(smooth_enh_db, enh_db);
                    smooth_out_db = smooth_meter(smooth_out_db, out_db);
                    let mon_db = if want_mon_clean {
                        smooth_enh_db
                    } else {
                        smooth_in_db
                    };

                    store_f32(&shared_worker.input_rms_db_bits, smooth_in_db);
                    store_f32(&shared_worker.output_rms_db_bits, smooth_out_db);
                    store_f32(&shared_worker.monitor_rms_db_bits, mon_db);
                    store_f32(&shared_worker.lsnr_db_bits, lsnr);

                    let elapsed_us = t0.elapsed().as_micros() as f32;
                    smooth_proc_us = if smooth_proc_us <= 0.0 {
                        elapsed_us
                    } else {
                        0.85 * smooth_proc_us + 0.15 * elapsed_us
                    };
                    shared_worker
                        .process_time_us
                        .store(smooth_proc_us.round() as u32, Ordering::Relaxed);
                }
            }

            shared_worker
                .rt_priority_active
                .store(false, Ordering::Relaxed);
            if let Some(h) = rt_handle {
                let _ = demote_current_thread_from_real_time(h);
            }
        })
        .map_err(|e| PipelineStartError::Other(e.into()))?;

    // Guard ensures the RT worker thread + DFN3 model (~50 MB) + MMCSS/Mach RT handle are
    // always stopped and joined if any subsequent stream initialization step returns Err.
    let mut worker_guard = WorkerInitGuard {
        stop_flag: Arc::clone(&stop_flag),
        worker_join: Some(worker_join),
    };

    init_rx
        .recv()
        .map_err(|_| {
            PipelineStartError::Other(anyhow!(
                "{}",
                tr(
                    "AI推論スレッドの初期化待機中に中断されました",
                    "Interrupted while waiting for AI inference thread initialization"
                )
            ))
        })?
        .map_err(PipelineStartError::Other)?;

    // Start virtual output stream AFTER neural network init so ring buffer never starves at startup
    if let Some(virt_stream) = virtual_stream_opt.take() {
        match virt_stream.play() {
            Ok(()) => {
                virtual_stream_opt = Some(virt_stream);
            }
            Err(e) => {
                let label = actual_out_name
                    .take()
                    .unwrap_or_else(|| tr("仮想出力", "Virtual Output").to_string());
                virtual_out_warning = Some(match i18n::current_locale() {
                    Locale::Ja => format!("仮想出力「{}」の再生開始に失敗しました: {}", label, e),
                    Locale::En => format!("Failed to start virtual output \"{}\": {}", label, e),
                });
            }
        }
    }

    let worker_thread_handle = worker_guard
        .worker_join
        .as_ref()
        .expect("worker handle present before disarm")
        .thread()
        .clone();
    let in_err_notifier = StreamErrorNotifier::new(cmd_tx.clone(), generation, StreamKind::Input);
    let input_stream = build_input_stream_to_rb(
        &in_dev,
        &in_cfg,
        in_sample_format,
        ch_in,
        in_prod,
        worker_thread_handle,
        Arc::clone(shared),
        in_err_notifier,
    )
    .map_err(PipelineStartError::Other)?;
    input_stream
        .play()
        .context(tr(
            "マイク入力ストリームの開始に失敗しました",
            "Failed to start microphone input stream",
        ))
        .map_err(PipelineStartError::Other)?;

    let virt_connected = virtual_stream_opt.is_some();
    shared.stream_running.store(true, Ordering::Relaxed);
    shared
        .virtual_out_connected
        .store(virt_connected, Ordering::Relaxed);

    if let Ok(mut st) = shared.status.lock() {
        st.connection_state = ConnectionState::Running;
        st.waiting_device_name = None;
        st.active_input_name = Some(actual_in_name.clone());
        st.active_output_name = actual_out_name;
        st.input_sample_rate = sr_in;
        st.output_sample_rate = sr_out;
        st.last_error_source = if virtual_out_warning.is_some() {
            Some(ErrorSource::VirtualOutput)
        } else {
            None
        };
        st.last_error = virtual_out_warning;
    }

    Ok(RunningPipeline {
        stop_flag,
        worker_join: worker_guard.disarm(),
        mon_tx,
        aec_tx,
        speaker: speaker.clone(),
        active_input_name: actual_in_name,
        _input_stream: input_stream,
        _virtual_output_stream: virtual_stream_opt,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_input_stream_to_rb(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    prod: ringbuf::HeapProd<f32>,
    worker_thread: thread::Thread,
    shared: Arc<SharedAudioState>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream> {
    match format {
        SampleFormat::F32 => build_input_typed::<f32>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::F64 => build_input_typed::<f64>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::I8 => build_input_typed::<i8>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::U8 => build_input_typed::<u8>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::I16 => build_input_typed::<i16>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::U16 => build_input_typed::<u16>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::I24 => build_input_typed::<cpal::I24>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::U24 => build_input_typed::<cpal::U24>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::I32 => build_input_typed::<i32>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        SampleFormat::U32 => build_input_typed::<u32>(
            device,
            config,
            channels,
            prod,
            worker_thread,
            shared,
            err_notifier,
        ),
        other => Err(match i18n::current_locale() {
            Locale::Ja => anyhow!("未対応の入力サンプル形式: {:?}", other),
            Locale::En => anyhow!("Unsupported input sample format: {:?}", other),
        }),
    }
}

struct StereoDownmixer {
    env0: f32,
    env1: f32,
    scale: f32,
}

impl StereoDownmixer {
    fn new() -> Self {
        Self {
            env0: 0.0,
            env1: 0.0,
            scale: 0.5,
        }
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> f32 {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        let a = l.abs();
        let b = r.abs();
        self.env0 = if a > self.env0 {
            0.9 * self.env0 + 0.1 * a
        } else {
            let d = self.env0 * 0.99995;
            if d < 1e-9 { 0.0 } else { d }
        };
        self.env1 = if b > self.env1 {
            0.9 * self.env1 + 0.1 * b
        } else {
            let d = self.env1 * 0.99995;
            if d < 1e-9 { 0.0 } else { d }
        };

        let hi = self.env0.max(self.env1);
        let lo = self.env0.min(self.env1);

        // Single-sided 2-ch audio interfaces (e.g. MOTU M2/M4, Scarlett XLR on Ch1 with Ch2 empty)
        // lose -6 dB under naive (L + R) * 0.5 averaging. Glide smoothly to 1.0x when one channel
        // is active (> -80 dBFS) and the other is >18 dB quieter; stay at 0.5x for true stereo mics.
        let target_scale = if hi > 1e-4 {
            if lo < hi * 0.12 {
                1.0f32
            } else {
                0.5f32
            }
        } else {
            self.scale
        };

        self.scale += (target_scale - self.scale) * 0.002;
        (l + r) * self.scale
    }
}

/// Zero-allocation stack-resident downmixer for multi-channel (>2 ch) audio interfaces
/// (e.g. 4–16 ch MOTU / Scarlett / Behringer mixers) so a single plugged-in XLR mic is not
/// attenuated by `-12` to `-24 dB` under naive `1 / channels` division.
struct MultiChannelDownmixer {
    channels: usize,
    envs: [f32; Self::MAX_TRACKED_CH],
    scale: f32,
}

impl MultiChannelDownmixer {
    const MAX_TRACKED_CH: usize = 16;

    fn new(channels: usize) -> Self {
        let ch = channels.max(1);
        Self {
            channels: ch,
            envs: [0.0; Self::MAX_TRACKED_CH],
            scale: 1.0 / (ch as f32),
        }
    }

    #[inline]
    fn process_frame<T>(&mut self, frame: &[T]) -> f32
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let tracked = self.channels.min(Self::MAX_TRACKED_CH);
        let mut sum = 0.0f32;
        let mut hi = 0.0f32;

        for (idx, &raw) in frame.iter().enumerate() {
            let s = f32::from_sample(raw);
            let s = if s.is_finite() { s } else { 0.0 };
            sum += s;
            if idx < tracked {
                let a = s.abs();
                let env = if a > self.envs[idx] {
                    0.9 * self.envs[idx] + 0.1 * a
                } else {
                    let d = self.envs[idx] * 0.99995;
                    if d < 1e-9 { 0.0 } else { d }
                };
                self.envs[idx] = env;
                if env > hi {
                    hi = env;
                }
            }
        }

        let target_scale = if hi > 1e-4 {
            let mut active = 0usize;
            for &env in &self.envs[..tracked] {
                if env >= hi * 0.12 {
                    active += 1;
                }
            }
            1.0 / (active.max(1) as f32)
        } else {
            self.scale
        };

        self.scale += (target_scale - self.scale) * 0.002;
        sum * self.scale
    }
}

fn build_input_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut prod: ringbuf::HeapProd<f32>,
    worker_thread: thread::Thread,
    shared: Arc<SharedAudioState>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = channels.max(1);
    let mut stereo_dm = StereoDownmixer::new();
    let mut multi_dm = MultiChannelDownmixer::new(ch);

    // Zero-allocation callback using a fixed stack buffer
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut mono_scratch = [0.0f32; 512];
            let mut count = 0usize;
            let mut total_frames = 0u64;
            let mut dropped_frames = 0u64;

            match ch {
                1 => {
                    for &s in data {
                        let v = f32::from_sample(s);
                        mono_scratch[count] = if v.is_finite() { v } else { 0.0 };
                        count += 1;
                        total_frames += 1;
                        if count == mono_scratch.len() {
                            let pushed = prod.push_slice(&mono_scratch);
                            dropped_frames += (count - pushed) as u64;
                            count = 0;
                        }
                    }
                }
                2 => {
                    for frame in data.as_chunks::<2>().0 {
                        let l = f32::from_sample(frame[0]);
                        let r = f32::from_sample(frame[1]);
                        mono_scratch[count] = stereo_dm.process(l, r);
                        count += 1;
                        total_frames += 1;
                        if count == mono_scratch.len() {
                            let pushed = prod.push_slice(&mono_scratch);
                            dropped_frames += (count - pushed) as u64;
                            count = 0;
                        }
                    }
                }
                _ => {
                    for frame in data.chunks_exact(ch) {
                        mono_scratch[count] = multi_dm.process_frame(frame);
                        count += 1;
                        total_frames += 1;
                        if count == mono_scratch.len() {
                            let pushed = prod.push_slice(&mono_scratch);
                            dropped_frames += (count - pushed) as u64;
                            count = 0;
                        }
                    }
                }
            }
            if count > 0 {
                let pushed = prod.push_slice(&mono_scratch[..count]);
                dropped_frames += (count - pushed) as u64;
            }
            if total_frames > 0 {
                shared
                    .input_callback_frames
                    .fetch_add(total_frames, Ordering::Relaxed);
                shared
                    .observed_in_block_frames
                    .fetch_max(total_frames.min(u32::MAX as u64) as u32, Ordering::Relaxed);
            }
            if dropped_frames > 0 {
                shared
                    .input_drop_count
                    .fetch_add(dropped_frames, Ordering::Relaxed);
            }
            worker_thread.unpark();
        },
        move |err| err_notifier.notify(err),
        None,
    )?;

    Ok(stream)
}

fn build_output_stream_from_rb(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    cons: ringbuf::HeapCons<f32>,
    underrun_shared: Option<Arc<SharedAudioState>>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream> {
    match format {
        SampleFormat::F32 => build_output_typed::<f32>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::F64 => build_output_typed::<f64>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::I8 => build_output_typed::<i8>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::U8 => build_output_typed::<u8>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::I16 => build_output_typed::<i16>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::U16 => build_output_typed::<u16>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::I24 => build_output_typed::<cpal::I24>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::U24 => build_output_typed::<cpal::U24>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::I32 => build_output_typed::<i32>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        SampleFormat::U32 => build_output_typed::<u32>(
            device,
            config,
            channels,
            cons,
            underrun_shared,
            err_notifier,
        ),
        other => Err(match i18n::current_locale() {
            Locale::Ja => anyhow!("未対応の出力サンプル形式: {:?}", other),
            Locale::En => anyhow!("Unsupported output sample format: {:?}", other),
        }),
    }
}

fn build_output_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut cons: ringbuf::HeapCons<f32>,
    underrun_shared: Option<Arc<SharedAudioState>>,
    err_notifier: StreamErrorNotifier,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = channels.max(1);
    let mut last_sample = 0.0f32;

    // Zero-allocation output callback using fixed stack buffer
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let zero_sample = T::from_sample(0.0);
            let mut mono_scratch = [0.0f32; 512];
            let total_frames = data.len() / ch;
            let mut written_frames = 0usize;
            let mut had_underrun = false;

            while written_frames < total_frames {
                let batch = (total_frames - written_frames).min(mono_scratch.len());
                let popped = cons.pop_slice(&mut mono_scratch[..batch]);
                if popped < batch {
                    had_underrun = true;
                }

                for (i, &sample) in mono_scratch.iter().enumerate().take(batch) {
                    let val = if i < popped {
                        last_sample = sample;
                        sample
                    } else {
                        // Gentle exponential decay to zero on rare underrun (flush denormals)
                        last_sample *= 0.95;
                        if last_sample.abs() < 1e-6 {
                            last_sample = 0.0;
                        }
                        last_sample
                    };
                    let converted = T::from_sample(val);
                    let base = (written_frames + i) * ch;
                    // Route voice cleanly to Front Left & Front Right; keep surround/LFE channels silent on 5.1/7.1 headsets
                    for c in 0..ch {
                        data[base + c] = if c < 2 { converted } else { zero_sample };
                    }
                }
                written_frames += batch;
            }
            if let Some(ref sh) = underrun_shared {
                if total_frames > 0 {
                    sh.output_callback_frames
                        .fetch_add(total_frames as u64, Ordering::Relaxed);
                    sh.observed_out_block_frames
                        .fetch_max(total_frames.min(u32::MAX as usize) as u32, Ordering::Relaxed);
                }
                if had_underrun {
                    sh.output_underrun_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        move |err| err_notifier.notify(err),
        None,
    )?;

    Ok(stream)
}

#[inline]
fn monitor_volume_to_gain(vol: f32) -> f32 {
    let v = vol.clamp(0.0, 1.0);
    if v <= 0.001 {
        0.0
    } else {
        // 0% -> 0.0 (mute), 50% -> ~1.0 (0 dB unity), 80% -> ~2.1 (+6.5 dB), 100% -> 3.0 (+9.5 dB)
        // Gives comfortable audibility for unamplified XLR / audio-interface mics like Discord Mic Test,
        // protected against harsh clipping by soft_limit().
        3.0 * v.powf(1.585)
    }
}

#[inline]
fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -90.0;
    }
    let mut sum_sq = 0.0f32;
    for &s in samples {
        sum_sq += s * s;
    }
    let rms = (sum_sq / samples.len() as f32).sqrt();
    if rms < 1e-5 {
        -90.0
    } else {
        (20.0 * rms.log10()).clamp(-90.0, 0.0)
    }
}

#[inline]
fn smooth_meter(prev_db: f32, cur_db: f32) -> f32 {
    if cur_db > prev_db {
        // Fast attack
        0.45 * prev_db + 0.55 * cur_db
    } else {
        // Smooth release
        0.88 * prev_db + 0.12 * cur_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_monitor_sink_delivers_audio_to_ringbuffer() {
        let sr_mon = 48_000u32;
        let mon_target = ((sr_mon as usize) * 30) / 1000;
        let mon_rb = HeapRb::<f32>::new((sr_mon as usize / 2).max(8_192));
        let (mut prod, mut cons) = mon_rb.split();
        let _ = prod.push_slice(&vec![0.0f32; mon_target]);

        let mut resampler = AdaptiveOutputResampler::new(sr_mon).unwrap();
        let mut drift = DriftController::new(mon_target, sr_mon);
        let mut frame = [0.0f32; DFN_HOP_SIZE];
        let mut out_pop = [0.0f32; 480];
        let mut max_popped = 0.0f32;
        let mut total_pushed = 0usize;
        let mut total_popped = 0usize;

        for f in 0..20 {
            for (i, s) in frame.iter_mut().enumerate() {
                let t = (f * DFN_HOP_SIZE + i) as f32 / 48_000.0;
                *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
            }
            let occ = prod.occupied_len();
            let rel = drift.update(occ);
            let res = resampler.process(&frame, rel).unwrap();
            total_pushed += prod.push_slice(res);

            let p = cons.pop_slice(&mut out_pop);
            total_popped += p;
            for &s in &out_pop[..p] {
                max_popped = max_popped.max(s.abs());
            }
        }
        assert!(total_pushed > 0 && total_popped > 0);
        assert!(
            max_popped > 0.4,
            "Expected non-zero audio out of monitor ringbuf, got {}",
            max_popped
        );
    }

    #[test]
    fn test_stereo_downmixer_preserves_single_channel_interface_gain() {
        // 1. Single-sided 2-ch XLR audio interface (Ch0 active, Ch1 silent like MOTU M Series)
        let mut dm_mono = StereoDownmixer::new();
        let mut peak_mono = 0.0f32;
        for i in 0..4_800 {
            let sig = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48_000.0).sin() * 0.2;
            let out = dm_mono.process(sig, 0.0);
            if i > 4_000 {
                peak_mono = peak_mono.max(out.abs());
            }
        }
        assert!(
            (peak_mono - 0.2).abs() < 0.005,
            "Single-channel 2-ch interface should preserve unity gain (~0.2), got {}",
            peak_mono
        );

        // 2. True dual-channel stereo mic (both L and R active) should average cleanly at 0.5x
        let mut dm_stereo = StereoDownmixer::new();
        let mut peak_stereo = 0.0f32;
        for i in 0..4_800 {
            let sig = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48_000.0).sin() * 0.6;
            let out = dm_stereo.process(sig, sig);
            if i > 4_000 {
                peak_stereo = peak_stereo.max(out.abs());
            }
        }
        assert!(
            (peak_stereo - 0.6).abs() < 0.01,
            "Stereo input should average to ~0.6 without +6 dB boost, got {}",
            peak_stereo
        );

        // 3. Inf / NaN sample must not latch StereoDownmixer envelopes, and silence must flush to 0.0
        let _ = dm_mono.process(f32::INFINITY, f32::NAN);
        assert!(dm_mono.env0.is_finite() && dm_mono.env1.is_finite());
        for _ in 0..480_000 {
            let _ = dm_mono.process(0.0, 0.0);
        }
        assert_eq!(dm_mono.env0, 0.0);
        assert_eq!(dm_mono.env1, 0.0);

        // 4. Multi-channel (4ch) interface with single XLR mic on Ch0 keeps unity gain
        let mut dm4 = MultiChannelDownmixer::new(4);
        let mut peak_4ch = 0.0f32;
        for i in 0..4_800 {
            let sig = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 48_000.0).sin() * 0.25;
            let frame = [sig, 0.0, 0.0, 0.0];
            let out = dm4.process_frame(&frame);
            if i > 4_000 {
                peak_4ch = peak_4ch.max(out.abs());
            }
        }
        assert!(
            (peak_4ch - 0.25).abs() < 0.01,
            "Single active mic on 4-ch interface should keep unity gain (~0.25), got {}",
            peak_4ch
        );
    }

    #[test]
    fn test_monitor_volume_gain_curve() {
        assert_eq!(monitor_volume_to_gain(0.0), 0.0);
        assert!((monitor_volume_to_gain(0.5) - 1.0).abs() < 0.01);
        assert!(monitor_volume_to_gain(0.8) > 2.0 && monitor_volume_to_gain(0.8) < 2.2);
        assert!((monitor_volume_to_gain(1.0) - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_reconnect_backoff_and_error_dedup() {
        assert_eq!(reconnect_backoff_duration(0), Duration::from_millis(600));
        assert_eq!(reconnect_backoff_duration(1), Duration::from_millis(1200));
        assert_eq!(reconnect_backoff_duration(2), Duration::from_millis(2000));
        assert_eq!(reconnect_backoff_duration(3), Duration::from_millis(3000));
        assert_eq!(reconnect_backoff_duration(99), Duration::from_millis(3000));

        let (tx, rx) = mpsc::channel();
        let notifier = StreamErrorNotifier::new(tx, 7, StreamKind::Input);
        // Transient buffer underrun must NOT trigger a full pipeline teardown/reconnect
        notifier.notify(cpal::StreamError::BufferUnderrun);
        assert!(rx.try_recv().is_err());
        notifier.notify(cpal::StreamError::DeviceNotAvailable);
        notifier.notify(cpal::StreamError::DeviceNotAvailable);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "Duplicate errors on same stream must be suppressed");
    }
}
