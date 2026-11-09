//! Device keys, the space key and its recovery phrase, and wrapping the
//! space key for a newly paired device.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use terrarium_core::DeviceId;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::error::{Result, SyncError};

pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("operating system random source");
    bytes
}

pub(crate) mod b64 {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(
        bytes: &[u8; N],
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> std::result::Result<[u8; N], D::Error> {
        let text = String::deserialize(d)?;
        let bytes = B64.decode(text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom(format!("expected {N} bytes")))
    }
}

pub(crate) mod b64_vec {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
        B64.decode(String::deserialize(d)?)
            .map_err(serde::de::Error::custom)
    }
}

/// A device's secret keys. Stored only on that device.
#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceKeys {
    pub device: DeviceId,
    #[serde(with = "b64")]
    signing: [u8; 32],
    #[serde(with = "b64")]
    exchange: [u8; 32],
}

impl std::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

/// What other devices know about a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePublic {
    pub device: DeviceId,
    #[serde(with = "b64")]
    pub signing: [u8; 32],
    #[serde(with = "b64")]
    pub exchange: [u8; 32],
    /// Human label, e.g. "MacBook" or "Android phone".
    #[serde(default)]
    pub name: String,
}

/// The devices allowed to publish into a space.
pub type Roster = BTreeMap<DeviceId, DevicePublic>;

impl DeviceKeys {
    pub fn generate(device: DeviceId) -> Self {
        DeviceKeys {
            device,
            signing: random(),
            exchange: random(),
        }
    }

    pub fn public(&self, name: &str) -> DevicePublic {
        DevicePublic {
            device: self.device.clone(),
            signing: SigningKey::from_bytes(&self.signing)
                .verifying_key()
                .to_bytes(),
            exchange: PublicKey::from(&StaticSecret::from(self.exchange)).to_bytes(),
            name: name.to_string(),
        }
    }

    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(&self.signing)
            .sign(message)
            .to_bytes()
    }

    /// Unwraps a space key that another device wrapped for this device.
    pub fn unwrap_space_key(&self, wrapped: &WrappedKey) -> Result<SpaceKey> {
        let shared =
            StaticSecret::from(self.exchange).diffie_hellman(&PublicKey::from(wrapped.ephemeral));
        if !shared.was_contributory() {
            return Err(SyncError::Key("invalid key-exchange public key".into()));
        }
        let cipher = wrap_cipher(shared.as_bytes())?;
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&wrapped.nonce),
                AeadPayload {
                    msg: &wrapped.ciphertext,
                    aad: wrap_aad(&self.device).as_bytes(),
                },
            )
            .map_err(|_| SyncError::Key("space key could not be unwrapped".into()))?;
        let bytes: [u8; 32] = plain
            .try_into()
            .map_err(|_| SyncError::Key("wrapped key has wrong length".into()))?;
        Ok(SpaceKey(bytes))
    }
}

pub(crate) fn verify(public: &DevicePublic, message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(&public.signing)
        .map(|key| {
            key.verify_strict(message, &Signature::from_bytes(signature))
                .is_ok()
        })
        .unwrap_or(false)
}

/// The symmetric key every device of a space shares.
#[derive(Clone, PartialEq, Eq)]
pub struct SpaceKey(pub(crate) [u8; 32]);

impl std::fmt::Debug for SpaceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpaceKey(..)")
    }
}

/// A space key encrypted to one device's exchange key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WrappedKey {
    pub device: DeviceId,
    #[serde(with = "b64")]
    pub ephemeral: [u8; 32],
    #[serde(with = "b64")]
    pub nonce: [u8; 24],
    #[serde(with = "b64_vec")]
    pub ciphertext: Vec<u8>,
}

fn wrap_cipher(shared: &[u8; 32]) -> Result<XChaCha20Poly1305> {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, shared)
        .expand(b"terrarium space-key wrap v1", &mut key)
        .map_err(|_| SyncError::Key("key derivation failed".into()))?;
    Ok(XChaCha20Poly1305::new((&key).into()))
}

fn wrap_aad(device: &DeviceId) -> String {
    format!("pp-wrap-v1|{device}")
}

