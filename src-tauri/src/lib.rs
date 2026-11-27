pub mod audio;
pub mod config;
pub mod diagnostics;
pub mod i18n;
pub mod installer;
pub mod shortcuts;
pub mod updater;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};
use tauri_plugin_global_shortcut::ShortcutState;

use audio::cue::{CueKind, CuePlayer};
use audio::devices::{default_physical_input_name, list_audio_devices, DeviceListResponse};
use audio::engine::{AudioEngineHandle, AudioTelemetry, DspParams};
use config::{AppConfig, ConfigSaver, ModelVariant};
use i18n::{tr, tr_for, LanguagePreference, Locale};
use installer::{install_virtual_audio_driver, InstallDriverResult};
use shortcuts::{ShortcutAction, ShortcutRegistry};
use updater::{
    check_for_update_impl, install_update_impl, is_current_build_portable,
    open_releases_page_in_browser, UpdateCheckResult, UpdateProgressSnapshot,
};

const TRAY_ID: &str = "shizuka-tray";

fn tray_tooltip(locale: Locale, nc_enabled: bool, mic_muted: bool) -> &'static str {
    if mic_muted {
        tr_for(
            locale,
            "Shizuka — マイクミュート中",
            "Shizuka — Microphone Muted",
        )
    } else if nc_enabled {
        tr_for(
            locale,
            "Shizuka — AIノイズ除去: ON",
            "Shizuka — AI Noise Cancellation: ON",
        )
    } else {
        tr_for(
            locale,
            "Shizuka — AIノイズ除去: OFF (バイパス中)",
            "Shizuka — AI Noise Cancellation: OFF (Bypass)",
        )
    }
}

fn tray_menu_labels(locale: Locale) -> (&'static str, &'static str, &'static str, &'static str) {
    (
        tr_for(locale, "Shizuka を開く", "Open Shizuka"),
        tr_for(locale, "AI ノイズ除去", "AI Noise Cancellation"),
        tr_for(locale, "マイクミュート", "Mute Microphone"),
        tr_for(locale, "終了", "Quit"),
    )
}

struct TrayUiHandles {
    icon_on: Image<'static>,
    icon_off: Image<'static>,
    icon_mute: Image<'static>,
    show_item: MenuItem<tauri::Wry>,
    nc_item: CheckMenuItem<tauri::Wry>,
    mute_item: CheckMenuItem<tauri::Wry>,
    quit_item: MenuItem<tauri::Wry>,
}

pub struct AppState {
    pub engine: AudioEngineHandle,
    pub config: Arc<Mutex<AppConfig>>,
    pub config_saver: ConfigSaver,
    pub cue: CuePlayer,
    pub shortcuts: Mutex<ShortcutRegistry>,
    pub update_progress: Arc<Mutex<UpdateProgressSnapshot>>,
    tray_handles: Mutex<Option<TrayUiHandles>>,
    start_minimized: bool,
    window_shown_once: AtomicBool,
    shutting_down: AtomicBool,
}

fn update_tray_ui(app: &AppHandle, nc_enabled: bool, mic_muted: bool) {
    let st = app.state::<AppState>();
    let tooltip = tray_tooltip(i18n::current_locale(), nc_enabled, mic_muted);

    if let Ok(guard) = st.tray_handles.lock() {
        if let Some(ref handles) = *guard {
            let _ = handles.nc_item.set_checked(nc_enabled);
            let _ = handles.mute_item.set_checked(mic_muted);
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                let img = if mic_muted {
                    handles.icon_mute.clone()
                } else if nc_enabled {
                    handles.icon_on.clone()
                } else {
                    handles.icon_off.clone()
                };
                let _ = tray.set_icon(Some(img));
                let _ = tray.set_tooltip(Some(tooltip));
            }
            return;
        }
    }

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

fn refresh_tray_locale(app: &AppHandle, locale: Locale) {
    let st = app.state::<AppState>();
    let (show_lbl, nc_lbl, mute_lbl, quit_lbl) = tray_menu_labels(locale);
    if let Ok(guard) = st.tray_handles.lock() {
        if let Some(ref handles) = *guard {
            let _ = handles.show_item.set_text(show_lbl);
            let _ = handles.nc_item.set_text(nc_lbl);
            let _ = handles.mute_item.set_text(mute_lbl);
            let _ = handles.quit_item.set_text(quit_lbl);
        }
    }
    let nc_enabled = st.engine.shared.nc_enabled.load(Ordering::Relaxed);
    let mic_muted = st.engine.shared.mic_muted.load(Ordering::Relaxed);
    update_tray_ui(app, nc_enabled, mic_muted);
}

fn unpause_shortcuts(app: &AppHandle) {
    let st = app.state::<AppState>();
    if let Ok(mut reg) = st.shortcuts.lock() {
        let _ = reg.set_paused(app, false);
    };
}

