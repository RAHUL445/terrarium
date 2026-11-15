//! The app's commands for Settings → AI and Settings → Scheduled, shared by
//! every host (the Tauri app, `terra preview`). They answer from the host's
//! own state; network calls (testing the provider, listing models) are left
//! to the host so it can make them without holding the plugin host.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Map, Value};
use terrarium_ai::{AiConfig, PRESETS};
use terrarium_blobs::{BlobError, MAX_BLOB};
use terrarium_core::FileRef;
use terrarium_scheduler::{Repeat, Settings};

use crate::pages::{
    clean_config, clean_host_config, clean_period, host_widgets, Placement, SavedView, HOST_WIDGETS,
};
use crate::registry::{PluginHost, ViewKind};

// Detect after decryption, preserving the existing encrypted blob format.
fn blob_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn month(ms: u64) -> String {
    crate::time::local_date(ms, "UTC")
        .map(|d| d[..7].to_string())
        .unwrap_or_default()
}

fn ai_state(host: &PluginHost) -> Value {
    let ai = host.ai();
    let users: Vec<Value> = host
        .manifests()
        .filter(|m| m.permissions.iter().any(|p| p == "ai"))
        .map(|m| json!({ "id": m.id, "name": m.name }))
        .collect();
    json!({
        "config": ai.config(),
        "defaults": AiConfig::default(),
        "presets": PRESETS,
        "status": ai.status(),
        "usage": ai.usage(&month(now_ms())),
        "plugins": users,
    })
}

fn schedules_state(host: &PluginHost) -> Value {
    let scheduler = host.scheduler();
    let names: BTreeMap<String, String> = host
        .manifests()
        .flat_map(|m| {
            std::iter::once(&m.id)
                .chain(&m.former_ids)
                .map(|id| (id.clone(), m.name.clone()))
        })
        .collect();
    let spaces: BTreeMap<String, String> = host
        .spaces()
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect();
    let schedules: Vec<Value> = scheduler
        .list(None, None)
        .into_iter()
        .map(|s| {
            let mut v = serde_json::to_value(&s).unwrap_or_default();
            v["plugin_name"] = json!(names.get(&s.plugin).cloned().unwrap_or(s.plugin.clone()));
            v["space_name"] = json!(spaces.get(&s.space));
            v["installed"] = json!(names.contains_key(&s.plugin));
            v
        })
        .collect();
    let schedule_names: BTreeMap<String, String> = scheduler
        .list(None, None)
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect();
    let runs: Vec<Value> = scheduler
        .recent_runs(60)
        .into_iter()
        .map(|r| {
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["schedule_name"] = json!(schedule_names.get(&r.schedule));
            v["plugin_name"] = json!(names.get(&r.plugin));
            v
        })
        .collect();
    json!({
        "settings": scheduler.settings(),
        "schedules": schedules,
        "runs": runs,
    })
}

// Page placements and saved views are synced rows. They name a renamed
// plugin by its storage id, so devices on older builds keep showing them;
// callers see the current id.

/// `plugin` (and a view's `config.plugin`) by current id, for callers.
fn present_view_config(host: &PluginHost, config: &mut Map<String, Value>) {
    if let Some(Value::String(plugin)) = config.get_mut("plugin") {
        *plugin = host.canonical(plugin).to_string();
    }
}

/// `plugin` (and a view's `config.plugin`) by storage id, for storing.
fn store_view_config(host: &PluginHost, config: &mut Map<String, Value>) {
    if let Some(Value::String(plugin)) = config.get_mut("plugin") {
        *plugin = host.storage_plugin_id(plugin).to_string();
    }
}

fn present_placement(host: &PluginHost, p: &mut Placement) {
    p.plugin = host.canonical(&p.plugin).to_string();
    if p.plugin == HOST_WIDGETS {
        present_view_config(host, &mut p.config);
    }
}

fn store_placement(host: &PluginHost, p: &mut Placement) {
    p.plugin = host.storage_plugin_id(&p.plugin).to_string();
    if p.plugin == HOST_WIDGETS {
        store_view_config(host, &mut p.config);
    }
}

fn present_view(host: &PluginHost, mut view: SavedView) -> SavedView {
    present_view_config(host, &mut view.config);
    view
}

