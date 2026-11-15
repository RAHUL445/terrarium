//! Loaded plugins on one device. Each plugin is compiled once and has one
//! store per space; an operation runs against the store of one space, and
//! interop only reaches plugins' stores in that same space.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use terrarium_core::{DeviceId, Store as CoreStore, StoreConfig, SystemTime};

use terrarium_ai::{Ai, AiConfig, AiError, Completion, Job, Sealer};
use terrarium_blobs::{BlobKey, BlobStore};
use terrarium_scheduler::{Finished, Outcome, Repeat, ScheduleInput, Scheduler};

use crate::error::{Result, RuntimeError};
use crate::events::{EventLog, EventRecord, EVENTS_STORE_ID, EVENT_RETENTION_MS};
use crate::host::{CallContext, Execution, HostState, PluginPre, Runtime, ScheduleChange};
use crate::links::{Links, LINKS_STORE_ID};
use crate::manifest::{Kind, Manifest, ParamType, QueryDef, RunOn, Schedule};
use crate::packages::{Packages, STORE_ID};
use crate::pages::{Pages, PAGES_STORE_ID};
use crate::rules::{RuleInfo, Rules, RULES_STORE_ID};
use crate::spaces::{split_store_id, store_id, Spaces, DEFAULT_SPACE, SPACES_STORE_ID};

/// Rows search.query indexes before answering, per plugin and space.
pub const SEARCH_CATCH_UP_ROWS: usize = 2_000;
/// Rows one background step indexes in total.
pub const INDEX_STEP_ROWS: usize = 1_000;
const MAX_VIEW_OFFSET: u64 = 1_000_000;

/// Apply v1 defaults and validate caller-controlled values before they can
/// reach SQLite. Legacy queries continue to use the original binding path.
fn validate_view_query_values(
    query: &QueryDef,
    input: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let view = query.view.as_ref().expect("v1 query view");
    for key in input.keys() {
        if !query.params.iter().any(|param| param == key) {
            return Err(RuntimeError::Manifest(format!(
                "query input `{key}` is not a declared parameter"
            )));
        }
    }
    let mut values = view.params.clone();
    values.extend(
        input
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );

    if let Some(paging) = &view.pagination {
        if values
            .get(&paging.page_size_param)
            .is_none_or(Value::is_null)
        {
            values.insert(
                paging.page_size_param.clone(),
                Value::from(paging.default_page_size),
            );
        }
        if values.get(&paging.offset_param).is_none_or(Value::is_null) {
            values.insert(paging.offset_param.clone(), Value::from(0));
        }
        let page_size = values[&paging.page_size_param]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| RuntimeError::Manifest("page_size must be a positive integer".into()))?;
        // UI overfetches one row to tell whether another page exists.
        if page_size > u64::from(paging.max_page_size) + 1 {
            return Err(RuntimeError::Manifest(format!(
                "page_size exceeds the declared maximum of {} plus one overfetch row",
                paging.max_page_size
            )));
        }
        let offset = values[&paging.offset_param].as_u64().ok_or_else(|| {
            RuntimeError::Manifest("offset must be a non-negative integer".into())
        })?;
        if offset > MAX_VIEW_OFFSET {
            return Err(RuntimeError::Manifest(format!(
                "offset exceeds the maximum of {MAX_VIEW_OFFSET}"
            )));
        }
    }

    if let Some(temporal) = &view.temporal {
        let start = values.get(&temporal.start_param).filter(|v| !v.is_null());
        let end = values.get(&temporal.end_param).filter(|v| !v.is_null());
        match (start, end) {
            (None, None) => {}
            (Some(start), Some(end)) => {
                let start = start.as_i64().ok_or_else(|| {
                    RuntimeError::Manifest(
                        "temporal start bound must be an integer timestamp".into(),
                    )
                })?;
                let end = end.as_i64().ok_or_else(|| {
                    RuntimeError::Manifest("temporal end bound must be an integer timestamp".into())
                })?;
                if start > end {
                    return Err(RuntimeError::Manifest(
                        "temporal start bound must not exceed end bound".into(),
                    ));
                }
            }
            _ => {
                return Err(RuntimeError::Manifest(
                    "temporal start and end bounds must be provided together".into(),
                ));
            }
        }
    }

    for sort in &view.sorts {
        if let Some(value) = values.get(&sort.param).filter(|value| !value.is_null()) {
            let value = value.as_str().ok_or_else(|| {
                RuntimeError::Manifest(format!("sort parameter `{}` must be a string", sort.param))
            })?;
            if !sort.values.iter().any(|allowed| allowed == value) {
                return Err(RuntimeError::Manifest(format!(
                    "unsupported sort value `{value}` for `{}`",
                    sort.param
                )));
            }
        }
        if let Some(param) = &sort.direction_param {
            if let Some(value) = values.get(param).filter(|value| !value.is_null()) {
                let value = value.as_str().ok_or_else(|| {
                    RuntimeError::Manifest(format!("sort direction `{param}` must be a string"))
                })?;
                if !sort.directions.iter().any(|allowed| allowed == value) {
                    return Err(RuntimeError::Manifest(format!(
                        "unsupported sort direction `{value}` for `{param}`"
                    )));
                }
            }
        }
    }

    for param in &query.params {
        if crate::manifest::CONTEXT_PARAMS.contains(&param.as_str()) {
            continue;
        }
        let value = values.get(param).unwrap_or(&Value::Null);
        if value.is_null() {
            if query.types.get(param).is_some_and(|spec| spec.required) {
                return Err(RuntimeError::Manifest(format!(
                    "required query parameter `{param}` is missing"
                )));
            }
            continue;
        }
        if let Some(spec) = query.types.get(param) {
            let valid = match spec.ty {
                ParamType::String | ParamType::Text | ParamType::Id => value.is_string(),
                ParamType::Integer => value.as_i64().is_some(),
                ParamType::Number => value.as_f64().is_some(),
                ParamType::Boolean => value.is_boolean(),
                ParamType::Date => value.as_str().is_some_and(|v| crate::time::date(v).is_ok()),
                ParamType::Time => value.as_str().is_some_and(|v| crate::time::time(v).is_ok()),
                ParamType::Enum => value
                    .as_str()
                    .is_some_and(|v| spec.values.iter().any(|allowed| allowed == v)),
                ParamType::Json => true,
            };
            if !valid {
                return Err(RuntimeError::Manifest(format!(
                    "query parameter `{param}` has the wrong type or value"
                )));
            }
            if matches!(spec.ty, ParamType::Integer | ParamType::Number) {
                if let Some(number) = value.as_f64() {
                    if spec.min.is_some_and(|min| number < min)
                        || spec.max.is_some_and(|max| number > max)
                    {
                        return Err(RuntimeError::Manifest(format!(
                            "query parameter `{param}` is outside its declared range"
                        )));
                    }
                }
            }
        }
    }
    Ok(values)
}

#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Directory of the default space's plugin databases. It also holds
    /// the device-wide stores (installed plugins, the space list).
    pub data_dir: PathBuf,
    /// Where other spaces keep their databases: `<dir>/<space id>/plugins`.
    /// `None`: `<data_dir>/spaces`.
    pub spaces_dir: Option<PathBuf>,
    /// Label stored with changes (not the space id of a store).
    pub space: String,
    pub device: DeviceId,
    pub user: String,
    pub timezone: String,
    pub execution: Execution,
    /// Local speech-to-text, where this device has it.
    pub speech: Option<crate::speech::SpeechConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    Page,
    Widget,
}

enum Code {
    Wasm(PluginPre<HostState>),
    Declarative,
}

pub(crate) struct LoadedPlugin {
    manifest: Manifest,
    identity: Value,
    dir: PathBuf,
    code: Code,
    /// One store per open space; `None` while an operation runs in it.
    stores: BTreeMap<String, Option<CoreStore>>,
    templates: BTreeMap<(ViewKind, String), Value>,
}

pub struct PluginHost {
    runtime: Runtime,
    cfg: HostConfig,
    plugins: BTreeMap<String, LoadedPlugin>,
    packages: Packages,
    spaces: Spaces,
    pages: Pages,
    links: Links,
    vaults: BTreeMap<String, terrarium_net::vault::Vault>,
    vault_sealer: Option<Sealer>,
    /// Spaces whose stores are open (always includes the default space).
    open_spaces: BTreeSet<String>,
    /// AI service: `None` only while lent to a running call.
    ai: Option<Ai>,
    /// Installed by an HTTP worker; also lent exclusively during calls.
    net: Option<terrarium_net::Net>,
    scheduler: Option<Scheduler>,
    blobs: Arc<Mutex<BlobStore>>,
    assistant: crate::assistant::Assistant,
    async_outcomes: BTreeMap<String, Outcome>,
    active_ai: BTreeMap<String, String>,
    active_http_accounts: BTreeMap<String, terrarium_net::vault::AccountLease>,
    /// Search index: None only while lent to a host indexing call.
    index: Option<crate::index::SearchIndex>,
    events: EventLog,
    rules: Rules,
    automation: crate::automation::Local,
    automation_context: bool,
    automation_problem: Option<String>,
}

const BLOB_QUOTA_DEFAULT_MB: u64 = 2048;
const BLOB_QUOTA_MAX_MB: u64 = 10 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BlobSettings {
    #[serde(default)]
    blob_quota_mb: Option<u64>,
}

