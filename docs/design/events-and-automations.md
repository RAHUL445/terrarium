# Design: events and automations

Status: as built through EVT-06; documentation completed in EVT-07.
The delivery and trust limits below remain part of the implemented contract.

Review: pending user review (approved to proceed)

The owner authorized completing EVT-01 and proceeding with later tasks without
waiting for human sign-off. This records that authorization, not a completed
human review of this design.

Goal: a task completed with #gym can check in the Gym habit, with explicit
consent, on one chosen device. This document settles implementation choices
for EVT-02 through EVT-07. HTTP actions, AI actions and time triggers are out
of scope. Time triggers remain schedules.

## 1. What exists today

Status: historical baseline before EVT-02; the observations below describe the
starting point, not the current contract. See sections 2–9 for as-built behavior.

These were source observations at design time:

- [WIT](../../wit/plugin.wit) imports AI and scheduling but has no events.
  [HostState and Runtime::call](../../crates/runtime/src/host.rs) hold schedule
  changes until each operation commits; AI submissions enter an in-memory
  queue immediately and are cancelled on rollback. Events copy scheduling.
- `interop::Host::call` runs a separate `Runtime::call`; the inner plugin
  commits independently. `MAX_DEPTH = 4` limits interop nesting, not rule chains.
- [PluginHost and Peers](../../crates/runtime/src/registry.rs) temporarily move
  plugins and services into `Peers`, recover them after `invoke`, and run due
  schedules outside plugin calls. Rule evaluation must also happen outside calls.
  `StoreSet` currently exposes `_packages`, `_spaces`, `_pages`, `_ai` and
  `_scheduler`, plus each space's plugin stores.
- [Scheduler::begin](../../crates/scheduler/src/lib.rs) uses deterministic run
  ids and a ten-minute LWW lease. Offline contenders can both act before their
  rows converge. [RunOn](../../crates/scheduler/src/spec.rs) has `Any` and
  `Desktop`, with phones running only while open; neither means primary.
- [Sync hosting](../../apps/tauri/src-tauri/src/sync.rs) uses `Role::Host` for
  the desktop peer relay. It confers no automation authority.
- [Manifests](../../crates/runtime/src/manifest.rs) reject unknown fields and
  require exactly `terrarium:plugin@0.1.0`. `ParamSpec` already supplies types,
  requiredness, enum values and bounds. [check_args](../../crates/runtime/src/commands.rs)
  validates external callers; app calls currently bypass that validation.
- [Package capabilities](../../crates/runtime/src/package.rs) drive install and
  update consent. [approve_local](../../crates/runtime/src/packages.rs) binds
  device-local approval to package digest and signer, not synced flags.
- [Sync receive](../../crates/sync/src/engine.rs) parks unknown stores, so an old
  peer can retain new host-store batches without interpreting them.
- [Pages](../../crates/runtime/src/pages.rs) demonstrate host-owned synced
  schemas. [Core schemas](../../crates/core/src/schema.rs) give columns LWW
  merging by default and forbid deletion from append tables.
- [Control audit](../../crates/host/src/control.rs) is local JSONL, retaining
  200 entries after crossing 512 KiB. [The threat model](../security/threat-model.md)
  includes malicious plugins and compromised peers; headless `terrarium serve`
  lacks device keys and sync.

The requested style reference `links-and-search.md` is absent from this checkout.
This proposal follows the specified headings and the existing
[CLI design](cli-control.md); it does not depend on a missing implementation.

## 2. Contract

Status: as built.

### Decision 1: one emit function; no ambient cross-plugin permission

Add this interface after `schedule`, and `import events;` to `world plugin`.
Keep the other world imports and exports unchanged:

```wit
interface events {
  use types.{json, plugin-error};
  /// Payload must be an object matching this plugin's manifest declaration.
  /// Held until this operation commits; dropped on rollback and dry runs.
  emit: func(name: string, payload: json) -> result<_, plugin-error>;
}
```

`emit` is enough: plugins cannot subscribe, enumerate rules, read the event log,
or select a recipient. There is no new `permissions` string. The host attributes
plugin and space from the call, never from payload values. A rule approved on
the execution device is the explicit, scoped permission to pass selected fields
to its action plugin. Declaring an event also advertises replication: add an
install/update capability `events:<name>:<payload-sha256>` labelled
“Sync <plugin name>: <event description> for automations”. The hash covers the
canonical payload ParamSpecs. Thus adding an event or widening its payload
requires package capability review before new event data is synced. A standalone
`events` permission would obscure which data can leave the plugin.

### Decision 2: declared typed objects, bounded at emission

The following is an exact manifest fragment; insert it into To-do's existing
manifest without changing its storage schema:

```json
{
  "events": {
    "task_done": {
      "description": "A task was marked done.",
      "payload": {
        "task_id": {"type": "id", "required": true},
        "title": {"type": "string", "required": true},
        "tags": {"type": "string"},
        "completed_date": {"type": "date", "required": true}
      }
    }
  }
}
```

Habits 0.3.0 adds this exact declaration; `partial` is an emitted effective
status even though the check-in operation accepts only done/skipped/cleared:

