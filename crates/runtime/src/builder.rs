//! Building a plugin crate into the component the host loads.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, RuntimeError};

const TARGET: &str = "wasm32-unknown-unknown";

/// Wraps a core Wasm module (built with `wit-bindgen`) into a component.
pub fn componentize(module: &[u8]) -> Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .module(module)
        .and_then(|encoder| encoder.validate(true).encode())
        .map_err(|e| RuntimeError::Build(format!("componentize: {e:#}")))
}

/// Runs `cargo build` for the plugin at `dir` and writes `dist/plugin.wasm`.
pub fn build_plugin(dir: &Path) -> Result<PathBuf> {
    let cargo_toml = dir.join("Cargo.toml");
    let crate_name = package_name(&cargo_toml)?.replace('-', "_");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["build", "--release", "--target", TARGET, "--manifest-path"])
        .arg(&cargo_toml)
        .output()
        .map_err(|e| RuntimeError::Build(format!("could not run cargo: {e}")))?;
    if !output.status.success() {
        return Err(RuntimeError::Build(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let module_path = dir
        .join("target")
        .join(TARGET)
        .join("release")
        .join(format!("{crate_name}.wasm"));
    let module = std::fs::read(&module_path).map_err(|e| RuntimeError::io(&module_path, e))?;
    let component = componentize(&module)?;
    let dist = dir.join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| RuntimeError::io(&dist, e))?;
    let out = dist.join("plugin.wasm");
    std::fs::write(&out, component).map_err(|e| RuntimeError::io(&out, e))?;
    Ok(out)
}

fn package_name(cargo_toml: &Path) -> Result<String> {
    let text = std::fs::read_to_string(cargo_toml).map_err(|e| RuntimeError::io(cargo_toml, e))?;
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(value) = line
                .strip_prefix("name")
                .map(str::trim)
                .and_then(|l| l.strip_prefix('='))
            {
                return Ok(value.trim().trim_matches('"').to_string());
            }
        }
    }
    Err(RuntimeError::Build(format!(
        "no [package] name in {}",
        cargo_toml.display()
    )))
}
