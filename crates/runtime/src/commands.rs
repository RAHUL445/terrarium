//! The command registry: everything the app can do, as named commands with
//! described parameters, shared by the app's own UI, `terra preview` and
//! callers outside the app (the `terrarium` command line, agents).
//!
//! Plugin commands come from manifests: each operation is an action and
//! each query a query, named `<plugin>.<name>`. Host commands live under
//! reserved namespaces (`app`, `plugin`, `space`, `page`, `search`,
//! `schedule`); plugin ids can't take those names.
//!
//! A command is identified by its kind and id, since a plugin may declare
//! an operation and a query with the same name.
//!
//! The app's own UI is trusted to pass what its templates declare, so its
//! calls are not checked against `params`. Outside callers are: unknown
//! arguments are refused, types and required arguments are checked, and a
//! read-only caller can only run `read` commands.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::RuntimeError;
use crate::manifest::{Effect, Manifest, ParamSpec, ParamType, CONTEXT_PARAMS};
use crate::pages::{host_widgets, Placement, HOST_WIDGETS};
use crate::registry::{ChangeSummary, PluginHost, ViewKind};
use crate::spaces::DEFAULT_SPACE;

/// Namespaces of host commands. Plugin ids can't use them.
pub const HOST_NAMESPACES: &[&str] = &[
    "app", "plugin", "space", "page", "search", "schedule", "theme", "sync", "backup", "ui",
];

const SEARCH_LIMIT: usize = 200;
const SEARCH_MAX: usize = 500;

/// Who is calling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The app's own UI (or `terra preview`): every command, unchecked.
    App,
    /// A caller outside the app, limited to what its token allows.
    Client(Scope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Only `read` commands.
    Read,
    /// Every command it can see.
    Change,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Query,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    Host,
    Plugin,
}

/// One command as callers see it.
#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: String,
    pub kind: Kind,
    pub owner: Owner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The first sentence of `description`.
    pub summary: String,
    pub description: String,
    /// `None`: untyped; any object is accepted.
    pub params: Option<BTreeMap<String, ParamSpec>>,
    /// Parameters the host fills in when left out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, ParamSpec>>,
    pub effect: Effect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
    /// Whether it runs in a space (the default space when none is given).
    pub space: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Value>,
    /// Only the app itself can see or call it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// A failed command: a stable `code` and a message for people.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    /// `invalid`, `not-found`, `forbidden`, `unknown-space`, `stopped`,
    /// `failed`, or a code the plugin returned (e.g. `in-trash`).
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        CommandError {
            code: code.into(),
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        CommandError::new("invalid", message)
    }

    fn not_found(kind: Kind, id: &str) -> Self {
        let kind = match kind {
            Kind::Query => "query",
            Kind::Action => "action",
        };
        CommandError::new("not-found", format!("no {kind} `{id}`"))
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<RuntimeError> for CommandError {
    fn from(e: RuntimeError) -> Self {
        let code = match &e {
            RuntimeError::Plugin { code, .. } => code.as_str(),
            RuntimeError::UnknownPlugin(_) | RuntimeError::Undeclared { .. } => "not-found",
            RuntimeError::UnknownSpace(_) => "unknown-space",
            RuntimeError::Trap(_) => "stopped",
            _ => "failed",
        };
        CommandError::new(code, e.to_string())
    }
}

fn failed(message: String) -> CommandError {
    CommandError::new("failed", message)
}

fn summary(description: &str) -> String {
    let text = description.trim();
    match text.find(". ") {
        Some(end) => text[..=end].to_string(),
        None => text.to_string(),
    }
}

// ---- Host commands --------------------------------------------------------

type Handler = fn(&mut PluginHost, &str, &Map<String, Value>, bool) -> Result<Value, CommandError>;

struct HostCommand {
    id: &'static str,
    kind: Kind,
    effect: Effect,
    space: bool,
    description: &'static str,
    confirm: Option<&'static str>,
    /// Name, type, required, description.
    params: &'static [(&'static str, ParamType, bool, &'static str)],
    run: Handler,
}

/// Runs a command of [`crate::services`] with `args`, adding `space`.
fn service(
    host: &mut PluginHost,
    name: &str,
    space: Option<&str>,
    args: &Map<String, Value>,
    desktop: bool,
) -> Result<Value, CommandError> {
    let mut args = args.clone();
    if let Some(space) = space {
        args.insert("space".into(), json!(space));
    }
    crate::services::invoke(host, name, &Value::Object(args), desktop)
        .unwrap_or_else(|| Err(format!("unknown service `{name}`")))
        .map_err(failed)
}

fn text<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, CommandError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::invalid(format!("`{key}` must be text")))
}