fn resolve_active_mic_name(state: &AppState, cfg: &AppConfig) -> Option<String> {
    if let Ok(st) = state.engine.shared.status.lock() {
        if let Some(ref active) = st.active_input_name {
            if !active.is_empty() {
                return Some(active.clone());
            }
        }
    }
    cfg.input_device
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(default_physical_input_name)
}

fn apply_nc_enabled(app: &AppHandle, enabled: bool, play_cue: bool) {
    let st = app.state::<AppState>();
    st.engine.set_nc_enabled(enabled);
    let (sound_enabled, mon_dev, snapshot_cfg) = if let Ok(mut cfg) = st.config.lock() {
        cfg.nc_enabled = enabled;
        (
            cfg.shortcut_sound_enabled,
            cfg.monitor_device.clone(),
            Some(cfg.clone()),
        )
    } else {
        (false, None, None)
    };
    if let Some(cfg) = snapshot_cfg {
        st.config_saver.flush_async(cfg);
    }
    let mic_muted = st.engine.shared.mic_muted.load(Ordering::Relaxed);
    update_tray_ui(app, enabled, mic_muted);
    let _ = app.emit("shizuka-state-changed", ());
    if play_cue && sound_enabled {
        st.cue.play(
            if enabled {
                CueKind::NcOn
            } else {
                CueKind::NcOff
            },
            mon_dev,
        );
    }
}

fn apply_mic_muted(app: &AppHandle, muted: bool, play_cue: bool) {
    let st = app.state::<AppState>();
    st.engine.set_mic_muted(muted);
    let (sound_enabled, mon_dev) = if let Ok(cfg) = st.config.lock() {
        (cfg.shortcut_sound_enabled, cfg.monitor_device.clone())
    } else {
        (false, None)
    };
    let nc_enabled = st.engine.shared.nc_enabled.load(Ordering::Relaxed);
    update_tray_ui(app, nc_enabled, muted);
    let _ = app.emit("shizuka-state-changed", ());
    if play_cue && sound_enabled {
        st.cue.play(
            if muted {
                CueKind::MuteOn
            } else {
                CueKind::MuteOff
            },
            mon_dev,
        );
    }
}

fn shutdown_cleanly(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.shutting_down.swap(true, Ordering::AcqRel) {
        return;
    }
    diagnostics::info("Shutting down Shizuka cleanly");
    if let Ok(cfg) = st.config.lock() {
        st.config_saver.flush_sync(cfg.clone(), Duration::from_secs(2));
    }
    let stopped = st.engine.stop_and_wait(Duration::from_secs(3));
    if !stopped {
        diagnostics::warn("Audio engine stop timed out after 3s during shutdown");
    }
}

#[tauri::command]
fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn frontend_ready(app: AppHandle, state: State<'_, AppState>) -> bool {
    if !state.start_minimized {
        if !state.window_shown_once.swap(true, Ordering::AcqRel) {
            show_main_window(&app);
        }
        true
    } else {
        false
    }
}

#[tauri::command]
fn get_devices() -> DeviceListResponse {
    list_audio_devices()
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    let mut cfg = state
        .config
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    if cfg.output_device.is_none() {
        let devs = list_audio_devices();
        if let Some(rec) = devs.recommended_virtual_output {
            cfg.output_device = Some(rec);
        }
    }
    cfg
}

#[tauri::command]
fn get_telemetry(state: State<'_, AppState>) -> AudioTelemetry {
    state.engine.shared.snapshot()
}

#[tauri::command]
fn set_nc_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_nc_enabled(&app, enabled, false);
    Ok(())
}

