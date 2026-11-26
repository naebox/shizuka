use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

/// User-configurable UI language preference stored in `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanguagePreference {
    /// Follow the operating system's primary UI language (`ja*` -> Japanese, otherwise English).
    #[default]
    Auto,
    /// Always display UI and tray menus in Japanese.
    Ja,
    /// Always display UI and tray menus in English.
    En,
}

/// Concrete resolved UI language (`ja` or `en`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    Ja,
    En,
}

impl LanguagePreference {
    #[inline]
    pub fn resolve(self) -> Locale {
        match self {
            Self::Auto => detect_os_locale(),
            Self::Ja => Locale::Ja,
            Self::En => Locale::En,
        }
    }
}

impl Locale {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }

    #[inline]
    const fn to_u8(self) -> u8 {
        match self {
            Self::Ja => 0,
            Self::En => 1,
        }
    }

    #[inline]
    const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::En,
            _ => Self::Ja,
        }
    }
}

/// Process-wide active locale for non-RT backend messages, tray labels, and error formatting.
/// Initialized at startup from `AppConfig` and updated atomically whenever `set_language` runs.
static ACTIVE_LOCALE: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn set_active_locale(locale: Locale) {
    ACTIVE_LOCALE.store(locale.to_u8(), Ordering::Relaxed);
}

#[inline]
pub fn current_locale() -> Locale {
    Locale::from_u8(ACTIVE_LOCALE.load(Ordering::Relaxed))
}

#[inline]
pub fn tr_for<'a>(locale: Locale, ja: &'a str, en: &'a str) -> &'a str {
    match locale {
        Locale::Ja => ja,
        Locale::En => en,
    }
}

#[inline]
pub fn tr<'a>(ja: &'a str, en: &'a str) -> &'a str {
    tr_for(current_locale(), ja, en)
}

/// Parses a BCP-47 / POSIX locale identifier (e.g. `"ja"`, `"ja-JP"`, `"ja_JP.UTF-8"`, `"en-US"`)
/// into Shizuka's supported `Locale` (`Ja` for Japanese, `En` for all other languages).
pub fn parse_locale_tag(raw: &str) -> Locale {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Locale::En;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "ja"
        || lower.starts_with("ja-")
        || lower.starts_with("ja_")
        || lower.starts_with("ja.")
        || lower.starts_with("japanese")
    {
        Locale::Ja
    } else {
        Locale::En
    }
}

fn detect_locale_from_env() -> Option<Locale> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("c") && !trimmed.eq_ignore_ascii_case("posix") {
                return Some(parse_locale_tag(trimmed));
            }
        }
    }
    None
}

/// Detects the operating system's primary UI display language without external dependencies.
pub fn detect_os_locale() -> Locale {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        let lang_id = unsafe { GetUserDefaultUILanguage() };
        if lang_id != 0 {
            // Primary language ID mask is 0x03ff; LANG_JAPANESE = 0x11
            return if (lang_id & 0x03ff) == 0x11 {
                Locale::Ja
            } else {
                Locale::En
            };
        }
        detect_locale_from_env().unwrap_or(Locale::En)
    }

    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSLocale;
        let preferred = NSLocale::preferredLanguages();
        if let Some(first) = preferred.firstObject() {
            return parse_locale_tag(&first.to_string());
        }
        detect_locale_from_env().unwrap_or(Locale::En)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        detect_locale_from_env().unwrap_or(Locale::En)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_locale_tag_variants() {
        assert_eq!(parse_locale_tag("ja"), Locale::Ja);
        assert_eq!(parse_locale_tag("ja-JP"), Locale::Ja);
        assert_eq!(parse_locale_tag("ja_JP.UTF-8"), Locale::Ja);
        assert_eq!(parse_locale_tag("ja-Kana-JP"), Locale::Ja);
        assert_eq!(parse_locale_tag("Japanese_Japan.932"), Locale::Ja);

        assert_eq!(parse_locale_tag("en"), Locale::En);
        assert_eq!(parse_locale_tag("en-US"), Locale::En);
        assert_eq!(parse_locale_tag("en_GB.UTF-8"), Locale::En);
        assert_eq!(parse_locale_tag("fr-FR"), Locale::En);
        assert_eq!(parse_locale_tag("zh-Hans-CN"), Locale::En);
        assert_eq!(parse_locale_tag("jv-ID"), Locale::En); // Javanese must NOT match Japanese
        assert_eq!(parse_locale_tag(""), Locale::En);
    }

    #[test]
    fn test_language_preference_serde_and_resolve() {
        assert_eq!(
            serde_json::to_string(&LanguagePreference::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&LanguagePreference::Ja).unwrap(),
            "\"ja\""
        );
        assert_eq!(
            serde_json::to_string(&LanguagePreference::En).unwrap(),
            "\"en\""
        );

        assert_eq!(
            serde_json::from_str::<LanguagePreference>("\"auto\"").unwrap(),
            LanguagePreference::Auto
        );
        assert_eq!(
            serde_json::from_str::<LanguagePreference>("\"ja\"").unwrap(),
            LanguagePreference::Ja
        );
        assert_eq!(
            serde_json::from_str::<LanguagePreference>("\"en\"").unwrap(),
            LanguagePreference::En
        );

        assert_eq!(LanguagePreference::Ja.resolve(), Locale::Ja);
        assert_eq!(LanguagePreference::En.resolve(), Locale::En);
        let _ = LanguagePreference::Auto.resolve();
    }
}
