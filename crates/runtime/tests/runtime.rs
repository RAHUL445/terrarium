//! Builds real plugin components with `build_plugin` and runs them.

mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_runtime::{build_plugin, Execution, HostConfig, PluginHost, RuntimeError, ViewKind};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Builds each package once per test run.
fn built(dir: PathBuf) -> PathBuf {
    static TODO: OnceLock<()> = OnceLock::new();
    static LIMITS: OnceLock<()> = OnceLock::new();
    let lock = if dir.ends_with("todo") {
        &TODO
    } else {
        &LIMITS
    };
    lock.get_or_init(|| {
        build_plugin(&dir).unwrap_or_else(|e| panic!("building {}: {e}", dir.display()));
    });
    dir
}

fn todo_dir() -> PathBuf {
    built(support::package_root().join("todo"))
}

fn limits_dir() -> PathBuf {
    built(repo().join("crates/runtime/tests/fixtures/limits"))
}

fn host(data: &TempDir) -> PluginHost {
    host_with(data, Execution::Auto)
}

fn host_with(data: &TempDir, execution: Execution) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "Asia/Kolkata".into(),
        execution,
        speech: None,
    })
    .unwrap()
}

fn params(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn titles(rows: &[Map<String, Value>]) -> Vec<&str> {
    rows.iter().map(|r| r["title"].as_str().unwrap()).collect()
}

/// Copies a built package so a test can change its manifest.
fn copy_package(from: &Path, to: &Path) {
    for entry in walk(from) {
        let rel = entry.strip_prefix(from).unwrap();
        if rel.starts_with("target") || rel.starts_with("src") {
            continue;
        }
        let dest = to.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&entry, dest).unwrap();
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn todo_create_list_toggle_and_trash() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();

    let created = host
        .call(
            "todo",
            "create",
            &json!({"title": "  Pay rent ", "due": "2026-11-01", "tags": "Home, bills, home"}),
        )
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    host.call(
        "todo",
        "create",
        &json!({"title": "Plan trip", "due": "2026-12-20"}),
    )
    .unwrap();

    let today = params(json!({"today": "2026-11-01", "filter": "today", "q": ""}));
    let rows = host.query("todo", "tasks", &today).unwrap();
    assert_eq!(titles(&rows), ["Pay rent"]);
    assert_eq!(rows[0]["tags"], json!("home, bills"));

    let upcoming = params(json!({"today": "2026-11-01", "filter": "upcoming", "q": ""}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &upcoming).unwrap()),
        ["Plan trip"]
    );

    let search = params(json!({"today": "2026-11-01", "filter": "all", "q": "bills"}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &search).unwrap()),
        ["Pay rent"]
    );

    host.call("todo", "toggle", &json!({"id": id, "done": true}))
        .unwrap();
    let done = params(json!({"today": "2026-11-01", "filter": "done", "q": ""}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &done).unwrap()),
        ["Pay rent"]
    );

    host.call("todo", "delete", &json!({"id": id})).unwrap();
    assert_eq!(
        titles(&host.query("todo", "trash", &Map::new()).unwrap()),
        ["Pay rent"]
    );
    host.call("todo", "restore", &json!({"id": id})).unwrap();
    assert!(host.query("todo", "trash", &Map::new()).unwrap().is_empty());

    // Every write went through the change log.
    assert!(host.store("todo").unwrap().changes(100).unwrap().len() >= 8);
}

#[test]
fn todo_validation_errors_reach_the_caller() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();

    let err = host
        .call("todo", "create", &json!({"title": "   "}))
        .unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "invalid"),
        "{err:?}"
    );
    let err = host
        .call("todo", "create", &json!({"title": "x", "due": "tomorrow"}))
        .unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "invalid"),
        "{err:?}"
    );
    let err = host
        .call("todo", "toggle", &json!({"id": "missing", "done": true}))
        .unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "not-found"),
        "{err:?}"
    );
    let err = host
        .call("todo", "drop_everything", &json!({}))
        .unwrap_err();
    assert!(
        matches!(
            err,
            RuntimeError::Undeclared {
                kind: "operation",
                ..
            }
        ),
        "{err:?}"
    );
    let err = host.query("todo", "nope", &Map::new()).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Undeclared { kind: "query", .. }),
        "{err:?}"
    );
}

#[test]
fn todo_templates_are_served() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    let page = host.template("todo", ViewKind::Page, "main").unwrap();
    assert_eq!(page["state"]["filter"], json!("today"));
    assert!(host.template("todo", ViewKind::Widget, "quick-add").is_ok());
    assert!(host.template("todo", ViewKind::Page, "missing").is_err());
}

