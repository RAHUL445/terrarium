//! Desktop quick-capture shortcut settings and window lifecycle.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(desktop)]
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuickSettings {
    pub enabled: bool,
    pub shortcut: String,
}

impl Default for QuickSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Alt+Space".to_string(),
        }
    }
}

pub struct Quick {
    path: PathBuf,
    settings: Mutex<QuickSettings>,
    error: Mutex<Option<String>>,
}

impl Quick {
    fn open(path: PathBuf, defaults: QuickSettings) -> Self {
        Self {
            path,
            settings: Mutex::new(defaults),
            error: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> Value {
        let settings = self.settings.lock().expect("quick settings mutex poisoned");
        let error = self.error.lock().expect("quick error mutex poisoned");
        json!({
            "available": cfg!(desktop),
            "enabled": settings.enabled,
            "shortcut": settings.shortcut,
            "label": label(&settings.shortcut),
            "error": error.clone(),
        })
    }
}

pub fn read_settings(path: &Path) -> QuickSettings {
    let Ok(bytes) = fs::read(path) else {
        return QuickSettings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn write_settings(path: &Path, s: &QuickSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(s)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(path, bytes)
}

pub fn check_shortcut(s: &str) -> Result<(), String> {
    let input = s.trim();
    #[cfg(desktop)]
    {
        let shortcut = input
            .parse::<Shortcut>()
            .map_err(|_| "That shortcut isn't recognised.".to_string())?;
        if shortcut.mods.is_empty() {
            return Err("Use at least one of ⌥, ⌃, ⇧ or ⌘ with the key.".to_string());
        }
        Ok(())
    }
    #[cfg(not(desktop))]
    {
        if input.is_empty() {
            return Err("That shortcut isn't recognised.".to_string());
        }
        Ok(())
    }
}

pub fn label(s: &str) -> String {
    let mut out = String::new();
    let mut key = "";
    for token in s.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "option" | "alt" => out.push('⌥'),
            "control" | "ctrl" => out.push('⌃'),
            "shift" => out.push('⇧'),
            "command" | "cmd" | "super" | "commandorcontrol" | "commandorctrl" | "cmdorctrl"
            | "cmdorcontrol" => out.push('⌘'),
            _ => key = token,
        }
    }
    out.push_str(key);
    out
}

#[cfg(desktop)]
fn apply(app: &AppHandle, quick: &Quick) -> Result<(), String> {
    let settings = quick
        .settings
        .lock()
        .map_err(|_| "Quick settings are unavailable right now.".to_string())?
        .clone();

    let shortcuts = app.global_shortcut();
    shortcuts.unregister_all().map_err(|e| e.to_string())?;

    let mut message = None;
    if settings.enabled {
        match settings.shortcut.parse::<Shortcut>() {
            Ok(shortcut) => {
                if let Err(err) = shortcuts.register(shortcut) {
                    eprintln!("quick capture shortcut registration failed: {err}");
                    message = Some(
                        "Couldn't use this shortcut. Another app may be using it.".to_string(),
                    );
                }
            }
            Err(err) => {
                eprintln!("quick capture shortcut parse failed: {err}");
                message = Some("That shortcut isn't recognised.".to_string());
            }
        }
    }

    let mut error = quick
        .error
        .lock()
        .map_err(|_| "Quick settings are unavailable right now.".to_string())?;
    *error = message;
    Ok(())
}

#[cfg(desktop)]
pub fn start(app: &AppHandle, app_data: &Path) -> tauri::Result<()> {
    let path = app_data.join("quick-capture.json");
    app.manage(Quick::open(path.clone(), read_settings(&path)));
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    toggle(app);
                }
            })
            .build(),
    )?;
    let _ = apply(app, &app.state::<Quick>());
    Ok(())
}

#[cfg(not(desktop))]
pub fn start(app: &AppHandle, app_data: &Path) -> tauri::Result<()> {
    let path = app_data.join("quick-capture.json");
    app.manage(Quick::open(
        path,
        QuickSettings {
            enabled: false,
            shortcut: QuickSettings::default().shortcut,
        },
    ));
    Ok(())
}

#[cfg(desktop)]
pub fn toggle(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("quick") {
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);
        if visible && focused {
            let _ = window.hide();
            return;
        }
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit_to("quick", "quick-capture-open", ());
        return;
    }

    let mut builder =
        tauri::WebviewWindowBuilder::new(app, "quick", tauri::WebviewUrl::App("quick.html".into()))
            .title("Quick capture")
            .inner_size(520.0, 210.0)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .center()
            .visible(false);
    if let Ok(profile) = crate::storage::profile_dir() {
        if let Some(id) = crate::storage::webview_id(profile.as_deref(), cfg!(debug_assertions)) {
            if let Ok(root) = crate::storage::root(app) {
                builder = builder
                    .data_directory(root.join("webview"))
                    .data_store_identifier(id);
            }
        }
    }
    let window = builder.build();
    let Ok(window) = window else {
        return;
    };
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit_to("quick", "quick-capture-open", ());
}

#[cfg(not(desktop))]
pub fn toggle(_app: &AppHandle) {}

#[tauri::command]
pub fn quick_capture_settings(state: State<'_, Quick>) -> Value {
    state.snapshot()
}

#[tauri::command]
pub fn quick_capture_set(
    app: AppHandle,
    state: State<'_, Quick>,
    enabled: bool,
    shortcut: String,
) -> Result<Value, String> {
    if !cfg!(desktop) {
        return Err("Quick capture is only available on a computer.".into());
    }
    let shortcut = shortcut.trim().to_string();
    check_shortcut(&shortcut)?;
    let next = QuickSettings { enabled, shortcut };
    write_settings(&state.path, &next).map_err(|e| e.to_string())?;
    *state
        .settings
        .lock()
        .map_err(|_| "Quick settings are unavailable right now.".to_string())? = next;

    #[cfg(desktop)]
    apply(&app, &state)?;

    Ok(state.snapshot())
}

#[tauri::command]
pub fn quick_capture_done(app: AppHandle, saved: bool) {
    if let Some(window) = app.get_webview_window("quick") {
        let _ = window.hide();
    }
    if saved {
        let _ = app.emit("data-changed", ());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_and_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("quick-capture.json");
        assert_eq!(read_settings(&path), QuickSettings::default());
        fs::write(&path, b"{not json").expect("write broken");
        assert_eq!(read_settings(&path), QuickSettings::default());

        let value = QuickSettings {
            enabled: false,
            shortcut: "CommandOrControl+Alt+N".to_string(),
        };
        write_settings(&path, &value).expect("write settings");
        assert_eq!(read_settings(&path), value);
    }

    #[cfg(desktop)]
    #[test]
    fn shortcuts_need_a_modifier() {
        assert!(check_shortcut("Alt+Space").is_ok());
        assert!(check_shortcut("CommandOrControl+Shift+Space").is_ok());
        assert_eq!(
            check_shortcut("Space").expect_err("missing modifier must fail"),
            "Use at least one of ⌥, ⌃, ⇧ or ⌘ with the key."
        );
        assert_eq!(
            check_shortcut("Banana+Q").expect_err("unknown combo must fail"),
            "That shortcut isn't recognised."
        );
    }

    #[test]
    fn labels_use_key_symbols() {
        assert_eq!(label("Alt+Space"), "⌥Space");
        assert_eq!(label("CommandOrControl+Shift+Space"), "⌘⇧Space");
        assert_eq!(label("Control+Alt+Space"), "⌃⌥Space");
    }
}
