//! Simulated devices editing offline, then syncing; every test checks that
//! all devices end with identical data.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_core::{DeviceId, Error, ManualTime, PluginSchema, RowState, Store, StoreConfig};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

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
    path: std::path::PathBuf,
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
            path,
            sent: HashMap::new(),
        }
    }

    fn id(&self) -> DeviceId {
        self.name.into()
    }

    fn reopen(&mut self, version: u32) {
        self.store = open(&self.path, self.name, version, &self.time).unwrap();
    }

    fn insert(&mut self, table: &str, values: Value) -> String {
        self.store.write(|w| w.insert(table, &obj(values))).unwrap()
    }

    fn update(&mut self, table: &str, row: &str, values: Value) {
        self.store
            .write(|w| w.update(table, row, &obj(values)))
            .unwrap()
    }

    fn rows(&self, table: &str) -> Vec<Map<String, Value>> {
        self.store.dump().unwrap().remove(table).unwrap()
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

#[test]
fn local_writes_are_queryable_and_logged() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let row = a.insert("tasks", json!({"title": "Buy milk"}));
    a.update("tasks", &row, json!({"done": true}));
    a.store
        .write(|w| w.increment("tasks", &row, "points", json!(3)))
        .unwrap();

    let rows = a
        .store
        .query(
            "SELECT title, done, points FROM tasks WHERE _deleted IS NULL",
            &[],
        )
        .unwrap();
    assert_eq!(
        rows,
        vec![obj(json!({"title": "Buy milk", "done": 1, "points": 3}))]
    );
    // create + title + done + increment
    assert_eq!(a.store.changes(100).unwrap().len(), 4);
    assert_eq!(uuid_version(&row), 7);
}

fn uuid_version(id: &str) -> usize {
    uuid::Uuid::parse_str(id).unwrap().get_version_num()
}

#[test]
fn concurrent_edits_converge_and_keep_the_loser() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title": "Buy milk"}));
    sync(&mut a, &mut b);

    a.time.set(2_000);
    a.update("tasks", &row, json!({"title": "Buy milk A"}));
    b.time.set(3_000);
    b.update("tasks", &row, json!({"title": "Buy milk B"}));
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    assert_eq!(a.cell("tasks", &row, "title"), json!("Buy milk B"));
    for d in [&a, &b] {
        let conflicts = d.store.conflicts().unwrap();
        assert_eq!(conflicts.len(), 1, "{}", d.name);
        assert_eq!(conflicts[0].kept_value, json!("Buy milk B"));
        assert_eq!(conflicts[0].lost_value, json!("Buy milk A"));
    }
}

#[test]
fn sequential_edits_are_not_conflicts() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title": "one"}));
    sync(&mut a, &mut b);
    b.update("tasks", &row, json!({"title": "two"}));
    sync(&mut a, &mut b);
    a.update("tasks", &row, json!({"title": "three"}));
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    assert_eq!(b.cell("tasks", &row, "title"), json!("three"));
    assert!(a.store.conflicts().unwrap().is_empty());
    assert!(b.store.conflicts().unwrap().is_empty());
}

#[test]
fn counter_increments_add_up_and_redelivery_is_harmless() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title": "streak", "points": 0}));
    sync(&mut a, &mut b);

    a.store
        .write(|w| w.increment("tasks", &row, "points", json!(1)))
        .unwrap();
    b.store
        .write(|w| w.increment("tasks", &row, "points", json!(2)))
        .unwrap();
    sync(&mut a, &mut b);

    let (everything, _) = a.store.export_since(0).unwrap();
    let report = b.store.import(&everything).unwrap();
    assert_eq!(report.applied, 0);

    assert_converged(&[&a, &b]);
    assert_eq!(a.cell("tasks", &row, "points"), json!(3));
}

#[test]
fn trash_keeps_an_edit_made_offline_after_delete() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title": "Trip plan"}));
    sync(&mut a, &mut b);

    a.store.write(|w| w.delete("tasks", &row)).unwrap();
    b.time.set(5_000);
    b.update("tasks", &row, json!({"title": "Trip plan v2"}));
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    for d in [&a, &b] {
        assert_eq!(
            d.store.row_state("tasks", &row).unwrap(),
            Some(RowState::Trashed)
        );
        assert_eq!(d.cell("tasks", &row, "title"), json!("Trip plan v2"));
        let notices = d.store.notices().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, "edited_in_trash");
    }

    a.store.write(|w| w.restore("tasks", &row)).unwrap();
    sync(&mut a, &mut b);
    assert_converged(&[&a, &b]);
    assert_eq!(
        b.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Live)
    );
}

