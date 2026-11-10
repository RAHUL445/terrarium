//! Plugin host wiring: decides which version of each plugin runs, loads it,
//! reloads when `terra dev` rebuilds one, and exposes queries, actions and
//! templates to the UI. The UI can only name declared queries and
//! operations, never raw SQL.
//!
//! For each plugin id the running version is, in order of preference: a
//! development directory, the installed version from the synced plugin set
//! (unless removed there), or the version bundled with the app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use terrarium_runtime::commands::{self, Caller, Kind};
use terrarium_runtime::{
    fingerprint, HostConfig, Manifest, PluginHost, RuntimeError, SpaceInfo, ViewKind,
    BUNDLED_DIGEST, DEFAULT_SPACE,
};

pub struct Plugins {
    host: Mutex<PluginHost>,
    /// Development package directories, watched and reloaded on change.
    /// They take precedence over installed and bundled versions.
    dev_dirs: Vec<PathBuf>,
    /// Packages shipped inside the app, by plugin id.
    bundled: BTreeMap<String, PathBuf>,
    errors: Mutex<Vec<LoadError>>,
    /// What is loaded now; held while reconciling so passes never overlap.
    loaded: Mutex<BTreeMap<String, Loaded>>,
    /// Set once the first load pass has finished.
    ready: std::sync::atomic::AtomicBool,
}

/// Where the running version of a plugin came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Dev,
    Installed,
    Bundled,
}

