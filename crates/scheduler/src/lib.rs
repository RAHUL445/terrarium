//! Scheduled plugin operations as a host service.
//!
//! Schedules, runs and settings live in a host-owned store, `_scheduler`,
//! which syncs like plugin data, so a schedule made on one device runs on
//! whichever device is open when it falls due. The library owns no
//! threads: the host asks [`Scheduler::due`] on its own timer, claims a
//! run with [`Scheduler::begin`], calls the plugin, and reports back with
//! [`Scheduler::finish`].
//!
//! - A run is one slot of one schedule, id `<schedule>@<slot ms>`, so two
//!   devices that both run a slot write the same row. Plugins must make a
//!   run with the same id harmless to repeat.
//! - The device running it holds a lease; others leave it alone until the
//!   lease runs out (the device stopped mid-run), then retry it.
//! - Missed slots (every device was off) run once: only the latest slot.
//! - A temporary failure is retried after 15 min, then 1 h, then the run
//!   is failed. Failed and skipped runs can be retried by hand.

mod spec;

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

pub use spec::{local_parts, Last, MonthDay, Repeat, RunOn};

pub const STORE_ID: &str = "_scheduler";
/// How long a device may hold a running run before others take it over.
const LEASE_MS: u64 = 10 * 60_000;
/// Same, while the run waits for an AI reply.
const WAIT_LEASE_MS: u64 = 30 * 60_000;
const MAX_ATTEMPTS: u32 = 3;
const BACKOFF_MS: [u64; 2] = [15 * 60_000, 60 * 60_000];
const KEEP_RUNS: usize = 50;

const SCHEMA: &str = r#"{
  "version": 2,
  "tables": [
    { "name": "schedules", "columns": [
      { "name": "space", "type": "text" },
      { "name": "plugin", "type": "text" },
      { "name": "key", "type": "text" },
      { "name": "name", "type": "text" },
      { "name": "spec", "type": "text" },
      { "name": "operation", "type": "text" },
      { "name": "args", "type": "text" },
      { "name": "enabled", "type": "boolean", "default": true },
      { "name": "source", "type": "text" },
      { "name": "created_ms", "type": "integer" },
      { "name": "updated_ms", "type": "integer" },
      { "name": "last_slot_ms", "type": "integer" }
    ]},
    { "name": "runs", "columns": [
      { "name": "schedule", "type": "text" },
      { "name": "space", "type": "text" },
      { "name": "plugin", "type": "text" },
      { "name": "slot_ms", "type": "integer" },
      { "name": "status", "type": "text" },
      { "name": "device", "type": "text" },
      { "name": "attempt", "type": "integer" },
      { "name": "error", "type": "text" },
      { "name": "lease_until", "type": "integer" },
      { "name": "next_attempt_ms", "type": "integer" },
      { "name": "started_ms", "type": "integer" },
      { "name": "finished_ms", "type": "integer" },
      { "name": "manual", "type": "boolean", "default": false }
    ]},
    { "name": "settings", "columns": [
      { "name": "paused", "type": "boolean", "default": false },
      { "name": "notify_failures", "type": "boolean", "default": true },
      { "name": "notify_done", "type": "boolean", "default": true }
    ]}
  ]
}"#;

