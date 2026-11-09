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
| `crates/runtime` (`terrarium-runtime`) | Wasmtime component host, manifest validation, limits, package install/update/rollback |
| `crates/sync` (`terrarium-sync`) | Encrypted, signed change batches, pairing, peer HTTP and Google Drive transports, snapshots |
| `crates/scheduler` (`terrarium-scheduler`) | Scheduled plugin operations, reminders, run history |
| `crates/ai` (`terrarium-ai`) | Text generation through the user's OpenAI-compatible provider |
| `crates/cli` (`terra`) | Developer CLI: `new`, `build`, `check`, `dev`, `pack`, `verify`, `keygen`, `preview` |
| `crates/guest` (`terrarium-guest`) | Rust SDK for writing plugins |
| `wit/` | `terrarium:plugin@0.1.0` WIT contract between host and plugins |
| `ui/` | Shared web shell and declarative template renderer |
| `apps/tauri` | One Tauri 2 app for macOS and Android |
| `vendor/loro-internal` | Reviewed Loro backport, see [dependency review](security/dependency-advisory-review.md) |

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

## Declarative UI

Pages and widgets are **templates shipped as data**. The host renders them,
themed and responsive, by running the plugin's declared read-only queries.
Rendering never calls plugin code; only user actions do. No plugin JavaScript
runs in the shell. See [plugin authoring](plugin-authoring.md) for components
and bindings.

## Storage

- Each plugin gets its own SQLite database per space.
- Reads are SQL restricted to the plugin's own database. Writes go through the
  host storage API so every change is logged and mergeable.
- Primary keys are UUIDv7 generated on the device. There are no UNIQUE
  constraints or enforced foreign keys, so offline edits never make sync fail.
- Migrations are additive only; old changes from other devices must stay valid.

## Sync

Every write records a change `(space, plugin, table, row, column, value, hlc,
device, user)`. Devices group changes into numbered batches, encrypt them with
the space key (XChaCha20-Poly1305) and sign them with the device key (Ed25519).
Any transport that can `put`, `list` and `get` opaque files can carry them;
transports never see plaintext and never merge.

| Merge rule | Behavior | Example |
| --- | --- | --- |
| `lww` (default) | Last writer by HLC wins | task title, status |
| `counter` | Increments from all devices add up | habit counts |
| `append` | Insert-only rows, never conflict | voice captures, history |
| `text` | Loro text CRDT; concurrent edits merge | note body |

Deletes are tombstones resolved by a per-table policy. The default, `trash`,
keeps a concurrent edit and leaves the record in trash with a restore prompt;
`delete-wins` and `edit-wins` are available for derived or special data.

Pairing uses a QR code with a one-time secret; the space key is wrapped to the
new device's X25519 key. A recovery phrase wraps the space key at creation.

## Spaces

A user can have several spaces (Personal, Work, …). They share one plugin list,
one pairing and one recovery phrase; only the data is per space. Every record
carries `space` and `user_id` so shared team spaces need no data migration.

## Platform notes

- macOS and Android arm64 run Wasmtime. iOS would need the Pulley interpreter
  and is not a current target.
- Android builds are sideloaded; app-store policy for downloadable code has not
  been reviewed.
- Reminders are scheduled as local notifications on the device chosen per
  reminder (`phone`, `desktop` or `both`).

Security boundaries and known gaps: [threat model](security/threat-model.md),
[SECURITY.md](../SECURITY.md) and the [roadmap](roadmap.md).
