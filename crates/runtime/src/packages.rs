//! The installed plugin set of a space. It lives in a host-owned store,
//! `_packages`, so it syncs like plugin data: install or update on one
//! device and the others follow.
//!
//! - `installed`: one row per plugin id with the active version and the
//!   one before it, for rollback. Updated last-writer-wins.
//! - `blobs`: package files (`.terrapkg`, base64) by digest, so devices that
//!   did not install a package can still load it.
//! - `signers`: publisher keys the user trusts.
//! - `themes`: installed themes (see [`Theme`]), as checked JSON.
//!
//! Packages shipped inside the app have the digest [`BUNDLED_DIGEST`].

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Map, Value};
use terrarium_core::{PluginSchema, Store as CoreStore, StoreConfig, SystemTime};

use crate::error::{Result, RuntimeError};
use crate::package::{Package, MAX_PACKAGE_BYTES};
use crate::theme::Theme;

pub const STORE_ID: &str = "_packages";
/// Digest standing for "the version shipped inside the app".
pub const BUNDLED_DIGEST: &str = "bundled";

const SCHEMA: &str = r#"{
  "version": 2,
  "tables": [
    { "name": "installed", "columns": [
      { "name": "version", "type": "text" },
      { "name": "digest", "type": "text" },
      { "name": "previous_version", "type": "text" },
      { "name": "previous_digest", "type": "text" },
      { "name": "removed", "type": "boolean", "default": false },
      { "name": "source", "type": "text" },
      { "name": "updated_ms", "type": "integer" }
    ]},
    { "name": "blobs", "columns": [
      { "name": "plugin", "type": "text" },
      { "name": "version", "type": "text" },
      { "name": "data", "type": "text" }
    ]},
    { "name": "signers", "columns": [
      { "name": "key", "type": "text" },
      { "name": "name", "type": "text" }
    ]},
    { "name": "themes", "columns": [
      { "name": "version", "type": "text" },
      { "name": "data", "type": "text" },
      { "name": "updated_ms", "type": "integer" }
    ]}
  ]
}"#;

/// Where a package came from, shown in the UI.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "location")]
pub enum PackageSource {
    Folder(String),
    File(String),
    Git(String),
}

