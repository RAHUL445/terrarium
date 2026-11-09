//! Plugin packages: the files a plugin ships, a digest over them, an
//! optional ed25519 signature, and the capabilities its manifest asks for.
//!
//! A package file (`.terrapkg`) is a ZIP of `manifest.json`,
//! `dist/plugin.wasm`, the UI templates the manifest names and, when
//! signed, `signature.json`. The digest covers every file except the
//! signature, so the same files always give the same digest however they
//! were zipped.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature as EdSignature, Signer as _, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Result, RuntimeError};
use crate::manifest::{Limits, Manifest, RunOn};

pub const PACKAGE_EXT: &str = "terrapkg";
const MANIFEST: &str = "manifest.json";
const WASM: &str = "dist/plugin.wasm";
const SIGNATURE: &str = "signature.json";
pub const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BYTES: u64 = MAX_PACKAGE_BYTES;
const MAX_FILES: usize = 256;

/// `signature.json`: who signed the digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signature {
    /// Base64 ed25519 public key.
    pub key: String,
    /// Base64 signature over `terrapkg-sig-v1:<digest>`.
    pub sig: String,
}

#[derive(Clone, Debug)]
pub struct Package {
    files: BTreeMap<String, Vec<u8>>,
    manifest: Manifest,
    signature: Option<Signature>,
}