fn app_only() -> CommandError {
    CommandError::new("unavailable", "Only the running app can do this.")
}

/// Reads a page's placements, lets `f` change them, and saves the page.
fn edit_placements(
    host: &mut PluginHost,
    args: &Map<String, Value>,
    desktop: bool,
    f: impl FnOnce(&mut Vec<Placement>, &Map<String, Value>) -> Result<(), CommandError>,
) -> Result<Value, CommandError> {
    let page = text(args, "page")?.to_string();
    if host.pages().page(&page)?.is_none() {
        return Err(CommandError::new("not-found", format!("no page `{page}`")));
    }
    let mut list = host.pages().placements(&page)?;
    f(&mut list, args)?;
    let save = Map::from_iter([
        ("page".to_string(), json!(page)),
        ("placements".to_string(), json!(list)),
    ]);
    service(host, "page_save", None, &save, desktop)
}

fn place(
    host: &mut PluginHost,
    args: &Map<String, Value>,
    desktop: bool,
) -> Result<Value, CommandError> {
    let widget = text(args, "widget")?;
    let (plugin, widget) = widget
        .split_once('/')
        .ok_or_else(|| CommandError::invalid("`widget` must look like `plugin/widget`"))?;
    let spec = if plugin == HOST_WIDGETS {
        host_widgets()
            .as_array()
            .and_then(|w| w.iter().find(|x| x["id"] == widget).cloned())
    } else {
        host.template(plugin, ViewKind::Widget, widget)
            .ok()
            .cloned()
    }
    .ok_or_else(|| CommandError::new("not-found", format!("no widget `{plugin}/{widget}`")))?;
    let size = &spec["page"]["size"]["default"];
    let (dw, dh) = (size[0].as_i64().unwrap_or(4), size[1].as_i64().unwrap_or(3));
    let int = |key: &str| args.get(key).and_then(Value::as_i64);
    let config = match args.get("config") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(CommandError::invalid("`config` must be an object")),
    };
    let (x, y, w, h) = (int("x"), int("y"), int("w"), int("h"));
    let (plugin, widget) = (plugin.to_string(), widget.to_string());
    edit_placements(host, args, desktop, move |list, _| {
        let below = list.iter().map(|p| p.y + p.h).max().unwrap_or(0);
        list.push(Placement {
            id: String::new(),
            plugin,
            widget,
            x: x.unwrap_or(0),
            y: y.unwrap_or(below),
            w: w.unwrap_or(dw),
            h: h.unwrap_or(dh),
            config,
            focus: None,
        });
        Ok(())
    })
}

fn change_spaces(
    host: &mut PluginHost,
    f: impl FnOnce(&mut crate::spaces::Spaces) -> crate::error::Result<Value>,
) -> Result<Value, CommandError> {
    let out = f(host.spaces_mut())?;
    host.sync_spaces()?;
    Ok(out)
}

