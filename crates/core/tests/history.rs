//! Simulated devices editing offline, then syncing; every test checks that
//! all devices end with identical data.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_core::{
    Error, HistoryOptions, ManualTime, Op, PluginSchema, RowState, Store, StoreConfig,
};

fn schema(version: u32) -> PluginSchema {
    let mut tasks = vec![
        json!({"name": "title", "type": "text"}),
        json!({"name": "done", "type": "boolean", "default": false}),
        json!({"name": "points", "type": "integer", "rule": "counter"}),
    ];
    if version >= 2 {
        tasks.push(json!({"name": "due", "type": "text"}));
    }
    serde_json::from_value(json!({"version": version, "tables": [
        {"name": "tasks", "columns": tasks},
        {"name": "tags", "delete_policy": "delete-wins", "columns": [{"name": "name", "type": "text"}]},
        {"name": "prefs", "delete_policy": "edit-wins", "columns": [{"name": "value", "type": "text"}]},
        {"name": "log", "kind": "append", "columns": [{"name": "msg", "type": "text"}]},
        {"name": "notes", "columns": [{"name": "body", "type": "text", "rule": "text"}]}
    ]}))
    .unwrap()
}

struct Device {
    name: &'static str,
    store: Store,
    time: ManualTime,
    /// Last local log position already sent to each peer.
    sent: HashMap<&'static str, i64>,
}

impl Device {
    fn new(dir: &TempDir, name: &'static str, version: u32) -> Self {
        let time = ManualTime::new(1_000);
        let path = dir.path().join(format!("{name}.db"));
        let store = open(&path, name, version, &time).unwrap();
        Device {
            name,
            store,
            time,
            sent: HashMap::new(),
        }
    }

    fn insert(&mut self, table: &str, values: Value) -> String {
        self.store.write(|w| w.insert(table, &obj(values))).unwrap()
    }

    fn update(&mut self, table: &str, row: &str, values: Value) {
        self.store
            .write(|w| w.update(table, row, &obj(values)))
            .unwrap()
    }

    fn cell(&self, table: &str, row: &str, column: &str) -> Value {
        self.store
            .query(
                &format!("SELECT \"{column}\" AS v FROM \"{table}\" WHERE id = ?1"),
                &[json!(row)],
            )
            .unwrap()
            .pop()
            .map(|mut r| r.remove("v").unwrap())
            .unwrap_or(Value::Null)
    }
}

fn open(
    path: impl AsRef<std::path::Path>,
    name: &str,
    version: u32,
    time: &ManualTime,
) -> terrarium_core::Result<Store> {
    let cfg = StoreConfig {
        space: "personal".into(),
        plugin: "test".into(),
        device: name.into(),
        user: "owner".into(),
    };
    Store::open(path, cfg, schema(version), Box::new(time.clone()))
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// Sends `from`'s new changes to `to`.
fn send(from: &mut Device, to: &mut Device) {
    let after = *from.sent.get(to.name).unwrap_or(&0);
    let (batch, cursor) = from.store.export_since(after).unwrap();
    to.store.import(&batch).unwrap();
    from.sent.insert(to.name, cursor);
}

fn sync(a: &mut Device, b: &mut Device) {
    send(a, b);
    send(b, a);
}

fn assert_converged(devices: &[&Device]) {
    let first = devices[0].store.dump().unwrap();
    for d in &devices[1..] {
        assert_eq!(
            d.store.dump().unwrap(),
            first,
            "{} differs from {}",
            d.name,
            devices[0].name
        );
    }
}

fn versions(d: &Device, table: &str, row: &str) -> Vec<terrarium_core::Version> {
    d.store
        .history(table, row, &HistoryOptions::default())
        .unwrap()
}

#[test]
fn versions_group_by_device_and_time() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title":"Draft"}));
    a.time.set(2_000);
    a.update("tasks", &row, json!({"title":"Second"}));
    a.time.set(200_000);
    a.update("tasks", &row, json!({"done":true}));
    let v = versions(&a, "tasks", &row);
    assert_eq!(v.len(), 2);
    assert!(v[1].created);
    assert_eq!(v[0].columns, ["done"]);
    assert_eq!(v[1].columns, ["title"]);
    let old = a.store.row_at("tasks", &row, &v[1].id).unwrap().unwrap();
    assert_eq!(
        old.values,
        obj(json!({"title":"Second", "done":false, "points":null}))
    );
    let opts = HistoryOptions {
        limit: 0,
        ..Default::default()
    };
    assert_eq!(a.store.history("tasks", &row, &opts).unwrap(), v[..1]);
    let opts = HistoryOptions {
        since_ms: Some(v[0].at_ms),
        ..Default::default()
    };
    assert_eq!(a.store.history("tasks", &row, &opts).unwrap(), v[..1]);
    assert!(a
        .store
        .history("tasks", "missing", &opts)
        .unwrap()
        .is_empty());
    assert!(matches!(
        a.store.history("missing", &row, &opts),
        Err(Error::UnknownTable(_))
    ));
}

