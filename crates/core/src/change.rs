//! The unit of sync: a single change to one row, and batches of them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hlc::{DeviceId, Hlc};

/// Highest change timestamp seen from each origin device.
pub type VersionVector = BTreeMap<DeviceId, Hlc>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub hlc: Hlc,
    pub user: String,
    pub table: String,
    pub row: String,
    /// Plugin schema version the writer had; newer versions wait in a buffer.
    pub schema_version: u32,
    pub op: Op,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Create,
    /// Last-writer-wins cell write. `base` is the cell timestamp the writer
    /// saw; a mismatch on another device means the writes were concurrent.
    Set {
        column: String,
        value: Value,
        base: Option<Hlc>,
    },
    Increment {
        column: String,
        delta: Value,
    },
    /// A Loro update for a `text` column, base64-encoded.
    TextEdit {
        column: String,
        update: String,
    },
    /// `seen` is the row's per-device version vector at delete time; edits
    /// newer than it were concurrent with the delete.
    Delete {
        seen: VersionVector,
    },
    Restore,
}

impl Op {
    pub(crate) fn is_edit(&self) -> bool {
        matches!(
            self,
            Op::Set { .. } | Op::Increment { .. } | Op::TextEdit { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub plugin: String,
    pub sender: DeviceId,
    /// What the sender had applied when it built this batch; receivers use
    /// it as the sender's acknowledgement for purge decisions.
    pub sender_seen: VersionVector,
    pub changes: Vec<Change>,
}
