//! Main-window-only OAuth commands; no command-registry/plugin entry point.
use crate::plugins::Plugins;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use terrarium_runtime::net::oauth::{authorize, Cancel, OAuthConfig};

#[derive(Default)]
pub struct Pending(pub Mutex<Option<Cancel>>);

#[tauri::command]
pub fn vault_cancel(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Accounts are available only in Settings.".into());
    }
    if let Some(cancel) = app.state::<Pending>().0.lock().unwrap().as_ref() {
        cancel.cancel();
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignIn {
    space: String,
    plugin: String,
    account: String,
    kind: String,
    client_id: String,
    client_secret: Option<String>,
}

#[tauri::command]
pub async fn vault_sign_in(
    app: AppHandle,
    window: WebviewWindow,
    request: SignIn,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("Accounts are available only in Settings.".into());
    }
    if !cfg!(desktop) {
        return Err(
            "Sign in on your computer, then sync and approve this account on this device.".into(),
        );
    }
    let SignIn {
        space,
        plugin,
        account,
        kind,
        client_id,
        client_secret,
    } = request;
    let cancel = Cancel::default();
    {
        let state = app.state::<Pending>();
        let mut pending = state.0.lock().unwrap();
        if pending.is_some() {
            return Err("A sign-in is already in progress.".into());
        }
        *pending = Some(cancel.clone());
    }
    let cleanup = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        let terrarium_runtime::OAuthPreparation {
            provider,
            signer,
            generation,
            mut client,
        } = plugins
            .with_host(|h| h.prepare_oauth(&space, &plugin, &account, &kind))
            .ok_or("Plugin host unavailable.")??;
        let (
            guard_plugins,
            guard_space,
            guard_plugin,
            guard_account,
            guard_signer,
            guard_generation,
            guard_provider,
        ) = (
            plugins.inner().clone(),
            space.clone(),
            plugin.clone(),
            account.clone(),
            signer.clone(),
            generation.clone(),
            provider.clone(),
        );
        client.set_guard(move || {
            if guard_plugins.with_host(|h| {
                h.oauth_current(
                    &guard_space,
                    &guard_plugin,
                    &guard_account,
                    (&guard_signer, guard_generation.as_deref()),
                    &guard_provider,
                )
            }) == Some(true)
            {
                Ok(())
            } else {
                Err(terrarium_runtime::net::NetError {
                    code: "cancelled".into(),
                    message: "Sign-in was cancelled.".into(),
                    temporary: false,
                })
            }
        });
        let cfg = OAuthConfig {
            provider,
            client_id,
            client_secret,
            cancel: cancel.clone(),
        };
        let tokens = authorize(&cfg, &client, crate::sync::open_browser).map_err(|e| e.message)?;
        let pending = app.state::<Pending>();
        let _guard = pending.0.lock().unwrap();
        let result = plugins
            .with_host(|h| {
                cancel.check().map_err(|e| e.message)?;
                h.finish_oauth(
                    &space,
                    &plugin,
                    &account,
                    (&signer, generation.as_deref()),
                    tokens,
                )
            })
            .ok_or("Plugin host unavailable.")?;
        let _ = app.emit("data-changed", ());
        result
    })
    .await
    .map_err(|_| "Could not sign in. Try again.".to_string());
    cleanup.state::<Pending>().0.lock().unwrap().take();
    result?
}
