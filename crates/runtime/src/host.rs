//! The Wasmtime side: engine configuration, host imports, and running one
//! operation in a fresh, limited instance.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};
use terrarium_core::{Error as CoreError, Store as CoreStore};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap, UpdateDeadline};

use crate::error::{Result, RuntimeError};
use crate::manifest::{EventDef, Limits};
use crate::registry::Peers;

mod bindings {
    wasmtime::component::bindgen!({ path: "../../wit", world: "plugin" });
}

use bindings::terrarium::plugin as wit;
use bindings::PluginError;
pub(crate) use bindings::{Plugin, PluginPre};

const EPOCH_TICK_MS: u64 = 10;
/// Storage and log calls allowed per operation.
const MAX_HOST_CALLS: u32 = 5_000;
const MAX_LOG_LINES: usize = 100;
pub(crate) const MAX_ARGS: usize = 12 * 1024 * 1024;
pub(crate) const MAX_REPLY: usize = 4 * 1024 * 1024;
const MAX_EVENTS_PER_CALL: usize = 50;
const MAX_EVENT_PAYLOAD: usize = 16 * 1024;
const MAX_WRITES: usize = 32 * 1024 * 1024;
/// How deep plugin-to-plugin calls may nest.
pub(crate) const MAX_DEPTH: u8 = 4;

pub(crate) struct HostState {
    pub store: CoreStore,
    pub timezone: String,
    pub plugin_id: String,
    limits: StoreLimits,
    host_calls: u32,
    log_lines: usize,
    write_bytes: usize,
    /// Interfaces the running plugin declared under `uses`.
    uses: Vec<String>,
    /// Whether the manifest asked for the `speech` permission.
    speech: bool,
    /// Whether the manifest asked for the `ai` permission.
    ai: bool,
    http: bool,
    hosts: Vec<String>,
    http_jobs: Vec<String>,
    /// The plugin's declared operations (targets for AI replies and schedules).
    operations: Vec<String>,
    /// AI requests queued by this call, dropped again if it rolls back.
    ai_jobs: Vec<String>,
    /// Schedule changes, applied only if this call commits.
    schedule_changes: Vec<ScheduleChange>,
    declared_events: BTreeMap<String, EventDef>,
    pub(crate) emitted: Vec<EmittedEvent>,
    depth: u8,
    /// Epoch ticks spent blocked in slow host calls (speech), given back
    /// to the plugin's time limit.
    grace_ticks: u64,
    /// Every other loaded plugin, moved in for the duration of the call so
    /// interop can run them; handed back when the call ends.
    pub peers: Peers,
}

/// Who is calling and with what limits.
pub(crate) struct CallContext<'a> {
    pub plugin_id: &'a str,
    pub timezone: &'a str,
    pub limits: Limits,
    pub uses: &'a [String],
    pub speech: bool,
    pub ai: bool,
    pub http: bool,
    pub hosts: Vec<String>,
    pub operations: Vec<String>,
    pub events: BTreeMap<String, EventDef>,
    pub depth: u8,
}

/// One accepted event, held until its operation commits.
pub(crate) type EmittedEvent = crate::events::EventRecord;

/// A schedule change a plugin made, held until its operation commits.
pub(crate) enum ScheduleChange {
    Set(String, terrarium_scheduler::ScheduleInput),
    Remove(String),
    RunNow(String, u64),
    Retry(String),
}

/// Cheap to clone: engine and linker are shared.
#[derive(Clone)]
pub(crate) struct Runtime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    engine: Engine,
    linker: Linker<HostState>,
    stop: Arc<AtomicBool>,
    native: bool,
}

/// How plugin code is executed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Execution {
    /// Native code via the Cranelift JIT when this device supports it,
    /// otherwise the Pulley interpreter.
    #[default]
    Auto,
    Native,
    /// Pulley bytecode interpreter: no executable memory needed. Used on
    /// iOS and on older Android kernels (before Linux 4.16 the JIT cannot
    /// flush the instruction cache).
    Interpreter,
}

/// A tiny module with one function: compiling it publishes executable code,
/// which is the step that fails where the JIT is not supported.
const PROBE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version
    0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
    0x03, 0x02, 0x01, 0x00, // func 0 has type 0
    0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b, // body: end
];

