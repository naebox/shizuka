use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const RELEASES_PAGE_URL: &str = "https://github.com/seedling-nae/shizuka/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub available: bool,
    pub latest_version: Option<String>,
    pub date: Option<String>,
    pub notes: Option<String>,
    pub is_portable: bool,
    pub releases_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressSnapshot {
    pub phase: String, // "idle" | "downloading" | "installing" | "error"
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

impl Default for UpdateProgressSnapshot {
    fn default() -> Self {
        Self {
            phase: "idle".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
}

/// Platform-independent Windows install-directory classifier (testable on all OSes).
/// Returns `true` when `exe_dir` looks like a standalone portable ZIP extraction rather than
/// an NSIS (`uninstall.exe` present) or MSI / per-user installer location.
pub fn is_portable_windows_dir(exe_dir: &Path) -> bool {
    if exe_dir.join("uninstall.exe").is_file() {
        return false;
    }
    let norm = exe_dir
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    !(norm.contains("/program files/")
        || norm.contains("/program files (x86)/")
        || norm.contains("/appdata/local/programs/shizuka")
        || norm.contains("/appdata/local/shizuka"))
}

pub fn is_current_build_portable() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(is_portable_windows_dir))
            .unwrap_or(true)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

pub fn open_releases_page_in_browser() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", RELEASES_PAGE_URL])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("ブラウザの起動に失敗しました: {}", e))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(RELEASES_PAGE_URL)
            .spawn()
            .map_err(|e| format!("ブラウザの起動に失敗しました: {}", e))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(RELEASES_PAGE_URL)
            .spawn()
            .map_err(|e| format!("ブラウザの起動に失敗しました: {}", e))?;
        Ok(())
    }
}

pub async fn check_for_update_impl(app: &AppHandle) -> Result<UpdateCheckResult, String> {
    let current_version = app.package_info().version.to_string();
    let is_portable = is_current_build_portable();

    let updater = app
        .updater()
        .map_err(|e| format!("アップデータの初期化に失敗しました: {}", e))?;

    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateCheckResult {
            current_version,
            available: true,
            latest_version: Some(update.version.clone()),
            date: update.date.map(|d| d.to_string()),
            notes: update.body.clone(),
            is_portable,
            releases_url: RELEASES_PAGE_URL.to_string(),
        }),
        Ok(None) => Ok(UpdateCheckResult {
            current_version,
            available: false,
            latest_version: None,
            date: None,
            notes: None,
            is_portable,
            releases_url: RELEASES_PAGE_URL.to_string(),
        }),
        Err(e) => Err(format!("アップデートの確認に失敗しました: {}", e)),
    }
}

pub async fn install_update_impl<F>(
    app: &AppHandle,
    progress: Arc<Mutex<UpdateProgressSnapshot>>,
    stop_audio: F,
) -> Result<(), String>
where
    F: FnOnce() + Send + 'static,
{
    if is_current_build_portable() {
        return Err(
            "ポータブル版は自動更新に対応していません。GitHub Releases から最新版をダウンロードしてください。"
                .to_string(),
        );
    }

    if let Ok(mut p) = progress.lock() {
        *p = UpdateProgressSnapshot {
            phase: "downloading".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        };
    }

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            let msg = format!("アップデータの初期化に失敗しました: {}", e);
            if let Ok(mut p) = progress.lock() {
                p.phase = "error".to_string();
                p.error = Some(msg.clone());
            }
            return Err(msg);
        }
    };

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            if let Ok(mut p) = progress.lock() {
                *p = UpdateProgressSnapshot::default();
            }
            return Err("利用可能なアップデートはありません。".to_string());
        }
        Err(e) => {
            let msg = format!("アップデートの確認に失敗しました: {}", e);
            if let Ok(mut p) = progress.lock() {
                p.phase = "error".to_string();
                p.error = Some(msg.clone());
            }
            return Err(msg);
        }
    };

    let prog_chunk = Arc::clone(&progress);
    let prog_finish = Arc::clone(&progress);

    let install_res = update
        .download_and_install(
            move |chunk_len, content_len| {
                if let Ok(mut p) = prog_chunk.lock() {
                    p.phase = "downloading".to_string();
                    p.downloaded_bytes = p.downloaded_bytes.saturating_add(chunk_len as u64);
                    if content_len.is_some() {
                        p.total_bytes = content_len;
                    }
                }
            },
            move || {
                if let Ok(mut p) = prog_finish.lock() {
                    p.phase = "installing".to_string();
                }
                stop_audio();
            },
        )
        .await;

    if let Err(e) = install_res {
        let msg = format!("アップデートの適用に失敗しました: {}", e);
        if let Ok(mut p) = progress.lock() {
            p.phase = "error".to_string();
            p.error = Some(msg.clone());
        }
        return Err(msg);
    }

    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_portable_windows_dir_detection() {
        let base = std::env::temp_dir().join(format!("shizuka-portable-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        // Arbitrary folder without uninstall.exe -> portable
        assert!(is_portable_windows_dir(&base));

        // Folder containing NSIS uninstall.exe -> installed (not portable)
        fs::write(base.join("uninstall.exe"), b"nsis").unwrap();
        assert!(!is_portable_windows_dir(&base));

        // Standard Windows installer locations -> installed (not portable)
        assert!(!is_portable_windows_dir(Path::new(
            r"C:\Program Files\Shizuka"
        )));
        assert!(!is_portable_windows_dir(Path::new(
            r"C:\Users\alice\AppData\Local\Programs\Shizuka"
        )));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_update_progress_default_is_idle() {
        let snap = UpdateProgressSnapshot::default();
        assert_eq!(snap.phase, "idle");
        assert_eq!(snap.downloaded_bytes, 0);
        assert_eq!(snap.total_bytes, None);
        assert!(snap.error.is_none());
    }
}
