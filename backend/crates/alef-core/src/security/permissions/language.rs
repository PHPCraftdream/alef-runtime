// SPDX-License-Identifier: MIT OR Apache-2.0
//! The interface language of the runtime and where it is stored: the table of the supported
//! languages with their UI strings, and a small Ktav file (`settings.ktav`) with one key. The
//! setting is cosmetic, so a missing, damaged, or wrong-version file simply reads as English.

use std::io;
use std::path::PathBuf;

use crate::security::permissions::store;
use serde::{Deserialize, Serialize};

/// The version of the Ktav state file.
const VERSION: u32 = 1;

/// The file with the stored setting.
const SETTINGS_FILE: &str = "settings.ktav";

/// An interface language the runtime supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
    Russian,
    Spanish,
    Arabic,
    Hebrew,
}

impl Default for Language {
    fn default() -> Self {
        Self::English
    }
}

/// One row of the string table: every UI string of a language, kept together so a window needs
/// only one lookup per language.
struct Strings {
    tray_apps: &'static str,
    tray_settings: &'static str,
    tray_quit: &'static str,
    tray_tooltip: &'static str,
    window_title: &'static str,
    section_language: &'static str,
    selection_label: &'static str,
}

const STRINGS: [(&str, Strings); 6] = [
    (
        "en",
        Strings {
            tray_apps: "Apps",
            tray_settings: "Settings",
            tray_quit: "Quit",
            tray_tooltip: "Alef Broker",
            window_title: "Settings",
            section_language: "Language",
            selection_label: "Interface language",
        },
    ),
    (
        "zh",
        Strings {
            tray_apps: "应用",
            tray_settings: "设置",
            tray_quit: "退出",
            tray_tooltip: "Alef 代理",
            window_title: "设置",
            section_language: "语言",
            selection_label: "界面语言",
        },
    ),
    (
        "ru",
        Strings {
            tray_apps: "Приложения",
            tray_settings: "Настройки",
            tray_quit: "Выход",
            tray_tooltip: "Брокер Alef",
            window_title: "Настройки",
            section_language: "Язык",
            selection_label: "Язык интерфейса",
        },
    ),
    (
        "es",
        Strings {
            tray_apps: "Aplicaciones",
            tray_settings: "Ajustes",
            tray_quit: "Salir",
            tray_tooltip: "Broker de Alef",
            window_title: "Ajustes",
            section_language: "Idioma",
            selection_label: "Idioma de la interfaz",
        },
    ),
    (
        "ar",
        Strings {
            tray_apps: "التطبيقات",
            tray_settings: "الإعدادات",
            tray_quit: "إنهاء",
            tray_tooltip: "وسيط Alef",
            window_title: "الإعدادات",
            section_language: "اللغة",
            selection_label: "لغة الواجهة",
        },
    ),
    (
        "he",
        Strings {
            tray_apps: "יישומים",
            tray_settings: "הגדרות",
            tray_quit: "יציאה",
            tray_tooltip: "תיווך Alef",
            window_title: "הגדרות",
            section_language: "שפה",
            selection_label: "שפת הממשק",
        },
    ),
];

impl Language {
    /// The language of an exact lowercase code; unknown codes are `None`.
    pub fn from_code(code: &str) -> Option<Language> {
        Some(match code {
            "en" => Self::English,
            "zh" => Self::Chinese,
            "ru" => Self::Russian,
            "es" => Self::Spanish,
            "ar" => Self::Arabic,
            "he" => Self::Hebrew,
            _ => return None,
        })
    }

    /// Like [`Self::from_code`], but an unknown code falls back to the default.
    pub fn parse_or_default(code: &str) -> Language {
        Self::from_code(code).unwrap_or_default()
    }

    /// All languages in display order.
    pub fn all() -> [Language; 6] {
        [
            Self::English,
            Self::Chinese,
            Self::Russian,
            Self::Spanish,
            Self::Arabic,
            Self::Hebrew,
        ]
    }