```json
{
  "events": {
    "checked_in": {
      "description": "A habit was checked in for a day.",
      "payload": {
        "habit_id": {"type": "id", "required": true},
        "title": {"type": "string", "required": true},
        "day": {"type": "date", "required": true},
        "status": {"type": "enum", "values": ["done", "partial", "skipped", "cleared"], "required": true}
      }
    }
  }
}
```

For example, its payload is
`{"habit_id":"habit-gym","title":"Gym","day":"2026-11-11","status":"done"}`.
Neither first event includes task/habit notes or a nonexistent To-do project.

`Manifest.events` defaults to an empty BTreeMap and is omitted when empty;
`EventDef {description: String, payload: BTreeMap<String, ParamSpec>}` defaults
both fields and rejects unknown fields. At most 32 events, 32 fields per event;
event and payload names match `^[a-z][a-z0-9_]{0,39}$`. Descriptions are at most
512 UTF-8 bytes. Reuse `ParamSpec::validate`; reuse a shared `check_args` /
`check_value` with an empty context list to validate actual payloads. Unknown
fields, nonobjects, required null/missing fields and wrong types are `invalid`.
Optional missing/null fields are allowed. The serialized UTF-8 JSON input must
be at most 16,384 bytes; at most 50 accepted emissions per Runtime::call, also
50 across one top-level call including interop. Exceeding either is `quota`.
Every attempt counts toward the existing host-call limit, including dry runs.
Validation errors name fields, never include values. A count-only interface or
untyped JSON would make consent and mapping impossible to check statically.

### Decision 3: keep 0.1.0, upgrade all hosts before event packages

Keep the WIT package and manifest API at `terrarium:plugin@0.1.0`. The new host
links the additive import and continues accepting old manifests/components.
Old hosts reject `events` as an unknown manifest field and cannot link a
component that actually imports the new interface. Release the host to **all**
devices before publishing event-enabled packages or refreshing the synced set.
Do not silently strip declarations or replace packages on old hosts. New hosts
should explain unsupported contract errors as “Update the app to use this
plugin”; already released old hosts retain their existing parser/linker error.

Experiment, 2026-11-11: copied `crates/guest` and WIT to a temporary directory,
added exactly the interface above plus the world import, and built two minimal
`cdylib` plugins with `cargo build --release --target wasm32-unknown-unknown`.
Both implement the SDK `Plugin` trait; one returns null, the other invokes the
generated `events::emit("pinged", "{}")`. The SDK uses wit-bindgen 0.63.0.
Componentized each module with the same `ComponentEncoder` sequence as
`runtime/src/builder.rs`, using wit-component 0.263.0, and decoded actual world
imports with wit-parser 0.263.0. The unused fixture imported only `types` and
world type aliases; it **did not import events**. The used fixture additionally
imported `events`. Both components validated. This is an experiment on this
build toolchain, not a guarantee for every linker/profile. EVT-02 repeats it
with the final SDK and tests pre-change packages against the new host.

Bumping to 0.2.0 with dual bindings would cost two contract paths for an additive
interface. `min_host` would itself be an unknown field to released hosts. Neither
avoids coordinated upgrades of the synced package set. Keep 0.1.0 for this slice
and document this upgrade policy in author and release documentation.

### Decision 4: each independent commit keeps its own events

On successful `core.commit`, move that call's held emissions to `Peers.events`;
on rollback, trap, commit failure or dry run drop only that call's emissions.
A committed inner call's events survive an outer failure, just as its data does.
After the top-level invocation restores all services, drain committed events
**even if the outer result is an error**. Do not evaluate rules until then.
Interop receives the same automation chain context and shared emission budget.
Holding all events hostage to the outer commit would lose notifications of real
inner changes; making interop atomic is a separate redesign.

## 3. Event log and delivery

Status: as built.

### Decision 5: one synced, mutable host log

Use `_events` for all devices, including primary emissions. A local-only log
contradicts primary-device execution; a hybrid creates two recovery paths.
These are host-only records. Plugin code sees only the explicitly mapped action
arguments. The event payload syncs through existing encrypted batches to paired
peers even when no rule uses it, as disclosed by the package capability. No
plugin event may contain credentials, audio, task notes or note bodies.

Exact `_events` version-1 SCHEMA JSON (standard tables, not append):

```json
{
  "version": 1,
  "tables": [
    {"name": "events", "delete_policy": "delete-wins", "columns": [
      {"name": "space", "type": "text"},
      {"name": "plugin", "type": "text"},
      {"name": "name", "type": "text"},
      {"name": "payload", "type": "text"},
      {"name": "contract", "type": "text"},
      {"name": "depth", "type": "integer", "default": 0},
      {"name": "root", "type": "text"},
      {"name": "device", "type": "text"},
      {"name": "emitted_ms", "type": "integer"},
      {"name": "date", "type": "text"},
      {"name": "handled_ms", "type": "integer"}
    ]}
  ]
}
```

`contract` is SHA-256 of the canonical EventDef payload specification at emission.
`date` is the emitting host's local calendar date at `emitted_ms`, frozen before
replication. `root` equals the original event id, inherited by rule actions and
interop; depth starts at 0. Id, device, time, date, contract and chain metadata
are host-generated; user payload keys cannot override them. Unknown/null/invalid
metadata on imported rows makes them ineligible, with a value-free problem.
Actual imported payloads are revalidated, not trusted because emission validates.

A decoded event example (the schema stores `payload` as JSON text):