#[tauri::command]
fn set_mic_muted(app: AppHandle, muted: bool) -> Result<(), String> {
    apply_mic_muted(&app, muted, false);
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn update_dsp_params(
    state: State<'_, AppState>,
    atten_lim_db: f32,
    post_filter_beta: f32,
    input_gain_db: f32,
    min_db_thresh: f32,
    vad_gate_enabled: bool,
    vad_threshold_db: f32,
    hpf_enabled: bool,
    agc_enabled: bool,
    aec_enabled: bool,
) -> Result<(), String> {
    let atten_lim_db = if atten_lim_db.is_finite() {
        atten_lim_db.clamp(6.0, 100.0)
    } else {
        65.0
    };
    let post_filter_beta = if post_filter_beta.is_finite() {
        post_filter_beta.clamp(0.0, 0.06)
    } else {
        0.0
    };
    let input_gain_db = if input_gain_db.is_finite() {
        input_gain_db.clamp(-24.0, 24.0)
    } else {
        0.0
    };
    let min_db_thresh = if min_db_thresh.is_finite() {
        min_db_thresh.clamp(-20.0, 20.0)
    } else {
        -16.0
    };
    let vad_threshold_db = if vad_threshold_db.is_finite() {
        vad_threshold_db.clamp(-18.0, 12.0)
    } else {
        -5.0
    };

    let prev_aec = state.engine.shared.aec_enabled.load(Ordering::Relaxed);

    // Update real-time DSP atomics immediately (zero lock / zero disk I/O)
    state.engine.set_params(DspParams {
        atten_lim_db,
        post_filter_beta,
        input_gain_db,
        min_db_thresh,
        vad_gate_enabled,
        vad_threshold_db,
        hpf_enabled,
        agc_enabled,
        aec_enabled,
    });

    // Debounce disk persistence on background thread so slider drags never trigger fsync storms
    let (aec_ref_dev, snapshot_cfg) = if let Ok(mut cfg) = state.config.lock() {
        cfg.atten_lim_db = atten_lim_db;
        cfg.post_filter_beta = post_filter_beta;
        cfg.input_gain_db = input_gain_db;
        cfg.min_db_thresh = min_db_thresh;
        cfg.vad_gate_enabled = vad_gate_enabled;
        cfg.vad_threshold_db = vad_threshold_db;
        cfg.hpf_enabled = hpf_enabled;
        cfg.agc_enabled = agc_enabled;
        cfg.aec_enabled = aec_enabled;
        if let Some(mic_name) = resolve_active_mic_name(&state, &cfg) {
            cfg.save_profile_for_mic(&mic_name);
        }
        (cfg.aec_reference_device.clone(), Some(cfg.clone()))
    } else {
        (None, None)
    };
    if prev_aec != aec_enabled {
        state.engine.set_aec_config(aec_enabled, aec_ref_dev);
    }
    if let Some(cfg) = snapshot_cfg {
        state.config_saver.schedule(cfg);
    }
    Ok(())
}

#[tauri::command]
fn set_aec_reference_device(
    state: State<'_, AppState>,
    reference_device: Option<String>,
) -> Result<AppConfig, String> {
    let clean_ref = reference_device.filter(|s| !s.is_empty());
    let (aec_enabled, updated_cfg) = if let Ok(mut cfg) = state.config.lock() {
        cfg.aec_reference_device = clean_ref.clone();
        (cfg.aec_enabled, cfg.clone())
    } else {
        (false, AppConfig::default())
    };
    state.config_saver.flush_async(updated_cfg.clone());
    state.engine.set_aec_config(aec_enabled, clean_ref);
    Ok(updated_cfg)
}

#[tauri::command]
fn set_voice_gate_config(
    state: State<'_, AppState>,
    enabled: bool,
    threshold: f32,
) -> Result<AppConfig, String> {
    let clamped = if threshold.is_finite() {
        threshold.clamp(0.16, 0.48)
    } else {
        audio::speaker::DEFAULT_VOICE_GATE_THRESHOLD
    };
    let updated_cfg = {
        let mut cfg = state
            .config
            .lock()
            .map_err(|_| {
                tr(
                    "設定のロック取得に失敗しました",
                    "Failed to acquire settings lock",
                )
                .to_string()
            })?;
        cfg.voice_gate_threshold = clamped;
        cfg.voice_gate_enabled = enabled;
        cfg.clone()
    };
    state.engine.speaker.set_gate_config(enabled, clamped);
    state.config_saver.schedule(updated_cfg.clone());
    Ok(updated_cfg)
}

#[tauri::command]
fn reset_voice_profile(state: State<'_, AppState>) -> Result<AppConfig, String> {
    state.engine.speaker.reset_profile();
    let updated_cfg = {
        let mut cfg = state
            .config
            .lock()
            .map_err(|_| {
                tr(
                    "設定のロック取得に失敗しました",
                    "Failed to acquire settings lock",
                )
                .to_string()
            })?;
        cfg.voice_profile = None;
        cfg.voice_fbank_tilt = None;
        cfg.clone()
    };
    state.config_saver.flush_async(updated_cfg.clone());
    Ok(updated_cfg)
}

#[tauri::command]
fn set_model_variant(state: State<'_, AppState>, model_variant: ModelVariant) -> Result<(), String> {
    let snapshot_cfg = if let Ok(mut cfg) = state.config.lock() {
        cfg.model_variant = model_variant;
        Some(cfg.clone())
    } else {
        None
    };
    if let Some(cfg) = snapshot_cfg {
        state.config_saver.flush_async(cfg);
    }
    state.engine.set_model_variant(model_variant);
    Ok(())
}

#[tauri::command]
fn select_devices(
    state: State<'_, AppState>,
    input_device: Option<String>,
    output_device: Option<String>,
    monitor_device: Option<String>,
) -> Result<AppConfig, String> {
    let clean_in = input_device.filter(|s| !s.is_empty());
    let clean_out = output_device.filter(|s| !s.is_empty());
    let clean_mon = monitor_device.filter(|s| !s.is_empty());

    let prev_active_mic = state
        .engine
        .shared
        .status
        .lock()
        .ok()
        .and_then(|s| s.active_input_name.clone());

    let (
        should_run,
        main_devices_changed,
        monitor_changed,
        profile_applied,
        synced_profile_mic,
        updated_cfg,
    ) = if let Ok(mut cfg) = state.config.lock() {
        let input_changed = cfg.input_device != clean_in;
        let main_changed = input_changed || cfg.output_device != clean_out;
        let mon_changed = cfg.monitor_device != clean_mon;

        let mut applied_now = false;
        let mut target_profile_mic = None;
        if input_changed {
            let prev_mic = prev_active_mic.or_else(|| cfg.input_device.clone());
            let next_mic = clean_in.clone().or_else(default_physical_input_name);
            if let Some(ref target_mic) = next_mic {
                let _ = cfg.switch_to_mic_profile(prev_mic.as_deref(), target_mic);
                state.engine.shared.apply_config_dsp(&cfg);
                applied_now = true;
                target_profile_mic = Some(target_mic.clone());
            }
        }

        cfg.input_device = clean_in.clone();
        cfg.output_device = clean_out.clone();
        cfg.monitor_device = clean_mon.clone();
        let cloned = cfg.clone();
        (
            cfg.stream_active,
            main_changed,
            mon_changed,
            applied_now,
            target_profile_mic,
            cloned,
        )
    } else {
        (true, true, true, false, None, AppConfig::default())
    };

    state.config_saver.flush_async(updated_cfg.clone());

    let mon_active = state
        .engine
        .shared
        .live_monitor_enabled
        .load(Ordering::Relaxed);

    if should_run && main_devices_changed {
        state
            .engine
            .start_or_restart(clean_in, clean_out, clean_mon, profile_applied);
    } else {
        // Keep audio controller's `last_profile_mic` in sync when switching mics while stopped
        // so resuming later never overwrites the previous mic's saved profile.
        if profile_applied {
            state.engine.sync_last_profile_mic(synced_profile_mic);
        }
        if monitor_changed && mon_active {
            state.engine.set_live_monitor(true, clean_mon);
        }
    }
    Ok(updated_cfg)
}

#[tauri::command]
fn set_stream_active(state: State<'_, AppState>, active: bool) -> Result<(), String> {
    let (in_dev, out_dev, mon_dev, snapshot_cfg) = if let Ok(mut cfg) = state.config.lock() {
        cfg.stream_active = active;
        (
            cfg.input_device.clone(),
            cfg.output_device.clone(),
            cfg.monitor_device.clone(),
            Some(cfg.clone()),
        )
    } else {
        (None, None, None, None)
    };
    if let Some(cfg) = snapshot_cfg {
        state.config_saver.flush_async(cfg);
    }

    if active {
        state
            .engine
            .start_or_restart(in_dev, out_dev, mon_dev, false);
    } else {
        state.engine.stop();
    }
    Ok(())
}

#[tauri::command]
fn set_live_monitor(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mon_dev = state
        .config
        .lock()
        .ok()
        .and_then(|c| c.monitor_device.clone());
    state.engine.set_live_monitor(enabled, mon_dev);
    Ok(())
}

#[tauri::command]
fn set_monitor_mode(state: State<'_, AppState>, clean: bool) -> Result<(), String> {
    state.engine.set_monitor_mode(clean);
    Ok(())
}

#[tauri::command]
fn set_monitor_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
    let clamped = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        0.8
    };
    state.engine.set_monitor_volume(clamped);
    let snapshot_cfg = if let Ok(mut cfg) = state.config.lock() {
        cfg.monitor_volume = clamped;
        Some(cfg.clone())
    } else {
        None
    };
    if let Some(cfg) = snapshot_cfg {
        state.config_saver.schedule(cfg);
    }
    Ok(())
}

