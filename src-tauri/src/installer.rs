use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::audio::devices::list_audio_devices;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallDriverResult {
    pub success: bool,
    pub device_detected_now: bool,
    pub detected_device_name: Option<String>,
    pub reboot_recommended: bool,
    pub message: String,
}

pub fn install_virtual_audio_driver() -> Result<InstallDriverResult> {
    #[cfg(target_os = "windows")]
    {
        install_windows_vbcable_from_official()
    }
    #[cfg(target_os = "macos")]
    {
        install_macos_mit_hal_driver()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
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
    let work_dir = std::env::temp_dir().join("shizuka-vbcable-setup");
    let _ = fs::remove_dir_all(&work_dir);
    fs::create_dir_all(&work_dir).context("一時作業フォルダの作成に失敗しました")?;

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
        let _ = fs::remove_dir_all(&work_dir);
        return Err(anyhow!(
            "VB-Audio 公式サイトからドライバパッケージを取得できませんでした。インターネット接続をご確認ください。"
        ));
    }

    // Extract ZIP using Windows built-in tar.exe (zero console popup)
    let zip_str = zip_path.to_string_lossy().to_string();
    let ext_str = extract_dir.to_string_lossy().to_string();
    let tar_out = run_hidden(Command::new("tar.exe").args(["-xf", &zip_str, "-C", &ext_str]))?;
    if !tar_out.status.success() {
        let _ = fs::remove_dir_all(&work_dir);
        return Err(anyhow!("ダウンロードしたドライバ ZIP の展開に失敗しました。"));
    }

    let setup_exe = find_setup_exe(&extract_dir).ok_or_else(|| {
        let _ = fs::remove_dir_all(&work_dir);
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

    // Clean up temporary downloaded files regardless of outcome
    let _ = fs::remove_dir_all(&work_dir);

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
fn install_macos_mit_hal_driver() -> Result<InstallDriverResult> {
    const DRIVER_C_SRC: &str = include_str!("../../drivers/macos-hal/ShizukaAudio.c");
    const DRIVER_PLIST: &str = include_str!("../../drivers/macos-hal/Info.plist");

    let work_dir = std::env::temp_dir().join("shizuka-hal-build");
    let _ = fs::remove_dir_all(&work_dir);
    let bundle_dir = work_dir.join("ShizukaAudio.driver");
    let macos_bin_dir = bundle_dir.join("Contents").join("MacOS");
    fs::create_dir_all(&macos_bin_dir)?;

    let c_path = work_dir.join("ShizukaAudio.c");
    let plist_path = bundle_dir.join("Contents").join("Info.plist");
    let bin_path = macos_bin_dir.join("ShizukaAudio");

    fs::write(&c_path, DRIVER_C_SRC)?;
    fs::write(&plist_path, DRIVER_PLIST)?;

    let compile = Command::new("clang")
        .args([
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
        let _ = fs::remove_dir_all(&work_dir);
        return Err(anyhow!("MIT 仮想オーディオドライバのビルドに失敗しました: {}", stderr));
    }

    let _ = Command::new("codesign")
        .args([
            "--force",
            "--deep",
            "--sign",
            "-",
            bundle_dir.to_str().unwrap_or_default(),
        ])
        .output();

    let bundle_escaped = bundle_dir.to_string_lossy().replace('\'', "'\\''");
    let osa_cmd = format!(
        "do shell script \"rm -rf '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && cp -R '{}' '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && chown -R root:wheel '/Library/Audio/Plug-Ins/HAL/ShizukaAudio.driver' && launchctl kickstart -kp system/com.apple.audio.coreaudiod\" with administrator privileges",
        bundle_escaped
    );

    let osa_out = Command::new("osascript")
        .args(["-e", &osa_cmd])
        .output()
        .context("管理者権限プロンプトの起動に失敗しました")?;

    let _ = fs::remove_dir_all(&work_dir);

    if !osa_out.status.success() {
        return Err(anyhow!("ドライバの配置がキャンセルされたか失敗しました。"));
    }

    for _ in 0..5 {
        thread::sleep(Duration::from_millis(500));
        let devs = list_audio_devices();
        if devs.has_virtual_cable {
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
