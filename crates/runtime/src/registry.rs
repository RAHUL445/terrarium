//! Loaded plugins on one device. Each plugin is compiled once and has one
//! store per space; an operation runs against the store of one space, and
//! interop only reaches plugins' stores in that same space.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use terrarium_core::{DeviceId, Store as CoreStore, StoreConfig, SystemTime};

use terrarium_ai::{Ai, AiConfig, AiError, Completion, Job, Sealer};
use terrarium_scheduler::{Finished, Outcome, Repeat, ScheduleInput, Scheduler};

use crate::error::{Result, RuntimeError};
use crate::host::{CallContext, Execution, HostState, PluginPre, Runtime, ScheduleChange};
use crate::manifest::{Manifest, RunOn, Schedule};
use crate::packages::{Packages, STORE_ID};
use crate::pages::{Pages, PAGES_STORE_ID};
use crate::spaces::{split_store_id, store_id, Spaces, DEFAULT_SPACE, SPACES_STORE_ID};

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

pub(crate) struct LoadedPlugin {
    manifest: Manifest,
    dir: PathBuf,
    pre: PluginPre<HostState>,
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
    /// Spaces whose stores are open (always includes the default space).
    open_spaces: BTreeSet<String>,
    /// AI service and scheduler: `None` only while lent to a running call.
    ai: Option<Ai>,
    scheduler: Option<Scheduler>,
    assistant: crate::assistant::Assistant,
}

