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

use crate::audio::devices::{
    default_physical_input_name, find_input_device, find_output_device, list_audio_devices,
};
use crate::audio::processor::{soft_limit, NoiseProcessor, DFN_HOP_SIZE, DFN_SAMPLE_RATE};
use crate::audio::resampler::{AdaptiveOutputResampler, DriftController, InputResampler48k};
use crate::config::{AppConfig, ModelVariant};

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineDeviceStatus {
    pub connection_state: ConnectionState,
    pub waiting_device_name: Option<String>,
    pub active_input_name: Option<String>,
    pub active_output_name: Option<String>,
    pub active_monitor_name: Option<String>,
    pub input_sample_rate: u32,
    pub output_sample_rate: u32,
    pub reconnect_count: u32,
    pub input_drop_count: u64,
    pub output_underrun_count: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTelemetry {
    pub stream_running: bool,
    pub nc_enabled: bool,
    pub model_variant: ModelVariant,
    pub vad_gate_enabled: bool,
    pub vad_speaking: bool,
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
    pub model_variant_u8: AtomicU8,
    pub atten_lim_bits: AtomicU32,
    pub pf_beta_bits: AtomicU32,
    pub input_gain_bits: AtomicU32,
    pub min_db_thresh_bits: AtomicU32,
    pub vad_gate_enabled: AtomicBool,
    pub vad_threshold_bits: AtomicU32,
    pub vad_speaking: AtomicBool,
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
    pub input_drop_count: AtomicU64,
    pub output_underrun_count: AtomicU64,
    pub reconnect_count: AtomicU32,

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
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            nc_enabled: AtomicBool::new(cfg.nc_enabled),
            model_variant_u8: AtomicU8::new(variant_to_u8(cfg.model_variant)),
            atten_lim_bits: AtomicU32::new(cfg.atten_lim_db.to_bits()),
            pf_beta_bits: AtomicU32::new(cfg.post_filter_beta.to_bits()),
            input_gain_bits: AtomicU32::new(cfg.input_gain_db.to_bits()),
            min_db_thresh_bits: AtomicU32::new(cfg.min_db_thresh.to_bits()),
            vad_gate_enabled: AtomicBool::new(cfg.vad_gate_enabled),
            vad_threshold_bits: AtomicU32::new(cfg.vad_threshold_db.to_bits()),
            vad_speaking: AtomicBool::new(false),
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
            input_drop_count: AtomicU64::new(0),
            output_underrun_count: AtomicU64::new(0),
            reconnect_count: AtomicU32::new(0),

            status: Mutex::new(EngineDeviceStatus::default()),
        }
    }

    pub fn snapshot(&self) -> AudioTelemetry {
        let in_db = load_f32(&self.input_rms_db_bits);
        let out_db = load_f32(&self.output_rms_db_bits);
        let reduced = if self.nc_enabled.load(Ordering::Relaxed) {
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
            nc_enabled: self.nc_enabled.load(Ordering::Relaxed),
            model_variant: u8_to_variant(self.model_variant_u8.load(Ordering::Relaxed)),
            vad_gate_enabled: self.vad_gate_enabled.load(Ordering::Relaxed),
            vad_speaking: self.vad_speaking.load(Ordering::Relaxed),
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
enum StreamKind {
    Input,
    VirtualOutput,
    Monitor,
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
    },
    SetModelVariant(ModelVariant),
    Stop,
    SetLiveMonitor {
        enabled: bool,
        monitor_device: Option<String>,
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
}

struct MonitorSink {
    prod: ringbuf::HeapProd<f32>,
    resampler: AdaptiveOutputResampler,
    drift: DriftController,
    target_frames: usize,
}

struct RunningPipeline {
    stop_flag: Arc<AtomicBool>,
    worker_join: Option<JoinHandle<()>>,
    mon_tx: mpsc::Sender<Option<MonitorSink>>,
    active_input_name: String,
    _input_stream: Stream,
    _virtual_output_stream: Option<Stream>,
}

impl RunningPipeline {
    fn stop(mut self) {
        let _ = self.mon_tx.send(None);
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(jh) = self.worker_join.take() {
            jh.thread().unpark();
            let _ = jh.join();
        }
    }
}

struct MonitorPipeline {
    _stream: Stream,
}

impl AudioEngineHandle {
    pub fn spawn(initial_cfg: &AppConfig) -> Self {
        let shared = Arc::new(SharedAudioState::from_config(initial_cfg));
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>();

        let shared_ctrl = Arc::clone(&shared);
        let init_cfg = initial_cfg.clone();
        let ctrl_tx = cmd_tx.clone();

        thread::Builder::new()
            .name("shizuka-audio-ctrl".into())
            .spawn(move || {
                audio_control_loop(cmd_rx, ctrl_tx, shared_ctrl, init_cfg);
            })
            .expect("Failed to spawn audio controller thread");

        Self { cmd_tx, shared }
    }

    pub fn start_or_restart(
        &self,
        input_device: Option<String>,
        output_device: Option<String>,
        monitor_device: Option<String>,
    ) {
        let _ = self.cmd_tx.send(EngineCommand::StartOrRestart {
            input_device,
            output_device,
            monitor_device,
        });
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(EngineCommand::Stop);
    }

    pub fn set_nc_enabled(&self, enabled: bool) {
        self.shared.nc_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_params(
        &self,
        atten_lim_db: f32,
        pf_beta: f32,
        input_gain_db: f32,
        min_db_thresh: f32,
        vad_gate_enabled: bool,
        vad_threshold_db: f32,
    ) {
        store_f32(&self.shared.atten_lim_bits, atten_lim_db);
        store_f32(&self.shared.pf_beta_bits, pf_beta);
        store_f32(&self.shared.input_gain_bits, input_gain_db);
        store_f32(&self.shared.min_db_thresh_bits, min_db_thresh);
        self.shared
            .vad_gate_enabled
            .store(vad_gate_enabled, Ordering::Relaxed);
        store_f32(&self.shared.vad_threshold_bits, vad_threshold_db);
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
}

#[derive(Clone)]
struct StreamErrorNotifier {
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

    fn notify(&self, err: cpal::StreamError) {
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

fn reset_audio_meters(shared: &SharedAudioState) {
    store_f32(&shared.input_rms_db_bits, -90.0);
    store_f32(&shared.output_rms_db_bits, -90.0);
    store_f32(&shared.monitor_rms_db_bits, -90.0);
    shared.vad_speaking.store(false, Ordering::Relaxed);
}

fn audio_control_loop(
    cmd_rx: mpsc::Receiver<EngineCommand>,
    cmd_tx: mpsc::Sender<EngineCommand>,
    shared: Arc<SharedAudioState>,
    init_cfg: AppConfig,
) {
    let mut main_pipe: Option<RunningPipeline> = None;
    let mut live_mon_pipe: Option<MonitorPipeline> = None;

    let mut cur_in_dev = init_cfg.input_device.clone();
    let mut cur_out_dev = init_cfg.output_device.clone();
    let mut cur_mon_dev = init_cfg.monitor_device.clone();
    let mut want_running = init_cfg.stream_active;
    let mut want_mon = false;

    let mut main_gen: u64 = 0;
    let mut mon_gen: u64 = 0;
    let mut retry_attempt: u32 = 0;
    let mut next_retry_at: Option<Instant> = None;
    let mut pipe_started_at = Instant::now();
    let mut last_seen_frames: u64 = 0;
    let mut stall_ticks: u32 = 0;
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
                        if st
                            .last_error
                            .as_deref()
                            .is_some_and(|e| e.contains("再生デバイス") || e.contains("マイクテスト"))
                        {
                            st.last_error = None;
                        }
                    }
                    true
                }
                Err(e) => {
                    shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                    if let Ok(mut st) = shared.status.lock() {
                        st.active_monitor_name = None;
                        st.last_error = Some(format!("{:#}", e));
                    }
                    false
                }
            }
        } else {
            shared.live_monitor_enabled.store(false, Ordering::Relaxed);
            false
        }
    };

    let start_main = |in_name: Option<String>,
                      out_name: Option<String>,
                      mon_name: Option<String>,
                      want_mon: &mut bool,
                      main_gen: &mut u64,
                      mon_gen: &mut u64,
                      retry_attempt: &mut u32,
                      next_retry_at: &mut Option<Instant>,
                      pipe_started_at: &mut Instant,
                      last_seen_frames: &mut u64,
                      stall_ticks: &mut u32,
                      main_pipe: &mut Option<RunningPipeline>,
                      live_mon_pipe: &mut Option<MonitorPipeline>| {
        *main_gen = main_gen.wrapping_add(1);
        let current_gen = *main_gen;
        if let Some(old) = main_pipe.take() {
            old.stop();
        }
        let _ = live_mon_pipe.take();

        match build_main_pipeline(
            &shared,
            &cmd_tx,
            current_gen,
            in_name.as_deref(),
            out_name.as_deref(),
        ) {
            Ok(pipe) => {
                *main_pipe = Some(pipe);
                *retry_attempt = 0;
                *next_retry_at = None;
                *pipe_started_at = Instant::now();
                *last_seen_frames = shared.input_callback_frames.load(Ordering::Relaxed);
                *stall_ticks = 0;
                if *want_mon {
                    *want_mon = apply_live_monitor(true, mon_name, mon_gen, main_pipe, live_mon_pipe);
                }
            }
            Err(PipelineStartError::WaitingForDevice(missing_name)) => {
                shared.stream_running.store(false, Ordering::Relaxed);
                shared.virtual_out_connected.store(false, Ordering::Relaxed);
                shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                reset_audio_meters(&shared);
                if let Ok(mut st) = shared.status.lock() {
                    st.connection_state = ConnectionState::WaitingDevice;
                    st.waiting_device_name = missing_name;
                    st.active_input_name = None;
                    st.active_output_name = None;
                    st.active_monitor_name = None;
                    st.last_error = None;
                }
                *next_retry_at = Some(Instant::now() + reconnect_backoff_duration(*retry_attempt));
                *retry_attempt = retry_attempt.saturating_add(1);
            }
            Err(PipelineStartError::Other(e)) => {
                shared.stream_running.store(false, Ordering::Relaxed);
                shared.virtual_out_connected.store(false, Ordering::Relaxed);
                shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                reset_audio_meters(&shared);
                if let Ok(mut st) = shared.status.lock() {
                    st.connection_state = ConnectionState::Reconnecting;
                    st.waiting_device_name = in_name;
                    st.active_monitor_name = None;
                    st.last_error = Some(format!("{:#}", e));
                }
                *next_retry_at = Some(Instant::now() + reconnect_backoff_duration(*retry_attempt));
                *retry_attempt = retry_attempt.saturating_add(1);
            }
        }
    };

    if want_running {
        start_main(
            cur_in_dev.clone(),
            cur_out_dev.clone(),
            cur_mon_dev.clone(),
            &mut want_mon,
            &mut main_gen,
            &mut mon_gen,
            &mut retry_attempt,
            &mut next_retry_at,
            &mut pipe_started_at,
            &mut last_seen_frames,
            &mut stall_ticks,
            &mut main_pipe,
            &mut live_mon_pipe,
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
                } => {
                    cur_in_dev = input_device;
                    cur_out_dev = output_device;
                    cur_mon_dev = monitor_device;
                    want_running = true;
                    retry_attempt = 0;
                    start_main(
                        cur_in_dev.clone(),
                        cur_out_dev.clone(),
                        cur_mon_dev.clone(),
                        &mut want_mon,
                        &mut main_gen,
                        &mut mon_gen,
                        &mut retry_attempt,
                        &mut next_retry_at,
                        &mut pipe_started_at,
                        &mut last_seen_frames,
                        &mut stall_ticks,
                        &mut main_pipe,
                        &mut live_mon_pipe,
                    );
                }
                EngineCommand::SetModelVariant(variant) => {
                    let prev = u8_to_variant(shared.model_variant_u8.load(Ordering::Relaxed));
                    shared
                        .model_variant_u8
                        .store(variant_to_u8(variant), Ordering::SeqCst);
                    if main_pipe.is_some() && prev != variant {
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut retry_attempt,
                            &mut next_retry_at,
                            &mut pipe_started_at,
                            &mut last_seen_frames,
                            &mut stall_ticks,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                    }
                }
                EngineCommand::Stop => {
                    want_running = false;
                    want_mon = false;
                    main_gen = main_gen.wrapping_add(1);
                    mon_gen = mon_gen.wrapping_add(1);
                    next_retry_at = None;
                    retry_attempt = 0;
                    if let Some(old) = main_pipe.take() {
                        old.stop();
                    }
                    let _ = live_mon_pipe.take();
                    shared.stream_running.store(false, Ordering::Relaxed);
                    shared.virtual_out_connected.store(false, Ordering::Relaxed);
                    shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                    reset_audio_meters(&shared);
                    if let Ok(mut st) = shared.status.lock() {
                        st.connection_state = ConnectionState::Stopped;
                        st.waiting_device_name = None;
                        st.active_monitor_name = None;
                    }
                }
                EngineCommand::SetLiveMonitor {
                    enabled,
                    monitor_device,
                } => {
                    cur_mon_dev = monitor_device;
                    want_mon = enabled;
                    if enabled && main_pipe.is_none() {
                        want_running = true;
                        retry_attempt = 0;
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut retry_attempt,
                            &mut next_retry_at,
                            &mut pipe_started_at,
                            &mut last_seen_frames,
                            &mut stall_ticks,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                    } else {
                        want_mon = apply_live_monitor(
                            enabled,
                            cur_mon_dev.clone(),
                            &mut mon_gen,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                    }
                }
                EngineCommand::StreamError {
                    generation,
                    kind,
                    message,
                } => match kind {
                    StreamKind::Monitor => {
                        if generation == mon_gen && want_mon {
                            log::warn!(
                                "Live monitor stream interrupted ({}); attempting immediate reopen",
                                message
                            );
                            want_mon = apply_live_monitor(
                                true,
                                cur_mon_dev.clone(),
                                &mut mon_gen,
                                &mut main_pipe,
                                &mut live_mon_pipe,
                            );
                        }
                    }
                    StreamKind::Input | StreamKind::VirtualOutput => {
                        if generation == main_gen && want_running {
                            log::warn!(
                                "Audio {:?} stream interrupted ({}); scheduling auto-reconnect",
                                kind,
                                message
                            );
                            main_gen = main_gen.wrapping_add(1);
                            if let Some(old) = main_pipe.take() {
                                old.stop();
                            }
                            let _ = live_mon_pipe.take();
                            shared.stream_running.store(false, Ordering::Relaxed);
                            shared.virtual_out_connected.store(false, Ordering::Relaxed);
                            shared.live_monitor_enabled.store(false, Ordering::Relaxed);
                            shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                            reset_audio_meters(&shared);

                            let input_missing = find_input_device(cur_in_dev.as_deref()).is_none();
                            if let Ok(mut st) = shared.status.lock() {
                                st.active_input_name = None;
                                st.active_output_name = None;
                                st.active_monitor_name = None;
                                if input_missing {
                                    st.connection_state = ConnectionState::WaitingDevice;
                                    st.waiting_device_name = cur_in_dev.clone();
                                    st.last_error = None;
                                } else {
                                    st.connection_state = ConnectionState::Reconnecting;
                                    st.waiting_device_name = cur_in_dev.clone();
                                    st.last_error = Some(format!(
                                        "オーディオストリームを再接続しています: {}",
                                        message
                                    ));
                                }
                            }
                            retry_attempt = 0;
                            next_retry_at =
                                Some(Instant::now() + reconnect_backoff_duration(retry_attempt));
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
                    // 1. Sleep/wake & silent USB stall watchdog (after 2s startup grace period)
                    if pipe_started_at.elapsed() >= Duration::from_secs(2) {
                        let cur_frames = shared.input_callback_frames.load(Ordering::Relaxed);
                        if cur_frames == last_seen_frames {
                            stall_ticks = stall_ticks.saturating_add(1);
                            if stall_ticks >= 5 {
                                log::warn!(
                                    "Input audio stream stalled for 2.5s (sleep/wake or USB disconnect); reconnecting"
                                );
                                shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                                start_main(
                                    cur_in_dev.clone(),
                                    cur_out_dev.clone(),
                                    cur_mon_dev.clone(),
                                    &mut want_mon,
                                    &mut main_gen,
                                    &mut mon_gen,
                                    &mut retry_attempt,
                                    &mut next_retry_at,
                                    &mut pipe_started_at,
                                    &mut last_seen_frames,
                                    &mut stall_ticks,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                );
                                continue;
                            }
                        } else {
                            last_seen_frames = cur_frames;
                            stall_ticks = 0;
                        }
                    }

                    // 2. When set to "auto" (None), follow OS default physical microphone changes (~every 2s)
                    if cur_in_dev.is_none() && watchdog_ticks % 4 == 0 {
                        if let Some(os_default) = default_physical_input_name() {
                            let changed = main_pipe
                                .as_ref()
                                .is_some_and(|p| p.active_input_name != os_default);
                            if changed {
                                log::info!(
                                    "OS default microphone changed to '{}'; switching automatically",
                                    os_default
                                );
                                shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                                start_main(
                                    cur_in_dev.clone(),
                                    cur_out_dev.clone(),
                                    cur_mon_dev.clone(),
                                    &mut want_mon,
                                    &mut main_gen,
                                    &mut mon_gen,
                                    &mut retry_attempt,
                                    &mut next_retry_at,
                                    &mut pipe_started_at,
                                    &mut last_seen_frames,
                                    &mut stall_ticks,
                                    &mut main_pipe,
                                    &mut live_mon_pipe,
                                );
                                continue;
                            }
                        }
                    }

                    // 3. Reattach virtual output automatically if it reappears after driver install/reset (~every 3s)
                    if cur_out_dev.is_some()
                        && !shared.virtual_out_connected.load(Ordering::Relaxed)
                        && watchdog_ticks % 6 == 0
                        && find_output_device(cur_out_dev.as_deref(), false).is_some()
                    {
                        log::info!("Configured virtual output reappeared; reconnecting pipeline");
                        shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut retry_attempt,
                            &mut next_retry_at,
                            &mut pipe_started_at,
                            &mut last_seen_frames,
                            &mut stall_ticks,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                    }
                } else if next_retry_at.is_none_or(|t| Instant::now() >= t) {
                    // Waiting for disconnected physical microphone or transient retry
                    if find_input_device(cur_in_dev.as_deref()).is_some() {
                        shared.reconnect_count.fetch_add(1, Ordering::Relaxed);
                        start_main(
                            cur_in_dev.clone(),
                            cur_out_dev.clone(),
                            cur_mon_dev.clone(),
                            &mut want_mon,
                            &mut main_gen,
                            &mut mon_gen,
                            &mut retry_attempt,
                            &mut next_retry_at,
                            &mut pipe_started_at,
                            &mut last_seen_frames,
                            &mut stall_ticks,
                            &mut main_pipe,
                            &mut live_mon_pipe,
                        );
                    } else {
                        if let Ok(mut st) = shared.status.lock() {
                            st.connection_state = ConnectionState::WaitingDevice;
                            st.waiting_device_name = cur_in_dev.clone();
                        }
                        next_retry_at =
                            Some(Instant::now() + reconnect_backoff_duration(retry_attempt));
                        retry_attempt = retry_attempt.saturating_add(1);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(old) = main_pipe.take() {
                    old.stop();
                }
                let _ = live_mon_pipe.take();
                break;
            }
        }
    }
}

fn build_live_monitor_stream(
    mon_name: Option<&str>,
    err_notifier: StreamErrorNotifier,
) -> Result<(MonitorPipeline, MonitorSink, String)> {
    let mon_dev = find_output_device(mon_name, true)
        .ok_or_else(|| anyhow!("マイクテスト用のスピーカー／ヘッドホンが見つかりません"))?;
    let actual_mon_name = mon_dev
        .name()
        .unwrap_or_else(|_| "Default Output".to_string());
    let supported_mon = mon_dev
        .default_output_config()
        .with_context(|| format!("再生デバイス「{}」のフォーマット取得に失敗しました", actual_mon_name))?;
    let mon_fmt = supported_mon.sample_format();
    let mon_cfg: StreamConfig = supported_mon.into();
    let sr_mon = mon_cfg.sample_rate.0;
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
    .with_context(|| format!("再生デバイス「{}」のストリーム作成に失敗しました", actual_mon_name))?;
    stream
        .play()
        .with_context(|| format!("再生デバイス「{}」の再生開始に失敗しました", actual_mon_name))?;

    Ok((
        MonitorPipeline { _stream: stream },
        MonitorSink {
            prod: mon_prod,
            resampler,
            drift,
            target_frames: mon_target,
        },
        actual_mon_name,
    ))
}

fn build_main_pipeline(
    shared: &Arc<SharedAudioState>,
    cmd_tx: &mpsc::Sender<EngineCommand>,
    generation: u64,
    in_name: Option<&str>,
    out_name: Option<&str>,
) -> std::result::Result<RunningPipeline, PipelineStartError> {
    let in_dev = find_input_device(in_name).ok_or_else(|| {
        PipelineStartError::WaitingForDevice(in_name.filter(|s| !s.is_empty()).map(String::from))
    })?;
    let actual_in_name = in_dev.name().unwrap_or_else(|_| "Unknown Mic".into());
    let supported_in = in_dev
        .default_input_config()
        .context("マイクの標準入力フォーマットを取得できません")
        .map_err(PipelineStartError::Other)?;
    let in_sample_format = supported_in.sample_format();
    let mut in_cfg: StreamConfig = supported_in.into();
    // Request a low-latency buffer (~10ms) when supported by WASAPI/CoreAudio
    in_cfg.buffer_size = cpal::BufferSize::Default;
    let sr_in = in_cfg.sample_rate.0;
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
                    let resolved_out_name = out_dev
                        .name()
                        .unwrap_or_else(|_| target_out.to_string());
                    let out_fmt = supported_out.sample_format();
                    let out_cfg: StreamConfig = supported_out.into();
                    sr_out = out_cfg.sample_rate.0;
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
                            virtual_out_warning = Some(format!(
                                "仮想出力「{}」のストリーム作成に失敗しました: {:#}",
                                resolved_out_name, e
                            ));
                        }
                    }
                }
                Err(e) => {
                    virtual_out_warning = Some(format!(
                        "仮想出力「{}」のフォーマット取得に失敗しました: {}",
                        target_out, e
                    ));
                }
            },
            None => {
                virtual_out_warning = Some(format!(
                    "仮想出力「{}」が見つかりません（再接続待ち）",
                    target_out
                ));
            }
        }
    }

    // Initialize Resamplers BEFORE launching real-time thread (DfTract holds Rc<Tensor> so it initializes on the worker thread)
    let init_variant = u8_to_variant(shared.model_variant_u8.load(Ordering::Relaxed));
    let init_atten = load_f32(&shared.atten_lim_bits);
    let init_beta = load_f32(&shared.pf_beta_bits);
    let init_thresh = load_f32(&shared.min_db_thresh_bits);
    let init_vad_en = shared.vad_gate_enabled.load(Ordering::Relaxed);
    let init_vad_thresh = load_f32(&shared.vad_threshold_bits);
    let mut in_resampler =
        InputResampler48k::new(sr_in).map_err(PipelineStartError::Other)?;
    let mut out_resampler =
        AdaptiveOutputResampler::new(sr_out).map_err(PipelineStartError::Other)?;
    let mut drift_ctrl = DriftController::new(target_out_frames, sr_out);

    let (mon_tx, mon_rx) = mpsc::channel::<Option<MonitorSink>>();
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
            let alg_latency_ms = processor.algorithmic_latency_ms();

            // Promote worker thread to OS real-time audio priority (Windows MMCSS Pro Audio / macOS CoreAudio RT)
            let rt_handle =
                promote_current_thread_to_real_time(DFN_HOP_SIZE as u32, DFN_SAMPLE_RATE as u32)
                    .ok();

            let mut in_chunk_scratch = vec![0.0f32; 4_096];
            let silence_scratch = vec![0.0f32; 4_096];
            let mut frame_48k_in = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_raw = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_enh = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_out = [0.0f32; DFN_HOP_SIZE];
            let mut frame_48k_mon = [0.0f32; DFN_HOP_SIZE];
            let mut mon_sink_opt: Option<MonitorSink> = None;

            let mut smooth_in_db = -85.0f32;
            let mut smooth_enh_db = -85.0f32;
            let mut smooth_out_db = -85.0f32;

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
                    thread::park_timeout(Duration::from_millis(4));
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

                    // Sync lock-free parameters from UI
                    processor.set_enabled(shared_worker.nc_enabled.load(Ordering::Relaxed));
                    processor.set_attenuation_limit(load_f32(&shared_worker.atten_lim_bits));
                    processor.set_post_filter_beta(load_f32(&shared_worker.pf_beta_bits));
                    processor.set_input_gain_db(load_f32(&shared_worker.input_gain_bits));
                    processor.set_min_db_thresh(load_f32(&shared_worker.min_db_thresh_bits));
                    processor.set_vad_gate(
                        shared_worker.vad_gate_enabled.load(Ordering::Relaxed),
                        load_f32(&shared_worker.vad_threshold_bits),
                    );

                    if in_resampler
                        .process(&in_chunk_scratch[..needed], &mut frame_48k_in)
                        .is_err()
                    {
                        continue;
                    }

                    let (lsnr, vad_speaking) = processor.process_frame_48k(
                        &frame_48k_in,
                        &mut frame_48k_raw,
                        &mut frame_48k_enh,
                        &mut frame_48k_out,
                    );
                    shared_worker
                        .vad_speaking
                        .store(vad_speaking, Ordering::Relaxed);

                    // Push to Virtual Microphone Output with PI Clock Drift Compensation
                    if let Some(out_prod) = &mut out_prod_opt {
                        let occ = out_prod.occupied_len();
                        let cap = out_prod.capacity().get();

                        // Guard against extreme OS stall (e.g. PC sleep/wake): recenter buffer cleanly
                        let rel_ratio = if occ > cap * 3 / 4 {
                            drift_ctrl.reset();
                            0.990 // Drain excess smoothly
                        } else if occ == 0 {
                            let fill = (target_out_frames / 2).min(silence_scratch.len());
                            let _ = out_prod.push_slice(&silence_scratch[..fill]);
                            drift_ctrl.reset();
                            1.0
                        } else {
                            drift_ctrl.update(occ)
                        };

                        if let Ok(resampled_out) =
                            out_resampler.process(&frame_48k_out, rel_ratio)
                        {
                            let _ = out_prod.push_slice(resampled_out);
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

                    // Drain any live-monitor attach/detach updates without blocking
                    while let Ok(next_sink) = mon_rx.try_recv() {
                        mon_sink_opt = next_sink;
                    }

                    // Push to Discord-style Live Mic Test Monitor with smooth A/B crossfade & volume gain
                    let want_mon_clean = shared_worker.monitor_clean.load(Ordering::Relaxed);
                    let target_mon_mix = if want_mon_clean { 1.0f32 } else { 0.0f32 };
                    let target_mon_gain =
                        monitor_volume_to_gain(load_f32(&shared_worker.monitor_volume_bits));

                    if let Some(sink) = &mut mon_sink_opt {
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
                        let rel = if occ > cap * 3 / 4 {
                            sink.drift.reset();
                            0.990
                        } else if occ == 0 {
                            let fill = (sink.target_frames / 2).min(silence_scratch.len());
                            let _ = sink.prod.push_slice(&silence_scratch[..fill]);
                            sink.drift.reset();
                            1.0
                        } else {
                            sink.drift.update(occ)
                        };
                        if let Ok(mon_samples) = sink.resampler.process(&frame_48k_mon, rel) {
                            let _ = sink.prod.push_slice(mon_samples);
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

                    let elapsed_us = t0.elapsed().as_micros() as u32;
                    shared_worker
                        .process_time_us
                        .store(elapsed_us, Ordering::Relaxed);
                }
            }

            if let Some(h) = rt_handle {
                let _ = demote_current_thread_from_real_time(h);
            }
        })
        .map_err(|e| PipelineStartError::Other(e.into()))?;

    init_rx
        .recv()
        .map_err(|_| {
            PipelineStartError::Other(anyhow!("AI推論スレッドの初期化待機中に中断されました"))
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
                    .unwrap_or_else(|| "仮想出力".to_string());
                virtual_out_warning = Some(format!(
                    "仮想出力「{}」の再生開始に失敗しました: {}",
                    label, e
                ));
            }
        }
    }

    let worker_thread_handle = worker_join.thread().clone();
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
        .context("マイク入力ストリームの開始に失敗しました")
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
        st.last_error = virtual_out_warning;
    }

    Ok(RunningPipeline {
        stop_flag,
        worker_join: Some(worker_join),
        mon_tx,
        active_input_name: actual_in_name,
        _input_stream: input_stream,
        _virtual_output_stream: virtual_stream_opt,
    })
}

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
        other => Err(anyhow!("未対応の入力サンプル形式: {:?}", other)),
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
        let a = l.abs();
        let b = r.abs();
        self.env0 = if a > self.env0 {
            0.9 * self.env0 + 0.1 * a
        } else {
            self.env0 * 0.99995
        };
        self.env1 = if b > self.env1 {
            0.9 * self.env1 + 0.1 * b
        } else {
            self.env1 * 0.99995
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
    let inv_ch = 1.0f32 / (ch as f32);
    let mut stereo_dm = StereoDownmixer::new();

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
                        mono_scratch[count] = f32::from_sample(s);
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
                    for frame in data.chunks_exact(2) {
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
                    for frame in data.chunks(ch) {
                        let mut sum = 0.0f32;
                        for &s in frame {
                            sum += f32::from_sample(s);
                        }
                        mono_scratch[count] = sum * inv_ch;
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
        other => Err(anyhow!("未対応の出力サンプル形式: {:?}", other)),
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

                for i in 0..batch {
                    let val = if i < popped {
                        last_sample = mono_scratch[i];
                        mono_scratch[i]
                    } else {
                        // Gentle exponential decay to zero on rare underrun to avoid sharp click
                        last_sample *= 0.95;
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
            if had_underrun {
                if let Some(ref sh) = underrun_shared {
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
            for i in 0..DFN_HOP_SIZE {
                let t = (f * DFN_HOP_SIZE + i) as f32 / 48_000.0;
                frame[i] = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
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
        notifier.notify(cpal::StreamError::DeviceNotAvailable);
        notifier.notify(cpal::StreamError::DeviceNotAvailable);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "Duplicate errors on same stream must be suppressed");
    }
}
