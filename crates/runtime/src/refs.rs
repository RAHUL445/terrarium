//! Host-owned item references and batched descriptions. Plugin code never
//! receives another plugin's resolved rows.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use terrarium_core::RowState;

use crate::PluginHost;

/// `<plugin>/<table>/<row id>`: one item in one space.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemRef {
    pub plugin: String,
    pub table: String,
    pub id: String,
}

impl ItemRef {
    /// Parses a ref; `None` unless it has exactly three parts of the allowed characters.
    pub fn parse(text: &str) -> Option<ItemRef> {
        let mut parts = text.split('/');
        let (plugin, table, id) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some()
            || plugin.len() > 63
            || !plugin.starts_with(|c: char| c.is_ascii_lowercase())
            || !plugin
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || table.len() > 63
            || !table.starts_with(|c: char| c.is_ascii_lowercase())
            || !table
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            || table.contains("__")
            || id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        Some(ItemRef {
            plugin: plugin.into(),
            table: table.into(),
            id: id.into(),
        })
    }
}

impl std::fmt::Display for ItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.plugin, self.table, self.id)
    }
}

pub const MAX_RESOLVE: usize = 200;

/// Current descriptions of `refs` in `space`, deduplicated in input order.
pub fn resolve(host: &PluginHost, space: &str, refs: &[String]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    let mut groups: BTreeMap<(String, String), Vec<(usize, ItemRef)>> = BTreeMap::new();
    for text in refs {
        if !seen.insert(text) {
            continue;
        }
        let position = results.len();
        results.push(json!({"ref": text, "state": "unknown"}));
        if let Some(item) = ItemRef::parse(text) {
            groups
                .entry((item.plugin.clone(), item.table.clone()))
                .or_default()
                .push((position, item));
        }
    }
    for ((plugin, table), items) in groups {
        let Some(manifest) = host.manifests().find(|m| m.id == plugin) else {
            continue;
        };
        let Some(def) = manifest.refs.iter().find(|r| r.table == table) else {
            continue;
        };
        let ids: BTreeSet<_> = items.iter().map(|(_, item)| item.id.clone()).collect();
        let rows = host.query_in(
            space,
            &plugin,
            &def.resolve,
            &Map::from_iter([("ids".into(), json!(ids))]),
        );
        let failed = rows.is_err();
        let rows: BTreeMap<_, _> = rows
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                let id = row.get("id")?.as_str()?.to_string();
                ids.contains(&id).then_some((id, row))
            })
            .collect();
        for (position, item) in items {
            let mut result = json!({
                "ref": item.to_string(), "state": "missing", "plugin": plugin,
                "table": table, "id": item.id, "kind": def.kind, "icon": def.icon,
                "title": null,
            });
            if let Some(row) = rows.get(&item.id) {
                let title = match row.get("title") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    _ => String::new(),
                };
                result["state"] = json!("ok");
                result["title"] = json!(if title.trim().is_empty() {
                    "Untitled"
                } else {
                    &title
                });
                result["subtitle"] = row.get("subtitle").cloned().unwrap_or(Value::Null);
                result["done"] = json!(row
                    .get("done")
                    .is_some_and(|v| v == &Value::Bool(true) || v.as_f64() == Some(1.0)));
                result["open"] = json!(crate::commands::open_page(manifest)
                    .map(|page| json!({"plugin":plugin,"page":page,"item":item.id})));
            } else if !failed
                && host.store_in(space, &plugin).is_ok_and(|store| {
                    matches!(
                        store.row_state(&table, &item.id),
                        Ok(Some(RowState::Trashed))
                    )
                })
            {
                result["state"] = json!("trash");
            }
            results[position] = result;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_refs() {
        assert_eq!(
            ItemRef::parse("todo/tasks/0193f-ab"),
            Some(ItemRef {
                plugin: "todo".into(),
                table: "tasks".into(),
                id: "0193f-ab".into()
            })
        );
        assert!(ItemRef::parse("my-plugin/task_items/A_9-z").is_some());
        assert!(ItemRef::parse(&format!(
            "{}/{}/{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(64)
        ))
        .is_some());
    }

    #[test]
    fn parse_rejects_bad_refs() {
        for text in [
            "",
            "todo/tasks",
            "a/b/c/d",
            "Todo/tasks/x",
            "todo/Tasks/x",
            "todo/tasks/x y",
            "../tasks/x",
            "/tasks/x",
            "todo//x",
            "todo/tasks/",
            "a_b/tasks/x",
            "1a/tasks/x",
            "a/1b/x",
            "a/b__c/x",
            "a/b/x.y",
            "a/b/é",
            "a/b/\nx",
        ] {
            assert!(ItemRef::parse(text).is_none(), "{text:?}");
        }
        for text in [
            format!("todo/tasks/{}", "x".repeat(65)),
            format!("{}/tasks/x", "a".repeat(64)),
            format!("todo/{}/x", "b".repeat(64)),
        ] {
            assert!(ItemRef::parse(&text).is_none());
        }
    }

    #[test]
    fn display_round_trips() {
        let text = "todo/tasks/0193f-ab";
        let item = ItemRef::parse(text).unwrap();
        assert_eq!(item.to_string(), text);
        assert_eq!(ItemRef::parse(&item.to_string()), Some(item));
    }
}
