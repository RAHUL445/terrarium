# Version history

## Status

Design decisions settled for HIST-02 through HIST-06; no implementation in
HIST-01. Date: 2026-11-11.

Review: pending user review (approved to proceed)

## 1. What is stored today

The authoritative source is [the core store](../../crates/core/src/store.rs).
Each plugin has a separate SQLite store per space. Every accepted local or
remote change is retained in `_pp_changes(lseq, hlc, device, user_id, tbl,
row_id, schema_version, op)`. `lseq` is local arrival order, not version
order; `_pp_changes_row(tbl, row_id)` indexes item reads. Nothing currently
deletes log rows.

[Change and Op](../../crates/core/src/change.rs) carry `hlc`, `user`,
`table`, `row`, `schema_version`, and a tagged JSON operation:

- `{"op":"create"}` establishes a row; defaults come from the schema.
- `{"op":"set","column":"title","value":"Draft","base":null}`
  records the new LWW value and the cell HLC the author saw (or null).
- `{"op":"increment","column":"points","delta":2}` records a delta.
- `{"op":"text_edit","column":"body","update":"<base64>"}`
  records a Loro update, not the resulting plain text.
- `{"op":"delete","seen":{}}` records the per-device HLC vector seen
  by the deleter; a populated vector maps device strings to HLC strings.
- `{"op":"restore"}` moves a recoverable row out of trash.

`_pp_cells` holds only current LWW cell timestamps. `_pp_rows` tracks
`live`, `trashed`, or terminal `gone`. `_pp_conflicts` keeps losing and
winning concurrent LWW values with `recorded_ms`; it is not a complete
version source. `_pp_pending` buffers newer-schema changes before applying
them; these are excluded from History until applied.

`apply_change` logs an accepted op, then updates materialized values. A
losing LWW Set remains in the log. Increments add; text imports updates
into the hidden `<column>__crdt` snapshot and writes plain text separately.
[Loro helpers](../../crates/core/src/text.rs) show that snapshots include
document history. Delete/Restore recompute deletion from the whole log;
edit-wins additionally considers edits concurrent with a delete.
`remove_row` deletes materialized values and cell timestamps but preserves
the log and a terminal tombstone. Later ops for a gone row are logged but
do not revive it.

`Writer::insert` emits Create followed by column operations (counter
initial values are deltas). Update emits Set or TextEdit; counters use
increment. Writes are transactional through `Store::write`.

`purge` can remove acknowledged, sufficiently old trashed rows;
`prune_conflicts` can remove old conflict records. Source search of
`crates` and `apps` found no application callers (purge callers are tests).
Neither deletes `_pp_changes`. [Sync snapshots](../../crates/sync/src/engine.rs)
export every store with `export_since(0)`; sync `compact` removes covered
transport batch files, not SQLite change rows. `row_seen` also needs the log.

## 2. Goals and non-goals

Let users browse earlier values of one opted-in plugin item and bring
those values back with ordinary synced writes. Give UI, CLI, MCP and the
assistant one host-mediated command contract, with the same space and
retention checks.

This is a deterministic view of accepted changes, not a record of exactly
what a person saw on a screen. Offline edits can change the interpretation
of an earlier cutoff when they arrive. No host-store history (`_pages`,
`_spaces`, `_ai`), field-level restore, word diff, new sync operation,
log compaction, or secure erasure is included.

## 3. Decisions

### 3.1 Source: replay the change log

Read the row's full `_pp_changes` log, ordered by HLC. No new snapshot
table, migration or per-edit write is needed. This covers existing edits
and avoids a second source of truth. A local snapshot cache could be
considered later if measurements justify it; it cannot replace the log
for sync or erasure.

### 3.2 Versions and stable cutoffs

Group adjacent changes while both device and user stay the same and the
gap from the previous change is at most `GROUP_MS = 120_000` milliseconds.
A larger gap, different device or different user starts another version.
This sliding rule can group a long uninterrupted autosave session into one
version; it is not a two-minute maximum session length.

