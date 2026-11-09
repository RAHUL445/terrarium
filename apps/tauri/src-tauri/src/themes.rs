//! Installing and removing themes. A theme is data only — checked colours
//! and a font name — so it installs without a review step. Themes live in
//! the synced plugin set, so the user's other devices get them too; which
//! theme is in use is chosen per device, in the UI.

use serde::Serialize;
use tauri::State;
use terrarium_runtime::Theme;

use crate::plugins::{err, Plugins};

#[derive(Serialize)]
pub struct ThemeInfo {
    theme: Theme,
    /// Colour pairs below WCAG AA contrast.
    warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct Installed {
    theme: Theme,
    /// The version this one replaced, if it was installed before.
    replaced: Option<String>,
    warnings: Vec<String>,
}

#[tauri::command]
pub fn list_themes(plugins: State<'_, Plugins>) -> Result<Vec<ThemeInfo>, String> {
    let themes = plugins
        .with_host(|host| host.packages().themes().map_err(err))
        .ok_or("plugin host unavailable")??;
    Ok(themes
        .into_iter()
        .map(|theme| ThemeInfo {
            warnings: theme.warnings(),
            theme,
        })
        .collect())
}

/// `data` is the theme file's text.
#[tauri::command]
pub fn install_theme(plugins: State<'_, Plugins>, data: String) -> Result<Installed, String> {
    let theme = Theme::parse(data.as_bytes()).map_err(err)?;
    let replaced = plugins
        .with_host(|host| host.packages_mut().install_theme(&theme).map_err(err))
        .ok_or("plugin host unavailable")??;
    Ok(Installed {
        warnings: theme.warnings(),
        theme,
        replaced,
    })
}

#[tauri::command]
pub fn remove_theme(plugins: State<'_, Plugins>, id: String) -> Result<(), String> {
    plugins
        .with_host(|host| host.packages_mut().remove_theme(&id).map_err(err))
        .ok_or("plugin host unavailable")?
}
