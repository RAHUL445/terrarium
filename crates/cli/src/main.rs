//! `terra`: plugin developer tool.
//!
//!   terra new <dir> [--id <id>] [--name <name>] [--sdk <path>]   scaffold a plugin
//!   terra build <dir>                             build dist/plugin.wasm and check it loads
//!   terra check <dir>                             check a built package loads
//!   terra dev <dir>                               rebuild on every change (the app reloads it)
//!   terra keygen                                  create your signing key
//!   terra pack <dir> [-o <file>] [--unsigned]     build a signed .terrapkg
//!   terra verify <file>                           show a package's digest and signer
//!   terra theme <file>                            check a theme file and its contrast
//!   terra preview <dir>... [--port <n>] [--data <dir>]
//!                                                the app UI in a browser with these plugins

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, UNIX_EPOCH};

mod preview;

use terrarium_runtime::{
    build_plugin, default_signing_key_path, generate_signing_key, key_fingerprint,
    read_signing_key, HostConfig, Package, PluginHost, Theme, PACKAGE_EXT,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("new") => cmd_new(&args[1..]),
        Some("build") => dir_arg(&args[1..]).and_then(|d| cmd_build(&d)),
        Some("check") => dir_arg(&args[1..]).and_then(|d| check(&d)),
        Some("dev") => dir_arg(&args[1..]).and_then(|d| cmd_dev(&d)),
        Some("keygen") => cmd_keygen(),
        Some("pack") => cmd_pack(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("theme") => cmd_theme(&args[1..]),
        Some("preview") => cmd_preview(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:
  terra new <dir> [--id <id>] [--name <name>] [--sdk <path>]
  terra build <dir>
  terra check <dir>
  terra dev <dir>
  terra keygen
  terra pack <dir> [-o <file>] [--unsigned]
  terra verify <file>
  terra theme <file>
  terra preview <dir>... [--port <n>] [--data <dir>]";

fn dir_arg(args: &[String]) -> Result<PathBuf, String> {
    let dir = args.first().ok_or(USAGE)?;
    std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))
}

fn cmd_build(dir: &Path) -> Result<(), String> {
    let started = Instant::now();
    let out = build_plugin(dir).map_err(|e| e.to_string())?;
    check(dir)?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!(
        "built {} ({} KB) in {:.1}s",
        out.display(),
        size / 1024,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Loads the package into a throwaway host: manifest, component imports,
/// storage schema, query SQL and templates must all be valid.
fn check(dir: &Path) -> Result<(), String> {
    let scratch = std::env::temp_dir().join(format!("terra-check-{}", std::process::id()));
    let result = (|| {
        let mut host = PluginHost::new(HostConfig {
            data_dir: scratch.clone(),
            spaces_dir: None,
            space: "check".into(),
            device: "terra-check".into(),
            user: "developer".into(),
            timezone: "UTC".into(),
            execution: Default::default(),
            speech: None,
        })
        .map_err(|e| e.to_string())?;
        let manifest = host.load(dir).map_err(|e| e.to_string())?;
        println!(
            "ok: {} {} ({} operations, {} queries, {} pages, {} widgets)",
            manifest.id,
            manifest.version,
            manifest.operations.len(),
            manifest.queries.len(),
            manifest.pages.len(),
            manifest.widgets.len()
        );
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn cmd_dev(dir: &Path) -> Result<(), String> {
    println!("watching {} (Ctrl-C to stop)", dir.display());
    let mut last = 0;
    loop {
        let current = source_fingerprint(dir);
        if current != last {
            last = current;
            match cmd_build(dir) {
                Ok(()) => println!("ready: the app reloads this plugin automatically"),
                Err(e) => eprintln!("build failed:\n{e}"),
            }
            // Building touches nothing we watch, but skip its own writes.
            last = last.max(source_fingerprint(dir));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Latest modification time of everything a build depends on.
fn source_fingerprint(dir: &Path) -> u128 {
    fn walk(path: &Path, latest: &mut u128) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if matches!(entry.file_name().to_str(), Some("target" | "dist" | ".git")) {
                continue;
            }
            if path.is_dir() {
                walk(&path, latest);
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
    walk(dir, &mut latest);
    latest
}

/// Latest modification time of what a host loads: manifest, templates and
/// the built component.
pub(crate) fn package_fingerprint(dir: &Path) -> u128 {
    let stamp = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos())
    };
    let mut latest = stamp(&dir.join("manifest.json")).max(stamp(&dir.join("dist/plugin.wasm")));
    if let Ok(entries) = std::fs::read_dir(dir.join("ui")) {
        for entry in entries.flatten() {
            latest = latest.max(stamp(&entry.path()));
        }
    }
    latest
}

fn cmd_preview(args: &[String]) -> Result<(), String> {
    let mut dirs = Vec::new();
    let mut port = 8766;
    let mut data = std::env::temp_dir().join("terra-preview");
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--port" => {
                port = rest
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port needs a number")?
            }
            "--data" => data = PathBuf::from(rest.next().ok_or("--data needs a folder")?),
            dir => dirs.push(std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?),
        }
    }
    if dirs.is_empty() {
        return Err(USAGE.to_string());
    }
    preview::run(dirs, port, data)
}

fn cmd_new(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or(USAGE)?);
    let mut id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin")
        .to_lowercase();
    let mut name = None;
    let mut sdk = std::env::var_os("TERRA_SDK").map(PathBuf::from);
    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        match flag.as_str() {
            "--id" => id = rest.next().ok_or("--id needs a value")?.clone(),
            "--name" => name = Some(rest.next().ok_or("--name needs a value")?.clone()),
            "--sdk" => sdk = Some(PathBuf::from(rest.next().ok_or("--sdk needs a path")?)),
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    let valid = id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(format!(
            "id `{id}` must use a-z, 0-9 and - and start with a letter (use --id)"
        ));
    }
    if dir.exists()
        && std::fs::read_dir(&dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(format!("{} already exists and is not empty", dir.display()));
    }
    let name = name.unwrap_or_else(|| title_case(&id));
    let guest = sdk.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../guest"));
    let guest = std::fs::canonicalize(&guest).map_err(|e| format!("{}: {e}", guest.display()))?;

    let files = [
        ("Cargo.toml", template::cargo_toml(&id, &guest)),
        ("src/lib.rs", template::LIB_RS.to_string()),
        ("manifest.json", template::manifest(&id, &name)),
        ("ui/page-main.json", template::page(&name)),
        (".gitignore", "target/\ndist/\n".to_string()),
    ];
    for (path, contents) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!(
        "created {} ({id}). Next:\n  terra dev {}",
        dir.display(),
        dir.display()
    );
    Ok(())
}

fn title_case(id: &str) -> String {
    id.split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

mod template {
    use std::path::Path;

    pub fn cargo_toml(id: &str, guest: &Path) -> String {
        format!(
            r#"[package]
name = "{id}"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
terrarium-guest = {{ path = "{}" }}
serde_json = "1"

[profile.release]
opt-level = "s"
lto = true
strip = true
"#,
            guest.display()
        )
    }

    pub const LIB_RS: &str = r#"use terrarium_guest::{storage, Args, Error, Plugin, Result};
use serde_json::{json, Map, Value};

struct MyPlugin;

impl Plugin for MyPlugin {
    fn handle(operation: &str, args: Args) -> Result<Value> {
        match operation {
            "add" => {
                let text = args.str("text")?.trim();
                if text.is_empty() {
                    return Err(Error::invalid("Text is required"));
                }
                let mut values = Map::new();
                values.insert("text".into(), json!(text));
                Ok(json!({ "id": storage::insert("items", &values)? }))
            }
            "remove" => {
                storage::delete("items", args.str("id")?)?;
                Ok(Value::Null)
            }
            _ => Err(Error::unknown_operation(operation)),
        }
    }
}

terrarium_guest::plugin!(MyPlugin);
"#;

    pub fn manifest(id: &str, name: &str) -> String {
        format!(
            r#"{{
  "id": "{id}",
  "name": "{name}",
  "version": "0.1.0",
  "api": "terrarium:plugin@0.1.0",
  "storage": {{
    "version": 1,
    "tables": [
      {{ "name": "items", "columns": [ {{ "name": "text", "type": "text" }} ] }}
    ]
  }},
  "operations": {{
    "add": {{ "description": "Add an item." }},
    "remove": {{ "description": "Move an item to trash." }}
  }},
  "queries": {{
    "items": {{ "sql": "SELECT id, text FROM items WHERE _deleted IS NULL ORDER BY id DESC", "params": [] }}
  }},
  "pages": [ {{ "id": "main", "title": "{name}", "template": "ui/page-main.json" }} ]
}}
"#
        )
    }

    pub fn page(name: &str) -> String {
        format!(
            r#"{{
  "state": {{}},
  "body": {{
    "type": "stack", "gap": 16,
    "children": [
      {{ "type": "heading", "text": "{name}" }},
      {{
        "type": "form",
        "submit": {{ "op": "add", "args": {{ "text": "{{{{form.text}}}}" }}, "reset": true }},
        "submit_label": "Add",
        "children": [ {{ "type": "text-field", "name": "text", "label": "Text", "required": true }} ]
      }},
      {{
        "type": "list", "query": "items", "empty": "No items yet.",
        "item": {{
          "type": "stack", "direction": "row", "align": "center", "gap": 12,
          "children": [
            {{ "type": "text", "text": "{{{{row.text}}}}", "grow": 1 }},
            {{ "type": "button", "label": "Remove", "aria_label": "Remove {{{{row.text}}}}",
              "action": {{ "op": "remove", "args": {{ "id": "{{{{row.id}}}}" }} }} }}
          ]
        }}
      }}
    ]
  }}
}}
"#
        )
    }
}

fn key_path() -> Result<PathBuf, String> {
    default_signing_key_path().ok_or_else(|| "HOME is not set".to_string())
}

fn read_key() -> Result<[u8; 32], String> {
    let path = key_path()?;
    if !path.exists() {
        return Err(format!(
            "{} does not exist; run `terra keygen` first, or pass --unsigned",
            path.display()
        ));
    }
    read_signing_key(&path).map_err(|e| e.to_string())
}

fn cmd_keygen() -> Result<(), String> {
    let path = key_path()?;
    if path.exists() {
        return Err(format!(
            "{} already exists; delete it first to make a new key",
            path.display()
        ));
    }
    let (secret, public) = generate_signing_key().map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(
        &mut options.open(&path).map_err(|e| e.to_string())?,
        hex.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "signing key written to {} (keep it private)",
        path.display()
    );
    println!("public key:  {public}");
    println!("fingerprint: {}", key_fingerprint(&public));
    Ok(())
}

fn cmd_pack(args: &[String]) -> Result<(), String> {
    let dir = dir_arg(args)?;
    let mut out = None;
    let mut unsigned = false;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-o" | "--out" => out = Some(PathBuf::from(rest.next().ok_or(USAGE)?)),
            "--unsigned" => unsigned = true,
            other => return Err(format!("unknown option `{other}`\n{USAGE}")),
        }
    }
    check(&dir)?;
    let mut package = Package::from_dir(&dir).map_err(|e| e.to_string())?;
    if !unsigned {
        package.sign(&read_key()?);
    }
    let manifest = package.manifest();
    let out = out.unwrap_or_else(|| {
        dir.join("dist").join(format!(
            "{}-{}.{PACKAGE_EXT}",
            manifest.id, manifest.version
        ))
    });
    let bytes = package.to_zip().map_err(|e| e.to_string())?;
    std::fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("packed {} ({} KB)", out.display(), bytes.len() / 1024);
    println!("digest: {}", package.digest());
    match package.signer() {
        Some(key) => println!("signed by {}", key_fingerprint(key)),
        None => println!("unsigned"),
    }
    Ok(())
}

fn cmd_theme(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or(USAGE)?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let theme = Theme::parse(&bytes).map_err(|e| e.to_string())?;
    let modes: Vec<&str> = [
        ("light", theme.light.is_some()),
        ("dark", theme.dark.is_some()),
    ]
    .into_iter()
    .filter_map(|(m, has)| has.then_some(m))
    .collect();
    println!(
        "{} {} ({}), {}",
        theme.id,
        theme.version,
        theme.name,
        modes.join(" and ")
    );
    let warnings = theme.warnings();
    for w in &warnings {
        println!("  warning: {w}");
    }
    if warnings.is_empty() {
        println!("contrast ok");
    }
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or(USAGE)?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let package = Package::from_zip(&bytes).map_err(|e| e.to_string())?;
    let manifest = package.manifest();
    println!("{} {} ({})", manifest.id, manifest.version, manifest.name);
    println!("digest: {}", package.digest());
    match package.signer() {
        Some(key) => println!("signature ok, signed by {} ({key})", key_fingerprint(key)),
        None => println!("unsigned"),
    }
    for cap in terrarium_runtime::capabilities(manifest) {
        println!("  can: {}", cap.label);
    }
    Ok(())
}