A version id is the canonical HLC of the group's last change. HLC order
is `(wall_ms, counter, device)`; the encoded form is
`{wall:016x}-{counter:08x}-{device}`. Parse supplied ids as
[Hlc](../../crates/core/src/hlc.rs), rather than trusting string input.
List newest first; each entry reports time, author, sorted unique written
columns, Create/Delete/Restore flags and resulting `in_trash`.

The state at an id includes every row change whose HLC is less than or
equal to that id, even if that id later stops being a group boundary.
Peers with the same change set and schema compute the same state. Groups
can split or merge when offline changes arrive; previously returned ids
remain usable cutoffs. An id is not a cryptographic integrity token.

The core `row_at` accepts any valid HLC cutoff, returning None if no row
change exists at or before it. Public commands additionally require that
the cutoff equals an actual HLC in this row's applied log. Do not require
it to remain a current group boundary. This prevents fabricated future
timestamps from bypassing retention while preserving previously listed
ids.

### 3.3 Rebuild values in one pass

Use the current [schema](../../crates/core/src/schema.rs). Include every
declared column, starting with its default or null. Newly added columns
therefore appear with their current declared default even in old versions;
this is not historical-schema reconstruction. Rules and defaults of
existing columns cannot change under additive migration.

- LWW: apply Set in HLC order, so the last Set wins. `base` supports
  conflict diagnostics, not a separate value-selection rule.
- Counter: default (null means zero) plus every delta through the cutoff.
  Preserve integers with checked integer arithmetic where possible; use
  real arithmetic for real values and reject overflow/non-finite results.
  Do not silently wrap integers. Inserted counter values are deltas too.
- Text: keep one Loro document per text column; base64-decode and import
  TextEdit updates in HLC order. Read plain text at requested boundaries.
  Never call `text::edit` during replay or show `__crdt`. Before the first
  update, use the schema default/null; after it, use the document text.
  Loro dependencies normally precede edits because the clock observes
  imported HLCs; Loro also supports pending dependencies.
- Trash: latest Delete means in trash; latest Restore means live. This
  deliberately omits edit-wins concurrency details. `in_trash` describes
  this simplified historical state, not a guarantee about current state.

Unknown columns are not exposed. Pending newer-schema ops are not read.
Replay is one pass over operations, with cost also dependent on Loro work
and text copied at boundaries; it is not constant time per byte of text.
Gone rows can have readable historical values, but are never restorable.
Replay after terminal removal can include logged late edits that never
materialized; do not describe such previews as exact former live state.

### 3.4 Restore: write selected values as a new edit

Reject append-only tables and current missing/gone rows, and reject a
cutoff with no prior changes. Perform all mutations in one Store::write
transaction. If currently trashed, call Writer::restore first, regardless
of the historical `in_trash` flag. Then update only differing LWW/text
columns and increment counters by `target - current`. Restore copies
values and brings the item back; it never moves a live item into trash.
The historical trash flag is informative. This avoids trying to update a
trashed row, which Writer correctly refuses.

Normalize boolean values through the store's SQL representation before
comparing (`true` and SQLite `1` are equivalent). Text null means empty
string on write, matching Writer::text_edit. Counter null means zero;
checked subtraction must reject overflow before any changes commit.
Read current values and calculate differences without invoking plugin code.
Return sorted changed column names; a trash-only restore may return an
empty list despite emitting Restore.

Only Restore, Set, TextEdit and Increment are emitted. Never replace CRDT
blobs, rewrite old changes, or restore `_deleted` directly. Concurrent
remote edits still merge normally; a counter delta or text restoration
cannot promise to overwrite unknown offline edits. A restore can group
with a nearby edit under the ordinary grouping rule; it need not create a
separate visible version. Earlier cutoff ids remain readable.

### 3.5 Visibility: explicit table opt-in

Add manifest `"history": ["tasks"]`, defaulting to an empty list, with
validation for declared standard tables and duplicates. Reject append-only
tables. All declared columns of an opted-in table become readable; there
is no column allowlist in this feature. Plugin authors must not opt in a
table containing fields they intend to hide behind their named queries.
This explicit choice is necessary because History bypasses those queries.

