//! Synced automation configuration; consent is local to this device.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::{
    commands::check_args, Effect, OperationDef, ParamSpec, ParamType, PluginHost, Result,
    RuntimeError,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

pub const RULES_STORE_ID: &str = "_rules";
const SCHEMA: &str = r#"{
  "version": 1,
  "tables": [
    {
      "name": "rules",
      "delete_policy": "delete-wins",
      "columns": [
        {
          "name": "space",
          "type": "text"
        },
        {
          "name": "name",
          "type": "text"
        },
        {
          "name": "trigger",
          "type": "text"
        },
        {
          "name": "filter",
          "type": "text"
        },
        {
          "name": "action",
          "type": "text"
        },
        {
          "name": "enabled",
          "type": "boolean",
          "default": true
        },
        {
          "name": "created_ms",
          "type": "integer"
        },
        {
          "name": "updated_ms",
          "type": "integer"
        }
      ]
    },
    {
      "name": "runs",
      "delete_policy": "delete-wins",
      "columns": [
        {
          "name": "space",
          "type": "text"
        },
        {
          "name": "rule",
          "type": "text"
        },
        {
          "name": "rule_name",
          "type": "text"
        },
        {
          "name": "digest",
          "type": "text"
        },
        {
          "name": "event",
          "type": "text"
        },
        {
          "name": "status",
          "type": "text"
        },
        {
          "name": "device",
          "type": "text"
        },
        {
          "name": "error",
          "type": "text"
        },
        {
          "name": "started_ms",
          "type": "integer"
        },
        {
          "name": "finished_ms",
          "type": "integer"
        }
      ]
    },
    {
      "name": "settings",
      "delete_policy": "delete-wins",
      "columns": [
        {
          "name": "space",
          "type": "text"
        },
        {
          "name": "primary_device",
          "type": "text"
        },
        {
          "name": "paused",
          "type": "boolean",
          "default": false
        }
      ]
    }
  ]
}"#;
const APPROVAL_LIMIT: u64 = 1024 * 1024;

fn yes() -> bool {
    true
}
fn bad(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Plugin {
        code: "invalid".into(),
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub plugin: String,
    pub event: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub field: String,
    pub op: CondOp,
    pub value: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondOp {
    Eq,
    Ne,
    Contains,
    HasTag,
    Gt,
    Lt,
    Exists,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub plugin: String,
    pub operation: String,
    pub args: Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub name: String,
    pub trigger: Trigger,
    #[serde(default)]
    pub filter: Vec<Condition>,
    pub action: Action,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInfo {
    pub id: String,
    pub space: String,
    pub name: String,
    pub trigger: Trigger,
    pub filter: Vec<Condition>,
    pub action: Action,
    pub enabled: bool,
    pub approved: bool,
    pub problem: Option<String>,
    pub updated_ms: u64,
    pub last_run: Option<Value>,
    pub waiting: u64,
    // Review tokens and identities never leave through list/read commands.
    #[serde(skip)]
    pub(crate) source_package: Value,
    #[serde(skip)]
    pub(crate) target_package: Value,
}
impl RuleInfo {
    pub fn input(&self) -> RuleInput {
        RuleInput {
            name: self.name.clone(),
            trigger: self.trigger.clone(),
            filter: self.filter.clone(),
            action: self.action.clone(),
            enabled: self.enabled,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub primary_device: Option<String>,
    pub paused: bool,
}

/// Recursively sorted objects, including objects nested inside literal args.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(k, v)| (k, canonical(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
        v => v,
    }
}
pub fn digest(rule: &RuleInfo) -> String {
    let domain = json!({"version":1,"id":rule.id,"space":rule.space,
        "trigger":rule.trigger,"filter":rule.filter,"action":rule.action,
        "source_package":rule.source_package,"target_package":rule.target_package});
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(domain)).expect("JSON serializes"))
    )
}
fn limits(input: &RuleInput) -> Result<()> {
    if input.name.trim().is_empty() || input.name.chars().count() > 60 {
        return Err(bad("Rule name must be 1–60 characters"));
    }
    if input.filter.len() > 10 {
        return Err(bad("A rule holds at most 10 conditions"));
    }
    if serde_json::to_vec(input)?.len() > 8192 {
        return Err(bad("A rule must fit within 8 KiB"));
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    id.strip_prefix("rule-")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_some_and(|u| u.get_version_num() == 7)
}
fn space_id(space: &str) -> Result<()> {
    if space.is_empty()
        || space.len() > 63
        || !space
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(bad("Invalid space id"));
    }
    Ok(())
}