#[test]
fn delete_wins_discards_concurrent_edits_in_any_order() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tags", json!({"name": "home"}));
    sync(&mut a, &mut b);

    a.store.write(|w| w.delete("tags", &row)).unwrap();
    b.update("tags", &row, json!({"name": "house"}));
    // b receives the delete after its edit; a receives the edit after its delete
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    for d in [&a, &b] {
        assert!(d.rows("tags").is_empty());
        assert_eq!(
            d.store.row_state("tags", &row).unwrap(),
            Some(RowState::Gone)
        );
    }
    let err = a.store.write(|w| w.restore("tags", &row)).unwrap_err();
    assert!(matches!(err, Error::Policy { .. }));
}

#[test]
fn edit_wins_restores_a_concurrently_deleted_row() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("prefs", json!({"value": "dark"}));
    sync(&mut a, &mut b);

    a.store.write(|w| w.delete("prefs", &row)).unwrap();
    b.update("prefs", &row, json!({"value": "light"}));
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    for d in [&a, &b] {
        assert_eq!(
            d.store.row_state("prefs", &row).unwrap(),
            Some(RowState::Live)
        );
        assert_eq!(d.cell("prefs", &row, "value"), json!("light"));
        assert_eq!(d.store.notices().unwrap()[0].kind, "restored_by_edit");
    }

    // A later delete that has seen the edit sticks.
    a.store.write(|w| w.delete("prefs", &row)).unwrap();
    sync(&mut a, &mut b);
    assert_converged(&[&a, &b]);
    assert_eq!(
        b.store.row_state("prefs", &row).unwrap(),
        Some(RowState::Trashed)
    );
}

#[test]
fn write_api_enforces_rules() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let task = a.insert("tasks", json!({"title": "x"}));
    let entry = a.insert("log", json!({"msg": "hello"}));

    let err = a
        .store
        .write(|w| w.update("tasks", &task, &obj(json!({"points": 5}))))
        .unwrap_err();
    assert!(matches!(err, Error::IsCounter { .. }));
    let err = a
        .store
        .write(|w| w.increment("tasks", &task, "title", json!(1)))
        .unwrap_err();
    assert!(matches!(err, Error::NotCounter { .. }));
    let err = a
        .store
        .write(|w| w.update("tasks", &task, &obj(json!({"done": "yes"}))))
        .unwrap_err();
    assert!(matches!(err, Error::Type { .. }));
    let err = a
        .store
        .write(|w| w.update("log", &entry, &obj(json!({"msg": "edit"}))))
        .unwrap_err();
    assert!(matches!(err, Error::AppendOnly(_)));
    let err = a.store.write(|w| w.delete("log", &entry)).unwrap_err();
    assert!(matches!(err, Error::AppendOnly(_)));

    a.store.write(|w| w.delete("tasks", &task)).unwrap();
    let err = a
        .store
        .write(|w| w.update("tasks", &task, &obj(json!({"title": "y"}))))
        .unwrap_err();
    assert!(matches!(err, Error::InTrash { .. }));

    let err = a.store.query("DELETE FROM tasks", &[]).unwrap_err();
    assert!(matches!(err, Error::ReadOnlyQuery));
    let err = a.store.query("SELECT * FROM _pp_changes", &[]).unwrap_err();
    assert!(matches!(err, Error::ReadOnlyQuery));
}

#[test]
fn failed_write_leaves_nothing_behind() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    let result = a.store.write(|w| {
        w.insert("tasks", &obj(json!({"title": "kept?"})))?;
        w.insert("tasks", &obj(json!({"title": 42})))
    });
    assert!(result.is_err());
    assert!(a.rows("tasks").is_empty());
    assert!(a.store.changes(10).unwrap().is_empty());
}

#[test]
fn changes_from_a_newer_schema_wait_until_upgrade() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 2));
    let row = a.insert("tasks", json!({"title": "file taxes"}));
    sync(&mut a, &mut b);

    b.update(
        "tasks",
        &row,
        json!({"due": "2027-04-15", "title": "file taxes!"}),
    );
    send(&mut b, &mut a);
    // Both writes carry schema version 2, so a buffers both.
    assert_eq!(a.cell("tasks", &row, "title"), json!("file taxes"));

    a.reopen(2);
    assert_eq!(a.cell("tasks", &row, "due"), json!("2027-04-15"));
    assert_eq!(a.cell("tasks", &row, "title"), json!("file taxes!"));
    assert_converged(&[&a, &b]);
}

