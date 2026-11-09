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

const VERSION: u8 = 1;
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
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RestoreReport {
    pub applied: usize,
    pub duplicates: usize,
    /// Stores in the backup this device has no plugin for (yet).
    pub missing: Vec<String>,
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
        let mut batches = Vec::new();
        for id in stores.plugin_ids() {
            if let Some(store) = stores.store_mut(&id) {
                batches.push(store.export_since(0)?.0);
            }
        }
        Ok(Backup {
            info: BackupInfo {
                v: VERSION,
                space: space.to_string(),
                created_ms,
                device: device.clone(),
            },
            roster,
            stores: batches,
        })
    }

    pub fn seal(&self, key: &SpaceKey) -> Result<Vec<u8>> {
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

    pub fn info(bytes: &[u8]) -> Result<BackupInfo> {
        let file: BackupFile = serde_json::from_slice(bytes)
            .map_err(|e| SyncError::Key(format!("not a backup file: {e}")))?;
        if file.info.v != VERSION {
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
        if file.info.v != VERSION {
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
        let (roster, stores): (Roster, Vec<Batch>) = serde_json::from_slice(&json)?;
        Ok(Backup {
            info: file.info,
            roster,
            stores,
        })
    }

    /// Imports every store this device has. Safe to repeat: run it again
    /// after plugins that were missing have been installed.
    pub fn restore(&self, stores: &mut dyn StoreSet) -> Result<RestoreReport> {
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
