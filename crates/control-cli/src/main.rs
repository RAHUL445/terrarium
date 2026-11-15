//! `terrarium`: control the running Terrarium app from the command line.
//!
//! It talks to the app over the local control socket (see
//! `terrarium-control-proto`) and never opens the app's data itself.
//! Turn on Settings → Developer → Command-line control, add a client, then
//! run `terrarium login` with its token.

mod mcp;

use std::io::{BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{json, Map, Value};
use terrarium_control_proto::{
    read_message, socket_path, write_message, Request, Response, RpcError, TOKEN_PREFIX,
};

const USAGE: &str = "usage: terrarium <command> [options]

  login                         save a token from Settings → Developer (read from stdin)
  logout                        forget the saved token
  status                        the app, this client, spaces and plugins
  commands [prefix]             commands this client can run
  query <id> [--param value]... run a query, e.g. terrarium query tasks.tasks --filter upcoming
  run <id> [--param value]...   run an action, e.g. terrarium run tasks.create --title 'Buy milk'
  run <id> --ids a,b | --where <query> [--where-params json]   run a bulk action on many items (at most 500)
  search <text>                 search every plugin (--all-spaces, --plugin <id>, --limit <n>)
  export [--out file.zip] [--include-trash]   save this space's data as a ZIP (not encrypted)
  import <plugin>.<importer> <file|folder> [--dry-run]   add items from a CSV file or Markdown folder
  watch                         print app events as data changes
  mcp                           serve these commands as MCP tools on stdin/stdout, for agents
  serve                         run Terrarium without its window until the app opens

  page list | show <page> | widgets | create <title> | rename <page> <title> | remove <page>
  page place <page> <plugin/widget> | move <page> <placement> | unplace <page> <placement>
  space list | create <name> | rename <id> <name> | remove <id> | restore <id>
  open <plugin> [--page <id>] [--item <id>]   show it in the app window
  notify <text>                               show a message in the app window

options:
  --space <id>      run in this space (default: the default space)
  --args <json>     arguments as a JSON object, or @file.json
  --dry-run         run an action, show what it would change, keep nothing
  --yes             confirm a destructive action without asking
  --json            print JSON (the default when output is not a terminal)
  --socket <path>   the app's socket (default: in the app's data folder, or $TERRA_SOCKET)

The token comes from $TERRA_TOKEN, or from the file `terrarium login` saves.

exit codes: 0 ok, 1 failed, 2 invalid, 3 not found, 4 in trash,
            5 not allowed, 6 the app is not reachable, 7 needs confirmation";

/// Exit codes for error codes.
fn exit_code(code: &str) -> u8 {
    match code {
        "invalid" | "usage" => 2,
        "not-found" | "unknown-space" => 3,
        "in-trash" => 4,
        "forbidden" | "unauthorized" => 5,
        "unreachable" => 6,
        "needs-confirmation" => 7,
        _ => 1,
    }
}

#[derive(Debug)]
struct Failure {
    code: String,
    message: String,
}

impl Failure {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Failure {
            code: code.into(),
            message: message.into(),
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Failure::new("usage", message)
    }
}

impl From<RpcError> for Failure {
    fn from(e: RpcError) -> Self {
        let code = e.kind().to_string();
        Failure::new(&code, e.message)
    }
}

type Outcome = Result<(), Failure>;

// ---- Options ------------------------------------------------------------------

#[derive(Clone, Default)]
struct Options {
    json: bool,
    dry_run: bool,
    yes: bool,
    socket: Option<PathBuf>,
    space: Option<String>,
    args: Option<String>,
    ids: Option<String>,
    where_query: Option<String>,
    where_params: Option<String>,
    /// Positional words after the command.
    words: Vec<String>,
    /// `--name value` pairs not recognised above, in order.
    flags: Vec<(String, Option<String>)>,
}

fn parse(args: &[String]) -> Result<Options, Failure> {
    let mut o = Options::default();
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        let Some(name) = arg.strip_prefix("--") else {
            o.words.push(arg.clone());
            continue;
        };
        let (name, inline) = match name.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (name.to_string(), None),
        };
        let mut value = |what: &str| -> Result<String, Failure> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| Failure::usage(format!("--{what} needs a value")))
        };
        match name.as_str() {
            "json" => o.json = true,
            "dry-run" => o.dry_run = true,
            "yes" => o.yes = true,
            "socket" => o.socket = Some(PathBuf::from(value("socket")?)),
            "space" => o.space = Some(value("space")?),
            "args" => o.args = Some(value("args")?),
            "ids" => o.ids = Some(value("ids")?),
            "where" => o.where_query = Some(value("where")?),
            "where-params" => o.where_params = Some(value("where-params")?),
            _ => {
                // A flag's value is the next word unless that is another flag.
                let v = inline.or_else(|| it.next_if(|next| !next.starts_with("--")).cloned());
                o.flags.push((name, v));
            }
        }
    }
    Ok(o)
}

fn parse_ids(text: &str) -> Result<Vec<String>, Failure> {
    let text = match text.strip_prefix('@') {
        Some(file) => {
            std::fs::read_to_string(file).map_err(|e| Failure::usage(format!("{file}: {e}")))?
        }
        None => text.to_string(),
    };
    let ids: Vec<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Err(Failure::usage("--ids needs at least one id"));
    }
    Ok(ids)
}

fn parse_json_object(text: &str, flag: &str) -> Result<Map<String, Value>, Failure> {
    let text = match text.strip_prefix('@') {
        Some(file) => {
            std::fs::read_to_string(file).map_err(|e| Failure::usage(format!("{file}: {e}")))?
        }
        None => text.to_string(),
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(Failure::usage(format!("{flag} must be a JSON object"))),
    }
}