#[test]
fn failed_or_trapped_operations_roll_back_their_writes() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();

    let err = host
        .call("limits", "insert_then_fail", &json!({}))
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Plugin { .. }), "{err:?}");
    let err = host
        .call("limits", "insert_then_trap", &json!({}))
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Trap(_)), "{err:?}");
    assert!(host.query("limits", "all", &Map::new()).unwrap().is_empty());
    assert!(host
        .store("limits")
        .unwrap()
        .changes(10)
        .unwrap()
        .is_empty());

    // The plugin still works afterwards.
    assert_eq!(
        host.call("limits", "ok", &json!({})).unwrap(),
        json!({"ok": true})
    );
}

#[test]
fn runaway_plugins_are_stopped() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();

    let err = host.call("limits", "spin", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Trap(ref m) if m.contains("fuel")),
        "{err:?}"
    );

    let err = host.call("limits", "grow", &json!({})).unwrap_err();
    assert!(matches!(err, RuntimeError::Trap(_)), "{err:?}");

    let err = host.call("limits", "spam", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "quota"),
        "{err:?}"
    );

    assert_eq!(
        host.call("limits", "write_sql", &json!({})).unwrap(),
        json!(true),
        "write SQL must be refused"
    );
}

#[test]
fn time_limit_stops_an_interpreted_plugin_with_plenty_of_fuel() {
    let data = TempDir::new().unwrap();
    let pkg = data.path().join("slow");
    copy_package(&limits_dir(), &pkg);
    let manifest_path = pkg.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["limits"] = json!({"fuel": 2_000_000_000u64, "memory_mb": 16, "timeout_ms": 100});
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();

    // Native optimized loops can exhaust fuel before the wall deadline.
    // Use the interpreter to isolate epoch interruption from fuel exhaustion.
    let mut host = host_with(&data, Execution::Interpreter);
    host.load(&pkg).unwrap();
    let started = std::time::Instant::now();
    let err = host.call("limits", "spin", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Trap(ref m) if m.contains("time")),
        "{err:?}"
    );
    assert!(started.elapsed().as_millis() < 2_000);
}

#[test]
fn reload_with_additive_migration_keeps_data() {
    let data = TempDir::new().unwrap();
    let pkg = data.path().join("todo-pkg");
    copy_package(&todo_dir(), &pkg);
    let mut host = host(&data);
    host.load(&pkg).unwrap();
    host.call("todo", "create", &json!({"title": "Survives upgrade"}))
        .unwrap();

    let manifest_path = pkg.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let version = manifest["storage"]["version"].as_u64().unwrap();
    manifest["storage"]["version"] = json!(version + 1);
    manifest["storage"]["tables"][0]["columns"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "estimate", "type": "integer"}));
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    host.load(&pkg).unwrap();

    let all = params(json!({"today": "2026-11-01", "filter": "all", "q": ""}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &all).unwrap()),
        ["Survives upgrade"]
    );

    // Dropping a column is refused.
    manifest["storage"]["version"] = json!(version + 2);
    manifest["storage"]["tables"][0]["columns"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["name"] != "notes");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(matches!(
        host.load(&pkg),
        Err(RuntimeError::Core(terrarium_core::Error::Migration(_)))
    ));
}

