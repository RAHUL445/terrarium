//! Push and pull for one device of one space.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use terrarium_core::{Batch, DeviceId, StoreSet, VersionVector};

use crate::envelope::{Envelope, Kind, Payload};
use crate::error::{Result, SyncError};
use crate::keys::{DeviceKeys, Roster, SpaceKey};
use crate::transport::{BatchStore, FolderStore};

/// What this device has published and received. Saved after every step so
/// a crash at any point only causes harmless re-delivery.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncState {
    /// Highest sequence number used. Bumped before publishing, so a number
    /// is never reused even if the device crashes mid-publish.
    pub last_seq: u64,
    /// Per plugin: local change-log position already published.
    pub sent: BTreeMap<String, i64>,
    /// Per plugin: the version vector last published, so acknowledgements
    /// are re-sent when they change even without new changes.
    pub sent_seen: BTreeMap<String, VersionVector>,
    /// Per remote device: highest sequence number applied.
    pub received: BTreeMap<DeviceId, u64>,
    /// Set when a received batch applied new changes, so the next push
    /// tells the host even if this device has nothing new itself. Batches
    /// that only acknowledge or repeat changes do not set it, so two
    /// devices never ping-pong acknowledgements.
    #[serde(default)]
    pub acks_pending: bool,
    /// What each peer reported it has applied (from its own batches).
    #[serde(default)]
    pub peer_received: BTreeMap<DeviceId, BTreeMap<DeviceId, u64>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rejected {
    pub device: DeviceId,
    pub seq: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub pushed: Option<u64>,
    pub batches: usize,
    pub applied: usize,
    pub duplicates: usize,
    /// Changes waiting for a newer plugin schema, or for a plugin this
    /// device does not have installed yet.
    pub pending: usize,
    /// Batches that failed verification. Later batches from the same device
    /// are not applied until it is resolved, to keep causal order.
    pub rejected: Vec<Rejected>,
    /// Devices publishing into the store that are not in the roster.
    pub unknown_devices: Vec<DeviceId>,
    /// Set when this device started from another device's snapshot.
    pub bootstrapped_from: Option<DeviceId>,
}

pub struct SpaceSync {
    space: String,
    keys: DeviceKeys,
    key: SpaceKey,
    roster: Roster,
    state_path: PathBuf,
    pending_dir: PathBuf,
    state: SyncState,
}

