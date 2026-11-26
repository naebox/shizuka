use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::diagnostics;
use crate::i18n::LanguagePreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelVariant {
    /// Standard DeepFilterNet3 (2-frame / 20ms lookahead): higher suppression accuracy & lower CPU
    #[default]
    Standard,
    /// Low-Latency DeepFilterNet3_ll (0ms lookahead): minimal latency for live voice monitoring
    LowLatency,
}

/// Per-microphone DSP profile automatically saved and restored by physical microphone name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MicProfile {
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

impl Default for MicProfile {
    fn default() -> Self {
        Self {
            atten_lim_db: 65.0,
            post_filter_beta: 0.0,
            input_gain_db: 0.0,
            min_db_thresh: -16.0,
            vad_gate_enabled: true,
            vad_threshold_db: -5.0,
            hpf_enabled: true,
            agc_enabled: false,
            aec_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub monitor_device: Option<String>,
    pub aec_reference_device: Option<String>,
    pub nc_enabled: bool,
    pub model_variant: ModelVariant,
    pub atten_lim_db: f32,
    pub post_filter_beta: f32,
    pub input_gain_db: f32,
    pub min_db_thresh: f32,
    pub vad_gate_enabled: bool,
    pub vad_threshold_db: f32,
    pub hpf_enabled: bool,
    pub agc_enabled: bool,
    pub aec_enabled: bool,
    pub voice_gate_enabled: bool,
    pub voice_gate_threshold: f32,
    pub voice_profile: Option<Vec<f32>>,
    pub voice_fbank_tilt: Option<Vec<f32>>,
    pub monitor_volume: f32,
    pub stream_active: bool,
    pub mic_profiles: BTreeMap<String, MicProfile>,
    pub shortcut_toggle_nc: Option<String>,
    pub shortcut_toggle_mute: Option<String>,
    pub shortcut_sound_enabled: bool,
    pub language: LanguagePreference,
}

impl Default for AppConfig {
    fn default() -> Self {
        let def_prof = MicProfile::default();
        Self {
            input_device: None,
            output_device: None,
            monitor_device: None,
            aec_reference_device: None,
            nc_enabled: true,
            model_variant: ModelVariant::Standard,
            atten_lim_db: def_prof.atten_lim_db,
            post_filter_beta: def_prof.post_filter_beta,
            input_gain_db: def_prof.input_gain_db,
            min_db_thresh: def_prof.min_db_thresh,
            vad_gate_enabled: def_prof.vad_gate_enabled,
            vad_threshold_db: def_prof.vad_threshold_db,
            hpf_enabled: def_prof.hpf_enabled,
            agc_enabled: def_prof.agc_enabled,
            aec_enabled: def_prof.aec_enabled,
            voice_gate_enabled: false,
            voice_gate_threshold: crate::audio::speaker::DEFAULT_VOICE_GATE_THRESHOLD,
            voice_profile: None,
            voice_fbank_tilt: None,
            monitor_volume: 0.8,
            stream_active: true,
            mic_profiles: BTreeMap::new(),
            shortcut_toggle_nc: None,
            shortcut_toggle_mute: None,
            shortcut_sound_enabled: true,
            language: LanguagePreference::Auto,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("shizuka").join("settings.json")
    }

    fn legacy_config_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("clear-voice").join("settings.json")
    }

    pub fn to_mic_profile(&self) -> MicProfile {
        MicProfile {
            atten_lim_db: self.atten_lim_db,
            post_filter_beta: self.post_filter_beta,
            input_gain_db: self.input_gain_db,
            min_db_thresh: self.min_db_thresh,
            vad_gate_enabled: self.vad_gate_enabled,
            vad_threshold_db: self.vad_threshold_db,
            hpf_enabled: self.hpf_enabled,
            agc_enabled: self.agc_enabled,
            aec_enabled: self.aec_enabled,
        }
    }

    pub fn apply_mic_profile(&mut self, profile: &MicProfile) {
        self.atten_lim_db = profile.atten_lim_db;
        self.post_filter_beta = profile.post_filter_beta;
        self.input_gain_db = profile.input_gain_db;
        self.min_db_thresh = profile.min_db_thresh;
        self.vad_gate_enabled = profile.vad_gate_enabled;
        self.vad_threshold_db = profile.vad_threshold_db;
        self.hpf_enabled = profile.hpf_enabled;
        self.agc_enabled = profile.agc_enabled;
        self.aec_enabled = profile.aec_enabled;
    }

    pub fn save_profile_for_mic(&mut self, mic_name: &str) {
        let trimmed = mic_name.trim();
        if trimmed.is_empty() {
            return;
        }
        let prof = self.to_mic_profile();
        self.mic_profiles.insert(trimmed.to_string(), prof);
    }

    /// Switches active DSP settings to `next_mic`'s saved profile if present.
    /// - When `prev_mic` is `Some(prev)` and `prev != next`: snapshots current settings under `prev`
    ///   (preserving latest edits), then restores `next`'s saved profile if found; for an unseen
    ///   `next`, keeps current suppression/VAD/HPF/AGC while resetting `input_gain_db` to `0.0` dB
    ///   to prevent accidental clipping on higher-sensitivity microphones.
    /// - When `prev_mic` is `None` (initial app startup): restores `next`'s saved profile if found,
    ///   otherwise preserves the loaded top-level settings untouched (v0.2.0 upgrade compatibility).
    ///
    /// Returns `true` if any active DSP setting changed.
    pub fn switch_to_mic_profile(&mut self, prev_mic: Option<&str>, next_mic: &str) -> bool {
        let next = next_mic.trim();
        if next.is_empty() {
            return false;
        }

        let is_mic_change = match prev_mic.map(str::trim).filter(|s| !s.is_empty()) {
            Some(prev) if prev != next => {
                self.save_profile_for_mic(prev);
                true
            }
            Some(_) => false,
            None => false,
        };

        if let Some(existing) = self.mic_profiles.get(next).cloned() {
            let changed = self.to_mic_profile() != existing;
            self.apply_mic_profile(&existing);
            changed
        } else if is_mic_change {
            let mut changed = false;
            if self.input_gain_db.abs() > f32::EPSILON {
                self.input_gain_db = 0.0;
                changed = true;
            }
            // Default unseen microphones (e.g. headsets) to AEC OFF while keeping previous
            // speaker-call microphone's AEC=ON safely stored in `mic_profiles`.
            if self.aec_enabled {
                self.aec_enabled = false;
                changed = true;
            }
            changed
        } else {
            false
        }
    }

    fn migrate_dsp_defaults(&mut self) -> bool {
        let mut changed = false;
        let migrate_values = |min_db: &mut f32, pf_beta: &mut f32| -> bool {
            let mut m = false;
            // Relax legacy -10 dB / -15 dB hard-zero cutoff below model lsnr_min (-15 dB)
            // so DFN3 relies on continuous ERB/DF estimation instead of abrupt zero-masking
            if *min_db >= -15.05 {
                *min_db = -16.0;
                m = true;
            }
            // Disable legacy default post-filter (0.02 -> 0.0) and soften max preset (0.035 -> 0.015)
            // to eliminate metallic/robotic speech artifacts
            if (*pf_beta - 0.02).abs() < 0.0005 {
                *pf_beta = 0.0;
                m = true;
            } else if (*pf_beta - 0.035).abs() < 0.0005 {
                *pf_beta = 0.015;
                m = true;
            }
            m
        };

        if migrate_values(&mut self.min_db_thresh, &mut self.post_filter_beta) {
            changed = true;
        }
        // Recalibrate early 0.6.0-dev default voice gate threshold (0.42 -> 0.28) to prevent
        // false mid-phrase dropouts on casual/soft speech.
        if (self.voice_gate_threshold - 0.42).abs() < 0.005 {
            self.voice_gate_threshold = crate::audio::speaker::DEFAULT_VOICE_GATE_THRESHOLD;
            changed = true;
        }
        for prof in self.mic_profiles.values_mut() {
            if migrate_values(&mut prof.min_db_thresh, &mut prof.post_filter_beta) {
                changed = true;
            }
        }
        changed
    }

    pub fn clamp_dsp_values(&mut self) {
        let clamp_vals = |atten: &mut f32,
                          pf: &mut f32,
                          gain: &mut f32,
                          min_db: &mut f32,
                          vad: &mut f32| {
            *atten = if atten.is_finite() { atten.clamp(6.0, 100.0) } else { 65.0 };
            *pf = if pf.is_finite() { pf.clamp(0.0, 0.06) } else { 0.0 };
            *gain = if gain.is_finite() { gain.clamp(-24.0, 24.0) } else { 0.0 };
            *min_db = if min_db.is_finite() { min_db.clamp(-20.0, 20.0) } else { -16.0 };
            *vad = if vad.is_finite() { vad.clamp(-18.0, 12.0) } else { -5.0 };
        };
        clamp_vals(
            &mut self.atten_lim_db,
            &mut self.post_filter_beta,
            &mut self.input_gain_db,
            &mut self.min_db_thresh,
            &mut self.vad_threshold_db,
        );
        self.monitor_volume = if self.monitor_volume.is_finite() {
            self.monitor_volume.clamp(0.0, 1.0)
        } else {
            0.8
        };
        self.voice_gate_threshold = if self.voice_gate_threshold.is_finite() {
            self.voice_gate_threshold.clamp(0.16, 0.48)
        } else {
            crate::audio::speaker::DEFAULT_VOICE_GATE_THRESHOLD
        };
        if self
            .voice_profile
            .as_ref()
            .is_some_and(|v| !crate::audio::speaker::is_valid_embedding(v))
        {
            self.voice_profile = None;
            self.voice_fbank_tilt = None;
        }
        if self
            .voice_fbank_tilt
            .as_ref()
            .is_some_and(|t| !crate::audio::speaker::is_valid_fbank_tilt(t))
        {
            self.voice_fbank_tilt = None;
        }
        for prof in self.mic_profiles.values_mut() {
            clamp_vals(
                &mut prof.atten_lim_db,
                &mut prof.post_filter_beta,
                &mut prof.input_gain_db,
                &mut prof.min_db_thresh,
                &mut prof.vad_threshold_db,
            );
        }
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&raw) {
                cfg.clamp_dsp_values();
                if cfg.migrate_dsp_defaults() {
                    if let Err(e) = cfg.save() {
                        diagnostics::error(format!("Migrated config save failed: {:#}", e));
                    }
                }
                return cfg;
            }
        }
        // One-time migration fallback from legacy clear-voice settings if present
        let legacy = Self::legacy_config_path();
        if let Ok(raw) = fs::read_to_string(&legacy) {
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&raw) {
                cfg.clamp_dsp_values();
                let _ = cfg.migrate_dsp_defaults();
                if let Err(e) = cfg.save() {
                    diagnostics::error(format!("Legacy config migration save failed: {:#}", e));
                }
                return cfg;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path();
        self.save_to_path(&path)
    }

    pub fn save_to_path(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp_name = format!(
            "{}.{}.tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("settings.json"),
            std::process::id()
        );
        let tmp_path = path.with_file_name(tmp_name);
        {
            let mut file = File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            let _ = file.sync_all();
        }
        if let Err(err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err.into());
        }
        Ok(())
    }
}

