//! Deterministic historical views of a row's applied change log.
use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use loro::LoroDoc;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Change, ChangeRecord, Error, Hlc, Op, Result, TableDef};

/// How to group changes into versions.
#[derive(Clone, Copy, Debug)]
pub struct HistoryOptions {
    /// Adjacent changes by one device and user within this gap form a version.
    pub group_ms: u64,
    /// Newest versions returned, clamped to 1..=200.
    pub limit: usize,
    /// Hide versions ending before this wall time.
    pub since_ms: Option<u64>,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            group_ms: 120_000,
            limit: 50,
            since_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Version {
    /// Canonical HLC of this group's last change.
    pub id: String,
    pub at_ms: u64,
    pub device: String,
    pub user: String,
    pub columns: Vec<String>,
    pub created: bool,
    pub trashed: bool,
    pub restored: bool,
    /// Simplified Delete/Restore state, without edit-wins concurrency rules.
    pub in_trash: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RowAt {
    pub version: String,
    /// All current schema columns, excluding storage metadata.
    pub values: Map<String, Value>,
    pub in_trash: bool,
}

struct Replay<'a> {
    table: &'a TableDef,
    values: Map<String, Value>,
    texts: BTreeMap<String, LoroDoc>,
    in_trash: bool,
}

impl<'a> Replay<'a> {
    fn new(table: &'a TableDef) -> Self {
        Self {
            table,
            values: table
                .columns
                .iter()
                .map(|c| (c.name.clone(), c.default.clone().unwrap_or(Value::Null)))
                .collect(),
            texts: BTreeMap::new(),
            in_trash: false,
        }
    }

    fn apply(&mut self, change: &Change) -> Result<()> {
        match &change.op {
            Op::Set { column, value, .. } if self.values.contains_key(column) => {
                self.values.insert(column.clone(), value.clone());
            }
            Op::Increment { column, delta } if self.values.contains_key(column) => {
                let value = arithmetic(&self.values[column], delta, false)?;
                self.values.insert(column.clone(), value);
            }
            Op::TextEdit { column, update }
                if self.table.columns.iter().any(|c| c.name == *column) =>
            {
                let corrupt = |e| Error::Corrupt(format!("text CRDT: {e}"));
                let bytes = B64.decode(update).map_err(corrupt)?;
                self.texts
                    .entry(column.clone())
                    .or_default()
                    .import(&bytes)
                    .map_err(|e| Error::Corrupt(format!("text CRDT: {e}")))?;
            }
            Op::Delete { .. } => self.in_trash = true,
            Op::Restore => self.in_trash = false,
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self) -> Map<String, Value> {
        let mut values = self.values.clone();
        for (column, doc) in &self.texts {
            values.insert(
                column.clone(),
                Value::String(doc.get_text(crate::text::CONTAINER).to_string()),
            );
        }
        values
    }
}

/// Checked addition for replay and subtraction for restoration; null is zero.
pub(crate) fn arithmetic(a: &Value, b: &Value, subtract: bool) -> Result<Value> {
    let zero = Value::from(0);
    let a = if a.is_null() { &zero } else { a };
    let b = if b.is_null() { &zero } else { b };
    let bad = || Error::Corrupt("history counter arithmetic overflow or invalid number".into());
    if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
        return (if subtract {
            a.checked_sub(b)
        } else {
            a.checked_add(b)
        })
        .map(Value::from)
        .ok_or_else(bad);
    }
    let (a, b) = (a.as_f64().ok_or_else(bad)?, b.as_f64().ok_or_else(bad)?);
    serde_json::Number::from_f64(if subtract { a - b } else { a + b })
        .map(Value::Number)
        .ok_or_else(bad)
}

pub(crate) fn history(
    table: &TableDef,
    records: &[ChangeRecord],
    opts: &HistoryOptions,
) -> Result<Vec<Version>> {
    let mut replay = Replay::new(table);
    let mut versions: Vec<Version> = Vec::new();
    let mut previous: Option<&Change> = None;
    let mut columns = BTreeSet::new();
    for record in records {
        let change = &record.change;
        let new_group = previous.is_none_or(|p| {
            p.hlc.device != change.hlc.device
                || p.user != change.user
                || change.hlc.wall_ms.saturating_sub(p.hlc.wall_ms) > opts.group_ms
        });
        if new_group {
            columns.clear();
            versions.push(Version {
                id: String::new(),
                at_ms: 0,
                device: change.hlc.device.to_string(),
                user: change.user.clone(),
                columns: Vec::new(),
                created: false,
                trashed: false,
                restored: false,
                in_trash: false,
            });
        }
        replay.apply(change)?;
        let version = versions.last_mut().expect("group exists");
        match &change.op {
            Op::Create => version.created = true,
            Op::Delete { .. } => version.trashed = true,
            Op::Restore => version.restored = true,
            Op::Set { column, .. } | Op::Increment { column, .. } | Op::TextEdit { column, .. } => {
                if replay.values.contains_key(column) {
                    columns.insert(column.clone());
                }
            }
        }
        version.id = change.hlc.to_string();
        version.at_ms = change.hlc.wall_ms;
        version.in_trash = replay.in_trash;
        version.columns = columns.iter().cloned().collect();
        previous = Some(change);
    }
    Ok(versions
        .into_iter()
        .rev()
        .filter(|v| opts.since_ms.is_none_or(|since| v.at_ms >= since))
        .take(opts.limit.clamp(1, 200))
        .collect())
}

pub(crate) fn row_at(
    table: &TableDef,
    records: &[ChangeRecord],
    cutoff: &Hlc,
) -> Result<Option<RowAt>> {
    let mut replay = Replay::new(table);
    let mut found = false;
    for record in records.iter().take_while(|r| r.change.hlc <= *cutoff) {
        replay.apply(&record.change)?;
        found = true;
    }
    Ok(found.then(|| RowAt {
        version: cutoff.to_string(),
        values: replay.snapshot(),
        in_trash: replay.in_trash,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counter_arithmetic_preserves_numbers_and_rejects_invalid_results() {
        assert_eq!(
            arithmetic(&Value::Null, &json!(2), false).unwrap(),
            json!(2)
        );
        assert_eq!(
            arithmetic(&json!(2.5), &json!(0.5), true).unwrap(),
            json!(2.0)
        );
        assert!(arithmetic(&json!(i64::MAX), &json!(1), false).is_err());
        assert!(arithmetic(&json!(i64::MIN), &json!(1), true).is_err());
        assert!(arithmetic(&json!(f64::MAX), &json!(f64::MAX), false).is_err());
        assert!(arithmetic(&json!("invalid"), &json!(1), false).is_err());
    }
}
