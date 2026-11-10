//! `terrarium mcp`: the running app's commands as MCP tools over stdio.
//!
//! An MCP client (an agent) starts `terrarium mcp`; it talks MCP on stdin
//! and stdout (newline-delimited JSON-RPC 2.0) and the app's socket on the
//! other side, with the client's token. No logic lives here: `tools/list`
//! is `describe`, `tools/call` is `invoke`, so the token's scope, argument
//! checks, confirmation and the audit log all apply as for `terrarium`.
//!
//! - Tool names are command ids with `.` turned into `_` (`todo_create`),
//!   since many clients allow only letters, digits, `_` and `-`. A query
//!   that shares its id with an action is `<name>_query`.
//! - Every tool that runs in a space takes `space`; plugin actions take
//!   `dry_run`.
//! - Destructive tools need the user's yes. When the client supports
//!   elicitation the server asks the user directly; otherwise the tool
//!   takes `confirmed`, and its description tells the agent to ask first.
//! - When plugins change, the server sends `notifications/tools/list_changed`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{saved_token, Conn, Failure, Options, Outcome};

/// Protocol versions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Terrarium is the user's personal app: tasks, notes, habits, \
bookmarks, voice captures and reports, in one or more spaces. These tools are the running \
app's own commands. Dates are YYYY-MM-DD and times HH:MM in the user's time zone; today's \
date and similar clock values are filled in for you. Use search_query to find items across \
plugins. Actions return the rows they changed; pass dry_run: true to a plugin action to see \
what it would change without keeping anything. Destructive tools need the user's confirmation.";

/// Commands that are not offered as tools: `tools/list` already describes
/// everything.
const SKIPPED: &[&str] = &["app.describe"];

/// Where the server writes: stdout, shared with the plugin watcher.
type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(out: &Out, message: &Value) {
    if let Ok(mut out) = out.lock() {
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }
}

// ---- Tools --------------------------------------------------------------------

/// A command offered as a tool.
#[derive(Clone, Debug)]
struct Tool {
    kind: String,
    id: String,
    info: Value,
    /// The extra arguments this tool takes on top of the command's own.
    takes_space: bool,
    takes_dry_run: bool,
    takes_confirmed: bool,
}

impl Tool {
    fn destructive(&self) -> bool {
        self.info["effect"] == "destructive"
    }
}

/// Tools for every command in `describe`, by name.
fn tools_from(all: &Value, elicit: bool) -> BTreeMap<String, Tool> {
    let commands: Vec<&Value> = all
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let base = |c: &Value| c["id"].as_str().unwrap_or_default().replace('.', "_");
    let actions: Vec<String> = commands
        .iter()
        .filter(|c| c["kind"] == "action")
        .map(|c| base(c))
        .collect();
    let mut tools = BTreeMap::new();
    for c in commands {
        let id = c["id"].as_str().unwrap_or_default();
        if id.is_empty() || SKIPPED.contains(&id) {
            continue;
        }
        let kind = c["kind"].as_str().unwrap_or_default().to_string();
        let mut name = base(c);
        if kind == "query" && actions.contains(&name) {
            name.push_str("_query");
        }
        let declared = |p: &str| c["params"].get(p).is_some();
        let tool = Tool {
            takes_space: c["space"] == true && !declared("space"),
            takes_dry_run: kind == "action" && c["owner"] == "plugin" && !declared("dry_run"),
            takes_confirmed: c["effect"] == "destructive" && !elicit && !declared("confirmed"),
            kind,
            id: id.to_string(),
            info: c.clone(),
        };
        tools.insert(name, tool);
    }
    tools
}

