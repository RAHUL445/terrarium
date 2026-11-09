//! Host services on this device: AI requests and Settings → AI /
//! Settings → Scheduled commands.
//!
//! AI requests run on threads started here, never while holding the plugin
//! host: a job is taken under the lock, the HTTP call runs without it, and
//! the reply is delivered under the lock again. The key that seals the API
//! key follows this device's space membership.

use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use terrarium_runtime::ai::Sealer;
use terrarium_runtime::scheduler::Finished;
use terrarium_runtime::services;

use crate::plugins::{err, Plugins};
use crate::sync::Sync;

const TICK: Duration = Duration::from_millis(250);
/// How often the sealing root is checked (joining a space changes it).
const SEALER_EVERY: u32 = 20;

/// Tells the user how a scheduled run ended: a failure, or the notice the
/// run left ("Report ready").
pub fn notify_ended(app: &AppHandle, done: &Finished) {
    let (title, body) = if done.notify {
        (
            format!("{} failed", done.schedule_name),
            "Open Settings → Scheduled to review the error and retry.".to_string(),
        )
    } else if let Some(notice) = &done.notice {
        (done.schedule_name.clone(), notice.clone())
    } else {
        return;
    };
    let shown = app.notification().builder().title(title).body(body).show();
    if let Err(e) = shown {
        eprintln!("could not show run notice: {e}");
    }
}

fn update_sealer(app: &AppHandle, current: &mut Option<String>) {
    let Ok((kind, root)) = app.state::<Sync>().secrets_root() else {
        return;
    };
    let sealer = Sealer::new(kind, &root);
    if current.as_deref() == Some(sealer.id()) {
        return;
    }
    let id = sealer.id().to_string();
    if app
        .state::<Plugins>()
        .with_host(|host| host.set_ai_sealer(sealer))
        .is_some()
    {
        *current = Some(id);
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut sealer = None;
        let mut tick = 0u32;
        loop {
            if tick.is_multiple_of(SEALER_EVERY) {
                update_sealer(&app, &mut sealer);
            }
            tick = tick.wrapping_add(1);
            while let Some((job, config, key)) = app
                .state::<Plugins>()
                .with_host(|host| host.take_ai_job())
                .flatten()
            {
                let app = app.clone();
                std::thread::spawn(move || {
                    let result =
                        terrarium_runtime::ai::complete(&config, key.as_deref(), &job.prompt);
                    if let Err(e) = &result {
                        eprintln!("AI request for {} failed: {}", job.plugin, e.message);
                    }
                    let ended = app
                        .state::<Plugins>()
                        .with_host(|host| host.deliver_ai(job, result))
                        .flatten();
                    if let Some(done) = ended {
                        notify_ended(&app, &done);
                    }
                    let _ = app.emit("data-changed", ());
                });
            }
            std::thread::sleep(TICK);
        }
    });
}

/// Settings → AI and Settings → Scheduled commands answered from host state.
#[tauri::command]
pub fn service(
    app: AppHandle,
    state: State<'_, Plugins>,
    name: String,
    args: Option<Value>,
) -> Result<Value, String> {
    let args = args.unwrap_or(Value::Null);
    let result = state
        .with_host(|host| services::invoke(host, &name, &args, cfg!(desktop)))
        .ok_or("plugin host unavailable")?
        .ok_or_else(|| format!("unknown command `{name}`"))?;
    if result.is_ok() && name.starts_with("schedule") {
        let _ = app.emit("data-changed", ());
    }
    result
}

/// "Test connection" and the model list: the network call runs off the
/// plugin host. `config` is the unsaved form, if any.
#[tauri::command]
pub async fn service_net(
    app: AppHandle,
    name: String,
    config: Option<Value>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (config, key) = app
            .state::<Plugins>()
            .with_host(|host| services::ai_probe(host, config.as_ref()))
            .ok_or("plugin host unavailable")??;
        services::ai_network(&name, &config, key.as_deref())
    })
    .await
    .map_err(err)?
}
