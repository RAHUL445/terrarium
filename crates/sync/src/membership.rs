//! Staged v2 membership protocol. Not yet connected to live pairing/transports.
//!
//! Authority pins must come from explicit local adoption or authenticated pairing,
//! never from the record being verified. Persistence is required before activation.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrarium_core::DeviceId;

use crate::{
    keys::{b64, verify},
    DeviceKeys, DevicePublic, Result, Roster, SpaceKey, SyncError, WrappedKey,
};

const MAX_MEMBERS: usize = 256;
const MAX_RECORD: usize = 256 * 1024;
const DOMAIN: &[u8] = b"terrarium membership v2\0";

fn reject(reason: &str) -> SyncError {
    SyncError::Key(format!("membership: {reason}"))
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn key_id(key: &SpaceKey) -> String {
    let mut data = b"terrarium epoch key v2\0".to_vec();
    data.extend_from_slice(&key.to_bytes());
    digest(&data)
}

/// A signed transition, including key distribution to exactly the current members.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipRecord {
    version: u32,
    space: String,
    epoch: u64,
    previous: Option<String>,
    authority: DevicePublic,
    members: Roster,
    key_id: String,
    wrapped: BTreeMap<DeviceId, WrappedKey>,
    #[serde(with = "b64")]
    signature: [u8; 64],
}

