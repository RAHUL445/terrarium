//! Scheduled plugin jobs and reminder notifications on this device.
//!
//! Every 30 s this thread:
//! - runs the scheduled runs due on this device (the scheduler in the
//!   runtime decides which; `desktop` schedules never run on a phone);
//! - reads every plugin's reminders query and hands rows meant for this
//!   kind of device (`notify_on`: `phone` by default, `desktop`, `both`) to
//!   the [`Shell`]. On a computer they are shown when due; the Android app
//!   schedules them ahead with exact alarms instead.
//!
//! State (notifications already handled) is kept in
//! `<space>/jobs-state.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use terrarium_runtime::scheduler::Finished;
use terrarium_runtime::{store_id, DEFAULT_SPACE};

use crate::{now_ms, Plugins, Shell, DESKTOP};

const TICK: Duration = Duration::from_secs(30);
/// A computer shows a reminder only if it fell due this recently.
pub const LATE_LIMIT_MS: u64 = 15 * 60_000;

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    /// Reminder keys (`plugin/id@at_ms`) already shown or scheduled.
    pub handled: BTreeSet<String>,
    /// Android: notification id -> reminder key, to cancel removed ones.
    #[serde(default)]
    pub scheduled: BTreeMap<i32, String>,
}

pub struct Reminder {
    pub key: String,
    pub at_ms: u64,
    pub title: String,
    pub body: String,
}

/// Stable notification id from the reminder's plugin and row id.
pub fn notification_id(key: &str) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash & 0x7fff_ffff) as i32
}

pub fn start(plugins: Plugins, shell: Arc<dyn Shell>, state_path: PathBuf) {
    std::thread::spawn(move || {
        let mut state: State = std::fs::read(&state_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Android drops an app's alarms when it is updated, force-stopped
        // or the phone restarts. Schedule every future reminder again;
        // the same id replaces an alarm that did survive.
        if !DESKTOP {
            state
                .handled
                .retain(|key| !state.scheduled.values().any(|k| k == key));
        }
        loop {
            std::thread::sleep(TICK);
            let ran = run_due_jobs(&plugins, &*shell);
            let now = now_ms();
            let reminders = reminders(&plugins, now);
            shell.deliver_reminders(&reminders, &mut state, now);
            forget_old(&mut state, now);
            if ran {
                shell.emit("data-changed", json!(null));
            }
            if let Ok(bytes) = serde_json::to_vec(&state) {
                let _ = crate::private::write_private(&state_path, &bytes);
            }
        }
    });
}

/// Tells the user how a scheduled run ended: a failure, or the notice the
/// run left ("Report ready").
pub fn notify_ended(shell: &dyn Shell, done: &Finished) {
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
    shell.notify(None, &title, &body);
}

fn run_due_jobs(plugins: &Plugins, shell: &dyn Shell) -> bool {
    let ended = plugins
        .with_host(|host| host.run_due(now_ms(), DESKTOP))
        .unwrap_or_default();
    for done in &ended {
        eprintln!("run {} {}", done.run.id, done.run.status);
        notify_ended(shell, done);
    }
    !ended.is_empty()
}

fn for_this_device(row: &serde_json::Map<String, Value>) -> bool {
    let target = row
        .get("notify_on")
        .and_then(Value::as_str)
        .unwrap_or("phone");
    let this = if DESKTOP { "desktop" } else { "phone" };
    match target {
        "both" => true,
        "desktop" | "phone" => target == this,
        // Unknown targets fall back to the default: phones.
        _ => this == "phone",
    }
}

/// Reminders meant for this kind of device that are due from
/// [`LATE_LIMIT_MS`] ago on.
fn reminders(plugins: &Plugins, now: u64) -> Vec<Reminder> {
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
    rows.into_iter()
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
        .collect()
}

/// Shows the reminders that are due now and not shown yet.
pub fn show_due<S: Shell + ?Sized>(shell: &S, reminders: &[Reminder], state: &mut State, now: u64) {
    for reminder in reminders {
        if state.handled.contains(&reminder.key)
            || reminder.at_ms > now
            || reminder.at_ms + LATE_LIMIT_MS < now
        {
            continue;
        }
        shell.notify(
            Some(notification_id(&reminder.key)),
            &reminder.title,
            &reminder.body,
        );
        state.handled.insert(reminder.key.clone());
    }
}

/// Forgets handled keys older than a day so the state stays small.
fn forget_old(state: &mut State, now: u64) {
    let horizon = now.saturating_sub(24 * 3_600_000);
    state.handled.retain(|key| {
        key.rsplit('@')
            .next()
            .and_then(|t| t.parse::<u64>().ok())
            .is_none_or(|at| at >= horizon)
    });
}
