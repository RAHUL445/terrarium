use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tauri::State;

use crate::plugins::Plugins;

fn run(state: State<'_, Plugins>, name: &str, args: Value) -> Result<Value, String> {
    state
        .with_host(|host| terrarium_runtime::services::invoke(host, name, &args, cfg!(desktop)))
        .ok_or("plugin host unavailable")?
        .ok_or_else(|| format!("unknown command `{name}`"))?
}

#[tauri::command]
pub fn blob_put(
    state: State<'_, Plugins>,
    name: String,
    mime: String,
    data_base64: String,
) -> Result<String, String> {
    run(
        state,
        "blob.put",
        json!({"name": name, "mime": mime, "data_base64": data_base64}),
    )?
    .as_str()
    .map(str::to_string)
    .ok_or("invalid blob.put result".into())
}

#[tauri::command]
pub fn blob_get(state: State<'_, Plugins>, blob: String) -> Result<Value, String> {
    run(state, "blob.get", json!({"blob": blob}))
}

#[tauri::command]
pub fn blob_info(state: State<'_, Plugins>, blob: String) -> Result<Value, String> {
    run(state, "blob.info", json!({"blob": blob}))
}

#[tauri::command]
pub fn blob_save(
    state: State<'_, Plugins>,
    blob: String,
    suggested_name: String,
) -> Result<(), String> {
    #[cfg(desktop)]
    {
        let value = run(state, "blob.get", json!({"blob": blob}))?;
        if value
            .get("missing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err("Not on this device yet".into());
        }
        let data = value
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or("blob data is unavailable")?;
        let bytes = B64
            .decode(data)
            .map_err(|_| "blob data is not valid base64".to_string())?;
        let mut dialog = rfd::FileDialog::new();
        if let Some(name) = safe_name(&suggested_name) {
            dialog = dialog.set_file_name(&name);
        }
        let Some(path) = dialog.save_file() else {
            return Ok(());
        };
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(desktop))]
    {
        let _ = (state, blob, suggested_name);
        Err("Saving files is not available on this device.".into())
    }
}

#[cfg(desktop)]
fn safe_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|c| *c >= ' ' && *c != '/' && *c != '\\')
        .take(120)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}
