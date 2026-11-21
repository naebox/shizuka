use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub monitor_device: Option<String>,
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
    pub monitor_volume: f32,
    pub stream_active: bool,
    pub mic_profiles: BTreeMap<String, MicProfile>,
    pub shortcut_toggle_nc: Option<String>,
    pub shortcut_toggle_mute: Option<String>,
    pub shortcut_sound_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let def_prof = MicProfile::default();
        Self {
            input_device: None,
            output_device: None,
            monitor_device: None,
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
            monitor_volume: 0.8,
            stream_active: true,
            mic_profiles: BTreeMap::new(),
            shortcut_toggle_nc: None,
            shortcut_toggle_mute: None,
            shortcut_sound_enabled: true,
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
        } else if is_mic_change && self.input_gain_db.abs() > f32::EPSILON {
            self.input_gain_db = 0.0;
            true
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
        for prof in self.mic_profiles.values_mut() {
            if migrate_values(&mut prof.min_db_thresh, &mut prof.post_filter_beta) {
                changed = true;
            }
        }
        changed
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&raw) {
                if cfg.migrate_dsp_defaults() {
                    let _ = cfg.save();
                }
                return cfg;
            }
        }
        // One-time migration fallback from legacy clear-voice settings if present
        let legacy = Self::legacy_config_path();
        if let Ok(raw) = fs::read_to_string(&legacy) {
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&raw) {
                let _ = cfg.migrate_dsp_defaults();
                let _ = cfg.save();
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
            ..Default::default()
        };

        // Switching from Mic A (unsaved yet) to unseen Mic B:
        // snapshots Mic A, keeps suppression/VAD/HPF/AGC, resets input_gain_db to 0.0
        let changed = cfg.switch_to_mic_profile(Some("Dynamic XLR Mic"), "USB Headset");
        assert!(changed);
        assert_eq!(cfg.input_gain_db, 0.0);
        assert_eq!(cfg.atten_lim_db, 95.0);
        assert!(cfg.agc_enabled);
        assert_eq!(
            cfg.mic_profiles
                .get("Dynamic XLR Mic")
                .map(|p| p.input_gain_db),
            Some(9.5)
        );

        // Customize Mic B and save
        cfg.atten_lim_db = 30.0;
        cfg.input_gain_db = -3.0;
        cfg.hpf_enabled = false;
        cfg.agc_enabled = false;
        cfg.save_profile_for_mic("USB Headset");

        // Switch back to Mic A -> restores Mic A's full profile
        let restored = cfg.switch_to_mic_profile(Some("USB Headset"), "Dynamic XLR Mic");
        assert!(restored);
        assert_eq!(cfg.atten_lim_db, 95.0);
        assert_eq!(cfg.input_gain_db, 9.5);
        assert!(cfg.hpf_enabled);
        assert!(cfg.agc_enabled);

        // Switch back to Mic B -> restores Mic B's full profile
        let restored_b = cfg.switch_to_mic_profile(Some("Dynamic XLR Mic"), "USB Headset");
        assert!(restored_b);
        assert_eq!(cfg.atten_lim_db, 30.0);
        assert_eq!(cfg.input_gain_db, -3.0);
        assert!(!cfg.hpf_enabled);
        assert!(!cfg.agc_enabled);
    }
}
