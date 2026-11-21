use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelVariant {
    /// Standard DeepFilterNet3 (2-frame / 20ms lookahead): higher suppression accuracy & lower CPU
    Standard,
    /// Low-Latency DeepFilterNet3_ll (0ms lookahead): minimal latency for live voice monitoring
    LowLatency,
}

impl Default for ModelVariant {
    fn default() -> Self {
        Self::Standard
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
    pub monitor_volume: f32,
    pub stream_active: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            monitor_device: None,
            nc_enabled: true,
            model_variant: ModelVariant::Standard,
            atten_lim_db: 65.0,
            post_filter_beta: 0.02,
            input_gain_db: 0.0,
            min_db_thresh: -10.0,
            vad_gate_enabled: true,
            vad_threshold_db: -5.0,
            monitor_volume: 0.8,
            stream_active: true,
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

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<Self>(&raw) {
                return cfg;
            }
        }
        // One-time migration fallback from legacy clear-voice settings if present
        let legacy = Self::legacy_config_path();
        if let Ok(raw) = fs::read_to_string(&legacy) {
            if let Ok(cfg) = serde_json::from_str::<Self>(&raw) {
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

        let mut cfg = AppConfig::default();
        cfg.input_device = Some("In 1-2 (MOTU M Series)".to_string());
        cfg.atten_lim_db = 80.0;
        cfg.save_to_path(&path).expect("initial atomic save");

        cfg.atten_lim_db = 55.0;
        cfg.save_to_path(&path).expect("overwrite atomic save");

        let raw = fs::read_to_string(&path).expect("read saved json");
        let loaded: AppConfig = serde_json::from_str(&raw).expect("parse saved json");
        assert_eq!(loaded.input_device.as_deref(), Some("In 1-2 (MOTU M Series)"));
        assert_eq!(loaded.atten_lim_db, 55.0);

        let _ = fs::remove_dir_all(&dir);
    }
}
