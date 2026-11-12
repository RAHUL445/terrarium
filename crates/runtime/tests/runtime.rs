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

fn take_events(host: &mut PluginHost) -> Vec<(String, String, Map<String, Value>)> {
    let events = host.pending_events(1000).unwrap();
    for event in &events {
        host.mark_event_handled(&event.id).unwrap();
    }
    events
        .into_iter()
        .map(|e| (e.plugin, e.name, e.payload))
        .collect()
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
    host_with_device(data, execution, "test-device")
}

fn host_with_device(data: &TempDir, execution: Execution, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: device.into(),
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

fn todo_organization_row(host: &PluginHost, id: &str) -> Map<String, Value> {
    host.query(
        "todo",
        "organization_tasks",
        &params(json!({"id": id})),
    )
    .unwrap()
    .into_iter()
    .next()
    .expect("organization query returns the requested task")
}

fn todo_stored_task(host: &PluginHost, id: &str) -> Map<String, Value> {
    host.store("todo")
        .unwrap()
        .query("SELECT * FROM tasks WHERE id = ?1", &[json!(id)])
        .unwrap()
        .into_iter()
        .next()
        .expect("task row exists")
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
    assert_eq!(page["state"]["filter"], json!("my_day"));
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
fn todo_v4_to_v5_additive_schema_preserves_tasks_and_steps() {
    let data = TempDir::new().unwrap();
    let pkg = data.path().join("todo-pkg");
    copy_package(&todo_dir(), &pkg);
    let manifest_path = pkg.join("manifest.json");
    let v5_manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(v5_manifest["storage"]["version"], json!(5));

    // Model the shipped v4 package against the v5 plugin source. Keep its old
    // operations, but remove every schema addition and all new organization queries.
    let mut v4_manifest = v5_manifest.clone();
    v4_manifest["storage"]["version"] = json!(4);
    let new_task_columns = [
        "list_id",
        "group_id",
        "status_id",
        "previous_status_id",
        "planned_for",
        "sort_order",
    ];
    v4_manifest["storage"]["tables"][0]["columns"]
        .as_array_mut()
        .unwrap()
        .retain(|column| {
            !new_task_columns.contains(&column["name"].as_str().unwrap_or_default())
        });
    v4_manifest["storage"]["tables"]
        .as_array_mut()
        .unwrap()
        .retain(|table| {
            !["task_lists", "task_groups", "task_statuses"]
                .contains(&table["name"].as_str().unwrap_or_default())
        });
    if let Some(queries) = v4_manifest["queries"].as_object_mut() {
        for query in [
            "organization_tasks",
            "task_lists",
            "task_groups",
            "task_statuses",
            "workspace_tasks",
            "task",
            "list_choices",
            "group_choices",
            "status_choices",
            "all_group_choices",
            "all_status_choices",
            "list_detail",
            "group_detail",
            "status_detail",
        ] {
            queries.remove(query);
        }
    }
    std::fs::write(&manifest_path, v4_manifest.to_string()).unwrap();

    let mut host = host(&data);
    host.load(&pkg).unwrap();
    let task = host
        .call(
            "todo",
            "create",
            &json!({
                "title": "Recurring preservation fixture",
                "notes": "Keep these notes byte for byte.",
                "due": "2026-11-20",
                "remind_time": "09:15",
                "priority": 3,
                "repeat": "weekly",
                "tags": "work, migration",
                "in_progress": true,
                "urgent": true,
                "important": true
            }),
        )
        .unwrap();
    let task_id = task["id"].as_str().unwrap();
    let step = host
        .call(
            "todo",
            "add_step",
            &json!({"task_id": task_id, "title": "Preserve this step"}),
        )
        .unwrap();
    host.call(
        "todo",
        "toggle_step",
        &json!({"id": step["id"], "done": true}),
    )
    .unwrap();
    let completion = host
        .call("todo", "toggle", &json!({"id": task_id, "done": true}))
        .unwrap();
    let next_id = completion["next"]["id"].as_str().unwrap();
    host.call(
        "todo",
        "save_note",
        &json!({"id": task_id, "text": "Preserve completion note."}),
    )
    .unwrap();
    host.call("todo", "clear_done", &json!({})).unwrap();

    let trashed = host
        .call("todo", "create", &json!({"title": "Preserve trash state"}))
        .unwrap();
    host.call("todo", "delete", &json!({"id": trashed["id"]}))
        .unwrap();

    let before = host.store("todo").unwrap().dump().unwrap();
    assert_eq!(before["tasks"].len(), 3);
    assert_eq!(before["steps"].len(), 2);
    assert!(before["tasks"].iter().any(|row| row["cleared_at"].is_number()));
    assert!(before["tasks"].iter().any(|row| !row["_deleted"].is_null()));
    let original_step = before["steps"]
        .iter()
        .find(|row| row["task_id"] == task_id)
        .unwrap();
    let repeated_step = before["steps"]
        .iter()
        .find(|row| row["task_id"] == next_id)
        .unwrap();
    assert_eq!(original_step["done"], json!(1));
    assert_eq!(repeated_step["done"], json!(0));

    // Upgrade the same package/store. All old columns, including hidden and
    // trashed state, and every step field must survive exactly; only the six
    // new task columns and three new tables may be introduced.
    std::fs::write(&manifest_path, v5_manifest.to_string()).unwrap();
    host.load(&pkg).unwrap();
    let after = host.store("todo").unwrap().dump().unwrap();
    let mut expected_tasks = before["tasks"].clone();
    for row in &mut expected_tasks {
        row.insert("list_id".into(), Value::Null);
        row.insert("group_id".into(), Value::Null);
        row.insert("status_id".into(), Value::Null);
        row.insert("previous_status_id".into(), Value::Null);
        row.insert("planned_for".into(), Value::Null);
        row.insert("sort_order".into(), json!(0));
    }
    assert_eq!(after["tasks"], expected_tasks);
    assert_eq!(after["steps"], before["steps"]);
    for table in ["task_lists", "task_groups", "task_statuses"] {
        assert!(after[table].is_empty(), "{table}");
    }

    // Loading v5 again must neither rewrite prior records nor seed duplicates.
    host.load(&pkg).unwrap();
    assert_eq!(host.store("todo").unwrap().dump().unwrap(), after);
}

#[test]
fn todo_organization_definitions_create_defaults_and_update_metadata() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();

    let list = host
        .call(
            "todo",
            "create_list",
            &json!({"name": "Projects", "color": "#AABBCC", "position": 8}),
        )
        .unwrap();
    let list_id = list["id"].as_str().unwrap();
    let statuses = host
        .query("todo", "task_statuses", &params(json!({"list_id": list_id})))
        .unwrap();
    assert_eq!(
        statuses.iter().map(|row| row["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["To pick", "In progress", "Waiting", "Done"]
    );
    assert_eq!(statuses[0]["category"], json!("not_started"));
    assert_eq!(statuses[0]["ready"], json!(1));
    assert_eq!(statuses[1]["category"], json!("active"));
    assert_eq!(statuses[1]["ready"], json!(1));
    assert_eq!(statuses[2]["category"], json!("not_started"));
    assert_eq!(statuses[2]["ready"], json!(0));
    assert_eq!(statuses[3]["category"], json!("completed"));
    assert_eq!(statuses[3]["ready"], json!(0));

    let group = host
        .call(
            "todo",
            "create_group",
            &json!({"name": "Planning", "list_id": list_id, "color": "#123456", "position": 4}),
        )
        .unwrap();
    let group_id = group["id"].as_str().unwrap();
    let custom_status = host
        .call(
            "todo",
            "create_status",
            &json!({"name": "Reviewing", "list_id": list_id, "category": "active", "ready": true, "position": 5}),
        )
        .unwrap();
    let status_id = custom_status["id"].as_str().unwrap();

    host.call(
        "todo",
        "update_list",
        &json!({"id": list_id, "name": "Renamed projects", "color": "#445566", "position": 1}),
    )
    .unwrap();
    host.call(
        "todo",
        "update_group",
        &json!({"id": group_id, "name": "Active planning", "color": "#654321", "position": 2}),
    )
    .unwrap();
    host.call(
        "todo",
        "update_status",
        &json!({"id": status_id, "name": "In review", "color": "#abcdef", "position": 3}),
    )
    .unwrap();

    let lists = host.query("todo", "task_lists", &Map::new()).unwrap();
    let saved_list = lists.iter().find(|row| row["id"] == list_id).unwrap();
    assert_eq!(saved_list["name"], json!("Renamed projects"));
    assert_eq!(saved_list["color"], json!("#445566"));
    assert_eq!(saved_list["position"], json!(1));
    let groups = host
        .query("todo", "task_groups", &params(json!({"list_id": list_id})))
        .unwrap();
    let saved_group = groups.iter().find(|row| row["id"] == group_id).unwrap();
    assert_eq!(saved_group["name"], json!("Active planning"));
    assert_eq!(saved_group["color"], json!("#654321"));
    assert_eq!(saved_group["position"], json!(2));
    let statuses = host
        .query("todo", "task_statuses", &params(json!({"list_id": list_id})))
        .unwrap();
    let saved_status = statuses.iter().find(|row| row["id"] == status_id).unwrap();
    assert_eq!(saved_status["name"], json!("In review"));
    assert_eq!(saved_status["category"], json!("active"));
    assert_eq!(saved_status["color"], json!("#abcdef"));
    assert_eq!(saved_status["position"], json!(3));
}

#[test]
fn todo_organization_recurring_completion_reopen_and_legacy_moves_stay_coherent() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();

    let list_id = host
        .call("todo", "create_list", &json!({"name": "Studio"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let group_id = host
        .call(
            "todo",
            "create_group",
            &json!({"name": "Launch", "list_id": list_id}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let active_id = host
        .call(
            "todo",
            "create_status",
            &json!({"name": "Reviewing", "list_id": list_id, "category": "active", "ready": true}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let done_id = host
        .call(
            "todo",
            "create_status",
            &json!({"name": "Shipped", "list_id": list_id, "category": "completed", "ready": false}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let task_id = host
        .call(
            "todo",
            "create",
            &json!({
                "title": "Recurring organized task",
                "due": "2099-01-01",
                "remind_time": "09:15",
                "repeat": "daily",
                "list_id": list_id,
                "group_id": group_id,
                "status_id": active_id
            }),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let step_id = host
        .call(
            "todo",
            "add_step",
            &json!({"task_id": task_id, "title": "Repeat this step"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("todo", "toggle_step", &json!({"id": step_id, "done": true}))
        .unwrap();
    host.call(
        "todo",
        "update",
        &json!({"id": task_id, "planned_for": "2027-02-03", "sort_order": 12}),
    )
    .unwrap();

    let before_planning = todo_stored_task(&host, &task_id);
    assert_eq!(before_planning["planned_for"], json!("2027-02-03"));
    host.call(
        "todo",
        "update",
        &json!({"id": task_id, "planned_for": ""}),
    )
    .unwrap();
    let cleared_planning = todo_stored_task(&host, &task_id);
    assert!(cleared_planning["planned_for"].is_null());
    for key in ["due", "remind_at"] {
        assert_eq!(cleared_planning[key], before_planning[key], "{key}");
    }
    host.call(
        "todo",
        "update",
        &json!({"id": task_id, "planned_for": "2027-02-03"}),
    )
    .unwrap();

    let completion = host
        .call(
            "todo",
            "organize",
            &json!({"id": task_id, "status_id": done_id}),
        )
        .unwrap();
    let next_id = completion["completion"]["next"]["id"]
        .as_str()
        .expect("status completion returns the recurring successor")
        .to_string();
    let next_org = todo_organization_row(&host, &next_id);
    assert_eq!(next_org["list_id"], json!(list_id));
    assert_eq!(next_org["group_id"], json!(group_id));
    assert_eq!(next_org["status_name"], json!("To pick"));
    let next_task = todo_stored_task(&host, &next_id);
    assert!(next_task["planned_for"].is_null());
    let next_steps = host
        .store("todo")
        .unwrap()
        .query("SELECT done FROM steps WHERE task_id = ?1 AND _deleted IS NULL", &[json!(next_id)])
        .unwrap();
    assert_eq!(next_steps.len(), 1);
    assert_eq!(next_steps[0]["done"], json!(0));
    let task_count = host.store("todo").unwrap().dump().unwrap()["tasks"].len();

    // Retrying the same completed status must not create another successor.
    host.call(
        "todo",
        "organize",
        &json!({"id": task_id, "status_id": done_id}),
    )
    .unwrap();
    assert_eq!(host.store("todo").unwrap().dump().unwrap()["tasks"].len(), task_count);

    host.call("todo", "toggle", &json!({"id": task_id, "done": false}))
        .unwrap();
    let reopened = todo_stored_task(&host, &task_id);
    assert_eq!(reopened["previous_status_id"], Value::Null);
    assert_eq!(todo_organization_row(&host, &task_id)["status_name"], json!("Reviewing"));
    host.call(
        "todo",
        "update_status",
        &json!({"id": active_id, "name": "In review"}),
    )
    .unwrap();
    assert_eq!(todo_organization_row(&host, &task_id)["status_name"], json!("In review"));

    let legacy = host
        .call(
            "todo",
            "create",
            &json!({"title": "Legacy board controls", "list_id": list_id, "status_id": active_id}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("todo", "move", &json!({"id": legacy, "lane": "todo"}))
        .unwrap();
    assert_eq!(todo_organization_row(&host, &legacy)["status_name"], json!("To pick"));
    host.call("todo", "move", &json!({"id": legacy, "lane": "doing"}))
        .unwrap();
    assert_eq!(todo_organization_row(&host, &legacy)["status_category"], json!("active"));
}

#[test]
fn todo_organization_rejects_invalid_targets_atomically_and_keeps_archived_refs() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();

    let first_list = host
        .call("todo", "create_list", &json!({"name": "First"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let second_list = host
        .call("todo", "create_list", &json!({"name": "Second"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let archived_list = host
        .call("todo", "create_list", &json!({"name": "Archived"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let group_id = host
        .call(
            "todo",
            "create_group",
            &json!({"name": "First group", "list_id": first_list}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let status_id = host
        .call(
            "todo",
            "create_status",
            &json!({"name": "Custom active", "list_id": first_list, "category": "active"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let task_id = host
        .call(
            "todo",
            "create",
            &json!({
                "title": "Validate organization targets",
                "list_id": first_list,
                "group_id": group_id,
                "status_id": status_id
            }),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(host
        .call(
            "todo",
            "update_status",
            &json!({"id": status_id, "category": "completed"}),
        )
        .is_err());
    assert_eq!(todo_stored_task(&host, &task_id)["status_id"], json!(status_id));

    let tasks_before_invalid_create = host.store("todo").unwrap().dump().unwrap()["tasks"].len();
    assert!(host
        .call(
            "todo",
            "create",
            &json!({
                "title": "Wrong-scope create must roll back",
                "list_id": second_list,
                "group_id": group_id
            }),
        )
        .is_err());
    assert_eq!(
        host.store("todo").unwrap().dump().unwrap()["tasks"].len(),
        tasks_before_invalid_create
    );

    let before_invalid_move = todo_stored_task(&host, &task_id);
    assert!(host
        .call(
            "todo",
            "organize",
            &json!({"id": task_id, "list_id": second_list, "group_id": group_id}),
        )
        .is_err());
    assert_eq!(todo_stored_task(&host, &task_id), before_invalid_move);

    // Archiving an assigned definition preserves its reference. Metadata edits
    // to those archived definitions remain possible, while assigning them anew
    // is rejected.
    host.call("todo", "update_group", &json!({"id": group_id, "archived": true}))
        .unwrap();
    host.call(
        "todo",
        "update_status",
        &json!({"id": status_id, "archived": true}),
    )
    .unwrap();
    host.call(
        "todo",
        "update_group",
        &json!({"id": group_id, "name": "Archived but retained"}),
    )
    .unwrap();
    host.call(
        "todo",
        "update_status",
        &json!({"id": status_id, "name": "Archived status retained"}),
    )
    .unwrap();
    assert_eq!(todo_stored_task(&host, &task_id)["group_id"], json!(group_id));
    assert_eq!(todo_stored_task(&host, &task_id)["status_id"], json!(status_id));
    host.call(
        "todo",
        "organize",
        &json!({"id": task_id, "group_id": ""}),
    )
    .unwrap();
    let ungrouped = todo_stored_task(&host, &task_id);
    assert!(ungrouped["group_id"].is_null());
    assert_eq!(ungrouped["status_id"], json!(status_id));
    assert!(host
        .call("todo", "organize", &json!({"id": task_id, "group_id": group_id}))
        .is_err());
    assert!(host
        .call("todo", "organize", &json!({"id": task_id, "status_id": status_id}))
        .is_err());

    host.call(
        "todo",
        "organize",
        &json!({"id": task_id, "list_id": second_list}),
    )
    .unwrap();
    let moved = todo_organization_row(&host, &task_id);
    assert_eq!(moved["list_id"], json!(second_list));
    assert!(moved["group_id"].is_null());
    assert_eq!(moved["status_name"], json!("In progress"));

    host.call(
        "todo",
        "update_list",
        &json!({"id": archived_list, "archived": true}),
    )
    .unwrap();
    let before_archived_list = todo_stored_task(&host, &task_id);
    assert!(host
        .call(
            "todo",
            "organize",
            &json!({"id": task_id, "list_id": archived_list}),
        )
        .is_err());
    assert_eq!(todo_stored_task(&host, &task_id), before_archived_list);
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
fn todo_move_sets_lanes_and_quadrants() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&todo_dir()).unwrap();
    let id = host
        .call(
            "todo",
            "create",
            &json!({"title":"View move check","due":"2026-11-07"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let list = |host: &PluginHost, q: &str| {
        host.query(
            "todo",
            "tasks",
            &params(json!({"today":"2026-11-07", "filter":"everything", "q":q})),
        )
        .unwrap()
    };
    let row = |rows: &[Map<String, Value>], id: &str| {
        rows.iter().find(|r| r["id"] == json!(id)).unwrap().clone()
    };
    let apply_move = |host: &mut PluginHost, id: &str, lane: &str| match host.call(
        "todo",
        "move",
        &json!({"id": id, "lane": lane}),
    ) {
        ok @ Ok(_) => ok,
        Err(RuntimeError::Plugin { ref code, .. }) if code == "unknown-operation" => {
            match lane {
                "todo" => host.call("todo", "toggle", &json!({"id": id, "done": false}))?,
                "doing" => host.call(
                    "todo",
                    "update",
                    &json!({"id": id, "in_progress": true, "done": false}),
                )?,
                "done" => host.call("todo", "toggle", &json!({"id": id, "done": true}))?,
                "do" => host.call(
                    "todo",
                    "update",
                    &json!({"id": id, "urgent": true, "important": true}),
                )?,
                "schedule" => host.call(
                    "todo",
                    "update",
                    &json!({"id": id, "urgent": false, "important": true}),
                )?,
                "delegate" => host.call(
                    "todo",
                    "update",
                    &json!({"id": id, "urgent": true, "important": false}),
                )?,
                "eliminate" => host.call(
                    "todo",
                    "update",
                    &json!({"id": id, "urgent": false, "important": false}),
                )?,
                _ => {
                    return Err(RuntimeError::Plugin {
                        code: "invalid".into(),
                        message: "Unknown lane".into(),
                    })
                }
            };
            Ok(Value::Null)
        }
        Err(err) => Err(err),
    };

    apply_move(&mut host, &id, "doing").unwrap();
    let doing = row(&list(&host, "View move check"), &id);
    assert_eq!(doing["board_lane"], json!("doing"));

    apply_move(&mut host, &id, "done").unwrap();
    let done = row(&list(&host, "View move check"), &id);
    assert_eq!(done["board_lane"], json!("done"));
    assert_eq!(done["in_progress"], json!(0));

    apply_move(&mut host, &id, "todo").unwrap();
    let todo = row(&list(&host, "View move check"), &id);
    assert_eq!(todo["board_lane"], json!("todo"));
    assert_eq!(todo["done"], json!(0));

    apply_move(&mut host, &id, "schedule").unwrap();
    let schedule = row(&list(&host, "View move check"), &id);
    assert_eq!(schedule["matrix_lane"], json!("schedule"));

    apply_move(&mut host, &id, "eliminate").unwrap();
    let eliminate = row(&list(&host, "View move check"), &id);
    assert_eq!(eliminate["matrix_lane"], json!("eliminate"));

    let repeated = host
        .call(
            "todo",
            "create",
            &json!({"title":"View repeat move","due":"2026-11-07","repeat":"daily"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    apply_move(&mut host, &repeated, "done").unwrap();
    apply_move(&mut host, &repeated, "done").unwrap();
    assert_eq!(list(&host, "View repeat move").len(), 2);

    let before = row(&list(&host, "View move check"), &id);
    let err = apply_move(&mut host, &id, "later").unwrap_err();
    assert!(
        matches!(err, RuntimeError::Plugin { ref code, .. } if code == "invalid"),
        "{err:?}"
    );
    let after = row(&list(&host, "View move check"), &id);
    assert_eq!(after["board_lane"], before["board_lane"]);
    assert_eq!(after["matrix_lane"], before["matrix_lane"]);
    assert_eq!(after["done"], before["done"]);
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
    for field in [
        "badge",
        "search",
        "reminders",
        "index",
        "refs",
        "export",
        "import",
    ] {
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

#[test]
fn emitted_events_are_kept_only_when_the_call_commits() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    host.call("limits", "emit_ok", &json!({})).unwrap();
    let events = take_events(&mut host);
    assert_eq!(
        events,
        vec![("limits".into(), "pinged".into(), params(json!({"n":1})))]
    );
    for op in ["emit_then_fail", "emit_then_trap", "emit_many"] {
        assert!(host.call("limits", op, &json!({})).is_err());
        assert!(take_events(&mut host).is_empty());
    }
}
#[test]
fn dry_run_emits_nothing() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    host.call_traced("personal", "limits", "emit_ok", &json!({}), true)
        .unwrap();
    assert!(take_events(&mut host).is_empty());
}
#[test]
fn undeclared_and_malformed_events_are_rejected() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    for op in ["emit_undeclared", "emit_bad_payload"] {
        assert!(
            matches!(host.call("limits",op,&json!({})),Err(RuntimeError::Plugin {code,..}) if code=="invalid")
        );
    }
    for payload in [
        json!(null),
        json!([]),
        json!({}),
        json!({"n":null}),
        json!({"n":true}),
        json!({"n":1.5}),
        json!({"n":1,"plugin":"forged"}),
        json!({"n":1,"space":"forged"}),
    ] {
        assert!(
            matches!(host.call("limits","emit_payload",&json!({"payload":payload})),Err(RuntimeError::Plugin {code,..}) if code=="invalid")
        );
    }
    assert!(
        matches!(host.call("limits","emit_payload",&json!({"payload":{"n":1,"extra":"x".repeat(16384)}})),Err(RuntimeError::Plugin {code,..}) if code=="quota")
    );
    assert!(take_events(&mut host).is_empty());
}
#[test]
fn too_many_events_hit_the_quota() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    host.call("limits", "emit_count", &json!({"count":50}))
        .unwrap();
    assert_eq!(take_events(&mut host).len(), 50);
    assert!(
        matches!(host.call("limits","emit_many",&json!({})),Err(RuntimeError::Plugin {code,..}) if code=="quota")
    );
    assert!(take_events(&mut host).is_empty());
    host.call("limits", "emit_ok", &json!({})).unwrap();
    assert_eq!(take_events(&mut host).len(), 1);
}

#[test]
fn nested_commits_survive_outer_failure_and_share_the_budget() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    let child = TempDir::new().unwrap();
    copy_package(&limits_dir(), child.path());
    let path = child.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["id"] = json!("event-child");
    manifest["provides"] = json!({"event_sink":{"operations":["emit_count"]}});
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    host.load(child.path()).unwrap();
    for op in [
        "emit_child",
        "emit_child_then_fail",
        "emit_child_budget",
        "emit_child_then_trap",
    ] {
        let result = host.call("limits", op, &json!({}));
        if op == "emit_child" {
            result.unwrap();
        } else {
            assert!(result.is_err());
        }
        let pending = host.pending_events(100).unwrap();
        assert!(pending.iter().all(|e| e.space == "personal"
            && e.plugin == "event-child"
            && e.depth == 0
            && e.root == e.id));
        let events = take_events(&mut host);
        assert_eq!(events.len(), 50);
        assert!(events
            .iter()
            .all(|(plugin, name, payload)| plugin == "event-child"
                && name == "pinged"
                && payload == &params(json!({"n":1}))));
    }
    assert!(
        matches!(host.call("limits", "emit_failed_child_budget", &json!({})), Err(RuntimeError::Plugin {code,..}) if code=="quota")
    );
    assert!(take_events(&mut host).is_empty());
    host.call_traced("personal", "limits", "emit_child", &json!({}), true)
        .unwrap();
    assert!(take_events(&mut host).is_empty());
}

#[test]
fn unused_events_import_is_eliminated_by_the_final_sdk() {
    let imports = |dir: PathBuf| {
        let bytes = std::fs::read(dir.join("dist/plugin.wasm")).unwrap();
        let wit_component::DecodedWasm::Component(resolve, world) =
            wit_component::decode(&bytes).unwrap()
        else {
            panic!("expected a component");
        };
        resolve.worlds[world]
            .imports
            .keys()
            .map(|key| resolve.name_world_key(key))
            .collect::<Vec<_>>()
    };
    let used = imports(limits_dir());
    // To-do now emits events; Bookmarks still exercises an unused SDK import.
    let unused_package = support::package_root().join("bookmarks");
    build_plugin(&unused_package).unwrap();
    let unused = imports(unused_package);
    println!("used imports: {used:?}; unused imports: {unused:?}");
    assert!(used
        .iter()
        .any(|name| name == "terrarium:plugin/events@0.1.0"));
    assert!(!unused
        .iter()
        .any(|name| name.starts_with("terrarium:plugin/events@")));
}

#[test]
fn event_payload_types_bounds_optional_fields_and_size_boundary() {
    let package = TempDir::new().unwrap();
    copy_package(&limits_dir(), package.path());
    let path = package.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["events"]["pinged"]["payload"] = json!({
        "n":{"type":"integer","required":true,"min":0,"max":5},
        "status":{"type":"enum","values":["ok","waiting"]},
        "day":{"type":"date"},"flag":{"type":"boolean"},"blob":{"type":"string"}
    });
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(package.path()).unwrap();
    for payload in [
        json!({"n":0}),
        json!({"n":5,"status":null,"day":null,"flag":null}),
        json!({"n":1,"status":"ok","day":"2026-11-11","flag":true}),
    ] {
        host.call("limits", "emit_payload", &json!({"payload":payload}))
            .unwrap();
        assert_eq!(take_events(&mut host).len(), 1);
    }
    for payload in [
        json!({"n":-1}),
        json!({"n":6}),
        json!({"n":1,"status":"secret-bad-value"}),
        json!({"n":1,"day":"secret-bad-value"}),
        json!({"n":1,"flag":"secret-bad-value"}),
    ] {
        let error = host
            .call("limits", "emit_payload", &json!({"payload":payload}))
            .unwrap_err();
        assert!(matches!(&error,RuntimeError::Plugin {code,..} if code=="invalid"));
        assert!(!error.to_string().contains("secret-bad-value"));
        assert!(take_events(&mut host).is_empty());
    }
    let overhead = json!({"n":1,"blob":""}).to_string().len();
    let exact = json!({"n":1,"blob":"x".repeat(16384-overhead)});
    assert_eq!(exact.to_string().len(), 16384);
    host.call("limits", "emit_payload", &json!({"payload":exact}))
        .unwrap();
    assert_eq!(take_events(&mut host).len(), 1);
    let over = json!({"n":1,"blob":"x".repeat(16385-overhead)});
    assert!(
        matches!(host.call("limits","emit_payload",&json!({"payload":over})),Err(RuntimeError::Plugin {code,..}) if code=="quota")
    );
    assert!(take_events(&mut host).is_empty());
}

#[test]
fn committed_events_reach_the_log_with_depth_zero() {
    use terrarium_core::StoreSet;
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    assert!(host.plugin_ids().contains(&"_events".into()));
    host.call("limits", "emit_ok", &json!({})).unwrap();
    let events = host.pending_events(10).unwrap();
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(
        (&*e.plugin, &*e.name, e.depth, &*e.space),
        ("limits", "pinged", 0, "personal")
    );
    assert_eq!(e.payload, params(json!({"n":1})));
    assert_eq!(e.device, "test-device");
    assert!(e.id.starts_with("ev-test-device-"));
    assert_eq!(e.root, e.id);
    assert_eq!(e.contract.len(), 64);
    let date = jiff::Timestamp::from_millisecond(e.emitted_ms as i64)
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::get("Asia/Kolkata").unwrap())
        .date()
        .to_string();
    assert_eq!(e.date, date);
    assert!(host.pending_events(0).unwrap().is_empty());
    host.mark_event_handled(&e.id).unwrap();
    assert!(host.pending_events(10).unwrap().is_empty());
}

#[test]
fn failed_and_dry_run_calls_leave_the_log_empty() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    for op in ["emit_then_fail", "emit_then_trap", "emit_many"] {
        assert!(host.call("limits", op, &json!({})).is_err());
        assert!(host.pending_events(10).unwrap().is_empty());
    }
    host.call_traced("personal", "limits", "emit_ok", &json!({}), true)
        .unwrap();
    assert!(host.pending_events(10).unwrap().is_empty());
}

#[test]
fn event_insert_failure_does_not_roll_back_or_repeat_plugin_commit() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    // Force an actual SQLite insert failure after the independent plugin commit.
    let connection = rusqlite::Connection::open(data.path().join("plugins/_events.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'secret-private-value'); END;").unwrap();
    let result = host.call("limits", "emit_write", &json!({})).unwrap();
    connection
        .execute_batch("DROP TRIGGER reject_event")
        .unwrap();
    assert!(host.pending_events(10).unwrap().is_empty());
    let rows = host.query("limits", "all", &Map::new()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], result["id"]);
}

#[test]
fn imported_event_payload_is_revalidated_without_disclosing_values() {
    use terrarium_core::StoreSet;
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.load(&limits_dir()).unwrap();
    host.call("limits", "emit_ok", &json!({})).unwrap();
    let id = host.pending_events(1).unwrap()[0].id.clone();
    host.store_mut("_events")
        .unwrap()
        .write(|w| {
            w.update(
                "events",
                &id,
                json!({"payload":"{\"n\":\"secret-private-value\"}"})
                    .as_object()
                    .unwrap(),
            )
        })
        .unwrap();
    assert!(host.pending_events(10).unwrap().is_empty());
}

#[test]
fn older_peer_parks_events_and_replays_after_upgrade() {
    use terrarium_core::{Store, StoreSet};
    use terrarium_sync::{DeviceKeys, MemoryStore, Roster, SpaceKey, SpaceSync};
    // Simulate an old host which knows all stores except _events.
    struct Older<'a>(&'a mut PluginHost);
    impl StoreSet for Older<'_> {
        fn plugin_ids(&self) -> Vec<String> {
            self.0
                .plugin_ids()
                .into_iter()
                .filter(|id| id != "_events")
                .collect()
        }
        fn store_mut(&mut self, id: &str) -> Option<&mut Store> {
            if id == "_events" {
                None
            } else {
                self.0.store_mut(id)
            }
        }
    }
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let mut source = host_with_device(&a, Execution::Auto, "source");
    let mut destination = host_with_device(&b, Execution::Auto, "destination");
    source.load(&limits_dir()).unwrap();
    destination.load(&limits_dir()).unwrap();
    let key = SpaceKey::generate();
    let ka = DeviceKeys::generate("source".into());
    let kb = DeviceKeys::generate("destination".into());
    let roster: Roster = [&ka, &kb]
        .into_iter()
        .map(|k| (k.device.clone(), k.public(&k.device.0)))
        .collect();
    let mut sa = SpaceSync::open(
        "personal",
        ka,
        key.clone(),
        roster.clone(),
        &a.path().join("sync"),
    )
    .unwrap();
    let mut sb = SpaceSync::open("personal", kb, key, roster, &b.path().join("sync")).unwrap();
    source.call("limits", "emit_ok", &json!({})).unwrap();
    let original = source.pending_events(10).unwrap();
    let transport = MemoryStore::new();
    sa.sync(&mut source, &transport).unwrap();
    let parked = sb.sync(&mut Older(&mut destination), &transport).unwrap();
    assert!(parked.rejected.is_empty());
    assert!(parked.pending > 0);
    assert!(destination.pending_events(10).unwrap().is_empty());
    // Once the host exposes _events, its parked encrypted batches are replayed.
    let replayed = sb.sync(&mut destination, &transport).unwrap();
    assert!(replayed.rejected.is_empty());
    assert!(replayed.applied > 0);
    assert_eq!(destination.pending_events(10).unwrap(), original);
}

fn rules_host(data: &TempDir) -> PluginHost {
    let mut host = host(data);
    let dir = data.path().join("rule-fixture");
    copy_package(&limits_dir(), &dir);
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["events"]["pinged"]["payload"] = json!({
        "n":{"type":"integer","required":true}, "optional":{"type":"integer"},
        "tags":{"type":"string"}, "day":{"type":"date","required":true},
        "status":{"type":"enum","values":["done","skip"],"required":true},
        "flag":{"type":"boolean","required":true}
    });
    m["operations"]["ok"] = json!({"effect":"write","params":{
        "n":{"type":"number","required":true}, "day":{"type":"date"},
        "text":{"type":"string"}, "status":{"type":"enum","values":["done"]},
        "nested":{"type":"json"}
    }});
    for (name, def) in [
        ("read", json!({"effect":"read","params":{}})),
        ("destroy", json!({"effect":"destructive","params":{}})),
        ("on_callback", json!({"effect":"write","params":{}})),
        ("hidden", json!({"effect":"write","params":{},"cli":false})),
        (
            "confirm",
            json!({"effect":"write","params":{},"confirm":"Confirm"}),
        ),
        ("untyped", json!({"effect":"write"})),
    ] {
        m["operations"][name] = def;
    }
    std::fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
    host
}
fn rule_input() -> terrarium_runtime::rules::RuleInput {
    serde_json::from_value(json!({"name":"Gym","trigger":{"plugin":"limits","event":"pinged"},"action":{"plugin":"limits","operation":"ok","args":{"n":"{{event.n}}"}}})).unwrap()
}
#[test]
fn rule_check_rejects_unknown_events_fields_and_destructive_targets() {
    use terrarium_runtime::rules::check;
    let data = TempDir::new().unwrap();
    let host = rules_host(&data);
    let mut r = rule_input();
    r.trigger.event = "missing".into();
    assert!(check(&host, &r).unwrap_err().contains("event"));
    r = rule_input();
    r.trigger.plugin = "missing".into();
    assert!(check(&host, &r).unwrap_err().contains("plugin"));
    r = rule_input();
    r.filter =
        serde_json::from_value(json!([{"field":"missing","op":"exists","value":true}])).unwrap();
    assert!(check(&host, &r).unwrap_err().contains("field"));
    for op in [
        "read",
        "destroy",
        "on_callback",
        "hidden",
        "confirm",
        "untyped",
    ] {
        r = rule_input();
        r.action.operation = op.into();
        assert!(check(&host, &r).is_err(), "{op}");
    }
    for (field, op, value) in [
        ("n", "contains", json!("1")),
        ("tags", "gt", json!(2)),
        ("n", "exists", json!("true")),
        ("tags", "has_tag", json!("two tags")),
        ("n", "eq", json!("1")),
    ] {
        r = rule_input();
        r.filter = serde_json::from_value(json!([{"field":field,"op":op,"value":value}])).unwrap();
        assert!(check(&host, &r).is_err(), "{field} {op}");
    }
}
#[test]
fn rule_check_accepts_bindings_that_match_param_types() {
    use terrarium_runtime::rules::check;
    let data = TempDir::new().unwrap();
    let host = rules_host(&data);
    let mut r = rule_input();
    r.action.args.insert("day".into(), json!("{{event.$date}}"));
    r.action
        .args
        .insert("text".into(), json!("{{event.$plugin}}"));
    r.action
        .args
        .insert("nested".into(), json!({"literal":"{{event.missing}}"}));
    assert!(check(&host, &r).is_ok());
    for binding in [
        "{{event.day}}",
        "{{event.optional}}",
        "{{event.flag}}",
        "{{event.unknown}}",
        "prefix {{event.n}}",
        "{{event.n}} suffix",
        "{{event.n}}}}",
    ] {
        let mut r = rule_input();
        r.action.args.insert("n".into(), json!(binding));
        assert!(check(&host, &r).is_err(), "{binding}");
    }
    r = rule_input();
    r.action
        .args
        .insert("status".into(), json!("{{event.status}}"));
    assert!(check(&host, &r).is_err(), "enum subset");
    r = rule_input();
    r.action.args.insert("n".into(), json!(42));
    assert!(check(&host, &r).is_ok());
    r.action.args.insert("n".into(), json!("42"));
    assert!(check(&host, &r).is_err());
}
#[test]
fn rule_list_command_is_read_only() {
    use terrarium_core::StoreSet;
    use terrarium_runtime::{
        commands::{self, Caller, Kind, Scope},
        services,
    };
    let data = TempDir::new().unwrap();
    let mut host = rules_host(&data);
    assert!(host.plugin_ids().contains(&"_rules".into()));
    assert!(StoreSet::store_mut(&mut host, "_rules").is_some());
    let work = host.spaces_mut().create("Work").unwrap();
    host.sync_spaces().unwrap();
    let rule = services::invoke(
        &mut host,
        "rule_create",
        &json!({"space":"personal","rule":rule_input()}),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(rule["approved"], false);
    for cmd in [
        "rule_update",
        "rule_set_enabled",
        "rule_remove",
        "rule_review",
        "rule_approve",
    ] {
        let mut args = json!({"space":work.id,"id":rule["id"]});
        if cmd == "rule_update" {
            args["rule"] = json!(rule_input());
        }
        if cmd == "rule_set_enabled" {
            args["enabled"] = json!(false);
        }
        if cmd == "rule_approve" {
            args["digest"] = json!("stale");
        }
        assert!(
            services::invoke(&mut host, cmd, &args, true)
                .unwrap()
                .unwrap_err()
                .contains("space"),
            "{cmd}"
        );
    }

    for caller in [Caller::Client(Scope::Read), Caller::Client(Scope::Change)] {
        let listed = commands::invoke(
            &mut host,
            caller,
            Kind::Query,
            "rule.list",
            Some("personal"),
            &json!({}),
            true,
        )
        .unwrap();
        assert_eq!(listed["rules"].as_array().unwrap().len(), 1);
        assert!(!listed.to_string().contains("source_package"));
        for name in [
            "rule.create",
            "rule.update",
            "rule.approve",
            "rule.remove",
            "rule.review",
            "rule_create",
        ] {
            assert_eq!(
                commands::invoke(
                    &mut host,
                    caller,
                    Kind::Action,
                    name,
                    Some("personal"),
                    &json!({}),
                    true
                )
                .unwrap_err()
                .code,
                "not-found"
            );
        }
        assert!(commands::invoke(
            &mut host,
            caller,
            Kind::Query,
            "rule.runs",
            Some("personal"),
            &json!({"limit":51}),
            true
        )
        .is_err());
    }
    let mut m: Value =
        serde_json::from_slice(&std::fs::read(limits_dir().join("manifest.json")).unwrap())
            .unwrap();
    m["id"] = json!("rule");
    assert!(
        terrarium_runtime::Manifest::parse(&serde_json::to_vec(&m).unwrap())
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    assert!(services::invoke(
        &mut host,
        "rules",
        &json!({"space":"personal","unknown":true}),
        true
    )
    .unwrap()
    .is_err());
}
#[test]
fn rule_review_tokens_bind_loaded_packages_and_current_content() {
    use terrarium_runtime::services;
    let data = TempDir::new().unwrap();
    let mut host = rules_host(&data);
    let invoke = |host: &mut PluginHost, cmd: &str, args: Value| {
        services::invoke(host, cmd, &args, true).unwrap()
    };
    let r = invoke(
        &mut host,
        "rule_create",
        json!({"space":"personal","rule":rule_input()}),
    )
    .unwrap();
    let id = r["id"].as_str().unwrap();
    let review = invoke(
        &mut host,
        "rule_review",
        json!({"space":"personal","id":id}),
    )
    .unwrap();
    let approved = invoke(
        &mut host,
        "rule_approve",
        json!({"space":"personal","id":id,"digest":review["digest"]}),
    )
    .unwrap();
    assert_eq!(approved["approved"], true);
    let mut next = rule_input();
    next.name = "Rename".into();
    assert_eq!(
        invoke(
            &mut host,
            "rule_update",
            json!({"space":"personal","id":id,"rule":next})
        )
        .unwrap()["approved"],
        true
    );
    next.action.args.insert("n".into(), json!(2));
    invoke(
        &mut host,
        "rule_update",
        json!({"space":"personal","id":id,"rule":next}),
    )
    .unwrap();
    assert!(invoke(
        &mut host,
        "rule_approve",
        json!({"space":"personal","id":id,"digest":review["digest"]})
    )
    .is_err());
    // Restoring the mapping restores its content approval, but a package reload revokes it.
    invoke(
        &mut host,
        "rule_update",
        json!({"space":"personal","id":id,"rule":rule_input()}),
    )
    .unwrap();
    let path = data.path().join("rule-fixture/manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["name"] = json!("Updated fixture");
    std::fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&data.path().join("rule-fixture")).unwrap();
    let rule = host.rules().get(id).unwrap().unwrap();
    assert!(!host.rule_info(rule).unwrap().approved);
    host.unload("limits");
    let info = host
        .rule_info(host.rules().get(id).unwrap().unwrap())
        .unwrap();
    assert!(info.problem.is_some());
    assert!(!info.approved);
    assert!(invoke(&mut host, "rule_remove", json!({"space":"missing","id":id})).is_err());
}

#[test]
fn older_peer_parks_rules_and_replays_after_upgrade() {
    use terrarium_core::{Store, StoreSet};
    use terrarium_sync::{DeviceKeys, MemoryStore, Roster, SpaceKey, SpaceSync};
    // Simulate an old host which knows all stores except _rules.
    struct Older<'a>(&'a mut PluginHost);
    impl StoreSet for Older<'_> {
        fn plugin_ids(&self) -> Vec<String> {
            self.0
                .plugin_ids()
                .into_iter()
                .filter(|id| id != "_rules")
                .collect()
        }
        fn store_mut(&mut self, id: &str) -> Option<&mut Store> {
            if id == "_rules" {
                None
            } else {
                self.0.store_mut(id)
            }
        }
    }
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let mut source = host_with_device(&a, Execution::Auto, "source");
    let mut destination = host_with_device(&b, Execution::Auto, "destination");
    source.load(&limits_dir()).unwrap();
    destination.load(&limits_dir()).unwrap();
    let key = SpaceKey::generate();
    let ka = DeviceKeys::generate("source".into());
    let kb = DeviceKeys::generate("destination".into());
    let roster: Roster = [&ka, &kb]
        .into_iter()
        .map(|k| (k.device.clone(), k.public(&k.device.0)))
        .collect();
    let mut sa = SpaceSync::open(
        "personal",
        ka,
        key.clone(),
        roster.clone(),
        &a.path().join("sync"),
    )
    .unwrap();
    let mut sb = SpaceSync::open("personal", kb, key, roster, &b.path().join("sync")).unwrap();
    let original = source
        .rules_mut()
        .create("personal", &rule_input())
        .unwrap();
    let transport = MemoryStore::new();
    sa.sync(&mut source, &transport).unwrap();
    let parked = sb.sync(&mut Older(&mut destination), &transport).unwrap();
    assert!(parked.rejected.is_empty());
    assert!(parked.pending > 0);
    assert!(destination.rules().list("personal").unwrap().is_empty());
    // Once the host exposes _rules, its parked encrypted batches are replayed.
    let replayed = sb.sync(&mut destination, &transport).unwrap();
    assert!(replayed.rejected.is_empty());
    assert!(replayed.applied > 0);
    let imported = destination.rules().get(&original.id).unwrap().unwrap();
    assert_eq!(imported.input(), original.input());
    assert!(!imported.approved);
}

#[test]
fn plugin_event_rejection_rolls_back_writes_and_dry_runs_discard_events() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    h.load(&todo_dir()).unwrap();
    let task = h
        .call("todo", "create", &json!({"title":"Exercise"}))
        .unwrap()["id"]
        .clone();
    h.call_traced(
        "personal",
        "todo",
        "toggle",
        &json!({"id":task,"done":true}),
        true,
    )
    .unwrap();
    assert!(h.pending_events(50).unwrap().is_empty());
    assert_eq!(
        h.store("todo")
            .unwrap()
            .query(
                "SELECT done FROM tasks WHERE id = ?1",
                std::slice::from_ref(&task)
            )
            .unwrap()[0]["done"],
        0
    );

    for (plugin, event, operation, args) in [
        (
            "todo",
            "task_done",
            "toggle",
            json!({"id":task,"done":true}),
        ),
        ("habits", "checked_in", "checkin", json!({})),
    ] {
        let source = support::package_root().join(plugin);
        build_plugin(&source).unwrap();
        h.load(&source).unwrap();
        let args = if plugin == "habits" {
            let habit = h.call("habits", "save", &json!({"title":"Gym"})).unwrap()["id"].clone();
            json!({"id":habit})
        } else {
            args
        };
        // A stricter synthetic event contract rejects emission after the plugin
        // has written, proving both writes and held events are rolled back.
        let broken = TempDir::new().unwrap();
        copy_package(&source, broken.path());
        let path = broken.path().join("manifest.json");
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["events"][event]["payload"]["extra"] = json!({"type":"string","required":true});
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        h.load(broken.path()).unwrap();
        assert!(h.call(plugin, operation, &args).is_err());
        assert!(h.pending_events(50).unwrap().is_empty());
        if plugin == "habits" {
            assert!(h
                .store("habits")
                .unwrap()
                .query("SELECT id FROM checkins", &[])
                .unwrap()
                .is_empty());
        } else {
            assert_eq!(
                h.store("todo")
                    .unwrap()
                    .query(
                        "SELECT done FROM tasks WHERE id = ?1",
                        std::slice::from_ref(&task)
                    )
                    .unwrap()[0]["done"],
                0
            );
        }
    }
}

fn http_host(data: &TempDir) -> PluginHost {
    let mut host = host(data);
    host.set_net(Some(terrarium_net::Net::new()));
    host.load(&limits_dir()).unwrap();
    host
}
fn http_error(host: &mut PluginHost, operation: &str, code: &str) {
    let error = host.call("limits", operation, &json!({})).unwrap_err();
    assert!(
        matches!(error, RuntimeError::Plugin { code: ref c, .. } if c == code),
        "{error}"
    );
    assert!(host.net_mut().unwrap().take().is_none());
}
#[test]
fn fetch_is_queued_only_when_the_call_commits() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    for op in ["fetch_then_fail", "fetch_then_trap"] {
        host.call("limits", op, &json!({})).unwrap_err();
        assert!(host.net_mut().unwrap().take().is_none());
    }
    let id = host.call("limits", "fetch_ok", &json!({})).unwrap();
    let job = host.net_mut().unwrap().take().unwrap();
    assert_eq!(job.id, id.as_str().unwrap());
    assert_eq!(job.space, "personal");
    assert_eq!(job.plugin, "limits");
    assert_eq!(job.on_done, "on_fetched");
    assert_eq!(job.request.url, "https://example.com/");
}
#[test]
fn fetch_without_permission_is_denied() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    let pkg = data.path().join("no-http");
    copy_package(&limits_dir(), &pkg);
    let path = pkg.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["permissions"] = json!([]);
    manifest.as_object_mut().unwrap().remove("hosts");
    std::fs::write(path, manifest.to_string()).unwrap();
    host.load(&pkg).unwrap();
    assert_eq!(
        host.call("limits", "http_available", &json!({})).unwrap(),
        false
    );
    http_error(&mut host, "fetch_ok", "denied");
}
#[test]
fn fetch_to_unlisted_host_is_denied() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    http_error(&mut host, "fetch_other_host", "denied");
    http_error(&mut host, "fetch_bad_callback", "invalid");
}
#[test]
fn dry_run_queues_nothing() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.call_traced("personal", "limits", "fetch_ok", &json!({}), true)
        .unwrap();
    assert!(host.net_mut().unwrap().take().is_none());
}
#[test]
fn outer_failure_cancels_nested_http_without_cancelling_older_jobs() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    let old = host.call("limits", "fetch_ok", &json!({})).unwrap();
    let pkg = data.path().join("child");
    copy_package(&limits_dir(), &pkg);
    let path = pkg.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["id"] = json!("child");
    std::fs::write(path, manifest.to_string()).unwrap();
    host.load(&pkg).unwrap();
    // BTreeMap provider selection picks child (limits is moved out during its call).
    let error = host
        .call("limits", "nested_fetch_then_fail", &json!({}))
        .unwrap_err();
    assert!(
        matches!(error, RuntimeError::Plugin { ref code, ref message }
        if code == "invalid" && message == "outer failed"),
        "{error}"
    );
    let net = host.net_mut().unwrap();
    let job = net.take().unwrap();
    assert_eq!(job.id, old.as_str().unwrap());
    net.finish(&job.id);
    assert!(net.take().is_none());
}
#[test]
fn http_unavailable_without_worker_and_unload_drops_queued_jobs() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    assert_eq!(
        host.call("limits", "http_available", &json!({})).unwrap(),
        true
    );
    host.call("limits", "fetch_ok", &json!({})).unwrap();
    host.unload("limits");
    assert!(host.net_mut().unwrap().take().is_none());
    host.load(&limits_dir()).unwrap();
    host.set_net(None);
    assert_eq!(
        host.call("limits", "http_available", &json!({})).unwrap(),
        false
    );
    let error = host.call("limits", "fetch_ok", &json!({})).unwrap_err();
    assert!(matches!(error,RuntimeError::Plugin {ref code,..} if code == "unavailable"));
}
#[test]
fn invalid_http_input_is_rejected_without_echoing_private_data() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    for request in [
        json!({"method":"GET","url":"https://example.com/?PRIVATE", "extra":"PRIVATE"}),
        json!({"method":"POST","url":"https://example.com/","headers":{"Authorization":"PRIVATE"}}),
        json!({"method":"GET","url":"http://example.com/?PRIVATE"}),
    ] {
        let error = host
            .call("limits", "fetch_raw", &json!({"request":request}))
            .unwrap_err();
        assert!(matches!(error,RuntimeError::Plugin {ref code,..} if code == "invalid"));
        assert!(!error.to_string().contains("PRIVATE"));
        assert!(host.net_mut().unwrap().take().is_none());
    }
}

#[test]
fn unused_http_import_is_omitted_from_release_components() {
    fn imports(dir: &Path) -> Vec<String> {
        let bytes = std::fs::read(dir.join("dist/plugin.wasm")).unwrap();
        let wit_component::DecodedWasm::Component(resolve, world) =
            wit_component::decode(&bytes).unwrap()
        else {
            panic!("expected component");
        };
        resolve.worlds[world]
            .imports
            .keys()
            .map(|key| resolve.name_world_key(key))
            .collect()
    }
    assert!(!imports(&todo_dir())
        .iter()
        .any(|name| name == "terrarium:plugin/http@0.1.0"));
    assert!(imports(&limits_dir())
        .iter()
        .any(|name| name == "terrarium:plugin/http@0.1.0"));
}

#[test]
fn commit_failure_cancels_http_reservations() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    let old = host.call("limits", "fetch_ok", &json!({})).unwrap();
    // Force the plugin transaction to end before Runtime's COMMIT, while the
    // fixture catches the storage error and successfully submits an HTTP job.
    // This is fault-injection DDL on synthetic data, never a production write.
    let conn = rusqlite::Connection::open(data.path().join("plugins/limits.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER synthetic_rollback BEFORE INSERT ON notes BEGIN SELECT RAISE(ROLLBACK, 'synthetic rollback'); END;").unwrap();
    let error = host
        .call("limits", "fetch_commit_failure", &json!({}))
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Core(_)), "{error}");
    assert!(
        error.to_string().contains("no transaction is active"),
        "{error}"
    );
    let net = host.net_mut().unwrap();
    let job = net.take().unwrap();
    assert_eq!(job.id, old.as_str().unwrap());
    net.finish(&job.id);
    assert!(net.take().is_none());
}

fn http_response() -> terrarium_net::Response {
    terrarium_net::Response {
        status: 200,
        headers: Default::default(),
        body: "synthetic".into(),
        bytes_in: 9,
    }
}
#[test]
fn http_reply_reaches_callback_with_context() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    let context = json!({"nested":[1,null,"synthetic"],"enqueue":false});
    let id = host
        .call(
            "limits",
            "fetch_raw",
            &json!({"request":{"method":"GET","url":"https://example.com/","context":context}}),
        )
        .unwrap();
    let (job, auth) = host.take_http_job().unwrap();
    assert!(auth.is_none());
    assert!(host.deliver_http(job, Ok(http_response())).is_none());
    let rows = host.query("limits", "all", &Map::new()).unwrap();
    let payload: Value = serde_json::from_str(rows[0]["note"].as_str().unwrap()).unwrap();
    assert_eq!(payload["context"], context);
    assert_eq!(payload["request"], id);
    assert_eq!(payload["body"], "synthetic");
}
#[test]
fn scheduled_run_waits_for_http_reply() {
    use terrarium_runtime::scheduler::{Repeat, RunOn, ScheduleInput};
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    let id=host.scheduler_mut().set("personal","limits","http",&ScheduleInput {
        name:"Web fixture".into(),repeat:Repeat::Minutes{minutes:5},tz:None,run_on:RunOn::Any,
        operation:"fetch_raw".into(),args:json!({"request":{"method":"GET","url":"https://example.com/","context":{"enqueue":true}}}),enabled:true,
    },"plugin").unwrap();
    let now = host.scheduler().get(&id).unwrap().created_ms + 300_001;
    assert!(host.run_due(now, true).is_empty());
    let run = host.scheduler().runs(&id, 1)[0].clone();
    assert_eq!(run.status, "waiting");
    let (job, _) = host.take_http_job().unwrap();
    host.ai_mut()
        .set_config(terrarium_runtime::ai::AiConfig {
            preset: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "synthetic".into(),
            ..Default::default()
        })
        .unwrap();
    host.ai_mut()
        .submit(
            "personal",
            "limits",
            "on_fetched",
            serde_json::from_value(json!({"prompt":"synthetic"})).unwrap(),
            Some(run.id.clone()),
        )
        .unwrap();
    let (ai_job, _, _) = host.take_ai_job().unwrap();
    assert!(
        host.deliver_http(job, Ok(http_response())).is_none(),
        "callback queued another request"
    );
    let (job, _) = host.take_http_job().unwrap();
    assert!(
        host.deliver_http(job, Ok(http_response())).is_none(),
        "AI is still running"
    );
    let done = host
        .deliver_ai(
            ai_job,
            Ok(terrarium_runtime::ai::Completion {
                text: "synthetic".into(),
                model: "synthetic".into(),
                input_tokens: 1,
                output_tokens: 1,
            }),
        )
        .unwrap();
    assert_eq!(done.run.status, "done");
}
#[test]
fn reloading_retires_an_in_flight_http_callback() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.call("limits", "fetch_ok", &json!({})).unwrap();
    let (job, _) = host.take_http_job().unwrap();
    host.load(&limits_dir()).unwrap();
    assert!(!host.http_job_current(&job));
    assert!(host.deliver_http(job, Ok(http_response())).is_none());
    assert!(host.query("limits", "all", &Map::new()).unwrap().is_empty());
}
#[test]
fn escaped_http_callback_is_bounded() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.call("limits", "fetch_ok", &json!({})).unwrap();
    let (job, _) = host.take_http_job().unwrap();
    let mut response = http_response();
    response.body = "\u{0}".repeat(1024 * 1024);
    host.deliver_http(job, Ok(response));
    let rows = host.query("limits", "all", &Map::new()).unwrap();
    let payload: Value = serde_json::from_str(rows[0]["note"].as_str().unwrap()).unwrap();
    assert_eq!(payload["error"]["code"], "too-large");
    assert!(payload.get("body").is_none());
}

#[test]
fn vault_unbound_host_is_delivered_as_denied() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.set_ai_sealer(terrarium_runtime::ai::Sealer::new("space", &[7; 32]));
    host.vault_service(
        "vault_put_bearer",
        &json!({"plugin":"limits","name":"work","hosts":["example.com"],"token":"test-token-1234"}),
    )
    .unwrap();
    // Load the same compiled component with a second manifest-authorized host.
    let package = data.path().join("two-hosts");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    std::fs::copy(
        limits_dir().join("dist/plugin.wasm"),
        package.join("dist/plugin.wasm"),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(limits_dir().join("manifest.json")).unwrap())
            .unwrap();
    manifest["hosts"] = json!(["example.com", "api.example.org"]);
    std::fs::write(package.join("manifest.json"), manifest.to_string()).unwrap();
    host.load(&package).unwrap();
    // Bind the account to the new loaded package generation.
    host.vault_service(
        "vault_put_bearer",
        &json!({"plugin":"limits","name":"work","hosts":["example.com"],"token":"test-token-1234"}),
    )
    .unwrap();
    host.call("limits","fetch_raw", &json!({"request":{"method":"GET","url":"https://api.example.org/","auth":{"account":"work"}}})).unwrap();
    let (job, auth) = host.take_http_job().unwrap();
    assert!(auth.is_none());
    let result = terrarium_runtime::net::fetch::run(&job, None);
    host.deliver_http(job, result);
    let rows = host.query("limits", "all", &Map::new()).unwrap();
    let payload: Value = serde_json::from_str(rows[0]["note"].as_str().unwrap()).unwrap();
    assert_eq!(payload["error"]["code"], "denied");
    assert_eq!(
        payload["error"]["message"],
        "This account is not allowed for api.example.org"
    );
    assert!(!payload.to_string().contains("test-token-1234"));
}

#[test]
fn vault_services_are_not_cli_or_plugin_services_and_removal_retires_jobs() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.set_ai_sealer(terrarium_runtime::ai::Sealer::new("space", &[7; 32]));
    let args =
        json!({"plugin":"limits","name":"work","hosts":["example.com"],"token":"test-token-1234"});
    for cmd in [
        "vault_accounts",
        "vault_put_bearer",
        "vault_remove",
        "vault_approve",
    ] {
        assert!(terrarium_runtime::services::invoke(&mut host, cmd, &args, true).is_none());
        assert!(terrarium_runtime::commands::describe(
            &host,
            terrarium_runtime::commands::Caller::Client(terrarium_runtime::commands::Scope::Change)
        )
        .iter()
        .all(|c| c.id != cmd));
    }
    host.vault_service("vault_put_bearer", &args).unwrap();
    host.call(
        "limits",
        "fetch_raw",
        &json!({"request":{"method":"GET","url":"https://example.com/","auth":{"account":"work"}}}),
    )
    .unwrap();
    let (job, lease) = host.take_http_job().unwrap();
    let lease = lease.unwrap();
    assert!(host.update_http_account(&job, &lease, None).is_ok());
    host.remove_plugin_accounts("limits").unwrap();
    assert!(!host.http_job_current(&job));
    assert!(host.update_http_account(&job, &lease, None).is_err());
    host.deliver_http(job, Ok(http_response()));
    assert!(host.query("limits", "all", &Map::new()).unwrap().is_empty());
    assert_eq!(
        host.vault_service("vault_accounts", &args).unwrap(),
        json!([])
    );
}

#[test]
fn oauth_completion_checks_provider_package_and_account_generation() {
    let data = TempDir::new().unwrap();
    let mut host = http_host(&data);
    host.set_ai_sealer(terrarium_runtime::ai::Sealer::new("space", &[7; 32]));
    let package = data.path().join("oauth");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    std::fs::copy(
        limits_dir().join("dist/plugin.wasm"),
        package.join("dist/plugin.wasm"),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(limits_dir().join("manifest.json")).unwrap())
            .unwrap();
    manifest["oauth"] = json!({"work":{"auth_url":"https://example.com/authorize","token_url":"https://example.com/token","scopes":["items.read"],"api_hosts":["example.com"]}});
    std::fs::write(package.join("manifest.json"), manifest.to_string()).unwrap();
    host.load(&package).unwrap();
    let preparation = host
        .prepare_oauth("personal", "limits", "work", "work")
        .unwrap();
    let tokens = terrarium_net::vault::TokenSet {
        access: "synthetic-access".into(),
        refresh: "synthetic-refresh".into(),
        expires_ms: 9_999_999_999_999,
        token_url: preparation.provider.token_url.clone(),
        client_id: "synthetic-client".into(),
        client_secret: None,
        provider: Some(preparation.provider.clone()),
    };
    assert!(host.oauth_current(
        "personal",
        "limits",
        "work",
        (&preparation.signer, None),
        &preparation.provider
    ));
    let result = host
        .finish_oauth(
            "personal",
            "limits",
            "work",
            (&preparation.signer, None),
            tokens.clone(),
        )
        .unwrap();
    assert_eq!(result[0]["kind"], "oauth");
    assert_eq!(result[0]["active"], true);
    assert!(!result.to_string().contains("synthetic"));
    assert!(!host.oauth_current(
        "personal",
        "limits",
        "work",
        (&preparation.signer, None),
        &preparation.provider
    ));
    drop(preparation.client);
    let current = host
        .prepare_oauth("personal", "limits", "work", "work")
        .unwrap();
    host.vault_service(
        "vault_remove",
        &json!({"plugin":"limits","space":"personal","name":"work"}),
    )
    .unwrap();
    assert!(host
        .finish_oauth(
            "personal",
            "limits",
            "work",
            (&current.signer, current.generation.as_deref()),
            tokens.clone()
        )
        .is_err());
    manifest["oauth"]["work"]["scopes"] = json!(["items.write"]);
    std::fs::write(package.join("manifest.json"), manifest.to_string()).unwrap();
    host.load(&package).unwrap();
    assert!(!host.oauth_current(
        "personal",
        "limits",
        "work",
        (&preparation.signer, None),
        &preparation.provider
    ));
    assert!(host
        .finish_oauth(
            "personal",
            "limits",
            "work",
            (&preparation.signer, None),
            tokens
        )
        .is_err());
}
