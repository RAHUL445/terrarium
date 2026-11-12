//! The command registry over a real plugin: what callers outside the app
//! can see and run, argument checks, host commands and search.

mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_runtime::blobs::{BlobKey, MAX_BLOB};
use terrarium_runtime::commands::{self, Caller, CommandError, Kind, Scope};
use terrarium_runtime::{build_plugin, services, HostConfig, Manifest, PluginHost};

const READ: Caller = Caller::Client(Scope::Read);
const CHANGE: Caller = Caller::Client(Scope::Change);

fn todo_dir() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = support::package_root().join("todo").canonicalize().unwrap();
            build_plugin(&dir).unwrap_or_else(|e| panic!("building todo: {e}"));
            dir
        })
        .clone()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "src")
            {
                continue;
            }
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// A copy of To-do whose manifest describes some of its commands.
fn described_todo(root: &Path) -> PathBuf {
    let from = todo_dir();
    let to = root.join("todo");
    for entry in walk(&from) {
        let dest = to.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // Start from no descriptions, whatever the plugin bundle declares, so
    // these tests cover the host's defaults (untyped, inferred effects).
    for op in m["operations"].as_object_mut().unwrap().values_mut() {
        let op = op.as_object_mut().unwrap();
        for key in ["effect", "confirm", "params", "result", "examples", "cli"] {
            op.remove(key);
        }
    }
    for q in m["queries"].as_object_mut().unwrap().values_mut() {
        let q = q.as_object_mut().unwrap();
        for key in ["description", "types", "cli"] {
            q.remove(key);
        }
    }
    m.as_object_mut().unwrap().remove("open");
    m["operations"]["create"]["params"] = json!({
        "title": { "type": "string", "required": true },
        "due": { "type": "date" },
        "priority": { "type": "integer", "min": 0, "max": 3 },
        "tags": { "type": "string" },
    });
    m["operations"]["create"]["result"] = json!({ "id": { "type": "id" } });
    m["operations"]["delete"]["effect"] = json!("destructive");
    m["operations"]["delete"]["params"] = json!({ "id": { "type": "id", "required": true } });
    m["operations"]["note_settings"]["cli"] = json!(false);
    m["queries"]["tasks"]["types"] = json!({
        "filter": { "type": "enum", "values": ["today", "upcoming", "all", "done"] }
    });
    m["queries"]["badge"]["cli"] = json!(false);
    m["queries"]["search"]["types"] = json!({ "q": { "type": "string", "required": true } });
    m["queries"]["export_tasks"] = json!({"sql": "SELECT title, due, done FROM tasks WHERE _deleted IS NULL ORDER BY title", "params": []});
    m["export"] = json!([
        {"query": "export_tasks", "format": "csv", "path": "tasks.csv", "columns": ["title", "due", "done"]},
        {"query": "export_tasks", "format": "markdown", "path": "notes/{{title}}.md", "body": "title"}
    ]);
    m["operations"]["toggle"]["bulk"] = json!(true);
    m["operations"]["delete"]["bulk"] = json!(true);
    m["open"] = json!({ "page": "main" });
    m["history"] = json!(["tasks"]);
    std::fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    to
}

fn host(data: &TempDir) -> PluginHost {
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "Asia/Kolkata".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    host.load(&described_todo(data.path())).unwrap();
    host
}

fn run(
    host: &mut PluginHost,
    caller: Caller,
    id: &str,
    args: Value,
) -> Result<Value, CommandError> {
    commands::invoke(host, caller, Kind::Action, id, None, &args, true)
}

fn query(
    host: &mut PluginHost,
    caller: Caller,
    id: &str,
    args: Value,
) -> Result<Value, CommandError> {
    commands::invoke(host, caller, Kind::Query, id, None, &args, true)
}

fn code(r: Result<Value, CommandError>) -> String {
    r.expect_err("should fail").code
}

#[test]
fn describe_lists_host_and_plugin_commands() {
    let data = TempDir::new().unwrap();
    let host = host(&data);
    let all = commands::describe(&host, CHANGE);
    let find = |kind, id: &str| all.iter().find(|c| c.kind == kind && c.id == id);

    for id in [
        "app.describe",
        "space.list",
        "page.list",
        "search.query",
        "plugin.list",
        "item.history",
    ] {
        assert!(find(Kind::Query, id).is_some(), "{id}");
    }
    assert_eq!(
        find(Kind::Query, "item.history").unwrap().effect,
        terrarium_runtime::Effect::Read
    );
    assert_eq!(
        find(Kind::Action, "item.restore_version").unwrap().effect,
        terrarium_runtime::Effect::Write
    );
    let create = find(Kind::Action, "todo.create").unwrap();
    assert!(create.params.as_ref().unwrap()["title"].required);
    assert!(create.summary.starts_with("Create a task."));
    assert!(serde_json::to_value(create).unwrap().get("bulk").is_none());
    // Untyped operations say so with no params.
    assert!(find(Kind::Action, "todo.update").unwrap().params.is_none());
    // Undeclared effects: delete-like names are destructive, others write.
    use terrarium_runtime::Effect;
    assert_eq!(
        find(Kind::Action, "todo.clear_done").unwrap().effect,
        Effect::Destructive
    );
    assert_eq!(
        find(Kind::Action, "todo.delete_step").unwrap().effect,
        Effect::Destructive
    );
    assert_eq!(
        find(Kind::Action, "todo.update").unwrap().effect,
        Effect::Write
    );
    // Context parameters are listed apart from the caller's own.
    let tasks = find(Kind::Query, "todo.tasks").unwrap();
    assert_eq!(tasks.context, ["today"]);
    assert!(!tasks.params.as_ref().unwrap().contains_key("today"));
    // A query and an operation may share a name; only the query is visible
    // because the operation is marked `cli: false`.
    assert!(find(Kind::Query, "todo.note_settings").is_some());
    assert!(find(Kind::Action, "todo.note_settings").is_none());

    // A read-only client is shown only what it can run.
    let read = commands::describe(&host, READ);
    assert!(read
        .iter()
        .all(|c| c.effect == terrarium_runtime::Effect::Read));
    assert!(read.iter().any(|c| c.id == "todo.tasks"));
    assert!(!read.iter().any(|c| c.id == "todo.create"));

    let app = commands::describe(&host, Caller::App);
    assert!(app
        .iter()
        .any(|c| c.id == "todo.note_settings" && c.kind == Kind::Action && c.hidden));
}

#[test]
fn read_only_clients_cannot_change_data() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    assert_eq!(
        code(run(&mut host, READ, "todo.create", json!({"title": "x"}))),
        "forbidden"
    );
    assert_eq!(
        code(run(
            &mut host,
            READ,
            "space.create",
            json!({"name": "Work"})
        )),
        "forbidden"
    );
    assert!(query(&mut host, READ, "todo.tasks", json!({"filter": "all"})).is_ok());
}