fn engine(native: bool) -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
    if !native {
        config.target("pulley64")?;
    }
    Ok(Engine::new(&config)?)
}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Runtime {
    pub fn new(execution: Execution) -> Result<Runtime> {
        let (engine, native) = match execution {
            Execution::Native => (engine(true)?, true),
            Execution::Interpreter => (engine(false)?, false),
            Execution::Auto => {
                let native = engine(true)?;
                if wasmtime::Module::new(&native, PROBE).is_ok() {
                    (native, true)
                } else {
                    (engine(false)?, false)
                }
            }
        };

        let mut linker = Linker::new(&engine);
        Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;

        let stop = Arc::new(AtomicBool::new(false));
        let ticker_engine = engine.clone();
        let ticker_stop = stop.clone();
        thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                while !ticker_stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(EPOCH_TICK_MS));
                    ticker_engine.increment_epoch();
                }
            })
            .map_err(|e| RuntimeError::Wasm(format!("epoch thread: {e}")))?;

        Ok(Runtime {
            inner: Arc::new(RuntimeInner {
                engine,
                linker,
                stop,
                native,
            }),
        })
    }

    /// `"native"` (Cranelift JIT) or `"interpreter"` (Pulley).
    pub fn mode(&self) -> &'static str {
        if self.inner.native {
            "native"
        } else {
            "interpreter"
        }
    }

    /// Compiles and links a component. Linking fails for any import other
    /// than the platform interfaces, so no WASI or other ambient access.
    pub fn prepare(&self, bytes: &[u8]) -> Result<PluginPre<HostState>> {
        let component = Component::from_binary(&self.inner.engine, bytes)?;
        let pre = self.inner.linker.instantiate_pre(&component)?;
        Ok(PluginPre::new(pre)?)
    }

    /// Runs `operation` in a fresh instance. The storage transaction commits
    /// only when the plugin returns `ok`; errors and traps roll it back.
    /// The store is passed by value and always handed back, even on failure.
    pub fn call(
        &self,
        pre: &PluginPre<HostState>,
        core: CoreStore,
        ctx: CallContext<'_>,
        peers: Peers,
        operation: &str,
        args: &Value,
    ) -> (CoreStore, Peers, Result<Value>) {
        let args_json = match bounded_json(args, MAX_ARGS) {
            Ok(json) => json,
            Err(error) => return (core, peers, Err(error)),
        };
        let limits = ctx.limits;
        transaction(core, ctx, peers, |state| {
            let mut store = Store::new(&self.inner.engine, state);
            store.limiter(|s| &mut s.limits);
            store.epoch_deadline_callback(|mut ctx| {
                let state = ctx.data_mut();
                if state.grace_ticks > 0 {
                    let extra = std::mem::take(&mut state.grace_ticks);
                    Ok(UpdateDeadline::Continue(extra))
                } else {
                    Err(Trap::Interrupt.into())
                }
            });

            let result = (|| -> Result<Value> {
                store.set_fuel(limits.fuel)?;
                store.set_epoch_deadline(limits.timeout_ms.div_ceil(EPOCH_TICK_MS) + 1);
                let instance = pre.instantiate(&mut store).map_err(stopped)?;
                let reply = instance
                    .call_handle(&mut store, operation, &args_json)
                    .map_err(stopped)?;
                match reply {
                    Ok(json) if json.len() > MAX_REPLY => Err(RuntimeError::Plugin {
                        code: "quota".into(),
                        message: "Plugin result exceeds 4 MiB".into(),
                    }),
                    Ok(json) => serde_json::from_str(&json)
                        .map_err(|_| RuntimeError::Trap("returned invalid JSON".into())),
                    Err(e) if e.code.len() > 64 || e.message.len() > 16 * 1024 => {
                        Err(RuntimeError::Plugin {
                            code: "quota".into(),
                            message: "Plugin error exceeds its size limit".into(),
                        })
                    }
                    Err(e) => Err(RuntimeError::Plugin {
                        code: e.code,
                        message: e.message,
                    }),
                }
            })();

            (store.into_data(), result)
        })
    }
}