/// What a plugin or the user sets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInput {
    pub name: String,
    pub repeat: Repeat,
    /// IANA zone; the user's when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(default)]
    pub run_on: RunOn,
    pub operation: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Spec {
    repeat: Repeat,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    run_on: RunOn,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScheduleInfo {
    pub id: String,
    pub space: String,
    pub plugin: String,
    pub key: String,
    pub name: String,
    pub repeat: Repeat,
    pub tz: Option<String>,
    pub run_on: RunOn,
    pub operation: String,
    pub args: Value,
    pub enabled: bool,
    /// `plugin` (made through the contract) or `manifest` (declared job).
    pub source: String,
    pub created_ms: u64,
    pub last_slot_ms: Option<u64>,
    /// When it runs next (`None` when off or paused).
    pub next_ms: Option<u64>,
    pub describe: String,
    pub last_run: Option<RunInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunInfo {
    pub id: String,
    pub schedule: String,
    pub space: String,
    pub plugin: String,
    pub slot_ms: u64,
    /// `running`, `waiting` (for an AI reply), `retry`, `done`, `failed`, `skipped`.
    pub status: String,
    pub device: Option<String>,
    pub attempt: u32,
    pub error: Option<String>,
    pub lease_until: Option<u64>,
    pub next_attempt_ms: Option<u64>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub manual: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub paused: bool,
    pub notify_failures: bool,
    /// Show the notice a run leaves when it ends well ("Report ready").
    #[serde(default = "yes")]
    pub notify_done: bool,
}

fn yes() -> bool {
    true
}

/// A run this device may start.
#[derive(Clone, Debug)]
pub struct Plan {
    pub run: String,
    pub schedule: String,
    pub slot_ms: u64,
    pub attempt: u32,
    pub manual: bool,
}

/// A claimed run: call `operation` of `plugin` in `space` with `args`.
#[derive(Clone, Debug)]
pub struct Started {
    pub run: String,
    pub space: String,
    pub plugin: String,
    pub operation: String,
    pub args: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Ended well, with an optional notice for the user.
    Done(Option<String>),
    /// The operation queued an AI request; the run ends when it does.
    Waiting,
    Retry(String),
    Skip(String),
    Failed(String),
}

/// A run that ended, for notifications and logs.
#[derive(Clone, Debug, Serialize)]
pub struct Finished {
    pub run: RunInfo,
    pub schedule_name: String,
    /// Tell the user: the run failed and failure alerts are on.
    pub notify: bool,
    /// What to tell the user about a run that ended well, if anything.
    pub notice: Option<String>,
}

pub struct Scheduler {
    store: CoreStore,
    timezone: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn obj(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn int(row: &Map<String, Value>, key: &str) -> Option<u64> {
    row.get(key)
        .and_then(Value::as_i64)
        .map(|n| n.max(0) as u64)
}

fn flag(row: &Map<String, Value>, key: &str) -> bool {
    row.get(key)
        .is_some_and(|v| v.as_bool().unwrap_or(v.as_i64().unwrap_or(0) != 0))
}

/// Like `flag`, but a missing value means on.
fn flag_on(row: &Map<String, Value>, key: &str) -> bool {
    row.get(key)
        .is_none_or(|v| v.is_null() || v.as_bool().unwrap_or(v.as_i64().unwrap_or(1) != 0))
}

fn store_err(e: terrarium_core::Error) -> String {
    e.to_string()
}

pub fn schedule_id(space: &str, plugin: &str, key: &str) -> String {
    format!("{space}/{plugin}/{key}")
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 60
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn run_info(row: &Map<String, Value>) -> Option<RunInfo> {
    Some(RunInfo {
        id: text(row, "id")?,
        schedule: text(row, "schedule")?,
        space: text(row, "space").unwrap_or_default(),
        plugin: text(row, "plugin").unwrap_or_default(),
        slot_ms: int(row, "slot_ms").unwrap_or(0),
        status: text(row, "status").unwrap_or_else(|| "retry".into()),
        device: text(row, "device"),
        attempt: int(row, "attempt").unwrap_or(0) as u32,
        error: text(row, "error"),
        lease_until: int(row, "lease_until"),
        next_attempt_ms: int(row, "next_attempt_ms"),
        started_ms: int(row, "started_ms"),
        finished_ms: int(row, "finished_ms"),
        manual: flag(row, "manual"),
    })
}

const RUN_COLUMNS: &str = "id, schedule, space, plugin, slot_ms, status, device, attempt, error, lease_until, next_attempt_ms, started_ms, finished_ms, manual";

impl Scheduler {
    /// `timezone` is the user's, used by schedules that do not name one.
    pub fn open(
        data_dir: &Path,
        cfg: StoreConfig,
        timezone: &str,
    ) -> terrarium_core::Result<Scheduler> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA).expect("valid scheduler schema");
        let store = CoreStore::open(
            data_dir.join(format!("{STORE_ID}.db")),
            StoreConfig {
                plugin: STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Scheduler {
            store,
            timezone: timezone.to_string(),
        })
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    pub fn settings(&self) -> Settings {
        self.store
            .query(
                "SELECT paused, notify_failures, notify_done FROM settings WHERE id = 'config' AND _deleted IS NULL",
                &[],
            )
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .map(|r| Settings {
                paused: flag(&r, "paused"),
                notify_failures: flag_on(&r, "notify_failures"),
                notify_done: flag_on(&r, "notify_done"),
            })
            .unwrap_or(Settings {
                paused: false,
                notify_failures: true,
                notify_done: true,
            })
    }

    pub fn set_settings(&mut self, settings: &Settings) -> Result<(), String> {
        let values = obj(json!({
            "paused": settings.paused,
            "notify_failures": settings.notify_failures,
            "notify_done": settings.notify_done,
        }));
        self.store
            .write(|w| w.upsert("settings", "config", &values))
            .map_err(store_err)
    }

    fn tz<'a>(&'a self, tz: &'a Option<String>) -> &'a str {
        tz.as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.timezone)
    }

    /// Checks `input` without saving it.
    pub fn check(&self, input: &ScheduleInput) -> Result<(), String> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("name must be 1–80 characters".into());
        }
        input.repeat.validate()?;
        if let Some(tz) = &input.tz {
            if !tz.is_empty() {
                local_parts(0, tz)?;
            }
        }
        if input.operation.is_empty() {
            return Err("operation is required".into());
        }
        if !(input.args.is_null() || input.args.is_object()) {
            return Err("args must be a JSON object".into());
        }
        if input.args.to_string().len() > 16 * 1024 {
            return Err("args must be under 16 KB".into());
        }
        Ok(())
    }

    /// Creates or replaces `space/plugin/key`. The slot it last ran is
    /// kept, so editing a schedule does not run it again at once.
    pub fn set(
        &mut self,
        space: &str,
        plugin: &str,
        key: &str,
        input: &ScheduleInput,
        source: &str,
    ) -> Result<String, String> {
        if !valid_key(key) {
            return Err(format!(
                "schedule id `{key}` must be 1–60 letters, digits, `-`, `_` or `.`"
            ));
        }
        self.check(input)?;
        let id = schedule_id(space, plugin, key);
        let now = now_ms();
        let existing = self.row(&id);
        let spec = Spec {
            repeat: input.repeat.clone(),
            tz: input.tz.clone().filter(|t| !t.is_empty()),
            run_on: input.run_on,
        };
        let mut values = obj(json!({
            "space": space,
            "plugin": plugin,
            "key": key,
            "name": input.name.trim(),
            "spec": serde_json::to_string(&spec).map_err(|e| e.to_string())?,
            "operation": input.operation,
            "args": if input.args.is_null() { "{}".to_string() } else { input.args.to_string() },
            "enabled": input.enabled,
            "source": source,
            "updated_ms": now,
        }));
        if existing.is_none() {
            values.insert("created_ms".into(), json!(now));
        }
        self.store
            .write(|w| w.upsert("schedules", &id, &values))
            .map_err(store_err)?;
        Ok(id)
    }

    /// Declared manifest jobs: created once; the user may then change when
    /// they run or turn them off, which later loads keep. The operation and
    /// arguments follow the manifest.
    pub fn ensure(
        &mut self,
        space: &str,
        plugin: &str,
        key: &str,
        input: &ScheduleInput,
    ) -> Result<(), String> {
        let id = schedule_id(space, plugin, key);
        match self.row(&id) {
            None => self.set(space, plugin, key, input, "manifest").map(|_| ()),
            Some(row) => {
                let args = if input.args.is_null() {
                    "{}".to_string()
                } else {
                    input.args.to_string()
                };
                if text(&row, "operation").as_deref() == Some(&input.operation)
                    && text(&row, "args").as_deref() == Some(&args)
                {
                    return Ok(());
                }
                let values = obj(json!({ "operation": input.operation, "args": args }));
                self.store
                    .write(|w| w.update("schedules", &id, &values))
                    .map_err(store_err)
            }
        }
    }

    /// Removes manifest schedules of `plugin` in `space` that are no longer declared.
    pub fn retain_manifest(
        &mut self,
        space: &str,
        plugin: &str,
        keys: &[String],
    ) -> Result<(), String> {
        for info in self.list(Some(space), Some(plugin)) {
            if info.source == "manifest" && !keys.contains(&info.key) {
                self.remove(&info.id)?;
            }
        }
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        if self.row(id).is_none() {
            return Err(format!("no schedule `{id}`"));
        }
        let values = obj(json!({ "enabled": enabled, "updated_ms": now_ms() }));
        self.store
            .write(|w| w.update("schedules", id, &values))
            .map_err(store_err)
    }

    /// Changes when a schedule runs (any source), keeping the rest.
    pub fn set_repeat(&mut self, id: &str, repeat: &Repeat) -> Result<(), String> {
        repeat.validate()?;
        let row = self.row(id).ok_or_else(|| format!("no schedule `{id}`"))?;
        let mut spec: Spec = text(&row, "spec")
            .and_then(|s| serde_json::from_str(&s).ok())
            .ok_or("stored schedule is unreadable")?;
        spec.repeat = repeat.clone();
        let values = obj(json!({
            "spec": serde_json::to_string(&spec).map_err(|e| e.to_string())?,
            "updated_ms": now_ms(),
        }));
        self.store
            .write(|w| w.update("schedules", id, &values))
            .map_err(store_err)
    }

    /// Removes a schedule and its run history.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if self.row(id).is_none() {
            return Err(format!("no schedule `{id}`"));
        }
        let runs: Vec<String> = self
            .runs(id, usize::MAX)
            .into_iter()
            .map(|r| r.id)
            .collect();
        self.store
            .write(|w| {
                for run in &runs {
                    w.delete("runs", run)?;
                }
                w.delete("schedules", id)
            })
            .map_err(store_err)
    }

    fn row(&self, id: &str) -> Option<Map<String, Value>> {
        self.store
            .query(
                "SELECT * FROM schedules WHERE id = ?1 AND _deleted IS NULL",
                &[json!(id)],
            )
            .ok()?
            .into_iter()
            .next()
    }

    fn info(&self, row: &Map<String, Value>, now: u64, paused: bool) -> Option<ScheduleInfo> {
        let spec: Spec = serde_json::from_str(&text(row, "spec")?).ok()?;
        let id = text(row, "id")?;
        let created_ms = int(row, "created_ms").unwrap_or(0);
        let enabled = flag(row, "enabled") || row.get("enabled").is_none_or(Value::is_null);
        let tz = self.tz(&spec.tz).to_string();
        let next_ms = if enabled && !paused {
            spec.repeat.next(now, created_ms, &tz).ok().flatten()
        } else {
            None
        };
        Some(ScheduleInfo {
            last_run: self.runs(&id, 1).into_iter().next(),
            id,
            space: text(row, "space")?,
            plugin: text(row, "plugin")?,
            key: text(row, "key").unwrap_or_default(),
            name: text(row, "name").unwrap_or_default(),
            describe: spec.repeat.describe(),
            repeat: spec.repeat,
            tz: spec.tz,
            run_on: spec.run_on,
            operation: text(row, "operation").unwrap_or_default(),
            args: text(row, "args")
                .and_then(|a| serde_json::from_str(&a).ok())
                .unwrap_or(json!({})),
            enabled,
            source: text(row, "source").unwrap_or_else(|| "plugin".into()),
            created_ms,
            last_slot_ms: int(row, "last_slot_ms"),
            next_ms,
        })
    }

    pub fn get(&self, id: &str) -> Option<ScheduleInfo> {
        let paused = self.settings().paused;
        self.info(&self.row(id)?, now_ms(), paused)
    }

    /// Schedules, optionally of one space and plugin, by name.
    pub fn list(&self, space: Option<&str>, plugin: Option<&str>) -> Vec<ScheduleInfo> {
        let now = now_ms();
        let paused = self.settings().paused;
        let rows = self
            .store
            .query(
                "SELECT * FROM schedules WHERE _deleted IS NULL \
                 AND (?1 IS NULL OR space = ?1) AND (?2 IS NULL OR plugin = ?2) \
                 ORDER BY name, id",
                &[json!(space), json!(plugin)],
            )
            .unwrap_or_default();
        rows.iter()
            .filter_map(|r| self.info(r, now, paused))
            .collect()
    }

    /// Runs of a schedule, newest first.
    pub fn runs(&self, schedule: &str, limit: usize) -> Vec<RunInfo> {
        let limit = limit.min(10_000) as i64;
        self.store
            .query(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM runs WHERE schedule = ?1 AND _deleted IS NULL \
                     ORDER BY slot_ms DESC, started_ms DESC LIMIT ?2"
                ),
                &[json!(schedule), json!(limit)],
            )
            .unwrap_or_default()
            .iter()
            .filter_map(run_info)
            .collect()
    }

    /// Recent runs of every schedule, newest first.
    pub fn recent_runs(&self, limit: usize) -> Vec<RunInfo> {
        self.store
            .query(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM runs WHERE _deleted IS NULL \
                     ORDER BY COALESCE(started_ms, slot_ms) DESC LIMIT ?1"
                ),
                &[json!(limit.min(1000) as i64)],
            )
            .unwrap_or_default()
            .iter()
            .filter_map(run_info)
            .collect()
    }

    pub fn run(&self, id: &str) -> Option<RunInfo> {
        self.store
            .query(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1 AND _deleted IS NULL"),
                &[json!(id)],
            )
            .ok()?
            .first()
            .and_then(run_info)
    }

    /// Runs this device may start now. `desktop`: whether it is a computer.
    /// `runnable` says whether the schedule's plugin is loaded here.
    pub fn due(
        &self,
        now: u64,
        desktop: bool,
        runnable: impl Fn(&ScheduleInfo) -> bool,
    ) -> Vec<Plan> {
        if self.settings().paused {
            return Vec::new();
        }
        let schedules: BTreeMap<String, ScheduleInfo> = self
            .list(None, None)
            .into_iter()
            .filter(|s| s.enabled && (desktop || s.run_on == RunOn::Any) && runnable(s))
            .map(|s| (s.id.clone(), s))
            .collect();
        let mut plans = Vec::new();
        for s in schedules.values() {
            let tz = self.tz(&s.tz).to_string();
            let Ok(Some(slot)) = s.repeat.latest(now, s.created_ms, &tz) else {
                continue;
            };
            let after = s.created_ms.max(s.last_slot_ms.unwrap_or(0));
            let run = format!("{}@{slot}", s.id);
            if slot > after && self.run(&run).is_none() {
                plans.push(Plan {
                    run,
                    schedule: s.id.clone(),
                    slot_ms: slot,
                    attempt: 1,
                    manual: false,
                });
            }
        }
        let open = self
            .store
            .query(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM runs WHERE _deleted IS NULL \
                     AND status IN ('retry', 'running', 'waiting')"
                ),
                &[],
            )
            .unwrap_or_default();
        for run in open.iter().filter_map(run_info) {
            if !schedules.contains_key(&run.schedule) {
                continue;
            }
            let ready = match run.status.as_str() {
                "retry" => run.next_attempt_ms.unwrap_or(0) <= now,
                // A device stopped while holding it.
                _ => run.lease_until.unwrap_or(0) < now,
            };
            if ready {
                plans.push(Plan {
                    schedule: run.schedule.clone(),
                    slot_ms: run.slot_ms,
                    attempt: run.attempt + 1,
                    manual: run.manual,
                    run: run.id,
                });
            }
        }
        plans.sort_by_key(|p| p.slot_ms);
        plans
    }

    /// Claims `plan` for `device`. `None` when another device holds it or
    /// it already ended.
    pub fn begin(
        &mut self,
        plan: &Plan,
        device: &str,
        now: u64,
    ) -> Result<Option<Started>, String> {
        let Some(schedule) = self.get(&plan.schedule) else {
            return Ok(None);
        };
        if let Some(run) = self.run(&plan.run) {
            let held = matches!(run.status.as_str(), "running" | "waiting")
                && run.lease_until.unwrap_or(0) >= now
                && run.device.as_deref() != Some(device);
            let ended = matches!(run.status.as_str(), "done" | "failed" | "skipped");
            if held || ended {
                return Ok(None);
            }
            if run.status != "retry" && run.attempt >= MAX_ATTEMPTS {
                self.end(
                    &plan.run,
                    "failed",
                    Some("Stopped before it finished.".into()),
                    now,
                )?;
                return Ok(None);
            }
        }
        let values = obj(json!({
            "schedule": schedule.id,
            "space": schedule.space,
            "plugin": schedule.plugin,
            "slot_ms": plan.slot_ms,
            "status": "running",
            "device": device,
            "attempt": plan.attempt,
            "error": null,
            "lease_until": now + LEASE_MS,
            "next_attempt_ms": null,
            "started_ms": now,
            "finished_ms": null,
            "manual": plan.manual,
        }));
        let advance = !plan.manual && schedule.last_slot_ms.unwrap_or(0) < plan.slot_ms;
        self.store
            .write(|w| {
                w.upsert("runs", &plan.run, &values)?;
                if advance {
                    w.update(
                        "schedules",
                        &schedule.id,
                        &obj(json!({ "last_slot_ms": plan.slot_ms })),
                    )?;
                }
                Ok(())
            })
            .map_err(store_err)?;
        let tz = self.tz(&schedule.tz).to_string();
        let (date, time) = local_parts(plan.slot_ms, &tz)?;
        let previous_ms = schedule
            .repeat
            .latest(
                plan.slot_ms.saturating_sub(1),
                schedule.created_ms.min(plan.slot_ms),
                &tz,
            )
            .ok()
            .flatten();
        let mut args = match schedule.args {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        args.insert(
            "$run".into(),
            json!({
                "id": plan.run,
                "schedule": schedule.key,
                "slot_ms": plan.slot_ms,
                "date": date,
                "time": time,
                "tz": tz,
                "previous_ms": previous_ms,
                "attempt": plan.attempt,
                "manual": plan.manual,
            }),
        );
        Ok(Some(Started {
            run: plan.run.clone(),
            space: schedule.space,
            plugin: schedule.plugin,
            operation: schedule.operation,
            args: Value::Object(args),
        }))
    }

    fn end(
        &mut self,
        run: &str,
        status: &str,
        error: Option<String>,
        now: u64,
    ) -> Result<(), String> {
        let values = obj(json!({
            "status": status,
            "error": error,
            "finished_ms": now,
            "lease_until": null,
            "next_attempt_ms": null,
        }));
        self.store
            .write(|w| w.update("runs", run, &values))
            .map_err(store_err)
    }

    /// Records how a run went. Returns the ended run, if it ended.
    pub fn finish(
        &mut self,
        run_id: &str,
        outcome: Outcome,
        now: u64,
    ) -> Result<Option<Finished>, String> {
        let Some(run) = self.run(run_id) else {
            return Ok(None);
        };
        let mut notice = None;
        let (status, error, next) = match outcome {
            Outcome::Done(message) => {
                notice = message.filter(|m| !m.trim().is_empty());
                ("done", None, None)
            }
            Outcome::Waiting => {
                let values =
                    obj(json!({ "status": "waiting", "lease_until": now + WAIT_LEASE_MS }));
                self.store
                    .write(|w| w.update("runs", run_id, &values))
                    .map_err(store_err)?;
                return Ok(None);
            }
            Outcome::Retry(message) if run.attempt < MAX_ATTEMPTS => {
                let wait = BACKOFF_MS[(run.attempt.max(1) as usize - 1).min(BACKOFF_MS.len() - 1)];
                ("retry", Some(message), Some(now + wait))
            }
            Outcome::Retry(message) => (
                "failed",
                Some(format!("{message} (gave up after {} tries)", run.attempt)),
                None,
            ),
            Outcome::Skip(message) => ("skipped", Some(message), None),
            Outcome::Failed(message) => ("failed", Some(message), None),
        };
        if status == "retry" {
            let values = obj(json!({
                "status": "retry",
                "error": error,
                "next_attempt_ms": next,
                "lease_until": null,
            }));
            self.store
                .write(|w| w.update("runs", run_id, &values))
                .map_err(store_err)?;
            return Ok(None);
        }
        self.end(run_id, status, error, now)?;
        self.prune(&run.schedule)?;
        let run = self.run(run_id).ok_or("run vanished")?;
        let schedule_name = self
            .get(&run.schedule)
            .map(|s| s.name)
            .unwrap_or_else(|| run.schedule.clone());
        let settings = self.settings();
        let notify = run.status == "failed" && settings.notify_failures;
        Ok(Some(Finished {
            run,
            schedule_name,
            notify,
            notice: notice.filter(|_| settings.notify_done),
        }))
    }

    fn prune(&mut self, schedule: &str) -> Result<(), String> {
        let old: Vec<String> = self
            .runs(schedule, usize::MAX)
            .into_iter()
            .skip(KEEP_RUNS)
            .filter(|r| !matches!(r.status.as_str(), "running" | "waiting" | "retry"))
            .map(|r| r.id)
            .collect();
        if old.is_empty() {
            return Ok(());
        }
        self.store
            .write(|w| {
                for id in &old {
                    w.delete("runs", id)?;
                }
                Ok(())
            })
            .map_err(store_err)
    }

    /// Runs an ended (or waiting-to-retry) run again as soon as possible,
    /// with a fresh set of tries.
    pub fn retry(&mut self, run_id: &str, now: u64) -> Result<(), String> {
        let run = self
            .run(run_id)
            .ok_or_else(|| format!("no run `{run_id}`"))?;
        if matches!(run.status.as_str(), "running" | "waiting")
            && run.lease_until.unwrap_or(0) >= now
        {
            return Err("That run is still going.".into());
        }
        let values = obj(json!({
            "status": "retry",
            "attempt": 0,
            "next_attempt_ms": now,
            "lease_until": null,
            "finished_ms": null,
        }));
        self.store
            .write(|w| w.update("runs", run_id, &values))
            .map_err(store_err)
    }

    /// Queues a run of `schedule` now, outside its repeat. Returns its id.
    pub fn run_now(&mut self, schedule: &str, now: u64) -> Result<String, String> {
        let info = self
            .get(schedule)
            .ok_or_else(|| format!("no schedule `{schedule}`"))?;
        let id = format!("{}@m{now}", info.id);
        let values = obj(json!({
            "schedule": info.id,
            "space": info.space,
            "plugin": info.plugin,
            "slot_ms": now,
            "status": "retry",
            "attempt": 0,
            "next_attempt_ms": now,
            "manual": true,
        }));
        self.store
            .write(|w| w.upsert("runs", &id, &values))
            .map_err(store_err)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: &str = "Asia/Kolkata";
    const HOUR: u64 = 3_600_000;

    fn open(dir: &Path, device: &str) -> Scheduler {
        Scheduler::open(
            dir,
            StoreConfig {
                space: "t".into(),
                plugin: STORE_ID.into(),
                device: device.into(),
                user: "u".into(),
            },
            TZ,
        )
        .unwrap()
    }

    fn hourly(name: &str) -> ScheduleInput {
        ScheduleInput {
            name: name.into(),
            repeat: Repeat::Minutes { minutes: 60 },
            tz: None,
            run_on: RunOn::Any,
            operation: "generate".into(),
            args: json!({"skill": "self"}),
            enabled: true,
        }
    }

    fn created(s: &Scheduler, id: &str) -> u64 {
        s.get(id).unwrap().created_ms
    }

    #[test]
    fn runs_the_latest_slot_once_and_passes_run_info() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        let id = s
            .set("personal", "reports", "weekly", &hourly("Weekly"), "plugin")
            .unwrap();
        let start = created(&s, &id);
        assert!(s.due(start + HOUR / 2, true, |_| true).is_empty());

        // Off for five hours: one run, for the latest slot.
        let now = start + 5 * HOUR + 60_000;
        let plans = s.due(now, true, |_| true);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].slot_ms, start + 5 * HOUR);
        let started = s.begin(&plans[0], "a", now).unwrap().unwrap();
        assert_eq!(started.args["skill"], "self");
        assert_eq!(started.args["$run"]["schedule"], "weekly");
        assert_eq!(started.args["$run"]["previous_ms"], start + 4 * HOUR);
        assert!(s.due(now, true, |_| true).is_empty(), "claimed");
        let done = s
            .finish(&started.run, Outcome::Done(None), now)
            .unwrap()
            .unwrap();
        assert_eq!(done.run.status, "done");
        assert!(!done.notify);
        assert!(s.due(now + 1000, true, |_| true).is_empty());
        assert_eq!(s.get(&id).unwrap().last_slot_ms, Some(start + 5 * HOUR));
        let info = s.get(&id).unwrap();
        assert_eq!(
            info.repeat.next(now, start, TZ).unwrap(),
            Some(start + 6 * HOUR)
        );
    }

    #[test]
    fn retries_with_backoff_then_fails_and_notifies() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        let weekly = ScheduleInput {
            repeat: Repeat::Minutes { minutes: 10_080 },
            ..hourly("Weekly")
        };
        let id = s.set("p", "reports", "r", &weekly, "plugin").unwrap();
        let mut now = created(&s, &id) + 168 * HOUR + 1;
        for attempt in 1..=3 {
            let plans = s.due(now, true, |_| true);
            assert_eq!(plans.len(), 1, "attempt {attempt}");
            assert_eq!(plans[0].attempt, attempt);
            let started = s.begin(&plans[0], "a", now).unwrap().unwrap();
            let ended = s
                .finish(&started.run, Outcome::Retry("rate limited".into()), now)
                .unwrap();
            if attempt < 3 {
                assert!(ended.is_none());
                let run = s.run(&started.run).unwrap();
                assert_eq!(run.status, "retry");
                assert!(s
                    .due(now + 60_000, true, |_| true)
                    .iter()
                    .all(|p| p.run != started.run));
                now = run.next_attempt_ms.unwrap();
            } else {
                let ended = ended.unwrap();
                assert_eq!(ended.run.status, "failed");
                assert!(ended.notify);
                assert!(ended.run.error.unwrap().contains("gave up after 3"));
                // A manual retry gets a fresh set of tries.
                s.retry(&started.run, now).unwrap();
                let plans = s.due(now, true, |_| true);
                assert_eq!(plans[0].attempt, 1);
            }
        }
    }

    #[test]
    fn another_device_waits_for_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        let id = s.set("p", "reports", "r", &hourly("R"), "plugin").unwrap();
        let now = created(&s, &id) + HOUR + 1;
        let plan = s.due(now, true, |_| true).remove(0);
        s.begin(&plan, "a", now).unwrap().unwrap();
        assert!(s.begin(&plan, "b", now + 1000).unwrap().is_none());
        // Device a vanished: after the lease another device takes over.
        let later = now + LEASE_MS + 1;
        let plans = s.due(later, true, |_| true);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].attempt, 2);
        assert!(s.begin(&plans[0], "b", later).unwrap().is_some());
        s.finish(&plans[0].run, Outcome::Waiting, later).unwrap();
        assert_eq!(s.run(&plans[0].run).unwrap().status, "waiting");
        assert!(
            s.due(later + LEASE_MS, true, |_| true).is_empty(),
            "waiting lease is longer"
        );
    }

    #[test]
    fn skip_desktop_only_pause_and_run_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        let mut input = hourly("Desk");
        input.run_on = RunOn::Desktop;
        let id = s.set("p", "x", "desk", &input, "plugin").unwrap();
        let now = created(&s, &id) + HOUR + 1;
        assert!(s.due(now, false, |_| true).is_empty(), "phone");
        assert!(s.due(now, true, |_| false).is_empty(), "plugin not loaded");
        s.set_settings(&Settings {
            paused: true,
            notify_failures: true,
            notify_done: true,
        })
        .unwrap();
        assert!(s.due(now, true, |_| true).is_empty(), "paused");
        assert_eq!(s.get(&id).unwrap().next_ms, None);
        s.set_settings(&Settings {
            paused: false,
            notify_failures: false,
            notify_done: true,
        })
        .unwrap();

        let plan = s.due(now, true, |_| true).remove(0);
        let started = s.begin(&plan, "a", now).unwrap().unwrap();
        let ended = s
            .finish(&started.run, Outcome::Skip("no key".into()), now)
            .unwrap()
            .unwrap();
        assert_eq!(ended.run.status, "skipped");

        let manual = s.run_now(&id, now + 5).unwrap();
        let plans = s.due(now + 5, true, |_| true);
        assert!(plans.iter().any(|p| p.run == manual && p.manual));
        let failed = {
            let p = plans.iter().find(|p| p.run == manual).unwrap();
            let started = s.begin(p, "a", now + 5).unwrap().unwrap();
            assert_eq!(started.args["$run"]["manual"], true);
            s.finish(&started.run, Outcome::Failed("boom".into()), now + 6)
                .unwrap()
                .unwrap()
        };
        assert!(!failed.notify, "alerts off");
        // A manual run does not move the schedule's own slot.
        assert_eq!(s.get(&id).unwrap().last_slot_ms, Some(plan.slot_ms));
        assert_eq!(s.runs(&id, 10).len(), 2);
        s.remove(&id).unwrap();
        assert!(s.runs(&id, 10).is_empty());
    }

    #[test]
    fn manifest_schedules_keep_user_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        let weekly = ScheduleInput {
            repeat: Repeat::Week {
                days: vec!["sun".into()],
                at: "18:00".into(),
            },
            ..hourly("Weekly review")
        };
        s.ensure("p", "reports", "weekly-review", &weekly).unwrap();
        let id = schedule_id("p", "reports", "weekly-review");
        s.set_repeat(&id, &Repeat::Day { at: "20:00".into() })
            .unwrap();
        s.set_enabled(&id, false).unwrap();
        s.ensure(
            "p",
            "reports",
            "weekly-review",
            &ScheduleInput {
                args: json!({"v": 2}),
                ..weekly.clone()
            },
        )
        .unwrap();
        let info = s.get(&id).unwrap();
        assert_eq!(info.repeat, Repeat::Day { at: "20:00".into() });
        assert!(!info.enabled);
        assert_eq!(info.args["v"], 2);
        assert_eq!(info.source, "manifest");
        s.retain_manifest("p", "reports", &[]).unwrap();
        assert!(s.get(&id).is_none());
    }

    #[test]
    fn rejects_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = open(dir.path(), "a");
        assert!(s.set("p", "x", "bad id", &hourly("n"), "plugin").is_err());
        assert!(s.set("p", "x", "k", &hourly(""), "plugin").is_err());
        assert!(s
            .set(
                "p",
                "x",
                "k",
                &ScheduleInput {
                    tz: Some("Mars/Base".into()),
                    ..hourly("n")
                },
                "plugin"
            )
            .is_err());
        assert!(s
            .set(
                "p",
                "x",
                "k",
                &ScheduleInput {
                    args: json!([1]),
                    ..hourly("n")
                },
                "plugin"
            )
            .is_err());
    }
}