#[tauri::command]
fn set_shortcut(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ShortcutAction,
    accelerator: Option<String>,
) -> Result<AppConfig, String> {
    let normalized = {
        let mut reg = state.shortcuts.lock().map_err(|_| {
            tr(
                "ショートカット管理のロック取得に失敗しました",
                "Failed to acquire shortcut manager lock",
            )
            .to_string()
        })?;
        reg.update_binding(&app, action, accelerator.as_deref())?
    };

    let cloned = {
        let mut cfg = state.config.lock().map_err(|_| {
            tr(
                "設定のロック取得に失敗しました",
                "Failed to acquire settings lock",
            )
            .to_string()
        })?;
        match action {
            ShortcutAction::ToggleNc => cfg.shortcut_toggle_nc = normalized,
            ShortcutAction::ToggleMute => cfg.shortcut_toggle_mute = normalized,
        }
        cfg.clone()
    };
    state.config_saver.flush_async(cloned.clone());
    Ok(cloned)
}

#[tauri::command]
fn set_shortcuts_paused(
    app: AppHandle,
    state: State<'_, AppState>,
    paused: bool,
) -> Result<(), String> {
    let mut reg = state.shortcuts.lock().map_err(|_| {
        tr(
            "ショートカット管理のロック取得に失敗しました",
            "Failed to acquire shortcut manager lock",
        )
        .to_string()
    })?;
    reg.set_paused(&app, paused)
}

