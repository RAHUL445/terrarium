//! User pages: per-space pages holding a grid of widget placements. The
//! host owns them (plugins never read or change them) and keeps them in a
//! host-owned store, `_pages`, so a page arranged on one device appears on
//! the others.
//!
//! Each placement is its own row: edits to different widgets on two devices
//! merge, and overlaps left by a merge are resolved when the page is drawn.
//! Each space has a Home page, `home-<space>`, that cannot be removed; its
//! id is fixed so devices creating it offline end up with one page.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

use crate::error::{Result, RuntimeError};

pub const PAGES_STORE_ID: &str = "_pages";
/// The plugin id of placements of the host's own widgets.
pub const HOST_WIDGETS: &str = "_host";
pub const GRID_COLUMNS: i64 = 12;
pub const MAX_PLACEMENTS: usize = 40;
const MAX_PAGES: usize = 50;
const MAX_FOLDERS: usize = 200;
const MAX_POSITION: i64 = 1_000_000;
const MAX_TITLE: usize = 40;
const MAX_FOCUS: usize = 40;
const MAX_ROWS: i64 = 400;
const MAX_CONFIG_BYTES: usize = 4096;
const MAX_FILTERS_BYTES: usize = 2048;
const MAX_FIELDS_BYTES: usize = 1400;
const MAX_ID: usize = 64;

const SCHEMA: &str = r#"{
  "version": 6,
  "tables": [
    { "name": "pages", "delete_policy": "delete-wins", "columns": [
      { "name": "space", "type": "text" },
      { "name": "folder_id", "type": "text" },
      { "name": "archived", "type": "boolean", "default": false },
      { "name": "trash_batch", "type": "text" },
      { "name": "layout_mode", "type": "text", "default": "grid" },
      { "name": "title", "type": "text" },
      { "name": "icon", "type": "text" },
      { "name": "position", "type": "integer", "default": 0 },
      { "name": "system", "type": "boolean", "default": false },
      { "name": "focus", "type": "text" },
      { "name": "period", "type": "text" }
    ]},
    { "name": "folders", "delete_policy": "delete-wins", "columns": [
      { "name": "space", "type": "text" },
      { "name": "parent_id", "type": "text" },
      { "name": "title", "type": "text" },
      { "name": "position", "type": "integer", "default": 0 },
      { "name": "archived", "type": "boolean", "default": false },
      { "name": "trash_batch", "type": "text" }
    ]},
    { "name": "placements", "delete_policy": "delete-wins", "columns": [
      { "name": "page", "type": "text" },
      { "name": "plugin", "type": "text" },
      { "name": "widget", "type": "text" },
      { "name": "x", "type": "integer", "default": 0 },
      { "name": "y", "type": "integer", "default": 0 },
      { "name": "w", "type": "integer", "default": 4 },
      { "name": "h", "type": "integer", "default": 3 },
      { "name": "config", "type": "text" },
      { "name": "focus", "type": "text" },
      { "name": "flow_order", "type": "integer" }
    ]},
    { "name": "saved_views", "delete_policy": "delete-wins", "columns": [
      { "name": "space", "type": "text" },
      { "name": "title", "type": "text" },
      { "name": "config", "type": "text" }
    ]}
  ]
}"#;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PageInfo {
    pub id: String,
    pub space: String,
    pub title: String,
    pub icon: Option<String>,
    pub position: i64,
    pub system: bool,
    /// What the page is about (a project, tag or word). Widgets that opt in
    /// read it as `$page.focus` and filter their own data by it.
    pub focus: Option<String>,
    /// Optional date window shared by widgets that follow the page period.
    pub period: Option<Value>,
    pub folder_id: Option<String>,
    pub recovered: bool,
    pub archived: bool,
    pub trash_batch: Option<String>,
    pub layout_mode: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FolderInfo {
    pub id: String,
    pub space: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub position: i64,
    pub archived: bool,
    pub trash_batch: Option<String>,
    pub recovered: bool,
}

/// A widget on a page. `plugin` is `_host` for the host's own widgets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub id: String,
    pub plugin: String,
    pub widget: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    #[serde(default)]
    pub config: Map<String, Value>,
    /// This widget's own filter: `None` follows the page focus, `""` shows
    /// all items, other text filters by that project, tag or word.
    #[serde(default)]
    pub focus: Option<String>,
    /// Optional order used by flow layouts. Grid coordinates remain intact.
    #[serde(default)]
    pub flow_order: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    pub id: String,
    pub space: String,
    pub title: String,
    pub config: Map<String, Value>,
}

pub fn home_id(space: &str) -> String {
    format!("home-{space}")
}

pub struct Pages {
    store: CoreStore,
}

fn bad(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Package(message.into())
}

fn clean_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(bad("Give the page a name"));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(bad(format!(
            "Page names are limited to {MAX_TITLE} characters"
        )));
    }
    Ok(title.to_string())
}

/// Blank clears the focus.
fn clean_focus(focus: &str) -> Result<Option<String>> {
    let focus = focus.trim();
    if focus.chars().count() > MAX_FOCUS {
        return Err(bad(format!(
            "A page focus is limited to {MAX_FOCUS} characters"
        )));
    }
    if focus.chars().any(char::is_control) {
        return Err(bad("A page focus is a single line of text"));
    }
    Ok((!focus.is_empty()).then(|| focus.to_string()))
}

fn clean_id(kind: &str, id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(bad(format!("`{id}` is not a valid {kind} id")))
    }
}

fn clean_position(position: i64) -> Result<i64> {
    if (0..=MAX_POSITION).contains(&position) {
        Ok(position)
    } else {
        Err(bad(format!(
            "Position must be between 0 and {MAX_POSITION}"
        )))
    }
}

fn valid_manifest_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_view_setting_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_fields_csv(fields: &str) -> bool {
    if fields.len() > MAX_FIELDS_BYTES {
        return false;
    }
    if fields.is_empty() {
        return true;
    }
    let mut unique = BTreeSet::new();
    let keys: Vec<_> = fields.split(',').collect();
    keys.len() <= 20
        && keys.iter().all(|key| {
            !key.is_empty()
                && key.len() <= 64
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                && unique.insert(*key)
        })
}

fn valid_saved_view_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn clean_filters(value: &Value) -> Result<Value> {
    let raw = value
        .as_str()
        .ok_or_else(|| bad("View filters must be JSON text"))?;
    if raw.len() > MAX_FILTERS_BYTES {
        return Err(bad("View filters are limited to 2 KB"));
    }
    let parsed: Value =
        serde_json::from_str(raw).map_err(|_| bad("View filters must be a JSON object"))?;
    let filters = parsed
        .as_object()
        .ok_or_else(|| bad("View filters must be a JSON object"))?;
    if filters.iter().any(|(key, value)| {
        !valid_view_setting_id(key)
            || !(value.is_null() || value.is_boolean() || value.is_string() || value.is_number())
    }) {
        return Err(bad(
            "View filters need safe parameter names and scalar values",
        ));
    }
    Ok(json!(raw))
}

/// Validates a complete reusable view definition; references cannot nest.
pub fn clean_saved_view_config(config: &Map<String, Value>) -> Result<Map<String, Value>> {
    if config.contains_key("saved_view") {
        return Err(bad("A saved view cannot reference another saved view"));
    }
    let allowed = [
        "plugin", "query", "layout", "sort", "dir", "group", "date", "filters", "density", "fields",
    ];
    if config.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(bad("Saved view has an unknown setting"));
    }
    let plugin = config
        .get("plugin")
        .and_then(Value::as_str)
        .filter(|id| valid_manifest_id(id))
        .ok_or_else(|| bad("Saved view needs a valid app id"))?;
    let query = config
        .get("query")
        .and_then(Value::as_str)
        .filter(|id| valid_view_setting_id(id))
        .ok_or_else(|| bad("Saved view needs a valid data id"))?;
    let mut out = Map::new();
    out.insert("plugin".into(), json!(plugin));
    out.insert("query".into(), json!(query));
    if let Some(value) = config.get("layout") {
        let layout = value
            .as_str()
            .filter(|value| ["list", "table", "board", "calendar"].contains(value))
            .ok_or_else(|| bad("Invalid saved view layout"))?;
        out.insert("layout".into(), json!(layout));
    }
    if let Some(value) = config.get("density") {
        let density = value
            .as_str()
            .filter(|value| ["comfortable", "compact"].contains(value))
            .ok_or_else(|| bad("Invalid saved view density"))?;
        out.insert("density".into(), json!(density));
    }
    if let Some(value) = config.get("fields") {
        let fields = value
            .as_str()
            .filter(|fields| valid_fields_csv(fields))
            .ok_or_else(|| bad("Invalid saved view fields"))?;
        out.insert("fields".into(), json!(fields));
    }
    if let Some(value) = config.get("dir") {
        let dir = value
            .as_str()
            .filter(|value| ["asc", "desc"].contains(value))
            .ok_or_else(|| bad("Invalid saved view sort direction"))?;
        out.insert("dir".into(), json!(dir));
    }
    for key in ["sort", "group", "date"] {
        if let Some(value) = config.get(key) {
            let setting = value
                .as_str()
                .filter(|id| id.is_empty() || valid_view_setting_id(id))
                .ok_or_else(|| bad(format!("Invalid saved view `{key}` setting")))?;
            if !setting.is_empty() {
                out.insert(key.into(), json!(setting));
            }
        }
    }
    if let Some(value) = config.get("filters") {
        out.insert("filters".into(), clean_filters(value)?);
    }
    Ok(out)
}