/// Keep side effects only after the storage transaction successfully commits.
fn finish_call(
    data: HostState,
    result: Result<Value>,
    mark: i64,
    http_checkpoint: Option<u64>,
) -> (CoreStore, Peers, Result<Value>) {
    let (mut core, mut peers) = (data.store, data.peers);
    if result.is_ok() {
        if let Some(trace) = peers.trace.as_mut() {
            if let Ok((batch, _)) = core.export_since(mark) {
                trace.extend(
                    batch
                        .changes
                        .iter()
                        .map(|c| crate::registry::ChangeSummary::of(&data.plugin_id, c)),
                );
            }
        }
    }
    let keep = result.is_ok() && !peers.dry_run;
    let finished = match keep {
        true => core.commit(),
        false => core.rollback(),
    };
    let (committed, result) = match finished {
        Ok(()) => (keep, result),
        Err(e) => {
            let _ = core.rollback();
            (false, Err(e.into()))
        }
    };
    if committed {
        peers.events.extend(
            data.emitted
                .into_iter()
                .map(|event| (data.plugin_id.clone(), event)),
        );
        peers.apply_schedule_changes(&data.plugin_id, data.schedule_changes);
    } else {
        if let Some(ai) = peers.ai.as_mut() {
            for id in &data.ai_jobs {
                ai.cancel(id);
            }
        }
        if let Some(net) = peers.net.as_mut() {
            for id in &data.http_jobs {
                net.cancel(id);
            }
            if let Some(mark) = http_checkpoint {
                net.cancel_since(mark);
            }
        }
    }
    (core, peers, result)
}

pub(crate) fn transaction(
    core: CoreStore,
    ctx: CallContext<'_>,
    peers: Peers,
    execute: impl FnOnce(HostState) -> (HostState, Result<Value>),
) -> (CoreStore, Peers, Result<Value>) {
    let mut core = core;
    // The newest change before the call: what follows is this call's.
    let mark = match peers.trace {
        Some(_) => core
            .changes(1)
            .ok()
            .and_then(|c| c.first().map(|r| r.lseq))
            .unwrap_or(0),
        None => 0,
    };
    if let Err(e) = core.begin() {
        return (core, peers, Err(e.into()));
    }
    let http_checkpoint = peers.net.as_ref().map(terrarium_net::Net::checkpoint);
    let limits = ctx.limits;
    let state = HostState {
        store: core,
        timezone: ctx.timezone.to_string(),
        plugin_id: ctx.plugin_id.to_string(),
        limits: StoreLimitsBuilder::new()
            .memory_size(limits.memory_mb as usize * 1024 * 1024)
            .memories(4)
            .instances(32)
            .tables(32)
            .table_elements(20_000)
            .trap_on_grow_failure(true)
            .build(),
        host_calls: 0,
        log_lines: 0,
        write_bytes: 0,
        uses: ctx.uses.to_vec(),
        speech: ctx.speech,
        ai: ctx.ai,
        http: ctx.http,
        hosts: ctx.hosts,
        http_jobs: Vec::new(),
        operations: ctx.operations,
        ai_jobs: Vec::new(),
        schedule_changes: Vec::new(),
        declared_events: ctx.events,
        emitted: Vec::new(),
        depth: ctx.depth,
        grace_ticks: 0,
        peers,
    };
    let (data, result) = execute(state);
    let result = result.and_then(|v| {
        bounded_json(&v, MAX_REPLY)?;
        Ok(v)
    });
    finish_call(data, result, mark, http_checkpoint)
}

pub(crate) fn plugin_error(e: PluginError) -> RuntimeError {
    RuntimeError::Plugin {
        code: e.code,
        message: e.message,
    }
}
pub(crate) fn charge_write(
    total: &mut usize,
    bytes: usize,
) -> std::result::Result<(), PluginError> {
    *total = total.saturating_add(bytes);
    if *total > MAX_WRITES {
        return Err(PluginError {
            code: "quota".into(),
            message: "Storage writes exceed 32 MiB in one operation".into(),
        });
    }
    Ok(())
}

fn stopped(e: wasmtime::Error) -> RuntimeError {
    let reason = match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "it used more than its fuel limit".to_string(),
        Some(Trap::Interrupt) => "it ran past its time limit".to_string(),
        Some(trap) => format!("{trap}"),
        None => format!("{e:#}"),
    };
    RuntimeError::Trap(reason)
}

impl HostState {
    fn charge_write(&mut self, bytes: usize) -> std::result::Result<(), PluginError> {
        charge_write(&mut self.write_bytes, bytes)
    }

