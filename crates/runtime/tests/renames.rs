//! A renamed plugin (`tasks`, formerly `todo`) keeps its data, sync id and
//! every way the old id was used.
mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::commands::{self, Caller, Kind, Scope};
use terrarium_runtime::Placement;
use terrarium_runtime::{build_plugin, services, HostConfig, PluginHost, ViewKind, DEFAULT_SPACE};

fn package(name: &str) -> PathBuf {
    static BUILT: OnceLock<()> = OnceLock::new();
    let dir = |n: &str| support::package_root().join(n).canonicalize().unwrap();
    BUILT.get_or_init(|| {
        for n in ["tasks", "notes"] {
            build_plugin(&dir(n)).unwrap();
        }
    });
    dir(name)
}

fn copy(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == "target" || name == "src" || name == "tests" {
                continue;
            }
            copy(&path, &to.join(&name));
        } else {
            std::fs::create_dir_all(to).unwrap();
            std::fs::copy(&path, to.join(&name)).unwrap();
        }
    }
}

/// The plugin as an older build shipped it: id `todo`, no `former_ids`.
fn old_todo(root: &Path) -> PathBuf {
    let dir = root.join("old-todo");
    copy(&package("tasks"), &dir);
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["id"] = json!("todo");
    m["name"] = json!("To-do");
    m.as_object_mut().unwrap().remove("former_ids");
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    dir
}

fn host(root: &Path, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: root.join(device).join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap()
}

