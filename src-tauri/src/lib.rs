pub mod audio;
pub mod config;
pub mod installer;

use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};

use audio::devices::{list_audio_devices, DeviceListResponse};
use audio::engine::{AudioEngineHandle, AudioTelemetry};
use config::{AppConfig, ModelVariant};
use installer::{install_virtual_audio_driver, InstallDriverResult};

const TRAY_ID: &str = "shizuka-tray";

pub struct AppState {
    pub engine: AudioEngineHandle,
    pub config: Mutex<AppConfig>,
}

fn update_tray_icon(app: &AppHandle, nc_enabled: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let icon_bytes: &[u8] = if nc_enabled {
            include_bytes!("../icons/tray-on.png")
        } else {
            include_bytes!("../icons/tray-off.png")
        };
        if let Ok(img) = Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(img));
        }
        let tooltip = if nc_enabled {
            "Shizuka — AIノイズ除去: ON"
        } else {
            "Shizuka — AIノイズ除去: OFF (バイパス中)"
        };
        let _ = tray.set_tooltip(Some(tooltip));
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
fn set_nc_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    state.engine.set_nc_enabled(enabled);
    if let Ok(mut cfg) = state.config.lock() {
        cfg.nc_enabled = enabled;
        let _ = cfg.save();
    }
    update_tray_icon(&app, enabled);
    Ok(())
}

#[tauri::command]
fn update_dsp_params(
    state: State<'_, AppState>,
    atten_lim_db: f32,
    post_filter_beta: f32,
    input_gain_db: f32,
    min_db_thresh: f32,
    vad_gate_enabled: bool,
    vad_threshold_db: f32,
) -> Result<(), String> {
    state.engine.set_params(
        atten_lim_db,
        post_filter_beta,
        input_gain_db,
        min_db_thresh,
        vad_gate_enabled,
        vad_threshold_db,
    );
    if let Ok(mut cfg) = state.config.lock() {
        cfg.atten_lim_db = atten_lim_db;
        cfg.post_filter_beta = post_filter_beta;
        cfg.input_gain_db = input_gain_db;
        cfg.min_db_thresh = min_db_thresh;
        cfg.vad_gate_enabled = vad_gate_enabled;
        cfg.vad_threshold_db = vad_threshold_db;
        let _ = cfg.save();
    }
    Ok(())
}