    fn count_call(&mut self) -> std::result::Result<(), PluginError> {
        self.host_calls += 1;
        if self.host_calls > MAX_HOST_CALLS {
            return Err(PluginError {
                code: "quota".into(),
                message: format!("more than {MAX_HOST_CALLS} host calls in one operation"),
            });
        }
        Ok(())
    }
}

pub(crate) fn to_plugin_error(e: CoreError) -> PluginError {
    let code = match &e {
        CoreError::NotFound { .. } => "not-found",
        CoreError::InTrash { .. } => "in-trash",
        CoreError::ReadOnlyQuery => "read-only",
        CoreError::QueryLimit(_) => "quota",
        CoreError::Schema(_)
        | CoreError::UnknownTable(_)
        | CoreError::UnknownColumn { .. }
        | CoreError::Type { .. }
        | CoreError::AppendOnly(_)
        | CoreError::IsCounter { .. }
        | CoreError::NotCounter { .. }
        | CoreError::Policy { .. } => "invalid",
        _ => "storage",
    };
    PluginError {
        code: code.into(),
        message: e.to_string(),
    }
}

fn parse_object(json: &str, what: &str) -> std::result::Result<Map<String, Value>, PluginError> {
    check_json_size(json)?;
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(PluginError {
            code: "invalid".into(),
            message: format!("{what} must be a JSON object"),
        }),
    }
}

fn parse_value(json: &str) -> std::result::Result<Value, PluginError> {
    check_json_size(json)?;
    serde_json::from_str(json).map_err(|e| PluginError {
        code: "invalid".into(),
        message: e.to_string(),
    })
}

impl wit::types::Host for HostState {}

impl wit::storage::Host for HostState {
    fn query(&mut self, sql: String, params: String) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        if sql.len() > 64 * 1024 || params.len() > 1024 * 1024 {
            return Err(PluginError {
                code: "quota".into(),
                message: "Query input exceeds its size limit".into(),
            });
        }
        let params = match parse_value(&params)? {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            _ => {
                return Err(PluginError {
                    code: "invalid".into(),
                    message: "params must be a JSON array".into(),
                })
            }
        };
        let rows = if self.speech {
            self.store.query_bounded_media(&sql, &params)
        } else {
            self.store.query_bounded(&sql, &params)
        }
        .map_err(to_plugin_error)?;
        Ok(Value::Array(rows.into_iter().map(Value::Object).collect()).to_string())
    }

    fn insert(
        &mut self,
        table: String,
        values: String,
    ) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        self.charge_write(table.len().saturating_add(values.len()))?;
        let values = parse_object(&values, "values")?;
        self.store
            .write(|w| w.insert(&table, &values))
            .map_err(to_plugin_error)
    }

    fn update(
        &mut self,
        table: String,
        row: String,
        values: String,
    ) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        self.charge_write(
            table
                .len()
                .saturating_add(row.len())
                .saturating_add(values.len()),
        )?;
        let values = parse_object(&values, "values")?;
        self.store
            .write(|w| w.update(&table, &row, &values))
            .map_err(to_plugin_error)
    }

    fn increment(
        &mut self,
        table: String,
        row: String,
        column: String,
        delta: String,
    ) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        self.charge_write(
            table
                .len()
                .saturating_add(row.len())
                .saturating_add(column.len())
                .saturating_add(delta.len()),
        )?;
        let delta = parse_value(&delta)?;
        self.store
            .write(|w| w.increment(&table, &row, &column, delta))
            .map_err(to_plugin_error)
    }

    fn delete(&mut self, table: String, row: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        self.charge_write(table.len().saturating_add(row.len()))?;
        self.store
            .write(|w| w.delete(&table, &row))
            .map_err(to_plugin_error)
    }

    fn restore(&mut self, table: String, row: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        self.charge_write(table.len().saturating_add(row.len()))?;
        self.store
            .write(|w| w.restore(&table, &row))
            .map_err(to_plugin_error)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn invalid(message: String) -> PluginError {
    PluginError {
        code: "invalid".into(),
        message,
    }
}

impl HostState {
    fn tz<'a>(&'a self, tz: &'a str) -> &'a str {
        if tz.is_empty() {
            &self.timezone
        } else {
            tz
        }
    }
}

impl wit::clock::Host for HostState {
    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    fn timezone(&mut self) -> String {
        self.timezone.clone()
    }

    fn today(&mut self, tz: String) -> std::result::Result<String, PluginError> {
        crate::time::today(now_ms(), self.tz(&tz)).map_err(invalid)
    }

    fn local_date(&mut self, at_ms: u64, tz: String) -> std::result::Result<String, PluginError> {
        crate::time::local_date(at_ms, self.tz(&tz)).map_err(invalid)
    }

    fn local_to_utc(
        &mut self,
        date: String,
        time: String,
        tz: String,
    ) -> std::result::Result<u64, PluginError> {
        crate::time::local_to_utc(&date, &time, self.tz(&tz)).map_err(invalid)
    }

    fn weekday(&mut self, date: String) -> std::result::Result<u8, PluginError> {
        crate::time::weekday(&date).map_err(invalid)
    }

    fn add_days(&mut self, date: String, days: i32) -> std::result::Result<String, PluginError> {
        crate::time::add_days(&date, days).map_err(invalid)
    }
}

impl wit::interop::Host for HostState {
    fn call(
        &mut self,
        iface: String,
        operation: String,
        args: String,
    ) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        if !self.uses.contains(&iface) {
            return Err(PluginError {
                code: "not-allowed".into(),
                message: format!(
                    "`{}` does not declare `{iface}` under `uses`",
                    self.plugin_id
                ),
            });
        }
        if self.depth + 1 >= MAX_DEPTH {
            return Err(PluginError {
                code: "quota".into(),
                message: "plugin calls are nested too deeply".into(),
            });
        }
        let args = parse_value(&args)?;
        let peers = std::mem::take(&mut self.peers);
        let (peers, result) = peers.call_interface(&iface, &operation, &args, self.depth + 1);
        self.peers = peers;
        match result {
            Ok(value) => Ok(value.to_string()),
            Err(RuntimeError::Plugin { code, message }) => Err(PluginError { code, message }),
            Err(e) => Err(PluginError {
                code: "unavailable".into(),
                message: e.to_string(),
            }),
        }
    }

    fn available(&mut self, iface: String) -> bool {
        self.peers.provider(&iface).is_some()
    }
}

