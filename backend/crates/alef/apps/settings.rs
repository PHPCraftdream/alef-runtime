// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime-owned settings window and its local commands.
use std::{fs, path::Path, sync::Arc};

use alef_core::{
    error::{AlefError, ErrorCode},
    registry::{command::Reply, dispatch::Registry, host::Host},
    security::{
        permissions::language::{Language, SettingsStore},
        window::WindowDef,
    },
};
use alef_runtime::{Bridge, BridgeOptions, Commands, WindowOptions};
use serde::Deserialize;
use serde_json::json;
#[cfg(feature = "e2e")]
use serde_json::Value;

const PAGE: [(&str, &str); 3] = [
    ("index.html", include_str!("../assets/settings/index.html")),
    (
        "settings.css",
        include_str!("../assets/settings/settings.css"),
    ),
    (
        "settings.js",
        include_str!("../assets/settings/settings.js"),
    ),
];
#[cfg(feature = "e2e")]
const AUTOMATION: (&str, &str) = (
    "settings-e2e.js",
    include_str!("../assets/settings/settings-e2e.js"),
);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    code: String,
}

fn invalid(error: impl ToString) -> AlefError {
    AlefError::new(ErrorCode::InvalidArgument, error.to_string())
}
fn store() -> SettingsStore {
    SettingsStore::new(alef_launch::consent::runtime_home())
}
/// The one-line report of a language, shared by the CLI prints.
fn language_line(language: Language) -> String {
    format!("language: {}", language.code())
}
/// The whole page payload: the current language, the table, and the strings of the language.
fn view() -> Result<serde_json::Value, AlefError> {
    let language = store().language();
    let languages: Vec<_> = Language::all()
        .into_iter()
        .map(|row| {
            json!({
                "code": row.code(),
                "nativeName": row.native_name(),
                "rtl": row.is_rtl(),
            })
        })
        .collect();
    Ok(json!({
        "language": language.code(),
        "rtl": language.is_rtl(),
        "languages": languages,
        "texts": {
            "title": language.window_title(),
            "section": language.section_language(),
            "label": language.selection_label(),
        },
    }))
}

/// `alef settings`: the stored language, as a line or as JSON.
pub fn print_current(json_output: bool) -> Result<String, String> {
    let language = store().language();
    Ok(if json_output {
        json!({ "language": language.code() }).to_string()
    } else {
        language_line(language)
    })
}
/// `alef settings set language <CODE>`: validates and stores, then reports the stored code.
pub fn settings_set_line(code: &str) -> Result<String, String> {
    match store().set_language(code) {
        Ok(language) => Ok(language_line(language)),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            Err(format!("settings: unknown language {code}"))
        }
        Err(error) => Err(format!("settings: {error}")),
    }
}

fn install(registry: &mut Registry, host: Arc<dyn Host>) -> Result<(), AlefError> {
    alef_launch::singleton::on_raise({
        let host = host.clone();
        Arc::new(move || host.focus())
    });
    install_commands(registry, host)?;
    #[cfg(feature = "e2e")]
    install_automation(registry)?;
    Ok(())
}
fn install_commands(registry: &mut Registry, host: Arc<dyn Host>) -> Result<(), AlefError> {
    registry
        .register_runtime::<()>("runtime.settings.get")?
        .handler(|_, ()| async move { Ok(Reply::Json(view()?)) })?;
    registry
        .register_runtime::<SetArgs>("runtime.settings.set")?
        .handler(move |_, args: SetArgs| {
            let code = args.code;
            async move {
                let language = store().set_language(&code).map_err(invalid)?;
                Ok(Reply::Json(json!({ "language": language.code() })))
            }
        })?;
    registry
        .register_runtime::<()>("runtime.settings.close")?
        .handler(move |_, ()| {
            let host = host.clone();
            async move {
                host.quit(0);
                Ok(Reply::Json(serde_json::Value::Null))
            }
        })
}
#[cfg(feature = "e2e")]
fn install_automation(registry: &mut Registry) -> Result<(), AlefError> {
    if !std::env::var("ALEF_E2E").is_ok_and(|value| value == "1") {
        return Ok(());
    }
    registry
        .register_runtime::<()>("runtime.settings.automation")?
        .handler(|_, ()| async move {
            let raw = std::env::var("ALEF_E2E_SETTINGS_UI").unwrap_or_default();
            let steps = if raw.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&raw).map_err(invalid)?
            };
            Ok(Reply::Json(steps))
        })
}

