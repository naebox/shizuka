use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

const TRIGGER_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    ToggleNc,
    ToggleMute,
}

fn is_unmodified_key_allowed(code: Code) -> bool {
    matches!(
        code,
        Code::F1
            | Code::F2
            | Code::F3
            | Code::F4
            | Code::F5
            | Code::F6
            | Code::F7
            | Code::F8
            | Code::F9
            | Code::F10
            | Code::F11
            | Code::F12
            | Code::F13
            | Code::F14
            | Code::F15
            | Code::F16
            | Code::F17
            | Code::F18
            | Code::F19
            | Code::F20
            | Code::F21
            | Code::F22
            | Code::F23
            | Code::F24
            | Code::Pause
            | Code::ScrollLock
            | Code::PrintScreen
            | Code::NumLock
            | Code::AudioVolumeMute
            | Code::MediaPlayPause
            | Code::MediaStop
            | Code::MediaTrackNext
            | Code::MediaTrackPrevious
    )
}

pub fn validate_and_parse_shortcut(raw: &str) -> Result<Shortcut, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("ショートカットキーが空です".to_string());
    }
    let shortcut = Shortcut::from_str(trimmed)
        .map_err(|_| format!("未対応または無効なショートカット形式です: {}", trimmed))?;

    if shortcut.mods.is_empty() && !is_unmodified_key_allowed(shortcut.key) {
        return Err(
            "単一キーはファンクションキー（F1〜F24 等）のみ登録できます。Ctrl / Alt / Shift 等と組み合わせてください。"
                .to_string(),
        );
    }

    Ok(shortcut)
}

#[derive(Default)]
pub struct ShortcutRegistry {
    toggle_nc: Option<Shortcut>,
    toggle_mute: Option<Shortcut>,
    paused: bool,
    last_triggered_at: Option<Instant>,
    startup_warning: Option<String>,
}

impl ShortcutRegistry {
    pub fn register_initial(
        &mut self,
        app: &AppHandle,
        nc_raw: Option<&str>,
        mute_raw: Option<&str>,
    ) {
        let mut warnings = Vec::new();
        let manager = app.global_shortcut();

        if let Some(raw) = nc_raw.map(str::trim).filter(|s| !s.is_empty()) {
            match validate_and_parse_shortcut(raw) {
                Ok(sc) => match manager.register(sc) {
                    Ok(()) => self.toggle_nc = Some(sc),
                    Err(e) => warnings.push(format!(
                        "ノイズ除去ショートカット（{}）を登録できませんでした: {}",
                        raw, e
                    )),
                },
                Err(e) => warnings.push(e),
            }
        }

        if let Some(raw) = mute_raw.map(str::trim).filter(|s| !s.is_empty()) {
            match validate_and_parse_shortcut(raw) {
                Ok(sc) => {
                    if self.toggle_nc == Some(sc) {
                        warnings.push(
                            "ノイズ除去とマイクミュートに同じショートカットが設定されています。"
                                .to_string(),
                        );
                    } else {
                        match manager.register(sc) {
                            Ok(()) => self.toggle_mute = Some(sc),
                            Err(e) => warnings.push(format!(
                                "マイクミュートショートカット（{}）を登録できませんでした: {}",
                                raw, e
                            )),
                        }
                    }
                }
                Err(e) => warnings.push(e),
            }
        }

        self.startup_warning = if warnings.is_empty() {
            None
        } else {
            Some(warnings.join(" / "))
        };
    }

    pub fn startup_warning(&self) -> Option<String> {
        self.startup_warning.clone()
    }