#[test]
fn outside_arguments_are_checked() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    assert_eq!(
        code(run(&mut host, CHANGE, "todo.create", json!({}))),
        "invalid"
    );
    assert_eq!(
        code(run(
            &mut host,
            CHANGE,
            "todo.create",
            json!({"title": "x", "colour": "red"})
        )),
        "invalid"
    );
    assert_eq!(
        code(run(
            &mut host,
            CHANGE,
            "todo.create",
            json!({"title": "x", "priority": 9})
        )),
        "invalid"
    );
    assert_eq!(
        code(run(
            &mut host,
            CHANGE,
            "todo.create",
            json!({"title": "x", "due": "tomorrow"})
        )),
        "invalid"
    );
    assert_eq!(
        code(run(&mut host, CHANGE, "todo.create", json!("x"))),
        "invalid"
    );

    let made = run(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Pay rent", "due": "2026-11-01", "priority": 2}),
    )
    .unwrap();
    assert!(made["id"].is_string());

    // Unknown commands and hidden operations look the same from outside.
    assert_eq!(
        code(run(&mut host, CHANGE, "todo.nope", json!({}))),
        "not-found"
    );
    assert_eq!(
        code(run(&mut host, CHANGE, "todo.note_settings", json!({}))),
        "not-found"
    );
    // Plugin error codes pass through.
    assert_eq!(
        code(run(
            &mut host,
            CHANGE,
            "todo.delete",
            json!({"id": "missing"})
        )),
        "not-found"
    );
}

#[test]
fn the_app_is_not_checked() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    // The UI may pass fields a template adds; the plugin decides.
    let made = run(
        &mut host,
        Caller::App,
        "todo.create",
        json!({"title": "x", "extra": 1}),
    )
    .unwrap();
    assert!(made["id"].is_string());
    assert!(run(
        &mut host,
        Caller::App,
        "todo.note_settings",
        json!({"ask": "never"})
    )
    .is_ok());
}

#[test]
fn queries_get_clock_parameters_filled_in() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    run(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Water plants today"}),
    )
    .unwrap();
    let rows = query(&mut host, READ, "todo.tasks", json!({"filter": "all"})).unwrap();
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["title"] == "Water plants"));
    // Callers can still pin the date.
    assert!(query(
        &mut host,
        READ,
        "todo.tasks",
        json!({"filter": "all", "today": "2026-01-01"})
    )
    .is_ok());
}

#[test]
fn search_spans_plugins_and_spaces() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let work = run(&mut host, CHANGE, "space.create", json!({"name": "Work"})).unwrap();
    let work = work["id"].as_str().unwrap().to_string();
    run(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Buy milk"}),
    )
    .unwrap();
    commands::invoke(
        &mut host,
        CHANGE,
        Kind::Action,
        "todo.create",
        Some(&work),
        &json!({"title": "Milk the budget"}),
        true,
    )
    .unwrap();

    let here = query(&mut host, READ, "search.query", json!({"q": "milk"})).unwrap();
    assert_eq!(here["truncated"], false);
    let results = here["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["title"], "Buy milk");
    assert_eq!(results[0]["open"]["page"], "main");
    assert_eq!(results[0]["open"]["item"], results[0]["id"]);

    let everywhere = query(
        &mut host,
        READ,
        "search.query",
        json!({"q": "milk", "all_spaces": true}),
    )
    .unwrap();
    assert_eq!(everywhere["results"].as_array().unwrap().len(), 2);
    let limited = query(
        &mut host,
        READ,
        "search.query",
        json!({"q": "milk", "all_spaces": true, "limit": 1}),
    )
    .unwrap();
    assert_eq!(limited["results"].as_array().unwrap().len(), 1);
    assert_eq!(limited["truncated"], true);
    let none = query(
        &mut host,
        READ,
        "search.query",
        json!({"q": "milk", "plugins": ["notes"]}),
    )
    .unwrap();
    assert!(none["results"].as_array().unwrap().is_empty());
    assert_eq!(
        code(query(&mut host, READ, "search.query", json!({"q": " "}))),
        "invalid"
    );
}

#[test]
fn pages_and_spaces_through_commands() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let page = commands::invoke(
        &mut host,
        CHANGE,
        Kind::Action,
        "page.create",
        None,
        &json!({"title": "Planning"}),
        true,
    )
    .unwrap();
    let id = page["id"].as_str().unwrap();
    let pages = query(&mut host, READ, "page.list", json!({})).unwrap();
    assert!(pages
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["title"] == "Planning"));
    run(
        &mut host,
        CHANGE,
        "page.update",
        json!({"page": id, "title": "Plans"}),
    )
    .unwrap();
    let got = query(&mut host, READ, "page.get", json!({"page": id})).unwrap();
    assert_eq!(got["page"]["title"], "Plans");
    run(&mut host, CHANGE, "page.remove", json!({"page": id})).unwrap();

    let spaces = query(&mut host, READ, "space.list", json!({})).unwrap();
    assert_eq!(spaces.as_array().unwrap().len(), 1);
}

#[test]
fn manifests_reject_bad_descriptions() {
    let todo: Value =
        serde_json::from_slice(&std::fs::read(todo_dir().join("manifest.json")).unwrap()).unwrap();
    let check = |edit: &dyn Fn(&mut Value)| {
        let mut m = todo.clone();
        edit(&mut m);
        Manifest::parse(&serde_json::to_vec(&m).unwrap())
            .err()
            .map(|e| e.to_string())
    };
    assert!(check(&|_| {}).is_none());
    assert!(check(&|m| m["id"] = json!("ref"))
        .unwrap()
        .contains("reserved"));
    assert!(check(&|m| m["id"] = json!("search"))
        .unwrap()
        .contains("reserved"));
    assert!(
        check(&|m| m["operations"]["create"]["params"] = json!({"kind": {"type": "enum"}}))
            .unwrap()
            .contains("values")
    );
    assert!(check(&|m| m["operations"]["create"]["effect"] = json!("dangerous")).is_some());
    assert!(
        check(&|m| m["queries"]["search"]["types"] = json!({"nope": {"type": "string"}}))
            .unwrap()
            .contains("not a parameter")
    );
    assert!(
        check(&|m| m["queries"]["tasks"]["types"] = json!({"today": {"type": "date"}}))
            .unwrap()
            .contains("filled in by the host")
    );
    assert!(check(&|m| m["open"] = json!({"page": "nope"}))
        .unwrap()
        .contains("not a declared page"));
}

fn run_with(
    host: &mut PluginHost,
    caller: Caller,
    id: &str,
    args: Value,
    dry_run: bool,
    confirmed: bool,
) -> Result<commands::Outcome, CommandError> {
    let options = commands::InvokeOptions { dry_run, confirmed };
    commands::invoke_with(host, caller, Kind::Action, id, None, &args, true, options)
}