/// JSON Schema for one declared parameter.
fn param_schema(spec: &Value) -> Value {
    let ty = spec["type"].as_str().unwrap_or("json");
    let mut s = match ty {
        "string" | "text" | "id" => json!({ "type": "string" }),
        "date" => {
            json!({ "type": "string", "format": "date", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" })
        }
        "time" => json!({ "type": "string", "pattern": "^\\d{2}:\\d{2}$" }),
        "integer" => json!({ "type": "integer" }),
        "number" => json!({ "type": "number" }),
        "boolean" => json!({ "type": "boolean" }),
        "enum" => json!({ "type": "string", "enum": spec["values"] }),
        _ => json!({}),
    };
    if matches!(ty, "integer" | "number") {
        if let Some(min) = spec.get("min").filter(|v| !v.is_null()) {
            s["minimum"] = min.clone();
        }
        if let Some(max) = spec.get("max").filter(|v| !v.is_null()) {
            s["maximum"] = max.clone();
        }
    }
    let mut description = spec["description"].as_str().unwrap_or_default().to_string();
    let hint = match ty {
        "date" => "YYYY-MM-DD.",
        "time" => "HH:MM, 24-hour.",
        "id" => "An item id from another tool's result.",
        "json" => "Any JSON value.",
        _ => "",
    };
    if !hint.is_empty() && !description.contains(hint) {
        if !description.is_empty() {
            description.push(' ');
        }
        description.push_str(hint);
    }
    if !description.is_empty() {
        s["description"] = json!(description);
    }
    s
}

fn input_schema(tool: &Tool) -> Value {
    let params = tool.info["params"].as_object();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, spec) in params.into_iter().flatten() {
        properties.insert(name.clone(), param_schema(spec));
        if spec["required"] == true {
            required.push(json!(name));
        }
    }
    if tool.takes_space {
        properties.insert(
            "space".into(),
            json!({
                "type": "string",
                "description": "The space to run in, by id (see space_list). Default: the default space.",
            }),
        );
    }
    if tool.takes_dry_run {
        properties.insert(
            "dry_run".into(),
            json!({
                "type": "boolean",
                "description": "Run it, report what it would change, and keep nothing.",
            }),
        );
    }
    if tool.takes_confirmed {
        properties.insert(
            "confirmed".into(),
            json!({
                "type": "boolean",
                "description": "Set to true only after the user has agreed to this change.",
            }),
        );
    }
    let mut schema = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    // Commands that declare their parameters take nothing else.
    if params.is_some() {
        schema["additionalProperties"] = json!(false);
    }
    schema
}

fn description(tool: &Tool) -> String {
    let info = &tool.info;
    let mut text = info["description"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| info["summary"].as_str())
        .unwrap_or_default()
        .to_string();
    if tool.destructive() {
        let question = info["confirm"]
            .as_str()
            .unwrap_or("It removes or hides data.");
        text.push_str(&format!("\n\nDestructive. {question}"));
        if tool.takes_confirmed {
            text.push_str(
                " Ask the user first and pass confirmed: true only if they agree. \
                 dry_run: true shows what it would change.",
            );
        }
    }
    if let Some(example) = info["examples"].as_array().and_then(|e| e.first()) {
        text.push_str(&format!("\n\nExample arguments: {example}"));
    }
    text
}

fn tool_json(name: &str, tool: &Tool) -> Value {
    let effect = tool.info["effect"].as_str().unwrap_or("write");
    let mut annotations = json!({
        "title": tool.id,
        "readOnlyHint": effect == "read",
        "openWorldHint": false,
    });
    if effect != "read" {
        annotations["destructiveHint"] = json!(effect == "destructive");
        annotations["idempotentHint"] = json!(false);
    }
    json!({
        "name": name,
        "title": tool.id,
        "description": description(tool),
        "inputSchema": input_schema(tool),
        "annotations": annotations,
    })
}

// ---- Session ------------------------------------------------------------------

struct Session {
    options: Options,
    token: String,
    conn: Option<Conn>,
    tools: BTreeMap<String, Tool>,
    /// Set by the watcher when plugins change, so tools are listed again.
    stale: Arc<AtomicBool>,
    /// The client can ask its user questions (`elicitation/create`).
    elicit: bool,
    out: Out,
    input: Receiver<Value>,
    /// Messages that arrived while waiting for the client's answer.
    queued: VecDeque<Value>,
    next: u64,
    watching: bool,
}

/// A JSON-RPC error.
fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// A tool result that reports an error to the agent.
fn tool_error(code: &str, message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("{code}: {message}") }],
        "structuredContent": { "error": { "code": code, "message": message } },
        "isError": true,
    })
}