pub async fn run(icon: Vec<u8>) -> Result<(), String> {
    // Singleton gate: `None` = an existing window was raised; exit 0.
    let _singleton = match alef_launch::singleton::enter(alef_launch::singleton::Kind::Settings) {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let pages = alef_launch::consent::ask::exchange_folder()
        .join(format!("{}-settings-page", std::process::id()));
    fs::create_dir_all(&pages).map_err(|error| format!("the page cannot be put down: {error}"))?;
    for (name, text) in PAGE {
        fs::write(pages.join(name), text)
            .map_err(|error| format!("the page cannot be put down: {error}"))?;
    }
    #[cfg(feature = "e2e")]
    if std::env::var("ALEF_E2E").is_ok_and(|value| value == "1") {
        fs::write(pages.join(AUTOMATION.0), AUTOMATION.1)
            .map_err(|error| format!("the page cannot be put down: {error}"))?;
        let index = PAGE[0].1.replace(
            "</head>",
            "<script type=\"module\" src=\"settings-e2e.js\"></script>\n</head>",
        );
        fs::write(pages.join("index.html"), index)
            .map_err(|error| format!("the page cannot be put down: {error}"))?;
    }
    let result = show(&pages, icon).await;
    let _ = fs::remove_dir_all(&pages);
    result
}
async fn show(pages: &Path, icon: Vec<u8>) -> Result<(), String> {
    let assets = alef_runtime::prepare_assets(
        alef_runtime::AssetInput::Disk {
            application_dir: pages
                .parent()
                .ok_or("Missing settings application boundary")?
                .to_owned(),
            selected_root: pages.to_owned(),
        },
        Some("/index.html"),
        &[],
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut bridge = Bridge::with_options(
        Commands::new(),
        Some(assets),
        None,
        BridgeOptions {
            modules: Some(Box::new(|registry, host| install(registry, host))),
            entry: Some("/index.html".to_owned()),
            ..BridgeOptions::default()
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let window: WindowDef = serde_json::from_value(json!({
        "label": "settings", "url": "/index.html", "width": 520, "height": 420,
        "minWidth": 420, "minHeight": 320, "title": "Settings",
    }))
    .map_err(|error| error.to_string())?;
    let result = alef_runtime::run(
        &mut bridge,
        WindowOptions {
            windows: vec![window],
            ..WindowOptions::new("Settings".to_owned(), icon)
        },
    );
    bridge.shutdown().await.map_err(|e| e.to_string())?;
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Holds the lock and the scratch folder, and restores `ALEF_HOME` on drop.
    struct HomeGuard<'a> {
        _lock: std::sync::MutexGuard<'a, ()>,
        _scratch: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    impl HomeGuard<'_> {
        fn take() -> Self {
            // Poisoned lock is fine: it still serializes.
            let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let scratch = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("ALEF_HOME");
            std::env::set_var("ALEF_HOME", scratch.path());
            Self {
                _lock: lock,
                _scratch: scratch,
                previous,
            }
        }
    }
    impl Drop for HomeGuard<'_> {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("ALEF_HOME", value),
                None => std::env::remove_var("ALEF_HOME"),
            }
        }
    }

    #[test]
    fn the_language_line_carries_the_code() {
        assert_eq!(language_line(Language::Russian), "language: ru");
        assert_eq!(language_line(Language::English), "language: en");
    }

    #[test]
    fn the_json_print_carries_only_the_language() {
        let line = language_line(Language::Chinese);
        assert_eq!(line, "language: zh");
        let value = json!({ "language": Language::Chinese.code() });
        assert_eq!(value.to_string(), r#"{"language":"zh"}"#);
    }

    #[test]
    fn an_unknown_code_is_a_usage_failure_and_a_known_code_stores() {
        let scratch = tempfile::tempdir().unwrap();
        let settings = SettingsStore::new(scratch.path());
        let error = settings.set_language("klingon").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(settings.language(), Language::English);
        assert_eq!(settings.set_language("ru").unwrap(), Language::Russian);
        assert_eq!(settings.language(), Language::Russian);
    }

    #[test]
    fn print_current_reads_the_stored_setting_through_the_env() {
        let _guard = HomeGuard::take();
        store().set_language("ru").unwrap();
        assert_eq!(print_current(false).unwrap(), "language: ru");
        assert_eq!(print_current(true).unwrap(), r#"{"language":"ru"}"#);
    }
}