impl PackageSource {
    fn describe(&self) -> String {
        match self {
            PackageSource::Folder(p) => format!("folder {p}"),
            PackageSource::File(p) => format!("file {p}"),
            PackageSource::Git(u) => format!("git {u}"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub digest: String,
    pub previous_version: Option<String>,
    pub previous_digest: Option<String>,
    pub removed: bool,
    pub source: Option<String>,
    pub updated_ms: Option<i64>,
}

pub struct Packages {
    store: CoreStore,
    /// Verified caches: `<root>/<id>/<full digest>-<signer fingerprint>/`.
    root: PathBuf,
    approvals: PathBuf,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

impl Packages {
    pub fn open(data_dir: &Path, cfg: StoreConfig) -> Result<Packages> {
        let schema: PluginSchema = serde_json::from_str(SCHEMA)?;
        let store = CoreStore::open(
            data_dir.join(format!("{STORE_ID}.db")),
            StoreConfig {
                plugin: STORE_ID.into(),
                ..cfg
            },
            schema,
            Box::new(SystemTime),
        )?;
        Ok(Packages {
            store,
            root: data_dir.join("packages"),
            approvals: data_dir.join("package-approvals.json"),
        })
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    /// Every plugin with an install record, including removed ones.
    pub fn installed(&self) -> Result<Vec<Installed>> {
        let rows = self.store.query(
            "SELECT id, version, digest, previous_version, previous_digest, removed, source, updated_ms \
             FROM installed WHERE _deleted IS NULL ORDER BY id",
            &[],
        )?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(Installed {
                    id: text(row, "id")?,
                    version: text(row, "version")?,
                    digest: text(row, "digest")?,
                    previous_version: text(row, "previous_version"),
                    previous_digest: text(row, "previous_digest"),
                    removed: row
                        .get("removed")
                        .is_some_and(|v| v.as_bool().unwrap_or(v.as_i64().unwrap_or(0) != 0)),
                    source: text(row, "source"),
                    updated_ms: row.get("updated_ms").and_then(Value::as_i64),
                })
            })
            .collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<Installed>> {
        Ok(self.installed()?.into_iter().find(|i| i.id == id))
    }

    /// Makes `package` the active version of its plugin. `current` is the
    /// version running now (`(version, digest)`), kept for rollback.
    pub fn install(
        &mut self,
        package: &Package,
        source: &PackageSource,
        current: Option<(&str, &str)>,
    ) -> Result<Installed> {
        let manifest = package.manifest();
        let digest = package.digest();
        // Signatures are outside the file digest. Preserve a newly reviewed
        // signer even when the payload files have not changed.
        let encoded = B64.encode(package.to_zip()?);
        let existing = self.store.query(
            "SELECT data FROM blobs WHERE id = ?1 AND _deleted IS NULL",
            &[json!(digest)],
        )?;
        let data = if existing
            .first()
            .and_then(|r| r.get("data"))
            .and_then(Value::as_str)
            == Some(encoded.as_str())
        {
            None
        } else {
            Some(encoded)
        };
        let mut values = Map::new();
        values.insert("version".into(), json!(manifest.version));
        values.insert("digest".into(), json!(digest));
        values.insert("removed".into(), json!(false));
        values.insert("source".into(), json!(source.describe()));
        values.insert("updated_ms".into(), json!(now_ms()));
        if let Some((version, previous)) = current.filter(|(_, d)| *d != digest) {
            values.insert("previous_version".into(), json!(version));
            values.insert("previous_digest".into(), json!(previous));
        }
        self.store.write(|w| {
            if let Some(data) = &data {
                let mut blob = Map::new();
                blob.insert("plugin".into(), json!(manifest.id));
                blob.insert("version".into(), json!(manifest.version));
                blob.insert("data".into(), json!(data));
                w.upsert("blobs", &digest, &blob)?;
            }
            w.upsert("installed", &manifest.id, &values)
        })?;
        self.get(&manifest.id)?
            .ok_or_else(|| RuntimeError::Package("install record missing".into()))
    }

    /// Swaps the active and previous versions.
    pub fn rollback(&mut self, id: &str) -> Result<Installed> {
        let current = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Package(format!("`{id}` has no install record")))?;
        let (Some(version), Some(digest)) = (&current.previous_version, &current.previous_digest)
        else {
            return Err(RuntimeError::Package(format!(
                "`{id}` has no earlier version to roll back to"
            )));
        };
        let mut values = Map::new();
        values.insert("version".into(), json!(version));
        values.insert("digest".into(), json!(digest));
        values.insert("previous_version".into(), json!(current.version));
        values.insert("previous_digest".into(), json!(current.digest));
        values.insert("updated_ms".into(), json!(now_ms()));
        self.store.write(|w| w.update("installed", id, &values))?;
        self.get(id)?
            .ok_or_else(|| RuntimeError::Package("install record missing".into()))
    }

    /// Removes the plugin from every device (its data stays). `current`
    /// creates the record for a plugin that only ran bundled until now.
    pub fn set_removed(
        &mut self,
        id: &str,
        removed: bool,
        current: Option<(&str, &str)>,
    ) -> Result<()> {
        let mut values = Map::new();
        values.insert("removed".into(), json!(removed));
        values.insert("updated_ms".into(), json!(now_ms()));
        if self.get(id)?.is_none() {
            let (version, digest) =
                current.ok_or_else(|| RuntimeError::Package(format!("`{id}` is not installed")))?;
            values.insert("version".into(), json!(version));
            values.insert("digest".into(), json!(digest));
        }
        self.store.write(|w| w.upsert("installed", id, &values))?;
        Ok(())
    }

    /// The stored package with `digest`, checked against it.
    pub fn blob(&self, digest: &str) -> Result<Option<Package>> {
        let rows = self.store.query(
            "SELECT data FROM blobs WHERE id = ?1 AND _deleted IS NULL",
            &[json!(digest)],
        )?;
        let Some(data) = rows.first().and_then(|r| text(r, "data")) else {
            return Ok(None);
        };
        if data.len() as u64 > MAX_PACKAGE_BYTES.div_ceil(3) * 4 {
            return Err(RuntimeError::Package("stored package exceeds 32 MB".into()));
        }
        let bytes = B64
            .decode(data)
            .map_err(|e| RuntimeError::Package(format!("stored package {digest}: {e}")))?;
        let package = Package::from_zip(&bytes)?;
        if package.digest() != digest {
            return Err(RuntimeError::Package(format!(
                "stored package {digest} does not match its digest"
            )));
        }
        Ok(Some(package))
    }

    fn approval_id(package: &Package) -> String {
        format!(
            "{}:{}:{}",
            package.manifest().id,
            package.digest(),
            package.signer().unwrap_or("unsigned")
        )
    }

    fn approvals(&self) -> Result<BTreeSet<String>> {
        let file = match std::fs::File::open(&self.approvals) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
            Err(e) => return Err(RuntimeError::io(&self.approvals, e)),
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| RuntimeError::io(&self.approvals, e))?;
        if bytes.len() > 1024 * 1024 {
            return Err(RuntimeError::Package(
                "local approval file is too large".into(),
            ));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Records explicit consent on this device. Never synced with package data.
    /// The identity includes the signer because a package digest excludes its signature.
    pub fn approve_local(&mut self, package: &Package) -> Result<()> {
        let mut approved = self.approvals()?;
        approved.insert(Self::approval_id(package));
        let bytes = serde_json::to_vec(&approved)?;
        if bytes.len() > 1024 * 1024 {
            return Err(RuntimeError::Package("local approval file is full".into()));
        }
        let parent = self.approvals.parent().unwrap();
        let mut tmp =
            tempfile::NamedTempFile::new_in(parent).map_err(|e| RuntimeError::io(parent, e))?;
        tmp.write_all(&bytes)
            .map_err(|e| RuntimeError::io(tmp.path(), e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| RuntimeError::io(tmp.path(), e))?;
        tmp.persist(&self.approvals)
            .map_err(|e| RuntimeError::io(&self.approvals, e.error))?;
        Ok(())
    }

    /// Approves a still-current synced version without changing the shared
    /// install record or rollback history. A review cannot approve a stale version.
    pub fn approve_synced(&mut self, package: &Package) -> Result<()> {
        let current = self
            .get(&package.manifest().id)?
            .ok_or_else(|| RuntimeError::Package("plugin was removed; review again".into()))?;
        if current.removed
            || current.digest != package.digest()
            || current.version != package.manifest().version
        {
            return Err(RuntimeError::Package(
                "installed version changed; review again".into(),
            ));
        }
        let stored = self
            .blob(&current.digest)?
            .ok_or_else(|| RuntimeError::Package("package has not arrived".into()))?;
        if stored.signer() != package.signer() {
            return Err(RuntimeError::Package(
                "publisher changed; review again".into(),
            ));
        }
        self.approve_local(package)
    }

    /// Production activation entry point: sync/rollback/cache must all pass
    /// signature, current publisher trust and device-local exact-package consent.
    pub fn materialize_approved(
        &self,
        installed: &Installed,
        development: bool,
    ) -> Result<Option<PathBuf>> {
        if installed.digest == BUNDLED_DIGEST {
            return Ok(None);
        }
        let package = self.installed_package(installed)?;
        match package.signer() {
            None if !development => {
                return Err(RuntimeError::Package(
                    "unsigned plugin blocked in production; install a signed package".into(),
                ))
            }
            Some(key) if !self.is_trusted(key)? => {
                return Err(RuntimeError::Package(
                    "publisher is not trusted; review this plugin in Settings → Plugins".into(),
                ))
            }
            _ => {}
        }
        if !self.approvals()?.contains(&Self::approval_id(&package)) {
            return Err(RuntimeError::Package(
                "review this plugin version in Settings → Plugins before running it on this device"
                    .into(),
            ));
        }
        self.materialize_package(installed, &package).map(Some)
    }

    fn installed_package(&self, installed: &Installed) -> Result<Package> {
        // Install records are synced input, so validate before constructing paths.
        if installed.id.len() > 63
            || !installed.id.starts_with(|c: char| c.is_ascii_lowercase())
            || !installed
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || installed.digest.len() != 64
            || !installed
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(RuntimeError::Package(
                "invalid installed plugin identity".into(),
            ));
        }
        let package = self.blob(&installed.digest)?.ok_or_else(|| {
            RuntimeError::Package(format!(
                "{} {} has not arrived on this device yet",
                installed.id, installed.version
            ))
        })?;
        if package.manifest().id != installed.id || package.manifest().version != installed.version
        {
            return Err(RuntimeError::Package(
                "stored package identity differs from its install record".into(),
            ));
        }
        Ok(package)
    }

    /// Integrity-checked unpacking for trusted host tooling. This does not grant
    /// execution consent; app activation must use materialize_approved instead.
    pub fn materialize(&self, installed: &Installed) -> Result<Option<PathBuf>> {
        if installed.digest == BUNDLED_DIGEST {
            return Ok(None);
        }
        let package = self.installed_package(installed)?;
        self.materialize_package(installed, &package).map(Some)
    }

    fn materialize_package(&self, installed: &Installed, package: &Package) -> Result<PathBuf> {
        let parent = self.root.join(&installed.id);
        // Check existing redirects before creating any descendant directories.
        for path in [&self.root, &parent] {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(RuntimeError::Package(
                        "plugin cache directories must not be symlinks".into(),
                    ))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(RuntimeError::io(path, e)),
            }
        }
        std::fs::create_dir_all(&parent).map_err(|e| RuntimeError::io(&parent, e))?;
        let signer = crate::package::key_fingerprint(package.signer().unwrap_or("unsigned"));
        let dir = parent.join(format!("{}-{signer}", installed.digest));
        if dir.exists() {
            if std::fs::symlink_metadata(&dir)
                .map_err(|e| RuntimeError::io(&dir, e))?
                .file_type()
                .is_symlink()
            {
                return Err(RuntimeError::Package(
                    "plugin cache must not be a symlink".into(),
                ));
            }
            // A .complete marker alone is not evidence of integrity.
            let cached = Package::from_dir(&dir)?;
            if cached.digest() != package.digest() || cached.signer() != package.signer() {
                return Err(RuntimeError::Package(
                    "plugin cache was changed; activation blocked".into(),
                ));
            }
            return Ok(dir);
        }
        // Fresh exclusive directory avoids predictable staging paths and partial caches.
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&parent)
            .map_err(|e| RuntimeError::io(&parent, e))?;
        package.write_to(staging.path())?;
        std::fs::rename(staging.path(), &dir).map_err(|e| RuntimeError::io(&dir, e))?;
        Ok(dir)
    }

    pub fn signers(&self) -> Result<Vec<(String, String)>> {
        let rows = self.store.query(
            "SELECT key, name FROM signers WHERE _deleted IS NULL ORDER BY name",
            &[],
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| Some((text(r, "key")?, text(r, "name").unwrap_or_default())))
            .collect())
    }

    pub fn is_trusted(&self, key: &str) -> Result<bool> {
        Ok(self.signers()?.iter().any(|(k, _)| k == key))
    }

    /// Trusts packages signed with `key` on every device of the space.
    pub fn trust(&mut self, key: &str, name: &str) -> Result<()> {
        let mut values = Map::new();
        values.insert("key".into(), json!(key));
        values.insert("name".into(), json!(name));
        let row = crate::package::key_fingerprint(key);
        self.store.write(|w| w.upsert("signers", &row, &values))?;
        Ok(())
    }

    /// Installed themes, by name. Parts a newer app wrote that this one
    /// does not know are dropped; rows that still fail the checks are skipped.
    pub fn themes(&self) -> Result<Vec<Theme>> {
        let rows = self
            .store
            .query("SELECT data FROM themes WHERE _deleted IS NULL", &[])?;
        let mut themes: Vec<Theme> = rows
            .iter()
            .filter_map(|r| Theme::from_stored(&text(r, "data")?).ok())
            .collect();
        themes.sort_by_key(|t| t.name.to_lowercase());
        Ok(themes)
    }

    /// Installs `theme` on every device of the space, replacing an
    /// installed theme with the same id. Returns the version it replaced.
    pub fn install_theme(&mut self, theme: &Theme) -> Result<Option<String>> {
        theme.validate()?;
        let previous = self
            .themes()?
            .into_iter()
            .find(|t| t.id == theme.id)
            .map(|t| t.version);
        let mut values = Map::new();
        values.insert("version".into(), json!(theme.version));
        values.insert("data".into(), json!(serde_json::to_string(theme)?));
        values.insert("updated_ms".into(), json!(now_ms()));
        self.store
            .write(|w| w.upsert("themes", &theme.id, &values))?;
        Ok(previous)
    }

    pub fn remove_theme(&mut self, id: &str) -> Result<()> {
        if !self.themes()?.iter().any(|t| t.id == id) {
            return Err(RuntimeError::Theme(format!("`{id}` is not installed")));
        }
        self.store.write(|w| w.delete("themes", id))?;
        Ok(())
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn packages(root: &Path, device: &str) -> Packages {
        let dir = root.join(device);
        std::fs::create_dir_all(&dir).unwrap();
        Packages::open(
            &dir,
            StoreConfig {
                space: "personal".into(),
                plugin: STORE_ID.into(),
                device: device.into(),
                user: "owner".into(),
            },
        )
        .unwrap()
    }

    fn package(root: &Path, version: &str, signer: Option<u8>) -> Package {
        let dir = root.join(format!("fixture-{version}-{}", signer.unwrap_or(0)));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/limits/manifest.json"))
                .unwrap();
        manifest["version"] = json!(version);
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("dist/plugin.wasm"), b"fixture bytes").unwrap();
        let mut package = Package::from_dir(&dir).unwrap();
        if let Some(key) = signer {
            package.sign(&[key; 32]);
        }
        package
    }

    fn install(packages: &mut Packages, package: &Package) -> Installed {
        packages
            .install(
                package,
                &PackageSource::File("fixture.terrapkg".into()),
                None,
            )
            .unwrap()
    }

    #[test]
    fn synced_publisher_trust_does_not_transfer_execution_consent() {
        let root = tempfile::tempdir().unwrap();
        let p = package(root.path(), "1.0.0", Some(7));
        let mut laptop = packages(root.path(), "laptop");
        let mut phone = packages(root.path(), "phone");
        laptop.trust(p.signer().unwrap(), "publisher").unwrap();
        laptop.approve_local(&p).unwrap();
        let installed = install(&mut laptop, &p);
        assert!(laptop.materialize_approved(&installed, false).is_ok());
        let (batch, _) = laptop.store().export_since(0).unwrap();
        phone.store_mut().import(&batch).unwrap();
        assert!(phone.is_trusted(p.signer().unwrap()).unwrap());
        assert!(phone
            .materialize_approved(&installed, false)
            .unwrap_err()
            .to_string()
            .contains("review"));
        phone.approve_local(&p).unwrap();
        assert!(phone.materialize_approved(&installed, false).is_ok());
        drop(phone);
        let phone = packages(root.path(), "phone");
        assert!(
            phone.materialize_approved(&installed, false).is_ok(),
            "consent persists across restarts"
        );
    }

    #[test]
    fn unsigned_updates_and_changed_publishers_cannot_reuse_consent() {
        let root = tempfile::tempdir().unwrap();
        let mut packages = packages(root.path(), "device");
        let p = package(root.path(), "1.0.0", Some(7));
        packages
            .trust(p.signer().unwrap(), "old publisher")
            .unwrap();
        packages.approve_local(&p).unwrap();
        let unsigned = package(root.path(), "1.0.0", None);
        packages.approve_local(&unsigned).unwrap();
        let installed = install(&mut packages, &unsigned);
        assert!(packages
            .materialize_approved(&installed, false)
            .unwrap_err()
            .to_string()
            .contains("unsigned"));
        assert!(packages.materialize_approved(&installed, true).is_ok());
        let replacement = package(root.path(), "1.0.0", Some(8));
        assert_eq!(
            p.digest(),
            replacement.digest(),
            "file digest alone does not bind signer"
        );
        packages
            .trust(replacement.signer().unwrap(), "new publisher")
            .unwrap();
        let installed = install(&mut packages, &replacement);
        assert!(packages
            .materialize_approved(&installed, false)
            .unwrap_err()
            .to_string()
            .contains("review"));
        let update = package(root.path(), "2.0.0", Some(7));
        let installed = install(&mut packages, &update);
        assert!(packages.materialize_approved(&installed, false).is_err());
    }

    #[test]
    fn cached_bytes_are_checked_even_with_a_complete_marker() {
        let root = tempfile::tempdir().unwrap();
        let p = package(root.path(), "1.0.0", Some(7));
        let mut packages = packages(root.path(), "device");
        packages.trust(p.signer().unwrap(), "publisher").unwrap();
        packages.approve_local(&p).unwrap();
        let installed = install(&mut packages, &p);
        let dir = packages
            .materialize_approved(&installed, false)
            .unwrap()
            .unwrap();
        std::fs::write(dir.join(".complete"), b"").unwrap();
        std::fs::write(dir.join("dist/plugin.wasm"), b"tampered").unwrap();
        assert!(packages.materialize_approved(&installed, false).is_err());
        let mut invalid = installed.clone();
        invalid.id = "../escape".into();
        assert!(packages.materialize_approved(&invalid, false).is_err());
        invalid = installed;
        invalid.version = "other".into();
        assert!(packages.materialize_approved(&invalid, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cache_symlinks_do_not_redirect_materialization() {
        let root = tempfile::tempdir().unwrap();
        let p = package(root.path(), "1.0.0", Some(7));
        let mut packages = packages(root.path(), "device");
        let installed = install(&mut packages, &p);
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &packages.root).unwrap();
        assert!(packages.materialize(&installed).is_err());
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
    #[test]
    fn synced_review_preserves_shared_install_history_and_rejects_stale_versions() {
        let root = tempfile::tempdir().unwrap();
        let mut packages = packages(root.path(), "device");
        let first = package(root.path(), "1.0.0", Some(7));
        let installed = install(&mut packages, &first);
        let second = package(root.path(), "2.0.0", Some(7));
        packages
            .install(
                &second,
                &PackageSource::File("update".into()),
                Some((&installed.version, &installed.digest)),
            )
            .unwrap();
        let before = serde_json::to_value(packages.get("limits").unwrap()).unwrap();
        let (_, cursor) = packages.store().export_since(0).unwrap();
        assert!(packages.approve_synced(&first).is_err());
        packages.approve_synced(&second).unwrap();
        assert_eq!(
            serde_json::to_value(packages.get("limits").unwrap()).unwrap(),
            before
        );
        assert!(packages
            .store()
            .export_since(cursor)
            .unwrap()
            .0
            .changes
            .is_empty());
    }
}