impl Session {
    fn conn(&mut self) -> Result<&mut Conn, Failure> {
        if self.conn.as_ref().is_some_and(Conn::is_closed) {
            self.conn = None;
        }
        if self.conn.is_none() {
            let (conn, _) = Conn::open(&self.options, &self.token)?;
            self.conn = Some(conn);
        }
        Ok(self.conn.as_mut().expect("opened above"))
    }

    /// Calls the app. A connection the app closed is replaced before
    /// sending; one lost while waiting for the answer is retried only when
    /// `retry` (reads), since an action may already have run.
    fn call(&mut self, method: &str, params: Value, retry: bool) -> Result<Value, Failure> {
        match self.conn()?.call(method, params.clone()) {
            Err(f) if f.code == "unreachable" => {
                self.conn = None;
                if retry {
                    self.conn()?.call(method, params)
                } else {
                    Err(Failure::new(
                        "unreachable",
                        "Lost the connection to the app while it was running this. \
                         It may or may not have run; check before trying again.",
                    ))
                }
            }
            other => other,
        }
    }

    fn refresh(&mut self) -> Result<(), Failure> {
        self.stale.store(false, Ordering::SeqCst);
        let all = self.call("describe", json!({}), true)?;
        self.tools = tools_from(&all, self.elicit);
        Ok(())
    }

    /// The next message from the client, or `None` once stdin closes.
    fn next_message(&mut self) -> Option<Value> {
        self.queued.pop_front().or_else(|| self.input.recv().ok())
    }