#[test]
fn row_at_rebuilds_lww_counter_and_text() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let note = a.insert("notes", json!({"body":"Buy milk"}));
    a.time.advance(200_000);
    a.update("notes", &note, json!({"body":"Buy oat milk"}));
    a.time.advance(200_000);
    a.update("notes", &note, json!({"body":"Buy oat milk and eggs"}));
    for (v, expected) in versions(&a, "notes", &note).iter().rev().zip([
        "Buy milk",
        "Buy oat milk",
        "Buy oat milk and eggs",
    ]) {
        let at = a.store.row_at("notes", &note, &v.id).unwrap().unwrap();
        assert_eq!(at.values, obj(json!({"body":expected})));
        assert_eq!(at.version, v.id);
    }
    let task = a.insert("tasks", json!({"points":2}));
    a.time.advance(200_000);
    a.store
        .write(|w| w.increment("tasks", &task, "points", json!(3)))
        .unwrap();
    let v = versions(&a, "tasks", &task);
    assert_eq!(
        a.store
            .row_at("tasks", &task, &v[1].id)
            .unwrap()
            .unwrap()
            .values["points"],
        2
    );
    assert_eq!(
        a.store
            .row_at("tasks", &task, &v[0].id)
            .unwrap()
            .unwrap()
            .values["points"],
        5
    );
    assert!(a
        .store
        .row_at("tasks", &task, "0000000000000000-00000000-a")
        .unwrap()
        .is_none());
}

#[test]
fn history_is_the_same_on_every_device() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("notes", json!({"body":"Buy milk"}));
    sync(&mut a, &mut b);
    a.time.set(200_000);
    b.time.set(300_000);
    a.update("notes", &row, json!({"body":"Buy oat milk"}));
    b.update("notes", &row, json!({"body":"Buy milk and eggs"}));
    sync(&mut a, &mut b);
    assert_eq!(versions(&a, "notes", &row), versions(&b, "notes", &row));
    for v in versions(&a, "notes", &row) {
        assert_eq!(
            a.store.row_at("notes", &row, &v.id).unwrap(),
            b.store.row_at("notes", &row, &v.id).unwrap()
        );
    }
    assert_converged(&[&a, &b]);
}

#[test]
fn restore_to_writes_normal_changes_that_sync() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("notes", json!({"body":"Buy milk"}));
    let oldest = versions(&a, "notes", &row)[0].id.clone();
    a.time.advance(200_000);
    a.update("notes", &row, json!({"body":"Buy oat milk"}));
    sync(&mut a, &mut b);
    a.time.advance(200_000);
    assert_eq!(
        a.store.restore_to("notes", &row, &oldest).unwrap(),
        ["body"]
    );
    assert_eq!(a.cell("notes", &row, "body"), "Buy milk");
    assert_eq!(versions(&a, "notes", &row)[0].columns, ["body"]);
    sync(&mut a, &mut b);
    assert_converged(&[&a, &b]);
    assert_eq!(b.cell("notes", &row, "body"), "Buy milk");
    let task = a.insert("tasks", json!({"title":"x","done":true,"points":2}));
    let initial = versions(&a, "tasks", &task)[0].id.clone();
    a.time.advance(200_000);
    a.store
        .write(|w| w.increment("tasks", &task, "points", json!(3)))
        .unwrap();
    assert_eq!(
        a.store.restore_to("tasks", &task, &initial).unwrap(),
        ["points"]
    );
    assert!(
        matches!(&a.store.changes(1).unwrap()[0].change.op, Op::Increment { delta, .. } if *delta == json!(-3))
    );
    let before = a.store.export_since(0).unwrap();
    assert!(a
        .store
        .restore_to("tasks", &task, &initial)
        .unwrap()
        .is_empty());
    assert_eq!(before, a.store.export_since(0).unwrap());
}

