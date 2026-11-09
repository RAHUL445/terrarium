//! Pairing a new device with a one-time code.
//!
//! The host device shows a short code (valid 5 minutes, 5 attempts, used
//! once). The joining device sends its public keys with an HMAC keyed by the
//! code. The host replies with the space id, roster and the space key wrapped
//! to the joiner's exchange key, also authenticated with the code, so each
//! side knows the other typed or showed the same code.

use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{Result, SyncError};
use crate::keys::{random, DeviceKeys, DevicePublic, Roster, SpaceKey, WrappedKey};

const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 10;
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_ATTEMPTS: u32 = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairRequest {
    pub device: DevicePublic,
    pub proof: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairResponse {
    pub space: String,
    pub roster: Roster,
    pub wrapped: WrappedKey,
    pub proof: String,
}

/// A code waiting to be used, on the host.
#[derive(Debug)]
pub struct PairingSession {
    code: String,
    expires: Instant,
    attempts: u32,
}

/// Uppercase, without spaces or dashes; `O`/`I`/`L`/`0`/`1` never appear.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `ABCDE-FGHJK` for display.
pub fn display_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

fn mac(code: &str, label: &str, parts: &[&[u8]]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_bytes()).expect("any key length works");
    mac.update(label.as_bytes());
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    mac
}

fn sign(code: &str, label: &str, parts: &[&[u8]]) -> String {
    B64.encode(mac(code, label, parts).finalize().into_bytes())
}

fn check(code: &str, label: &str, parts: &[&[u8]], proof: &str) -> bool {
    let Ok(bytes) = B64.decode(proof) else {
        return false;
    };
    mac(code, label, parts).verify_slice(&bytes).is_ok()
}

