use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use terrarium_blobs::BlobId;
use terrarium_core::{ColumnType, FileRef};

use crate::registry::PluginHost;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const GRACE_MS: u64 = 30 * DAY_MS;
const MIN_MANUAL_AGE_MS: u64 = DAY_MS;
const LINK_PREFIX: &str = "terrarium:blob/";
const BLOB_ID_LEN: usize = 76;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct GcState {
    #[serde(default)]
    first_unreferenced_ms: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    pub referenced: usize,
    pub known_orphans: usize,
    pub deleted: usize,
    pub pending: usize,
    pub skipped_closed_spaces: bool,
}

pub fn referenced(host: &PluginHost) -> BTreeSet<BlobId> {
    let spaces: Vec<String> = host.open_spaces().map(str::to_string).collect();
    let mut out = BTreeSet::new();
    for manifest in host.manifests() {
        for space in &spaces {
            let Ok(store) = host.store_in(space, &manifest.id) else {
                continue;
            };
            for (table, column) in manifest.storage.file_columns() {
                let sql =
                    format!("SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL");
                if let Ok(rows) = store.query(&sql, &[]) {
                    for row in rows {
                        if let Some(text) = row.get(&column).and_then(serde_json::Value::as_str) {
                            if let Ok(file) = FileRef::parse_json(text) {
                                out.insert(file.blob);
                            }
                        }
                    }
                }
            }
            for table in &manifest.storage.tables {
                let text_columns: Vec<&str> = table
                    .columns
                    .iter()
                    .filter(|c| c.ty == ColumnType::Text)
                    .map(|c| c.name.as_str())
                    .collect();
                if text_columns.is_empty() {
                    continue;
                }
                let select = text_columns
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                let where_clause = text_columns
                    .iter()
                    .map(|c| format!("\"{c}\" LIKE '%{LINK_PREFIX}%'"))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let sql = format!(
                    "SELECT {select} FROM \"{}\" WHERE {where_clause}",
                    table.name
                );
                if let Ok(rows) = store.query(&sql, &[]) {
                    for row in rows {
                        for value in row.values() {
                            if let Some(text) = value.as_str() {
                                extract_markdown_refs(text, &mut out);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

pub fn collect(host: &PluginHost, now_ms: u64) -> GcReport {
    collect_with(host, now_ms, GRACE_MS, 0)
}

pub fn collect_now(host: &PluginHost, now_ms: u64) -> GcReport {
    collect_with(host, now_ms, 0, MIN_MANUAL_AGE_MS)
}

fn collect_with(host: &PluginHost, now_ms: u64, grace_ms: u64, min_age_ms: u64) -> GcReport {
    if host.has_closed_spaces() {
        return GcReport {
            skipped_closed_spaces: true,
            ..GcReport::default()
        };
    }
    let mut report = GcReport::default();
    let refs = referenced(host);
    report.referenced = refs.len();
    let mut state = load_state(host);
    state
        .first_unreferenced_ms
        .retain(|id, _| BlobId::from_str(id).is_ok() && !refs.iter().any(|r| r.as_str() == id));

    let blob_store = host.blob_store();
    let mut store = match blob_store.lock() {
        Ok(store) => store,
        Err(_) => return report,
    };
    let ids = store.list();
    let referenced_set: BTreeSet<String> = refs.into_iter().map(|id| id.to_string()).collect();
    for id in ids {
        let key = id.to_string();
        if referenced_set.contains(&key) {
            state.first_unreferenced_ms.remove(&key);
            continue;
        }
        let first = *state
            .first_unreferenced_ms
            .entry(key.clone())
            .or_insert(now_ms);
        let age = now_ms.saturating_sub(first);
        let deadline = grace_ms.max(min_age_ms);
        if age >= deadline {
            if store.remove(&id).is_ok() {
                state.first_unreferenced_ms.remove(&key);
                report.deleted += 1;
            } else {
                report.pending += 1;
            }
        } else {
            report.pending += 1;
        }
    }
    report.known_orphans = state.first_unreferenced_ms.len();
    save_state(host, &state);
    report
}

fn load_state(host: &PluginHost) -> GcState {
    let path = host.blobs_gc_path();
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_state(host: &PluginHost, state: &GcState) {
    let path = host.blobs_gc_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(state) {
        let _ = std::fs::write(path, bytes);
    }
}

fn extract_markdown_refs(text: &str, out: &mut BTreeSet<BlobId>) {
    let mut i = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        let Some(found) = rest.find(LINK_PREFIX) else {
            break;
        };
        let start = i + found + LINK_PREFIX.len();
        if text.len() >= start + BLOB_ID_LEN {
            let candidate = &text[start..start + BLOB_ID_LEN];
            if let Ok(id) = BlobId::from_str(candidate) {
                out.insert(id);
            }
        }
        i = start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extractor_finds_blob_links() {
        let mut out = BTreeSet::new();
        extract_markdown_refs(
            "![a](terrarium:blob/b1-01234567-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)",
            &mut out,
        );
        assert_eq!(out.len(), 1);
    }
}