fn random_id(prefix: char) -> Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|e| bad(format!("no randomness: {e}")))?;
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    Ok(std::iter::once(prefix)
        .chain(
            bytes
                .iter()
                .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char),
        )
        .collect())
}

fn int(row: &Map<String, Value>, key: &str, default: i64) -> i64 {
    row.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn truthy(row: &Map<String, Value>, key: &str) -> bool {
    row.get(key)
        .is_some_and(|v| v.as_bool().unwrap_or(v.as_i64().unwrap_or(0) != 0))
}

fn saved_view_info(row: &Map<String, Value>) -> Option<SavedView> {
    Some(SavedView {
        id: text(row, "id")?,
        space: text(row, "space")?,
        title: text(row, "title")?,
        config: text(row, "config")
            .and_then(|config| serde_json::from_str(&config).ok())
            .unwrap_or_default(),
    })
}

/// Checks a placement's ids, keeps it inside the grid and its config small.
pub fn clean_placement(mut p: Placement) -> Result<Placement> {
    clean_id("plugin", &p.plugin)?;
    clean_id("widget", &p.widget)?;
    p.w = p.w.clamp(1, GRID_COLUMNS);
    p.h = p.h.clamp(1, 12);
    p.x = p.x.clamp(0, GRID_COLUMNS - p.w);
    p.y = p.y.clamp(0, MAX_ROWS);
    if serde_json::to_string(&p.config)?.len() > MAX_CONFIG_BYTES {
        return Err(bad("Widget settings are too large"));
    }
    if let Some(focus) = &p.focus {
        // Blank is meaningful here ("all items"), so it stays blank.
        p.focus = Some(clean_focus(focus)?.unwrap_or_default());
    }
    if let Some(order) = p.flow_order.as_mut() {
        *order = (*order).clamp(0, MAX_PLACEMENTS as i64 - 1);
    }
    Ok(p)
}

/// The host's own widgets, shaped like plugin widget hints: `{id, title,
/// page: {size, config}}`. They need no plugin and their content is their
/// settings.
pub fn host_widgets() -> Value {
    json!([
        { "id": "text", "title": "Text", "page": {
            "size": { "default": [4, 2], "min": [2, 1], "max": [12, 8] },
            "config": { "text": { "type": "text", "label": "Text (Markdown)", "multiline": true, "max_length": 2000, "default": "" } }
        }},
        { "id": "link", "title": "Link", "page": {
            "size": { "default": [3, 1], "min": [2, 1], "max": [6, 3] },
            "config": {
                "label": { "type": "text", "label": "Label", "max_length": 60, "default": "" },
                "url": { "type": "text", "label": "Address (https://…)", "max_length": 500, "default": "" }
            }
        }},
        { "id": "clock", "title": "Clock", "page": {
            "size": { "default": [3, 2], "min": [2, 2], "max": [6, 4] },
            "config": {
                "seconds": { "type": "boolean", "label": "Show seconds", "default": false },
                "date": { "type": "boolean", "label": "Show the date", "default": true }
            }
        }},
        { "id": "view", "title": "View", "home": { "hidden": true }, "page": {
            "size": { "default": [6, 4], "min": [3, 3], "max": [12, 10] },
            "focus": true,
            "config": {
                "plugin": { "type": "text", "label": "App", "max_length": 63, "default": "" },
                "query": { "type": "text", "label": "Data", "max_length": 64, "default": "" },
                "saved_view": { "type": "text", "label": "Saved view", "max_length": 64, "default": "" },
                "follow_page_period": { "type": "boolean", "label": "Follow page period", "default": false },
                "filters": { "type": "text", "label": "Filters", "max_length": 2048, "default": "" },
                "layout": {
                    "type": "enum",
                    "label": "Show as",
                    "values": ["list", "table", "board", "calendar"],
                    "labels": ["List", "Table", "Board", "Calendar"],
                    "default": "list"
                },
                "density": {
                    "type": "enum",
                    "label": "Density",
                    "values": ["comfortable", "compact"],
                    "labels": ["Comfortable", "Compact"],
                    "default": "comfortable"
                },
                "fields": { "type": "text", "label": "Columns", "max_length": 1400 },
                "sort": { "type": "text", "label": "Sort by", "max_length": 64, "default": "" },
                "dir": {
                    "type": "enum",
                    "label": "Order",
                    "values": ["asc", "desc"],
                    "labels": ["Ascending", "Descending"],
                    "default": "asc"
                },
                "group": { "type": "text", "label": "Group by", "max_length": 64, "default": "" },
                "date": { "type": "text", "label": "Date", "max_length": 64, "default": "" }
            }
        }}
    ])
}

/// Settings of a host widget, checked like a plugin's; a link only keeps
/// web addresses.
pub fn clean_host_config(widget: &str, config: &Map<String, Value>) -> Result<Map<String, Value>> {
    let all = host_widgets();
    let spec = all
        .as_array()
        .and_then(|w| w.iter().find(|w| w["id"] == widget))
        .ok_or_else(|| bad(format!("`{widget}` is not a built-in widget")))?;
    let preserved_filters = if widget == "view" {
        config.get("filters").map(clean_filters).transpose()?
    } else {
        None
    };
    let mut out = clean_config(&spec["page"]["config"], config);
    if widget == "link" {
        let web = out
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|u| u.starts_with("https://") || u.starts_with("http://"));
        if !web {
            out.remove("url");
        }
    } else if widget == "view" {
        let follow_page_period = config.get("follow_page_period").and_then(Value::as_bool);
        let plugin_ok = out
            .get("plugin")
            .and_then(Value::as_str)
            .is_some_and(valid_manifest_id);
        if !plugin_ok {
            out.remove("plugin");
        }
        for key in ["query", "sort", "group", "date"] {
            let ok = out
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(valid_view_setting_id);
            if !ok {
                out.remove(key);
            }
        }
        if out
            .get("fields")
            .and_then(Value::as_str)
            .is_some_and(|fields| !valid_fields_csv(fields))
        {
            out.remove("fields");
        }
        if let Some(id) = out.get("saved_view").and_then(Value::as_str) {
            if !valid_saved_view_id(id) {
                out.remove("saved_view");
            }
        }
        if let Some(filters) = preserved_filters {
            out.insert("filters".into(), filters);
        }
        if let Some(follow) = follow_page_period {
            out.insert("follow_page_period".into(), json!(follow));
        }
    }
    Ok(out)
}

/// Keeps the settings a widget declares (`page.config` in its template)
/// and drops anything else: unknown keys, wrong types, values out of range.
pub fn clean_config(spec: &Value, config: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(fields) = spec.as_object() else {
        return out;
    };
    for (name, field) in fields {
        let Some(value) = config.get(name) else {
            continue;
        };
        let ok = match field.get("type").and_then(Value::as_str) {
            Some("enum") => value.as_str().is_some_and(|v| {
                field
                    .get("values")
                    .and_then(Value::as_array)
                    .is_some_and(|values| values.iter().any(|x| x.as_str() == Some(v)))
            }),
            Some("text") => value.as_str().is_some_and(|v| {
                let max = field
                    .get("max_length")
                    .and_then(Value::as_u64)
                    .unwrap_or(200);
                v.chars().count() as u64 <= max.min(2000)
            }),
            // A pick from options the widget's own query lists; several
            // picks are one comma-separated string so they reach queries
            // as a single parameter.
            Some("choice") => value.as_str().is_some_and(|v| {
                let multiple = field.get("multiple").and_then(Value::as_bool) == Some(true);
                let picks: Vec<&str> = if multiple {
                    v.split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .collect()
                } else {
                    vec![v.trim()]
                };
                !v.chars().any(char::is_control)
                    && (multiple || !v.contains(','))
                    && picks.len() <= 20
                    && picks.iter().all(|p| p.chars().count() <= 60)
            }),
            Some("boolean") => value.is_boolean(),
            Some("number") => value.as_f64().is_some_and(|v| {
                field
                    .get("min")
                    .and_then(Value::as_f64)
                    .is_none_or(|min| v >= min)
                    && field
                        .get("max")
                        .and_then(Value::as_f64)
                        .is_none_or(|max| v <= max)
            }),
            _ => false,
        };
        if ok {
            out.insert(name.clone(), value.clone());
        }
    }
    out
}

fn page_info(row: &Map<String, Value>) -> Option<PageInfo> {
    let period = text(row, "period").and_then(|raw| serde_json::from_str(&raw).ok());
    Some(PageInfo {
        id: text(row, "id")?,
        space: text(row, "space")?,
        title: text(row, "title").unwrap_or_else(|| "Page".into()),
        icon: text(row, "icon"),
        position: int(row, "position", 0),
        system: truthy(row, "system"),
        focus: text(row, "focus").filter(|f| !f.is_empty()),
        period,
        folder_id: text(row, "folder_id"),
        recovered: truthy(row, "recovered"),
        archived: truthy(row, "archived"),
        trash_batch: text(row, "trash_batch"),
        layout_mode: text(row, "layout_mode").unwrap_or_else(|| "grid".into()),
    })
}

fn folder_rows(store: &CoreStore) -> Result<Vec<Map<String, Value>>> {
    Ok(store.query(
        "SELECT id, space, parent_id, title, position, archived, trash_batch FROM folders WHERE _deleted IS NULL",
        &[],
    )?)
}