    fn handle(&mut self, message: Value) {
        let Some(method) = message["method"].as_str().map(str::to_string) else {
            return; // a response we did not wait for
        };
        let Some(id) = message.get("id").cloned() else {
            if method == "notifications/initialized" {
                self.watch();
            }
            return;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let reply = match method.as_str() {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => self.list().map_err(|f| (-32603, f.message)),
            "tools/call" => self.tool_call(&params),
            other => Err((-32601, format!("Method not found: {other}"))),
        };
        let response = match reply {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => rpc_error(id, code, message),
        };
        send(&self.out, &response);
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params["protocolVersion"].as_str().unwrap_or_default();
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        self.elicit = params["capabilities"].get("elicitation").is_some();
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": {
                "name": "terrarium",
                "title": "Terrarium",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": INSTRUCTIONS,
        })
    }

    fn list(&mut self) -> Result<Value, Failure> {
        self.refresh()?;
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|(name, tool)| tool_json(name, tool))
            .collect();
        Ok(json!({ "tools": tools }))
    }

    fn tool_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default().to_string();
        if self.tools.is_empty()
            || self.stale.load(Ordering::SeqCst)
            || !self.tools.contains_key(&name)
        {
            if let Err(f) = self.refresh() {
                return Ok(tool_error(&f.code, &f.message));
            }
        }
        let tool = self
            .tools
            .get(&name)
            .cloned()
            .ok_or_else(|| (-32602, format!("Unknown tool: {name}")))?;
        let mut args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => return Err((-32602, "arguments must be an object".into())),
        };
        let space = match tool.takes_space {
            true => args
                .remove("space")
                .or_else(|| self.options.space.clone().map(Value::from)),
            false => None,
        };
        let dry_run = match tool.takes_dry_run {
            true => match args.remove("dry_run") {
                None | Some(Value::Null) | Some(Value::Bool(false)) => false,
                Some(Value::Bool(true)) => true,
                // Never run for real when a preview was meant.
                Some(_) => return Ok(tool_error("invalid", "`dry_run` must be true or false")),
            },
            false => false,
        };
        let said_yes = match tool.takes_confirmed {
            true => args.remove("confirmed") == Some(Value::Bool(true)),
            // A command that declares `confirmed` itself keeps it.
            false => args.get("confirmed") == Some(&Value::Bool(true)),
        };
        let mut confirmed = false;
        if tool.destructive() && !dry_run {
            let question = tool.info["confirm"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("Run `{}`? It removes or hides data.", tool.id));
            if self.elicit {
                match self.ask(&question, &tool.id) {
                    Ok(true) => confirmed = true,
                    Ok(false) => {
                        return Ok(tool_error(
                            "declined",
                            "The user did not confirm; nothing was changed.",
                        ))
                    }
                    Err(message) => return Ok(tool_error("failed", &message)),
                }
            } else if said_yes {
                confirmed = true;
            } else {
                return Ok(tool_error(
                    "needs-confirmation",
                    &format!(
                        "{question} Ask the user; if they agree, call again with confirmed: true. \
                         dry_run: true shows what it would change."
                    ),
                ));
            }
        }
        let request = json!({
            "kind": tool.kind,
            "id": tool.id,
            "space": space,
            "args": Value::Object(args),
            "dry_run": dry_run,
            "confirmed": confirmed,
        });
        let retry = tool.kind == "query";
        Ok(match self.call("invoke", request, retry) {
            Ok(outcome) => {
                let text = serde_json::to_string_pretty(&outcome).unwrap_or_default();
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": outcome,
                    "isError": false,
                })
            }
            Err(f) => tool_error(&f.code, &f.message),
        })
    }

    /// Asks the user through the client; `Ok(true)` when they agree.
    fn ask(&mut self, question: &str, id: &str) -> Result<bool, String> {
        self.next += 1;
        let request_id = format!("terrarium-confirm-{}", self.next);
        send(
            &self.out,
            &json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "elicitation/create",
                "params": {
                    "message": format!("Terrarium: {question}\n\n({id})"),
                    "requestedSchema": {
                        "type": "object",
                        "properties": {
                            "confirm": {
                                "type": "boolean",
                                "title": "Yes, do it",
                                "default": true,
                            }
                        },
                    },
                },
            }),
        );
        loop {
            let Ok(message) = self.input.recv() else {
                return Err("The client closed the connection.".into());
            };
            if message["id"] != json!(request_id) || message.get("method").is_some() {
                self.queued.push_back(message);
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!(
                    "The client could not ask the user: {}",
                    error["message"].as_str().unwrap_or("error")
                ));
            }
            let result = &message["result"];
            return Ok(result["action"] == "accept" && result["content"]["confirm"] != false);
        }
    }

    /// Follows the app's events on a second connection and tells the client
    /// when the tools change.
    fn watch(&mut self) {
        if self.watching {
            return;
        }
        self.watching = true;
        let (options, token) = (self.options.clone(), self.token.clone());
        let (out, stale) = (self.out.clone(), self.stale.clone());
        let changed = move || {
            stale.store(true, Ordering::SeqCst);
            send(
                &out,
                &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
            );
        };
        std::thread::spawn(move || {
            let mut reconnecting = false;
            loop {
                match Conn::open(&options, &token) {
                    Ok((mut conn, _)) => {
                        if conn.call("subscribe", json!({})).is_ok() {
                            // After a restart the app may offer other tools.
                            if reconnecting {
                                changed();
                            }
                            while let Ok(message) = conn.read() {
                                let name = message.params.as_ref().map(|p| p["name"].clone());
                                if message.method.as_deref() == Some("event")
                                    && name == Some(json!("plugins-changed"))
                                {
                                    changed();
                                }
                            }
                        }
                    }
                    // A bad token or control turned off: retrying won't help,
                    // and each try would fill the app's activity log.
                    Err(f) if f.code != "unreachable" => return,
                    Err(_) => {}
                }
                // The app is closed or restarting: try again shortly.
                reconnecting = true;
                std::thread::sleep(Duration::from_secs(5));
            }
        });
    }
}

/// Reads newline-delimited JSON from `input` into a channel; malformed
/// lines get a parse error.
fn reader(input: impl BufRead + Send + 'static, out: Out) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(Value::Object(m)) => {
                    if tx.send(Value::Object(m)).is_err() {
                        break;
                    }
                }
                Ok(_) => send(&out, &rpc_error(Value::Null, -32600, "Invalid request")),
                Err(e) => send(
                    &out,
                    &rpc_error(Value::Null, -32700, format!("Parse error: {e}")),
                ),
            }
        }
    });
    rx
}