impl PairingSession {
    pub fn new() -> Self {
        let code: String = random::<CODE_LEN>()
            .iter()
            .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
            .collect();
        PairingSession {
            code,
            expires: Instant::now() + CODE_TTL,
            attempts: 0,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn remaining(&self) -> Duration {
        self.expires.saturating_duration_since(Instant::now())
    }

    /// Checks a request. Counts the attempt either way; the caller discards
    /// the session after success or when this returns `Err` for exhaustion.
    pub fn verify(&mut self, request: &PairRequest) -> Result<()> {
        if Instant::now() >= self.expires {
            return Err(SyncError::Pairing("the pairing code has expired".into()));
        }
        if self.attempts >= MAX_ATTEMPTS {
            return Err(SyncError::Pairing(
                "too many attempts; start pairing again".into(),
            ));
        }
        self.attempts += 1;
        let device = serde_json::to_vec(&request.device)?;
        if !check(&self.code, "pp-pair-request-v1", &[&device], &request.proof) {
            return Err(SyncError::Pairing("the pairing code does not match".into()));
        }
        Ok(())
    }

    /// Builds the host's answer once `verify` passed.
    pub fn respond(
        &self,
        space: &str,
        roster: &Roster,
        key: &SpaceKey,
        joiner: &DevicePublic,
    ) -> Result<PairResponse> {
        let wrapped = key.wrap_for(joiner)?;
        let proof = response_proof(&self.code, space, roster, &wrapped)?;
        Ok(PairResponse {
            space: space.to_string(),
            roster: roster.clone(),
            wrapped,
            proof,
        })
    }
}

impl Default for PairingSession {
    fn default() -> Self {
        Self::new()
    }
}

fn response_proof(
    code: &str,
    space: &str,
    roster: &Roster,
    wrapped: &WrappedKey,
) -> Result<String> {
    let roster = serde_json::to_vec(roster)?;
    let wrapped = serde_json::to_vec(wrapped)?;
    Ok(sign(
        code,
        "pp-pair-response-v1",
        &[space.as_bytes(), &roster, &wrapped],
    ))
}

/// The joiner's request for `code`.
pub fn request(code: &str, device: &DevicePublic) -> Result<PairRequest> {
    let code = normalize_code(code);
    let bytes = serde_json::to_vec(device)?;
    Ok(PairRequest {
        device: device.clone(),
        proof: sign(&code, "pp-pair-request-v1", &[&bytes]),
    })
}

/// What the joiner keeps after a successful pairing.
pub struct Joined {
    pub space: String,
    pub roster: Roster,
    pub key: SpaceKey,
}

/// Checks the host's answer and unwraps the space key.
pub fn accept(code: &str, keys: &DeviceKeys, response: &PairResponse) -> Result<Joined> {
    let code = normalize_code(code);
    let expected = response_proof(&code, &response.space, &response.roster, &response.wrapped)?;
    let ok = B64
        .decode(&response.proof)
        .ok()
        .zip(B64.decode(&expected).ok())
        .is_some_and(|(a, b)| {
            a.len() == b.len() && a.iter().zip(&b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        });
    if !ok {
        return Err(SyncError::Pairing(
            "the other device did not prove it holds the code".into(),
        ));
    }
    if !response.roster.get(&keys.device).is_some_and(|public| {
        let own = keys.public(&public.name);
        own.signing == public.signing && own.exchange == public.exchange
    }) {
        return Err(SyncError::Pairing(
            "the host did not add this device".into(),
        ));
    }
    let key = keys.unwrap_space_key(&response.wrapped)?;
    Ok(Joined {
        space: response.space.clone(),
        roster: response.roster.clone(),
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_roster(host: &DeviceKeys) -> Roster {
        [(host.device.clone(), host.public("Mac"))]
            .into_iter()
            .collect()
    }

    #[test]
    fn matching_code_pairs_and_hands_over_the_key() {
        let host = DeviceKeys::generate("mac".into());
        let phone = DeviceKeys::generate("phone".into());
        let key = SpaceKey::generate();
        let mut session = PairingSession::new();
        assert_eq!(session.code().len(), CODE_LEN);

        let typed = display_code(session.code()).to_lowercase();
        let req = request(&typed, &phone.public("Phone")).unwrap();
        session.verify(&req).unwrap();
        let mut roster = host_roster(&host);
        roster.insert(phone.device.clone(), req.device.clone());
        let resp = session
            .respond("personal", &roster, &key, &req.device)
            .unwrap();

        let joined = accept(&typed, &phone, &resp).unwrap();
        assert_eq!(joined.key, key);
        assert_eq!(joined.roster.len(), 2);
    }

    #[test]
    fn wrong_code_is_refused_and_attempts_run_out() {
        let phone = DeviceKeys::generate("phone".into());
        let mut session = PairingSession::new();
        let wrong = request("AAAAA-AAAAA", &phone.public("Phone")).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(session.verify(&wrong).is_err());
        }
        let right = request(session.code(), &phone.public("Phone")).unwrap();
        let err = session.verify(&right).unwrap_err();
        assert!(err.to_string().contains("too many attempts"));
    }

    #[test]
    fn joiner_rejects_a_host_that_does_not_know_the_code() {
        let host = DeviceKeys::generate("mac".into());
        let phone = DeviceKeys::generate("phone".into());
        let impostor = PairingSession::new();
        let mut roster = host_roster(&host);
        roster.insert(phone.device.clone(), phone.public("Phone"));
        let resp = impostor
            .respond(
                "personal",
                &roster,
                &SpaceKey::generate(),
                &phone.public("Phone"),
            )
            .unwrap();
        assert!(accept("BBBBB-BBBBB", &phone, &resp).is_err());
    }

    #[test]
    fn joiner_rejects_a_code_authenticated_roster_with_different_own_keys() {
        let host = DeviceKeys::generate("host".into());
        let phone = DeviceKeys::generate("phone".into());
        let other = DeviceKeys::generate("phone".into());
        let session = PairingSession::new();
        let mut roster = host_roster(&host);
        roster.insert(phone.device.clone(), other.public("Phone"));
        let response = session
            .respond(
                "space",
                &roster,
                &SpaceKey::generate(),
                &phone.public("Phone"),
            )
            .unwrap();
        assert!(accept(session.code(), &phone, &response).is_err());
    }

    #[test]
    fn tampered_device_keys_fail_the_request_proof() {
        let phone = DeviceKeys::generate("phone".into());
        let attacker = DeviceKeys::generate("phone".into());
        let mut session = PairingSession::new();
        let mut req = request(session.code(), &phone.public("Phone")).unwrap();
        req.device = attacker.public("Phone");
        assert!(session.verify(&req).is_err());
    }
}