#[derive(Clone)]
pub struct Loaded {
    dir: PathBuf,
    fingerprint: u128,
    pub origin: Origin,
    pub version: String,
    /// Digest of the installed package ([`BUNDLED_DIGEST`] for bundled).
    pub digest: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct LoadError {
    dir: String,
    error: String,
}

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

#[derive(Clone, Serialize)]
struct Reloaded {
    dir: String,
    plugin: Option<String>,
    error: Option<String>,
    /// First load at startup rather than a live reload.
    initial: bool,
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

/// Explicit development package paths; release/mobile use the imported bundle.
pub fn package_dirs() -> Vec<PathBuf> {
    if !cfg!(all(debug_assertions, desktop)) {
        return Vec::new();
    }
    std::env::var_os("TERRA_PLUGINS")
        .map(|list| {
            std::env::split_paths(&list)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
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

impl Plugins {
    /// Runs `f` with exclusive access to the plugin host (used by sync).
    pub fn with_host<T>(&self, f: impl FnOnce(&mut PluginHost) -> T) -> Option<T> {
        self.host.lock().ok().map(|mut host| f(&mut host))
    }

    pub fn package_dir(&self, id: &str) -> Option<PathBuf> {
        self.host
            .lock()
            .ok()?
            .package_dir(id)
            .map(Path::to_path_buf)
    }

    /// The running version of `id`, if loaded.
    pub fn loaded(&self, id: &str) -> Option<Loaded> {
        self.loaded.lock().ok()?.get(id).cloned()
    }

    /// `(version, digest)` to keep for rollback when `id` is replaced.
    pub fn current_for_rollback(&self, id: &str) -> Option<(String, String)> {
        let loaded = self.loaded(id)?;
        Some((loaded.version, loaded.digest?))
    }

    /// An empty host; [`Plugins::start`] loads packages in the background
    /// so the window and IPC are ready before plugins finish compiling
    /// (several seconds on a phone using the interpreter).
    pub fn new(
        cfg: HostConfig,
        dev_dirs: Vec<PathBuf>,
        bundled: BTreeMap<String, PathBuf>,
    ) -> Result<Plugins, RuntimeError> {
        Ok(Plugins {
            host: Mutex::new(PluginHost::new(cfg)?),
            dev_dirs,
            bundled,
            errors: Mutex::new(Vec::new()),
            loaded: Mutex::new(BTreeMap::new()),
            ready: Default::default(),
        })
    }

    /// Reconciles now, then keeps reconciling: dev rebuilds, and plugin
    /// set changes arriving by sync.
    pub fn start(app: AppHandle) {
        std::thread::spawn(move || loop {
            let plugins = app.state::<Plugins>();
            plugins.reconcile(&app);
            plugins
                .ready
                .store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(800));
        });
    }

    /// Whether the first load pass has finished, so every plugin that
    /// can load has.
    pub fn ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn set_error(&self, label: &str, error: Option<String>) {
        if let Ok(mut errors) = self.errors.lock() {
            errors.retain(|e| e.dir != label);
            if let Some(error) = error {
                errors.push(LoadError {
                    dir: label.to_string(),
                    error,
                });
            }
        }
    }

    /// The directory each plugin should run from, with its origin.
    fn desired(
        &self,
        host: &PluginHost,
        loaded: &BTreeMap<String, Loaded>,
    ) -> BTreeMap<String, Loaded> {
        let mut out = BTreeMap::new();
        let entry = |dir: &Path, origin, digest: Option<String>| {
            let manifest = Manifest::read(dir).ok()?;
            Some((
                manifest.id,
                Loaded {
                    dir: dir.to_path_buf(),
                    fingerprint: 0,
                    origin,
                    version: manifest.version,
                    digest,
                },
            ))
        };
        for dir in self.bundled.values() {
            if let Some((id, l)) = entry(dir, Origin::Bundled, Some(BUNDLED_DIGEST.into())) {
                out.insert(id, l);
            }
        }
        match host.packages().installed() {
            Ok(installed) => {
                for inst in installed {
                    let label = format!("installed {}", inst.id);
                    if inst.removed {
                        out.remove(&inst.id);
                        self.set_error(&label, None);
                        continue;
                    }
                    match host
                        .packages()
                        .materialize_approved(&inst, cfg!(debug_assertions))
                    {
                        Ok(Some(dir)) => {
                            self.set_error(&label, None);
                            if let Some((id, l)) =
                                entry(&dir, Origin::Installed, Some(inst.digest.clone()))
                            {
                                out.insert(id, l);
                            }
                        }
                        Ok(None) if out.contains_key(&inst.id) => self.set_error(&label, None),
                        Ok(None) => self.set_error(
                            &label,
                            Some(format!("{} is not bundled with this app version", inst.id)),
                        ),
                        Err(e) => {
                            self.set_error(&label, Some(e.to_string()));
                            // Fail closed: keep only the separately trusted bundled fallback.
                            // Never preserve an installed version after a failed security check.
                        }
                    }
                }
            }
            Err(e) => self.set_error("installed plugins", Some(e.to_string())),
        }
        for dir in &self.dev_dirs {
            match Manifest::read(dir) {
                Ok(m) => {
                    out.insert(
                        m.id,
                        Loaded {
                            dir: dir.clone(),
                            fingerprint: 0,
                            origin: Origin::Dev,
                            version: m.version,
                            digest: None,
                        },
                    );
                }
                Err(e) => {
                    self.set_error(&dir.display().to_string(), Some(e.to_string()));
                    // Mid-edit: keep the version that is running.
                    if let Some((id, current)) = loaded.iter().find(|(_, l)| &l.dir == dir) {
                        out.insert(id.clone(), current.clone());
                    }
                }
            }
        }
        out
    }

    /// Loads, reloads and unloads plugins to match [`Plugins::desired`].
    pub fn reconcile(&self, app: &AppHandle) {
        let Ok(mut loaded) = self.loaded.lock() else {
            return;
        };
        let initial = loaded.is_empty();
        let desired = match self.host.lock() {
            Ok(mut host) => {
                // Spaces created or removed on another device.
                match host.sync_spaces() {
                    Ok(true) => {
                        let _ = app.emit("spaces-changed", ());
                    }
                    Ok(false) => {}
                    Err(e) => self.set_error("spaces", Some(e.to_string())),
                }
                self.desired(&host, &loaded)
            }
            Err(_) => return,
        };
        for (id, mut want) in desired.clone() {
            want.fingerprint = fingerprint(&want.dir);
            if let Some(have) = loaded.get(&id) {
                if have.dir == want.dir && have.fingerprint == want.fingerprint {
                    continue;
                }
                if have.dir == want.dir && want.origin == Origin::Dev {
                    // Let the writer finish (build output lands in several files).
                    std::thread::sleep(Duration::from_millis(300));
                    want.fingerprint = fingerprint(&want.dir);
                }
            }
            let outcome = match self.host.lock() {
                Ok(mut host) => {
                    let start = std::time::Instant::now();
                    let result = host.load(&want.dir).map(|m| m.id.clone());
                    crate::startup_span("plugin-load", start);
                    result
                }
                Err(_) => continue,
            };
            let shown = want.dir.display().to_string();
            self.set_error(&shown, outcome.as_ref().err().map(|e| e.to_string()));
            let event = match outcome {
                Ok(loaded_id) => {
                    eprintln!("loaded plugin {loaded_id} {} from {shown}", want.version);
                    Reloaded {
                        dir: shown,
                        plugin: Some(loaded_id),
                        error: None,
                        initial,
                    }
                }
                Err(e) => {
                    // A failed verified replacement must not leave old installed
                    // code executing behind the displayed fallback/error state.
                    if want.origin != Origin::Dev {
                        if let Ok(mut host) = self.host.lock() {
                            host.unload(&id);
                        }
                    }
                    eprintln!("loading {id} failed from {shown}: {e}");
                    Reloaded {
                        dir: shown,
                        plugin: None,
                        error: Some(e.to_string()),
                        initial,
                    }
                }
            };
            // Recorded even on failure, so a broken package is retried only
            // once it changes.
            loaded.insert(id, want);
            let _ = app.emit("plugins-changed", event);
        }
        let gone: Vec<String> = loaded
            .keys()
            .filter(|id| !desired.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            loaded.remove(&id);
            if let Ok(mut host) = self.host.lock() {
                host.unload(&id);
            }
            eprintln!("unloaded plugin {id}");
            let _ = app.emit(
                "plugins-changed",
                Reloaded {
                    dir: String::new(),
                    plugin: Some(id),
                    error: None,
                    initial: false,
                },
            );
        }
    }
}

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn plugins(state: State<'_, Plugins>) -> Result<PluginList, String> {
    let loaded = state.loaded.lock().map_err(err)?.clone();
    let host = state.host.lock().map_err(err)?;
    let installed = host.packages().installed().map_err(err)?;
    let plugins = host
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
        errors: state.errors.lock().map_err(err)?.clone(),
        dev: !state.dev_dirs.is_empty(),
        execution: host.execution_mode(),
    })
}

#[tauri::command]
pub fn view_template(
    state: State<'_, Plugins>,
    plugin: String,
    kind: ViewKind,
    id: String,
) -> Result<Value, String> {
    let host = state.host.lock().map_err(err)?;
    host.template(&plugin, kind, &id).cloned().map_err(err)
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
    let mut host = state.host.lock().map_err(err)?;
    let space = space_or_default(space);
    commands::invoke(
        &mut host,
        Caller::App,
        kind,
        id,
        Some(&space),
        &args,
        cfg!(desktop),
    )
    .map_err(|e| e.message)
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
    let host = state.host.lock().map_err(err)?;
    serde_json::to_value(commands::describe(&host, Caller::App)).map_err(err)
}

// ---- Spaces --------------------------------------------------------------

/// Every space, the default first; removed ones are listed so they can be
/// brought back.
#[tauri::command]
pub fn list_spaces(state: State<'_, Plugins>) -> Result<Vec<SpaceInfo>, String> {
    let host = state.host.lock().map_err(err)?;
    host.spaces().list().map_err(err)
}

/// Applies a change to the space list and opens or closes spaces to match.
fn change_spaces<T>(
    app: &AppHandle,
    f: impl FnOnce(&mut terrarium_runtime::Spaces) -> Result<T, RuntimeError>,
) -> Result<T, String> {
    let state = app.state::<Plugins>();
    let mut host = state.host.lock().map_err(err)?;
    let out = f(host.spaces_mut()).map_err(err)?;
    host.sync_spaces().map_err(err)?;
    drop(host);
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
