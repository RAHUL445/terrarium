# Device blocking and key rotation

## Implemented: host-local blocking (2026-11-07)

Sync settings on the host offers **Block device** for other paired devices. The host refuses self-removal and unknown IDs. It saves the new roster atomically using the existing private-file writer before changing live authorization, then updates the local sync engine and cancels the displayed pairing code. Pairing also persists its tentative roster before publishing membership or returning a successful response. Persistence failures are returned rather than silently ignored.

The persistence callback runs while the roster lock is held; it must not call the relay or acquire app state. App commands acquire `inner` before `relay`; callbacks only read/write the membership file. Tests exercise failed pairing/removal writes, unchanged membership/code on failure, self-removal denial, and reopening the persisted roster after success. Existing peer tests exercise authorization after removal, replay and authenticated real-key sync.

This is host access blocking, **not full compromised-device revocation**. Previously authorized requests may finish. Already downloaded data, old keys and Google Drive tokens cannot be erased remotely. A newly shown pairing code can explicitly authorize a device again. Historical records are preserved; batches signed only by removed roster members may be rejected when replayed by a newly restored device, so restore/compaction compatibility needs review before describing this as complete lifecycle management.

## Rotation design — application integration pending

The [v2 protocol primitive milestone](membership-v2-milestone.md) now implements signed removal records, chained epochs, remaining-member key wrapping and candidate activation ordering in isolation. It is not connected to live pairing, durable key storage, transports or backup/Drive flows; the complete steps below remain the activation gate.

A safe implementation must commit these pieces together; changing the current single `space-key` value alone would strand data and shared settings:

1. Host authority: pin a host signing identity during authenticated pairing. Existing paired devices need an explicit, verifiable trust transition; never guess the host from sorted roster entries.
2. Signed membership records: bind space ID, authority, monotonically increasing epoch, previous-record hash, current members and key identifiers. Persist the highest accepted epoch and reject rollback, cross-space records and untrusted signers. Define backup/host-recovery authority explicitly.
3. Keyring and distribution: generate a fresh data key for the new epoch; wrap it only for remaining device exchange keys. Keep earlier keys locally for historical ciphertext. A removed member must never receive the new key through HTTP, Drive, recovery exports or old shared credentials.
4. Atomic transition: stage private keyring/membership data; verify copies; persist a recoverable transition journal; switch the live relay/engine only after commit. Crash at each boundary must reopen safely without generating replacement identities or deleting offline edits.
5. Envelope/version support: authenticate epoch/key ID in batch, snapshot, config and backup envelopes. Define signed capability negotiation; incompatible peers must stop publishing with a clear upgrade path, without deleting their unsent edits.
6. Re-seal AI/shared secrets, publish a new authorized snapshot, and distribute the transition to offline members. Preserve old envelopes needed for recovery without allowing old-key writers to publish new-epoch data.
7. Revoke/replace Drive authorization through the provider. Disconnecting only this host does not invalidate tokens copied onto former members.
8. Prove old/new CRDT reopen, offline edits, restored backups, failed disk/Keychain writes, crashes, stale epochs, forged authority, revoked-device writes/reads, and mixed-version rejection. Independent protocol review and real paired desktop/Android upgrade tests remain release gates.

No live wire format, keyring migration, application key rotation or credential revocation is claimed by this blocking milestone. No live membership or credentials were changed by development tests.