#[test]
fn schema_changes_require_a_version_bump_and_no_downgrade() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1);
    let path = dir.path().join("x.db");
    drop(open(&path, "x", 2, &time).unwrap());
    assert!(matches!(
        open(&path, "x", 1, &time),
        Err(Error::Downgrade { .. })
    ));

    let cfg = StoreConfig {
        space: "personal".into(),
        plugin: "test".into(),
        device: "x".into(),
        user: "owner".into(),
    };
    let mut changed = schema(2);
    changed.tables[0]
        .columns
        .push(serde_json::from_value(json!({"name": "extra", "type": "text"})).unwrap());
    let err = Store::open(&path, cfg, changed, Box::new(time.clone()))
        .err()
        .unwrap();
    assert!(matches!(err, Error::Migration(_)));
}

#[test]
fn clock_stays_monotonic_across_restart() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    a.time.set(50_000);
    let row = a.insert("tasks", json!({"title": "x"}));
    a.time.set(10); // wall clock jumped back
    a.reopen(1);
    a.update("tasks", &row, json!({"title": "y"}));
    let changes = a.store.changes(10).unwrap();
    assert!(changes[0].change.hlc > changes[1].change.hlc);
}

#[test]
fn purge_waits_for_retention_and_every_device() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b, mut c) = (
        Device::new(&dir, "a", 1),
        Device::new(&dir, "b", 1),
        Device::new(&dir, "c", 1),
    );
    let row = a.insert("tasks", json!({"title": "old"}));
    sync(&mut a, &mut b);
    sync(&mut a, &mut c);
    a.store.write(|w| w.delete("tasks", &row)).unwrap();

    let peers = [a.id(), b.id(), c.id()];
    a.time.advance(31 * DAY_MS);
    assert_eq!(
        a.store.purge(&peers, 30 * DAY_MS).unwrap(),
        0,
        "no one has seen the delete"
    );

    // b receives the delete, and a learns that on b's reply.
    sync(&mut a, &mut b);
    send(&mut b, &mut a);
    assert_eq!(
        a.store.purge(&peers, 30 * DAY_MS).unwrap(),
        0,
        "c has not seen it"
    );

    sync(&mut a, &mut c);
    send(&mut c, &mut a);
    assert_eq!(a.store.purge(&peers, 30 * DAY_MS).unwrap(), 1);
    assert_eq!(
        a.store.row_state("tasks", &row).unwrap(),
        Some(RowState::Gone)
    );
    assert!(a.rows("tasks").is_empty());
}

#[test]
fn purge_respects_retention() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("tasks", json!({"title": "recent"}));
    a.store.write(|w| w.delete("tasks", &row)).unwrap();
    sync(&mut a, &mut b);
    send(&mut b, &mut a);
    a.time.advance(DAY_MS);
    assert_eq!(a.store.purge(&[a.id(), b.id()], 30 * DAY_MS).unwrap(), 0);
}

/// Random offline edits on three devices with random partial syncs must
/// always converge once everyone has exchanged everything.
#[test]
fn random_offline_edits_converge() {
    let (mut trashed, mut gone, mut conflicts) = (0, 0, 0);
    for seed in 1..=12u64 {
        let (t, g, c) = run_random(seed);
        trashed += t;
        gone += g;
        conflicts += c;
    }
    // Guard against a vacuous test: the random runs must hit the hard cases.
    assert!(
        trashed > 0 && gone > 0 && conflicts > 0,
        "trashed {trashed}, gone {gone}, conflicts {conflicts}"
    );
}

