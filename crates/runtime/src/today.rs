//! `today-v1`: rows a plugin contributes to the host's Today view.
//!
//! A plugin declares sources top-level under `"today"` (with `"requires"`
//! including `"today-v1"`). Each source names one of its own queries and maps
//! each result row to the host's shared row through `{{row.*}}` templates.
//! The host runs the queries, merges rows from all plugins by section and
//! dispatches actions through the owning plugin's operation bridge. An
//! invalid entry is dropped with an error; the rest of the plugin loads.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::manifest::{Effect, Manifest};

pub const TODAY_REQUIREMENT: &str = "today-v1";

/// Sections in display order.
pub const TODAY_SECTIONS: &[&str] = &[
    "my_day", "earlier", "due", "habits", "sort", "catch_up", "wrap_up", "done",
];

pub const MAX_TODAY_SOURCES: usize = 16;
const MAX_LIMIT: u32 = 50;
const DEFAULT_LIMIT: u32 = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodaySource {
    pub id: String,
    pub section: String,
    pub query: String,
    /// Query arguments; strings may use `{{$today}}`, `{{$tomorrow}}`,
    /// `{{$now}}`, `{{$weekday}}` and `{{$utc_offset_ms}}`.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// A declared query returning `[{count}]`, the total behind "N more".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_query: Option<String>,
    /// The section's one text link, e.g. "Open Inbox".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<TodayLink>,
    pub row: TodayRow,
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayLink {
    pub label: String,
    pub action: TodayAction,
    /// Run `action` once per shown row of this source: `all`, or only rows
    /// whose resolved `tone` equals this value (e.g. `overdue`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayRow {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chips: Vec<TodayChip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<TodayCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<TodayRowAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<TodayAction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayChip {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayCheck {
    pub action: TodayAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayRowAction {
    pub label: String,
    /// Keyboard letter on Today (`t` and `s` are the documented ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub action: TodayAction,
}

/// An operation (with optional URL to open first), or a navigation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayAction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub args: Map<String, Value>,
    /// `{item?, state?}` on the plugin's first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigate: Option<Map<String, Value>>,
    /// An http(s) address opened in the system browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<Box<TodayAction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followup: Option<Box<TodayFollowup>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodayFollowup {
    pub label: String,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_value: Option<Value>,
    pub action: TodayAction,
}

/// The valid sources of one plugin and one message per rejected entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TodayLoad {
    pub sources: Vec<TodaySource>,
    pub errors: Vec<String>,
}

/// Checks the gate and size; entry contents are checked by [`today_sources`].
pub(crate) fn validate_declaration(m: &Manifest) -> Result<(), String> {
    if m.today.is_empty() {
        return Ok(());
    }
    if !m.requires.iter().any(|r| r == TODAY_REQUIREMENT) {
        return Err("today sources require `requires: [\"today-v1\"]`".into());
    }
    if m.today.len() > MAX_TODAY_SOURCES {
        return Err(format!("today: at most {MAX_TODAY_SOURCES} sources"));
    }
    Ok(())
}

/// Parses and validates each declared source independently.
pub fn today_sources(m: &Manifest) -> TodayLoad {
    let mut load = TodayLoad::default();
    let mut ids = BTreeSet::new();
    for (index, raw) in m.today.iter().enumerate() {
        let label = raw
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| format!("#{}", index + 1), |id| format!("`{id}`"));
        let result = serde_json::from_value::<TodaySource>(raw.clone())
            .map_err(|e| e.to_string())
            .and_then(|source| validate_source(m, &source).map(|()| source));
        match result {
            Ok(source) if !ids.insert(source.id.clone()) => load
                .errors
                .push(format!("{}: today {label}: id repeats", m.id)),
            Ok(source) => load.sources.push(source),
            Err(e) => load.errors.push(format!("{}: today {label}: {e}", m.id)),
        }
    }
    load
}

