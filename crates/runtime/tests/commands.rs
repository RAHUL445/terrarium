//! The command registry over a real plugin: what callers outside the app
//! can see and run, argument checks, host commands and search.

mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_runtime::commands::{self, Caller, CommandError, Kind, Scope};
use terrarium_runtime::{build_plugin, HostConfig, Manifest, PluginHost};

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
    m["operations"]["toggle"]["bulk"] = json!(true);
    m["operations"]["delete"]["bulk"] = json!(true);
    m["open"] = json!({ "page": "main" });
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
    ] {
        assert!(find(Kind::Query, id).is_some(), "{id}");
    }
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
