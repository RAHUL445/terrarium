//! Running one operation on many items: `plugin.run_bulk`.
use std::collections::HashSet;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::commands::{self, Caller, CommandError, InvokeOptions, Kind, Outcome};
use crate::manifest::Effect;
use crate::registry::{ChangeSummary, PluginHost};

pub(crate) const BULK_COMMAND: &str = "plugin.run_bulk";
/// Most items one bulk command may touch.
pub(crate) const MAX_ITEMS: usize = 500;

/// The outcome for one input item.
#[derive(Clone, Debug, Serialize)]
pub struct ItemResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Item id, or None for import rows (EXIM-04a).
    pub id: Option<String>,
    /// Position in the input, starting at zero.
    pub index: usize,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

/// Counts and per-item outcomes for a bulk command.
#[derive(Clone, Debug, Serialize)]
pub struct BulkReport {
    pub plugin: String,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub message: String,
    pub items: Vec<ItemResult>,
}

/// The ids of the rows a declared query returns, for `where`.
fn ids_where(
    host: &PluginHost,
    caller: Caller,
    space: &str,
    plugin: &str,
    spec: &Value,
) -> Result<Vec<String>, CommandError> {
    let invalid = || CommandError::invalid("`where` must be {query, params}");
    let Some(where_obj) = spec.as_object() else {
        return Err(invalid());
    };
    let Some(query) = where_obj.get("query").and_then(Value::as_str) else {
        return Err(invalid());
    };
    let mut params = match where_obj.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        _ => return Err(invalid()),
    };
    let name = format!("{plugin}.{query}");
    let manifest = host
        .manifest(plugin)
        .ok_or_else(|| CommandError::not_found(Kind::Query, &name))?;
    let info = commands::query_info(manifest, query)
        .ok_or_else(|| CommandError::not_found(Kind::Query, &name))?;
    if matches!(caller, Caller::Client(_)) {
        if info.hidden {
            return Err(CommandError::not_found(Kind::Query, &name));
        }
        if let Some(param_specs) = info.params.as_ref() {
            commands::check_args(param_specs, &info.context, &params)?;
        }
    }
    commands::fill_context(host, &info.context, &mut params);
    let rows = host.query_in(space, plugin, query, &params)?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            return Err(CommandError::invalid(format!(
                "query `{query}` returns rows without `id`"
            )));
        };
        let id = id.to_owned();
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Each call commits independently. Failed calls do not stop later calls.
/// Dry runs are independent too: later calls cannot see earlier rolled-back writes.
pub(crate) fn run_each(
    host: &mut PluginHost,
    space: &str,
    plugin: &str,
    op: &str,
    calls: Vec<(Option<String>, Value)>,
    trace: bool,
    dry_run: bool,
) -> (Vec<ItemResult>, Vec<ChangeSummary>) {
    let mut changes = Vec::new();
    let items = calls
        .into_iter()
        .enumerate()
        .map(|(index, (id, args))| {
            let result = if trace || dry_run {
                host.call_traced(space, plugin, op, &args, dry_run)
                    .map(|(value, delta)| {
                        changes.extend(delta);
                        value
                    })
            } else {
                host.call_in(space, plugin, op, &args)
            };
            match result {
                Ok(value) => ItemResult {
                    id,
                    index,
                    ok: true,
                    result: (!value.is_null()).then_some(value),
                    error: None,
                },
                Err(error) => ItemResult {
                    id,
                    index,
                    ok: false,
                    result: None,
                    error: Some(error.into()),
                },
            }
        })
        .collect();
    (items, changes)
}

pub(crate) fn summary(done: usize, failed: usize, dry_run: bool) -> String {
    if dry_run {
        let failures = if failed == 0 {
            String::new()
        } else {
            format!(", {failed} would fail")
        };
        format!("Dry run: {done} would be done{failures}. Nothing was kept.")
    } else if failed == 0 {
        format!("{done} done")
    } else {
        format!("{done} done, {failed} failed")
    }
}

