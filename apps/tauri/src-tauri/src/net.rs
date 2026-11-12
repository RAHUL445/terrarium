//! Foreground-only HTTP workers. Network I/O never owns the plugin-host lock.
use crate::plugins::Plugins;
use crate::shell::AppShell;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(mobile)]
static FOREGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
#[cfg(mobile)]
pub fn lifecycle(event: &tauri::WindowEvent) {
    use std::sync::atomic::Ordering;
    match event {
        tauri::WindowEvent::Suspended => FOREGROUND.store(false, Ordering::SeqCst),
        tauri::WindowEvent::Resumed => FOREGROUND.store(true, Ordering::SeqCst),
        _ => {}
    }
}
fn foreground() -> bool {
    #[cfg(mobile)]
    {
        FOREGROUND.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(desktop)]
    {
        true
    }
}
pub fn start(app: AppHandle) {
    if app
        .state::<Plugins>()
        .with_host(|host| host.enable_http())
        .is_none_or(|r| r.is_err())
    {
        return;
    }
    std::thread::spawn(move || loop {
        // Mobile lifecycle visibility gates *new* requests; an already sent
        // request cannot be recalled. No background worker is registered.
        if foreground() {
            while let Some((job, auth)) = app
                .state::<Plugins>()
                .with_host(|h| h.take_http_job())
                .flatten()
            {
                let app = app.clone();
                std::thread::spawn(move || {
                    while !foreground() {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    let current = app
                        .state::<Plugins>()
                        .with_host(|h| h.http_job_current(&job))
                        .unwrap_or(false);
                    if !current {
                        app.state::<Plugins>().with_host(|h| {
                            h.discard_http_job(&job.id);
                        });
                        return;
                    }
                    let result = terrarium_runtime::net::fetch::run_account(
                        &job,
                        auth,
                        &mut |lease, tokens| {
                            app.state::<Plugins>()
                                .with_host(|h| h.update_http_account(&job, lease, tokens))
                                .unwrap_or_else(|| {
                                    Err(terrarium_runtime::net::NetError {
                                        code: "unavailable".into(),
                                        message: "Sign in again on this device".into(),
                                        temporary: false,
                                    })
                                })
                        },
                    );
                    let ended = app
                        .state::<Plugins>()
                        .with_host(|h| h.deliver_http(job, result))
                        .flatten();
                    if let Some(done) = ended {
                        terrarium_host::jobs::notify_ended(&AppShell(app.clone()), &done);
                    }
                    let _ = app.emit("data-changed", ());
                });
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    });
}