```json
{
  "id": "ev-desktop-a-019a7190-1111-7000-8000-000000000001",
  "space": "personal", "plugin": "todo", "name": "task_done",
  "payload": {"task_id": "task-1", "title": "Gym", "tags": "gym health", "completed_date": "2026-11-11"},
  "contract": "<64 lowercase hex characters>",
  "device": "desktop-a", "depth": 0,
  "root": "ev-desktop-a-019a7190-1111-7000-8000-000000000001",
  "emitted_ms": 1794393000000, "date": "2026-11-11", "handled_ms": null
}
```

Plugin commit and `_events` insert are separate SQLite transactions. This slice
chooses **best-effort delivery**: a crash or disk failure between them may lose
an event after the plugin write succeeded. Insert failures must produce a local,
value-free delivery warning; do not turn success into an apparent rolled-back
plugin error, or retry plugin operations automatically. A durable outbox in each
plugin store would close this gap but expands core transaction and sync scope;
it is deferred, and no exactly-once claim is permitted.

### Decision 6: stable ids and a local claim before invoking

Event ids are `ev-<emitting-device-id>-<UUIDv7>`, generated once per accepted
emission and kept across insertion attempts. Rule run ids are exactly
`<rule-id>@<event-id>`; the id does not change when a rule is edited. Persist a
device-local claim keyed by run id **before** the action; the host mutex and
exclusive data-directory lock serialize claims. Use a local SQLite database
`automation-local.sqlite` with a unique run-id key and durable transaction
commit. A failed claim write prevents invocation. Store rule digest, claim time
and terminal status; never store payloads/args in this ledger. It is not a new
synced host store. Startup never retries claimed work: an unfinished claim is
reported as failed with code `interrupted` (“Stopped before it finished”).

UUIDv7 gives offline uniqueness without choosing among several plugin-store
`lseq` cursors or allocating a new global sequence. Including the device makes
diagnosis easier; neither that prefix nor UUID time is authentication.

This chooses at-most-once invocation per device, including after a crash; a
crash before the action can therefore lose work. The synced `runs` row is history,
not an execution lock. Two devices with stale primary settings can still both
act; deterministic ids converge to one history row but do not undo duplicate
side effects. Targets must tolerate repetition; the sample check-in uses the
same day/status, and EVT-06 must prevent duplicate logical check-ins. No scheduler
lease or synced `handled_ms` is a distributed exactly-once guarantee.

A valid existing terminal synced run also suppresses a new invocation during
ordinary device reassignment. This is an optimization, not trusted proof of
execution: a compromised peer can fabricate it and suppress work. Local claims
remain necessary even when such history is later deleted or changed.

### Decision 7: bounded useful history, honest retention

Expire all events seven days after `emitted_ms`, including pending ones: an
unbounded offline queue retains private data indefinitely. Future timestamps
more than five minutes ahead wait for clock correction rather than execute.
Process within the seven-day window when primary reconnects; older queued events
show “Expired before it could run”. Pending counts include only valid unexpired
events. Keep the newest 50 run rows per rule and at most 30 days; deleting a rule
retains its history until 30 days using run `space` and `rule_name` snapshots.
Prune at most hourly through Store API deletes. Standard delete-wins tables allow
this; append tables do not. Tombstones and old encrypted batches may retain data:
this is logical retention, not guaranteed physical erasure.

Local claims persist across synced history pruning and peer replay. Bound the
ledger to 100,000 claims per profile; on exhaustion stop new actions with
“Automation history is full” rather than evict replay protection. Reclaiming that
ledger requires a future safe compaction design. History shows time, automation,
status, execution device and fixed error code/message only, never event values.
Payloads are not exposed through CLI, MCP, UI history or audit.

## 4. Primary device

Status: as built.

### Decision 8: explicit selection per space, no automatic failover

Use `_rules.settings` row `settings-<space-id>` with `space`, `primary_device`
and `paused`. Null primary means off. Settings may suggest the known desktop
sync host but never persist that suggestion without user choice; unknown device
kind and unpaired profiles also start unselected. Choosing a phone is allowed
with “Phones run automations only while the app is open.” The chosen device
must also have locally approved each rule and locally armed automations for
that space. This local arming bit is separate from the synced setting, defaults
to false and is granted only by the trusted Settings action on that device.
A synced primary-device change disarms locally; choosing this device or a local
“Start on this device” confirmation arms it again. A peer cannot arm it by
writing a setting. Recheck settings and local authority before each action.

Removed or locally blocked primary devices never execute; other devices show
“Choose a device to run automations”. If merely offline, show “Waiting for
<device>” and queue up to seven days. No failover timer: it could create two
primaries during a partition. Moving execution requires selecting a replacement,
reviewing rules there and arming that device. Old offline devices might keep
working on stale settings until sync; explain this on reassignment. The existing
revocation gap remains; do not promise global immediate stop or leader election.

### Decision 9: no rules in keyless serve; explicit preview exception

`terrarium serve` may run plugin operations and record local events but cannot
run automations without device membership/keys and the local arming check. Do
not infer authority from matching the configured device string. The shared jobs
tick gets an explicit execution-context flag; serve passes false. `terra preview`
may use a development-only local identity and isolated data directory, default
unselected/unarmed, to exercise the full flow after explicit UI consent. It never
uses production device authority. Letting serve run existing locally synced
rules would make reassignment/revocation invisible to that process.