fn page_state(host: &PluginHost, id: &str) -> Result<Value, String> {
    let pages = host.pages();
    let page = pages
        .page(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no page `{id}`"))?;
    let mut placements = pages.placements(id).map_err(|e| e.to_string())?;
    for p in &mut placements {
        present_placement(host, p);
    }
    Ok(json!({ "page": page, "placements": placements }))
}

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a Value, String> {
    args.get(key)
        .filter(|v| !v.is_null())
        .ok_or_else(|| format!("missing `{key}`"))
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    arg(args, key)?
        .as_str()
        .ok_or_else(|| format!("`{key}` must be text"))
}

fn parse_blob_id(args: &Value) -> Result<terrarium_blobs::BlobId, String> {
    let id = arg_str(args, "blob")?;
    id.parse()
        .map_err(|_| format!("`blob` is not a valid blob id: `{id}`"))
}

fn quota_error_text(quota_mb: u64) -> String {
    format!(
        "Attachments are full ({} GB). Remove some or raise the limit in Settings.",
        quota_mb / 1024
    )
}

fn quota_allows_blob(host: &PluginHost, current: u64, bytes: usize) -> Result<(), String> {
    let limit = host.blob_quota_bytes();
    if current.saturating_add(bytes as u64) > limit {
        return Err(quota_error_text(host.blob_quota_mb()));
    }
    Ok(())
}

fn attachment_settings(host: &PluginHost) -> Value {
    json!({
        "used_bytes": host.blob_usage_bytes(),
        "quota_mb": host.blob_quota_mb(),
        "quota_bytes": host.blob_quota_bytes(),
    })
}

fn blob_error(e: BlobError) -> String {
    match e {
        BlobError::TooLarge => format!("blob is larger than {MAX_BLOB} bytes"),
        BlobError::NoKey => "Attachments are available after this device joins a space.".into(),
        BlobError::Corrupt => "stored blob data is corrupt".into(),
        BlobError::NotFound => "blob not found".into(),
        BlobError::Io(message) => message,
    }
}

/// Answers `cmd` if it is one of these commands. `desktop`: whether this
/// device is a computer (for "run now").
pub fn invoke(
    host: &mut PluginHost,
    cmd: &str,
    args: &Value,
    desktop: bool,
) -> Option<Result<Value, String>> {
    let result = match cmd {
        "rules"
        | "rule_list"
        | "rule_runs"
        | "rule_create"
        | "rule_update"
        | "rule_set_enabled"
        | "rule_remove"
        | "rule_review"
        | "rule_approve"
        | "rules_save_settings"
        | "rules_start" => rule_service(host, cmd, args),
        "net_audit" | "net_audit_clear" => (|| {
            let plugin = arg_str(args, "plugin")?;
            let net = host
                .net_mut()
                .ok_or("Network activity is unavailable on this device.")?;
            serde_json::to_value(
                net.activity(plugin, cmd == "net_audit_clear")
                    .map_err(|e| e.message)?,
            )
            .map_err(|_| "Could not read network activity.".into())
        })(),
        "ai_settings" => Ok(ai_state(host)),
        "assistant_submit" => host.assistant_submit(args),
        "assistant_context" => (|| host.assistant_context(arg_str(args, "space")?))(),
        "assistant_reply" => {
            (|| host.assistant_reply(arg_str(args, "space")?, arg_str(args, "id")?))()
        }
        "blob.settings" => Ok(attachment_settings(host)),
        "blob.set_quota" => (|| {
            let quota_mb = arg(args, "quota_mb")?
                .as_u64()
                .ok_or("`quota_mb` must be a whole number")?;
            if !(512..=10_240).contains(&quota_mb) {
                return Err("`quota_mb` must be between 512 and 10240".into());
            }
            host.set_blob_quota_mb(quota_mb);
            Ok(attachment_settings(host))
        })(),
        "blob.cleanup" => {
            let report = host.blob_gc_collect_now(now_ms());
            Ok(json!({
                "report": report,
                "settings": attachment_settings(host),
            }))
        }
        "blob.put" => (|| {
            let name = arg_str(args, "name")?.to_string();
            let mime = arg_str(args, "mime")?.to_string();
            let data = arg_str(args, "data_base64")?;
            let bytes = B64
                .decode(data)
                .map_err(|_| "`data_base64` must be valid base64".to_string())?;
            if bytes.len() as u64 > MAX_BLOB {
                return Err(format!("blob exceeds {MAX_BLOB} bytes"));
            }
            let blobs = host.blob_store();
            let mut store = blobs
                .lock()
                .map_err(|_| "blob store is unavailable".to_string())?;
            let maybe_existing = store.id_of(&bytes).ok();
            if let Some(id) = &maybe_existing {
                if !store.has(id) {
                    quota_allows_blob(host, store.total_bytes(), bytes.len())?;
                }
            } else {
                quota_allows_blob(host, store.total_bytes(), bytes.len())?;
            }
            let blob = store.put(&bytes).map_err(blob_error)?;
            let value = FileRef {
                blob,
                name,
                mime,
                size: bytes.len() as u64,
            }
            .to_json_string()
            .map_err(|e| e.to_string())?;
            Ok(json!(value))
        })(),
        "blob.get" => (|| {
            let blob = parse_blob_id(args)?;
            let blobs = host.blob_store();
            let store = blobs
                .lock()
                .map_err(|_| "blob store is unavailable".to_string())?;
            match store.get(&blob) {
                Ok(bytes) => Ok(json!({
                    "mime": blob_mime(&bytes),
                    "data_base64": B64.encode(bytes),
                })),
                Err(BlobError::NotFound) => Ok(json!({"missing": true})),
                Err(e) => Err(blob_error(e)),
            }
        })(),
        "blob.info" => (|| {
            let blob = parse_blob_id(args)?;
            let blobs = host.blob_store();
            let store = blobs
                .lock()
                .map_err(|_| "blob store is unavailable".to_string())?;
            if !store.has(&blob) {
                return Ok(json!({"present": false, "size": Value::Null}));
            }
            let size = store.size(&blob).map_err(blob_error)?;
            Ok(json!({"present": true, "size": size}))
        })(),
        "ai_save_settings" => (|| {
            let config: AiConfig =
                serde_json::from_value(arg(args, "config")?.clone()).map_err(|e| e.to_string())?;
            host.ai_mut().set_config(config)?;
            Ok(ai_state(host))
        })(),
        "ai_set_key" => (|| {
            host.ai_mut().set_key(arg_str(args, "key")?)?;
            Ok(ai_state(host))
        })(),
        "ai_remove_key" => host.ai_mut().remove_key().map(|_| ai_state(host)),
        "schedules" => Ok(schedules_state(host)),
        "schedule_runs" => (|| {
            let id = arg_str(args, "id")?;
            Ok(json!(host.scheduler().runs(id, 50)))
        })(),
        "schedule_set_enabled" => (|| {
            let id = arg_str(args, "id")?;
            if let Some(schedule) = host.scheduler().get(id) {
                host.validate_schedule_target(&schedule.plugin)
                    .map_err(|e| e.to_string())?;
            }
            let enabled = arg(args, "enabled")?
                .as_bool()
                .ok_or("`enabled` must be true or false")?;
            host.scheduler_mut().set_enabled(id, enabled)?;
            Ok(schedules_state(host))
        })(),
        "schedule_set_repeat" => (|| {
            let id = arg_str(args, "id")?;
            if let Some(schedule) = host.scheduler().get(id) {
                host.validate_schedule_target(&schedule.plugin)
                    .map_err(|e| e.to_string())?;
            }
            let repeat: Repeat =
                serde_json::from_value(arg(args, "repeat")?.clone()).map_err(|e| e.to_string())?;
            host.scheduler_mut().set_repeat(id, &repeat)?;
            Ok(schedules_state(host))
        })(),
        "schedule_remove" => (|| {
            let id = arg_str(args, "id")?;
            if host
                .scheduler()
                .get(id)
                .is_some_and(|s| s.source == "manifest" && host.manifest(&s.plugin).is_some())
            {
                return Err("This schedule comes with its plugin; turn it off instead.".into());
            }
            host.scheduler_mut().remove(id)?;
            Ok(schedules_state(host))
        })(),
        "schedule_run_now" => (|| {
            let id = arg_str(args, "id")?;
            if let Some(schedule) = host.scheduler().get(id) {
                host.validate_schedule_target(&schedule.plugin)
                    .map_err(|e| e.to_string())?;
            }
            let run = host.scheduler_mut().run_now(id, now_ms())?;
            host.run_due(now_ms(), desktop);
            Ok(json!({ "run": run, "state": schedules_state(host) }))
        })(),
        "schedule_retry" => (|| {
            let run = arg_str(args, "run")?;
            if let Some(schedule) = host
                .scheduler()
                .run(run)
                .and_then(|r| host.scheduler().get(&r.schedule))
            {
                host.validate_schedule_target(&schedule.plugin)
                    .map_err(|e| e.to_string())?;
            }
            host.scheduler_mut().retry(run, now_ms())?;
            host.run_due(now_ms(), desktop);
            Ok(schedules_state(host))
        })(),
        "scheduler_save_settings" => (|| {
            let settings: Settings = serde_json::from_value(arg(args, "settings")?.clone())
                .map_err(|e| e.to_string())?;
            host.scheduler_mut().set_settings(&settings)?;
            Ok(schedules_state(host))
        })(),
        "page_widgets" => Ok(host_widgets()),
        "views_list" => (|| {
            let views = host
                .pages()
                .list_saved_views(arg_str(args, "space")?)
                .map_err(|e| e.to_string())?;
            Ok(json!(views
                .into_iter()
                .map(|v| present_view(host, v))
                .collect::<Vec<_>>()))
        })(),
        "view_get" => (|| {
            let id = arg_str(args, "id")?;
            host.pages()
                .get_saved_view(id)
                .map_err(|e| e.to_string())?
                .map(|view| Ok(json!(present_view(host, view))))
                .unwrap_or_else(|| Err(format!("no saved view `{id}`")))
        })(),
        "view_save" => (|| {
            let mut config = arg(args, "config")?
                .as_object()
                .cloned()
                .ok_or("`config` must be an object")?;
            store_view_config(host, &mut config);
            let id = args.get("id").and_then(Value::as_str);
            let view = host
                .pages_mut()
                .save_view(arg_str(args, "space")?, id, arg_str(args, "title")?, config)
                .map_err(|e| e.to_string())?;
            Ok(json!(present_view(host, view)))
        })(),
        "view_remove" => (|| {
            host.pages_mut()
                .remove_saved_view(arg_str(args, "id")?)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })(),
        "pages_list" => (|| {
            Ok(json!(host
                .pages()
                .list(arg_str(args, "space")?)
                .map_err(|e| e.to_string())?))
        })(),
        "folders_list" => (|| {
            let include_archived = match args.get("include_archived") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err("`include_archived` must be a boolean".into()),
            };
            Ok(json!(host
                .pages()
                .folders_list(arg_str(args, "space")?, include_archived)
                .map_err(|e| e.to_string())?))
        })(),
        "folder_create" => (|| {
            let parent_id = match args.get("parent_id") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_str().ok_or("`parent_id` must be text or null")?),
            };
            Ok(json!(host
                .pages_mut()
                .folder_create(arg_str(args, "space")?, arg_str(args, "title")?, parent_id)
                .map_err(|e| e.to_string())?))
        })(),
        "folder_update" => (|| {
            let parent_id = match args.get("parent_id") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(value) => Some(Some(
                    value.as_str().ok_or("`parent_id` must be text or null")?,
                )),
            };
            let position = match args.get("position") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_i64().ok_or("`position` must be an integer")?),
            };
            let title = match args.get("title") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_str().ok_or("`title` must be text")?),
            };
            Ok(json!(host
                .pages_mut()
                .folder_update(arg_str(args, "id")?, title, parent_id, position)
                .map_err(|e| e.to_string())?))
        })(),
        "folder_remove" => (|| {
            let subtree = match args.get("subtree") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Err("`subtree` must be a boolean".into()),
            };
            host.pages_mut()
                .folder_remove(arg_str(args, "id")?, subtree)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })(),
        "folder_restore" => (|| {
            host.pages_mut()
                .folder_restore(arg_str(args, "id")?)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })(),
        "page_move" => (|| {
            let folder_id = match args.get("folder_id") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(value) => Some(Some(
                    value.as_str().ok_or("`folder_id` must be text or null")?,
                )),
            };
            let position = match args.get("position") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_i64().ok_or("`position` must be an integer")?),
            };
            Ok(json!(host
                .pages_mut()
                .page_move(arg_str(args, "page")?, folder_id, position)
                .map_err(|e| e.to_string())?))
        })(),
        "page_trash" => (|| {
            host.pages_mut()
                .page_trash(arg_str(args, "page")?)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })(),
        "page_restore" => (|| {
            Ok(json!(host
                .pages_mut()
                .page_restore(arg_str(args, "page")?)
                .map_err(|e| e.to_string())?))
        })(),
        "pages_trash" => (|| {
            Ok(json!(host
                .pages()
                .pages_trash(arg_str(args, "space")?)
                .map_err(|e| e.to_string())?))
        })(),
        "page_layout" => (|| {
            Ok(json!(host
                .pages_mut()
                .page_layout(arg_str(args, "page")?, arg_str(args, "mode")?)
                .map_err(|e| e.to_string())?))
        })(),
        "page_get" => (|| page_state(host, arg_str(args, "page")?))(),
        "page_home" => (|| {
            let space = arg_str(args, "space")?;
            let mut seed: Vec<Placement> = match args.get("seed") {
                Some(v) if !v.is_null() => {
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?
                }
                _ => Vec::new(),
            };
            for p in &mut seed {
                store_placement(host, p);
            }
            let page = host
                .pages_mut()
                .ensure_home(space, seed)
                .map_err(|e| e.to_string())?;
            page_state(host, &page.id)
        })(),
        "page_create" => (|| {
            let icon = args.get("icon").and_then(Value::as_str);
            let period = args
                .get("period")
                .filter(|value| !value.is_null())
                .map(clean_period)
                .transpose()
                .map_err(|e| e.to_string())?;
            let mut page = host
                .pages_mut()
                .create_with_period(
                    arg_str(args, "space")?,
                    arg_str(args, "title")?,
                    icon,
                    period.as_ref(),
                )
                .map_err(|e| e.to_string())?;
            if let Some(focus) = args.get("focus").and_then(Value::as_str) {
                page = host
                    .pages_mut()
                    .update(&page.id, None, None, None, Some(focus))
                    .map_err(|e| e.to_string())?;
            }
            Ok(json!(page))
        })(),
        "page_update" => (|| {
            let icon = match args.get("icon") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(v) => Some(Some(v.as_str().ok_or("`icon` must be text")?)),
            };
            let period = args
                .get("period")
                .map(|value| {
                    if value.is_null() {
                        Ok(Value::Null)
                    } else {
                        clean_period(value).map_err(|e| e.to_string())
                    }
                })
                .transpose()?;
            let page = host
                .pages_mut()
                .update_with_period(
                    arg_str(args, "page")?,
                    args.get("title").and_then(Value::as_str),
                    icon,
                    args.get("position").and_then(Value::as_i64),
                    match args.get("focus") {
                        None | Some(Value::Null) => None,
                        Some(v) => Some(v.as_str().ok_or("`focus` must be text")?),
                    },
                    period.as_ref(),
                )
                .map_err(|e| e.to_string())?;
            Ok(json!(page))
        })(),
        "page_remove" => (|| {
            host.pages_mut()
                .remove(arg_str(args, "page")?)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })(),
        "page_save" => (|| {
            let page = arg_str(args, "page")?;
            let mut placements: Vec<Placement> =
                serde_json::from_value(arg(args, "placements")?.clone())
                    .map_err(|e| e.to_string())?;
            // Settings must match what the widget declares; a widget whose
            // plugin is not loaded keeps its settings for when it returns.
            for p in &mut placements {
                if p.plugin == HOST_WIDGETS {
                    p.config =
                        clean_host_config(&p.widget, &p.config).map_err(|e| e.to_string())?;
                } else if let Ok(template) = host.template(&p.plugin, ViewKind::Widget, &p.widget) {
                    p.config = clean_config(&template["page"]["config"], &p.config);
                }
                store_placement(host, p);
            }
            host.pages_mut()
                .save(page, placements)
                .map_err(|e| e.to_string())?;
            page_state(host, page)
        })(),
        _ => return None,
    };
    Some(result)
}

