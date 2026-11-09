//! Sealing secrets at rest. A secret is encrypted with XChaCha20-Poly1305
//! under a key derived (HKDF-SHA256) from a root the app supplies: the
//! space key once the device is in a space, so every paired device can open
//! it, or a key kept on this device before that. The key id tells which
//! root sealed a value, so a device can tell "not for me" from "corrupt".

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

const INFO: &[u8] = b"terrarium/secrets/v1";

#[derive(Clone)]
pub struct Sealer {
    key: [u8; 32],
    id: String,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealer({})", self.id)
    }
}

impl Sealer {
    /// `kind` names the root, e.g. `space` or `device`. The id includes a
    /// fingerprint of the derived key, never the key itself.
    pub fn new(kind: &str, root: &[u8; 32]) -> Sealer {
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(None, root)
            .expand(INFO, &mut key)
            .expect("32 bytes is a valid HKDF output length");
        let fingerprint = Sha256::digest(key);
        let id = format!(
            "{kind}:{}",
            fingerprint[..6]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        Sealer { key, id }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// `name` is bound into the ciphertext, so a sealed value cannot be
    /// moved to another secret's row.
    pub fn seal(&self, name: &str, plain: &[u8]) -> String {
        let mut nonce = [0u8; 24];
        getrandom::getrandom(&mut nonce).expect("operating system random source");
        let sealed = XChaCha20Poly1305::new((&self.key).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: name.as_bytes(),
                },
            )
            .expect("encryption with a valid key cannot fail");
        let mut out = nonce.to_vec();
        out.extend(sealed);
        B64.encode(out)
    }

    pub fn open(&self, name: &str, sealed: &str) -> Option<Vec<u8>> {
        let bytes = B64.decode(sealed).ok()?;
        if bytes.len() < 24 {
            return None;
        }
        let (nonce, body) = bytes.split_at(24);
        XChaCha20Poly1305::new((&self.key).into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: name.as_bytes(),
                },
            )
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_binding() {
        let a = Sealer::new("space", &[7; 32]);
        let sealed = a.seal("ai.key", b"sk-secret");
        assert_eq!(a.open("ai.key", &sealed).unwrap(), b"sk-secret");
        assert!(a.open("other", &sealed).is_none());
        assert!(Sealer::new("space", &[8; 32])
            .open("ai.key", &sealed)
            .is_none());
        assert_eq!(a.id(), Sealer::new("space", &[7; 32]).id());
        assert!(!sealed.contains("sk-secret"));
    }
}