impl MembershipRecord {
    fn message(&self) -> Result<Vec<u8>> {
        // Struct/BTreeMap serialization gives a deterministic signing transcript.
        let mut unsigned = self.clone();
        unsigned.signature = [0; 64];
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(&unsigned)?);
        Ok(bytes)
    }
    fn hash(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
    fn issue(
        space: &str,
        epoch: u64,
        previous: Option<String>,
        authority: &DevicePublic,
        signer: &DeviceKeys,
        members: Roster,
        key: &SpaceKey,
    ) -> Result<Self> {
        if signer.public(&authority.name) != *authority {
            return Err(reject("only the pinned authority may issue a transition"));
        }
        if members.is_empty()
            || members.len() > MAX_MEMBERS
            || members.iter().any(|(id, public)| {
                id.0.is_empty()
                    || id.0.len() > 256
                    || public.name.len() > 256
                    || id != &public.device
            })
        {
            return Err(reject("invalid membership budget or identities"));
        }
        let wrapped = members
            .iter()
            .map(|(id, public)| Ok((id.clone(), key.wrap_for(public)?)))
            .collect::<Result<_>>()?;
        let mut record = Self {
            version: 2,
            space: space.into(),
            epoch,
            previous,
            authority: authority.clone(),
            members,
            key_id: key_id(key),
            wrapped,
            signature: [0; 64],
        };
        record.signature = signer.sign(&record.message()?);
        record.validate(space, authority)?;
        Ok(record)
    }
    fn validate(&self, space: &str, pin: &DevicePublic) -> Result<()> {
        if self.version != 2 || space.is_empty() || space.len() > 256 || self.space != space {
            return Err(reject("unsupported version or wrong space"));
        }
        if &self.authority != pin || !verify(pin, &self.message()?, &self.signature) {
            return Err(reject("untrusted authority or invalid signature"));
        }
        if self.members.is_empty()
            || self.members.len() > MAX_MEMBERS
            || self.members.get(&pin.device) != Some(pin)
            || self.wrapped.len() != self.members.len()
            || self.members.keys().ne(self.wrapped.keys())
        {
            return Err(reject("invalid membership or recipients"));
        }
        for (id, member) in &self.members {
            if id.0.is_empty()
                || id.0.len() > 256
                || &member.device != id
                || member.name.len() > 256
                || &self.wrapped[id].device != id
                || self.wrapped[id].ciphertext.len() != 48
            {
                return Err(reject("invalid member or key envelope"));
            }
        }
        if self.to_bytes()?.len() > MAX_RECORD {
            return Err(reject("record exceeds budget"));
        }
        Ok(())
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RECORD {
            return Err(reject("record exceeds budget"));
        }
        Ok(serde_json::from_slice(bytes)?)
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn members(&self) -> &Roster {
        &self.members
    }
}

/// Device-local state; key material is deliberately absent from Debug and public
/// serialization. The eventual durable adapter must use the platform secret store.
#[derive(Clone)]
pub struct EpochState {
    current: MembershipRecord,
    keys: BTreeMap<u64, SpaceKey>,
}
impl std::fmt::Debug for EpochState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpochState")
            .field("epoch", &self.current.epoch)
            .finish_non_exhaustive()
    }
}
impl EpochState {
    /// Explicit adoption of a legacy space by its locally selected authority.
    /// Callers must persist the returned state before using it, and authenticate
    /// the exact root record/pin to each other device during migration.
    pub fn adopt_legacy(
        space: &str,
        signer: &DeviceKeys,
        authority: DevicePublic,
        members: Roster,
        legacy_key: SpaceKey,
    ) -> Result<Self> {
        let current =
            MembershipRecord::issue(space, 0, None, &authority, signer, members, &legacy_key)?;
        Ok(Self {
            current,
            keys: [(0, legacy_key)].into(),
        })
    }
    /// The root itself is not a trust source. `pin` and `root` must have been
    /// authenticated together outside this API (pairing or explicit adoption).
    pub fn accept_root(
        space: &str,
        pin: &DevicePublic,
        root: MembershipRecord,
        own: &DeviceKeys,
    ) -> Result<Self> {
        root.validate(space, pin)?;
        if root.epoch != 0 || root.previous.is_some() {
            return Err(reject("not a root"));
        }
        let key = Self::recipient_key(&root, own)?;
        Ok(Self {
            current: root,
            keys: [(0, key)].into(),
        })
    }
    fn recipient_key(record: &MembershipRecord, own: &DeviceKeys) -> Result<SpaceKey> {
        let public = record
            .members
            .get(&own.device)
            .ok_or_else(|| reject("device is revoked"))?;
        if own.public(&public.name) != *public {
            return Err(reject("recipient keys differ"));
        }
        let key = own.unwrap_space_key(&record.wrapped[&own.device])?;
        if key_id(&key) != record.key_id {
            return Err(reject("key identifier mismatch"));
        }
        Ok(key)
    }
    pub fn current(&self) -> &MembershipRecord {
        &self.current
    }
    pub fn historical_key(&self, epoch: u64) -> Option<&SpaceKey> {
        self.keys.get(&epoch)
    }
    /// Constructs a candidate; it does not mutate or activate local membership.
    pub fn prepare_removal(
        &self,
        authority: &DeviceKeys,
        removed: &str,
    ) -> Result<MembershipRecord> {
        if removed == self.current.authority.device.0 {
            return Err(reject("cannot remove authority"));
        }
        let mut members = self.current.members.clone();
        if members.remove(&DeviceId::from(removed)).is_none() {
            return Err(reject("unknown device"));
        }
        let epoch = self
            .current
            .epoch
            .checked_add(1)
            .ok_or_else(|| reject("epoch exhausted"))?;
        MembershipRecord::issue(
            &self.current.space,
            epoch,
            Some(self.current.hash()?),
            &self.current.authority,
            authority,
            members,
            &SpaceKey::generate(),
        )
    }
    /// Verifies the immediate successor; callers must persist the complete
    /// candidate keyring + record atomically before replacing their live state.
    pub fn stage(&self, next: MembershipRecord, own: &DeviceKeys) -> Result<Self> {
        next.validate(&self.current.space, &self.current.authority)?;
        if self.current.epoch.checked_add(1) != Some(next.epoch)
            || next.previous.as_deref() != Some(&self.current.hash()?)
            || next.key_id == self.current.key_id
        {
            return Err(reject("stale, skipped, forked or reused-key transition"));
        }
        // Removal cannot silently replace a retained device's identity or add a
        // member. Enrollment needs a separate authenticated pairing transition.
        for (id, member) in &next.members {
            if self.current.members.get(id) != Some(member) {
                return Err(reject("unexpected membership change"));
            }
        }
        if next.members.len() >= self.current.members.len() {
            return Err(reject("not a removal"));
        }
        let key = Self::recipient_key(&next, own)?;
        if self.keys.values().any(|old| old == &key) {
            return Err(reject("historical key reused"));
        }
        let mut candidate = self.clone();
        candidate.keys.insert(next.epoch, key);
        candidate.current = next;
        Ok(candidate)
    }
    /// Transaction boundary for a future platform adapter. Persistence failure
    /// leaves all live state unchanged; disk crash recovery is the adapter's job.
    pub fn activate<F>(&mut self, candidate: Self, persist: F) -> Result<()>
    where
        F: FnOnce(&Self) -> Result<()>,
    {
        // Recheck ancestry so an arbitrary/stale candidate cannot bypass stage.
        if candidate.current.epoch
            != self
                .current
                .epoch
                .checked_add(1)
                .ok_or_else(|| reject("epoch exhausted"))?
            || candidate.current.previous.as_deref() != Some(&self.current.hash()?)
            || candidate.current.authority != self.current.authority
            || candidate.current.space != self.current.space
        {
            return Err(reject("candidate no longer follows current state"));
        }
        persist(&candidate)?;
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (DeviceKeys, DeviceKeys, DeviceKeys, EpochState) {
        let host = DeviceKeys::generate("host".into());
        let phone = DeviceKeys::generate("phone".into());
        let lost = DeviceKeys::generate("lost".into());
        let members = [&host, &phone, &lost]
            .into_iter()
            .map(|k| (k.device.clone(), k.public(&k.device.0)))
            .collect();
        let state = EpochState::adopt_legacy(
            "space",
            &host,
            host.public("host"),
            members,
            SpaceKey::generate(),
        )
        .unwrap();
        (host, phone, lost, state)
    }
    #[test]
    fn rotation_distributes_only_to_remaining_members_and_preserves_old_keys() {
        let (host, phone, lost, state) = fixture();
        let follower =
            EpochState::accept_root("space", &host.public("host"), state.current.clone(), &phone)
                .unwrap();
        let next = state.prepare_removal(&host, "lost").unwrap();
        assert!(!next.wrapped.contains_key(&DeviceId::from("lost")));
        assert!(state.stage(next.clone(), &lost).is_err());
        let candidate = follower.stage(next.clone(), &phone).unwrap();
        let host_candidate = state.stage(next, &host).unwrap();
        assert_eq!(
            candidate.historical_key(1),
            host_candidate.historical_key(1)
        );
        assert_eq!(candidate.historical_key(0), state.historical_key(0));
        assert_ne!(candidate.historical_key(1), state.historical_key(0));
        let old_wrapped = candidate.current.wrapped[&DeviceId::from("phone")].clone();
        assert!(lost.unwrap_space_key(&old_wrapped).is_err());
    }
    #[test]
    fn forged_authority_cross_space_replay_and_forks_fail() {
        let (host, phone, _, state) = fixture();
        assert!(EpochState::accept_root(
            "other",
            &host.public("host"),
            state.current.clone(),
            &phone
        )
        .is_err());
        assert!(EpochState::accept_root(
            "space",
            &phone.public("phone"),
            state.current.clone(),
            &phone
        )
        .is_err());
        assert!(state.prepare_removal(&phone, "lost").is_err());
        let next = state.prepare_removal(&host, "lost").unwrap();
        let candidate = state.stage(next.clone(), &host).unwrap();
        assert!(candidate.stage(next, &host).is_err());
        let mut forged = state.prepare_removal(&host, "lost").unwrap();
        forged.epoch += 1;
        assert!(state.stage(forged, &host).is_err());
        let fork = state.prepare_removal(&host, "phone").unwrap();
        assert!(candidate.stage(fork, &host).is_err());
        assert!(state.prepare_removal(&host, "host").is_err());
        assert!(state.prepare_removal(&host, "unknown").is_err());
    }
    #[test]
    fn persistence_failure_preserves_live_state_and_retry_commits() {
        let (host, _, _, mut state) = fixture();
        let next = state.prepare_removal(&host, "lost").unwrap();
        let candidate = state.stage(next, &host).unwrap();
        assert!(state
            .activate(candidate.clone(), |_| Err(reject("synthetic disk failure")))
            .is_err());
        assert_eq!(state.current.epoch, 0);
        assert!(state.current.members.contains_key(&DeviceId::from("lost")));
        state.activate(candidate, |_| Ok(())).unwrap();
        assert_eq!(state.current.epoch, 1);
        assert!(!state.current.members.contains_key(&DeviceId::from("lost")));
    }
    #[test]
    fn authenticated_but_invalid_transitions_do_not_activate() {
        let (host, _, _, state) = fixture();
        let resign = |record: &mut MembershipRecord| {
            record.signature = host.sign(&record.message().unwrap());
        };
        let mut skipped = state.prepare_removal(&host, "lost").unwrap();
        skipped.epoch = 2;
        resign(&mut skipped);
        assert!(state.stage(skipped, &host).is_err());
        let mut forked = state.prepare_removal(&host, "lost").unwrap();
        forked.previous = Some("0".repeat(64));
        resign(&mut forked);
        assert!(state.stage(forked, &host).is_err());
        let mut reused = MembershipRecord::issue(
            "space",
            1,
            Some(state.current.hash().unwrap()),
            &host.public("host"),
            &host,
            state.prepare_removal(&host, "lost").unwrap().members,
            state.historical_key(0).unwrap(),
        )
        .unwrap();
        assert!(state.stage(reused.clone(), &host).is_err());
        reused.key_id = "f".repeat(64);
        resign(&mut reused);
        assert!(state.stage(reused, &host).is_err());
        let mut wrong_recipient = state.prepare_removal(&host, "lost").unwrap();
        wrong_recipient
            .wrapped
            .get_mut(&DeviceId::from("host"))
            .unwrap()
            .device = "lost".into();
        resign(&mut wrong_recipient);
        assert!(state.stage(wrong_recipient, &host).is_err());
        assert_eq!(state.current.epoch, 0);
    }

    #[test]
    fn record_budget_roundtrip_and_debug_redaction() {
        let (host, _, _, state) = fixture();
        let bytes = state.current.to_bytes().unwrap();
        let restored = MembershipRecord::from_bytes(&bytes).unwrap();
        restored.validate("space", &host.public("host")).unwrap();
        assert!(MembershipRecord::from_bytes(&vec![b' '; MAX_RECORD + 1]).is_err());
        let debug = format!("{state:?}");
        assert!(!debug.contains(&state.historical_key(0).unwrap().recovery_phrase()));
    }
}
