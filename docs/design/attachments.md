# Design: attachments (blob store, transfer, quotas)

Status: as built (ATT-02 through ATT-06).

Human review required: yes. This design changes crypto, sync transfer, backup format, and storage contracts across devices.

Built sections:
- 3.1 Identity and encryption
- 3.2 Local blob store
- 3.3 Transfer and sync behavior
- 3.4 Column contract and API surface
- 3.5 UI and markdown behavior
- 3.6 Quota, GC, and backups

## 1. Scope and goals

Attachments are encrypted blobs stored outside plugin SQLite rows, referenced from row values and markdown text. The design must:

- keep plugin isolation (plugins never get raw bytes directly),
- keep sync causal and robust across host, peer HTTP, and Drive transports,
- work with phones on mobile data,
- include quota and garbage collection policy,
- include backup/restore coverage for blobs,
- define older-device and rolling-upgrade behavior.

Out of scope: code changes, and migrating existing voice/package base64 columns into the blob store.

## 2. Current constraints that drive the design

- Sync envelopes already provide authenticated encryption and signatures for batch/snapshot/config messages.
- `BatchStore` is batch-oriented today (`devices/list/get/put/snapshot/put_snapshot`).
- Peer HTTP routes currently cover batches, snapshots, and config only.
- Drive transport stores files in appData (`b.<device>.<seq>`, `s.<device>`).
- Tauri sync currently runs `engine.sync` under `plugins.with_host(...)` (plugin-host lock), which must not include long blob network I/O.
- Plugin schema columns are currently `text|integer|real|boolean`.
- UI CSP is `img-src 'self' data:`; there is no custom blob scheme yet.

## 3. Decisions by topic

### 3.1 Identity and encryption

#### D1. Blob id

Use keyed content identity, scoped to the space:

- Blob id string format: `b1-<kid8>-<mac64>`
- `<kid8>`: first 8 lowercase hex chars of `SHA-256(blob-id-key)` (key identifier, not secret).
- `<mac64>`: lowercase hex `HMAC-SHA256(blob-id-key, plaintext-bytes)`.

`blob-id-key` derivation:

- HKDF-SHA256 root: current space key bytes.
- info: `terrarium/blob-id/v1`.
- output: 32 bytes.

Why:

- dedupe within a space,
- no cross-space dedupe oracle,
- deterministic id before encryption, so encryption can stay randomized.

Tradeoff: a key holder can still confirm a guessed file by recomputing id. This is acceptable because paired devices already hold the space key.

#### D2. Blob encryption

Each blob is encrypted independently with random nonce:

- cipher: XChaCha20-Poly1305,
- key: `blob-data-key = HKDF(space-key, info="terrarium/blobs/v1")`,
- nonce: 24 random bytes per blob,
- AAD: `terrarium-blob-v1|<blob-id>|<plain-size>`.

Deterministic encryption is not needed (id already dedupes plaintext).

#### D3. Pre-join devices and key rotation

- A device not joined to a space cannot create or fetch attachments.
- UI message: "Attachments are available after this device joins a space."
- No device-root blob mode and no re-encryption workflow in ATT-01.

Key rotation policy (SEC-02 forward-compat):

- New space key => new derived blob keys and likely new `kid8`.
- Existing blob ids remain valid for old content.
- New writes use the active key epoch.
- Readers keep a keyring of active + retired blob keys until migration is complete.

#### D4. Integrity checks and signatures

- Receiver verifies envelope signature (already done for batch/snapshot/config).
- For each blob payload, receiver decrypts and recomputes blob id (`HMAC` over plaintext).
- Mismatch => reject blob bytes and mark transfer error.

No per-blob signature is added. Signed batch references plus id verification are sufficient.

### 3.2 Local blob store

#### D5. Layout

Store blobs per space, not globally shared:

- root: `<space-data>/blobs/`
- path: `<space-data>/blobs/<kid8>/<first2>/<blob-id>.blob`
- temp write: same directory, `.<blob-id>.<rand>.tmp`
- commit: atomic rename temp -> final.

`<first2>` is the first 2 hex chars from `<mac64>`.

Why per-space:

