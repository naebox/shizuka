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
fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    state.engine.stop();
    app.exit(0);
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(false);
        if visible {
            let _ = win.show();
            let _ = win.set_focus();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

pub fn run() {
    let _ = env_logger::try_init();
    let initial_cfg = AppConfig::load();
    let engine = AudioEngineHandle::spawn(&initial_cfg);
    let init_nc = initial_cfg.nc_enabled;

    tauri::Builder::default()
        .manage(AppState {
            engine,
            config: Mutex::new(initial_cfg),
        })
        .setup(move |app| {
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
                        toggle_main_window(app_handle);
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

            Ok(())
        })
        .on_window_event(|window, event| {
            // Keep running in system tray when window close [X] button is clicked (Krisp behavior)
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
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shizuka tauri application");
}
