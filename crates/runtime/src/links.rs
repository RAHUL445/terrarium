//! Host-owned item relations, synced and backed up through the store set.
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

use crate::{ItemRef, Result, RuntimeError};

pub const LINKS_STORE_ID: &str = "_links";
pub const LINK_KINDS: &[&str] = &["related"];
const MAX_LIST: usize = 500;
const SCHEMA: &str = r#"{
  "version": 1,
  "tables": [
    { "name": "links", "columns": [
      { "name": "space", "type": "text" },
      { "name": "source", "type": "text" },
      { "name": "target", "type": "text" },
      { "name": "kind", "type": "text" },
      { "name": "created_ms", "type": "integer" }
    ]}
  ]
}"#;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkInfo {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: String,
    pub created_ms: i64,
}

pub struct Links {
    store: CoreStore,
}

fn bad(message: &str) -> RuntimeError {
    RuntimeError::Package(message.into())
}
fn valid_ref(item: &str) -> Result<()> {
    if ItemRef::parse(item).is_none() {
        return Err(bad("Links need refs like plugin/table/id"));
    }
    Ok(())
}
fn valid_pair(source: &str, target: &str, kind: &str) -> Result<()> {
    valid_ref(source)?;
    valid_ref(target)?;
    if !LINK_KINDS.contains(&kind) {
        return Err(bad("Unknown link kind"));
    }
    Ok(())
}
fn info(row: &Map<String, Value>) -> Result<LinkInfo> {
    Ok(LinkInfo {
        id: serde_json::from_value(row["id"].clone())?,
        source: serde_json::from_value(row["source"].clone())?,
        target: serde_json::from_value(row["target"].clone())?,
        kind: serde_json::from_value(row["kind"].clone())?,
        created_ms: serde_json::from_value(row["created_ms"].clone())?,
    })
}
impl Links {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Links> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA)?;
        let store = CoreStore::open(
            data_dir.join(format!("{LINKS_STORE_ID}.db")),
            StoreConfig {
                plugin: LINKS_STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Links { store })
    }
    pub fn store(&self) -> &CoreStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    /// Adds a relation, or returns the existing live one with the same space, source, target and kind.
    pub fn add(&mut self, space: &str, source: &str, target: &str, kind: &str) -> Result<LinkInfo> {
        valid_pair(source, target, kind)?;
        if source == target {
            return Err(bad("An item can't link to itself"));
        }
        let existing = self.store.query(
            "SELECT id,source,target,kind,created_ms FROM links WHERE _deleted IS NULL AND space=?1 AND source=?2 AND target=?3 AND kind=?4 ORDER BY id LIMIT 1",
            &[json!(space), json!(source), json!(target), json!(kind)],
        )?;
        if let Some(row) = existing.first() {
            return info(row);
        }
        let created_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let values = json!({"space":space,"source":source,"target":target,"kind":kind,"created_ms":created_ms});
        let id = self
            .store
            .write(|w| w.insert("links", values.as_object().expect("object")))?;
        Ok(LinkInfo {
            id,
            source: source.into(),
            target: target.into(),
            kind: kind.into(),
            created_ms,
        })
    }

    /// Removes every live relation between `a` and `b` (either direction) of `kind`. Returns how many.
    pub fn remove(&mut self, space: &str, a: &str, b: &str, kind: &str) -> Result<usize> {
        valid_pair(a, b, kind)?;
        let rows = self.store.query(
            "SELECT id FROM links WHERE _deleted IS NULL AND space=?1 AND kind=?4 AND ((source=?2 AND target=?3) OR (source=?3 AND target=?2))",
            &[json!(space), json!(a), json!(b), json!(kind)],
        )?;
        self.store.write(|w| {
            for row in &rows {
                w.delete("links", row["id"].as_str().expect("row id"))?;
            }
            Ok(())
        })?;
        Ok(rows.len())
    }

    /// Relations of `item` in `space`, either direction, newest first,
    /// one per (source, target, kind) even if two devices added it.
    pub fn list(&self, space: &str, item: &str) -> Result<Vec<LinkInfo>> {
        valid_ref(item)?;
        let rows = self.store.query(
            "SELECT id,source,target,kind,created_ms FROM links WHERE id IN (SELECT min(id) FROM links WHERE _deleted IS NULL AND space=?1 AND (source=?2 OR target=?2) GROUP BY source,target,kind) ORDER BY created_ms DESC,id LIMIT ?3",
            &[json!(space), json!(item), json!(MAX_LIST)],
        )?;
        rows.iter().map(info).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "todo/tasks/t1";
    const B: &str = "notes/notes/n1";
    fn open(dir: &tempfile::TempDir, device: &str) -> Links {
        Links::open(
            dir.path(),
            StoreConfig {
                space: "personal".into(),
                plugin: LINKS_STORE_ID.into(),
                device: device.into(),
                user: "owner".into(),
            },
        )
        .unwrap()
    }
    #[test]
    fn add_lists_both_directions_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = open(&dir, "a");
        let link = links.add("personal", A, B, "related").unwrap();
        assert_eq!(
            links.list("personal", A).unwrap().as_slice(),
            std::slice::from_ref(&link)
        );
        assert_eq!(
            links.list("personal", B).unwrap().as_slice(),
            std::slice::from_ref(&link)
        );
        assert_eq!(links.add("personal", A, B, "related").unwrap(), link);
        assert!(link.created_ms > 0);
        drop(links);
        assert_eq!(open(&dir, "a").list("personal", A).unwrap(), [link]);
    }
    #[test]
    fn bad_links_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = open(&dir, "a");
        for (source, target, kind, message) in [
            (A, A, "related", "An item can't link to itself"),
            ("nope", B, "related", "Links need refs like plugin/table/id"),
            (
                A,
                "../../etc",
                "related",
                "Links need refs like plugin/table/id",
            ),
            (A, B, "parent", "Unknown link kind"),
        ] {
            assert!(
                matches!(links.add("personal", source, target, kind), Err(RuntimeError::Package(s)) if s == message)
            );
        }
        assert!(links.list("personal", "nope").is_err());
        assert!(links.remove("personal", A, B, "parent").is_err());
        assert!(links.list("personal", A).unwrap().is_empty());
    }
    #[test]
    fn remove_deletes_either_direction() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = open(&dir, "a");
        links.add("personal", A, B, "related").unwrap();
        links.add("personal", B, A, "related").unwrap();
        assert_eq!(links.remove("personal", B, A, "related").unwrap(), 2);
        assert!(links.list("personal", A).unwrap().is_empty());
        assert_eq!(links.remove("personal", A, B, "related").unwrap(), 0);
        let trash = links
            .store()
            .query("SELECT id FROM links WHERE _deleted IS NOT NULL", &[])
            .unwrap();
        assert_eq!(trash.len(), 2);
        links.add("personal", A, B, "related").unwrap();
        assert_eq!(links.list("personal", A).unwrap().len(), 1);
        let schema: PluginSchema = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(schema.version, 1);
        assert_eq!(
            schema.tables[0].delete_policy,
            terrarium_core::DeletePolicy::Trash
        );
    }
    #[test]
    fn links_are_per_space() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = open(&dir, "a");
        let personal = links.add("personal", A, B, "related").unwrap();
        assert!(links.list("s2", A).unwrap().is_empty());
        assert_eq!(links.remove("s2", A, B, "related").unwrap(), 0);
        let other = links.add("s2", A, B, "related").unwrap();
        assert_ne!(other.id, personal.id);
        assert_eq!(links.list("personal", A).unwrap(), [personal]);
        assert_eq!(links.list("s2", B).unwrap(), [other]);
    }
    #[test]
    fn duplicates_from_two_devices_show_once() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut a = open(&dir_a, "a");
        let mut b = open(&dir_b, "b");
        let first = a.add("personal", A, B, "related").unwrap();
        let second = b.add("personal", A, B, "related").unwrap();
        let (from_a, _) = a.store().export_since(0).unwrap();
        let (from_b, _) = b.store().export_since(0).unwrap();
        a.store_mut().import(&from_b).unwrap();
        b.store_mut().import(&from_a).unwrap();
        let expected = if first.id < second.id { first } else { second };
        assert_eq!(
            a.list("personal", A).unwrap().as_slice(),
            std::slice::from_ref(&expected)
        );
        assert_eq!(
            a.list("personal", A).unwrap(),
            b.list("personal", A).unwrap()
        );
        assert_eq!(a.add("personal", A, B, "related").unwrap(), expected);
        assert_eq!(a.remove("personal", B, A, "related").unwrap(), 2);
        let (changes, _) = a.store().export_since(0).unwrap();
        b.store_mut().import(&changes).unwrap();
        assert!(a.list("personal", A).unwrap().is_empty());
        assert!(b.list("personal", A).unwrap().is_empty());
    }
    #[test]
    fn list_is_newest_first_and_limited_to_500() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = open(&dir, "a");
        links.store_mut().write(|w| {
            for n in 0..505 {
                w.insert("links", json!({"space":"personal","source":A,"target":format!("notes/notes/n{n}"),"kind":"related","created_ms":n}).as_object().unwrap())?;
            }
            Ok(())
        }).unwrap();
        let found = links.list("personal", A).unwrap();
        assert_eq!(found.len(), 500);
        assert_eq!(found[0].created_ms, 504);
        assert_eq!(found[499].created_ms, 5);
    }
}