fn bulk_request(o: &Options, info: &Value, mut args: Value) -> Result<Value, Failure> {
    if o.ids.is_some() && o.where_query.is_some() {
        return Err(Failure::usage("use --ids or --where, not both"));
    }
    if info["bulk"] != true {
        let id = info["id"].as_str().unwrap_or_default();
        return Err(Failure::new(
            "invalid",
            format!("`{id}` can't run on many items"),
        ));
    }
    if let Some(map) = args.as_object_mut() {
        map.remove("id");
    }
    let id = info["id"]
        .as_str()
        .ok_or_else(|| Failure::new("failed", "The command has no id."))?;
    let (plugin, op) = id
        .split_once('.')
        .ok_or_else(|| Failure::new("failed", format!("`{id}` is not a plugin action id")))?;
    let mut request = Map::new();
    request.insert("plugin".into(), json!(plugin));
    request.insert("op".into(), json!(op));
    request.insert("args".into(), args);
    if let Some(text) = &o.ids {
        request.insert("ids".into(), json!(parse_ids(text)?));
    } else if let Some(query) = &o.where_query {
        let params = match &o.where_params {
            Some(text) => parse_json_object(text, "--where-params")?,
            None => Map::new(),
        };
        request.insert("where".into(), json!({ "query": query, "params": params }));
    } else {
        return Err(Failure::usage("use --ids or --where"));
    }
    Ok(Value::Object(request))
}

fn human(o: &Options) -> bool {
    !o.json && std::io::stdout().is_terminal()
}

// ---- Token --------------------------------------------------------------------

fn token_file() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("terrarium").join("token"))
}

fn saved_token() -> Result<String, Failure> {
    if let Some(t) = std::env::var("TERRA_TOKEN").ok().filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    let path = token_file().ok_or_else(|| Failure::new("unauthorized", "No home folder."))?;
    std::fs::read_to_string(&path)
        .map(|t| t.trim().to_string())
        .map_err(|_| {
            Failure::new(
                "unauthorized",
                "Not logged in. Add a client in Settings → Developer, then run `terrarium login`.",
            )
        })
}

fn save_token(token: &str) -> Result<PathBuf, Failure> {
    let path = token_file().ok_or_else(|| Failure::new("failed", "No home folder."))?;
    let dir = path.parent().expect("has a parent");
    let fail = |e: std::io::Error| Failure::new("failed", format!("{}: {e}", path.display()));
    std::fs::create_dir_all(dir).map_err(fail)?;
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(fail)?;
        let tmp = dir.join(".token.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(fail)?;
        file.write_all(token.as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        std::fs::rename(&tmp, &path).map_err(fail)?;
    }
    Ok(path)
}

// ---- Connection ---------------------------------------------------------------

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Conn {
    fn open(o: &Options, token: &str) -> Result<(Conn, Value), Failure> {
        let path =
            o.socket.clone().or_else(socket_path).ok_or_else(|| {
                Failure::new("unreachable", "Can't tell where the app's socket is.")
            })?;
        let stream = UnixStream::connect(&path).map_err(|e| {
            Failure::new(
                "unreachable",
                format!(
                    "Can't reach Terrarium at {} ({e}). Is the app open (or `terrarium serve` running), with Settings → Developer → Command-line control on?",
                    path.display()
                ),
            )
        })?;
        let writer = stream
            .try_clone()
            .map_err(|e| Failure::new("unreachable", e.to_string()))?;
        let mut conn = Conn {
            reader: BufReader::new(stream),
            writer,
            next: 0,
        };
        let hello = conn.call(
            "hello",
            json!({ "token": token, "client": "terrarium-cli" }),
        )?;
        Ok((conn, hello))
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        self.next += 1;
        let id = self.next;
        write_message(&mut self.writer, &Request::new(id, method, params))
            .map_err(|e| Failure::new("unreachable", e.to_string()))?;
        loop {
            let response = self.read()?;
            if response.id == Some(id) || (response.id.is_none() && response.error.is_some()) {
                return match (response.result, response.error) {
                    (_, Some(e)) => Err(e.into()),
                    (Some(v), None) => Ok(v),
                    (None, None) => Ok(Value::Null),
                };
            }
        }
    }

    /// Whether this connection can't be used for the next request: the app
    /// closed it (it quit or restarted). Found without sending anything, by
    /// a non-blocking read; the app sends nothing unasked on a connection
    /// that hasn't subscribed, so any byte waiting there also means the
    /// connection is out of step and is reported as closed.
    fn is_closed(&self) -> bool {
        let mut socket = self.reader.get_ref();
        if !self.reader.buffer().is_empty() || socket.set_nonblocking(true).is_err() {
            return true;
        }
        let closed = match socket.read(&mut [0u8; 1]) {
            Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
            Ok(_) => true,
        };
        closed || socket.set_nonblocking(false).is_err()
    }

    fn read(&mut self) -> Result<Response, Failure> {
        match read_message::<Response>(&mut self.reader) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(Failure::new(
                "unreachable",
                "The app closed the connection.",
            )),
            Err(e) => Err(Failure::new("failed", e.to_string())),
        }
    }

    /// Runs a command; the reply is `{result, changes?, dry_run?}`.
    fn invoke(
        &mut self,
        o: &Options,
        kind: &str,
        id: &str,
        args: Value,
        confirmed: bool,
    ) -> Result<Value, Failure> {
        self.call("invoke", invoke_params(o, kind, id, &args, confirmed))
    }
}

fn invoke_params(o: &Options, kind: &str, id: &str, args: &Value, confirmed: bool) -> Value {
    json!({
        "kind": kind,
        "id": id,
        "space": o.space,
        "args": args,
        "dry_run": o.dry_run && kind == "action",
        "confirmed": confirmed,
    })
}

fn connect(o: &Options) -> Result<(Conn, Value), Failure> {
    Conn::open(o, &saved_token()?)
}