pub struct Rules {
    store: CoreStore,
    approvals: PathBuf,
}
impl Rules {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Self> {
        Ok(Self {
            store: CoreStore::open(
                data_dir.join("_rules.db"),
                StoreConfig {
                    plugin: RULES_STORE_ID.into(),
                    ..cfg
                },
                serde_json::from_str::<PluginSchema>(SCHEMA)?,
                Box::new(SystemTime),
            )?,
            approvals: data_dir.join("rule-approvals.json"),
        })
    }
    pub fn store(&self) -> &CoreStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }
    fn decode(&self, row: &Map<String, Value>) -> Result<RuleInfo> {
        let text = |k: &str| {
            row.get(k)
                .and_then(Value::as_str)
                .ok_or_else(|| bad("Invalid stored rule"))
        };
        let id = text("id")?.to_owned();
        let space = text("space")?.to_owned();
        if !valid_id(&id) {
            return Err(bad("Invalid rule id"));
        }
        space_id(&space)?;
        // Bound each serialized column before parsing untrusted synced JSON.
        for key in ["trigger", "filter", "action"] {
            if text(key)?.len() > 8192 {
                return Err(bad("Stored rule is too large"));
            }
        }
        let input = RuleInput {
            name: text("name")?.into(),
            trigger: serde_json::from_str(text("trigger")?)
                .map_err(|_| bad("Invalid stored trigger"))?,
            filter: serde_json::from_str(text("filter")?)
                .map_err(|_| bad("Invalid stored conditions"))?,
            action: serde_json::from_str(text("action")?)
                .map_err(|_| bad("Invalid stored action"))?,
            enabled: match row.get("enabled") {
                Some(Value::Bool(v)) => *v,
                Some(v) if v == &json!(0) => false,
                Some(v) if v == &json!(1) => true,
                _ => return Err(bad("Invalid stored enabled flag")),
            },
        };
        limits(&input)?;
        let mut info = RuleInfo {
            id,
            space,
            name: input.name,
            trigger: input.trigger,
            filter: input.filter,
            action: input.action,
            enabled: input.enabled,
            approved: false,
            problem: None,
            updated_ms: row
                .get("updated_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad("Invalid rule time"))?,
            last_run: None,
            waiting: 0,
            source_package: Value::Null,
            target_package: Value::Null,
        };
        info.approved = self.is_approved(&info)?;
        Ok(info)
    }
    fn read(&self, row: &Map<String, Value>) -> Result<RuleInfo> {
        match self.decode(row) {
            Ok(rule) => Ok(rule),
            Err(error) => {
                // A newer/compromised peer must produce a visible, inert problem,
                // rather than hiding every other rule or silently dropping fields.
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| valid_id(s))
                    .ok_or_else(|| bad("Invalid stored rule id"))?;
                let space = row
                    .get("space")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("Invalid stored rule space"))?;
                space_id(space)?;
                Ok(RuleInfo {
                    id: id.into(),
                    space: space.into(),
                    name: "Invalid automation".into(),
                    trigger: Trigger {
                        plugin: String::new(),
                        event: String::new(),
                    },
                    filter: Vec::new(),
                    action: Action {
                        plugin: String::new(),
                        operation: String::new(),
                        args: Map::new(),
                    },
                    enabled: false,
                    approved: false,
                    problem: Some(error.to_string()),
                    updated_ms: row.get("updated_ms").and_then(Value::as_u64).unwrap_or(0),
                    last_run: None,
                    waiting: 0,
                    source_package: Value::Null,
                    target_package: Value::Null,
                })
            }
        }
    }
    pub fn list(&self, space: &str) -> Result<Vec<RuleInfo>> {
        space_id(space)?;
        self.store
            .query(
                "SELECT * FROM rules WHERE _deleted IS NULL AND space = ?1 ORDER BY created_ms, id",
                &[json!(space)],
            )?
            .iter()
            .map(|r| self.read(r))
            .collect()
    }
    pub fn get(&self, id: &str) -> Result<Option<RuleInfo>> {
        if !valid_id(id) {
            return Err(bad("Invalid rule id"));
        }
        self.store
            .query(
                "SELECT * FROM rules WHERE _deleted IS NULL AND id = ?1",
                &[json!(id)],
            )?
            .first()
            .map(|r| self.read(r))
            .transpose()
    }
    fn require(&self, id: &str) -> Result<RuleInfo> {
        self.get(id)?.ok_or_else(|| bad("Rule was not found"))
    }
    pub fn create(&mut self, space: &str, input: &RuleInput) -> Result<RuleInfo> {
        space_id(space)?;
        limits(input)?;
        let count = self.store.query(
            "SELECT COUNT(*) AS n FROM rules WHERE _deleted IS NULL AND space = ?1",
            &[json!(space)],
        )?;
        if count[0]["n"].as_u64().unwrap_or(100) >= 100 {
            return Err(bad("A space holds at most 100 rules"));
        }
        let id = format!("rule-{}", uuid::Uuid::now_v7());
        let mut values = input_values(input)?;
        values.insert("space".into(), json!(space));
        values.insert("created_ms".into(), json!(crate::registry::now_ms()));
        self.store.write(|w| w.upsert("rules", &id, &values))?;
        self.require(&id)
    }
    pub fn update(&mut self, id: &str, input: &RuleInput) -> Result<RuleInfo> {
        self.require(id)?;
        limits(input)?;
        let values = input_values(input)?;
        self.store.write(|w| w.update("rules", id, &values))?;
        self.require(id)
    }
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<RuleInfo> {
        self.require(id)?;
        let values = json!({"enabled":enabled,"updated_ms":crate::registry::now_ms()});
        self.store
            .write(|w| w.update("rules", id, values.as_object().expect("object")))?;
        self.require(id)
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.get(id)?.is_some() {
            self.store.write(|w| w.delete("rules", id))?;
        }
        Ok(())
    }
    pub fn settings(&self, space: &str) -> Result<Settings> {
        space_id(space)?;
        let rows=self.store.query("SELECT primary_device, paused FROM settings WHERE _deleted IS NULL AND id = ?1 AND space = ?2", &[json!(format!("settings-{space}")),json!(space)])?;
        let Some(row) = rows.first() else {
            return Ok(Settings::default());
        };
        let primary_device = match &row["primary_device"] {
            Value::Null => None,
            Value::String(s)
                if !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control) =>
            {
                Some(s.clone())
            }
            _ => return Err(bad("Invalid primary device")),
        };
        let paused = match &row["paused"] {
            Value::Bool(v) => *v,
            v if v == &json!(0) => false,
            v if v == &json!(1) => true,
            _ => return Err(bad("Invalid pause setting")),
        };
        Ok(Settings {
            primary_device,
            paused,
        })
    }
    pub fn save_settings(&mut self, space: &str, settings: &Settings) -> Result<()> {
        if settings
            .primary_device
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(bad("Invalid primary device"));
        }
        self.save_setting(space,json!({"space":space,"primary_device":settings.primary_device,"paused":settings.paused}))
    }
    pub fn set_primary_device(&mut self, space: &str, device: Option<&str>) -> Result<()> {
        if device.is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(bad("Invalid primary device"));
        }
        self.save_setting(space, json!({"space":space,"primary_device":device}))
    }
    pub fn set_paused(&mut self, space: &str, paused: bool) -> Result<()> {
        self.save_setting(space, json!({"space":space,"paused":paused}))
    }
    fn save_setting(&mut self, space: &str, values: Value) -> Result<()> {
        space_id(space)?;
        self.store.write(|w| {
            w.upsert(
                "settings",
                &format!("settings-{space}"),
                values.as_object().expect("object"),
            )
        })?;
        Ok(())
    }
    fn approvals(&self) -> Result<BTreeSet<String>> {
        let file = match std::fs::File::open(&self.approvals) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
            Err(e) => return Err(RuntimeError::io(&self.approvals, e)),
        };
        let mut bytes = Vec::new();
        file.take(APPROVAL_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| RuntimeError::io(&self.approvals, e))?;
        if bytes.len() as u64 > APPROVAL_LIMIT {
            return Err(bad("Local approval file is too large"));
        }
        serde_json::from_slice(&bytes).map_err(|_| bad("Invalid local approval file"))
    }
    pub fn is_approved(&self, rule: &RuleInfo) -> Result<bool> {
        Ok(self
            .approvals()?
            .contains(&format!("{}:{}", rule.id, digest(rule))))
    }
    /// The trusted host supplies freshly resolved package identities and review token.
    pub(crate) fn approve(&mut self, rule: &RuleInfo, expected: &str) -> Result<()> {
        let mut current = self.require(&rule.id)?;
        current.source_package = rule.source_package.clone();
        current.target_package = rule.target_package.clone();
        if digest(&current) != expected || rule.problem.is_some() {
            return Err(bad("Rule changed; review it again"));
        }
        let mut approved = self.approvals()?;
        approved.insert(format!("{}:{expected}", rule.id));
        let bytes = serde_json::to_vec(&approved)?;
        if bytes.len() as u64 > APPROVAL_LIMIT {
            return Err(bad("Local approval file is full"));
        }
        let parent = self.approvals.parent().expect("data directory");
        let mut tmp =
            tempfile::NamedTempFile::new_in(parent).map_err(|e| RuntimeError::io(parent, e))?;
        tmp.write_all(&bytes)
            .map_err(|e| RuntimeError::io(tmp.path(), e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| RuntimeError::io(tmp.path(), e))?;
        tmp.persist(&self.approvals)
            .map_err(|e| RuntimeError::io(&self.approvals, e.error))?;
        Ok(())
    }
    pub fn runs(
        &self,
        space: &str,
        rule: Option<&str>,
        limit: usize,
        before: Option<&str>,
    ) -> Result<Value> {
        space_id(space)?;
        if !(1..=50).contains(&limit) {
            return Err(bad("Run limit must be 1–50"));
        }
        if rule.is_some_and(|r| !valid_id(r)) {
            return Err(bad("Invalid rule id"));
        }
        let cursor: Option<(u64, String)> = before
            .map(|s| {
                if s.len() > 1024 {
                    return Err(bad("Invalid history cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(s)
                    .map_err(|_| bad("Invalid history cursor"))?;
                serde_json::from_slice(&bytes).map_err(|_| bad("Invalid history cursor"))
            })
            .transpose()?;
        let (at, id) = cursor.unwrap_or((i64::MAX as u64, String::new()));
        if at > i64::MAX as u64 || id.len() > 512 {
            return Err(bad("Invalid history cursor"));
        }
        let rows=self.store.query("SELECT id, space, rule, rule_name, digest, event, status, device, error, started_ms, finished_ms FROM runs WHERE _deleted IS NULL AND space = ?1 AND (?2 IS NULL OR rule = ?2) AND (started_ms < ?3 OR (started_ms = ?3 AND id < ?4)) ORDER BY started_ms DESC, id DESC LIMIT ?5", &[json!(space),json!(rule),json!(at),json!(id),json!(limit+1)])?;
        let next = if rows.len() > limit {
            let r = &rows[limit - 1];
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&(
                r["started_ms"].clone(),
                r["id"].clone(),
            ))?))
        } else {
            None
        };
        let runs: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(|mut r| {
                // Imported/plugin-supplied error text is never history output.
                if !matches!(
                    r.get("error").and_then(Value::as_str),
                    Some(
                        "interrupted"
                            | "fanout"
                            | "rate"
                            | "invalid"
                            | "failed"
                            | "stopped"
                            | "skip"
                            | "expired"
                    )
                ) {
                    r.insert("error".into(), Value::Null);
                }
                r
            })
            .collect();
        Ok(json!({"runs":runs,"next":next}))
    }
}
fn input_values(input: &RuleInput) -> Result<Map<String, Value>> {
    Ok(json!({"name":input.name,"trigger":serde_json::to_string(&input.trigger)?,"filter":serde_json::to_string(&input.filter)?,"action":serde_json::to_string(&input.action)?,"enabled":input.enabled,"updated_ms":crate::registry::now_ms()}).as_object().expect("object").clone())
}