- data is already space-scoped,
- simpler quota accounting and GC,
- avoids accidental cross-space leakage.

#### D6. On-disk blob file format

Each `.blob` file is a small header plus ciphertext:

- bytes 0..4: magic `TBL1`
- byte 4: format version `1`
- bytes 5..12: plaintext size `u64` little-endian
- bytes 13..36: nonce (24 bytes)
- bytes 37..52: reserved (16 bytes, currently zero)
- bytes 53..end: ciphertext+tag from XChaCha20-Poly1305

The blob id is not stored in the file body because it is in the filename and AAD.

#### D7. Size and memory limits

- Max plaintext blob size: 25 MiB.
- Max sealed blob file size: 25 MiB + 4 KiB envelope/headroom.
- Hash/encrypt/decrypt streamed in chunks (target 256 KiB buffer).
- No whole-blob base64 in rows.

Why 25 MiB:

- fits under existing 32 MiB sync HTTP body limits with framing headroom,
- large enough for common screenshots/docs/audio clips,
- keeps mobile transfers bounded.

### 3.3 Transfer and sync behavior

#### D8. Transport API shape

Keep one store abstraction by extending `BatchStore` with default blob methods that return `Unsupported`:

- `blob_list(after, limit) -> Vec<String>`
- `blob_has(id) -> bool`
- `blob_get(id) -> Vec<u8>`
- `blob_put(id, bytes) -> ()`

Implement in Folder, Memory, HTTP, Drive, Multi. Existing custom stores compile because defaults exist.

#### D9. Blob namespace and store naming

Blobs are immutable and content-addressed, so namespace is shared per space, not per device.

- FolderStore files: `blobs/<blob-id>.blob`
- Drive files: `x.<blob-id>`
- MemoryStore key: `<blob-id>`
- HTTP path: `/v1/blobs/<blob-id>`

#### D10. Peer HTTP routes and limits

Add authenticated routes (same challenge/proof + signed response flow as batches):

- `HEAD /v1/blobs/<blob-id>` -> `200` if present, `404` if missing
- `GET /v1/blobs/<blob-id>` -> raw `.blob` bytes
- `PUT /v1/blobs/<blob-id>` -> raw `.blob` bytes (idempotent)
- `GET /v1/blobs?after=<cursor>&limit=<n>` -> JSON `{ids: [...], next: "...|null"}`

Limits:

- max blob request/response body: 26 MiB,
- max list response body: 1 MiB,
- max `limit`: 5000 ids.

Id validation: route rejects malformed ids before any file I/O.

#### D11. What to fetch

Primary policy: fetch only blobs referenced by newly received rows/markdown refs that are missing locally.

Secondary policy:

- UI open/download action can trigger on-demand fetch for a single blob.
- No "fetch all blobs from all peers" background mode by default.

Why:

- protects phone bandwidth and storage,
- avoids scanning full blob catalogs on every sync loop.

#### D12. Ordering

Publisher order:

1. ensure required blobs are uploaded to at least one transport,
2. then publish batch that references them.

Receiver behavior if batch arrives first anyway:

- keep row value,
- mark blob as missing,
- UI shows "Not on this device yet" (or "Downloading..." while fetch active),
- background fetch retries.

#### D13. Locking and execution split

Blob transfer must run outside plugin-host lock:

- keep `engine.sync` (batch pull/push) under `plugins.with_host(...)` for store import/export,
- run blob publish/fetch phase after that call returns,
- blob phase uses dedicated blob store lock + worker queue, not plugin host lock.

This avoids blocking plugin calls/UI while network I/O is in flight.

#### D14. Older peers and stores

- Peers without blob routes still sync batches/snapshots normally.
- `blob_*` operations returning `Unsupported` are logged as transport capability gaps, not hard failures.
- Drive peers that do not know `x.*` names ignore them.
- MultiStore still succeeds if at least one transport supports blob upload/download.

### 3.4 Column contract and API surface

#### D15. `file` value format

A file cell stores JSON text, canonical object form:

```json
{"blob":"b1-1a2b3c4d-...","name":"photo.jpg","mime":"image/jpeg","size":1234}
```

Rules:

