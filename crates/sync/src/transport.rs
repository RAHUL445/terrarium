//! Where batches travel: anything that can put, list and get opaque files.
//! Every device writes only under its own `devices/<id>/`, so stores never
//! see write conflicts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use terrarium_core::DeviceId;

use crate::error::{Result, SyncError};

pub trait BatchStore {
    /// Devices that have published anything.
    fn devices(&self) -> Result<Vec<DeviceId>>;
    /// Sequence numbers published by `device` after `after`, ascending.
    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>>;
    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>>;
    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()>;
    /// The latest snapshot `device` published, if any.
    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>>;
    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()>;
}

fn write_atomic(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| SyncError::io(dir, e))?;
    let path = dir.join(name);
    // Write then rename so readers never see a partial file.
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| SyncError::io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| SyncError::io(&path, e))
}

pub(crate) fn check_device(device: &DeviceId) -> Result<()> {
    let ok = !device.0.is_empty()
        && device.0.len() <= 64
        && device
            .0
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(SyncError::Transport(format!(
            "device id `{device}` is not a safe name"
        )))
    }
}

/// A directory: local, network share, or a folder another tool syncs.
pub struct FolderStore {
    root: PathBuf,
}

impl FolderStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FolderStore { root: root.into() }
    }

    fn device_dir(&self, device: &DeviceId) -> Result<PathBuf> {
        check_device(device)?;
        Ok(self.root.join("devices").join(&device.0))
    }
}

impl BatchStore for FolderStore {
    fn devices(&self) -> Result<Vec<DeviceId>> {
        let dir = self.root.join("devices");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(SyncError::io(&dir, e)),
        };
        let mut out: Vec<DeviceId> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(DeviceId::from))
            .filter(|d| check_device(d).is_ok())
            .collect();
        out.sort();
        Ok(out)
    }

    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>> {
        let dir = self.device_dir(device)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(SyncError::io(&dir, e)),
        };
        let mut seqs: Vec<u64> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".batch")?.parse().ok())
            .filter(|seq| *seq > after)
            .collect();
        seqs.sort_unstable();
        Ok(seqs)
    }

    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>> {
        let path = self.device_dir(device)?.join(format!("{seq:020}.batch"));
        std::fs::read(&path).map_err(|e| SyncError::io(&path, e))
    }

    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        write_atomic(
            &self.device_dir(device)?,
            &format!("{seq:020}.batch"),
            bytes,
        )
    }

    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>> {
        check_device(device)?;
        let path = self
            .root
            .join("snapshots")
            .join(format!("{}.snapshot", device.0));
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SyncError::io(&path, e)),
        }
    }

    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        check_device(device)?;
        write_atomic(
            &self.root.join("snapshots"),
            &format!("{}.snapshot", device.0),
            bytes,
        )
    }
}

impl FolderStore {
    /// Deletes one batch (compaction, on the device hosting the folder).
    pub fn remove(&self, device: &DeviceId, seq: u64) -> Result<()> {
        let path = self.device_dir(device)?.join(format!("{seq:020}.batch"));
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SyncError::io(&path, e)),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore {
    files: Mutex<BTreeMap<(DeviceId, u64), Vec<u8>>>,
    snapshots: Mutex<BTreeMap<DeviceId, Vec<u8>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets a test tamper with a stored batch.
    pub fn replace(&self, device: &DeviceId, seq: u64, bytes: Vec<u8>) {
        self.files
            .lock()
            .unwrap()
            .insert((device.clone(), seq), bytes);
    }
}

impl BatchStore for MemoryStore {
    fn devices(&self) -> Result<Vec<DeviceId>> {
        let files = self.files.lock().unwrap();
        let mut out: Vec<DeviceId> = files.keys().map(|(d, _)| d.clone()).collect();
        out.dedup();
        Ok(out)
    }

    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>> {
        let files = self.files.lock().unwrap();
        Ok(files
            .keys()
            .filter(|(d, s)| d == device && *s > after)
            .map(|(_, s)| *s)
            .collect())
    }

    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(&(device.clone(), seq))
            .cloned()
            .ok_or_else(|| SyncError::Transport(format!("no batch {device}/{seq}")))
    }

    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        check_device(device)?;
        self.files
            .lock()
            .unwrap()
            .insert((device.clone(), seq), bytes.to_vec());
        Ok(())
    }

    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>> {
        Ok(self.snapshots.lock().unwrap().get(device).cloned())
    }

    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        check_device(device)?;
        self.snapshots
            .lock()
            .unwrap()
            .insert(device.clone(), bytes.to_vec());
        Ok(())
    }
}

/// Several stores used together, e.g. the host device and Google Drive.
/// Reads take the union (so a batch reaches you through whichever store is
/// up); writes go to every store and succeed if at least one does. Every
/// device publishes the same signed bytes under the same number, so stores
/// never disagree about content.
pub struct MultiStore {
    stores: Vec<(String, Box<dyn BatchStore + Send + Sync>)>,
    errors: Mutex<BTreeMap<String, String>>,
}

impl MultiStore {
    pub fn new(stores: Vec<(String, Box<dyn BatchStore + Send + Sync>)>) -> Self {
        MultiStore {
            stores,
            errors: Mutex::new(BTreeMap::new()),
        }
    }

    /// Stores that failed during the last calls, with the error.
    pub fn errors(&self) -> BTreeMap<String, String> {
        self.errors.lock().unwrap().clone()
    }

    fn each<T>(&self, mut f: impl FnMut(&dyn BatchStore) -> Result<T>) -> Result<Vec<T>> {
        let mut ok = Vec::new();
        let mut last = None;
        for (name, store) in &self.stores {
            match f(store.as_ref()) {
                Ok(value) => {
                    self.errors.lock().unwrap().remove(name);
                    ok.push(value);
                }
                Err(e) => {
                    self.errors
                        .lock()
                        .unwrap()
                        .insert(name.clone(), e.to_string());
                    last = Some(e);
                }
            }
        }
        match (ok.is_empty(), last) {
            (true, Some(e)) => Err(e),
            _ => Ok(ok),
        }
    }
}

impl BatchStore for MultiStore {
    fn devices(&self) -> Result<Vec<DeviceId>> {
        let mut all: Vec<DeviceId> = self.each(|s| s.devices())?.into_iter().flatten().collect();
        all.sort();
        all.dedup();
        Ok(all)
    }

    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>> {
        let mut all: Vec<u64> = self
            .each(|s| s.list(device, after))?
            .into_iter()
            .flatten()
            .collect();
        all.sort_unstable();
        all.dedup();
        Ok(all)
    }

    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>> {
        let mut last = None;
        for (_, store) in &self.stores {
            match store.get(device, seq) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| SyncError::Transport("no stores".into())))
    }

    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        self.each(|s| s.put(device, seq, bytes)).map(|_| ())
    }

    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>> {
        Ok(self
            .each(|s| s.snapshot(device))?
            .into_iter()
            .flatten()
            .next())
    }

    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        self.each(|s| s.put_snapshot(device, bytes)).map(|_| ())
    }
}