fn task_titles(host: &mut PluginHost) -> Vec<String> {
    let rows = query(host, READ, "todo.tasks", json!({"filter": "all"})).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_dry_run_reports_changes_and_keeps_nothing() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let out = run_with(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Pay rent"}),
        true,
        false,
    )
    .unwrap();
    assert!(out.dry_run);
    assert!(out.result["id"].is_string());
    assert!(out
        .changes
        .iter()
        .any(|c| c.plugin == "todo" && c.table == "tasks" && c.op == "create"));
    assert!(task_titles(&mut host).is_empty());

    // For real: the same changes are reported and kept.
    let out = run_with(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Pay rent"}),
        false,
        false,
    )
    .unwrap();
    assert!(!out.dry_run);
    assert!(out.changes.iter().any(|c| c.op == "create"));
    assert_eq!(task_titles(&mut host), ["Pay rent"]);

    // Host actions have no dry run.
    let err = run_with(
        &mut host,
        CHANGE,
        "space.create",
        json!({"name": "W"}),
        true,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[test]
fn a_dry_run_rolls_back_other_plugins_too() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let notes = support::package_root()
        .join("notes")
        .canonicalize()
        .unwrap();
    build_plugin(&notes).unwrap();
    host.load(&notes).unwrap();

    let out = run_with(
        &mut host,
        CHANGE,
        "notes.to_task",
        json!({"text": "- [ ] Call Sam"}),
        true,
        false,
    )
    .unwrap();
    assert!(out
        .changes
        .iter()
        .any(|c| c.plugin == "todo" && c.op == "create"));
    assert!(task_titles(&mut host).is_empty());

    run_with(
        &mut host,
        CHANGE,
        "notes.to_task",
        json!({"text": "- [ ] Call Sam"}),
        false,
        false,
    )
    .unwrap();
    assert_eq!(task_titles(&mut host), ["Call Sam"]);
}

#[test]
fn destructive_actions_need_confirmation_from_outside() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = run(&mut host, CHANGE, "todo.create", json!({"title": "Old"})).unwrap()["id"].clone();
    let err = run_with(
        &mut host,
        CHANGE,
        "todo.delete",
        json!({"id": id}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, "needs-confirmation");
    // A dry run shows what would happen without confirming.
    let out = run_with(
        &mut host,
        CHANGE,
        "todo.delete",
        json!({"id": id}),
        true,
        false,
    )
    .unwrap();
    assert!(out.changes.iter().any(|c| c.op == "delete"));
    assert_eq!(task_titles(&mut host), ["Old"]);
    run_with(
        &mut host,
        CHANGE,
        "todo.delete",
        json!({"id": id}),
        false,
        true,
    )
    .unwrap();
    assert!(task_titles(&mut host).is_empty());
    // The app's own UI confirms for itself.
    let id = run(
        &mut host,
        Caller::App,
        "todo.create",
        json!({"title": "New"}),
    )
    .unwrap()["id"]
        .clone();
    assert!(run(&mut host, Caller::App, "todo.delete", json!({"id": id})).is_ok());
}

#[test]
fn widgets_are_placed_moved_and_removed() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let page = run(&mut host, CHANGE, "page.create", json!({"title": "Plan"})).unwrap();
    let page = page["id"].as_str().unwrap().to_string();

    let saved = run(
        &mut host,
        CHANGE,
        "page.place",
        json!({"page": page, "widget": "todo/today"}),
    )
    .unwrap();
    let first = &saved["placements"][0];
    assert_eq!(
        (first["x"].as_i64(), first["y"].as_i64()),
        (Some(0), Some(0))
    );
    let saved = run(
        &mut host,
        CHANGE,
        "page.place",
        json!({"page": page, "widget": "_host/text", "config": {"text": "Hi"}}),
    )
    .unwrap();
    let placements = saved["placements"].as_array().unwrap();
    assert_eq!(placements.len(), 2);
    let text = placements.iter().find(|p| p["plugin"] == "_host").unwrap();
    // Placed below the first widget.
    assert_eq!(text["y"], json!(first["h"].as_i64().unwrap()));
    assert_eq!(text["config"]["text"], "Hi");

    let text_id = text["id"].as_str().unwrap().to_string();
    let moved = run(
        &mut host,
        CHANGE,
        "page.move",
        json!({"page": page, "placement": text_id, "x": 6, "y": 0}),
    )
    .unwrap();
    let text = moved["placements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == text_id.as_str())
        .unwrap()
        .clone();
    assert_eq!((text["x"].as_i64(), text["y"].as_i64()), (Some(6), Some(0)));

    assert_eq!(
        code(run(
            &mut host,
            CHANGE,
            "page.place",
            json!({"page": page, "widget": "todo/nope"})
        )),
        "not-found"
    );
    let unconfirmed = run_with(
        &mut host,
        CHANGE,
        "page.unplace",
        json!({"page": page, "placement": text_id}),
        false,
        false,
    );
    assert_eq!(unconfirmed.unwrap_err().code, "needs-confirmation");
    let out = run_with(
        &mut host,
        CHANGE,
        "page.unplace",
        json!({"page": page, "placement": text_id}),
        false,
        true,
    )
    .unwrap();
    assert_eq!(out.result["placements"].as_array().unwrap().len(), 1);
}

#[test]
fn ui_commands_need_the_app() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    assert_eq!(
        code(run(&mut host, CHANGE, "ui.notify", json!({"text": "hi"}))),
        "unavailable"
    );
    let read = commands::describe(&host, READ);
    assert!(!read.iter().any(|c| c.id.starts_with("ui.")));
}

fn read_zip(path: &Path) -> std::collections::BTreeMap<String, String> {
    use std::io::Read;

    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut entry = zip.by_index(i).unwrap();
            let name = entry.name().unwrap().into_owned();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            (name, text)
        })
        .collect()
}