impl PluginHost {
    pub fn new(cfg: HostConfig) -> Result<PluginHost> {
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| RuntimeError::io(&cfg.data_dir, e))?;
        let packages = Packages::open(&cfg.data_dir, store_config(&cfg, STORE_ID))?;
        let spaces = Spaces::open(&cfg.data_dir, store_config(&cfg, SPACES_STORE_ID))?;
        let events = EventLog::open(&cfg.data_dir, store_config(&cfg, EVENTS_STORE_ID))?;
        let rules = Rules::open(&cfg.data_dir, store_config(&cfg, RULES_STORE_ID))?;
        let pages = Pages::open(&cfg.data_dir, store_config(&cfg, PAGES_STORE_ID))?;
        let vault = terrarium_net::vault::Vault::open(
            &cfg.data_dir,
            store_config(&cfg, terrarium_net::vault::VAULT_STORE_ID),
            DEFAULT_SPACE,
        )
        .map_err(|e| RuntimeError::Manifest(e.message))?;
        let links = Links::open(&cfg.data_dir, store_config(&cfg, LINKS_STORE_ID))?;
        let ai = Ai::open(&cfg.data_dir, store_config(&cfg, terrarium_ai::STORE_ID))?;
        let blobs = BlobStore::open(&blob_dir_for_space(&cfg, DEFAULT_SPACE))
            .map_err(|e| RuntimeError::Wasm(format!("blob store open failed: {e}")))?;
        let mut scheduler = Scheduler::open(
            &cfg.data_dir,
            store_config(&cfg, terrarium_scheduler::STORE_ID),
            &cfg.timezone,
        )?;
        let index = crate::index::SearchIndex::new(cfg.data_dir.join("index"));
        let automation = crate::automation::Local::open(&cfg.data_dir)?;
        scheduler
            .interrupt_device(&cfg.device.0, now_ms())
            .map_err(RuntimeError::Manifest)?;
        Ok(PluginHost {
            index: Some(index),
            runtime: Runtime::new(cfg.execution)?,
            cfg,
            plugins: BTreeMap::new(),
            packages,
            spaces,
            pages,
            links,
            vaults: [(DEFAULT_SPACE.to_string(), vault)].into_iter().collect(),
            vault_sealer: None,
            open_spaces: [DEFAULT_SPACE.to_string()].into_iter().collect(),
            ai: Some(ai),
            net: None,
            scheduler: Some(scheduler),
            blobs: Arc::new(Mutex::new(blobs)),
            assistant: crate::assistant::Assistant::default(),
            events,
            automation,
            automation_context: false,
            automation_problem: None,
            rules,
            async_outcomes: BTreeMap::new(),
            active_ai: BTreeMap::new(),
            active_http_accounts: BTreeMap::new(),
        })
    }

    /// Only the trusted native membership path or isolated developer preview enables this.
    /// Keyless serve leaves it false even when the configured device id matches.
    pub fn set_automation_context(&mut self, authorized: bool) {
        self.automation_context = authorized;
    }

    pub fn automations_armed(&self, space: &str) -> Result<bool> {
        self.automation.observe(space, &self.rules.settings(space)?)
    }

    /// Trusted Settings gesture, routed only through Caller::App.
    pub(crate) fn arm_automations(&self, space: &str) -> Result<bool> {
        self.automation
            .arm(space, &self.rules.settings(space)?, &self.cfg.device.0)
    }

    pub(crate) fn grant_rule(&self, rule: &RuleInfo) -> Result<()> {
        if rule.enabled && rule.approved {
            self.automation.grant(rule, now_ms())
        } else {
            self.automation.revoke(&rule.id)
        }
    }

    fn automation_authorized(&self, space: &str) -> Result<bool> {
        let settings = self.rules.settings(space)?;
        let armed = self.automation.observe(space, &settings)?;
        Ok(self.automation_context
            && armed
            && !settings.paused
            && settings.primary_device.as_deref() == Some(&self.cfg.device.0))
    }

    /// A bounded snapshot; child events are intentionally left to the next pass.
    pub fn run_rules(&mut self, now: u64) -> Vec<crate::RuleRunSummary> {
        let mut summaries = Vec::new();
        if let Err(error) = self.run_rules_checked(now, &mut summaries) {
            let message = match error {
                RuntimeError::Plugin { code, .. } if code == "automation_full" => {
                    "Automation history is full"
                }
                _ => "Automation history is unavailable or full",
            };
            self.automation_problem = Some(message.into());
            eprintln!("{message}");
        } else {
            self.automation_problem = None;
        }
        summaries
    }

    pub fn automation_problem(&self) -> Option<&str> {
        self.automation_problem.as_deref()
    }

    fn publish_rule_runs(&mut self) -> Result<Vec<crate::RuleRunSummary>> {
        let mut summaries = Vec::new();
        for (id, mut summary, history) in self.automation.unpublished()? {
            self.rules
                .store_mut()
                .write(|w| w.upsert("runs", &id, history.as_object().expect("host history")))?;
            self.automation.audit(&summary)?;
            self.automation.published(&id)?;
            if summary.status == "failed" {
                summary.notify = self.automation.notify(&summary.rule, summary.at_ms)?;
            }
            summaries.push(summary);
        }
        Ok(summaries)
    }

    fn run_rules_checked(
        &mut self,
        now: u64,
        summaries: &mut Vec<crate::RuleRunSummary>,
    ) -> Result<()> {
        use crate::rules::{self, EVENTS_PER_TICK, MAX_RULES_PER_EVENT, MAX_RULE_DEPTH};
        let now = self.automation.clock(now.min(i64::MAX as u64))?;
        // Rebuild terminal history after interrupted work or a history-write failure.
        summaries.extend(self.publish_rule_runs()?);
        let mut snapshot = Vec::new();
        for event in self.events.pending_at(usize::MAX, now)? {
            if !self.open_spaces.contains(&event.space)
                || !self.automation_authorized(&event.space)?
            {
                continue;
            }
            snapshot.push(event);
            if snapshot.len() == EVENTS_PER_TICK {
                break;
            }
        }
        for event in snapshot {
            let definition = self
                .loaded(&event.plugin)
                .and_then(|p| p.manifest.events.get(&event.name));
            if !definition.is_some_and(|d| {
                crate::events::contract(d) == event.contract
                    && crate::commands::check_args(&d.payload, &[], &event.payload).is_ok()
            }) {
                self.events.mark_handled(&event.id, now)?;
                continue;
            }
            let mut matching = Vec::new();
            for rule in self.rules.list(&event.space)? {
                if !rule.enabled {
                    self.automation.revoke(&rule.id)?;
                    continue;
                }
                let rule = self.rule_info(rule)?;
                if rule.approved
                    && rule.problem.is_none()
                    && self.same_plugin(&rule.trigger.plugin, &event.plugin)
                    && rule.trigger.event == event.name
                    && self.automation.granted(&rule, event.emitted_ms)?
                    && rules::matches(&rule.filter, &event.payload)
                {
                    matching.push(rule);
                }
            }
            matching.sort_by(|a, b| a.id.cmp(&b.id));
            let mut complete = true;
            for (index, rule) in matching.into_iter().enumerate() {
                let id = format!("{}@{}", rule.id, event.id);
                if self.automation.has_claim(&id)? {
                    continue;
                }
                let existing = self.rules.store().query(
                    "SELECT status FROM runs WHERE id=?1 AND _deleted IS NULL",
                    &[json!(id)],
                )?;
                if existing
                    .first()
                    .and_then(|r| r["status"].as_str())
                    .is_some_and(|s| matches!(s, "done" | "failed" | "skipped" | "stopped"))
                {
                    continue;
                }
                // Re-resolve consent and settings immediately before claiming/invoking.
                let rule = self.rule_info(self.rules.get(&rule.id)?.expect("listed rule"))?;
                if !self.automation_authorized(&event.space)? {
                    complete = false;
                    break;
                }
                if !rule.enabled
                    || !rule.approved
                    || rule.problem.is_some()
                    || !self.automation.granted(&rule, event.emitted_ms)?
                {
                    continue;
                }
                let stop = if event.depth >= MAX_RULE_DEPTH {
                    Some(("stopped", "stopped"))
                } else if index >= MAX_RULES_PER_EVENT {
                    Some(("stopped", "fanout"))
                } else if self.automation.limited(&rule.id, now)? {
                    Some(("skipped", "rate"))
                } else {
                    None
                };
                self.automation.audit_ready()?;
                let mut summary = crate::RuleRunSummary {
                    at_ms: now,
                    space: event.space.clone(),
                    rule: rule.id.clone(),
                    event: event.id.clone(),
                    plugin: rule.action.plugin.clone(),
                    operation: rule.action.operation.clone(),
                    status: stop.map_or("failed", |s| s.0).into(),
                    code: Some(stop.map_or("interrupted", |s| s.1).into()),
                    change_count: 0,
                    notify: false,
                };
                let history = |s: &crate::RuleRunSummary| json!({"space":s.space,"rule":s.rule,"rule_name":rule.name,"digest":rules::digest(&rule),"event":s.event,"status":s.status,"device":self.cfg.device.0,"error":s.code,"started_ms":now,"finished_ms":now});
                let mut row = history(&summary);
                self.automation
                    .claim(&id, &rule, now, stop.is_none(), &row, &summary)?;
                if stop.is_none() {
                    let args = rules::map_args(&rule.action.args, &event);
                    let op = &self
                        .loaded(&rule.action.plugin)
                        .expect("validated rule")
                        .manifest
                        .operations[&rule.action.operation];
                    let validation = crate::commands::check_args(
                        op.params.as_ref().expect("validated rule"),
                        &[],
                        &args,
                    );
                    let (result, changes) = if validation.is_err() {
                        (
                            Err(RuntimeError::Plugin {
                                code: "invalid".into(),
                                message: "Invalid automation arguments".into(),
                            }),
                            None,
                        )
                    } else {
                        self.call_with(
                            &event.space,
                            &rule.action.plugin,
                            &rule.action.operation,
                            &Value::Object(args),
                            None,
                            false,
                            true,
                            event.depth + 1,
                            Some(event.root.clone()),
                        )
                    };
                    summary.change_count = changes.unwrap_or_default().len();
                    let (status, code) = match result {
                        Ok(_) => ("done", None),
                        Err(RuntimeError::Plugin { code, .. }) if code == "skip" => {
                            ("skipped", Some("skip"))
                        }
                        Err(_) => (
                            "failed",
                            Some(if validation.is_err() {
                                "invalid"
                            } else {
                                "failed"
                            }),
                        ),
                    };
                    summary.status = status.into();
                    summary.code = code.map(str::to_owned);
                    row["status"] = json!(summary.status);
                    row["error"] = json!(summary.code);
                    self.automation.finish(&id, &summary, &row)?;
                }
                summaries.extend(self.publish_rule_runs()?);
            }
            if complete {
                self.events.mark_handled(&event.id, now)?;
            }
        }
        self.prune_rule_runs(now)?;
        Ok(())
    }

    fn prune_rule_runs(&mut self, now: u64) -> Result<()> {
        // Hourly cadence is local and durable; deleting synced history never removes claims.
        let path = self.cfg.data_dir.join("automation-pruned.json");
        let last = std::fs::read(&path)
            .ok()
            .and_then(|v| serde_json::from_slice::<u64>(&v).ok());
        if last.is_some_and(|last| now.saturating_sub(last) < 3_600_000) {
            return Ok(());
        }
        let rows = self.rules.store().query("SELECT id,rule,started_ms FROM runs WHERE _deleted IS NULL ORDER BY started_ms DESC,id DESC", &[])?;
        let mut counts = BTreeMap::<String, usize>::new();
        let mut remove = Vec::new();
        for r in rows {
            let count = counts
                .entry(r["rule"].as_str().unwrap_or("").to_owned())
                .or_default();
            *count += 1;
            if *count > 50
                || r["started_ms"]
                    .as_u64()
                    .is_none_or(|at| at < now.saturating_sub(30 * 24 * 3_600_000))
            {
                if let Some(id) = r["id"].as_str() {
                    remove.push(id.to_owned());
                }
            }
        }
        self.rules.store_mut().write(|w| {
            for id in remove {
                w.delete("runs", &id)?;
            }
            Ok(())
        })?;
        std::fs::write(&path, serde_json::to_vec(&now)?).map_err(|e| RuntimeError::io(&path, e))?;
        Ok(())
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }
    pub fn rules_mut(&mut self) -> &mut Rules {
        &mut self.rules
    }

    /// Revalidate synced configuration and resolve consent against the loaded code.
    pub fn rule_info(&self, mut rule: RuleInfo) -> Result<RuleInfo> {
        if rule.problem.is_none() {
            rule.problem = crate::rules::check(self, &rule.input()).err();
        }
        rule.source_package = self
            .loaded(&rule.trigger.plugin)
            .map(|p| p.identity.clone())
            .unwrap_or(Value::Null);
        rule.target_package = self
            .loaded(&rule.action.plugin)
            .map(|p| p.identity.clone())
            .unwrap_or(Value::Null);
        rule.approved = rule.problem.is_none() && self.rules.is_approved(&rule)?;
        let history = self.rules.runs(&rule.space, Some(&rule.id), 1, None)?;
        rule.last_run = history["runs"].as_array().and_then(|a| a.first()).cloned();
        let settings = self.rules.settings(&rule.space)?;
        self.automation.observe(&rule.space, &settings)?;
        if !rule.enabled {
            self.automation.revoke(&rule.id)?;
        }
        if settings.primary_device.as_deref() != Some(&self.cfg.device.0) {
            rule.waiting = self
                .pending_events(usize::MAX)?
                .iter()
                .filter(|e| {
                    e.space == rule.space
                        && self.same_plugin(&e.plugin, &rule.trigger.plugin)
                        && e.name == rule.trigger.event
                })
                .count() as u64;
        }
        Ok(rule)
    }

    /// The space list (synced with the other devices).
    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    pub fn spaces_mut(&mut self) -> &mut Spaces {
        &mut self.spaces
    }

    /// User pages and their widget placements (synced with the other devices).
    pub fn pages(&self) -> &Pages {
        &self.pages
    }

    pub fn pages_mut(&mut self) -> &mut Pages {
        &mut self.pages
    }

    /// Host-owned item relations (synced with the other devices).
    pub fn links(&self) -> &Links {
        &self.links
    }

    pub fn links_mut(&mut self) -> &mut Links {
        &mut self.links
    }

    /// Spaces whose data is open now.
    pub fn open_spaces(&self) -> impl Iterator<Item = &str> {
        self.open_spaces.iter().map(String::as_str)
    }

    fn space_dir(&self, space: &str) -> PathBuf {
        if space == DEFAULT_SPACE {
            return self.cfg.data_dir.clone();
        }
        self.cfg
            .spaces_dir
            .clone()
            .unwrap_or_else(|| self.cfg.data_dir.join("spaces"))
            .join(space)
            .join("plugins")
    }

    fn open_store(&self, space: &str, manifest: &Manifest) -> Result<CoreStore> {
        let dir = self.space_dir(space);
        std::fs::create_dir_all(&dir).map_err(|e| RuntimeError::io(&dir, e))?;
        // A renamed plugin keeps its first id's database and sync id.
        let db = dir.join(format!("{}.db", manifest.storage_id()));
        // After a rollback the database may be ahead of this version's
        // schema. Migrations only add, so keep the newer schema.
        let schema = match CoreStore::stored_schema(&db)? {
            Some(stored) if stored.version > manifest.storage.version => stored,
            _ => manifest.storage.clone(),
        };
        let cfg = store_config(&self.cfg, &store_id(space, manifest.storage_id()));
        Ok(CoreStore::open(db, cfg, schema, Box::new(SystemTime))?)
    }

    /// Opens and closes spaces to match the space list. Returns whether
    /// anything changed. Closed spaces keep their data on disk.
    pub fn sync_spaces(&mut self) -> Result<bool> {
        // Peer uninstall is authoritative even if this device has no loaded package.
        let removed: Vec<_> = self
            .packages
            .installed()?
            .into_iter()
            .filter(|p| p.removed)
            .map(|p| p.id)
            .collect();
        for plugin in removed {
            self.remove_plugin_accounts(&plugin)
                .map_err(RuntimeError::Manifest)?;
        }
        let want: BTreeSet<String> = self.spaces.active()?.into_iter().collect();
        let mut changed = false;
        for space in self.open_spaces.clone() {
            if !want.contains(&space) {
                for loaded in self.plugins.values_mut() {
                    loaded.stores.remove(&space);
                }
                self.index.as_mut().expect("index available").close(&space);
                self.vaults.remove(&space);
                if let Some(net) = self.net_mut() {
                    net.clear_space(&space);
                }
                self.open_spaces.remove(&space);
                changed = true;
            }
        }
        for space in want {
            if self.open_spaces.contains(&space) {
                continue;
            }
            let mut vault = terrarium_net::vault::Vault::open(
                &self.space_dir(&space),
                store_config(
                    &self.cfg,
                    &store_id(&space, terrarium_net::vault::VAULT_STORE_ID),
                ),
                &space,
            )
            .map_err(|e| RuntimeError::Manifest(e.message))?;
            if let Some(sealer) = &self.vault_sealer {
                vault.set_sealer(sealer.clone());
            }
            self.vaults.insert(space.clone(), vault);
            let mut opened = Vec::new();
            for (id, loaded) in &self.plugins {
                opened.push((id.clone(), self.open_store(&space, &loaded.manifest)?));
            }
            for (id, store) in opened {
                if let Some(loaded) = self.plugins.get_mut(&id) {
                    loaded.stores.insert(space.clone(), Some(store));
                }
            }
            self.open_spaces.insert(space.clone());
            let ids: Vec<String> = self.plugins.keys().cloned().collect();
            for id in ids {
                self.sync_jobs(&space, &id);
            }
            changed = true;
        }
        Ok(changed)
    }

    /// The installed plugin set (synced with the other devices).
    pub fn packages(&self) -> &Packages {
        &self.packages
    }

    pub fn packages_mut(&mut self) -> &mut Packages {
        &mut self.packages
    }

    /// Stops running a plugin. Its data stays on disk.
    pub fn unload(&mut self, plugin: &str) -> bool {
        // Exact id only: unloading a former id never stops its successor.
        let removed = self.plugins.remove(plugin).is_some();
        if let Some(net) = self.net_mut() {
            net.clear(Some(plugin));
        }
        for space in self.open_spaces.clone() {
            if let Err(e) = self
                .index
                .as_mut()
                .expect("index available")
                .forget(&space, plugin)
            {
                eprintln!("index {space}/{plugin}: {e}");
            }
        }
        removed
    }

    /// Loads (or reloads) the built plugin package at `dir`: its manifest,
    /// `dist/plugin.wasm`, templates and storage. A reload with a higher
    /// storage version runs the additive migration.
    pub fn load(&mut self, dir: &Path) -> Result<&Manifest> {
        let manifest = Manifest::read(dir)?;
        let package = crate::Package::from_dir(dir)?;
        let identity = serde_json::json!({"digest":package.digest(),"signer":package.signer().unwrap_or("unsigned")});
        let wasm_path = dir.join("dist").join("plugin.wasm");
        let code = match manifest.kind {
            Kind::Wasm => {
                let bytes =
                    std::fs::read(&wasm_path).map_err(|e| RuntimeError::io(&wasm_path, e))?;
                Code::Wasm(self.runtime.prepare(&bytes)?)
            }
            Kind::Declarative => {
                if std::fs::symlink_metadata(&wasm_path).is_ok() {
                    return Err(RuntimeError::Manifest(
                        "declarative plugins cannot contain dist/plugin.wasm".into(),
                    ));
                }
                Code::Declarative
            }
        };

        let mut templates = BTreeMap::new();
        for (kind, views) in [
            (ViewKind::Page, &manifest.pages),
            (ViewKind::Widget, &manifest.widgets),
        ] {
            for view in views {
                let path = dir.join(&view.template);
                let text =
                    std::fs::read_to_string(&path).map_err(|e| RuntimeError::io(&path, e))?;
                let template: Value = serde_json::from_str(&text)
                    .map_err(|e| RuntimeError::Manifest(format!("{}: {e}", path.display())))?;
                templates.insert((kind, view.id.clone()), template);
            }
        }

        // A plugin may not take another loaded plugin's id or storage: only
        // a successor (naming it in `former_ids`) replaces it.
        for (other_id, other) in &self.plugins {
            if *other_id == manifest.id || manifest.former_ids.contains(other_id) {
                continue;
            }
            if other.manifest.answers_to(&manifest.id) {
                return Err(RuntimeError::Manifest(format!(
                    "`{}` was renamed to `{other_id}`, which is already loaded",
                    manifest.id
                )));
            }
            if let Some(clash) = manifest
                .former_ids
                .iter()
                .find(|f| other.manifest.former_ids.contains(f))
            {
                return Err(RuntimeError::Manifest(format!(
                    "former id `{clash}` is also a former id of `{other_id}`"
                )));
            }
        }
        // Release the previous connections before migrating the same files,
        // including a predecessor this plugin was renamed from.
        self.unload(&manifest.id);
        for former in manifest.former_ids.clone() {
            if self.plugins.contains_key(&former) {
                self.unload(&former);
            }
        }
        let mut stores = BTreeMap::new();
        for space in &self.open_spaces {
            stores.insert(space.clone(), Some(self.open_store(space, &manifest)?));
        }
        let store = stores[DEFAULT_SPACE].as_ref().expect("just opened");

        for (name, query) in &manifest.queries {
            let expected = store
                .check_query(&query.sql)
                .map_err(|e| RuntimeError::Manifest(format!("query `{name}`: {e}")))?;
            if expected != query.params.len() {
                return Err(RuntimeError::Manifest(format!(
                    "query `{name}` takes {expected} parameters but declares {}",
                    query.params.len()
                )));
            }
        }

        let id = manifest.id.clone();
        self.plugins.insert(
            id.clone(),
            LoadedPlugin {
                manifest,
                identity,
                dir: dir.to_path_buf(),
                code,
                stores,
                templates,
            },
        );
        for space in self.open_spaces.clone() {
            self.sync_jobs(&space, &id);
        }
        Ok(&self.plugins[&id].manifest)
    }

    /// One bounded round over indexed plugins in every open space.
    /// Returns true while work remains.
    pub fn index_step(&mut self, budget: usize) -> bool {
        let plugins: Vec<_> = self
            .manifests()
            .filter(|m| !m.index.is_empty())
            .map(|m| m.id.clone())
            .collect();
        let pairs: Vec<_> = self
            .open_spaces()
            .flat_map(|space| {
                plugins
                    .iter()
                    .map(move |id| (space.to_string(), id.clone()))
            })
            .collect();
        let mut remaining = budget;
        let mut more = false;
        for (space, plugin) in pairs {
            if remaining == 0 {
                return true;
            }
            match self.index_catch_up(&space, &plugin, remaining) {
                Ok(progress) => {
                    remaining = remaining.saturating_sub(progress.rows);
                    more |= !progress.done;
                }
                Err(e) => eprintln!("index {space}/{plugin}: {e}"),
            }
        }
        more
    }

    /// Runs `f` with the search index lent out, so it can read plugins.
    pub(crate) fn with_index<T>(
        &mut self,
        f: impl FnOnce(&mut crate::index::SearchIndex, &PluginHost) -> T,
    ) -> T {
        let mut index = self.index.take().expect("search index lent twice");
        let out = f(&mut index, self);
        self.index = Some(index);
        out
    }

    /// Brings `plugin`'s part of `space`'s search index up to date,
    /// handling at most about `budget` changed rows.
    pub fn index_catch_up(
        &mut self,
        space: &str,
        plugin: &str,
        budget: usize,
    ) -> Result<crate::index::IndexProgress> {
        self.with_index(|index, host| crate::index::catch_up(index, host, space, plugin, budget))
    }

    /// `(item, kind)` of every indexed row of `plugin` in `space`.
    pub fn indexed_items(&mut self, space: &str, plugin: &str) -> Result<Vec<(String, String)>> {
        let plugin = self.canonical(plugin).to_string();
        self.with_index(|index, _| index.items(space, &plugin))
    }

    /// The user's IANA timezone.
    pub fn timezone(&self) -> &str {
        &self.cfg.timezone
    }

    /// `"native"` or `"interpreter"`: how this host runs plugin code.
    pub fn execution_mode(&self) -> &'static str {
        self.runtime.mode()
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.plugins.values().map(|p| &p.manifest)
    }

    pub fn package_dir(&self, plugin: &str) -> Option<&Path> {
        self.loaded(plugin).map(|p| p.dir.as_path())
    }

    pub fn template(&self, plugin: &str, kind: ViewKind, id: &str) -> Result<&Value> {
        let loaded = self.get(plugin)?;
        loaded
            .templates
            .get(&(kind, id.to_string()))
            .ok_or_else(|| RuntimeError::Undeclared {
                plugin: plugin.into(),
                kind: match kind {
                    ViewKind::Page => "page",
                    ViewKind::Widget => "widget",
                },
                name: id.into(),
            })
    }

    /// [`PluginHost::query_in`] the default space.
    pub fn query(
        &self,
        plugin: &str,
        name: &str,
        values: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>> {
        self.query_in(DEFAULT_SPACE, plugin, name, values)
    }

    /// Runs a manifest-declared query, binding `values` by parameter name.
    pub fn query_in(
        &self,
        space: &str,
        plugin: &str,
        name: &str,
        values: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>> {
        let loaded = self.get(plugin)?;
        let query = loaded
            .manifest
            .queries
            .get(name)
            .ok_or_else(|| RuntimeError::Undeclared {
                plugin: plugin.into(),
                kind: "query",
                name: name.into(),
            })?;
        let checked_values;
        let values = if query
            .view
            .as_ref()
            .is_some_and(|view| view.version == Some(1))
        {
            checked_values = validate_view_query_values(query, values)?;
            &checked_values
        } else {
            values
        };
        let params: Vec<Value> = query
            .params
            .iter()
            .map(|p| values.get(p).cloned().unwrap_or(Value::Null))
            .collect();
        Ok(loaded.store(space)?.query_bounded(&query.sql, &params)?)
    }

    /// [`PluginHost::call_in`] the default space.
    pub fn call(&mut self, plugin: &str, operation: &str, args: &Value) -> Result<Value> {
        self.call_in(DEFAULT_SPACE, plugin, operation, args)
    }

    /// Runs a manifest-declared operation against one space's data.
    pub fn call_in(
        &mut self,
        space: &str,
        plugin: &str,
        operation: &str,
        args: &Value,
    ) -> Result<Value> {
        self.call_for_run(space, plugin, operation, args, None)
    }

    /// [`PluginHost::call_in`], also returning the changes the call made
    /// in every plugin it reached. With `dry_run` nothing is kept: every
    /// store is rolled back and no schedule changes or AI requests remain.
    pub fn call_traced(
        &mut self,
        space: &str,
        plugin: &str,
        operation: &str,
        args: &Value,
        dry_run: bool,
    ) -> Result<(Value, Vec<ChangeSummary>)> {
        let (result, trace) =
            self.call_with(space, plugin, operation, args, None, dry_run, true, 0, None);
        result.map(|v| (v, trace.unwrap_or_default()))
    }

    /// [`PluginHost::call_in`] on behalf of scheduled run `run`, so AI
    /// requests it makes are tied to the run.
    fn call_for_run(
        &mut self,
        space: &str,
        plugin: &str,
        operation: &str,
        args: &Value,
        run: Option<String>,
    ) -> Result<Value> {
        if run.is_some() {
            self.validate_schedule_target(plugin)?;
        }
        self.call_with(space, plugin, operation, args, run, false, false, 0, None)
            .0
    }

    #[allow(clippy::too_many_arguments)]
    fn call_with(
        &mut self,
        space: &str,
        plugin: &str,
        operation: &str,
        args: &Value,
        run: Option<String>,
        dry_run: bool,
        trace: bool,
        event_depth: u8,
        event_root: Option<String>,
    ) -> (Result<Value>, Option<Vec<ChangeSummary>>) {
        if !self.open_spaces.contains(space) {
            return (Err(RuntimeError::UnknownSpace(space.into())), None);
        }
        let peers = Peers {
            space: space.to_string(),
            runtime: Some(self.runtime.clone()),
            timezone: self.cfg.timezone.clone(),
            speech: self.cfg.speech.clone(),
            plugins: std::mem::take(&mut self.plugins),
            ai: self.ai.take(),
            net: self.net.take(),
            scheduler: self.scheduler.take(),
            run,
            dry_run,
            trace: trace.then(Vec::new),
            events: Vec::new(),
            event_count: 0,
            event_depth,
            event_root,
            device: self.cfg.device.0.clone(),
        };
        let plugin = resolve(&peers.plugins, plugin)
            .unwrap_or(plugin)
            .to_string();
        let (peers, result) = invoke(peers, &plugin, operation, args, 0);
        self.plugins = peers.plugins;
        self.ai = peers.ai;
        self.net = peers.net;
        self.scheduler = peers.scheduler;
        for (_, event) in peers.events {
            if self.events.record(&event).is_err() {
                eprintln!("Event delivery failed after plugin commit");
            }
        }
        (result, peers.trace)
    }

    /// Valid, unexpired events across spaces, with source contracts revalidated.
    pub fn pending_events(&self, limit: usize) -> Result<Vec<EventRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut valid = Vec::new();
        for event in self.events.pending(usize::MAX)? {
            let definition = self
                .loaded(&event.plugin)
                .and_then(|p| p.manifest.events.get(&event.name));
            if definition.is_some_and(|definition| {
                crate::events::contract(definition) == event.contract
                    && crate::commands::check_args(&definition.payload, &[], &event.payload).is_ok()
            }) {
                if valid.len() == limit {
                    break;
                }
                valid.push(event);
            } else {
                eprintln!("Invalid event contract or payload; event skipped");
            }
        }
        Ok(valid)
    }

    pub fn mark_event_handled(&mut self, id: &str) -> Result<()> {
        self.events.mark_handled(id, now_ms())
    }

    pub fn prune_events(&mut self, now_ms: u64) -> Result<usize> {
        self.events.prune(now_ms.saturating_sub(EVENT_RETENTION_MS))
    }

    /// HTTP-03 installs the worker's queue; headless hosts have no worker.
    pub fn set_net(&mut self, net: Option<terrarium_net::Net>) {
        self.net = net;
    }
    pub fn net(&self) -> Option<&terrarium_net::Net> {
        self.net.as_ref()
    }
    pub fn net_mut(&mut self) -> Option<&mut terrarium_net::Net> {
        self.net.as_mut()
    }

    /// Creates private counters/audit next to this host's device-local data.
    pub fn enable_http(&mut self) -> std::result::Result<(), String> {
        self.net = Some(terrarium_net::Net::open(&self.cfg.data_dir).map_err(|e| e.message)?);
        Ok(())
    }
    fn http_identity(&self, job: &terrarium_net::Job) -> bool {
        let Some(loaded) = self.loaded(&job.plugin) else {
            return false;
        };
        if !self.open_spaces.contains(&job.space)
            || !loaded.manifest.permissions.iter().any(|p| p == "http")
        {
            return false;
        }
        let Ok(rules) = terrarium_net::HostRules::new(&loaded.manifest.hosts) else {
            return false;
        };
        if rules.patterns() != job.allowed.patterns() {
            return false;
        }
        self.http_package_approved(&job.plugin)
    }
    fn http_package_approved(&self, plugin: &str) -> bool {
        let Some(loaded) = self.loaded(plugin) else {
            return false;
        };
        let Ok(installed) = self.packages.installed() else {
            return false;
        };
        let record = installed
            .iter()
            .find(|p| p.id == loaded.manifest.id)
            .or_else(|| installed.iter().find(|p| loaded.manifest.answers_to(&p.id)));
        if let Some(current) = record {
            if current.removed {
                return false;
            }
            if current.digest != crate::BUNDLED_DIGEST {
                let Ok(Some(dir)) = self
                    .packages
                    .materialize_approved(current, cfg!(debug_assertions))
                else {
                    return false;
                };
                return dir == loaded.dir;
            }
        }
        // Explicit load() callers (including preview) authorize dev/bundled packages.
        true
    }
    pub fn http_job_current(&self, job: &terrarium_net::Job) -> bool {
        self.net().is_some_and(|n| n.is_live(&job.id))
            && self.http_identity(job)
            && self.active_http_accounts.get(&job.id).is_none_or(|lease| {
                self.vaults
                    .get(&job.space)
                    .is_some_and(|v| v.current(lease))
            })
    }
    pub fn take_http_job(
        &mut self,
    ) -> Option<(
        terrarium_net::Job,
        Option<terrarium_net::vault::AccountLease>,
    )> {
        loop {
            let mut job = self.net_mut()?.take()?;
            if self.http_identity(&job) {
                let auth = if let Some(auth) = &job.request.auth {
                    let result = (|| {
                        let error = || terrarium_net::NetError {
                            code: "unavailable".into(),
                            message: vault_unavailable(),
                            temporary: false,
                        };
                        let (rules, signer) =
                            self.vault_authority(&job.plugin).map_err(|_| error())?;
                        let host = url_host(&job.request.url).map_err(|_| error())?;
                        let lease = self.vaults.get(&job.space).ok_or_else(error)?.lease(
                            &job.plugin,
                            &auth.account,
                            &host,
                            &rules,
                            &signer,
                        )?;
                        if lease.is_oauth() && lease.provider().is_none() {
                            return Err(error());
                        }
                        if let Some(provider) = lease.provider() {
                            if !self
                                .loaded(&job.plugin)
                                .is_some_and(|p| p.manifest.oauth.values().any(|d| d == provider))
                            {
                                return Err(error());
                            }
                        }
                        Ok(lease)
                    })();
                    match result {
                        Ok(lease) => Some(lease),
                        Err(error) => {
                            job.dispatch_error = Some(error);
                            return Some((job, None));
                        }
                    }
                } else {
                    None
                };
                if let Some(lease) = &auth {
                    self.active_http_accounts
                        .insert(job.id.clone(), lease.clone());
                }
                return Some((job, auth));
            }
            let net = self.net_mut()?;
            net.cancel(&job.id);
            net.finish(&job.id);
        }
    }
    pub fn discard_http_job(&mut self, id: &str) {
        self.active_http_accounts.remove(id);
        if let Some(net) = self.net_mut() {
            net.cancel(id);
            net.finish(id);
        }
    }
    pub fn deliver_http(
        &mut self,
        job: terrarium_net::Job,
        result: std::result::Result<terrarium_net::Response, terrarium_net::NetError>,
    ) -> Option<Finished> {
        let live = self.http_job_current(&job);
        self.net_mut()?.finish(&job.id);
        let account = self.active_http_accounts.remove(&job.id);
        if live && result.as_ref().is_err_and(|e| e.code == "unauthorized") {
            if let Some(lease) = &account {
                if let Some(vault) = self.vaults.get_mut(&job.space) {
                    let _ = vault.deactivate(lease);
                }
            }
        }
        if !live {
            return None;
        }
        let mut payload = match &result {
            Ok(r) => {
                serde_json::json!({"request":job.id,"ok":true,"status":r.status,"headers":r.headers,"body":r.body,"context":job.request.context})
            }
            Err(e) => {
                serde_json::json!({"request":job.id,"ok":false,"error":{"code":e.code,"message":e.message,"temporary":e.temporary},"context":job.request.context})
            }
        };
        let mut failure = result.as_ref().err().map(|e| {
            if e.temporary {
                Outcome::Retry(e.message.clone())
            } else {
                Outcome::Failed(e.message.clone())
            }
        });
        if payload.to_string().len() > 4 * 1024 * 1024 {
            payload = serde_json::json!({"request":job.id,"ok":false,"error":{"code":"too-large","message":"The reply is too large.","temporary":false},"context":job.request.context});
            failure = Some(Outcome::Failed("The reply is too large.".into()));
        }
        let call = self.call_for_run(
            &job.space,
            &job.plugin,
            &job.on_done,
            &payload,
            job.run.clone(),
        );
        let outcome = match call {
            Err(_) => Outcome::Failed("Saving the web reply failed.".into()),
            Ok(v) if v["$run"]["status"].as_str() == Some("failed") => {
                Outcome::Failed("The web reply callback failed.".into())
            }
            Ok(v) if v["$run"]["status"].as_str() == Some("retry") => {
                Outcome::Retry("The web reply callback asked to retry.".into())
            }
            Ok(v) => failure.unwrap_or_else(|| Outcome::Done(notice(&v))),
        };
        self.finish_async(job.run?, outcome)
    }
    fn has_async(&self, run: &str) -> bool {
        self.ai().has_run(run)
            || self.active_ai.values().any(|r| r == run)
            || self.net().is_some_and(|n| n.has_run(run))
    }
    fn finish_async(&mut self, run: String, outcome: Outcome) -> Option<Finished> {
        // Retain failures while other replies and callback-submitted work remain.
        let replace = match self.async_outcomes.get(&run) {
            Some(Outcome::Failed(_)) => false,
            Some(Outcome::Retry(_)) => matches!(outcome, Outcome::Failed(_)),
            _ => true,
        };
        if replace {
            self.async_outcomes.insert(run.clone(), outcome);
        }
        if self.has_async(&run) {
            return None;
        }
        let outcome = self.async_outcomes.remove(&run)?;
        self.scheduler_mut()
            .finish(&run, outcome, now_ms())
            .ok()
            .flatten()
    }

    fn vault_authority(
        &self,
        plugin: &str,
    ) -> std::result::Result<(terrarium_net::HostRules, String), String> {
        if !self.http_package_approved(plugin) {
            return Err("This plugin needs local approval.".into());
        }
        let loaded = self.loaded(plugin).ok_or("This plugin is unavailable.")?;
        if !loaded.manifest.permissions.iter().any(|p| p == "http") {
            return Err("This plugin has no network access.".into());
        }
        let rules = terrarium_net::HostRules::new(&loaded.manifest.hosts).map_err(|e| e.message)?;
        // Approved installed directories contain both full digest and signer fingerprint.
        // Bundled/dev identities also bind the exact loaded package generation.
        use sha2::{Digest, Sha256};
        let signer = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{}",
                loaded.dir.display(),
                fingerprint(&loaded.dir)
            ))
        );
        Ok((rules, signer))
    }
    /// Main-window OAuth preparation. The caller performs network I/O outside this lock.
    pub fn prepare_oauth(
        &mut self,
        space: &str,
        plugin: &str,
        name: &str,
        kind: &str,
    ) -> std::result::Result<OAuthPreparation, String> {
        if !self.open_spaces.contains(space) {
            return Err(vault_unavailable());
        }
        let (rules, signer) = self.vault_authority(plugin)?;
        let def = self
            .plugins
            .get(plugin)
            .and_then(|p| p.manifest.oauth.get(kind))
            .cloned()
            .ok_or("Choose a sign-in provider.")?;
        let generation = self
            .vaults
            .get_mut(space)
            .ok_or_else(vault_unavailable)?
            .list(plugin, &signer)
            .map_err(|e| e.message)?
            .into_iter()
            .find(|a| a.name == name)
            .map(|a| a.generation);
        let client = self
            .net_mut()
            .ok_or_else(vault_unavailable)?
            .oauth_client(space, plugin, &def, &rules)
            .map_err(|e| e.message)?;
        Ok(OAuthPreparation {
            provider: def,
            signer,
            generation,
            client,
        })
    }
    pub fn oauth_current(
        &mut self,
        space: &str,
        plugin: &str,
        name: &str,
        identity: (&str, Option<&str>),
        provider: &terrarium_net::oauth::OAuthDef,
    ) -> bool {
        let (signer, generation) = identity;
        self.open_spaces.contains(space)
            && self
                .vault_authority(plugin)
                .is_ok_and(|(_, current)| current == signer)
            && self
                .plugins
                .get(plugin)
                .is_some_and(|p| p.manifest.oauth.values().any(|d| d == provider))
            && self
                .vaults
                .get_mut(space)
                .and_then(|v| v.list(plugin, signer).ok())
                .is_some_and(|list| {
                    list.into_iter()
                        .find(|a| a.name == name)
                        .map(|a| a.generation)
                        .as_deref()
                        == generation
                })
    }
    pub fn finish_oauth(
        &mut self,
        space: &str,
        plugin: &str,
        name: &str,
        identity: (&str, Option<&str>),
        tokens: terrarium_net::vault::TokenSet,
    ) -> std::result::Result<Value, String> {
        let (signer, generation) = identity;
        let (rules, current_signer) = self.vault_authority(plugin)?;
        if !self.open_spaces.contains(space)
            || signer != current_signer
            || !self.loaded(plugin).is_some_and(|p| {
                p.manifest
                    .oauth
                    .values()
                    .any(|d| Some(d) == tokens.provider.as_ref())
            })
        {
            return Err(vault_unavailable());
        }
        let vault = self.vaults.get_mut(space).ok_or_else(vault_unavailable)?;
        let current = vault
            .list(plugin, signer)
            .map_err(|e| e.message)?
            .into_iter()
            .find(|a| a.name == name)
            .map(|a| a.generation);
        if current.as_deref() != generation {
            return Err(vault_unavailable());
        }
        let hosts = tokens
            .provider
            .as_ref()
            .ok_or_else(vault_unavailable)?
            .api_hosts
            .clone();
        vault
            .put_oauth(plugin, name, &hosts, tokens, &rules, signer)
            .map_err(|e| e.message)?;
        if let Some(net) = self.net_mut() {
            net.clear_account(space, plugin, name);
        }
        self.vault_service(
            "vault_accounts",
            &serde_json::json!({"space":space,"plugin":plugin}),
        )
    }
    /// Trusted Settings service entry only; deliberately absent from invoke/commands.
    pub fn vault_service(&mut self, cmd: &str, args: &Value) -> std::result::Result<Value, String> {
        let plugin = args["plugin"].as_str().ok_or("Choose a plugin.")?;
        let space = args["space"].as_str().unwrap_or(DEFAULT_SPACE);
        let (rules, signer) = self.vault_authority(plugin)?;
        let vault = self.vaults.get_mut(space).ok_or_else(vault_unavailable)?;
        let name = args["name"].as_str().unwrap_or_default();
        match cmd {
            "vault_accounts" => (),
            "vault_put_bearer" => {
                let hosts: Vec<String> = serde_json::from_value(args["hosts"].clone())
                    .map_err(|_| "Choose account hosts.")?;
                vault
                    .put_bearer(
                        plugin,
                        name,
                        &hosts,
                        args["token"].as_str().unwrap_or_default(),
                        &rules,
                        &signer,
                    )
                    .map_err(|e| e.message)?;
            }
            "vault_remove" => {
                vault.remove(plugin, name).map_err(|e| e.message)?;
            }
            "vault_approve" => {
                if let Some(provider) = vault.oauth_provider(plugin, name).map_err(|e| e.message)? {
                    if !self
                        .plugins
                        .get(plugin)
                        .is_some_and(|p| p.manifest.oauth.values().any(|d| d == &provider))
                    {
                        return Err(vault_unavailable());
                    }
                }
                // Revalidate manifest before activating a peer account.
                let info = vault
                    .list(plugin, &signer)
                    .map_err(|e| e.message)?
                    .into_iter()
                    .find(|a| a.name == name)
                    .ok_or_else(vault_unavailable)?;
                if !info.readable
                    || info.hosts.iter().any(|h| !rules.allows(h))
                    || (info.kind == "oauth"
                        && vault
                            .oauth_provider(plugin, name)
                            .map_err(|e| e.message)?
                            .is_none())
                {
                    return Err(vault_unavailable());
                }
                vault
                    .approve(
                        plugin,
                        name,
                        &signer,
                        args["generation"].as_str().unwrap_or_default(),
                    )
                    .map_err(|e| e.message)?;
            }
            _ => return Err("Unknown account action.".into()),
        }
        let result = serde_json::to_value(vault.list(plugin, &signer).map_err(|e| e.message)?)
            .map_err(|_| vault_unavailable());
        if cmd != "vault_accounts" {
            if let Some(net) = self.net_mut() {
                net.clear_account(space, plugin, name);
            }
        }
        result
    }
    pub fn remove_plugin_accounts(&mut self, plugin: &str) -> std::result::Result<(), String> {
        if let Some(net) = self.net_mut() {
            net.clear(Some(plugin));
        }
        for vault in self.vaults.values_mut() {
            vault.remove_plugin(plugin).map_err(|e| e.message)?;
        }
        for space in self
            .spaces
            .list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| s.id)
        {
            if !self.vaults.contains_key(&space) {
                let dir = self.space_dir(&space);
                if dir.join("_vault.db").exists() {
                    let mut vault = terrarium_net::vault::Vault::open(
                        &dir,
                        store_config(
                            &self.cfg,
                            &store_id(&space, terrarium_net::vault::VAULT_STORE_ID),
                        ),
                        &space,
                    )
                    .map_err(|e| e.message)?;
                    vault.remove_plugin(plugin).map_err(|e| e.message)?;
                }
            }
        }
        Ok(())
    }
    /// Worker calls this under the host lock before sending and after refresh I/O.
    pub fn update_http_account(
        &mut self,
        job: &terrarium_net::Job,
        lease: &terrarium_net::vault::AccountLease,
        tokens: Option<terrarium_net::vault::TokenSet>,
    ) -> std::result::Result<terrarium_net::vault::AccountLease, terrarium_net::NetError> {
        let error = || terrarium_net::NetError {
            code: "unavailable".into(),
            message: vault_unavailable(),
            temporary: false,
        };
        if !self.http_job_current(job)
            || lease.space() != job.space
            || lease.plugin() != job.plugin
            || job
                .request
                .auth
                .as_ref()
                .is_none_or(|a| a.account != lease.name())
        {
            return Err(error());
        }
        let (rules, signer) = self.vault_authority(&job.plugin).map_err(|_| error())?;
        let host = url_host(&job.request.url).map_err(|_| error())?;
        let vault = self.vaults.get_mut(&job.space).ok_or_else(error)?;
        if !vault.current(lease) {
            return Err(error());
        }
        let current = vault.lease(&job.plugin, lease.name(), &host, &rules, &signer)?;
        let next = if let Some(tokens) = tokens {
            vault.refreshed(&current, tokens)?
        } else {
            current
        };
        self.active_http_accounts
            .insert(job.id.clone(), next.clone());
        Ok(next)
    }

    // ---- AI ------------------------------------------------------------

    pub fn ai(&self) -> &Ai {
        self.ai
            .as_ref()
            .expect("AI service is only lent during a call")
    }

    pub fn ai_mut(&mut self) -> &mut Ai {
        self.ai
            .as_mut()
            .expect("AI service is only lent during a call")
    }

    /// Sets the root that seals the API key (see [`Sealer`]).
    pub fn set_ai_sealer(&mut self, sealer: Sealer) {
        self.ai_mut().set_sealer(sealer.clone());
        for vault in self.vaults.values_mut() {
            vault.set_sealer(sealer.clone());
        }
        self.vault_sealer = Some(sealer);
    }

    /// Sets the key that encrypts local blobs at rest.
    pub fn set_blob_key(&mut self, key: BlobKey) {
        if let Ok(mut blobs) = self.blobs.lock() {
            blobs.set_key(key);
        }
    }

    pub fn blob_store(&self) -> Arc<Mutex<BlobStore>> {
        self.blobs.clone()
    }

    pub fn blobs_gc_path(&self) -> PathBuf {
        self.data_root().join("blobs-gc.json")
    }

    pub fn blob_quota_mb(&self) -> u64 {
        let settings = self.blob_settings();
        settings
            .blob_quota_mb
            .unwrap_or(BLOB_QUOTA_DEFAULT_MB)
            .clamp(1, BLOB_QUOTA_MAX_MB)
    }

    pub fn blob_quota_bytes(&self) -> u64 {
        self.blob_quota_mb() * 1024 * 1024
    }

    pub fn set_blob_quota_mb(&mut self, quota_mb: u64) {
        let mut settings = self.blob_settings();
        settings.blob_quota_mb = Some(quota_mb.clamp(1, BLOB_QUOTA_MAX_MB));
        self.save_blob_settings(&settings);
    }

    pub fn blob_usage_bytes(&self) -> u64 {
        self.blob_store()
            .lock()
            .map(|store| store.total_bytes())
            .unwrap_or(0)
    }

    pub fn has_closed_spaces(&self) -> bool {
        let active = self.spaces.active().map(|s| s.len()).unwrap_or_default();
        active > self.open_spaces.len()
    }

    pub fn blob_gc_collect(&self, now_ms: u64) -> crate::blobs_gc::GcReport {
        crate::blobs_gc::collect(self, now_ms)
    }

    pub fn blob_gc_collect_now(&self, now_ms: u64) -> crate::blobs_gc::GcReport {
        crate::blobs_gc::collect_now(self, now_ms)
    }

    pub fn referenced_blob_ids(&self) -> BTreeSet<String> {
        crate::blobs_gc::referenced(self)
            .into_iter()
            .map(|id| id.to_string())
            .collect()
    }

    /// The next queued AI request, if a worker slot is free. Run it with
    /// [`terrarium_ai::complete`] without holding the host, then hand the
    /// result to [`PluginHost::deliver_ai`].
    pub fn take_ai_job(&mut self) -> Option<(Job, AiConfig, Option<String>)> {
        let next = self.ai_mut().take()?;
        if let Some(run) = &next.0.run {
            self.active_ai.insert(next.0.id.clone(), run.clone());
        }
        if next.0.plugin == crate::assistant::PLUGIN {
            self.assistant.begin(&next.0);
        }
        Some(next)
    }

    /// Records an AI reply, passes it to the plugin's `on_done` operation,
    /// and ends the scheduled run it belonged to.
    pub fn deliver_ai(
        &mut self,
        job: Job,
        result: std::result::Result<Completion, AiError>,
    ) -> Option<Finished> {
        self.active_ai.remove(&job.id);
        self.ai_mut().finish(&result);
        if job.plugin == crate::assistant::PLUGIN {
            self.assistant.finish(&job, result);
            return None;
        }
        let payload = match &result {
            Ok(c) => serde_json::json!({
                "request": job.id,
                "ok": true,
                "text": c.text,
                "model": c.model,
                "context": job.context,
            }),
            Err(e) => serde_json::json!({
                "request": job.id,
                "ok": false,
                "error": e,
                "context": job.context,
            }),
        };
        let call = self.call_for_run(
            &job.space,
            &job.plugin,
            &job.on_done,
            &payload,
            job.run.clone(),
        );
        if let Err(e) = &call {
            eprintln!("AI reply to {}/{} failed: {e}", job.plugin, job.on_done);
        }
        let run = job.run?;
        let outcome = match (&result, &call) {
            (_, Err(e)) => Outcome::Failed(format!("Saving the AI reply failed: {e}")),
            (_, Ok(v)) if v["$run"]["status"].as_str() == Some("failed") => {
                Outcome::Failed(v["$run"]["message"].as_str().unwrap_or("").to_string())
            }
            (_, Ok(v)) if v["$run"]["status"].as_str() == Some("retry") => {
                Outcome::Retry("The AI reply callback asked to retry.".into())
            }
            (Err(e), _) if e.temporary => Outcome::Retry(e.message.clone()),
            (Err(e), _) => Outcome::Failed(e.message.clone()),

            (Ok(_), Ok(v)) => Outcome::Done(notice(v)),
        };
        self.finish_async(run, outcome)
    }

    pub(crate) fn assistant_submit(&mut self, args: &Value) -> std::result::Result<Value, String> {
        let input: crate::assistant::Input =
            serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        input.validate()?;
        if !self.open_spaces.contains(&input.space) {
            return Err("This space is unavailable.".into());
        }
        let (context, sources) = if input.include_today {
            let attached = self.assistant_context(&input.space)?;
            let sources = attached["sources"].as_str().unwrap_or_default().to_string();
            (attached, sources)
        } else {
            (
                Value::Null,
                "Conversation messages only; no new app data attached.".into(),
            )
        };
        self.assistant.submit(
            self.ai.as_mut().expect("AI available outside plugin calls"),
            input,
            context,
            sources,
        )
    }

    pub(crate) fn assistant_context(&self, space: &str) -> std::result::Result<Value, String> {
        if !self.open_spaces.contains(space) {
            return Err("This space is unavailable.".into());
        }
        let today = crate::time::local_date(now_ms(), &self.cfg.timezone)?;
        let params = serde_json::json!({"today": today, "filter": "today", "q": ""})
            .as_object()
            .unwrap()
            .clone();
        let rows = self
            .query_in(space, "tasks", "tasks", &params)
            .map_err(|_| {
                "Task manager is unavailable in this space. Uncheck today's tasks and try again."
                    .to_string()
            })?;
        let tasks: Vec<Value> = rows
            .into_iter()
            .filter(|r| {
                !r.get("tags")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .split(',')
                    .any(|tag| {
                        tag.trim()
                            .trim_start_matches('#')
                            .eq_ignore_ascii_case("private")
                    })
            })
            .take(100)
            .map(|r| serde_json::json!({"title": r.get("title"), "due": r.get("due")}))
            .collect();
        let sources = format!(
            "{} task titles and due dates; #private excluded (up to 100).",
            tasks.len()
        );
        Ok(serde_json::json!({"date": today, "tasks": tasks, "sources": sources}))
    }

    pub(crate) fn assistant_reply(
        &self,
        space: &str,
        id: &str,
    ) -> std::result::Result<Value, String> {
        if !self.open_spaces.contains(space) {
            return Err("This space is unavailable.".into());
        }
        self.assistant.get(space, id)
    }

    // ---- Scheduler -----------------------------------------------------

    pub fn scheduler(&self) -> &Scheduler {
        self.scheduler
            .as_ref()
            .expect("scheduler is only lent during a call")
    }

    pub fn scheduler_mut(&mut self) -> &mut Scheduler {
        self.scheduler
            .as_mut()
            .expect("scheduler is only lent during a call")
    }

    /// Declarative v1 deliberately excludes background execution.
    pub(crate) fn validate_schedule_target(&self, plugin: &str) -> Result<()> {
        if self
            .loaded(plugin)
            .is_some_and(|p| p.manifest.kind == Kind::Declarative)
        {
            return Err(RuntimeError::Plugin {
                code: "invalid".into(),
                message: "Simple plugins cannot run on a schedule".into(),
            });
        }
        Ok(())
    }

    /// Makes the manifest's jobs schedules of `plugin` in `space`.
    fn sync_jobs(&mut self, space: &str, plugin: &str) {
        let Some(loaded) = self.loaded(plugin) else {
            return;
        };
        // Schedules are synced rows: keyed by the storage id.
        let plugin = loaded.manifest.storage_id().to_string();
        let plugin = plugin.as_str();
        let jobs: Vec<(String, ScheduleInput)> = loaded
            .manifest
            .jobs
            .iter()
            .map(|(name, job)| (name.clone(), job_input(name, job)))
            .collect();
        let keys: Vec<String> = jobs.iter().map(|(k, _)| k.clone()).collect();
        let scheduler = self.scheduler_mut();
        for (key, input) in jobs {
            if let Err(e) = scheduler.ensure(space, plugin, &key, &input) {
                eprintln!("job {plugin}/{key}: {e}");
            }
        }
        let _ = scheduler.retain_manifest(space, plugin, &keys);
    }

    /// Runs every scheduled run due on this device now, one after another.
    /// `desktop`: whether this device is a computer. Returns the runs that
    /// ended (for notifications).
    pub fn run_due(&mut self, now: u64, desktop: bool) -> Vec<Finished> {
        let loaded: BTreeSet<(String, String)> = self
            .open_spaces
            .iter()
            .flat_map(|s| {
                self.plugins.values().flat_map(move |p| {
                    std::iter::once(&p.manifest.id)
                        .chain(&p.manifest.former_ids)
                        .map(move |id| (s.clone(), id.clone()))
                })
            })
            .collect();
        let plans = self.scheduler().due(now, desktop, |s| {
            loaded.contains(&(s.space.clone(), s.plugin.clone()))
        });
        let device = self.cfg.device.0.clone();
        let mut ended = Vec::new();
        for plan in plans {
            let started = match self.scheduler_mut().begin(&plan, &device, now) {
                Ok(Some(started)) => started,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("run {}: {e}", plan.run);
                    continue;
                }
            };
            let result = self.call_for_run(
                &started.space,
                &started.plugin,
                &started.operation,
                &started.args,
                Some(started.run.clone()),
            );
            let outcome = match result {
                Ok(value) if value.get("$run").is_some_and(Value::is_object) => {
                    // The operation kept its changes but ends the run its own way.
                    let message = value["$run"]["message"].as_str().unwrap_or("").to_string();
                    match value["$run"]["status"].as_str() {
                        Some("failed") => Outcome::Failed(message),
                        Some("skipped") => Outcome::Skip(message),
                        Some("retry") => Outcome::Retry(message),
                        _ if self.has_async(&started.run) => Outcome::Waiting,
                        _ => Outcome::Done(notice(&value)),
                    }
                }
                Ok(_) if self.has_async(&started.run) => Outcome::Waiting,
                Ok(_) => Outcome::Done(None),
                Err(RuntimeError::Plugin { code, message }) if code == "retry" => {
                    Outcome::Retry(message)
                }
                Err(RuntimeError::Plugin { code, message }) if code == "skip" => {
                    Outcome::Skip(message)
                }
                Err(e) => Outcome::Failed(e.to_string()),
            };
            let outcome = if self.has_async(&started.run) {
                if !matches!(outcome, Outcome::Waiting) {
                    self.async_outcomes.insert(started.run.clone(), outcome);
                }
                Outcome::Waiting
            } else {
                outcome
            };
            match self.scheduler_mut().finish(&started.run, outcome, now_ms()) {
                Ok(Some(done)) => ended.push(done),
                Ok(None) => {}
                Err(e) => eprintln!("run {}: {e}", started.run),
            }
        }
        ended
    }

    /// Rows of every plugin's reminders query in every open space, as
    /// `(space, plugin storage id, row)`.
    pub fn reminders(&self, now_ms: u64) -> Vec<(String, String, Map<String, Value>)> {
        let mut out = Vec::new();
        for space in &self.open_spaces {
            for (id, loaded) in &self.plugins {
                let Some(def) = &loaded.manifest.reminders else {
                    continue;
                };
                let mut values = Map::new();
                values.insert("now_ms".into(), Value::from(now_ms));
                // Reminder keys stay stable across a rename.
                let key = loaded.manifest.storage_id().to_string();
                match self.query_in(space, id, &def.query, &values) {
                    Ok(rows) => {
                        out.extend(rows.into_iter().map(|r| (space.clone(), key.clone(), r)))
                    }
                    Err(e) => eprintln!("reminders query of {id} in {space} failed: {e}"),
                }
            }
        }
        out
    }

    /// This host's device id, for identifying locally authored versions.
    pub fn device(&self) -> &str {
        &self.cfg.device.0
    }

    /// The plugin's storage in the default space, for inspection.
    pub fn store(&self, plugin: &str) -> Result<&CoreStore> {
        self.get(plugin)?.store(DEFAULT_SPACE)
    }

    pub fn store_in(&self, space: &str, plugin: &str) -> Result<&CoreStore> {
        self.get(plugin)?.store(space)
    }

    fn get(&self, plugin: &str) -> Result<&LoadedPlugin> {
        self.loaded(plugin)
            .ok_or_else(|| RuntimeError::UnknownPlugin(plugin.into()))
    }

    /// The loaded plugin named `id` or, after a rename, one of its former ids.
    fn loaded(&self, id: &str) -> Option<&LoadedPlugin> {
        resolve(&self.plugins, id).map(|key| &self.plugins[key])
    }

    /// The current id of the plugin `id` names: `id` itself unless it is a
    /// former id of a loaded plugin.
    pub fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        resolve(&self.plugins, id).unwrap_or(id)
    }

    /// The manifest of the plugin `id` names (current or former id).
    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.loaded(id).map(|p| &p.manifest)
    }

    /// The id the plugin `id` names stores and syncs under: its first former
    /// id after a rename. Unknown ids are returned unchanged.
    pub fn storage_plugin_id<'a>(&'a self, id: &'a str) -> &'a str {
        self.manifest(id).map_or(id, Manifest::storage_id)
    }

    /// Whether `a` and `b` name the same plugin.
    pub fn same_plugin(&self, a: &str, b: &str) -> bool {
        a == b || self.canonical(a) == self.canonical(b)
    }

    /// Every spelling of the item ref `text`: as given first, then with the
    /// plugin's current, storage and former ids. Text that is not a ref, or
    /// names no loaded plugin, is returned alone.
    pub fn ref_forms(&self, text: &str) -> Vec<String> {
        let mut out = vec![text.to_string()];
        let Some(item) = crate::ItemRef::parse(text) else {
            return out;
        };
        let Some(m) = self.manifest(&item.plugin) else {
            return out;
        };
        for id in std::iter::once(&m.id).chain(&m.former_ids) {
            let form = format!("{id}/{}/{}", item.table, item.id);
            if !out.contains(&form) {
                out.push(form);
            }
        }
        out
    }

    fn data_root(&self) -> PathBuf {
        self.cfg
            .data_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.cfg.data_dir.clone())
    }

    fn blob_settings_path(&self) -> PathBuf {
        self.data_root().join("blob-settings.json")
    }

    fn blob_settings(&self) -> BlobSettings {
        let path = self.blob_settings_path();
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_blob_settings(&self, settings: &BlobSettings) {
        let path = self.blob_settings_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(settings) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

/// The other loaded plugins, carried through a call so that interop can
/// run them, then handed back. Moving them (instead of sharing) means a
/// plugin's storage is only ever used by one call at a time, and a plugin
/// can never re-enter itself.
#[derive(Default)]
pub(crate) struct Peers {
    /// The space the call runs in; interop stays inside it.
    pub space: String,
    runtime: Option<Runtime>,
    pub(crate) timezone: String,
    pub(crate) device: String,
    pub event_depth: u8,
    pub event_root: Option<String>,
    pub speech: Option<crate::speech::SpeechConfig>,
    plugins: BTreeMap<String, LoadedPlugin>,
    pub ai: Option<Ai>,
    pub net: Option<terrarium_net::Net>,
    pub scheduler: Option<Scheduler>,
    /// The scheduled run this call serves, if any.
    pub run: Option<String>,
    /// Roll back every store the call touched, even on success, and skip
    /// its schedule changes and AI requests.
    pub dry_run: bool,
    /// When set, each call appends the changes it made (or would make).
    pub trace: Option<Vec<ChangeSummary>>,
    pub(crate) events: Vec<(String, crate::host::EmittedEvent)>,
    /// Accepted emissions across this top-level call, including rolled back children.
    pub(crate) event_count: usize,
}

/// One change an operation made, for dry runs and the audit log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChangeSummary {
    pub plugin: String,
    pub table: String,
    pub row: String,
    /// `create`, `set`, `increment`, `text_edit`, `delete` or `restore`.
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
}

impl ChangeSummary {
    pub(crate) fn of(plugin: &str, change: &terrarium_core::Change) -> Self {
        use terrarium_core::Op;
        let (op, column) = match &change.op {
            Op::Create => ("create", None),
            Op::Set { column, .. } => ("set", Some(column.clone())),
            Op::Increment { column, .. } => ("increment", Some(column.clone())),
            Op::TextEdit { column, .. } => ("text_edit", Some(column.clone())),
            Op::Delete { .. } => ("delete", None),
            Op::Restore => ("restore", None),
        };
        ChangeSummary {
            plugin: plugin.to_string(),
            table: change.table.clone(),
            row: change.row.clone(),
            op: op.to_string(),
            column,
        }
    }
}

impl Peers {
    /// Applies a committed call's schedule changes.
    pub fn apply_schedule_changes(&mut self, plugin: &str, changes: Vec<ScheduleChange>) {
        let space = self.space.clone();
        let Some(scheduler) = self.scheduler.as_mut() else {
            return;
        };
        for change in changes {
            let id = |key: &str| terrarium_scheduler::schedule_id(&space, plugin, key);
            let result = match change {
                ScheduleChange::Set(key, input) => scheduler
                    .set(&space, plugin, &key, &input, "plugin")
                    .map(|_| ()),
                ScheduleChange::Remove(key) => scheduler.remove(&id(&key)),
                ScheduleChange::RunNow(key, now) => scheduler.run_now(&id(&key), now).map(|_| ()),
                ScheduleChange::Retry(run) => scheduler.retry(&run, now_ms()),
            };
            if let Err(e) = result {
                eprintln!("schedule change by {plugin}: {e}");
            }
        }
    }

    /// The plugin that provides `iface`, if any.
    pub fn provider(&self, iface: &str) -> Option<String> {
        self.plugins
            .iter()
            .find(|(_, p)| p.manifest.provides.contains_key(iface))
            .map(|(id, _)| id.clone())
    }

    pub fn call_interface(
        self,
        iface: &str,
        operation: &str,
        args: &Value,
        depth: u8,
    ) -> (Peers, Result<Value>) {
        let Some(id) = self.provider(iface) else {
            return (
                self,
                Err(RuntimeError::UnknownPlugin(format!(
                    "no installed plugin provides `{iface}`"
                ))),
            );
        };
        let offered = self.plugins[&id].manifest.provides[iface]
            .operations
            .iter()
            .any(|op| op == operation);
        if !offered {
            return (
                self,
                Err(RuntimeError::Undeclared {
                    plugin: id,
                    kind: "interface operation",
                    name: operation.into(),
                }),
            );
        }
        invoke(self, &id, operation, args, depth)
    }
}

/// The key in `plugins` of the plugin `id` names: its id, or a former id.
fn resolve<'a>(plugins: &'a BTreeMap<String, LoadedPlugin>, id: &str) -> Option<&'a str> {
    if let Some((key, _)) = plugins.get_key_value(id) {
        return Some(key);
    }
    plugins
        .iter()
        .find(|(_, p)| p.manifest.former_ids.iter().any(|f| f == id))
        .map(|(key, _)| key.as_str())
}