impl wit::speech::Host for HostState {
    fn available(&mut self) -> bool {
        self.speech && self.peers.speech.as_ref().is_some_and(|c| c.ready())
    }

    fn transcribe(&mut self, audio_base64: String) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        if !self.speech {
            return Err(PluginError {
                code: "denied".into(),
                message: "the manifest does not ask for the `speech` permission".into(),
            });
        }
        let Some(config) = self.peers.speech.clone() else {
            return Err(PluginError {
                code: "unavailable".into(),
                message: "local speech-to-text is not set up on this device".into(),
            });
        };
        let started = std::time::Instant::now();
        let result = crate::speech::transcribe(&config, &audio_base64);
        // Time spent waiting for the model does not count against the plugin.
        self.grace_ticks += (started.elapsed().as_millis() as u64).div_ceil(EPOCH_TICK_MS) + 1;
        result.map_err(|message| PluginError {
            code: "unavailable".into(),
            message,
        })
    }
}

fn denied(message: impl Into<String>) -> PluginError {
    PluginError {
        code: "denied".into(),
        message: message.into(),
    }
}

fn unavailable(message: impl Into<String>) -> PluginError {
    PluginError {
        code: "unavailable".into(),
        message: message.into(),
    }
}

impl wit::http::Host for HostState {
    fn available(&mut self) -> bool {
        self.count_call().is_ok()
            && self.http
            && self
                .peers
                .net
                .as_ref()
                .is_some_and(|net| net.available(&self.peers.space, &self.plugin_id))
    }
    fn fetch(
        &mut self,
        request: String,
        on_done: String,
    ) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        let result = (|| {
            if !self.http {
                return Err(denied("http"));
            }
            if !self.operations.contains(&on_done) {
                return Err(invalid("The callback must be a declared operation.".into()));
            }
            let map = |e: terrarium_net::NetError| PluginError {
                code: e.code,
                message: e.message,
            };
            let request = terrarium_net::Request::parse(&request).map_err(map)?;
            let allowed = terrarium_net::HostRules::new(&self.hosts).map_err(map)?;
            let net = self
                .peers
                .net
                .as_mut()
                .ok_or_else(|| unavailable("Web requests are not available in this app."))?;
            let id = net
                .submit(
                    &self.peers.space,
                    &self.plugin_id,
                    &on_done,
                    request,
                    self.peers.run.clone(),
                    &allowed,
                )
                .map_err(map)?;
            self.http_jobs.push(id.clone());
            Ok(id)
        })();
        if let Err(error) = &result {
            if let Some(net) = self.peers.net.as_mut() {
                net.record_rejection(&self.peers.space, &self.plugin_id, &error.code)
                    .map_err(|e| unavailable(e.message))?;
            }
        }
        result
    }
}