impl SpaceKey {
    pub fn generate() -> Self {
        SpaceKey(random())
    }

    /// The 24-word recovery phrase; it is the key itself, so anyone holding
    /// it can read the space.
    pub fn recovery_phrase(&self) -> String {
        bip39::Mnemonic::from_entropy(&self.0)
            .expect("32 bytes is valid bip39 entropy")
            .to_string()
    }

    pub fn from_recovery_phrase(phrase: &str) -> Result<Self> {
        let normalized = phrase
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mnemonic =
            bip39::Mnemonic::parse(&normalized).map_err(|e| SyncError::Phrase(e.to_string()))?;
        let entropy = mnemonic.to_entropy();
        let bytes: [u8; 32] = entropy
            .try_into()
            .map_err(|_| SyncError::Phrase("expected a 24-word phrase".into()))?;
        Ok(SpaceKey(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        SpaceKey(bytes)
    }

    /// Wraps this key for `recipient` with a fresh ephemeral X25519 key.
    pub fn wrap_for(&self, recipient: &DevicePublic) -> Result<WrappedKey> {
        let ephemeral = StaticSecret::from(random::<32>());
        let shared = ephemeral.diffie_hellman(&PublicKey::from(recipient.exchange));
        if !shared.was_contributory() {
            return Err(SyncError::Key("invalid key-exchange public key".into()));
        }
        let nonce: [u8; 24] = random();
        let ciphertext = wrap_cipher(shared.as_bytes())?
            .encrypt(
                XNonce::from_slice(&nonce),
                AeadPayload {
                    msg: &self.0,
                    aad: wrap_aad(&recipient.device).as_bytes(),
                },
            )
            .map_err(|_| SyncError::Key("wrapping failed".into()))?;
        Ok(WrappedKey {
            device: recipient.device.clone(),
            ephemeral: PublicKey::from(&ephemeral).to_bytes(),
            nonce,
            ciphertext,
        })
    }

    pub(crate) fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_phrase_round_trips() {
        let key = SpaceKey::generate();
        let phrase = key.recovery_phrase();
        assert_eq!(phrase.split(' ').count(), 24);
        let messy = format!("  {}  ", phrase.to_uppercase().replace(' ', "   "));
        assert_eq!(SpaceKey::from_recovery_phrase(&messy).unwrap(), key);
        assert!(SpaceKey::from_recovery_phrase("abandon abandon").is_err());
    }

    #[test]
    fn wrapped_key_opens_only_for_its_device() {
        let key = SpaceKey::generate();
        let phone = DeviceKeys::generate("phone".into());
        let other = DeviceKeys::generate("other".into());
        let wrapped = key.wrap_for(&phone.public("Phone")).unwrap();
        assert_eq!(phone.unwrap_space_key(&wrapped).unwrap(), key);
        assert!(other.unwrap_space_key(&wrapped).is_err());

        let mut tampered = wrapped.clone();
        tampered.ciphertext[0] ^= 1;
        assert!(phone.unwrap_space_key(&tampered).is_err());
    }

    #[test]
    fn noncontributory_exchange_keys_are_rejected_before_wrapping_or_unwrapping() {
        let key = SpaceKey::generate();
        let keys = DeviceKeys::generate("test".into());
        let mut public = keys.public("Test");
        public.exchange = [0; 32];
        assert!(key.wrap_for(&public).is_err());
        let mut wrapped = key.wrap_for(&keys.public("Test")).unwrap();
        wrapped.ephemeral = [0; 32];
        assert!(keys.unwrap_space_key(&wrapped).is_err());
    }

    #[test]
    fn device_keys_serialize_and_sign() {
        let keys = DeviceKeys::generate("mac".into());
        let restored: DeviceKeys =
            serde_json::from_str(&serde_json::to_string(&keys).unwrap()).unwrap();
        let public = restored.public("Mac");
        assert_eq!(public, keys.public("Mac"));
        let sig = keys.sign(b"hello");
        assert!(verify(&public, b"hello", &sig));
        assert!(!verify(&public, b"hellp", &sig));
        assert!(!format!("{keys:?}").contains(&B64.encode(keys.signing)));
    }
}