/// Projects parent links into a safe forest without writing repairs back to the
/// synced store. For a cycle, the edge owned by the lexicographically greatest
/// stable ID is cut, so every device makes the same projection.
fn project_folders(rows: &[Map<String, Value>]) -> Vec<FolderInfo> {
    let mut raw =
        BTreeMap::<String, (String, Option<String>, String, i64, bool, Option<String>)>::new();
    for row in rows {
        if let (Some(id), Some(space)) = (text(row, "id"), text(row, "space")) {
            raw.insert(
                id,
                (
                    space,
                    text(row, "parent_id"),
                    text(row, "title").unwrap_or_else(|| "Folder".into()),
                    int(row, "position", 0),
                    truthy(row, "archived"),
                    text(row, "trash_batch"),
                ),
            );
        }
    }
    let mut parents = BTreeMap::<String, Option<String>>::new();
    let mut recovered = BTreeSet::<String>::new();
    for (id, (space, parent, _, _, archived, _)) in &raw {
        let resolved = parent
            .as_ref()
            .filter(|parent_id| {
                raw.get(*parent_id)
                    .is_some_and(|(parent_space, _, _, _, parent_archived, _)| {
                        parent_space == space && !*parent_archived && !*archived
                    })
            })
            .cloned();
        if parent.is_some() && resolved.is_none() {
            recovered.insert(id.clone());
        }
        parents.insert(id.clone(), resolved);
    }
    for start in raw.keys() {
        let mut path = Vec::<String>::new();
        let mut seen = BTreeMap::<String, usize>::new();
        let mut current = Some(start.clone());
        while let Some(id) = current {
            if let Some(&cycle_start) = seen.get(&id) {
                if let Some(cut) = path[cycle_start..].iter().max() {
                    parents.insert(cut.clone(), None);
                    recovered.insert(cut.clone());
                }
                break;
            }
            seen.insert(id.clone(), path.len());
            path.push(id.clone());
            current = parents.get(&id).cloned().flatten();
        }
    }
    raw.into_iter()
        .map(
            |(id, (space, _, title, position, archived, trash_batch))| FolderInfo {
                parent_id: parents.remove(&id).flatten(),
                recovered: recovered.contains(&id),
                id,
                space,
                title,
                position,
                archived,
                trash_batch,
            },
        )
        .collect()
}

fn project_page_folder(page: &mut PageInfo, folders: &[FolderInfo]) {
    if let Some(id) = page.folder_id.clone() {
        match folders.iter().find(|folder| folder.id == id) {
            Some(folder) if folder.space == page.space && !folder.archived => {
                if folder.recovered {
                    page.recovered = true;
                }
            }
            _ => {
                page.folder_id = None;
                page.recovered = true;
            }
        }
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        value[0..4].parse::<i32>(),
        value[5..7].parse::<u32>(),
        value[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    (1..=days[(month - 1) as usize]).contains(&day)
}

/// Validates the persisted period shape. Intl on the UI side validates that
/// the syntactically safe timezone is a real IANA timezone.
pub fn clean_period(period: &Value) -> Result<Value> {
    let object = period
        .as_object()
        .ok_or_else(|| bad("Page period must be an object"))?;
    if object
        .keys()
        .any(|key| !["mode", "timezone", "start", "end"].contains(&key.as_str()))
    {
        return Err(bad("Page period has an unknown setting"));
    }
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .filter(|mode| ["previous_week", "this_week", "custom"].contains(mode))
        .ok_or_else(|| bad("Invalid page period mode"))?;
    let timezone = object
        .get("timezone")
        .and_then(Value::as_str)
        .filter(|tz| {
            !tz.is_empty()
                && tz.len() <= 128
                && tz.split('/').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
                })
        })
        .ok_or_else(|| bad("Invalid page period timezone"))?;
    let mut out = Map::new();
    out.insert("mode".into(), json!(mode));
    out.insert("timezone".into(), json!(timezone));
    if mode == "custom" {
        let start = object
            .get("start")
            .and_then(Value::as_str)
            .filter(|date| valid_date(date))
            .ok_or_else(|| bad("Custom page period needs a valid start date"))?;
        let end = object
            .get("end")
            .and_then(Value::as_str)
            .filter(|date| valid_date(date))
            .ok_or_else(|| bad("Custom page period needs a valid exclusive end date"))?;
        let day_number = |date: &str| {
            let year = date[0..4].parse::<i64>().unwrap();
            let month = date[5..7].parse::<i64>().unwrap();
            let day = date[8..10].parse::<i64>().unwrap();
            let year = year - i64::from(month <= 2);
            let era = year.div_euclid(400);
            let yoe = year - era * 400;
            let adjusted_month = month + if month > 2 { -3 } else { 9 };
            let doy = (153 * adjusted_month + 2) / 5 + day - 1;
            era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy
        };
        let days = day_number(end) - day_number(start);
        if !(1..=366).contains(&days) {
            return Err(bad(
                "Custom page period must be 1 to 366 days; end is exclusive",
            ));
        }
        out.insert("start".into(), json!(start));
        out.insert("end".into(), json!(end));
    } else if object.contains_key("start") || object.contains_key("end") {
        return Err(bad(
            "Only a custom page period can have start and end dates",
        ));
    }
    Ok(Value::Object(out))
}

fn period_value(period: Option<&Value>) -> Result<Option<String>> {
    period
        .map(|value| {
            if value.is_null() {
                Ok(None)
            } else {
                Ok(Some(serde_json::to_string(&clean_period(value)?)?))
            }
        })
        .transpose()
        .map(Option::flatten)
}