#[test]
fn bad_packages_are_rejected() {
    let data = TempDir::new().unwrap();
    let pkg = data.path().join("bad");
    copy_package(&todo_dir(), &pkg);
    let mut host = host(&data);

    // Not a component.
    std::fs::write(pkg.join("dist/plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();
    assert!(matches!(host.load(&pkg), Err(RuntimeError::Wasm(_))));

    // Query parameter count must match its declaration.
    copy_package(&todo_dir(), &pkg);
    let manifest_path = pkg.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["queries"]["trash"]["params"] = json!(["extra"]);
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(matches!(host.load(&pkg), Err(RuntimeError::Manifest(_))));

    // Write SQL in a declared query is refused at load.
    manifest["queries"]["trash"] = json!({"sql": "DELETE FROM tasks", "params": []});
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(matches!(host.load(&pkg), Err(RuntimeError::Manifest(_))));

    // Templates must stay inside the package.
    manifest =
        serde_json::from_str(&std::fs::read_to_string(todo_dir().join("manifest.json")).unwrap())
            .unwrap();
    manifest["pages"][0]["template"] = json!("../outside.json");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(matches!(host.load(&pkg), Err(RuntimeError::Manifest(_))));
}

/// The path used on iOS and older Android kernels, where the JIT cannot run.
#[test]
fn interpreter_runs_plugins_with_the_same_limits() {
    let data = TempDir::new().unwrap();
    let mut host = host_with(&data, Execution::Interpreter);
    assert_eq!(host.execution_mode(), "interpreter");
    host.load(&todo_dir()).unwrap();
    host.call(
        "todo",
        "create",
        &json!({"title": "Interpreted", "due": "2026-11-01"}),
    )
    .unwrap();
    let today = params(json!({"today": "2026-11-01", "filter": "today", "q": ""}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &today).unwrap()),
        ["Interpreted"]
    );

    let fuel_pkg = data.path().join("fuel-limits");
    copy_package(&limits_dir(), &fuel_pkg);
    let manifest_path = fuel_pkg.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    // Isolate fuel exhaustion from the separately tested wall-clock deadline.
    // The shared 50-million-fuel fixture can hit the deadline first in debug
    // interpreter builds, even though both guards are functioning correctly.
    manifest["limits"]["fuel"] = json!(1_000_000);
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    host.load(&fuel_pkg).unwrap();
    let err = host.call("limits", "spin", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Trap(ref m) if m.contains("fuel")),
        "{err:?}"
    );
    let err = host
        .call("limits", "insert_then_trap", &json!({}))
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Trap(_)), "{err:?}");
    assert!(host.query("limits", "all", &Map::new()).unwrap().is_empty());
}

#[test]
fn auto_uses_native_code_on_this_machine() {
    let data = TempDir::new().unwrap();
    assert_eq!(host(&data).execution_mode(), "native");
}

#[test]
fn plugins_call_each_other_through_declared_interfaces() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    host.load(&limits_dir()).unwrap();

    let made = host.call("limits", "make_task", &json!({})).unwrap();
    assert!(made["id"].is_string());
    let all = params(json!({"today": "2026-11-01", "filter": "all", "q": ""}));
    assert_eq!(
        titles(&host.query("todo", "tasks", &all).unwrap()),
        ["from another plugin"]
    );

    let err = host.call("limits", "undeclared", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "not-allowed"),
        "{err:?}"
    );
    let err = host.call("limits", "forbidden_op", &json!({})).unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "unavailable"),
        "{err:?}"
    );

    // The provider's write commits on its own; the caller's failure does not undo it.
    host.call("limits", "make_then_fail", &json!({}))
        .unwrap_err();
    assert_eq!(host.query("todo", "tasks", &all).unwrap().len(), 2);

    // Both plugins are back in the host and usable.
    assert_eq!(
        host.call("limits", "ok", &json!({})).unwrap(),
        json!({"ok": true})
    );
    host.call("todo", "create", &json!({"title": "direct"}))
        .unwrap();
}

#[test]
fn completed_tasks_by_local_date_range() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    let id = host
        .call("todo", "create", &json!({"title": "done today"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("todo", "toggle", &json!({"id": id, "done": true}))
        .unwrap();
    let today = host
        .call(
            "todo",
            "completed_between",
            &json!({"start": "2000-01-01", "end": "2100-01-01"}),
        )
        .unwrap();
    assert_eq!(today["tasks"].as_array().unwrap().len(), 1);
    let none = host
        .call(
            "todo",
            "completed_between",
            &json!({"start": "2000-01-01", "end": "2000-01-02"}),
        )
        .unwrap();
    assert!(none["tasks"].as_array().unwrap().is_empty());
}

#[test]
fn oversized_action_input_is_rejected_without_writes_or_losing_the_plugin() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    let error = host
        .call(
            "todo",
            "create",
            &json!({"title":"Should not exist", "extra":"x".repeat(12 * 1024 * 1024)}),
        )
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Plugin { ref code, .. } if code == "quota"));
    assert!(host
        .store("todo")
        .unwrap()
        .query("SELECT id FROM tasks", &[])
        .unwrap()
        .is_empty());
    assert!(host
        .call("todo", "create", &json!({"title":"Still works"}))
        .is_ok());
}

