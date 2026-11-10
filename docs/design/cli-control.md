# Design: controlling Terrarium from the command line

Status: proposal with the open questions settled (see section 4). Nothing
is built yet.

Goal: everything a person can do in the UI (create and edit pages, add and
update to-dos, search, and so on) can also be done from a command line. The
command line is for power users and for AI agents. It uses the same
contracts as the UI, so it can never do more than the UI and never bypasses
plugin logic.

## 1. What exists today

### The UI is already contract-driven

The webview reaches the host only through Tauri commands, wired up in
`apps/tauri/src-tauri/src/lib.rs`. There are three kinds:

| Kind | Commands | Shape |
| --- | --- | --- |
| Plugin data | `run_query`, `run_action`, `view_template`, `plugins` | Generic: `(space, plugin, name/op, JSON)` |
| Host service | `ai::service` (and `service_net`) dispatches about 30 string commands, shared with `terra preview` (`crates/runtime/src/services.rs`) | Generic: `(command, JSON)`, e.g. `page_create`, `pages_list`, `schedules`, `ai_settings` |
| Typed host commands | spaces, install, rollback/remove, themes, sync, pairing, backups, Drive | One Tauri command each, typed arguments |

Every plugin change already goes through `PluginHost::call_in(space,
plugin, op, args)` and every read through `query_in(...)`. These are the
natural entry points for a CLI. Nothing new is needed underneath them.

### The plugin contract is not machine-describable

Each manifest declares `operations`, `queries`, `search`, `quick_actions`,
`pages` and `widgets`. The recommended plugins expose about 60 operations
and 70 queries in total. Three gaps matter for a CLI or an agent:

1. **Operations have prose only.** `OperationDef` is `{ description }`. The
   argument names, types, required fields and result shape exist only in
   each plugin's Rust (`args.str("title")`) and in UI templates
   (`"action": {"op": "toggle", "args": {"id": "{{row.id}}"}}`). An agent
   has to guess.
2. **Query params are bare names.** `QueryDef.params` is a list of names
   with no types or descriptions. Several are really context the host
   should fill in: `today`, `utc_offset_ms`, `now_ms`, `weekday`.
3. **There are no effect markers.** Nothing says whether an operation is
   read-only (`completed_between`, `summary`), a normal write, or
   destructive (`delete`, `clear_done`). The UI keeps its confirmations
   inside templates (`"confirm": "Hide all completed tasks?"`).

### Search lives in the UI

The manifest's `search` names a query that takes `q` and returns rows with
`title` and `subtitle`. The fan-out across plugins and spaces runs in
JavaScript (`ui/app.js`, around line 1894), so there is no host command for
search. The rows also don't say how to open the result.

### Host-owned objects

Pages are owned by the host (`crates/runtime/src/pages.rs`, store `_pages`):
a grid of widget placements per space, a fixed Home page, at most 50 pages
and 40 placements each. Spaces, schedules, AI settings, themes, sync and
backups are also host-owned. Plugins can't reach any of them, so the CLI
needs host commands for these.

### Running the app and the CLI together

- One `PluginHost` sits behind a `Mutex` inside the app process. The
  scheduler, AI queue, reminders and the sync server (port 7810) all run in
  that process.
- SQLite uses WAL, but there is no lock on the data folder. If a second
  process opened the same folder, it would run due schedules twice and
  could race the sync merges.
- The UI refreshes when the app emits `data-changed`, `spaces-changed` or
  `plugins-changed`. Changes made from outside would have to emit these
  too.
- A `Change` records the `user` and the device (from its HLC), but not
  where the change came from (UI, schedule, CLI or agent).
- Release builds keep keys in the Keychain under the app's signature. A
  separate CLI binary reading them would trigger Keychain prompts.

**Conclusion:** the CLI should send commands to the running app, which stays
the only process writing the data. A headless mode is possible later, but
only behind an exclusive lock on the data folder.

Android has no shell for users, so the CLI controls the desktop and sync
carries the changes to the phone. Remote control (for example over
Tailscale) is a later, separate decision.