impl SpaceSync {
    /// `dir` holds this device's sync state for the space.
    pub fn open(
        space: &str,
        keys: DeviceKeys,
        key: SpaceKey,
        roster: Roster,
        dir: &Path,
    ) -> Result<SpaceSync> {
        std::fs::create_dir_all(dir).map_err(|e| SyncError::io(dir, e))?;
        let state_path = dir.join("sync-state.json");
        let state = match std::fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SyncState::default(),
            Err(e) => return Err(SyncError::io(&state_path, e)),
        };
        Ok(SpaceSync {
            space: space.to_string(),
            keys,
            key,
            roster,
            state_path,
            pending_dir: dir.join("pending-plugins"),
            state,
        })
    }

    pub fn device(&self) -> &DeviceId {
        &self.keys.device
    }

    pub fn state(&self) -> &SyncState {
        &self.state
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub fn set_roster(&mut self, roster: Roster) {
        self.roster = roster;
    }

    /// Pull, then push (so this device also relays what it just received).
    pub fn sync(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
    ) -> Result<SyncReport> {
        let mut report = self.pull(stores, transport)?;
        report.pushed = self.push(stores, transport)?;
        Ok(report)
    }

    /// Publishes everything new in this device's logs as one batch.
    pub fn push(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
    ) -> Result<Option<u64>> {
        let mut batches = Vec::new();
        let mut cursors = Vec::new();
        for plugin in stores.plugin_ids() {
            let Some(store) = stores.store_mut(&plugin) else {
                continue;
            };
            let after = self.state.sent.get(&plugin).copied().unwrap_or(0);
            let (batch, cursor) = store.export_since(after)?;
            let ack_changed = self.state.sent_seen.get(&plugin) != Some(&batch.sender_seen);
            if batch.changes.is_empty() && !ack_changed {
                continue;
            }
            cursors.push((plugin, cursor, batch.sender_seen.clone()));
            batches.push(batch);
        }
        if batches.is_empty() && !self.state.acks_pending {
            return Ok(None);
        }

        let seq = self.state.last_seq + 1;
        self.state.last_seq = seq;
        self.save()?;
        let received = self.state.received.clone();
        let envelope = Envelope::seal(
            &self.space,
            seq,
            &Payload {
                batches,
                received: received.clone(),
            },
            &self.key,
            &self.keys,
        )?;
        transport.put(&self.keys.device, seq, &envelope.to_bytes()?)?;
        self.state.acks_pending = false;
        for (plugin, cursor, seen) in cursors {
            self.state.sent.insert(plugin.clone(), cursor);
            self.state.sent_seen.insert(plugin, seen);
        }
        self.save()?;
        Ok(Some(seq))
    }

    /// Applies every new batch from every paired device.
    pub fn pull(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        self.replay_pending(stores, &mut report)?;
        if self.state.received.is_empty() && self.state.last_seq == 0 {
            self.bootstrap(stores, transport, &mut report)?;
        }
        for device in transport.devices()? {
            if device == self.keys.device {
                continue;
            }
            if !self.roster.contains_key(&device) {
                report.unknown_devices.push(device);
                continue;
            }
            let after = self.state.received.get(&device).copied().unwrap_or(0);
            for seq in transport.list(&device, after)? {
                match self.receive(stores, transport, &device, seq, &mut report) {
                    Ok(()) => {
                        self.state.received.insert(device.clone(), seq);
                        self.save()?;
                        report.batches += 1;
                    }
                    Err(SyncError::Rejected { reason, .. }) => {
                        report.rejected.push(Rejected {
                            device: device.clone(),
                            seq,
                            reason,
                        });
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(report)
    }

    fn receive(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
        device: &DeviceId,
        seq: u64,
        report: &mut SyncReport,
    ) -> Result<()> {
        let reject = |reason: &str| SyncError::Rejected {
            device: device.to_string(),
            seq,
            reason: reason.into(),
        };
        let bytes = transport.get(device, seq)?;
        let envelope = Envelope::from_bytes(&bytes).map_err(|_| reject("not a batch file"))?;
        if &envelope.device != device || envelope.seq != seq {
            return Err(reject("batch is stored under the wrong device or number"));
        }
        if envelope.kind != Kind::Batch {
            return Err(reject("not a batch"));
        }
        let payload = envelope.open(&self.space, &self.key, &self.roster)?;
        self.note_acks(device, &payload.received);
        for batch in payload.batches {
            match stores.store_mut(&batch.plugin) {
                Some(store) => {
                    let imported = store.import(&batch)?;
                    if imported.applied > 0 {
                        self.state.acks_pending = true;
                    }
                    add(report, imported)
                }
                None => {
                    self.park(&batch, device, seq)?;
                    report.pending += batch.changes.len();
                }
            }
        }
        Ok(())
    }

    fn note_acks(&mut self, device: &DeviceId, received: &BTreeMap<DeviceId, u64>) {
        let acks = self.state.peer_received.entry(device.clone()).or_default();
        for (origin, seq) in received {
            let entry = acks.entry(origin.clone()).or_insert(0);
            *entry = (*entry).max(*seq);
        }
    }

    /// Publishes this device's whole state as one snapshot: every plugin's
    /// full log plus batches kept for plugins not installed here. Covers
    /// this device's batches up to `last_seq` and what it has received.
    pub fn snapshot(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
    ) -> Result<BTreeMap<DeviceId, u64>> {
        let mut batches = Vec::new();
        for plugin in stores.plugin_ids() {
            if let Some(store) = stores.store_mut(&plugin) {
                batches.push(store.export_since(0)?.0);
            }
        }
        batches.extend(self.parked()?.into_iter().map(|(_, batch)| batch));
        let mut covers = self.state.received.clone();
        covers.insert(self.keys.device.clone(), self.state.last_seq);
        let payload = Payload {
            batches,
            received: covers.clone(),
        };
        let envelope = Envelope::seal_kind(
            Kind::Snapshot,
            &self.space,
            self.state.last_seq,
            &payload,
            &self.key,
            &self.keys,
        )?;
        transport.put_snapshot(&self.keys.device, &envelope.to_bytes()?)?;
        Ok(covers)
    }

    /// A fresh device starts from the most complete snapshot in the store
    /// instead of replaying every batch (some may already be compacted).
    fn bootstrap(
        &mut self,
        stores: &mut dyn StoreSet,
        transport: &dyn BatchStore,
        report: &mut SyncReport,
    ) -> Result<()> {
        let mut best: Option<(u64, DeviceId, Payload)> = None;
        for device in transport
            .devices()?
            .into_iter()
            .chain(self.roster.keys().cloned())
        {
            if device == self.keys.device || best.as_ref().is_some_and(|(_, d, _)| *d == device) {
                continue;
            }
            let Some(bytes) = transport.snapshot(&device)? else {
                continue;
            };
            let opened = Envelope::from_bytes(&bytes).and_then(|envelope| {
                if envelope.kind != Kind::Snapshot || envelope.device != device {
                    return Err(SyncError::Rejected {
                        device: device.to_string(),
                        seq: 0,
                        reason: "not a snapshot".into(),
                    });
                }
                envelope.open(&self.space, &self.key, &self.roster)
            });
            match opened {
                Ok(payload) => {
                    let total: u64 = payload.received.values().sum();
                    if best.as_ref().is_none_or(|(t, _, _)| total > *t) {
                        best = Some((total, device, payload));
                    }
                }
                Err(SyncError::Rejected { reason, .. }) => {
                    report.rejected.push(Rejected {
                        device: device.clone(),
                        seq: 0,
                        reason,
                    });
                }
                Err(e) => return Err(e),
            }
        }
        let Some((_, from, payload)) = best else {
            return Ok(());
        };
        for (i, batch) in payload.batches.iter().enumerate() {
            match stores.store_mut(&batch.plugin) {
                Some(store) => add(report, store.import(batch)?),
                None => {
                    self.park_as(batch, &format!("{}-snapshot-{i:06}", from.0))?;
                    report.pending += batch.changes.len();
                }
            }
        }
        for (device, seq) in payload.received {
            if device != self.keys.device {
                self.state.received.insert(device, seq);
            }
        }
        self.save()?;
        report.bootstrapped_from = Some(from);
        Ok(())
    }

    /// On the device hosting the folder: publishes a fresh snapshot, then
    /// deletes batches that the snapshot covers and every paired device has
    /// confirmed applying. Returns how many batches were removed.
    pub fn compact(&mut self, stores: &mut dyn StoreSet, folder: &FolderStore) -> Result<usize> {
        let covers = self.snapshot(stores, folder)?;
        let me = self.keys.device.clone();
        let mut removed = 0;
        for device in folder.devices()? {
            let covered = covers.get(&device).copied().unwrap_or(0);
            for seq in folder.list(&device, 0)? {
                if seq > covered {
                    break;
                }
                let everyone_has_it = self.roster.keys().filter(|r| **r != device).all(|r| {
                    if *r == me {
                        self.state.received.get(&device).is_some_and(|s| *s >= seq)
                    } else {
                        self.state
                            .peer_received
                            .get(r)
                            .and_then(|acks| acks.get(&device))
                            .is_some_and(|s| *s >= seq)
                    }
                });
                if !everyone_has_it {
                    break;
                }
                folder.remove(&device, seq)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn parked(&self) -> Result<Vec<(PathBuf, Batch)>> {
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(&self.pending_dir) else {
            return Ok(out);
        };
        for dir in dirs.flatten() {
            let Ok(entries) = std::fs::read_dir(dir.path()) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            files.sort();
            for path in files {
                let bytes = std::fs::read(&path).map_err(|e| SyncError::io(&path, e))?;
                out.push((path, serde_json::from_slice(&bytes)?));
            }
        }
        Ok(out)
    }

    /// Keeps a batch for a plugin this device does not have yet.
    fn park(&self, batch: &Batch, device: &DeviceId, seq: u64) -> Result<()> {
        self.park_as(batch, &format!("{}-{seq:020}", device.0))
    }

    fn park_as(&self, batch: &Batch, name: &str) -> Result<()> {
        let dir = self.pending_dir.join(safe_name(&batch.plugin));
        std::fs::create_dir_all(&dir).map_err(|e| SyncError::io(&dir, e))?;
        let path = dir.join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_vec(batch)?).map_err(|e| SyncError::io(&path, e))
    }

    fn replay_pending(&self, stores: &mut dyn StoreSet, report: &mut SyncReport) -> Result<()> {
        for plugin in stores.plugin_ids() {
            let dir = self.pending_dir.join(safe_name(&plugin));
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            files.sort();
            for path in files {
                let bytes = std::fs::read(&path).map_err(|e| SyncError::io(&path, e))?;
                let batch: Batch = serde_json::from_slice(&bytes)?;
                if let Some(store) = stores.store_mut(&plugin) {
                    add(report, store.import(&batch)?);
                    std::fs::remove_file(&path).map_err(|e| SyncError::io(&path, e))?;
                }
            }
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let tmp = self.state_path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.state)?)
            .map_err(|e| SyncError::io(&tmp, e))?;
        std::fs::rename(&tmp, &self.state_path).map_err(|e| SyncError::io(&self.state_path, e))
    }
}

fn add(report: &mut SyncReport, import: terrarium_core::ImportReport) {
    report.applied += import.applied;
    report.duplicates += import.duplicates;
    report.pending += import.pending;
}

fn safe_name(plugin: &str) -> String {
    plugin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