#[tauri::command]
fn set_shortcut_sound_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let snapshot_cfg = if let Ok(mut cfg) = state.config.lock() {
        cfg.shortcut_sound_enabled = enabled;
        Some(cfg.clone())
    } else {
        None
    };
    if let Some(cfg) = snapshot_cfg {
        state.config_saver.flush_async(cfg);
    }
    Ok(())
}

#[tauri::command]
fn get_resolved_locale(state: State<'_, AppState>) -> Locale {
    let pref = state
        .config
        .lock()
        .map(|g| g.language)
        .unwrap_or_default();
    let resolved = pref.resolve();
    i18n::set_active_locale(resolved);
    resolved
}

#[tauri::command]
fn set_language(
    app: AppHandle,
    state: State<'_, AppState>,
    language: LanguagePreference,
) -> Result<Locale, String> {
    let snapshot_cfg = {
        let mut cfg = state.config.lock().map_err(|_| {
            tr(
                "設定のロック取得に失敗しました",
                "Failed to acquire settings lock",
            )
            .to_string()
        })?;
        cfg.language = language;
        cfg.clone()
    };
    state.config_saver.flush_async(snapshot_cfg);

    let resolved = language.resolve();
    i18n::set_active_locale(resolved);
    refresh_tray_locale(&app, resolved);
    let _ = app.emit("shizuka-state-changed", ());
    Ok(resolved)
}

#[tauri::command]
fn get_shortcut_warning(state: State<'_, AppState>) -> Option<String> {
    state
        .shortcuts
        .lock()
        .ok()
        .and_then(|reg| reg.startup_warning())
}

#[tauri::command]
async fn install_virtual_driver(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<InstallDriverResult, String> {
    let resource_dir = app.path().resource_dir().ok();
    let result = tauri::async_runtime::spawn_blocking(move || {
        install_virtual_audio_driver(resource_dir)
    })
    .await
    .map_err(|e| match i18n::current_locale() {
        Locale::Ja => format!("セットアップタスク実行エラー: {}", e),
        Locale::En => format!("Driver setup task failed: {}", e),
    })?
    .map_err(|e| format!("{:#}", e))?;

    if result.device_detected_now {
        if let Some(ref virt_out) = result.detected_device_name {
            let (in_dev, mon_dev, should_run, snapshot_cfg) =
                if let Ok(mut cfg) = state.config.lock() {
                    cfg.output_device = Some(virt_out.clone());
                    (
                        cfg.input_device.clone(),
                        cfg.monitor_device.clone(),
                        cfg.stream_active,
                        Some(cfg.clone()),
                    )
                } else {
                    (None, None, true, None)
                };
            if let Some(cfg) = snapshot_cfg {
                state.config_saver.flush_async(cfg);
            }

            if should_run {
                state
                    .engine
                    .start_or_restart(in_dev, Some(virt_out.clone()), mon_dev, true);
            }
        }
    }

    Ok(result)
}

#[tauri::command]
async fn check_for_update(app: AppHandle) -> Result<UpdateCheckResult, String> {
    check_for_update_impl(&app).await
}

#[tauri::command]
async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let progress = Arc::clone(&state.update_progress);
    let engine_stop = state.engine.clone();
    let engine_resume = state.engine.clone();
    let cfg_stop = Arc::clone(&state.config);
    let cfg_resume = Arc::clone(&state.config);
    let saver_stop = state.config_saver.clone();

    install_update_impl(
        &app,
        progress,
        move || {
            if let Ok(cfg) = cfg_stop.lock() {
                saver_stop.flush_sync(cfg.clone(), Duration::from_secs(2));
            }
            let _ = engine_stop.stop_and_wait(Duration::from_secs(3));
        },
        move || {
            if let Ok(cfg) = cfg_resume.lock() {
                if cfg.stream_active {
                    engine_resume.start_or_restart(
                        cfg.input_device.clone(),
                        cfg.output_device.clone(),
                        cfg.monitor_device.clone(),
                        true,
                    );
                }
            }
        },
    )
    .await
}