fn serve(options: Options, token: String, input: impl BufRead + Send + 'static, out: Out) {
    let rx = reader(input, out.clone());
    let mut session = Session {
        options,
        token,
        conn: None,
        tools: BTreeMap::new(),
        stale: Arc::new(AtomicBool::new(false)),
        elicit: false,
        out,
        input: rx,
        queued: VecDeque::new(),
        next: 0,
        watching: false,
    };
    while let Some(message) = session.next_message() {
        session.handle(message);
    }
}

pub(crate) fn cmd_mcp(o: &Options) -> Outcome {
    if !o.words.is_empty() || !o.flags.is_empty() {
        return Err(Failure::usage(
            "terrarium mcp [--space <id>] [--socket <path>]",
        ));
    }
    let token = saved_token()?;
    let out: Out = Arc::new(Mutex::new(Box::new(std::io::stdout())));
    serve(
        o.clone(),
        token,
        std::io::BufReader::new(std::io::stdin()),
        out,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use terrarium_control_proto::{read_message, write_message, Request, Response};

    fn describe() -> Value {
        json!([
            { "id": "app.describe", "kind": "query", "owner": "host", "effect": "read", "space": false, "summary": "Every command." },
            { "id": "space.list", "kind": "query", "owner": "host", "effect": "read", "space": false, "summary": "Spaces." },
            { "id": "todo.tasks", "kind": "query", "owner": "plugin", "effect": "read", "space": true,
              "description": "List tasks.",
              "params": { "filter": { "type": "string" }, "q": { "type": "string" } } },
            { "id": "todo.note_settings", "kind": "query", "owner": "plugin", "effect": "read", "space": true, "params": {} },
            { "id": "todo.note_settings", "kind": "action", "owner": "plugin", "effect": "write", "space": true,
              "params": { "ask": { "type": "enum", "values": ["always", "never"] } } },
            { "id": "todo.create", "kind": "action", "owner": "plugin", "effect": "write", "space": true,
              "description": "Create a task.",
              "params": {
                  "title": { "type": "string", "required": true },
                  "due": { "type": "date" },
                  "priority": { "type": "integer", "min": 0, "max": 3 }
              },
              "examples": [{ "title": "Call Sam" }] },
            { "id": "todo.delete", "kind": "action", "owner": "plugin", "effect": "destructive", "space": true,
              "confirm": "Move this task to the trash?",
              "params": { "id": { "type": "id", "required": true } } },
            { "id": "notes.loose", "kind": "action", "owner": "plugin", "effect": "write", "space": true },
            { "id": "page.create", "kind": "action", "owner": "host", "effect": "write", "space": true,
              "params": { "title": { "type": "string", "required": true } } }
        ])
    }

    #[test]
    fn names_follow_command_ids() {
        let tools = tools_from(&describe(), false);
        let names: Vec<&str> = tools.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "notes_loose",
                "page_create",
                "space_list",
                "todo_create",
                "todo_delete",
                "todo_note_settings",
                "todo_note_settings_query",
                "todo_tasks"
            ]
        );
        assert_eq!(tools["todo_note_settings"].kind, "action");
        assert_eq!(tools["todo_note_settings_query"].kind, "query");
    }

    #[test]
    fn schemas_follow_parameters() {
        let tools = tools_from(&describe(), false);
        let create = tool_json("todo_create", &tools["todo_create"]);
        let schema = &create["inputSchema"];
        assert_eq!(schema["required"], json!(["title"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["priority"]["maximum"], 3.0);
        assert_eq!(schema["properties"]["due"]["format"], "date");
        assert_eq!(schema["properties"]["dry_run"]["type"], "boolean");
        assert_eq!(schema["properties"]["space"]["type"], "string");
        assert!(schema["properties"].get("confirmed").is_none());
        assert_eq!(create["annotations"]["readOnlyHint"], false);
        assert_eq!(create["annotations"]["destructiveHint"], false);
        assert!(create["description"]
            .as_str()
            .unwrap()
            .contains(r#"Example arguments: {"title":"Call Sam"}"#));

        // Untyped commands take any object.
        let loose = tool_json("notes_loose", &tools["notes_loose"]);
        assert!(loose["inputSchema"].get("additionalProperties").is_none());
        // Host actions can't be dry-run; queries are read-only.
        assert!(tools["page_create"].takes_space && !tools["page_create"].takes_dry_run);
        let tasks = tool_json("todo_tasks", &tools["todo_tasks"]);
        assert_eq!(tasks["annotations"]["readOnlyHint"], true);
        assert!(tasks["inputSchema"]["properties"].get("dry_run").is_none());
        assert!(!tools["space_list"].takes_space);
    }

    #[test]
    fn destructive_tools_ask_for_confirmation() {
        let without = tools_from(&describe(), false);
        let delete = tool_json("todo_delete", &without["todo_delete"]);
        assert_eq!(delete["annotations"]["destructiveHint"], true);
        assert_eq!(
            delete["inputSchema"]["properties"]["confirmed"]["type"],
            "boolean"
        );
        let text = delete["description"].as_str().unwrap();
        assert!(text.contains("Move this task to the trash?") && text.contains("confirmed: true"));

        // With elicitation the server asks the user itself.
        let with = tools_from(&describe(), true);
        let delete = tool_json("todo_delete", &with["todo_delete"]);
        assert!(delete["inputSchema"]["properties"]
            .get("confirmed")
            .is_none());
    }

    // ---- A session against a fake app ----

    /// A fake app socket: answers `hello` and `describe`, records each
    /// `invoke` and answers it with `{result: true}`.
    fn fake_app(invokes: Arc<Mutex<Vec<Value>>>) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tmcp-{}-{:?}.sock",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let invokes = invokes.clone();
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    let mut reader = BufReader::new(stream);
                    while let Ok(Some(req)) = read_message::<Request>(&mut reader) {
                        let result = match req.method.as_str() {
                            "hello" => json!({ "client": "test", "scope": "change" }),
                            "describe" => describe(),
                            "invoke" => {
                                invokes.lock().unwrap().push(req.params.clone());
                                json!({ "result": true, "changes": [] })
                            }
                            _ => json!({}),
                        };
                        let _ = write_message(&mut writer, &Response::ok(req.id, result));
                    }
                });
            }
        });
        path
    }

    /// Runs a session over `lines` and returns what it wrote.
    fn session(lines: &[Value], invokes: Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
        let socket = fake_app(invokes);
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct Shared(Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out: Out = Arc::new(Mutex::new(Box::new(Shared(buffer.clone()))));
        let options = Options {
            socket: Some(socket.clone()),
            ..Options::default()
        };
        serve(
            options,
            "trm_test".into(),
            Cursor::new(input.into_bytes()),
            out,
        );
        let _ = std::fs::remove_file(socket);
        let text = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn by_id(out: &[Value], id: i64) -> Value {
        out.iter()
            .find(|m| m["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[test]
    fn clients_that_can_ask_confirm_with_their_user() {
        let invokes = Arc::new(Mutex::new(Vec::new()));
        let out = session(
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": { "protocolVersion": "2025-11-25",
                                    "capabilities": { "elicitation": {} } } }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                        "params": { "name": "todo_delete", "arguments": { "id": "t1" } } }),
                // A request that arrives while the server waits is kept.
                json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
                json!({ "jsonrpc": "2.0", "id": "terrarium-confirm-1",
                        "result": { "action": "accept", "content": { "confirm": true } } }),
                json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                        "params": { "name": "todo_delete", "arguments": { "id": "t2" } } }),
                json!({ "jsonrpc": "2.0", "id": "terrarium-confirm-2",
                        "result": { "action": "decline" } }),
            ],
            invokes.clone(),
        );
        let ask = out
            .iter()
            .find(|m| m["method"] == "elicitation/create")
            .unwrap();
        assert!(ask["params"]["message"]
            .as_str()
            .unwrap()
            .contains("Move this task to the trash?"));
        assert_eq!(by_id(&out, 2)["result"]["isError"], false);
        assert_eq!(by_id(&out, 3)["result"], json!({}));
        assert_eq!(
            by_id(&out, 4)["result"]["structuredContent"]["error"]["code"],
            "declined"
        );
        let invokes = invokes.lock().unwrap();
        assert_eq!(invokes.len(), 1, "only the accepted delete ran");
        assert_eq!(invokes[0]["args"], json!({ "id": "t1" }));
        assert_eq!(invokes[0]["confirmed"], true);
    }

    #[test]
    fn a_session_lists_and_calls_tools() {
        let invokes = Arc::new(Mutex::new(Vec::new()));
        let out = session(
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": { "protocolVersion": "2025-06-18", "capabilities": {} } }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
                json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                        "params": { "name": "todo_create",
                                    "arguments": { "title": "Call Sam", "dry_run": true, "space": "work" } } }),
                json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                        "params": { "name": "todo_delete", "arguments": { "id": "t1" } } }),
                json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                        "params": { "name": "todo_delete", "arguments": { "id": "t1", "confirmed": true } } }),
                json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                        "params": { "name": "nope" } }),
                json!({ "jsonrpc": "2.0", "id": 7, "method": "bogus" }),
                // A preview asked for in the wrong shape never runs for real.
                json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                        "params": { "name": "todo_create",
                                    "arguments": { "title": "x", "dry_run": "true" } } }),
                json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                        "params": { "name": "space_list" } }),
            ],
            invokes.clone(),
        );
        assert_eq!(
            by_id(&out, 8)["result"]["structuredContent"]["error"]["code"],
            "invalid"
        );
        assert_eq!(by_id(&out, 9)["result"]["isError"], false);
        assert_eq!(by_id(&out, 1)["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            by_id(&out, 1)["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        let tools = by_id(&out, 2)["result"]["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert!(tools.iter().all(|t| t["name"] != "app_describe"));
        assert_eq!(by_id(&out, 3)["result"]["isError"], false);
        assert_eq!(by_id(&out, 4)["result"]["isError"], true);
        assert_eq!(
            by_id(&out, 4)["result"]["structuredContent"]["error"]["code"],
            "needs-confirmation"
        );
        assert_eq!(by_id(&out, 5)["result"]["isError"], false);
        assert_eq!(by_id(&out, 6)["error"]["code"], -32602);
        assert_eq!(by_id(&out, 7)["error"]["code"], -32601);

        let invokes = invokes.lock().unwrap();
        assert_eq!(
            invokes.len(),
            3,
            "the unconfirmed delete and the bad dry run never reached the app"
        );
        assert_eq!(invokes[2]["id"], "space.list");
        assert_eq!(invokes[2]["space"], Value::Null);
        assert_eq!(invokes[0]["id"], "todo.create");
        assert_eq!(invokes[0]["space"], "work");
        assert_eq!(invokes[0]["dry_run"], true);
        assert_eq!(invokes[0]["args"], json!({ "title": "Call Sam" }));
        assert_eq!(invokes[1]["confirmed"], true);
        assert_eq!(invokes[1]["args"], json!({ "id": "t1" }));
    }

    #[test]
    fn closed_connections_are_noticed_before_sending() {
        let invokes = Arc::new(Mutex::new(Vec::new()));
        let socket = fake_app(invokes);
        let options = Options {
            socket: Some(socket.clone()),
            ..Options::default()
        };
        let (conn, _) = Conn::open(&options, "trm_test").unwrap();
        assert!(!conn.is_closed(), "an idle open connection is usable");
        // The app goes away: its end of the socket closes.
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(b);
        let dead = Conn {
            reader: BufReader::new(a.try_clone().unwrap()),
            writer: a,
            next: 0,
        };
        assert!(dead.is_closed());
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn unknown_versions_get_the_newest() {
        let out = session(
            &[json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                      "params": { "protocolVersion": "1999-01-01", "capabilities": {} } })],
            Arc::new(Mutex::new(Vec::new())),
        );
        assert_eq!(
            by_id(&out, 1)["result"]["protocolVersion"],
            PROTOCOL_VERSIONS[0]
        );
    }
}