#[test]
fn export_writes_every_table_as_json() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for title in ["Pay rent", "Call Sam"] {
        run(&mut host, CHANGE, "todo.create", json!({"title": title})).unwrap();
    }
    let path = data.path().join("out.ZIP");
    let summary = run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap();
    assert_eq!(summary["path"], path.to_str().unwrap());
    assert_eq!(summary["plugins"], 1);
    assert!(summary["rows"].as_u64().unwrap() >= 2);
    assert_eq!(summary["bytes"], std::fs::metadata(&path).unwrap().len());
    let entries = read_zip(&path);
    let tasks: Vec<Value> = serde_json::from_str(&entries["data/todo/tasks.json"]).unwrap();
    let mut titles: Vec<_> = tasks.iter().map(|r| r["title"].as_str().unwrap()).collect();
    titles.sort();
    assert_eq!(titles, ["Call Sam", "Pay rent"]);
    assert!(tasks.iter().all(|r| r.get("_deleted").is_none()));
    let manifest: Value = serde_json::from_str(&entries["manifest.json"]).unwrap();
    assert_eq!(manifest["format"], "terrarium-export");
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["space"]["id"], "personal");
    assert_eq!(manifest["include_trash"], false);
    assert!(manifest["created_ms"].as_i64().unwrap() > 0);
    assert_eq!(manifest["plugins"][0]["tables"]["tasks"], 2);
    let store = host.store_in("personal", "todo").unwrap();
    let tables = store.dump().unwrap();
    assert_eq!(entries.len(), tables.len() + 5);
    for table in tables.keys() {
        let rows: Vec<Value> =
            serde_json::from_str(&entries[&format!("data/todo/{table}.json")]).unwrap();
        assert_eq!(manifest["plugins"][0]["tables"][table], rows.len());
    }
    assert_eq!(
        manifest["plugins"][0]["schema_version"],
        store.schema().version
    );
    assert_eq!(entries["README.txt"], "Terrarium export\n\nThis file holds the data of one space as plain JSON (and Markdown or\nCSV copies where apps offer them). It is not encrypted: anyone who\nhas this file can read everything in it. Keep it somewhere safe and\ndelete it when you no longer need it.\n\ndata/<app>/<table>.json  every item, one JSON object per item\nfiles/<app>/...             Markdown and CSV copies, where apps offer them\nmanifest.json            what was exported and when\n");
}

#[test]
fn export_leaves_out_trash_unless_asked() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    run(&mut host, CHANGE, "todo.create", json!({"title": "Keep"})).unwrap();
    let id =
        run(&mut host, CHANGE, "todo.create", json!({"title": "Trash"})).unwrap()["id"].clone();
    run(&mut host, Caller::App, "todo.delete", json!({"id": id})).unwrap();
    for include_trash in [false, true] {
        let path = data.path().join(format!("trash-{include_trash}.zip"));
        let args = if include_trash {
            json!({"path": path, "include_trash": true})
        } else {
            json!({"path": path})
        };
        run(&mut host, Caller::App, "space.export", args).unwrap();
        let entries = read_zip(&path);
        let tasks: Vec<Value> = serde_json::from_str(&entries["data/todo/tasks.json"]).unwrap();
        if include_trash {
            assert_eq!(tasks.len(), 2);
            assert!(tasks.iter().all(|r| r.get("_deleted").is_some()));
            assert_eq!(tasks.iter().filter(|r| !r["_deleted"].is_null()).count(), 1);
        } else {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0]["title"], "Keep");
            assert!(tasks[0].get("_deleted").is_none());
        }
    }
}

fn limits_dir() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/limits");
            build_plugin(&dir).unwrap();
            dir
        })
        .clone()
}

fn bulk(
    host: &mut PluginHost,
    caller: Caller,
    args: Value,
    dry_run: bool,
    confirmed: bool,
) -> Result<commands::Outcome, CommandError> {
    commands::invoke_with(
        host,
        caller,
        Kind::Action,
        "plugin.run_bulk",
        None,
        &args,
        true,
        commands::InvokeOptions { dry_run, confirmed },
    )
}

#[test]
fn bulk_runs_once_per_id_with_per_item_results() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap()["id"].clone();
    let b = run(&mut h, CHANGE, "todo.create", json!({"title":"B"})).unwrap()["id"].clone();
    run(&mut h, CHANGE, "todo.create", json!({"title":"C"})).unwrap();
    let out = bulk(
        &mut h,
        Caller::App,
        json!({"plugin":"todo","op":"toggle","ids":[a,b,"missing"],"args":{"done":true}}),
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.result["total"], 3);
    assert_eq!(out.result["done"], 2);
    assert_eq!(out.result["failed"], 1);
    assert_eq!(out.result["message"], "2 done, 1 failed");
    assert_eq!(out.result["items"][2]["error"]["code"], "not-found");
    assert!(out.changes.is_empty());
    let rows = query(&mut h, READ, "todo.tasks", json!({"filter":"all"})).unwrap();
    for row in rows.as_array().unwrap() {
        assert_eq!(row["done"], json!(if row["title"] == "C" { 0 } else { 1 }));
    }
}

#[test]
fn export_refuses_bad_paths() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let existing = data.path().join("existing.zip");
    std::fs::write(&existing, "keep me").unwrap();
    for path in [
        PathBuf::from("out.zip"),
        data.path().join("out.txt"),
        data.path().join("nope/out.zip"),
        existing.clone(),
    ] {
        assert_eq!(
            code(run(
                &mut host,
                Caller::App,
                "space.export",
                json!({"path": path})
            )),
            "invalid"
        );
    }
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "keep me");
}

#[test]
#[cfg(unix)]
fn export_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let path = data.path().join("private.zip");
    run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn bulk_refuses_bad_input() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    for (patch, message) in [
        (json!({"op":"create"}), "can't run on many"),
        (json!({"ids":[]}), "at least one"),
        (json!({"ids":"a"}), "list of item ids"),
        (json!({"ids":[""]}), "list of item ids"),
        (json!({"ids":[1]}), "list of item ids"),
        (
            json!({"ids":(0..501).map(|i|format!("x{i}")).collect::<Vec<_>>()}),
            "at most 500",
        ),
        (json!({"args":{"id":"x"}}), "must not contain"),
        (json!({"args":{"ids":[]}}), "must not contain"),
        (json!({"args":true}), "must be an object"),
    ] {
        let mut args = json!({"plugin":"todo","op":"toggle","ids":["a"]});
        args.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let e = bulk(&mut h, Caller::App, args, false, false).unwrap_err();
        assert_eq!(e.code, "invalid");
        assert!(e.message.contains(message), "{}", e.message);
    }
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap()["id"].clone();
    assert_eq!(
        bulk(
            &mut h,
            Caller::App,
            json!({"plugin":"todo","op":"toggle","ids":[a,a]}),
            false,
            false
        )
        .unwrap()
        .result["total"],
        1
    );
    let desc = commands::describe(&h, CHANGE);
    assert_eq!(
        desc.iter()
            .find(|c| c.id == "plugin.run_bulk")
            .unwrap()
            .params
            .as_ref()
            .unwrap()
            .len(),
        6
    );
}

#[test]
fn read_only_clients_cannot_export() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let path = data.path().join("forbidden.zip");
    assert_eq!(
        code(run(&mut host, READ, "space.export", json!({"path": path}))),
        "forbidden"
    );
    assert!(!path.exists());
}

#[test]
fn export_changes_nothing() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    run(&mut host, CHANGE, "todo.create", json!({"title": "Keep"})).unwrap();
    let before = query(&mut host, READ, "todo.tasks", json!({"filter": "all"})).unwrap();
    let dump = host.store_in("personal", "todo").unwrap().dump().unwrap();
    run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": data.path().join("unchanged.zip")}),
    )
    .unwrap();
    assert_eq!(
        before,
        query(&mut host, READ, "todo.tasks", json!({"filter": "all"})).unwrap()
    );
    assert_eq!(
        dump,
        host.store_in("personal", "todo").unwrap().dump().unwrap()
    );
}