enum SaverCmd {
    /// Coalesce rapid updates (e.g. slider drags) and persist after debounce window.
    Schedule(AppConfig),
    /// Persist promptly on the background thread (discrete user settings changes).
    FlushAsync(AppConfig),
    /// Persist synchronously and acknowledge once written (clean shutdown / updater).
    FlushSync(AppConfig, mpsc::SyncSender<()>),
}

/// Non-blocking, debounced background writer for `settings.json` so high-frequency slider drags
/// and global shortcut presses never block the UI/event-loop thread on synchronous `fsync`.
#[derive(Clone)]
pub struct ConfigSaver {
    tx: mpsc::Sender<SaverCmd>,
    _guard: Arc<Mutex<()>>,
}

impl ConfigSaver {
    const DEBOUNCE: Duration = Duration::from_millis(350);

    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<SaverCmd>();
        thread::Builder::new()
            .name("shizuka-cfg-saver".into())
            .spawn(move || {
                let mut pending: Option<AppConfig> = None;
                let mut deadline: Option<Instant> = None;

                let write_cfg = |cfg: &AppConfig| {
                    if let Err(e) = cfg.save() {
                        diagnostics::error(format!("Failed to save settings.json: {:#}", e));
                    }
                };

                loop {
                    let cmd = if let Some(dl) = deadline {
                        let wait = dl.saturating_duration_since(Instant::now());
                        match rx.recv_timeout(wait) {
                            Ok(c) => Some(c),
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if let Some(cfg) = pending.take() {
                                    write_cfg(&cfg);
                                }
                                deadline = None;
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                if let Some(cfg) = pending.take() {
                                    write_cfg(&cfg);
                                }
                                break;
                            }
                        }
                    } else {
                        match rx.recv() {
                            Ok(c) => Some(c),
                            Err(_) => break,
                        }
                    };

                    if let Some(c) = cmd {
                        match c {
                            SaverCmd::Schedule(cfg) => {
                                pending = Some(cfg);
                                deadline = Some(Instant::now() + Self::DEBOUNCE);
                            }
                            SaverCmd::FlushAsync(cfg) => {
                                pending = None;
                                deadline = None;
                                write_cfg(&cfg);
                            }
                            SaverCmd::FlushSync(cfg, ack) => {
                                pending = None;
                                deadline = None;
                                write_cfg(&cfg);
                                let _ = ack.send(());
                            }
                        }
                    }
                }
            })
            .expect("Failed to spawn config saver thread");

        Self {
            tx,
            _guard: Arc::new(Mutex::new(())),
        }
    }

    pub fn schedule(&self, cfg: AppConfig) {
        let _ = self.tx.send(SaverCmd::Schedule(cfg));
    }

    pub fn flush_async(&self, cfg: AppConfig) {
        let _ = self.tx.send(SaverCmd::FlushAsync(cfg));
    }

    pub fn flush_sync(&self, cfg: AppConfig, timeout: Duration) {
        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        if self.tx.send(SaverCmd::FlushSync(cfg, ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(timeout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_atomic_save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("shizuka-cfg-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let _ = fs::remove_dir_all(&dir);

        let mut cfg = AppConfig {
            input_device: Some("In 1-2 (MOTU M Series)".to_string()),
            atten_lim_db: 80.0,
            hpf_enabled: false,
            agc_enabled: true,
            shortcut_toggle_mute: Some("Control+Shift+KeyM".to_string()),
            ..Default::default()
        };
        cfg.save_profile_for_mic("In 1-2 (MOTU M Series)");
        cfg.save_to_path(&path).expect("initial atomic save");

        cfg.atten_lim_db = 55.0;
        cfg.save_to_path(&path).expect("overwrite atomic save");

        let raw = fs::read_to_string(&path).expect("read saved json");
        let loaded: AppConfig = serde_json::from_str(&raw).expect("parse saved json");
        assert_eq!(
            loaded.input_device.as_deref(),
            Some("In 1-2 (MOTU M Series)")
        );
        assert_eq!(loaded.atten_lim_db, 55.0);
        assert!(!loaded.hpf_enabled);
        assert!(loaded.agc_enabled);
        assert_eq!(
            loaded.shortcut_toggle_mute.as_deref(),
            Some("Control+Shift+KeyM")
        );
        assert_eq!(
            loaded
                .mic_profiles
                .get("In 1-2 (MOTU M Series)")
                .map(|p| p.atten_lim_db),
            Some(80.0)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v020_json_backward_compatibility() {
        let v020_json = r#"{
            "input_device": "USB Condenser Mic",
            "output_device": "CABLE Input (VB-Audio Virtual Cable)",
            "monitor_device": null,
            "nc_enabled": true,
            "model_variant": "standard",
            "atten_lim_db": 70.0,
            "post_filter_beta": 0.02,
            "input_gain_db": 4.5,
            "min_db_thresh": -10.0,
            "vad_gate_enabled": true,
            "vad_threshold_db": -3.0,
            "monitor_volume": 0.75,
            "stream_active": true
        }"#;

        let loaded: AppConfig =
            serde_json::from_str(v020_json).expect("v0.2.0 config should deserialize cleanly");
        assert_eq!(loaded.input_device.as_deref(), Some("USB Condenser Mic"));
        assert_eq!(loaded.atten_lim_db, 70.0);
        assert_eq!(loaded.input_gain_db, 4.5);
        assert!(loaded.hpf_enabled, "HPF defaults to enabled");
        assert!(!loaded.agc_enabled, "AGC defaults to disabled");
        assert!(loaded.mic_profiles.is_empty());
        assert_eq!(loaded.shortcut_toggle_nc, None);
        assert_eq!(loaded.shortcut_toggle_mute, None);
        assert!(loaded.shortcut_sound_enabled);
        assert_eq!(loaded.language, LanguagePreference::Auto);

        // Initial startup with unseen mic must NOT wipe the migrated v0.2.0 gain (+4.5 dB)
        let mut migrated = loaded;
        assert!(!migrated.switch_to_mic_profile(None, "USB Condenser Mic"));
        assert_eq!(migrated.input_gain_db, 4.5);

        // Legacy aggressive DFN3 defaults (-10 dB hard cutoff, 0.02 post-filter) migrate smoothly
        assert!(migrated.migrate_dsp_defaults());
        assert_eq!(migrated.min_db_thresh, -16.0);
        assert_eq!(migrated.post_filter_beta, 0.0);
    }

    #[test]
    fn test_mic_profile_auto_switch_and_gain_reset_for_unseen_mic() {
        let mut cfg = AppConfig {
            atten_lim_db: 95.0,
            input_gain_db: 9.5,
            vad_threshold_db: -2.0,
            hpf_enabled: true,
            agc_enabled: true,
            aec_enabled: true,
            ..Default::default()
        };

        // Switching from Mic A (unsaved yet, speaker-call AEC=true) to unseen Mic B (headset):
        // snapshots Mic A (with aec_enabled=true), keeps suppression/VAD/HPF/AGC, resets input_gain_db=0.0 & aec_enabled=false
        let changed = cfg.switch_to_mic_profile(Some("Dynamic XLR Mic"), "USB Headset");
        assert!(changed);
        assert_eq!(cfg.input_gain_db, 0.0);
        assert!(!cfg.aec_enabled);
        assert_eq!(cfg.atten_lim_db, 95.0);
        assert!(cfg.agc_enabled);
        assert_eq!(
            cfg.mic_profiles
                .get("Dynamic XLR Mic")
                .map(|p| (p.input_gain_db, p.aec_enabled)),
            Some((9.5, true))
        );

        // Customize Mic B and save
        cfg.atten_lim_db = 30.0;
        cfg.input_gain_db = -3.0;
        cfg.hpf_enabled = false;
        cfg.agc_enabled = false;
        cfg.aec_enabled = false;
        cfg.save_profile_for_mic("USB Headset");

        // Switch back to Mic A -> restores Mic A's full profile including aec_enabled=true
        let restored = cfg.switch_to_mic_profile(Some("USB Headset"), "Dynamic XLR Mic");
        assert!(restored);
        assert_eq!(cfg.atten_lim_db, 95.0);
        assert_eq!(cfg.input_gain_db, 9.5);
        assert!(cfg.hpf_enabled);
        assert!(cfg.agc_enabled);
        assert!(cfg.aec_enabled);

        // Switch back to Mic B -> restores Mic B's full profile
        let restored_b = cfg.switch_to_mic_profile(Some("Dynamic XLR Mic"), "USB Headset");
        assert!(restored_b);
        assert_eq!(cfg.atten_lim_db, 30.0);
        assert_eq!(cfg.input_gain_db, -3.0);
        assert!(!cfg.hpf_enabled);
        assert!(!cfg.agc_enabled);
    }
}
