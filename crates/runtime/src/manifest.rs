//! `manifest.json`: what a plugin declares to the host.

use std::collections::BTreeMap;
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
    /// Where search hits and items named by id open. Defaults to the
    /// first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<OpenDef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickAction {
    pub label: String,
    pub op: String,
    pub arg: String,
}

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
    /// need a token that allows changes for anything but `read`.
    #[serde(default)]
    pub effect: Effect,
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
}

impl Default for OperationDef {
    fn default() -> Self {
        OperationDef {
            description: String::new(),
            effect: Effect::default(),
            confirm: None,
            params: None,
            result: None,
            examples: Vec::new(),
            cli: true,
        }
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
}

/// Where an item found by search, or named by id, opens in the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenDef {
    /// A declared page; the item id is passed to it.
    pub page: String,
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