pub(crate) fn eligible(name: &str, op: &OperationDef) -> bool {
    op.effect_of(name) == Effect::Write
        && op.params.is_some()
        && op.cli
        && op.confirm.is_none()
        && !name.starts_with("on_")
}
/// Whole-string bindings only; nested values remain literal JSON.
fn binding(value: &Value) -> std::result::Result<Option<&str>, String> {
    let Some(s) = value.as_str() else {
        return Ok(None);
    };
    if let Some(field) = s
        .strip_prefix("{{event.")
        .and_then(|s| s.strip_suffix("}}"))
    {
        if !field.is_empty() && !field.contains(['{', '}']) {
            return Ok(Some(field));
        }
    }
    if s.contains("{{") || s.contains("}}") {
        return Err("Use a whole event field binding".into());
    }
    Ok(None)
}
fn compatible(source: &ParamSpec, target: &ParamSpec) -> bool {
    use ParamType::*;
    if target.required && !source.required {
        return false;
    }
    if source.ty == Enum
        && target.ty == Enum
        && !source.values.iter().all(|v| target.values.contains(v))
    {
        return false;
    }
    source.ty == target.ty
        || target.ty == Json
        || (source.ty == Integer && target.ty == Number)
        || (matches!(source.ty, String | Text | Id | Date | Time | Enum)
            && matches!(target.ty, String | Text))
}
fn check_condition(c: &Condition, spec: &ParamSpec) -> std::result::Result<(), String> {
    use CondOp::*;
    use ParamType::*;
    let valid = match c.op {
        Exists => c.value.is_boolean(),
        Contains => matches!(spec.ty, String | Text) && c.value.is_string(),
        HasTag => {
            matches!(spec.ty, String | Text | Json)
                && c.value.as_str().is_some_and(|s| {
                    !s.trim_start_matches('#').is_empty()
                        && !s.contains(|c: char| c.is_whitespace() || c == ',')
                })
        }
        Gt | Lt => matches!(spec.ty, Integer | Number | Date),
        Eq | Ne => true,
    };
    if !valid {
        return Err(format!(
            "Condition for `{}` has an incompatible operator or value",
            c.field
        ));
    }
    if matches!(c.op, Eq | Ne | Gt | Lt) {
        let mut spec = spec.clone();
        spec.required = true;
        check_args(
            &BTreeMap::from([(c.field.clone(), spec)]),
            &[],
            &Map::from_iter([(c.field.clone(), c.value.clone())]),
        )
        .map_err(|e| e.message)?;
    }
    Ok(())
}
pub fn check(host: &PluginHost, input: &RuleInput) -> std::result::Result<(), String> {
    limits(input).map_err(|e| e.to_string())?;
    let source = host
        .manifest(&input.trigger.plugin)
        .ok_or("Trigger plugin is not installed")?;
    let event = source
        .events
        .get(&input.trigger.event)
        .ok_or("Trigger event is not declared")?;
    for c in &input.filter {
        let spec = event
            .payload
            .get(&c.field)
            .ok_or_else(|| format!("Condition field `{}` is not declared", c.field))?;
        check_condition(c, spec)?;
    }
    let target = host
        .manifest(&input.action.plugin)
        .ok_or("Action plugin is not installed")?;
    let op = target
        .operations
        .get(&input.action.operation)
        .ok_or("Action operation is not declared")?;
    if !eligible(&input.action.operation, op) {
        return Err(
            "Action must be a typed, visible write without confirmation or callback".into(),
        );
    }
    let params = op.params.as_ref().expect("eligible");
    let mut literals = input.action.args.clone();
    let mut literal_params = params.clone();
    for (name, value) in &input.action.args {
        let Some(field) = binding(value)? else {
            continue;
        };
        let source = match field {
            "$date" => {
                let mut s = ParamSpec::new(ParamType::Date);
                s.required = true;
                s
            }
            "$plugin" => {
                let mut s = ParamSpec::new(ParamType::Id);
                s.required = true;
                s
            }
            _ => event
                .payload
                .get(field)
                .cloned()
                .ok_or_else(|| format!("Binding field `{field}` is not declared"))?,
        };
        let target = params
            .get(name)
            .ok_or_else(|| format!("Unknown action argument `{name}`"))?;
        if !compatible(&source, target) {
            return Err(format!(
                "Binding for `{name}` has an incompatible type or is optional"
            ));
        }
        // Only literals are value-checked here. Real bindings are value-checked at execution;
        // samples cannot prove type compatibility, bounds or enum membership.
        literals.remove(name);
        literal_params.remove(name);
    }
    check_args(&literal_params, &[], &literals).map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn open(dir: &tempfile::TempDir, device: &str) -> Rules {
        Rules::open(
            dir.path(),
            StoreConfig {
                space: "personal".into(),
                plugin: RULES_STORE_ID.into(),
                device: device.into(),
                user: "owner".into(),
            },
        )
        .unwrap()
    }
    fn input() -> RuleInput {
        serde_json::from_value(json!({"name":"Gym","trigger":{"plugin":"todo","event":"task_done"},"action":{"plugin":"habits","operation":"checkin","args":{"day":"{{event.$date}}"}}})).unwrap()
    }
    fn approve(rules: &mut Rules, rule: &RuleInfo) {
        rules.approve(rule, &digest(rule)).unwrap();
    }
    #[test]
    fn create_list_update_remove_rule() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let mut rule = rules.create("personal", &input()).unwrap();
        assert_eq!(rules.list("personal").unwrap().len(), 1);
        assert!(rules.list("work").unwrap().is_empty());
        rule.name = "Evening".into();
        let updated = rules.update(&rule.id, &rule.input()).unwrap();
        assert_eq!(updated.name, "Evening");
        assert_eq!(updated.id, rule.id);
        rules.remove(&rule.id).unwrap();
        rules.remove(&rule.id).unwrap();
        assert!(rules.get(&rule.id).unwrap().is_none());
    }
    #[test]
    fn editing_trigger_filter_or_action_needs_approval_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let original = rules.create("personal", &input()).unwrap();
        for kind in 0..3 {
            let rule = rules.update(&original.id, &input()).unwrap();
            approve(&mut rules, &rule);
            let mut next = rule.input();
            match kind {
                0 => next.trigger.event = "other".into(),
                1 => next.filter.push(Condition {
                    field: "tags".into(),
                    op: CondOp::HasTag,
                    value: json!("gym"),
                }),
                _ => {
                    next.action.args.insert("id".into(), json!("other"));
                }
            }
            let changed = rules.update(&rule.id, &next).unwrap();
            assert!(!rules.is_approved(&changed).unwrap());
            assert!(rules.approve(&changed, &digest(&rule)).is_err());
        }
    }
    #[test]
    fn renaming_or_disabling_keeps_approval() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let mut rule = rules.create("personal", &input()).unwrap();
        approve(&mut rules, &rule);
        rule.name = "Renamed".into();
        let rule = rules.update(&rule.id, &rule.input()).unwrap();
        assert!(rule.approved);
        assert!(rules.set_enabled(&rule.id, false).unwrap().approved);
        drop(rules);
        assert!(open(&dir, "a").get(&rule.id).unwrap().unwrap().approved);
    }
    #[test]
    fn approvals_are_device_local() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let mut a = open(&da, "a");
        let mut b = open(&db, "b");
        let r = a.create("personal", &input()).unwrap();
        approve(&mut a, &r);
        let (batch, _) = a.store().export_since(0).unwrap();
        b.store_mut().import(&batch).unwrap();
        assert!(a.is_approved(&r).unwrap());
        assert!(!b.get(&r.id).unwrap().unwrap().approved);
        // New ids, spaces, package bytes and signers cannot reuse a consent digest.
        let mut changed = r.clone();
        changed.source_package = json!({"digest":"a","signer":"publisher"});
        assert!(!a.is_approved(&changed).unwrap());
        assert!(!db.path().join("rule-approvals.json").exists());
    }
    #[test]
    fn settings_row_converges_between_devices() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let mut a = open(&da, "a");
        let mut b = open(&db, "b");
        a.set_primary_device("personal", Some("a")).unwrap();
        b.set_paused("personal", true).unwrap();
        let (ba, _) = a.store().export_since(0).unwrap();
        let (bb, _) = b.store().export_since(0).unwrap();
        a.store_mut().import(&bb).unwrap();
        b.store_mut().import(&ba).unwrap();
        assert_eq!(
            a.settings("personal").unwrap(),
            b.settings("personal").unwrap()
        );
        assert_eq!(
            a.settings("personal").unwrap(),
            Settings {
                primary_device: Some("a".into()),
                paused: true
            }
        );
        assert_eq!(
            a.store()
                .query("SELECT id FROM settings WHERE _deleted IS NULL", &[])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(a.settings("work").unwrap(), Settings::default());
    }
    #[test]
    fn limits_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let mut r = input();
        r.name = "界".repeat(61);
        assert!(rules.create("personal", &r).is_err());
        r.name = "界".repeat(60);
        assert!(rules.create("work", &r).is_ok());
        r = input();
        r.filter = vec![
            Condition {
                field: "tags".into(),
                op: CondOp::Exists,
                value: json!(true)
            };
            11
        ];
        assert!(rules.create("personal", &r).is_err());
        r = input();
        r.action
            .args
            .insert("large".into(), json!("x".repeat(8192)));
        assert!(rules.create("personal", &r).is_err());
        for _ in 0..100 {
            rules.create("personal", &input()).unwrap();
        }
        assert!(rules.create("personal", &input()).is_err());
        let mut value = serde_json::to_value(input()).unwrap();
        value["unknown"] = json!(true);
        assert!(serde_json::from_value::<RuleInput>(value).is_err());
    }
    #[test]
    fn canonical_digest_and_approval_file_limits() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let mut r = rules.create("personal", &input()).unwrap();
        r.action.args.insert(
            "nested".into(),
            serde_json::from_str(r#"{"z":[{"b":1,"a":2}],"a":0}"#).unwrap(),
        );
        let mut other = r.clone();
        other.action.args.insert(
            "nested".into(),
            serde_json::from_str(r#"{"a":0,"z":[{"a":2,"b":1}]}"#).unwrap(),
        );
        assert_eq!(digest(&r), digest(&other));
        std::fs::write(&rules.approvals, vec![b' '; APPROVAL_LIMIT as usize + 1]).unwrap();
        assert!(rules.is_approved(&r).is_err());
    }
    #[test]
    fn malformed_imports_are_inert_and_disable_preserves_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let r = rules.create("personal", &input()).unwrap();
        let trigger = r#"{"plugin":"todo","event":"task_done","future_field":"private-value"}"#;
        rules
            .store_mut()
            .write(|w| {
                w.update(
                    "rules",
                    &r.id,
                    json!({"trigger":trigger}).as_object().unwrap(),
                )
            })
            .unwrap();
        let bad = rules.get(&r.id).unwrap().unwrap();
        assert!(bad.problem.is_some());
        assert!(!bad.approved);
        assert!(!bad.problem.unwrap().contains("private-value"));
        assert_eq!(rules.list("personal").unwrap().len(), 1);
        rules.set_enabled(&r.id, false).unwrap();
        assert_eq!(
            rules
                .store()
                .query("SELECT trigger FROM rules WHERE id = ?1", &[json!(r.id)])
                .unwrap()[0]["trigger"],
            trigger
        );
    }

    #[test]
    fn history_is_paged_scoped_and_redacts_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut rules = open(&dir, "a");
        let r = rules.create("personal", &input()).unwrap();
        for (id, space) in [("a", "personal"), ("b", "personal"), ("c", "work")] {
            rules
                .store_mut()
                .write(|w| {
                    w.upsert(
                        "runs",
                        id,
                        json!({"space":space,"rule":r.id,"started_ms":1,"error":"private-value"})
                            .as_object()
                            .unwrap(),
                    )
                })
                .unwrap();
        }
        let first = rules.runs("personal", None, 1, None).unwrap();
        assert_eq!(first["runs"][0]["id"], "b");
        assert!(first["runs"][0]["error"].is_null());
        let next = rules
            .runs("personal", None, 1, first["next"].as_str())
            .unwrap();
        assert_eq!(next["runs"][0]["id"], "a");
        assert!(next["next"].is_null());
        assert!(rules.runs("personal", None, 0, None).is_err());
        assert!(rules.runs("personal", None, 1, Some("bad")).is_err());
    }
}