fn bad(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Package(message.into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A path inside the package: relative, `/`-separated, no `..`.
fn clean_path(path: &str) -> Result<String> {
    let p = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(bad(format!(
            "`{path}` is not a plain path inside the package"
        )));
    }
    Ok(path.to_string())
}

impl Package {
    /// The loadable files of a built package directory.
    pub fn from_dir(dir: &Path) -> Result<Package> {
        let root = dir.canonicalize().map_err(|e| RuntimeError::io(dir, e))?;
        let mut total = 0u64;
        let mut read = |rel: &str| -> Result<Vec<u8>> {
            clean_path(rel)?;
            let path = root.join(rel);
            let canonical = path
                .canonicalize()
                .map_err(|e| RuntimeError::io(&path, e))?;
            if !canonical.starts_with(&root) {
                return Err(bad("package file escapes its directory"));
            }
            let file = std::fs::File::open(&canonical).map_err(|e| RuntimeError::io(&path, e))?;
            if !file
                .metadata()
                .map_err(|e| RuntimeError::io(&path, e))?
                .is_file()
            {
                return Err(bad("package entries must be regular files"));
            }
            let mut data = Vec::new();
            file.take(MAX_BYTES - total + 1)
                .read_to_end(&mut data)
                .map_err(|e| RuntimeError::io(&path, e))?;
            total += data.len() as u64;
            if total > MAX_BYTES {
                return Err(bad("package unpacks to more than 32 MB"));
            }
            Ok(data)
        };
        let manifest_bytes = read(MANIFEST)?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        let mut files = BTreeMap::new();
        files.insert(MANIFEST.to_string(), manifest_bytes);
        files.insert(WASM.to_string(), read(WASM)?);
        for view in manifest.pages.iter().chain(&manifest.widgets) {
            let name = clean_path(&view.template)?;
            if name == SIGNATURE {
                return Err(bad("signature.json cannot be a UI template"));
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = files.entry(name) {
                entry.insert(read(&view.template)?);
            }
            if files.len() > MAX_FILES - 1 {
                return Err(bad("package has too many files"));
            }
        }
        // Only a missing signature is unsigned; other IO failures must not
        // silently downgrade a signed package to an unsigned one.
        let signature = match std::fs::symlink_metadata(root.join(SIGNATURE)) {
            Ok(_) => Some(
                serde_json::from_slice(&read(SIGNATURE)?)
                    .map_err(|e| bad(format!("{SIGNATURE}: {e}")))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RuntimeError::io(&root.join(SIGNATURE), e)),
        };
        Package::from_files(files, signature)
    }

    /// Reads a `.terrapkg` file.
    pub fn from_zip(bytes: &[u8]) -> Result<Package> {
        if bytes.len() as u64 > MAX_BYTES {
            return Err(bad("package is larger than 32 MB"));
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| bad(format!("not a package: {e}")))?;
        archive
            .validate_header_consistency()
            .map_err(|e| bad(e.to_string()))?;
        if archive.len() > MAX_FILES {
            return Err(bad("package has too many files"));
        }
        let mut files = BTreeMap::new();
        let mut signature = None;
        let mut seen = BTreeSet::new();
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| bad(e.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            let name = clean_path(&entry.name().map_err(|e| bad(e.to_string()))?)?;
            if !seen.insert(name.clone()) {
                return Err(bad("package contains duplicate file paths"));
            }
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
            {
                return Err(bad("package entries must be regular files"));
            }
            if entry.size() > MAX_BYTES - total {
                return Err(bad("package unpacks to more than 32 MB"));
            }
            let mut data = Vec::new();
            entry
                .by_ref()
                .take(MAX_BYTES - total + 1)
                .read_to_end(&mut data)
                .map_err(|e| bad(e.to_string()))?;
            total += data.len() as u64;
            if total > MAX_BYTES {
                return Err(bad("package unpacks to more than 32 MB"));
            }
            if name == SIGNATURE {
                signature = Some(
                    serde_json::from_slice(&data).map_err(|e| bad(format!("{SIGNATURE}: {e}")))?,
                );
            } else {
                files.insert(name, data);
            }
        }
        Package::from_files(files, signature)
    }

    fn from_files(
        files: BTreeMap<String, Vec<u8>>,
        signature: Option<Signature>,
    ) -> Result<Package> {
        let manifest = Manifest::parse(
            files
                .get(MANIFEST)
                .ok_or_else(|| bad("missing manifest.json"))?,
        )?;
        if !files.contains_key(WASM) {
            return Err(bad(format!("missing {WASM}; build the plugin first")));
        }
        for view in manifest.pages.iter().chain(&manifest.widgets) {
            if !files.contains_key(&view.template) {
                return Err(bad(format!("missing template {}", view.template)));
            }
        }
        let package = Package {
            files,
            manifest,
            signature,
        };
        package.verify()?;
        Ok(package)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Hex SHA-256 over every file's path, size and hash.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"terrapkg-v1\n");
        for (path, data) in &self.files {
            hasher.update(format!(
                "{path}\n{}\n{}\n",
                data.len(),
                hex(&Sha256::digest(data))
            ));
        }
        hex(&hasher.finalize())
    }

    fn message(&self) -> Vec<u8> {
        format!("terrapkg-sig-v1:{}", self.digest()).into_bytes()
    }

    /// Signs the package with a 32-byte ed25519 secret key.
    pub fn sign(&mut self, secret: &[u8; 32]) {
        let key = SigningKey::from_bytes(secret);
        let sig = key.sign(&self.message());
        self.signature = Some(Signature {
            key: B64.encode(key.verifying_key().as_bytes()),
            sig: B64.encode(sig.to_bytes()),
        });
    }

    /// The signer's public key, if signed. A present signature has already
    /// been checked against the files.
    pub fn signer(&self) -> Option<&str> {
        self.signature.as_ref().map(|s| s.key.as_str())
    }

    fn verify(&self) -> Result<()> {
        let Some(signature) = &self.signature else {
            return Ok(());
        };
        let key: [u8; 32] = B64
            .decode(&signature.key)
            .ok()
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| bad("signature key is malformed"))?;
        let sig: [u8; 64] = B64
            .decode(&signature.sig)
            .ok()
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| bad("signature is malformed"))?;
        VerifyingKey::from_bytes(&key)
            .map_err(|_| bad("signature key is malformed"))?
            .verify(&self.message(), &EdSignature::from_bytes(&sig))
            .map_err(|_| {
                bad("the signature does not match the files; the package was changed after signing")
            })
    }

    /// A deterministic ZIP of the package, with its signature.
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        let mut entries: Vec<(&str, Vec<u8>)> = self
            .files
            .iter()
            .map(|(p, d)| (p.as_str(), d.clone()))
            .collect();
        if let Some(sig) = &self.signature {
            entries.push((SIGNATURE, serde_json::to_vec_pretty(sig)?));
        }
        for (path, data) in entries {
            out.start_file(path, options)
                .map_err(|e| bad(e.to_string()))?;
            out.write_all(&data).map_err(|e| bad(e.to_string()))?;
        }
        Ok(out.finish().map_err(|e| bad(e.to_string()))?.into_inner())
    }

    /// Writes the files (and signature) under `dir`, replacing nothing
    /// outside it.
    pub fn write_to(&self, dir: &Path) -> Result<()> {
        let write = |rel: &str, data: &[u8]| {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| RuntimeError::io(parent, e))?;
            }
            std::fs::write(&path, data).map_err(|e| RuntimeError::io(&path, e))
        };
        for (rel, data) in &self.files {
            write(rel, data)?;
        }
        if let Some(sig) = &self.signature {
            write(SIGNATURE, &serde_json::to_vec_pretty(sig)?)?;
        }
        Ok(())
    }
}