pub(crate) fn invoke(
    host: &mut PluginHost,
    caller: Caller,
    space: &str,
    args: &Map<String, Value>,
    options: InvokeOptions,
) -> Result<Outcome, CommandError> {
    let plugin = commands::text(args, "plugin")?;
    let op = commands::text(args, "op")?;
    let dry_run = options.dry_run || args.get("dry_run") == Some(&Value::Bool(true));
    let missing = || CommandError::not_found(Kind::Action, &format!("{plugin}.{op}"));
    let manifest = host.manifest(plugin).ok_or_else(missing)?;
    let operation = manifest.operations.get(op).ok_or_else(missing)?;
    if matches!(caller, Caller::Client(_)) && commands::operation_hidden(manifest, op) {
        return Err(missing());
    }
    if !operation.bulk {
        return Err(CommandError::invalid(format!(
            "`{plugin}.{op}` can't run on many items"
        )));
    }
    let params = operation.params.clone();
    let many = operation.bulk_op.clone();
    let confirm = operation.confirm.clone();
    let destructive = operation.effect_of(op) == Effect::Destructive;
    let id_spec = args.get("ids").filter(|v| !v.is_null());
    let where_spec = args.get("where").filter(|v| !v.is_null());
    if id_spec.is_some() && where_spec.is_some() {
        return Err(CommandError::invalid("give `ids` or `where`, not both"));
    }
    if id_spec.is_none() && where_spec.is_none() {
        return Err(CommandError::invalid("give `ids` or `where`"));
    }
    let (ids, used_query) = if let Some(raw_ids) = id_spec {
        let raw_ids = raw_ids
            .as_array()
            .ok_or_else(|| CommandError::invalid("`ids` must be a list of item ids"))?;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for id in raw_ids {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CommandError::invalid("`ids` must be a list of item ids"))?;
            if seen.insert(id) {
                ids.push(id.to_owned());
            }
        }
        if ids.is_empty() {
            return Err(CommandError::invalid("`ids` must list at least one item"));
        }
        if ids.len() > MAX_ITEMS {
            return Err(CommandError::invalid(format!(
                "at most 500 items at a time; got {}",
                ids.len()
            )));
        }
        (ids, None)
    } else if let Some(spec) = where_spec {
        let query = spec
            .as_object()
            .and_then(|obj| obj.get("query"))
            .and_then(Value::as_str)
            .ok_or_else(|| CommandError::invalid("`where` must be {query, params}"))?
            .to_owned();
        let ids = ids_where(host, caller, space, plugin, spec)?;
        if ids.is_empty() {
            return Err(CommandError::invalid("`where` matches nothing"));
        }
        if ids.len() > MAX_ITEMS {
            return Err(CommandError::invalid(format!(
                "`where` matches {} items; at most 500 at a time",
                ids.len()
            )));
        }
        (ids, Some(query))
    } else {
        unreachable!("checked mutually exclusive ids/where");
    };
    let extra = match args.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        _ => return Err(CommandError::invalid("`args` must be an object")),
    };
    if extra.contains_key("id") || extra.contains_key("ids") {
        return Err(CommandError::invalid(
            "`args` must not contain `id` or `ids`",
        ));
    }
    if let (Caller::Client(_), Some(params)) = (caller, params) {
        let mut item = extra.clone();
        item.insert("id".into(), json!(ids[0]));
        commands::check_args(&params, &[], &item)?;
    }
    if matches!(caller, Caller::Client(_)) && destructive && !dry_run && !options.confirmed {
        let message = confirm.unwrap_or_else(|| format!("`{plugin}.{op}` removes or hides data."));
        let noun = if ids.len() == 1 { "item" } else { "items" };
        return Err(CommandError::new(
            "needs-confirmation",
            format!("{message} This applies to {} {noun}.", ids.len()),
        ));
    }
    let trace = caller != Caller::App || dry_run;
    let (items, changes) = if let Some(many) = many {
        let mut args = extra;
        args.insert("ids".into(), json!(ids));
        let (mut results, changes) = run_each(
            host,
            space,
            plugin,
            &many,
            vec![(None, json!(args))],
            trace,
            dry_run,
        );
        let single = results.remove(0);
        let items = ids
            .iter()
            .enumerate()
            .map(|(index, id)| ItemResult {
                id: Some(id.clone()),
                index,
                ok: single.ok,
                result: if index == 0 {
                    single.result.clone()
                } else {
                    None
                },
                error: single.error.clone(),
            })
            .collect::<Vec<_>>();
        (items, changes)
    } else {
        let calls = ids
            .iter()
            .map(|id| {
                let mut args = extra.clone();
                args.insert("id".into(), json!(id));
                (Some(id.clone()), json!(args))
            })
            .collect();
        run_each(host, space, plugin, op, calls, trace, dry_run)
    };
    let done = items.iter().filter(|item| item.ok).count();
    let failed = items.len() - done;
    let report = BulkReport {
        plugin: plugin.into(),
        op: op.into(),
        query: used_query,
        total: items.len(),
        done,
        failed,
        message: summary(done, failed, dry_run),
        items,
    };
    Ok(Outcome {
        result: json!(report),
        changes,
        dry_run,
    })
}