- required keys: `blob`, `name`, `size`
- optional key: `mime`
- no extra keys
- `blob`: valid blob id format
- `name`: UTF-8 string, 1..255 chars, path separators and control chars rejected
- `mime`: ASCII token/subtype optionally with parameters, max 128 chars
- `size`: integer, 1..26214400

`check_value` validates the decoded object for file-formatted columns.

#### D16. Schema shape choice and upgrade policy

Choose `type: "text"` plus optional `format: "file"` on `ColumnDef` (not new SQLite primitive type).

Why:

- SQLite representation remains TEXT (migration-safe),
- parser/validation can enforce file contract by format,
- easier interoperability with existing query/render paths.

Upgrade policy:

- older hosts reject manifests with unknown `format` field (expected),
- therefore attachment columns are allowed only after all paired devices are on attachment-capable host builds,
- host must block installing/updating a plugin manifest with `format: "file"` when roster has older app versions.

#### D17. How blobs enter the system

- UI paste/drop/picker calls a host command (`attachment.store`) with bytes + metadata.
- Host validates, stores blob, and returns canonical file JSON value.
- Plugin operations only receive that JSON value string; they never receive raw bytes.

This preserves plugin isolation and existing write/arg limits.

Future option: separate WIT blob-read interface for explicitly permitted plugins (not in ATT-01).

#### D18. Cross-plugin references

Any plugin row may reference any blob id string. This does not break isolation:

- plugins do not get blob bytes,
- ids are keyed and not easy to guess without space key,
- byte access remains host-mediated and permission-checked.

### 3.5 UI and markdown behavior

#### D19. Rendering under CSP

Use a custom local URI scheme for blob reads:

- URL form: `terrarium-blob://<space>/<blob-id>?mode=<inline|download>`
- CSP update: add `terrarium-blob:` to `img-src` and `media-src`.

Why not `data:` for everything:

- large memory duplication (binary -> base64 -> decoded image),
- expensive for repeated list rendering.

Inline limits:

- auto-inline images up to 8 MiB,
- above that: show file card with explicit open/save action.

SVG policy:

- treat `image/svg+xml` as downloadable file by default (no inline rendering) to avoid script/external-reference surprises.

#### D20. Markdown reference form

Standard markdown link targets for attachments:

- file link: `[label](terrarium:blob/<blob-id>)`
- image link: `![alt](terrarium:blob/<blob-id>)`

Optional query fields for display hints:

- `?name=<url-encoded>`
- `?mime=<url-encoded>`

`terrarium:` aligns with LINK-02 direction; renderer/router resolve to host blob open.

#### D21. Open/save behavior by platform

- Desktop:
  - open: write temp file and ask OS "open with default app",
  - save: user chooses path in save dialog.
- Android:
  - open/share through platform share intent using temporary exported file.

Temp file rules:

- random file names in app cache,
- extension derived from sanitized `name` or `mime`,
- auto-cleanup on startup and periodic sweep.

### 3.6 Quota, GC, and backups

#### D22. Quota policy

Per-space hard disk quota:

- default: 2 GiB,
- configurable (Settings): 512 MiB .. 10 GiB.

Per-device ingest quota for remote blob downloads:

- default: 512 MiB/day on desktop,
- default: 128 MiB/day on mobile,
- reset at local midnight.

Behavior when full:

- local create/import of new blobs is rejected with clear error,
- remote blob fetch pauses (rows can still sync as missing blobs),
- Settings -> Devices and sync shows used bytes, quota, and paused reason.

#### D23. Garbage collection policy

Local liveness:

- live if referenced by any `format:file` cell in any row state (live or trashed),
- or referenced by `terrarium:blob/...` markdown links in text columns.

GC:

- blobs with no references enter "orphaned" state with timestamp,
- if still orphaned after 30 days, delete local blob file.

Remote cleanup:

- only host deletes from relay folder and Drive,
- host deletes a remote blob only after:
  1) blob unreferenced in current state,
  2) orphaned >= 30 days,
  3) all roster devices acknowledged at least one sync after orphan timestamp.

#### D24. Backup format v2

Current backup v1 stores `(roster, stores)` as one deflated JSON payload.
v2 adds blobs as separate stream entries.