#[test]
fn restore_brings_a_trashed_row_back() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title":"Draft"}));
    let initial = versions(&a, "tasks", &row)[0].id.clone();
    a.time.advance(200_000);
    a.update("tasks", &row, json!({"title":"Second"}));
    a.store.write(|w| w.delete("tasks", &row)).unwrap();
    let trash = versions(&a, "tasks", &row)[0].id.clone();
    assert!(
        a.store
            .row_at("tasks", &row, &trash)
            .unwrap()
            .unwrap()
            .in_trash
    );
    assert_eq!(
        a.store.restore_to("tasks", &row, &initial).unwrap(),
        ["title"]
    );
    assert_eq!(
        a.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Live)
    );
    assert_eq!(a.cell("tasks", &row, "title"), "Draft");
    // A live row stays live even when copying a trashed historical target.
    a.store.restore_to("tasks", &row, &trash).unwrap();
    assert_eq!(
        a.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Live)
    );
    assert_eq!(a.cell("tasks", &row, "title"), "Second");
    a.store.write(|w| w.delete("tasks", &row)).unwrap();
    // A currently trashed row is revived for a trashed target, even with no value changes.
    assert!(a
        .store
        .restore_to("tasks", &row, &trash)
        .unwrap()
        .is_empty());
    assert_eq!(
        a.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Live)
    );
    let v = &versions(&a, "tasks", &row)[0];
    assert!(v.restored);
    assert!(!v.in_trash);
}

#[test]
fn restore_refuses_gone_and_append_rows() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tags", json!({"name":"home"}));
    let id = versions(&a, "tags", &row)[0].id.clone();
    a.store.write(|w| w.delete("tags", &row)).unwrap();
    let before = a.store.export_since(0).unwrap();
    assert!(matches!(
        a.store.restore_to("tags", &row, &id),
        Err(Error::NotFound { .. })
    ));
    assert_eq!(before, a.store.export_since(0).unwrap());
    assert!(a.store.row_at("tags", &row, &id).unwrap().is_some());
    let log = a.insert("log", json!({"msg":"entry"}));
    assert!(matches!(
        a.store.restore_to("log", &log, &id),
        Err(Error::AppendOnly(_))
    ));
    assert!(matches!(
        a.store.restore_to("tasks", "missing", &id),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn older_peer_accepts_a_restore() {
    let dir = TempDir::new().unwrap();
    let (mut old, mut new) = (Device::new(&dir, "old", 1), Device::new(&dir, "new", 1));
    let row = new.insert("notes", json!({"body":"original"}));
    let id = versions(&new, "notes", &row)[0].id.clone();
    sync(&mut new, &mut old);
    new.time.advance(200_000);
    new.update("notes", &row, json!({"body":"changed"}));
    new.store.restore_to("notes", &row, &id).unwrap();
    sync(&mut new, &mut old);
    assert_converged(&[&old, &new]);
    assert_eq!(old.cell("notes", &row, "body"), "original");
    let (batch, _) = new.store.export_since(0).unwrap();
    assert!(batch.changes.iter().skip(2).all(|c| matches!(
        c.op,
        Op::Set { .. } | Op::TextEdit { .. } | Op::Increment { .. } | Op::Restore
    )));
}

#[test]
fn bad_version_text_is_an_error() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title":"x"}));
    assert!(matches!(
        a.store.row_at("tasks", &row, "nonsense"),
        Err(Error::Hlc(_))
    ));
    assert!(matches!(
        a.store.restore_to("tasks", &row, "nonsense"),
        Err(Error::Hlc(_))
    ));
}

#[test]
fn cutoff_survives_late_edits_and_changed_groups() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title":"initial"}));
    sync(&mut a, &mut b);
    a.time.set(10_000);
    a.update("tasks", &row, json!({"title":"last"}));
    let id = versions(&a, "tasks", &row)[0].id.clone();
    b.time.set(5_000);
    b.update("tasks", &row, json!({"done":true}));
    a.time.set(20_000);
    a.update("tasks", &row, json!({"title":"later"}));
    sync(&mut a, &mut b);
    assert!(versions(&a, "tasks", &row).iter().all(|v| v.id != id));
    let at = a.store.row_at("tasks", &row, &id).unwrap().unwrap();
    assert_eq!(at.values["title"], "last");
    assert_eq!(at.values["done"], true);
    assert_eq!(Some(at), b.store.row_at("tasks", &row, &id).unwrap());
}

