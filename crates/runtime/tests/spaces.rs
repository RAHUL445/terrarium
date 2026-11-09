//! Spaces: separate data per space with one plugin list, interop kept
//! inside a space, and the space list syncing between devices.

mod support;

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Map};
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{build_plugin, HostConfig, PluginHost, DEFAULT_SPACE};

fn package(name: &str) -> PathBuf {
    static BUILT: OnceLock<()> = OnceLock::new();
    let dir = |n: &str| support::package_root().join(n).canonicalize().unwrap();
    BUILT.get_or_init(|| {
        for n in ["todo", "notes"] {
            build_plugin(&dir(n)).unwrap();
        }
    });
    dir(name)
}

fn host(root: &TempDir, device: &str) -> PluginHost {
    let mut host = PluginHost::new(HostConfig {
        data_dir: root.path().join(device).join("personal"),
        spaces_dir: Some(root.path().join(device).join("spaces")),
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    for name in ["todo", "notes"] {
        host.load(&package(name)).unwrap();
    }
    host
}

fn titles(host: &PluginHost, space: &str) -> Vec<String> {
    host.store_in(space, "todo")
        .unwrap()
        .query(
            "SELECT title FROM tasks WHERE _deleted IS NULL ORDER BY title",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn each_space_has_its_own_data_and_interop_stays_inside_it() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "laptop");
    let work = host.spaces_mut().create("Work").unwrap();
    assert!(host.sync_spaces().unwrap());
    assert_eq!(host.spaces().list().unwrap()[0].id, DEFAULT_SPACE);

    host.call("todo", "create", &json!({"title": "Buy milk"}))
        .unwrap();
    host.call_in(&work.id, "todo", "create", &json!({"title": "Ship report"}))
        .unwrap();
    // Notes in Work sends its task to Work's To-do.
    host.call_in(
        &work.id,
        "notes",
        "to_task",
        &json!({"text": "- [ ] Call client"}),
    )
    .unwrap();
    assert_eq!(titles(&host, DEFAULT_SPACE), vec!["Buy milk"]);
    assert_eq!(titles(&host, &work.id), vec!["Call client", "Ship report"]);

    // Declared queries are per space too.
    let count = |host: &PluginHost, space: &str| {
        let mut values = Map::new();
        values.insert("today".into(), json!("2026-11-02"));
        values.insert("filter".into(), json!("all"));
        values.insert("q".into(), json!(""));
        host.query_in(space, "todo", "tasks", &values)
            .unwrap()
            .len()
    };
    assert_eq!(count(&host, DEFAULT_SPACE), 1);
    assert_eq!(count(&host, &work.id), 2);

    // A plugin loaded later gets a store in every open space.
    host.load(&package("todo")).unwrap();
    assert_eq!(titles(&host, &work.id), vec!["Call client", "Ship report"]);

    // Removing closes the space (data kept), adding back reopens it.
    host.spaces_mut().set_removed(&work.id, true).unwrap();
    assert!(host.sync_spaces().unwrap());
    assert!(host
        .call_in(&work.id, "todo", "create", &json!({"title": "x"}))
        .is_err());
    host.spaces_mut().set_removed(&work.id, false).unwrap();
    host.sync_spaces().unwrap();
    assert_eq!(titles(&host, &work.id), vec!["Call client", "Ship report"]);

    assert!(host.spaces_mut().set_removed(DEFAULT_SPACE, true).is_err());
    host.spaces_mut().rename(DEFAULT_SPACE, "Home").unwrap();
    assert_eq!(host.spaces().list().unwrap()[0].name, "Home");
    assert!(host.spaces_mut().create("  ").is_err());
}

#[test]
fn spaces_and_their_data_sync_to_another_device() {
    let root = TempDir::new().unwrap();
    let mut laptop = host(&root, "laptop");
    let mut phone = host(&root, "phone");
    let work = laptop.spaces_mut().create("Work").unwrap();
    laptop.sync_spaces().unwrap();
    laptop
        .call_in(
            &work.id,
            "todo",
            "create",
            &json!({"title": "Standup notes"}),
        )
        .unwrap();

    // What the sync engine does: export every store, import by id.
    let ids = laptop.plugin_ids();
    assert!(ids.contains(&format!("todo@{}", work.id)), "{ids:?}");
    let mut batches = Vec::new();
    for id in &ids {
        batches.push(laptop.store_mut(id).unwrap().export_since(0).unwrap().0);
    }
    // The space list first, then the phone opens the space and takes its data.
    for batch in batches.iter().filter(|b| b.plugin.starts_with('_')) {
        phone
            .store_mut(&batch.plugin)
            .unwrap()
            .import(batch)
            .unwrap();
    }
    assert!(phone.sync_spaces().unwrap());
    for batch in batches.iter().filter(|b| !b.plugin.starts_with('_')) {
        phone
            .store_mut(&batch.plugin)
            .unwrap()
            .import(batch)
            .unwrap();
    }
    let names: Vec<String> = phone
        .spaces()
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["Personal".to_string(), "Work".to_string()]);
    assert_eq!(titles(&phone, &work.id), vec!["Standup notes"]);
    assert!(titles(&phone, DEFAULT_SPACE).is_empty());
}