impl PluginHost {
    pub fn new(cfg: HostConfig) -> Result<PluginHost> {
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| RuntimeError::io(&cfg.data_dir, e))?;
        let packages = Packages::open(&cfg.data_dir, store_config(&cfg, STORE_ID))?;
        let spaces = Spaces::open(&cfg.data_dir, store_config(&cfg, SPACES_STORE_ID))?;
        let pages = Pages::open(&cfg.data_dir, store_config(&cfg, PAGES_STORE_ID))?;
        let ai = Ai::open(&cfg.data_dir, store_config(&cfg, terrarium_ai::STORE_ID))?;
        let scheduler = Scheduler::open(
            &cfg.data_dir,
            store_config(&cfg, terrarium_scheduler::STORE_ID),
            &cfg.timezone,
        )?;
        Ok(PluginHost {
            runtime: Runtime::new(cfg.execution)?,
            cfg,
            plugins: BTreeMap::new(),
            packages,
            spaces,
            pages,
            open_spaces: [DEFAULT_SPACE.to_string()].into_iter().collect(),
            ai: Some(ai),
            scheduler: Some(scheduler),
            assistant: crate::assistant::Assistant::default(),
        })
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
        let db = dir.join(format!("{}.db", manifest.id));
        // After a rollback the database may be ahead of this version's
        // schema. Migrations only add, so keep the newer schema.
        let schema = match CoreStore::stored_schema(&db)? {
            Some(stored) if stored.version > manifest.storage.version => stored,
            _ => manifest.storage.clone(),
        };
        let cfg = store_config(&self.cfg, &store_id(space, &manifest.id));
        Ok(CoreStore::open(db, cfg, schema, Box::new(SystemTime))?)
    }

    /// Opens and closes spaces to match the space list. Returns whether
    /// anything changed. Closed spaces keep their data on disk.
    pub fn sync_spaces(&mut self) -> Result<bool> {
        let want: BTreeSet<String> = self.spaces.active()?.into_iter().collect();
        let mut changed = false;
        for space in self.open_spaces.clone() {
            if !want.contains(&space) {
                for loaded in self.plugins.values_mut() {
                    loaded.stores.remove(&space);
                }
                self.open_spaces.remove(&space);
                changed = true;
            }
        }
        for space in want {
            if self.open_spaces.contains(&space) {
                continue;
            }
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
        self.plugins.remove(plugin).is_some()
    }

    /// Loads (or reloads) the built plugin package at `dir`: its manifest,
    /// `dist/plugin.wasm`, templates and storage. A reload with a higher
    /// storage version runs the additive migration.
    pub fn load(&mut self, dir: &Path) -> Result<&Manifest> {
        let manifest = Manifest::read(dir)?;
        let wasm_path = dir.join("dist").join("plugin.wasm");
        let bytes = std::fs::read(&wasm_path).map_err(|e| RuntimeError::io(&wasm_path, e))?;
        let pre = self.runtime.prepare(&bytes)?;

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

        // Release the previous connections before migrating the same files.
        self.plugins.remove(&manifest.id);
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
                dir: dir.to_path_buf(),
                pre,
                stores,
                templates,
            },
        );
        for space in self.open_spaces.clone() {
            self.sync_jobs(&space, &id);
        }
        Ok(&self.plugins[&id].manifest)
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
        self.plugins.get(plugin).map(|p| p.dir.as_path())
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
        if !self.open_spaces.contains(space) {
            return Err(RuntimeError::UnknownSpace(space.into()));
        }
        let peers = Peers {
            space: space.to_string(),
            runtime: Some(self.runtime.clone()),
            timezone: self.cfg.timezone.clone(),
            speech: self.cfg.speech.clone(),
            plugins: std::mem::take(&mut self.plugins),
            ai: self.ai.take(),
            scheduler: self.scheduler.take(),
            run,
        };
        let (peers, result) = invoke(peers, plugin, operation, args, 0);
        self.plugins = peers.plugins;
        self.ai = peers.ai;
        self.scheduler = peers.scheduler;
        result
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
        self.ai_mut().set_sealer(sealer);
    }

    /// The next queued AI request, if a worker slot is free. Run it with
    /// [`terrarium_ai::complete`] without holding the host, then hand the
    /// result to [`PluginHost::deliver_ai`].
    pub fn take_ai_job(&mut self) -> Option<(Job, AiConfig, Option<String>)> {
        let next = self.ai_mut().take()?;
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
            (Err(e), _) if e.temporary => Outcome::Retry(e.message.clone()),
            (Err(e), _) => Outcome::Failed(e.message.clone()),
            (Ok(_), Ok(_)) if self.ai().has_run(&run) => Outcome::Waiting,
            (Ok(_), Ok(v)) => Outcome::Done(notice(v)),
        };
        self.scheduler_mut()
            .finish(&run, outcome, now_ms())
            .unwrap_or_else(|e| {
                eprintln!("run {run}: {e}");
                None
            })
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
            .query_in(space, "todo", "tasks", &params)
            .map_err(|_| {
                "To-do is unavailable in this space. Uncheck today's tasks and try again."
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

    /// Makes the manifest's jobs schedules of `plugin` in `space`.
    fn sync_jobs(&mut self, space: &str, plugin: &str) {
        let Some(loaded) = self.plugins.get(plugin) else {
            return;
        };
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
            .flat_map(|s| self.plugins.keys().map(move |p| (s.clone(), p.clone())))
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
                        _ if self.ai().has_run(&started.run) => Outcome::Waiting,
                        _ => Outcome::Done(notice(&value)),
                    }
                }
                Ok(_) if self.ai().has_run(&started.run) => Outcome::Waiting,
                Ok(_) => Outcome::Done(None),
                Err(RuntimeError::Plugin { code, message }) if code == "retry" => {
                    Outcome::Retry(message)
                }
                Err(RuntimeError::Plugin { code, message }) if code == "skip" => {
                    Outcome::Skip(message)
                }
                Err(e) => Outcome::Failed(e.to_string()),
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
    /// `(space, plugin, row)`.
    pub fn reminders(&self, now_ms: u64) -> Vec<(String, String, Map<String, Value>)> {
        let mut out = Vec::new();
        for space in &self.open_spaces {
            for (id, loaded) in &self.plugins {
                let Some(def) = &loaded.manifest.reminders else {
                    continue;
                };
                let mut values = Map::new();
                values.insert("now_ms".into(), Value::from(now_ms));
                match self.query_in(space, id, &def.query, &values) {
                    Ok(rows) => {
                        out.extend(rows.into_iter().map(|r| (space.clone(), id.clone(), r)))
                    }
                    Err(e) => eprintln!("reminders query of {id} in {space} failed: {e}"),
                }
            }
        }
        out
    }

    /// The plugin's storage in the default space, for inspection.
    pub fn store(&self, plugin: &str) -> Result<&CoreStore> {
        self.get(plugin)?.store(DEFAULT_SPACE)
    }

    pub fn store_in(&self, space: &str, plugin: &str) -> Result<&CoreStore> {
        self.get(plugin)?.store(space)
    }

    fn get(&self, plugin: &str) -> Result<&LoadedPlugin> {
        self.plugins
            .get(plugin)
            .ok_or_else(|| RuntimeError::UnknownPlugin(plugin.into()))
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
    timezone: String,
    pub speech: Option<crate::speech::SpeechConfig>,
    plugins: BTreeMap<String, LoadedPlugin>,
    pub ai: Option<Ai>,
    pub scheduler: Option<Scheduler>,
    /// The scheduled run this call serves, if any.
    pub run: Option<String>,
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
    let uses = loaded.manifest.uses.clone();
    let ctx = CallContext {
        plugin_id: id,
        timezone: &timezone,
        limits: loaded.manifest.limits,
        uses: &uses,
        speech: loaded.manifest.permissions.iter().any(|p| p == "speech"),
        ai: loaded.manifest.permissions.iter().any(|p| p == "ai"),
        operations: loaded.manifest.operations.keys().cloned().collect(),
        depth,
    };
    let (store, mut peers, result) = runtime.call(&loaded.pre, store, ctx, peers, operation, args);
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

fn now_ms() -> u64 {
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

impl terrarium_core::StoreSet for PluginHost {
    fn plugin_ids(&self) -> Vec<String> {
        let mut ids = vec![
            STORE_ID.to_string(),
            SPACES_STORE_ID.to_string(),
            PAGES_STORE_ID.to_string(),
            terrarium_ai::STORE_ID.to_string(),
            terrarium_scheduler::STORE_ID.to_string(),
        ];
        for (id, loaded) in &self.plugins {
            ids.extend(loaded.stores.keys().map(|space| store_id(space, id)));
        }
        ids
    }

    fn store_mut(&mut self, id: &str) -> Option<&mut CoreStore> {
        match id {
            STORE_ID => return Some(self.packages.store_mut()),
            SPACES_STORE_ID => return Some(self.spaces.store_mut()),
            PAGES_STORE_ID => return Some(self.pages.store_mut()),
            terrarium_ai::STORE_ID => return self.ai.as_mut().map(Ai::store_mut),
            terrarium_scheduler::STORE_ID => {
                return self.scheduler.as_mut().map(Scheduler::store_mut)
            }
            _ => {}
        }
        let (space, plugin) = split_store_id(id);
        self.plugins
            .get_mut(plugin)?
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
