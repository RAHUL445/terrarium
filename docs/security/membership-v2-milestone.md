# Device revocation v2 — staged protocol milestone

Date: 2026-11-07. Scope: cryptographic membership/key distribution primitives.
**Not active in the application. Full device revocation is not complete.**
The existing Block device action remains host-local blocking. No live keys,
paired devices, Drive credentials or records changed during this milestone.

## Implemented in crates/sync/src/membership.rs

- Explicit legacy adoption by a locally chosen authority; no inferred authority
  from roster ordering. Remote root acceptance requires the exact root and
  authority pin authenticated outside this API. A record cannot establish its
  own trust merely by containing its signer's public key.
- Deterministic, domain-separated Ed25519 signatures over version, space,
  monotonic epoch, predecessor hash, authority, member identities, key ID and
  wrapped keys. All member/recipient maps are ordered, and unknown fields denied.
- Removal creates a fresh XChaCha20 space key with X25519 wrapping only for
  remaining members. Retained identities cannot silently change. Self-removal,
  unknown targets, non-authority issuers, skipped epochs, forks, wrong spaces,
  wrong recipients, invalid signatures, inconsistent key IDs and historical key
  reuse fail. Enrollment is deliberately not hidden in the removal transition.
- Candidate staging preserves old keys for history and leaves live membership
  unchanged. Activation calls a persistence callback first; callback failure
  preserves live state. This boundary does not itself implement disk crash
  recovery or secure-store journaling.
- 256-member and 256 KiB record limits; wrapping checks existing noncontributory
  exchange-key protections. Epoch-state Debug omits secrets. The state has no
  public plaintext serialization; durable storage must use the platform backend.

Five tests cover remaining-device agreement, exclusion of the removed device,
old-key preservation, authority/space checks, forged and correctly signed invalid
transitions, replay/forks, fresh keys, failed persistence/retry, record budget,
round trip and Debug redaction. They use only generated synthetic identities.
These are developer regression tests, not an independent cryptographic review.

## Current readers and writers; remaining integration

The following changes must be completed together before enabling this module:

1. **Authority/enrollment:** apps/tauri/src-tauri/src/sync.rs currently persists
   SpaceFile/roster and a single `space-key`. Pairing response in
   crates/sync/src/pairing.rs does not authenticate the v2 root/authority pin or
   negotiate protocol support. Add explicit legacy trust adoption, signed
   enrollment and fail-closed mixed-version negotiation. Offline edits remain
   preserved when an old peer is told to upgrade.
2. **Durability:** store the epoch floor, root/chain, historical keyring and
   migration journal in private/secure storage with idempotent recovery after
   every write boundary. Prove disk/Keychain/Keystore failures and app restarts.
   This milestone's callback is not that durable implementation.
3. **Batch/snapshot transport:** envelope.rs, engine.rs, peer.rs, http_auth.rs and
   http.rs still use the v1 single-key path. Bind epoch/key ID into authenticated
   wire metadata, select historical keys only for historical reads, enforce the
   current signed roster for new writes, distribute chained transitions to
   offline peers and stop revoked readers before serving new data. An already
   authorized in-flight request needs defined handling.
4. **Backup/shared secrets:** backup.rs and the app's AI/shared-Drive sealing
   currently use the old single key. Version their envelopes/keyring recovery,
   re-seal current shared secrets and publish a new authorized snapshot. Test
   rollback/reopen and backups without silently changing existing recovery keys.
5. **Drive:** removing a member from the app cannot revoke a copied Google token.
   Implement provider revocation/reconnection and a fresh authorization path,
   then distribute new credentials only to the remaining roster. A removed
   device's previously downloaded data cannot be remotely erased.
6. **Acceptance:** hostile old/new peers, mixed versions, offline members,
   historical CRDT replay, restored backups, process death, actual desktop/Android
   upgrade, provider revocation and independent protocol review remain pending.

Until these pass, no UI label or publishing checklist may present this primitive
as complete compromised-device revocation. No production activation flag or
implicit credential/key migration was added.

## Rollback

The module is not called by production paths and changes no current file/wire
format. Reverting this milestone removes the library API/tests only. After a
future v2 migration, rollback must be treated separately: an old binary must not
resume writes using revoked membership or erase the epoch floor/keyring.

## Verification snapshot

All five primitive tests pass. The scoped workspace suite passed 171 tests,
with one real-Keychain test ignored and two unrelated uncommitted board/matrix
feature tests excluded against the clean plugin bundle. Native workspace
Clippy and Android arm64 core/sync Clippy passed with warnings denied.
These checks
establish the primitive milestone only; none of the live acceptance gates above
is replaced by compilation or these tests.
