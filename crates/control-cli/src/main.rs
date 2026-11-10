//! `terrarium`: control the running Terrarium app from the command line.
//!
//! It talks to the app over the local control socket (see
//! `terrarium-control-proto`) and never opens the app's data itself.
//! Turn on Settings → Developer → Command-line control, add a client, then
//! run `terrarium login` with its token.

use std::io::{BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
  query <id> [--param value]... run a query, e.g. terrarium query todo.tasks --filter all
  search <text>                 search every plugin (--all-spaces, --plugin <id>, --limit <n>)
  watch                         print app events as data changes

options:
  --space <id>      run in this space (default: the default space)
  --args <json>     arguments as a JSON object, or @file.json
  --json            print JSON (the default when output is not a terminal)
  --socket <path>   the app's socket (default: in the app's data folder, or $TERRA_SOCKET)

The token comes from $TERRA_TOKEN, or from the file `terrarium login` saves.

exit codes: 0 ok, 1 failed, 2 invalid, 3 not found, 4 in trash,
            5 not allowed, 6 the app is not reachable";

/// Exit codes for error codes.
fn exit_code(code: &str) -> u8 {
    match code {
        "invalid" | "usage" => 2,
        "not-found" | "unknown-space" => 3,
        "in-trash" => 4,
        "forbidden" | "unauthorized" => 5,
        "unreachable" => 6,
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

#[derive(Default)]
struct Options {
    json: bool,
    socket: Option<PathBuf>,
    space: Option<String>,
    args: Option<String>,
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
            "socket" => o.socket = Some(PathBuf::from(value("socket")?)),
            "space" => o.space = Some(value("space")?),
            "args" => o.args = Some(value("args")?),
            _ => {
                // A flag's value is the next word unless that is another flag.
                let v = inline.or_else(|| it.next_if(|next| !next.starts_with("--")).cloned());
                o.flags.push((name, v));
            }
        }
    }
    Ok(o)
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
                    "Can't reach Terrarium at {} ({e}). Is the app open, with Settings → Developer → Command-line control on?",
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

    fn invoke(&mut self, o: &Options, kind: &str, id: &str, args: Value) -> Result<Value, Failure> {
        self.call(
            "invoke",
            json!({ "kind": kind, "id": id, "space": o.space, "args": args }),
        )
    }
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
    Ok(())
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
    let result = conn.invoke(o, "query", id, args)?;
    if human(o) {
        print_value(&result);
    } else {
        print_json(&result);
    }
    Ok(())
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
    let found = conn.invoke(o, "query", "search.query", args)?;
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
                cell(&r["title"]),
                cell(&r["subtitle"]),
                cell(&r["space"]),
                cell(&r["id"]),
            ]
        })
        .collect();
    if !rows.is_empty() {
        print_table(
            &["plugin", "title", "detail", "space", "id"].map(String::from),
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
        "search" => cmd_search(&o),
        "watch" => cmd_watch(&o),
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
    fn exit_codes() {
        assert_eq!(exit_code("invalid"), 2);
        assert_eq!(exit_code("not-found"), 3);
        assert_eq!(exit_code("forbidden"), 5);
        assert_eq!(exit_code("unreachable"), 6);
        assert_eq!(exit_code("anything"), 1);
    }
}