## 5. Rules

Status: as built.

### Decision 10: synced configuration and history, local authority

Exact `_rules` SCHEMA JSON; all fields use default LWW merging:

```json
{
  "version": 1,
  "tables": [
    {"name": "rules", "delete_policy": "delete-wins", "columns": [
      {"name": "space", "type": "text"},
      {"name": "name", "type": "text"},
      {"name": "trigger", "type": "text"},
      {"name": "filter", "type": "text"},
      {"name": "action", "type": "text"},
      {"name": "enabled", "type": "boolean", "default": true},
      {"name": "created_ms", "type": "integer"},
      {"name": "updated_ms", "type": "integer"}
    ]},
    {"name": "runs", "delete_policy": "delete-wins", "columns": [
      {"name": "space", "type": "text"},
      {"name": "rule", "type": "text"},
      {"name": "rule_name", "type": "text"},
      {"name": "digest", "type": "text"},
      {"name": "event", "type": "text"},
      {"name": "status", "type": "text"},
      {"name": "device", "type": "text"},
      {"name": "error", "type": "text"},
      {"name": "started_ms", "type": "integer"},
      {"name": "finished_ms", "type": "integer"}
    ]},
    {"name": "settings", "delete_policy": "delete-wins", "columns": [
      {"name": "space", "type": "text"},
      {"name": "primary_device", "type": "text"},
      {"name": "paused", "type": "boolean", "default": false}
    ]}
  ]
}
```

Ids and deletion metadata are supplied by CoreStore, not user columns. Rule ids
are host-generated UUIDv7 with prefix `rule-`; JSON-valued columns store compact
JSON text. All writes use `insert/update/delete/upsert` within `store.write`.
Restore is not offered for these tables. A new rule gets a new id; recreate no
removed rule id. Limit 100 live rules per space, name 1–60 Unicode characters,
10 conditions, full input at most 8 KiB. Parse stored records as strictly as new
ones; compromised peers can bypass create-time limits. Concurrent column edits
may form a hybrid rule: recompute the digest and demand new local approval.
A synced approved flag would turn untrusted configuration into authority.

Exact `RuleInput` example (space is a command parameter, not input data):

```json
{
  "name": "Gym task done → Gym habit",
  "trigger": {"plugin": "todo", "event": "task_done"},
  "filter": [{"field": "tags", "op": "has_tag", "value": "gym"}],
  "action": {
    "plugin": "habits", "operation": "checkin",
    "args": {"id": "habit-gym", "status": "done", "day": "{{event.completed_date}}"}
  },
  "enabled": true
}
```

`RuleInput`, `Trigger`, `Condition`, `Action` reject unknown fields. `filter`
defaults to `[]`, `enabled` to true. RuleInfo adds `id`, `space`, `approved`,
`problem` (null or a fixed reason), `updated_ms`, `last_run`, and `waiting`.
`approved` is computed locally, never stored/synced. `problem` suspends execution
without mutating synced `enabled`, so disappearance of a plugin on one device
does not disable another device's valid rule.

### Decision 11: ten simple AND conditions, no expressions

Conditions are `{field, op, value}`; `field` names a top-level declared payload
field, not a path or metadata. All conditions must pass; an empty list passes.
`eq`/`ne` compare typed JSON values with no string/number coercion. An absent or
null field makes every comparison false, including `ne`; `exists` alone tests
presence and non-null, requires a boolean `value`, and compares that result to
it (`false` tests absence). `contains` uses Unicode lowercase substring on
string/text fields. `has_tag` uses Unicode lowercase exact tokens: split strings
on Unicode whitespace or comma, strip one leading #, ignore empty tokens; also
accept arrays of strings for fields declared `json`. Its value is one nonempty
tag without whitespace/comma. `gt`/`lt` accept numeric types or validated ISO
dates; dates compare lexically. Strings use no locale-sensitive ordering or
regex. The validator rejects incompatible operators and values before save.
Typed JSON equality is also allowed for `json` fields. No nested traversal,
SQL, template evaluation or arbitrary code is allowed; an expression language
adds a sandbox and makes consent hard to describe.

### Decision 12: one typed, non-destructive plugin action

Eligible targets have `effect: write`, explicit `params` (including an explicit
empty map), `cli != false`, no `confirm` text, and a name not starting `on_`.
Reject untyped, read, destructive, callback and host-command targets. `cli`
is an eligibility marker, not permission to execute. An operation's declared
effect cannot prove benign behavior; ordinary package trust still applies.
All execution stays in the event space; there is no cross-space mapping.

Each top-level arg is a literal JSON value or a whole-string binding
`{{event.<field>}}`, `{{event.$date}}` or `{{event.$plugin}}`. `$date` uses the
frozen emitting date, `$plugin` the host-attributed source id. Do not interpolate
substrings or recursively expand objects/arrays. Any top-level string containing
`{{` or `}}` that is not a valid binding is rejected; plain strings otherwise
remain literal. Missing optional fields map to null and fail if the target
requires them. Pass only mapped args, never the whole event, filter-only fields,
origin metadata or implicit `$run`/idempotency parameters.

