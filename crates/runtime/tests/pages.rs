//! Pages through the app's commands: Home seeding, page lifecycle, layout
//! saves, and the `_pages` store taking part in sync.

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{services, HostConfig, PluginHost, PAGES_STORE_ID};

fn host(root: &TempDir, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: root.path().join(device).join("personal"),
        spaces_dir: Some(root.path().join(device).join("spaces")),
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap()
}

fn call(host: &mut PluginHost, cmd: &str, args: Value) -> Result<Value, String> {
    services::invoke(host, cmd, &args, true).expect("known command")
}

#[test]
fn page_commands_round_trip() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let seed = json!([{ "plugin": "todo", "widget": "today", "x": 0, "y": 0, "w": 6, "h": 3 }]);
    let home = call(
        &mut host,
        "page_home",
        json!({ "space": "personal", "seed": seed }),
    )
    .unwrap();
    assert_eq!(home["page"]["id"], "home-personal");
    assert_eq!(home["placements"].as_array().unwrap().len(), 1);

    let page = call(
        &mut host,
        "page_create",
        json!({ "space": "personal", "title": "Focus", "focus": " Launch " }),
    )
    .unwrap();
    assert_eq!(page["focus"], "Launch");
    let id = page["id"].as_str().unwrap().to_string();
    let saved = call(
        &mut host,
        "page_save",
        json!({ "page": id, "placements": [
            { "plugin": "todo", "widget": "today", "x": 0, "y": 0, "w": 4, "h": 3, "config": { "filter": "upcoming" } },
            { "plugin": "_host", "widget": "text", "x": 4, "y": 0, "w": 4, "h": 2 }
        ]}),
    )
    .unwrap();
    assert_eq!(saved["placements"].as_array().unwrap().len(), 2);

    call(
        &mut host,
        "page_update",
        json!({ "page": id, "title": "Deep work", "icon": "star" }),
    )
    .unwrap();
    let list = call(&mut host, "pages_list", json!({ "space": "personal" })).unwrap();
    let titles: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].clone())
        .collect();
    assert_eq!(titles, [json!("Home"), json!("Deep work")]);
    assert_eq!(list[1]["focus"], "Launch");
    let cleared = call(&mut host, "page_update", json!({ "page": id, "focus": "" })).unwrap();
    assert!(cleared["focus"].is_null());
    assert!(call(&mut host, "page_update", json!({ "page": id, "focus": 7 })).is_err());

    assert!(call(&mut host, "page_remove", json!({ "page": "home-personal" })).is_err());
    call(&mut host, "page_remove", json!({ "page": id })).unwrap();
    assert!(call(&mut host, "page_get", json!({ "page": id })).is_err());
    assert!(call(&mut host, "page_save", json!({ "page": "home-personal" })).is_err());
}

#[test]
fn pages_sync_with_other_devices() {
    let root = TempDir::new().unwrap();
    let mut a = host(&root, "a");
    let mut b = host(&root, "b");
    assert!(a.plugin_ids().contains(&PAGES_STORE_ID.to_string()));
    call(
        &mut a,
        "page_create",
        json!({ "space": "personal", "title": "Shared" }),
    )
    .unwrap();
    let (batch, _) = a
        .store_mut(PAGES_STORE_ID)
        .unwrap()
        .export_since(0)
        .unwrap();
    b.store_mut(PAGES_STORE_ID).unwrap().import(&batch).unwrap();
    let list = call(&mut b, "pages_list", json!({ "space": "personal" })).unwrap();
    assert_eq!(list[0]["title"], "Shared");
}

#[test]
fn built_in_widgets_are_listed_and_checked() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let widgets = call(&mut host, "page_widgets", json!({})).unwrap();
    let ids: Vec<_> = widgets
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].clone())
        .collect();
    assert_eq!(ids, [json!("text"), json!("link"), json!("clock")]);
    let page = call(
        &mut host,
        "page_create",
        json!({ "space": "personal", "title": "Links" }),
    )
    .unwrap();
    let saved = call(
        &mut host,
        "page_save",
        json!({ "page": page["id"], "placements": [
            { "plugin": "_host", "widget": "link", "x": 0, "y": 0, "w": 3, "h": 1,
              "config": { "label": "Docs", "url": "javascript:alert(1)", "extra": true } },
            { "plugin": "todo", "widget": "today", "x": 3, "y": 0, "w": 4, "h": 3,
              "config": { "filter": "open" } }
        ]}),
    )
    .unwrap();
    let placements = saved["placements"].as_array().unwrap();
    assert_eq!(placements[0]["config"], json!({ "label": "Docs" }));
    assert_eq!(
        placements[1]["config"],
        json!({ "filter": "open" }),
        "an absent plugin keeps its settings"
    );
    let unknown = call(
        &mut host,
        "page_save",
        json!({ "page": page["id"], "placements": [
            { "plugin": "_host", "widget": "iframe", "x": 0, "y": 0, "w": 3, "h": 1 }
        ]}),
    );
    assert!(unknown.is_err());
}
