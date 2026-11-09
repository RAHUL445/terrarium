//! Request authentication independent of clocks. Challenges are short-lived,
//! bounded, single-use and bound to the space, method, target and body digest.
use crate::{DeviceKeys, Roster, SpaceKey};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub(crate) const AUTH_HEADER: &str = "x-pp-auth";
pub(crate) const RESPONSE_HEADER: &str = "x-pp-response";
const TTL: Duration = Duration::from_secs(60);
const CAPACITY: usize = 1024;

#[derive(Serialize, Deserialize)]
pub(crate) struct Proof {
    device: terrarium_core::DeviceId,
    nonce: String,
    digest: String,
    signature: String,
}

fn digest(body: &[u8]) -> String {
    B64.encode(Sha256::digest(body))
}
fn message(
    device: &terrarium_core::DeviceId,
    space: &str,
    method: &str,
    target: &str,
    nonce: &str,
    digest: &str,
) -> Vec<u8> {
    serde_json::to_vec(&(
        "pp-http-request-v1",
        device,
        space,
        method,
        target,
        nonce,
        digest,
    ))
    .expect("string tuple")
}

impl Proof {
    pub(crate) fn create(
        keys: &DeviceKeys,
        space: &str,
        method: &str,
        target: &str,
        nonce: String,
        body: &[u8],
    ) -> Self {
        let digest = digest(body);
        let signature = B64.encode(keys.sign(&message(
            &keys.device,
            space,
            method,
            target,
            &nonce,
            &digest,
        )));
        Self {
            device: keys.device.clone(),
            nonce,
            digest,
            signature,
        }
    }
    pub(crate) fn header(&self) -> String {
        B64.encode(serde_json::to_vec(self).expect("proof serialization"))
    }
    pub(crate) fn parse(header: &str) -> Option<Self> {
        if header.len() > 1024 {
            return None;
        }
        let p: Self = serde_json::from_slice(&B64.decode(header).ok()?).ok()?;
        crate::transport::check_device(&p.device).ok()?;
        if B64.decode(&p.nonce).ok()?.len() != 32
            || B64.decode(&p.digest).ok()?.len() != 32
            || B64.decode(&p.signature).ok()?.len() != 64
        {
            return None;
        }
        Some(p)
    }
    pub(crate) fn matches_body(&self, body: &[u8]) -> bool {
        self.digest == digest(body)
    }
    fn tag(
        &self,
        key: &SpaceKey,
        space: &str,
        method: &str,
        target: &str,
        status: u16,
        body: &[u8],
    ) -> Hmac<Sha256> {
        let mut response_key = [0u8; 32];
        hkdf::Hkdf::<Sha256>::new(None, &key.to_bytes())
            .expand(b"terrarium peer HTTP response v1", &mut response_key)
            .expect("fixed HKDF output length");
        let mut mac = Hmac::<Sha256>::new_from_slice(&response_key).expect("32-byte key");
        let request = message(
            &self.device,
            space,
            method,
            target,
            &self.nonce,
            &self.digest,
        );
        mac.update(
            &serde_json::to_vec(&(
                "pp-http-response-v1",
                B64.encode(request),
                status,
                digest(body),
            ))
            .expect("response tuple"),
        );
        mac
    }
    pub(crate) fn response_tag(
        &self,
        key: &SpaceKey,
        space: &str,
        method: &str,
        target: &str,
        status: u16,
        body: &[u8],
    ) -> String {
        B64.encode(
            self.tag(key, space, method, target, status, body)
                .finalize()
                .into_bytes(),
        )
    }
    pub(crate) fn verify_response(
        &self,
        tag: &str,
        key: &SpaceKey,
        request: (&str, &str, &str),
        status: u16,
        body: &[u8],
    ) -> bool {
        let (space, method, target) = request;
        if tag.len() > 64 {
            return false;
        }
        B64.decode(tag).ok().is_some_and(|bytes| {
            self.tag(key, space, method, target, status, body)
                .verify_slice(&bytes)
                .is_ok()
        })
    }
}

