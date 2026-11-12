use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use tempfile::TempDir;
use terrarium_core::{Error, ManualTime, PluginSchema, Store, StoreConfig};

fn schema(version: u32) -> PluginSchema {
    serde_json::from_value(json!({"version": version, "tables": [
        {"name": "tasks", "columns": [{"name": "title", "type": "text"}]}
    ]}))
    .unwrap()
}
fn open(path: impl AsRef<std::path::Path>, name: &str, time: &ManualTime) -> Store {
    Store::open(
        path,
        StoreConfig {
            space: "personal".into(),
            plugin: "test".into(),
            device: name.into(),
            user: "owner".into(),
        },
        schema(1),
        Box::new(time.clone()),
    )
    .unwrap()
}
fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}
fn fresh(dir: &TempDir, name: &str) -> Store {
    open(
        dir.path().join(format!("{name}.db")),
        name,
        &ManualTime::new(1_000),
    )
}

#[test]
fn an_empty_log_is_at_zero() {
    let dir = TempDir::new().unwrap();
    let s = fresh(&dir, "a");
    assert_eq!(s.last_lseq().unwrap(), 0);
    assert!(s.changed_rows(0, 10).unwrap().is_empty());
}
#[test]
fn changed_rows_page_through_the_log() {
    let dir = TempDir::new().unwrap();
    let mut s = fresh(&dir, "a");
    let ids: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|title| {
            s.write(|w| w.insert("tasks", &obj(json!({"title": title}))))
                .unwrap()
        })
        .collect();
    assert_eq!(s.last_lseq().unwrap(), 6);
    s.write(|w| w.update("tasks", &ids[1], &obj(json!({"title": "b edited"}))))
        .unwrap();
    assert_eq!(s.last_lseq().unwrap(), 7);
    assert_eq!(
        s.changed_rows(0, 2).unwrap(),
        vec![
            (1, "tasks".into(), ids[0].clone()),
            (2, "tasks".into(), ids[0].clone())
        ]
    );
    assert_eq!(
        s.changed_rows(6, 10).unwrap(),
        vec![(7, "tasks".into(), ids[1].clone())]
    );
    assert_eq!(
        s.changed_rows(0, 100)
            .unwrap()
            .into_iter()
            .map(|(_, _, id)| id)
            .collect::<BTreeSet<_>>(),
        ids.into_iter().collect()
    );
    assert!(s.changed_rows(0, 0).unwrap().is_empty());
}
#[test]
fn trash_and_restore_are_changes() {
    let dir = TempDir::new().unwrap();
    let mut s = fresh(&dir, "a");
    let id = s
        .write(|w| w.insert("tasks", &obj(json!({"title": "a"}))))
        .unwrap();
    s.write(|w| w.delete("tasks", &id)).unwrap();
    s.write(|w| w.restore("tasks", &id)).unwrap();
    assert_eq!(
        s.changed_rows(2, 10).unwrap(),
        vec![(3, "tasks".into(), id.clone()), (4, "tasks".into(), id)]
    );
}
#[test]
fn imported_changes_move_the_log() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (fresh(&dir, "a"), fresh(&dir, "b"));
    // An empty row emits just Create; title-bearing rows emit Create + Set.
    let id = a.write(|w| w.insert("tasks", &Map::new())).unwrap();
    let before = b.last_lseq().unwrap();
    b.import(&a.export_since(0).unwrap().0).unwrap();
    assert_eq!(b.last_lseq().unwrap(), before + 1);
    assert_eq!(
        b.changed_rows(before, 10).unwrap(),
        vec![(before + 1, "tasks".into(), id)]
    );
}
#[test]
fn row_ids_after_pages_in_id_order() {
    let dir = TempDir::new().unwrap();
    let mut s = fresh(&dir, "a");
    let mut ids: Vec<_> = (0..5)
        .map(|i| {
            s.write(|w| w.insert("tasks", &obj(json!({"title": format!("row {i}")}))))
                .unwrap()
        })
        .collect();
    ids.sort();
    s.write(|w| w.delete("tasks", &ids[0])).unwrap();
    assert_eq!(s.row_ids_after("tasks", "", 2).unwrap(), ids[..2]);
    assert_eq!(s.row_ids_after("tasks", &ids[1], 10).unwrap(), ids[2..]);
    assert!(s.row_ids_after("tasks", "", 0).unwrap().is_empty());
    assert!(matches!(
        s.row_ids_after("nope", "", 1),
        Err(Error::UnknownTable(_))
    ));
}