#[test]
fn todo_board_and_matrix_share_filters_and_preserve_workflow() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    let mut ids = Vec::new();
    for (title, urgent, important, lane) in [
        ("Do first #work", true, true, "do"),
        ("Schedule #work", false, true, "schedule"),
        ("Delegate #work", true, false, "delegate"),
        ("Eliminate #home", false, false, "eliminate"),
    ] {
        let task = host
            .call(
                "todo",
                "create",
                &json!({
                    "title":title, "urgent":urgent, "important":important, "due":"2026-11-07"
                }),
            )
            .unwrap();
        ids.push((task["id"].clone(), lane));
    }
    let filters = params(json!({"today":"2026-11-07", "filter":"all", "q":""}));
    let rows = host.query("todo", "tasks", &filters).unwrap();
    assert_eq!(rows.len(), 4);
    for (id, quadrant) in &ids {
        let row = rows.iter().find(|r| r["id"] == *id).unwrap();
        assert_eq!(row["board_lane"], json!("todo"));
        assert_eq!(row["matrix_lane"], json!(quadrant));
    }
    host.call("todo", "update", &json!({"id":ids[0].0,"in_progress":true}))
        .unwrap();
    let rows = host.query("todo", "tasks", &filters).unwrap();
    assert_eq!(
        rows.iter().find(|r| r["id"] == ids[0].0).unwrap()["board_lane"],
        json!("doing")
    );
    let work = params(json!({"today":"2026-11-07", "filter":"today", "q":"work"}));
    assert_eq!(host.query("todo", "tasks", &work).unwrap().len(), 3);
    let upcoming = params(json!({"today":"2026-11-07", "filter":"upcoming", "q":"work"}));
    assert!(host.query("todo", "tasks", &upcoming).unwrap().is_empty());
    host.call("todo", "update", &json!({"id":ids[0].0,"repeat":"daily"}))
        .unwrap();
    host.call("todo", "toggle", &json!({"id":ids[0].0,"done":true}))
        .unwrap();
    host.call("todo", "toggle", &json!({"id":ids[0].0,"done":true}))
        .unwrap();
    let completed = params(json!({"today":"2026-11-07", "filter":"done", "q":"work"}));
    let rows = host.query("todo", "tasks", &completed).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["board_lane"], json!("done"));
    assert_eq!(rows[0]["in_progress"], json!(0));
    let everything = params(json!({"today":"2026-11-07", "filter":"everything", "q":"work"}));
    let all = host.query("todo", "tasks", &everything).unwrap();
    assert_eq!(
        all.len(),
        4,
        "All tasks includes open and completed matches"
    );
    assert!(all.iter().any(|r| r["board_lane"] == json!("done")));
    let rows = host.query("todo", "tasks", &filters).unwrap();
    assert_eq!(
        rows.len(),
        4,
        "one next occurrence even after a completion retry"
    );
    let next = rows
        .iter()
        .find(|r| r["title"] == json!("Do first"))
        .unwrap();
    assert_eq!(next["matrix_lane"], json!("do"));
    assert_eq!(next["board_lane"], json!("todo"));
    host.call("todo", "toggle", &json!({"id":ids[0].0,"done":false}))
        .unwrap();
    let rows = host.query("todo", "tasks", &filters).unwrap();
    assert_eq!(
        rows.iter().find(|r| r["id"] == ids[0].0).unwrap()["board_lane"],
        json!("todo")
    );
    assert!(host
        .call("todo", "update", &json!({"id":ids[1].0,"urgent":"yes"}))
        .is_err());
    host.call("todo", "delete", &json!({"id":ids[1].0}))
        .unwrap();
    assert!(!host
        .query("todo", "tasks", &work)
        .unwrap()
        .iter()
        .any(|r| r["id"] == ids[1].0));
}

#[test]
fn todo_existing_tasks_gain_safe_board_and_matrix_defaults() {
    let data = TempDir::new().unwrap();
    let old = data.path().join("old-todo");
    copy_package(&todo_dir(), &old);
    let manifest = old.join("manifest.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    value["storage"]["version"] = json!(3);
    value["storage"]["tables"][0]["columns"]
        .as_array_mut()
        .unwrap()
        .retain(|c| !["in_progress", "urgent", "important"].contains(&c["name"].as_str().unwrap()));
    value["queries"] = json!({"tasks":{"sql":"SELECT id, title FROM tasks", "params":[]}});
    for field in ["badge", "search", "reminders"] {
        value.as_object_mut().unwrap().remove(field);
    }
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut host = host(&data);
    host.load(&old).unwrap();
    let id = host
        .call("todo", "create", &json!({"title":"Existing task"}))
        .unwrap()["id"]
        .clone();
    host.load(&todo_dir()).unwrap();
    let rows = host
        .query(
            "todo",
            "tasks",
            &params(json!({"today":"2026-11-07","filter":"all","q":""})),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["board_lane"], json!("todo"));
    assert_eq!(rows[0]["matrix_lane"], json!("eliminate"));
}
