//! Devices syncing through a shared batch store, end to end: encryption,
//! signatures, relaying, tampering and late-installed plugins.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_core::{DeviceId, ManualTime, PluginSchema, RowState, Store, StoreConfig};
use terrarium_sync::{
    BatchStore, DeviceKeys, Envelope, FolderStore, MemoryStore, Roster, SpaceKey, SpaceSync,
};

fn schema() -> PluginSchema {
    serde_json::from_value(json!({"version": 1, "tables": [
        {"name": "tasks", "columns": [
            {"name": "title", "type": "text"},
            {"name": "points", "type": "integer", "rule": "counter"}
        ]}
    ]}))
    .unwrap()
}

struct Device {
    stores: BTreeMap<String, Store>,
    sync: SpaceSync,
    time: ManualTime,
    dir: std::path::PathBuf,
    id: DeviceId,
}

fn open_store(dir: &Path, device: &DeviceId, plugin: &str, time: &ManualTime) -> Store {
    let cfg = StoreConfig {
        space: "personal".into(),
        plugin: plugin.into(),
        device: device.clone(),
        user: "owner".into(),
    };
    Store::open(
        dir.join(format!("{plugin}.db")),
        cfg,
        schema(),
        Box::new(time.clone()),
    )
    .unwrap()
}

fn device(
    root: &TempDir,
    name: &str,
    key: &SpaceKey,
    keys: &DeviceKeys,
    roster: &Roster,
    plugins: &[&str],
) -> Device {
    let dir = root.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let time = ManualTime::new(1_000);
    let id = keys.device.clone();
    let stores = plugins
        .iter()
        .map(|p| (p.to_string(), open_store(&dir, &id, p, &time)))
        .collect();
    let sync = SpaceSync::open(
        "personal",
        keys.clone(),
        key.clone(),
        roster.clone(),
        &dir.join("sync"),
    )
    .unwrap();
    Device {
        stores,
        sync,
        time,
        dir,
        id,
    }
}

fn space(names: &[&str]) -> (SpaceKey, Vec<DeviceKeys>, Roster) {
    let key = SpaceKey::generate();
    let keys: Vec<DeviceKeys> = names
        .iter()
        .map(|n| DeviceKeys::generate((*n).into()))
        .collect();
    let roster = keys
        .iter()
        .map(|k| (k.device.clone(), k.public(k.device.0.as_str())))
        .collect();
    (key, keys, roster)
}

impl Device {
    fn insert(&mut self, title: &str) -> String {
        let values = obj(json!({"title": title}));
        self.stores
            .get_mut("todo")
            .unwrap()
            .write(|w| w.insert("tasks", &values))
            .unwrap()
    }

    fn sync(&mut self, transport: &dyn BatchStore) -> terrarium_sync::SyncReport {
        self.sync.sync(&mut self.stores, transport).unwrap()
    }

    fn dump(&self, plugin: &str) -> Value {
        json!(self.stores[plugin].dump().unwrap())
    }
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn two_devices_converge_through_a_folder() {
    let root = TempDir::new().unwrap();
    let shared = FolderStore::new(root.path().join("shared"));
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);

    let row = mac.insert("Pay rent");
    assert_eq!(mac.sync(&shared).pushed, Some(1));
    let report = phone.sync(&shared);
    assert_eq!(report.batches, 1);
    assert!(report.applied >= 2);

    // Both edit offline, then sync in either order.
    mac.time.set(2_000);
    mac.stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.update("tasks", &row, &obj(json!({"title": "Pay rent (mac)"}))))
        .unwrap();
    phone.time.set(3_000);
    phone
        .stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.update("tasks", &row, &obj(json!({"title": "Pay rent (phone)"}))))
        .unwrap();
    phone
        .stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.increment("tasks", &row, "points", json!(2)))
        .unwrap();
    mac.stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.increment("tasks", &row, "points", json!(1)))
        .unwrap();

    phone.sync(&shared);
    mac.sync(&shared);
    phone.sync(&shared);
    assert_eq!(mac.dump("todo"), phone.dump("todo"));
    assert_eq!(
        mac.dump("todo")["tasks"][0]["title"],
        json!("Pay rent (phone)")
    );
    assert_eq!(mac.dump("todo")["tasks"][0]["points"], json!(3));

    // The store only holds ciphertext.
    for device in shared.devices().unwrap() {
        for seq in shared.list(&device, 0).unwrap() {
            let bytes = String::from_utf8(shared.get(&device, seq).unwrap()).unwrap();
            assert!(
                !bytes.contains("Pay rent") && !bytes.contains("tasks"),
                "plaintext leaked: {bytes}"
            );
        }
    }

    // Nothing new: nothing published.
    assert_eq!(mac.sync(&shared).pushed, None);
}

