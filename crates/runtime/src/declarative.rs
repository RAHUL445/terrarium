//! Bounded host-side store writes. No guest, SQL, or expression evaluation.
use crate::{
    error::{Result, RuntimeError},
    manifest::{Effect, Kind, Manifest, ParamSpec, ParamType},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use terrarium_core::{
    ColumnDef, ColumnType, DeletePolicy, MergeRule, Store as CoreStore, TableKind,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Step {
    Insert {
        insert: String,
        values: Map<String, Value>,
    },
    Update {
        update: String,
        row: Value,
        values: Map<String, Value>,
    },
    Delete {
        delete: String,
        row: Value,
    },
    Restore {
        restore: String,
        row: Value,
    },
    Increment {
        increment: String,
        row: Value,
        column: String,
        delta: Value,
    },
}
impl Step {
    fn table(&self) -> &str {
        match self {
            Self::Insert { insert, .. } => insert,
            Self::Update { update, .. } => update,
            Self::Delete { delete, .. } => delete,
            Self::Restore { restore, .. } => restore,
            Self::Increment { increment, .. } => increment,
        }
    }
    fn row(&self) -> Option<&Value> {
        match self {
            Self::Insert { .. } => None,
            Self::Update { row, .. }
            | Self::Delete { row, .. }
            | Self::Restore { row, .. }
            | Self::Increment { row, .. } => Some(row),
        }
    }
}
fn invalid(message: &str) -> RuntimeError {
    RuntimeError::Plugin {
        code: "invalid".into(),
        message: message.into(),
    }
}
enum Binding<'a> {
    Arg(&'a str),
    Now,
    Today,
    Literal,
}
fn binding(v: &Value) -> std::result::Result<Binding<'_>, &'static str> {
    match v {
        Value::Array(_) | Value::Object(_) => Err("write values must be scalars"),
        Value::String(s) if s == "{{$now_ms}}" => Ok(Binding::Now),
        Value::String(s) if s == "{{$today}}" => Ok(Binding::Today),
        Value::String(s) if s.starts_with("{{args.") && s.ends_with("}}") => {
            let n = &s[7..s.len() - 2];
            let mut chars = n.chars();
            if chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                Ok(Binding::Arg(n))
            } else {
                Err("invalid argument binding")
            }
        }
        Value::String(s) if s.contains("{{") || s.contains("}}") => Err("invalid binding"),
        _ => Ok(Binding::Literal),
    }
}
fn compatible(ty: ParamType, col: ColumnType) -> bool {
    match col {
        ColumnType::Text => matches!(
            ty,
            ParamType::String
                | ParamType::Text
                | ParamType::Id
                | ParamType::Date
                | ParamType::Time
                | ParamType::Enum
        ),
        ColumnType::File => matches!(ty, ParamType::String | ParamType::Text),
        ColumnType::Integer => ty == ParamType::Integer,
        ColumnType::Real => matches!(ty, ParamType::Integer | ParamType::Number),
        ColumnType::Boolean => ty == ParamType::Boolean,
    }
}
fn check_binding(
    v: &Value,
    params: &BTreeMap<String, ParamSpec>,
    column: &ColumnDef,
    table: &str,
    delta: bool,
) -> std::result::Result<(), String> {
    let ty = match binding(v)? {
        Binding::Arg(n) => {
            let p = params.get(n).ok_or("undeclared argument binding")?;
            if delta && !p.required {
                return Err("delta must be required".into());
            }
            Some(p.ty)
        }
        Binding::Now => Some(ParamType::Integer),
        Binding::Today => Some(ParamType::Date),
        Binding::Literal => {
            column
                .check_value(table, v)
                .map_err(|_| "invalid literal value")?;
            if delta && v.is_null() {
                return Err("delta cannot be null".into());
            }
            None
        }
    };
    if ty.is_some_and(|t| !compatible(t, column.ty)) {
        return Err("incompatible argument binding".into());
    }
    Ok(())
}
pub(crate) fn validate(m: &Manifest) -> Result<()> {
    let check = || -> std::result::Result<(), String> {
        if m.kind == Kind::Declarative
            && (!m.permissions.is_empty()
                || !m.uses.is_empty()
                || !m.provides.is_empty()
                || !m.jobs.is_empty()
                || m.reminders.is_some())
        {
            return Err("declarative plugins cannot declare capabilities or schedules".into());
        }
        for (name, op) in &m.operations {
            if m.kind == Kind::Wasm {
                if op.steps.is_some() {
                    return Err("Wasm operations cannot declare do".into());
                }
                continue;
            }
            let params = op
                .params
                .as_ref()
                .ok_or("declarative operations require params")?;
            let steps = op
                .steps
                .as_ref()
                .ok_or("declarative operations require do")?;
            if !(1..=16).contains(&steps.len()) {
                return Err("do must contain 1–16 steps".into());
            }
            let insert = steps.iter().any(|s| matches!(s, Step::Insert { .. }));
            if insert && steps.len() != 1 {
                return Err("insert must be the only step".into());
            }
            if op.effect_of(name) == Effect::Read
                || (steps.iter().any(|s| matches!(s, Step::Delete { .. }))
                    && op.effect_of(name) != Effect::Destructive)
            {
                return Err("invalid declarative operation effect".into());
            }
            if let Some(result) = &op.result {
                if insert {
                    if result.len() != 1
                        || !result.get("id").is_some_and(|s| {
                            s.ty == ParamType::Id
                                && s.required
                                && s.values.is_empty()
                                && s.min.is_none()
                                && s.max.is_none()
                        })
                    {
                        return Err("insert result must be a required id".into());
                    }
                } else if !result.is_empty() {
                    return Err("non-insert result must be empty".into());
                }
            }
            for example in &op.examples {
                crate::commands::check_args(
                    params,
                    &[],
                    example.as_object().ok_or("example must be an object")?,
                )
                .map_err(|_| "invalid operation example")?;
            }
            for step in steps {
                let table = m
                    .storage
                    .table(step.table())
                    .map_err(|_| "undeclared step table")?;
                if let Some(row) = step.row() {
                    if table.kind == TableKind::Append {
                        return Err("append tables only support insert".into());
                    }
                    match binding(row)? {
                        Binding::Arg(n)
                            if params
                                .get(n)
                                .is_some_and(|p| p.required && p.ty == ParamType::Id) => {}
                        Binding::Literal if row.as_str().is_some_and(|s| !s.trim().is_empty()) => {}
                        _ => return Err("row requires a literal id or required id argument".into()),
                    }
                }
                if matches!(step, Step::Restore { .. })
                    && table.delete_policy == DeletePolicy::DeleteWins
                {
                    return Err("delete-wins tables cannot restore".into());
                }
                match step {
                    Step::Insert { values, .. } | Step::Update { values, .. } => {
                        if values.is_empty() {
                            return Err("values must not be empty".into());
                        }
                        for (name, v) in values {
                            let c = table.column(name).map_err(|_| "undeclared write column")?;
                            if matches!(step, Step::Update { .. }) && c.rule == MergeRule::Counter {
                                return Err("update cannot set counters".into());
                            }
                            check_binding(v, params, c, &table.name, false)?;
                        }
                    }
                    Step::Increment { column, delta, .. } => {
                        let c = table.column(column).map_err(|_| "undeclared counter")?;
                        if c.rule != MergeRule::Counter {
                            return Err("increment requires a counter".into());
                        }
                        check_binding(delta, params, c, &table.name, true)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    };
    check().map_err(RuntimeError::Manifest)
}
fn resolve(v: &Value, args: &Map<String, Value>, now: u64, tz: &str) -> Result<Option<Value>> {
    Ok(match binding(v).map_err(invalid)? {
        Binding::Arg(n) => args.get(n).cloned(),
        Binding::Literal => Some(v.clone()),
        Binding::Now => Some(Value::from(
            i64::try_from(now).map_err(|_| invalid("clock out of range"))?,
        )),
        Binding::Today => Some(Value::String(
            crate::time::today(now, tz).map_err(|_| invalid("invalid clock or timezone"))?,
        )),
    })
}
pub(crate) fn run(
    store: &mut CoreStore,
    manifest: &Manifest,
    op: &str,
    args: &Map<String, Value>,
    now_ms: u64,
    tz: &str,
) -> Result<Value> {
    i64::try_from(now_ms).map_err(|_| invalid("clock out of range"))?;
    let op = &manifest.operations[op];
    crate::commands::check_args(
        op.params
            .as_ref()
            .ok_or_else(|| invalid("missing params"))?,
        &[],
        args,
    )
    .map_err(|_| invalid("invalid operation arguments"))?;
    let steps = op.steps.as_ref().ok_or_else(|| invalid("missing steps"))?;
    if !(1..=16).contains(&steps.len()) {
        return Err(invalid("invalid step count"));
    }
    let started = std::time::Instant::now();
    let mut bytes = 0;
    let mut result = Value::Null;
    for step in steps {
        if started.elapsed().as_millis() >= u128::from(manifest.limits.timeout_ms) {
            return Err(RuntimeError::Trap("it ran past its time limit".into()));
        }
        let table = step.table();
        let row = step
            .row()
            .map(|v| resolve(v, args, now_ms, tz))
            .transpose()?
            .flatten();
        let row = match row {
            Some(Value::String(s)) => s,
            None if step.row().is_none() => String::new(),
            _ => return Err(invalid("invalid row id")),
        };
        let mut values = Map::new();
        if let Step::Insert { values: plan, .. } | Step::Update { values: plan, .. } = step {
            for (k, v) in plan {
                if let Some(v) = resolve(v, args, now_ms, tz)? {
                    values.insert(k.clone(), v);
                }
            }
            if values.is_empty() && matches!(step, Step::Update { .. }) {
                return Err(invalid("no write fields supplied"));
            }
        }
        let (column, delta) = if let Step::Increment { column, delta, .. } = step {
            (
                column.as_str(),
                resolve(delta, args, now_ms, tz)?
                    .filter(|v| !v.is_null())
                    .ok_or_else(|| invalid("missing delta"))?,
            )
        } else {
            ("", Value::Null)
        };
        let payload = match step {
            Step::Insert { .. } | Step::Update { .. } => {
                serde_json::to_string(&values).expect("JSON")
            }
            Step::Increment { .. } => delta.to_string(),
            _ => String::new(),
        };
        crate::host::charge_write(
            &mut bytes,
            table
                .len()
                .saturating_add(row.len())
                .saturating_add(column.len())
                .saturating_add(payload.len()),
        )
        .map_err(crate::host::plugin_error)?;
        result = store
            .write(|w| match step {
                Step::Insert { .. } => w
                    .insert(table, &values)
                    .map(|id| serde_json::json!({"id":id})),
                Step::Update { .. } => w.update(table, &row, &values).map(|_| Value::Null),
                Step::Delete { .. } => w.delete(table, &row).map(|_| Value::Null),
                Step::Restore { .. } => w.restore(table, &row).map(|_| Value::Null),
                Step::Increment { .. } => {
                    w.increment(table, &row, column, delta).map(|_| Value::Null)
                }
            })
            .map_err(|e| crate::host::plugin_error(crate::host::to_plugin_error(e)))?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Manifest {
        Manifest::parse(include_bytes!(
            "../tests/fixtures/declarative/manifest.json"
        ))
        .unwrap()
    }
    fn store(m: &Manifest) -> CoreStore {
        CoreStore::open(
            ":memory:",
            terrarium_core::StoreConfig {
                space: "test".into(),
                plugin: m.id.clone(),
                device: "test".into(),
                user: "test".into(),
            },
            m.storage.clone(),
            Box::new(terrarium_core::SystemTime),
        )
        .unwrap()
    }
    #[test]
    fn one_clock_instant_local_midnight_and_dst() {
        for (utc, tz, day) in [
            ("2026-11-01T20:00:00Z", "Asia/Kolkata", "2026-11-02"),
            ("2026-11-01T05:59:59Z", "America/New_York", "2026-11-01"),
            ("2026-11-01T06:00:00Z", "America/New_York", "2026-11-01"),
            ("2026-03-08T07:00:00Z", "America/New_York", "2026-03-08"),
        ] {
            let now = utc.parse::<jiff::Timestamp>().unwrap().as_millisecond() as u64;
            assert_eq!(
                resolve(&json!("{{$today}}"), &Map::new(), now, tz).unwrap(),
                Some(json!(day))
            );
            assert_eq!(
                resolve(&json!("{{$now_ms}}"), &Map::new(), now, tz).unwrap(),
                Some(json!(now))
            );
        }
        assert!(resolve(&json!("{{$now_ms}}"), &Map::new(), u64::MAX, "UTC").is_err());
        let m = fixture();
        assert!(run(
            &mut store(&m),
            &m,
            "create",
            json!({"title":"x"}).as_object().unwrap(),
            u64::MAX,
            "UTC"
        )
        .is_err());
    }
    #[test]
    fn omitted_insert_values_keep_defaults_and_null_is_supplied() {
        let mut m = fixture();
        let op = m.operations.get_mut("create").unwrap();
        op.params
            .as_mut()
            .unwrap()
            .get_mut("title")
            .unwrap()
            .required = false;
        op.steps = Some(vec![Step::Insert {
            insert: "items".into(),
            values: json!({"title":"{{args.title}}"})
                .as_object()
                .unwrap()
                .clone(),
        }]);
        let mut s = store(&m);
        run(&mut s, &m, "create", &Map::new(), 0, "UTC").unwrap();
        let row = &s.query("SELECT title, done FROM items", &[]).unwrap()[0];
        assert!(row["title"].is_null());
        assert_eq!(row["done"], 0);
        run(
            &mut s,
            &m,
            "create",
            json!({"title":null}).as_object().unwrap(),
            0,
            "UTC",
        )
        .unwrap();
        assert_eq!(s.query("SELECT title FROM items", &[]).unwrap().len(), 2);
    }
    #[test]
    fn timeout_and_execution_step_cap_are_errors() {
        let mut m = fixture();
        let mut s = store(&m);
        m.limits.timeout_ms = 0;
        assert!(matches!(
            run(
                &mut s,
                &m,
                "create",
                json!({"title":"x"}).as_object().unwrap(),
                0,
                "UTC"
            ),
            Err(RuntimeError::Trap(_))
        ));
        m.operations.get_mut("create").unwrap().steps = Some(vec![]);
        assert!(run(&mut s, &m, "create", &Map::new(), 0, "UTC").is_err());
        assert!(s.export_since(0).unwrap().0.changes.is_empty());
    }
    #[test]
    fn counter_append_and_delete_policy_validation() {
        let base = serde_json::to_value(fixture()).unwrap();
        for body in [
            json!({"increment":"items","row":"{{args.id}}","column":"done","delta":1}),
            json!({"update":"items","row":"{{$now_ms}}","values":{"title":"x"}}),
            json!({"update":"items","row":"{{args.id.extra}}","values":{"title":"x"}}),
        ] {
            let mut m = base.clone();
            m["operations"]["update"]["do"] = json!([body]);
            assert!(Manifest::parse(&serde_json::to_vec(&m).unwrap()).is_err());
        }
        for edit in ["append", "delete-wins"] {
            let mut m = base.clone();
            if edit == "append" {
                m["storage"]["tables"][0]["kind"] = json!(edit);
            } else {
                m["storage"]["tables"][0]["delete_policy"] = json!(edit);
            }
            assert!(Manifest::parse(&serde_json::to_vec(&m).unwrap()).is_err());
        }
        let mut m = base.clone();
        m["storage"]["tables"][0]["columns"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"visits","type":"integer","rule":"counter"}));
        for delta in [
            json!(null),
            json!("1"),
            json!(true),
            json!(1.5),
            json!("{{args.title}}"),
            json!("{{args.amount}}"),
            json!(18446744073709551615u64),
        ] {
            m["operations"]["update"]["params"]["amount"] = json!({"type":"integer"});
            m["operations"]["update"]["do"] =
                json!([{"increment":"items","row":"{{args.id}}","column":"visits","delta":delta}]);
            assert!(Manifest::parse(&serde_json::to_vec(&m).unwrap()).is_err());
        }
        m["operations"]["update"]["do"] =
            json!([{"update":"items","row":"{{args.id}}","values":{"visits":1}}]);
        assert!(Manifest::parse(&serde_json::to_vec(&m).unwrap()).is_err());
    }
}