At save, require binding type compatibility: same type; string/text/id/date/time/
enum may feed string or text; integer may feed number; anything may feed json;
other conversions are rejected. For enum-to-enum, source values must be a subset
of target values. A required target needs a required source (metadata always
exists). Validate literals with shared `check_args`; bounds and enum membership
are checked again on actual bound values immediately before invocation. Sample
values are not proof of compatibility. This internal `Automation` caller must
use external argument/visibility checks even though the app normally bypasses
them. No arbitrary host service dispatch is available to a rule.

### Decision 13: bounded chains and deterministic work

Root events are depth 0. A rule on depth d invokes its target with event depth
d+1; interop inherits it. Events at depth 3 cause matching eligible rules to get
`stopped` runs and no target calls. The WIT payload cannot reset depth. Never
recursively evaluate inside Wasm or `call_with`; restore `Peers` first, then
process a snapshot of at most 200 pending events per tick (oldest emitted time,
then id). New child events wait for a subsequent tick. This keeps a burst from
holding the host mutex indefinitely. UI actions may request one such bounded
pass; background/preview ticks also request it. No double pass inside nested calls.

Match enabled, locally approved, valid rules by space, trigger and filter; sort
by rule id. At most ten matching rules invoke per event. Write `stopped` runs
with code `fanout` for further eligible matches. At most 60 attempted actions
per rule in a sliding local hour, using the local claim ledger, unaffected by
synced run-history deletions. Further matches become `skipped` with code `rate`.
A local profile-wide ceiling of 600 attempted actions/hour limits many rules or
peer-manufactured root events; excess matches are also `skipped`. Device-clock
rollback does not clear the window (clamp the local accounting clock to its last
value). Rate-limited events are consumed, not deferred bursts. The existing
interop depth limit remains independent. Raising limits is a later product
change, not a per-rule escape hatch.

### Decision 14: one attempt; failures are visible, never silently retried

Do not copy scheduler backoff. Claim, recheck local approval and settings, map
and validate args, invoke, finish local claim, upsert the synced history row,
then mark the event handled after all eligible matches have terminal records.
Persist claims/terminal outcomes before `handled_ms`; on history-write failure,
rebuild history from local claims without reinvoking. Events from closed spaces
stay pending until opened or expired. When primary is selected and armed,
paused events wait; unapproved/disabled/invalid rules are not eligible. On a pass
with no eligible matches mark the event handled: later approval does not replay
it. Approval shows “Only new events will run”; dispatch additionally ignores
events with `emitted_ms` before that local approval time. Expired events never
invoke. This avoids surprising execution of days of old tasks after approval.

`Ok` means `done`; plugin error code `skip` means `skipped`; every other error
(including `retry`, traps and bad mapped values) means `failed`. Action failures
roll back that action's own transaction; independently committed nested calls
remain committed and may emit. Scheduler `$run` result envelopes have no special
meaning here. Store fixed host error codes, not plugin-supplied messages, which
can contain private payload values. No Retry or Run now button for rules. Show a
Settings problem and a shell notification, at most one failure notification per
rule per local hour; notification text is “An automation could not run”. A person
can fix the rule and perform a missed action manually. Automatic retries require
a stronger idempotency contract and are deferred.

## 6. Consent, security and privacy

Status: as built.

### Decision 15: exact local consent, revalidated at use

Store approval ids `<rule-id>:<sha256>` in `rule-approvals.json`, bounded to 1 MiB,
using same-directory temporary file, fsync and atomic persist as Packages does.
Never sync this file or local arming/claims. The digest is SHA-256 of compact UTF-8
canonical JSON: recursively lexicographically sorted object keys, array order
preserved, serde_json number encoding, no whitespace. Its domain is
`{version:1, id, space, trigger, filter, action, source_package, target_package}`,
where each package identity contains digest and signer (unsigned explicit only
in development). Resolve exact current package identities and EventDef/operation
metadata before review; return the digest as a review token. Approval must submit
that expected digest and fail if anything changed since the sheet opened.
Immediately before each action recompute it and check both package approvals.
A synced `approved` flag is never consulted.

The sheet says “Allow this automation?”, describes the trigger, full conditions,
target operation and fixed arguments, lists **exactly mapped event fields** under
“Passed from To-do to Habits”, and separately lists “Used to check conditions”.
Example: “When To-do finishes a task tagged gym, Habits will check in Gym.
Passed from To-do to Habits: completed date. Used to check conditions: tags.
It runs on Desktop. Only new events will run.” Task titles are not claimed to
be passed by the sample rule: they are in the replicated log but not in its args.
Also disclose the seven-day queue and possible duplicates after device changes.
Use trusted host labels and textContent, never HTML from a rule/plugin.

Rename and synced disable do not change the digest; disable also clears the
local execution grant for that rule. Enabling on this device requires a local
Settings gesture, restores that grant only for a still-approved digest, and
resets its new-events cutoff. A synced enable cannot restore a cleared grant.
A newly imported rule starts with no approval/grant. Sync pause=true clears
space arming when observed; clearing pause by sync does not re-arm. This preserves
harmless renames while preventing peers from silently reactivating dormant rules.

### Decision 16: automation is an outside caller with preapproved scope

