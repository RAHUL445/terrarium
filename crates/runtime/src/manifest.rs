//! `manifest.json`: what a plugin declares to the host.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use terrarium_core::PluginSchema;

use crate::error::{Result, RuntimeError};

/// The WIT world plugins are built against.
pub const API_VERSION: &str = "terrarium:plugin@0.1.0";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    #[serde(default)]
    pub description: String,
    pub storage: PluginSchema,
    /// Operations the UI and other callers may invoke via `handle`.
    #[serde(default)]
    pub operations: BTreeMap<String, OperationDef>,
    /// Named read-only queries the host runs for UI bindings, without
    /// calling plugin code.
    #[serde(default)]
    pub queries: BTreeMap<String, QueryDef>,
    #[serde(default)]
    pub pages: Vec<ViewDef>,
    #[serde(default)]
    pub widgets: Vec<ViewDef>,
    #[serde(default)]
    pub limits: Limits,
    /// Interfaces this plugin offers other plugins: name -> operations.
    #[serde(default)]
    pub provides: BTreeMap<String, InterfaceDef>,
    /// Interfaces this plugin calls through `interop`.
    #[serde(default)]
    pub uses: Vec<String>,
    /// Operations the host runs on a schedule.
    #[serde(default)]
    pub jobs: BTreeMap<String, JobDef>,
    /// A declared query the host reads to schedule system notifications.
    /// It gets `now_ms` and returns rows with `id`, `at_ms`, `title`, and
    /// optionally `body` and `notify_on` (`phone`, `desktop` or `both`).
    #[serde(default)]
    pub reminders: Option<RemindersDef>,
    /// Device features beyond storage, interop and notifications, each of
    /// which the user consents to at install: `speech`.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Icon in the host's set (see [`ICONS`]); a letter tile otherwise.
    #[serde(default)]
    pub icon: Option<String>,
    /// Tile color, `#rrggbb`. White icons must stay readable on it.
    #[serde(default)]
    pub color: Option<String>,
    /// A declared query returning one row with `count`, shown next to the
    /// app in navigation. Its params may only be `today` and `weekday`.
    #[serde(default)]
    pub badge: Option<String>,
    /// A declared query taking `q` and returning rows with `title` (and
    /// optionally `subtitle`), for the host's search.
    #[serde(default)]
    pub search: Option<String>,
    /// Commands the host offers in search and quick add. The typed text is
    /// passed to `op` as the `arg` argument.
    #[serde(default)]
    pub quick_actions: Vec<QuickAction>,
    /// Accepts text shared from other apps (Android).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_target: Option<ShareTarget>,
    /// Where search hits and items named by id open. Defaults to the
    /// first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<OpenDef>,
    /// Tables whose rows the host's search index holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index: Vec<IndexDef>,
    /// How the host describes and links to rows by ref.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<RefDef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickAction {
    pub label: String,
    pub op: String,
    pub arg: String,
}

pub const MAX_INDEX_ENTRIES: usize = 8;

/// Android share sheet: shared text goes to `op` as `arg`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareTarget {
    pub types: Vec<String>,
    pub op: String,
    pub arg: String,
}

pub const SHARE_TYPES: &[&str] = &["text/plain", "text/uri-list"];

/// Icon names the host can draw.
pub const ICONS: &[&str] = &[
    "check", "note", "target", "bookmark", "chart", "mic", "calendar", "book", "star", "heart",
    "folder", "inbox", "link", "clock", "list",
];

