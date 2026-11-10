//! Installing, updating, rolling back and removing plugins.
//!
//! Installing is two steps: [`stage_install`] reads and checks a package
//! and returns what the user must see (signer, capabilities, what an update
//! adds); [`confirm_install`] activates it. Changes go into the synced
//! plugin set, so the user's other devices follow.

use std::io::Read;
use std::path::Path;
#[cfg(desktop)]
use std::path::PathBuf;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
#[cfg(desktop)]
use terrarium_runtime::PACKAGE_EXT;
use terrarium_runtime::{
    added_capabilities, capabilities, key_fingerprint, Capability, Package, PackageSource,
    MAX_PACKAGE_BYTES,
};

use crate::plugins::{err, Origin, Plugins};

/// The package waiting for the user's consent.
#[derive(Default)]
pub struct Pending(Mutex<Option<Staged>>);

struct Staged {
    token: String,
    package: Package,
    source: PackageSource,
    added: Vec<Capability>,
    staged_at: std::time::Instant,
    synced: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum InstallSource {
    /// A built package directory (desktop).
    Folder { path: String },
    /// A `.terrapkg` file on disk (desktop).
    File { path: String },
    /// A `.terrapkg` chosen in the UI, base64.
    Upload { name: String, data: String },
    /// A Git repository holding a built package (desktop).
    Git { url: String },
    /// A package already transferred by sync, still requiring local consent.
    Synced { id: String },
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Trust {
    Trusted,
    Unknown,
    Unsigned,
}

#[derive(Serialize)]
pub struct Review {
    token: String,
    id: String,
    name: String,
    version: String,
    description: String,
    /// The version running now, if this is an update.
    current_version: Option<String>,
    trust: Trust,
    signer: Option<String>,
    signer_name: Option<String>,
    capabilities: Vec<Capability>,
    /// Capabilities the running version does not have; needs consent.
    added: Vec<Capability>,
    /// The running version comes from a development folder, which keeps
    /// precedence on this device.
    shadowed_by_dev: bool,
}

fn token() -> Result<String, String> {
    let mut bytes = [0u8; 12];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("Secure randomness unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(desktop)]
fn from_git(url: &str) -> Result<Package, String> {
    let (repo, subdir) = url.split_once('#').unwrap_or((url, ""));
    let allowed =
        repo.starts_with("https://") || repo.starts_with("ssh://") || repo.starts_with("git@");
    if !allowed || repo.chars().any(char::is_whitespace) {
        return Err("Use an https://, ssh:// or git@ repository address.".into());
    }
    if Path::new(subdir)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("The part after # must be a folder inside the repository.".into());
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix("terra-git-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let workspace = builder
        .tempdir()
        .map_err(|_| "Private download workspace unavailable")?;
    let tmp = workspace.path().join("repository");
    let mut command = std::process::Command::new("git");
    command
        .args([
            "-c",
            "protocol.file.allow=never",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "transfer.fsckObjects=true",
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--no-tags",
            "--no-recurse-submodules",
            "--quiet",
            "--",
        ])
        .arg(repo)
        .arg(&tmp)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    supervise_clone(
        command,
        workspace.path(),
        std::time::Duration::from_secs(60),
        128 * 1024 * 1024,
    )?;
    let result = (|| {
        let dir = tmp.join(subdir);
        if dir.join("manifest.json").is_file() && dir.join("dist/plugin.wasm").is_file() {
            return Package::from_dir(&dir).map_err(err);
        }
        // Otherwise a released .terrapkg in the folder or its dist/.
        for folder in [dir.clone(), dir.join("dist")] {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&folder)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == PACKAGE_EXT))
                .collect();
            found.sort();
            if let Some(path) = found.pop() {
                let bytes = read_archive(&path)?;
                return Package::from_zip(&bytes).map_err(err);
            }
        }
        // Building would run the repository's code on this computer.
        Err(
            "The repository has no built package (dist/plugin.wasm or a .terrapkg). \
             The app does not build code from Git; build and commit it, or publish a .terrapkg."
                .into(),
        )
    })();
    result
}

/// A watchdog, not a filesystem quota: a fast writer can overshoot between polls.
#[cfg(desktop)]
fn clone_size(root: &Path, limit: u64) -> Result<(), String> {
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0u64;
    let mut count = 0u32;
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Could not inspect download".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "Could not inspect download")?;
            // Git renames/removes temporary objects while the watchdog scans.
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("Could not inspect download".into()),
            };
            count += 1;
            bytes = bytes.saturating_add(meta.len());
            if bytes > limit || count > 20_000 {
                return Err("Git download exceeds its workspace budget".into());
            }
            if meta.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(desktop)]
fn supervise_clone(
    mut command: std::process::Command,
    root: &Path,
    timeout: std::time::Duration,
    limit: u64,
) -> Result<(), String> {
    use std::process::Stdio;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start Git")?;
    let started = std::time::Instant::now();
    let outcome = loop {
        if started.elapsed() > timeout {
            break Err("Git download timed out".into());
        }
        if let Err(error) = clone_size(root, limit) {
            break Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    clone_size(root, limit)
                } else {
                    Err("Git download failed; check the repository address and access".into())
                }
            }
            Err(_) => break Err("Could not wait for Git".into()),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    };
    if outcome.is_err() {
        // git may spawn SSH/remote helpers; stop its dedicated group on Unix.
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    outcome
}

#[cfg(not(desktop))]
fn from_git(_url: &str) -> Result<Package, String> {
    Err("Install from Git on your computer; the phone gets the plugin by sync.".into())
}

fn read_archive(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path).map_err(err)?;
    if !metadata.is_file() || metadata.len() > MAX_PACKAGE_BYTES {
        return Err("Choose a regular package file no larger than 32 MB.".into());
    }
    let file = std::fs::File::open(path).map_err(err)?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err("Package is larger than 32 MB.".into());
    }
    Ok(bytes)
}

