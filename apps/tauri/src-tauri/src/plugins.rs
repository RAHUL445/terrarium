//! Plugin commands for the app's UI: the plugin list, templates, queries,
//! actions and spaces. The UI can only name declared queries and
//! operations, never raw SQL. Which version of each plugin runs, and
//! reloading, live in [`terrarium_host::plugins`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use terrarium_runtime::commands::{self, Caller, Kind};
use terrarium_runtime::{
    Manifest, RuntimeError, SpaceInfo, ViewKind, BUNDLED_DIGEST, DEFAULT_SPACE,
};

pub use terrarium_host::plugins::{package_dirs, LoadError, Origin, Plugins};
pub use terrarium_host::system_timezone;

#[derive(Serialize)]
pub struct PluginList {
    plugins: Vec<PluginInfo>,
    /// Removed from the space; can be added back.
    removed: Vec<RemovedInfo>,
    /// Synced versions awaiting local review or failed verification.
    pending: Vec<RemovedInfo>,
    errors: Vec<LoadError>,
    /// True when packages come from a development directory and reload live.
    dev: bool,
    /// `native` or `interpreter`.
    execution: &'static str,
}

#[derive(Serialize)]
struct PluginInfo {
    id: String,
    name: String,
    version: String,
    description: String,
    pages: Vec<ViewInfo>,
    widgets: Vec<ViewInfo>,
    origin: Option<Origin>,
    /// The version a rollback returns to.
    previous_version: Option<String>,
    source: Option<String>,
    capabilities: Vec<terrarium_runtime::Capability>,
    icon: Option<String>,
    color: Option<String>,
    /// Query names the shell runs for the nav badge and for search.
    badge: Option<String>,
    search: Option<String>,
    quick_actions: Vec<terrarium_runtime::QuickAction>,
}

#[derive(Serialize)]
struct RemovedInfo {
    id: String,
    version: String,
}

#[derive(Serialize)]
struct ViewInfo {
    id: String,
    title: String,
}

impl From<&Manifest> for PluginInfo {
    fn from(m: &Manifest) -> Self {
        let views = |v: &[terrarium_runtime::ViewDef]| {
            v.iter()
                .map(|v| ViewInfo {
                    id: v.id.clone(),
                    title: v.title.clone(),
                })
                .collect()
        };
        PluginInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            description: m.description.clone(),
            pages: views(&m.pages),
            widgets: views(&m.widgets),
            origin: None,
            previous_version: None,
            source: None,
            capabilities: terrarium_runtime::capabilities(m),
            icon: m.icon.clone(),
            color: m.color.clone(),
            badge: m.badge.clone(),
            search: m.search.clone(),
            quick_actions: m.quick_actions.clone(),
        }
    }
}

/// Plugins shipped inside the app, for builds without a package directory
/// (mobile, release). Built by `terra build` before the app is compiled.
/// (package id, [(path inside the package, contents)])
type BundledPackage = (&'static str, &'static [(&'static str, &'static [u8])]);

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// Writes bundled packages under `root` (only files whose content changed)
/// and returns their directories by plugin id.
pub fn install_bundled(root: &Path) -> std::io::Result<BTreeMap<String, PathBuf>> {
    let mut dirs = BTreeMap::new();
    for (id, files) in BUNDLED {
        let dir = root.join(id);
        for (rel, bytes) in *files {
            let path = dir.join(rel);
            if std::fs::read(&path).ok().as_deref() != Some(*bytes) {
                std::fs::create_dir_all(path.parent().unwrap_or(&dir))?;
                std::fs::write(&path, bytes)?;
            }
        }
        dirs.insert(id.to_string(), dir);
    }
    Ok(dirs)
}

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn plugins(state: State<'_, Plugins>) -> Result<PluginList, String> {
    let loaded = state.all_loaded();
    let (plugins, installed, execution) = state
        .with_host(|host| -> Result<_, String> {
            let installed = host.packages().installed().map_err(err)?;
            let plugins: Vec<PluginInfo> = host
                .manifests()
                .map(|m| {
                    let mut info = PluginInfo::from(m);
                    info.origin = loaded.get(&m.id).map(|l| l.origin);
                    if let Some(inst) = installed.iter().find(|i| i.id == m.id) {
                        if info.origin != Some(Origin::Dev) {
                            info.previous_version = inst.previous_version.clone();
                        }
                        info.source = inst.source.clone();
                    }
                    info
                })
                .collect();
            Ok((plugins, installed, host.execution_mode()))
        })
        .ok_or("plugin host unavailable")??;
    Ok(PluginList {
        plugins,
        pending: installed
            .iter()
            .filter(|i| {
                !i.removed
                    && i.digest != BUNDLED_DIGEST
                    && loaded.get(&i.id).and_then(|l| l.digest.as_deref())
                        != Some(i.digest.as_str())
            })
            .map(|i| RemovedInfo {
                id: i.id.clone(),
                version: i.version.clone(),
            })
            .collect(),
        removed: installed
            .iter()
            .filter(|i| i.removed)
            .map(|i| RemovedInfo {
                id: i.id.clone(),
                version: i.version.clone(),
            })
            .collect(),
        errors: state.errors(),
        dev: state.dev(),
        execution,
    })
}

