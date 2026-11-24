use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::audio::devices::list_audio_devices;
use crate::diagnostics;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static INSTALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct InstallLockGuard;

impl InstallLockGuard {
    fn acquire() -> Result<Self> {
        if INSTALL_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Ok(Self)
        } else {
            Err(anyhow!("仮想マイクドライバのセットアップがすでに進行中です。"))
        }
    }
}

impl Drop for InstallLockGuard {
    fn drop(&mut self) {
        INSTALL_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// RAII cleanup guard guaranteeing temporary driver archives and extracted files are
/// ALWAYS removed on every normal or early-error return path (AGENTS.md Invariant #1).
struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Escapes a path for safe embedding inside POSIX single-quoted strings (`'...'`).
pub fn escape_posix_single_quoted(raw: &str) -> String {
    raw.replace('\'', "'\\''")
}

/// Escapes an arbitrary string for safe embedding inside an AppleScript double-quoted string literal.
pub fn escape_applescript_string(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Builds the full AppleScript `do shell script "..." with administrator privileges` command
/// with proper two-layer escaping (POSIX single quotes first, then AppleScript double quotes).
///
/// Uses `killall -9 coreaudiod` (auto-respawned by `launchd`) instead of requiring
/// `launchctl kickstart -kp system/com.apple.audio.coreaudiod`, which fails with
/// `1: Operation not permitted` on macOS Sonoma 14.4+ / Sequoia / newer.
pub fn build_macos_hal_applescript(bundle_path: &Path) -> String {
    let escaped_path = escape_posix_single_quoted(&bundle_path.to_string_lossy());
    let shell_cmd = format!(
        "mkdir -p '/Library/Audio/Plug-Ins/HAL' \
         && rm -rf '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' \
         && cp -R '{}' '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' \
         && (xattr -dr com.apple.quarantine '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' 2>/dev/null || true) \
         && (xattr -cr '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' 2>/dev/null || true) \
         && chown -R root:wheel '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' \
         && chmod -R u+rwX,go+rX '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' \
         && (killall -9 coreaudiod 2>/dev/null || killall coreaudiod 2>/dev/null || launchctl kickstart -kp system/com.apple.audio.coreaudiod 2>/dev/null || true)",
        escaped_path
    );
    format!(
        "do shell script \"{}\" with administrator privileges",
        escape_applescript_string(&shell_cmd)
    )
}

/// Formats a user-facing error message for a failed macOS `osascript` privilege escalation step,
/// distinguishing explicit user cancellation (`-128`) from shell/system errors.
pub fn format_macos_osascript_error(exit_code: Option<i32>, stderr: &str) -> String {
    let trimmed = stderr.trim();
    let lower = trimmed.to_lowercase();
    if trimmed.contains("(-128)") || lower.contains("user canceled") || lower.contains("user cancelled") {
        return "管理者認証がキャンセルされました。".to_string();
    }
    if trimmed.is_empty() {
        match exit_code {
            Some(code) => format!("ドライバの配置に失敗しました（終了コード: {}）。", code),
            None => "ドライバの配置に失敗しました。".to_string(),
        }
    } else {
        let single_line = trimmed.lines().collect::<Vec<_>>().join(" / ");
        match exit_code {
            Some(code) => format!("ドライバの配置に失敗しました（終了コード: {}）: {}", code, single_line),
            None => format!("ドライバの配置に失敗しました: {}", single_line),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallDriverResult {
    pub success: bool,
    pub device_detected_now: bool,
    pub detected_device_name: Option<String>,
    pub reboot_recommended: bool,
    pub message: String,
}

/// Checks whether a prebuilt Universal `ShizukaAudio.driver` bundle is present inside `resource_dir`.
pub fn find_bundled_macos_driver(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let base = resource_dir?;
    let candidates = [
        base.join("ShizukaAudio.driver"),
        base.join("resources").join("ShizukaAudio.driver"),
    ];
    for candidate in candidates {
        let bin = candidate.join("Contents").join("MacOS").join("ShizukaAudio");
        let plist = candidate.join("Contents").join("Info.plist");
        if bin.is_file() && plist.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn install_virtual_audio_driver(resource_dir: Option<PathBuf>) -> Result<InstallDriverResult> {
    let _lock = InstallLockGuard::acquire()?;
    diagnostics::info("Starting virtual audio driver setup");
    #[cfg(target_os = "windows")]
    {
        let _ = resource_dir;
        install_windows_vbcable_from_official()
    }
    #[cfg(target_os = "macos")]
    {
        install_macos_mit_hal_driver(resource_dir.as_deref())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = resource_dir;
        Err(anyhow!(
            "自動セットアップは Windows および macOS のみ対応しています。"
        ))
    }
}

#[cfg(target_os = "windows")]
fn run_hidden(cmd: &mut Command) -> Result<std::process::Output> {
    cmd.creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("外部コマンドの実行に失敗しました")
}

#[cfg(target_os = "windows")]
fn discover_latest_vbcable_url() -> Vec<String> {
    let mut candidates = Vec::new();

    // Try scraping official VB-Audio Cable page for the latest VBCABLE_Driver_PackXX.zip URL
    if let Ok(out) = run_hidden(
        Command::new("curl.exe").args(["-sL", "--max-time", "8", "https://vb-audio.com/Cable/"]),
    ) {
        if out.status.success() {
            let html = String::from_utf8_lossy(&out.stdout);
            for marker in [
                "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack",
                "//download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack",
            ] {
                if let Some(start) = html.find(marker) {
                    let rest = &html[start..];
                    if let Some(end) = rest.find(".zip") {
                        let raw_url = &rest[..end + 4];
                        let full_url = if raw_url.starts_with("//") {
                            format!("https:{}", raw_url)
                        } else {
                            raw_url.to_string()
                        };
                        if !candidates.contains(&full_url) {
                            candidates.push(full_url);
                        }
                    }
                }
            }
        }
    }

    for fallback in [
        "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip",
        "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip",
        "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack43.zip",
    ] {
        let s = fallback.to_string();
        if !candidates.contains(&s) {
            candidates.push(s);
        }
    }

    candidates
}

#[cfg(target_os = "windows")]
fn find_setup_exe(dir: &Path) -> Option<PathBuf> {
    let preferred = if cfg!(target_arch = "aarch64") {
        ["VBCABLE_Setup_ARM64.exe", "VBCABLE_Setup_x64.exe"]
    } else if cfg!(target_arch = "x86_64") {
        ["VBCABLE_Setup_x64.exe", "VBCABLE_Setup.exe"]
    } else {
        ["VBCABLE_Setup.exe", "VBCABLE_Setup_x64.exe"]
    };

    for target_name in preferred {
        let candidate = dir.join(target_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(found) = find_setup_exe(&p) {
                    return Some(found);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn install_windows_vbcable_from_official() -> Result<InstallDriverResult> {
    let work_path = std::env::temp_dir().join(format!("shizuka-vbcable-setup-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work_path);
    fs::create_dir_all(&work_path).context("一時作業フォルダの作成に失敗しました")?;
    let work_guard = TempDirGuard(work_path);
    let work_dir = work_guard.path();

    let zip_path = work_dir.join("vbcable_official.zip");
    let extract_dir = work_dir.join("extracted");
    fs::create_dir_all(&extract_dir)?;

    let urls = discover_latest_vbcable_url();
    let mut downloaded = false;

    for url in &urls {
        let zip_str = zip_path.to_string_lossy().to_string();
        let res = run_hidden(Command::new("curl.exe").args([
            "-fL",
            "--max-time",
            "45",
            "-o",
            &zip_str,
            url,
        ]));
        if let Ok(out) = res {
            if out.status.success()
                && zip_path.metadata().map(|m| m.len() > 50_000).unwrap_or(false)
            {
                downloaded = true;
                break;
            }
        }
    }

    if !downloaded {
        return Err(anyhow!(
            "VB-Audio 公式サイトからドライバパッケージを取得できませんでした。インターネット接続をご確認ください。"
        ));
    }

    // Extract ZIP using Windows built-in tar.exe (zero console popup)
    let zip_str = zip_path.to_string_lossy().to_string();
    let ext_str = extract_dir.to_string_lossy().to_string();
    let tar_out = run_hidden(Command::new("tar.exe").args(["-xf", &zip_str, "-C", &ext_str]))?;
    if !tar_out.status.success() {
        return Err(anyhow!("ダウンロードしたドライバ ZIP の展開に失敗しました。"));
    }

    let setup_exe = find_setup_exe(&extract_dir).ok_or_else(|| {
        anyhow!("展開されたアーカイブ内に VBCABLE_Setup 実行ファイルが見つかりませんでした。")
    })?;

    let setup_dir = setup_exe
        .parent()
        .unwrap_or(&extract_dir)
        .to_string_lossy()
        .replace('\'', "''");
    let setup_path_escaped = setup_exe.to_string_lossy().replace('\'', "''");

    // Launch official VB-CABLE installer elevated via UAC (-i -h = silent install)
    let ps_script = format!(
        "$ErrorActionPreference = 'Stop'; \
         $p = Start-Process -FilePath '{exe}' -WorkingDirectory '{wd}' -ArgumentList '-i','-h' -Verb RunAs -PassThru -Wait; \
         exit $p.ExitCode",
        exe = setup_path_escaped,
        wd = setup_dir
    );

    let install_out = run_hidden(Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &ps_script,
    ]));

    // Clean up temporary downloaded files immediately after installer exits
    drop(work_guard);

    match install_out {
        Ok(out) if out.status.success() => {}
        Ok(_) => {
            // Note: VBCABLE_Setup_x64.exe sometimes returns non-zero even when driver is staged or UAC cancelled;
            // check if device appeared before reporting cancellation/error!
            thread::sleep(Duration::from_millis(1500));
            let check = list_audio_devices();
            if !check.has_virtual_cable {
                return Err(anyhow!(
                    "インストールがキャンセルされたか完了しませんでした（管理者権限ダイアログで「はい」を選択してください）。"
                ));
            }
        }
        Err(e) => {
            return Err(anyhow!("インストーラの起動に失敗しました: {:#}", e));
        }
    }

    // Wait briefly for Windows AudioEndpointBuilder to register CABLE Input / CABLE Output
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(600));
        let devs = list_audio_devices();
        if devs.has_virtual_cable {
            let det = devs.recommended_virtual_output.clone();
            return Ok(InstallDriverResult {
                success: true,
                device_detected_now: true,
                detected_device_name: det,
                reboot_recommended: true,
                message: "VB-CABLE (by VB-Audio) の導入が完了し、CABLE Input に自動接続しました。会議アプリのマイク設定で「CABLE Output」を選択してください（※音声が安定しない場合は一度Windowsを再起動してください）。".into(),
            });
        }
    }

    Ok(InstallDriverResult {
        success: true,
        device_detected_now: false,
        detected_device_name: None,
        reboot_recommended: true,
        message: "VB-CABLE ドライバのインストールが完了しました。Windows に仮想マイクを認識させるため、一度 PC を再起動してください。".into(),
    })
}

#[cfg(target_os = "macos")]
fn install_macos_mit_hal_driver(resource_dir: Option<&Path>) -> Result<InstallDriverResult> {
    let work_path = std::env::temp_dir().join(format!("shizuka-hal-install-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work_path);
    fs::create_dir_all(&work_path).context("一時作業フォルダの作成に失敗しました")?;
    let work_guard = TempDirGuard(work_path);
    let work_dir = work_guard.path();
    let bundle_dir = work_dir.join("ShizukaAudio.driver");

    if let Some(prebuilt) = find_bundled_macos_driver(resource_dir) {
        // Preferred path: use prebuilt Universal (arm64 + x86_64) driver bundled in .app Resources (no Xcode CLT required)
        let copy_out = Command::new("cp")
            .args([
                "-R",
                prebuilt.to_str().unwrap_or_default(),
                bundle_dir.to_str().unwrap_or_default(),
            ])
            .output()
            .context("同梱ドライバの一時コピーに失敗しました")?;
        if !copy_out.status.success() {
            let stderr = String::from_utf8_lossy(&copy_out.stderr);
            return Err(anyhow!("同梱ドライバの展開に失敗しました: {}", stderr));
        }
    } else {
        // Fallback path for local `tauri dev` without prebuilt driver resource: compile from bundled MIT source
        const DRIVER_C_SRC: &str = include_str!("../../drivers/macos-hal/ShizukaAudio.c");
        const DRIVER_PLIST: &str = include_str!("../../drivers/macos-hal/Info.plist");

        let macos_bin_dir = bundle_dir.join("Contents").join("MacOS");
        fs::create_dir_all(&macos_bin_dir)?;

        let c_path = work_dir.join("ShizukaAudio.c");
        let plist_path = bundle_dir.join("Contents").join("Info.plist");
        let bin_path = macos_bin_dir.join("ShizukaAudio");

        fs::write(&c_path, DRIVER_C_SRC)?;
        fs::write(&plist_path, DRIVER_PLIST)?;

        let compile = Command::new("clang")
            .args([
                "-std=c11",
                "-O3",
                "-bundle",
                "-arch",
                "arm64",
                "-arch",
                "x86_64",
                "-mmacosx-version-min=12.0",
                "-framework",
                "CoreAudio",
                "-framework",
                "CoreFoundation",
                c_path.to_str().unwrap_or_default(),
                "-o",
                bin_path.to_str().unwrap_or_default(),
            ])
            .output()
            .context("clang の起動に失敗しました（Xcode Command Line Tools が必要です: xcode-select --install）")?;

        if !compile.status.success() {
            let stderr = String::from_utf8_lossy(&compile.stderr);
            return Err(anyhow!("MIT 仮想オーディオドライバのビルドに失敗しました: {}", stderr));
        }

        let sign_out = Command::new("codesign")
            .args([
                "--force",
                "--deep",
                "--sign",
                "-",
                bundle_dir.to_str().unwrap_or_default(),
            ])
            .output()
            .context("ドライバへのアドホック署名 (codesign) の起動に失敗しました")?;
        if !sign_out.status.success() {
            let stderr = String::from_utf8_lossy(&sign_out.stderr);
            return Err(anyhow!(
                "仮想オーディオドライバの署名 (codesign) に失敗しました: {}",
                stderr
            ));
        }
    }

    let osa_cmd = build_macos_hal_applescript(&bundle_dir);

    let osa_out = Command::new("osascript")
        .args(["-e", &osa_cmd])
        .output()
        .context("管理者権限プロンプトの起動に失敗しました")?;

    drop(work_guard);

    if !osa_out.status.success() {
        let stderr = String::from_utf8_lossy(&osa_out.stderr);
        let msg = format_macos_osascript_error(osa_out.status.code(), &stderr);
        diagnostics::error(format!(
            "macOS HAL driver osascript failed (code={:?}): {}",
            osa_out.status.code(),
            stderr.trim()
        ));
        return Err(anyhow!(msg));
    }

    diagnostics::info("macOS HAL driver installed; waiting for coreaudiod reload");

    for _ in 0..10 {
        thread::sleep(Duration::from_millis(500));
        let devs = list_audio_devices();
        if devs.has_virtual_cable {
            diagnostics::info(format!(
                "Detected macOS virtual audio device: {:?}",
                devs.recommended_virtual_output
            ));
            return Ok(InstallDriverResult {
                success: true,
                device_detected_now: true,
                detected_device_name: devs.recommended_virtual_output,
                reboot_recommended: false,
                message: "Shizuka Virtual Mic (MIT ライセンス) の導入が完了し自動接続しました！会議アプリのマイク設定で「Shizuka Virtual Mic」を選択してください。".into(),
            });
        }
    }

    Ok(InstallDriverResult {
        success: true,
        device_detected_now: false,
        detected_device_name: None,
        reboot_recommended: false,
        message: "Shizuka Virtual Mic をインストールしました。右上の更新ボタンを押してデバイスを再読み込みしてください。".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_bundled_macos_driver() {
        let dir = std::env::temp_dir().join(format!("shizuka-hal-res-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        assert!(find_bundled_macos_driver(None).is_none());
        assert!(find_bundled_macos_driver(Some(&dir)).is_none());

        let drv = dir.join("ShizukaAudio.driver");
        fs::create_dir_all(drv.join("Contents").join("MacOS")).unwrap();
        fs::write(drv.join("Contents").join("Info.plist"), "<plist/>").unwrap();
        // Missing executable binary should still return None
        assert!(find_bundled_macos_driver(Some(&dir)).is_none());

        fs::write(
            drv.join("Contents").join("MacOS").join("ShizukaAudio"),
            b"stub-macho",
        )
        .unwrap();
        assert_eq!(find_bundled_macos_driver(Some(&dir)), Some(drv));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_macos_applescript_two_layer_escaping() {
        let tricky = Path::new(r#"/tmp/a'b"c\d/ShizukaAudio.driver"#);
        let script = build_macos_hal_applescript(tricky);
        // Backslashes and double-quotes inside the AppleScript literal must be escaped
        assert!(script.starts_with("do shell script \""));
        assert!(script.ends_with("\" with administrator privileges"));
        assert!(script.contains(r#"a'\\''b\"c\\d"#));
        // Ensure target HAL directory is created, permissions are safe for _coreaudiod,
        // and coreaudiod restart uses killall -9 (macOS 14.4+ blocks launchctl kickstart)
        assert!(script.contains("mkdir -p '/Library/Audio/Plug-Ins/HAL'"));
        assert!(script.contains("chmod -R u+rwX,go+rX '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver'"));
        assert!(script.contains("killall -9 coreaudiod"));
        assert!(!script.contains("&& launchctl kickstart"));
    }

    #[test]
    fn test_format_macos_osascript_error() {
        assert_eq!(
            format_macos_osascript_error(Some(1), "36:45: execution error: User canceled. (-128)\n"),
            "管理者認証がキャンセルされました。"
        );
        let detail = format_macos_osascript_error(
            Some(1),
            "Could not kickstart service: 1: Operation not permitted\n",
        );
        assert!(detail.contains("終了コード: 1"));
        assert!(detail.contains("Operation not permitted"));
        assert_eq!(
            format_macos_osascript_error(Some(2), "   "),
            "ドライバの配置に失敗しました（終了コード: 2）。"
        );
    }
}
