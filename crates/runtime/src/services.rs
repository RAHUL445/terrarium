//! The app's commands for Settings → AI and Settings → Scheduled, shared by
//! every host (the Tauri app, `terra preview`). They answer from the host's
//! own state; network calls (testing the provider, listing models) are left
//! to the host so it can make them without holding the plugin host.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use terrarium_ai::{AiConfig, PRESETS};
use terrarium_scheduler::{Repeat, Settings};

use crate::pages::{clean_config, clean_host_config, host_widgets, Placement, HOST_WIDGETS};
use crate::registry::{PluginHost, ViewKind};

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
        .map(|m| (m.id.clone(), m.name.clone()))
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

fn page_state(host: &PluginHost, id: &str) -> Result<Value, String> {
    let pages = host.pages();
    let page = pages
        .page(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no page `{id}`"))?;
    Ok(json!({ "page": page, "placements": pages.placements(id).map_err(|e| e.to_string())? }))
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

/// Answers `cmd` if it is one of these commands. `desktop`: whether this
/// device is a computer (for "run now").
pub fn invoke(
    host: &mut PluginHost,
    cmd: &str,
    args: &Value,
    desktop: bool,
) -> Option<Result<Value, String>> {
    let result = match cmd {
        "ai_settings" => Ok(ai_state(host)),
        "assistant_submit" => host.assistant_submit(args),
        "assistant_context" => (|| host.assistant_context(arg_str(args, "space")?))(),
        "assistant_reply" => {
            (|| host.assistant_reply(arg_str(args, "space")?, arg_str(args, "id")?))()
        }
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
            let enabled = arg(args, "enabled")?
                .as_bool()
                .ok_or("`enabled` must be true or false")?;
            host.scheduler_mut().set_enabled(id, enabled)?;
            Ok(schedules_state(host))
        })(),
        "schedule_set_repeat" => (|| {
            let id = arg_str(args, "id")?;
            let repeat: Repeat =
                serde_json::from_value(arg(args, "repeat")?.clone()).map_err(|e| e.to_string())?;
            host.scheduler_mut().set_repeat(id, &repeat)?;
            Ok(schedules_state(host))
        })(),
        "schedule_remove" => (|| {
            let id = arg_str(args, "id")?;
            if host.scheduler().get(id).is_some_and(|s| {
                s.source == "manifest" && host.manifests().any(|m| m.id == s.plugin)
            }) {
                return Err("This schedule comes with its plugin; turn it off instead.".into());
            }
            host.scheduler_mut().remove(id)?;
            Ok(schedules_state(host))
        })(),
        "schedule_run_now" => (|| {
            let id = arg_str(args, "id")?;
            let run = host.scheduler_mut().run_now(id, now_ms())?;
            host.run_due(now_ms(), desktop);
            Ok(json!({ "run": run, "state": schedules_state(host) }))
        })(),
        "schedule_retry" => (|| {
            let run = arg_str(args, "run")?;
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
        "pages_list" => (|| {
            Ok(json!(host
                .pages()
                .list(arg_str(args, "space")?)
                .map_err(|e| e.to_string())?))
        })(),
        "page_get" => (|| page_state(host, arg_str(args, "page")?))(),
        "page_home" => (|| {
            let space = arg_str(args, "space")?;
            let seed: Vec<Placement> = match args.get("seed") {
                Some(v) if !v.is_null() => {
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?
                }
                _ => Vec::new(),
            };
            let page = host
                .pages_mut()
                .ensure_home(space, seed)
                .map_err(|e| e.to_string())?;
            page_state(host, &page.id)
        })(),
        "page_create" => (|| {
            let icon = args.get("icon").and_then(Value::as_str);
            let mut page = host
                .pages_mut()
                .create(arg_str(args, "space")?, arg_str(args, "title")?, icon)
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
            let page = host
                .pages_mut()
                .update(
                    arg_str(args, "page")?,
                    args.get("title").and_then(Value::as_str),
                    icon,
                    args.get("position").and_then(Value::as_i64),
                    match args.get("focus") {
                        None | Some(Value::Null) => None,
                        Some(v) => Some(v.as_str().ok_or("`focus` must be text")?),
                    },
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
