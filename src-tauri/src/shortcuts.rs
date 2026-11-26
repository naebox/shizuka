use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

use crate::i18n::{self, tr, tr_for, Locale};

const TRIGGER_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    ToggleNc,
    ToggleMute,
}

#[derive(Debug, Clone)]
enum ShortcutParseIssue {
    Empty,
    InvalidFormat(String),
    UnmodifiedKeyDisallowed,
}

impl ShortcutParseIssue {
    fn format_for(&self, locale: Locale) -> String {
        match self {
            Self::Empty => {
                tr_for(locale, "ショートカットキーが空です", "Shortcut key is empty").to_string()
            }
            Self::InvalidFormat(raw) => match locale {
                Locale::Ja => format!("未対応または無効なショートカット形式です: {}", raw),
                Locale::En => format!("Unsupported or invalid shortcut format: {}", raw),
            },
            Self::UnmodifiedKeyDisallowed => tr_for(
                locale,
                "単一キーはファンクションキー（F1〜F24 等）のみ登録できます。Ctrl / Alt / Shift 等と組み合わせてください。",
                "Single-key shortcuts are only allowed for function keys (F1–F24, etc.). Combine with Ctrl, Alt, or Shift.",
            )
            .to_string(),
        }
    }
}

#[derive(Debug, Clone)]
enum StartupWarning {
    ParseIssue(ShortcutParseIssue),
    RegisterNcFailed { raw: String, err: String },
    RegisterMuteFailed { raw: String, err: String },
    DuplicateBinding,
}

impl StartupWarning {
    fn format_for(&self, locale: Locale) -> String {
        match self {
            Self::ParseIssue(issue) => issue.format_for(locale),
            Self::RegisterNcFailed { raw, err } => match locale {
                Locale::Ja => format!(
                    "ノイズ除去ショートカット（{}）を登録できませんでした: {}",
                    raw, err
                ),
                Locale::En => format!(
                    "Could not register noise cancellation shortcut ({}): {}",
                    raw, err
                ),
            },
            Self::RegisterMuteFailed { raw, err } => match locale {
                Locale::Ja => format!(
                    "マイクミュートショートカット（{}）を登録できませんでした: {}",
                    raw, err
                ),
                Locale::En => format!(
                    "Could not register mic mute shortcut ({}): {}",
                    raw, err
                ),
            },
            Self::DuplicateBinding => tr_for(
                locale,
                "ノイズ除去とマイクミュートに同じショートカットが設定されています。",
                "Noise cancellation and mic mute are assigned the same shortcut.",
            )
            .to_string(),
        }
    }
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

fn parse_shortcut_inner(raw: &str) -> Result<Shortcut, ShortcutParseIssue> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ShortcutParseIssue::Empty);
    }
    let shortcut = Shortcut::from_str(trimmed)
        .map_err(|_| ShortcutParseIssue::InvalidFormat(trimmed.to_string()))?;

    if shortcut.mods.is_empty() && !is_unmodified_key_allowed(shortcut.key) {
        return Err(ShortcutParseIssue::UnmodifiedKeyDisallowed);
    }

    Ok(shortcut)
}

pub fn validate_and_parse_shortcut(raw: &str) -> Result<Shortcut, String> {
    parse_shortcut_inner(raw).map_err(|issue| issue.format_for(i18n::current_locale()))
}