#[tauri::command]
fn get_update_progress(state: State<'_, AppState>) -> UpdateProgressSnapshot {
    state
        .update_progress
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn open_releases_page() -> Result<(), String> {
    open_releases_page_in_browser()
}

#[tauri::command]
fn set_window_theme(window: tauri::WebviewWindow, theme: Option<String>) -> Result<(), String> {
    let t = match theme.as_deref() {
        Some("dark") => Some(tauri::Theme::Dark),
        Some("light") => Some(tauri::Theme::Light),
        _ => None,
    };
    let _ = window.set_theme(t);
    Ok(())
}

#[tauri::command]
fn get_autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch
            .enable()
            .map_err(|e| match i18n::current_locale() {
                Locale::Ja => format!("自動起動の有効化に失敗しました: {}", e),
                Locale::En => format!("Failed to enable launch at login: {}", e),
            })?;
    } else {
        autolaunch
            .disable()
            .map_err(|e| match i18n::current_locale() {
                Locale::Ja => format!("自動起動の無効化に失敗しました: {}", e),
                Locale::En => format!("Failed to disable launch at login: {}", e),
            })?;
    }
    Ok(autolaunch.is_enabled().unwrap_or(enabled))
}

#[tauri::command]
fn get_diagnostics_text(app: AppHandle, state: State<'_, AppState>) -> String {
    let version = app.package_info().version.to_string();
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let telem = state.engine.shared.snapshot();
    let shared = &state.engine.shared;
    let rt_active = shared.rt_priority_active.load(Ordering::Relaxed);
    let in_frames = shared.input_callback_frames.load(Ordering::Relaxed);
    let out_frames = shared.output_callback_frames.load(Ordering::Relaxed);
    let out_drops = shared.output_drop_count.load(Ordering::Relaxed);
    let dfn_errs = shared.dfn_error_count.load(Ordering::Relaxed);
    let rs_errs = shared.resampler_error_count.load(Ordering::Relaxed);
    let obs_in_blk = shared.observed_in_block_frames.load(Ordering::Relaxed);
    let obs_out_blk = shared.observed_out_block_frames.load(Ordering::Relaxed);

    let cfg = state.config.lock().map(|g| g.clone()).unwrap_or_default();
    let devs = list_audio_devices();

    let mut out = String::new();
    out.push_str(&format!("Shizuka Diagnostics (v{})\n", version));
    out.push_str(&format!(
        "Platform: {} / {} (install_mode={})\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        if is_current_build_portable() {
            "portable"
        } else {
            "installed"
        }
    ));
    out.push_str(&format!("Autostart: {}\n", autostart));
    out.push_str(&format!(
        "Connection State: {:?} (running={}, virtual_out={}, monitor={}, muted={}, rt_priority={})\n",
        telem.status.connection_state,
        telem.stream_running,
        telem.virtual_out_connected,
        telem.live_monitor_enabled,
        telem.mic_muted,
        rt_active
    ));
    out.push_str(&format!(
        "Active Input: {:?} ({} Hz, max_block={} frames, total_frames={})\n",
        telem.status.active_input_name, telem.status.input_sample_rate, obs_in_blk, in_frames
    ));
    out.push_str(&format!(
        "Active Virtual Output: {:?} ({} Hz, max_block={} frames, total_frames={})\n",
        telem.status.active_output_name, telem.status.output_sample_rate, obs_out_blk, out_frames
    ));
    out.push_str(&format!(
        "Active Monitor: {:?}\n",
        telem.status.active_monitor_name
    ));
    out.push_str(&format!(
        "Active AEC Reference: {:?}\n",
        telem.status.active_aec_ref_name
    ));
    if let Some(ref waiting) = telem.status.waiting_device_name {
        out.push_str(&format!("Waiting Device: {}\n", waiting));
    }
    out.push_str(&format!(
        "Configured: in={:?}, out={:?}, mon={:?}, aec_ref={:?}, saved_mic_profiles={}\n",
        cfg.input_device,
        cfg.output_device,
        cfg.monitor_device,
        cfg.aec_reference_device,
        cfg.mic_profiles.len()
    ));
    out.push_str(&format!(
        "DSP: nc={}, model={:?}, atten={} dB, gain={} dB, hpf={}, agc={} ({:+.1} dB), aec={} (active={}, erle={:.1} dB), vad={} ({} dB), voice_gate={} (enrolled={}, thresh={:.2}, sim={:.2}, blocking={})\n",
        telem.nc_enabled,
        telem.model_variant,
        cfg.atten_lim_db,
        cfg.input_gain_db,
        cfg.hpf_enabled,
        cfg.agc_enabled,
        telem.agc_gain_db,
        cfg.aec_enabled,
        telem.aec_active,
        telem.aec_erle_db,
        cfg.vad_gate_enabled,
        cfg.vad_threshold_db,
        telem.voice_gate_enabled,
        telem.has_voice_profile,
        cfg.voice_gate_threshold,
        telem.voice_gate_similarity,
        telem.voice_gate_blocking
    ));
    out.push_str(&format!(
        "Shortcuts: nc={:?}, mute={:?}, sound={}\n",
        cfg.shortcut_toggle_nc, cfg.shortcut_toggle_mute, cfg.shortcut_sound_enabled
    ));
    out.push_str(&format!(
        "Metrics: cpu={:.1}%, latency={:.1} ms, drift={:+.1} ppm, clips={}, reconnects={}, in_drops={}, out_drops={}, underruns={}, dfn_errs={}, rs_errs={}\n",
        telem.cpu_load_pct,
        telem.latency_ms,
        telem.drift_ppm,
        telem.clip_count,
        telem.status.reconnect_count,
        telem.status.input_drop_count,
        out_drops,
        telem.status.output_underrun_count,
        dfn_errs,
        rs_errs
    ));
    if let Some(ref err) = telem.status.last_error {
        out.push_str(&format!("Last Error: {}\n", err));
    }
    out.push_str("Recent Events:\n");
    out.push_str(&diagnostics::format_recent_events());
    out.push_str("Detected Inputs:\n");
    for d in &devs.inputs {
        out.push_str(&format!(
            "  - {} ({} Hz, {} ch{}{})\n",
            d.name,
            d.sample_rate,
            d.channels,
            if d.is_default { ", default" } else { "" },
            if d.is_virtual_cable { ", virtual" } else { "" }
        ));
    }
    out.push_str("Detected Outputs:\n");
    for d in &devs.outputs {
        out.push_str(&format!(
            "  - {} ({} Hz, {} ch{}{})\n",
            d.name,
            d.sample_rate,
            d.channels,
            if d.is_default { ", default" } else { "" },
            if d.is_virtual_cable { ", virtual" } else { "" }
        ));
    }
    out
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    shutdown_cleanly(&app);
    app.exit(0);
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        let _ = app.emit("shizuka-window-visibility", true);
    }
}