/// Something a plugin may do beyond keeping its own data, shown to the
/// user before install and when an update asks for more.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Capability {
    pub id: String,
    pub label: String,
}

pub fn capabilities(manifest: &Manifest) -> Vec<Capability> {
    let mut out = Vec::new();
    let mut add = |id: String, label: String| out.push(Capability { id, label });
    for iface in &manifest.uses {
        add(
            format!("uses:{iface}"),
            format!("Read and change data through other plugins' {iface}"),
        );
    }
    for iface in manifest.provides.keys() {
        add(
            format!("provides:{iface}"),
            format!("Offer {iface} to other plugins"),
        );
    }
    if manifest.reminders.is_some() {
        add("notifications".into(), "Show notifications".into());
    }
    if manifest.jobs.values().any(|j| j.run_on == RunOn::Any) {
        add(
            "jobs:any".into(),
            "Run scheduled work on any device, including phones".into(),
        );
    } else if !manifest.jobs.is_empty() {
        add(
            "jobs:desktop".into(),
            "Run scheduled work on your computer".into(),
        );
    }
    for permission in &manifest.permissions {
        match permission.as_str() {
            "speech" => add(
                "speech".into(),
                "Transcribe audio with local speech-to-text".into(),
            ),
            "ai" => add(
                "ai".into(),
                "Send text you choose to your AI provider (Settings → AI)".into(),
            ),
            other => add(other.to_string(), other.to_string()),
        }
    }
    let base = Limits::default();
    let limits = manifest.limits;
    if limits.fuel > base.fuel
        || limits.memory_mb > base.memory_mb
        || limits.timeout_ms > base.timeout_ms
    {
        add(
            format!(
                "limits:{}:{}:{}",
                limits.fuel, limits.memory_mb, limits.timeout_ms
            ),
            format!(
                "Use more resources: up to {} MB and {:.1} s per action",
                limits.memory_mb,
                limits.timeout_ms as f64 / 1000.0
            ),
        );
    }
    out.sort();
    out
}

/// Capabilities in `new` that `old` did not have.
pub fn added_capabilities(old: Option<&Manifest>, new: &Manifest) -> Vec<Capability> {
    let before: Vec<String> = old
        .map(capabilities)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.id)
        .collect();
    capabilities(new)
        .into_iter()
        .filter(|c| !before.contains(&c.id))
        .collect()
}

/// A new ed25519 signing key: (secret, base64 public key).
pub fn generate_signing_key() -> Result<([u8; 32], String)> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| bad(format!("no randomness: {e}")))?;
    let public = B64.encode(SigningKey::from_bytes(&secret).verifying_key().as_bytes());
    Ok((secret, public))
}