fn read(source: &InstallSource, plugins: &Plugins) -> Result<(Package, PackageSource), String> {
    let desktop_only = |what: &str| -> Result<(), String> {
        if cfg!(desktop) {
            Ok(())
        } else {
            Err(format!(
                "Install from a {what} on your computer, or choose a .terrapkg file."
            ))
        }
    };
    match source {
        InstallSource::Folder { path } => {
            desktop_only("folder")?;
            let package = Package::from_dir(Path::new(path)).map_err(err)?;
            Ok((package, PackageSource::Folder(path.clone())))
        }
        InstallSource::File { path } => {
            desktop_only("file path")?;
            let bytes = read_archive(Path::new(path))?;
            Ok((
                Package::from_zip(&bytes).map_err(err)?,
                PackageSource::File(path.clone()),
            ))
        }
        InstallSource::Upload { name, data } => {
            if data.len() as u64 > MAX_PACKAGE_BYTES.div_ceil(3) * 4 {
                return Err("Package is larger than 32 MB.".into());
            }
            let bytes = B64.decode(data).map_err(err)?;
            Ok((
                Package::from_zip(&bytes).map_err(err)?,
                PackageSource::File(name.clone()),
            ))
        }
        InstallSource::Git { url } => Ok((from_git(url)?, PackageSource::Git(url.clone()))),
        InstallSource::Synced { id } => plugins
            .with_host(|host| {
                let installed = host
                    .packages()
                    .get(id)
                    .map_err(err)?
                    .ok_or("Plugin is no longer installed.")?;
                if installed.removed {
                    return Err("Plugin was removed; add it back before reviewing.".into());
                }
                let package = host
                    .packages()
                    .blob(&installed.digest)
                    .map_err(err)?
                    .ok_or("Package has not arrived yet.")?;
                if package.manifest().id != *id || package.manifest().version != installed.version {
                    return Err("Synced package identity does not match.".into());
                }
                Ok((package, PackageSource::File("Synced package".into())))
            })
            .ok_or("plugin host unavailable")?,
    }
}

