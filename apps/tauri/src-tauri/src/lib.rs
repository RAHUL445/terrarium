//! App shell: opens the personal space, runs installed plugins (phase 2:
//! loaded from development package directories), and keeps the phase 1
//! developer storage inspector over a built-in test plugin.

mod ai;
mod install;
mod jobs;
mod plugins;
mod secrets;
mod sync;
mod themes;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{Manager, State};
use terrarium_core::{
    ChangeRecord, ConflictRecord, DeviceId, NoticeRecord, PluginSchema, Store, StoreConfig,
    SystemTime,
};

const SPACE: &str = "personal";
const PLUGIN: &str = "inspector-test";
const SIM_DEVICE: &str = "sim-b";

/// Static phase labels and elapsed time only. Opt-in even in release builds.
pub(crate) fn startup_span(phase: &'static str, start: Instant) {
    if cfg!(feature = "startup-profiling") {
        eprintln!(
            "[terrarium-startup] {}",
            json!({"phase": phase, "duration_ms": ms(start)})
        );
    }
}

struct AppState {
    store: Mutex<Store>,
    data_dir: PathBuf,
    host_cfg: terrarium_runtime::HostConfig,
    startup_ms: f64,
    launched: Instant,
    profiling: bool,
}

/// Local profiling builds signal readiness after the frontend's startup work.
/// No record contents or identifiers are included in the diagnostic output.
#[tauri::command]
fn profile_startup(state: State<'_, AppState>, frontend_ready_ms: f64) -> CmdResult<()> {
    if state.profiling && frontend_ready_ms.is_finite() && frontend_ready_ms >= 0.0 {
        eprintln!(
            "[terrarium-native-perf] {}",
            json!({
                "native_ready_ms": ms(state.launched),
                "setup_ms": state.startup_ms,
                "frontend_ready_ms": frontend_ready_ms,
            })
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct Snapshot {
    space: String,
    device: String,
    plugin: String,
    schema_version: u32,
    items: Vec<Map<String, Value>>,
    changes: Vec<ChangeRecord>,
    conflicts: Vec<ConflictRecord>,
    notices: Vec<NoticeRecord>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn demo_schema() -> PluginSchema {
    serde_json::from_str(include_str!("../demo-schema.json")).expect("demo schema is valid JSON")
}

fn open_store(dir: &Path, device: DeviceId) -> terrarium_core::Result<Store> {
    let cfg = StoreConfig {
        space: SPACE.into(),
        plugin: PLUGIN.into(),
        device,
        user: "owner".into(),
    };
    Store::open(
        dir.join(format!("{PLUGIN}.db")),
        cfg,
        demo_schema(),
        Box::new(SystemTime),
    )
}

/// A stable random id for this installation, created on first run.
fn device_id(data_dir: &Path) -> std::io::Result<DeviceId> {
    let path = data_dir.join("device-id");
    match fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => Ok(DeviceId(id.trim().to_string())),
        _ => {
            let id = DeviceId::new_random();
            fs::write(&path, &id.0)?;
            Ok(id)
        }
    }
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let store = state.store.lock().map_err(err)?;
    let cfg = store.config();
    Ok(Snapshot {
        space: cfg.space.clone(),
        device: cfg.device.0.clone(),
        plugin: cfg.plugin.clone(),
        schema_version: store.schema().version,
        items: store
            .query(
                "SELECT id, title, done, points, _deleted FROM items ORDER BY id DESC",
                &[],
            )
            .map_err(err)?,
        changes: store.changes(50).map_err(err)?,
        conflicts: store.conflicts().map_err(err)?,
        notices: store.notices().map_err(err)?,
    })
}

#[tauri::command]
fn add_item(state: State<'_, AppState>, title: String) -> CmdResult<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Title is required".into());
    }
    let mut store = state.store.lock().map_err(err)?;
    let values = json!({ "title": title })
        .as_object()
        .cloned()
        .unwrap_or_default();
    store.write(|w| w.insert("items", &values)).map_err(err)
}

#[tauri::command]
fn toggle_item(state: State<'_, AppState>, id: String, done: bool) -> CmdResult<()> {
    let mut store = state.store.lock().map_err(err)?;
    let values = json!({ "done": done })
        .as_object()
        .cloned()
        .unwrap_or_default();
    store
        .write(|w| w.update("items", &id, &values))
        .map_err(err)
}

#[tauri::command]
fn bump_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let mut store = state.store.lock().map_err(err)?;
    store
        .write(|w| w.increment("items", &id, "points", json!(1)))
        .map_err(err)
}

#[tauri::command]
fn delete_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let mut store = state.store.lock().map_err(err)?;
    store.write(|w| w.delete("items", &id)).map_err(err)
}