impl Pages {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Pages> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA)?;
        let store = CoreStore::open(
            data_dir.join(format!("{PAGES_STORE_ID}.db")),
            StoreConfig {
                plugin: PAGES_STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Pages { store })
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    /// A space's pages: Home first (when it exists), then by position.
    pub fn list(&self, space: &str) -> Result<Vec<PageInfo>> {
        let rows = self.store.query(
            "SELECT id, space, title, icon, position, system, focus, period, folder_id, archived, trash_batch, layout_mode FROM pages \
             WHERE _deleted IS NULL AND space = ?1 AND archived = 0 \
             ORDER BY system DESC, position, title COLLATE NOCASE, id",
            &[json!(space)],
        )?;
        let folders = project_folders(&folder_rows(&self.store)?);
        let mut pages: Vec<_> = rows.iter().filter_map(page_info).collect();
        for page in &mut pages {
            project_page_folder(page, &folders);
        }
        Ok(pages)
    }

    pub fn page(&self, id: &str) -> Result<Option<PageInfo>> {
        let rows = self.store.query(
            "SELECT id, space, title, icon, position, system, focus, period, folder_id, archived, trash_batch, layout_mode FROM pages \
             WHERE _deleted IS NULL AND id = ?1",
            &[json!(id)],
        )?;
        let folders = project_folders(&folder_rows(&self.store)?);
        let mut page = rows.first().and_then(page_info);
        if let Some(page) = page.as_mut() {
            project_page_folder(page, &folders);
        }
        Ok(page)
    }

    fn require(&self, id: &str) -> Result<PageInfo> {
        self.page(id)?.ok_or_else(|| bad(format!("no page `{id}`")))
    }

    fn require_active(&self, id: &str) -> Result<PageInfo> {
        let page = self.require(id)?;
        if page.archived {
            return Err(bad(format!("page `{id}` is in the trash")));
        }
        Ok(page)
    }

    pub fn folders_list(&self, space: &str, include_archived: bool) -> Result<Vec<FolderInfo>> {
        let rows = folder_rows(&self.store)?;
        let mut folders: Vec<_> = project_folders(&rows)
            .into_iter()
            .filter(|folder| folder.space == space && (include_archived || !folder.archived))
            .collect();
        folders.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
        Ok(folders)
    }

    fn folder(&self, id: &str) -> Result<FolderInfo> {
        project_folders(&folder_rows(&self.store)?)
            .into_iter()
            .find(|folder| folder.id == id)
            .ok_or_else(|| bad(format!("no folder `{id}`")))
    }

    fn validate_parent(&self, space: &str, id: Option<&str>, moving: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            let parent = self.folder(id)?;
            if parent.archived {
                return Err(bad("Cannot use an archived folder"));
            }
            if parent.space != space {
                return Err(bad("Folder belongs to another space"));
            }
            if Some(id) == moving {
                return Err(bad("A folder cannot be its own parent"));
            }
            if let Some(moving) = moving {
                let mut cursor = Some(id.to_string());
                let mut seen = BTreeSet::new();
                while let Some(current) = cursor {
                    if current == moving {
                        return Err(bad("A folder cannot move inside its descendant"));
                    }
                    if !seen.insert(current.clone()) {
                        break;
                    }
                    cursor = self.folder(&current)?.parent_id;
                }
            }
        }
        Ok(())
    }

    pub fn folder_create(
        &mut self,
        space: &str,
        title: &str,
        parent_id: Option<&str>,
    ) -> Result<FolderInfo> {
        let title = clean_title(title)?;
        self.validate_parent(space, parent_id, None)?;
        let active = self.folders_list(space, false)?;
        if active.len() >= MAX_FOLDERS {
            return Err(bad(format!("A space holds at most {MAX_FOLDERS} folders")));
        }
        let siblings = active
            .into_iter()
            .filter(|folder| folder.parent_id.as_deref() == parent_id)
            .collect::<Vec<_>>();
        let position = siblings
            .iter()
            .map(|folder| folder.position)
            .max()
            .unwrap_or(-1)
            .checked_add(1)
            .filter(|position| *position <= MAX_POSITION)
            .ok_or_else(|| bad("No available folder position"))?;
        let id = random_id('f')?;
        let mut values = Map::new();
        values.insert("space".into(), json!(space));
        values.insert("parent_id".into(), json!(parent_id));
        values.insert("title".into(), json!(title));
        values.insert("position".into(), json!(position));
        self.store
            .write(|writer| writer.upsert("folders", &id, &values))?;
        self.folder(&id)
    }

    pub fn folder_update(
        &mut self,
        id: &str,
        title: Option<&str>,
        parent_id: Option<Option<&str>>,
        position: Option<i64>,
    ) -> Result<FolderInfo> {
        let folder = self.folder(id)?;
        if folder.archived {
            return Err(bad("Archived folders cannot be edited"));
        }
        if let Some(parent_id) = parent_id {
            self.validate_parent(&folder.space, parent_id, Some(id))?;
        }
        let mut values = Map::new();
        if let Some(title) = title {
            values.insert("title".into(), json!(clean_title(title)?));
        }
        if let Some(parent_id) = parent_id {
            values.insert("parent_id".into(), json!(parent_id));
        }
        if let Some(position) = position {
            values.insert("position".into(), json!(clean_position(position)?));
        }
        if !values.is_empty() {
            self.store
                .write(|writer| writer.update("folders", id, &values))?;
        }
        self.folder(id)
    }

    pub fn folder_remove(&mut self, id: &str, subtree: bool) -> Result<()> {
        let root = self.folder(id)?;
        if root.archived {
            return Err(bad("Folder is already in the trash"));
        }
        let all = self.folders_list(&root.space, false)?;
        let batch = random_id('t')?;
        let mut folders = BTreeSet::from([id.to_string()]);
        let mut pages = Vec::new();
        if subtree {
            loop {
                let before = folders.len();
                for folder in &all {
                    if folder
                        .parent_id
                        .as_ref()
                        .is_some_and(|parent| folders.contains(parent))
                    {
                        folders.insert(folder.id.clone());
                    }
                }
                if folders.len() == before {
                    break;
                }
            }
            pages = self
                .list(&root.space)?
                .into_iter()
                .filter(|page| {
                    page.folder_id
                        .as_ref()
                        .is_some_and(|folder| folders.contains(folder))
                })
                .map(|page| page.id)
                .collect();
        }
        let direct_children: Vec<_> = if subtree {
            Vec::new()
        } else {
            all.iter()
                .filter(|folder| folder.parent_id.as_deref() == Some(id))
                .map(|folder| folder.id.clone())
                .collect()
        };
        let direct_pages: Vec<_> = if subtree {
            Vec::new()
        } else {
            self.list(&root.space)?
                .into_iter()
                .filter(|page| page.folder_id.as_deref() == Some(id))
                .map(|page| page.id)
                .collect()
        };
        self.store.write(|writer| {
            for child in &direct_children {
                let mut values = Map::new();
                values.insert("parent_id".into(), json!(root.parent_id));
                writer.update("folders", child, &values)?;
            }
            for page in &direct_pages {
                let mut values = Map::new();
                values.insert("folder_id".into(), json!(root.parent_id));
                writer.update("pages", page, &values)?;
            }
            for folder in &folders {
                let mut values = Map::new();
                values.insert("archived".into(), json!(true));
                values.insert("trash_batch".into(), json!(batch));
                writer.update("folders", folder, &values)?;
            }
            for page in &pages {
                let mut values = Map::new();
                values.insert("archived".into(), json!(true));
                values.insert("trash_batch".into(), json!(batch));
                writer.update("pages", page, &values)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn folder_restore(&mut self, id: &str) -> Result<()> {
        let folder = self.folder(id)?;
        if !folder.archived {
            return Err(bad("Folder is not in the trash"));
        }
        let batch = folder
            .trash_batch
            .ok_or_else(|| bad("Folder has no restore batch"))?;
        let rows = folder_rows(&self.store)?;
        let restore_ids: Vec<_> = rows
            .iter()
            .filter(|row| {
                text(row, "space").as_deref() == Some(&folder.space)
                    && text(row, "trash_batch").as_deref() == Some(&batch)
            })
            .filter_map(|row| text(row, "id"))
            .collect();
        let pages = self.store.query(
            "SELECT id, space, folder_id, trash_batch FROM pages WHERE _deleted IS NULL AND archived = 1",
            &[],
        )?;
        let restore_pages: Vec<_> = pages
            .iter()
            .filter(|row| {
                text(row, "space").as_deref() == Some(&folder.space)
                    && text(row, "trash_batch").as_deref() == Some(&batch)
            })
            .filter_map(|row| text(row, "id"))
            .collect();
        let restore_set: BTreeSet<_> = restore_ids.iter().cloned().collect();
        let mut projected_rows = rows.clone();
        for row in &mut projected_rows {
            if text(row, "space").as_deref() == Some(&folder.space)
                && text(row, "trash_batch").as_deref() == Some(&batch)
            {
                row.insert("archived".into(), json!(false));
            }
        }
        let projected = project_folders(&projected_rows);
        let reset_folders: BTreeSet<_> = projected
            .into_iter()
            .filter(|candidate| {
                candidate.space == folder.space
                    && restore_set.contains(&candidate.id)
                    && candidate.recovered
            })
            .map(|candidate| candidate.id)
            .collect();
        let folder_map: BTreeMap<_, _> = projected_rows
            .iter()
            .filter_map(|row| {
                Some((
                    text(row, "id")?,
                    (text(row, "space")?, truthy(row, "archived")),
                ))
            })
            .collect();
        self.store.write(|writer| {
            for folder_id in &restore_ids {
                let mut values = Map::new();
                values.insert("archived".into(), json!(false));
                values.insert("trash_batch".into(), Value::Null);
                if reset_folders.contains(folder_id) {
                    values.insert("parent_id".into(), Value::Null);
                }
                writer.update("folders", folder_id, &values)?;
            }
            for page_id in &restore_pages {
                let mut values = Map::new();
                values.insert("archived".into(), json!(false));
                values.insert("trash_batch".into(), Value::Null);
                if let Some(row) = pages
                    .iter()
                    .find(|row| text(row, "id").as_deref() == Some(page_id))
                {
                    if let Some(parent_id) = text(row, "folder_id") {
                        let valid = folder_map.get(&parent_id).is_some_and(|(space, archived)| {
                            space == &folder.space
                                && (!*archived || restore_set.contains(&parent_id))
                        });
                        if !valid {
                            values.insert("folder_id".into(), Value::Null);
                        }
                    }
                }
                writer.update("pages", page_id, &values)?;
            }
            Ok(())
        })?;
        // Parent links are retained; projection safely roots missing, archived,
        // or cross-space links on reads.
        Ok(())
    }

    pub fn page_move(
        &mut self,
        id: &str,
        folder_id: Option<Option<&str>>,
        position: Option<i64>,
    ) -> Result<PageInfo> {
        let page = self.require_active(id)?;
        if page.system {
            return Err(bad("Home cannot be moved"));
        }
        if let Some(folder_id) = folder_id {
            self.validate_parent(&page.space, folder_id, None)?;
        }
        let mut values = Map::new();
        if let Some(folder_id) = folder_id {
            values.insert("folder_id".into(), json!(folder_id));
        }
        if let Some(position) = position {
            values.insert("position".into(), json!(clean_position(position)?));
        }
        self.store
            .write(|writer| writer.update("pages", id, &values))?;
        self.require(id)
    }

    pub fn page_trash(&mut self, id: &str) -> Result<()> {
        let page = self.require_active(id)?;
        if page.system {
            return Err(bad("Home cannot be moved to the trash"));
        }
        let mut values = Map::new();
        values.insert("archived".into(), json!(true));
        values.insert("trash_batch".into(), json!(random_id('t')?));
        self.store
            .write(|writer| writer.update("pages", id, &values))?;
        Ok(())
    }

    pub fn page_restore(&mut self, id: &str) -> Result<PageInfo> {
        let page = self.require(id)?;
        if !page.archived {
            return Err(bad("Page is not in the trash"));
        }
        let mut values = Map::new();
        values.insert("archived".into(), json!(false));
        values.insert("trash_batch".into(), Value::Null);
        if page.recovered {
            values.insert("folder_id".into(), Value::Null);
        }
        self.store
            .write(|writer| writer.update("pages", id, &values))?;
        self.require(id)
    }

    pub fn pages_trash(&self, space: &str) -> Result<Vec<PageInfo>> {
        let rows = self.store.query(
            "SELECT id, space, title, icon, position, system, focus, period, folder_id, archived, trash_batch, layout_mode FROM pages WHERE _deleted IS NULL AND space = ?1 AND archived = 1 ORDER BY position, id",
            &[json!(space)],
        )?;
        let folders = project_folders(&folder_rows(&self.store)?);
        let mut pages: Vec<_> = rows.iter().filter_map(page_info).collect();
        for page in &mut pages {
            project_page_folder(page, &folders);
        }
        Ok(pages)
    }

    pub fn page_layout(&mut self, id: &str, mode: &str) -> Result<PageInfo> {
        if !matches!(mode, "grid" | "flow") {
            return Err(bad("Page layout must be `grid` or `flow`"));
        }
        self.require_active(id)?;
        let mut values = Map::new();
        values.insert("layout_mode".into(), json!(mode));
        self.store
            .write(|writer| writer.update("pages", id, &values))?;
        self.require(id)
    }

    pub fn placements(&self, page: &str) -> Result<Vec<Placement>> {
        let rows = self.store.query(
            "SELECT id, plugin, widget, x, y, w, h, config, focus, flow_order FROM placements \
             WHERE _deleted IS NULL AND page = ?1 ORDER BY y, x, id",
            &[json!(page)],
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some(Placement {
                    id: text(r, "id")?,
                    plugin: text(r, "plugin")?,
                    widget: text(r, "widget")?,
                    x: int(r, "x", 0),
                    y: int(r, "y", 0),
                    w: int(r, "w", 4),
                    h: int(r, "h", 3),
                    config: text(r, "config")
                        .and_then(|c| serde_json::from_str(&c).ok())
                        .unwrap_or_default(),
                    focus: text(r, "focus"),
                    flow_order: r.get("flow_order").and_then(Value::as_i64),
                })
            })
            .collect())
    }

    pub fn list_saved_views(&self, space: &str) -> Result<Vec<SavedView>> {
        let rows = self.store.query(
            "SELECT id, space, title, config FROM saved_views WHERE _deleted IS NULL AND space = ?1 ORDER BY title COLLATE NOCASE, id",
            &[json!(space)],
        )?;
        Ok(rows.iter().filter_map(saved_view_info).collect())
    }

    pub fn get_saved_view(&self, id: &str) -> Result<Option<SavedView>> {
        let rows = self.store.query(
            "SELECT id, space, title, config FROM saved_views WHERE _deleted IS NULL AND id = ?1",
            &[json!(id)],
        )?;
        Ok(rows.first().and_then(saved_view_info))
    }

    pub fn save_view(
        &mut self,
        space: &str,
        id: Option<&str>,
        title: &str,
        config: Map<String, Value>,
    ) -> Result<SavedView> {
        let title = clean_title(title)?;
        let config = clean_saved_view_config(&config)?;
        let updating = id.is_some();
        let id = match id {
            Some(id) => {
                if !valid_saved_view_id(id) {
                    return Err(bad(format!("`{id}` is not a valid saved view id")));
                }
                let existing = self
                    .get_saved_view(id)?
                    .ok_or_else(|| bad(format!("no saved view `{id}`")))?;
                if existing.space != space {
                    return Err(bad("Saved view belongs to another space"));
                }
                id.to_string()
            }
            None => random_id('v')?,
        };
        let mut values = Map::new();
        values.insert("space".into(), json!(space));
        values.insert("title".into(), json!(title));
        values.insert("config".into(), json!(serde_json::to_string(&config)?));
        self.store.write(|w| {
            if updating {
                w.update("saved_views", &id, &values)
            } else {
                w.upsert("saved_views", &id, &values)
            }
        })?;
        self.get_saved_view(&id)?
            .ok_or_else(|| bad("Saved view could not be read after saving"))
    }

    /// Removes a saved definition only. Placements referencing its id remain intact.
    pub fn remove_saved_view(&mut self, id: &str) -> Result<()> {
        if self.get_saved_view(id)?.is_none() {
            return Err(bad(format!("no saved view `{id}`")));
        }
        self.store.write(|w| w.delete("saved_views", id))?;
        Ok(())
    }

    pub fn create(&mut self, space: &str, title: &str, icon: Option<&str>) -> Result<PageInfo> {
        self.create_with_period(space, title, icon, None)
    }

    pub fn create_with_period(
        &mut self,
        space: &str,
        title: &str,
        icon: Option<&str>,
        period: Option<&Value>,
    ) -> Result<PageInfo> {
        let title = clean_title(title)?;
        let period = period_value(period)?;
        let pages = self.list(space)?;
        if pages.len() >= MAX_PAGES {
            return Err(bad(format!("A space holds at most {MAX_PAGES} pages")));
        }
        let position = pages
            .iter()
            .map(|p| p.position)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|position| *position <= MAX_POSITION)
            .ok_or_else(|| bad("No available page position"))?;
        let id = random_id('p')?;
        let mut values = Map::new();
        values.insert("space".into(), json!(space));
        values.insert("title".into(), json!(title));
        values.insert("icon".into(), json!(icon));
        values.insert("position".into(), json!(position));
        values.insert("system".into(), json!(false));
        values.insert("period".into(), json!(period));
        self.store.write(|w| w.upsert("pages", &id, &values))?;
        self.require(&id)
    }

    /// Creates the space's Home page with `seed` placements the first time;
    /// afterwards returns the existing one unchanged.
    pub fn ensure_home(&mut self, space: &str, seed: Vec<Placement>) -> Result<PageInfo> {
        let id = home_id(space);
        if let Some(page) = self.page(&id)? {
            return Ok(page);
        }
        let mut values = Map::new();
        values.insert("space".into(), json!(space));
        values.insert("title".into(), json!("Home"));
        values.insert("position".into(), json!(0));
        values.insert("system".into(), json!(true));
        let seed = seed
            .into_iter()
            .take(MAX_PLACEMENTS)
            .map(clean_placement)
            .collect::<Result<Vec<_>>>()?;
        let rows = seed
            .iter()
            .map(|p| Ok((random_id('w')?, placement_values(&id, p)?)))
            .collect::<Result<Vec<_>>>()?;
        self.store.write(|w| {
            w.upsert("pages", &id, &values)?;
            for (row, p) in &rows {
                w.upsert("placements", row, p)?;
            }
            Ok(())
        })?;
        self.require(&id)
    }

    pub fn update(
        &mut self,
        id: &str,
        title: Option<&str>,
        icon: Option<Option<&str>>,
        position: Option<i64>,
        focus: Option<&str>,
    ) -> Result<PageInfo> {
        self.update_with_period(id, title, icon, position, focus, None)
    }

    pub fn update_with_period(
        &mut self,
        id: &str,
        title: Option<&str>,
        icon: Option<Option<&str>>,
        position: Option<i64>,
        focus: Option<&str>,
        period: Option<&Value>,
    ) -> Result<PageInfo> {
        self.require_active(id)?;
        let mut values = Map::new();
        if let Some(title) = title {
            values.insert("title".into(), json!(clean_title(title)?));
        }
        if let Some(icon) = icon {
            values.insert("icon".into(), json!(icon));
        }
        if let Some(position) = position {
            values.insert("position".into(), json!(clean_position(position)?));
        }
        if let Some(focus) = focus {
            values.insert("focus".into(), json!(clean_focus(focus)?));
        }
        if let Some(period) = period {
            values.insert("period".into(), json!(period_value(Some(period))?));
        }
        if !values.is_empty() {
            self.store.write(|w| w.update("pages", id, &values))?;
        }
        self.require(id)
    }

    /// Removes a page and its placements on every device. Home stays.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.require_active(id)?.system {
            return Err(bad("Home cannot be removed; rename it instead"));
        }
        let placements = self.placements(id)?;
        self.store.write(|w| {
            for p in &placements {
                w.delete("placements", &p.id)?;
            }
            w.delete("pages", id)
        })?;
        Ok(())
    }

    /// Replaces a page's layout: placements with a known id are updated,
    /// those without one are added, and the rest are removed. Only changed
    /// columns are written so concurrent edits elsewhere survive.
    pub fn save(&mut self, page: &str, placements: Vec<Placement>) -> Result<Vec<Placement>> {
        self.require_active(page)?;
        if placements.len() > MAX_PLACEMENTS {
            return Err(bad(format!(
                "A page holds at most {MAX_PLACEMENTS} widgets"
            )));
        }
        let current = self.placements(page)?;
        let mut keep = BTreeSet::new();
        let mut writes = Vec::new();
        for p in placements {
            let p = clean_placement(p)?;
            match current.iter().find(|c| !p.id.is_empty() && c.id == p.id) {
                Some(old) => {
                    keep.insert(old.id.clone());
                    let mut changed = Map::new();
                    for (key, new, before) in [
                        ("x", p.x, old.x),
                        ("y", p.y, old.y),
                        ("w", p.w, old.w),
                        ("h", p.h, old.h),
                    ] {
                        if new != before {
                            changed.insert(key.into(), json!(new));
                        }
                    }
                    if p.flow_order != old.flow_order {
                        changed.insert("flow_order".into(), json!(p.flow_order));
                    }
                    if p.focus != old.focus {
                        changed.insert("focus".into(), json!(p.focus));
                    }
                    if p.config != old.config {
                        changed.insert("config".into(), json!(serde_json::to_string(&p.config)?));
                    }
                    if !changed.is_empty() {
                        writes.push((old.id.clone(), changed, false));
                    }
                }
                None => writes.push((random_id('w')?, placement_values(page, &p)?, true)),
            }
        }
        let removed: Vec<String> = current
            .iter()
            .filter(|c| !keep.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        self.store.write(|w| {
            for id in &removed {
                w.delete("placements", id)?;
            }
            for (id, values, new) in &writes {
                if *new {
                    w.upsert("placements", id, values)?;
                } else {
                    w.update("placements", id, values)?;
                }
            }
            Ok(())
        })?;
        self.placements(page)
    }
}