#[tauri::command]
pub fn view_template(
    state: State<'_, Plugins>,
    plugin: String,
    kind: ViewKind,
    id: String,
) -> Result<Value, String> {
    state
        .with_host(|host| host.template(&plugin, kind, &id).cloned().map_err(err))
        .ok_or("plugin host unavailable")?
}

fn space_or_default(space: Option<String>) -> String {
    space.unwrap_or_else(|| DEFAULT_SPACE.to_string())
}

#[tauri::command]
pub fn run_query(
    state: State<'_, Plugins>,
    space: Option<String>,
    plugin: String,
    name: String,
    params: Map<String, Value>,
) -> Result<Value, String> {
    let id = format!("{plugin}.{name}");
    app_command(&state, Kind::Query, &id, space, Value::Object(params))
}

#[tauri::command]
pub fn run_action(
    state: State<'_, Plugins>,
    space: Option<String>,
    plugin: String,
    op: String,
    args: Value,
) -> Result<Value, String> {
    let id = format!("{plugin}.{op}");
    app_command(&state, Kind::Action, &id, space, args)
}

/// Runs a registry command for the app's own UI.
fn app_command(
    state: &Plugins,
    kind: Kind,
    id: &str,
    space: Option<String>,
    args: Value,
) -> Result<Value, String> {
    let space = space_or_default(space);
    state
        .with_host(|host| {
            commands::invoke(
                host,
                Caller::App,
                kind,
                id,
                Some(&space),
                &args,
                cfg!(desktop),
            )
            .map_err(|e| e.message)
        })
        .ok_or("plugin host unavailable")?
}

/// Any registry command (host or plugin), as the UI calls it.
#[tauri::command]
pub fn command(
    app: AppHandle,
    state: State<'_, Plugins>,
    kind: Kind,
    id: String,
    space: Option<String>,
    args: Option<Value>,
) -> Result<Value, String> {
    let out = app_command(&state, kind, &id, space, args.unwrap_or(Value::Null))?;
    if kind == Kind::Action {
        if id.starts_with("space.") {
            let _ = app.emit("spaces-changed", ());
        } else if id.starts_with("schedule.") {
            let _ = app.emit("data-changed", ());
        }
    }
    Ok(out)
}

/// Every command the app can run, with its parameters.
#[tauri::command]
pub fn describe_commands(state: State<'_, Plugins>) -> Result<Value, String> {
    state
        .with_host(|host| serde_json::to_value(commands::describe(host, Caller::App)).map_err(err))
        .ok_or("plugin host unavailable")?
}

// ---- Spaces --------------------------------------------------------------

/// Every space, the default first; removed ones are listed so they can be
/// brought back.
#[tauri::command]
pub fn list_spaces(state: State<'_, Plugins>) -> Result<Vec<SpaceInfo>, String> {
    state
        .with_host(|host| host.spaces().list().map_err(err))
        .ok_or("plugin host unavailable")?
}

/// Applies a change to the space list and opens or closes spaces to match.
fn change_spaces<T>(
    app: &AppHandle,
    f: impl FnOnce(&mut terrarium_runtime::Spaces) -> Result<T, RuntimeError>,
) -> Result<T, String> {
    let out = app
        .state::<Plugins>()
        .with_host(|host| -> Result<T, String> {
            let out = f(host.spaces_mut()).map_err(err)?;
            host.sync_spaces().map_err(err)?;
            Ok(out)
        })
        .ok_or("plugin host unavailable")??;
    let _ = app.emit("spaces-changed", ());
    Ok(out)
}

#[tauri::command]
pub fn create_space(app: AppHandle, name: String) -> Result<SpaceInfo, String> {
    change_spaces(&app, |spaces| spaces.create(&name))
}

#[tauri::command]
pub fn rename_space(app: AppHandle, id: String, name: String) -> Result<(), String> {
    change_spaces(&app, |spaces| spaces.rename(&id, &name))
}

/// Removes a space from every device (its data stays), or brings it back.
#[tauri::command]
pub fn set_space_removed(app: AppHandle, id: String, removed: bool) -> Result<(), String> {
    change_spaces(&app, |spaces| spaces.set_removed(&id, removed))
}
