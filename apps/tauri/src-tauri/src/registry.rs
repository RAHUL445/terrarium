//! Desktop registry adapter. No registry operation holds the plugin-host lock
//! during HTTP, and no catalog result confers publisher trust.
use crate::plugins::{err, Origin, Plugins};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{AppHandle, Emitter, Manager, State};
use terrarium_runtime::registry_client::{self as client, Settings, Transport};
use terrarium_runtime::registry_index::{
    self as index, Config, IndexPlugin, IndexVersion, Inventory,
};

// Activation requires REL-01, REL-03 and owner-supplied origin/keys.
const REGISTRY_KEYS: &[&str] = &[];
const REGISTRY_ORIGIN: &str = "";
const REGISTRY_REVOKED: &[&str] = &[];
const MINIMUM_SEQUENCE: u64 = 1;
const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Registry {
    root: PathBuf,
    config: Config,
    refresh: Mutex<()>,
    download: Mutex<()>,
    refresh_cancel: AtomicBool,
    download_cancel: AtomicBool,
}
impl Registry {
    pub fn open(root: &Path) -> Result<Self, String> {
        let config = Config {
            origin: REGISTRY_ORIGIN.into(),
            keys: REGISTRY_KEYS.iter().map(|s| (*s).into()).collect(),
            revoked: REGISTRY_REVOKED.iter().map(|s| (*s).into()).collect(),
            minimum_sequence: MINIMUM_SEQUENCE,
        };
        let state = Self {
            root: root.to_owned(),
            config,
            refresh: Mutex::new(()),
            download: Mutex::new(()),
            refresh_cancel: AtomicBool::new(false),
            download_cancel: AtomicBool::new(false),
        };
        #[cfg(debug_assertions)]
        let state = {
            let mut state = state;
            if let Some(origin) = std::env::var_os("TERRA_REGISTRY_ORIGIN") {
                let key = std::env::var("TERRA_REGISTRY_KEY").map_err(|_| index::VERIFY_ERROR)?;
                // Separate fixture cache AND package/consent profile. Never reuse
                // production state when a debug registry override is supplied.
                let profile = std::env::var_os("TERRA_PROFILE_DIR")
                    .ok_or("Registry fixtures require TERRA_PROFILE_DIR")?;
                if !Path::new(&profile).is_absolute() || Path::new(&profile) != root {
                    return Err(index::VERIFY_ERROR.into());
                }
                state.config.origin = origin.into_string().map_err(|_| index::VERIFY_ERROR)?;
                state.config.keys = vec![key];
                state.config.validate()?;
                state.root = root.join(format!(
                    "registry-fixture-{}",
                    index::hash(
                        format!("{}{:?}", state.config.origin, state.config.keys).as_bytes()
                    )
                ));
                std::fs::create_dir_all(&state.root).map_err(err)?;
            }
            state
        };
        Ok(state)
    }
    fn settings_path(&self) -> PathBuf {
        self.root.join("registry-settings.json")
    }
    fn enabled(&self) -> bool {
        cfg!(desktop) && !self.config.keys.is_empty()
    }
    fn fresh(&self) -> Result<index::Index, String> {
        if !self.enabled() {
            return Err("No plugin list is set up".into());
        }
        client::load(&client::cache_path(&self.root))?
            .ok_or(index::VERIFY_ERROR)?
            .fresh(&self.config, now())
    }
    fn check(&self, manual: bool) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        // Calls from other windows coalesce; they can read the prior safe list.
        let Ok(_guard) = self.refresh.try_lock() else {
            return Ok(());
        };
        let mut settings = Settings::load(&self.settings_path())?;
        if !manual && !settings.due(now()) {
            return Ok(());
        }
        settings.last_attempt_ms = Some(now());
        settings.save(&self.settings_path())?;
        self.refresh_cancel.store(false, Ordering::SeqCst);
        client::refresh(
            &client::Https,
            &client::cache_path(&self.root),
            &self.config,
            now,
            &self.refresh_cancel,
        )?;
        Ok(())
    }
    pub(crate) fn validate_context(&self, context: &Context) -> Result<(), String> {
        let latest = self.fresh()?;
        index::selection_unchanged(
            &latest,
            &context.registry,
            context.sequence,
            &context.plugin,
            &context.version,
            HOST_VERSION,
        )
    }
    /// Serialize confirmation with refresh until the install has consumed consent.
    pub(crate) fn confirmation_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.refresh.lock().map_err(err)
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
#[derive(Clone)]
pub(crate) struct Context {
    registry: String,
    sequence: u64,
    plugin: IndexPlugin,
    version: IndexVersion,
}
#[derive(Serialize)]
pub struct Listing {
    configured: bool,
    automatic_checks: bool,
    fresh: bool,
    expires_ms: Option<u64>,
    message: Option<String>,
    plugins: Vec<Card>,
}
#[derive(Serialize)]
struct Card {
    id: String,
    name: String,
    description: String,
    icon: String,
    publisher: String,
    version: String,
    capabilities: Vec<String>,
    installed_version: Option<String>,
    update: bool,
    can_install: bool,
    warning: Option<String>,
}
fn inventory(plugins: &Plugins) -> Result<Vec<Inventory>, String> {
    let loaded = plugins.all_loaded();
    plugins
        .with_host(|host| {
            let mut rows: Vec<Inventory> = host
                .packages()
                .installed()
                .map_err(err)?
                .into_iter()
                .map(|i| Inventory {
                    shadowed_by_dev: loaded.get(&i.id).is_some_and(|l| l.origin == Origin::Dev),
                    id: i.id,
                    version: i.version,
                    removed: i.removed,
                })
                .collect();
            for (id, l) in loaded {
                if !rows.iter().any(|i| i.id == id) {
                    rows.push(Inventory {
                        id,
                        version: l.version,
                        removed: false,
                        shadowed_by_dev: l.origin == Origin::Dev,
                    });
                }
            }
            Ok(rows)
        })
        .ok_or("plugin host unavailable")?
}
fn listing(state: &Registry, plugins: &Plugins, error: Option<String>) -> Result<Listing, String> {
    let settings = Settings::load(&state.settings_path())?;
    let mut list = Listing {
        configured: state.enabled(),
        automatic_checks: settings.automatic_checks,
        fresh: false,
        expires_ms: None,
        message: error,
        plugins: vec![],
    };
    if !state.enabled() {
        list.message = Some(
            if cfg!(desktop) {
                "No plugin list is set up"
            } else {
                "Plugin downloads are available on desktop."
            }
            .into(),
        );
        return Ok(list);
    }
    let cached = match client::load(&client::cache_path(&state.root)) {
        Ok(Some(cache)) => cache,
        Ok(None) => return Ok(list),
        Err(e) => {
            list.message = Some(e);
            return Ok(list);
        }
    };
    let (catalog, fresh) = match cached.display(&state.config, now()) {
        Ok(value) => value,
        Err(e) => {
            list.message = Some(e);
            return Ok(list);
        }
    };
    list.fresh = fresh;
    list.expires_ms = Some(catalog.expires_ms);
    if !fresh {
        list.message = Some(index::EXPIRED.into());
    }
    let installed = inventory(plugins)?;
    let updates = index::updates(&catalog, &installed, HOST_VERSION);
    for p in &catalog.plugins {
        let newest = index::newest(p, HOST_VERSION);
        let v = newest
            .or_else(|| p.versions.iter().max_by_key(|v| index::stable(&v.version)))
            .ok_or(index::VERIFY_ERROR)?;
        let current = installed.iter().find(|i| i.id == p.id && !i.removed);
        let update = fresh && updates.iter().any(|(id, _)| id == &p.id);
        let warning = if current.is_some_and(|i| i.shadowed_by_dev) {
            Some("Remove the development folder to install this plugin.")
        } else if newest.is_none() {
            Some("Update Terrarium to install this version.")
        } else if current.is_some_and(|i| {
            p.versions
                .iter()
                .any(|v| v.version == i.version && v.yanked)
        }) {
            Some("This version was withdrawn by its publisher.")
        } else if current.is_some_and(|i| index::stable(&i.version).is_none()) {
            Some("Check this plugin's version before installing an update.")
        } else {
            None
        };
        let can_install = fresh
            && newest.is_some()
            && !current.is_some_and(|i| i.shadowed_by_dev)
            && (current.is_none() || update);
        let mut caps = v.capabilities.clone();
        caps.sort();
        list.plugins.push(Card {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            icon: p.icon.clone(),
            publisher: terrarium_runtime::key_fingerprint(&p.publisher),
            version: v.version.clone(),
            capabilities: caps,
            installed_version: current.map(|i| i.version.clone()),
            update,
            can_install,
            warning: warning.map(str::to_owned),
        });
    }
    Ok(list)
}
#[tauri::command]
pub async fn registry_index(app: AppHandle, refresh: bool) -> Result<Listing, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Registry>();
        let error = if refresh {
            state.check(true).err()
        } else {
            None
        };
        listing(&state, &app.state::<Plugins>(), error)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub async fn registry_settings(app: AppHandle, automatic_checks: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Registry>();
        let _guard = state.refresh.lock().map_err(err)?;
        let mut settings = Settings::load(&state.settings_path())?;
        settings.automatic_checks = automatic_checks;
        settings.save(&state.settings_path())
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub fn registry_cancel(state: State<'_, Registry>) {
    state.refresh_cancel.store(true, Ordering::SeqCst);
    state.download_cancel.store(true, Ordering::SeqCst);
}
#[tauri::command]
pub async fn registry_stage(
    app: AppHandle,
    id: String,
    version: String,
) -> Result<crate::install::Review, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Registry>();
        if !state.enabled() {
            return Err("Plugin downloads are available on a configured desktop.".into());
        }
        let _download = state
            .download
            .try_lock()
            .map_err(|_| "A plugin is already downloading.")?;
        let catalog = state.fresh()?;
        let p = catalog
            .plugins
            .iter()
            .find(|p| p.id == id)
            .ok_or(index::VERIFY_ERROR)?;
        let v = p
            .versions
            .iter()
            .find(|v| v.version == version)
            .filter(|v| index::compatible(v, HOST_VERSION))
            .ok_or(index::VERIFY_ERROR)?;
        if let Some(current) = inventory(&app.state::<Plugins>())?
            .iter()
            .find(|i| i.id == id && !i.removed)
        {
            if current.shadowed_by_dev {
                return Err("Remove the development folder to install this plugin.".into());
            }
            if index::stable(&current.version)
                .zip(index::stable(&v.version))
                .is_none_or(|(old, new)| new <= old)
            {
                return Err("This plugin is already installed. Check for plugins again.".into());
            }
        }
        state.download_cancel.store(false, Ordering::SeqCst);
        let bytes = client::Https.get(
            &v.url,
            terrarium_runtime::MAX_PACKAGE_BYTES as usize,
            false,
            &state.download_cancel,
        )?;
        let package = index::check_package(&bytes, p, v)?;
        let context = Context {
            registry: catalog.registry.clone(),
            sequence: catalog.sequence,
            plugin: p.clone(),
            version: v.clone(),
        };
        let _refresh = state.confirmation_guard()?;
        state.validate_context(&context)?;
        if state.download_cancel.load(Ordering::SeqCst) {
            return Err(client::NETWORK_ERROR.into());
        }
        // Same checked bytes parsed above, with existing review and consent logic.
        crate::install::stage_package(
            &app,
            package,
            terrarium_runtime::PackageSource::File(format!("Registry: {id} {version}")),
            false,
            Some(context),
        )
    })
    .await
    .map_err(err)?
}
pub fn start(app: AppHandle) {
    if !app.state::<Registry>().enabled() {
        return;
    }
    std::thread::spawn(move || loop {
        let error = app.state::<Registry>().check(false).err();
        let _ = app.emit("registry-changed", error);
        std::thread::sleep(std::time::Duration::from_secs(60));
    });
}