/// The base64 public key of a 32-byte secret.
pub fn signing_public_key(secret: &[u8; 32]) -> String {
    B64.encode(SigningKey::from_bytes(secret).verifying_key().as_bytes())
}

/// `$TERRA_SIGNING_KEY`, else `~/.config/terra/signing.key`.
pub fn default_signing_key_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("TERRA_SIGNING_KEY") {
        return Some(path.into());
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".config/terra/signing.key"))
}

/// Reads a signing key file written by `terra keygen` (64 hex digits).
pub fn read_signing_key(path: &Path) -> Result<[u8; 32]> {
    let text = std::fs::read_to_string(path).map_err(|e| RuntimeError::io(path, e))?;
    let text = text.trim();
    let not_key = || bad(format!("{} is not a signing key", path.display()));
    if text.len() != 64 || !text.is_ascii() {
        return Err(not_key());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| not_key())?;
    }
    Ok(out)
}

/// Short, readable form of a public key for the UI.
pub fn key_fingerprint(key: &str) -> String {
    let hash = hex(&Sha256::digest(key.as_bytes()));
    hash.as_bytes()[..16]
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("dist")).unwrap();
        std::fs::write(
            dir.path().join(MANIFEST),
            include_bytes!("../tests/fixtures/limits/manifest.json"),
        )
        .unwrap();
        std::fs::write(dir.path().join(WASM), b"fixture bytes").unwrap();
        dir
    }

    #[test]
    fn signed_folder_and_zip_preserve_identity_and_detect_tampering() {
        let dir = fixture();
        let mut package = Package::from_dir(dir.path()).unwrap();
        package.sign(&[7; 32]);
        let copy = Package::from_zip(&package.to_zip().unwrap()).unwrap();
        assert_eq!(copy.signer(), package.signer());
        assert_eq!(copy.digest(), package.digest());
        package.write_to(dir.path()).unwrap();
        std::fs::write(dir.path().join(WASM), b"tampered").unwrap();
        assert!(Package::from_dir(dir.path())
            .unwrap_err()
            .to_string()
            .contains("signature does not match"));
    }

    #[test]
    fn package_paths_have_one_unambiguous_spelling() {
        for path in ["../x", "/x", "./x", "a//b", "a/./b", "a/../b", "a\\b", "a/"] {
            assert!(clean_path(path).is_err(), "{path}");
        }
        assert!(clean_path("ui/widget.json").is_ok());
    }

    #[test]
    fn duplicate_zip_entries_are_rejected_before_manifest_parsing() {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["one.txt", "two.txt"] {
            out.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(b"x").unwrap();
        }
        let mut bytes = out.finish().unwrap().into_inner();
        // Forge duplicate names in both the local and central directory headers.
        for start in 0..bytes.len() - 6 {
            if &bytes[start..start + 7] == b"two.txt" {
                bytes[start..start + 7].copy_from_slice(b"one.txt");
            }
        }
        let error = Package::from_zip(&bytes).unwrap_err().to_string();
        assert!(error.contains("duplicate"), "{error}");
    }

    #[test]
    fn folder_size_is_bounded_and_signature_io_errors_do_not_mean_unsigned() {
        let dir = fixture();
        std::fs::create_dir(dir.path().join(SIGNATURE)).unwrap();
        assert!(Package::from_dir(dir.path()).is_err());
        std::fs::remove_dir(dir.path().join(SIGNATURE)).unwrap();
        std::fs::File::create(dir.path().join(WASM))
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(Package::from_dir(dir.path())
            .unwrap_err()
            .to_string()
            .contains("32 MB"));
    }

    #[cfg(unix)]
    #[test]
    fn folder_symlinks_cannot_read_outside_the_package() {
        let dir = fixture();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::remove_file(dir.path().join(WASM)).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(WASM)).unwrap();
        assert!(Package::from_dir(dir.path())
            .unwrap_err()
            .to_string()
            .contains("escapes"));
    }
}
