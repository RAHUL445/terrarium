# Declarative plugins without Wasm

Status: built through ECO-03b (runtime and packaging) and ECO-04
(`terra new --declarative`). This document records the contract and its
acceptance requirements. See [plugin authoring](../develop/plugin-authoring.md#simple-plugins-without-code).

Review: pending user review (approved to proceed)

## Decisions and alternatives

A declarative plugin ships `manifest.json` and the JSON templates named by
its pages and widgets. The host executes a small, typed list of store writes.
It has no component, guest code, or new template language.

- Choose `"kind": "declarative"`. Omitted `kind` means `"wasm"`, preserving
  existing manifests. Inferring kind from a missing file would hide broken
  Wasm builds. Inferring it from operation bodies permits ambiguous mixed
  execution and makes packages with no operations unclear.
- Choose `"do": [step, ...]` per operation. Always an array, even for one
  step: a single-object shorthand buys little and complicates validation.
  Limit it to 1–16 steps. Multiple writes support increment plus timestamp;
  disallow intermediate results, chaining ids, branching, and loops.
- Choose five verbs: insert, update, delete, restore, increment. A toggle is
  an update receiving the desired boolean from the UI, never a read/flip
  verb. This avoids hidden reads and preserves ordinary LWW sync behavior;
  it does not offer compare-and-swap or protect against stale UI state.
- Choose exact, whole-value argument/clock bindings. Reject expressions,
  interpolation and nested evaluation rather than gradually adding scripts.
- Choose a separate manifest API identifier, `terrarium:declarative@0.1.0`.
  Reusing the WIT API obscures compatibility; bumping WIT globally forces
  unrelated Wasm rebuilds. No `min_host` field or fabricated release number
  is needed: support for this exact API is the minimum host requirement.
- Allow existing host queries, pages, widgets, search, badges, open targets
  and quick actions. Reject jobs, reminders, permissions and interop for
  this first version. Host scheduling/notifications could work without
  Wasm, but add background behavior and consent beyond this bounded slice.

## Existing implementation and required seams

Read against the current source in this branch:

- `crates/runtime/src/manifest.rs`: `Manifest` and `OperationDef` reject
  unknown fields. `params` is currently optional, and `API_VERSION` names
  only `terrarium:plugin@0.1.0`. Queries already execute host-side.
- `crates/runtime/src/package.rs`: both `from_dir` and `from_files` require
  `dist/plugin.wasm`. The digest covers every packaged file except
  `signature.json`, including paths, lengths and content hashes.
- `crates/runtime/src/registry.rs`: `LoadedPlugin.pre`, `PluginHost::load`
  and `invoke` currently always prepare/call a component.
- `crates/runtime/src/host.rs`: `Runtime::call` owns begin, trace mark,
  execution, trace export, commit/rollback, dry run and post-commit schedule
  changes. Storage imports charge write bytes before using `CoreStore::write`.
- `crates/runtime/src/commands.rs`: `check_args` is private and checks
  outside callers; `Caller::App` and direct host calls do not universally
  pass it. Make it reusable within the crate for declarative execution.
- `crates/core/src/store.rs`: `Writer` implements typed writes, live-row
  checks, trash and counter semantics; use these APIs, never SQL writes.
- `crates/cli/src/main.rs`: `cmd_build` runs cargo/componentization before
  `check`; `check` loads a throwaway host; `cmd_pack` checks then packages.
  Its default output is under `dist/`, which may not exist without a build.

No change to WIT, Wasm argument checking, templates, query semantics or the
store schema contract is required by this design.

## Manifest contract

Add `Manifest.kind` as a defaulted enum (`wasm`, `declarative`), serializing
it only when non-default. Add `OperationDef.do` as a defaulted optional
array (Rust field can be `steps` with `serde(rename = "do")`). Omission is
allowed for Wasm only. Explicit null for `kind` or `do` is invalid; explicit
`do: null` must not bypass the Wasm prohibition. Preserve unknown-field
rejection on the manifest, operation, step and existing nested structures.

Wasm requires `api: "terrarium:plugin@0.1.0"`, no `do`, and a component.
Declarative requires `api: "terrarium:declarative@0.1.0"`, no component,
and `params` plus `do` on every operation. Empty `operations` is permitted
for a read-only plugin; a zero-argument operation must declare `params: {}`.
Do not globally replace `API_VERSION` and break existing WIT plugins.

Each step is exactly one of these objects (angle names below describe
values, not additional JSON syntax):

```json
{"insert":"items","values":{"title":"{{args.title}}","done":false}}
{"update":"items","row":"{{args.id}}","values":{"title":"{{args.title}}"}}
{"delete":"items","row":"{{args.id}}"}
{"restore":"items","row":"{{args.id}}"}
{"increment":"items","row":"{{args.id}}","column":"visits","delta":1}
```

Keys shown are required; no other keys or combination of verbs is valid.
Table names, column names and keys in `values` are literal schema identifiers,
never bindings. `values` must be a nonempty object. An insert must be the
operation's only step: its result is `{"id": "<generated row id>"}`. Every
other operation returns JSON null, including multiple-step operations.
No user-defined return expressions or access to earlier step results.
Existing `result` metadata may be absent; if present, it must describe exactly
the insert's required `id` of type `id`, or be `{}` for null-returning ops.
It is descriptive metadata, not an output evaluator.

Existing description, examples, cli, effect and confirm fields remain.
Declarative `effect_of(name)` must never resolve to `read`; if any step is
`delete`, it must resolve to `destructive` (even if named `hide`). Other
operations may explicitly be destructive. Examples must pass argument
checking. `cli: false` continues hiding outside entry points, not granting
access to otherwise forbidden capabilities.

### Bindings, missing values and types

The only operation bindings are `{{args.NAME}}`, `{{$now_ms}}`, and
`{{$today}}`, matched as the entire string, with no whitespace variants.
NAME is an exact declared parameter key consisting of an ASCII letter or
underscore followed by ASCII letters, digits or underscores. Reject dotted
paths, empty names, unknown parameters, unknown host values, and any other
string containing `{{` or `}}`. Other strings are literal. A caller's string
value is never evaluated again, even when it looks like a binding.

Values and delta may be JSON scalar literals or a binding, not arrays or
objects. `row` is a nonblank literal string or a binding to a required
`id` parameter; host clock bindings are not row ids. There is no binding
for row/query data, environment, secrets, space, results or other plugins.

Normalize a null argument object to `{}` just as `commands::invoke_with`
does. Reject other non-object arguments. Call the shared `check_args` with
an empty context allowlist for every declarative invocation, regardless of
UI, CLI, MCP, direct host call or internal dispatch. Reject unknown args,
missing/null required args and wrong types. Do not change the checker’s
existing rules: required string accepts even empty text; optional empty
dates/times/enums have the existing blank-field exception.

If an optional argument is missing, omit the corresponding `values` entry;
on insert this leaves the column's default/null. Explicit null is a supplied
value and clears a non-counter column. Empty strings remain empty strings.
After omissions, an empty update is `invalid`; never silently succeed.
An absent/null delta is `invalid`, so delta argument bindings must reference
required numeric params. There are no argument defaults or conditionals.

Validate literal values against `ColumnDef::check_value` at load time.
Validate binding type compatibility at load time and resolved values again
before writing. Text columns accept string/text/id/date/time/enum params;
integer columns accept integer params; real accepts number or integer;
boolean accepts boolean. `json` params cannot be bound to a write value.
`$now_ms` produces a checked signed 64-bit epoch millisecond integer;
`$today` produces a YYYY-MM-DD string in the host timezone. Capture one
clock instant and timezone per call for both bindings; never use UTC date
as a substitute or accept caller overrides for host bindings.

There is no string/number/boolean coercion or implicit JSON serialization.
The renderer's checkbox `{{value}}` is a JSON boolean; SQL may return 0/1
for display, but that does not make numeric args booleans. Numeric form
text must already arrive as a typed number or be rejected. Integer range,
finite numbers and date/time checks use existing store/parameter validation.
Null is allowed only where the store permits it; counters never accept null.

### Schema and feature validation

Keep all existing manifest, SQL, template path and storage checks. Additionally:

- All step tables/columns must belong to this plugin's declared storage.
  No writes to `id`, `_deleted`, CRDT companion columns or host tables.
- Insert supports standard or append tables; counters on insert use the
  store's initial increment semantics. Update may not set counters;
  increment requires a numeric column with `rule: "counter"` and a matching
  numeric delta. Reject update/delete/restore/increment on append tables.
  Reject restore on a delete-wins table. Keep merge/delete policies unchanged.
- `permissions`, `uses`, `provides` and `jobs` must be empty/omitted;
  `reminders` must be absent or null. Reject nonempty declarations rather
  than ignore them. External scheduling of declarative operations is also
  refused in this API version; validate schedule targets in the host's
  schedule creation/update and dispatch paths, not only manifest jobs.
- Every optional or required arg binding must match its target type.
  `row` and delta cannot reference optional args. Reused parameters must
  satisfy every destination. Constant null is forbidden for delta/counters.
- Existing `limits` remain parseable and validated. Fuel/memory are Wasm
  budgets and do not create a guest for this kind. The bounded executor
  observes the existing timeout between steps, rolling back on timeout.

## Complete Reading list package

`manifest.json` (the proposed API; an old host should reject it):

```json
{
  "id": "reading-list",
  "name": "Reading list",
  "description": "Keep a list of things to read.",
  "version": "0.1.0",
  "kind": "declarative",
  "api": "terrarium:declarative@0.1.0",
  "icon": "book",
  "storage": {
    "version": 1,
    "tables": [{"name": "items", "columns": [
      {"name": "title", "type": "text"},
      {"name": "done", "type": "boolean", "default": false},
      {"name": "created_ms", "type": "integer"}
    ]}]
  },
  "operations": {
    "create": {
      "params": {"title": {"type": "string", "required": true}},
      "effect": "write",
      "result": {"id": {"type": "id", "required": true}},
      "do": [{"insert": "items", "values": {
        "title": "{{args.title}}", "done": false, "created_ms": "{{$now_ms}}"
      }}]
    },
    "update": {
      "params": {
        "id": {"type": "id", "required": true},
        "title": {"type": "string", "required": true}
      },
      "effect": "write",
      "do": [{"update": "items", "row": "{{args.id}}", "values": {"title": "{{args.title}}"}}]
    },
    "toggle": {
      "params": {
        "id": {"type": "id", "required": true},
        "done": {"type": "boolean", "required": true}
      },
      "effect": "write",
      "do": [{"update": "items", "row": "{{args.id}}", "values": {"done": "{{args.done}}"}}]
    },
    "delete": {
      "params": {"id": {"type": "id", "required": true}},
      "effect": "destructive",
      "confirm": "Move this item to trash?",
      "do": [{"delete": "items", "row": "{{args.id}}"}]
    },
    "restore": {
      "params": {"id": {"type": "id", "required": true}},
      "effect": "write",
      "do": [{"restore": "items", "row": "{{args.id}}"}]
    }
  },
  "queries": {
    "open": {"sql": "SELECT id, title, done FROM items WHERE _deleted IS NULL AND done = 0 ORDER BY created_ms DESC", "params": []},
    "done": {"sql": "SELECT id, title, done FROM items WHERE _deleted IS NULL AND done = 1 ORDER BY created_ms DESC", "params": []},
    "trash": {"sql": "SELECT id, title FROM items WHERE _deleted IS NOT NULL ORDER BY _deleted DESC", "params": []},
    "search": {
      "sql": "SELECT id, title FROM items WHERE _deleted IS NULL AND title LIKE '%' || ?1 || '%' LIMIT 8",
      "params": ["q"], "types": {"q": {"type": "string", "required": true}}
    }
  },
  "search": "search",
  "quick_actions": [{"label": "Add reading", "op": "create", "arg": "title"}],
  "pages": [{"id": "main", "title": "Reading list", "template": "ui/page-main.json"}],
  "open": {"page": "main"}
}
```

`ui/page-main.json` uses the existing renderer, including explicit confirmation
for its delete button. The checkbox supplies the desired state via `value`.

```json
{
  "state": {},
  "body": {"type": "stack", "gap": 12, "children": [
    {"type": "heading", "text": "Reading list"},
    {"type": "form", "inline": true, "submit_label": "Add", "children": [
      {"type": "text-field", "name": "title", "label": "Title", "required": true}
    ], "submit": {"op": "create", "args": {"title": "{{form.title}}"}, "reset": true}},
    {"type": "list", "query": "open", "empty": "Nothing to read yet.", "item": {
      "type": "item", "title": "{{row.title}}",
      "check": {"checked": "{{row.done}}", "action": {"op": "toggle", "args": {"id": "{{row.id}}", "done": "{{value}}"}}},
      "trailing": [{"type": "button", "label": "Delete", "action": {
        "op": "delete", "args": {"id": "{{row.id}}"}, "confirm": "Move this item to trash?"
      }}]
    }},
    {"type": "list", "query": "done", "empty": "Nothing finished yet.", "item": {
      "type": "item", "title": "{{row.title}}", "done": "{{row.done}}",
      "check": {"checked": "{{row.done}}", "action": {"op": "toggle", "args": {"id": "{{row.id}}", "done": "{{value}}"}}}
    }},
    {"type": "list", "query": "trash", "empty": "Trash is empty.", "item": {
      "type": "item", "title": "{{row.title}}", "trailing": [
        {"type": "button", "label": "Restore", "action": {"op": "restore", "args": {"id": "{{row.id}}"}}}
      ]
    }}
  ]}
}
```

End-to-end calls are `create({"title":"Read the guide"}) → {"id":I}`,
`update({"id":I,"title":"Read chapter one"}) → null`,
`toggle({"id":I,"done":true}) → null`, `delete({"id":I}) → null`,
`restore({"id":I}) → null`. Here I denotes the returned id, not a binding
syntax. Queries move the item from open to done to trash and back to done.
The example page handles create/toggle/delete/restore; update is callable
through the existing CLI/MCP action interface with the same id and params.
Outside destructive execution requires confirmation; use dry run to inspect.

## Runtime execution and failures

Replace `LoadedPlugin.pre` with `code: Code`, where
`Code::Wasm(PluginPre<HostState>)` and `Code::Declarative` share manifest,
store and templates. `load` prepares a component only for Wasm. `invoke`
branches after resolving the declared operation and active space, restoring
the borrowed store and plugin to their slots on every exit path.

Extract a shared transaction wrapper from `Runtime::call`, rather than
copying it into `declarative.rs`. It must preserve the current Wasm instance
limits and cleanup, take the pre-call trace cursor, begin once, run the
executor, export successful changes before rollback for dry run, and commit
only on success without dry run. On error or commit failure, roll back.
Retain schedule application after successful commit and AI cancellation
on failure/dry run for Wasm. Declarative execution produces neither side
effect. Keep the current trace ordering; a trace is not proof of a commit
when finalization fails. A failing call must not publish data-changed events.

The declarative runner takes the plugin's `CoreStore`, manifest, operation,
argument map, captured clock and timezone. Resolve/check values, charge
budgets, and execute ordered steps through `core.write(|w| ...)` inside the
outer transaction. Inner store writes must retain the outer transaction;
never commit a step independently. A later missing row/type/quota error
rolls back all previous steps and their `_pp_changes` records. No executor
SQL reads or writes: queries continue through the existing read-only path.
Use the store for ids, HLCs, trash, additive schema and sync changes.

Expose the existing storage-to-plugin error mapping for reuse:

- `invalid`: argument/binding/type/no-fields errors and store schema,
  counter, append-only or policy errors.
- `not-found`: nonexistent row, or restore on an already live/missing row.
- `in-trash`: update, delete or increment of a trashed row.
- `quota`: input, output or cumulative write budget exceeded.
- `storage`: other store failures; preserve existing error sanitization,
  never include supplied values or secrets in new diagnostics.
- Timeout uses the existing `RuntimeError::Trap` surface, not a successful
  null result. Load-time contract errors are `RuntimeError::Manifest`;
  malformed files/signatures are the existing package/IO errors.

Execution errors use `RuntimeError::Plugin {code, message}` where applicable.
Unknown plugin/space/operation and outside `forbidden`/`needs-confirmation`
retain the existing dispatch/command errors, rather than being remapped.
A repeated delete is an error, not idempotent success; a repeated update
with the same boolean is an ordinary store update. No automatic retries.

Reuse host ceilings: `MAX_ARGS` = 12 MiB serialized input, `MAX_REPLY` =
4 MiB serialized result, `MAX_WRITES` = 32 MiB per call. Share constants and
write charging code rather than maintaining independent limits. Charge UTF-8
bytes of table + row (if any) + column (increment only) + compact serialized
values/delta, using saturating addition before each write, exactly as the
Wasm imports do. No extra per-value cap: each value is bounded by input and
write budgets; literal plan values also count toward writes. The 16-step
cap is independent of emitted change count (an insert can emit many changes).
Package size/file-count limits remain 32 MiB/256 entries. Enforce step caps
at validation and execution, and retain query row/byte ceilings unchanged.

## Packaging, tools and consent

Update both `Package::from_dir` and `from_files`: require the component for
Wasm, forbid `dist/plugin.wasm` for declarative. Directory loading/build/check
must also detect a stale component, not silently leave it out of the archive.
Keep path containment, regular-file, size, duplicate and signature checks.
No new archive format: digest/signature algorithms and trust policy stay the
same. Manifest changes (including `kind` and `do`) and every template remain
signed. A manifest-only plugin with no views is valid; named templates are
always required. Extra archive files retain existing digest coverage; none
become executable. Reject a component even if disguised as a template path
at `dist/plugin.wasm`.

`terra build` reads the manifest first; declarative runs `check` only, with
no cargo, componentization, SDK lookup or Wasm output. `terra check` uses the
same throwaway host validation. `terra pack` retains check/sign/zip behavior;
it must create the default output's `dist/` parent for a fresh declarative
package. Do not require a dummy component or Cargo files. `terra preview`
loads this kind through the same host and refresh/data-changed mechanisms.

ECO-04 adds `terra new <dir> --declarative`, generating `manifest.json`,
`ui/page-main.json` and `.gitignore` containing `dist/`. Use the Reading list
schema and create/toggle/delete/restore, plus open/done/search queries and
quick action as above (update and trash UI may also follow the fixture).
Honor id/name overrides and existing directory validation. Reject an explicit
`--sdk` with `--declarative` in either order; ignore inherited `TERRA_SDK`
for this kind. Without the flag preserve Wasm output. Next steps are
`terra check <dir>` and `terra preview <dir>`.

Keep existing install/update review, signature trust and user consent. Do
not auto-trust declarative packages: their writes can still delete data and
templates can request existing host UI actions. No new install-review UI is
required in ECO-03b; the manifest kind is available for later labeling.
Outside callers keep write scope, hidden-operation and confirmation gates;
app templates keep explicit action confirmations and existing undo behavior.
No component means no guest capabilities, not exemption from consent.

## Deliberate exclusions and upgrade policy

No loops, conditionals, expressions, parsing, normalization, validation
scripts, bulk selectors, query-to-write bindings, dynamic tables, custom ids,
upsert, hard purge, events, HTTP, AI, speech, interop, schedules, reminders,
plugin JS, or arbitrary return shapes. Host-owned queries/renderer features
remain available; the operation binding restriction does not change template
bindings. Do not invent a general workflow engine to support edge cases.

Bookmarks is not a drop-in declarative conversion: its Rust code parses
free text and #tags, normalizes/deduplicates tags, cleans URLs and strips
tracking parameters, finds existing URLs, chooses fallback titles, trims and
limits text, and optionally reads the current pinned state. Its visit action
(increment + visited timestamp) fits a two-step operation; its create and
update behavior require Wasm. A simple explicit-field reading list fits.

New hosts accept old Wasm manifests without kind/do unchanged. Old hosts
reject new `kind`/`do` via unknown-field validation and reject the new API;
if fields were removed they would still require the absent component.
There is no safe downgrade by removing metadata or shipping a dummy Wasm.

Before installing/publishing into a synced plugin set, upgrade every device
that must run it to a host supporting `terrarium:declarative@0.1.0` (ECO-03b
or later). New-host diagnostics for unsupported API/kind must say
"This plugin needs a newer Terrarium host. Update Terrarium on each device."
and include the requested API identifier as developer detail. Already-shipped
old binaries cannot be made to show this message retroactively; they may show
unknown-field/unsupported-api errors. Preserve package data and existing
stores when refusing activation; an old offline device cannot run the plugin
until upgraded. No new synced minimum-version record or device negotiation
is implied by this design; rollout coordination remains the user's action.

Upgrade to Wasm with the same plugin id, a newer package version,
`api: "terrarium:plugin@0.1.0"`, kind omitted/wasm, no do bodies and a real
component. Keep table names, column types, merge/delete policies and storage
version unchanged if schema is unchanged; add only tables/columns with a
storage version bump when needed. Existing data, ids, trash, spaces and
sync history remain owned by that plugin. Retain operation names/params and
results for callers. Added capabilities still require consent. A rollback
must use a manifest compatible with the current additive schema, never drop
new columns or replay create calls to migrate data.

## Required implementation tests

ECO-03b must add a synthetic fixture matching the complete example and cover:

1. Manifest default Wasm compatibility and unchanged existing fixtures; exact
   new API/kind pairing; no hybrid; required params/do; null/empty/unknown
   fields; 1/16/17-step boundaries; sole-insert rule; result metadata and
   destructive effects; every unsupported capability/schedule target.
2. Each verb, literal and clock binding; unknown/malformed/interpolated and
   nested bindings; cross-plugin/host/system column rejection; every type
   compatibility including counter/null/append/delete-wins restrictions.
3. `create_update_toggle_delete_restore`, return shapes, missing rows,
   repeated delete/restore, trash errors; explicit false stays false;
   required/optional/null/blank/unknown/wrong-type args for UI/direct host,
   CLI/MCP command callers and internal invocation. Caller strings containing
   bindings stay literal. Wasm UI arguments remain unchanged.
4. Optional omission/default/null clearing; empty resolved update; one clock
   instant and timezone date across steps including midnight and DST;
   integer overflow and numeric/boolean strings rejected without coercion.
5. Multi-step increment plus timestamp, and failure in step two rolling back
   step one and its change log. Dry run returns normal result and trace,
   keeps no rows/tombstones/change records, and emits no committed events.
   Commit failure, timeout and quota also restore borrowed stores safely.
6. Exact/over input and write budgets (including literal values and repeated
   writes), step cap, result budget in the shared wrapper; regression tests
   for Wasm rollback, trace, AI cancellation and post-commit schedules.
7. Export/import between two hosts and separate spaces: identical normal
   rows, trash/restore and counters, no cross-plugin/space access, no custom
   sync pathway. Declarative-to-Wasm same-id upgrade preserves rows/history;
   additive migration and incompatible rollback refusals remain intact.
8. Directory and ZIP pack/sign/verify/install without Wasm, unsigned consent,
   manifest/template tamper failures, missing template, stale/forbidden
   component and path/size protections. Legacy parser fixtures reject the
   new format; new loader supports both APIs and refuses unsupported APIs
   without altering stores. Assert refusal text and digest coverage.
9. `terra build/check/pack` on a fresh package without Rust tooling; default
   pack output creates dist. Preview renders, quick-add/create/update/check/
   delete/restore refresh queries, and confirmation gates remain effective.
   Update `docs/develop/plugin-authoring.md` when implementation lands.

ECO-04 must test a generated scaffold through `check`, build, pack and preview;
parse generated JSON under strict validation; exercise create/toggle (typed
boolean), delete/restore and search/quick-add; verify exact file list, no
Cargo/src/Wasm, id/name escaping, nonempty-directory refusal, explicit SDK
conflict in both flag orders, invalid inherited SDK ignored, and unchanged
Wasm scaffold without the flag. Run every CONVENTIONS host check for both
tasks, plus the cross-repository integration checks when runtime contracts
change in ECO-03b. This design alone adds no runtime or plugin changes.
