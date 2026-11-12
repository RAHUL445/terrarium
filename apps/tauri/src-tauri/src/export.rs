//! Export the current space to a fixed folder and reveal completed exports.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use terrarium_runtime::commands::Kind;
use terrarium_runtime::DEFAULT_SPACE;

use crate::err;
use crate::plugins::Plugins;

fn exports_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = if cfg!(desktop) {
        app.path()
            .document_dir()
            .map_err(err)?
            .join("Terrarium Exports")
    } else {
        app.path().app_data_dir().map_err(err)?.join("exports")
    };
    fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir)
}

#[derive(Serialize, Deserialize)]
pub struct ExportResult {
    path: String,
    bytes: u64,
    rows: u64,
    plugins: u64,
}

#[tauri::command]
pub async fn export_space(
    app: AppHandle,
    space: Option<String>,
    include_trash: bool,
) -> Result<ExportResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let space = space.unwrap_or_else(|| DEFAULT_SPACE.to_string());
        let stamp = time::OffsetDateTime::now_utc();
        let name = format!(
            "terrarium-{}-{:04}{:02}{:02}-{:02}{:02}{:02}Z.zip",
            space,
            stamp.year(),
            u8::from(stamp.month()),
            stamp.day(),
            stamp.hour(),
            stamp.minute(),
            stamp.second()
        );
        let path = exports_dir(&app)?.join(name);
        let result = crate::plugins::app_command(
            &app.state::<Plugins>(),
            Kind::Action,
            "space.export",
            Some(space),
            json!({"path": path, "include_trash": include_trash}),
        )?;
        serde_json::from_value(result).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn reveal_export(app: AppHandle, path: String) -> Result<(), String> {
    let dir = exports_dir(&app)?.canonicalize().map_err(err)?;
    let path = PathBuf::from(path)
        .canonicalize()
        .map_err(|_| "Only exports can be shown.".to_string())?;
    if !path.starts_with(&dir) {
        return Err("Only exports can be shown.".into());
    }
    app.opener().reveal_item_in_dir(path).map_err(err)
}
