//! Ref resolution through the public command registry over synthetic rows.
mod support;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;
use terrarium_runtime::commands::{self, Caller, CommandError, Kind, Scope};
use terrarium_runtime::{build_plugin, HostConfig, PluginHost};
const READ: Caller = Caller::Client(Scope::Read);
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

fn described_todo(root: &Path) -> PathBuf {
    let from = todo_dir();
    let to = root.join("todo");
    for entry in walk(&from) {
        let dest = to.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["refs"] = json!([{"table":"tasks","resolve":"ref_tasks","kind":"Task","icon":"check"}]);
    m["queries"]["ref_tasks"] = json!({"sql":"SELECT id, title, CASE WHEN done = 1 THEN 'Done' ELSE due END AS subtitle, done FROM tasks WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1))", "params":["ids"], "cli":false});
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    to
}
fn host(root: &TempDir) -> PluginHost {
    let mut host = PluginHost::new(HostConfig {
        data_dir: root.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    host.load(&described_todo(root.path())).unwrap();
    host
}
fn create(host: &mut PluginHost, title: &str) -> String {
    let id = host
        .call("todo", "create", &json!({"title":title}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    format!("todo/tasks/{id}")
}
fn resolve(host: &mut PluginHost, refs: Value) -> Result<Value, CommandError> {
    commands::invoke(
        host,
        READ,
        Kind::Query,
        "ref.resolve",
        None,
        &json!({"refs":refs}),
        true,
    )
}
fn rewrite(host: &mut PluginHost, edit: impl FnOnce(&mut Value)) {
    let dir = host.package_dir("todo").unwrap().to_path_buf();
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut m);
    std::fs::write(path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    host.load(&dir).unwrap();
}
#[test]
fn resolve_describes_live_trashed_missing_and_unknown() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root);
    let live = create(&mut host, "Buy lamp");
    let trashed = create(&mut host, "Old chair");
    let id = trashed.rsplit('/').next().unwrap();
    host.call("todo", "delete", &json!({"id":id})).unwrap();
    let found = resolve(
        &mut host,
        json!([live, trashed, "todo/tasks/nope", "notes/notes/x", "garbage"]),
    )
    .unwrap();
    let rows = found.as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|v| v["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["ok", "trash", "missing", "unknown", "unknown"]
    );
    assert_eq!(rows[0]["title"], "Buy lamp");
    assert_eq!(rows[0]["kind"], "Task");
    assert_eq!(rows[0]["icon"], "check");
    assert_eq!(rows[0]["done"], false);
    assert_eq!(rows[0]["open"]["page"], "main");
    for row in &rows[1..3] {
        assert!(row["title"].is_null());
        // Refs may name the former id; results name the plugin's current id.
        assert_eq!(row["plugin"], "tasks");
        assert!(row.get("open").is_none());
    }
    for row in &rows[3..] {
        assert_eq!(row.as_object().unwrap().len(), 2);
    }
    assert_eq!(
        resolve(&mut host, json!(["todo/steps/x"])).unwrap()[0],
        json!({"ref":"todo/steps/x","state":"unknown"})
    );
}
#[test]
fn resolve_is_batched_and_deduplicated() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root);
    let a = create(&mut host, "Lamp");
    let b = create(&mut host, "Chair");
    rewrite(&mut host, |m| {
        m["queries"]["ref_tasks"]["sql"]=json!("SELECT id,title,(SELECT count(*) FROM json_each(?1)) AS subtitle,done FROM tasks WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1))")
    });
    let found = resolve(&mut host, json!([b, a, b])).unwrap();
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert_eq!(found[0]["ref"], b);
    assert_eq!(found[1]["ref"], a);
    assert_eq!(found[0]["subtitle"], 2);
    assert_eq!(found[1]["subtitle"], 2);
    let error = resolve(&mut host, json!(vec![a; 201])).unwrap_err();
    assert_eq!(error.code, "invalid");
    assert_eq!(error.message, "At most 200 refs at a time");
    for invalid in [json!({}), json!([1]), json!("todo/tasks/x")] {
        let error = resolve(&mut host, invalid).unwrap_err();
        assert_eq!(error.code, "invalid");
        assert_eq!(error.message, "`refs` must be a list of refs");
    }
    assert_eq!(resolve(&mut host, json!([])).unwrap(), json!([]));
}
#[test]
fn read_only_clients_may_resolve() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root);
    let item = create(&mut host, "Lamp");
    assert_eq!(resolve(&mut host, json!([item])).unwrap()[0]["state"], "ok");
    let info = commands::describe(&host, READ)
        .into_iter()
        .find(|c| c.id == "ref.resolve")
        .unwrap();
    assert_eq!(info.effect, terrarium_runtime::Effect::Read);
    assert!(info.space);
}
#[test]
fn query_errors_are_missing_without_hiding_other_groups() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root);
    let item = create(&mut host, "Lamp");
    let trashed = create(&mut host, "Chair");
    host.call(
        "todo",
        "delete",
        &json!({"id":trashed.rsplit('/').next().unwrap()}),
    )
    .unwrap();
    rewrite(&mut host, |m| {
        m["queries"]["ref_tasks"]["sql"]=json!("SELECT id,title FROM tasks WHERE id IN (SELECT value FROM json_each(?1)) AND json_extract('invalid json','$') IS NULL");
        m["refs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"table":"steps","resolve":"ref_steps","kind":"Step"}));
        m["queries"]["ref_steps"] = json!({"sql":"SELECT value AS id,'Working step' AS title FROM json_each(?1)","params":["ids"],"cli":false});
    });
    let found = resolve(&mut host, json!([item, "todo/steps/working", trashed])).unwrap();
    assert_eq!(found[0]["state"], "missing");
    assert_eq!(found[1]["state"], "ok");
    assert_eq!(found[2]["state"], "missing");
    assert!(found[1]["icon"].is_null());
}
#[test]
fn resolve_coerces_titles_and_done_and_ignores_unrequested_rows() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root);
    rewrite(&mut host, |m| {
        m["queries"]["ref_tasks"]["sql"]=json!("SELECT value AS id, CASE value WHEN 'number' THEN 42 WHEN 'blank' THEN '' ELSE NULL END AS title, CASE value WHEN 'number' THEN 1.0 ELSE 0 END AS done FROM json_each(?1) UNION ALL SELECT 'injected','Not requested',1")
    });
    let found = resolve(
        &mut host,
        json!(["todo/tasks/number", "todo/tasks/blank", "todo/tasks/null"]),
    )
    .unwrap();
    assert_eq!(found.as_array().unwrap().len(), 3);
    assert_eq!(found[0]["title"], "42");
    assert_eq!(found[0]["done"], true);
    assert_eq!(found[1]["title"], "Untitled");
    assert_eq!(found[2]["title"], "Untitled");
    assert_eq!(found[1]["done"], false);
}
