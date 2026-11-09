//! Text generation as a host service. Plugins ask through the `ai` WIT
//! interface; they never see the provider, the key or the network.
//!
//! The library owns no threads. A host takes queued jobs with [`Ai::take`],
//! runs [`complete`] on a thread of its choosing (without holding the
//! plugin host), then hands the result back with [`Ai::finish`] and
//! delivers it to the plugin.
//!
//! Settings, the sealed API key and monthly usage live in a host-owned
//! store, `_ai`, which syncs like plugin data.

mod client;
mod config;
mod seal;

use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

pub use client::{complete, models, AiError, Completion, Prompt};
pub use config::{preset, AiConfig, Preset, PRESETS};
pub use seal::Sealer;

pub const STORE_ID: &str = "_ai";
const KEY: &str = "api_key";
/// Plugin-supplied `context` echoed back to `on-done`.
const MAX_CONTEXT: usize = 64 * 1024;
/// Jobs waiting at once, across plugins.
const MAX_QUEUED: usize = 50;

const SCHEMA: &str = r#"{
  "version": 1,
  "tables": [
    { "name": "settings", "columns": [
      { "name": "data", "type": "text" },
      { "name": "updated_ms", "type": "integer" }
    ]},
    { "name": "secrets", "columns": [
      { "name": "sealed", "type": "text" },
      { "name": "key_id", "type": "text" },
      { "name": "hint", "type": "text" },
      { "name": "updated_ms", "type": "integer" }
    ]},
    { "name": "usage", "columns": [
      { "name": "month", "type": "text" },
      { "name": "requests", "type": "integer", "rule": "counter" },
      { "name": "failures", "type": "integer", "rule": "counter" },
      { "name": "input_tokens", "type": "integer", "rule": "counter" },
      { "name": "output_tokens", "type": "integer", "rule": "counter" }
    ]}
  ]
}"#;

/// A queued request from a plugin.
#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: String,
    pub space: String,
    pub plugin: String,
    /// The plugin operation that receives the result.
    pub on_done: String,
    pub prompt: Prompt,
    pub context: Value,
    /// The scheduled run this request belongs to, if any.
    pub run: Option<String>,
    pub queued_ms: u64,
}

/// What a plugin's `submit` call carries.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(default)]
    pub system: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub context: Value,
}

