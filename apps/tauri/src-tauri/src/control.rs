//! Settings → Developer → Command-line control, and starting the socket
//! with the app. The socket itself is in [`terrarium_host::control`].

use serde_json::{json, Value};
use tauri::{AppHandle, Listener, Manager, State};
use terrarium_runtime::commands::Scope;

pub use terrarium_host::Control;

use crate::plugins::Plugins;
use crate::shell::AppShell;

/// Starts the socket once plugins have loaded, and forwards app events to
/// subscribed clients.
pub fn start(app: &AppHandle) {
    terrarium_host::control::start(
        &app.state::<Control>(),
        &app.state::<Plugins>(),
        AppShell::arc(app),
    );
    for name in terrarium_host::EVENTS {
        let handle = app.clone();
        app.listen_any(name, move |_| handle.state::<Control>().broadcast(name));
    }
}

#[tauri::command]
pub fn control_state(state: State<'_, Control>) -> Value {
    state.state()
}

#[tauri::command]
pub fn control_set_enabled(
    app: AppHandle,
    state: State<'_, Control>,
    enabled: bool,
) -> Result<Value, String> {
    if !cfg!(desktop) {
        return Err("Command-line control is only available on a computer.".into());
    }
    state.set_enabled(enabled)?;
    state.apply(&app.state::<Plugins>(), &AppShell::arc(&app));
    Ok(state.state())
}

/// Adds a client and returns its token, which is never shown again.
#[tauri::command]
pub fn control_add_client(
    state: State<'_, Control>,
    name: String,
    scope: Scope,
) -> Result<Value, String> {
    let token = state.add_client(&name, scope)?;
    Ok(json!({ "token": token, "state": state.state() }))
}

#[tauri::command]
pub fn control_remove_client(state: State<'_, Control>, id: String) -> Result<Value, String> {
    state.remove_client(&id)?;
    Ok(state.state())
}