/// Runs `operation` of `id`, which is moved out of `peers` for the call.
fn invoke(
    mut peers: Peers,
    id: &str,
    operation: &str,
    args: &Value,
    depth: u8,
) -> (Peers, Result<Value>) {
    let Some(mut loaded) = peers.plugins.remove(id) else {
        return (peers, Err(RuntimeError::UnknownPlugin(id.into())));
    };
    let fail = |peers: Peers, loaded: LoadedPlugin, e: RuntimeError| {
        let mut peers = peers;
        peers.plugins.insert(id.to_string(), loaded);
        (peers, Err(e))
    };
    if !loaded.manifest.operations.contains_key(operation) {
        let e = RuntimeError::Undeclared {
            plugin: id.into(),
            kind: "operation",
            name: operation.into(),
        };
        return fail(peers, loaded, e);
    }
    let slot = loaded.stores.get_mut(&peers.space).and_then(Option::take);
    let Some(store) = slot else {
        let e = match loaded.stores.contains_key(&peers.space) {
            true => RuntimeError::Wasm("plugin is already running".into()),
            false => RuntimeError::UnknownSpace(peers.space.clone()),
        };
        return fail(peers, loaded, e);
    };
    let Some(runtime) = peers.runtime.clone() else {
        loaded.stores.insert(peers.space.clone(), Some(store));
        return fail(peers, loaded, RuntimeError::Wasm("no runtime".into()));
    };
    let timezone = peers.timezone.clone();
    let storage_id = loaded.manifest.storage_id().to_string();
    let uses = loaded.manifest.uses.clone();
    let ctx = CallContext {
        plugin_id: id,
        storage_id: &storage_id,
        timezone: &timezone,
        limits: loaded.manifest.limits,
        uses: &uses,
        speech: loaded.manifest.permissions.iter().any(|p| p == "speech"),
        ai: loaded.manifest.permissions.iter().any(|p| p == "ai"),
        http: loaded.manifest.permissions.iter().any(|p| p == "http"),
        hosts: loaded.manifest.hosts.clone(),
        operations: loaded.manifest.operations.keys().cloned().collect(),
        events: loaded.manifest.events.clone(),
        depth,
    };
    let (store, mut peers, result) = match &loaded.code {
        Code::Wasm(pre) => runtime.call(pre, store, ctx, peers, operation, args),
        Code::Declarative => {
            let checked = crate::host::bounded_json(args, crate::host::MAX_ARGS);
            match checked {
                Err(e) => (store, peers, Err(e)),
                Ok(_) => crate::host::transaction(store, ctx, peers, |mut state| {
                    let empty = Map::new();
                    let result = if args.is_null() || args.is_object() {
                        crate::declarative::run(
                            &mut state.store,
                            &loaded.manifest,
                            operation,
                            args.as_object().unwrap_or(&empty),
                            now_ms(),
                            &timezone,
                        )
                    } else {
                        Err(RuntimeError::Plugin {
                            code: "invalid".into(),
                            message: "arguments must be an object".into(),
                        })
                    };
                    (state, result)
                }),
            }
        }
    };
    loaded.stores.insert(peers.space.clone(), Some(store));
    peers.plugins.insert(id.to_string(), loaded);
    (peers, result)
}

