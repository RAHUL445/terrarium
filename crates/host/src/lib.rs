//! Services shared by the Terrarium app and its headless mode
//! (`terrarium serve`): the plugin host and how plugins are chosen and
//! reloaded, scheduled runs and reminders, the command-line control socket,
//! and the lock that keeps one process at a time writing a data folder.
//!
//! Nothing here knows about a window. What the services need from whatever
//! runs them (send an event to the UI, show a notification, show something
//! in the window) goes through [`Shell`].

pub mod control;
pub mod jobs;
pub mod lock;
pub mod plugins;
pub mod private;

use serde_json::Value;

pub use control::Control;
pub use plugins::Plugins;

/// True on computers, false on phones.
pub const DESKTOP: bool = !cfg!(any(target_os = "android", target_os = "ios"));

/// The app events the UI and control clients follow.
pub const EVENTS: [&str; 3] = ["data-changed", "spaces-changed", "plugins-changed"];

/// What the services need from the process running them: the app window,
/// or nothing in headless mode.
pub trait Shell: Send + Sync + 'static {
    /// An app event, such as `data-changed`.
    fn emit(&self, event: &str, payload: Value);

    /// A system notification. `id` is stable for a reminder, so showing it
    /// again replaces it.
    fn notify(&self, id: Option<i32>, title: &str, body: &str);

    /// Shows something in the app window (`ui.open`, `ui.notify`).
    fn show(&self, event: Value) -> Result<(), String>;

    /// Whether there is an app window (false in headless mode).
    fn has_window(&self) -> bool {
        true
    }

    /// Hands reminders to the system. By default shows those due now.
    fn deliver_reminders(
        &self,
        reminders: &[jobs::Reminder],
        state: &mut jobs::State,
        now_ms: u64,
    ) {
        jobs::show_due(self, reminders, state, now_ms);
    }
}

/// Static phase labels and elapsed time only. Opt-in even in release builds.
pub fn startup_span(phase: &'static str, start: std::time::Instant) {
    if cfg!(feature = "startup-profiling") {
        eprintln!(
            "[terrarium-startup] {}",
            serde_json::json!({"phase": phase, "duration_ms": start.elapsed().as_secs_f64() * 1000.0})
        );
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// IANA name of the system timezone: the `persist.sys.timezone` property
/// on Android (which has no `/etc/localtime`), else the `/etc/localtime`
/// link on macOS and Linux; `UTC` if neither can be read.
pub fn system_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if cfg!(target_os = "android") {
        let prop = std::process::Command::new("/system/bin/getprop")
            .arg("persist.sys.timezone")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|tz| !tz.is_empty());
        if let Some(tz) = prop {
            return tz;
        }
    }
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|target| {
            let target = target.to_string_lossy().into_owned();
            target.split("zoneinfo/").nth(1).map(str::to_string)
        })
        .unwrap_or_else(|| "UTC".into())
}