fn placement_values(page: &str, p: &Placement) -> Result<Map<String, Value>> {
    let mut values = Map::new();
    values.insert("page".into(), json!(page));
    values.insert("plugin".into(), json!(p.plugin));
    values.insert("widget".into(), json!(p.widget));
    values.insert("x".into(), json!(p.x));
    values.insert("y".into(), json!(p.y));
    values.insert("w".into(), json!(p.w));
    values.insert("h".into(), json!(p.h));
    values.insert("config".into(), json!(serde_json::to_string(&p.config)?));
    values.insert("focus".into(), json!(p.focus));
    values.insert("flow_order".into(), json!(p.flow_order));
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(dir: &tempfile::TempDir, device: &str) -> Pages {
        Pages::open(
            dir.path(),
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: device.into(),
                user: "owner".into(),
            },
        )
        .unwrap()
    }

    fn widget(plugin: &str, widget: &str, x: i64, y: i64) -> Placement {
        Placement {
            id: String::new(),
            plugin: plugin.into(),
            widget: widget.into(),
            x,
            y,
            w: 4,
            h: 3,
            config: Map::new(),
            focus: None,
            flow_order: None,
        }
    }

    #[test]
    fn host_widgets_check_their_settings() {
        let link = json!({ "label": "Docs", "url": "javascript:alert(1)" });
        assert_eq!(
            Value::Object(clean_host_config("link", link.as_object().unwrap()).unwrap()),
            json!({ "label": "Docs" })
        );
        let link = json!({ "url": "https://example.com/a" });
        assert_eq!(
            clean_host_config("link", link.as_object().unwrap()).unwrap()["url"],
            "https://example.com/a"
        );
        let text = json!({ "text": "x".repeat(2001) });
        assert!(clean_host_config("text", text.as_object().unwrap())
            .unwrap()
            .is_empty());
        assert!(clean_host_config("iframe", &Map::new()).is_err());
    }

    #[test]
    fn view_widget_keeps_only_safe_settings() {
        let config = json!({
            "plugin": "todo",
            "query": "tasks",
            "layout": "board",
            "sort": "due",
            "dir": "desc",
            "group": "board_lane",
            "date": "due",
            "extra": 1
        });
        assert_eq!(
            Value::Object(clean_host_config("view", config.as_object().unwrap()).unwrap()),
            json!({
                "plugin": "todo",
                "query": "tasks",
                "layout": "board",
                "sort": "due",
                "dir": "desc",
                "group": "board_lane",
                "date": "due"
            })
        );
        let unsafe_settings = json!({
            "plugin": "../x",
            "query": "t; drop",
            "sort": "a b",
            "layout": "grid"
        });
        assert_eq!(
            clean_host_config("view", unsafe_settings.as_object().unwrap()).unwrap(),
            Map::new()
        );
    }

    fn named_config(filters: &str) -> Map<String, Value> {
        json!({"plugin":"todo", "query":"tasks", "layout":"list", "filters":filters})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn saved_views_round_trip_update_and_leave_shared_placements_on_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "device-a");
        let saved = pages
            .save_view(
                "personal",
                None,
                "Open tasks",
                named_config(r#"{"status":"open"}"#),
            )
            .unwrap();
        assert_eq!(
            pages.get_saved_view(&saved.id).unwrap(),
            Some(saved.clone())
        );
        assert_eq!(
            pages.list_saved_views("personal").unwrap(),
            vec![saved.clone()]
        );
        let home = pages.ensure_home("personal", vec![]).unwrap();
        let mut placement = widget(HOST_WIDGETS, "view", 0, 0);
        placement.config = json!({"saved_view": saved.id}).as_object().unwrap().clone();
        let placement = pages.save(&home.id, vec![placement]).unwrap().remove(0);
        let updated = pages
            .save_view(
                "personal",
                Some(&saved.id),
                "Urgent",
                named_config(r#"{"status":"urgent"}"#),
            )
            .unwrap();
        assert_eq!(updated.id, saved.id);
        assert_eq!(updated.title, "Urgent");
        assert_eq!(pages.placements(&home.id).unwrap()[0], placement);
        pages.remove_saved_view(&saved.id).unwrap();
        assert!(pages.get_saved_view(&saved.id).unwrap().is_none());
        assert_eq!(
            pages.placements(&home.id).unwrap()[0].config["saved_view"],
            saved.id
        );
    }

    #[test]
    fn saved_view_configs_accept_empty_optional_fields_and_round_trip_builder_shape() {
        let config = json!({"plugin":"todo", "query":"tasks", "layout":"list", "sort":"", "dir":"asc", "group":"", "date":"", "filters":"{}"});
        let clean = clean_saved_view_config(config.as_object().unwrap()).unwrap();
        assert!(!clean.contains_key("sort"));
        assert!(!clean.contains_key("group"));
        assert!(!clean.contains_key("date"));
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "device-a");
        let saved = pages
            .save_view("personal", None, "Default tasks", clean)
            .unwrap();
        assert_eq!(pages.get_saved_view(&saved.id).unwrap(), Some(saved));
    }

    #[test]
    fn saved_view_fields_and_density_round_trip_and_validate() {
        let config = json!({
            "plugin": "todo",
            "query": "tasks",
            "density": "compact",
            "fields": "title,due_date,board_lane"
        });
        let clean = clean_saved_view_config(config.as_object().unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "device-a");
        let saved = pages
            .save_view("personal", None, "Compact tasks", clean)
            .unwrap();
        assert_eq!(
            pages.get_saved_view(&saved.id).unwrap(),
            Some(saved.clone())
        );
        assert_eq!(saved.config["density"], "compact");
        assert_eq!(saved.config["fields"], "title,due_date,board_lane");

        let title_only = json!({"plugin":"todo", "query":"tasks", "fields":""});
        assert_eq!(
            clean_saved_view_config(title_only.as_object().unwrap()).unwrap()["fields"],
            ""
        );

        let invalid_fields = vec![
            "bad-key".to_string(),
            "a".repeat(65),
            "title,title".to_string(),
            "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r,s,t,u".to_string(),
            "a,".repeat(701),
        ];
        for fields in invalid_fields {
            let invalid = json!({"plugin":"todo", "query":"tasks", "fields":fields});
            assert!(
                clean_saved_view_config(invalid.as_object().unwrap()).is_err(),
                "accepted fields {}",
                invalid["fields"]
            );
        }
        let invalid_density = json!({"plugin":"todo", "query":"tasks", "density":"dense"});
        assert!(clean_saved_view_config(invalid_density.as_object().unwrap()).is_err());

        let widget = json!({"density":"compact", "fields":"title,due_date"});
        assert_eq!(
            clean_host_config("view", widget.as_object().unwrap()).unwrap(),
            widget.as_object().unwrap().clone()
        );
        let unsafe_widget = json!({"fields":"title,title", "density":"dense"});
        assert_eq!(
            clean_host_config("view", unsafe_widget.as_object().unwrap()).unwrap(),
            Map::new()
        );
    }

    #[test]
    fn saved_view_configs_reject_bad_filters_and_nested_references() {
        for invalid in ["[]", r#"{"bad-key":"x"}"#, r#"{"status":{"nested":true}}"#] {
            assert!(
                clean_saved_view_config(&named_config(invalid)).is_err(),
                "accepted {invalid}"
            );
        }
        assert!(clean_saved_view_config(&named_config(&format!(
            "{{\"q\":\"{}\"}}",
            "x".repeat(2100)
        )))
        .is_err());
        let mut recursive = named_config("{}");
        recursive.insert("saved_view".into(), json!("vabc"));
        assert!(clean_saved_view_config(&recursive).is_err());
        assert!(
            clean_host_config("view", json!({"filters":"not-json"}).as_object().unwrap()).is_err()
        );
    }

    #[test]
    fn saved_view_update_cannot_cross_spaces_and_legacy_view_config_survives() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "device-a");
        let saved = pages
            .save_view("personal", None, "Tasks", named_config("{}"))
            .unwrap();
        assert!(pages
            .save_view("work", Some(&saved.id), "Hijack", named_config("{}"))
            .is_err());
        assert_eq!(
            pages.get_saved_view(&saved.id).unwrap().unwrap().space,
            "personal"
        );
        let legacy = json!({"plugin":"todo", "query":"tasks", "layout":"board", "sort":"due", "dir":"desc", "group":"lane", "date":"due"});
        assert_eq!(
            clean_host_config("view", legacy.as_object().unwrap()).unwrap(),
            legacy.as_object().unwrap().clone()
        );
        let reference = json!({"saved_view":"vabc123"});
        assert_eq!(
            clean_host_config("view", reference.as_object().unwrap()).unwrap(),
            reference.as_object().unwrap().clone()
        );
        let filters = format!("{{\"q\":\"{}\"}}", "x".repeat(2030));
        let config = json!({"filters":filters.clone()});
        assert_eq!(
            clean_host_config("view", config.as_object().unwrap()).unwrap()["filters"],
            filters
        );
    }

    #[test]
    fn a_version_three_store_gains_the_saved_views_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut old_schema: Value = serde_json::from_str(SCHEMA).unwrap();
        old_schema["version"] = json!(3);
        old_schema["tables"]
            .as_array_mut()
            .unwrap()
            .retain(|table| table["name"] != "saved_views");
        let old_schema: PluginSchema = serde_json::from_value(old_schema).unwrap();
        let old = CoreStore::open(
            dir.path().join(format!("{PAGES_STORE_ID}.db")),
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: "a".into(),
                user: "owner".into(),
            },
            old_schema,
            Box::new(SystemTime),
        )
        .unwrap();
        drop(old);
        let mut pages = open(&dir, "a");
        let saved = pages
            .save_view("personal", None, "Tasks", named_config("{}"))
            .unwrap();
        assert_eq!(pages.get_saved_view(&saved.id).unwrap(), Some(saved));
    }

    #[test]
    fn a_version_four_page_without_period_migrates_and_can_be_updated() {
        let dir = tempfile::tempdir().unwrap();
        let mut old_schema: Value = serde_json::from_str(SCHEMA).unwrap();
        old_schema["version"] = json!(4);
        let tables = old_schema["tables"].as_array_mut().unwrap();
        tables.retain(|table| table["name"] != "folders");
        for table in tables {
            if let Some(columns) = table["columns"].as_array_mut() {
                columns.retain(|column| {
                    !matches!(
                        column["name"].as_str(),
                        Some(
                            "folder_id"
                                | "archived"
                                | "trash_batch"
                                | "layout_mode"
                                | "flow_order"
                                | "period"
                        )
                    )
                });
            }
        }
        let old_schema: PluginSchema = serde_json::from_value(old_schema).unwrap();
        let mut old = CoreStore::open(
            dir.path().join(format!("{PAGES_STORE_ID}.db")),
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: "a".into(),
                user: "owner".into(),
            },
            old_schema,
            Box::new(SystemTime),
        )
        .unwrap();
        let mut values = Map::new();
        values.insert("space".into(), json!("personal"));
        values.insert("title".into(), json!("Legacy"));
        values.insert("system".into(), json!(false));
        old.write(|writer| writer.upsert("pages", "plegacy", &values))
            .unwrap();
        drop(old);

        let mut pages = open(&dir, "a");
        assert_eq!(pages.page("plegacy").unwrap().unwrap().period, None);
        let period = json!({ "mode": "custom", "timezone": "Asia/Kolkata", "start": "2024-02-28", "end": "2024-03-01" });
        assert_eq!(
            pages
                .update_with_period("plegacy", None, None, None, None, Some(&period))
                .unwrap()
                .period,
            Some(period.clone())
        );
        assert_eq!(
            pages
                .update_with_period("plegacy", None, None, None, None, Some(&Value::Null))
                .unwrap()
                .period,
            None
        );
    }

    #[test]
    fn version_five_pages_migrate_additively_to_folders_trash_and_flow_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut old_schema: Value = serde_json::from_str(SCHEMA).unwrap();
        old_schema["version"] = json!(5);
        let tables = old_schema["tables"].as_array_mut().unwrap();
        tables.retain(|table| table["name"] != "folders");
        for table in tables {
            let columns = table["columns"].as_array_mut().unwrap();
            columns.retain(|column| {
                !matches!(
                    column["name"].as_str(),
                    Some("folder_id" | "archived" | "trash_batch" | "layout_mode" | "flow_order")
                )
            });
        }
        let old_schema: PluginSchema = serde_json::from_value(old_schema).unwrap();
        let path = dir.path().join(format!("{PAGES_STORE_ID}.db"));
        let mut old = CoreStore::open(
            path,
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: "old".into(),
                user: "owner".into(),
            },
            old_schema,
            Box::new(SystemTime),
        )
        .unwrap();
        old.write(|writer| {
            let mut page = Map::new();
            page.insert("space".into(), json!("personal"));
            page.insert("title".into(), json!("Legacy"));
            writer.upsert("pages", "plegacy", &page)?;
            let mut placement = Map::new();
            placement.insert("page".into(), json!("plegacy"));
            placement.insert("plugin".into(), json!("todo"));
            placement.insert("widget".into(), json!("today"));
            writer.upsert("placements", "wlegacy", &placement)
        })
        .unwrap();
        drop(old);

        let mut pages = open(&dir, "new");
        let page = pages.page("plegacy").unwrap().unwrap();
        assert_eq!(page.layout_mode, "grid");
        assert_eq!(page.folder_id, None);
        assert!(!page.archived);
        assert!(pages.placements("plegacy").unwrap()[0].flow_order.is_none());
        let folder = pages.folder_create("personal", "Migrated", None).unwrap();
        assert_eq!(folder.space, "personal");
    }

    #[test]
    fn version_five_clients_buffer_version_six_folder_changes_until_upgrade() {
        let old_dir = tempfile::tempdir().unwrap();
        let new_dir = tempfile::tempdir().unwrap();
        let mut old_schema: Value = serde_json::from_str(SCHEMA).unwrap();
        old_schema["version"] = json!(5);
        let tables = old_schema["tables"].as_array_mut().unwrap();
        tables.retain(|table| table["name"] != "folders");
        for table in tables {
            if let Some(columns) = table["columns"].as_array_mut() {
                columns.retain(|column| {
                    !matches!(
                        column["name"].as_str(),
                        Some(
                            "folder_id" | "archived" | "trash_batch" | "layout_mode" | "flow_order"
                        )
                    )
                });
            }
        }
        let old_schema: PluginSchema = serde_json::from_value(old_schema).unwrap();
        let mut old = CoreStore::open(
            old_dir.path().join(format!("{PAGES_STORE_ID}.db")),
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: "old".into(),
                user: "owner".into(),
            },
            old_schema,
            Box::new(SystemTime),
        )
        .unwrap();
        let mut new = open(&new_dir, "new");
        new.folder_create("personal", "New folder", None).unwrap();
        let (batch, _) = new.store().export_since(0).unwrap();
        let report = old.import(&batch).unwrap();
        assert_eq!(report.pending, batch.changes.len());
        assert_eq!(report.applied, 0);
    }

    #[test]
    fn period_config_checks_real_dates_modes_and_exclusive_span() {
        let good = json!({ "mode": "custom", "timezone": "Asia/Kolkata", "start": "2024-02-28", "end": "2024-03-01" });
        assert_eq!(clean_period(&good).unwrap(), good);
        for invalid in [
            json!({ "mode": "next_week", "timezone": "UTC" }),
            json!({ "mode": "custom", "timezone": "UTC", "start": "2023-02-29", "end": "2023-03-01" }),
            json!({ "mode": "custom", "timezone": "UTC", "start": "2024-03-02", "end": "2024-03-01" }),
            json!({ "mode": "custom", "timezone": "UTC", "start": "2023-01-01", "end": "2024-01-03" }),
            json!({ "mode": "this_week", "timezone": "UTC", "start": "2024-01-01" }),
            json!({ "mode": "this_week", "timezone": "Bad Zone" }),
        ] {
            assert!(clean_period(&invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn invalid_period_creation_writes_no_orphan_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let invalid = json!({ "mode": "custom", "timezone": "UTC", "start": "2024-02-30", "end": "2024-03-01" });
        assert!(pages
            .create_with_period("personal", "Bad", None, Some(&invalid))
            .is_err());
        assert!(pages.list("personal").unwrap().is_empty());
    }

    #[test]
    fn view_placement_keeps_its_period_binding_and_saved_view_rejects_it() {
        let config = json!({ "plugin": "todo", "query": "tasks", "follow_page_period": true });
        assert_eq!(
            clean_host_config("view", config.as_object().unwrap()).unwrap(),
            config.as_object().unwrap().clone()
        );
        let mut saved = config.as_object().unwrap().clone();
        assert!(clean_saved_view_config(&saved).is_err());
        saved.remove("follow_page_period");
        assert!(clean_saved_view_config(&saved).is_ok());
    }

    #[test]
    fn host_widgets_list_the_view_widget_hidden_from_home() {
        let all = host_widgets().as_array().cloned().unwrap_or_default();
        let view = all.iter().find(|widget| widget["id"] == "view").unwrap();
        assert_eq!(view["home"]["hidden"], json!(true));
        assert_eq!(view["page"]["focus"], json!(true));
    }

    #[test]
    fn config_keeps_only_declared_valid_settings() {
        let spec = json!({
            "filter": { "type": "enum", "values": ["today", "upcoming"] },
            "label": { "type": "text", "max_length": 5 },
            "compact": { "type": "boolean" },
            "limit": { "type": "number", "min": 1, "max": 20 },
            "odd": { "type": "colour" }
        });
        let given = json!({
            "filter": "upcoming", "label": "Work", "compact": true, "limit": 8,
            "odd": "red", "extra": 1
        });
        let kept = clean_config(&spec, given.as_object().unwrap());
        assert_eq!(
            Value::Object(kept),
            json!({ "filter": "upcoming", "label": "Work", "compact": true, "limit": 8 })
        );
        let choices = json!({
            "project": { "type": "choice", "suggest": "projects" },
            "tags": { "type": "choice", "multiple": true, "suggest": "tags" }
        });
        let kept = clean_config(
            &choices,
            json!({ "project": "Work", "tags": "a, b c, d" })
                .as_object()
                .unwrap(),
        );
        assert_eq!(
            Value::Object(kept),
            json!({ "project": "Work", "tags": "a, b c, d" })
        );
        let many = (0..21)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        for wrong in [
            json!({ "project": "a, b" }),
            json!({ "project": "x".repeat(61) }),
            json!({ "tags": many }),
            json!({ "tags": "a\nb" }),
            json!({ "tags": ["a"] }),
        ] {
            assert!(
                clean_config(&choices, wrong.as_object().unwrap()).is_empty(),
                "{wrong}"
            );
        }
        let bad = json!({ "filter": "never", "label": "Too long", "compact": "yes", "limit": 99 });
        assert!(clean_config(&spec, bad.as_object().unwrap()).is_empty());
        assert!(clean_config(&Value::Null, given.as_object().unwrap()).is_empty());
    }

    #[test]
    fn home_is_created_once_and_cannot_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let home = pages
            .ensure_home("personal", vec![widget("todo", "today", 0, 0)])
            .unwrap();
        assert_eq!(home.id, "home-personal");
        assert!(home.system);
        let again = pages
            .ensure_home("personal", vec![widget("notes", "capture", 0, 0)])
            .unwrap();
        assert_eq!(again, home);
        assert_eq!(pages.placements(&home.id).unwrap().len(), 1);
        assert!(pages.remove(&home.id).is_err());
        pages
            .update(&home.id, Some("Start"), None, None, None)
            .unwrap();
        assert_eq!(pages.list("personal").unwrap()[0].title, "Start");
    }

    #[test]
    fn pages_are_per_space_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        pages.ensure_home("personal", vec![]).unwrap();
        let focus = pages.create("personal", " Focus ", Some("star")).unwrap();
        let review = pages.create("personal", "Review", None).unwrap();
        pages.create("work", "Sprint", None).unwrap();
        assert_eq!(focus.title, "Focus");
        let titles: Vec<_> = pages
            .list("personal")
            .unwrap()
            .into_iter()
            .map(|p| p.title)
            .collect();
        assert_eq!(titles, ["Home", "Focus", "Review"]);
        pages.update(&review.id, None, None, Some(0), None).unwrap();
        assert_eq!(pages.list("personal").unwrap()[1].id, review.id);
        assert_eq!(pages.list("work").unwrap().len(), 1);
        assert!(pages.create("personal", "  ", None).is_err());
        assert!(pages.create("personal", &"x".repeat(41), None).is_err());
    }

    #[test]
    fn focus_is_trimmed_cleared_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let page = pages.create("personal", "Launch", None).unwrap();
        assert_eq!(page.focus, None);
        let page = pages
            .update(&page.id, None, None, None, Some("  Project X "))
            .unwrap();
        assert_eq!(page.focus.as_deref(), Some("Project X"));
        let page = pages
            .update(&page.id, Some("Launch!"), None, None, None)
            .unwrap();
        assert_eq!(page.focus.as_deref(), Some("Project X"));
        assert!(pages
            .update(&page.id, None, None, None, Some(&"x".repeat(41)))
            .is_err());
        assert!(pages
            .update(&page.id, None, None, None, Some("a\nb"))
            .is_err());
        let page = pages
            .update(&page.id, None, None, None, Some("  "))
            .unwrap();
        assert_eq!(page.focus, None);
    }

    #[test]
    fn a_version_one_store_gains_the_focus_column() {
        let dir = tempfile::tempdir().unwrap();
        let mut v1: Value = serde_json::from_str(SCHEMA).unwrap();
        v1["version"] = json!(1);
        let tables = v1["tables"].as_array_mut().unwrap();
        tables.retain(|table| !matches!(table["name"].as_str(), Some("folders" | "saved_views")));
        for table in tables {
            if let Some(columns) = table["columns"].as_array_mut() {
                columns.retain(|column| {
                    !matches!(
                        column["name"].as_str(),
                        Some(
                            "focus"
                                | "period"
                                | "folder_id"
                                | "archived"
                                | "trash_batch"
                                | "layout_mode"
                                | "flow_order"
                        )
                    )
                });
            }
        }
        let v1: PluginSchema = serde_json::from_value(v1).unwrap();
        assert_eq!(v1.version, 1);
        assert!(v1.tables[0].columns.iter().all(|c| c.name != "focus"));
        let mut old = CoreStore::open(
            dir.path().join(format!("{PAGES_STORE_ID}.db")),
            StoreConfig {
                space: "personal".into(),
                plugin: PAGES_STORE_ID.into(),
                device: "a".into(),
                user: "owner".into(),
            },
            v1,
            Box::new(SystemTime),
        )
        .unwrap();
        let mut values = Map::new();
        values.insert("space".into(), json!("personal"));
        values.insert("title".into(), json!("Old"));
        values.insert("system".into(), json!(false));
        old.write(|w| w.upsert("pages", "pold", &values)).unwrap();
        drop(old);
        let mut pages = open(&dir, "a");
        assert_eq!(pages.page("pold").unwrap().unwrap().focus, None);
        let page = pages
            .update("pold", None, None, None, Some("Work"))
            .unwrap();
        assert_eq!(page.focus.as_deref(), Some("Work"));
        let mut own = widget("todo", "today", 0, 0);
        own.focus = Some("tag1".into());
        let saved = pages.save("pold", vec![own]).unwrap();
        assert_eq!(saved[0].focus.as_deref(), Some("tag1"));
    }

    #[test]
    fn save_adds_updates_and_removes_placements() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let page = pages.create("personal", "Focus", None).unwrap();
        let saved = pages
            .save(
                &page.id,
                vec![widget("todo", "today", 0, 0), widget("todo", "today", 4, 0)],
            )
            .unwrap();
        assert_eq!(saved.len(), 2, "the same widget can be placed twice");
        let mut first = saved[0].clone();
        first.x = 30;
        first.w = 6;
        first.config.insert("filter".into(), json!("upcoming"));
        let saved = pages.save(&page.id, vec![first.clone()]).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (saved[0].id.as_str(), saved[0].x, saved[0].w),
            (first.id.as_str(), 6, 6)
        );
        assert_eq!(saved[0].config["filter"], "upcoming");
        pages.remove(&page.id).unwrap();
        assert!(pages.page(&page.id).unwrap().is_none());
        assert!(pages.placements(&page.id).unwrap().is_empty());
    }

    #[test]
    fn a_widget_filter_follows_the_page_shows_all_or_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let page = pages.create("personal", "Mixed", None).unwrap();
        let mut all = widget("todo", "today", 4, 0);
        all.focus = Some("  ".into());
        let mut own = widget("todo", "today", 8, 0);
        own.focus = Some(" Project 1 ".into());
        let saved = pages
            .save(&page.id, vec![widget("todo", "today", 0, 0), all, own])
            .unwrap();
        let focus: Vec<_> = saved.iter().map(|p| p.focus.clone()).collect();
        assert_eq!(focus, [None, Some(String::new()), Some("Project 1".into())]);

        // Changing only the filter is saved; clearing it follows the page again.
        let mut next = saved.clone();
        next[0].focus = Some("tag1".into());
        next[2].focus = None;
        let saved = pages.save(&page.id, next).unwrap();
        let focus: Vec<_> = saved.iter().map(|p| p.focus.clone()).collect();
        assert_eq!(focus, [Some("tag1".into()), Some(String::new()), None]);

        let mut long = widget("todo", "today", 0, 4);
        long.focus = Some("x".repeat(41));
        assert!(pages.save(&page.id, vec![long]).is_err());
    }

    #[test]
    fn invalid_placements_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut pages = open(&dir, "a");
        let page = pages.create("personal", "Focus", None).unwrap();
        assert!(pages
            .save(&page.id, vec![widget("../x", "today", 0, 0)])
            .is_err());
        let mut big = widget("todo", "today", 0, 0);
        big.config.insert("text".into(), json!("x".repeat(5000)));
        assert!(pages.save(&page.id, vec![big]).is_err());
        let many = (0..41).map(|i| widget("todo", "today", 0, i)).collect();
        assert!(pages.save(&page.id, many).is_err());
        assert!(pages.save("missing", vec![]).is_err());
    }

    #[test]
    fn edits_to_different_widgets_merge_between_devices() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut a = open(&dir_a, "a");
        let mut b = open(&dir_b, "b");
        let home = a
            .ensure_home(
                "personal",
                vec![
                    widget("todo", "today", 0, 0),
                    widget("notes", "capture", 4, 0),
                ],
            )
            .unwrap();
        let (batch, _) = a.store().export_since(0).unwrap();
        b.store_mut().import(&batch).unwrap();
        let shared = a.placements(&home.id).unwrap();
        let (mut on_a, mut on_b) = (shared.clone(), shared.clone());
        on_a[0].y = 5;
        on_b[1].w = 8;
        on_b[1].x = 0;
        let (seen_a, seen_b) = (
            a.store().export_since(0).unwrap().1,
            b.store().export_since(0).unwrap().1,
        );
        a.save(&home.id, on_a).unwrap();
        b.save(&home.id, on_b).unwrap();
        let (from_a, _) = a.store().export_since(seen_a).unwrap();
        let (from_b, _) = b.store().export_since(seen_b).unwrap();
        a.store_mut().import(&from_b).unwrap();
        b.store_mut().import(&from_a).unwrap();
        let merged = a.placements(&home.id).unwrap();
        assert_eq!(merged, b.placements(&home.id).unwrap());
        let todo = merged.iter().find(|p| p.plugin == "todo").unwrap();
        let notes = merged.iter().find(|p| p.plugin == "notes").unwrap();
        assert_eq!((todo.y, notes.w, notes.x), (5, 8, 0));
    }
}
