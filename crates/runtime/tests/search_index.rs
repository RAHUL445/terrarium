mod support;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{build_plugin, HostConfig, IndexProgress, PluginHost};
fn todo_dir() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = support::package_root()
                .join("tasks")
                .canonicalize()
                .unwrap();
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

fn indexed_todo(root: &Path, kind: &str) -> PathBuf {
    let from = todo_dir();
    let to = root.join("todo");
    for entry in walk(&from) {
        let dest = to.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["index"] = json!([{"table":"tasks","query":"index_tasks","kind":kind}]);
    m["queries"]["index_tasks"] = json!({"sql":"SELECT id, title, notes AS body, tags AS keywords, CASE WHEN done = 1 THEN 'Done' ELSE due END AS subtitle FROM tasks WHERE _deleted IS NULL AND cleared_at IS NULL AND id IN (SELECT value FROM json_each(?1))","params":["ids"]});
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    to
}
fn host(root: &TempDir, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: root.path().join(device).join("data"),
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
fn setup() -> (TempDir, PluginHost) {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    host.load(&indexed_todo(&root.path().join("package"), "Task"))
        .unwrap();
    (root, host)
}
fn catch_up(host: &mut PluginHost) {
    assert!(
        host.index_catch_up("personal", "todo", 100_000)
            .unwrap()
            .done
    );
}
fn create(host: &mut PluginHost, title: &str) -> String {
    host.call("todo", "create", &json!({"title":title}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}
#[test]
fn local_writes_are_indexed() {
    let (_root, mut host) = setup();
    let a = create(&mut host, "Lamp");
    let b = create(&mut host, "Chair");
    catch_up(&mut host);
    let mut expected = vec![(a, "Task".into()), (b, "Task".into())];
    expected.sort();
    assert_eq!(host.indexed_items("personal", "todo").unwrap(), expected);
    assert!(!host.plugin_ids().iter().any(|id| id == "index"));
}
#[test]
fn trash_restore_and_filtered_rows_follow_the_query() {
    let (_root, mut host) = setup();
    let id = create(&mut host, "Lamp");
    catch_up(&mut host);
    host.call("todo", "delete", &json!({"id":id})).unwrap();
    catch_up(&mut host);
    assert!(host.indexed_items("personal", "todo").unwrap().is_empty());
    host.call("todo", "restore", &json!({"id":id})).unwrap();
    catch_up(&mut host);
    assert_eq!(host.indexed_items("personal", "todo").unwrap().len(), 1);
    host.call("todo", "toggle", &json!({"id":id,"done":true}))
        .unwrap();
    host.call("todo", "clear_done", &json!({})).unwrap();
    catch_up(&mut host);
    assert!(host.indexed_items("personal", "todo").unwrap().is_empty());
}
#[test]
fn imported_changes_are_indexed() {
    let (root, mut a) = setup();
    let mut b = host(&root, "b");
    b.load(&indexed_todo(&root.path().join("package-b"), "Task"))
        .unwrap();
    catch_up(&mut b);
    let id = create(&mut a, "Shared lamp");
    let (batch, _) = a.store_mut("todo").unwrap().export_since(0).unwrap();
    b.store_mut("todo").unwrap().import(&batch).unwrap();
    catch_up(&mut b);
    assert_eq!(
        b.indexed_items("personal", "todo").unwrap(),
        vec![(id, "Task".into())]
    );
}
#[test]
fn a_changed_declaration_rebuilds() {
    let (root, mut host) = setup();
    let id = create(&mut host, "Lamp");
    catch_up(&mut host);
    host.load(&indexed_todo(&root.path().join("package"), "Job"))
        .unwrap();
    catch_up(&mut host);
    assert_eq!(
        host.indexed_items("personal", "todo").unwrap(),
        vec![(id, "Job".into())]
    );
}
#[test]
fn a_large_plugin_is_indexed_in_chunks() {
    let (_root, mut host) = setup();
    host.store_mut("todo")
        .unwrap()
        .write(|w| {
            for i in 0..10_000 {
                w.insert(
                    "tasks",
                    json!({"title":format!("Lamp {i}")}).as_object().unwrap(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let p = host.index_catch_up("personal", "todo", 1_000).unwrap();
    assert!(!p.done);
    assert!(p.rows <= 1_500);
    let mut done = false;
    for _ in 0..30 {
        if host.index_catch_up("personal", "todo", 1_000).unwrap().done {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(
        host.indexed_items("personal", "todo").unwrap().len(),
        10_000
    );
}
#[test]
fn unloading_forgets_and_no_index_is_skipped() {
    let (root, mut host) = setup();
    create(&mut host, "Lamp");
    catch_up(&mut host);
    assert!(host.unload("tasks"));
    assert!(host.indexed_items("personal", "todo").unwrap().is_empty());
    let dir = indexed_todo(&root.path().join("plain"), "Task");
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m.as_object_mut().unwrap().remove("index");
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
    assert_eq!(
        host.index_catch_up("personal", "todo", 1_000).unwrap(),
        IndexProgress {
            rows: 0,
            done: true
        }
    );
}
fn search(host: &mut PluginHost, q: &str) -> Value {
    terrarium_runtime::commands::invoke(
        host,
        terrarium_runtime::commands::Caller::Client(terrarium_runtime::commands::Scope::Read),
        terrarium_runtime::commands::Kind::Query,
        "search.query",
        None,
        &json!({"q":q}),
        true,
    )
    .unwrap()
}
#[test]
fn indexed_hits_carry_kind_ref_and_snippet() {
    let (_root, mut host) = setup();
    let id = host
        .call(
            "todo",
            "create",
            &json!({"title":"Buy desk lamp","notes":"remember the lamp in the hall"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    catch_up(&mut host);
    let found = search(&mut host, "lamp");
    let hit = &found["results"][0];
    assert_eq!(hit["kind"], "Task");
    assert_eq!(hit["ref"], format!("todo/tasks/{id}"));
    assert!(hit["snippet"].as_str().unwrap().contains("lamp"));
    assert_eq!(hit["open"]["item"], id);
    assert_eq!(found["truncated"], false);
}
#[test]
fn prefix_and_operator_text_work() {
    let (_root, mut host) = setup();
    create(&mut host, "Desk lamp");
    catch_up(&mut host);
    assert_eq!(
        search(&mut host, "lam")["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for q in ["lamp OR", "\""] {
        assert!(search(&mut host, q)["errors"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
#[test]
fn an_unready_index_falls_back() {
    let (_root, mut host) = setup();
    create(&mut host, "Desk lamp");
    // Keep the rebuild larger than the search catch-up budget.
    host.store_mut("todo")
        .unwrap()
        .write(|w| {
            for i in 0..3_000 {
                w.insert(
                    "tasks",
                    json!({"title":format!("Filler {i}")}).as_object().unwrap(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let found = search(&mut host, "lamp");
    let hit = &found["results"][0];
    assert_eq!(found["results"].as_array().unwrap().len(), 1);
    assert!(hit["kind"].is_null());
    assert!(hit.get("ref").is_none());
}
#[test]
fn fallback_hits_merge_after_equal_indexed_hits() {
    let (root, mut host) = setup();
    create(&mut host, "lamp");
    catch_up(&mut host);
    let from = support::package_root().join("notes");
    build_plugin(&from).unwrap();
    let to = root.path().join("notes");
    for entry in walk(&from) {
        let dest = to.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m.as_object_mut().unwrap().remove("index");
    std::fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&to).unwrap();
    host.call("notes", "create", &json!({"body":"lamp"}))
        .unwrap();
    let found = search(&mut host, "lamp");
    assert_eq!(found["results"].as_array().unwrap().len(), 2);
    assert_eq!(found["results"][0]["plugin"], "tasks");
    assert_eq!(found["results"][1]["plugin"], "notes");
}
#[test]
fn indexed_truncation_is_exact() {
    let (_root, mut host) = setup();
    create(&mut host, "Lamp one");
    create(&mut host, "Lamp two");
    catch_up(&mut host);
    for (limit, truncated) in [(1, true), (2, false)] {
        let found = terrarium_runtime::commands::search(
            &mut host,
            "lamp",
            &["personal".into()],
            None,
            limit,
        );
        assert_eq!(found["truncated"], truncated);
        assert_eq!(found["results"].as_array().unwrap().len(), limit);
    }
}
#[test]
fn search_catches_up_first() {
    let (_root, mut host) = setup();
    create(&mut host, "Lamp");
    let found = search(&mut host, "lamp");
    assert_eq!(found["results"][0]["kind"], "Task");
    assert_eq!(found["indexing"], json!([]));
}
#[test]
fn a_big_rebuild_reports_indexing() {
    let (_root, mut host) = setup();
    host.store_mut("todo")
        .unwrap()
        .write(|w| {
            for i in 0..10_000 {
                w.insert(
                    "tasks",
                    json!({"title":format!("Lamp {i}")}).as_object().unwrap(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let found = search(&mut host, "lamp");
    assert_eq!(found["indexing"], json!(["tasks"]));
    assert!(!found["results"].as_array().unwrap().is_empty());
    assert!(found["results"][0]["kind"].is_null());
    let mut done = false;
    for _ in 0..50 {
        if !host.index_step(terrarium_runtime::INDEX_STEP_ROWS) {
            done = true;
            break;
        }
    }
    assert!(done);
    let found = search(&mut host, "lamp");
    assert_eq!(found["indexing"], json!([]));
    assert_eq!(found["results"][0]["kind"], "Task");
}
#[test]
fn index_step_covers_new_spaces() {
    let (_root, mut host) = setup();
    let work = host.spaces_mut().create("Work").unwrap();
    host.sync_spaces().unwrap();
    let id = host
        .call_in(&work.id, "todo", "create", &json!({"title":"Lamp"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..50 {
        if !host.index_step(terrarium_runtime::INDEX_STEP_ROWS) {
            break;
        }
    }
    assert!(host
        .indexed_items(&work.id, "todo")
        .unwrap()
        .contains(&(id, "Task".into())));
}
#[test]
fn catch_up_respects_small_remaining_budgets() {
    let (_root, mut host) = setup();
    for _ in 0..5 {
        create(&mut host, "Lamp");
    }
    assert!(host.index_catch_up("personal", "todo", 1).unwrap().rows <= 1);
    catch_up(&mut host);
    create(&mut host, "Chair");
    assert!(host.index_catch_up("personal", "todo", 1).unwrap().rows <= 1);
}
#[test]
fn deleted_index_is_rebuilt_while_host_runs() {
    let (root, mut host) = setup();
    let id = create(&mut host, "Lamp");
    catch_up(&mut host);
    std::fs::remove_file(root.path().join("a/data/index/personal.db")).unwrap();
    let found = search(&mut host, "lamp");
    assert_eq!(found["results"][0]["id"], id);
    assert_eq!(found["results"][0]["kind"], "Task");
    assert!(root.path().join("a/data/index/personal.db").exists());
}

fn indexed_notes(root: &Path) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let from = BUILT.get_or_init(|| {
        let dir = support::package_root()
            .join("notes")
            .canonicalize()
            .unwrap();
        build_plugin(&dir).unwrap();
        dir
    });
    let to = root.join("notes");
    for entry in walk(from) {
        let dest = to.join(entry.strip_prefix(from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["index"] = json!([{"table":"notes","query":"index_notes","kind":"Note"}]);
    m["queries"]["index_notes"] = json!({"sql":"SELECT id,title,body FROM notes WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1))","params":["ids"],"cli":false});
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    to
}
fn backlinks(
    host: &mut PluginHost,
    space: Option<&str>,
    to: &str,
) -> Result<Value, terrarium_runtime::commands::CommandError> {
    use terrarium_runtime::commands::{invoke, Caller, Kind, Scope};
    invoke(
        host,
        Caller::Client(Scope::Read),
        Kind::Query,
        "ref.backlinks",
        space,
        &json!({"ref":to}),
        true,
    )
}
#[test]
fn notes_that_mention_a_task_are_backlinks() {
    let (root, mut host) = setup();
    host.load(&indexed_notes(&root.path().join("notes-package")))
        .unwrap();
    let task = create(&mut host, "Lamp");
    let to = format!("todo/tasks/{task}");
    let note = host
        .call(
            "notes",
            "create",
            &json!({"title":"Note", "body":format!("See [T](terrarium:{to})")}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let found = backlinks(&mut host, None, &to).unwrap();
    assert_eq!(found["mentions"], json!([format!("notes/notes/{note}")]));
    assert_eq!(found["indexing"], json!([]));
    let work = host.spaces_mut().create("Work").unwrap();
    host.sync_spaces().unwrap();
    assert_eq!(
        backlinks(&mut host, Some(&work.id), &to).unwrap()["mentions"],
        json!([])
    );
    host.call("notes", "update", &json!({"id":note,"body":"No link"}))
        .unwrap();
    assert_eq!(
        backlinks(&mut host, None, &to).unwrap()["mentions"],
        json!([])
    );
    host.call(
        "notes",
        "update",
        &json!({"id":note,"body":format!("[T](terrarium:{to})")}),
    )
    .unwrap();
    assert_eq!(
        backlinks(&mut host, None, &to).unwrap()["mentions"],
        json!([format!("notes/notes/{note}")])
    );
    host.call("notes", "delete", &json!({"id":note})).unwrap();
    assert_eq!(
        backlinks(&mut host, None, &to).unwrap()["mentions"],
        json!([])
    );
}
#[test]
fn bad_refs_are_invalid() {
    let (_root, mut host) = setup();
    let error = backlinks(&mut host, None, "nope").unwrap_err();
    assert_eq!(error.code, "invalid");
    assert_eq!(error.message, "`ref` must look like plugin/table/id");
}
#[test]
fn backlinks_report_plugins_still_indexing() {
    let (_root, mut host) = setup();
    host.store_mut("todo")
        .unwrap()
        .write(|w| {
            for n in 0..3000 {
                w.insert(
                    "tasks",
                    json!({"title":format!("Task {n}")}).as_object().unwrap(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let found = backlinks(&mut host, None, "notes/notes/n1").unwrap();
    assert_eq!(found["indexing"], json!(["tasks"]));
    assert_eq!(found["mentions"], json!([]));
}

#[test]
fn backlinks_keep_working_when_one_plugin_cannot_catch_up() {
    let (root, mut host) = setup();
    let dir = indexed_notes(&root.path().join("broken-notes"));
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["queries"]["index_notes"]["sql"] = json!("SELECT id,title,body FROM notes WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1)) AND json_extract('invalid json','$') IS NULL");
    std::fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
    host.call(
        "notes",
        "create",
        &json!({"title":"Broken index", "body":"Note"}),
    )
    .unwrap();
    let task = create(&mut host, "[Note](terrarium:notes/notes/n1)");
    let found = backlinks(&mut host, None, "notes/notes/n1").unwrap();
    assert_eq!(found["indexing"], json!(["notes"]));
    assert_eq!(found["mentions"], json!([format!("todo/tasks/{task}")]));
}
