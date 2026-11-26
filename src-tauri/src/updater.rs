use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::diagnostics;
use crate::i18n::{self, tr, Locale};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct UpdateInProgressGuard;

impl UpdateInProgressGuard {
    fn acquire() -> Option<Self> {
        if UPDATE_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for UpdateInProgressGuard {
    fn drop(&mut self) {
        UPDATE_IN_PROGRESS.store(false, Ordering::Release);
    }
}

pub const RELEASES_PAGE_URL: &str = "https://github.com/naebox/shizuka/releases/latest";

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
    if exe_dir.join(".portable").is_file() {
        return true;
    }
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

#[cfg(target_os = "windows")]
fn is_registered_msi_install_dir(exe_dir: &Path) -> bool {
    // Explicit portable ZIP bundles ship README.md + LICENSE alongside Shizuka.exe
    if exe_dir.join(".portable").is_file()
        || (exe_dir.join("README.md").is_file() && exe_dir.join("LICENSE").is_file())
    {
        return false;
    }
    let dir_str = exe_dir.to_string_lossy().trim_end_matches('\\').to_string();
    if dir_str.is_empty() {
        return false;
    }
    for root in [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ] {
        if let Ok(out) = Command::new("reg.exe")
            .args(["query", root, "/s", "/v", "InstallLocation", "/f", &dir_str])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
                if text.contains(&dir_str.to_ascii_lowercase()) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn is_current_build_portable() -> bool {
    #[cfg(target_os = "windows")]
    {
        static CACHED_PORTABLE: OnceLock<bool> = OnceLock::new();
        *CACHED_PORTABLE.get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| {
                    p.parent().map(|dir| {
                        if !is_portable_windows_dir(dir) {
                            false
                        } else {
                            !is_registered_msi_install_dir(dir)
                        }
                    })
                })
                .unwrap_or(true)
        })
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
            .map_err(|e| match i18n::current_locale() {
                Locale::Ja => format!("ブラウザの起動に失敗しました: {}", e),
                Locale::En => format!("Failed to open web browser: {}", e),
            })?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(RELEASES_PAGE_URL)
            .spawn()
            .map_err(|e| match i18n::current_locale() {
                Locale::Ja => format!("ブラウザの起動に失敗しました: {}", e),
                Locale::En => format!("Failed to open web browser: {}", e),
            })?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(RELEASES_PAGE_URL)
            .spawn()
            .map_err(|e| match i18n::current_locale() {
                Locale::Ja => format!("ブラウザの起動に失敗しました: {}", e),
                Locale::En => format!("Failed to open web browser: {}", e),
            })?;
        Ok(())
    }
}

pub async fn check_for_update_impl(app: &AppHandle) -> Result<UpdateCheckResult, String> {
    let current_version = app.package_info().version.to_string();
    let is_portable = is_current_build_portable();

    let updater = app
        .updater()
        .map_err(|e| match i18n::current_locale() {
            Locale::Ja => format!("アップデータの初期化に失敗しました: {}", e),
            Locale::En => format!("Failed to initialize updater: {}", e),
        })?;

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
        Err(e) => Err(match i18n::current_locale() {
            Locale::Ja => format!("アップデートの確認に失敗しました: {}", e),
            Locale::En => format!("Failed to check for updates: {}", e),
        }),
    }
}

pub async fn install_update_impl<FStop, FResume>(
    app: &AppHandle,
    progress: Arc<Mutex<UpdateProgressSnapshot>>,
    stop_audio: FStop,
    resume_audio: FResume,
) -> Result<(), String>
where
    FStop: FnOnce() + Send + 'static,
    FResume: FnOnce() + Send + 'static,
{
    if is_current_build_portable() {
        return Err(tr(
            "ポータブル版は自動更新に対応していません。GitHub Releases から最新版をダウンロードしてください。",
            "Portable builds do not support automatic updates. Please download the latest version from GitHub Releases.",
        )
        .to_string());
    }

    let _guard = UpdateInProgressGuard::acquire().ok_or_else(|| {
        tr(
            "アップデートの適用がすでに進行中です。",
            "An update installation is already in progress.",
        )
        .to_string()
    })?;

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
            let msg = match i18n::current_locale() {
                Locale::Ja => format!("アップデータの初期化に失敗しました: {}", e),
                Locale::En => format!("Failed to initialize updater: {}", e),
            };
            diagnostics::error(&msg);
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
            return Err(tr(
                "利用可能なアップデートはありません。",
                "No updates are available.",
            )
            .to_string());
        }
        Err(e) => {
            let msg = match i18n::current_locale() {
                Locale::Ja => format!("アップデートの確認に失敗しました: {}", e),
                Locale::En => format!("Failed to check for updates: {}", e),
            };
            diagnostics::warn(&msg);
            if let Ok(mut p) = progress.lock() {
                p.phase = "error".to_string();
                p.error = Some(msg.clone());
            }
            return Err(msg);
        }
    };

    diagnostics::info(format!("Downloading update v{}", update.version));
    let prog_chunk = Arc::clone(&progress);

    // Step 1: Download AND verify Ed25519 signature BEFORE touching the live audio pipeline.
    // If download or signature verification fails, the user's microphone stays uninterrupted.
    let verified_bytes = match update
        .download(
            move |chunk_len, content_len| {
                if let Ok(mut p) = prog_chunk.lock() {
                    p.phase = "downloading".to_string();
                    p.downloaded_bytes = p.downloaded_bytes.saturating_add(chunk_len as u64);
                    if content_len.is_some() {
                        p.total_bytes = content_len;
                    }
                }
            },
            || {},
        )
        .await
    {
        Ok(bytes) => bytes,
        Err(e) => {
            let msg = match i18n::current_locale() {
                Locale::Ja => {
                    format!("アップデートのダウンロードまたは署名検証に失敗しました: {}", e)
                }
                Locale::En => {
                    format!("Failed to download or verify update signature: {}", e)
                }
            };
            diagnostics::error(&msg);
            if let Ok(mut p) = progress.lock() {
                p.phase = "error".to_string();
                p.error = Some(msg.clone());
            }
            return Err(msg);
        }
    };

    // Step 2: Signature verified — cleanly stop audio & flush settings before launching installer.
    if let Ok(mut p) = progress.lock() {
        p.phase = "installing".to_string();
    }
    diagnostics::info("Update signature verified; stopping audio before install");
    let _ = tauri::async_runtime::spawn_blocking(stop_audio).await;

    let install_res = tauri::async_runtime::spawn_blocking(move || update.install(&verified_bytes))
        .await
        .map_err(|e| match i18n::current_locale() {
            Locale::Ja => format!("アップデート適用タスクの実行に失敗しました: {}", e),
            Locale::En => format!("Update installation task failed: {}", e),
        })?;

    if let Err(e) = install_res {
        let msg = match i18n::current_locale() {
            Locale::Ja => format!("アップデートの適用に失敗しました: {}", e),
            Locale::En => format!("Failed to install update: {}", e),
        };
        diagnostics::error(&msg);
        // Restore microphone pipeline if installer failed or user cancelled UAC prompt
        resume_audio();
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