    /// The lowercase code of the language.
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh",
            Self::Russian => "ru",
            Self::Spanish => "es",
            Self::Arabic => "ar",
            Self::Hebrew => "he",
        }
    }

    /// The name of the language, written in the language itself.
    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "中文",
            Self::Russian => "Русский",
            Self::Spanish => "Español",
            Self::Arabic => "العربية",
            Self::Hebrew => "עברית",
        }
    }

    /// Whether the language is written right to left.
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Arabic | Self::Hebrew)
    }

    fn strings(self) -> &'static Strings {
        // The table is in the same order as `all`, and every language is in `all`.
        let index = Self::all()
            .iter()
            .position(|language| *language == self)
            .expect("every language has a row in the string table");
        &STRINGS[index].1
    }

    /// "Apps" of the tray menu.
    pub fn tray_apps(self) -> &'static str {
        self.strings().tray_apps
    }

    /// "Settings" of the tray menu.
    pub fn tray_settings(self) -> &'static str {
        self.strings().tray_settings
    }

    /// "Quit" of the tray menu.
    pub fn tray_quit(self) -> &'static str {
        self.strings().tray_quit
    }

    /// Tooltip of the tray icon.
    pub fn tray_tooltip(self) -> &'static str {
        self.strings().tray_tooltip
    }

    /// Title of the settings window.
    pub fn window_title(self) -> &'static str {
        self.strings().window_title
    }

    /// Heading of the language section.
    pub fn section_language(self) -> &'static str {
        self.strings().section_language
    }

    /// Label near the language selector.
    pub fn selection_label(self) -> &'static str {
        self.strings().selection_label
    }
}

#[derive(Serialize, Deserialize)]
struct SavedSettings {
    version: u32,
    language: String,
}

/// Where the stored interface setting of the runtime lives.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    folder: PathBuf,
}

impl SettingsStore {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
        }
    }

    fn path(&self) -> PathBuf {
        self.folder.join(SETTINGS_FILE)
    }

    /// The stored language; a missing, damaged, or wrong-version file, or an unknown code, is
    /// English — the setting is cosmetic and never makes the runtime fail.
    pub fn language(&self) -> Language {
        let Ok(bytes) = std::fs::read(self.path()) else {
            return Language::English;
        };
        let Ok(saved) = store::from_bytes::<SavedSettings>(&bytes) else {
            return Language::English;
        };
        if saved.version != VERSION {
            return Language::English;
        }
        Language::parse_or_default(&saved.language)
    }

    /// Validates and stores the language of `code`, atomically (`.part` write, then rename).
    pub fn set_language(&self, code: &str) -> io::Result<Language> {
        let language = Language::from_code(code).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown interface language: {code}"),
            )
        })?;
        let saved = SavedSettings {
            version: VERSION,
            language: language.code().to_owned(),
        };
        let bytes = store::to_bytes(&saved)?;
        std::fs::create_dir_all(&self.folder)?;
        let path = self.path();
        let partial = path.with_extension("ktav.part");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &path)?;
        Ok(language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn every_language_has_a_code_and_a_native_name() {
        let codes: Vec<_> = Language::all().iter().map(|l| l.code()).collect();
        assert_eq!(codes, ["en", "zh", "ru", "es", "ar", "he"]);
        for language in Language::all() {
            assert_eq!(Language::from_code(language.code()), Some(language));
            assert!(!language.native_name().is_empty());
        }
        let names: Vec<_> = Language::all().iter().map(|l| l.native_name()).collect();
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn only_arabic_and_hebrew_are_rtl() {
        for language in Language::all() {
            assert_eq!(language.is_rtl(), matches!(language.code(), "ar" | "he"));
        }
    }

    #[test]
    fn every_string_is_non_empty_for_every_language() {
        for language in Language::all() {
            for text in [
                language.tray_apps(),
                language.tray_settings(),
                language.tray_quit(),
                language.tray_tooltip(),
                language.window_title(),
                language.section_language(),
                language.selection_label(),
            ] {
                assert!(!text.is_empty());
            }
        }
    }

    #[test]
    fn unknown_codes_do_not_parse() {
        assert_eq!(Language::from_code("EN"), None);
        assert_eq!(Language::from_code("fr"), None);
        assert_eq!(Language::parse_or_default("xx"), Language::English);
    }

    #[test]
    fn the_setting_round_trips() {
        let dir = tempdir().unwrap();
        let store = SettingsStore::new(dir.path());
        assert_eq!(store.set_language("ru").unwrap(), Language::Russian);
        assert_eq!(store.language(), Language::Russian);
        assert!(dir.path().join("settings.ktav").exists());
    }

    #[test]
    fn a_missing_file_is_english() {
        let dir = tempdir().unwrap();
        assert_eq!(SettingsStore::new(dir.path()).language(), Language::English);
    }

    #[test]
    fn a_damaged_file_is_english() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("settings.ktav"), b"garbage").unwrap();
        assert_eq!(SettingsStore::new(dir.path()).language(), Language::English);
    }

    #[test]
    fn a_wrong_version_is_english() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let bytes = store::to_bytes(&SavedSettings {
            version: 99,
            language: "ru".to_owned(),
        })
        .unwrap();
        std::fs::write(dir.path().join("settings.ktav"), bytes).unwrap();
        assert_eq!(SettingsStore::new(dir.path()).language(), Language::English);
    }

    #[test]
    fn an_unknown_code_is_rejected() {
        let dir = tempdir().unwrap();
        let store = SettingsStore::new(dir.path());
        let error = store.set_language("fr").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