/// Permissions a manifest may ask for.
pub const PERMISSIONS: &[&str] = &["speech", "ai"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDef {
    pub operations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobDef {
    pub operation: String,
    #[serde(default)]
    pub args: serde_json::Value,
    pub schedule: Schedule,
    #[serde(default)]
    pub run_on: RunOn,
}

/// `{"every_minutes": 60}`, `{"daily_at": "07:00"}` or
/// `{"weekly": {"day": "sun", "at": "18:00"}}`, in the user's timezone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Schedule {
    EveryMinutes(u32),
    DailyAt(String),
    Weekly { day: String, at: String },
}

/// Where a job may run. Phones only run jobs while the app is open, so
/// heavy or periodic work belongs on the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunOn {
    Any,
    #[default]
    Desktop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemindersDef {
    pub query: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDef {
    #[serde(default)]
    pub description: String,
    /// What running it does to the user's data. Callers outside the app
    /// need a token that allows changes for anything but `read`. When left
    /// out, see [`OperationDef::effect_of`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    /// Text a person must approve before the operation runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
    /// The arguments it takes. Without them the operation is "untyped":
    /// any object is passed through unchecked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, ParamSpec>>,
    /// Fields of the object it returns, for callers and documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, ParamSpec>>,
    /// Example argument objects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<serde_json::Value>,
    /// `false` keeps the operation away from callers outside the app (the
    /// command line, agents). The UI, schedules and interop still reach it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cli: bool,
    /// The operation works on one item named by `id`, and the host may run
    /// it on many items, once per id (`plugin.run_bulk`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bulk: bool,
    /// A declared operation that does the same for many items in one call:
    /// it takes `ids` (a list of ids) plus the same other arguments, and
    /// commits all or nothing. Needs `bulk: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bulk_op: Option<String>,
}

impl Default for OperationDef {
    fn default() -> Self {
        OperationDef {
            description: String::new(),
            effect: None,
            confirm: None,
            params: None,
            result: None,
            examples: Vec::new(),
            cli: true,
            bulk: false,
            bulk_op: None,
        }
    }
}

/// Name prefixes of operations that are destructive unless the manifest
/// says otherwise.
const DESTRUCTIVE_NAMES: &[&str] = &["delete", "remove", "clear", "purge", "archive"];

impl OperationDef {
    /// The declared effect, or `destructive` for an operation named like
    /// `delete…`, `remove…`, `clear…`, `purge…` or `archive…`, else `write`.
    pub fn effect_of(&self, name: &str) -> Effect {
        self.effect.unwrap_or_else(
            || match DESTRUCTIVE_NAMES.iter().any(|p| name.starts_with(p)) {
                true => Effect::Destructive,
                false => Effect::Write,
            },
        )
    }
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

/// What an operation does to the user's data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// Reads only.
    Read,
    /// Creates or changes data.
    #[default]
    Write,
    /// Removes or hides data, or changes many items at once.
    Destructive,
}

/// The type of an argument, query parameter or result field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    /// One line of text.
    String,
    /// Longer text.
    Text,
    Integer,
    Number,
    Boolean,
    /// `YYYY-MM-DD`.
    Date,
    /// `HH:MM`.
    Time,
    /// A row id.
    Id,
    /// One of `values`.
    Enum,
    /// Any JSON value.
    Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Allowed values of an `enum`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Bounds of an `integer` or `number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

impl ParamSpec {
    pub fn new(ty: ParamType) -> Self {
        ParamSpec {
            ty,
            required: false,
            description: String::new(),
            values: Vec::new(),
            min: None,
            max: None,
        }
    }

    fn validate(&self, what: &str) -> std::result::Result<(), String> {
        match self.ty {
            ParamType::Enum if self.values.is_empty() => {
                Err(format!("{what}: an enum needs `values`"))
            }
            ParamType::Enum => Ok(()),
            _ if !self.values.is_empty() => Err(format!("{what}: only an enum takes `values`")),
            ParamType::Integer | ParamType::Number => match (self.min, self.max) {
                (Some(lo), Some(hi)) if lo > hi => Err(format!("{what}: `min` is above `max`")),
                _ => Ok(()),
            },
            _ if self.min.is_some() || self.max.is_some() => {
                Err(format!("{what}: only numbers take `min` and `max`"))
            }
            _ => Ok(()),
        }
    }
}

/// Query parameters the host fills in from the device clock when a caller
/// leaves them out.
pub const CONTEXT_PARAMS: &[&str] = &["today", "weekday", "now_ms", "utc_offset_ms", "tz"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryDef {
    pub sql: String,
    /// Names bound in order to `?1`, `?2`, ... from the caller's values.
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Types of some or all of `params`. Context parameters (see
    /// [`CONTEXT_PARAMS`]) need none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub types: BTreeMap<String, ParamSpec>,
    /// `false` keeps the query away from callers outside the app.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cli: bool,
    /// Lets people show this query in a View widget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<QueryView>,
}

/// How the host may show a query's rows in a user-built view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryView {
    /// Shown when people pick data for a view, e.g. "Tasks".
    pub title: String,
    /// The column used as each row's title. Defaults to `title`.
    #[serde(default = "default_title_column")]
    pub title_column: String,
    /// Fixed values for the query's own parameters (not context ones).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Columns the query returns that a view may show, in display order.
    /// The host cannot check SQL output names statically; queries should
    /// return each declared `key` and `id`.
    pub columns: Vec<ViewColumn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewColumn {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: ParamType,
    /// Allowed values of an `enum` column, in lane order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Labels for `values`, same length.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Operation that moves a row to another value of this enum column
    /// (board drag and drop). `args` may bind `{{row.*}}` and `{{value}}`.
    #[serde(default, rename = "move", skip_serializing_if = "Option::is_none")]
    pub move_op: Option<MoveDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveDef {
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Map<String, serde_json::Value>,
}

fn default_title_column() -> String {
    "title".into()
}

/// Where an item found by search, or named by id, opens in the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenDef {
    /// A declared page; the item id is passed to it.
    pub page: String,
}

/// A table the host's search index holds, read through one of the
/// plugin's own queries. The query takes `ids` (a JSON array of row ids)
/// and returns `id`, `title` and optionally `body`, `keywords` and
/// `subtitle`. Rows it leaves out (trashed, hidden) are dropped from
/// the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDef {
    pub table: String,
    pub query: String,
    /// Short label shown on hits, e.g. "Task".
    pub kind: String,
}

/// How the host describes and links to rows of one table. `resolve`
/// takes `ids` and returns `id`, `title`, optional `subtitle` and `done`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefDef {
    pub table: String,
    pub resolve: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDef {
    pub id: String,
    pub title: String,
    /// Path to the UI template JSON, relative to the package directory.
    pub template: String,
}

/// Per-call execution limits. Defaults suit small logic; manifests may ask
/// for more, up to the host ceilings below.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub fuel: u64,
    pub memory_mb: u32,
    pub timeout_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 200_000_000,
            memory_mb: 32,
            timeout_ms: 2_000,
        }
    }
}

const MAX_FUEL: u64 = 2_000_000_000;
const MAX_MEMORY_MB: u32 = 128;
const MAX_TIMEOUT_MS: u64 = 10_000;

impl Manifest {
    pub fn read(dir: &Path) -> Result<Manifest> {
        let path = dir.join("manifest.json");
        let bytes = std::fs::read(&path).map_err(|e| RuntimeError::io(&path, e))?;
        Manifest::parse(&bytes)
            .map_err(|e| RuntimeError::Manifest(format!("{}: {e}", path.display())))
    }

    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|e| RuntimeError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(RuntimeError::Manifest(m));
        let id_ok = self.id.len() <= 63
            && self.id.starts_with(|c: char| c.is_ascii_lowercase())
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !id_ok {
            return bad(format!(
                "id `{}` must use a-z, 0-9 and - and start with a letter",
                self.id
            ));
        }
        if crate::commands::HOST_NAMESPACES.contains(&self.id.as_str()) {
            return bad(format!("id `{}` is reserved for the host", self.id));
        }
        if self.api != API_VERSION {
            return bad(format!(
                "api `{}` is not supported; expected `{API_VERSION}`",
                self.api
            ));
        }
        self.storage.validate()?;
        let limits = self.limits;
        if limits.fuel == 0 || limits.fuel > MAX_FUEL {
            return bad(format!("limits.fuel must be 1..={MAX_FUEL}"));
        }
        if limits.memory_mb == 0 || limits.memory_mb > MAX_MEMORY_MB {
            return bad(format!("limits.memory_mb must be 1..={MAX_MEMORY_MB}"));
        }
        if limits.timeout_ms == 0 || limits.timeout_ms > MAX_TIMEOUT_MS {
            return bad(format!("limits.timeout_ms must be 1..={MAX_TIMEOUT_MS}"));
        }
        for view in self.pages.iter().chain(&self.widgets) {
            let path = Path::new(&view.template);
            if path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            {
                return bad(format!(
                    "template `{}` must be a relative path inside the package",
                    view.template
                ));
            }
        }
        for (name, iface) in &self.provides {
            if let Some(op) = iface
                .operations
                .iter()
                .find(|op| !self.operations.contains_key(*op))
            {
                return bad(format!(
                    "provides `{name}`: `{op}` is not a declared operation"
                ));
            }
        }
        for (name, job) in &self.jobs {
            if !self.operations.contains_key(&job.operation) {
                return bad(format!(
                    "job `{name}`: `{}` is not a declared operation",
                    job.operation
                ));
            }
            let check_time = |at: &str| crate::time::time(at).map(|_| ());
            let valid = match &job.schedule {
                Schedule::EveryMinutes(m) => (5..=10_080)
                    .contains(m)
                    .then_some(())
                    .ok_or("every_minutes must be 5..=10080".to_string()),
                Schedule::DailyAt(at) => check_time(at),
                Schedule::Weekly { day, at } => crate::time::weekday_name(day)
                    .ok_or(format!("unknown weekday `{day}` (use mon..sun)"))
                    .and_then(|_| check_time(at)),
            };
            if let Err(e) = valid {
                return bad(format!("job `{name}`: {e}"));
            }
        }
        if let Some(p) = self
            .permissions
            .iter()
            .find(|p| !PERMISSIONS.contains(&p.as_str()))
        {
            return bad(format!(
                "unknown permission `{p}` (known: {})",
                PERMISSIONS.join(", ")
            ));
        }
        if let Some(icon) = &self.icon {
            if !ICONS.contains(&icon.as_str()) {
                return bad(format!(
                    "unknown icon `{icon}` (known: {})",
                    ICONS.join(", ")
                ));
            }
        }
        if let Some(color) = &self.color {
            let ok = color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                return bad(format!("color `{color}` must look like #2563eb"));
            }
        }
        let check_query = |what: &str, name: &Option<String>, allowed: &[&str]| match name {
            None => Ok(()),
            Some(name) => match self.queries.get(name) {
                None => Err(RuntimeError::Manifest(format!(
                    "{what} query `{name}` is not declared"
                ))),
                Some(q) if q.params.iter().any(|p| !allowed.contains(&p.as_str())) => {
                    Err(RuntimeError::Manifest(format!(
                        "{what} query `{name}` may only take {}",
                        allowed.join(", ")
                    )))
                }
                Some(_) => Ok(()),
            },
        };
        check_query("badge", &self.badge, &["today", "weekday"])?;
        check_query("search", &self.search, &["q"])?;
        if self.index.len() > MAX_INDEX_ENTRIES {
            return bad("index: at most 8 entries".into());
        }
        let mut indexed = std::collections::BTreeSet::new();
        for entry in &self.index {
            let IndexDef { table, query, kind } = entry;
            if !self.storage.tables.iter().any(|t| &t.name == table) {
                return bad(format!("index: `{table}` is not a declared table"));
            }
            if !indexed.insert(table) {
                return bad(format!("index: `{table}` is listed twice"));
            }
            match self.queries.get(query) {
                None => return bad(format!("index query `{query}` is not declared")),
                Some(q) if q.params != ["ids"] => {
                    return bad(format!("index query `{query}` must take exactly `ids`"));
                }
                Some(_) => {}
            }
            if kind.trim().is_empty()
                || kind.chars().count() > 20
                || kind.chars().any(char::is_control)
            {
                return bad("index kind must be 1-20 characters".into());
            }
        }
        if self.refs.len() > 8 {
            return bad("refs: at most 8 entries".into());
        }
        let mut referenced = std::collections::BTreeSet::new();
        for entry in &self.refs {
            let RefDef {
                table,
                resolve,
                kind,
                icon,
            } = entry;
            if !self.storage.tables.iter().any(|t| &t.name == table) {
                return bad(format!("refs: `{table}` is not a declared table"));
            }
            if !referenced.insert(table) {
                return bad(format!("refs: `{table}` is listed twice"));
            }
            match self.queries.get(resolve) {
                None => return bad(format!("refs query `{resolve}` is not declared")),
                Some(q) if q.params != ["ids"] => {
                    return bad(format!("refs query `{resolve}` must take exactly `ids`"))
                }
                Some(_) => {}
            }
            if kind.trim().is_empty()
                || kind.chars().count() > 20
                || kind.chars().any(char::is_control)
            {
                return bad("refs kind must be 1-20 characters".into());
            }
            if let Some(icon) = icon {
                if !ICONS.contains(&icon.as_str()) {
                    return bad(format!(
                        "unknown icon `{icon}` (known: {})",
                        ICONS.join(", ")
                    ));
                }
            }
        }
        for action in &self.quick_actions {
            if !self.operations.contains_key(&action.op) {
                return bad(format!(
                    "quick action `{}`: `{}` is not a declared operation",
                    action.label, action.op
                ));
            }
            if action.label.trim().is_empty() || action.label.chars().count() > 40 {
                return bad("quick action labels must be 1-40 characters".into());
            }
        }
        if let Some(target) = &self.share_target {
            if target.types.is_empty() {
                return bad("share_target: types must list text/plain or text/uri-list".into());
            }
            if let Some(ty) = target
                .types
                .iter()
                .find(|ty| !SHARE_TYPES.contains(&ty.as_str()))
            {
                return bad(format!(
                    "share_target: unknown type `{ty}` (known: text/plain, text/uri-list)"
                ));
            }
            let unique = target
                .types
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            if unique != target.types.len() {
                return bad("share_target: types repeat".into());
            }
            let op = match self.operations.get(&target.op) {
                Some(op) => op,
                None => {
                    return bad(format!(
                        "share_target: `{}` is not a declared operation",
                        target.op
                    ))
                }
            };
            if op.effect_of(&target.op) == Effect::Destructive {
                return bad(format!(
                    "share_target: `{}` must not be destructive",
                    target.op
                ));
            }
            let arg_len = target.arg.chars().count();
            if arg_len == 0 || arg_len > 40 {
                return bad("share_target: arg must be 1-40 characters".into());
            }
            if let Some(params) = &op.params {
                if !params.contains_key(&target.arg) {
                    return bad(format!(
                        "share_target: `{}` has no parameter `{}`",
                        target.op, target.arg
                    ));
                }
            }
        }
        for (name, op) in &self.operations {
            for (param, spec) in op.params.iter().chain(op.result.iter()).flatten() {
                spec.validate(&format!("operation `{name}`, `{param}`"))
                    .map_err(RuntimeError::Manifest)?;
            }
            if op.examples.iter().any(|e| !e.is_object()) {
                return bad(format!("operation `{name}`: examples must be objects"));
            }
            if op.confirm.as_ref().is_some_and(|c| c.trim().is_empty()) {
                return bad(format!("operation `{name}`: `confirm` must not be empty"));
            }
            if op.bulk_op.is_some() && !op.bulk {
                return bad(format!("operation `{name}`: `bulk_op` needs `bulk: true`"));
            }
            if op.bulk && op.effect_of(name) == Effect::Read {
                return bad(format!(
                    "operation `{name}`: a read-only operation can't be bulk"
                ));
            }
            if op.bulk && op.params.as_ref().is_some_and(|p| !p.contains_key("id")) {
                return bad(format!("operation `{name}`: a bulk operation takes `id`"));
            }
            if let Some(many) = &op.bulk_op {
                if many == name {
                    return bad(format!(
                        "operation `{name}`: `bulk_op` must name another operation"
                    ));
                }
                let Some(target) = self.operations.get(many) else {
                    return bad(format!(
                        "operation `{name}`: bulk_op `{many}` is not a declared operation"
                    ));
                };
                if target
                    .params
                    .as_ref()
                    .is_some_and(|p| !p.get("ids").is_some_and(|spec| spec.ty == ParamType::Json))
                {
                    return bad(format!(
                        "operation `{many}`: takes `ids` (type json) as the bulk_op of `{name}`"
                    ));
                }
            }
        }
        for (name, query) in &self.queries {
            for (param, spec) in &query.types {
                if !query.params.contains(param) {
                    return bad(format!(
                        "query `{name}`: `{param}` has a type but is not a parameter"
                    ));
                }
                if CONTEXT_PARAMS.contains(&param.as_str()) {
                    return bad(format!(
                        "query `{name}`: `{param}` is filled in by the host and takes no type"
                    ));
                }
                spec.validate(&format!("query `{name}`, `{param}`"))
                    .map_err(RuntimeError::Manifest)?;
            }
            if let Some(view) = &query.view {
                let title_len = view.title.trim().chars().count();
                if !(1..=40).contains(&title_len) {
                    return bad(format!(
                        "query `{name}`: view title must be 1-40 characters"
                    ));
                }
                if !(1..=20).contains(&view.columns.len()) {
                    return bad(format!("query `{name}`: a view needs 1-20 columns"));
                }
                let mut seen = BTreeSet::new();
                for column in &view.columns {
                    let key_len = column.key.chars().count();
                    let key_ok = (1..=64).contains(&key_len)
                        && column
                            .key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if !key_ok {
                        return bad(format!(
                            "query `{name}`: view column `{}` is not a valid name",
                            column.key
                        ));
                    }
                    if !seen.insert(column.key.clone()) {
                        return bad(format!(
                            "query `{name}`: view column `{}` repeats",
                            column.key
                        ));
                    }
                    let label_len = column.label.trim().chars().count();
                    if !(1..=40).contains(&label_len) {
                        return bad(format!(
                            "query `{name}`: view column `{}` needs a label of 1-40 characters",
                            column.key
                        ));
                    }
                    if column.ty == ParamType::Enum {
                        let values_ok = (1..=12).contains(&column.values.len())
                            && column.values.iter().all(|v| !v.is_empty());
                        if !values_ok {
                            return bad(format!(
                                "query `{name}`: view column `{}`: an enum needs 1-12 values",
                                column.key
                            ));
                        }
                    } else if !column.values.is_empty() {
                        return bad(format!(
                            "query `{name}`: view column `{}`: only an enum takes values",
                            column.key
                        ));
                    }
                    if !column.labels.is_empty() && column.labels.len() != column.values.len() {
                        return bad(format!(
                            "query `{name}`: view column `{}`: labels must match values",
                            column.key
                        ));
                    }
                    if let Some(move_op) = &column.move_op {
                        if column.ty != ParamType::Enum {
                            return bad(format!(
                                "query `{name}`: view column `{}`: only an enum column can have move",
                                column.key
                            ));
                        }
                        let Some(op) = self.operations.get(&move_op.op) else {
                            return bad(format!(
                                "query `{name}`: view column `{}`: move `{}` is not a declared operation",
                                column.key, move_op.op
                            ));
                        };
                        if op.effect_of(&move_op.op) == Effect::Destructive {
                            return bad(format!(
                                "query `{name}`: view column `{}`: move `{}` must not be destructive",
                                column.key, move_op.op
                            ));
                        }
                    }
                }
                let title_column_ok = view.columns.iter().any(|column| {
                    column.key == view.title_column
                        && (column.ty == ParamType::String || column.ty == ParamType::Text)
                });
                if !title_column_ok {
                    return bad(format!(
                        "query `{name}`: view title_column `{}` must be a string column",
                        view.title_column
                    ));
                }
                for (param, value) in &view.params {
                    if !query.params.contains(param) || CONTEXT_PARAMS.contains(&param.as_str()) {
                        return bad(format!(
                            "query `{name}`: view param `{param}` is not a parameter of the query"
                        ));
                    }
                    let plain = value.is_null()
                        || value.is_boolean()
                        || value.is_number()
                        || value.is_string();
                    if !plain {
                        return bad(format!(
                            "query `{name}`: view param `{param}` must be a plain value"
                        ));
                    }
                }
            }
        }
        if let Some(open) = &self.open {
            if !self.pages.iter().any(|p| p.id == open.page) {
                return bad(format!("open: `{}` is not a declared page", open.page));
            }
        }
        if let Some(reminders) = &self.reminders {
            match self.queries.get(&reminders.query) {
                Some(q) if q.params == ["now_ms"] => {}
                Some(_) => {
                    return bad(format!(
                        "reminders query `{}` must take exactly `now_ms`",
                        reminders.query
                    ))
                }
                None => {
                    return bad(format!(
                        "reminders query `{}` is not declared",
                        reminders.query
                    ))
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn base() -> Value {
        json!({
            "id": "sample", "name": "Sample", "version": "1.0.0", "api": API_VERSION,
            "storage": {"version": 1, "tables": [
                {"name": "items", "columns": [
                    {"name": "title", "type": "text"},
                    {"name": "text", "type": "text"}
                ]},
                {"name": "steps", "columns": [{"name": "item_id", "type": "text"}]}
            ]},
            "queries": {
                "index_items": {"sql": "SELECT id, title FROM items WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1))", "params": ["ids"]},
                "all": {"sql": "SELECT id FROM items", "params": []}
            },
            "operations": {
                "ok": {},
                "mark": { "params": { "id": { "type": "id", "required": true } } },
                "mark_many": { "params": { "ids": { "type": "json", "required": true } } }
            }
        })
    }

    fn parse(v: &Value) -> Result<Manifest> {
        Manifest::parse(&serde_json::to_vec(v).unwrap())
    }

    fn error(v: &Value) -> String {
        parse(v).unwrap_err().to_string()
    }

    fn entry() -> Value {
        json!({"table": "items", "query": "index_items", "kind": "Item"})
    }

    #[test]
    fn a_manifest_without_index_still_loads() {
        let m = parse(&base()).unwrap();
        assert!(m.index.is_empty());
        assert!(serde_json::to_value(m).unwrap().get("index").is_none());
    }

    #[test]
    fn a_valid_index_is_kept() {
        let mut v = base();
        v["index"] = json!([entry()]);
        assert_eq!(
            parse(&v).unwrap().index[0],
            IndexDef {
                table: "items".into(),
                query: "index_items".into(),
                kind: "Item".into()
            }
        );
    }

    #[test]
    fn index_entries_are_checked() {
        for (key, value, message) in [
            (
                "table",
                json!("nope"),
                "index: `nope` is not a declared table",
            ),
            (
                "query",
                json!("missing"),
                "index query `missing` is not declared",
            ),
            (
                "query",
                json!("all"),
                "index query `all` must take exactly `ids`",
            ),
            ("kind", json!(""), "index kind must be 1-20 characters"),
            ("kind", json!("  "), "index kind must be 1-20 characters"),
            (
                "kind",
                json!("x".repeat(21)),
                "index kind must be 1-20 characters",
            ),
            (
                "kind",
                json!("Item\n"),
                "index kind must be 1-20 characters",
            ),
            ("extra", json!(1), "unknown field `extra`"),
        ] {
            let mut v = base();
            let mut e = entry();
            e[key] = value;
            v["index"] = json!([e]);
            assert!(error(&v).contains(message), "{}", error(&v));
        }
        let mut v = base();
        v["index"] = json!([entry(), entry()]);
        assert!(error(&v).contains("index: `items` is listed twice"));
        let mut v = base();
        v["index"] = json!(vec![entry(); 9]);
        assert!(error(&v).contains("index: at most 8 entries"));
    }

    fn ref_entry() -> Value {
        json!({"table": "items", "resolve": "index_items", "kind": "Item", "icon": "check"})
    }

    #[test]
    fn refs_are_optional() {
        let m = parse(&base()).unwrap();
        assert!(m.refs.is_empty());
        assert!(serde_json::to_value(m).unwrap().get("refs").is_none());
        let mut v = base();
        v["refs"] = json!([ref_entry()]);
        assert_eq!(
            parse(&v).unwrap().refs[0],
            RefDef {
                table: "items".into(),
                resolve: "index_items".into(),
                kind: "Item".into(),
                icon: Some("check".into())
            }
        );
        v["refs"][0].as_object_mut().unwrap().remove("icon");
        let m = parse(&v).unwrap();
        assert_eq!(m.refs[0].icon, None);
        assert!(serde_json::to_value(m).unwrap()["refs"][0]
            .get("icon")
            .is_none());
    }

    #[test]
    fn refs_entries_are_checked() {
        for (key, value, message) in [
            (
                "table",
                json!("nope"),
                "refs: `nope` is not a declared table",
            ),
            (
                "resolve",
                json!("missing"),
                "refs query `missing` is not declared",
            ),
            (
                "resolve",
                json!("all"),
                "refs query `all` must take exactly `ids`",
            ),
            ("kind", json!(""), "refs kind must be 1-20 characters"),
            ("kind", json!("  "), "refs kind must be 1-20 characters"),
            (
                "kind",
                json!("x".repeat(21)),
                "refs kind must be 1-20 characters",
            ),
            ("kind", json!("Item\n"), "refs kind must be 1-20 characters"),
            ("icon", json!("nope"), "unknown icon `nope` (known: "),
            ("extra", json!(1), "unknown field `extra`"),
        ] {
            let mut v = base();
            let mut entry = ref_entry();
            entry[key] = value;
            v["refs"] = json!([entry]);
            assert!(error(&v).contains(message), "{}", error(&v));
        }
        let mut v = base();
        v["refs"] = json!([ref_entry(), ref_entry()]);
        assert!(error(&v).contains("refs: `items` is listed twice"));
        let mut v = base();
        v["refs"] = json!(vec![ref_entry(); 9]);
        assert!(error(&v).contains("refs: at most 8 entries"));
    }

    fn fixture() -> Value {
        serde_json::from_slice(include_bytes!("../tests/fixtures/limits/manifest.json")).unwrap()
    }

    fn with_view(mut view: Value) -> Value {
        let mut manifest = fixture();
        manifest["queries"]["all"]["view"] = view.take();
        manifest
    }

    fn valid_view() -> Value {
        json!({
            "title": "Notes",
            "title_column": "note",
            "columns": [
                { "key": "note", "label": "Note", "type": "string" },
                {
                    "key": "state",
                    "label": "State",
                    "type": "enum",
                    "values": ["a", "b"],
                    "labels": ["A", "B"],
                    "move": { "op": "ok", "args": { "id": "{{row.id}}", "to": "{{value}}" } }
                }
            ]
        })
    }

    fn check(edit: impl FnOnce(&mut Value)) -> Option<String> {
        let mut manifest = base();
        edit(&mut manifest);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        match Manifest::parse(&bytes) {
            Ok(_) => None,
            Err(RuntimeError::Manifest(message)) => Some(message),
            Err(e) => Some(e.to_string()),
        }
    }

    fn expect_error(edit: impl FnOnce(&mut Value), needle: &str) {
        let error = check(edit).expect("expected manifest error");
        assert!(error.contains(needle), "expected `{needle}` in `{error}`");
    }

    #[test]
    fn an_old_manifest_without_views_still_loads() {
        assert!(Manifest::parse(include_bytes!("../tests/fixtures/limits/manifest.json")).is_ok());
    }

    #[test]
    fn a_valid_view_loads_and_round_trips() {
        let manifest =
            Manifest::parse(&serde_json::to_vec(&with_view(valid_view())).unwrap()).unwrap();
        assert!(manifest.queries["all"].view.is_some());
        let view = serde_json::to_value(&manifest.queries["all"].view).unwrap();
        assert_eq!(view["title_column"], "note");
        assert!(view["columns"][1].get("move").is_some());
        assert!(view["columns"][1].get("move_op").is_none());
    }

    #[test]
    fn view_title_column_defaults_to_title() {
        let view: QueryView = serde_json::from_value(json!({
            "title": "Notes",
            "columns": [{ "key": "title", "label": "Title", "type": "string" }]
        }))
        .unwrap();
        assert_eq!(view.title_column, "title");
    }

    #[test]
    fn bad_views_are_rejected() {
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "",
                    "title_column": "note",
                    "columns": [{ "key": "note", "label": "Note", "type": "string" }]
                });
            },
            "view title must be 1-40 characters",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": []
                });
            },
            "a view needs 1-20 columns",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [{ "key": "bad-key", "label": "Note", "type": "string" }]
                });
            },
            "view column `bad-key` is not a valid name",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [
                        { "key": "note", "label": "Note", "type": "string" },
                        { "key": "note", "label": "Second", "type": "string" }
                    ]
                });
            },
            "view column `note` repeats",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [{ "key": "note", "label": "", "type": "string" }]
                });
            },
            "view column `note` needs a label of 1-40 characters",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "state",
                    "columns": [{ "key": "state", "label": "State", "type": "enum", "values": [] }]
                });
            },
            "view column `state`: an enum needs 1-12 values",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [{ "key": "note", "label": "Note", "type": "string", "values": ["x"] }]
                });
            },
            "view column `note`: only an enum takes values",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "state",
                    "columns": [{
                        "key": "state",
                        "label": "State",
                        "type": "enum",
                        "values": ["a", "b"],
                        "labels": ["A"]
                    }]
                });
            },
            "view column `state`: labels must match values",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [{
                        "key": "note",
                        "label": "Note",
                        "type": "string",
                        "move": { "op": "ok" }
                    }]
                });
            },
            "view column `note`: only an enum column can have move",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "state",
                    "columns": [{
                        "key": "state",
                        "label": "State",
                        "type": "enum",
                        "values": ["a"],
                        "move": { "op": "nope" }
                    }]
                });
            },
            "view column `state`: move `nope` is not a declared operation",
        );
        expect_error(
            |manifest| {
                manifest["operations"]["delete_move"] = json!({});
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "state",
                    "columns": [{
                        "key": "state",
                        "label": "State",
                        "type": "enum",
                        "values": ["a"],
                        "move": { "op": "delete_move" }
                    }]
                });
            },
            "view column `state`: move `delete_move` must not be destructive",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "state",
                    "columns": [{ "key": "state", "label": "State", "type": "enum", "values": ["a"] }]
                });
            },
            "view title_column `state` must be a string column",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "params": { "today": "2026-01-01" },
                    "columns": [{ "key": "note", "label": "Note", "type": "string" }]
                });
            },
            "view param `today` is not a parameter of the query",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["params"] = json!(["x"]);
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "params": { "x": [1] },
                    "columns": [{ "key": "note", "label": "Note", "type": "string" }]
                });
            },
            "view param `x` must be a plain value",
        );
        expect_error(
            |manifest| {
                manifest["queries"]["all"]["view"] = json!({
                    "title": "Notes",
                    "title_column": "note",
                    "columns": [{ "key": "note", "label": "Note", "type": "string", "extra": true }]
                });
            },
            "unknown field `extra`",
        );
    }

    #[test]
    fn an_old_manifest_without_share_target_still_loads() {
        assert_eq!(check(|_| {}), None);
    }

    #[test]
    fn share_target_is_checked() {
        assert_eq!(
            check(|m| {
                m["share_target"] =
                    serde_json::json!({"types":["text/plain"],"op":"ok","arg":"text"});
            }),
            None
        );
        assert!(check(|m| {
            m["share_target"] = serde_json::json!({"types":[],"op":"ok","arg":"text"});
        })
        .is_some_and(|e| e.contains("must list")));
        assert!(check(|m| {
            m["share_target"] = serde_json::json!({"types":["image/png"],"op":"ok","arg":"text"});
        })
        .is_some_and(|e| e.contains("unknown type")));
        assert!(check(|m| {
            m["share_target"] =
                serde_json::json!({"types":["text/plain", "text/plain"],"op":"ok","arg":"text"});
        })
        .is_some_and(|e| e.contains("types repeat")));
        assert!(check(|m| {
            m["share_target"] =
                serde_json::json!({"types":["text/plain"],"op":"nope","arg":"text"});
        })
        .is_some_and(|e| e.contains("not a declared operation")));
        assert!(check(|m| {
            m["operations"]["ok"]["effect"] = serde_json::json!("destructive");
            m["share_target"] = serde_json::json!({"types":["text/plain"],"op":"ok","arg":"text"});
        })
        .is_some_and(|e| e.contains("must not be destructive")));
        assert!(check(|m| {
            m["share_target"] = serde_json::json!({"types":["text/plain"],"op":"ok","arg":""});
        })
        .is_some_and(|e| e.contains("1-40")));
        assert!(check(|m| {
            m["operations"]["ok"]["params"] = serde_json::json!({"url": {"type":"string"}});
            m["share_target"] = serde_json::json!({"types":["text/plain"],"op":"ok","arg":"text"});
        })
        .is_some_and(|e| e.contains("has no parameter")));
        assert!(check(|m| {
            m["share_target"] =
                serde_json::json!({"types":["text/plain"],"op":"ok","arg":"text","extra":"x"});
        })
        .is_some());
    }

    #[test]
    fn old_manifests_still_load() {
        assert_eq!(check(|_| {}), None);
        let m = Manifest::parse(&serde_json::to_vec(&base()).unwrap()).unwrap();
        assert!(!m.operations["mark"].bulk);
        assert!(m.operations["mark"].bulk_op.is_none());
        let default = OperationDef::default();
        assert!(!default.bulk);
        assert!(default.bulk_op.is_none());
        let value = serde_json::to_value(m).unwrap();
        assert!(value["operations"]["mark"].get("bulk").is_none());
        assert!(value["operations"]["mark"].get("bulk_op").is_none());
    }

    #[test]
    fn bulk_fields_load() {
        assert_eq!(
            check(|m| {
                m["operations"]["mark"]["bulk"] = json!(true);
                m["operations"]["mark"]["bulk_op"] = json!("mark_many");
            }),
            None
        );
        let mut value = base();
        value["operations"]["mark"]["bulk"] = json!(true);
        value["operations"]["mark"]["bulk_op"] = json!("mark_many");
        let m = Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        let value = serde_json::to_value(m).unwrap();
        assert_eq!(value["operations"]["mark"]["bulk"], true);
        assert_eq!(value["operations"]["mark"]["bulk_op"], "mark_many");
    }

    #[test]
    fn bulk_fields_are_checked() {
        let cases = [
            (
                json!({"bulk_op": "mark_many"}),
                "`bulk_op` needs `bulk: true`",
            ),
            (
                json!({"bulk": true, "effect": "read"}),
                "a read-only operation can't be bulk",
            ),
            (
                json!({"bulk": true, "params": {"text": {"type": "string"}}}),
                "a bulk operation takes `id`",
            ),
            (
                json!({"bulk": true, "bulk_op": "mark"}),
                "`bulk_op` must name another operation",
            ),
            (
                json!({"bulk": true, "bulk_op": "nope"}),
                "bulk_op `nope` is not a declared operation",
            ),
        ];
        for (fields, expected) in cases {
            let error = check(|m| {
                for (key, value) in fields.as_object().unwrap() {
                    m["operations"]["mark"][key] = value.clone();
                }
            })
            .unwrap();
            assert!(
                error.contains(&format!("operation `mark`: {expected}")),
                "{error}"
            );
        }
        for params in [json!({"ids": {"type": "string"}}), json!({})] {
            let error = check(|m| {
                m["operations"]["mark"]["bulk"] = json!(true);
                m["operations"]["mark"]["bulk_op"] = json!("mark_many");
                m["operations"]["mark_many"]["params"] = params.clone();
            })
            .unwrap();
            assert!(
                error.contains(
                    "operation `mark_many`: takes `ids` (type json) as the bulk_op of `mark`"
                ),
                "{error}"
            );
        }
        assert!(check(|m| m["operations"]["mark"]["bulk"] = json!("yes")).is_some());
    }

    #[test]
    fn untyped_bulk_operations_load() {
        assert_eq!(
            check(|m| {
                m["operations"]["mark"]["bulk"] = json!(true);
                m["operations"]["mark"]
                    .as_object_mut()
                    .unwrap()
                    .remove("params");
            }),
            None
        );
        assert_eq!(
            check(|m| {
                m["operations"]["mark"]["bulk"] = json!(true);
                m["operations"]["mark"]["bulk_op"] = json!("mark_many");
                m["operations"]["mark_many"]
                    .as_object_mut()
                    .unwrap()
                    .remove("params");
            }),
            None
        );
    }
}
