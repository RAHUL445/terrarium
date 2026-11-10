//! The app window as the [`Shell`] the shared services report to.

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use terrarium_host::jobs::{Reminder, State};
use terrarium_host::Shell;

pub struct AppShell(pub AppHandle);

impl AppShell {
    pub fn arc(app: &AppHandle) -> Arc<dyn Shell> {
        Arc::new(AppShell(app.clone()))
    }
}

impl Shell for AppShell {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }

    fn notify(&self, id: Option<i32>, title: &str, body: &str) {
        let mut builder = self.0.notification().builder().title(title).body(body);
        if let Some(id) = id {
            builder = builder.id(id);
        }
        if let Err(e) = builder.show() {
            eprintln!("could not show notification: {e}");
        }
    }

    /// The UI shows it (event `control-ui`).
    fn show(&self, event: Value) -> Result<(), String> {
        self.0.emit("control-ui", event).map_err(|e| e.to_string())
    }

    /// On a phone reminders are scheduled ahead with exact alarms, so they
    /// fire even when the app is closed.
    #[cfg(mobile)]
    fn deliver_reminders(&self, reminders: &[Reminder], state: &mut State, now: u64) {
        use std::collections::BTreeSet;
        use tauri_plugin_notification::Schedule;
        use terrarium_host::jobs::notification_id;

        let app = &self.0;
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
        for reminder in reminders {
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
    fn deliver_reminders(&self, reminders: &[Reminder], state: &mut State, now: u64) {
        terrarium_host::jobs::show_due(self, reminders, state, now);
    }
}