Add internal `Caller::Automation` enforcement or an equivalent private validated
entry point. It gets no blanket app bypass and cannot invoke rule-management or
other host commands. `confirm` targets are excluded rather than synthesizing
`confirmed:true`. Approving a rule grants recurring authority only to the exact
reviewed mapping and package identities. Ordinary clients cannot manufacture this
caller mode. The host writes `automation-audit.jsonl` for every terminal run from
all entry paths, including immediate UI passes and preview, not just the jobs
tick. Runtime may own the small writer to avoid omissions.

Audit shape is `{at_ms, space, rule, event, plugin, operation, status, code,
change_count}`; identifiers are bounded and validated. No args, payload, rule
name, title, raw result or free-form plugin error. Use private-file permissions,
512 KiB trim threshold and newest 200 entries like control audit. Synced `runs`
provide user history; audit is separate local evidence and neither grants
execution authority. An unwritable audit preflight prevents new action calls;
a post-action audit failure records a fixed local problem and never retries.

### Decision 17: explicit field transfer, no private-event escape hatch

A locally approved rule is the explicit permission exception to plugin isolation:
the host reads a declared event and hands only selected bound values to one
specified target operation. Filters are host-only. Plugins never query another
database, `_events`, `_rules` or approvals, and never receive unbound payloads.
Do not add `private` now: an event promised private must not enter a synced log
at all. Authors should omit sensitive fields/events. A future local-only event
contract needs a separate design; an ignored `private` flag would mislead users.

Threats and limits:

- Malicious plugins can lie about business events and can encode sensitive data
  in declared strings. Type checks are not truth checks. Package consent,
  minimal event declarations, payload quotas and exact mapping bound exposure;
  users still must trust the installed source and target plugins.
- A compromised paired peer can fabricate `_events` and rewrite `_rules`,
  settings and history. Local digest approval prevents new mappings; local
  arming/grants prevent self-authorization. Validate imports again and use local
  claims/rate accounting. The existing sync signature authenticates a peer,
  not a real task completion. **Forged events can trigger an already approved
  rule**, and peers can suppress work or falsify synced history. No claim of
  event attestation or trustworthy global audit is made. The consent boundary
  authorizes recurring reactions to source events, not cryptographic proof of
  source activity. Stronger provenance requires a later signed-event design.
- Loops are bounded by host depth, fan-out, call quotas and local rate ceilings.
  Malicious peer-created depth-zero events bypass causal depth, but not local
  rates/consent. Exhausting budgets can deny service, never widen permission.
- No notifications/logs interpolate raw plugin errors or event values. E2E
  encryption does not hide event payloads from a compromised authorized peer.
  Existing incomplete device revocation remains a gap.

### Decision 18: package changes suspend until reviewed

Missing source plugin/event or target plugin/operation, incompatible fields,
unapproved package or changed current package identity suspends a rule with a
computed reason. Revalidate on package load/update/remove, rule reads and every
execution. Any package digest/signer change requires rule review even if shape
is unchanged; comparing only field names would miss changed code behavior.
Queued events whose `contract` differs from the current source payload signature
are consumed without action and get a value-free incompatible-event problem.
Reviewing an updated rule starts from new events; no replay of the old queue.
This is deliberately stricter than auto-accepting additive payload changes.
Do not mutate or drop existing plugin data on upgrade or uninstall.

## 7. UI and tooling

Status: as built.

### Decision 19: Settings → Automations, three small screens

The list shows runs-on device, “Pause all automations”, name, on/off, source →
target sentence, latest status and Review/Edit/Delete. Empty state:
“Create an automation to connect your apps.” Other fixed strings include
“Choose a device to start running automations”, “Needs your OK on this device”,
“Start on this device”, “Waiting for <device>”, “Stopped: too many automations
in a row”, and “Update the app to use this plugin”. Show “Runs only while open”
for phone primary. Rendering remains host-owned and declarative plugin templates
cannot open approval actions.

The editor offers one trigger, up to ten AND conditions, one eligible action
and typed arguments with Fixed value / From the event choices. Use the type
compatibility rules above to restrict choices. A habit-id picker can follow;
“Paste the item's id” is sufficient initially. Save creates/updates an unapproved
rule before showing the review sheet; “Not now” leaves it saved but inactive.
Saving a rename retains consent. Review must occur on the selected execution
device; approval on a phone does not approve its desktop. A review sheet on
another device explicitly says to review again on primary. History shows newest
20 across the space, with pagination to 50, statuses Done/Failed/Skipped/Stopped,
fixed reasons and device. No event inspector, payload preview or manual retry.
All controls need labels, keyboard operation, phone widths and light/dark checks.
Use `window.terrariumConfirm`, never native `window.confirm`.

### Decision 20: read-only CLI/MCP, trusted app-only mutations

Reserve host namespace `rule`. Registry query ids exposed to both read and
change clients (space required) are exactly:

- `rule.list` args `{}` → `{rules:[RuleInfo], settings:{primary_device,paused},
  armed:boolean, waiting:integer, problems:[string]}` for the requested space.
- `rule.runs` args `{rule?:id, limit?:integer, before?:string}` →
  `{runs:[RunInfo], next:string|null}`; limit defaults 20, range 1–50; cursor is
  an opaque validated `(started_ms,id)` pair encoded as base64url JSON.

`RunInfo` contains the run schema fields plus `id`; `error` is a fixed code or
null. `RuleInfo.action.args` and conditions are rule configuration, potentially
sensitive literal values; ordinary read scope already reads plugin data, so
`rule.list` shares that scope. It never returns payloads, approvals or tokens.
Client actions `rule.create/update/approve/...` do not exist and look not-found.
Agents cannot approve rules or set execution authority via CLI/MCP.

