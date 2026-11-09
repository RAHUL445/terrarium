//! Packages: signing, safe unpacking, capability diffs, install, update,
//! rollback across a schema change, the installed set syncing, and themes.

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_runtime::{
    added_capabilities, build_plugin, generate_signing_key, HostConfig, Manifest, Package,
    PackageSource, PluginHost, Theme,
};

fn todo_dir() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = support::package_root().join("todo").canonicalize().unwrap();
            build_plugin(&dir).unwrap();
            dir
        })
        .clone()
}

/// A copy of To-do with an edited manifest.
fn variant(root: &TempDir, name: &str, edit: impl FnOnce(&mut Value)) -> PathBuf {
    let dir = root.path().join(name);
    Package::from_dir(&todo_dir())
        .unwrap()
        .write_to(&dir)
        .unwrap();
    let path = dir.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut manifest);
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    dir
}

fn host(root: &TempDir, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: root.path().join(device),
        spaces_dir: None,
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap()
}

#[test]
fn signed_packages_round_trip_and_tampering_is_caught() {
    let root = TempDir::new().unwrap();
    let (secret, public) = generate_signing_key().unwrap();
    let mut package = Package::from_dir(&todo_dir()).unwrap();
    let unsigned_digest = package.digest();
    package.sign(&secret);
    assert_eq!(
        package.digest(),
        unsigned_digest,
        "signing does not change the digest"
    );

    let zip = package.to_zip().unwrap();
    assert_eq!(zip, package.to_zip().unwrap(), "zips are deterministic");
    let read = Package::from_zip(&zip).unwrap();
    assert_eq!(read.digest(), unsigned_digest);
    assert_eq!(read.signer(), Some(public.as_str()));

    // Changing a file after signing breaks the signature.
    let dir = root.path().join("tampered");
    read.write_to(&dir).unwrap();
    let wasm = dir.join("dist/plugin.wasm");
    let mut bytes = std::fs::read(&wasm).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&wasm, bytes).unwrap();
    let err = Package::from_dir(&dir).unwrap_err().to_string();
    assert!(err.contains("signature does not match"), "{err}");
}

#[test]
fn zips_with_paths_outside_the_package_are_refused() {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    out.start_file("../escape.txt", options).unwrap();
    out.write_all(b"x").unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let err = Package::from_zip(&bytes).unwrap_err().to_string();
    assert!(err.contains("not a plain path"), "{err}");
    assert!(Package::from_zip(b"not a zip").is_err());
}

#[test]
fn updates_list_only_new_capabilities() {
    let old = Package::from_dir(&todo_dir()).unwrap().manifest().clone();
    let mut json = serde_json::to_value(&old).unwrap();
    json["uses"] = json!(["notes-api"]);
    json["permissions"] = json!(["speech"]);
    let new: Manifest = serde_json::from_value(json).unwrap();
    let added: Vec<String> = added_capabilities(Some(&old), &new)
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(
        added,
        vec!["speech".to_string(), "uses:notes-api".to_string()]
    );
    assert!(added_capabilities(Some(&old), &old).is_empty());
    // Fresh installs list everything.
    assert!(!added_capabilities(None, &old).is_empty());
}

