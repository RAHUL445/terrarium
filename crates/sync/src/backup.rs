//! Encrypted backups: every store's whole change log in one file, sealed
//! with the space key, so the 24-word recovery phrase is all that is needed
//! to read it back.
//!
//! Restoring imports the changes instead of replacing databases. Changes
//! already present are skipped and later edits keep winning by timestamp,
//! so restoring an old backup never undoes newer work, and restored data
//! syncs to the other devices like any other change.

use std::collections::BTreeMap;
use std::io::Read;

use chacha20poly1305::aead::{Aead, Payload as AeadPayload};
use chacha20poly1305::XNonce;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use terrarium_core::{Batch, DeviceId, StoreSet};

use crate::error::{Result, SyncError};
use crate::keys::{b64, b64_vec, random, Roster, SpaceKey};

const VERSION_V1: u8 = 1;
const VERSION_V2: u8 = 2;
pub const BACKUP_EXT: &str = "terrabak";

/// What a backup file shows without the key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupInfo {
    pub v: u8,
    pub space: String,
    pub created_ms: u64,
    pub device: DeviceId,
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    #[serde(flatten)]
    info: BackupInfo,
    #[serde(with = "b64")]
    nonce: [u8; 24],
    #[serde(with = "b64_vec")]
    ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub info: BackupInfo,
    /// The space's devices when the backup was made, so a restored host
    /// still accepts batches from them.
    pub roster: Roster,
    pub stores: Vec<Batch>,
    #[serde(default)]
    pub blobs: Vec<BlobEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobEntry {
    pub id: String,
    #[serde(with = "b64_vec")]
    pub sealed: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct BackupV2Payload {
    roster: Roster,
    stores: Vec<Batch>,
    #[serde(default)]
    blobs: Vec<BlobEntry>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RestoreReport {
    pub applied: usize,
    pub duplicates: usize,
    /// Stores in the backup this device has no plugin for (yet).
    pub missing: Vec<String>,
}

pub trait BackupBlobs {
    fn list_blob_ids(&self) -> Result<Vec<String>>;
    fn get_sealed(&self, id: &str) -> Result<Vec<u8>>;
    fn put_sealed(&self, id: &str, sealed: &[u8]) -> Result<()>;
}

struct NoBlobs;

impl BackupBlobs for NoBlobs {
    fn list_blob_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn get_sealed(&self, _id: &str) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn put_sealed(&self, _id: &str, _sealed: &[u8]) -> Result<()> {
        Ok(())
    }
}

fn aad(info: &BackupInfo) -> String {
    format!(
        "pp-backup-v{}|{}|{}|{}",
        info.v, info.space, info.device, info.created_ms
    )
}

impl Backup {
    /// Every store's full log.
    pub fn collect(
        space: &str,
        device: &DeviceId,
        created_ms: u64,
        roster: Roster,
        stores: &mut dyn StoreSet,
    ) -> Result<Backup> {
        Self::collect_with_blobs(space, device, created_ms, roster, stores, &NoBlobs)
    }

    pub fn collect_with_blobs(
        space: &str,
        device: &DeviceId,
        created_ms: u64,
        roster: Roster,
        stores: &mut dyn StoreSet,
        blobs: &dyn BackupBlobs,
    ) -> Result<Backup> {
        let mut batches = Vec::new();
        for id in stores.plugin_ids() {
            if let Some(store) = stores.store_mut(&id) {
                batches.push(store.export_since(0)?.0);
            }
        }
        let mut blob_entries = Vec::new();
        for id in blobs.list_blob_ids()? {
            blob_entries.push(BlobEntry {
                id: id.clone(),
                sealed: blobs.get_sealed(&id)?,
            });
        }
        Ok(Backup {
            info: BackupInfo {
                v: VERSION_V2,
                space: space.to_string(),
                created_ms,
                device: device.clone(),
            },
            roster,
            stores: batches,
            blobs: blob_entries,
        })
    }

    pub fn seal(&self, key: &SpaceKey) -> Result<Vec<u8>> {
        let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        let payload = BackupV2Payload {
            roster: self.roster.clone(),
            stores: self.stores.clone(),
            blobs: self.blobs.clone(),
        };
        serde_json::to_writer(&mut encoder, &payload)?;
        let plain = encoder
            .finish()
            .map_err(|e| SyncError::Transport(format!("compress: {e}")))?;
        let nonce: [u8; 24] = random();
        let ciphertext = key
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                AeadPayload {
                    msg: &plain,
                    aad: aad(&self.info).as_bytes(),
                },
            )
            .map_err(|_| SyncError::Key("encryption failed".into()))?;
        Ok(serde_json::to_vec(&BackupFile {
            info: self.info.clone(),
            nonce,
            ciphertext,
        })?)
    }

    /// Legacy backup format used before blobs were included (v1).
    pub fn seal_v1_compat(&self, key: &SpaceKey) -> Result<Vec<u8>> {
        let info = BackupInfo {
            v: VERSION_V1,
            space: self.info.space.clone(),
            created_ms: self.info.created_ms,
            device: self.info.device.clone(),
        };
        let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut encoder, &(&self.roster, &self.stores))?;
        let plain = encoder
            .finish()
            .map_err(|e| SyncError::Transport(format!("compress: {e}")))?;
        let nonce: [u8; 24] = random();
        let ciphertext = key
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                AeadPayload {
                    msg: &plain,
                    aad: aad(&info).as_bytes(),
                },
            )
            .map_err(|_| SyncError::Key("encryption failed".into()))?;
        Ok(serde_json::to_vec(&BackupFile {
            info,
            nonce,
            ciphertext,
        })?)
    }

    pub fn info(bytes: &[u8]) -> Result<BackupInfo> {
        let file: BackupFile = serde_json::from_slice(bytes)
            .map_err(|e| SyncError::Key(format!("not a backup file: {e}")))?;
        if !matches!(file.info.v, VERSION_V1 | VERSION_V2) {
            return Err(SyncError::Key(format!(
                "backup version {} is not supported",
                file.info.v
            )));
        }
        Ok(file.info)
    }

    pub fn open(bytes: &[u8], key: &SpaceKey) -> Result<Backup> {
        let file: BackupFile = serde_json::from_slice(bytes)
            .map_err(|e| SyncError::Key(format!("not a backup file: {e}")))?;
        if !matches!(file.info.v, VERSION_V1 | VERSION_V2) {
            return Err(SyncError::Key(format!(
                "backup version {} is not supported",
                file.info.v
            )));
        }
        let plain = key
            .cipher()
            .decrypt(
                XNonce::from_slice(&file.nonce),
                AeadPayload {
                    msg: &file.ciphertext,
                    aad: aad(&file.info).as_bytes(),
                },
            )
            .map_err(|_| {
                SyncError::Key("this backup was made with a different recovery phrase".into())
            })?;
        let mut json = Vec::new();
        DeflateDecoder::new(plain.as_slice())
            .read_to_end(&mut json)
            .map_err(|e| SyncError::Transport(format!("decompress: {e}")))?;
        let (roster, stores, blobs) = match file.info.v {
            VERSION_V1 => {
                let (roster, stores): (Roster, Vec<Batch>) = serde_json::from_slice(&json)?;
                (roster, stores, Vec::new())
            }
            VERSION_V2 => {
                let payload: BackupV2Payload = serde_json::from_slice(&json)?;
                (payload.roster, payload.stores, payload.blobs)
            }
            _ => unreachable!("checked above"),
        };
        Ok(Backup {
            info: file.info,
            roster,
            stores,
            blobs,
        })
    }

    /// Imports every store this device has. Safe to repeat: run it again
    /// after plugins that were missing have been installed.
    pub fn restore(&self, stores: &mut dyn StoreSet) -> Result<RestoreReport> {
        self.restore_with_blobs(stores, &NoBlobs)
    }

    pub fn restore_with_blobs(
        &self,
        stores: &mut dyn StoreSet,
        blobs: &dyn BackupBlobs,
    ) -> Result<RestoreReport> {
        for blob in &self.blobs {
            blobs.put_sealed(&blob.id, &blob.sealed)?;
        }
        let mut report = RestoreReport::default();
        for batch in &self.stores {
            let Some(store) = stores.store_mut(&batch.plugin) else {
                report.missing.push(batch.plugin.clone());
                continue;
            };
            // Not an acknowledgement from the backed-up device: purge
            // decisions must not rely on it.
            let batch = Batch {
                sender_seen: Default::default(),
                ..batch.clone()
            };
            let imported = store.import(&batch)?;
            report.applied += imported.applied;
            report.duplicates += imported.duplicates;
        }
        Ok(report)
    }

    /// Changes per store, for showing what a backup holds.
    pub fn counts(&self) -> BTreeMap<String, usize> {
        self.stores
            .iter()
            .map(|b| (b.plugin.clone(), b.changes.len()))
            .collect()
    }
}