#[test]
fn export_removes_partial_file_for_unopened_space() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let path = data.path().join("partial.zip");
    let result = commands::invoke(
        &mut host,
        Caller::App,
        Kind::Action,
        "space.export",
        Some("unopened"),
        &json!({"path": path}),
        true,
    );
    assert_eq!(code(result), "unknown-space");
    assert!(!path.exists());
}

#[test]
fn export_writes_csv_and_markdown() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for title in ["Pay, rent", "a/b"] {
        run(&mut host, CHANGE, "todo.create", json!({"title": title})).unwrap();
    }
    let path = data.path().join("out.zip");
    run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap();
    let entries = read_zip(&path);
    assert!(entries["files/todo/tasks.csv"].starts_with("title,due,done\r\n"));
    assert!(entries["files/todo/tasks.csv"].contains("\"Pay, rent\""));
    assert_eq!(entries["files/todo/notes/Pay, rent.md"], "Pay, rent");
    assert_eq!(entries["files/todo/notes/a-b.md"], "a/b");
    let manifest: Value = serde_json::from_str(&entries["manifest.json"]).unwrap();
    assert_eq!(manifest["plugins"][0]["files"], 3);
}

#[test]
fn markdown_names_are_unique() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for title in ["Same", "Same", "Same (2)"] {
        run(&mut host, CHANGE, "todo.create", json!({"title": title})).unwrap();
    }
    let path = data.path().join("out.zip");
    run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap();
    let entries = read_zip(&path);
    assert_eq!(entries["files/todo/notes/Same.md"], "Same");
    assert_eq!(entries["files/todo/notes/Same (2).md"], "Same");
    assert_eq!(entries["files/todo/notes/Same (2) (2).md"], "Same (2)");
}

#[test]
fn export_binds_clock_params_in_query_order() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let dir = data.path().join("todo");
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["queries"]["export_clock"] = json!({
        "sql": "SELECT ?1 AS zone, ?2 AS clock, ?3 AS day, ?4 AS weekday, ?5 AS offset",
        "params": ["tz", "now_ms", "today", "weekday", "utc_offset_ms"]
    });
    m["export"] = json!([{"query":"export_clock", "format":"csv", "path":"clock.csv", "columns":["zone", "clock", "day", "weekday", "offset", "missing"]}]);
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
    let path = data.path().join("out.zip");
    run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap();
    let entries = read_zip(&path);
    let lines: Vec<_> = entries["files/todo/clock.csv"].split("\r\n").collect();
    assert_eq!(lines[0], "zone,clock,day,weekday,offset,missing");
    let cells: Vec<_> = lines[1].split(',').collect();
    assert_eq!(cells[0], "Asia/Kolkata");
    assert!(cells[1].parse::<i64>().unwrap() > 0);
    assert_eq!(cells[2].len(), 10);
    assert!(!cells[3].is_empty());
    assert_eq!(cells[4], "19800000");
    assert_eq!(cells[5], "");
}

#[test]
fn export_query_failure_names_plugin_and_removes_archive() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let dir = data.path().join("todo");
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // Valid SQL whose evaluation fails, after the JSON has been written.
    m["queries"]["export_tasks"]["sql"] = json!("SELECT abs(-9223372036854775808) AS title");
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
    let path = data.path().join("out.zip");
    let error = run(
        &mut host,
        Caller::App,
        "space.export",
        json!({"path": path}),
    )
    .unwrap_err();
    assert_eq!(error.code, "failed");
    assert!(error.message.contains("todo"), "{error:?}");
    assert!(!path.exists());
}

#[test]
fn bulk_destructive_needs_confirmation_with_count() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap()["id"].clone();
    let b = run(&mut h, CHANGE, "todo.create", json!({"title":"B"})).unwrap()["id"].clone();
    let args = json!({"plugin":"todo","op":"delete","ids":[a,b]});
    let e = bulk(&mut h, CHANGE, args.clone(), false, false).unwrap_err();
    assert_eq!(e.code, "needs-confirmation");
    assert!(e.message.ends_with("This applies to 2 items."));
    let out = bulk(&mut h, CHANGE, args.clone(), true, false).unwrap();
    assert!(out.dry_run);
    assert_eq!(out.changes.iter().filter(|c| c.op == "delete").count(), 2);
    assert_eq!(task_titles(&mut h).len(), 2);
    bulk(&mut h, CHANGE, args, false, true).unwrap();
    assert!(task_titles(&mut h).is_empty());
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"C"})).unwrap()["id"].clone();
    assert_eq!(
        bulk(
            &mut h,
            Caller::App,
            json!({"plugin":"todo","op":"delete","ids":[a]}),
            false,
            false
        )
        .unwrap()
        .result["done"],
        1
    );
}

#[test]
fn bulk_read_client_is_forbidden() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    assert_eq!(
        bulk(
            &mut h,
            READ,
            json!({"plugin":"todo","op":"toggle","ids":["x"]}),
            false,
            false
        )
        .unwrap_err()
        .code,
        "forbidden"
    );
}

#[test]
fn bulk_dry_run_in_args_works_for_the_app() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap()["id"].clone();
    let out = bulk(
        &mut h,
        Caller::App,
        json!({"plugin":"todo","op":"delete","ids":[a],"dry_run":true}),
        false,
        false,
    )
    .unwrap();
    assert!(out.dry_run);
    assert_eq!(
        out.result["message"],
        "Dry run: 1 would be done. Nothing was kept."
    );
    assert_eq!(out.changes.len(), 1);
    assert_eq!(task_titles(&mut h), vec!["A"]);
}

#[test]
fn bulk_op_runs_once_for_all_ids() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    h.load(&limits_dir()).unwrap();
    let a = run(&mut h, Caller::App, "limits.note", json!({"text":"a"})).unwrap()["id"].clone();
    let b = run(&mut h, Caller::App, "limits.note", json!({"text":"b"})).unwrap()["id"].clone();
    let args = json!({"plugin":"limits","op":"mark","ids":[a,b],"args":{"text":"x"}});
    let out = bulk(&mut h, CHANGE, args, false, false).unwrap();
    assert_eq!(out.result["done"], 2);
    assert_eq!(out.result["items"][0]["result"]["marked"], 2);
    assert!(out.result["items"][1].get("result").is_none());
    let out = bulk(
        &mut h,
        Caller::App,
        json!({"plugin":"limits","op":"mark","ids":[a,"missing"],"args":{"text":"y"}}),
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.result["failed"], 2);
    let rows = query(&mut h, READ, "limits.all", json!({})).unwrap();
    assert!(rows.as_array().unwrap().iter().all(|r| r["note"] == "x"));
}

