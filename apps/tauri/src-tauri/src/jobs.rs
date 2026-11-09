//! Scheduled plugin jobs and reminder notifications on this device.
//!
//! Every 30 s this thread:
//! - runs the scheduled runs due on this device (the scheduler in the
//!   runtime decides which; `desktop` schedules never run on a phone);
//! - reads every plugin's reminders query and turns rows meant for this
//!   kind of device (`notify_on`: `phone` by default, `desktop`, `both`)
//!   into system notifications. On Android they are scheduled ahead with
//!   exact alarms, so they fire even when the app is closed; on desktop
//!   they are shown when due while the app runs.
//!
//! State (notifications already handled) is kept in
//! `<space>/jobs-state.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use terrarium_runtime::{store_id, DEFAULT_SPACE};

use crate::plugins::Plugins;

const TICK: Duration = Duration::from_secs(30);
/// Desktop shows a reminder only if it fell due this recently.
const LATE_LIMIT_MS: u64 = 15 * 60_000;

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Reminder keys (`plugin/id@at_ms`) already shown or scheduled.
    handled: BTreeSet<String>,
    /// Android: notification id -> reminder key, to cancel removed ones.
    #[serde(default)]
    scheduled: BTreeMap<i32, String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Stable notification id from the reminder's plugin and row id.
fn notification_id(key: &str) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash & 0x7fff_ffff) as i32
}

pub fn start(app: AppHandle, state_path: PathBuf) {
    std::thread::spawn(move || {
        let mut state: State = fs::read(&state_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Android drops an app's alarms when it is updated, force-stopped
        // or the phone restarts. Schedule every future reminder again;
        // the same id replaces an alarm that did survive.
        if cfg!(mobile) {
            state
                .handled
                .retain(|key| !state.scheduled.values().any(|k| k == key));
        }
        loop {
            std::thread::sleep(TICK);
            let plugins = app.state::<Plugins>();
            let ran = run_due_jobs(&app, &plugins);
            notify(&app, &plugins, &mut state);
            if ran {
                let _ = app.emit("data-changed", ());
            }
            if let Ok(bytes) = serde_json::to_vec(&state) {
                let _ = fs::write(&state_path, bytes);
            }
        }
    });
}

fn run_due_jobs(app: &AppHandle, plugins: &Plugins) -> bool {
    let ended = plugins
        .with_host(|host| host.run_due(now_ms(), cfg!(desktop)))
        .unwrap_or_default();
    for done in &ended {
        eprintln!("run {} {}", done.run.id, done.run.status);
        crate::ai::notify_ended(app, done);
    }
    !ended.is_empty()
}

struct Reminder {
    key: String,
    at_ms: u64,
    title: String,
    body: String,
}

fn for_this_device(row: &serde_json::Map<String, Value>) -> bool {
    let target = row
        .get("notify_on")
        .and_then(Value::as_str)
        .unwrap_or("phone");
    let this = if cfg!(mobile) { "phone" } else { "desktop" };
    match target {
        "both" => true,
        "desktop" | "phone" => target == this,
        // Unknown targets fall back to the default: phones.
        _ => this == "phone",
    }
}

fn notify(app: &AppHandle, plugins: &Plugins, state: &mut State) {
    let now = now_ms();
    let (rows, names) = plugins
        .with_host(|host| {
            let names: BTreeMap<String, String> = host
                .spaces()
                .list()
                .unwrap_or_default()
                .into_iter()
                .map(|s| (s.id, s.name))
                .collect();
            (host.reminders(now.saturating_sub(LATE_LIMIT_MS)), names)
        })
        .unwrap_or_default();
    let reminders: Vec<Reminder> = rows
        .into_iter()
        .filter(|(_, _, row)| for_this_device(row))
        .filter_map(|(space, plugin, row)| {
            let id = row.get("id")?.as_str()?.to_string();
            let at_ms = row.get("at_ms")?.as_u64()?;
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Reminder");
            Some(Reminder {
                key: format!("{}/{id}@{at_ms}", store_id(&space, &plugin)),
                at_ms,
                // Say which space a reminder is from, except the default one.
                title: match names.get(&space) {
                    Some(name) if space != DEFAULT_SPACE => format!("{name} · {title}"),
                    _ => title.to_string(),
                },
                body: row
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();

    #[cfg(mobile)]
    {
        use tauri_plugin_notification::Schedule;
        let wanted: BTreeSet<&str> = reminders.iter().map(|r| r.key.as_str()).collect();
        // Cancel alarms whose reminder disappeared (done, deleted, moved).
        let stale: Vec<i32> = state
            .scheduled
            .iter()
            .filter(|(_, key)| !wanted.contains(key.as_str()))
            .map(|(id, _)| *id)
            .collect();
        if !stale.is_empty() {
            let _ = app.notification().cancel(stale.clone());
            for id in stale {
                if let Some(key) = state.scheduled.remove(&id) {
                    state.handled.remove(&key);
                }
            }
        }
        for reminder in &reminders {
            if state.handled.contains(&reminder.key) || reminder.at_ms <= now {
                continue;
            }
            let Ok(at) = time::OffsetDateTime::from_unix_timestamp((reminder.at_ms / 1000) as i64)
            else {
                continue;
            };
            let id = notification_id(&reminder.key);
            let result = app
                .notification()
                .builder()
                .id(id)
                .title(&reminder.title)
                .body(&reminder.body)
                .schedule(Schedule::At {
                    date: at,
                    repeating: false,
                    allow_while_idle: true,
                })
                .show();
            match result {
                Ok(()) => {
                    state.handled.insert(reminder.key.clone());
                    state.scheduled.insert(id, reminder.key.clone());
                }
                Err(e) => eprintln!("could not schedule reminder: {e}"),
            }
        }
    }

    #[cfg(desktop)]
    for reminder in &reminders {
        if state.handled.contains(&reminder.key)
            || reminder.at_ms > now
            || reminder.at_ms + LATE_LIMIT_MS < now
        {
            continue;
        }
        let shown = app
            .notification()
            .builder()
            .id(notification_id(&reminder.key))
            .title(&reminder.title)
            .body(&reminder.body)
            .show();
        if let Err(e) = shown {
            eprintln!("could not show reminder: {e}");
        }
        state.handled.insert(reminder.key.clone());
    }

    // Forget handled keys older than a day so the state stays small.
    let horizon = now.saturating_sub(24 * 3_600_000);
    state.handled.retain(|key| {
        key.rsplit('@')
            .next()
            .and_then(|t| t.parse::<u64>().ok())
            .is_none_or(|at| at >= horizon)
    });
}