impl LoadedPlugin {
    fn store(&self, space: &str) -> Result<&CoreStore> {
        match self.stores.get(space) {
            Some(Some(store)) => Ok(store),
            Some(None) => Err(RuntimeError::Wasm("plugin is busy".into())),
            None => Err(RuntimeError::UnknownSpace(space.into())),
        }
    }
}

/// `{"$run": {"notify": "…"}}`: what to tell the user about a run that
/// ended well.
fn notice(value: &Value) -> Option<String> {
    value["$run"]["notify"].as_str().map(str::to_string)
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A manifest job as a schedule: named after the job, e.g. `weekly-review`
/// → "Weekly review".
fn job_input(name: &str, job: &crate::manifest::JobDef) -> ScheduleInput {
    let repeat = match &job.schedule {
        Schedule::EveryMinutes(m) => Repeat::Minutes { minutes: *m },
        Schedule::DailyAt(at) => Repeat::Day { at: at.clone() },
        Schedule::Weekly { day, at } => Repeat::Week {
            days: vec![day.clone()],
            at: at.clone(),
        },
    };
    let words = name.replace(['-', '_'], " ");
    let mut chars = words.chars();
    let title = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default();
    ScheduleInput {
        name: title,
        repeat,
        tz: None,
        run_on: match job.run_on {
            RunOn::Any => terrarium_scheduler::RunOn::Any,
            RunOn::Desktop => terrarium_scheduler::RunOn::Desktop,
        },
        operation: job.operation.clone(),
        args: job.args.clone(),
        enabled: true,
    }
}

fn store_config(cfg: &HostConfig, plugin: &str) -> StoreConfig {
    StoreConfig {
        space: cfg.space.clone(),
        plugin: plugin.to_string(),
        device: cfg.device.clone(),
        user: cfg.user.clone(),
    }
}

fn blob_dir_for_space(cfg: &HostConfig, space: &str) -> PathBuf {
    let plugins_dir = if space == DEFAULT_SPACE {
        cfg.data_dir.clone()
    } else {
        cfg.spaces_dir
            .clone()
            .unwrap_or_else(|| cfg.data_dir.join("spaces"))
            .join(space)
            .join("plugins")
    };
    plugins_dir
        .parent()
        .map(|p| p.join("blobs"))
        .unwrap_or_else(|| plugins_dir.join("blobs"))
}

impl terrarium_core::StoreSet for PluginHost {
    fn plugin_ids(&self) -> Vec<String> {
        let mut ids = vec![
            STORE_ID.to_string(),
            SPACES_STORE_ID.to_string(),
            PAGES_STORE_ID.to_string(),
            LINKS_STORE_ID.to_string(),
            EVENTS_STORE_ID.to_string(),
            RULES_STORE_ID.to_string(),
            terrarium_ai::STORE_ID.to_string(),
            terrarium_scheduler::STORE_ID.to_string(),
        ];
        ids.extend(
            self.vaults
                .keys()
                .map(|space| store_id(space, terrarium_net::vault::VAULT_STORE_ID)),
        );
        for loaded in self.plugins.values() {
            let id = loaded.manifest.storage_id();
            ids.extend(loaded.stores.keys().map(|space| store_id(space, id)));
        }
        ids
    }

    fn store_mut(&mut self, id: &str) -> Option<&mut CoreStore> {
        match id {
            STORE_ID => return Some(self.packages.store_mut()),
            SPACES_STORE_ID => return Some(self.spaces.store_mut()),
            PAGES_STORE_ID => return Some(self.pages.store_mut()),
            LINKS_STORE_ID => return Some(self.links.store_mut()),
            EVENTS_STORE_ID => return Some(self.events.store_mut()),
            RULES_STORE_ID => return Some(self.rules.store_mut()),
            terrarium_ai::STORE_ID => return self.ai.as_mut().map(Ai::store_mut),
            terrarium_scheduler::STORE_ID => {
                return self.scheduler.as_mut().map(Scheduler::store_mut)
            }
            _ => {}
        }
        let (space, plugin) = split_store_id(id);
        if plugin == terrarium_net::vault::VAULT_STORE_ID {
            return self
                .vaults
                .get_mut(space)
                .map(terrarium_net::vault::Vault::store_mut);
        }
        // Sync names a plugin's store by its storage id, which after a
        // rename is a former id.
        self.plugins
            .values_mut()
            .find(|p| p.manifest.storage_id() == plugin)?
            .stores
            .get_mut(space)?
            .as_mut()
    }
}

/// Latest modification time (ns) of a package's loadable files: manifest,
/// templates and `dist/`. Source and build output are ignored, so this
/// changes when a rebuilt package is ready to reload.
pub fn fingerprint(dir: &Path) -> u128 {
    fn walk(path: &Path, top: bool, latest: &mut u128) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if top && matches!(name.to_str(), Some("target" | "src" | ".git")) {
                continue;
            }
            if path.is_dir() {
                walk(&path, false, latest);
            } else if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                let ns = modified
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                *latest = (*latest).max(ns);
            }
        }
    }
    let mut latest = 0;
    walk(dir, true, &mut latest);
    latest
}

