//! User pages: per-space pages holding a grid of widget placements. The
//! host owns them (plugins never read or change them) and keeps them in a
//! host-owned store, `_pages`, so a page arranged on one device appears on
//! the others.
//!
//! Each placement is its own row: edits to different widgets on two devices
//! merge, and overlaps left by a merge are resolved when the page is drawn.
//! Each space has a Home page, `home-<space>`, that cannot be removed; its
//! id is fixed so devices creating it offline end up with one page.

use std::collections::BTreeSet;
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
const MAX_TITLE: usize = 40;
const MAX_FOCUS: usize = 40;
const MAX_ROWS: i64 = 400;
const MAX_CONFIG_BYTES: usize = 4096;
const MAX_ID: usize = 64;

const SCHEMA: &str = r#"{
  "version": 3,
  "tables": [
    { "name": "pages", "delete_policy": "delete-wins", "columns": [
      { "name": "space", "type": "text" },
      { "name": "title", "type": "text" },
      { "name": "icon", "type": "text" },
      { "name": "position", "type": "integer", "default": 0 },
      { "name": "system", "type": "boolean", "default": false },
      { "name": "focus", "type": "text" }
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
      { "name": "focus", "type": "text" }
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
    let mut out = clean_config(&spec["page"]["config"], config);
    if widget == "link" {
        let web = out
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|u| u.starts_with("https://") || u.starts_with("http://"));
        if !web {
            out.remove("url");
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
    Some(PageInfo {
        id: text(row, "id")?,
        space: text(row, "space")?,
        title: text(row, "title").unwrap_or_else(|| "Page".into()),
        icon: text(row, "icon"),
        position: int(row, "position", 0),
        system: truthy(row, "system"),
        focus: text(row, "focus").filter(|f| !f.is_empty()),
    })
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
            "SELECT id, space, title, icon, position, system, focus FROM pages \
             WHERE _deleted IS NULL AND space = ?1 \
             ORDER BY system DESC, position, title COLLATE NOCASE, id",
            &[json!(space)],
        )?;
        Ok(rows.iter().filter_map(page_info).collect())
    }

    pub fn page(&self, id: &str) -> Result<Option<PageInfo>> {
        let rows = self.store.query(
            "SELECT id, space, title, icon, position, system, focus FROM pages \
             WHERE _deleted IS NULL AND id = ?1",
            &[json!(id)],
        )?;
        Ok(rows.first().and_then(page_info))
    }

    fn require(&self, id: &str) -> Result<PageInfo> {
        self.page(id)?.ok_or_else(|| bad(format!("no page `{id}`")))
    }

    pub fn placements(&self, page: &str) -> Result<Vec<Placement>> {
        let rows = self.store.query(
            "SELECT id, plugin, widget, x, y, w, h, config, focus FROM placements \
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
                })
            })
            .collect())
    }

    pub fn create(&mut self, space: &str, title: &str, icon: Option<&str>) -> Result<PageInfo> {
        let title = clean_title(title)?;
        let pages = self.list(space)?;
        if pages.len() >= MAX_PAGES {
            return Err(bad(format!("A space holds at most {MAX_PAGES} pages")));
        }
        let position = pages.iter().map(|p| p.position).max().unwrap_or(0) + 1;
        let id = random_id('p')?;
        let mut values = Map::new();
        values.insert("space".into(), json!(space));
        values.insert("title".into(), json!(title));
        values.insert("icon".into(), json!(icon));
        values.insert("position".into(), json!(position));
        values.insert("system".into(), json!(false));
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
        self.require(id)?;
        let mut values = Map::new();
        if let Some(title) = title {
            values.insert("title".into(), json!(clean_title(title)?));
        }
        if let Some(icon) = icon {
            values.insert("icon".into(), json!(icon));
        }
        if let Some(position) = position {
            values.insert("position".into(), json!(position));
        }
        if let Some(focus) = focus {
            values.insert("focus".into(), json!(clean_focus(focus)?));
        }
        if !values.is_empty() {
            self.store.write(|w| w.update("pages", id, &values))?;
        }
        self.require(id)
    }

    /// Removes a page and its placements on every device. Home stays.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.require(id)?.system {
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
        self.require(page)?;
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
        let v1: PluginSchema = serde_json::from_str(
            &SCHEMA
                .replace("\"version\": 3", "\"version\": 1")
                .replace(",\n      { \"name\": \"focus\", \"type\": \"text\" }", ""),
        )
        .unwrap();
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
