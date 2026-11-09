//! The batch file format: an encrypted, signed envelope.

use chacha20poly1305::aead::{Aead, Payload as AeadPayload};
use chacha20poly1305::XNonce;
use serde::{Deserialize, Serialize};
use terrarium_core::{Batch, DeviceId};

use crate::error::{Result, SyncError};
use crate::keys::{b64, b64_vec, random, verify, DeviceKeys, Roster, SpaceKey};

const VERSION: u8 = 1;

/// What a batch carries once decrypted: one core batch per plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub batches: Vec<Batch>,
    /// The sender's received cursors: highest batch applied per device.
    /// Lets the host tell when every device has a batch, so it can be
    /// compacted away. In a snapshot: what the snapshot covers.
    #[serde(default)]
    pub received: std::collections::BTreeMap<DeviceId, u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Batch,
    /// A device's whole log, so new devices need not replay every batch.
    /// It may carry other devices' batches the sender kept for plugins it
    /// does not have installed.
    Snapshot,
    /// Space settings shared by the host, e.g. Google Drive credentials.
    Config,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    #[serde(default)]
    pub kind: Kind,
    pub space: String,
    pub device: DeviceId,
    pub seq: u64,
    #[serde(with = "b64")]
    nonce: [u8; 24],
    #[serde(with = "b64_vec")]
    ciphertext: Vec<u8>,
    #[serde(with = "b64")]
    signature: [u8; 64],
}

/// Binds the ciphertext to its position: a batch moved to another device's
/// folder, sequence number or space fails to decrypt.
fn header(kind: Kind, space: &str, device: &DeviceId, seq: u64) -> String {
    let kind = match kind {
        Kind::Batch => "batch",
        Kind::Snapshot => "snapshot",
        Kind::Config => "config",
    };
    format!("pp-{kind}-v{VERSION}|{space}|{device}|{seq}")
}

fn signed_bytes(header: &str, nonce: &[u8; 24], ciphertext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + 24 + ciphertext.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(nonce);
    out.extend_from_slice(ciphertext);
    out
}

impl Envelope {
    pub fn seal(
        space: &str,
        seq: u64,
        payload: &Payload,
        key: &SpaceKey,
        sender: &DeviceKeys,
    ) -> Result<Envelope> {
        Self::seal_kind(Kind::Batch, space, seq, payload, key, sender)
    }

    pub fn seal_kind(
        kind: Kind,
        space: &str,
        seq: u64,
        payload: &Payload,
        key: &SpaceKey,
        sender: &DeviceKeys,
    ) -> Result<Envelope> {
        Self::seal_bytes(kind, space, seq, &serde_json::to_vec(payload)?, key, sender)
    }

    /// Seals arbitrary bytes, for kinds whose content is not a [`Payload`].
    pub fn seal_bytes(
        kind: Kind,
        space: &str,
        seq: u64,
        plain: &[u8],
        key: &SpaceKey,
        sender: &DeviceKeys,
    ) -> Result<Envelope> {
        let header = header(kind, space, &sender.device, seq);
        let nonce: [u8; 24] = random();
        let ciphertext = key
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                AeadPayload {
                    msg: plain,
                    aad: header.as_bytes(),
                },
            )
            .map_err(|_| SyncError::Key("encryption failed".into()))?;
        let signature = sender.sign(&signed_bytes(&header, &nonce, &ciphertext));
        Ok(Envelope {
            v: VERSION,
            kind,
            space: space.to_string(),
            device: sender.device.clone(),
            seq,
            nonce,
            ciphertext,
            signature,
        })
    }

    /// Checks the signature against the roster, then decrypts.
    pub fn open(&self, space: &str, key: &SpaceKey, roster: &Roster) -> Result<Payload> {
        let reject = |reason: &str| SyncError::Rejected {
            device: self.device.to_string(),
            seq: self.seq,
            reason: reason.to_string(),
        };
        let plain = self.open_bytes(space, key, roster)?;
        let payload: Payload =
            serde_json::from_slice(&plain).map_err(|_| reject("payload is malformed"))?;
        // A signed batch may only carry changes stamped by devices of this
        // space, and the sender must be the batch's sender.
        for batch in &payload.batches {
            if self.kind == Kind::Batch && batch.sender != self.device {
                return Err(reject("inner batch names another sender"));
            }
            if let Some(change) = batch
                .changes
                .iter()
                .find(|c| !roster.contains_key(&c.hlc.device))
            {
                return Err(reject(&format!(
                    "change from unknown device {}",
                    change.hlc.device
                )));
            }
        }
        Ok(payload)
    }

    /// Checks the signature against the roster and decrypts, without
    /// interpreting the content.
    pub fn open_bytes(&self, space: &str, key: &SpaceKey, roster: &Roster) -> Result<Vec<u8>> {
        let reject = |reason: &str| SyncError::Rejected {
            device: self.device.to_string(),
            seq: self.seq,
            reason: reason.to_string(),
        };
        if self.v != VERSION {
            return Err(reject("unsupported batch version"));
        }
        if self.space != space {
            return Err(reject("batch belongs to another space"));
        }
        let signer = roster
            .get(&self.device)
            .ok_or_else(|| reject("sender is not a paired device"))?;
        let header = header(self.kind, space, &self.device, self.seq);
        if !verify(
            signer,
            &signed_bytes(&header, &self.nonce, &self.ciphertext),
            &self.signature,
        ) {
            return Err(reject("signature does not match"));
        }
        let plain = key
            .cipher()
            .decrypt(
                XNonce::from_slice(&self.nonce),
                AeadPayload {
                    msg: &self.ciphertext,
                    aad: header.as_bytes(),
                },
            )
            .map_err(|_| reject("could not decrypt with this space key"))?;
        Ok(plain)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Envelope> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