#[test]
fn bulk_where_uses_query_rows() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    let a = run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap()["id"].clone();
    run(&mut h, CHANGE, "todo.create", json!({"title":"B"})).unwrap();
    run(&mut h, CHANGE, "todo.toggle", json!({"id":a, "done":true})).unwrap();
    let out = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","where":{"query":"tasks","params":{"filter":"all"}},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap();
    assert_eq!(out.result["total"], 1);
    assert_eq!(out.result["query"], "tasks");
    let rows = query(&mut h, READ, "todo.tasks", json!({"filter":"all"})).unwrap();
    for row in rows.as_array().unwrap() {
        assert_eq!(row["done"], json!(1));
    }
}

fn import_host(data: &TempDir) -> PluginHost {
    static NOTES: OnceLock<PathBuf> = OnceLock::new();
    let from = NOTES.get_or_init(|| {
        let path = support::package_root()
            .join("notes")
            .canonicalize()
            .unwrap();
        build_plugin(&path).unwrap();
        path
    });
    let to = data.path().join("notes");
    for entry in walk(from) {
        let dest = to.join(entry.strip_prefix(from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["import"] = json!([{"id":"markdown", "label":"Notes from Markdown files", "format":"markdown", "op":"create", "fields":{"title":"$name", "body":"$text", "project":"$folder"}}]);
    // A deliberately declared importer may call a hidden operation.
    m["operations"]["create"]["cli"] = json!(false);
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    let mut host = host(data);
    host.load(&to).unwrap();
    host
}

fn import_args() -> Value {
    json!({"plugin":"notes", "importer":"markdown", "files":[
        {"path":"Work/Plan.md", "text":"Plan body"},
        {"path":"Ideas.md", "text":"Idea body"}
    ]})
}

fn imported_notes(host: &mut PluginHost) -> Vec<Value> {
    query(
        host,
        Caller::App,
        "notes.list",
        json!({"q":"", "project":"", "utc_offset_ms":0, "today":"2026-11-11"}),
    )
    .unwrap()
    .as_array()
    .unwrap()
    .clone()
}

#[test]
fn import_markdown_creates_notes() {
    for caller in [Caller::App, CHANGE] {
        let data = TempDir::new().unwrap();
        let mut host = import_host(&data);
        let result = run(&mut host, caller, "space.import", import_args()).unwrap();
        assert_eq!(result["done"], 2);
        assert_eq!(result["message"], "2 imported");
        let notes = imported_notes(&mut host);
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().any(|n| n["title"] == "Ideas"));
        assert!(notes
            .iter()
            .any(|n| n["title"] == "Plan" && n["project"] == "Work"));
    }
}

#[test]
fn bulk_where_is_checked() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap();

    let both = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","ids":["x"],"where":{"query":"tasks"},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(both.code, "invalid");

    let missing = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","where":{"query":"nope"},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(missing.code, "not-found");

    let bad_params = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","where":{"query":"tasks","params":{"filter":3}},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(bad_params.code, "invalid");

    let no_match = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","where":{"query":"tasks","params":{"filter":"all","q":"zzz-no-match"}},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(no_match.code, "invalid");
    assert!(no_match.message.contains("matches nothing"));
}

#[test]
fn bulk_where_hidden_query_is_not_found_for_clients() {
    let data = TempDir::new().unwrap();
    let mut h = host(&data);
    run(&mut h, CHANGE, "todo.create", json!({"title":"A"})).unwrap();

    let hidden = bulk(
        &mut h,
        CHANGE,
        json!({"plugin":"todo","op":"toggle","where":{"query":"badge"},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_eq!(hidden.code, "not-found");

    let app = bulk(
        &mut h,
        Caller::App,
        json!({"plugin":"todo","op":"toggle","where":{"query":"badge"},"args":{"done":true}}),
        false,
        false,
    )
    .unwrap_err();
    assert_ne!(app.code, "not-found");
}

#[test]
fn import_dry_run_keeps_nothing() {
    let data = TempDir::new().unwrap();
    let mut host = import_host(&data);
    for options_dry_run in [false, true] {
        let mut args = import_args();
        args["dry_run"] = json!(!options_dry_run);
        let out = run_with(
            &mut host,
            Caller::App,
            "space.import",
            args,
            options_dry_run,
            false,
        )
        .unwrap();
        assert!(out.dry_run);
        assert_eq!(out.result["done"], 2);
        assert!(out.result["message"]
            .as_str()
            .unwrap()
            .starts_with("Dry run:"));
        assert!(out.result["message"]
            .as_str()
            .unwrap()
            .ends_with(" Nothing was kept."));
        assert!(imported_notes(&mut host).is_empty());
    }
}

#[test]
fn import_errors() {
    let data = TempDir::new().unwrap();
    let mut host = import_host(&data);
    let mut args = import_args();
    args["importer"] = json!("unknown");
    let error = run(&mut host, Caller::App, "space.import", args).unwrap_err();
    assert_eq!(error.code, "not-found");
    assert_eq!(error.message, "no importer `notes.unknown`");
    assert_eq!(
        code(run(&mut host, READ, "space.import", import_args())),
        "forbidden"
    );
    let mut args = import_args();
    args["files"] = json!("x");
    assert_eq!(
        run(&mut host, Caller::App, "space.import", args).unwrap_err(),
        CommandError {
            code: "invalid".into(),
            message: "`files` must be a list of {path, text}".into()
        }
    );
}

#[test]
fn blob_put_then_get_round_trip() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.set_blob_key(BlobKey::new("space", &[9u8; 32]));
    let payload = b"hello attachments";
    let put = services::invoke(
        &mut host,
        "blob.put",
        &json!({
            "name": "hello.txt",
            "mime": "text/plain",
            "data_base64": B64.encode(payload),
        }),
        true,
    )
    .unwrap()
    .unwrap();
    let file: terrarium_core::FileRef = serde_json::from_str(put.as_str().unwrap()).unwrap();
    let get = services::invoke(&mut host, "blob.get", &json!({"blob": file.blob}), true)
        .unwrap()
        .unwrap();
    assert_eq!(get["mime"], "application/octet-stream");
    assert_eq!(
        B64.decode(get["data_base64"].as_str().unwrap()).unwrap(),
        payload
    );
}

#[test]
fn plugin_list_shows_importers() {
    let data = TempDir::new().unwrap();
    let mut host = import_host(&data);
    let plugins = query(&mut host, READ, "plugin.list", json!({})).unwrap();
    let plugins = plugins.as_array().unwrap();
    let notes = plugins.iter().find(|p| p["id"] == "notes").unwrap();
    assert_eq!(
        notes["imports"],
        json!([{"id":"markdown", "label":"Notes from Markdown files", "format":"markdown"}])
    );
    let todo = plugins.iter().find(|p| p["id"] == "todo").unwrap();
    assert_eq!(
        todo["imports"],
        json!([
            {"id":"tasks_csv", "label":"Tasks from a CSV file", "format":"csv"},
            {"id":"titles_csv", "label":"Task titles from a CSV file", "format":"csv"}
        ])
    );
}

#[test]
fn import_csv_reports_failures_and_keeps_successful_items() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    // Declare an importer on the test copy, keeping the real To-do create checks.
    let path = data.path().join("todo/manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["import"] = json!([{"id":"csv", "label":"Tasks", "format":"csv", "op":"create", "fields":{"title":"Title"}}]);
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    host.load(&data.path().join("todo")).unwrap();
    // Overlong titles fail plugin validation; a blank mapped row is skipped.
    let text = format!(
        "Title\nFirst\n{}\nLast\n,\n",
        ("z".repeat(501) + "\n").repeat(52)
    );
    let result = run(
        &mut host,
        CHANGE,
        "space.import",
        json!({"plugin":"todo", "importer":"csv", "files":[{"path":"tasks.csv", "text":text}]}),
    )
    .unwrap();
    assert_eq!(result["done"], 2);
    assert_eq!(result["failed"], 52);
    assert_eq!(result["skipped"], 2);
    assert_eq!(result["message"], "2 imported, 52 failed, 2 skipped");
    assert_eq!(result["failures"].as_array().unwrap().len(), 50);
    assert_eq!(result["failures"][0]["item"], "Row 3");
    let mut titles = task_titles(&mut host);
    titles.sort();
    assert_eq!(titles, ["First", "Last"]);
}

#[test]
fn blob_get_unknown_reports_missing() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.set_blob_key(BlobKey::new("space", &[9u8; 32]));
    let missing = services::invoke(
        &mut host,
        "blob.get",
        &json!({"blob":"b1-01234567-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(missing, json!({"missing": true}));
}

#[test]
fn blob_put_over_limit_fails() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.set_blob_key(BlobKey::new("space", &[9u8; 32]));
    let large = vec![0u8; MAX_BLOB as usize + 1];
    let err = services::invoke(
        &mut host,
        "blob.put",
        &json!({
            "name":"big.bin",
            "mime":"application/octet-stream",
            "data_base64": B64.encode(large),
        }),
        true,
    )
    .unwrap()
    .unwrap_err();
    assert!(err.contains("blob exceeds"));
}

#[test]
fn blob_put_fails_over_quota() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    host.set_blob_key(BlobKey::new("space", &[9u8; 32]));
    host.set_blob_quota_mb(1);
    let payload = vec![0u8; 2 * 1024 * 1024];
    let err = services::invoke(
        &mut host,
        "blob.put",
        &json!({
            "name":"big.bin",
            "mime":"application/octet-stream",
            "data_base64": B64.encode(payload),
        }),
        true,
    )
    .unwrap()
    .unwrap_err();
    assert!(err.contains("Attachments are full"));
}

fn history_args(id: &Value) -> Value {
    json!({"plugin": "todo", "table": "tasks", "id": id})
}

#[test]
fn history_lists_and_restores_versions() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = run(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Pay rent"}),
    )
    .unwrap()["id"]
        .clone();
    // Keep a listed cutoff before the next edit: nearby edits can group together.
    let first = query(&mut host, READ, "item.history", history_args(&id)).unwrap();
    assert_eq!(first["this_device"], "test-device");
    let version = first["versions"].as_array().unwrap().last().unwrap()["id"].clone();
    run(
        &mut host,
        CHANGE,
        "todo.update",
        json!({"id": id, "title": "Pay rent today"}),
    )
    .unwrap();
    let list = query(&mut host, READ, "item.history", history_args(&id)).unwrap();
    assert!(!list["versions"].as_array().unwrap().is_empty());
    let mut args = history_args(&id);
    args["version"] = version.clone();
    let old = query(&mut host, READ, "item.history", args.clone()).unwrap();
    assert_eq!(old["values"]["title"], "Pay rent");
    let restored = run(&mut host, CHANGE, "item.restore_version", args).unwrap();
    assert_eq!(restored["restored"], version);
    assert!(restored["columns"]
        .as_array()
        .unwrap()
        .contains(&json!("title")));
    assert_eq!(task_titles(&mut host), ["Pay rent"]);
    let after = query(&mut host, READ, "item.history", history_args(&id)).unwrap();
    assert_eq!(after["versions"].as_array().unwrap().len(), 2);
    assert_eq!(after["versions"][0]["restored"], true);
    assert_eq!(after["versions"][1], list["versions"][0]);
    // The value we replaced remains listed and readable after the restore.
    let mut args = history_args(&id);
    args["version"] = list["versions"][0]["id"].clone();
    assert_eq!(
        query(&mut host, READ, "item.history", args).unwrap()["values"]["title"],
        "Pay rent today"
    );
    let mut args = history_args(&id);
    args["version"] = after["versions"][1]["id"].clone();
    run(&mut host, CHANGE, "item.restore_version", args).unwrap();
    assert_eq!(task_titles(&mut host), ["Pay rent today"]);
}

#[test]
fn history_needs_a_declared_table() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for (plugin, table) in [("todo", "steps"), ("unknown", "tasks"), ("_pages", "pages")] {
        let args = json!({"plugin": plugin, "table": table, "id": "missing", "version": "bad"});
        assert_eq!(
            code(query(&mut host, READ, "item.history", args.clone())),
            "not-found"
        );
        assert_eq!(
            code(run(&mut host, CHANGE, "item.restore_version", args)),
            "not-found"
        );
    }
    let empty = query(
        &mut host,
        READ,
        "item.history",
        history_args(&json!("missing")),
    )
    .unwrap();
    assert_eq!(empty["versions"], json!([]));
}

#[test]
fn read_clients_cannot_restore() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let mut args = history_args(&json!("missing"));
    args["version"] = json!("bad");
    assert_eq!(
        code(run(&mut host, READ, "item.restore_version", args)),
        "forbidden"
    );
}

