//! Command-line control: a Unix socket in the app's data folder that the
//! `terrarium` command line talks to (protocol in
//! `terrarium-control-proto`). Off until turned on in Settings → Developer.
//!
//! Each client is added by name in Settings and gets a token, shown once;
//! the app keeps only its hash. A connection says `hello` with its token,
//! then runs registry commands with that client's scope. Every command a
//! client runs is recorded in an audit log (without its arguments).
//!
//! Settings and the audit log are kept on this device only: a token for
//! this Mac means nothing on another device.

// Phones have no socket; only the settings commands are used there.
#![cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use terrarium_control_proto::{token_hash, TOKEN_PREFIX};
use terrarium_runtime::commands::Scope;

pub use crate::now_ms;
use crate::{Plugins, Shell};

const CONFIG_FILE: &str = "control.json";
const AUDIT_FILE: &str = "control-audit.jsonl";
/// Audit entries kept in memory and shown in Settings.
const AUDIT_KEEP: usize = 200;
/// The audit file is trimmed back to `AUDIT_KEEP` entries past this size.
const AUDIT_MAX_BYTES: u64 = 512 * 1024;
const MAX_CLIENTS: usize = 20;
const MAX_NAME: usize = 40;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Config {
    enabled: bool,
    clients: Vec<Client>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    /// Hex SHA-256 of the token.
    hash: String,
    pub created_ms: u64,
    #[serde(default)]
    pub last_used_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at_ms: u64,
    pub client: String,
    /// `hello`, `describe`, `status`, `subscribe`, or the command id.
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// The rows an action changed (or would have, on a dry run), at most
    /// `AUDIT_CHANGES`: `{plugin, table, row, op, column?}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Value>,
    /// How many changes there were in all.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub change_count: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Changes kept per audit entry.
const AUDIT_CHANGES: usize = 50;
/// Longest `ui.notify` message.
const MAX_NOTICE: usize = 200;

/// Command-line control settings, audit log and socket. Cloning shares it.
#[derive(Clone)]
pub struct Control(Arc<Inner>);

pub struct Inner {
    dir: PathBuf,
    config: Mutex<Config>,
    audit: Mutex<VecDeque<AuditEntry>>,
    /// Why the socket is not listening, when it should be.
    error: Mutex<Option<String>>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    server: Mutex<Option<server::Server>>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    hub: Arc<server::Hub>,
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn new_token() -> Result<String, String> {
    use base64::Engine;
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| e.to_string())?;
    Ok(format!(
        "{TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    ))
}

impl std::ops::Deref for Control {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Control {
    /// Reads settings and recent audit entries from `dir` (the app's data
    /// folder).
    pub fn open(dir: &Path) -> Control {
        let config = crate::private::read_private(&dir.join(CONFIG_FILE))
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let audit = std::fs::read_to_string(dir.join(AUDIT_FILE))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect::<VecDeque<AuditEntry>>();
        let skip = audit.len().saturating_sub(AUDIT_KEEP);
        Control(Arc::new(Inner {
            dir: dir.to_path_buf(),
            config: Mutex::new(config),
            audit: Mutex::new(audit.into_iter().skip(skip).collect()),
            error: Mutex::new(None),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            server: Mutex::new(None),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            hub: Arc::default(),
        }))
    }

    fn save(&self, config: &Config) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
        crate::private::write_private(&self.dir.join(CONFIG_FILE), &bytes)
            .map_err(|e| e.to_string())
    }

    pub fn socket_path(&self) -> PathBuf {
        self.dir.join(terrarium_control_proto::SOCKET_FILE)
    }

    /// The client a token belongs to, if any, marked as used now.
    fn authenticate(&self, token: &str) -> Option<Client> {
        let hash = token_hash(token);
        let mut config = self.config.lock().ok()?;
        if !config.enabled {
            return None;
        }
        let client = config.clients.iter_mut().find(|c| c.hash == hash)?;
        client.last_used_ms = Some(now_ms());
        let found = client.clone();
        // Saved under the lock, so a client removed meanwhile can't be
        // written back.
        let _ = self.save(&config);
        Some(found)
    }

    /// Whether client `id` may still run commands.
    fn allowed(&self, id: &str) -> bool {
        self.config
            .lock()
            .is_ok_and(|c| c.enabled && c.clients.iter().any(|x| x.id == id))
    }

    fn record(&self, entry: AuditEntry) {
        let path = self.dir.join(AUDIT_FILE);
        if let Ok(mut audit) = self.audit.lock() {
            audit.push_back(entry.clone());
            while audit.len() > AUDIT_KEEP {
                audit.pop_front();
            }
            let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > AUDIT_MAX_BYTES);
            let result = if too_big {
                let text: String = audit
                    .iter()
                    .filter_map(|e| serde_json::to_string(e).ok())
                    .map(|l| l + "\n")
                    .collect();
                crate::private::write_private(&path, text.as_bytes())
            } else {
                append_private(&path, &entry)
            };
            if let Err(e) = result {
                eprintln!("control audit: {e}");
            }
        }
    }

    pub fn state(&self) -> Value {
        let config = self.config.lock().map(|c| c.clone()).unwrap_or_default();
        let audit: Vec<AuditEntry> = self
            .audit
            .lock()
            .map(|a| a.iter().rev().take(50).cloned().collect())
            .unwrap_or_default();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let listening = self.server.lock().is_ok_and(|s| s.is_some());
        #[cfg(not(not(any(target_os = "android", target_os = "ios"))))]
        let listening = false;
        json!({
            "available": crate::DESKTOP,
            "enabled": config.enabled,
            "listening": listening,
            "socket": self.socket_path(),
            "cli_path": cli_path(),
            "error": self.error.lock().ok().and_then(|e| e.clone()),
            // Token hashes stay out of the webview.
            "clients": config.clients.iter().map(|c| json!({
                "id": c.id,
                "name": c.name,
                "scope": c.scope,
                "created_ms": c.created_ms,
                "last_used_ms": c.last_used_ms,
            })).collect::<Vec<_>>(),
            "audit": audit,
        })
    }

    /// Whether command-line control is turned on.
    pub fn enabled(&self) -> bool {
        self.config.lock().is_ok_and(|c| c.enabled)
    }

    /// Starts or stops the socket to match the settings.
    pub fn apply(&self, plugins: &Plugins, shell: &Arc<dyn Shell>) {
        let enabled = self.config.lock().is_ok_and(|c| c.enabled);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let Ok(mut server) = self.server.lock() else {
                return;
            };
            if enabled && server.is_none() {
                let started = server::Server::start(
                    self.clone(),
                    plugins.clone(),
                    shell.clone(),
                    self.socket_path(),
                );
                match started {
                    Ok(s) => {
                        *server = Some(s);
                        self.set_error(None);
                    }
                    Err(e) => self.set_error(Some(e)),
                }
            } else if !enabled {
                if let Some(s) = server.take() {
                    s.stop();
                }
                self.hub.close_all();
                self.set_error(None);
            }
        }
        #[cfg(not(not(any(target_os = "android", target_os = "ios"))))]
        let _ = (plugins, shell, enabled);
    }

    fn set_error(&self, error: Option<String>) {
        if let Ok(mut e) = self.error.lock() {
            *e = error;
        }
    }

    /// Passes an app event to subscribed clients.
    pub fn broadcast(&self, name: &str) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        self.hub.broadcast(name);
        #[cfg(not(not(any(target_os = "android", target_os = "ios"))))]
        let _ = name;
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut config = self.config.lock().map_err(|e| e.to_string())?;
        config.enabled = enabled;
        self.save(&config)
    }

    pub fn add_client(&self, name: &str, scope: Scope) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!(
                "Give the client a name of 1 to {MAX_NAME} characters."
            ));
        }
        let mut config = self.config.lock().map_err(|e| e.to_string())?;
        if config.clients.len() >= MAX_CLIENTS {
            return Err(format!("At most {MAX_CLIENTS} clients. Remove one first."));
        }
        if config
            .clients
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Err(format!("A client named “{name}” already exists."));
        }
        let token = new_token()?;
        config.clients.push(Client {
            id: random_hex(8)?,
            name: name.to_string(),
            scope,
            hash: token_hash(&token),
            created_ms: now_ms(),
            last_used_ms: None,
        });
        self.save(&config)?;
        Ok(token)
    }

    pub fn remove_client(&self, id: &str) -> Result<(), String> {
        let mut config = self.config.lock().map_err(|e| e.to_string())?;
        let before = config.clients.len();
        config.clients.retain(|c| c.id != id);
        if config.clients.len() == before {
            return Err("No such client.".into());
        }
        self.save(&config)?;
        drop(config);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        self.hub.close_client(id);
        Ok(())
    }
}