#[test]
fn relayed_changes_reach_a_device_that_never_talks_to_the_origin() {
    let root = TempDir::new().unwrap();
    let (key, keys, roster) = space(&["a", "b", "c"]);
    let ab = MemoryStore::new();
    let bc = MemoryStore::new();
    let mut a = device(&root, "a", &key, &keys[0], &roster, &["todo"]);
    let mut b = device(&root, "b", &key, &keys[1], &roster, &["todo"]);
    let mut c = device(&root, "c", &key, &keys[2], &roster, &["todo"]);

    let row = a.insert("from a");
    a.sync(&ab);
    b.sync(&ab);
    b.stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.update("tasks", &row, &obj(json!({"title": "edited by b"}))))
        .unwrap();
    b.sync(&bc);
    c.sync(&bc);

    assert_eq!(c.dump("todo"), b.dump("todo"));
    assert!(
        c.stores["todo"].conflicts().unwrap().is_empty(),
        "causal order kept: no false conflict"
    );
}

#[test]
fn tampered_forged_and_foreign_batches_are_rejected() {
    let root = TempDir::new().unwrap();
    let (key, keys, roster) = space(&["mac", "phone"]);
    let store = MemoryStore::new();
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
    mac.insert("one");
    mac.sync(&store);

    // Flip a byte of the stored ciphertext.
    let original = store.get(&mac.id, 1).unwrap();
    let mut env: Value = serde_json::from_slice(&original).unwrap();
    let ct = env["ciphertext"].as_str().unwrap().to_string();
    let flipped = match ct.strip_prefix('A') {
        Some(rest) => format!("B{rest}"),
        None => format!("A{}", &ct[1..]),
    };
    env["ciphertext"] = json!(flipped);
    store.replace(&mac.id, 1, serde_json::to_vec(&env).unwrap());
    let report = phone.sync(&store);
    assert_eq!(report.rejected.len(), 1);
    assert_eq!(report.rejected[0].reason, "signature does not match");
    assert!(phone.dump("todo")["tasks"].as_array().unwrap().is_empty());

    // The real batch, once restored, applies; rejection did not skip it.
    store.replace(&mac.id, 1, original);
    assert_eq!(phone.sync(&store).batches, 1);
    assert_eq!(phone.dump("todo"), mac.dump("todo"));

    // A device outside the roster, even holding the space key, is ignored.
    let intruder = DeviceKeys::generate("intruder".into());
    let sealed = Envelope::seal(
        "personal",
        1,
        &terrarium_sync::Payload {
            batches: vec![],
            received: Default::default(),
        },
        &key,
        &intruder,
    )
    .unwrap();
    store
        .put(&intruder.device, 1, &sealed.to_bytes().unwrap())
        .unwrap();
    let report = phone.sync(&store);
    assert_eq!(report.unknown_devices, vec![intruder.device.clone()]);

    // A paired device with the wrong space key cannot be read.
    let wrong = SpaceKey::generate();
    let mut stranger = device(&root, "stranger", &wrong, &keys[0], &roster, &["todo"]);
    let report = stranger.sync.pull(&mut stranger.stores, &store).unwrap();
    assert_eq!(
        report.rejected[0].reason,
        "could not decrypt with this space key"
    );

    // A batch copied to another sequence number fails.
    store
        .put(&mac.id, 7, &store.get(&mac.id, 1).unwrap())
        .unwrap();
    let mut fresh = device(&root, "fresh", &key, &keys[1], &roster, &["todo"]);
    let report = fresh.sync(&store);
    assert!(report.rejected.iter().any(|r| r.seq == 7), "{report:?}");
}

#[test]
fn batches_for_a_missing_plugin_wait_until_it_is_installed() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo", "notes"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);

    let values = obj(json!({"title": "note"}));
    mac.stores
        .get_mut("notes")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();
    mac.sync(&store);
    let report = phone.sync(&store);
    assert!(report.pending > 0);

    let notes = open_store(&phone.dir, &phone.id, "notes", &phone.time);
    phone.stores.insert("notes".into(), notes);
    let report = phone.sync(&store);
    assert!(report.applied > 0);
    assert_eq!(phone.dump("notes"), mac.dump("notes"));
}

#[test]
fn acknowledgements_flow_so_trash_can_be_purged() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
    let row = mac.insert("old");
    mac.stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.delete("tasks", &row))
        .unwrap();
    mac.sync(&store);
    phone.sync(&store); // phone has no changes, but publishes its acknowledgement
    mac.sync(&store);

    mac.time.advance(31 * 24 * 3600 * 1000);
    let peers = [mac.id.clone(), phone.id.clone()];
    let purged = mac
        .stores
        .get_mut("todo")
        .unwrap()
        .purge(&peers, 30 * 24 * 3600 * 1000)
        .unwrap();
    assert_eq!(purged, 1);
    assert_eq!(
        mac.stores["todo"].row_state("tasks", &row).unwrap(),
        Some(RowState::Gone)
    );
}