#[test]
fn limit_is_checked() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for limit in [
        json!(0),
        json!(201),
        json!(-1),
        json!(1.5),
        json!("5"),
        Value::Null,
    ] {
        let mut args = history_args(&json!("missing"));
        args["limit"] = limit;
        assert_eq!(
            code(query(&mut host, READ, "item.history", args.clone())),
            "invalid"
        );
        assert_eq!(
            code(query(&mut host, Caller::App, "item.history", args)),
            "invalid"
        );
    }
}

#[test]
fn history_rejects_fabricated_and_other_row_versions() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id =
        run(&mut host, CHANGE, "todo.create", json!({"title": "First"})).unwrap()["id"].clone();
    let version = query(&mut host, READ, "item.history", history_args(&id)).unwrap()["versions"][0]
        ["id"]
        .clone();
    let other =
        run(&mut host, CHANGE, "todo.create", json!({"title": "Second"})).unwrap()["id"].clone();
    for (target, cutoff, expected) in [
        (id.clone(), json!("bad"), "invalid"),
        (
            id.clone(),
            json!("ffffffffffffffff-ffffffff-future"),
            "not-found",
        ),
        (other, version, "not-found"),
        (json!(""), json!("bad"), "invalid"),
    ] {
        let mut args = history_args(&target);
        args["version"] = cutoff;
        assert_eq!(
            code(query(&mut host, READ, "item.history", args.clone())),
            expected
        );
        assert_eq!(
            code(run(&mut host, CHANGE, "item.restore_version", args)),
            expected
        );
    }
    assert_eq!(task_titles(&mut host).len(), 2);
}