Container:

- outer file remains encrypted (`BackupInfo { v: 2, ... }`, nonce, ciphertext),
- plaintext is a length-prefixed stream:
  - magic: `TBK2`
  - entry sequence:
    - `u8 kind` (`1 = manifest`, `2 = store-batch`, `3 = blob`)
    - `u32 header_len`
    - `u64 body_len`
    - `header_json[header_len]`
    - `body[body_len]`

Entry details:

- manifest entry body: JSON with roster and metadata.
- store-batch entry header: `{"plugin":"todo","sender":"...","changes":123}`; body is one batch JSON bytes.
- blob entry header: `{"id":"b1-...","size":1234,"kid":"1a2b3c4d"}`; body is raw local `.blob` file bytes (already encrypted).

Restore order:

1. read manifest and validate,
2. restore blob entries to blob store,
3. import store-batch entries through normal `store.import`.

Compatibility:

- v1 backups still restore in new app,
- older apps reject v2 with explicit "backup version 2 is not supported".

## 4. Older-host and older-peer behavior summary

- Older peers: continue batch/snapshot sync; blob routes absent => missing attachments on those peers.
- Older Drive-only peers: ignore `x.*` blob files.
- Older hosts: reject manifests using `format: "file"`; host must gate rollout until roster upgraded.
- Mixed-version roster policy: no attachment schema rollout until all paired devices are attachment-capable.
- Rows containing file JSON in plain text columns remain harmless text on older clients that cannot render attachments.

## 5. Threat handling

- Malicious plugin disk fill via references: plugin cannot upload blob bytes; host command enforces quota and size limits.
- Compromised peer serves wrong bytes: decrypt + recompute id check rejects content.
- Decompression/parser bombs: no decompression in blob path; image decode guarded by size caps and deferred decode.
- SVG script risk: do not inline SVG; treat as downloadable by default.
- MIME spoofing: trust detection from bytes for rendering decisions when possible; metadata MIME is advisory.
- Path traversal via names: file names sanitized, separators/control chars rejected, open/save never uses raw path segments.
- Drive quota exhaustion: per-day ingest and per-space quota pause downloads before uncontrolled growth.

## 6. Tests to implement in ATT-02..ATT-06

ATT-02 (blob store):

- id determinism across devices with same key and plaintext,
- random nonce makes ciphertext differ for same plaintext,
- decrypt+id verification rejects tampered bytes,
- atomic temp-write/rename leaves no partial files after crash simulation,
- size limit enforcement at 25 MiB.

ATT-03a (transport API):

- each store implements blob methods or returns `Unsupported`,
- FolderStore/Drive naming and id validation tests,
- HTTP blob route auth/limits/status code tests,
- MultiStore blob read/write fallback semantics.

ATT-03b (sync transfer wiring):

- publish blobs before referencing batch,
- receive missing-reference row then fetch blob later,
- no blob network I/O while plugin-host lock is held,
- mobile/day-ingest quota stop/resume behavior.

ATT-04 (column/API):

- `format:file` manifest parsing and `deny_unknown_fields` compatibility tests,
- file JSON validation tests (missing/extra/bad fields),
- host command returns canonical JSON and plugin never sees bytes.

ATT-05 (renderer/editor):

- markdown/file/image rendering for `terrarium:blob/...`,
- CSP and scheme integration tests,
- paste/drop/picker flow creates file reference and shows placeholder while downloading.

ATT-06 (cleanup/quota/backups):

- orphan detection and 30-day GC for local blobs,
- remote delete guard on roster acknowledgements,
- backup v2 round-trip with blobs + rows,
- v1 restore still works,
- older-version rejection message for v2.

## 7. Rollout plan

1. ATT-02 and ATT-03 build storage + transport with host-side capability checks.
2. ATT-04 introduces schema format and value validation.
3. ATT-05 exposes UI affordances and markdown route handling.
4. ATT-06 enforces quotas, GC, and backup v2.
5. Only then enable attachment manifests for recommended plugins.

## 8. Differences from task files

- `docs/design/links-and-search.md` is referenced by the task brief but is not present in this worktree; this document follows the style of existing `docs/design/cli-control.md` instead.