Only the host reads history. Validate plugin, table, row and selected space
for every command. A template cannot choose another plugin; its bridge
provides the originating plugin identity. No database access or another
plugin's data is handed to Wasm. Host stores remain excluded.

### 3.6 Commands

Reserve host namespace `item` in [the command registry](../../crates/runtime/src/commands.rs).
Register these space-scoped commands so existing CLI/MCP discovery works:

- `item.history {plugin, table, id, version?, limit?}`: Query, Read.
  Without version, return `{versions, this_device, keeps_days}`; default
  limit 50, valid range 1..200. An unknown item with no changes returns an
  empty list. With version, return `{version, values, in_trash}` or
  not-found. Apply manifest and retention checks to both paths.
- `item.restore_version {plugin, table, id, version}`: Action, Write.
  Return `{restored: version, columns}`. Read-only clients cannot invoke
  it. UI asks before restoring; it is an ordinary write, not a destructive
  command. Emit data-changed after success, including in the native app.

Malformed ids/limits and expired versions produce `invalid`; missing or
ineligible targets produce `not-found`. No plugin code runs. Preserve the
existing core HistoryOptions, Version and RowAt types proposed in HIST-02.
Add a shared runtime cutoff check using actual row change membership for
both commands; a raw core `row_at` result alone does not validate an id.

### 3.7 Retention: visibility, not deletion

Store `history_days` on table `spaces` in synced host store `_spaces`.
Advance its schema from 1 to 2, adding only this integer column with
default 0. Allowed values are 0 (Forever), 365 (1 year), 90 and 30.
Expose `space.set_history {id, days}` and Settings → Spaces, labelled
“Keep history for”. This is shared preference data, so it follows normal
LWW sync and may differ temporarily while devices are offline.

Compute one cutoff per request as `now_ms.saturating_sub(days *
86_400_000)`; 0 means no cutoff. Include versions whose last HLC wall
time is at or after it. Always keep the newest group visible as “Current”,
even if older. An expired current entry is a display exception only:
requesting its values or restoring it by id still fails the age check.
Disable restore for that entry. Apply age checks to direct detail and
restore requests, including ids no longer in the displayed groups.
Group first, filter second, limit last. A recent group can include older
ops and its preview necessarily includes older unchanged values.

No log, conflict, Loro, pending, backup or sync data is deleted. Reducing
retention hides old cutoffs; increasing it can reveal them again. Do not
call purge or prune_conflicts as part of this setting. Real erasure needs
the separate SEC-03 change-log compaction work. HLC wall time is affected
by clock skew, so this preference is not an erasure deadline or security
boundary against local tools.

### 3.8 Authorship

Show “This device” or “Another device” based on the HLC's device, not the
raw id. Show user only when it differs from the host's current user.
Supply the current-user context from the host to the UI as needed; never
guess it from the first version. API version ids and device fields still
contain device identifiers for comparison; the labels are presentation,
not anonymization. Authors are change metadata, not verified identities.

### 3.9 Limits and measurement

Return at most 200 versions (core clamps 1..200; commands reject invalid
limits). Replay the row's full log even when returning a small page; a
recent text update alone cannot reconstruct earlier dependencies. Do not
use the ordinary query path's 10,000-row cap to truncate replay silently.

On 2026-11-11, a temporary Rust harness using the checkout's pinned Loro
1.17.1, base64, rusqlite and Op/Hlc types measured 20,000 synthetic note
autosaves. Each changed a five-digit suffix in a roughly 2 KB note at
one-second intervals, exported a real Loro update, and stored full op JSON
in a temporary indexed SQLite log. Final text was 2,115 bytes; fixture
database was 6,672,384 bytes. On this arm64 macOS host, unoptimized Cargo
dev build, three passes read ordered rows, parsed HLC/JSON, decoded updates,
imported them into one Loro document, and checked final text equality.
All edits form one group under the chosen rule.

Total replay times were 1.654 s, 1.530 s and 1.531 s (median 1.531 s).
Ordered SQL reads accounted for 47.2 ms, 29.0 ms and 32.2 ms respectively.