#[derive(Default)]
pub struct ShortcutRegistry {
    toggle_nc: Option<Shortcut>,
    toggle_mute: Option<Shortcut>,
    paused: bool,
    last_triggered_nc: Option<Instant>,
    last_triggered_mute: Option<Instant>,
    startup_warnings: Vec<StartupWarning>,
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
            match parse_shortcut_inner(raw) {
                Ok(sc) => match manager.register(sc) {
                    Ok(()) => self.toggle_nc = Some(sc),
                    Err(e) => warnings.push(StartupWarning::RegisterNcFailed {
                        raw: raw.to_string(),
                        err: e.to_string(),
                    }),
                },
                Err(issue) => warnings.push(StartupWarning::ParseIssue(issue)),
            }
        }

        if let Some(raw) = mute_raw.map(str::trim).filter(|s| !s.is_empty()) {
            match parse_shortcut_inner(raw) {
                Ok(sc) => {
                    if self.toggle_nc == Some(sc) {
                        warnings.push(StartupWarning::DuplicateBinding);
                    } else {
                        match manager.register(sc) {
                            Ok(()) => self.toggle_mute = Some(sc),
                            Err(e) => warnings.push(StartupWarning::RegisterMuteFailed {
                                raw: raw.to_string(),
                                err: e.to_string(),
                            }),
                        }
                    }
                }
                Err(issue) => warnings.push(StartupWarning::ParseIssue(issue)),
            }
        }

        self.startup_warnings = warnings;
    }

    pub fn startup_warning(&self) -> Option<String> {
        if self.startup_warnings.is_empty() {
            None
        } else {
            let locale = i18n::current_locale();
            Some(
                self.startup_warnings
                    .iter()
                    .map(|w| w.format_for(locale))
                    .collect::<Vec<_>>()
                    .join(" / "),
            )
        }
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
                return Err(tr(
                    "もう一方の操作と同じショートカットは登録できません。別のキーを指定してください。",
                    "This shortcut is already assigned to the other action. Please choose a different key.",
                )
                .to_string());
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
            self.startup_warnings.clear();
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
                return Err(match i18n::current_locale() {
                    Locale::Ja => format!(
                        "このショートカットは他のアプリまたは OS で使用されているため登録できません（{}）",
                        err
                    ),
                    Locale::En => format!(
                        "This shortcut is already in use by another app or the OS ({})",
                        err
                    ),
                });
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
        self.startup_warnings.clear();
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
                let joined = failed.join(", ");
                Err(match i18n::current_locale() {
                    Locale::Ja => format!("ショートカットの再登録に失敗しました: {}", joined),
                    Locale::En => format!("Failed to re-register shortcuts: {}", joined),
                })
            }
        }
    }

    pub fn match_triggered(&mut self, shortcut: &Shortcut) -> Option<ShortcutAction> {
        if self.paused {
            return None;
        }
        let now = Instant::now();
        let (action, last_slot) = if self.toggle_nc == Some(*shortcut) {
            (ShortcutAction::ToggleNc, &mut self.last_triggered_nc)
        } else if self.toggle_mute == Some(*shortcut) {
            (ShortcutAction::ToggleMute, &mut self.last_triggered_mute)
        } else {
            return None;
        };

        if let Some(last) = *last_slot {
            if now.duration_since(last) < TRIGGER_DEBOUNCE {
                return None;
            }
        }
        *last_slot = Some(now);
        Some(action)
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

    #[test]
    fn test_shortcut_match_triggered_independent_debounce_and_pause() {
        let nc = validate_and_parse_shortcut("Control+Shift+KeyN").unwrap();
        let mute = validate_and_parse_shortcut("Control+Shift+KeyM").unwrap();

        let mut reg = ShortcutRegistry {
            toggle_nc: Some(nc),
            toggle_mute: Some(mute),
            ..Default::default()
        };

        // First press of NC triggers; immediate duplicate NC is debounced
        assert_eq!(reg.match_triggered(&nc), Some(ShortcutAction::ToggleNc));
        assert_eq!(reg.match_triggered(&nc), None);

        // Pressing Mute immediately after NC must STILL trigger (independent debounce slots)
        assert_eq!(reg.match_triggered(&mute), Some(ShortcutAction::ToggleMute));
        assert_eq!(reg.match_triggered(&mute), None);

        // When paused (recording a new key), all triggers are suppressed
        reg.paused = true;
        reg.last_triggered_nc = None;
        assert_eq!(reg.match_triggered(&nc), None);
    }
}