#[tauri::command]
fn set_model_variant(state: State<'_, AppState>, model_variant: ModelVariant) -> Result<(), String> {
    if let Ok(mut cfg) = state.config.lock() {
        cfg.model_variant = model_variant;
        let _ = cfg.save();
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
) -> Result<(), String> {
    let clean_in = input_device.filter(|s| !s.is_empty());
    let clean_out = output_device.filter(|s| !s.is_empty());
    let clean_mon = monitor_device.filter(|s| !s.is_empty());

    let (should_run, main_devices_changed, monitor_changed) =
        if let Ok(mut cfg) = state.config.lock() {
            let main_changed =
                cfg.input_device != clean_in || cfg.output_device != clean_out;
            let mon_changed = cfg.monitor_device != clean_mon;
            cfg.input_device = clean_in.clone();
            cfg.output_device = clean_out.clone();
            cfg.monitor_device = clean_mon.clone();
            let _ = cfg.save();
            (cfg.stream_active, main_changed, mon_changed)
        } else {
            (true, true, true)
        };

    let mon_active = state
        .engine
        .shared
        .live_monitor_enabled
        .load(std::sync::atomic::Ordering::Relaxed);

    if should_run && main_devices_changed {
        state
            .engine
            .start_or_restart(clean_in, clean_out, clean_mon);
    } else if monitor_changed && mon_active {
        state.engine.set_live_monitor(true, clean_mon);
    }
    Ok(())
}

#[tauri::command]
fn set_stream_active(state: State<'_, AppState>, active: bool) -> Result<(), String> {
    let (in_dev, out_dev, mon_dev) = if let Ok(mut cfg) = state.config.lock() {
        cfg.stream_active = active;
        let _ = cfg.save();
        (
            cfg.input_device.clone(),
            cfg.output_device.clone(),
            cfg.monitor_device.clone(),
        )
    } else {
        (None, None, None)
    };

    if active {
        state.engine.start_or_restart(in_dev, out_dev, mon_dev);
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
    let clamped = volume.clamp(0.0, 1.0);
    state.engine.set_monitor_volume(clamped);
    if let Ok(mut cfg) = state.config.lock() {
        cfg.monitor_volume = clamped;
        let _ = cfg.save();
    }
    Ok(())
}

#[tauri::command]
async fn install_virtual_driver(state: State<'_, AppState>) -> Result<InstallDriverResult, String> {
    let result = tauri::async_runtime::spawn_blocking(install_virtual_audio_driver)
        .await
        .map_err(|e| format!("セットアップタスク実行エラー: {}", e))?
        .map_err(|e| format!("{:#}", e))?;

    if result.device_detected_now {
        if let Some(ref virt_out) = result.detected_device_name {
            let (in_dev, mon_dev, should_run) = if let Ok(mut cfg) = state.config.lock() {
                cfg.output_device = Some(virt_out.clone());
                let _ = cfg.save();
                (
                    cfg.input_device.clone(),
                    cfg.monitor_device.clone(),
                    cfg.stream_active,
                )
            } else {
                (None, None, true)
            };

            if should_run {
                state
                    .engine
                    .start_or_restart(in_dev, Some(virt_out.clone()), mon_dev);
            }
        }
    }

    Ok(result)
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
            .map_err(|e| format!("自動起動の有効化に失敗しました: {}", e))?;
    } else {
        autolaunch
            .disable()
            .map_err(|e| format!("自動起動の無効化に失敗しました: {}", e))?;
    }
    Ok(autolaunch.is_enabled().unwrap_or(enabled))
}

#[tauri::command]
fn get_diagnostics_text(app: AppHandle, state: State<'_, AppState>) -> String {
    let version = app.package_info().version.to_string();
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let telem = state.engine.shared.snapshot();
    let cfg = state.config.lock().map(|g| g.clone()).unwrap_or_default();
    let devs = list_audio_devices();

    let mut out = String::new();
    out.push_str(&format!("Shizuka Diagnostics (v{})\n", version));
    out.push_str(&format!(
        "Platform: {} / {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    out.push_str(&format!("Autostart: {}\n", autostart));
    out.push_str(&format!(
        "Connection State: {:?} (running={}, virtual_out={}, monitor={})\n",
        telem.status.connection_state,
        telem.stream_running,
        telem.virtual_out_connected,
        telem.live_monitor_enabled
    ));
    out.push_str(&format!(
        "Active Input: {:?} ({} Hz)\n",
        telem.status.active_input_name, telem.status.input_sample_rate
    ));
    out.push_str(&format!(
        "Active Virtual Output: {:?} ({} Hz)\n",
        telem.status.active_output_name, telem.status.output_sample_rate
    ));
    out.push_str(&format!(
        "Active Monitor: {:?}\n",
        telem.status.active_monitor_name
    ));
    if let Some(ref waiting) = telem.status.waiting_device_name {
        out.push_str(&format!("Waiting Device: {}\n", waiting));
    }
    out.push_str(&format!(
        "Configured: in={:?}, out={:?}, mon={:?}\n",
        cfg.input_device, cfg.output_device, cfg.monitor_device
    ));
    out.push_str(&format!(
        "DSP: nc={}, model={:?}, atten={} dB, gain={} dB, vad={} ({} dB)\n",
        telem.nc_enabled,
        telem.model_variant,
        cfg.atten_lim_db,
        cfg.input_gain_db,
        cfg.vad_gate_enabled,
        cfg.vad_threshold_db
    ));
    out.push_str(&format!(
        "Metrics: cpu={:.1}%, latency={:.1} ms, drift={:+.1} ppm, reconnects={}, drops={}, underruns={}\n",
        telem.cpu_load_pct,
        telem.latency_ms,
        telem.drift_ppm,
        telem.status.reconnect_count,
        telem.status.input_drop_count,
        telem.status.output_underrun_count
    ));
    if let Some(ref err) = telem.status.last_error {
        out.push_str(&format!("Last Error: {}\n", err));
    }
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
fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    state.engine.stop();
    app.exit(0);
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
    }
}

pub fn run() {
    let _ = env_logger::try_init();

    tauri::Builder::default()
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
            let initial_cfg = AppConfig::load();
            let init_nc = initial_cfg.nc_enabled;
            let engine = AudioEngineHandle::spawn(&initial_cfg);
            app.manage(AppState {
                engine,
                config: Mutex::new(initial_cfg),
            });

            let show_item =
                MenuItem::with_id(app, "show", "Shizuka を開く", true, None::<&str>)?;
            let toggle_item = MenuItem::with_id(
                app,
                "toggle_nc",
                "ノイズキャンセリング ON / OFF 切替",
                true,
                None::<&str>,
            )?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show_item, &toggle_item, &sep, &quit_item])?;

            let icon_bytes: &[u8] = if init_nc {
                include_bytes!("../icons/tray-on.png")
            } else {
                include_bytes!("../icons/tray-off.png")
            };
            let tray_img = Image::from_bytes(icon_bytes)?;

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_img)
                .tooltip(if init_nc {
                    "Shizuka — AIノイズ除去: ON"
                } else {
                    "Shizuka — AIノイズ除去: OFF"
                })
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app_handle, event| match event.id.as_ref() {
                    "show" => {
                        show_main_window(app_handle);
                    }
                    "toggle_nc" => {
                        let st = app_handle.state::<AppState>();
                        let cur = st
                            .engine
                            .shared
                            .nc_enabled
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let next = !cur;
                        st.engine.set_nc_enabled(next);
                        if let Ok(mut cfg) = st.config.lock() {
                            cfg.nc_enabled = next;
                            let _ = cfg.save();
                        }
                        update_tray_icon(app_handle, next);
                    }
                    "quit" => {
                        let st = app_handle.state::<AppState>();
                        st.engine.stop();
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

            let start_minimized = std::env::args().any(|arg| arg == "--minimized");
            if !start_minimized {
                show_main_window(app.handle());
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Keep running in system tray when window close [X] button is clicked
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_devices,
            get_config,
            get_telemetry,
            set_nc_enabled,
            update_dsp_params,
            set_model_variant,
            select_devices,
            set_stream_active,
            set_live_monitor,
            set_monitor_mode,
            set_monitor_volume,
            install_virtual_driver,
            set_window_theme,
            get_autostart_enabled,
            set_autostart_enabled,
            get_diagnostics_text,
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shizuka tauri application");
}
