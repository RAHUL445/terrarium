//! Host-only, synced event history. Persistence after a plugin commit is best effort.
use std::path::Path;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

use crate::{EventDef, Result, RuntimeError};

pub const EVENTS_STORE_ID: &str = "_events";
pub const EVENT_RETENTION_MS: u64 = 7 * 24 * 3_600_000;
const FUTURE_SKEW_MS: u64 = 5 * 60_000;
const SCHEMA: &str = r#"{"version":1,"tables":[{"name":"events","delete_policy":"delete-wins","columns":[
{"name":"space","type":"text"},{"name":"plugin","type":"text"},
{"name":"name","type":"text"},{"name":"payload","type":"text"},
{"name":"contract","type":"text"},{"name":"depth","type":"integer","default":0},
{"name":"root","type":"text"},{"name":"device","type":"text"},
{"name":"emitted_ms","type":"integer"},{"name":"date","type":"text"},
{"name":"handled_ms","type":"integer"}]}]}"#;

pub struct EventLog {
    store: CoreStore,
}

/// Host-generated provenance is frozen when the emission is accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    pub id: String,
    pub space: String,
    pub plugin: String,
    pub name: String,
    pub payload: Map<String, Value>,
    pub contract: String,
    pub depth: u8,
    pub root: String,
    pub device: String,
    pub emitted_ms: u64,
    pub date: String,
}

fn invalid() -> RuntimeError {
    RuntimeError::Plugin {
        code: "invalid".into(),
        message: "Invalid event record".into(),
    }
}

pub(crate) fn contract(definition: &EventDef) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&definition.payload).expect("event contract serializes"))
    )
}

impl EventRecord {
    pub(crate) fn accept(
        peers: &crate::registry::Peers,
        plugin: &str,
        name: String,
        payload: Map<String, Value>,
        definition: &EventDef,
    ) -> Result<Self> {
        let emitted_ms = crate::registry::now_ms();
        let id = format!("ev-{}-{}", peers.device, uuid::Uuid::now_v7());
        Ok(Self {
            root: peers.event_root.clone().unwrap_or_else(|| id.clone()),
            id,
            space: peers.space.clone(),
            plugin: plugin.into(),
            name,
            payload,
            contract: contract(definition),
            depth: peers.event_depth,
            device: peers.device.clone(),
            emitted_ms,
            date: crate::time::local_date(emitted_ms, &peers.timezone).map_err(|_| invalid())?,
        })
    }

    fn decode(row: &Map<String, Value>) -> Result<Self> {
        let text = |key: &str| -> Result<String> {
            row.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(invalid)
        };
        let payload = text("payload")?;
        if payload.len() > 16_384 {
            return Err(invalid());
        }
        let event = Self {
            id: text("id")?,
            space: text("space")?,
            plugin: text("plugin")?,
            name: text("name")?,
            payload: serde_json::from_str::<Value>(&payload)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .ok_or_else(invalid)?,
            contract: text("contract")?,
            depth: row
                .get("depth")
                .and_then(Value::as_u64)
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(invalid)?,
            root: text("root")?,
            device: text("device")?,
            emitted_ms: row
                .get("emitted_ms")
                .and_then(Value::as_u64)
                .ok_or_else(invalid)?,
            date: text("date")?,
        };
        if event.contract.len() != 64
            || !event
                .contract
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || crate::time::date(&event.date).is_err()
            || !valid_id(&event.id, Some(&event.device))
            || !valid_id(&event.root, None)
        {
            return Err(invalid());
        }
        Ok(event)
    }
}

fn valid_id(id: &str, device: Option<&str>) -> bool {
    let Some(prefix) = id.get(..id.len().saturating_sub(36)) else {
        return false;
    };
    let Some(suffix) = id.get(id.len().saturating_sub(36)..) else {
        return false;
    };
    id.len() <= 256
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        && prefix.starts_with("ev-")
        && prefix.len() > 4
        && prefix.ends_with('-')
        && device.is_none_or(|d| prefix == format!("ev-{d}-"))
        && uuid::Uuid::parse_str(suffix).is_ok_and(|id| id.get_version_num() == 7)
}

