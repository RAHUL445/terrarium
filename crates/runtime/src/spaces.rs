//! The spaces of a user: separate sets of plugin data (Personal, Work, …)
//! sharing one plugin list and one device pairing. The list lives in a
//! host-owned store, `_spaces`, so a space created on one device appears
//! on the others.
//!
//! The default space, [`DEFAULT_SPACE`], always exists. It can be renamed
//! but not removed: its folder also holds the device-wide stores.

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

use crate::error::{Result, RuntimeError};

pub const SPACES_STORE_ID: &str = "_spaces";
pub const DEFAULT_SPACE: &str = "personal";
const MAX_NAME: usize = 40;

const SCHEMA: &str = r#"{
  "version": 1,
  "tables": [
    { "name": "spaces", "columns": [
      { "name": "name", "type": "text" },
      { "name": "removed", "type": "boolean", "default": false },
      { "name": "created_ms", "type": "integer" }
    ]}
  ]
}"#;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    pub removed: bool,
}

/// The sync id of a plugin's store in a space. The default space keeps the
/// plain plugin id, so data synced before spaces existed stays valid.
pub fn store_id(space: &str, plugin: &str) -> String {
    if space == DEFAULT_SPACE {
        plugin.to_string()
    } else {
        format!("{plugin}@{space}")
    }
}

/// `(space, plugin)` of a store id.
pub fn split_store_id(id: &str) -> (&str, &str) {
    match id.split_once('@') {
        Some((plugin, space)) => (space, plugin),
        None => (DEFAULT_SPACE, id),
    }
}

pub struct Spaces {
    store: CoreStore,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RuntimeError::Package("Give the space a name".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(RuntimeError::Package(format!(
            "Space names are limited to {MAX_NAME} characters"
        )));
    }
    Ok(name.to_string())
}

impl Spaces {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Spaces> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA)?;
        let store = CoreStore::open(
            data_dir.join(format!("{SPACES_STORE_ID}.db")),
            StoreConfig {
                plugin: SPACES_STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Spaces { store })
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    /// Every space, the default one first, then by name; removed included.
    pub fn list(&self) -> Result<Vec<SpaceInfo>> {
        let rows = self.store.query(
            "SELECT id, name, removed FROM spaces WHERE _deleted IS NULL ORDER BY name COLLATE NOCASE",
            &[],
        )?;
        let mut out: Vec<SpaceInfo> = rows
            .iter()
            .filter_map(|r| {
                Some(SpaceInfo {
                    id: r.get("id")?.as_str()?.to_string(),
                    name: r
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Space")
                        .to_string(),
                    removed: r
                        .get("removed")
                        .is_some_and(|v| v.as_bool().unwrap_or(v.as_i64().unwrap_or(0) != 0)),
                })
            })
            .collect();
        let default = out
            .iter()
            .position(|s| s.id == DEFAULT_SPACE)
            .map(|i| out.remove(i))
            .unwrap_or(SpaceInfo {
                id: DEFAULT_SPACE.into(),
                name: "Personal".into(),
                removed: false,
            });
        out.insert(
            0,
            SpaceInfo {
                removed: false,
                ..default
            },
        );
        Ok(out)
    }

    /// Ids of the spaces in use (not removed).
    pub fn active(&self) -> Result<Vec<String>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| !s.removed)
            .map(|s| s.id)
            .collect())
    }

    pub fn create(&mut self, name: &str) -> Result<SpaceInfo> {
        let name = clean_name(name)?;
        let mut bytes = [0u8; 6];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| RuntimeError::Package(format!("no randomness: {e}")))?;
        // Lowercase letters and digits: safe in file names and store ids.
        const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
        let id: String = std::iter::once('s')
            .chain(
                bytes
                    .iter()
                    .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char),
            )
            .collect();
        let mut values = Map::new();
        values.insert("name".into(), json!(name));
        values.insert("removed".into(), json!(false));
        values.insert("created_ms".into(), json!(now_ms()));
        self.store.write(|w| w.upsert("spaces", &id, &values))?;
        Ok(SpaceInfo {
            id,
            name,
            removed: false,
        })
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        let mut values = Map::new();
        values.insert("name".into(), json!(clean_name(name)?));
        self.store.write(|w| w.upsert("spaces", id, &values))?;
        Ok(())
    }

    /// Hides a space on every device; its data stays and it can come back.
    pub fn set_removed(&mut self, id: &str, removed: bool) -> Result<()> {
        if id == DEFAULT_SPACE && removed {
            return Err(RuntimeError::Package(
                "The Personal space cannot be removed; rename it instead".into(),
            ));
        }
        if !self.list()?.iter().any(|s| s.id == id) {
            return Err(RuntimeError::Package(format!("no space `{id}`")));
        }
        let mut values = Map::new();
        values.insert("removed".into(), json!(removed));
        self.store.write(|w| w.upsert("spaces", id, &values))?;
        Ok(())
    }
}