const HOST_COMMANDS: &[HostCommand] = &[
    HostCommand {
        id: "app.describe",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Every command this caller can run, with its parameters.",
        confirm: None,
        params: &[],
        run: |_, _, _, _| unreachable!("answered by invoke"),
    },
    HostCommand {
        id: "plugin.list",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Installed plugins that are loaded on this device.",
        confirm: None,
        params: &[],
        run: |host, _, _, _| {
            Ok(json!(host
                .manifests()
                .map(|m| json!({
                    "id": m.id,
                    "name": m.name,
                    "version": m.version,
                    "description": m.description,
                    "search": m.search.is_some(),
                    "pages": m.pages.iter().map(|p| json!({"id": p.id, "title": p.title})).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>()))
        },
    },
    HostCommand {
        id: "space.list",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Spaces, including removed ones (marked `removed`).",
        confirm: None,
        params: &[],
        run: |host, _, _, _| Ok(json!(host.spaces().list()?)),
    },
    HostCommand {
        id: "space.create",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Create a space. It appears on every device.",
        confirm: None,
        params: &[("name", ParamType::String, true, "Name of the space")],
        run: |host, _, args, _| {
            let name = text(args, "name")?.to_string();
            change_spaces(host, |s| Ok(json!(s.create(&name)?)))
        },
    },
    HostCommand {
        id: "space.rename",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Rename a space.",
        confirm: None,
        params: &[
            ("id", ParamType::Id, true, "Space id"),
            ("name", ParamType::String, true, "New name"),
        ],
        run: |host, _, args, _| {
            let (id, name) = (text(args, "id")?.to_string(), text(args, "name")?.to_string());
            change_spaces(host, |s| s.rename(&id, &name).map(|_| Value::Null))
        },
    },
    HostCommand {
        id: "space.remove",
        kind: Kind::Action,
        effect: Effect::Destructive,
        space: false,
        description: "Remove a space from every device. Its data is kept and it can be restored.",
        confirm: Some("Remove this space from every device?"),
        params: &[("id", ParamType::Id, true, "Space id")],
        run: |host, _, args, _| {
            let id = text(args, "id")?.to_string();
            change_spaces(host, |s| s.set_removed(&id, true).map(|_| Value::Null))
        },
    },
    HostCommand {
        id: "space.restore",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Bring back a removed space.",
        confirm: None,
        params: &[("id", ParamType::Id, true, "Space id")],
        run: |host, _, args, _| {
            let id = text(args, "id")?.to_string();
            change_spaces(host, |s| s.set_removed(&id, false).map(|_| Value::Null))
        },
    },
    HostCommand {
        id: "page.list",
        kind: Kind::Query,
        effect: Effect::Read,
        space: true,
        description: "Pages of a space, in order.",
        confirm: None,
        params: &[],
        run: |host, space, args, d| service(host, "pages_list", Some(space), args, d),
    },
    HostCommand {
        id: "page.get",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "A page and its widget placements.",
        confirm: None,
        params: &[("page", ParamType::Id, true, "Page id")],
        run: |host, _, args, d| service(host, "page_get", None, args, d),
    },
    HostCommand {
        id: "page.widgets",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Widgets the host itself offers for pages.",
        confirm: None,
        params: &[],
        run: |host, _, args, d| service(host, "page_widgets", None, args, d),
    },
    HostCommand {
        id: "page.create",
        kind: Kind::Action,
        effect: Effect::Write,
        space: true,
        description: "Create an empty page in a space.",
        confirm: None,
        params: &[
            ("title", ParamType::String, true, "Page title, at most 40 characters"),
            ("icon", ParamType::String, false, "Icon name from the host's set"),
            ("focus", ParamType::String, false, "Project or tag the page's widgets focus on"),
        ],
        run: |host, space, args, d| service(host, "page_create", Some(space), args, d),
    },
    HostCommand {
        id: "page.update",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Change a page's title, icon, position or focus.",
        confirm: None,
        params: &[
            ("page", ParamType::Id, true, "Page id"),
            ("title", ParamType::String, false, "New title"),
            ("icon", ParamType::String, false, "Icon name; null removes it"),
            ("position", ParamType::Integer, false, "Position in the page list"),
            ("focus", ParamType::String, false, "Project or tag to focus on"),
        ],
        run: |host, _, args, d| service(host, "page_update", None, args, d),
    },
    HostCommand {
        id: "page.save",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Replace a page's widget placements. Each placement is `{plugin, widget, x, y, w, h, config?}` on a 12-column grid.",
        confirm: None,
        params: &[
            ("page", ParamType::Id, true, "Page id"),
            ("placements", ParamType::Json, true, "The full list of placements"),
        ],
        run: |host, _, args, d| service(host, "page_save", None, args, d),
    },
    HostCommand {
        id: "page.place",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Add a widget to a page. Without a position it goes below the others, at its default size.",
        confirm: None,
        params: &[
            ("page", ParamType::Id, true, "Page id"),
            ("widget", ParamType::String, true, "`plugin/widget`, e.g. `todo/today`, or `_host/text` for the host's own"),
            ("x", ParamType::Integer, false, "Column, 0 to 11"),
            ("y", ParamType::Integer, false, "Row"),
            ("w", ParamType::Integer, false, "Width in columns, 1 to 12"),
            ("h", ParamType::Integer, false, "Height in rows"),
            ("config", ParamType::Json, false, "The widget's settings"),
        ],
        run: |host, _, args, d| place(host, args, d),
    },
    HostCommand {
        id: "page.move",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Move or resize a widget on a page.",
        confirm: None,
        params: &[
            ("page", ParamType::Id, true, "Page id"),
            ("placement", ParamType::Id, true, "Placement id, from page.get"),
            ("x", ParamType::Integer, false, "Column, 0 to 11"),
            ("y", ParamType::Integer, false, "Row"),
            ("w", ParamType::Integer, false, "Width in columns, 1 to 12"),
            ("h", ParamType::Integer, false, "Height in rows"),
        ],
        run: |host, _, args, d| {
            edit_placements(host, args, d, |list, args| {
                let id = text(args, "placement")?;
                let p = list.iter_mut().find(|p| p.id == id).ok_or_else(|| {
                    CommandError::new("not-found", format!("no placement `{id}` on this page"))
                })?;
                for (key, field) in [("x", &mut p.x), ("y", &mut p.y), ("w", &mut p.w), ("h", &mut p.h)] {
                    if let Some(v) = args.get(key).and_then(Value::as_i64) {
                        *field = v;
                    }
                }
                Ok(())
            })
        },
    },
    HostCommand {
        id: "page.unplace",
        kind: Kind::Action,
        effect: Effect::Destructive,
        space: false,
        description: "Take a widget off a page. Its plugin's data is not touched.",
        confirm: Some("Take this widget off the page?"),
        params: &[
            ("page", ParamType::Id, true, "Page id"),
            ("placement", ParamType::Id, true, "Placement id, from page.get"),
        ],
        run: |host, _, args, d| {
            edit_placements(host, args, d, |list, args| {
                let id = text(args, "placement")?;
                let before = list.len();
                list.retain(|p| p.id != id);
                if list.len() == before {
                    return Err(CommandError::new(
                        "not-found",
                        format!("no placement `{id}` on this page"),
                    ));
                }
                Ok(())
            })
        },
    },
    HostCommand {
        id: "page.remove",
        kind: Kind::Action,
        effect: Effect::Destructive,
        space: false,
        description: "Remove a page from every device. The Home page can't be removed.",
        confirm: Some("Remove this page from every device?"),
        params: &[("page", ParamType::Id, true, "Page id")],
        run: |host, _, args, d| service(host, "page_remove", None, args, d),
    },
    HostCommand {
        id: "search.query",
        kind: Kind::Query,
        effect: Effect::Read,
        space: true,
        description: "Search every plugin that offers search, in one space or all of them.",
        confirm: None,
        params: &[
            ("q", ParamType::String, true, "Text to look for"),
            ("all_spaces", ParamType::Boolean, false, "Search every space, not only this one"),
            ("plugins", ParamType::Json, false, "Plugin ids to search; all by default"),
            ("limit", ParamType::Integer, false, "Most results to return (default 200, at most 500)"),
        ],
        run: |host, space, args, _| run_search(host, space, args),
    },
    HostCommand {
        id: "ui.open",
        kind: Kind::Action,
        effect: Effect::Write,
        space: true,
        description: "Show a plugin's page in the app window, optionally with one item open.",
        confirm: None,
        params: &[
            ("plugin", ParamType::String, true, "Plugin id"),
            ("page", ParamType::String, false, "Page id; the plugin's first page by default"),
            ("item", ParamType::Id, false, "Item to open, e.g. a search hit's id"),
        ],
        run: |_, _, _, _| Err(app_only()),
    },
    HostCommand {
        id: "ui.notify",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Show a short message in the app window.",
        confirm: None,
        params: &[("text", ParamType::String, true, "The message, at most 200 characters")],
        run: |_, _, _, _| Err(app_only()),
    },
    HostCommand {
        id: "schedule.list",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Scheduled runs set up by plugins, with recent runs and settings.",
        confirm: None,
        params: &[],
        run: |host, _, args, d| service(host, "schedules", None, args, d),
    },
    HostCommand {
        id: "schedule.runs",
        kind: Kind::Query,
        effect: Effect::Read,
        space: false,
        description: "Recent runs of one schedule, newest first.",
        confirm: None,
        params: &[("id", ParamType::Id, true, "Schedule id")],
        run: |host, _, args, d| service(host, "schedule_runs", None, args, d),
    },
    HostCommand {
        id: "schedule.run_now",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Run a schedule now.",
        confirm: None,
        params: &[("id", ParamType::Id, true, "Schedule id")],
        run: |host, _, args, d| service(host, "schedule_run_now", None, args, d),
    },
    HostCommand {
        id: "schedule.enable",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Turn a schedule on.",
        confirm: None,
        params: &[("id", ParamType::Id, true, "Schedule id")],
        run: |host, _, args, d| {
            let mut args = args.clone();
            args.insert("enabled".into(), json!(true));
            service(host, "schedule_set_enabled", None, &args, d)
        },
    },
    HostCommand {
        id: "schedule.disable",
        kind: Kind::Action,
        effect: Effect::Write,
        space: false,
        description: "Turn a schedule off.",
        confirm: None,
        params: &[("id", ParamType::Id, true, "Schedule id")],
        run: |host, _, args, d| {
            let mut args = args.clone();
            args.insert("enabled".into(), json!(false));
            service(host, "schedule_set_enabled", None, &args, d)
        },
    },
];

impl HostCommand {
    fn info(&self) -> CommandInfo {
        let params = self
            .params
            .iter()
            .map(|(name, ty, required, description)| {
                let mut spec = ParamSpec::new(*ty);
                spec.required = *required;
                spec.description = (*description).into();
                ((*name).to_string(), spec)
            })
            .collect();
        CommandInfo {
            id: self.id.into(),
            kind: self.kind,
            owner: Owner::Host,
            plugin: None,
            summary: summary(self.description),
            description: self.description.into(),
            params: Some(params),
            context: Vec::new(),
            result: None,
            effect: self.effect,
            confirm: self.confirm.map(str::to_string),
            space: self.space,
            examples: Vec::new(),
            hidden: false,
        }
    }
}

// ---- Plugin commands ------------------------------------------------------

/// Operations only the app may call: marked `cli: false`, and AI
/// callbacks (named `on_…`). Job targets stay visible: some, like Reports'
/// `generate`, are also things people run themselves; a plugin marks the
/// purely internal ones `cli: false`.
fn operation_hidden(m: &Manifest, op: &str) -> bool {
    m.operations.get(op).is_some_and(|d| !d.cli) || op.starts_with("on_")
}

fn operation_info(m: &Manifest, name: &str) -> Option<CommandInfo> {
    let op = m.operations.get(name)?;
    Some(CommandInfo {
        id: format!("{}.{name}", m.id),
        kind: Kind::Action,
        owner: Owner::Plugin,
        plugin: Some(m.id.clone()),
        summary: summary(&op.description),
        description: op.description.clone(),
        params: op.params.clone(),
        context: Vec::new(),
        result: op.result.clone(),
        effect: op.effect_of(name),
        confirm: op.confirm.clone(),
        space: true,
        examples: op.examples.clone(),
        hidden: operation_hidden(m, name),
    })
}

fn query_info(m: &Manifest, name: &str) -> Option<CommandInfo> {
    let q = m.queries.get(name)?;
    let context: Vec<String> = q
        .params
        .iter()
        .filter(|p| CONTEXT_PARAMS.contains(&p.as_str()))
        .cloned()
        .collect();
    // Every other parameter is listed; untyped ones as `json`.
    let params = q
        .params
        .iter()
        .filter(|p| !context.contains(p))
        .map(|p| {
            let spec = q
                .types
                .get(p)
                .cloned()
                .unwrap_or_else(|| ParamSpec::new(ParamType::Json));
            (p.clone(), spec)
        })
        .collect();
    Some(CommandInfo {
        id: format!("{}.{name}", m.id),
        kind: Kind::Query,
        owner: Owner::Plugin,
        plugin: Some(m.id.clone()),
        summary: summary(&q.description),
        description: q.description.clone(),
        params: Some(params),
        context,
        result: None,
        effect: Effect::Read,
        confirm: None,
        space: true,
        examples: Vec::new(),
        hidden: !q.cli,
    })
}

fn plugin_commands(m: &Manifest) -> impl Iterator<Item = CommandInfo> + '_ {
    let ops = m.operations.keys().filter_map(|n| operation_info(m, n));
    let queries = m.queries.keys().filter_map(|n| query_info(m, n));
    ops.chain(queries)
}

// ---- Registry ---------------------------------------------------------------

/// Every command `caller` may run: host commands first, then each
/// plugin's actions and queries. The app sees hidden commands too; a
/// read-only client sees only `read` commands.
pub fn describe(host: &PluginHost, caller: Caller) -> Vec<CommandInfo> {
    let mut out: Vec<CommandInfo> = HOST_COMMANDS.iter().map(HostCommand::info).collect();
    for m in host.manifests() {
        out.extend(plugin_commands(m));
    }
    match caller {
        Caller::App => {}
        Caller::Client(Scope::Change) => out.retain(|c| !c.hidden),
        Caller::Client(Scope::Read) => out.retain(|c| !c.hidden && c.effect == Effect::Read),
    }
    out
}

fn find(host: &PluginHost, kind: Kind, id: &str) -> Option<CommandInfo> {
    if let Some(c) = HOST_COMMANDS.iter().find(|c| c.kind == kind && c.id == id) {
        return Some(c.info());
    }
    let (plugin, name) = id.split_once('.')?;
    let m = host.manifests().find(|m| m.id == plugin)?;
    match kind {
        Kind::Action => operation_info(m, name),
        Kind::Query => query_info(m, name),
    }
}

/// Checks outside callers' arguments against `params`.
fn check_args(
    params: &BTreeMap<String, ParamSpec>,
    context: &[String],
    args: &Map<String, Value>,
) -> Result<(), CommandError> {
    if let Some(key) = args
        .keys()
        .find(|k| !params.contains_key(*k) && !context.contains(k))
    {
        return Err(CommandError::invalid(format!("unknown argument `{key}`")));
    }
    for (name, spec) in params {
        match args.get(name) {
            None | Some(Value::Null) if spec.required => {
                return Err(CommandError::invalid(format!("missing `{name}`")))
            }
            None | Some(Value::Null) => {}
            Some(v) => check_value(name, spec, v)?,
        }
    }
    Ok(())
}

fn check_value(name: &str, spec: &ParamSpec, v: &Value) -> Result<(), CommandError> {
    let bad = |what: &str| Err(CommandError::invalid(format!("`{name}` must be {what}")));
    // An empty string clears an optional field.
    let blank = !spec.required && v.as_str() == Some("");
    match spec.ty {
        ParamType::String | ParamType::Text => {
            if !v.is_string() {
                return bad("text");
            }
        }
        ParamType::Id => match v.as_str() {
            Some(s) if !s.trim().is_empty() => {}
            _ => return bad("an id"),
        },
        ParamType::Boolean => {
            if !v.is_boolean() {
                return bad("true or false");
            }
        }
        ParamType::Integer | ParamType::Number => {
            let n = if spec.ty == ParamType::Integer {
                v.as_i64().map(|n| n as f64)
            } else {
                v.as_f64()
            };
            let Some(n) = n else {
                return bad(if spec.ty == ParamType::Integer {
                    "a whole number"
                } else {
                    "a number"
                });
            };
            if spec.min.is_some_and(|lo| n < lo) || spec.max.is_some_and(|hi| n > hi) {
                let lo = spec
                    .min
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "…".into());
                let hi = spec
                    .max
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "…".into());
                return bad(&format!("between {lo} and {hi}"));
            }
        }
        ParamType::Date if !blank => match v.as_str().map(crate::time::date) {
            Some(Ok(_)) => {}
            _ => return bad("a date like 2026-11-01"),
        },
        ParamType::Time if !blank => match v.as_str().map(crate::time::time) {
            Some(Ok(_)) => {}
            _ => return bad("a time like 07:30"),
        },
        ParamType::Enum if !blank => match v.as_str() {
            Some(s) if spec.values.iter().any(|x| x == s) => {}
            _ => return bad(&format!("one of {}", spec.values.join(", "))),
        },
        ParamType::Date | ParamType::Time | ParamType::Enum | ParamType::Json => {}
    }
    Ok(())
}