impl wit::ai::Host for HostState {
    fn available(&mut self) -> bool {
        self.ai && self.peers.ai.as_ref().is_some_and(|ai| ai.status().ready)
    }

    fn submit(
        &mut self,
        request: String,
        on_done: String,
    ) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        if !self.ai {
            return Err(denied("the manifest does not ask for the `ai` permission"));
        }
        if !self.operations.contains(&on_done) {
            return Err(invalid(format!("`{on_done}` is not a declared operation")));
        }
        let request: terrarium_ai::Request =
            serde_json::from_str(&request).map_err(|e| invalid(format!("AI request: {e}")))?;
        let space = self.peers.space.clone();
        let run = self.peers.run.clone();
        let ai = self
            .peers
            .ai
            .as_mut()
            .ok_or_else(|| unavailable("AI is not available in this app"))?;
        let id = ai
            .submit(&space, &self.plugin_id, &on_done, request, run)
            .map_err(|e| PluginError {
                code: e.code,
                message: e.message,
            })?;
        self.ai_jobs.push(id.clone());
        Ok(id)
    }
}

impl HostState {
    fn scheduler(&self) -> std::result::Result<&terrarium_scheduler::Scheduler, PluginError> {
        self.peers
            .scheduler
            .as_ref()
            .ok_or_else(|| unavailable("scheduling is not available in this app"))
    }

    fn schedule_id(&self, key: &str) -> String {
        terrarium_scheduler::schedule_id(&self.peers.space, &self.plugin_id, key)
    }

    /// Fails unless `key` is one of this plugin's schedules in this space,
    /// counting changes made earlier in the same call.
    fn own_schedule(&self, key: &str) -> std::result::Result<String, PluginError> {
        let id = self.schedule_id(key);
        let mut exists = self.scheduler()?.get(&id).is_some();
        for change in &self.schedule_changes {
            match change {
                ScheduleChange::Set(k, _) if k == key => exists = true,
                ScheduleChange::Remove(k) if k == key => exists = false,
                _ => {}
            }
        }
        if exists {
            Ok(id)
        } else {
            Err(PluginError {
                code: "not-found".into(),
                message: format!("no schedule `{key}`"),
            })
        }
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> std::result::Result<String, PluginError> {
    serde_json::to_string(value).map_err(|e| invalid(e.to_string()))
}

impl wit::schedule::Host for HostState {
    fn set(&mut self, id: String, schedule: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        let input: terrarium_scheduler::ScheduleInput =
            serde_json::from_str(&schedule).map_err(|e| invalid(format!("schedule: {e}")))?;
        if !self.operations.contains(&input.operation) {
            return Err(invalid(format!(
                "`{}` is not a declared operation",
                input.operation
            )));
        }
        let scheduler = self.scheduler()?;
        scheduler.check(&input).map_err(invalid)?;
        if id.is_empty()
            || id.len() > 60
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(invalid(format!(
                "schedule id `{id}` must be 1–60 letters, digits, `-`, `_` or `.`"
            )));
        }
        let pending = self
            .schedule_changes
            .iter()
            .filter(|c| matches!(c, ScheduleChange::Set(..)))
            .count();
        if scheduler
            .list(Some(&self.peers.space), Some(&self.plugin_id))
            .len()
            + pending
            >= 100
        {
            return Err(PluginError {
                code: "quota".into(),
                message: "at most 100 schedules per plugin".into(),
            });
        }
        self.schedule_changes.push(ScheduleChange::Set(id, input));
        Ok(())
    }

    fn remove(&mut self, id: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        self.own_schedule(&id)?;
        self.schedule_changes.push(ScheduleChange::Remove(id));
        Ok(())
    }

    fn list(&mut self) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        let list = self
            .scheduler()?
            .list(Some(&self.peers.space), Some(&self.plugin_id));
        to_json(&list)
    }