/// Whether AI can be used on this device now, and why not.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub ready: bool,
    pub key_set: bool,
    /// The key was sealed for a space this device cannot open (yet).
    pub key_unreadable: bool,
    pub key_hint: Option<String>,
    pub reason: Option<String>,
    pub queued: usize,
    pub running: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Usage {
    pub month: String,
    pub requests: i64,
    pub failures: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

pub struct Ai {
    store: CoreStore,
    sealer: Option<Sealer>,
    queue: VecDeque<Job>,
    running: usize,
    next_id: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn store_err(e: terrarium_core::Error) -> String {
    e.to_string()
}

fn obj(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

impl Ai {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> terrarium_core::Result<Ai> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA).expect("valid AI schema");
        let store = CoreStore::open(
            data_dir.join(format!("{STORE_ID}.db")),
            StoreConfig {
                plugin: STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Ai {
            store,
            sealer: None,
            queue: VecDeque::new(),
            running: 0,
            next_id: 0,
        })
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    /// Sets the key that seals the API key. When the root changes (the
    /// device joined or created a space) a key this device can read is
    /// sealed again under the new root, so paired devices can open it.
    pub fn set_sealer(&mut self, sealer: Sealer) {
        let current = self.key();
        self.sealer = Some(sealer);
        if let (Some(key), Some(row)) = (current, self.key_row()) {
            if text(&row, "key_id").as_deref() != self.sealer.as_ref().map(Sealer::id) {
                let _ = self.set_key(&key);
            }
        }
    }

    pub fn config(&self) -> AiConfig {
        self.store
            .query(
                "SELECT data FROM settings WHERE id = 'config' AND _deleted IS NULL",
                &[],
            )
            .ok()
            .and_then(|rows| rows.first().and_then(|r| text(r, "data")))
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    pub fn set_config(&mut self, config: AiConfig) -> Result<AiConfig, String> {
        let config = config.validate()?;
        let values = obj(json!({
            "data": serde_json::to_string(&config).map_err(|e| e.to_string())?,
            "updated_ms": now_ms(),
        }));
        self.store
            .write(|w| w.upsert("settings", "config", &values))
            .map_err(store_err)?;
        Ok(config)
    }

    fn key_row(&self) -> Option<Map<String, Value>> {
        self.store
            .query(
                "SELECT sealed, key_id, hint FROM secrets WHERE id = ?1 AND _deleted IS NULL",
                &[json!(KEY)],
            )
            .ok()?
            .into_iter()
            .next()
    }

    /// The API key, if set and sealed for a root this device has.
    pub fn key(&self) -> Option<String> {
        let row = self.key_row()?;
        let sealer = self.sealer.as_ref()?;
        if text(&row, "key_id").as_deref() != Some(sealer.id()) {
            return None;
        }
        let plain = sealer.open(KEY, &text(&row, "sealed")?)?;
        String::from_utf8(plain).ok()
    }

    /// Seals and stores the key on every device of the space.
    pub fn set_key(&mut self, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() || key.len() > 500 || key.chars().any(char::is_whitespace) {
            return Err("The API key looks wrong: no spaces, at most 500 characters.".into());
        }
        let sealer = self
            .sealer
            .as_ref()
            .ok_or("Secret storage is not ready on this device yet.")?;
        let tail: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let head: String = key.chars().take(3).collect();
        let values = obj(json!({
            "sealed": sealer.seal(KEY, key.as_bytes()),
            "key_id": sealer.id(),
            "hint": format!("{head}…{tail}"),
            "updated_ms": now_ms(),
        }));
        self.store
            .write(|w| w.upsert("secrets", KEY, &values))
            .map_err(store_err)
    }

    /// Removes the key on every device.
    pub fn remove_key(&mut self) -> Result<(), String> {
        if self.key_row().is_none() {
            return Ok(());
        }
        self.store
            .write(|w| w.delete("secrets", KEY))
            .map_err(store_err)
    }

    pub fn status(&self) -> Status {
        let config = self.config();
        let row = self.key_row();
        let key = self.key();
        let key_set = row.is_some();
        let key_unreadable = key_set && key.is_none();
        let usage = self.usage(&month(now_ms()));
        let reason = if !config.enabled {
            Some("AI is turned off in Settings → AI.".to_string())
        } else if config.base_url.is_empty() {
            Some("Set the provider's base URL in Settings → AI.".into())
        } else if config.model.is_empty() {
            Some("Choose a model in Settings → AI.".into())
        } else if config.needs_key() && !key_set {
            Some("Add an API key in Settings → AI.".into())
        } else if config.needs_key() && key_unreadable {
            Some("The API key was saved on a device this one has not synced with yet. Sync, or enter it again.".into())
        } else if config.monthly_limit > 0 && usage.requests >= i64::from(config.monthly_limit) {
            Some(format!(
                "This month's limit of {} requests is used up.",
                config.monthly_limit
            ))
        } else {
            None
        };
        Status {
            ready: reason.is_none(),
            key_set,
            key_unreadable,
            key_hint: row.and_then(|r| text(&r, "hint")),
            reason,
            queued: self.queue.len(),
            running: self.running,
        }
    }

    pub fn usage(&self, month: &str) -> Usage {
        self.store
            .query(
                "SELECT requests, failures, input_tokens, output_tokens FROM usage WHERE id = ?1 AND _deleted IS NULL",
                &[json!(month)],
            )
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .map(|r| {
                let n = |k: &str| r.get(k).and_then(Value::as_i64).unwrap_or(0);
                Usage {
                    month: month.to_string(),
                    requests: n("requests"),
                    failures: n("failures"),
                    input_tokens: n("input_tokens"),
                    output_tokens: n("output_tokens"),
                }
            })
            .unwrap_or(Usage {
                month: month.to_string(),
                ..Usage::default()
            })
    }

    /// Checks a plugin's request against the config and queues it.
    /// Returns the job id.
    pub fn submit(
        &mut self,
        space: &str,
        plugin: &str,
        on_done: &str,
        request: Request,
        run: Option<String>,
    ) -> Result<String, AiError> {
        let status = self.status();
        if !status.ready {
            return Err(AiError::new(
                "unavailable",
                status.reason.unwrap_or_else(|| "AI is not set up.".into()),
            ));
        }
        let config = self.config();
        if request.prompt.trim().is_empty() {
            return Err(AiError::new("invalid", "the prompt is empty"));
        }
        let size = request.system.len() + request.prompt.len();
        if size > config.max_prompt_kb as usize * 1024 {
            return Err(AiError::new(
                "too-large",
                format!(
                    "The prompt is {} KB; the limit in Settings → AI is {} KB.",
                    size.div_ceil(1024),
                    config.max_prompt_kb
                ),
            ));
        }
        if request.context.to_string().len() > MAX_CONTEXT {
            return Err(AiError::new("invalid", "context must be under 64 KB"));
        }
        if self.queue.len() >= MAX_QUEUED {
            return Err(AiError::new(
                "rate-limited",
                "Too many AI requests are waiting.",
            ));
        }
        let model = request
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && m.len() <= 200)
            .unwrap_or(config.model.clone());
        self.next_id += 1;
        let id = format!("ai-{}-{}", now_ms(), self.next_id);
        self.queue.push_back(Job {
            id: id.clone(),
            space: space.to_string(),
            plugin: plugin.to_string(),
            on_done: on_done.to_string(),
            prompt: Prompt {
                system: request.system,
                prompt: request.prompt,
                model,
                max_tokens: request
                    .max_tokens
                    .unwrap_or(config.max_tokens)
                    .clamp(16, config.max_tokens),
                temperature: request
                    .temperature
                    .filter(|t| t.is_finite())
                    .unwrap_or(config.temperature)
                    .clamp(0.0, 2.0),
            },
            context: request.context,
            run,
            queued_ms: now_ms(),
        });
        Ok(id)
    }

    /// The next job to run, with what [`complete`] needs, if fewer than the
    /// configured number are running.
    pub fn take(&mut self) -> Option<(Job, AiConfig, Option<String>)> {
        let config = self.config();
        if self.running >= usize::from(config.concurrency) {
            return None;
        }
        let job = self.queue.pop_front()?;
        self.running += 1;
        let key = self.key();
        Some((job, config, key))
    }

    /// Records the outcome of a job taken with [`Ai::take`].
    pub fn finish(&mut self, result: &Result<Completion, AiError>) {
        self.running = self.running.saturating_sub(1);
        let month = month(now_ms());
        let _ = self.store.write(|w| {
            w.upsert("usage", &month, &obj(json!({ "month": month })))?;
            w.increment("usage", &month, "requests", json!(1))?;
            match result {
                Ok(c) => {
                    w.increment("usage", &month, "input_tokens", json!(c.input_tokens))?;
                    w.increment("usage", &month, "output_tokens", json!(c.output_tokens))
                }
                Err(_) => w.increment("usage", &month, "failures", json!(1)),
            }
        });
    }

    /// Drops a job that has not started (its operation rolled back).
    pub fn cancel(&mut self, id: &str) {
        self.queue.retain(|j| j.id != id);
    }

    /// Whether a job for scheduled run `run` is queued.
    pub fn has_run(&self, run: &str) -> bool {
        self.queue.iter().any(|j| j.run.as_deref() == Some(run))
    }

    /// Drops queued jobs of a plugin (uninstalled) or of everything.
    pub fn clear(&mut self, plugin: Option<&str>) {
        self.queue.retain(|j| plugin.is_some_and(|p| j.plugin != p));
    }
}

/// `YYYY-MM` of a UTC instant (usage is counted per UTC month).
fn month(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    // Civil-from-days (Howard Hinnant), UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(dir: &Path) -> Ai {
        Ai::open(
            dir,
            StoreConfig {
                space: "t".into(),
                plugin: STORE_ID.into(),
                device: "dev-a".into(),
                user: "u".into(),
            },
        )
        .unwrap()
    }

    fn ready(ai: &mut Ai) {
        ai.set_sealer(Sealer::new("device", &[1; 32]));
        ai.set_config(AiConfig {
            model: "m".into(),
            ..AiConfig::default()
        })
        .unwrap();
        ai.set_key("sk-test-1234").unwrap();
    }

    fn request(prompt: &str) -> Request {
        Request {
            system: String::new(),
            prompt: prompt.into(),
            model: None,
            max_tokens: Some(99_999),
            temperature: None,
            context: json!({"report": "r1"}),
        }
    }

    #[test]
    fn month_of() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(1_793_000_000_000), "2026-10");
    }

    #[test]
    fn key_is_sealed_hinted_and_resealed_for_a_new_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut ai = ai(dir.path());
        ready(&mut ai);
        assert_eq!(ai.key().as_deref(), Some("sk-test-1234"));
        let status = ai.status();
        assert!(status.ready, "{status:?}");
        assert_eq!(status.key_hint.as_deref(), Some("sk-…1234"));
        let dump = serde_json::to_string(&ai.store().dump().unwrap()).unwrap();
        assert!(!dump.contains("sk-test-1234"));

        // Joining a space: the key moves to the space root.
        let space = Sealer::new("space", &[2; 32]);
        ai.set_sealer(space.clone());
        assert_eq!(ai.key().as_deref(), Some("sk-test-1234"));
        assert_eq!(
            text(&ai.key_row().unwrap(), "key_id").as_deref(),
            Some(space.id())
        );

        // A device with another root sees the key but cannot read it.
        ai.sealer = Some(Sealer::new("space", &[3; 32]));
        let status = ai.status();
        assert!(status.key_unreadable && !status.ready);
        ai.remove_key().unwrap();
        assert!(!ai.status().key_set);
    }

    #[test]
    fn submit_checks_and_queue_respects_concurrency() {
        let dir = tempfile::tempdir().unwrap();
        let mut ai = ai(dir.path());
        assert_eq!(
            ai.submit("s", "p", "done", request("x"), None)
                .unwrap_err()
                .code,
            "unavailable"
        );
        ready(&mut ai);
        ai.set_config(AiConfig {
            model: "m".into(),
            concurrency: 1,
            max_prompt_kb: 8,
            ..AiConfig::default()
        })
        .unwrap();
        assert_eq!(
            ai.submit("s", "p", "done", request(&"x".repeat(9000)), None)
                .unwrap_err()
                .code,
            "too-large"
        );
        ai.submit("s", "p", "done", request("one"), Some("run-1".into()))
            .unwrap();
        ai.submit("s", "p", "done", request("two"), None).unwrap();
        let (job, _, key) = ai.take().unwrap();
        assert_eq!(job.prompt.prompt, "one");
        assert_eq!(job.prompt.max_tokens, 2000);
        assert_eq!(job.run.as_deref(), Some("run-1"));
        assert_eq!(key.as_deref(), Some("sk-test-1234"));
        assert!(ai.take().is_none(), "concurrency 1");
        ai.finish(&Ok(Completion {
            text: "hi".into(),
            model: "m".into(),
            input_tokens: 10,
            output_tokens: 5,
        }));
        assert!(ai.take().is_some());
        ai.finish(&Err(AiError::new("offline", "no")));
        let usage = ai.usage(&month(now_ms()));
        assert_eq!(
            (usage.requests, usage.failures, usage.input_tokens),
            (2, 1, 10)
        );
    }

    #[test]
    fn monthly_limit_stops_submits() {
        let dir = tempfile::tempdir().unwrap();
        let mut ai = ai(dir.path());
        ready(&mut ai);
        ai.set_config(AiConfig {
            model: "m".into(),
            monthly_limit: 1,
            ..AiConfig::default()
        })
        .unwrap();
        ai.submit("s", "p", "done", request("one"), None).unwrap();
        ai.take().unwrap();
        ai.finish(&Err(AiError::new("offline", "no")));
        assert!(ai.status().reason.unwrap().contains("limit"));
    }

    #[test]
    fn completes_against_an_openai_compatible_server() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            for status in [200, 401, 429] {
                let mut req = server.recv().unwrap();
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string());
                assert_eq!(auth.as_deref(), Some("Bearer sk-1"));
                let sent: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(sent["messages"][0]["role"], "system");
                let reply = if status == 200 {
                    json!({"model": "m-1", "choices": [{"message": {"content": " Hello "}}], "usage": {"prompt_tokens": 7, "completion_tokens": 2}})
                } else {
                    json!({"error": {"message": "PRIVATE_TRANSCRIPT"}})
                };
                req.respond(
                    tiny_http::Response::from_string(reply.to_string()).with_status_code(status),
                )
                .unwrap();
            }
        });
        let config = AiConfig {
            preset: "custom".into(),
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "m".into(),
            ..AiConfig::default()
        };
        let prompt = Prompt {
            system: "be brief".into(),
            prompt: "hi".into(),
            model: "m".into(),
            max_tokens: 50,
            temperature: 0.2,
        };
        let done = complete(&config, Some("sk-1"), &prompt).unwrap();
        assert_eq!((done.text.as_str(), done.input_tokens), ("Hello", 7));
        let e = complete(&config, Some("sk-1"), &prompt).unwrap_err();
        assert_eq!(e.code, "unauthorized");
        assert!(!e.temporary);
        let e = complete(&config, Some("sk-1"), &prompt).unwrap_err();
        assert!(
            e.code == "rate-limited" && e.temporary && !e.message.contains("PRIVATE_TRANSCRIPT")
        );
        handle.join().unwrap();
    }
}