/// Fills in the clock parameters a query takes and the caller left out.
fn fill_context(host: &PluginHost, context: &[String], values: &mut Map<String, Value>) {
    let now = now_ms();
    let tz = host.timezone().to_string();
    for name in context {
        if values.get(name).is_some_and(|v| !v.is_null()) {
            continue;
        }
        let value = match name.as_str() {
            "today" => crate::time::today(now, &tz).map(Value::from).ok(),
            "weekday" => crate::time::today(now, &tz)
                .and_then(|d| crate::time::weekday(&d))
                .map(Value::from)
                .ok(),
            "now_ms" => Some(json!(now)),
            "utc_offset_ms" => crate::time::utc_offset_ms(now, &tz).map(Value::from).ok(),
            "tz" => Some(json!(tz)),
            _ => None,
        };
        if let Some(value) = value {
            values.insert(name.clone(), value);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How an outside caller wants a command run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct InvokeOptions {
    /// Run a plugin action and report what it would change, keeping
    /// nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// A person approved a destructive command (its `confirm` text).
    #[serde(default)]
    pub confirmed: bool,
}

/// A command's result, with the changes a plugin action made (or would
/// make, on a dry run). Changes are listed only for outside callers.
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub result: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<ChangeSummary>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

/// Runs a command. `space` defaults to the default space. `desktop`:
/// whether this device is a computer (for running schedules now).
pub fn invoke(
    host: &mut PluginHost,
    caller: Caller,
    kind: Kind,
    id: &str,
    space: Option<&str>,
    args: &Value,
    desktop: bool,
) -> Result<Value, CommandError> {
    let options = InvokeOptions {
        dry_run: false,
        confirmed: true,
    };
    invoke_with(host, caller, kind, id, space, args, desktop, options).map(|o| o.result)
}

/// [`invoke`] with a dry run or confirmation. An outside caller must set
/// `confirmed` to run a destructive action for real.
#[allow(clippy::too_many_arguments)]
pub fn invoke_with(
    host: &mut PluginHost,
    caller: Caller,
    kind: Kind,
    id: &str,
    space: Option<&str>,
    args: &Value,
    desktop: bool,
    options: InvokeOptions,
) -> Result<Outcome, CommandError> {
    let space = space.unwrap_or(DEFAULT_SPACE);
    let info = find(host, kind, id).ok_or_else(|| CommandError::not_found(kind, id))?;
    let args = match args {
        Value::Null => Map::new(),
        Value::Object(map) => map.clone(),
        _ => return Err(CommandError::invalid("arguments must be an object")),
    };
    if let Caller::Client(scope) = caller {
        if info.hidden {
            return Err(CommandError::not_found(kind, id));
        }
        if scope == Scope::Read && info.effect != Effect::Read {
            return Err(CommandError::new(
                "forbidden",
                format!("`{id}` changes data; this client may only read"),
            ));
        }
        if let Some(params) = &info.params {
            check_args(params, &info.context, &args)?;
        }
        if info.effect == Effect::Destructive && !options.confirmed && !options.dry_run {
            let message = info
                .confirm
                .clone()
                .unwrap_or_else(|| format!("`{id}` removes or hides data. Confirm to run it."));
            return Err(CommandError::new("needs-confirmation", message));
        }
    }
    let plain = |result: Value| Outcome {
        result,
        changes: Vec::new(),
        dry_run: false,
    };
    if options.dry_run && kind == Kind::Action && info.owner == Owner::Host {
        return Err(CommandError::invalid(
            "A dry run works only for plugin actions.",
        ));
    }
    match (info.owner, kind) {
        (Owner::Host, _) if id == "app.describe" => Ok(plain(json!(describe(host, caller)))),
        (Owner::Host, _) => {
            let command = HOST_COMMANDS
                .iter()
                .find(|c| c.kind == kind && c.id == id)
                .expect("found above");
            (command.run)(host, space, &args, desktop).map(plain)
        }
        (Owner::Plugin, Kind::Query) => {
            let mut values = args;
            fill_context(host, &info.context, &mut values);
            let plugin = info.plugin.as_deref().unwrap_or_default();
            let name = &id[plugin.len() + 1..];
            Ok(plain(json!(host.query_in(space, plugin, name, &values)?)))
        }
        (Owner::Plugin, Kind::Action) => {
            let plugin = info.plugin.as_deref().unwrap_or_default();
            let op = &id[plugin.len() + 1..];
            let args = Value::Object(args);
            if caller == Caller::App && !options.dry_run {
                return Ok(plain(host.call_in(space, plugin, op, &args)?));
            }
            let (result, changes) = host.call_traced(space, plugin, op, &args, options.dry_run)?;
            Ok(Outcome {
                result,
                changes,
                dry_run: options.dry_run,
            })
        }
    }
}

// ---- Search -----------------------------------------------------------------

fn run_search(
    host: &mut PluginHost,
    space: &str,
    args: &Map<String, Value>,
) -> Result<Value, CommandError> {
    let q = text(args, "q")?.trim().to_string();
    if q.is_empty() {
        return Err(CommandError::invalid("`q` must not be empty"));
    }
    let spaces: Vec<String> = if args.get("all_spaces").and_then(Value::as_bool) == Some(true) {
        host.open_spaces().map(str::to_string).collect()
    } else {
        vec![space.to_string()]
    };
    let only: Option<Vec<String>> = match args.get("plugins") {
        None | Some(Value::Null) => None,
        Some(Value::Array(ids)) => Some(
            ids.iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or_else(|| CommandError::invalid("`plugins` must be a list of plugin ids"))?,
        ),
        Some(_) => {
            return Err(CommandError::invalid(
                "`plugins` must be a list of plugin ids",
            ))
        }
    };
    let limit = match args.get("limit") {
        None | Some(Value::Null) => SEARCH_LIMIT,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=SEARCH_MAX as u64).contains(n))
            .ok_or_else(|| CommandError::invalid(format!("`limit` must be 1 to {SEARCH_MAX}")))?
            as usize,
    };
    Ok(search(host, &q, &spaces, only.as_deref(), limit))
}

/// Runs each plugin's search query in each of `spaces`. A plugin that
/// fails is reported in `errors`; the others still answer.
pub fn search(
    host: &PluginHost,
    q: &str,
    spaces: &[String],
    plugins: Option<&[String]>,
    limit: usize,
) -> Value {
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let params = Map::from_iter([("q".to_string(), json!(q))]);
    'plugins: for m in host.manifests() {
        let Some(query) = &m.search else { continue };
        if plugins.is_some_and(|only| !only.contains(&m.id)) {
            continue;
        }
        let page = m
            .open
            .as_ref()
            .map(|o| o.page.clone())
            .or_else(|| m.pages.first().map(|p| p.id.clone()));
        for space in spaces {
            match host.query_in(space, &m.id, query, &params) {
                Ok(rows) => {
                    for row in rows {
                        if results.len() >= limit {
                            break 'plugins;
                        }
                        let id = row.get("id").cloned().unwrap_or(Value::Null);
                        results.push(json!({
                            "space": space,
                            "plugin": m.id,
                            "id": id,
                            "title": row.get("title").cloned().unwrap_or(json!("")),
                            "subtitle": row.get("subtitle").cloned().unwrap_or(Value::Null),
                            "open": page.as_ref().map(|p| json!({ "plugin": m.id, "page": p, "item": id })),
                        }));
                    }
                }
                Err(e) => errors.push(json!({
                    "space": space,
                    "plugin": m.id,
                    "message": e.to_string(),
                })),
            }
        }
    }
    json!({ "results": results, "errors": errors })
}