    fn runs(&mut self, id: String, limit: u32) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        let full = self.own_schedule(&id)?;
        to_json(&self.scheduler()?.runs(&full, limit.min(200) as usize))
    }

    fn run_now(&mut self, id: String) -> std::result::Result<String, PluginError> {
        self.count_call()?;
        let full = self.own_schedule(&id)?;
        let now = now_ms();
        self.schedule_changes.push(ScheduleChange::RunNow(id, now));
        Ok(format!("{full}@m{now}"))
    }

    fn retry(&mut self, run_id: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        let prefix = self.schedule_id("");
        let run = self
            .scheduler()?
            .run(&run_id)
            .filter(|r| r.schedule.starts_with(&prefix))
            .ok_or_else(|| PluginError {
                code: "not-found".into(),
                message: format!("no run `{run_id}`"),
            })?;
        if !matches!(run.status.as_str(), "failed" | "skipped" | "retry") {
            return Err(invalid("only failed or skipped runs can be retried".into()));
        }
        self.schedule_changes.push(ScheduleChange::Retry(run_id));
        Ok(())
    }
}

impl wit::events::Host for HostState {
    fn emit(&mut self, name: String, payload: String) -> std::result::Result<(), PluginError> {
        self.count_call()?;
        let event = self
            .declared_events
            .get(&name)
            .ok_or_else(|| invalid(format!("`{name}` is not a declared event")))?;
        let quota = || PluginError {
            code: "quota".into(),
            message: "Event limit exceeded".into(),
        };
        if payload.len() > MAX_EVENT_PAYLOAD {
            return Err(quota());
        }
        let payload = parse_object(&payload, "event payload")?;
        crate::commands::check_args(&event.payload, &[], &payload)
            .map_err(|e| invalid(e.message))?;
        if self.emitted.len() >= MAX_EVENTS_PER_CALL
            || self.peers.event_count >= MAX_EVENTS_PER_CALL
        {
            return Err(quota());
        }
        self.peers.event_count += 1;
        let accepted = EmittedEvent::accept(&self.peers, &self.plugin_id, name, payload, event)
            .map_err(|_| invalid("Event metadata unavailable".into()))?;
        self.emitted.push(accepted);
        Ok(())
    }
}

impl wit::log::Host for HostState {
    fn info(&mut self, message: String) {
        if self.log_lines < MAX_LOG_LINES {
            self.log_lines += 1;
            let message: String = message.chars().take(500).collect();
            eprintln!("[plugin {}] {message}", self.plugin_id);
        }
    }
}

fn check_json_size(json: &str) -> std::result::Result<(), PluginError> {
    if json.len() > MAX_ARGS {
        return Err(PluginError {
            code: "quota".into(),
            message: "Host-call JSON exceeds 12 MiB".into(),
        });
    }
    Ok(())
}