fn vault_unavailable() -> String {
    "Sign in again on this device".into()
}
fn url_host(url: &str) -> std::result::Result<String, String> {
    // Request parsing already canonicalized/validated the URL at admission.
    terrarium_net::request_host(url).ok_or_else(vault_unavailable)
}

/// Sealed-account generation and locally approved provider captured before browser I/O.
pub struct OAuthPreparation {
    pub provider: terrarium_net::oauth::OAuthDef,
    pub signer: String,
    pub generation: Option<String>,
    pub client: terrarium_net::oauth::Client,
}

#[cfg(test)]
mod view_query_tests {
    use super::*;

    fn query() -> QueryDef {
        serde_json::from_value(json!({
            "sql": "THIS SQL MUST NOT RUN",
            "params": ["start_ms", "end_ms", "page_size", "offset", "sort", "direction", "required"],
            "types": {
                "start_ms": {"type":"integer"}, "end_ms": {"type":"integer"},
                "page_size": {"type":"integer"}, "offset": {"type":"integer"},
                "sort": {"type":"string"}, "direction": {"type":"string"},
                "required": {"type":"string", "required":true}
            },
            "view": {
                "version":1, "title":"Rows",
                "columns":[{"key":"title", "label":"Title", "type":"string"}],
                "temporal":{"start_param":"start_ms", "end_param":"end_ms"},
                "pagination":{"page_size_param":"page_size", "offset_param":"offset", "default_page_size":100, "max_page_size":1000},
                "sorts":[{"param":"sort", "values":["title", "completed_at"], "direction_param":"direction", "directions":["asc", "desc"]}]
            }
        })).unwrap()
    }