#[test]
fn state_survives_restart_without_reapplying() {
    let root = TempDir::new().unwrap();
    let shared = FolderStore::new(root.path().join("shared"));
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    mac.insert("one");
    mac.sync(&shared);
    {
        let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
        assert_eq!(phone.sync(&shared).batches, 1);
    }
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
    let report = phone.sync(&shared);
    assert_eq!(report.batches, 0);
    assert_eq!(phone.sync.state().received[&mac.id], 1);

    mac.insert("two");
    assert_eq!(mac.sync(&shared).pushed, Some(2));
}

#[test]
fn compaction_keeps_unacknowledged_batches_and_new_devices_start_from_the_snapshot() {
    let root = TempDir::new().unwrap();
    let folder = FolderStore::new(root.path().join("relay"));
    let (key, keys, roster) = space(&["mac", "phone", "tablet"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);

    for i in 0..5 {
        mac.insert(&format!("mac {i}"));
        mac.sync(&folder);
        phone.insert(&format!("phone {i}"));
        phone.sync(&folder);
    }
    mac.sync(&folder);
    let before: usize = folder
        .devices()
        .unwrap()
        .iter()
        .map(|d| folder.list(d, 0).unwrap().len())
        .sum();

    // The tablet is paired but has never synced: nothing can be removed yet.
    assert_eq!(mac.sync.compact(&mut mac.stores, &folder).unwrap(), 0);

    // Without the tablet in the roster, everything both devices confirmed goes.
    let two: Roster = roster
        .iter()
        .filter(|(d, _)| d.0 != "tablet")
        .map(|(d, p)| (d.clone(), p.clone()))
        .collect();
    mac.sync.set_roster(two.clone());
    phone.sync.set_roster(two);
    phone.sync(&folder); // phone acknowledges the mac's latest batch
    mac.sync(&folder);
    let removed = mac.sync.compact(&mut mac.stores, &folder).unwrap();
    let after: usize = folder
        .devices()
        .unwrap()
        .iter()
        .map(|d| folder.list(d, 0).unwrap().len())
        .sum();
    assert!(
        removed > 0 && after < before,
        "removed {removed}, {before} -> {after}"
    );

    // A device joining now gets everything from the snapshot plus what is left.
    let mut tablet = device(&root, "tablet", &key, &keys[2], &roster, &["todo"]);
    let report = tablet.sync(&folder);
    assert_eq!(report.bootstrapped_from, Some(mac.id.clone()));
    assert_eq!(tablet.dump("todo"), mac.dump("todo"));
    assert_eq!(tablet.dump("todo")["tasks"].as_array().unwrap().len(), 10);

    // And keeps syncing normally afterwards.
    phone.sync.set_roster(roster.clone());
    phone.insert("after compaction");
    phone.sync(&folder);
    tablet.sync(&folder);
    assert_eq!(tablet.dump("todo"), phone.dump("todo"));
}

#[test]
fn snapshot_carries_batches_for_plugins_the_host_lacks() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let (key, keys, roster) = space(&["mac", "phone", "tablet"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo", "notes"]);
    let values = obj(json!({"title": "only on phone and tablet"}));
    phone
        .stores
        .get_mut("notes")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();
    phone.sync(&store);
    mac.sync(&store); // parks the notes batch
    mac.sync.snapshot(&mut mac.stores, &store).unwrap();

    // Only the mac's snapshot is visible to the tablet.
    let only_snapshot = MemoryStore::new();
    only_snapshot
        .put_snapshot(&mac.id, &store.snapshot(&mac.id).unwrap().unwrap())
        .unwrap();
    let mut tablet = device(&root, "tablet", &key, &keys[2], &roster, &["todo", "notes"]);
    tablet.sync(&only_snapshot);
    assert_eq!(tablet.dump("notes"), phone.dump("notes"));
}

#[test]
fn a_tampered_snapshot_is_not_used() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    mac.insert("one");
    mac.sync(&store);
    mac.sync.snapshot(&mut mac.stores, &store).unwrap();
    let mut env: Value =
        serde_json::from_slice(&store.snapshot(&mac.id).unwrap().unwrap()).unwrap();
    env["seq"] = json!(999);
    store
        .put_snapshot(&mac.id, &serde_json::to_vec(&env).unwrap())
        .unwrap();

    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
    let report = phone.sync(&store);
    assert!(report.bootstrapped_from.is_none());
    assert!(!report.rejected.is_empty());
    // It still gets the data from the batch itself.
    assert_eq!(phone.dump("todo"), mac.dump("todo"));
}

#[test]
fn idle_devices_stop_publishing() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let (key, keys, roster) = space(&["mac", "phone"]);
    let mut mac = device(&root, "mac", &key, &keys[0], &roster, &["todo"]);
    let mut phone = device(&root, "phone", &key, &keys[1], &roster, &["todo"]);
    mac.insert("one");
    phone.insert("two");
    for _ in 0..3 {
        mac.sync(&store);
        phone.sync(&store);
    }
    // After a few rounds nothing new and no acknowledgement is outstanding.
    assert_eq!(mac.sync(&store).pushed, None);
    assert_eq!(phone.sync(&store).pushed, None);
}
