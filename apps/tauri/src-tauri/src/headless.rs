//! Headless mode (`terrarium serve`, which runs this binary with
//! `--headless`): the plugin host, scheduled runs, reminders and the
//! command-line socket, without a window.
//!
//! It holds the data-folder lock, so it never runs alongside the app. When
//! the app opens it asks serve to hand over; serve waits for any command in
//! progress to finish and exits, and the app carries on. Sync, AI requests
//! and anything that needs the window wait until the app is open.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use terrarium_host::lock::{self, LockError, Role};
use terrarium_host::{Control, Shell, EVENTS};

use crate::Opened;

/// No window: events go only to subscribed clients, and notifications to
/// the system's notification centre.
struct Headless {
    control: Control,
}

impl Shell for Headless {
    fn emit(&self, event: &str, _payload: Value) {
        if EVENTS.contains(&event) {
            self.control.broadcast(event);
        }
    }

    fn notify(&self, _id: Option<i32>, title: &str, body: &str) {
        eprintln!("notification: {title}: {body}");
        if cfg!(target_os = "macos") {
            // Title and body are passed as arguments, never as script text.
            let shown = std::process::Command::new("/usr/bin/osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    "display notification (item 2 of argv) with title (item 1 of argv)",
                    "-e",
                    "end run",
                    title,
                    body,
                ])
                .output();
            if let Err(e) = shown {
                eprintln!("could not show notification: {e}");
            }
        }
    }

    fn show(&self, _event: Value) -> Result<(), String> {
        Err("terrarium serve has no window. Open the Terrarium app to show this.".into())
    }

    fn has_window(&self) -> bool {
        false
    }
}

fn app_data() -> Result<PathBuf, String> {
    match std::env::var_os("TERRA_PROFILE_DIR").map(PathBuf::from) {
        Some(path) if !path.is_absolute() => {
            Err("TERRA_PROFILE_DIR must be an absolute path".into())
        }
        Some(path) => Ok(path),
        None => terrarium_control_proto::app_data_dir()
            .ok_or_else(|| "Can't find the app's data folder (HOME is not set).".into()),
    }
}

/// Runs until the app takes over or the process is stopped. Returns the
/// exit code: 0 after handing over, 1 if it could not start.
pub fn run() -> i32 {
    match serve() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("terrarium serve: {e}");
            1
        }
    }
}

fn serve() -> Result<(), String> {
    let app_data = app_data()?;
    std::fs::create_dir_all(&app_data).map_err(|e| e.to_string())?;
    let _lock = lock::acquire(&app_data, Role::Serve).map_err(|e| match e {
        LockError::Busy(Some(Role::App)) => {
            "Terrarium is open, and it already answers the command line.".to_string()
        }
        other => other.to_string(),
    })?;
    let control = Control::open(&app_data);
    if !control.enabled() {
        return Err("command-line control is off. Turn it on in Terrarium: \
             Settings → Developer → Command-line control."
            .into());
    }
    let Opened {
        data_dir, plugins, ..
    } = crate::open_host(&app_data).map_err(|e| e.to_string())?;
    let shell: Arc<dyn Shell> = Arc::new(Headless {
        control: control.clone(),
    });
    // Watched from the start, so the app can take over even while plugins
    // are still loading.
    {
        let (app_data, plugins, control) = (app_data.clone(), plugins.clone(), control.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            if lock::handoff_requested(&app_data) {
                eprintln!("terrarium serve: the app is opening; handing over");
                // Exit holding the plugin host, so no command is cut off midway.
                plugins.with_host(|_| {
                    let _ = std::fs::remove_file(control.socket_path());
                    std::process::exit(0)
                });
                std::process::exit(0);
            }
        });
    }
    plugins.start(shell.clone());
    terrarium_host::jobs::start(
        plugins.clone(),
        shell.clone(),
        data_dir.join("jobs-state.json"),
    );
    plugins.wait_ready(Duration::from_secs(60));
    control.apply(&plugins, &shell);
    let state = control.state();
    if state["listening"] != true {
        let why = state["error"]
            .as_str()
            .unwrap_or("the socket did not start");
        return Err(why.to_string());
    }
    eprintln!(
        "terrarium serve: answering on {} (stops when the app opens; Ctrl-C to stop now)",
        control.socket_path().display()
    );
    loop {
        std::thread::park();
    }
}