#[test]
fn history_stays_in_the_selected_space() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = run(
        &mut host,
        CHANGE,
        "todo.create",
        json!({"title": "Personal"}),
    )
    .unwrap()["id"]
        .clone();
    let mut args = history_args(&id);
    let version =
        query(&mut host, READ, "item.history", args.clone()).unwrap()["versions"][0]["id"].clone();
    let work = run(&mut host, CHANGE, "space.create", json!({"name": "Work"})).unwrap();
    let work = work["id"].as_str().unwrap();
    let empty = commands::invoke(
        &mut host,
        READ,
        Kind::Query,
        "item.history",
        Some(work),
        &args,
        true,
    )
    .unwrap();
    assert_eq!(empty["versions"], json!([]));
    args["version"] = version;
    for kind in [Kind::Query, Kind::Action] {
        let command = if kind == Kind::Query {
            "item.history"
        } else {
            "item.restore_version"
        };
        assert_eq!(
            code(commands::invoke(
                &mut host,
                CHANGE,
                kind,
                command,
                Some(work),
                &args,
                true
            )),
            "not-found"
        );
    }
    assert_eq!(
        code(commands::invoke(
            &mut host,
            READ,
            Kind::Query,
            "item.history",
            Some("missing"),
            &args,
            true
        )),
        "unknown-space"
    );
    assert_eq!(task_titles(&mut host), ["Personal"]);
}

#[test]
fn restoring_a_purged_item_has_a_clear_error() {
    use terrarium_core::StoreSet;
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = run(&mut host, CHANGE, "todo.create", json!({"title": "Old"})).unwrap()["id"].clone();
    let mut args = history_args(&id);
    args["version"] =
        query(&mut host, READ, "item.history", args.clone()).unwrap()["versions"][0]["id"].clone();
    run(&mut host, Caller::App, "todo.delete", json!({"id": id})).unwrap();
    let store = host.store_mut("todo").unwrap();
    assert_eq!(store.purge(&[], 0).unwrap(), 1);
    let err = run(&mut host, CHANGE, "item.restore_version", args).unwrap_err();
    assert_eq!(err.code, "not-found");
    assert_eq!(
        err.message,
        "This item can't be restored; it was removed for good."
    );
}

#[test]
fn history_reports_retention() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    assert_eq!(
        code(run(
            &mut host,
            READ,
            "space.set_history",
            json!({"id": "personal", "days": 30})
        )),
        "forbidden"
    );
    let result = run(
        &mut host,
        CHANGE,
        "space.set_history",
        json!({"id": "personal", "days": 30}),
    )
    .unwrap();
    assert_eq!(result, json!({"id": "personal", "history_days": 30}));
    for days in [json!(7), json!(-1), json!(4294967326u64), json!("30")] {
        assert!(run(
            &mut host,
            CHANGE,
            "space.set_history",
            json!({"id": "personal", "days": days})
        )
        .is_err());
    }
    let id =
        run(&mut host, CHANGE, "todo.create", json!({"title": "Fresh"})).unwrap()["id"].clone();
    let result = query(&mut host, READ, "item.history", history_args(&id)).unwrap();
    assert_eq!(result["keeps_days"], 30);
    assert_eq!(result["versions"].as_array().unwrap().len(), 1);
}

#[test]
fn history_retention_hides_old_groups_and_refuses_expired_current_details() {
    use terrarium_core::{Batch, Change, Hlc, Op, StoreSet};
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let at = |days: u64, row: &str| Hlc {
        wall_ms: now - days * 86_400_000,
        counter: 0,
        device: terrarium_core::DeviceId(format!("old-{row}")),
    };
    let mut changes = Vec::new();
    for (row, days) in [
        ("expired", 40),
        ("expired", 31),
        ("mixed", 40),
        ("mixed", 1),
    ] {
        changes.push(Change {
            hlc: at(days, row),
            user: "owner".into(),
            table: "tasks".into(),
            row: row.into(),
            schema_version: 1,
            op: Op::Set {
                column: "title".into(),
                value: json!("Historical"),
                base: None,
            },
        });
    }
    // Create each item before its edits, without using the present-day clock.
    for row in ["expired", "mixed"] {
        changes.push(Change {
            hlc: at(41, row),
            user: "owner".into(),
            table: "tasks".into(),
            row: row.into(),
            schema_version: 1,
            op: Op::Create,
        });
    }
    host.store_mut("todo")
        .unwrap()
        .import(&Batch {
            plugin: "todo".into(),
            sender: "old-device".into(),
            sender_seen: Default::default(),
            changes,
        })
        .unwrap();
    run(
        &mut host,
        CHANGE,
        "space.set_history",
        json!({"id": "personal", "days": 30}),
    )
    .unwrap();
    let future = json!({"plugin": "todo", "table": "tasks", "id": "expired",
        "version": "ffffffffffffffff-00000000-old-expired"});
    assert_eq!(
        code(query(&mut host, READ, "item.history", future.clone())),
        "not-found"
    );
    assert_eq!(
        code(run(&mut host, CHANGE, "item.restore_version", future)),
        "not-found"
    );
    for (row, latest_days) in [("expired", 31), ("mixed", 1)] {
        let mut args = history_args(&json!(row));
        args["limit"] = json!(1);
        let result = query(&mut host, READ, "item.history", args.clone()).unwrap();
        assert_eq!(result["versions"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["versions"][0]["id"],
            at(latest_days, row).to_string()
        );
        for days in [40, latest_days] {
            args["version"] = json!(at(days, row).to_string());
            if days > 30 {
                assert_eq!(
                    code(query(&mut host, READ, "item.history", args.clone())),
                    "invalid"
                );
                assert_eq!(
                    code(run(&mut host, CHANGE, "item.restore_version", args.clone())),
                    "invalid"
                );
            } else {
                assert_eq!(
                    query(&mut host, READ, "item.history", args.clone()).unwrap()["values"]
                        ["title"],
                    "Historical"
                );
            }
        }
    }
    // Visibility can be widened again: data was never erased.
    run(
        &mut host,
        CHANGE,
        "space.set_history",
        json!({"id": "personal", "days": 0}),
    )
    .unwrap();
    let args = json!({"plugin": "todo", "table": "tasks", "id": "expired", "version": at(40, "expired").to_string()});
    assert!(query(&mut host, READ, "item.history", args).is_ok());
}
