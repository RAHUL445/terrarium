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
    assert_eq!(
        ids,
        [json!("text"), json!("link"), json!("clock"), json!("view")]
    );
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

#[test]
fn folders_move_pages_promote_and_reject_cross_space_or_cycles() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    call(&mut host, "page_home", json!({"space":"personal"})).unwrap();
    let parent = call(
        &mut host,
        "folder_create",
        json!({"space":"personal", "title":"Parent"}),
    )
    .unwrap();
    let child = call(
        &mut host,
        "folder_create",
        json!({"space":"personal", "title":"Child", "parent_id":parent["id"]}),
    )
    .unwrap();
    let grandchild = call(
        &mut host,
        "folder_create",
        json!({"space":"personal", "title":"Grandchild", "parent_id":child["id"]}),
    )
    .unwrap();
    let page = call(
        &mut host,
        "page_create",
        json!({"space":"personal", "title":"Nested"}),
    )
    .unwrap();
    call(
        &mut host,
        "page_move",
        json!({"page":page["id"], "folder_id":child["id"]}),
    )
    .unwrap();
    assert!(call(
        &mut host,
        "folder_update",
        json!({"id":parent["id"], "parent_id":child["id"]})
    )
    .is_err());
    let other_space = call(
        &mut host,
        "folder_create",
        json!({"space":"work", "title":"Other"}),
    )
    .unwrap();
    assert!(call(
        &mut host,
        "page_move",
        json!({"page":page["id"], "folder_id":other_space["id"]})
    )
    .is_err());

    call(&mut host, "folder_remove", json!({"id":child["id"]})).unwrap();
    let grandchild_after = call(&mut host, "folders_list", json!({"space":"personal"}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["id"] == grandchild["id"])
        .unwrap()
        .clone();
    assert_eq!(grandchild_after["parent_id"], parent["id"]);
    let page_after =
        call(&mut host, "page_get", json!({"page":page["id"]})).unwrap()["page"].clone();
    assert_eq!(page_after["folder_id"], parent["id"]);
    assert!(call(&mut host, "page_trash", json!({"page":"home-personal"})).is_err());
    call(&mut host, "page_trash", json!({"page":page["id"]})).unwrap();
    assert_eq!(
        call(&mut host, "pages_list", json!({"space":"personal"}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    call(&mut host, "page_restore", json!({"page":page["id"]})).unwrap();
    assert_eq!(
        call(&mut host, "page_get", json!({"page":page["id"]})).unwrap()["page"]["archived"],
        false
    );
}

#[test]
fn subtree_trash_restore_keeps_ids_and_placements_and_layout_coordinates() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let parent = call(
        &mut host,
        "folder_create",
        json!({"space":"personal", "title":"Parent"}),
    )
    .unwrap();
    let child = call(
        &mut host,
        "folder_create",
        json!({"space":"personal", "title":"Child", "parent_id":parent["id"]}),
    )
    .unwrap();
    let page = call(
        &mut host,
        "page_create",
        json!({"space":"personal", "title":"Nested"}),
    )
    .unwrap();
    call(
        &mut host,
        "page_move",
        json!({"page":page["id"], "folder_id":child["id"]}),
    )
    .unwrap();
    let saved = call(
        &mut host,
        "page_save",
        json!({"page":page["id"], "placements":[{
            "plugin":"todo", "widget":"today", "x":3, "y":7, "w":5, "h":4,
            "flow_order":2, "config":{"keep":true}
        }]}),
    )
    .unwrap();
    let placement_id = saved["placements"][0]["id"].clone();
    call(
        &mut host,
        "page_layout",
        json!({"page":page["id"], "mode":"flow"}),
    )
    .unwrap();
    call(
        &mut host,
        "folder_remove",
        json!({"id":parent["id"], "subtree":true}),
    )
    .unwrap();
    assert!(call(&mut host, "pages_list", json!({"space":"personal"}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != page["id"]));
    assert_eq!(
        call(&mut host, "pages_trash", json!({"space":"personal"})).unwrap()[0]["id"],
        page["id"]
    );
    assert_eq!(
        call(&mut host, "page_get", json!({"page":page["id"]})).unwrap()["page"]["archived"],
        true
    );
    call(&mut host, "folder_restore", json!({"id":parent["id"]})).unwrap();
    let folders = call(&mut host, "folders_list", json!({"space":"personal"})).unwrap();
    assert!(folders
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == parent["id"]));
    assert!(folders
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == child["id"]));
    let restored = call(&mut host, "page_get", json!({"page":page["id"]})).unwrap();
    assert_eq!(restored["page"]["archived"], false);
    assert_eq!(restored["page"]["layout_mode"], "flow");
    assert_eq!(restored["page"]["folder_id"], child["id"]);
    assert_eq!(restored["placements"][0]["id"], placement_id);
    assert_eq!(restored["placements"][0]["flow_order"], 2);
    assert_eq!(restored["placements"][0]["x"], 3);
    call(
        &mut host,
        "page_layout",
        json!({"page":page["id"], "mode":"grid"}),
    )
    .unwrap();
    let grid = call(&mut host, "page_get", json!({"page":page["id"]})).unwrap();
    assert_eq!(grid["page"]["layout_mode"], "grid");
    assert_eq!(grid["placements"][0]["x"], 3);
    assert_eq!(grid["placements"][0]["y"], 7);
    assert_eq!(grid["placements"][0]["flow_order"], 2);
}

#[test]
fn synced_folder_cycle_projects_deterministically_without_rewriting_rows() {
    let root = TempDir::new().unwrap();
    let mut a = host(&root, "a");
    let mut b = host(&root, "b");
    let first = call(
        &mut a,
        "folder_create",
        json!({"space":"personal", "title":"First"}),
    )
    .unwrap();
    let second = call(
        &mut a,
        "folder_create",
        json!({"space":"personal", "title":"Second"}),
    )
    .unwrap();
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    a.store_mut(PAGES_STORE_ID)
        .unwrap()
        .write(|writer| {
            let mut values = serde_json::Map::new();
            values.insert("parent_id".into(), json!(second_id));
            writer.update("folders", first_id, &values)?;
            values.insert("parent_id".into(), json!(first_id));
            writer.update("folders", second_id, &values)
        })
        .unwrap();
    let (batch, _) = a
        .store_mut(PAGES_STORE_ID)
        .unwrap()
        .export_since(0)
        .unwrap();
    b.store_mut(PAGES_STORE_ID).unwrap().import(&batch).unwrap();

    let folders = call(&mut b, "folders_list", json!({"space":"personal"})).unwrap();
    let projected_root = folders
        .as_array()
        .unwrap()
        .iter()
        .filter(|folder| folder["parent_id"].is_null())
        .count();
    assert_eq!(projected_root, 1);
    assert_eq!(
        folders
            .as_array()
            .unwrap()
            .iter()
            .filter(|folder| folder["recovered"] == true)
            .count(),
        1
    );
    let stored = b
        .pages()
        .store()
        .query(
            "SELECT id, parent_id FROM folders WHERE id = ?1 OR id = ?2 ORDER BY id",
            &[json!(first_id), json!(second_id)],
        )
        .unwrap();
    assert!(
        stored.iter().all(|row| !row["parent_id"].is_null()),
        "projection must not repair synced rows"
    );
}

#[test]
fn folder_positions_and_active_folder_count_are_bounded_without_depth_limit() {
    let root = TempDir::new().unwrap();
    let mut app = host(&root, "a");
    let folder = call(
        &mut app,
        "folder_create",
        json!({"space":"personal", "title":"Deep root"}),
    )
    .unwrap();
    assert!(call(
        &mut app,
        "folder_update",
        json!({"id":folder["id"], "position":i64::MAX})
    )
    .is_err());
    app.store_mut(PAGES_STORE_ID)
        .unwrap()
        .write(|writer| {
            let mut values = serde_json::Map::new();
            values.insert("position".into(), json!(i64::MAX));
            writer.update("folders", folder["id"].as_str().unwrap(), &values)
        })
        .unwrap();
    assert!(call(
        &mut app,
        "folder_create",
        json!({"space":"personal", "title":"Overflow"})
    )
    .is_err());
    let page = call(
        &mut app,
        "page_create",
        json!({"space":"personal", "title":"Page"}),
    )
    .unwrap();
    assert!(call(
        &mut app,
        "page_move",
        json!({"page":page["id"], "position":i64::MAX})
    )
    .is_err());

    let mut limited = host(&root, "limited");
    limited
        .store_mut(PAGES_STORE_ID)
        .unwrap()
        .write(|writer| {
            for index in 0..200 {
                let mut values = serde_json::Map::new();
                values.insert("space".into(), json!("personal"));
                values.insert("title".into(), json!(format!("Folder {index}")));
                values.insert("position".into(), json!(index));
                writer.upsert("folders", &format!("f{index:03}"), &values)?;
            }
            Ok(())
        })
        .unwrap();
    assert!(call(
        &mut limited,
        "folder_create",
        json!({"space":"personal", "title":"Over limit"})
    )
    .is_err());
}

#[test]
fn folder_restore_is_scoped_to_its_space_even_if_batches_collide() {
    let root = TempDir::new().unwrap();
    let mut app = host(&root, "a");
    let personal = call(
        &mut app,
        "folder_create",
        json!({"space":"personal", "title":"Personal"}),
    )
    .unwrap();
    let work = call(
        &mut app,
        "folder_create",
        json!({"space":"work", "title":"Work"}),
    )
    .unwrap();
    call(
        &mut app,
        "folder_remove",
        json!({"id":personal["id"], "subtree":true}),
    )
    .unwrap();
    let batch = app
        .pages()
        .store()
        .query(
            "SELECT trash_batch FROM folders WHERE id = ?1",
            &[personal["id"].clone()],
        )
        .unwrap()[0]["trash_batch"]
        .as_str()
        .unwrap()
        .to_string();
    app.pages_mut()
        .store_mut()
        .write(|writer| {
            let mut values = serde_json::Map::new();
            values.insert("archived".into(), json!(true));
            values.insert("trash_batch".into(), json!(batch));
            writer.update("folders", work["id"].as_str().unwrap(), &values)
        })
        .unwrap();
    call(&mut app, "folder_restore", json!({"id":personal["id"]})).unwrap();
    let folders = call(
        &mut app,
        "folders_list",
        json!({"space":"work", "include_archived":true}),
    )
    .unwrap();
    let work_folder = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["id"] == work["id"])
        .unwrap();
    assert_eq!(work_folder["archived"], true);
}