pub(crate) fn bounded_json(value: &Value, limit: usize) -> Result<String> {
    struct Output {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit - self.bytes.len() {
                return Err(std::io::Error::other("JSON size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Output {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value).map_err(|_| RuntimeError::Plugin {
        code: "quota".into(),
        message: if limit == MAX_REPLY {
            "Plugin result exceeds 4 MiB"
        } else {
            "Operation input exceeds 12 MiB"
        }
        .into(),
    })?;
    Ok(String::from_utf8(out.bytes).expect("JSON is UTF-8"))
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    #[test]
    fn serialized_input_budget_counts_escaped_bytes_without_unbounded_allocation() {
        assert!(bounded_json(&serde_json::json!({"key":"value"}), 64).is_ok());
        assert!(bounded_json(&serde_json::json!("\n".repeat(20)), 30).is_err());
        assert!(check_json_size(&"x".repeat(MAX_ARGS + 1)).is_err());
    }
    fn event_state() -> HostState {
        let store = CoreStore::open(
            ":memory:",
            terrarium_core::StoreConfig {
                plugin: "events-test".into(),
                space: "personal".into(),
                device: "device".into(),
                user: "test".into(),
            },
            serde_json::from_value(serde_json::json!({"version":1,"tables":[]})).unwrap(),
            Box::new(terrarium_core::SystemTime),
        )
        .unwrap();
        HostState {
            store,
            timezone: "UTC".into(),
            plugin_id: "events-test".into(),
            limits: StoreLimitsBuilder::new().build(),
            host_calls: 0,
            log_lines: 0,
            write_bytes: 0,
            uses: Vec::new(),
            speech: false,
            ai: false,
            http: false,
            hosts: Vec::new(),
            http_jobs: Vec::new(),
            operations: Vec::new(),
            ai_jobs: Vec::new(),
            schedule_changes: Vec::new(),
            declared_events: BTreeMap::from([("pinged".into(), EventDef::default())]),
            emitted: Vec::new(),
            depth: 0,
            grace_ticks: 0,
            peers: {
                let mut peers = Peers::default();
                peers.timezone = "UTC".into();
                peers
            },
        }
    }

    #[test]
    fn event_chain_context_and_source_are_frozen_at_acceptance() {
        let mut state = event_state();
        state.peers.device = "source-device".into();
        state.peers.space = "source-space".into();
        state.peers.event_depth = 2;
        state.peers.event_root = Some(format!("ev-source-device-{}", uuid::Uuid::now_v7()));
        wit::events::Host::emit(&mut state, "pinged".into(), "{}".into()).unwrap();
        let accepted = state.emitted[0].clone();
        assert_eq!(accepted.plugin, state.plugin_id);
        assert_eq!(accepted.space, "source-space");
        assert_eq!(accepted.device, "source-device");
        assert_eq!(accepted.depth, 2);
        assert_eq!(Some(&accepted.root), state.peers.event_root.as_ref());
        assert_eq!(
            accepted.contract,
            crate::events::contract(&state.declared_events["pinged"])
        );
        state.peers.space = "other-space".into();
        state.peers.event_depth = 3;
        state.peers.event_root = None;
        state.peers.timezone = "Asia/Kolkata".into();
        state.declared_events.clear();
        assert_eq!(state.emitted[0], accepted);
    }

    #[test]
    fn commit_failure_drops_own_events_and_preserves_committed_children() {
        let mut state = event_state();
        wit::events::Host::emit(&mut state, "pinged".into(), "{}".into()).unwrap();
        state
            .peers
            .events
            .push(("committed-child".into(), state.emitted[0].clone()));
        // No active transaction: SQLite rejects the actual COMMIT. Exercise the
        // same finalization path as Runtime::call, without a synthetic result.
        let (_, peers, result) = finish_call(state, Ok(Value::Null), 0, None);
        assert!(matches!(result, Err(RuntimeError::Core(_))));
        assert_eq!(peers.events.len(), 1);
        assert_eq!(peers.events[0].0, "committed-child");
    }

    #[test]
    fn every_event_attempt_counts_even_during_dry_runs() {
        let mut state = event_state();
        state.peers.dry_run = true;
        for _ in 0..MAX_HOST_CALLS {
            assert_eq!(
                wit::events::Host::emit(&mut state, "undeclared".into(), "{}".into())
                    .unwrap_err()
                    .code,
                "invalid"
            );
        }
        assert_eq!(
            wit::events::Host::emit(&mut state, "pinged".into(), "{}".into())
                .unwrap_err()
                .code,
            "quota"
        );
        assert!(state.emitted.is_empty());
    }

    #[test]
    fn malformed_json_has_a_value_free_error() {
        let mut state = event_state();
        let error = wit::events::Host::emit(&mut state, "pinged".into(), "{secret-value".into())
            .unwrap_err();
        assert_eq!(error.code, "invalid");
        assert!(!error.message.contains("secret-value"));
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn inclusive_write_budget_and_saturating_overflow() {
        let mut bytes = 0;
        charge_write(&mut bytes, MAX_WRITES).unwrap();
        assert!(charge_write(&mut bytes, 1).is_err());
        let mut bytes = usize::MAX;
        assert!(charge_write(&mut bytes, 1).is_err());
    }
    #[test]
    fn inclusive_serialized_input_and_reply_budgets() {
        for max in [MAX_ARGS, MAX_REPLY] {
            assert!(bounded_json(&Value::String("x".repeat(max - 2)), max).is_ok());
            assert!(bounded_json(&Value::String("x".repeat(max - 1)), max).is_err());
        }
    }
}