#[tauri::command]
fn restore_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let mut store = state.store.lock().map_err(err)?;
    store.write(|w| w.restore("items", &id)).map_err(err)
}

/// Shows a real merge: a simulated second device edits the newest live item
/// "offline" while this device edits it too, then both exchange changes.
#[tauri::command]
fn simulate_conflict(state: State<'_, AppState>) -> CmdResult<()> {
    let mut store = state.store.lock().map_err(err)?;
    let newest = store
        .query(
            "SELECT id, title FROM items WHERE _deleted IS NULL ORDER BY id DESC LIMIT 1",
            &[],
        )
        .map_err(err)?
        .pop();
    let (id, title) = match newest {
        Some(row) => (
            row.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            row.get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        None => {
            let values = json!({ "title": "Buy milk" })
                .as_object()
                .cloned()
                .unwrap_or_default();
            (
                store.write(|w| w.insert("items", &values)).map_err(err)?,
                "Buy milk".to_string(),
            )
        }
    };

    let sim_dir = state.data_dir.join("simulated-device");
    fs::create_dir_all(&sim_dir).map_err(err)?;
    let mut sim = open_store(&sim_dir, SIM_DEVICE.into()).map_err(err)?;
    sim.import(&store.export_since(0).map_err(err)?.0)
        .map_err(err)?;

    let edit = |suffix: &str| {
        json!({ "title": format!("{title} ({suffix})") })
            .as_object()
            .cloned()
            .unwrap_or_default()
    };
    sim.write(|w| w.update("items", &id, &edit("edited on sim-b")))
        .map_err(err)?;
    store
        .write(|w| w.update("items", &id, &edit("edited here")))
        .map_err(err)?;

    store
        .import(&sim.export_since(0).map_err(err)?.0)
        .map_err(err)?;
    sim.import(&store.export_since(0).map_err(err)?.0)
        .map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
struct Benchmark {
    os: &'static str,
    arch: &'static str,
    execution: &'static str,
    startup_ms: f64,
    plugin_load_ms: f64,
    calls: usize,
    call_avg_ms: f64,
    call_p95_ms: f64,
    query_ms: f64,
    inserts: usize,
    insert_total_ms: f64,
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Measures this device: To-do load (component compile + storage open),
/// operation round trips, a query, and raw storage inserts. Runs against
/// throwaway databases, never the user's data.
#[tauri::command]
fn run_benchmark(
    state: State<'_, AppState>,
    plugins: State<'_, plugins::Plugins>,
) -> CmdResult<Benchmark> {
    let todo = plugins
        .package_dir("todo")
        .ok_or("To-do plugin is not loaded")?;
    let scratch = state.data_dir.join("benchmark");
    let _ = fs::remove_dir_all(&scratch);
    let result = (|| -> CmdResult<Benchmark> {
        let mut cfg = state.host_cfg.clone();
        cfg.data_dir = scratch.join("plugins");
        let mut host = terrarium_runtime::PluginHost::new(cfg).map_err(err)?;
        let started = Instant::now();
        host.load(&todo).map_err(err)?;
        let plugin_load_ms = ms(started);

        let calls = 50;
        let mut times = Vec::with_capacity(calls);
        for i in 0..calls {
            let started = Instant::now();
            host.call(
                "todo",
                "create",
                &json!({ "title": format!("Benchmark task {i}") }),
            )
            .map_err(err)?;
            times.push(ms(started));
        }
        times.sort_by(f64::total_cmp);
        let started = Instant::now();
        let params = json!({ "today": "2026-11-01", "filter": "all", "q": "" });
        host.query("todo", "tasks", params.as_object().unwrap())
            .map_err(err)?;
        let query_ms = ms(started);

        let inserts = 1000;
        let mut store = open_store(&scratch, "benchmark".into()).map_err(err)?;
        let started = Instant::now();
        store
            .write(|w| {
                for i in 0..inserts {
                    let values = json!({ "title": format!("row {i}") })
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    w.insert("items", &values)?;
                }
                Ok(())
            })
            .map_err(err)?;
        Ok(Benchmark {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            execution: host.execution_mode(),
            startup_ms: state.startup_ms,
            plugin_load_ms,
            calls,
            call_avg_ms: times.iter().sum::<f64>() / calls as f64,
            call_p95_ms: times[calls * 95 / 100],
            query_ms,
            inserts,
            insert_total_ms: ms(started),
        })
    })();
    let _ = fs::remove_dir_all(&scratch);
    result
}

/// Local speech-to-text settings from `<app data>/speech.json`
/// (`{"python", "model_dir", "ffmpeg"}`), used only if the files exist.
fn speech_config(app_data: &Path) -> Option<terrarium_runtime::SpeechConfig> {
    let bytes = fs::read(app_data.join("speech.json")).ok()?;
    let config: terrarium_runtime::SpeechConfig = serde_json::from_slice(&bytes).ok()?;
    if config.ready() {
        Some(config)
    } else {
        eprintln!("speech.json is present but its files are missing; transcription stays off");
        None
    }
}

/// Opens an http(s) link from a plugin page in the system browser.
#[tauri::command]
fn open_external(app: tauri::AppHandle, url: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) || url.len() > 2048 {
        return Err("Only http and https links can be opened.".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let launched = Instant::now();
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // Explicit developer profiling uses a separate, absolute data root.
            let profile_dir = std::env::var_os("TERRA_PROFILE_DIR").map(PathBuf::from);
            if profile_dir.as_ref().is_some_and(|path| !path.is_absolute()) {
                return Err("TERRA_PROFILE_DIR must be an absolute path".into());
            }
            let app_data = match &profile_dir {
                Some(path) => path.clone(),
                None => app.path().app_data_dir()?,
            };
            let data_dir = app_data.join("spaces").join(SPACE);
            fs::create_dir_all(&data_dir)?;
            let device = device_id(&data_dir)?;
            let store = open_store(&data_dir, device.clone())?;
            startup_span("store-ready", launched);
            let host_cfg = terrarium_runtime::HostConfig {
                data_dir: data_dir.join("plugins"),
                // Other spaces: <app data>/spaces/<id>/plugins.
                spaces_dir: Some(app_data.join("spaces")),
                space: SPACE.into(),
                device,
                user: "owner".into(),
                timezone: plugins::system_timezone(),
                execution: Default::default(),
                speech: speech_config(&app_data),
            };
            let dev_dirs = plugins::package_dirs();
            let stage = Instant::now();
            let bundled = plugins::install_bundled(&app_data.join("packages"))?;
            startup_span("bundle-install", stage);
            let stage = Instant::now();
            let plugins = plugins::Plugins::new(host_cfg.clone(), dev_dirs, bundled)?;
            startup_span("plugin-host-create", stage);
            install::trust_developer_key(&plugins);
            app.manage(plugins);
            app.manage(install::Pending::default());
            let stage = Instant::now();
            app.manage(sync::Sync::open(&data_dir, SPACE, host_cfg.device.clone())?);
            startup_span("sync-open", stage);
            plugins::Plugins::start(app.handle().clone());
            sync::Sync::start(app.handle().clone());
            jobs::start(app.handle().clone(), data_dir.join("jobs-state.json"));
            ai::start(app.handle().clone());
            // Development aid: start pairing at launch and log the code.
            if cfg!(debug_assertions) && std::env::var_os("TERRA_DEV_PAIR").is_some() {
                match app.state::<sync::Sync>().begin_pairing() {
                    Ok(code) => eprintln!(
                        "pairing code {} (expires in {} s)",
                        code.code, code.expires_in_s
                    ),
                    Err(e) => eprintln!("pairing failed to start: {e}"),
                }
            }
            app.manage(AppState {
                store: Mutex::new(store),
                data_dir,
                host_cfg,
                startup_ms: ms(launched),
                launched,
                profiling: profile_dir.is_some() || cfg!(feature = "startup-profiling"),
            });
            startup_span("setup-ready", launched);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            profile_startup,
            add_item,
            toggle_item,
            bump_item,
            delete_item,
            restore_item,
            simulate_conflict,
            run_benchmark,
            open_external,
            plugins::plugins,
            install::stage_install,
            install::confirm_install,
            install::cancel_install,
            install::rollback_plugin,
            install::set_plugin_removed,
            install::trusted_signers,
            ai::service,
            ai::service_net,
            themes::list_themes,
            themes::install_theme,
            themes::remove_theme,
            sync::create_backup,
            sync::list_backups,
            sync::restore_backup,
            plugins::view_template,
            plugins::run_query,
            plugins::run_action,
            plugins::command,
            plugins::describe_commands,
            plugins::list_spaces,
            plugins::create_space,
            plugins::rename_space,
            plugins::set_space_removed,
            sync::sync_status,
            sync::remove_sync_device,
            sync::start_pairing,
            sync::cancel_pairing,
            sync::join_space,
            sync::sync_now,
            sync::recovery_phrase,
            sync::connect_drive,
            sync::disconnect_drive
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Terrarium");
}
