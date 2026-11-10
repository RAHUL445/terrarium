//! Plugin host wiring: decides which version of each plugin runs, loads it,
//! and reloads when `terra dev` rebuilds one or the synced plugin set
//! changes.
//!
//! For each plugin id the running version is, in order of preference: a
//! development directory, the installed version from the synced plugin set
//! (unless removed there), or the version bundled with the app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use terrarium_runtime::{
    fingerprint, HostConfig, Manifest, PluginHost, RuntimeError, BUNDLED_DIGEST,
};

use crate::Shell;

/// The plugin host and what is loaded in it. Cloning shares it.
#[derive(Clone)]
pub struct Plugins(Arc<Inner>);

struct Inner {
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
    ready: AtomicBool,
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

#[derive(Clone, Serialize)]
struct Reloaded {
    dir: String,
    plugin: Option<String>,
    error: Option<String>,
    /// First load at startup rather than a live reload.
    initial: bool,
}

/// Explicit development package paths (`TERRA_PLUGINS`), in debug builds on
/// a computer only; release and mobile builds use the bundled packages.
pub fn package_dirs() -> Vec<PathBuf> {
    if !(cfg!(debug_assertions) && crate::DESKTOP) {
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

impl Plugins {
    /// An empty host; [`Plugins::start`] loads packages in the background
    /// so the window and IPC are ready before plugins finish compiling
    /// (several seconds on a phone using the interpreter).
    pub fn new(
        cfg: HostConfig,
        dev_dirs: Vec<PathBuf>,
        bundled: BTreeMap<String, PathBuf>,
    ) -> Result<Plugins, RuntimeError> {
        Ok(Plugins(Arc::new(Inner {
            host: Mutex::new(PluginHost::new(cfg)?),
            dev_dirs,
            bundled,
            errors: Mutex::new(Vec::new()),
            loaded: Mutex::new(BTreeMap::new()),
            ready: AtomicBool::new(false),
        })))
    }

    /// Runs `f` with exclusive access to the plugin host.
    pub fn with_host<T>(&self, f: impl FnOnce(&mut PluginHost) -> T) -> Option<T> {
        self.0.host.lock().ok().map(|mut host| f(&mut host))
    }

    pub fn package_dir(&self, id: &str) -> Option<PathBuf> {
        self.with_host(|host| host.package_dir(id).map(Path::to_path_buf))
            .flatten()
    }

    /// The running version of `id`, if loaded.
    pub fn loaded(&self, id: &str) -> Option<Loaded> {
        self.0.loaded.lock().ok()?.get(id).cloned()
    }

    /// Every loaded plugin and where it came from.
    pub fn all_loaded(&self) -> BTreeMap<String, Loaded> {
        self.0.loaded.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Packages that failed to load, with why.
    pub fn errors(&self) -> Vec<LoadError> {
        self.0.errors.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Whether packages come from development directories.
    pub fn dev(&self) -> bool {
        !self.0.dev_dirs.is_empty()
    }

    /// `(version, digest)` to keep for rollback when `id` is replaced.
    pub fn current_for_rollback(&self, id: &str) -> Option<(String, String)> {
        let loaded = self.loaded(id)?;
        Some((loaded.version, loaded.digest?))
    }

    /// Reconciles now, then keeps reconciling: dev rebuilds, and plugin
    /// set changes arriving by sync.
    pub fn start(&self, shell: Arc<dyn Shell>) {
        let plugins = self.clone();
        std::thread::spawn(move || loop {
            plugins.reconcile(&*shell);
            plugins.0.ready.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(800));
        });
    }

    /// Whether the first load pass has finished, so every plugin that
    /// can load has.
    pub fn ready(&self) -> bool {
        self.0.ready.load(Ordering::SeqCst)
    }

    /// Waits up to `limit` for [`Plugins::ready`].
    pub fn wait_ready(&self, limit: Duration) -> bool {
        let started = std::time::Instant::now();
        while !self.ready() && started.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(100));
        }
        self.ready()
    }

    fn set_error(&self, label: &str, error: Option<String>) {
        if let Ok(mut errors) = self.0.errors.lock() {
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
        for dir in self.0.bundled.values() {
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
        for dir in &self.0.dev_dirs {
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
    pub fn reconcile(&self, shell: &dyn Shell) {
        let Ok(mut loaded) = self.0.loaded.lock() else {
            return;
        };
        let initial = loaded.is_empty();
        let desired = match self.0.host.lock() {
            Ok(mut host) => {
                // Spaces created or removed on another device.
                match host.sync_spaces() {
                    Ok(true) => shell.emit("spaces-changed", json!(null)),
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
            let outcome = match self.0.host.lock() {
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
                        if let Ok(mut host) = self.0.host.lock() {
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
            shell.emit("plugins-changed", json!(event));
        }
        let gone: Vec<String> = loaded
            .keys()
            .filter(|id| !desired.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            loaded.remove(&id);
            if let Ok(mut host) = self.0.host.lock() {
                host.unload(&id);
            }
            eprintln!("unloaded plugin {id}");
            shell.emit(
                "plugins-changed",
                json!(Reloaded {
                    dir: String::new(),
                    plugin: Some(id),
                    error: None,
                    initial: false,
                }),
            );
        }
    }
}