/// The `terrarium` binary shipped next to the app's own, if there is one.
fn cli_path() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("terrarium");
    path.is_file().then_some(path)
}

fn append_private(path: &Path, entry: &AuditEntry) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    file.write_all(&line)
}

/// Starts the socket if command-line control is on, once plugins have
/// loaded (so clients never see a half-empty command list).
pub fn start(control: &Control, plugins: &Plugins, shell: Arc<dyn Shell>) {
    let (control, plugins) = (control.clone(), plugins.clone());
    std::thread::spawn(move || {
        plugins.wait_ready(std::time::Duration::from_secs(60));
        control.apply(&plugins, &shell);
    });
}

// ---- The socket ---------------------------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod server {
    use std::io::BufReader;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use serde::Deserialize;
    use serde_json::{json, Value};
    use terrarium_control_proto::{
        read_message, write_message, ReadError, Request, Response, RpcError, INVALID_REQUEST,
        METHOD_NOT_FOUND, PARSE_ERROR,
    };
    use terrarium_runtime::commands::{self, Caller, Kind, Scope};
    use terrarium_runtime::DEFAULT_SPACE;

    use super::{now_ms, AuditEntry, Client, Control, AUDIT_CHANGES, MAX_NOTICE};
    use crate::{Plugins, Shell};

    /// What a connection works with.
    #[derive(Clone)]
    struct Ctx {
        control: Control,
        plugins: Plugins,
        shell: Arc<dyn Shell>,
    }

    /// Longest socket path macOS accepts (`sun_path`), minus the NUL.
    const MAX_SOCKET_PATH: usize = 103;
    const MAX_CONNECTIONS: usize = 16;

    pub struct Server {
        path: PathBuf,
        stop: Arc<AtomicBool>,
    }

    /// Open connections and event subscribers.
    #[derive(Default)]
    pub struct Hub {
        /// (connection id, client id, stream)
        connections: Mutex<Vec<(u64, Option<String>, UnixStream)>>,
        subscribers: Mutex<Vec<(u64, mpsc::Sender<String>)>>,
        next: Mutex<u64>,
    }

    impl Hub {
        fn add(&self, stream: &UnixStream) -> Option<u64> {
            let mut conns = self.connections.lock().ok()?;
            if conns.len() >= MAX_CONNECTIONS {
                return None;
            }
            let mut next = self.next.lock().ok()?;
            *next += 1;
            conns.push((*next, None, stream.try_clone().ok()?));
            Some(*next)
        }

        fn set_client(&self, conn: u64, client: &str) {
            if let Ok(mut conns) = self.connections.lock() {
                if let Some(c) = conns.iter_mut().find(|c| c.0 == conn) {
                    c.1 = Some(client.to_string());
                }
            }
        }

        fn remove(&self, conn: u64) {
            if let Ok(mut conns) = self.connections.lock() {
                conns.retain(|c| c.0 != conn);
            }
            if let Ok(mut subs) = self.subscribers.lock() {
                subs.retain(|s| s.0 != conn);
            }
        }

        fn subscribe(&self, conn: u64) -> mpsc::Receiver<String> {
            let (tx, rx) = mpsc::channel();
            if let Ok(mut subs) = self.subscribers.lock() {
                subs.push((conn, tx));
            }
            rx
        }

        pub fn broadcast(&self, name: &str) {
            if let Ok(mut subs) = self.subscribers.lock() {
                subs.retain(|s| s.1.send(name.to_string()).is_ok());
            }
        }

        fn shutdown(&self, keep: impl Fn(&Option<String>) -> bool) {
            if let Ok(conns) = self.connections.lock() {
                for (_, client, stream) in conns.iter() {
                    if !keep(client) {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                }
            }
        }

        pub fn close_all(&self) {
            self.shutdown(|_| false);
        }

        pub fn close_client(&self, id: &str) {
            self.shutdown(|c| c.as_deref() != Some(id));
        }
    }

    impl Server {
        pub fn start(
            control: Control,
            plugins: Plugins,
            shell: Arc<dyn Shell>,
            path: PathBuf,
        ) -> Result<Server, String> {
            let hub = control.hub.clone();
            let ctx = Ctx {
                control,
                plugins,
                shell,
            };
            if path.as_os_str().len() > MAX_SOCKET_PATH {
                return Err(format!(
                    "The socket path is too long for macOS ({} bytes): {}",
                    path.as_os_str().len(),
                    path.display()
                ));
            }
            // A socket left by a crash is removed; one another running copy
            // of the app answers on is not.
            if path.exists() {
                if UnixStream::connect(&path).is_ok() {
                    return Err("Another copy of Terrarium is already listening.".into());
                }
                let is_socket = {
                    use std::os::unix::fs::FileTypeExt;
                    std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket())
                };
                if !is_socket {
                    return Err(format!("{} exists and is not a socket.", path.display()));
                }
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
            let listener =
                UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let Some(conn) = hub.add(&stream) else {
                        let mut w = &stream;
                        let _ = write_message(
                            &mut w,
                            &Response::err(
                                None,
                                RpcError::command("busy", "Too many connections."),
                            ),
                        );
                        continue;
                    };
                    let (ctx, hub) = (ctx.clone(), hub.clone());
                    std::thread::spawn(move || {
                        serve(&ctx, &hub, conn, stream);
                        hub.remove(conn);
                    });
                }
            });
            Ok(Server { path, stop })
        }

        pub fn stop(self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wake the accept loop so it sees the flag.
            let _ = UnixStream::connect(&self.path);
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Hello {
        token: String,
        #[serde(default)]
        #[allow(dead_code)]
        client: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Invoke {
        kind: Kind,
        id: String,
        #[serde(default)]
        space: Option<String>,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        dry_run: bool,
        #[serde(default)]
        confirmed: bool,
    }

    /// `ui.open` and `ui.notify`: checked like any command, then shown in
    /// the app window by the UI (event `control-ui`).
    fn ui_command(ctx: &Ctx, scope: Scope, call: &Invoke) -> Result<Value, RpcError> {
        if scope != Scope::Change {
            return Err(RpcError::command(
                "forbidden",
                format!(
                    "`{}` changes what the app shows; this client may only read",
                    call.id
                ),
            ));
        }
        if call.dry_run {
            return Err(RpcError::command(
                "invalid",
                "A dry run works only for plugin actions.",
            ));
        }
        let text = |key: &str| call.args.get(key).and_then(Value::as_str);
        let event = match call.id.as_str() {
            "ui.notify" => {
                let message = text("text").map(str::trim).unwrap_or_default();
                if message.is_empty() || message.chars().count() > MAX_NOTICE {
                    return Err(RpcError::command(
                        "invalid",
                        format!("`text` must be 1 to {MAX_NOTICE} characters"),
                    ));
                }
                json!({ "action": "notify", "text": message })
            }
            "ui.open" => {
                let plugin = text("plugin").unwrap_or_default();
                let space = call
                    .space
                    .clone()
                    .unwrap_or_else(|| DEFAULT_SPACE.to_string());
                let page = ctx
                    .plugins
                    .with_host(|host| {
                        if !host.open_spaces().any(|s| s == space) {
                            return Err(RpcError::command(
                                "unknown-space",
                                format!("no open space `{space}`"),
                            ));
                        }
                        let m = host.manifests().find(|m| m.id == plugin).ok_or_else(|| {
                            RpcError::command("not-found", format!("no plugin `{plugin}`"))
                        })?;
                        match text("page") {
                            Some(p) if m.pages.iter().any(|x| x.id == p) => Ok(p.to_string()),
                            Some(p) => Err(RpcError::command(
                                "not-found",
                                format!("`{plugin}` has no page `{p}`"),
                            )),
                            None => m.pages.first().map(|p| p.id.clone()).ok_or_else(|| {
                                RpcError::command("not-found", format!("`{plugin}` has no pages"))
                            }),
                        }
                    })
                    .unwrap_or_else(|| {
                        Err(RpcError::command(
                            "failed",
                            "The plugin host is unavailable.",
                        ))
                    })?;
                json!({ "action": "open", "space": space, "plugin": plugin, "page": page, "item": text("item") })
            }
            other => {
                return Err(RpcError::command(
                    "not-found",
                    format!("no action `{other}`"),
                ))
            }
        };
        if call.args.as_object().is_some_and(|a| {
            a.keys()
                .any(|k| !matches!(k.as_str(), "text" | "plugin" | "page" | "item"))
        }) {
            return Err(RpcError::command("invalid", "unknown argument"));
        }
        ctx.shell
            .show(event)
            .map_err(|message| RpcError::command("unavailable", message))?;
        Ok(json!({ "result": true }))
    }

    fn serve(ctx: &Ctx, hub: &Hub, conn: u64, stream: UnixStream) {
        let control = &ctx.control;
        let Ok(write_half) = stream.try_clone() else {
            return;
        };
        let writer = Arc::new(Mutex::new(write_half));
        let send = |response: &Response| -> bool {
            writer
                .lock()
                .is_ok_and(|mut w| write_message(&mut *w, response).is_ok())
        };
        let mut reader = BufReader::new(stream);
        let mut client: Option<Client> = None;
        loop {
            let request: Request = match read_message(&mut reader) {
                Ok(Some(r)) => r,
                Ok(None) => return,
                Err(ReadError::Io(_)) => return,
                Err(e) => {
                    let code = if matches!(e, ReadError::Json(_)) {
                        PARSE_ERROR
                    } else {
                        INVALID_REQUEST
                    };
                    send(&Response::err(
                        None,
                        RpcError::protocol(code, e.to_string()),
                    ));
                    return;
                }
            };
            let id = request.id;
            if request.jsonrpc != "2.0" {
                send(&Response::err(
                    Some(id),
                    RpcError::protocol(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
                ));
                return;
            }
            // Everything but `hello` needs a client that is still allowed.
            let current = match (&client, request.method.as_str()) {
                (_, "hello") => None,
                (Some(c), _) if control.allowed(&c.id) => Some(c.clone()),
                _ => {
                    send(&Response::err(
                        Some(id),
                        RpcError::command("unauthorized", "Say hello with a valid token first."),
                    ));
                    return;
                }
            };
            let audit = |what: &str, kind: Option<Kind>, result: &Result<Value, RpcError>| {
                let changes = result
                    .as_ref()
                    .ok()
                    .and_then(|v| v["changes"].as_array().cloned())
                    .unwrap_or_default();
                control.record(AuditEntry {
                    at_ms: now_ms(),
                    client: current.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
                    what: what.to_string(),
                    kind: kind.map(|k| match k {
                        Kind::Query => "query".into(),
                        Kind::Action => "action".into(),
                    }),
                    ok: result.is_ok(),
                    code: result.as_ref().err().map(|e| e.kind().to_string()),
                    dry_run: result.as_ref().is_ok_and(|v| v["dry_run"] == true),
                    change_count: changes.len(),
                    changes: changes.into_iter().take(AUDIT_CHANGES).collect(),
                })
            };
            let result: Result<Value, RpcError> = match request.method.as_str() {
                "hello" => {
                    let hello: Result<Hello, _> = serde_json::from_value(request.params);
                    match hello.ok().and_then(|h| control.authenticate(&h.token)) {
                        Some(c) => {
                            hub.set_client(conn, &c.id);
                            let out = json!({
                                "client": c.name,
                                "scope": c.scope,
                                "app_version": env!("CARGO_PKG_VERSION"),
                            });
                            control.record(AuditEntry {
                                at_ms: now_ms(),
                                client: c.name.clone(),
                                what: "hello".into(),
                                ok: true,
                                ..AuditEntry::default()
                            });
                            client = Some(c);
                            Ok(out)
                        }
                        None => {
                            control.record(AuditEntry {
                                at_ms: now_ms(),
                                what: "hello".into(),
                                code: Some("unauthorized".into()),
                                ..AuditEntry::default()
                            });
                            send(&Response::err(
                                Some(id),
                                RpcError::command(
                                    "unauthorized",
                                    "This token is not known, or command-line control is off.",
                                ),
                            ));
                            return;
                        }
                    }
                }
                "status" => {
                    let c = current.as_ref().expect("checked above");
                    ctx.plugins
                        .with_host(|host| {
                            json!({
                                "app_version": env!("CARGO_PKG_VERSION"),
                                "client": c.name,
                                "scope": c.scope,
                                "window": ctx.shell.has_window(),
                                "spaces": host.spaces().list().unwrap_or_default(),
                                "plugins": host.manifests().map(|m| json!({"id": m.id, "name": m.name, "version": m.version})).collect::<Vec<_>>(),
                            })
                        })
                        .ok_or_else(|| RpcError::command("failed", "The plugin host is unavailable."))
                }
                "describe" => {
                    let c = current.as_ref().expect("checked above");
                    let caller = Caller::Client(c.scope);
                    let r = ctx
                        .plugins
                        .with_host(|host| json!(commands::describe(host, caller)))
                        .ok_or_else(|| {
                            RpcError::command("failed", "The plugin host is unavailable.")
                        });
                    audit("describe", None, &r);
                    r
                }
                "invoke" => {
                    let c = current.as_ref().expect("checked above");
                    match serde_json::from_value::<Invoke>(request.params) {
                        Err(e) => Err(RpcError::command(
                            "invalid",
                            format!("bad invoke params: {e}"),
                        )),
                        Ok(call) if call.kind == Kind::Action && call.id.starts_with("ui.") => {
                            let r = ui_command(ctx, c.scope, &call);
                            audit(&call.id, Some(call.kind), &r);
                            r
                        }
                        Ok(call) => {
                            let caller = Caller::Client(c.scope);
                            let options = commands::InvokeOptions {
                                dry_run: call.dry_run,
                                confirmed: call.confirmed,
                            };
                            let r = ctx
                                .plugins
                                .with_host(|host| {
                                    commands::invoke_with(
                                        host,
                                        caller,
                                        call.kind,
                                        &call.id,
                                        call.space.as_deref(),
                                        &call.args,
                                        crate::DESKTOP,
                                        options,
                                    )
                                })
                                .unwrap_or_else(|| {
                                    Err(commands::CommandError::new(
                                        "failed",
                                        "The plugin host is unavailable.",
                                    ))
                                })
                                .map(|outcome| json!(outcome))
                                .map_err(|e| RpcError::command(&e.code, e.message));
                            audit(&call.id, Some(call.kind), &r);
                            if r.is_ok() && call.kind == Kind::Action && !call.dry_run {
                                let event = if call.id.starts_with("space.") {
                                    "spaces-changed"
                                } else {
                                    "data-changed"
                                };
                                ctx.shell.emit(event, Value::Null);
                            }
                            r
                        }
                    }
                }
                "subscribe" => {
                    let rx = hub.subscribe(conn);
                    let writer = writer.clone();
                    std::thread::spawn(move || {
                        for name in rx {
                            let note = Response::notify("event", json!({ "name": name }));
                            let ok = writer
                                .lock()
                                .is_ok_and(|mut w| write_message(&mut *w, &note).is_ok());
                            if !ok {
                                break;
                            }
                        }
                    });
                    let r = Ok(json!(true));
                    audit("subscribe", None, &r);
                    r
                }
                other => Err(RpcError::protocol(
                    METHOD_NOT_FOUND,
                    format!("no method `{other}`"),
                )),
            };
            let response = match result {
                Ok(v) => Response::ok(id, v),
                Err(e) => Response::err(Some(id), e),
            };
            if !send(&response) {
                return;
            }
        }
    }
}