fn titles(host: &PluginHost, plugin: &str) -> Vec<String> {
    let params = json!({"today": "2026-11-15", "filter": "everything", "q": ""});
    let mut out: Vec<String> = host
        .query(plugin, "tasks", params.as_object().unwrap())
        .unwrap()
        .into_iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn create(host: &mut PluginHost, plugin: &str, title: &str) -> String {
    host.call(plugin, "create", &json!({ "title": title }))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn command(host: &mut PluginHost, kind: Kind, id: &str, args: Value) -> Value {
    commands::invoke(host, Caller::App, kind, id, None, &args, true)
        .unwrap_or_else(|e| panic!("{id}: {}", e.message))
}

#[test]
fn the_renamed_plugin_opens_the_old_database_and_answers_to_both_ids() {
    let root = TempDir::new().unwrap();
    let old = old_todo(root.path());
    let mut before = host(root.path(), "mac");
    before.load(&old).unwrap();
    create(&mut before, "todo", "Written by the old build");
    drop(before);

    let mut after = host(root.path(), "mac");
    let m = after.load(&package("tasks")).unwrap();
    assert_eq!((m.id.as_str(), m.storage_id()), ("tasks", "todo"));
    let dir = root.path().join("mac").join("plugins");
    assert!(dir.join("todo.db").exists());
    assert!(!dir.join("tasks.db").exists(), "no new database is made");
    assert_eq!(titles(&after, "tasks"), ["Written by the old build"]);
    // The former id still names it, for host calls and commands.
    create(&mut after, "todo", "Through the old id");
    let id = create(&mut after, "tasks", "Through the new id");
    assert_eq!(titles(&after, "todo").len(), 3);
    assert_eq!(after.canonical("todo"), "tasks");
    assert_eq!(after.storage_plugin_id("tasks"), "todo");
    assert!(after.manifest("todo").is_some());
    let done = command(
        &mut after,
        Kind::Action,
        "todo.toggle",
        json!({"id": id, "done": true}),
    );
    assert!(done.is_object());
    let rows = command(
        &mut after,
        Kind::Query,
        "todo.tasks",
        json!({"today":"2026-11-15","filter":"done","q":""}),
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let described = commands::describe(&after, Caller::App);
    let create_info = described.iter().find(|c| c.id == "tasks.create").unwrap();
    assert_eq!(create_info.former_plugins, ["todo"]);
    assert!(described.iter().all(|c| !c.id.starts_with("todo.")));
    after
        .template("todo", ViewKind::Widget, "today")
        .expect("templates resolve by the former id");

    // An older `todo` package can't load beside its successor.
    let error = after.load(&old).unwrap_err().to_string();
    assert!(error.contains("renamed to `tasks`"), "{error}");
}

#[test]
fn sync_keeps_the_todo_store_id_in_both_directions() {
    let root = TempDir::new().unwrap();
    let mut old = host(root.path(), "phone");
    old.load(&old_todo(root.path())).unwrap();
    let mut new = host(root.path(), "mac");
    new.load(&package("tasks")).unwrap();

    let ids = new.plugin_ids();
    assert!(ids.contains(&"todo".to_string()));
    assert!(!ids.contains(&"tasks".to_string()));
    assert!(new.store_mut("tasks").is_none());

    // Old peer → new peer.
    create(&mut old, "todo", "From the phone");
    let (batch, _) = old.store("todo").unwrap().export_since(0).unwrap();
    assert_eq!(batch.plugin, "todo");
    new.store_mut("todo").unwrap().import(&batch).unwrap();
    assert_eq!(titles(&new, "tasks"), ["From the phone"]);

    // New peer → old peer: the batch is still labelled `todo`.
    create(&mut new, "tasks", "From the mac");
    let (batch, _) = new.store("tasks").unwrap().export_since(0).unwrap();
    assert_eq!(batch.plugin, "todo");
    old.store_mut("todo").unwrap().import(&batch).unwrap();
    assert_eq!(titles(&old, "todo"), ["From the mac", "From the phone"]);

    // Other spaces use `todo@<space>`.
    let work = new.spaces_mut().create("Work").unwrap();
    new.sync_spaces().unwrap();
    assert!(new.plugin_ids().contains(&format!("todo@{}", work.id)));
    assert!(new.store_mut(&format!("todo@{}", work.id)).is_some());
}

#[test]
fn events_and_reminders_keep_the_storage_id_and_rules_match_either_id() {
    let root = TempDir::new().unwrap();
    let mut h = host(root.path(), "mac");
    h.load(&package("tasks")).unwrap();
    let id = create(&mut h, "tasks", "Gym");
    h.call("tasks", "toggle", &json!({"id": id, "done": true}))
        .unwrap();
    let events = h.pending_events(10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].plugin, "todo",
        "events stay readable by older devices"
    );
    assert!(h.same_plugin("tasks", &events[0].plugin));

    let at = terrarium_runtime_now() + 3_600_000;
    let due = h
        .call(
            "tasks",
            "create",
            &json!({"title": "Remind", "due": "2030-01-01", "remind_time": "09:00"}),
        )
        .unwrap();
    assert!(due["id"].is_string());
    for (_, plugin, _) in h.reminders(at) {
        assert_eq!(plugin, "todo", "reminder keys don't change");
    }
}

fn terrarium_runtime_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn refs_backlinks_and_links_resolve_by_either_id() {
    let root = TempDir::new().unwrap();
    let mut h = host(root.path(), "mac");
    h.load(&package("tasks")).unwrap();
    h.load(&package("notes")).unwrap();
    let task = create(&mut h, "tasks", "Lamp");
    let old_ref = format!("todo/tasks/{task}");
    let new_ref = format!("tasks/tasks/{task}");

    let resolved = command(
        &mut h,
        Kind::Query,
        "ref.resolve",
        json!({"refs": [old_ref, new_ref]}),
    );
    for (i, r) in resolved.as_array().unwrap().iter().enumerate() {
        assert_eq!(r["state"], "ok", "{i}: {r}");
        assert_eq!(r["title"], "Lamp");
        assert_eq!(r["plugin"], "tasks");
        assert_eq!(r["open"]["plugin"], "tasks");
    }
    assert_eq!(resolved[0]["ref"], json!(old_ref));
    assert_eq!(resolved[1]["ref"], json!(new_ref));

    // One note mentions the old form, another the new one.
    let mut notes = Vec::new();
    for to in [&old_ref, &new_ref] {
        let id = h
            .call(
                "notes",
                "create",
                &json!({"title": "N", "body": format!("See [T](terrarium:{to})")}),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        notes.push(format!("notes/notes/{id}"));
    }
    for to in [&old_ref, &new_ref] {
        let found = commands::invoke(
            &mut h,
            Caller::Client(Scope::Read),
            Kind::Query,
            "ref.backlinks",
            None,
            &json!({ "ref": to }),
            true,
        )
        .unwrap();
        let mut mentions: Vec<String> = serde_json::from_value(found["mentions"].clone()).unwrap();
        mentions.sort();
        let mut want = notes.clone();
        want.sort();
        assert_eq!(mentions, want, "backlinks of {to}");
    }

    // Search hands out refs under the storage id; the hit names the plugin by its current id.
    let found = command(
        &mut h,
        Kind::Query,
        "search.query",
        json!({"q": "Lamp", "plugins": ["todo"]}),
    );
    let hit = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(task))
        .unwrap();
    assert_eq!(hit["ref"], json!(old_ref));
    assert_eq!(hit["plugin"], "tasks");

    // Links stored under either form are listed for both.
    let note = notes[0].clone();
    command(
        &mut h,
        Kind::Action,
        "link.add",
        json!({"source": note, "target": old_ref}),
    );
    for item in [&old_ref, &new_ref] {
        let links = command(&mut h, Kind::Query, "link.list", json!({ "ref": item }));
        assert_eq!(links.as_array().unwrap().len(), 1, "links of {item}");
        assert_eq!(links[0]["direction"], "in");
        assert_eq!(links[0]["other"], json!(note));
    }
}

#[test]
fn page_widgets_and_views_naming_todo_still_render_and_stay_stored_as_todo() {
    let root = TempDir::new().unwrap();
    let mut h = host(root.path(), "mac");
    h.load(&package("tasks")).unwrap();
    create(&mut h, "tasks", "On the dashboard");
    let page = h
        .pages_mut()
        .create(DEFAULT_SPACE, "Dashboard", None)
        .unwrap();
    let placement = |plugin: &str, widget: &str, config: Value| -> Placement {
        serde_json::from_value(json!({
            "plugin": plugin, "widget": widget, "x": 0, "y": 0, "w": 4, "h": 3,
            "config": config
        }))
        .unwrap()
    };
    // As the user's synced rows have them.
    h.pages_mut()
        .save(
            &page.id,
            vec![
                placement("todo", "today", json!({})),
                placement(
                    "_host",
                    "view",
                    json!({"plugin": "todo", "query": "tasks", "layout": "list"}),
                ),
            ],
        )
        .unwrap();
    let state = services::invoke(&mut h, "page_get", &json!({"page": page.id}), true)
        .unwrap()
        .unwrap();
    let shown = state["placements"].as_array().unwrap();
    let widget = shown.iter().find(|p| p["widget"] == "today").unwrap();
    assert_eq!(widget["plugin"], "tasks");
    let view = shown.iter().find(|p| p["widget"] == "view").unwrap();
    assert_eq!(view["config"]["plugin"], "tasks");
    // What the UI then does with them.
    h.template("tasks", ViewKind::Widget, "today").unwrap();
    assert_eq!(
        titles(&h, view["config"]["plugin"].as_str().unwrap()),
        ["On the dashboard"]
    );

    // Saving what the UI shows writes `todo` back, unchanged for older devices.
    services::invoke(
        &mut h,
        "page_save",
        &json!({"page": page.id, "placements": shown}),
        true,
    )
    .unwrap()
    .unwrap();
    let stored = h.pages().placements(&page.id).unwrap();
    assert!(stored
        .iter()
        .any(|p| p.plugin == "todo" && p.widget == "today"));
    let stored_view = stored.iter().find(|p| p.widget == "view").unwrap();
    assert_eq!(stored_view.config["plugin"], "todo");
    // A newly placed widget is stored under the storage id too.
    command(
        &mut h,
        Kind::Action,
        "page.place",
        json!({"page": page.id, "widget": "tasks/quick-add"}),
    );
    assert!(h
        .pages()
        .placements(&page.id)
        .unwrap()
        .iter()
        .any(|p| p.plugin == "todo" && p.widget == "quick-add"));

    // Saved views: shown as `tasks`, stored as `todo`.
    let saved = services::invoke(
        &mut h,
        "view_save",
        &json!({"space": DEFAULT_SPACE, "title": "Open", "config": {"plugin": "tasks", "query": "tasks", "layout": "list"}}),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(saved["config"]["plugin"], "tasks");
    let raw = h.pages().list_saved_views(DEFAULT_SPACE).unwrap();
    assert_eq!(raw[0].config["plugin"], "todo");
    let listed = services::invoke(&mut h, "views_list", &json!({"space": DEFAULT_SPACE}), true)
        .unwrap()
        .unwrap();
    assert_eq!(listed[0]["config"]["plugin"], "tasks");
}