#[test]
fn install_update_and_roll_back_across_a_schema_change() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "laptop");
    let v1 = Package::from_dir(&todo_dir()).unwrap();
    let source = PackageSource::Folder("test".into());

    let installed = host.packages_mut().install(&v1, &source, None).unwrap();
    let dir = host.packages().materialize(&installed).unwrap().unwrap();
    host.load(&dir).unwrap();
    let task = host
        .call("todo", "create", &json!({"title": "Before update"}))
        .unwrap();

    // v2 adds a column (the next storage version).
    let next_schema = v1.manifest().storage.version + 1;
    let v2_dir = variant(&root, "v2", |m| {
        m["version"] = json!("9.0.0");
        m["storage"]["version"] = json!(next_schema);
        m["storage"]["tables"][0]["columns"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "estimate", "type": "integer"}));
    });
    let v2 = Package::from_dir(&v2_dir).unwrap();
    let current = (v1.manifest().version.clone(), v1.digest());
    let installed = host
        .packages_mut()
        .install(&v2, &source, Some((&current.0, &current.1)))
        .unwrap();
    assert_eq!(installed.version, "9.0.0");
    assert_eq!(
        installed.previous_digest.as_deref(),
        Some(current.1.as_str())
    );
    let dir = host.packages().materialize(&installed).unwrap().unwrap();
    assert_eq!(host.load(&dir).unwrap().version, "9.0.0");

    // Back to v1: the database keeps the newer schema and the data.
    let rolled = host.packages_mut().rollback("todo").unwrap();
    assert_eq!(rolled.version, v1.manifest().version);
    assert_eq!(rolled.previous_version.as_deref(), Some("9.0.0"));
    let dir = host.packages().materialize(&rolled).unwrap().unwrap();
    host.load(&dir).unwrap();
    assert_eq!(host.store("todo").unwrap().schema().version, next_schema);
    let rows = host
        .store("todo")
        .unwrap()
        .query(
            "SELECT title FROM tasks WHERE id = ?1",
            &[task["id"].clone()],
        )
        .unwrap();
    assert_eq!(rows[0]["title"], json!("Before update"));
    host.call("todo", "create", &json!({"title": "After rollback"}))
        .unwrap();
}

#[test]
fn the_installed_set_and_package_files_sync_to_other_devices() {
    let root = TempDir::new().unwrap();
    let mut laptop = host(&root, "laptop");
    let mut phone = host(&root, "phone");
    let package = Package::from_dir(&todo_dir()).unwrap();
    laptop
        .packages_mut()
        .install(&package, &PackageSource::File("todo.terrapkg".into()), None)
        .unwrap();
    let (key, name) = ("a2V5".to_string(), "Me".to_string());
    laptop.packages_mut().trust(&key, &name).unwrap();

    let (batch, _) = laptop.packages().store().export_since(0).unwrap();
    phone.packages_mut().store_mut().import(&batch).unwrap();

    let installed = phone.packages().get("todo").unwrap().unwrap();
    assert_eq!(installed.digest, package.digest());
    let dir = phone.packages().materialize(&installed).unwrap().unwrap();
    phone.load(&dir).unwrap();
    phone
        .call("todo", "create", &json!({"title": "Arrived"}))
        .unwrap();
    assert!(phone.packages().is_trusted(&key).unwrap());

    // Removing on one device removes everywhere; the data stays.
    phone
        .packages_mut()
        .set_removed("todo", true, None)
        .unwrap();
    let (batch, _) = phone.packages().store().export_since(0).unwrap();
    laptop.packages_mut().store_mut().import(&batch).unwrap();
    assert!(laptop.packages().get("todo").unwrap().unwrap().removed);
}

#[test]
fn themes_install_update_sync_and_remove() {
    let root = TempDir::new().unwrap();
    let mut laptop = host(&root, "laptop");
    let mut phone = host(&root, "phone");
    let theme = |version: &str, bg: &str| {
        Theme::parse(
            json!({ "id": "forest", "name": "Forest", "version": version, "light": { "bg": bg } })
                .to_string()
                .as_bytes(),
        )
        .unwrap()
    };
    assert_eq!(
        laptop
            .packages_mut()
            .install_theme(&theme("1.0.0", "#f3f7f2"))
            .unwrap(),
        None
    );
    let replaced = laptop
        .packages_mut()
        .install_theme(&theme("1.1.0", "#eef5ec"))
        .unwrap();
    assert_eq!(replaced.as_deref(), Some("1.0.0"));
    assert_eq!(
        laptop.packages().themes().unwrap(),
        vec![theme("1.1.0", "#eef5ec")]
    );

    let (batch, _) = laptop.packages().store().export_since(0).unwrap();
    phone.packages_mut().store_mut().import(&batch).unwrap();
    assert_eq!(phone.packages().themes().unwrap()[0].version, "1.1.0");

    phone.packages_mut().remove_theme("forest").unwrap();
    assert!(phone.packages_mut().remove_theme("forest").is_err());
    let (batch, _) = phone.packages().store().export_since(0).unwrap();
    laptop.packages_mut().store_mut().import(&batch).unwrap();
    assert!(laptop.packages().themes().unwrap().is_empty());
}