Exact app-only host service ids and argument objects (all require a valid open
`space`; unknown params rejected):

- `rules {space}` → the list state above plus `events`, `operations`, `examples`.
  `events` entries: `{plugin,event,description,payload}`; operation entries:
  `{plugin,operation,description,params}`. Return only eligible choices.
- `rule_create {space, rule:RuleInput}` → RuleInfo.
- `rule_update {space, id:id, rule:RuleInput}` → RuleInfo; replace input fields.
- `rule_set_enabled {space, id:id, enabled:boolean}` → RuleInfo.
- `rule_remove {space, id:id}` → `{ok:true}`; missing id is an idempotent success.
- `rule_review {space, id:id}` → `{digest:string, consent:object}`; consent has
  `summary`, `passed_fields`, `condition_fields`, `fixed_args`, `device`, `warnings`.
- `rule_approve {space, id:id, digest:string}` → RuleInfo; explicit local UI
  review only, never callable by plugin interop or command clients.
- `rules_save_settings {space, primary_device:string|null, paused:boolean}` →
  `{primary_device,paused,armed}`; trusted Settings only. On the selected local
  device, its explicit Start confirmation grants local arming; a remote selection
  never arms that remote device. Use a separate `rules_start {space}` service for
  the explicit local confirmation, returning the same state.
- `rule_runs {space, rule?:id, limit?:integer, before?:string}` → same shape as
  `rule.runs`.

Dispatch app-only services only through the trusted shell/preview service
boundary. Do not register them as arbitrary host command actions. Validate every
rule id belongs to `space`. Rule approval and local grants must not be writable
through generic store access. UI mutations emit `data-changed` for automations;
background/sync passes refresh the visible section just like scheduling does.

## 8. Choices and tradeoffs

Status: as built.

Each decision above records its alternatives and reason. The principal costs
are replicated payload data (instead of local-only delivery), explicit primary
selection/review (instead of unsafe silent failover), conservative package-bound
consent (instead of unattended updates), bounded queue expiry (instead of an
unbounded backlog), and at-most-once local attempts with crash loss (instead of
retries without idempotency). A durable outbox, consensus primary or signed event
attestation would strengthen guarantees but requires a separately reviewed core
and sync change. None is implicitly delegated to the later implementation tasks.

## 9. Compatibility and rollout

Status: as built.

Both `_events` and `_rules` start at schema version 1 and are added to `StoreSet`.
Older peers park either store's batches, do not execute rules and replay parked
batches after upgrade when stores exist. This is separate from plugin loading:
older manifest parsers reject `events`, and older WIT linkers reject an imported
`events` interface. New event metadata and host command ids do not affect old
plugin storage. Local claims/arming/approvals are never synced or imported from
another device/backup as authority. Restoring a profile on a new installation
requires fresh local consent and arming; do not copy device authority files.

Implementation evidence: [event validation and log](../../crates/runtime/src/events.rs),
[WIT interface](../../wit/plugin.wit), [SDK](../../crates/guest/src/lib.rs),
[rules and consent](../../crates/runtime/src/rules.rs),
[local claims and audit](../../crates/runtime/src/automation.rs),
[host dispatch](../../crates/runtime/src/registry.rs) and
[editor/list/history](../../ui/app.js), with
[UI regression coverage](../../ui/tests/automations.test.cjs).
The pre-event baseline in section 1 and the experiment in section 2 are retained
as historical evidence; deferred controls remain deferred.

Order: upgrade all hosts; deploy EVT-02 SDK and declarations support; add log,
rules and UI; then publish event-enabled To-do 0.5.0 / Habits 0.3.0 after manual
review. New hosts keep running pre-event packages; no bulk plugin migration.
On rollback to an old host, event-enabled packages remain unsupported until the
host is upgraded again or compatible packages are explicitly installed. Never
remove synced columns/tables to roll back. Future schemas are additive; future
rule JSON fields require a supported version/validation path, and unsupported
records are suspended rather than silently partially interpreted.

## 10. Phases and required tests

Status: implementation phases EVT-02–06 built; this checklist records their
required validation, not a claim that every manual/device check was performed.
EVT-07 documents the resulting contract.

Every phase runs CONVENTIONS checks; contract/templates/plugin changes also run
its cross-repository checks and package builds. All fixtures are synthetic.

- **EVT-02 — contract:** old manifest defaults; invalid event/field names,
  malformed ParamSpecs, enum/bounds, field/event count and unknown keys;
  size/count/shared interop budget boundaries; unknown event; required/null
  payload; success vs error/trap/commit failure/dry run; inner success followed
  by outer failure; host attribution cannot be overridden. Repeat used/unused
  import experiment and load old built components on new host. Refresh SDK,
  rebuild every plugin, verify added event capability approval and unknown old
  field/import behavior. No rule evaluation in this phase.
- **EVT-03 — log:** recording/read/handled state; source contract/date/root;
  merge two devices and parking/replay on older peers; logical expiry of pending
  and handled records and future clock handling; deletion convergence; failure
  after plugin commit warns without rerunning; drain on outer error; nested
  events preserve actual source; dry runs produce no log. Test plugin commit
  survives event insert failure to prove the documented delivery limit.