/// Reads and checks a package; nothing changes until [`confirm_install`].
#[tauri::command]
pub async fn stage_install(app: AppHandle, source: InstallSource) -> Result<Review, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        let synced = matches!(source, InstallSource::Synced { .. });
        let (package, source) = read(&source, &plugins)?;
        validate_signature_policy(package.signer(), cfg!(debug_assertions))?;
        let manifest = package.manifest().clone();
        let loaded = plugins.loaded(&manifest.id);
        let (current, trusted, signer_name) = plugins
            .with_host(|host| {
                let current = host.manifests().find(|m| m.id == manifest.id).cloned();
                let signers = host.packages().signers().unwrap_or_default();
                let name = package.signer().and_then(|key| {
                    signers
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, n)| n.clone())
                });
                (current, name.is_some(), name)
            })
            .ok_or("plugin host unavailable")?;
        let review = Review {
            token: token()?,
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            current_version: current.as_ref().map(|m| m.version.clone()),
            trust: match (package.signer(), trusted) {
                (None, _) => Trust::Unsigned,
                (Some(_), true) => Trust::Trusted,
                (Some(_), false) => Trust::Unknown,
            },
            signer: package.signer().map(key_fingerprint),
            signer_name,
            capabilities: capabilities(&manifest),
            added: added_capabilities(current.as_ref(), &manifest),
            shadowed_by_dev: loaded.is_some_and(|l| l.origin == Origin::Dev),
        };
        *app.state::<Pending>().0.lock().map_err(err)? = Some(Staged {
            token: review.token.clone(),
            package,
            source,
            added: review.added.clone(),
            staged_at: std::time::Instant::now(),
            synced,
        });
        Ok(review)
    })
    .await
    .map_err(err)?
}

/// Activates the staged package. `trust` must be true for a package from
/// an unknown publisher (its key is then trusted on every device) or an
/// unsigned one (development builds only). Additional capabilities require
/// separate backend consent, even for an already trusted publisher.
#[tauri::command]
pub async fn confirm_install(
    app: AppHandle,
    token: String,
    trust: bool,
    approve_capabilities: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Pending>();
        let staged = {
            let mut pending = state.0.lock().map_err(err)?;
            match pending.take() {
                Some(s) if s.token == token => s,
                other => {
                    *pending = other;
                    return Err("This install expired; choose the package again.".into());
                }
            }
        };
        validate_confirmation(
            staged.package.signer(),
            cfg!(debug_assertions),
            staged.staged_at.elapsed(),
            !staged.added.is_empty(),
            approve_capabilities,
        )?;
        let plugins = app.state::<Plugins>();
        let id = staged.package.manifest().id.clone();
        let current = plugins.current_for_rollback(&id);
        plugins
            .with_host(|host| {
                match staged.package.signer() {
                    Some(key) if !host.packages().is_trusted(key).map_err(err)? => {
                        if !trust {
                            return Err("Confirm that you trust this publisher.".to_string());
                        }
                        let name = format!("Publisher {}", key_fingerprint(key));
                        host.packages_mut().trust(key, &name).map_err(err)?;
                    }
                    None if !trust => {
                        return Err("Confirm installing an unsigned package.".to_string())
                    }
                    _ => {}
                }
                // Consent is device-local and bound to exact files and signer,
                // never inferred from a synced publisher trust record.
                if staged.synced {
                    return host
                        .packages_mut()
                        .approve_synced(&staged.package)
                        .map_err(err);
                }
                host.packages_mut()
                    .approve_local(&staged.package)
                    .map_err(err)?;
                host.packages_mut()
                    .install(
                        &staged.package,
                        &staged.source,
                        current.as_ref().map(|(v, d)| (v.as_str(), d.as_str())),
                    )
                    .map(|_| ())
                    .map_err(err)
            })
            .ok_or("plugin host unavailable")??;
        plugins.reconcile(&crate::shell::AppShell(app.clone()));
        Ok(id)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn cancel_install(pending: State<'_, Pending>) {
    if let Ok(mut p) = pending.0.lock() {
        *p = None;
    }
}

/// Returns to the version before the last update, on every device.
#[tauri::command]
pub async fn rollback_plugin(app: AppHandle, id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        let installed = plugins
            .with_host(|host| host.packages_mut().rollback(&id).map_err(err))
            .ok_or("plugin host unavailable")??;
        plugins.reconcile(&crate::shell::AppShell(app.clone()));
        Ok(installed.version)
    })
    .await
    .map_err(err)?
}