fn run_random(seed: u64) -> (usize, usize, usize) {
    let dir = TempDir::new().unwrap();
    let mut devices = vec![
        Device::new(&dir, "a", 1),
        Device::new(&dir, "b", 1),
        Device::new(&dir, "c", 1),
    ];
    let mut rng = Rng(seed);
    let mut rows: Vec<(&str, String)> = Vec::new();

    for step in 0..250 {
        let d = rng.below(3);
        devices[d].time.advance(rng.below(50) as u64);
        match rng.below(10) {
            0 | 1 => {
                let table = ["tasks", "tags", "prefs", "notes"][rng.below(4)];
                let values = match table {
                    "tasks" => json!({"title": format!("t{step}"), "points": rng.below(3)}),
                    "tags" => json!({"name": format!("g{step}")}),
                    "notes" => json!({"body": format!("note {step}")}),
                    _ => json!({"value": format!("p{step}")}),
                };
                let id = devices[d].insert(table, values);
                rows.push((table, id));
            }
            2..=4 if !rows.is_empty() => {
                let (table, id) = &rows[rng.below(rows.len())];
                let column = match *table {
                    "tasks" => "title",
                    "tags" => "name",
                    "notes" => "body",
                    _ => "value",
                };
                let mut values = Map::new();
                let text = if *table == "notes" {
                    // Edit inside the current text, so merges interleave.
                    let current = devices[d].cell("notes", id, "body");
                    let current = current.as_str().unwrap_or("");
                    let cut = rng.below(current.chars().count() + 1);
                    let (head, tail): (String, String) = (
                        current.chars().take(cut).collect(),
                        current.chars().skip(cut).collect(),
                    );
                    format!("{head}[{}{step}]{tail}", devices[d].name)
                } else {
                    format!("{}@{step}", devices[d].name)
                };
                values.insert(column.into(), json!(text));
                let _ = devices[d].store.write(|w| w.update(table, id, &values));
            }
            5 if !rows.is_empty() => {
                let (_, id) = rows
                    .iter()
                    .find(|(t, _)| *t == "tasks")
                    .cloned()
                    .unwrap_or(("", String::new()));
                if !id.is_empty() {
                    let _ = devices[d]
                        .store
                        .write(|w| w.increment("tasks", &id, "points", json!(1)));
                }
            }
            6 if !rows.is_empty() => {
                let (table, id) = &rows[rng.below(rows.len())];
                let _ = devices[d].store.write(|w| w.delete(table, id));
            }
            7 if !rows.is_empty() => {
                let (table, id) = &rows[rng.below(rows.len())];
                let _ = devices[d].store.write(|w| w.restore(table, id));
            }
            _ => {
                let (x, y) = (rng.below(3), rng.below(3));
                if x != y {
                    let (from, to) = pair(&mut devices, x, y);
                    send(from, to);
                }
            }
        }
    }

    for _ in 0..2 {
        for x in 0..3 {
            for y in 0..3 {
                if x != y {
                    let (from, to) = pair(&mut devices, x, y);
                    send(from, to);
                }
            }
        }
    }
    let refs: Vec<&Device> = devices.iter().collect();
    assert_converged(&refs);

    let first = &devices[0];
    let state = |s| {
        rows.iter()
            .filter(|(t, id)| first.store.row_state(t, id).unwrap() == Some(s))
            .count()
    };
    (
        state(RowState::Trashed),
        state(RowState::Gone),
        first.store.conflicts().unwrap().len(),
    )
}

fn pair(devices: &mut [Device], x: usize, y: usize) -> (&mut Device, &mut Device) {
    if x < y {
        let (left, right) = devices.split_at_mut(y);
        (&mut left[x], &mut right[0])
    } else {
        let (left, right) = devices.split_at_mut(x);
        (&mut right[0], &mut left[y])
    }
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n
    }
}

#[test]
fn explicit_transaction_spans_writes_and_rolls_back() {
    let dir = TempDir::new().unwrap();
    let mut a = Device::new(&dir, "a", 1);
    a.store.begin().unwrap();
    a.insert("tasks", json!({"title": "one"}));
    // A failed write inside the transaction leaves nothing behind by itself...
    assert!(a
        .store
        .write(|w| {
            w.insert("tasks", &obj(json!({"title": "half"})))?;
            w.insert("tasks", &obj(json!({"title": 1})))
        })
        .is_err());
    a.insert("tasks", json!({"title": "two"}));
    assert_eq!(a.rows("tasks").len(), 2);
    // ...and rolling back the transaction undoes the successful ones too.
    a.store.rollback().unwrap();
    assert!(a.rows("tasks").is_empty());
    assert!(a.store.changes(10).unwrap().is_empty());

    a.store.begin().unwrap();
    a.insert("tasks", json!({"title": "kept"}));
    a.store.commit().unwrap();
    assert_eq!(a.rows("tasks").len(), 1);
}

#[test]
fn concurrent_text_edits_keep_both_changes() {
    let dir = TempDir::new().unwrap();
    let (mut a, mut b) = (Device::new(&dir, "a", 1), Device::new(&dir, "b", 1));
    let row = a.insert("notes", json!({"body": "Trip: book train"}));
    sync(&mut a, &mut b);

    a.update(
        "notes",
        &row,
        json!({"body": "Trip: book train, pack bags"}),
    );
    b.update("notes", &row, json!({"body": "Goa trip: book train"}));
    sync(&mut a, &mut b);

    assert_converged(&[&a, &b]);
    assert_eq!(
        a.cell("notes", &row, "body"),
        json!("Goa trip: book train, pack bags")
    );
    // Merged, not overwritten: nothing lands in conflict history.
    assert!(a.store.conflicts().unwrap().is_empty());

    // Clearing the text is an edit like any other.
    b.update("notes", &row, json!({"body": null}));
    sync(&mut a, &mut b);
    assert_eq!(a.cell("notes", &row, "body"), json!(""));

    // The CRDT state is not readable through plugin queries' column names.
    let rows = a.store.query("SELECT body FROM notes", &[]).unwrap();
    assert_eq!(rows[0].len(), 1);
}