    /// Updates a shortcut binding transactionally and ensures shortcuts are active (unpaused):
    /// validates OS registration for the new key before committing, rolling back to the previous
    /// binding if the OS rejects registration (e.g., hotkey already taken by another app).
    pub fn update_binding(
        &mut self,
        app: &AppHandle,
        action: ShortcutAction,
        raw: Option<&str>,
    ) -> Result<Option<String>, String> {
        let cleaned = raw.map(str::trim).filter(|s| !s.is_empty());
        let new_shortcut = match cleaned {
            Some(s) => Some(validate_and_parse_shortcut(s)?),
            None => None,
        };

        let other_shortcut = match action {
            ShortcutAction::ToggleNc => self.toggle_mute,
            ShortcutAction::ToggleMute => self.toggle_nc,
        };
        if let (Some(new_sc), Some(other_sc)) = (new_shortcut, other_shortcut) {
            if new_sc == other_sc {
                return Err(
                    "もう一方の操作と同じショートカットは登録できません。別のキーを指定してください。"
                        .to_string(),
                );
            }
        }

        let old_shortcut = match action {
            ShortcutAction::ToggleNc => self.toggle_nc,
            ShortcutAction::ToggleMute => self.toggle_mute,
        };

        let manager = app.global_shortcut();
        let was_paused = self.paused;

        if old_shortcut == new_shortcut {
            if was_paused {
                self.set_paused(app, false)?;
            }
            self.startup_warning = None;
            return Ok(cleaned.map(String::from));
        }

        if !was_paused {
            if let Some(old_sc) = old_shortcut {
                let _ = manager.unregister(old_sc);
            }
        }

        if let Some(new_sc) = new_shortcut {
            if let Err(err) = manager.register(new_sc) {
                // Roll back so user retains previous working bindings
                if let Some(old_sc) = old_shortcut {
                    let _ = manager.register(old_sc);
                }
                if was_paused {
                    if let Some(other_sc) = other_shortcut {
                        let _ = manager.register(other_sc);
                    }
                    self.paused = false;
                }
                return Err(format!(
                    "このショートカットは他のアプリまたは OS で使用されているため登録できません（{}）",
                    err
                ));
            }
        }

        if was_paused {
            if let Some(other_sc) = other_shortcut {
                let _ = manager.register(other_sc);
            }
            self.paused = false;
        }

        match action {
            ShortcutAction::ToggleNc => self.toggle_nc = new_shortcut,
            ShortcutAction::ToggleMute => self.toggle_mute = new_shortcut,
        }
        self.startup_warning = None;
        Ok(cleaned.map(String::from))
    }

    /// Temporarily unregisters active global shortcuts while the UI is recording a new key combo.
    pub fn set_paused(&mut self, app: &AppHandle, paused: bool) -> Result<(), String> {
        if self.paused == paused {
            return Ok(());
        }
        let manager = app.global_shortcut();
        if paused {
            if let Some(sc) = self.toggle_nc {
                let _ = manager.unregister(sc);
            }
            if let Some(sc) = self.toggle_mute {
                let _ = manager.unregister(sc);
            }
            self.paused = true;
            Ok(())
        } else {
            self.paused = false;
            let mut failed = Vec::new();
            if let Some(sc) = self.toggle_nc {
                if let Err(e) = manager.register(sc) {
                    failed.push(format!("{}", e));
                }
            }
            if let Some(sc) = self.toggle_mute {
                if let Err(e) = manager.register(sc) {
                    failed.push(format!("{}", e));
                }
            }
            if failed.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "ショートカットの再登録に失敗しました: {}",
                    failed.join(", ")
                ))
            }
        }
    }

    pub fn match_triggered(&mut self, shortcut: &Shortcut) -> Option<ShortcutAction> {
        if self.paused {
            return None;
        }
        let now = Instant::now();
        if let Some(last) = self.last_triggered_at {
            if now.duration_since(last) < TRIGGER_DEBOUNCE {
                return None;
            }
        }

        let action = if self.toggle_nc == Some(*shortcut) {
            Some(ShortcutAction::ToggleNc)
        } else if self.toggle_mute == Some(*shortcut) {
            Some(ShortcutAction::ToggleMute)
        } else {
            None
        };

        if action.is_some() {
            self.last_triggered_at = Some(now);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shortcut_validation_rules() {
        assert!(validate_and_parse_shortcut("Control+Shift+KeyM").is_ok());
        assert!(validate_and_parse_shortcut("Alt+KeyN").is_ok());
        assert!(validate_and_parse_shortcut("Super+Shift+KeyD").is_ok());
        assert!(validate_and_parse_shortcut("F9").is_ok());
        assert!(validate_and_parse_shortcut("Pause").is_ok());

        // Plain typing keys without modifiers must be rejected
        assert!(validate_and_parse_shortcut("KeyA").is_err());
        assert!(validate_and_parse_shortcut("Digit1").is_err());
        assert!(validate_and_parse_shortcut("Space").is_err());
        assert!(validate_and_parse_shortcut("").is_err());
        assert!(validate_and_parse_shortcut("NotARealKey").is_err());
    }
}