/// Removes a plugin from every device, or adds a removed one back. Its
/// data stays either way.
#[tauri::command]
pub async fn set_plugin_removed(app: AppHandle, id: String, removed: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        let current = plugins
            .loaded(&id)
            .and_then(|l| Some((l.version, l.digest?)));
        plugins
            .with_host(|host| {
                host.packages_mut()
                    .set_removed(
                        &id,
                        removed,
                        current.as_ref().map(|(v, d)| (v.as_str(), d.as_str())),
                    )
                    .map_err(err)
            })
            .ok_or("plugin host unavailable")??;
        plugins.reconcile(&crate::shell::AppShell(app.clone()));
        Ok(())
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
pub struct SignerInfo {
    fingerprint: String,
    name: String,
}

#[tauri::command]
pub fn trusted_signers(plugins: State<'_, Plugins>) -> Result<Vec<SignerInfo>, String> {
    plugins
        .with_host(|host| host.packages().signers().map_err(err))
        .ok_or("plugin host unavailable")?
        .map(|list| {
            list.into_iter()
                .map(|(key, name)| SignerInfo {
                    fingerprint: key_fingerprint(&key),
                    name,
                })
                .collect()
        })
}

/// Trusts this computer's `terra keygen` key, so the owner's own packages
/// install without a prompt.
pub fn trust_developer_key(plugins: &Plugins) {
    if !cfg!(all(debug_assertions, desktop)) {
        return;
    }
    let Some(path) = terrarium_runtime::default_signing_key_path().filter(|p| p.is_file()) else {
        return;
    };
    let Ok(secret) = terrarium_runtime::read_signing_key(&path) else {
        return;
    };
    let public = terrarium_runtime::signing_public_key(&secret);
    plugins.with_host(|host| {
        if !host.packages().is_trusted(&public).unwrap_or(true) {
            let _ = host
                .packages_mut()
                .trust(&public, "Developer key on this computer");
        }
    });
}

/// Production installs always need a verifiable publisher identity. Explicit
/// publisher trust is still required by confirm_install for unknown signers.
fn validate_signature_policy(signer: Option<&str>, development: bool) -> Result<(), String> {
    if signer.is_none() && !development {
        return Err(
            "Unsigned plugins are only supported in development builds. Choose a signed .terrapkg."
                .into(),
        );
    }
    Ok(())
}

fn validate_confirmation(
    signer: Option<&str>,
    development: bool,
    age: std::time::Duration,
    added: bool,
    approved: bool,
) -> Result<(), String> {
    validate_signature_policy(signer, development)?;
    if age > std::time::Duration::from_secs(600) {
        return Err("This install expired; choose the package again.".into());
    }
    if added && !approved {
        return Err("Confirm the plugin permissions before installing.".into());
    }
    Ok(())
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use std::time::Duration;

    #[cfg(all(desktop, unix))]
    #[test]
    fn git_watchdog_limits_size_and_wall_time() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("large"), [0u8; 256]).unwrap();
        assert!(clone_size(dir.path(), 128).is_err());
        std::fs::remove_file(dir.path().join("large")).unwrap();
        // Symlinks are inspected but never followed into another directory.
        std::os::unix::fs::symlink("/", dir.path().join("link")).unwrap();
        assert!(clone_size(dir.path(), 1024).is_ok());
        let mut failed = std::process::Command::new("/bin/sh");
        failed.args(["-c", "echo PRIVATE_CREDENTIAL >&2; exit 1"]);
        let error = supervise_clone(failed, dir.path(), Duration::from_secs(2), 1024).unwrap_err();
        assert!(!error.contains("PRIVATE_CREDENTIAL"));
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 10"]);
        let start = std::time::Instant::now();
        assert!(
            supervise_clone(command, dir.path(), Duration::from_millis(20), 1024)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn production_rejects_unsigned_even_with_capability_consent() {
        assert!(validate_confirmation(None, false, Duration::ZERO, true, true).is_err());
        assert!(validate_signature_policy(None, true).is_ok());
        assert!(validate_signature_policy(Some("verified-key"), false).is_ok());
    }

    #[test]
    fn new_permissions_need_backend_consent_and_reviews_expire() {
        for development in [false, true] {
            assert!(
                validate_confirmation(Some("key"), development, Duration::ZERO, true, false)
                    .is_err()
            );
            assert!(
                validate_confirmation(Some("key"), development, Duration::ZERO, true, true).is_ok()
            );
            assert!(
                validate_confirmation(Some("key"), development, Duration::ZERO, false, false)
                    .is_ok()
            );
            assert!(validate_confirmation(
                Some("key"),
                development,
                Duration::from_secs(601),
                false,
                true
            )
            .is_err());
        }
    }
}
