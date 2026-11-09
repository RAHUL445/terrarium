//! `terra preview`: the app's UI in a browser, backed by a real plugin host.
//!
//! Serves the repository's `ui/` and answers the app's commands over HTTP
//! (`POST /invoke`). Plugin queries and operations run for real against a
//! scratch data folder; sync, devices and backups are stubbed. Packages are
//! reloaded whenever their manifest, templates or `dist/plugin.wasm` change.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use terrarium_runtime::{HostConfig, Manifest, PluginHost, ViewKind, DEFAULT_SPACE};
use tiny_http::{Header, Method, Response, Server};

/// Injected ahead of `app.js`: routes `invoke` to the preview server.
const SHIM: &str = r#"window.__TAURI__ = {
  core: { invoke: async (cmd, args) => {
    const res = await fetch("/invoke", { method: "POST", body: JSON.stringify({ cmd, args: args || {} }) });
    const body = await res.json();
    if (!res.ok) throw body.error;
    return body.result;
  } },
  event: { listen: async () => () => {} },
};"#;

pub fn run(dirs: Vec<PathBuf>, port: u16, data: PathBuf) -> Result<(), String> {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    let ui = std::fs::canonicalize(&ui).map_err(|e| format!("{}: {e}", ui.display()))?;
    std::fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.clone(),
        spaces_dir: None,
        space: "preview".into(),
        device: "terra-preview".into(),
        user: "developer".into(),
        timezone: local_timezone(),
        execution: Default::default(),
        speech: None,
    })
    .map_err(|e| e.to_string())?;
    host.set_ai_sealer(preview_sealer(&data)?);
    let mut stamps = vec![0u128; dirs.len()];
    reload(&mut host, &dirs, &mut stamps);

    let server = Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    println!(
        "preview on http://127.0.0.1:{port} (data in {})",
        data.display()
    );
    let mut last_tick = std::time::Instant::now();
    loop {
        let Ok(next) = server.recv_timeout(std::time::Duration::from_millis(500)) else {
            break;
        };
        // Scheduled runs and AI requests run between requests: the
        // preview serves one request at a time.
        if last_tick.elapsed() >= std::time::Duration::from_secs(5) {
            last_tick = std::time::Instant::now();
            for done in host.run_due(now_ms(), true) {
                println!("run {} {}", done.run.id, done.run.status);
            }
        }
        pump_ai(&mut host);
        let Some(mut request) = next else {
            continue;
        };
        let url = request.url().split('?').next().unwrap_or("/").to_string();
        let response = if *request.method() == Method::Post && url == "/invoke" {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            reload(&mut host, &dirs, &mut stamps);
            let (status, value) = match serde_json::from_str::<Value>(&body) {
                Ok(v) => match invoke(&mut host, v["cmd"].as_str().unwrap_or(""), &v["args"]) {
                    Ok(result) => (200, json!({ "result": result })),
                    Err(error) => (400, json!({ "error": error })),
                },
                Err(e) => (400, json!({ "error": e.to_string() })),
            };
            Response::from_string(value.to_string())
                .with_status_code(status)
                .with_header(header("Content-Type", "application/json"))
        } else {
            static_file(&ui, &url)
        };
        let _ = request.respond(response);
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The preview's data folder keeps its own sealing root.
fn preview_sealer(data: &Path) -> Result<terrarium_runtime::ai::Sealer, String> {
    let path = data.join("preview-secrets.key");
    let root: [u8; 32] = match std::fs::read(&path) {
        Ok(bytes) if bytes.len() == 32 => bytes.try_into().expect("32 bytes"),
        _ => {
            let mut root = [0u8; 32];
            getrandom::getrandom(&mut root).map_err(|e| e.to_string())?;
            std::fs::write(&path, root).map_err(|e| format!("{}: {e}", path.display()))?;
            root
        }
    };
    Ok(terrarium_runtime::ai::Sealer::new("device", &root))
}

/// Runs queued AI requests inline (the preview has one thread).
fn pump_ai(host: &mut PluginHost) {
    while let Some((job, config, key)) = host.take_ai_job() {
        let result = terrarium_runtime::ai::complete(&config, key.as_deref(), &job.prompt);
        match &result {
            Ok(c) => println!("AI reply for {} ({} tokens)", job.plugin, c.output_tokens),
            Err(e) => println!("AI request for {} failed: {}", job.plugin, e.message),
        }
        if let Some(done) = host.deliver_ai(job, result) {
            println!("run {} {}", done.run.id, done.run.status);
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn static_file(ui: &Path, url: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let rel = if url == "/" {
        "index.html"
    } else {
        url.trim_start_matches('/')
    };
    let path = ui.join(rel);
    let inside = std::fs::canonicalize(&path)
        .map(|p| p.starts_with(ui))
        .unwrap_or(false);
    let Some(bytes) = inside.then(|| std::fs::read(&path).ok()).flatten() else {
        return Response::from_string("not found").with_status_code(404);
    };
    let kind = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    let bytes = if rel == "index.html" {
        let html = String::from_utf8_lossy(&bytes);
        html.replacen("<script", &format!("<script>{SHIM}</script><script"), 1)
            .into_bytes()
    } else {
        bytes
    };
    Response::from_data(bytes)
        .with_header(header("Content-Type", kind))
        .with_header(header("Cache-Control", "no-store"))
}

/// Loads each package whose files changed since the last look.
fn reload(host: &mut PluginHost, dirs: &[PathBuf], stamps: &mut [u128]) {
    for (dir, stamp) in dirs.iter().zip(stamps.iter_mut()) {
        let now = crate::package_fingerprint(dir);
        if now == *stamp {
            continue;
        }
        *stamp = now;
        match host.load(dir) {
            Ok(m) => println!("loaded {} {}", m.id, m.version),
            Err(e) => eprintln!("{}: {e}", dir.display()),
        }
    }
}

fn invoke(host: &mut PluginHost, cmd: &str, args: &Value) -> Result<Value, String> {
    let text = |key: &str| args[key].as_str().unwrap_or_default().to_string();
    let space = args["space"].as_str().unwrap_or(DEFAULT_SPACE).to_string();
    let err = |e: terrarium_runtime::RuntimeError| e.to_string();
    Ok(match cmd {
        "plugins" => json!({
            "plugins": host.manifests().map(plugin_info).collect::<Vec<_>>(),
            "removed": [], "errors": [], "dev": true, "execution": host.execution_mode(),
        }),
        "view_template" => {
            let kind: ViewKind =
                serde_json::from_value(args["kind"].clone()).map_err(|e| e.to_string())?;
            host.template(&text("plugin"), kind, &text("id"))
                .map_err(err)?
                .clone()
        }
        "run_query" => {
            let params = args["params"].as_object().cloned().unwrap_or_default();
            json!(host
                .query_in(&space, &text("plugin"), &text("name"), &params)
                .map_err(err)?)
        }
        "run_action" => host
            .call_in(&space, &text("plugin"), &text("op"), &args["args"])
            .map_err(err)?,
        "list_spaces" => json!(host.spaces().list().map_err(err)?),
        "create_space" => {
            let space = host.spaces_mut().create(&text("name")).map_err(err)?;
            host.sync_spaces().map_err(err)?;
            json!(space)
        }
        "rename_space" => {
            host.spaces_mut()
                .rename(&text("id"), &text("name"))
                .map_err(err)?;
            Value::Null
        }
        "set_space_removed" => {
            host.spaces_mut()
                .set_removed(&text("id"), args["removed"].as_bool().unwrap_or(false))
                .map_err(err)?;
            host.sync_spaces().map_err(err)?;
            Value::Null
        }
        "sync_status" => json!({
            "key_storage": "preview", "drive": false, "device": "terra-preview", "name": "Preview",
            "role": "host", "can_host": false, "host": null, "devices": [], "tailnet_addresses": [],
            "pairing": false, "last": null,
        }),
        "snapshot" => {
            json!({ "items": [], "changes": [], "conflicts": [], "notices": [], "device": "terra-preview" })
        }
        "trusted_signers" | "list_backups" => json!([]),
        "open_external" | "profile_startup" => Value::Null,
        "list_themes" => json!(host
            .packages()
            .themes()
            .map_err(err)?
            .into_iter()
            .map(|t| json!({ "warnings": t.warnings(), "theme": t }))
            .collect::<Vec<_>>()),
        "install_theme" => {
            let theme = terrarium_runtime::Theme::parse(text("data").as_bytes()).map_err(err)?;
            let replaced = host.packages_mut().install_theme(&theme).map_err(err)?;
            json!({ "theme": theme, "replaced": replaced, "warnings": theme.warnings() })
        }
        "service" => {
            let name = text("name");
            terrarium_runtime::services::invoke(host, &name, &args["args"], true)
                .ok_or_else(|| format!("unknown command `{name}`"))??
        }
        "service_net" => {
            let (config, key) = terrarium_runtime::services::ai_probe(host, Some(&args["config"]))?;
            terrarium_runtime::services::ai_network(&text("name"), &config, key.as_deref())?
        }
        "remove_theme" => {
            host.packages_mut().remove_theme(&text("id")).map_err(err)?;
            Value::Null
        }
        other => return Err(format!("`{other}` is not available in preview")),
    })
}

fn plugin_info(m: &Manifest) -> Value {
    let views = |v: &[terrarium_runtime::ViewDef]| {
        v.iter()
            .map(|v| json!({ "id": v.id, "title": v.title }))
            .collect::<Vec<_>>()
    };
    let mut info = Map::new();
    for (k, v) in [
        ("id", json!(m.id)),
        ("name", json!(m.name)),
        ("version", json!(m.version)),
        ("description", json!(m.description)),
        ("pages", json!(views(&m.pages))),
        ("widgets", json!(views(&m.widgets))),
        ("origin", json!("dev")),
        ("previous_version", Value::Null),
        ("source", Value::Null),
        ("capabilities", json!(terrarium_runtime::capabilities(m))),
        ("icon", json!(m.icon)),
        ("color", json!(m.color)),
        ("badge", json!(m.badge)),
        ("search", json!(m.search)),
        ("quick_actions", json!(m.quick_actions)),
    ] {
        info.insert(k.into(), v);
    }
    Value::Object(info)
}

fn local_timezone() -> String {
    std::env::var("TZ")
        .ok()
        .filter(|tz| !tz.is_empty())
        .or_else(|| {
            let target = std::fs::read_link("/etc/localtime").ok()?;
            let target = target.to_string_lossy().into_owned();
            target.split("zoneinfo/").nth(1).map(str::to_string)
        })
        .unwrap_or_else(|| "UTC".into())
}