fn ident(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// `{{row.kind}}`: one lookup, so the kind can differ per row.
fn whole_template(value: &str) -> bool {
    value
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
        .map(str::trim)
        .is_some_and(|path| {
            (1..=60).contains(&path.len())
                && path
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$')
        })
}

fn text(value: &str, what: &str, max: usize) -> Result<(), String> {
    let len = value.trim().chars().count();
    if len == 0 || value.chars().count() > max {
        return Err(format!("{what} must be 1-{max} characters"));
    }
    Ok(())
}

fn plain(value: &Value) -> bool {
    value.is_null() || value.is_boolean() || value.is_number() || value.is_string()
}

fn validate_source(m: &Manifest, s: &TodaySource) -> Result<(), String> {
    if !ident(&s.id, 40) {
        return Err("id must use a-z, 0-9, _ and - and start with a letter".into());
    }
    if !TODAY_SECTIONS.contains(&s.section.as_str()) {
        return Err(format!(
            "unknown section `{}` (known: {})",
            s.section,
            TODAY_SECTIONS.join(", ")
        ));
    }
    let Some(query) = m.queries.get(&s.query) else {
        return Err(format!("query `{}` is not declared", s.query));
    };
    for (param, value) in &s.params {
        if !query.params.contains(param) {
            return Err(format!(
                "param `{param}` is not a parameter of `{}`",
                s.query
            ));
        }
        if !plain(value) {
            return Err(format!("param `{param}` must be a plain value"));
        }
    }
    if let Some(count) = &s.count_query {
        if !m.queries.contains_key(count) {
            return Err(format!("count_query `{count}` is not declared"));
        }
    }
    if !(1..=MAX_LIMIT).contains(&s.limit) {
        return Err(format!("limit must be 1..={MAX_LIMIT}"));
    }
    if let Some(link) = &s.link {
        text(&link.label, "link label", 40)?;
        validate_action(m, &link.action, true).map_err(|e| format!("link {e}"))?;
        if let Some(each) = &link.each {
            text(each, "link each", 20)?;
        }
    }
    let row = &s.row;
    if row.key.trim().is_empty() {
        return Err("row.key is required".into());
    }
    if row.title.trim().is_empty() {
        return Err("row.title is required".into());
    }
    if let Some(kind) = &row.kind {
        if !ident(kind, 20) && !whole_template(kind) {
            return Err("row.kind must use a-z, 0-9, _ and - or be one {{row.*}} lookup".into());
        }
    }
    if row.chips.len() > 4 {
        return Err("row: at most 4 chips".into());
    }
    if row.chips.iter().any(|c| c.text.chars().count() > 200) {
        return Err("row chip text exceeds 200 characters".into());
    }
    if let Some(check) = &row.check {
        if check.action.op.is_none() {
            return Err("row.check needs an operation".into());
        }
        validate_action(m, &check.action, false).map_err(|e| format!("row.check {e}"))?;
    }
    if row.actions.len() > 4 {
        return Err("row: at most 4 actions".into());
    }
    let mut keys = BTreeSet::new();
    for action in &row.actions {
        text(&action.label, "row action label", 24)?;
        if let Some(key) = &action.key {
            let single = key.len() == 1 && key.bytes().all(|c| c.is_ascii_lowercase());
            if !single || !keys.insert(key) {
                return Err(format!(
                    "row action `{}`: key must be one unused letter a-z",
                    action.label
                ));
            }
        }
        validate_action(m, &action.action, true)
            .map_err(|e| format!("row action `{}` {e}", action.label))?;
    }
    if let Some(open) = &row.open {
        validate_action(m, open, true).map_err(|e| format!("row.open {e}"))?;
    }
    Ok(())
}

fn validate_op(m: &Manifest, op: &str, args: &Map<String, Value>) -> Result<(), String> {
    let Some(operation) = m.operations.get(op) else {
        return Err(format!("`{op}` is not a declared operation"));
    };
    if operation.effect_of(op) == Effect::Destructive {
        return Err(format!("`{op}` must not be destructive"));
    }
    if let Some(params) = &operation.params {
        if let Some(unknown) = args.keys().find(|key| !params.contains_key(*key)) {
            return Err(format!("`{op}` argument `{unknown}` is not declared"));
        }
        if let Some((missing, _)) = params
            .iter()
            .find(|(key, spec)| spec.required && !args.contains_key(*key))
        {
            return Err(format!("`{op}` needs argument `{missing}`"));
        }
    }
    Ok(())
}

fn validate_navigate(navigate: &Map<String, Value>) -> Result<(), String> {
    if navigate.is_empty() {
        return Err("navigate needs item or state".into());
    }
    for (key, value) in navigate {
        match key.as_str() {
            "item" if value.is_string() => {}
            "state" => {
                let Some(state) = value.as_object() else {
                    return Err("navigate state must be an object".into());
                };
                let ok = state.len() <= 8
                    && state.iter().all(|(k, v)| {
                        k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && plain(v)
                    });
                if !ok {
                    return Err("navigate state takes up to 8 plain values".into());
                }
            }
            _ => return Err(format!("navigate takes item and state, not `{key}`")),
        }
    }
    Ok(())
}

fn validate_action(m: &Manifest, a: &TodayAction, nested: bool) -> Result<(), String> {
    match (&a.op, &a.navigate, &a.url) {
        (None, None, None) => return Err("needs an operation, navigate or url".into()),
        (Some(_), Some(_), _) | (None, Some(_), Some(_)) => {
            return Err("cannot combine navigate with an operation or url".into());
        }
        _ => {}
    }
    if let Some(op) = &a.op {
        validate_op(m, op, &a.args)?;
    } else if !a.args.is_empty() {
        return Err("args need an operation".into());
    }
    if let Some(navigate) = &a.navigate {
        validate_navigate(navigate)?;
    }
    if let Some(success) = &a.success {
        if success.chars().count() > 100 {
            return Err("success exceeds 100 characters".into());
        }
    }
    if let Some(undo) = &a.undo {
        if !nested
            || a.op.is_none()
            || undo.op.is_none()
            || undo.navigate.is_some()
            || undo.url.is_some()
            || undo.undo.is_some()
            || undo.followup.is_some()
        {
            return Err("undo must be a plain operation after an operation".into());
        }
        validate_op(m, undo.op.as_deref().unwrap_or_default(), &undo.args)
            .map_err(|e| format!("undo {e}"))?;
    }
    if let Some(followup) = &a.followup {
        text(&followup.label, "followup label", 40)?;
        let f = &followup.action;
        if f.undo.is_some() || f.followup.is_some() {
            return Err("followup cannot nest undo or followup".into());
        }
        validate_action(m, f, false).map_err(|e| format!("followup {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(today: Value) -> Value {
        json!({
            "id": "tasks", "name": "Tasks", "version": "1.0.0", "api": crate::API_VERSION,
            "requires": ["today-v1"],
            "storage": {"version": 1, "tables": [{"name": "items", "columns": [{"name": "title", "type": "text"}]}]},
            "operations": {
                "toggle": {"params": {"id": {"type": "id", "required": true}, "done": {"type": "boolean", "required": true}}},
                "plan": {"params": {"id": {"type": "id", "required": true}, "planned_for": {"type": "date"}}},
                "delete": {"params": {"id": {"type": "id", "required": true}}}
            },
            "queries": {
                "mine": {"sql": "SELECT id, title FROM items", "params": ["today"]},
                "count": {"sql": "SELECT COUNT(*) AS count FROM items", "params": ["today"]}
            },
            "today": today
        })
    }

    fn source() -> Value {
        json!({
            "id": "my-day", "section": "my_day", "query": "mine",
            "params": {"today": "{{$today}}"}, "limit": 5, "count_query": "count",
            "link": {"label": "Open Inbox", "action": {"navigate": {"state": {"filter": "inbox"}}}},
            "row": {
                "key": "{{row.id}}", "kind": "task", "title": "{{row.title}}",
                "chips": [{"text": "{{row.list}}"}],
                "check": {"action": {"op": "toggle", "args": {"id": "{{row.id}}", "done": "{{value}}"}}},
                "actions": [{"label": "Tomorrow", "key": "s", "action": {
                    "op": "plan", "args": {"id": "{{row.id}}", "planned_for": "{{$tomorrow}}"},
                    "success": "Planned tomorrow",
                    "undo": {"op": "plan", "args": {"id": "{{row.id}}", "planned_for": "{{$today}}"}}
                }}],
                "open": {"navigate": {"item": "{{row.id}}"}}
            }
        })
    }

    fn load(today: Value) -> TodayLoad {
        let m = Manifest::parse(&serde_json::to_vec(&manifest(today)).unwrap()).unwrap();
        today_sources(&m)
    }

    #[test]
    fn valid_source_loads_and_round_trips() {
        let loaded = load(json!([source()]));
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        assert_eq!(loaded.sources[0].limit, 5);
        assert_eq!(loaded.sources[0].section, "my_day");
        let m =
            Manifest::parse(&serde_json::to_vec(&manifest(json!([source()]))).unwrap()).unwrap();
        Manifest::parse(&serde_json::to_vec(&m).unwrap()).unwrap();
    }

    #[test]
    fn old_manifests_have_no_sources() {
        let mut value = manifest(json!([]));
        value.as_object_mut().unwrap().remove("today");
        value.as_object_mut().unwrap().remove("requires");
        let m = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(today_sources(&m), TodayLoad::default());
        assert!(serde_json::to_value(&m).unwrap().get("today").is_none());
    }

    #[test]
    fn sources_need_the_gate() {
        let mut value = manifest(json!([source()]));
        value["requires"] = json!([]);
        let error = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(error.to_string().contains("today-v1"), "{error}");
    }

    #[test]
    fn a_bad_entry_is_dropped_and_the_rest_load() {
        type Break = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(Break, &str)> = vec![
            (
                Box::new(|s| s["section"] = json!("later")),
                "unknown section `later`",
            ),
            (
                Box::new(|s| s["query"] = json!("missing")),
                "query `missing` is not declared",
            ),
            (
                Box::new(|s| s["count_query"] = json!("nope")),
                "count_query `nope`",
            ),
            (
                Box::new(|s| s["params"] = json!({"other": 1})),
                "param `other`",
            ),
            (Box::new(|s| s["limit"] = json!(0)), "limit must be"),
            (
                Box::new(|s| s["row"]["check"]["action"]["op"] = json!("nope")),
                "`nope` is not a declared operation",
            ),
            (
                Box::new(|s| s["row"]["actions"][0]["action"]["op"] = json!("delete")),
                "must not be destructive",
            ),
            (
                Box::new(|s| s["row"]["actions"][0]["action"]["args"]["extra"] = json!(1)),
                "argument `extra` is not declared",
            ),
            (
                Box::new(|s| s["row"]["open"] = json!({"navigate": {"page": "x"}})),
                "navigate takes item and state",
            ),
            (
                Box::new(|s| s["row"]["open"] = json!({"op": "toggle", "navigate": {"item": "x"}})),
                "cannot combine navigate",
            ),
            (
                Box::new(|s| s["row"]["title"] = json!(" ")),
                "row.title is required",
            ),
            (
                Box::new(|s| s["row"]["extra"] = json!(true)),
                "unknown field `extra`",
            ),
            (
                Box::new(|s| s["row"]["actions"][0]["key"] = json!("ss")),
                "key must be one unused letter",
            ),
            (
                Box::new(|s| s["row"]["kind"] = json!("Task!")),
                "row.kind must use",
            ),
            (
                Box::new(|s| s["row"]["kind"] = json!("{{row.kind}} x")),
                "row.kind must use",
            ),
        ];
        for (change, message) in cases {
            let mut bad = source();
            bad["id"] = json!("bad");
            change(&mut bad);
            let loaded = load(json!([bad, source()]));
            assert_eq!(loaded.sources.len(), 1, "{message}");
            assert_eq!(loaded.sources[0].id, "my-day");
            assert_eq!(loaded.errors.len(), 1, "{message}");
            assert!(
                loaded.errors[0].contains(message) && loaded.errors[0].contains("today `bad`"),
                "{message}: {}",
                loaded.errors[0]
            );
        }
    }

    #[test]
    fn kind_may_come_from_the_row() {
        let mut s = source();
        s["row"]["kind"] = json!("{{row.kind}}");
        let loaded = load(json!([s]));
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    }

    #[test]
    fn repeated_ids_keep_the_first() {
        let loaded = load(json!([source(), source()]));
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.errors[0].contains("id repeats"));
    }

    #[test]
    fn too_many_sources_reject_the_manifest() {
        let many: Vec<Value> = (0..17)
            .map(|i| {
                let mut s = source();
                s["id"] = json!(format!("s{i}"));
                s
            })
            .collect();
        let error =
            Manifest::parse(&serde_json::to_vec(&manifest(json!(many))).unwrap()).unwrap_err();
        assert!(error.to_string().contains("at most 16"), "{error}");
    }
}