    fn inputs(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn v1_query_uses_bounded_paging_defaults_and_accepts_declared_inputs() {
        let query = query();
        let missing = validate_view_query_values(&query, &Map::new()).unwrap_err();
        assert!(missing
            .to_string()
            .contains("required query parameter `required` is missing"));
        let defaults =
            validate_view_query_values(&query, &inputs(json!({"required":"ok"}))).unwrap();
        assert_eq!(defaults["page_size"], json!(100));
        assert_eq!(defaults["offset"], json!(0));

        let values = inputs(json!({"required":"ok", "start_ms":10, "end_ms":20,
            "page_size":1001, "offset":50, "sort":"completed_at", "direction":"desc"}));
        let accepted = validate_view_query_values(&query, &values).unwrap();
        assert_eq!(accepted["page_size"], json!(1001));
        assert_eq!(accepted["offset"], json!(50));
        assert_eq!(accepted["start_ms"], json!(10));
        assert_eq!(accepted["direction"], json!("desc"));
    }

    #[test]
    fn malformed_v1_inputs_are_rejected_before_the_query_can_run() {
        let query = query();
        let cases = [
            (
                json!({"required":"ok", "page_size":1002}),
                "page_size exceeds",
            ),
            (json!({"required":"ok", "offset":1000001}), "offset exceeds"),
            (
                json!({"required":"ok", "start_ms":5}),
                "must be provided together",
            ),
            (
                json!({"required":"ok", "start_ms":20, "end_ms":10}),
                "must not exceed",
            ),
            (
                json!({"required":"ok", "sort":"title; DROP TABLE tasks"}),
                "unsupported sort value",
            ),
            (
                json!({"required":"ok", "direction":"sideways"}),
                "unsupported sort direction",
            ),
            (
                json!({"required":"ok", "page_size":1.5}),
                "page_size must be a positive integer",
            ),
            (
                json!({"required":"ok", "typo":true}),
                "not a declared parameter",
            ),
        ];
        for (input, expected) in cases {
            let error = validate_view_query_values(&query, &inputs(input)).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