#[test]
fn counter_overflow_never_partially_restores() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title":"before","points":i64::MIN}));
    let id = versions(&a, "tasks", &row)[0].id.clone();
    a.store
        .write(|w| w.increment("tasks", &row, "points", json!(i64::MAX)))
        .unwrap();
    a.store
        .write(|w| w.increment("tasks", &row, "points", json!(2)))
        .unwrap();
    a.update("tasks", &row, json!({"title":"after"}));
    a.store.write(|w| w.delete("tasks", &row)).unwrap();
    let before = a.store.dump().unwrap();
    let log = a.store.export_since(0).unwrap();
    assert!(matches!(
        a.store.restore_to("tasks", &row, &id),
        Err(Error::Corrupt(_))
    ));
    assert_eq!(a.store.dump().unwrap(), before);
    assert_eq!(a.store.export_since(0).unwrap(), log);
    assert_eq!(
        a.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Trashed)
    );
    // Replay also rejects integer overflow rather than wrapping or rounding.
    let row = a.insert("tasks", json!({"points":i64::MAX}));
    a.store
        .write(|w| w.increment("tasks", &row, "points", json!(1)))
        .unwrap();
    let id = a.store.changes(1).unwrap()[0].change.hlc.to_string();
    assert!(matches!(
        a.store.row_at("tasks", &row, &id),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn grouping_uses_sliding_gap_and_user_and_clamps_limit() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title":"start"}));
    // Exact threshold stays in the group, including sessions longer than group_ms.
    for _ in 0..3 {
        a.time.advance(120_000);
        a.update("tasks", &row, json!({"title":"same session"}));
    }
    assert_eq!(versions(&a, "tasks", &row).len(), 1);
    let mut other_user = Store::open(
        dir.path().join("a.db"),
        StoreConfig {
            space: "personal".into(),
            plugin: "test".into(),
            device: "a".into(),
            user: "another".into(),
        },
        schema(1),
        Box::new(a.time.clone()),
    )
    .unwrap();
    other_user
        .write(|w| w.update("tasks", &row, &obj(json!({"title":"another user"}))))
        .unwrap();
    let v = versions(&a, "tasks", &row);
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].device, v[1].device);
    assert_eq!(v[0].user, "another");
    drop(other_user);
    // Reopen to observe the other writer's HLC before continuing as owner.
    a.store = open(dir.path().join("a.db"), "a", 1, &a.time).unwrap();
    for i in 0..205 {
        a.time.advance(120_001);
        a.update("tasks", &row, json!({"title":format!("edit {i}")}));
    }
    let opts = HistoryOptions {
        limit: usize::MAX,
        ..Default::default()
    };
    assert_eq!(a.store.history("tasks", &row, &opts).unwrap().len(), 200);
    let opts = HistoryOptions {
        since_ms: Some(u64::MAX),
        ..opts
    };
    assert!(a.store.history("tasks", &row, &opts).unwrap().is_empty());
}

#[test]
fn null_text_target_is_an_empty_text_edit() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("notes", json!({}));
    let initial = versions(&a, "notes", &row)[0].id.clone();
    assert_eq!(
        a.store
            .row_at("notes", &row, &initial)
            .unwrap()
            .unwrap()
            .values["body"],
        Value::Null
    );
    a.update("notes", &row, json!({"body":"now has text"}));
    assert_eq!(
        a.store.restore_to("notes", &row, &initial).unwrap(),
        ["body"]
    );
    assert_eq!(a.cell("notes", &row, "body"), "");
    assert!(matches!(
        a.store.changes(1).unwrap()[0].change.op,
        Op::TextEdit { .. }
    ));
    assert!(a
        .store
        .restore_to("notes", &row, &initial)
        .unwrap()
        .is_empty());
}