#[derive(Default)]
pub(crate) struct Challenges(HashMap<String, Instant>);
impl Challenges {
    pub(crate) fn issue(&mut self) -> Option<String> {
        self.0.retain(|_, expires| *expires > Instant::now());
        if self.0.len() >= CAPACITY {
            return None;
        }
        let mut bytes = [0; 32];
        getrandom::getrandom(&mut bytes).ok()?;
        let nonce = B64.encode(bytes);
        self.0.insert(nonce.clone(), Instant::now() + TTL);
        Some(nonce)
    }
    pub(crate) fn authorize(
        &mut self,
        proof: &Proof,
        roster: &Roster,
        space: &str,
        method: &str,
        target: &str,
    ) -> bool {
        if self
            .0
            .get(&proof.nonce)
            .is_none_or(|expires| *expires <= Instant::now())
        {
            return false;
        }
        let Some(public) = roster.get(&proof.device) else {
            return false;
        };
        let Ok(signature) = B64.decode(&proof.signature) else {
            return false;
        };
        let Ok(signature) = signature.try_into() else {
            return false;
        };
        if !crate::keys::verify(
            public,
            &message(
                &proof.device,
                space,
                method,
                target,
                &proof.nonce,
                &proof.digest,
            ),
            &signature,
        ) {
            return false;
        }
        self.0.remove(&proof.nonce);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proofs_bind_request_and_live_membership_and_reject_replay() {
        let keys = DeviceKeys::generate("phone".into());
        let roster = [(keys.device.clone(), keys.public("Phone"))]
            .into_iter()
            .collect();
        let mut challenges = Challenges::default();
        let nonce = challenges.issue().unwrap();
        let p = Proof::parse(
            &Proof::create(&keys, "space", "PUT", "/v1/a?q=1", nonce, b"data").header(),
        )
        .unwrap();
        for (space, method, target) in [
            ("other", "PUT", "/v1/a?q=1"),
            ("space", "GET", "/v1/a?q=1"),
            ("space", "PUT", "/v1/a?q=2"),
        ] {
            assert!(!challenges.authorize(&p, &roster, space, method, target));
        }
        assert!(!challenges.authorize(&p, &Roster::new(), "space", "PUT", "/v1/a?q=1"));
        assert!(!p.matches_body(b"tampered"));
        assert!(p.matches_body(b"data"));
        assert!(challenges.authorize(&p, &roster, "space", "PUT", "/v1/a?q=1"));
        assert!(!challenges.authorize(&p, &roster, "space", "PUT", "/v1/a?q=1"));
        let nonce = challenges.issue().unwrap();
        let expired = Proof::create(&keys, "space", "GET", "/v1/roster", nonce.clone(), b"");
        challenges.0.insert(nonce, Instant::now() - TTL);
        assert!(!challenges.authorize(&expired, &roster, "space", "GET", "/v1/roster"));
    }
    #[test]
    fn device_id_is_bound_even_when_roster_entries_share_a_signing_key() {
        let keys = DeviceKeys::generate("phone".into());
        let mut c = Challenges::default();
        let nonce = c.issue().unwrap();
        let mut proof = Proof::create(&keys, "space", "GET", "/v1/roster", nonce, b"");
        let mut public = keys.public("Phone");
        public.device = "alias".into();
        let roster = [(public.device.clone(), public)].into_iter().collect();
        proof.device = "alias".into();
        assert!(!c.authorize(&proof, &roster, "space", "GET", "/v1/roster"));
    }

    #[test]
    fn challenge_expiry_and_capacity_fail_closed() {
        let mut c = Challenges::default();
        for _ in 0..CAPACITY {
            c.issue().unwrap();
        }
        assert!(c.issue().is_none());
        for expiry in c.0.values_mut() {
            *expiry = Instant::now() - TTL;
        }
        assert!(c.issue().is_some());
        assert_eq!(c.0.len(), 1);
    }
    #[test]
    fn response_authentication_binds_status_body_space_and_request() {
        let key = SpaceKey::generate();
        let keys = DeviceKeys::generate("phone".into());
        let p = Proof::create(&keys, "space", "GET", "/v1/roster", "nonce".into(), b"");
        let tag = p.response_tag(&key, "space", "GET", "/v1/roster", 200, b"roster");
        assert!(p.verify_response(&tag, &key, ("space", "GET", "/v1/roster"), 200, b"roster"));
        for (space, method, path, status, body) in [
            ("wrong", "GET", "/v1/roster", 200, &b"roster"[..]),
            ("space", "GET", "/v1/roster", 404, &b"roster"[..]),
            ("space", "GET", "/v1/roster", 200, &b"tampered"[..]),
            ("space", "GET", "/v1/config/drive", 200, &b"roster"[..]),
        ] {
            assert!(!p.verify_response(&tag, &key, (space, method, path), status, body));
        }
        assert!(!p.verify_response(
            &tag,
            &SpaceKey::generate(),
            ("space", "GET", "/v1/roster"),
            200,
            b"roster"
        ));
        assert!(Proof::parse(&"x".repeat(1025)).is_none());
    }
}
