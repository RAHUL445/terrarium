# Architecture

Terrarium is a local-first plugin host. Every device runs the complete host and
owns a full copy of its data; nothing requires the network. Features are added
as sandboxed WebAssembly plugins, and devices sync end-to-end encrypted.

```
            ┌──────────────── host (Rust) ─────────────────┐
            │ plugin runtime · storage + change log · sync │
            │ permissions · scheduler · AI · UI renderer   │
            │ packages (install / update / rollback)       │
            └──────────────┬──────────────────────┬────────┘
                           │                      │
                 Tauri desktop app        Tauri Android app
                 macOS                    Android
                           │                      │
                           └──── batch store ─────┘
                     direct peer over Tailscale (both online)
                     Google Drive appData folder (any time)
```

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/core` (`terrarium-core`) | Per-plugin SQLite storage, change log, hybrid logical clock, merge rules, delete policies |
| `crates/runtime` (`terrarium-runtime`) | Wasmtime component host, manifest validation, limits, ranked search, package install/update/rollback |
| `crates/sync` (`terrarium-sync`) | Encrypted, signed change batches, pairing, peer HTTP and Google Drive transports, snapshots |
| `crates/scheduler` (`terrarium-scheduler`) | Scheduled plugin operations, reminders, run history |
| `crates/ai` (`terrarium-ai`) | Text generation through the user's OpenAI-compatible provider |
| `crates/cli` (`terra`) | Developer CLI: `new`, `build`, `check`, `dev`, `pack`, `verify`, `keygen`, `preview` |
| `crates/guest` (`terrarium-guest`) | Rust SDK for writing plugins |
| `wit/` | `terrarium:plugin@0.1.0` WIT contract between host and plugins |
| `ui/` | Shared web shell and declarative template renderer |
| `apps/tauri` | One Tauri 2 app for macOS and Android |
| `vendor/loro-internal` | Reviewed Loro backport, see [dependency review](../security/dependency-advisory-review.md) |

Plugin implementations live in the separate
[`terrarium-plugins`](https://github.com/RAHUL445/terrarium-plugins) repository.

## Plugins

- Plugins are Wasm **components** linked only against the `terrarium:*` WIT
  interfaces: no WASI, no ambient file or network access.
- Each operation runs in a fresh instance with fuel, a wall-clock deadline, a
  memory limit and a host-call quota, inside one storage transaction that
  commits only on success.
- A package is a component plus a manifest (id, version, capabilities, storage
  schema with merge rules, queries, pages, widgets, jobs) and assets. Packages
  are signed with Ed25519; installs and capability-expanding updates require
  user consent, and the previous version is kept for rollback.
- The installed plugin set is part of space state and syncs between devices.

## Events and automations

Plugins declare typed events and emit through WIT; the host holds them until
that operation commits. The synced host stores `_events` (bounded event log) and
`_rules` (per-space rules, settings and run history) are inaccessible to plugins.
Only the explicitly chosen primary device, locally armed and with locally
approved rules, invokes mapped plugin actions. Root depth is 0; events at depth
3 stop further actions. Fan-out and local hourly limits bound work. A durable
local claim before invocation gives at-most-once attempts per device, converging
across devices to one run-history row, but offline primaries can both act.
Plugin commits and event persistence are separate transactions, so crash loss is
possible; actions are never automatically retried. Consent, claims and arming
stay local. See the [design](../design/events-and-automations.md) and
[user guide](../guide/automations.md).

## Declarative UI

Pages and widgets are **templates shipped as data**. The host renders them,
themed and responsive, by running the plugin's declared read-only queries.
Rendering never calls plugin code; only user actions do. No plugin JavaScript
runs in the shell. Host-built views also stay declarative: the host turns
manifest `view` declarations into ordinary `list`, `table`, `board` or
`calendar` templates before rendering. See
[plugin authoring](plugin-authoring.md) for components and bindings.

## Storage

- Each plugin gets its own SQLite database per space.
- Reads are SQL restricted to the plugin's own database. Writes go through the
  host storage API so every change is logged and mergeable.
- Attachments are encrypted blobs in a host-owned blob store per space. Rows
  reference blobs by id through `file` column values and markdown
  `terrarium:blob/<id>` links.
- Plugin code never receives attachment bytes directly. It only reads and
  writes file-reference values; byte upload/download is host-mediated.
- Primary keys are UUIDv7 generated on the device. There are no UNIQUE
  constraints or enforced foreign keys, so offline edits never make sync fail.
- Migrations are additive only; old changes from other devices must stay valid.

## Search

The host keeps one SQLite FTS5 index per space at `<data>/index/<space>.db`,
where `<data>` is the default space's plugin database directory
(`HostConfig.data_dir`). It follows each store's change log and reads rows
through the plugin's declared index queries. Bounded steps in the app,
`terrarium serve` and `terra preview`, plus catch-up before a search, keep it
current. Only the host reads the index; plugin code cannot access it.

The index is local only: it is not synced or backed up, and can be rebuilt
from the stores. Ranked search across plugins moved into the host's small
kernel so it can combine results without giving plugins each other's data.

The indexer also extracts `terrarium:` Markdown links from indexed titles,
keywords and bodies into a local `mentions(doc, to_ref)` table joined to
indexed documents. `ref.backlinks` catches up indexed plugins and reads
these mentions; it does not read stored relations. Mentions are replaced
with each re-index and removed with their source documents. They are local
derived data, rebuilt from plugin text and never synced or backed up.

## Sync

Every write records a change `(space, plugin, table, row, column, value, hlc,
device, user)`. Devices group changes into numbered batches, encrypt them with
the space key (XChaCha20-Poly1305) and sign them with the device key (Ed25519).
Any transport that can `put`, `list` and `get` opaque files can carry them;
transports never see plaintext and never merge.

Attachment transfer runs after change-batch sync: devices fetch missing blobs
on demand when rows or markdown references point to them. While bytes are still
downloading, the UI shows "Not on this device yet". Blob upload/download uses
the same end-to-end encryption key material and verifies ids against decrypted
content before accepting bytes.

| Merge rule | Behavior | Example |
| --- | --- | --- |
| `lww` (default) | Last writer by HLC wins | task title, status |
| `counter` | Increments from all devices add up | habit counts |
| `append` | Insert-only rows, never conflict | voice captures, history |
| `text` | Loro text CRDT; concurrent edits merge | note body |

Deletes are tombstones resolved by a per-table policy. The default, `trash`,
keeps a concurrent edit and leaves the record in trash with a restore prompt;
`delete-wins` and `edit-wins` are available for derived or special data.

Host-owned stores participate in the same store set as plugin databases:
`_packages` (installed packages), `_spaces`, `_pages`, `_links` (item
relations), `_ai` and `_scheduler`. Sync and backups enumerate that set.
`_links` is one device-wide store with a `space` column and schema version
1; future migrations are additive. Relation deletion uses trash. Offline
duplicate relations remain mergeable rows; listing keeps the smallest id
per source/target/kind. Local search indexes and mentions are excluded.

Pairing uses a QR code with a one-time secret; the space key is wrapped to the
new device's X25519 key. A recovery phrase wraps the space key at creation.

## Spaces

A user can have several spaces (Personal, Work, …). They share one plugin list,
one pairing and one recovery phrase; only the data is per space. Every record
carries `space` and `user_id` so shared team spaces need no data migration.

## Platform notes

- macOS and Android arm64 run Wasmtime. iOS would need the Pulley interpreter
  and is not a current target.
- Deep links use the `terrarium://open/...` scheme registered by the bundled
  app (`Info.plist`) and by Android intent filters. The app parses links in
  `apps/tauri/src-tauri/src/links.rs`, then the UI drains queued links before
  navigation.
- Quick capture is desktop-only: a global shortcut opens a separate `quick`
  window (`ui/quick.html`) and uses the `quick-capture` capability file
  (`apps/tauri/src-tauri/capabilities/quick.json`).
- Android text sharing is routed through `MainActivity.kt`, which rewrites
  `ACTION_SEND` text into `terrarium://share?...` links handled by the same
  link queue.
- Android builds are sideloaded; app-store policy for downloadable code has not
  been reviewed.
- Reminders are scheduled as local notifications on the device chosen per
  reminder (`phone`, `desktop` or `both`).
- The current home-screen widget direction is tracked in the design note
  [home widgets](../design/home-widgets.md).

Security boundaries and known gaps: [threat model](../security/threat-model.md),
[SECURITY.md](../../SECURITY.md) and the [roadmap](../roadmap.md).