fn hide_main_window(app: &AppHandle) {
    unpause_shortcuts(app);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
        let _ = app.emit("shizuka-window-visibility", false);
    }
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(false);
        let minimized = win.is_minimized().unwrap_or(false);
        let focused = win.is_focused().unwrap_or(false);
        if visible && !minimized && focused {
            hide_main_window(app);
        } else {
            show_main_window(app);
        }
    }
}

pub fn run() {
    let _ = env_logger::try_init();

    let app = tauri::Builder::default()
        // Register single-instance FIRST so duplicate launches focus the running window and exit
        // before spawning any audio engine or loading DeepFilterNet3.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            if let Some(base_win_cfg) = app.config().app.windows.first() {
                #[cfg(target_os = "windows")]
                {
                    let mut win_cfg = base_win_cfg.clone();
                    win_cfg.decorations = false;
                    WebviewWindowBuilder::from_config(app, &win_cfg)?.build()?;
                }
                #[cfg(not(target_os = "windows"))]
                {
                    WebviewWindowBuilder::from_config(app, base_win_cfg)?.build()?;
                }
            }

            let start_minimized = std::env::args().any(|arg| arg == "--minimized");
            diagnostics::info(format!(
                "Shizuka v{} starting (minimized={})",
                app.package_info().version,
                start_minimized
            ));

            let initial_cfg = AppConfig::load();
            let init_locale = initial_cfg.language.resolve();
            i18n::set_active_locale(init_locale);
            let init_nc = initial_cfg.nc_enabled;
            let init_sc_nc = initial_cfg.shortcut_toggle_nc.clone();
            let init_sc_mute = initial_cfg.shortcut_toggle_mute.clone();

            let shared_cfg = Arc::new(Mutex::new(initial_cfg));
            let config_saver = ConfigSaver::spawn();
            let engine =
                AudioEngineHandle::spawn(Arc::clone(&shared_cfg), Some(config_saver.clone()));
            let cue = CuePlayer::spawn();

            app.manage(AppState {
                engine,
                config: shared_cfg,
                config_saver,
                cue,
                shortcuts: Mutex::new(ShortcutRegistry::default()),
                update_progress: Arc::new(Mutex::new(UpdateProgressSnapshot::default())),
                tray_handles: Mutex::new(None),
                start_minimized,
                window_shown_once: AtomicBool::new(false),
                shutting_down: AtomicBool::new(false),
            });

            // Initialize in-app auto-updater plugin (Ed25519 signature verified via tauri.conf.json pubkey)
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Initialize global shortcuts plugin & register user-configured bindings (default: none)
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|app_handle, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        let action = {
                            let st = app_handle.state::<AppState>();
                            st.shortcuts
                                .lock()
                                .ok()
                                .and_then(|mut reg| reg.match_triggered(shortcut))
                        };
                        match action {
                            Some(ShortcutAction::ToggleNc) => {
                                let st = app_handle.state::<AppState>();
                                let cur = st.engine.shared.nc_enabled.load(Ordering::Relaxed);
                                apply_nc_enabled(app_handle, !cur, true);
                            }
                            Some(ShortcutAction::ToggleMute) => {
                                let st = app_handle.state::<AppState>();
                                let cur = st.engine.shared.mic_muted.load(Ordering::Relaxed);
                                apply_mic_muted(app_handle, !cur, true);
                            }
                            None => {}
                        }
                    })
                    .build(),
            )?;

            {
                let st = app.state::<AppState>();
                if let Ok(mut reg) = st.shortcuts.lock() {
                    reg.register_initial(
                        app.handle(),
                        init_sc_nc.as_deref(),
                        init_sc_mute.as_deref(),
                    );
                };
            }

            let (show_lbl, nc_lbl, mute_lbl, quit_lbl) = tray_menu_labels(init_locale);
            let show_item =
                MenuItem::with_id(app, "show", show_lbl, true, None::<&str>)?;
            let toggle_item = CheckMenuItem::with_id(
                app,
                "toggle_nc",
                nc_lbl,
                true,
                init_nc,
                None::<&str>,
            )?;
            let mute_item = CheckMenuItem::with_id(
                app,
                "toggle_mute",
                mute_lbl,
                true,
                false,
                None::<&str>,
            )?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", quit_lbl, true, None::<&str>)?;

            let menu = Menu::with_items(
                app,
                &[&show_item, &toggle_item, &mute_item, &sep, &quit_item],
            )?;

            // Pre-decode tray icons once to avoid repeated PNG decoding on shortcut/tray toggles
            let icon_on = Image::from_bytes(include_bytes!("../icons/tray-on.png"))?.to_owned();
            let icon_off = Image::from_bytes(include_bytes!("../icons/tray-off.png"))?.to_owned();
            let icon_mute = Image::from_bytes(include_bytes!("../icons/tray-mute.png"))?.to_owned();
            let init_icon = if init_nc {
                icon_on.clone()
            } else {
                icon_off.clone()
            };

            {
                let st = app.state::<AppState>();
                if let Ok(mut guard) = st.tray_handles.lock() {
                    *guard = Some(TrayUiHandles {
                        icon_on,
                        icon_off,
                        icon_mute,
                        show_item,
                        nc_item: toggle_item,
                        mute_item,
                        quit_item,
                    });
                };
            }

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(init_icon)
                .tooltip(tray_tooltip(init_locale, init_nc, false))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app_handle, event| match event.id.as_ref() {
                    "show" => {
                        show_main_window(app_handle);
                    }
                    "toggle_nc" => {
                        let st = app_handle.state::<AppState>();
                        let cur = st.engine.shared.nc_enabled.load(Ordering::Relaxed);
                        apply_nc_enabled(app_handle, !cur, true);
                    }
                    "toggle_mute" => {
                        let st = app_handle.state::<AppState>();
                        let cur = st.engine.shared.mic_muted.load(Ordering::Relaxed);
                        apply_mic_muted(app_handle, !cur, true);
                    }
                    "quit" => {
                        shutdown_cleanly(app_handle);
                        app_handle.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // Safety fallback: keep window hidden until `frontend_ready` prevents white/default flash,
            // but guarantee visibility within 1.5s if WebView initialization is delayed.
            if !start_minimized {
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(1500));
                    let st = handle.state::<AppState>();
                    if !st.window_shown_once.swap(true, Ordering::AcqRel) {
                        show_main_window(&handle);
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Keep running in system tray / menu bar when window close [X] button is clicked
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide_main_window(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_version,
            frontend_ready,
            get_devices,
            get_config,
            get_telemetry,
            set_nc_enabled,
            set_mic_muted,
            update_dsp_params,
            set_aec_reference_device,
            set_voice_gate_config,
            reset_voice_profile,
            set_model_variant,
            select_devices,
            set_stream_active,
            set_live_monitor,
            set_monitor_mode,
            set_monitor_volume,
            set_shortcut,
            set_shortcuts_paused,
            set_shortcut_sound_enabled,
            get_resolved_locale,
            set_language,
            get_shortcut_warning,
            install_virtual_driver,
            check_for_update,
            install_update,
            get_update_progress,
            open_releases_page,
            set_window_theme,
            get_autostart_enabled,
            set_autostart_enabled,
            get_diagnostics_text,
            quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Shizuka tauri application");

    app.run(|app_handle, event| match event {
        RunEvent::Exit => {
            shutdown_cleanly(app_handle);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => {
            show_main_window(app_handle);
        }
        _ => {}
    });
}