/// For "Test connection" and the model list: what a network call needs,
/// read under the host lock. `config` (unsaved form values) overrides the
/// saved settings; the key is always the saved one.
pub fn ai_probe(
    host: &PluginHost,
    config: Option<&Value>,
) -> Result<(AiConfig, Option<String>), String> {
    let saved = host.ai().config();
    let config = match config {
        Some(v) if !v.is_null() => serde_json::from_value::<AiConfig>(v.clone())
            .map_err(|e| e.to_string())?
            .validate()?,
        _ => saved,
    };
    Ok((config, host.ai().key()))
}

/// Runs the network side of `ai_test` / `ai_models`.
pub fn ai_network(cmd: &str, config: &AiConfig, key: Option<&str>) -> Result<Value, String> {
    match cmd {
        "ai_test" => {
            let started = std::time::Instant::now();
            let prompt = terrarium_ai::Prompt {
                system: "You are checking a connection. Answer with one short sentence.".into(),
                prompt: "Say hello.".into(),
                model: config.model.clone(),
                max_tokens: 40,
                temperature: 0.0,
            };
            terrarium_ai::complete(config, key, &prompt)
                .map(|c| {
                    json!({
                        "ok": true,
                        "model": c.model,
                        "text": c.text,
                        "ms": started.elapsed().as_millis() as u64,
                    })
                })
                .map_err(|e| e.message)
        }
        "ai_models" => terrarium_ai::models(config, key)
            .map(|m| json!(m))
            .map_err(|e| e.message),
        _ => Err(format!("unknown command `{cmd}`")),
    }
}

