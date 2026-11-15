mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_core::{FileRef, StoreSet};
use terrarium_runtime::blobs::BlobKey;
use terrarium_runtime::{build_plugin, HostConfig, PluginHost};

fn todo_dir() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = support::package_root()
                .join("tasks")
                .canonicalize()
                .unwrap();
            build_plugin(&dir).unwrap_or_else(|e| panic!("building todo: {e}"));
            dir
        })
        .clone()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "src")
            {
                continue;
            }
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn todo_with_file_column(root: &Path) -> PathBuf {
    let from = todo_dir();
    let to = root.join("todo");
    for entry in walk(&from) {
        let dest = to.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&entry, dest).unwrap();
    }
    let path = to.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["storage"]["tables"][0]["columns"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"attachment","type":"file"}));
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    // Only the host schema changed; walk copied the already-built component.
    to
}

fn host(data: &TempDir) -> PluginHost {
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "Asia/Kolkata".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    host.load(&todo_with_file_column(data.path())).unwrap();
    host.set_blob_key(BlobKey::new("space", &[9u8; 32]));
    host
}

fn values(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

#[test]
fn referenced_blobs_include_trashed_rows_and_markdown() {
    let data = TempDir::new().unwrap();
    let host = &mut host(&data);
    let blobs = host.blob_store();
    let mut blob_store = blobs.lock().unwrap();
    let md = blob_store.put(b"markdown").unwrap();
    let file = blob_store.put(b"file").unwrap();
    let trashed = blob_store.put(b"trashed").unwrap();
    drop(blob_store);

    let file_json = FileRef {
        blob: file.clone(),
        name: "paper.txt".into(),
        mime: "text/plain".into(),
        size: 4,
    }
    .to_json_string()
    .unwrap();
    let trashed_markdown = format!("old ![x](terrarium:blob/{trashed})");
    host.store_mut("todo")
        .unwrap()
        .write(|w| {
            w.insert(
                "tasks",
                &values(json!({
                    "title": "Live",
                    "notes": format!("see terrarium:blob/{md}"),
                    "attachment": file_json,
                })),
            )?;
            let gone = w.insert(
                "tasks",
                &values(json!({
                    "title": "Trash me",
                    "notes": trashed_markdown,
                })),
            )?;
            w.delete("tasks", &gone)?;
            Ok(())
        })
        .unwrap();

    let refs = host.referenced_blob_ids();
    assert!(refs.contains(&md.to_string()));
    assert!(refs.contains(&file.to_string()));
    assert!(refs.contains(&trashed.to_string()));
}

#[test]
fn gc_waits_for_grace_period() {
    let data = TempDir::new().unwrap();
    let host = &mut host(&data);
    let id = host.blob_store().lock().unwrap().put(b"orphan").unwrap();

    let first = host.blob_gc_collect(1_000);
    assert_eq!(first.deleted, 0);
    assert!(host.blob_store().lock().unwrap().has(&id));

    let before = host.blob_gc_collect(1_000 + 30 * DAY_MS - 1);
    assert_eq!(before.deleted, 0);
    assert!(host.blob_store().lock().unwrap().has(&id));

    let after = host.blob_gc_collect(1_000 + 30 * DAY_MS + 1);
    assert_eq!(after.deleted, 1);
    assert!(!host.blob_store().lock().unwrap().has(&id));
}

#[test]
fn gc_keeps_blobs_referenced_again() {
    let data = TempDir::new().unwrap();
    let host = &mut host(&data);
    let id = host.blob_store().lock().unwrap().put(b"returning").unwrap();
    let note = format!("terrarium:blob/{id}");
    let row = host
        .store_mut("todo")
        .unwrap()
        .write(|w| w.insert("tasks", &values(json!({"title":"A","notes":note}))))
        .unwrap();

    host.store_mut("todo")
        .unwrap()
        .write(|w| w.update("tasks", &row, &values(json!({"notes":"none"}))))
        .unwrap();
    let marked = host.blob_gc_collect(5_000);
    assert_eq!(marked.deleted, 0);
    assert!(host.blob_store().lock().unwrap().has(&id));

    let restored = format!("back terrarium:blob/{id}");
    host.store_mut("todo")
        .unwrap()
        .write(|w| w.update("tasks", &row, &values(json!({"notes":restored}))))
        .unwrap();
    let after = host.blob_gc_collect(5_000 + 31 * DAY_MS);
    assert_eq!(after.deleted, 0);
    assert!(host.blob_store().lock().unwrap().has(&id));
}