This measures the proposed replay algorithm, not an implemented history
command or phone performance. It excludes fixture generation, RPC and UI;
it uses a warm local file and one final text materialization. Many groups,
large note bodies and adversarial updates can cost more. Accept full replay
for this first implementation, keep the UI responsive with loading/error
states, and measure on a phone before considering caching or a worker.
No phone latency claim is made.

### 3.10 UI entry point

A template action such as
`{"history":{"table":"tasks","id":"{{row.id}}","title":"{{row.title}}"}}`
opens a host-owned History dialog through the renderer bridge. To-do and
Notes opt in and add History buttons to item sheets. No plugin JavaScript.

Show newest versions first, plain field values and historical trash state.
Ask with terrariumConfirm before “Restore this version”; reload and refresh
views on success. Use textContent/DOM APIs for values, never innerHTML, and
do not expose hidden storage columns. Keep keyboard focus/escape handling
and mobile layout consistent with host dialogs. Close or ignore stale
responses when the space changes. Show an honest retention explanation.
An expired Current entry must explain that its historical preview is
unavailable; the live item remains available in its normal sheet.

## 4. Sync and compatibility

No new Op variants, Change fields, WIT imports or plugin API version.
HIST-02 needs no schema migration. Old peers can apply restoration writes
for a schema they already understand. No cross-plugin schema dependency.

There are two distinct upgrade constraints:

- New manifests use `history`, which older hosts reject because Manifest
  denies unknown fields. Upgrade all paired hosts to HIST-03 support before
  distributing HIST-04b plugin packages through the shared plugin set.
  New hosts still accept old manifests because history defaults empty.
  Keep old plugin packages until the host rollout is complete; do not
  silently strip the declaration or claim older hosts can load it.
- `_spaces` schema 2 introduces history_days. Old hosts buffer version-2
  space changes in `_pp_pending` until upgraded, including other space
  updates written under schema 2. They neither expose nor enforce History
  retention. Upgrade every peer before relying on uniform visibility.
  Downgrading a host against an already migrated local store is not
  promised; additive schema compatibility is not downgrade support.

An existing plugin id `item` will conflict with the new reserved namespace;
audit installed packages before rollout and report the conflict rather
than reinterpreting plugin commands as host commands. Manifest table opt-in
and retention are host policy, not changes to merge semantics.

## 5. Privacy

History intentionally broadens access for opted-in tables: authorized Read
clients and the assistant can read earlier raw column values, even values
the plugin's normal queries omit. The existing command caller permissions
still apply. Historical text is untrusted user content, not instructions
for the assistant. This task adds neither a new assistant capability nor
permission for it to write.

Use the same checks for UI, CLI, MCP and assistant calls. Do not put item
values or text update payloads in diagnostics or logs; do not render raw
device ids as UI labels. Hide storage metadata and CRDT blobs. Values may still exist after trash
purge and on offline devices or backups. Retention is a History visibility
filter only; encrypted transport/storage does not erase historical data.

Suggested Settings explanation: “Older versions are hidden from History
and can’t be restored. They stay in the space’s sync data on your devices.”
Mention that Current may remain listed without an old historical preview.

## 6. Tasks

The feature task sequence is retained below. Task briefs are kept in the
coordinator's separate tasks directory, outside this worktree.

| ID | Task | Depends | Size | Model |
| --- | --- | --- | --- | --- |
| HIST-01 | Design: log replay vs snapshots, versions, retention, privacy | none | S | strong |
| HIST-02 | Core: read a row's versions from the change log, restore to one | HIST-01 | M | any |
| HIST-03 | Manifest `history`, `item.history` and `item.restore_version` commands | HIST-02 | M | any |
| HIST-04a | History sheet in the shell, `history` template action | HIST-03 | M | any |
| HIST-04b | Plugins: declare `history`, History buttons in To-do and Notes | HIST-04a | S | any |
| HIST-05 | Retention setting per space (what History shows and restores) | HIST-03 | S | any |
| HIST-06 | Docs | HIST-04b, HIST-05 | S | any |

## Differences from the task files