fn rule_service(host: &mut PluginHost, cmd: &str, args: &Value) -> Result<Value, String> {
    use crate::rules::{self, RuleInput};
    let allowed: &[&str] = match cmd {
        "rules" | "rule_list" | "rules_start" => &["space"],
        "rule_runs" => &["space", "rule", "limit", "before"],
        "rule_create" => &["space", "rule"],
        "rule_update" => &["space", "id", "rule"],
        "rule_set_enabled" => &["space", "id", "enabled"],
        "rule_approve" => &["space", "id", "digest"],
        "rules_save_settings" => &["space", "primary_device", "paused"],
        _ => &["space", "id"],
    };
    let object = args.as_object().ok_or("Arguments must be an object")?;
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("Unknown argument `{key}`"));
    }
    let space = arg_str(args, "space")?;
    if !host.open_spaces().any(|s| s == space) {
        return Err("Space is not open".into());
    }
    let error = |e: crate::RuntimeError| e.to_string();
    // Every id-bearing mutation is fenced to the caller's open space.
    let current = if allowed.contains(&"id") {
        let id = arg_str(args, "id")?;
        let rule = host.rules().get(id).map_err(error)?;
        if rule.as_ref().is_some_and(|r| r.space != space) {
            return Err("Rule is not in this space".into());
        }
        if rule.is_none() && cmd != "rule_remove" {
            return Err("Rule was not found".into());
        }
        rule.map(|r| host.rule_info(r)).transpose().map_err(error)?
    } else {
        None
    };
    match cmd {
        "rules" | "rule_list" => {
            let mut rules = Vec::new();
            let mut problems = Vec::<String>::new();
            if let Some(problem) = host.automation_problem() {
                problems.push(problem.to_owned());
            }
            for rule in host.rules().list(space).map_err(error)? {
                let rule = host.rule_info(rule).map_err(error)?;
                if let Some(problem) = &rule.problem {
                    problems.push(problem.clone());
                }
                rules.push(rule);
            }
            let waiting = rules.iter().map(|r| r.waiting).sum::<u64>();
            let mut rules = json!(rules);
            for rule in rules.as_array_mut().into_iter().flatten() {
                for side in ["trigger", "action"] {
                    if let Some(Value::String(plugin)) = rule[side].get_mut("plugin") {
                        *plugin = host.canonical(plugin).to_string();
                    }
                }
            }
            let mut state = json!({"rules":rules,"settings":host.rules().settings(space).map_err(error)?,"armed":host.automations_armed(space).map_err(error)?,"waiting":waiting,"problems":problems});
            if cmd == "rules" {
                let mut events = Vec::new();
                let mut operations = Vec::new();
                for m in host.manifests() {
                    for (name, event) in &m.events {
                        events.push(json!({"plugin":m.id,"event":name,"description":event.description,"payload":event.payload}));
                    }
                    for (name, op) in &m.operations {
                        if rules::eligible(name, op) {
                            operations.push(json!({"plugin":m.id,"operation":name,"description":op.description,"params":op.params}));
                        }
                    }
                }
                state["events"] = json!(events);
                state["operations"] = json!(operations);
                state["examples"] = if events
                    .iter()
                    .any(|e| e["plugin"] == "tasks" && e["event"] == "task_done")
                    && operations
                        .iter()
                        .any(|o| o["plugin"] == "habits" && o["operation"] == "checkin")
                {
                    json!([{
                        "name": "Gym task done → Gym habit",
                        "trigger": {"plugin": "tasks", "event": "task_done"},
                        "filter": [{"field": "tags", "op": "has_tag", "value": "gym"}],
                        "action": {"plugin": "habits", "operation": "checkin",
                            "args": {"id": "", "status": "done", "day": "{{event.completed_date}}"}}
                    }])
                } else {
                    json!([])
                };
            }
            Ok(state)
        }
        "rule_create" | "rule_update" => {
            let mut input: RuleInput = serde_json::from_value(arg(args, "rule")?.clone())
                .map_err(|_| "Invalid rule fields")?;
            // Rules are synced rows: name renamed plugins by storage id.
            input.trigger.plugin = host.storage_plugin_id(&input.trigger.plugin).to_string();
            input.action.plugin = host.storage_plugin_id(&input.action.plugin).to_string();
            rules::check(host, &input)?;
            let rule = if cmd == "rule_create" {
                host.rules_mut().create(space, &input)
            } else {
                host.rules_mut()
                    .update(&current.expect("checked").id, &input)
            }
            .map_err(error)?;
            Ok(json!(host.rule_info(rule).map_err(error)?))
        }
        "rule_set_enabled" => {
            let enabled = arg(args, "enabled")?
                .as_bool()
                .ok_or("Enabled must be true or false")?;
            let current = current.expect("checked");
            if enabled {
                rules::check(host, &current.input())?;
            }
            let rule = host
                .rules_mut()
                .set_enabled(&current.id, enabled)
                .map_err(error)?;
            let rule = host.rule_info(rule).map_err(error)?;
            host.grant_rule(&rule).map_err(error)?;
            Ok(json!(rule))
        }
        "rule_remove" => {
            host.rules_mut()
                .remove(arg_str(args, "id")?)
                .map_err(error)?;
            Ok(json!({"ok":true}))
        }
        "rule_review" => {
            let rule = current.expect("checked");
            if let Some(problem) = &rule.problem {
                return Err(problem.clone());
            }
            let mut passed = Vec::new();
            let mut fixed = serde_json::Map::new();
            for (name, value) in &rule.action.args {
                if let Some(field) = value
                    .as_str()
                    .and_then(|s| s.strip_prefix("{{event."))
                    .and_then(|s| s.strip_suffix("}}"))
                {
                    passed.push(field.to_owned());
                } else {
                    fixed.insert(name.clone(), value.clone());
                }
            }
            passed.sort();
            passed.dedup();
            Ok(json!({"digest":rules::digest(&rule),"consent":{
                "summary":format!("When {} emits {}, run {}.{}",rule.trigger.plugin,rule.trigger.event,rule.action.plugin,rule.action.operation),
                "passed_fields":passed,"condition_fields":rule.filter.iter().map(|c|c.field.clone()).collect::<Vec<_>>(),
                "conditions":rule.filter,"fixed_args":fixed,"device":host.rules().settings(space).map_err(error)?.primary_device,
                "warnings":["Only new events will run", "Events wait up to seven days", "Changing devices can cause duplicate actions"]
            }}))
        }
        "rule_approve" => {
            let rule = current.expect("checked");
            host.rules_mut()
                .approve(&rule, arg_str(args, "digest")?)
                .map_err(error)?;
            let rule = host.rule_info(rule).map_err(error)?;
            host.grant_rule(&rule).map_err(error)?;
            Ok(json!(rule))
        }
        "rules_save_settings" => {
            let device = match object.get("primary_device") {
                Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err("Primary device must be text or null".into()),
            };
            let paused = arg(args, "paused")?
                .as_bool()
                .ok_or("Paused must be true or false")?;
            host.rules_mut()
                .save_settings(
                    space,
                    &rules::Settings {
                        primary_device: device.map(str::to_owned),
                        paused,
                    },
                )
                .map_err(error)?;
            let mut result = json!(host.rules().settings(space).map_err(error)?);
            result["armed"] = json!(host.arm_automations(space).map_err(error)?);
            Ok(result)
        }
        "rules_start" => {
            let mut result = json!(host.rules().settings(space).map_err(error)?);
            result["armed"] = json!(host.arm_automations(space).map_err(error)?);
            Ok(result)
        }
        "rule_runs" => {
            let rule = match object.get("rule") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err("Rule must be an id".into()),
            };
            let before = match object.get("before") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err("Invalid history cursor".into()),
            };
            let limit = match object.get("limit") {
                None | Some(Value::Null) => 20,
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=50).contains(n))
                    .ok_or("Run limit must be 1–50")? as usize,
            };
            host.rules().runs(space, rule, limit, before).map_err(error)
        }
        _ => unreachable!(),
    }
}