impl EventLog {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Self> {
        Ok(Self {
            store: CoreStore::open(
                data_dir.join(format!("{EVENTS_STORE_ID}.db")),
                StoreConfig {
                    plugin: EVENTS_STORE_ID.into(),
                    ..cfg
                },
                serde_json::from_str::<PluginSchema>(SCHEMA)?,
                Box::new(SystemTime),
            )?,
        })
    }

    /// The accepted id is retained even when an insertion attempt fails.
    pub fn record(&mut self, event: &EventRecord) -> Result<String> {
        let values = json!({
            "space":event.space,"plugin":event.plugin,"name":event.name,
            "payload":serde_json::to_string(&event.payload).map_err(|_| invalid())?,
            "contract":event.contract,"depth":event.depth,"root":event.root,
            "device":event.device,"emitted_ms":event.emitted_ms,"date":event.date
        })
        .as_object()
        .cloned()
        .ok_or_else(invalid)?;
        self.store
            .write(|w| w.upsert("events", &event.id, &values))
            .map_err(|_| invalid())?;
        Ok(event.id.clone())
    }

    pub fn pending(&self, limit: usize) -> Result<Vec<EventRecord>> {
        self.pending_at(limit, crate::registry::now_ms())
    }

    pub(crate) fn pending_at(&self, limit: usize, now: u64) -> Result<Vec<EventRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Decode before limiting: malformed imported records must not starve valid work.
        let rows = self.store.query(
            "SELECT * FROM events WHERE _deleted IS NULL AND handled_ms IS NULL AND emitted_ms > ?1 AND emitted_ms <= ?2 ORDER BY emitted_ms, id",
            &[json!(now.saturating_sub(EVENT_RETENTION_MS)), json!(now.saturating_add(FUTURE_SKEW_MS))],
        )?;
        let mut events = Vec::new();
        for row in rows {
            match EventRecord::decode(&row) {
                Ok(event) => events.push(event),
                Err(_) => eprintln!("Invalid event metadata; event skipped"),
            }
            if events.len() == limit {
                break;
            }
        }
        Ok(events)
    }

    pub fn mark_handled(&mut self, id: &str, now_ms: u64) -> Result<()> {
        self.store.write(|w| {
            w.update(
                "events",
                id,
                json!({"handled_ms":now_ms}).as_object().expect("object"),
            )
        })?;
        Ok(())
    }

    /// Logical expiry includes pending events; sync tombstones may retain data.
    pub fn prune(&mut self, older_than_ms: u64) -> Result<usize> {
        let rows = self.store.query(
            "SELECT id FROM events WHERE _deleted IS NULL AND emitted_ms <= ?1",
            &[json!(older_than_ms)],
        )?;
        self.store.write(|w| {
            for row in &rows {
                w.delete("events", row["id"].as_str().expect("row id"))?;
            }
            Ok(())
        })?;
        Ok(rows.len())
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn open(dir: &tempfile::TempDir, device: &str) -> EventLog {
        EventLog::open(
            dir.path(),
            StoreConfig {
                space: "personal".into(),
                plugin: EVENTS_STORE_ID.into(),
                device: device.into(),
                user: "owner".into(),
            },
        )
        .unwrap()
    }

    fn event(device: &str, at: u64) -> EventRecord {
        let id = format!("ev-{device}-{}", uuid::Uuid::now_v7());
        EventRecord {
            root: id.clone(),
            id,
            space: "personal".into(),
            plugin: "limits".into(),
            name: "pinged".into(),
            payload: Map::new(),
            contract: contract(&EventDef::default()),
            depth: 0,
            device: device.into(),
            emitted_ms: at,
            date: "2027-01-15".into(),
        }
    }

    #[test]
    fn record_pending_and_mark_handled() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = open(&dir, "a");
        let first = event("a", NOW - 1);
        let second = event("a", NOW);
        log.record(&second).unwrap();
        log.record(&first).unwrap();
        assert_eq!(log.pending_at(1, NOW).unwrap(), vec![first.clone()]);
        assert!(log.pending_at(0, NOW).unwrap().is_empty());
        log.mark_handled(&first.id, NOW).unwrap();
        assert_eq!(log.pending_at(5, NOW).unwrap(), vec![second.clone()]);
        drop(log);
        assert_eq!(open(&dir, "a").pending_at(5, NOW).unwrap(), vec![second]);
    }

    #[test]
    fn prune_keeps_unexpired_unhandled_events_and_expires_pending_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = open(&dir, "a");
        let cutoff = NOW - EVENT_RETENTION_MS;
        let pending = event("a", cutoff);
        let handled = event("a", cutoff - 1);
        let fresh = event("a", cutoff + 1);
        for e in [&pending, &handled, &fresh] {
            log.record(e).unwrap();
        }
        log.mark_handled(&handled.id, NOW).unwrap();
        assert_eq!(log.pending_at(10, NOW).unwrap(), vec![fresh.clone()]);
        assert_eq!(log.prune(cutoff).unwrap(), 2);
        assert_eq!(log.pending_at(10, NOW).unwrap(), vec![fresh]);
        assert_eq!(log.prune(cutoff).unwrap(), 0);
    }

    #[test]
    fn two_devices_merge_event_logs_and_deletions_converge() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let mut a = open(&da, "a");
        let mut b = open(&db, "b");
        let ea = event("a", NOW - 1);
        let eb = event("b", NOW);
        a.record(&ea).unwrap();
        b.record(&eb).unwrap();
        let (ba, _) = a.store().export_since(0).unwrap();
        let (bb, _) = b.store().export_since(0).unwrap();
        a.store_mut().import(&bb).unwrap();
        b.store_mut().import(&ba).unwrap();
        assert_eq!(a.pending_at(5, NOW).unwrap(), b.pending_at(5, NOW).unwrap());
        assert_eq!(a.pending_at(5, NOW).unwrap(), vec![ea.clone(), eb.clone()]);
        a.mark_handled(&ea.id, NOW).unwrap();
        let (ba, _) = a.store().export_since(0).unwrap();
        b.store_mut().import(&ba).unwrap();
        assert_eq!(b.pending_at(5, NOW).unwrap(), vec![eb]);
        a.prune(NOW).unwrap();
        let (ba, _) = a.store().export_since(0).unwrap();
        b.store_mut().import(&ba).unwrap();
        // Replaying original rows cannot undo delete-wins expiry.
        b.store_mut().import(&bb).unwrap();
        assert!(b.pending_at(5, NOW).unwrap().is_empty());
    }

    #[test]
    fn future_clock_waits_and_malformed_imports_are_value_free() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = open(&dir, "a");
        let future = event("a", NOW + FUTURE_SKEW_MS + 1);
        log.record(&future).unwrap();
        assert!(log.pending_at(10, NOW).unwrap().is_empty());
        assert_eq!(log.pending_at(10, NOW + 1).unwrap(), vec![future]);
        let bad = event("a", NOW);
        log.record(&bad).unwrap();
        log.store_mut()
            .write(|w| {
                w.update(
                    "events",
                    &bad.id,
                    json!({"payload":"secret-private-value"})
                        .as_object()
                        .unwrap(),
                )
            })
            .unwrap();
        let rows = log
            .store()
            .query("SELECT * FROM events WHERE id = ?1", &[json!(bad.id)])
            .unwrap();
        assert!(!EventRecord::decode(&rows[0])
            .unwrap_err()
            .to_string()
            .contains("secret-private-value"));
        assert!(log.pending_at(1, NOW).unwrap().is_empty());
        for field in [
            "space",
            "plugin",
            "name",
            "contract",
            "depth",
            "root",
            "device",
            "emitted_ms",
            "date",
        ] {
            let mut row = rows[0].clone();
            row.insert("payload".into(), json!("{}"));
            row.insert(field.into(), Value::Null);
            assert!(EventRecord::decode(&row).is_err(), "{field}");
        }
    }

    #[test]
    fn failed_insert_preserves_accepted_id_and_redacts_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = open(&dir, "a");
        let mut accepted = event("a", NOW);
        accepted
            .payload
            .insert("value".into(), json!("secret-private-value"));
        let connection = rusqlite::Connection::open(dir.path().join("_events.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'secret-private-value'); END;").unwrap();
        let error = log.record(&accepted).unwrap_err();
        assert!(!error.to_string().contains("secret-private-value"));
        connection
            .execute_batch("DROP TRIGGER reject_event")
            .unwrap();
        assert_eq!(log.record(&accepted).unwrap(), accepted.id);
    }
}