pub const MAX_RULE_DEPTH: u8 = 3;
pub const MAX_RULES_PER_EVENT: usize = 10;
pub const MAX_RUNS_PER_RULE_PER_HOUR: usize = 60;
pub const EVENTS_PER_TICK: usize = 200;

/// AND conditions, with absence distinct from JSON comparisons.
pub fn matches(filter: &[Condition], payload: &Map<String, Value>) -> bool {
    filter.iter().all(|c| {
        let v = payload.get(&c.field).filter(|v| !v.is_null());
        if c.op == CondOp::Exists {
            return c.value.as_bool() == Some(v.is_some());
        }
        let Some(v) = v else { return false };
        match c.op {
            CondOp::Eq => v == &c.value,
            CondOp::Ne => v != &c.value,
            CondOp::Contains => v
                .as_str()
                .zip(c.value.as_str())
                .is_some_and(|(v, q)| v.to_lowercase().contains(&q.to_lowercase())),
            CondOp::HasTag => {
                let Some(tag) = c.value.as_str() else {
                    return false;
                };
                let tag = tag.strip_prefix('#').unwrap_or(tag).to_lowercase();
                let eq = |s: &str| {
                    !tag.is_empty() && s.strip_prefix('#').unwrap_or(s).to_lowercase() == tag
                };
                match v {
                    Value::String(s) => s.split(|c: char| c.is_whitespace() || c == ',').any(eq),
                    Value::Array(a) => a.iter().filter_map(Value::as_str).any(eq),
                    _ => false,
                }
            }
            CondOp::Gt | CondOp::Lt => {
                let order = if let Some((a, b)) = v.as_f64().zip(c.value.as_f64()) {
                    a.partial_cmp(&b)
                } else if let Some((a, b)) = v.as_str().zip(c.value.as_str()) {
                    (crate::time::date(a).is_ok() && crate::time::date(b).is_ok()).then(|| a.cmp(b))
                } else {
                    None
                };
                order.is_some_and(|o| {
                    if c.op == CondOp::Gt {
                        o.is_gt()
                    } else {
                        o.is_lt()
                    }
                })
            }
            CondOp::Exists => unreachable!(),
        }
    })
}

