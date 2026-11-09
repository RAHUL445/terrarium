//! Encrypted backups: made on one device, read back with the recovery
//! phrase on a fresh one, merged without undoing newer edits.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_core::{ManualTime, PluginSchema, Store, StoreConfig};
use terrarium_sync::{Backup, Roster, SpaceKey};

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

#[test]
fn backup_restores_with_the_recovery_phrase_and_keeps_newer_edits() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1_000);
    let key = SpaceKey::generate();
    let phrase = key.recovery_phrase();

    let mut laptop = stores(&dir, "laptop", &time);
    let id = laptop
        .get_mut("notes")
        .unwrap()
        .write(|w| {
            w.insert(
                "notes",
                &values(json!({"title": "Plan", "body": "first draft"})),
            )
        })
        .unwrap();
    let backup = Backup::collect(
        "space-1",
        &"laptop".into(),
        5_000,
        Roster::new(),
        &mut laptop,
    )
    .unwrap();
    let sealed = backup.seal(&key).unwrap();

    // Readable header, unreadable content without the key.
    let info = Backup::info(&sealed).unwrap();
    assert_eq!(info.space, "space-1");
    assert!(!String::from_utf8_lossy(&sealed).contains("first draft"));
    let wrong = SpaceKey::generate();
    assert!(Backup::open(&sealed, &wrong).is_err());

    // A fresh device gets everything back.
    let mut phone = stores(&dir, "phone", &time);
    let restored =
        Backup::open(&sealed, &SpaceKey::from_recovery_phrase(&phrase).unwrap()).unwrap();
    let report = restored.restore(&mut phone).unwrap();
    assert!(report.applied > 0);
    let rows = phone["notes"]
        .query("SELECT title, body FROM notes", &[])
        .unwrap();
    assert_eq!(rows[0]["body"], json!("first draft"));

    // Restoring an old backup does not undo a later edit, and repeats are no-ops.
    time.advance(10_000);
    laptop
        .get_mut("notes")
        .unwrap()
        .write(|w| w.update("notes", &id, &values(json!({"title": "Plan v2"}))))
        .unwrap();
    let again = restored.restore(&mut laptop).unwrap();
    assert_eq!(again.applied, 0);
    let rows = laptop["notes"]
        .query("SELECT title FROM notes", &[])
        .unwrap();
    assert_eq!(rows[0]["title"], json!("Plan v2"));

    // Tampering is detected.
    let mut bad = sealed.clone();
    let text = String::from_utf8(bad.clone()).unwrap();
    let at = text.find("\"ciphertext\":\"").unwrap() + 20;
    bad[at] = if bad[at] == b'A' { b'B' } else { b'A' };
    assert!(Backup::open(&bad, &key).is_err());
}

#[test]
fn restore_reports_stores_this_device_lacks() {
    let dir = TempDir::new().unwrap();
    let time = ManualTime::new(1_000);
    let key = SpaceKey::generate();
    let mut laptop = stores(&dir, "laptop", &time);
    let backup = Backup::collect("s", &"laptop".into(), 1, Roster::new(), &mut laptop).unwrap();
    let mut only_notes: BTreeMap<String, Store> = stores(&dir, "tablet", &time)
        .into_iter()
        .filter(|(k, _)| k == "notes")
        .collect();
    let report = Backup::open(&backup.seal(&key).unwrap(), &key)
        .unwrap()
        .restore(&mut only_notes)
        .unwrap();
    assert_eq!(report.missing, vec!["other".to_string()]);
}