1. HIST-02 step 4: restore any currently trashed recoverable row before
   writing values, including a historically trashed target. Do not copy
   historical deletion state. Add live→trashed-target and
   trashed→trashed-target tests, and assert atomic failure for gone rows.
2. HIST-02 steps 3–4: row_at uses the requested cutoff, not only group ends.
   Add a test retaining an id after a late remote edit changes grouping.
   Use checked counter arithmetic and test overflow without partial writes.
3. HIST-03: validate public ids against actual row change HLCs while still
   allowing former group ends. Reject fabricated future cutoffs and ids
   from a different row. Core row_at keeps its general cutoff semantics.
4. HIST-05 step 6 checks retention for item.history detail requests as well
   as restore. Test expired detail/restore ids, unknown future ids, exact
   cutoff equality, and the newest-expired display exception. Capture now
   once per request and subtract with saturation.
5. HIST-04a/HIST-03: include host current-user context when implementing the
   differing-user label. Never render device identifiers as labels. Test
   same-user suppression and differing-user display.
6. HIST-04a/HIST-05: keep expired Current visible but refuse its historical
   detail and restoration; show an explanation rather than a generic error.
7. The referenced docs/design/assistant-tools.md is absent in this checkout.
   Existing command permissions govern the proposed assistant access;
   do not invent a dependency on that missing document. SEC-03 remains
   separate roadmap work; this design does not implement erasure.

Human review remains pending as recorded above, with authorization to proceed.

## As built

HIST-02 through HIST-05 implement the design above; HIST-06 documents it.

- [Core replay](../../crates/core/src/history.rs) groups changes by device
  and user with the sliding two-minute gap, replays LWW/counter/Loro values,
  and applies visibility cutoffs. [Store methods](../../crates/core/src/store.rs)
  read the row log and restore through a transaction of ordinary operations.
  [Core tests](../../crates/core/tests/history.rs) cover replay, grouping,
  retained cutoff IDs and restore behavior.
- [Manifest validation](../../crates/runtime/src/manifest.rs) accepts the
  optional `history` table list. [Host commands](../../crates/runtime/src/commands.rs)
  reserve `item`, validate actual change IDs, enforce read/write access and
  retention, and expose `item.history`, `item.restore_version` and
  `space.set_history`. The list response includes `keeps_days`.
- [Spaces](../../crates/runtime/src/spaces.rs) uses schema 2 with synced
  `history_days` (0, 30, 90 or 365). [Runtime tests](../../crates/runtime/tests/commands.rs)
  and [space tests](../../crates/runtime/tests/spaces.rs) cover command
  validation and the additive migration.
- [Renderer actions](../../ui/renderer.js) pass the history target through
  the originating plugin’s bridge. The [shell](../../ui/app.js) owns the
  History dialog, device labels, confirmation, field previews and the
  per-space **Keep history for** control; [Settings](../../ui/index.html)
  explains that older versions stay in sync data.
  [UI tests](../../ui/tests/history.test.cjs) cover history interactions.
  [Native command wiring](../../apps/tauri/src-tauri/src/plugins.rs) emits
  `data-changed` after restoration; the
  [control service](../../crates/host/src/control.rs) also publishes changes
  for CLI/MCP writes.
- In the sibling `terrarium-plugins` repository, To-do and Notes opt in via
  `packages/todo/manifest.json` and `packages/notes/manifest.json`, and add
  History buttons in their respective `ui/page-main.json` item sheets.
  These packages require HIST-03 hosts; uniform retention needs HIST-05 on
  every peer. No WIT or sync operation format changed.

Differences and limits observed in the implementation:

- The dialog requests up to 100 versions, within the command’s 200 maximum.
  Long field values initially show 2,000 characters with **Show all**.
- The dialog labels authors by device only. The differing-user label in
  section 3.8 is not implemented: the list response has `this_device` but
  no host current-user context. Version metadata still includes `user`.
- The newest entry is **Current** and has no restore button. If expired,
  it remains listed with an explanation; detail and restore commands still
  reject its cutoff. A restore may join an existing group.
- Retention is visibility only. No log compaction, secure erasure or phone
  performance validation is included; the measurement in section 3.9
  remains a design-time harness result, not runtime command latency.