pub fn map_args(
    args: &Map<String, Value>,
    event: &crate::events::EventRecord,
) -> Map<String, Value> {
    args.iter()
        .map(|(key, value)| {
            let mapped = match binding(value) {
                Ok(Some("$date")) => json!(event.date),
                Ok(Some("$plugin")) => json!(event.plugin),
                Ok(Some(field)) => event.payload.get(field).cloned().unwrap_or(Value::Null),
                _ => value.clone(),
            };
            (key.clone(), mapped)
        })
        .collect()
}

#[cfg(test)]
mod matching_tests {
    use super::*;
    #[test]
    fn filter_ops_match_as_documented() {
        let payload = json!({"s":"Hello WÖRLD","n":3,"day":"2026-11-11","null":null,"obj":{"a":1}})
            .as_object()
            .unwrap()
            .clone();
        for (field, op, value, expected) in [
            ("n", CondOp::Eq, json!(3), true),
            ("n", CondOp::Eq, json!("3"), false),
            ("n", CondOp::Ne, json!(4), true),
            ("missing", CondOp::Ne, json!(1), false),
            ("null", CondOp::Eq, Value::Null, false),
            ("s", CondOp::Contains, json!("wörld"), true),
            ("n", CondOp::Gt, json!(2), true),
            ("n", CondOp::Lt, json!(3), false),
            ("day", CondOp::Gt, json!("2026-11-10"), true),
            ("day", CondOp::Lt, json!("tomorrow"), false),
            ("missing", CondOp::Exists, json!(false), true),
            ("null", CondOp::Exists, json!(true), false),
            ("obj", CondOp::Eq, json!({"a":1}), true),
        ] {
            assert_eq!(
                matches(
                    &[Condition {
                        field: field.into(),
                        op,
                        value
                    }],
                    &payload
                ),
                expected,
                "{field} {op:?}"
            );
        }
        assert!(matches(&[], &payload));
        assert!(!matches(
            &[
                Condition {
                    field: "n".into(),
                    op: CondOp::Eq,
                    value: json!(3)
                },
                Condition {
                    field: "s".into(),
                    op: CondOp::Eq,
                    value: json!("no")
                }
            ],
            &payload
        ));
    }
    #[test]
    fn has_tag_matches_words_and_arrays() {
        for (value, expected) in [
            (json!("#Gym health"), true),
            (json!("health,GYM"), true),
            (json!(["#gYm", "x"]), true),
            (json!("gymnastics"), false),
            (json!("#gym-extra"), false),
            (json!([1, null]), false),
            (json!("health\u{2003}#GYM"), true),
        ] {
            let payload = Map::from_iter([("tags".into(), value)]);
            assert_eq!(
                matches(
                    &[Condition {
                        field: "tags".into(),
                        op: CondOp::HasTag,
                        value: json!("#gym")
                    }],
                    &payload
                ),
                expected
            );
        }
    }
    #[test]
    fn map_args_replaces_bindings_only() {
        let event = crate::events::EventRecord {
            id: "unused".into(),
            space: "personal".into(),
            plugin: "limits".into(),
            name: "pinged".into(),
            payload: json!({"n":1}).as_object().unwrap().clone(),
            contract: String::new(),
            depth: 0,
            root: String::new(),
            device: "a".into(),
            emitted_ms: 0,
            date: "2026-11-11".into(),
        };
        let args = json!({"n":"{{event.n}}","missing":"{{event.missing}}","date":"{{event.$date}}","plugin":"{{event.$plugin}}","literal":"prefix {{event.n}}","nested":["{{event.n}}"]});
        assert_eq!(
            Value::Object(map_args(args.as_object().unwrap(), &event)),
            json!({"n":1,"missing":null,"date":"2026-11-11","plugin":"limits","literal":"prefix {{event.n}}","nested":["{{event.n}}"]})
        );
    }
}