- **EVT-04a — rules/consent:** CRUD/space checks, limits on imported as well as
  created data; schema/settings merge/delete; every filter and mapping type;
  missing optional vs required binding; literal bounds, unknown args, malformed
  binding strings; reject host/callback/destructive/confirm/untyped targets;
  app-only service authorization and read-only command clients; local approvals,
  stale review-token rejection, package digest/signer changes, hybrid merges,
  rename retention, synced enable/pause/primary cannot arm; cross-space ids.
- **EVT-04b — execution:** only selected, locally armed, approved valid rules;
  headless refused, preview opt-in; pending closed spaces, seven-day expiry,
  future timestamps and approval cutoffs; AND operators/tag arrays/date/numbers;
  mapped-only payload transfer; depth 0–3, deterministic fan-out, per-rule and
  global rates, clock rollback; restart after claim-before-call and after
  call-before-finish never calls twice; claim limit/write failure; reconstructed
  history and no replay after history pruning; source schema mismatch; failed
  action and committed inner effects; two offline primaries can act twice while
  merging one run id (assert the limitation, not exactly-once); raw plugin
  messages never reach logs/history/notifications; all audit entry paths and
  notification throttling. Test forged peer events against the stated boundary.
- **EVT-05a — list/history:** approved/problem/waiting/expired states, local vs
  remote primary and phone warnings; pause/off/local re-arm flow; pagination;
  safe text rendering for hostile names, no payload display; data-change refresh.
- **EVT-05b — editor:** exact JSON for literal/bound args and all filter types;
  compatibility choices, consent separates passed and condition-only fields;
  trusted review digest and stale-sheet rejection; no title in sample transfer;
  Not now/rename/edit flows. Manual keyboard, phone, light/dark, isolated preview
  create→review on primary→arm→complete task→see check-in/history.
- **EVT-06 — plugin events:** To-do no-op toggle/undone emits none, completion
  including repeating task emits once; Habits checkin/add emit only on a real
  effective state change; failure rolls back its event. Duplicate same-day/status
  check-in is logically harmless, including two primary attempts; preserve normal
  counted increments while suppressing no-op checkin records. No project field
  (To-do has none). Resolve title/tags before repeating branch; exclude notes.
  Gym example requires a real selected habit id and consent; non-gym task does
  nothing; habit event chain stops. Rebuild changed packages, retain old data.
- **EVT-07 — docs:** author contract, upgrade-before-package procedure, user
  consent/device selection/expiry/crash/duplicate limits, headless exclusion,
  threat gaps and recovery; verify all local links. Mark sections as built only
  after implementation evidence; do not promise exactly-once delivery.

## 11. Differences from task files

Status: as-built task clarifications, retained for implementation history.

Only this section changes later-task instructions; their files remain untouched.

- EVT-02: add install/update event capability disclosure, shared top-level event
  budget and interop commit test; keep API 0.1.0. No unconditional clearing of
  committed child events after an outer error. SDK export still follows the
  task's commit/lock workflow, performed by the coordinator where necessary.
- EVT-03: schema gains `contract`, `root`, `date`; expire pending records too.
  Record source metadata at acceptance, drain committed child events regardless
  of outer success, surface delivery gaps. The task's `prune_keeps_unhandled_events`
  becomes “keeps unexpired unhandled events”; test seven-day expiry separately.
- EVT-04a: settings are per space, not a global `settings` row. Add run snapshots,
  digest and local SQLite claim/authority state; consent binds rule id and exact
  package identities. Add `rule_review`, expected digest on `rule_approve`, and
  `rules_start`. Do not validate bindings using sample values alone. Exclude
  `confirm` targets. Keep local enable grants distinct from synced enabled.
- EVT-04b: claim before invoke, no retry/recovery replay, run ids alone are not
  safety locks. Add global rate and deterministic fan-out overflow records;
  raw error strings are forbidden. Freeze `$date` at emitting device. Gate serve
  and preview separately. Preserve dedupe after pruning; audit every entry path;
  approval grants only new events. A synced history row is not trusted authority.
- EVT-05a/b: local primary arming, remote-review guidance, digest-token review,
  bounded history pagination; distinguish filter-only fields from transferred
  fields in the sheet. Sample transfers date only. No retry or payload inspector.
- EVT-06: omit nonexistent project; make repeated same-state checkin harmless
  (current Habits `record` always appends, so the task's “once per real change”
  criterion requires a no-op check before append). Keep `add` increments as real
  changes. Example is inactive until habit selected, primary reviewed and armed.
- EVT-07: include best-effort event persistence, no automatic failover/retries,
  explicit replication disclosure, peer-forged event limitation, local claim
  ceiling and logical-only retention. Preserve human review status honestly.

## 12. Open questions

Status: implementation complete for this slice; human review remains pending.

No implementation choice in questions 1–20 is left to EVT-02..07. User review
remains pending, with explicit authorization to proceed. Review should focus on
replication disclosure, seven-day expiry, crash-loss/duplicate limits,
forged-event trust boundary and the extra local-authority checks. Record the
reviewer name and review date at the top after actual review. Stronger
outbox delivery, signed provenance, distributed leadership, ledger compaction,
private events and safe retry are future features, not unresolved choices in
this slice.