// ---- Output -------------------------------------------------------------------

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn cell(v: &Value) -> String {
    let text = match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let text = text.replace(['\n', '\t'], " ");
    if text.chars().count() > 40 {
        format!("{}…", text.chars().take(39).collect::<String>())
    } else {
        text
    }
}

/// Rows of objects as an aligned table; anything else as JSON.
fn print_value(v: &Value) {
    let Some(rows) = v
        .as_array()
        .filter(|r| !r.is_empty() && r.iter().all(Value::is_object))
    else {
        if v.as_array().is_some_and(Vec::is_empty) {
            println!("(nothing)");
        } else {
            print_json(v);
        }
        return;
    };
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        for key in row.as_object().expect("objects").keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| columns.iter().map(|c| cell(&r[c])).collect())
        .collect();
    print_table(&columns, &table);
}

fn print_table(headers: &[String], rows: &[Vec<String>]) {
    let widths: Vec<usize> = (0..headers.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([headers[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| {
        let parts: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        println!("{}", parts.join("  ").trim_end());
    };
    line(&headers.iter().map(|h| h.to_uppercase()).collect::<Vec<_>>());
    for r in rows {
        line(r);
    }
}

// ---- Arguments ----------------------------------------------------------------

/// Builds a command's arguments from `--args` and `--name value` flags,
/// typed by the command's description.
fn build_args(o: &Options, info: &Value) -> Result<Value, Failure> {
    let mut args: Map<String, Value> = match &o.args {
        None => Map::new(),
        Some(text) => {
            let text = match text.strip_prefix('@') {
                Some(file) => std::fs::read_to_string(file)
                    .map_err(|e| Failure::usage(format!("{file}: {e}")))?,
                None => text.clone(),
            };
            match serde_json::from_str(&text) {
                Ok(Value::Object(m)) => m,
                _ => return Err(Failure::usage("--args must be a JSON object")),
            }
        }
    };
    let params = info["params"].as_object();
    let context: Vec<&str> = info["context"]
        .as_array()
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (name, raw) in &o.flags {
        let name = name.replace('-', "_");
        let ty = match params.and_then(|p| p.get(&name)) {
            Some(spec) => spec["type"].as_str().unwrap_or("json"),
            None if context.contains(&name.as_str()) => "string",
            // Untyped commands take any argument.
            None if params.is_none() => "auto",
            None => {
                let known: Vec<&String> = params.map(|p| p.keys().collect()).unwrap_or_default();
                return Err(Failure::new(
                    "invalid",
                    format!(
                        "`{}` takes no --{name}. It takes: {}",
                        info["id"].as_str().unwrap_or(""),
                        if known.is_empty() {
                            "nothing".to_string()
                        } else {
                            known
                                .iter()
                                .map(|k| format!("--{k}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                ));
            }
        };
        let value = typed(&name, ty, raw.as_deref())?;
        args.insert(name, value);
    }
    Ok(Value::Object(args))
}

fn typed(name: &str, ty: &str, raw: Option<&str>) -> Result<Value, Failure> {
    let bad = |what: &str| Failure::new("invalid", format!("--{name} must be {what}"));
    match (ty, raw) {
        ("boolean", None) => Ok(json!(true)),
        (_, None) => Err(Failure::usage(format!("--{name} needs a value"))),
        ("boolean", Some(v)) => match v {
            "true" | "yes" | "1" => Ok(json!(true)),
            "false" | "no" | "0" => Ok(json!(false)),
            _ => Err(bad("true or false")),
        },
        ("integer", Some(v)) => v
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| bad("a whole number")),
        ("number", Some(v)) => v
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number))
            .ok_or_else(|| bad("a number")),
        ("json", Some(v)) => Ok(serde_json::from_str(v).unwrap_or_else(|_| json!(v))),
        ("auto", Some(v)) => Ok(match serde_json::from_str::<Value>(v) {
            Ok(n @ (Value::Number(_) | Value::Bool(_) | Value::Null)) => n,
            _ => json!(v),
        }),
        (_, Some(v)) => Ok(json!(v)),
    }
}

fn find_command<'a>(all: &'a Value, kind: &str, id: &str) -> Option<&'a Value> {
    all.as_array()?
        .iter()
        .find(|c| c["kind"] == kind && c["id"] == id)
}

// ---- Commands -----------------------------------------------------------------

fn cmd_login(o: &Options) -> Outcome {
    let mut token = String::new();
    if std::io::stdin().is_terminal() {
        eprint!("Paste the token from Settings → Developer: ");
        let _ = std::io::stderr().flush();
        std::io::stdin()
            .read_line(&mut token)
            .map_err(|e| Failure::new("failed", e.to_string()))?;
    } else {
        std::io::stdin()
            .read_to_string(&mut token)
            .map_err(|e| Failure::new("failed", e.to_string()))?;
    }
    let token = token.trim();
    if !token.starts_with(TOKEN_PREFIX) {
        return Err(Failure::new(
            "invalid",
            format!("That doesn't look like a Terrarium token (they start with {TOKEN_PREFIX})."),
        ));
    }
    let (_, hello) = Conn::open(o, token)?;
    let path = save_token(token)?;
    if human(o) {
        println!(
            "Logged in as “{}” ({}). Token saved to {}.",
            hello["client"].as_str().unwrap_or(""),
            hello["scope"].as_str().unwrap_or(""),
            path.display()
        );
    } else {
        print_json(
            &json!({ "client": hello["client"], "scope": hello["scope"], "token_file": path }),
        );
    }
    Ok(())
}

fn cmd_logout(o: &Options) -> Outcome {
    let path = token_file().ok_or_else(|| Failure::new("failed", "No home folder."))?;
    let removed = std::fs::remove_file(&path).is_ok();
    if human(o) {
        println!(
            "{}",
            if removed {
                "Logged out."
            } else {
                "Not logged in."
            }
        );
    } else {
        print_json(&json!({ "removed": removed }));
    }
    Ok(())
}

fn cmd_status(o: &Options) -> Outcome {
    let (mut conn, _) = connect(o)?;
    let status = conn.call("status", json!({}))?;
    if !human(o) {
        print_json(&status);
        return Ok(());
    }
    println!(
        "Terrarium {}",
        status["app_version"].as_str().unwrap_or("?")
    );
    println!(
        "Client     {} ({})",
        status["client"].as_str().unwrap_or(""),
        status["scope"].as_str().unwrap_or("")
    );
    let spaces: Vec<String> = status["spaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["removed"] != true)
        .map(|s| format!("{} ({})", cell(&s["name"]), cell(&s["id"])))
        .collect();
    println!("Spaces     {}", spaces.join(", "));
    let plugins: Vec<String> = status["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| format!("{} {}", cell(&p["id"]), cell(&p["version"])))
        .collect();
    println!("Plugins    {}", plugins.join(", "));
    if status["window"] == false {
        println!("Window     none: `terrarium serve` is answering until the app opens");
    }
    Ok(())
}

/// The app binary that runs headless: `$TERRA_APP_BIN`, else
/// `terrarium-app` next to this command (as in `Terrarium.app/Contents/MacOS`
/// and in `target/<profile>`).
fn app_binary() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("TERRA_APP_BIN") {
        return Some(path.into());
    }
    // Follow the `~/.local/bin/terrarium` link back into the app.
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.parent()?.join("terrarium-app"))
}

/// Replaces this process with the app binary in headless mode.
fn cmd_serve(o: &Options) -> Outcome {
    if !o.words.is_empty() || o.socket.is_some() || o.space.is_some() {
        return Err(Failure::usage(
            "usage: terrarium serve\n\nIt uses the app's data folder and socket.",
        ));
    }
    let app = app_binary().filter(|p| p.is_file()).ok_or_else(|| {
        Failure::new(
            "failed",
            "Can't find the Terrarium app next to this command. Set TERRA_APP_BIN to its \
                 binary (Terrarium.app/Contents/MacOS/terrarium-app).",
        )
    })?;
    use std::os::unix::process::CommandExt;
    let e = std::process::Command::new(&app).arg("--headless").exec();
    Err(Failure::new(
        "failed",
        format!("Can't start {}: {e}", app.display()),
    ))
}

fn cmd_commands(o: &Options) -> Outcome {
    let (mut conn, _) = connect(o)?;
    let all = conn.call("describe", json!({}))?;
    let prefix = o.words.first().map(String::as_str).unwrap_or("");
    let list: Vec<&Value> = all
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["id"].as_str().is_some_and(|id| id.starts_with(prefix)))
        .collect();
    if !human(o) {
        print_json(&json!(list));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|c| {
            vec![
                cell(&c["kind"]),
                cell(&c["id"]),
                cell(&c["effect"]),
                if c["params"].is_null() {
                    "(untyped)".into()
                } else {
                    c["params"]
                        .as_object()
                        .map(|p| {
                            p.iter()
                                .map(|(k, s)| {
                                    if s["required"] == true {
                                        format!("{k}*")
                                    } else {
                                        k.clone()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default()
                },
                cell(&c["summary"]),
            ]
        })
        .collect();
    print_table(
        &["kind", "id", "effect", "params", "summary"].map(String::from),
        &rows,
    );
    Ok(())
}

fn cmd_query(o: &Options) -> Outcome {
    let id = o
        .words
        .first()
        .ok_or_else(|| Failure::usage("terrarium query <id> [--param value]..."))?;
    let (mut conn, _) = connect(o)?;
    let all = conn.call("describe", json!({}))?;
    let info = find_command(&all, "query", id).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!("no query `{id}`; see `terrarium commands`"),
        )
    })?;
    let args = build_args(o, info)?;
    let outcome = conn.invoke(o, "query", id, args, false)?;
    if human(o) {
        print_value(&outcome["result"]);
    } else {
        print_json(&outcome["result"]);
    }
    Ok(())
}

/// The error for an action `describe` didn't list: a read-only client
/// can't see actions at all.
fn missing_action(hello: &Value, id: &str) -> Failure {
    if hello["scope"] == "read" {
        Failure::new(
            "forbidden",
            format!("`{id}` changes data, and this client may only read. Add a client with \"Read and change\" in Settings → Developer."),
        )
    } else {
        Failure::new(
            "not-found",
            format!("no action `{id}`; see `terrarium commands`"),
        )
    }
}

fn cmd_run(o: &Options) -> Outcome {
    let id = o
        .words
        .first()
        .ok_or_else(|| Failure::usage("terrarium run <id> [--param value]..."))?
        .clone();
    let (mut conn, hello) = connect(o)?;
    let all = conn.call("describe", json!({}))?;
    let info = find_command(&all, "action", &id).ok_or_else(|| missing_action(&hello, &id))?;
    let args = build_args(o, info)?;
    if o.ids.is_some() || o.where_query.is_some() {
        let request = bulk_request(o, info, args)?;
        let count = request["ids"].as_array().map(Vec::len);
        let confirmed = confirm(o, info, count)?;
        let outcome = conn.invoke(o, "action", "plugin.run_bulk", request, confirmed)?;
        return print_bulk_outcome(o, &outcome);
    }
    act(o, &mut conn, info, args)
}

/// Asks before a destructive action unless `--yes` or `--dry-run`.
fn confirm(o: &Options, info: &Value, count: Option<usize>) -> Result<bool, Failure> {
    if info["effect"] != "destructive" || o.dry_run {
        return Ok(false);
    }
    if o.yes {
        return Ok(true);
    }
    let question = info["confirm"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("`{}` removes or hides data.", cell(&info["id"])));
    let question = match count {
        Some(n) => format!("{question} ({n} items)"),
        None => question,
    };
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(Failure::new(
            "needs-confirmation",
            format!("{question} Run again with --yes to confirm, or --dry-run to see what it would change."),
        ));
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| Failure::new("failed", e.to_string()))?;
    if matches!(answer.trim(), "y" | "Y" | "yes") {
        Ok(true)
    } else {
        Err(Failure::new(
            "needs-confirmation",
            "Not confirmed; nothing was changed.",
        ))
    }
}

/// Runs an action and prints its result and changes.
fn act(o: &Options, conn: &mut Conn, info: &Value, args: Value) -> Outcome {
    let id = info["id"].as_str().unwrap_or_default().to_string();
    let confirmed = confirm(o, info, None)?;
    let outcome = conn.invoke(o, "action", &id, args, confirmed)?;
    if !human(o) {
        print_json(&outcome);
        return Ok(());
    }
    let changes = outcome["changes"].as_array().cloned().unwrap_or_default();
    if outcome["dry_run"] == true {
        println!("Dry run: nothing was kept.");
    }
    if !changes.is_empty() {
        let rows: Vec<Vec<String>> = changes
            .iter()
            .map(|c| {
                vec![
                    cell(&c["op"]),
                    cell(&c["plugin"]),
                    cell(&c["table"]),
                    cell(&c["row"]),
                    cell(&c["column"]),
                ]
            })
            .collect();
        print_table(
            &["op", "plugin", "table", "row", "column"].map(String::from),
            &rows,
        );
    }
    match &outcome["result"] {
        Value::Null => {}
        Value::Bool(true) if changes.is_empty() => println!("Done."),
        result => print_value(result),
    }
    Ok(())
}

fn print_bulk_outcome(o: &Options, outcome: &Value) -> Outcome {
    if !human(o) {
        print_json(outcome);
        return Ok(());
    }
    if outcome["dry_run"] == true {
        println!("Dry run: nothing was kept.");
    }
    let failed_rows: Vec<Vec<String>> = outcome["result"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["ok"] != true)
        .map(|item| {
            vec![
                cell(&item["id"]),
                cell(&item["error"]["code"]),
                cell(&item["error"]["message"]),
            ]
        })
        .collect();
    if !failed_rows.is_empty() {
        print_table(&["id", "code", "message"].map(String::from), &failed_rows);
    }
    let message = outcome["result"]["message"]
        .as_str()
        .unwrap_or("Bulk command finished.");
    println!("{message}");
    if outcome["result"]["failed"].as_u64().unwrap_or(0) > 0 {
        return Err(Failure::new("failed", message));
    }
    Ok(())
}

/// A host command with positional arguments, e.g. `page create <title>`.
fn shortcut(o: &Options, parent: &str, kind: &str, id: &str, names: &[&str]) -> Outcome {
    let words = &o.words[1..];
    let usage = || {
        let parts: Vec<String> = names.iter().map(|n| format!("<{n}>")).collect();
        let command = [parent, o.words[0].as_str()].join(" ");
        Failure::usage(format!("terrarium {} {}", command.trim(), parts.join(" ")))
    };
    if words.len() > names.len() && id != "ui.notify" {
        return Err(usage());
    }
    let (mut conn, hello) = connect(o)?;
    let all = conn.call("describe", json!({}))?;
    let info = find_command(&all, kind, id).ok_or_else(|| missing_action(&hello, id))?;
    let mut args = build_args(o, info)?;
    if id == "ui.notify" {
        args["text"] = json!(words.join(" "));
    } else {
        for (name, word) in names.iter().zip(words) {
            let ty = info["params"][*name]["type"].as_str().unwrap_or("string");
            args[*name] = typed(name, ty, Some(word))?;
        }
    }
    if let Some(missing) = names
        .iter()
        .skip(words.len())
        .find(|n| info["params"][**n]["required"] == true && args.get(**n).is_none())
    {
        return Err(Failure::usage(format!(
            "{} (missing <{missing}>)",
            usage().message
        )));
    }
    if kind == "query" {
        let outcome = conn.invoke(o, "query", id, args, false)?;
        if human(o) {
            print_value(&outcome["result"]);
        } else {
            print_json(&outcome["result"]);
        }
        return Ok(());
    }
    act(o, &mut conn, info, args)
}

fn cmd_page(o: &Options) -> Outcome {
    let sub = o.words.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" => shortcut(o, "page", "query", "page.list", &[]),
        "show" => shortcut(o, "page", "query", "page.get", &["page"]),
        "widgets" => shortcut(o, "page", "query", "page.widgets", &[]),
        "create" => shortcut(o, "page", "action", "page.create", &["title"]),
        "rename" => shortcut(o, "page", "action", "page.update", &["page", "title"]),
        "remove" => shortcut(o, "page", "action", "page.remove", &["page"]),
        "place" => shortcut(o, "page", "action", "page.place", &["page", "widget"]),
        "move" => shortcut(o, "page", "action", "page.move", &["page", "placement"]),
        "unplace" => shortcut(o, "page", "action", "page.unplace", &["page", "placement"]),
        _ => Err(Failure::usage(
            "terrarium page list | show | widgets | create | rename | remove | place | move | unplace",
        )),
    }
}

fn cmd_space(o: &Options) -> Outcome {
    let sub = o.words.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" => shortcut(o, "space", "query", "space.list", &[]),
        "create" => shortcut(o, "space", "action", "space.create", &["name"]),
        "rename" => shortcut(o, "space", "action", "space.rename", &["id", "name"]),
        "remove" => shortcut(o, "space", "action", "space.remove", &["id"]),
        "restore" => shortcut(o, "space", "action", "space.restore", &["id"]),
        _ => Err(Failure::usage(
            "terrarium space list | create | rename | remove | restore",
        )),
    }
}

/// `space.export` arguments: `--out` (default `terrarium-export.zip`)
/// made absolute against `cwd`, and `--include-trash`.
fn export_args(o: &Options, cwd: &Path) -> Result<Value, Failure> {
    const USAGE: &str = "terrarium export [--out file.zip] [--include-trash]";
    if !o.words.is_empty() || o.args.is_some() {
        return Err(Failure::usage(USAGE));
    }
    let mut path = PathBuf::from("terrarium-export.zip");
    let mut include_trash = false;
    for (name, value) in &o.flags {
        match (name.as_str(), value) {
            ("out", Some(value)) if !value.is_empty() => path = PathBuf::from(value),
            ("include-trash", None) => include_trash = true,
            _ => return Err(Failure::usage(USAGE)),
        }
    }
    if !path.is_absolute() {
        path = cwd.join(path);
    }
    Ok(json!({"path": path, "include_trash": include_trash}))
}

fn cmd_export(o: &Options) -> Outcome {
    let cwd = std::env::current_dir().map_err(|e| Failure::new("failed", e.to_string()))?;
    let args = export_args(o, &cwd)?;
    let (mut conn, _) = connect(o)?;
    let outcome = conn.invoke(o, "action", "space.export", args, false)?;
    let result = &outcome["result"];
    if human(o) {
        println!(
            "Saved {} ({} items from {} apps). It is not encrypted; keep it somewhere safe.",
            cell(&result["path"]),
            cell(&result["rows"]),
            cell(&result["plugins"])
        );
    } else {
        print_json(result);
    }
    Ok(())
}

const IMPORT_LIMIT: usize = 7 * 1024 * 1024 / 2;
const IMPORT_TOO_LARGE: &str =
    "Too much to import at once from the command line (3.5 MB at most). Split the folder or file.";

fn import_files(format: &str, root: &Path) -> Result<Vec<Value>, Failure> {
    fn io_error(e: std::io::Error) -> Failure {
        Failure::new("invalid", e.to_string())
    }
    fn walk(root: &Path, dir: &Path, paths: &mut Vec<(String, PathBuf)>) -> Outcome {
        for entry in std::fs::read_dir(dir).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            // Do not follow symlinks outside the chosen folder or into cycles.
            let ty = entry.file_type().map_err(io_error)?;
            let path = entry.path();
            if ty.is_dir() {
                walk(root, &path, paths)?;
            } else if ty.is_file()
                && path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                })
            {
                let relative = path.strip_prefix(root).expect("walked beneath root");
                let name = relative
                    .to_str()
                    .ok_or_else(|| Failure::new("invalid", "File paths must be valid UTF-8."))?;
                paths.push((name.to_string(), path));
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    match format {
        "csv" if root.is_file() => {
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Failure::new("invalid", "File paths must be valid UTF-8."))?;
            paths.push((name.to_string(), root.to_path_buf()));
        }
        "markdown" if root.is_dir() => walk(root, root, &mut paths)?,
        "csv" => return Err(Failure::new("invalid", "Choose a CSV file to import.")),
        "markdown" => {
            return Err(Failure::new(
                "invalid",
                "Choose a Markdown folder to import.",
            ))
        }
        _ => return Err(Failure::new("invalid", "Unsupported import format.")),
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let mut bytes = 0;
    let mut files = Vec::new();
    for (path, source) in paths {
        let mut data = Vec::new();
        std::fs::File::open(source)
            .map_err(io_error)?
            .take((IMPORT_LIMIT - bytes + 1) as u64)
            .read_to_end(&mut data)
            .map_err(io_error)?;
        bytes += data.len() + path.len();
        if bytes > IMPORT_LIMIT {
            return Err(Failure::new("invalid", IMPORT_TOO_LARGE));
        }
        let text = String::from_utf8(data)
            .map_err(|_| Failure::new("invalid", "Import files must be valid UTF-8."))?;
        files.push(json!({"path": path, "text": text}));
    }
    Ok(files)
}

fn check_import_request(o: &Options, args: &Value, id: u64) -> Outcome {
    let request = Request::new(
        id,
        "invoke",
        invoke_params(o, "action", "space.import", args, false),
    );
    let bytes = serde_json::to_vec(&request).map_err(|e| Failure::new("invalid", e.to_string()))?;
    if bytes.len() > IMPORT_LIMIT {
        return Err(Failure::new("invalid", IMPORT_TOO_LARGE));
    }
    Ok(())
}

fn import_result(result: &Value, human_output: bool) -> Outcome {
    if human_output {
        let failures = result["failures"].as_array().cloned().unwrap_or_default();
        if !failures.is_empty() {
            let rows = failures
                .iter()
                .map(|f| vec![cell(&f["item"]), cell(&f["code"]), cell(&f["message"])])
                .collect::<Vec<_>>();
            print_table(&["ITEM", "CODE", "MESSAGE"].map(String::from), &rows);
        }
        println!("{}", result["message"].as_str().unwrap_or_default());
    } else {
        print_json(result);
    }
    if result["failed"].as_u64().unwrap_or(0) > 0 {
        return Err(Failure::new(
            "failed",
            result["message"].as_str().unwrap_or("Import failed."),
        ));
    }
    Ok(())
}

fn cmd_import(o: &Options) -> Outcome {
    if o.words.len() != 2 || !o.flags.is_empty() || o.args.is_some() {
        return Err(Failure::usage(
            "terrarium import <plugin>.<importer> <file|folder> [--dry-run]",
        ));
    }
    let name = &o.words[0];
    let missing = || {
        Failure::new(
            "not-found",
            format!("no importer `{name}`; see `terrarium status`"),
        )
    };
    let (plugin, importer) = name.split_once('.').ok_or_else(missing)?;
    let (mut conn, _) = connect(o)?;
    let listing = conn.invoke(o, "query", "plugin.list", json!({}), false)?;
    let format = listing["result"]
        .as_array()
        .and_then(|plugins| {
            plugins.iter().find(|p| {
                p["id"] == plugin
                    || p["former_ids"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == plugin))
            })
        })
        .and_then(|p| p["imports"].as_array())
        .and_then(|imports| imports.iter().find(|i| i["id"] == importer))
        .and_then(|i| i["format"].as_str())
        .ok_or_else(missing)?;
    let files = import_files(format, Path::new(&o.words[1]))?;
    let args = json!({"plugin": plugin, "importer": importer, "files": files});
    check_import_request(o, &args, conn.next + 1)?;
    let outcome = conn.invoke(o, "action", "space.import", args, false)?;
    import_result(&outcome["result"], human(o))
}

fn cmd_search(o: &Options) -> Outcome {
    let q = o.words.join(" ");
    if q.trim().is_empty() {
        return Err(Failure::usage("terrarium search <text>"));
    }
    let mut args = json!({ "q": q });
    let mut plugins = Vec::new();
    for (name, value) in &o.flags {
        match (name.as_str(), value) {
            ("all-spaces", _) => args["all_spaces"] = json!(true),
            ("plugin", Some(p)) => plugins.push(json!(p)),
            ("limit", Some(n)) => args["limit"] = typed("limit", "integer", Some(n))?,
            _ => return Err(Failure::usage(format!("search takes no --{name}"))),
        }
    }
    if !plugins.is_empty() {
        args["plugins"] = json!(plugins);
    }
    let (mut conn, _) = connect(o)?;
    let found = conn.invoke(o, "query", "search.query", args, false)?["result"].take();
    if !human(o) {
        print_json(&found);
        return Ok(());
    }
    let results = found["results"].as_array().cloned().unwrap_or_default();
    if results.is_empty() {
        println!("Nothing found.");
    }
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|r| {
            vec![
                cell(&r["plugin"]),
                cell(&r["kind"]),
                cell(&r["title"]),
                cell(&r["subtitle"]),
                cell(&r["space"]),
                cell(&r["id"]),
            ]
        })
        .collect();
    if !rows.is_empty() {
        print_table(
            &["plugin", "kind", "title", "detail", "space", "id"].map(String::from),
            &rows,
        );
    }
    for e in found["errors"].as_array().into_iter().flatten() {
        eprintln!(
            "{} in {}: {}",
            cell(&e["plugin"]),
            cell(&e["space"]),
            cell(&e["message"])
        );
    }
    Ok(())
}

fn cmd_watch(o: &Options) -> Outcome {
    let (mut conn, _) = connect(o)?;
    conn.call("subscribe", json!({}))?;
    if human(o) {
        eprintln!("Watching for changes. Press Ctrl-C to stop.");
    }
    loop {
        let message = conn.read()?;
        if message.method.as_deref() != Some("event") {
            continue;
        }
        let name = message
            .params
            .as_ref()
            .map_or(Value::Null, |p| p["name"].clone());
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if human(o) {
            let secs = (at / 1000) % 86_400;
            println!(
                "{:02}:{:02}:{:02} UTC  {}",
                secs / 3600,
                secs / 60 % 60,
                secs % 60,
                cell(&name)
            );
        } else {
            println!("{}", json!({ "event": name, "at_ms": at }));
        }
        let _ = std::io::stdout().flush();
    }
}

fn run(args: &[String]) -> Outcome {
    let Some(command) = args.first() else {
        return Err(Failure::usage(USAGE));
    };
    let o = parse(&args[1..])?;
    match command.as_str() {
        "login" => cmd_login(&o),
        "logout" => cmd_logout(&o),
        "status" => cmd_status(&o),
        "commands" => cmd_commands(&o),
        "query" => cmd_query(&o),
        "run" => cmd_run(&o),
        "page" => cmd_page(&o),
        "space" => cmd_space(&o),
        "open" | "notify" => {
            // `open tasks` is `ui.open` with the plugin first.
            let mut o = o;
            o.words.insert(0, command.clone());
            if command == "open" {
                shortcut(&o, "", "action", "ui.open", &["plugin"])
            } else {
                shortcut(&o, "", "action", "ui.notify", &["text"])
            }
        }
        "search" => cmd_search(&o),
        "export" => cmd_export(&o),
        "import" => cmd_import(&o),
        "watch" => cmd_watch(&o),
        "mcp" => mcp::cmd_mcp(&o),
        "serve" => cmd_serve(&o),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(Failure::usage(format!(
            "unknown command `{other}`\n\n{USAGE}"
        ))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json_errors = args.iter().any(|a| a == "--json") || !std::io::stderr().is_terminal();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            if json_errors && f.code != "usage" {
                eprintln!(
                    "{}",
                    json!({ "error": { "code": f.code, "message": f.message } })
                );
            } else {
                eprintln!("{}", f.message);
            }
            ExitCode::from(exit_code(&f.code))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(words: &[&str]) -> Options {
        parse(&words.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn import_files_walks_visible_markdown_in_path_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        for name in ["sub/b.md", "a.md", ".hidden/c.md", "x.txt", ".hidden.md"] {
            std::fs::write(root.join(name), "abc").unwrap();
        }
        std::os::unix::fs::symlink(root, root.join("sub/cycle")).unwrap();
        let files = import_files("markdown", root).unwrap();
        assert_eq!(
            files,
            vec![
                json!({"path": "a.md", "text": "abc"}),
                json!({"path": "sub/b.md", "text": "abc"})
            ]
        );
        assert_eq!(import_files("csv", root).unwrap_err().code, "invalid");
        let csv = root.join("tasks.csv");
        std::fs::write(&csv, "title\nA").unwrap();
        assert_eq!(
            import_files("csv", &csv).unwrap(),
            vec![json!({"path": "tasks.csv", "text": "title\nA"})]
        );
        assert_eq!(import_files("markdown", &csv).unwrap_err().code, "invalid");
    }

    #[test]
    fn import_request_limit_counts_json_escaping_and_envelope() {
        let o = opts(&["--space", "work", "--dry-run"]);
        let args = json!({"plugin": "todo", "importer": "csv", "files": [{"path": "a.csv", "text": "abc"}]});
        check_import_request(&o, &args, 3).unwrap();
        let params = invoke_params(&o, "action", "space.import", &args, false);
        assert_eq!(params["dry_run"], true);
        assert_eq!(params["space"], "work");
        assert_eq!(params["args"], args);
        let large = json!({"files": [{"path": "a.csv", "text": "\"".repeat(IMPORT_LIMIT / 2)}]});
        assert_eq!(
            check_import_request(&o, &large, 3).unwrap_err().message,
            IMPORT_TOO_LARGE
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.csv");
        std::fs::write(&path, vec![b'a'; IMPORT_LIMIT + 1]).unwrap();
        assert_eq!(
            import_files("csv", &path).unwrap_err().message,
            IMPORT_TOO_LARGE
        );
    }

    #[test]
    fn import_partial_failure_has_exit_code_one() {
        assert!(import_result(&json!({"message": "1 imported", "failed": 0}), false).is_ok());
        let failure =
            import_result(&json!({"message": "1 failed", "failed": 1}), false).unwrap_err();
        assert_eq!(exit_code(&failure.code), 1);
    }

    #[test]
    fn export_args_are_absolute() {
        let cwd = Path::new("/tmp/w");
        assert_eq!(
            export_args(&opts(&["--out", "a.zip"]), cwd).unwrap(),
            json!({"path": "/tmp/w/a.zip", "include_trash": false})
        );
        assert_eq!(
            export_args(&opts(&[]), cwd).unwrap(),
            json!({"path": "/tmp/w/terrarium-export.zip", "include_trash": false})
        );
        assert_eq!(
            export_args(&opts(&["--out", "/abs/b.zip", "--include-trash"]), cwd).unwrap(),
            json!({"path": "/abs/b.zip", "include_trash": true})
        );
        for args in [
            vec!["--nope", "1"],
            vec!["--out"],
            vec!["--out="],
            vec!["--include-trash", "false"],
            vec!["extra"],
            vec!["--args", "{}"],
        ] {
            assert_eq!(export_args(&opts(&args), cwd).unwrap_err().code, "usage");
        }
    }

    #[test]
    fn flags_and_words() {
        let o = opts(&[
            "todo.tasks",
            "--filter",
            "all",
            "--done",
            "--space=work",
            "--json",
        ]);
        assert_eq!(o.words, ["todo.tasks"]);
        assert_eq!(o.space.as_deref(), Some("work"));
        assert!(o.json);
        assert_eq!(
            o.flags,
            [
                ("filter".to_string(), Some("all".to_string())),
                ("done".to_string(), None)
            ]
        );
    }

    #[test]
    fn arguments_follow_the_description() {
        let info = json!({
            "id": "todo.tasks",
            "params": { "filter": { "type": "string" }, "limit": { "type": "integer" }, "flag": { "type": "boolean" } },
            "context": ["today"],
        });
        let o = opts(&[
            "x",
            "--filter",
            "all",
            "--limit",
            "5",
            "--flag",
            "--today",
            "2026-01-02",
        ]);
        assert_eq!(
            build_args(&o, &info).unwrap(),
            json!({"filter": "all", "limit": 5, "flag": true, "today": "2026-01-02"})
        );
        let bad = build_args(&opts(&["x", "--nope", "1"]), &info).unwrap_err();
        assert_eq!(bad.code, "invalid");
        assert!(bad.message.contains("--filter"));
        assert_eq!(
            build_args(&opts(&["x", "--limit", "five"]), &info)
                .unwrap_err()
                .code,
            "invalid"
        );
        // --args is the base; flags override it.
        let o = opts(&[
            "x",
            "--args",
            r#"{"filter":"done","q":"x"}"#,
            "--filter",
            "all",
        ]);
        let untyped = json!({ "id": "x.y", "params": null });
        assert_eq!(
            build_args(&o, &untyped).unwrap(),
            json!({"filter": "all", "q": "x"})
        );
    }

    #[test]
    fn ids_are_split_and_checked() {
        assert_eq!(parse_ids("a, b\nc").unwrap(), ["a", "b", "c"]);
        assert_eq!(parse_ids(" , ").unwrap_err().code, "usage");
    }

    #[test]
    fn bulk_flags_are_options() {
        let o = opts(&[
            "todo.toggle",
            "--ids",
            "a,b",
            "--where",
            "tasks",
            "--where-params",
            "{}",
            "--done",
            "true",
        ]);
        assert_eq!(o.ids.as_deref(), Some("a,b"));
        assert_eq!(o.where_query.as_deref(), Some("tasks"));
        assert_eq!(o.where_params.as_deref(), Some("{}"));
        assert_eq!(o.flags, [("done".to_string(), Some("true".to_string()))]);
    }

    #[test]
    fn bulk_request_with_ids_and_bulk_check() {
        let info = json!({
            "id": "todo.toggle",
            "bulk": true,
        });
        let request = bulk_request(
            &opts(&["todo.toggle", "--ids", "a,b"]),
            &info,
            json!({ "done": true, "id": "ignored" }),
        )
        .unwrap();
        assert_eq!(
            request,
            json!({
                "plugin": "todo",
                "op": "toggle",
                "ids": ["a", "b"],
                "args": { "done": true }
            })
        );
        let not_bulk = bulk_request(
            &opts(&["todo.toggle", "--ids", "a"]),
            &json!({ "id": "todo.toggle", "bulk": false }),
            json!({}),
        )
        .unwrap_err();
        assert_eq!(not_bulk.code, "invalid");
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code("invalid"), 2);
        assert_eq!(exit_code("not-found"), 3);
        assert_eq!(exit_code("forbidden"), 5);
        assert_eq!(exit_code("unreachable"), 6);
        assert_eq!(exit_code("needs-confirmation"), 7);
        assert_eq!(exit_code("anything"), 1);
    }
}
