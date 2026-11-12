//! Encrypted backups: made on one device, read back with the recovery
//! phrase on a fresh one, merged without undoing newer edits.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_blobs::{BlobKey, BlobStore};
use terrarium_core::{ManualTime, PluginSchema, Store, StoreConfig};
use terrarium_sync::{Backup, BackupBlobs, Roster, SpaceKey};

fn schema() -> PluginSchema {
    serde_json::from_value(json!({"version": 1, "tables": [
        {"name": "notes", "columns": [
            {"name": "title", "type": "text"},
            {"name": "body", "type": "text", "rule": "text"}
        ]}
    ]}))
    .unwrap()
}

fn stores(dir: &TempDir, device: &str, time: &ManualTime) -> BTreeMap<String, Store> {
    ["notes", "other"]
        .iter()
        .map(|p| {
            let cfg = StoreConfig {
                space: "personal".into(),
                plugin: p.to_string(),
                device: device.into(),
                user: "owner".into(),
            };
            let path = dir.path().join(format!("{device}-{p}.db"));
            (
                p.to_string(),
                Store::open(path, cfg, schema(), Box::new(time.clone())).unwrap(),
            )
        })
        .collect()
}

fn values(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

struct DiskBlobs {
    store: Mutex<BlobStore>,
}

impl DiskBlobs {
    fn new(root: &std::path::Path, key: &SpaceKey) -> Self {
        let mut store = BlobStore::open(root).unwrap();
        store.set_key(BlobKey::new("space", &key.to_bytes()));
        Self {
            store: Mutex::new(store),
        }
    }
}

impl BackupBlobs for DiskBlobs {
    fn list_blob_ids(&self) -> terrarium_sync::Result<Vec<String>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .list()
            .into_iter()
            .map(|id| id.to_string())
            .collect())
    }

    fn get_sealed(&self, id: &str) -> terrarium_sync::Result<Vec<u8>> {
        let id = id
            .parse()
            .map_err(|_| terrarium_sync::SyncError::Transport(format!("bad blob id {id}")))?;
        self.store
            .lock()
            .unwrap()
            .get_sealed(&id)
            .map_err(|e| terrarium_sync::SyncError::Transport(e.to_string()))
    }

    fn put_sealed(&self, id: &str, sealed: &[u8]) -> terrarium_sync::Result<()> {
        let id = id
            .parse()
            .map_err(|_| terrarium_sync::SyncError::Transport(format!("bad blob id {id}")))?;
        self.store
            .lock()
            .unwrap()
            .put_sealed(&id, sealed)
            .map_err(|e| terrarium_sync::SyncError::Transport(e.to_string()))
    }
}

#[test]
fn v2_backup_round_trip_restores_blobs() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1_000);
    let key = SpaceKey::generate();
    let mut laptop = stores(&dir, "laptop", &time);
    laptop
        .get_mut("notes")
        .unwrap()
        .write(|w| {
            w.insert(
                "notes",
                &values(json!({"title": "Plan", "body": "first draft"})),
            )
        })
        .unwrap();
    let source_blobs = DiskBlobs::new(&dir.path().join("blobs-src"), &key);
    let blob_id = source_blobs
        .store
        .lock()
        .unwrap()
        .put(b"hello from backup blob")
        .unwrap();

    let backup = Backup::collect_with_blobs(
        "space-1",
        &"laptop".into(),
        5_000,
        Roster::new(),
        &mut laptop,
        &source_blobs,
    )
    .unwrap();
    let sealed = backup.seal(&key).unwrap();
    assert_eq!(Backup::info(&sealed).unwrap().v, 2);
    let mut phone = stores(&dir, "phone", &time);
    let target_blobs = DiskBlobs::new(&dir.path().join("blobs-dst"), &key);
    let restored = Backup::open(&sealed, &key).unwrap();
    let report = restored
        .restore_with_blobs(&mut phone, &target_blobs)
        .unwrap();
    assert!(report.applied > 0);
    assert_eq!(restored.blobs.len(), 1);
    let opened = target_blobs.store.lock().unwrap().get(&blob_id).unwrap();
    assert_eq!(opened, b"hello from backup blob");
}

#[test]
fn v1_backup_still_restores() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1_000);
    let key = SpaceKey::generate();
    let mut laptop = stores(&dir, "laptop", &time);
    let backup = Backup::collect("s", &"laptop".into(), 1, Roster::new(), &mut laptop).unwrap();
    let v1 = backup.seal_v1_compat(&key).unwrap();
    assert_eq!(Backup::info(&v1).unwrap().v, 1);
    let mut only_notes: BTreeMap<String, Store> = stores(&dir, "tablet", &time)
        .into_iter()
        .filter(|(k, _)| k == "notes")
        .collect();
    let report = Backup::open(&v1, &key)
        .unwrap()
        .restore(&mut only_notes)
        .unwrap();
    assert_eq!(report.missing, vec!["other".to_string()]);
}

#[test]
fn tampered_blob_in_backup_is_rejected() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1_000);
    let key = SpaceKey::generate();
    let mut laptop = stores(&dir, "laptop", &time);
    let source_blobs = DiskBlobs::new(&dir.path().join("blobs-src"), &key);
    source_blobs
        .store
        .lock()
        .unwrap()
        .put(b"tamper me")
        .unwrap();
    let mut backup = Backup::collect_with_blobs(
        "space-1",
        &"laptop".into(),
        5_000,
        Roster::new(),
        &mut laptop,
        &source_blobs,
    )
    .unwrap();
    backup.blobs[0].sealed[10] ^= 1;
    let target_blobs = DiskBlobs::new(&dir.path().join("blobs-dst"), &key);
    let restored = Backup::open(&backup.seal(&key).unwrap(), &key).unwrap();
    let out = restored.restore_with_blobs(&mut laptop, &target_blobs);
    assert!(out.is_err());
}