## 2. Proposal

### 2.1 One command registry for UI, CLI and agents

Add a command registry to `terrarium-runtime`. Every capability becomes a
**command** with a description:

```jsonc
{
  "id": "todo.create",              // <namespace>.<name>
  "kind": "action",                 // "query" | "action"
  "owner": "plugin",                // "plugin" | "host"
  "summary": "Create a task.",
  "description": "Dates, times, #tags, !priority … in the title are understood.",
  "params": { /* JSON Schema subset, see 2.3 */ },
  "result": { /* JSON Schema subset, optional */ },
  "effect": "write",                // "read" | "write" | "destructive"
  "confirm": null,                  // text a human must approve, for destructive commands
  "space": "required"               // "required" | "none"
}
```

- **Plugin commands** are generated from the manifest: each operation
  becomes an action, each query becomes a query, under the plugin id
  (`todo.create`, `todo.tasks`, `notes.search`).
- **Host commands** are declared in Rust, under reserved namespaces:
  `app`, `space`, `page`, `search`, `plugin`, `theme`, `schedule`, `ai`,
  `sync`, `backup`, `ui`. Manifest validation must reject plugin ids that
  use these names. (Plugin ids must already start with a lowercase letter,
  so host stores such as `_pages` and `_host` can't collide.)
- There is one entry point,
  `invoke(caller, space, command_id, args) -> Result<Value, CommandError>`.
  It validates `args` against `params`, fills in context params, checks
  `caller` permissions, then calls the existing `call_in`, `query_in` or
  host function. Every Tauri command, `service()` and `terra preview` go
  through it, so the UI, the CLI and agents share one code path.
- `describe()` returns the whole registry as JSON. Agents use it for tool
  lists, the CLI uses it for help and flags, and the UI could use it for a
  command palette.

### 2.2 Host commands (first set)

| Namespace | Commands | Covers |
| --- | --- | --- |
| `app` | `status`, `describe`, `version` | Running state, device, open spaces |
| `space` | `list`, `create`, `rename`, `remove`, `restore` | Same as the existing Tauri commands |
| `page` | `list`, `get`, `create`, `update`, `remove`, `widgets`, `place`, `move`, `unplace` | Wraps `page_*` in `services.rs`; placements are rows with `x,y,w,h` on a 12-column grid |
| `search` | `query {q, space?, plugins?, limit?}` | Search fan-out moves from `app.js` into the runtime |
| `plugin` | `list`, `info`, `install` (file only), `rollback`, `remove`, `restore` | `install` keeps the existing signer and permission checks; a human confirms |
| `schedule` | `list`, `runs`, `run_now`, `retry`, `enable`, `disable` | Wraps the `schedule_*` services |
| `theme` | `list`, `install`, `remove`, `use` | |
| `backup` | `create`, `list` | `restore` stays UI-only |
| `sync` | `status`, `now` | Pairing, recovery phrase and Drive stay UI-only |
| `ui` | `open {plugin, page?, item?}`, `notify {text}` | Drive the visible app (phase 3) |

Some things stay out of reach on purpose: the recovery phrase, AI keys,
pairing codes, restoring a backup, starting over, and trusting signers.
These need a person.

### 2.3 Plugin contract additions (`terrarium:plugin@0.1.0`, optional fields)

All fields are optional, so existing packages keep loading. A command with
no `params` accepts any object and is labelled "untyped" in `describe`.

```jsonc
"operations": {
  "create": {
    "description": "Create a task. …",
    "effect": "write",
    "params": {
      "title":  { "type": "string", "required": true, "description": "Natural language allowed" },
      "due":    { "type": "date" },
      "priority": { "type": "integer", "min": 0, "max": 3 },
      "tags":   { "type": "string" }
    },
    "result": { "id": { "type": "id" } },
    "examples": [ { "title": "Call Sam tomorrow 6pm #work !high" } ]
  },
  "delete":     { "effect": "destructive", "params": { "id": { "type": "id", "required": true } } },
  "clear_done": { "effect": "destructive", "confirm": "Hide all completed tasks?" }
},
"queries": {
  "tasks": {
    "sql": "…",
    "params": ["today", "filter", "q", "tags"],          // unchanged
    "types": {
      "filter": { "type": "enum", "values": ["today", "upcoming", "all", "done"] },
      "q": { "type": "string" }
    }
  }
},
"search": "search",
"open": { "page": "main", "state": { "open": "{{row.id}}" } }   // how a search hit or id opens in the UI
```

- **Types:** a small, closed set that is easy to show as flags and to
  validate: `string`, `text`, `integer`, `number`, `boolean`, `date`
  (YYYY-MM-DD), `time` (HH:MM), `id`, `enum`, `json`. A full JSON Schema
  isn't needed.
- **Context params:** `today`, `weekday`, `now_ms`, `utc_offset_ms` and
  `tz` are filled in by the host, are hidden from the CLI, and can be
  overridden with `--at`.
- **`effect` defaults to `write`.** `terra check` warns when an operation
  named `delete`, `remove` or `clear*` isn't marked `destructive`.
- **Structured errors already exist** (`plugin-error {code, message}`). The
  CLI maps codes to exit codes: `invalid`=2, `not-found`=3, `in-trash`=4,
  and anything else=1.

- **Hidden operations:** the host automatically leaves internal operations
  out of the registry: any operation named as an AI `on-done` callback,
  and any target of a manifest `jobs` entry. A plugin can also hide any
  other operation with `"cli": false`. Hidden operations still work for
  the UI, schedules and interop; only external callers can't see or call
  them.

These additions are data the host reads, not new WIT functions, so the
plugin ABI stays at `0.1.0`.

### 2.4 Transport: a local control socket

- The app listens on a Unix socket in its data folder, `control.sock`,
  mode `0600`, so only the same macOS user can connect. There is no TCP
  port.
- Messages are newline-delimited JSON-RPC 2.0. The methods are `describe`,
  `invoke`, and `subscribe` (which streams `data-changed` with the plugin
  and table for `terrarium watch`).
- After an `invoke` from outside, the app emits the same events the UI
  already listens to, so open windows refresh.
- **On/off:** Settings → Developer → **Command-line control** is off by
  default. The socket doesn't exist until the user turns it on.
- **Named clients with scoped tokens:** the user adds each client in
  Settings → Developer (for example "Claude" or "my-scripts") and picks
  **Read** or **Read and change**. The app shows the token once and keeps
  only its hash. Every request carries a token; a read-scoped token can't
  run `write` or `destructive` commands. Tokens can be revoked one at a
  time. The CLI reads its token from `TERRA_TOKEN` or from
  `terrarium login`, which saves it to an owner-only file.
- **Audit:** each external invoke is logged with its time, client name,
  command and status, but not its arguments, in a host-owned log shown in
  Settings → Developer. `Change` gets an optional `origin` (`ui`, `cli`,
  `agent`, `schedule`) so the change log can show it.
- **Dry run:** a plugin operation already commits all or nothing, so
  `--dry-run` can run it, return the result and the changes it would make,
  and roll back. This is useful for agents. It doesn't apply to operations
  that submit AI requests or set schedules, which happen after commit; for
  those it reports "would queue".

If the app isn't running, `terrarium` says so and exits. Phase 4 can add a
headless host that takes an exclusive `flock` on the data folder, which the
app also takes when it starts.

### 2.5 CLI surface: a separate `terrarium` command

`terrarium` is the user-facing CLI. `terra` stays the plugin developer tool
(`new`, `build`, `pack`, `preview`, …). The two share no commands.

- **Packaging:** a new crate, `crates/control-cli`, builds the `terrarium`
  binary. The macOS bundle ships it inside the app
  (`Terrarium.app/Contents/MacOS/terrarium`, as a Tauri `externalBin`).
  Settings → Developer → **Install command-line tool** links it to
  `/usr/local/bin/terrarium`, or the user adds it to `PATH` themselves.
  It is a thin client: it speaks the socket protocol and never opens the
  data folder.
- **Shared protocol crate:** the request and response types and the
  JSON-RPC framing live in one small crate that both the app and the CLI
  depend on.

```sh
terrarium login                               # paste a token from Settings → Developer
terrarium status
terrarium commands [--json] [namespace]       # what can I do (from describe)
terrarium run todo.create --title "Buy milk tomorrow" [--space work] [--dry-run]
terrarium run todo.update --id 0193… --priority 2
terrarium query todo.tasks --filter today --json
terrarium search "milk" [--plugin notes]
terrarium page create --title Planning
terrarium page place planning todo/today --w 6 --h 4
terrarium space list
terrarium watch [--plugin todo]
terrarium mcp                                 # phase 4: the registry as MCP tools over stdio
```

- Flags come from `params`; `--args '{…}'` or `--args @file.json` work for
  anything, including untyped commands.
- Output is a table on a TTY and JSON when piped (or with `--json`).
  Errors go to stderr as `{code, message}` with the exit codes above.
- Agents use `terrarium` like a person does until MCP lands: `terrarium
  commands --json` is their tool list, and the JSON output and exit codes
  are stable.
- `terrarium mcp` (phase 4, once `terrarium run` is stable) maps each
  command to an MCP tool. The `effect` field becomes hints: `readOnlyHint`
  for read, `destructiveHint` for destructive. The `confirm` text is shown
  to the agent's user. It uses the client's token, so its scope and audit
  name apply.

## 3. Phases

| Phase | Delivers | Main files |
| --- | --- | --- |
| 1. Contract | Command registry, `describe`, manifest `params`/`effect`/`open`, host `search`; the UI moves onto `invoke` | `crates/runtime` (new `commands.rs`), `manifest.rs`, `services.rs`, `apps/tauri/src-tauri/src/plugins.rs`, `ui/app.js` |
| 2. Read-only CLI | Control socket, Settings switch, named read tokens, audit log, the `terrarium` binary in the bundle with `login/status/commands/query/search/watch` | `apps/tauri/src-tauri/src/control.rs` (new), new crates `control-proto` and `control-cli`, `tauri.conf.json` |
| 3. Writes | Read-and-change tokens, `terrarium run`, `--dry-run`, `origin` on changes, `page`/`space` subcommands, `ui.open`/`ui.notify` | `crates/core/src/change.rs`, `crates/control-cli`, `apps/tauri` |
| 4. Agents and later | `terrarium mcp`, a headless host with a lock, remote control over Tailscale | `crates/control-cli`, `crates/runtime` |

Annotating the recommended plugins' manifests (`params`, `effect`, `open`)
can start in phase 1, in `terrarium-plugins/packages/*/manifest.json`.

Phase 1 is useful by itself: the UI ends up with a single path for every
command, and `terra preview` gets the same commands for free.

### Phase 1 as built

- `crates/runtime/src/commands.rs` holds the registry: `describe(host,
  caller)` and `invoke(host, caller, kind, id, space, args, desktop)`.
- A command is identified by **kind and id**, because a plugin may declare
  an operation and a query with the same name (To-do has both a
  `note_settings` operation and a `note_settings` query). The CLI verbs
  `run` and `query` supply the kind.
- `Caller::App` (the UI and `terra preview`) is not checked against
  `params`, so templates that pass extra fields keep working.
  `Caller::Client(Read | Change)` is checked: unknown or mistyped arguments
  are refused, read-only clients get `forbidden` for anything but `read`,
  and hidden commands look like missing ones (`not-found`).
- Automatically hidden: operations named `on_…` (AI callbacks; this is a
  naming convention, because callbacks are chosen at run time). Job
  targets are **not** hidden after all: Reports' `generate` is both a job
  target and something people run from its page. Plugins mark purely
  internal job targets (`habits.plan_reminders`,
  `voice.transcribe_pending`) `"cli": false` instead.
- Host commands so far: `app.describe`, `plugin.list`,
  `space.list/create/rename/remove/restore`,
  `page.list/get/widgets/create/update/save/remove`, `search.query`,
  `schedule.list/runs/run_now/enable/disable`. `page.place/move/unplace`,
  `app.status`, and the theme, backup, sync and ui namespaces come later.
- The Tauri `run_query` and `run_action` commands, and the same commands in
  `terra preview`, now go through `invoke`. A new `command` Tauri command
  runs any registry command, and `describe_commands` lists them. ⌘K search
  calls `search.query` once instead of querying each plugin from
  JavaScript.
- Manifest fields: `effect`, `confirm`, `params`, `result`, `examples` and
  `cli` on operations; `description`, `types` and `cli` on queries; `open`
  on the manifest. The ids in `HOST_NAMESPACES` are reserved.
- The recommended plugins don't describe their commands yet. That work
  happens in `terrarium-plugins` and needs a new pinned bundle.

### Phase 2 as built

- `crates/control-proto` holds the message format: newline-delimited
  JSON-RPC 2.0 with `hello`, `status`, `describe`, `invoke` and `subscribe`,
  4 MiB per message, the token hash, and where the socket is
  (`TERRA_SOCKET`, else `control.sock` in the data folder, which honours
  `TERRA_PROFILE_DIR`).
- `apps/tauri/src-tauri/src/control.rs` runs the socket on desktop builds
  only. It binds with mode `0600`, refuses to start if another copy of the
  app is already listening, replaces a stale socket left by a crash, and
  refuses paths longer than macOS allows (103 bytes). It allows at most 16
  connections.
- Settings live in `control.json` (owner-only, this device only):
  `enabled` and the clients with name, scope, token hash, created and last
  used times. Tokens are `trm_` plus 32 random bytes, shown once.
- `describe` shows a read-only client only `read` commands, so
  `terrarium commands` lists what it can actually run.
- A connection must say `hello` first. Every later request checks that the
  client still exists and control is still on. Turning control off, or
  removing a client, shuts its connections down.
- The audit log is `control-audit.jsonl` (owner-only), trimmed to the last
  200 entries past 512 KB. It records `hello`, `describe`, `subscribe` and
  each `invoke` (command id, kind, ok or error code), never arguments.
- Actions a client runs emit `data-changed` (or `spaces-changed`) so open
  windows refresh. App events are forwarded to `subscribe` connections.
- Settings → Developer → **Command-line control**: the on/off switch, the
  clients with Remove, **Add read-only client** (shows the token once with
  Copy), the bundled CLI's path with a symlink command, and recent activity.
  Only read-only clients can be added in this phase.
- `crates/control-cli` builds `terrarium`: `login`, `logout`, `status`,
  `commands`, `query`, `search`, `watch`. Flags are typed from `describe`.
  Output is a table on a terminal and JSON otherwise, and the exit codes
  are listed in the [command-line guide](../guide/command-line.md).
- The macOS bundle copies `target/release/terrarium` into
  `Contents/MacOS` (`bundle.macOS.files`), built by `beforeBundleCommand`.
  Instead of an "Install command-line tool" button that writes to
  `/usr/local/bin` (which usually needs admin rights), Settings shows the
  path and a `ln -s` command for `~/.local/bin`.

## 4. Decisions

Settled on 2026-11-10.

| Question | Decision |
| --- | --- |
| Name of the CLI | A separate `terrarium` command, shipped inside the app bundle. `terra` stays the developer tool. |
| Default | Off until turned on in Settings → Developer. |
| MCP | After `terrarium run` is stable (phase 4). |
| Client identity | Named tokens per client, scoped to Read or Read and change, revocable, named in the audit log. |
| Hiding operations | Both: the host hides AI callbacks (`on_…`) automatically, and a plugin can mark any operation `"cli": false`. Hiding job targets automatically was dropped in Phase 1 (see above). |
