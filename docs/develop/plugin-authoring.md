# Writing a plugin

```bash
cargo run -p terra -- new ../my-plugin
```

```bash
cargo run -p terra -- dev ../my-plugin
```

Run the app with `TERRA_PLUGINS=/path/to/my-plugin` (supply explicit built package paths; there is no in-repo plugins folder). It reloads your plugin each time `terra dev` rebuilds it.

## Package layout

| File | Purpose |
| --- | --- |
| `manifest.json` | id, name, version, `api` (`terrarium:plugin@0.1.0`), `storage` schema, `operations`, `queries`, `pages`, `widgets`, optional `limits`, `provides`, `uses`, `jobs`, `reminders`, `events`, `permissions`, `hosts`, `oauth`, `open`, `refs`, `export`, `import` |
| `src/lib.rs` | `impl terrarium_guest::Plugin` with `handle(operation, args)`, then `terrarium_guest::plugin!(Type)` |
| `ui/*.json` | page and widget templates |
| `dist/plugin.wasm` | built component (`terra build`) |

## Storage

Tables and columns come from `storage` in the manifest. Every table gets `id` (UUIDv7) and `_deleted` (set while a row is in trash). Write through `storage::insert` / `update` / `increment` / `delete` / `restore`; read with `storage::query` (read-only SQL, only your tables). The rules in [the architecture](architecture.md#storage) apply:

- migrations may only add tables and columns
- no UNIQUE constraints or foreign keys
- counters only take `increment`

Attachment columns use `type: "file"`. The value is JSON text with:
`blob` (attachment id), `name`, optional `mime`, and `size` in bytes.
Example:

```json
{"blob":"b1-0123abcd-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"receipt.jpg","mime":"image/jpeg","size":24512}
```

Plugins can store and read this value like any other column value, but plugin
code never receives attachment bytes. Byte upload/download stays in host
commands and host-rendered UI. Hosts that do not support attachments reject
manifests with `type: "file"`, so publish attachment plugins only for
attachment-capable host builds.

## Queries and templates

UI lists never call your code. Declare named read-only queries in the manifest, with `params` naming the values bound to `?1`, `?2`, …:

```json
"queries": { "items": { "sql": "SELECT id, text FROM items WHERE _deleted IS NULL", "params": [] } }
```

A template is `{ "state": {...}, "body": node }`. Example node:

```json
{ "type": "list", "query": "items", "params": {}, "empty": "No items yet.",
  "item": { "type": "button", "label": "Remove {{row.text}}",
            "action": { "op": "remove", "args": { "id": "{{row.id}}" }, "success": "Removed" } } }
```

Templates are data: lists run declared queries through the host, so rendering never calls plugin code; only actions do. All text is set through `textContent` and `setAttribute`.

Core components: `stack`, `card`, `section`, `heading`, `text`, `badge`, `stat`, `divider`, `empty`, `button`, `checkbox`, `tabs`, `search-field`, `form`, `text-field`, `text-area`, `date`, `list`, `table`, `calendar`, `image`, `file`. Any component accepts `if`, `grow` and `basis`.

Bindings are `{{state.*}}`, `{{row.*}}`, `{{form.*}}`, `{{value}}` and `{{$today}}`. A whole-string binding keeps its raw type, e.g. `true` for `done`.

More components and keys:

| Component or key | What it does |
| --- | --- |
| `item` | A list row: `title`, `subtitle`, `done`, `flag` (priority 1–3 colors the checkbox), `check: {checked, action}`, `chips: [{text, tone, icon}]` (empty ones are dropped; `icon` and `tone` may be bindings), `avatar` (a letter tile colored from the text, e.g. a link's host), `trailing` nodes, and `action` when the row is tapped |
| `sheet` | A side panel (desktop) or bottom sheet (phone) shown while `bind` holds a value: `{bind: "state.open", title, query, params, children}`. The query's first row is `row` inside. Closing it clears `bind` |
| `choice` | A form field shown as chips: `{name, label, value, options: [{value, label}]}` |
| `choice.multiple` | `multiple: true` allows several chips; the value is a comma list in option order (`"0,2,4"`). `value` can be a comma string or an array |
| `week-strip` | Last N days as dots: `{days: "{{row.week}}", end: "{{$today}}", label, action}`. `days` has one char per day, oldest first, last = `end`: `d` done, `p` partly, `s` skipped, `m` missed, `o` open, `-` not scheduled. With `action`, a tap runs it with `{{day}}` (YYYY-MM-DD), `{{mark}}` (the dot's char) and `{{next}}` (`done`, or `cleared` when it was `d`); the dot flips at once and reverts if it fails |
| `list.group` | Column whose value starts a new heading when it changes; `group_tone` names a column holding `danger` / `accent` |
| `form.inline` | Field and button on one line; `submit_icon: "plus"` shows an icon button. With `reset`, fields clear at once so the next entry can be typed while the last one saves |
| `section.collapsible` | Folded until opened |
| `button.icon`, `variant: "ghost"` | Icons: `calendar`, `repeat`, `steps`, `bell`, `tag`, `close`, `trash`, `plus`, `check`, `note`, `flame`, `moon`, `pause`, `play`, `minus`, `chart`, `mic`, `stop`, `send`, `undo`, `archive`, `clock`, `pin`, `edit`, `external`, `folder`, `eye`, `link`. With `pressed`, an icon button shows on/off (a filled pin). A button inside a sheet waits for the sheet's autosaving forms before its action runs |
| `audio-recorder` | Round mic button: `{label, hint, max_seconds, action}`. Shows the elapsed time and a level meter while recording; Cancel drops the take. `action` gets `{{value.audio}}` (base64), `{{value.mime}}`, `{{value.duration_ms}}` |
| `label_hidden` | On fields and `search-field`: the label is read aloud but not shown |
| `action.open` | `{"open": "{{row.url}}", "op": "visit", "args": {…}}`: opens an http(s) address in the system browser, then runs `op` if given |
| `action.copy` | `{"copy": "{{row.copy_text}}", "success": "Copied"}`: puts the text on the clipboard |
| `stat.note`, `stat.tone` | A line under the number, e.g. "▲ 3 vs last week"; `tone` `good` (green) or `bad` (red) |
| `bar.layout: "columns"` | Vertical columns instead of horizontal bars (e.g. tasks per day). `bar` also takes `value_label` (a column shown instead of the number), `title` (a column used as tooltip and spoken label) and `empty` |
| `list.select` | Bulk selection bar for list rows: `{label, actions: [{label, icon, variant, op, args, ask, confirm, success, undo}]}`. `{{count}}` is the number of selected rows; `{{value}}` is the value typed through `ask`. Actions are disabled above 500 selected rows. Rows need an `id` column (string or number) to be selectable. |
| `list.layout: "plain"` | No box or row dividers, e.g. one featured card or lines of text |
| `list.layout: "board"` / `"matrix"` | Groups one query result into three columns / two columns. Supply `column` (row key), `columns: [{value, label, description?}]` and an accessible `label`. Every row must match a declared column value. Empty columns retain their headings and counts; both layouts stack on narrow screens. Filtering, limits, actions and refresh use the normal list contract. |
| `list.move` (board/matrix) | `{op, args, success, undo}`. Dragging a card (mouse, touch or pen) or using arrow keys on the card handle runs the operation; `{{value}}` is the lane value the card moved to. If the operation fails, the card snaps back. |
| `table` | `{query, params, label, columns: [{key, label, type}], sort, dir, empty, action}`. Header clicks sort by column. On phones, rows become label/value cards. With `action`, pressing the first cell runs it with `row` in scope. |
| `calendar` | `{query, params, date, title, label, empty, action}`. Shows a Monday-first month grid by date column, with a day list below. Params may use `{{$month_start}}` and `{{$month_end}}` to request only rows for that month. |
| Lists in list items | A `list` or `bar` inside a list item loads with it; its `params` can use `{{row.*}}` from the item (or the sheet) |
| `sheet.refresh` | Redraws the sheet when its row changes (say after a Refresh button inside), unless a field in it has focus |
| `json_each` | Queries may unpack JSON stored in a text column: `SELECT json_extract(j.value, '$.day') AS day FROM t, json_each(t.daily) j WHERE t.id = ?1` |
| `action.then` | `{"state.open": ""}`: state set after the operation succeeds. `success` and `then` can use `{{result.*}}`, the operation's return value |
| `{{!path}}`, `{{path == word}}`, `{{path != word}}` | Whole-string bindings for not and text comparison |
| `ref` | `{type: "ref", ref: "todo/tasks/{{row.task_id}}", label: "Task"}` draws a host-resolved chip with the current title, kind icon and done state. Empty or invalid refs draw nothing; missing or trashed items show “Missing” or “In trash” and are disabled. Resolution is batched and cached until the view refreshes; titles never enter plugin scope. |
| `backlinks` | `{type: "backlinks", ref: "{{row.$ref}}", title: "Linked from", empty: "Nothing links here yet."}` merges text mentions and relations, grouped by item kind. Each entry is a ref chip; relations also have Remove link. Reloads on view refresh. |
| `ref-picker` | `{label?, placeholder?, action}` searches indexed items across plugins in this space. For example, `{type: "ref-picker", action: {link: {from: "{{row.$ref}}", to: "{{value.ref}}"}}}` runs the action with only `value.ref`; default label is “Link to…” and placeholder is “Search items”. A failed action keeps the choice open. |
| `list.table` / `sheet.table` | `table: "tasks"` adds `row.$ref` as `plugin/tasks/<row.id>` using this template's plugin id. For example, `{type: "list", table: "tasks", query: "tasks", item: {type: "text", text: "{{row.$ref}}"}}`. A sheet with `query` uses its first row. Without `table`, no ref binding is added. |
| `action.link` | `{"link": {"from": "{{row.$ref}}", "to": "{{value.ref}}", "kind": "related"}}` calls host `link.add` before an optional `op`. Kind defaults to `related`; success shows “Linked” (or `success`) and refreshes views. Failure shows an error and stops the operation. |
| `markdown` links | `[Task](terrarium:todo/tasks/id)` renders the same chip in Markdown nodes and editor previews; http(s) links keep their usual behavior. Refs stay within the current space. |
| `markdown-editor` | Form field for Markdown: toolbar (heading, bold, italic, lists, checklist), Enter continues a list, live preview with tickable checkboxes, word count. Typing `@` plus 1–40 characters offers indexed items across plugins; choosing one inserts a `terrarium:` Markdown link. Use arrows and Enter/Tab to choose or Escape to close. `start: "preview"` opens in preview; `actions: [{label, icon, op, arg, success}]` send the selection or current line to an operation |
| `image` | Preview an attachment from a file value: `{type: "image", file: "{{row.photo}}", alt, max_height}`. Supports PNG/JPEG/GIF/WebP. Missing bytes show "Not on this device yet". SVG is not rendered inline. |
| `file` | File card from a file value: `{type: "file", file: "{{row.attachment}}"}` with name, size and a Save action when available. Missing bytes show "Not on this device yet". |
| `form.autosave` | Saves a moment after typing stops and when its sheet closes; no button. Status shows next to the editor's word count |
| `sheet.size: "large"`, `sheet.on_close` | Wide panel (full screen on phones); an action run after closing, with the sheet's `row` (e.g. drop an empty note) |
| `list.layout: "chips"`, `button.variant: "chip"`, `button.pressed` | A row of filter chips (scrolls sideways on phones) |
| `text-field.suggestions` | `{query, column}`: offers existing values while typing |
| `$utc_offset_ms` | Add to a UTC ms timestamp in SQL to get local time: `date((ts + ?n) / 1000, 'unixepoch')` |

Markdown attachments use `terrarium:blob/<blob-id>` links:

- image: `![caption](terrarium:blob/<blob-id>)`
- file: `[download](terrarium:blob/<blob-id>)`

The host resolves and opens these references. Plugin code still only sees
text/JSON values, never attachment bytes.

A node whose `if` reads `state.*` (outside a list row) appears and disappears as state changes.

To try a plugin's screens with real queries and operations in a browser:

```bash
cargo run -p terra -- preview /path/to/plugins/packages/todo /path/to/plugins/packages/notes
```

It serves the app UI on http://127.0.0.1:8766 against a scratch data folder (`--data`, `--port`), and reloads a package when its manifest, templates or `dist/plugin.wasm` change. Sync, devices and backups are not available there.

## Limits

Each operation gets a fresh instance with fuel, time and memory limits (manifest `limits`, within host ceilings). It also gets at most 5,000 host calls. All writes in one operation commit together, and only if `handle` returns `Ok`.

## Permissions

Device features beyond your own storage are listed in `permissions` and shown to the user before install:

- `speech`: local speech-to-text. Without it, `speech.available()` returns false and `speech.transcribe` fails with `denied`.
- `http`: bounded HTTPS through the host, restricted to locally approved `hosts`; optional host-managed accounts and OAuth. See [Network](#network) for requests, callbacks, errors and limits.
- `ai`: text generation with the provider the user set up in Settings → AI. The plugin never sees the provider, key or network. `ai::submit(&json!({"system": …, "prompt": …, "context": …}), "on_done")` queues a request and returns its id; it runs after your operation commits (and is dropped if it rolls back). The host then calls your operation `on_done` with `{request, ok, text, model, error: {code, message, temporary}, context}`. `ai::available()` says whether a request would be accepted now; `submit` fails with `unavailable` and the reason otherwise.

## Network

Declare `"http"` in `permissions` and the destinations in top-level `hosts`.
Both `hosts` and `oauth` default to empty for old manifests. Unknown manifest,
OAuth definition, request and auth fields are rejected.

```json
{
  "permissions": ["http"],
  "hosts": ["api.example.com", "login.example.com", "tokens.example.com"],
  "oauth": {
    "work": {
      "auth_url": "https://login.example.com/authorize",
      "token_url": "https://tokens.example.com/token",
      "scopes": ["items.read"],
      "api_hosts": ["api.example.com"]
    }
  },
  "operations": {
    "refresh": {"description": "Fetch items.", "effect": "write", "params": {}},
    "on_fetched": {"description": "Store fetched items.", "effect": "write", "params": {}, "cli": false}
  }
}
```

This is a manifest fragment; example domains are synthetic. `hosts` requires
1–20 entries with HTTP permission and must be empty/absent without it. Exact
names match only themselves; `*.example.org` matches one or more subdomains,
never the apex. A standalone `"*"` means any public HTTPS website and requires
an extra install checkbox. Adding hosts or changing OAuth configuration asks
for consent again on each device. Plugins can send data they can read to
allowed websites; permission is not an outgoing-data privacy filter.

Hosts are normalized to lowercase IDNA ASCII; canonical duplicates fail.
DNS names need at least two labels (each ≤63 bytes, total ≤253 bytes), without
trailing dots. No IP literals, ports, schemes, paths, userinfo, empty labels
or embedded wildcards. Special-use suffixes `localhost`, `local`, `internal`,
`home.arpa`, `test`, `invalid`, `example`, `onion` are rejected. Wildcards have
no public-suffix narrowing: `*.com` is broad authority. Requests require
HTTPS on port 443 (explicit `:443` is allowed), no fragment, userinfo,
backslash or control character. DNS must resolve only to permitted public
addresses; LAN, loopback, link-local, CGNAT/Tailscale and special-purpose
ranges are blocked. See the exact [address policy](../design/network-access.md#25-limits-and-ssrf-questions-68).

### Fetch and callback

The WIT `http` interface in `terrarium:plugin@0.1.0` imports
`available: func() -> bool` and
`fetch: func(request: json, on-done: string) -> result<string, plugin-error>`.
The Rust SDK takes a `&serde_json::Value` and returns the opaque request id:

```rust
use serde_json::json;
use terrarium_guest::{http, Args, Result};

fn refresh() -> Result<String> {
    http::fetch(&json!({
        "method": "GET",
        "url": "https://api.example.com/items",
        "headers": {"Accept": "application/json"},
        "auth": {"account": "work"},
        "context": {"feed": "example-feed"}
    }), "on_fetched")
}

// Dispatch "on_fetched" here from Plugin::handle.
fn on_fetched(args: Args) -> Result<serde_json::Value> {
    if args.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        // Record error.message for the user; decide whether a later retry is safe.
        return Ok(json!({}));
    }
    // Check status before parsing body; update your own store through storage::update.
    // Use context.feed to locate the row and tolerate its removal since submission.
    Ok(json!({}))
}
```

`method` and `url` are required strings. Methods are exactly uppercase GET,
HEAD, POST, PUT, PATCH, DELETE. `headers` defaults to an empty string map
(explicit null fails); `body` and `auth` default to absent and may be null;
`context` defaults to JSON null and is returned unchanged. Body is UTF-8 text,
forbidden on GET/HEAD. Binary/base64 transfer is unsupported. `auth`, if set,
is exactly `{"account":"work"}`. Account names are 1–60 ASCII letters,
digits, `-`, `_` or `.`. `on-done` must name a declared operation; mark it
`cli: false` so outside callers cannot forge replies.

Requests run after your operation commits. Rollback, traps, commit failure
and dry runs cancel submissions, including nested calls. Workers perform I/O
outside the host lock. A synchronous `{code, message}` error means no callback.
Accepted requests deliver one of these payloads to `on_fetched`:

```json
{"request":"opaque-job-id","ok":true,"status":200,"headers":{"content-type":"application/json","etag":"example-etag"},"body":"[]","context":{"feed":"example-feed"}}
```

```json
{"request":"opaque-job-id","ok":false,"error":{"code":"timeout","message":"The website did not answer in time.","temporary":true},"context":{"feed":"example-feed"}}
```

Success always includes status, headers and body. All final HTTP statuses,
including 401/404/500, are `ok: true`; failure includes none of these fields.
Bodies decode strictly as UTF-8 regardless of declared charset; HEAD and
204/304 have `body: ""`. The host does not parse HTML or JSON. Only lowercase
`content-type`, `etag`, `last-modified`, `cache-control`, `expires`,
`retry-after`, `date`, `content-language` are returned. Repeated values join
with `, ` in wire order. No final URL, Location, Set-Cookie or request headers
are exposed.

Make callbacks safe to repeat: use stable provider item ids and deduplicate
writes. Delivery is attempted once per in-memory job, without durable
exactly-once guarantees. Restart loses queued/in-flight work with no callback
or automatic replay, even if a remote write already happened. Scheduled runs
wait for all HTTP/AI work, including callback submissions; callback failure
rolls back local writes, never remote effects. Use provider idempotency keys
for writes and make scheduled operations safe to repeat across devices.

### Headers, redirects and limits

Allowed request headers: Accept, Accept-Language, Content-Type,
If-None-Match, If-Modified-Since, Idempotency-Key and `X-*`, except
`X-Forwarded-*`, `X-Real-IP`, `X-Api-Key`, `X-Auth-*`. Names must be HTTP tokens;
values reject control characters. Case-insensitive duplicates fail. All other
headers are rejected, including Authorization, Cookie, Host and framing
headers. The host sets User-Agent `Terrarium` and Accept-Encoding `identity`;
non-identity response compression fails. No cookies, proxies or cross-job
connection pools are used.

Only GET/HEAD follow 301/302/303/307/308, preserving method, at most five hops
within the original deadline. Mutating methods return 3xx without replay.
Missing Location returns 3xx; malformed/forbidden targets or a sixth redirect
fail `redirect-blocked`. Each hop repeats URL/host/DNS checks. Authenticated
cross-origin redirects fail; unauthenticated cross-origin requests forward
only Accept. Bodies are never forwarded across origins.

Limits are fixed by the host, not manifest overrides (KiB = 1,024 bytes):

| Resource | Limit |
| --- | --- |
| Request JSON / URL / UTF-8 body / serialized context | 384 KiB / 8 KiB / 256 KiB / 64 KiB |
| Request headers | 32 entries, 16 KiB total names and values; each name/value ≤4 KiB |
| Response headers on wire, including hidden fields | 32 KiB |
| Response body / complete serialized callback | 2 MiB / 4 MiB (escaped body and context included) |
| Connect / read-write inactivity / whole network job | 10 s / ≤10 s / 30 s including DNS, TLS, redirects, refresh and retry |
| Maximum queued age | 5 minutes |
| Per device, space and plugin | 1 in flight, 20 queued |
| Across a device | 4 in flight, 50 queued; fair dispatch across plugins |
| Per device, space and plugin attempts | 60 per rolling hour |
| Per device, space and plugin transferred body bytes | 50 MiB per UTC day, sent plus received |
| DNS answer set / resolver pool | 16 addresses per hop / 4 workers and 4 pending lookups |
| Device-local activity history | 500 entries and 1 MiB, oldest removed |

Redirects, failures, OAuth exchanges and retries consume attempts/bytes.
Reservations prevent overbooking; persisted counters survive restart and fail
closed on storage errors. Crash reservations remain charged conservatively.
Network quotas are not general storage/disk quotas.

### Errors

`temporary` is a suggestion for a later attempt, never proof that a remote
mutation did not happen. Only callback errors carry this boolean; WIT errors
carry code and a fixed safe message. There are no automatic network retries
except the OAuth GET/HEAD retry below.

| Code | temporary | Meaning |
| --- | --- | --- |
| `invalid` | false | Malformed request, URL, method, header, auth, callback or context (including context over 64 KiB) |
| `denied` | false | Missing permission/approval, unlisted host, absent account or wrong account host |
| `unavailable` | false | Worker, local state or credentials unavailable/unapproved/unreadable |
| `queue-full` | true | Queue or worker reservation capacity exhausted |
| `rate-limited` | true | Rolling-hour attempt budget exhausted |
| `quota` | true | Daily body-byte budget exhausted |
| `too-large` | false | Request, body, header or serialized callback exceeds its cap |
| `blocked` | false | DNS answer set exceeds 16 or includes a prohibited address |
| `redirect-blocked` | false | Redirect target, origin policy or hop limit fails |
| `timeout` | true | Queue age or deadline exceeded |
| `unreachable` | true | DNS/connect/TLS/read transport failure |
| `invalid-response` | false | Invalid UTF-8, content encoding or protocol |
| `unauthorized` | false | OAuth refresh denied; no API replay |
| `cancelled` | false | Live work cancelled where the callback is still valid |

Update, unload and removal suppress stale callbacks. Already sent requests
cannot be recalled. Do not log URLs, context, request/response data or secrets.

### Accounts and OAuth

Users add bearer accounts in Settings → Plugins → Accounts. Plugins refer to
an account by name using `auth: {"account":"work"}`; they cannot create,
list, approve or read secrets. The host injects Authorization: Bearer only
for that account's exact API hosts in the current space/plugin. Account hosts
are 1–20 exact public DNS names within manifest authority; wildcards cannot
expand an account. Synced accounts require explicit local activation with
**Use this account on this device**, including after peer credential changes.
Unreadable accounts require sign-in again. Preview has no account access.

An optional `oauth` map supports up to 4 providers. Provider keys follow the
same 1–60 character syntax as account names. Each definition requires
`auth_url`, `token_url`, `scopes` and `api_hosts`. Endpoint URLs are HTTPS/443,
≤8 KiB, with no userinfo, query or fragment; their hosts must be exact literal
manifest entries, not authorized just by a wildcard. `api_hosts` has 1–20
exact names within manifest authority. `scopes` has ≤20 nonempty strings of
≤256 bytes each, with no whitespace/control characters. No manifest secrets,
discovery, scripts or extra OAuth parameters. Provider consent binds canonical
endpoints, sorted scopes and API hosts; changes require approval again.

The user supplies client id and optional secret in Settings. Desktop sign-in
uses the system browser, authorization code flow with S256 PKCE (32-byte
verifier), random state (≥128 bits), one-use loopback callback on
`127.0.0.1` with a random port, and a five-minute sign-in deadline. Callback
headers/query are bounded to 8 KiB and token replies to 64 KiB; token endpoints
never follow redirects. Tokens/client material are sealed and synced under
the current space key. Bearer values are nonempty, ≤8 KiB without whitespace
or control characters; sealed envelopes are ≤64 KiB; at most 20 accounts per
plugin and 200 per space. Paired devices with the space key can decrypt them;
an approved provider can echo a token in a reply. Neither is prevented by the
plugin sandbox. Full rotation/revocation remains SEC-02 work.

OAuth refresh starts within 60 s of expiry, outside the host lock and serialized
per account. A 401 permits at most one refresh and API retry, only for GET/HEAD;
mutating requests return the original 401. Proactive refresh may precede any
method. All share the original 30 s deadline and budgets. Removing/replacing
an account prevents late refresh from restoring it. Sign out deletes the
synced account and cancels related work; revoke at the provider to invalidate
tokens already issued. Backups/change logs may retain old encrypted values.

### Devices and upgrades

`http::available()` checks permission, local approval, worker and queue/quota
capacity; it does not promise a URL, DNS or account will work. Desktop and
Android requests run only while the app is open; Android runs only in the
foreground. Suspended queued work expires after five minutes; process death
loses it. Desktop is still the default schedule device; `run_on: "any"`
permits foreground phone execution without cross-device deduplication.

`terra preview` supports unauthenticated HTTP with the same policy, isolated
activity/counters and developer approval of explicitly supplied packages.
It has no production credentials or OAuth. `terrarium serve` has no HTTP
worker: available is false and fetch returns `unavailable`.

This additive interface keeps API `terrarium:plugin@0.1.0`. Old manifests and
components still work on the new host. Older hosts may reject new manifest
fields/permissions or fail to link HTTP imports: available cannot rescue a
link failure. Upgrade every paired device before enabling network plugins;
keep unsupported packages inactive and do not roll back the host while they
remain enabled.

## Schedules

The host runs schedules; plugins never keep time themselves. Declared `jobs` become schedules the user can change or turn off in Settings → Scheduled. A plugin can also make its own with the `schedule` interface (no permission needed; only its own operations):

```rust
schedule::set("friday", &json!({
    "name": "Friday update",
    "repeat": {"every": "week", "days": ["fri"], "at": "17:00"},   // or day / month / minutes
    "operation": "scheduled",
    "args": {"kind": "update"},
}))?;
```

Changes apply when the operation commits. A due run calls the operation on whichever device is open, with `args.$run` = `{id, schedule, slot_ms, date, time, tz, previous_ms, attempt, manual}`. Make a run with the same id harmless to repeat (two devices may both run a slot while offline). Return error code `retry` for a temporary failure (tried again after 15 min, then 1 h) or `skip`; any other error fails the run and rolls back. To keep your changes and still end the run as failed, return `{"$run": {"status": "failed", "message": "…"}}`. A run that submits AI or HTTP requests ends after all replies and any callback-submitted work have been delivered. To tell the user a run produced something, return `{"$run": {"notify": "Weekly report is ready."}}` (from the operation, or from the AI `on-done` operation); it is shown as a notification when "Notify me when a run has something ready" is on. `list`, `runs`, `run_now`, `retry` and `remove` cover the rest.

The user also sees what `uses`, `provides`, `jobs`, `reminders` and raised `limits` allow. An update that adds any of these asks for consent again.

## Events

Declare business events in the optional manifest `events` map (empty by default).
For example, To-do 0.5.0 declares:

```json
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
```

After a real completion, emit with the Rust SDK:

```rust
use terrarium_guest::events;
use serde_json::json;

// Inside an operation, after updating the task; values come from that task.
events::emit("task_done", &json!({
    "task_id": task_id,
    "title": title,
    "tags": tags,
    "completed_date": completed_date,
}))?;
```

The WIT import `events.emit(name: string, payload: json) -> result<_, plugin-error>`
accepts JSON text; the SDK's `events::emit(name: &str, payload: &Value)` serializes
it. Events are delivered after the operation commits, dropped on rollback and
dry runs. A committed interop call keeps its events even if its outer caller
fails. Emit only on real changes: undoing or repeating the same completion must
not announce another completion. Habits 0.3.0 emits `checked_in` with `habit_id`,
`title`, `day` and effective `status` (`done`, `partial`, `skipped`, `cleared`).

An event has `description` (default empty, at most 512 UTF-8 bytes) and `payload`
(default empty), whose fields use the same ParamSpecs as command arguments.
Unknown declaration keys are rejected. Declare at most 32 events with 32 fields
each; event and field names match `^[a-z][a-z0-9_]{0,39}$`. Payloads must be objects,
contain only declared fields and satisfy requiredness, types, enum values and
bounds. Optional absent/null fields are allowed; required absent/null fields are
not. Invalid declarations fail manifest validation; invalid emissions return
`invalid`, naming fields without values. Serialized payloads are limited to 16,384 UTF-8 bytes, and at most 50
accepted emissions per operation and across a top-level call including interop;
exceeding these limits returns `quota`. Every attempt counts against the normal
host-call quota, including dry runs.

There is no new permission string. Install/update consent discloses each event
as `events:<name>:<payload-sha256>` (the hash covers canonical payload ParamSpecs).
Payloads sync to paired devices even without an automation using them. Never
include credentials, audio, task notes or note bodies; declare the minimum useful
fields. Plugins cannot subscribe, read event history or rules, or choose recipients.
A locally approved host rule passes only explicitly mapped fields to its target.

Automation actions must declare `effect: write` and explicit `params`, allow
external calls (`cli != false`), have no `confirm` text and not be named `on_…`.
Arguments are checked before execution. Operations used as automation actions
must be safe to run twice for the same event. Delivery is best effort: a crash
between the plugin commit and event-log write can lose an event. Invocation is
at most once per device, converging across devices to one history row; offline
primaries can still perform duplicate actions. Failed/interrupted actions are
not automatically retried. Do not depend on a scheduler `$run` argument.

The API remains `terrarium:plugin@0.1.0`. **Upgrade every host before publishing
or syncing event-enabled packages**: old hosts reject the new manifest field
and cannot link components importing events. New hosts still accept old plugins.
Rollback to an old host requires explicitly installing compatible packages or
upgrading again; never strip declarations or drop synced schema to roll back.
Package digest/signer changes require fresh rule review on the execution device.
See [automations](../guide/automations.md) and the
[as-built design](../design/events-and-automations.md) for mappings and limits.

## Signing and publishing

```bash
cargo run -p terra -- keygen
```

```bash
cargo run -p terra -- pack ../my-plugin
```

`keygen` writes your signing key to `~/.config/terra/signing.key` (or `$TERRA_SIGNING_KEY`); keep it private. `pack` checks the package, signs it, and writes `dist/<id>-<version>.terrapkg`. `terra verify <file>` prints the digest, the signer and the capabilities.

The app on the computer that holds the key trusts it automatically. Other publishers' keys are trusted when the user confirms the first install from them; trust syncs to all devices.

Users install a package from the Plugins page:

- a `.terrapkg` file (desktop and phone)
- a built package folder (desktop)
- a Git repository (desktop) that contains a built package (`manifest.json` + `dist/plugin.wasm`) or a `.terrapkg`, optionally `url#sub/folder`. The app never builds code from Git.

Install, update, rollback and removal go into the synced plugin set, so every device of the space follows. Rollback returns to the previous version; if the newer version added columns, the database keeps them (migrations only add), so bump `storage.version` only for additive changes, as always.

## In the host shell

The host draws each page's frame: the app tile, the title, page tabs (when a plugin has more than one page) and buttons for its quick actions. Your template fills the body. The template's first top-level `heading` is left out on pages, because the frame already shows the name.

Optional manifest fields for the shell:

| Field | What it does |
| --- | --- |
| `icon` | One of `check`, `note`, `target`, `bookmark`, `chart`, `mic`, `calendar`, `book`, `star`, `heart`, `folder`, `inbox`, `link`, `clock`, `list`. Without one, the tile shows the name's first letter. |
| `color` | Tile color, `#rrggbb`. The icon is white, so pick a color dark enough for it. |
| `badge` | A declared query returning one row with `count`, shown next to the app in the menu and on its tile. Params may be `today` and `weekday`. |
| `search` | A declared query taking `q` and returning `id`, `title` and optionally `subtitle`, for ⌘K search (this space or all spaces) and the host's `search.query` command; used while the index is being built, and for plugins without `index`. |
| `index` | `[{"table": "tasks", "query": "index_tasks", "kind": "Task"}]`: tables the host's search index holds. The query takes exactly `ids` (a JSON array) and returns `id`, `title` and optionally `body`, `keywords`, `subtitle`; filter out trashed or hidden rows in its SQL. At most 8 entries, with each table listed once. |
| `refs` | `[{"table": "tasks", "resolve": "ref_tasks", "kind": "Task", "icon": "check"}]`: item descriptions for host chips. The query takes exactly `ids`, returns `id`, `title`, optional `subtitle` and `done`. Keep it slim (no long text). At most 8 entries, one per declared table; `icon` is optional. |
| `open` | `{"page": "main"}`: the page a search hit opens, with the hit's `id` as the item. Defaults to the first page. `terrarium://open/<plugin>` links also land on that default page when no page is provided. |
| `quick_actions` | `[{"label": "New task", "op": "create", "arg": "title"}]`: offered in search, quick add and the Apps screen; the typed text goes to `op` as `arg`. |
| `share_target` | `{"types": ["text/plain", "text/uri-list"], "op": "create", "arg": "text"}`: offered in Android's share sheet; the shared text goes to `op` as `arg`. The operation must not be destructive, and people pick the plugin each time. |

### Search index

Add an index declaration and a query like To-do's to your manifest:

```json
{
  "index": [
    {
      "table": "tasks",
      "query": "index_tasks",
      "kind": "Task"
    }
  ],
  "queries": {
    "index_tasks": {
      "sql": "SELECT id, title, substr(COALESCE(notes, ''), 1, 20000) AS body, tags AS keywords, CASE WHEN done = 1 THEN 'Done' ELSE COALESCE(due, tags) END AS subtitle FROM tasks WHERE _deleted IS NULL AND cleared_at IS NULL AND id IN (SELECT value FROM json_each(?1))",
      "params": [
        "ids"
      ],
      "cli": false
    }
  }
}
```

The host passes up to 500 ids per call, including during a rebuild; mark the
query `"cli": false`. Titles weigh most, then keywords, then body; indexed
search requires all words and matches the last typed word as a prefix.
Bodies are capped at 20,000 characters; `kind` is a label of 1–20 characters.
Keep `search` for fallback while rebuilding and in plugin versions for older
hosts. Changing a query or plugin version rebuilds its index. Hosts predating
`index` reject manifests containing it: upgrade the host before installing
such a plugin; keeping `search` alone does not make that manifest compatible.

The indexer also extracts `terrarium:` Markdown links from indexed titles,
keywords and bodies, up to 100 distinct refs per item. The host-only query
`ref.backlinks {"ref": "todo/tasks/id"}` returns `{mentions, indexing}`:
mentioning item refs (newest indexed first, at most 200) and plugin ids still
indexing. It catches up each indexed plugin before answering. Re-indexing,
trash and unload remove old mentions; self-links are omitted. Mentions are
local derived data and are never passed to plugin code or synced. Index
schema version 2 rebuilds older local indexes to extract existing mentions.

## Item references

A ref names an item as `plugin/table/id`, for example `todo/tasks/t1`,
within one space. Refs never cross spaces. Declare a slim resolve query
separately from your index query so resolving chips does not fetch bodies
or hide cleared items. This manifest fragment assumes a declared `tasks`
table:

```json
{
  "refs": [{"table": "tasks", "resolve": "ref_tasks", "kind": "Task", "icon": "check"}],
  "queries": {
    "ref_tasks": {
      "sql": "SELECT id, title, CASE WHEN done = 1 THEN 'Done' ELSE due END AS subtitle, done FROM tasks WHERE _deleted IS NULL AND id IN (SELECT value FROM json_each(?1))",
      "params": ["ids"],
      "cli": false,
      "description": "Tasks by id, for links."
    }
  }
}
```

Each table may appear once, with at most 8 declarations. `kind` is a
1–20-character label and `icon`, if given, is one of the manifest icon
names above. Renderer icons differ: unsupported chip icons fall back to
`link`. Older hosts reject manifests containing `refs`; upgrade the host
before installing these plugin versions. Old manifests without `refs`
continue to load.

`ref.resolve {"refs": ["todo/tasks/t1"]}` describes up to 200 input refs
per call, deduplicated in input order. Each result has `ref` and `state`:
`ok`, `missing`, `trash` or `unknown`. Known tables also supply the item
identity and kind; live results include the current title, optional
subtitle, done state and `open: {plugin, page, item}` (or null when no page
is available). Invalid refs and undeclared plugin or table refs are
`unknown`. A chip uses its label for unknown refs;
backlinks omit them. Resolve failures can appear as missing items.

A list or queried sheet with `table: "tasks"` supplies `{{row.$ref}}` for
its children. For example, this sheet lets a task show its attached note,
create a relation and show incoming mentions plus relations. Declare
`state.open` in the containing template and return `note_id` from `task`.

```json
{
  "type": "sheet", "bind": "state.open", "table": "tasks", "query": "task",
  "params": {"id": "{{state.open}}"},
  "children": [
    {"type": "ref", "ref": "notes/notes/{{row.note_id}}", "label": "Note", "if": "{{row.note_id}}"},
    {"type": "ref-picker", "label": "Link to…", "action": {"link": {"from": "{{row.$ref}}", "to": "{{value.ref}}", "kind": "related"}}},
    {"type": "backlinks", "ref": "{{row.$ref}}", "title": "Linked from", "empty": "Nothing links here yet."}
  ]
}
```

In Markdown nodes and editor previews,
`[Buy lamp](terrarium:todo/tasks/t1)` becomes a chip with the current title.
Typing `@lam` in `markdown-editor` offers indexed items; choosing one
inserts the Markdown link. Other Markdown callers without a ref hook show
its label as plain text. HTTP links keep their existing behavior.

The host draws chips, backlinks and picker results. Resolved titles and
other plugins' result rows never enter template `row`, `state` or action
`value`; `ref-picker` exposes only `value.ref`, so your plugin code gets no
other plugin's resolved title fields. The Markdown `@` choice deliberately
writes a label and ref into the user's edited text, saved like any other
note text. No plugin-facing links WIT interface exists.

Chips resolve in batches of at most 200 and cache descriptions until view
refresh. Backlinks merge `ref.backlinks` mentions with `link.list`
relations, favor a relation when the same ref occurs in both, and group
chips by kind (missing/trash go under Other). They select at most 500
entries, relations first, resolve sequential 200-ref chunks, and show
“N more not shown” for excess entries. Removing a relation leaves text
mentions intact.

## Host-owned relations

The host stores `related` relations between item refs in `_links`, separate
from plugin databases. `link.add {source, target, kind?}` returns the live
relation (`id`, `source`, `target`, `kind`, `created_ms`), reusing an
existing one in that direction; `kind` defaults to `related`. Both refs must be valid and different, but the referenced items
need not be present on this device yet.

`link.list {ref}` returns relations in either direction, newest first (at
most 500), adding `other` and `direction` (`out` or `in`) to those fields.
Concurrent duplicate rows are shown once using the smallest row id. `link.remove {source,
target, kind?}` trashes every matching live row in either direction and
returns `{removed: count}`; outside callers must confirm this destructive
action. The items themselves are unchanged.

Relations are scoped to one space and sync and back up with the other host
stores. `_links` starts at schema version 1; future migrations only add to
it. Plugins have no WIT access to relations. These host commands require a
host version that implements them; older hosts do not provide them.

## Describing commands

Every operation and query is also a command that the command line and agents can call: `<plugin id>.<name>`, for example `todo.create` (an action) or `todo.tasks` (a query). Callers outside the app see what the manifest declares. Describe your commands so they can call them without guessing. All of these fields are optional; an operation without `params` is listed as untyped and accepts any object.

```json
"operations": {
  "create": {
    "description": "Create a task. The first sentence is the summary.",
    "effect": "write",
    "params": {
      "title": { "type": "string", "required": true, "description": "Natural language allowed" },
      "due": { "type": "date" },
      "priority": { "type": "integer", "min": 0, "max": 3 }
    },
    "result": { "id": { "type": "id" } },
    "examples": [{ "title": "Call Sam tomorrow 6pm #work" }]
  },
  "delete": { "effect": "destructive", "params": { "id": { "type": "id", "required": true } } },
  "clear_done": { "effect": "destructive", "confirm": "Hide all completed tasks?" }
},
"queries": {
  "tasks": {
    "sql": "…",
    "params": ["today", "filter", "q"],
    "description": "Open tasks for a list.",
    "types": { "filter": { "type": "enum", "values": ["today", "upcoming", "all"] }, "q": { "type": "string" } }
  }
}
```

| Field | What it does |
| --- | --- |
| `effect` | `read`, `write` or `destructive` (removes, hides, or changes many items). Left out, it is `destructive` for operations named `delete…`, `remove…`, `clear…`, `purge…` or `archive…`, and `write` otherwise. A read-only client can only run `read` commands, and a destructive one needs the person's confirmation from outside callers. |
| `confirm` | Text a person approves before a destructive command runs. |
| `params` / `types` | Argument types: `string`, `text`, `integer`, `number`, `boolean`, `date` (`YYYY-MM-DD`), `time` (`HH:MM`), `id`, `enum` (with `values`), `json`. `required`, `description`, and `min`/`max` for numbers. An empty string is accepted for optional fields, meaning "clear". |
| `result` | Fields of the returned object. |
| `cli` | `false` keeps an operation or query away from outside callers. The UI, schedules and interop still reach it. |
| `bulk` | `true` means the host may run this operation on many rows through `plugin.run_bulk`, once per id. The operation must take `id`. |
| `bulk_op` | Name of another operation that runs the same change for many rows at once, taking `ids` and the same other arguments. The host enforces at most 5,000 host calls per operation, so keep this under roughly 10 storage calls per item at 500 items. |

Outside callers can't pass arguments you didn't declare, and their types and required fields are checked before your code runs. Your own UI templates aren't checked, so a template can still pass extra fields.

Read-only operations cannot be bulk. Plugins that use `bulk` or `bulk_op` need a host version that supports bulk operations; older hosts reject those manifest fields.

The host fills in `today`, `weekday`, `now_ms`, `utc_offset_ms` and `tz` for queries when a caller leaves them out; don't give them types. It hides operations named `on_…` (AI callbacks) from outside callers automatically. Mark other internal operations, such as job targets nobody should run by hand, `"cli": false`. The plugin ids `app`, `plugin`, `space`, `page`, `rule`, `search`, `schedule`, `theme`, `sync`, `backup`, `ui`, `ref`, `link` and `blob` are reserved for host commands.

## Viewable queries

To make a query available in the built-in View widget, add a `view` declaration on that query:

```json
"view": {
  "title": "Tasks",
  "title_column": "title",
  "params": {
    "filter": "everything",
    "q": "",
    "tags": ""
  },
  "columns": [
    {
      "key": "title",
      "label": "Task",
      "type": "string"
    },
    {
      "key": "due",
      "label": "Due",
      "type": "date"
    },
    {
      "key": "board_lane",
      "label": "Status",
      "type": "enum",
      "values": ["todo", "doing", "done"],
      "labels": ["To do", "In progress", "Done"],
      "move": {
        "op": "move",
        "args": {
          "id": "{{row.id}}",
          "lane": "{{value}}"
        }
      }
    },
    {
      "key": "matrix_lane",
      "label": "Quadrant",
      "type": "enum",
      "values": ["do", "schedule", "delegate", "eliminate"],
      "labels": ["Do first", "Schedule", "Delegate", "Eliminate"],
      "move": {
        "op": "move",
        "args": {
          "id": "{{row.id}}",
          "lane": "{{value}}"
        }
      }
    },
    {
      "key": "priority",
      "label": "Priority",
      "type": "integer"
    },
    {
      "key": "tags",
      "label": "Tags",
      "type": "string"
    }
  ]
}
```

Field rules:

- `title`: 1–40 characters.
- `columns`: 1–20 entries in display order.
- `columns[*].key`: 1–64 characters, only `A-Z`, `a-z`, `0-9`, `_`, unique per view.
- `columns[*].type`: one of `string`, `text`, `integer`, `number`, `boolean`, `date`, `time`, `id`, `enum`, `json`.
- Enum columns require `values` (1–12 entries); `labels` is optional but if present must match `values` length.
- `move` is allowed only on enum columns, must point to a declared operation, and that operation must not be destructive.
- `title_column` must name a declared string/text column.
- `params` sets fixed values for the query's own declared params only; context params (`today`, `weekday`, `now_ms`, `utc_offset_ms`, `tz`) are host-filled and must not be listed here.

Query result requirements:

- The query must return `id`.
- The query must return every key declared in `columns`.

Example selection block from the To-do plugin:

```json
"select": {
  "label": "Select tasks",
  "actions": [
    { "label": "Complete", "icon": "check", "op": "toggle", "args": { "done": true }, "success": "{{count}} tasks completed" },
    { "label": "Add tag", "icon": "tag", "op": "add_tag", "ask": { "name": "tag", "label": "Tag", "title": "Add a tag to {{count}} tasks" }, "success": "Tagged {{count}} tasks" },
    { "label": "Delete", "icon": "trash", "variant": "danger", "op": "delete", "confirm": "Move {{count}} tasks to the trash?", "success": "Moved {{count}} tasks to the trash", "undo": { "op": "restore", "success": "Restored {{count}} tasks" } }
  ]
}
```

## Export and import

The optional top-level `export` and `import` arrays let the host make readable
copies and feed files through plugin operations. Unknown fields are rejected
in both entry types.

### Export fields

Declare up to ten `export` entries to add Markdown and CSV copies alongside
space export JSON. Each entry names a declared query whose only parameters
may be `today`, `weekday`, `now_ms`, `utc_offset_ms` and `tz`; the host supplies
these in the configured timezone. Queries run without UI row or byte limits.
Use `WHERE _deleted IS NULL` when readable copies should exclude trash.

| Field | Required | Meaning |
| --- | --- | --- |
| `query` | Yes | Declared query to read rows from this plugin's store |
| `format` | Yes | `markdown` or `csv` |
| `path` | Yes | Markdown path template or plain CSV file name |
| `body` | Markdown only | Query column containing the file text |
| `columns` | CSV only | Nonempty array of query column names, in output order |

```json
"export": [
  {"query": "export_notes", "format": "markdown", "path": "{{project}}/{{title}}.md", "body": "body"},
  {"query": "all_tasks", "format": "csv", "path": "tasks.csv", "columns": ["title", "due", "done"]}
]
```

Files appear under `files/<plugin-id>/`. Markdown requires a `body` column
and a relative path ending in `.md`, with no `columns`. Row placeholders
become safe path segments, trimmed and limited to 80 characters. Empty names
use `Untitled.md`; duplicate names receive ` (2)`, ` (3)` and so on.
CSV requires explicit ordered `columns`, no `body`, and a plain `.csv` file
name without folders or placeholders. Those column names form the header.
Null or missing values become empty cells; other values keep their text.
Paths cannot be absolute, contain backslashes, or contain `..` segments.
A failed query fails the export rather than producing an incomplete archive.
The export manifest records the number of readable `files` for each plugin.

Hosts predating this declaration reject the new field. Upgrade all hosts that
share a plugin set before installing a plugin that declares `export`; plugins
without it continue to load unchanged.

### Import fields

An optional `import` list maps CSV columns or Markdown file sources to a
write operation's arguments. The host parses files and calls the operation once
per item, so the plugin's own validation and store writes still apply.

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | Yes | Unique importer id within the plugin |
| `label` | Yes | Name shown in the import picker |
| `format` | Yes | `csv` or `markdown` |
| `op` | Yes | Declared operation called once per item; may be hidden with `cli: false` |
| `fields` | Yes | Nonempty object mapping operation parameter names to input sources |

Notes uses the following declaration:

```json
"import": [{
  "id": "markdown",
  "label": "Notes from a folder of Markdown files",
  "format": "markdown",
  "op": "create",
  "fields": {"title": "$name", "body": "$text", "project": "$folder"}
}]
```

Ids use 1–40 characters from `a-z`, `0-9` and `_`, and labels are 1–60
characters. Each operation must exist and must not be read-only. Field keys
must be declared parameters when the operation declares `params`. CSV sources
are column headers matched ignoring case and surrounding spaces; they cannot
start with `$`. Every mapped header must exist, even for optional parameters;
extra columns are ignored. Rows with only blank mapped cells are skipped.
Markdown sources are `$name` (without extension), `$folder`
(parent path, empty at the top), `$path` (relative file path) and `$text`
(whole file with CRLF normalized to LF). Only `.md` and `.markdown` files are
imported; hidden file names are skipped. Paths are data, never storage paths.

`plugin.list` exposes `imports` with each importer's `id`, `label` and `format`.
See the [host command record](../design/cli-control.md#export-and-import-as-built)
for transport and execution details. Call `space.import` with `plugin`,
`importer`, `files: [{path, text}]` and optional
`dry_run: true`. CSV accepts one file. Limits are 2,000 items and 16 MB of text.
Empty strings are omitted. Values are converted using the target operation's
parameter types: `integer` parses an i64, `number` a finite f64, and `boolean`
accepts `true`/`yes`/`1`/`x` or `false`/`no`/`0` ignoring case. Failed
conversions and other types remain strings for validation. Each item
commits independently. The report gives total, done, failed and skipped counts,
with at most 50 labelled failure reasons. Dry runs roll back every item.
Declared importers can call hidden operations; clients still need change access.

Older hosts reject manifests containing `import`. Upgrade every device sharing
the plugin set before installing a plugin that declares importers. Manifests
without `import` remain compatible; omit the field when no importer is declared.

To-do's title-only CSV importer shows the simplest mapping:

```json
"import": [{
  "id": "titles_csv",
  "label": "Task titles from a CSV file",
  "format": "csv",
  "op": "import",
  "fields": {"title": "title"}
}]
```

Its `tasks_csv` importer maps every header in the
[full To-do CSV](../guide/export-and-import.md#import-tasks-or-notes) to the
same-named parameter. Its `export_tasks` query exports those columns in that
order to `tasks.csv`. Notes uses `export_notes`, path
`{{project}}/{{title}}.md`, and body column `body` as shown above.

Use a dedicated import operation if normal creation parses quick-add text.
To-do's hidden `import` operation preserves the title literally and accepts
completion state; `create` parses dates, tags and priority out of the title.

## Home and search entry

Widget templates may set `home: {order: 10, collapsed: true}`. Lower order appears first (default 100); collapsed history stays available through a native disclosure. Existing templates retain their normal behavior.

## Navigation and Companion contributions

Declared pages are available to pin in the fixed left navigation. Declared widgets and `quick_actions` are available through the fixed right tool rail; on phones the same tools appear in a Companion drawer. Users configure pins in Settings → Appearance → Customize workspace. The host owns the permanent desktop status bar and derives its indicators from sync, local saves, scheduler/AI work and plugin-load health. Plugins contribute through the existing templates and operations, with the host controlling rendering, focus and navigation.

Widget templates may set `rail: {default: true, order: 10}` to appear in the default pinned widgets. Users can select any declared widget, reorder it, or fold it. Use `home: {hidden: true}` for a rail-only widget, such as Notes quick capture. A widget action can use `navigate: {item: "{{row.id}}"}` to open the plugin's first page with its declared `entry.bind`.

Customization is stored per space on the current device, independently from synced plugin data. An empty selection stays empty; temporarily unavailable contributions retain their saved positions. Keep widget and action IDs stable between releases. All apps remain accessible even when unpinned.

Assistant and Activity are host features. Assistant uses saved platform AI settings and shared limits, with session-only conversations per space. It sends conversation messages by default. The optional Today attachment previews at most 100 task titles/due dates, excludes `#private`, and does not include notes. Assistant has no plugin tools. Activity shows current-space scheduled runs, session actions and sync state; it is not a durable audit log.

A page template may declare `entry: {bind: "state.open"}` to open an item from global search. The search query must return `id`; the bind must name an existing state key. The host supplies only the item ID, and the page's declared detail query still loads the row. Search preserves its query and scope through “Back to search results”.

## Conditional forms and template defaults

A field or group can use `if: "{{form.period == custom}}"`. Conditions update on input/change; hidden fields retain their text and are omitted from submitted arguments. A form may declare `defaults: {field: "skill", query: "skill", params: {id: "{{form.skill}}"}, fields: {period: "period", sources: "sources"}, fallback: {period: "week", sources: "tasks"}}`. Defaults load initially and when that field changes, through a declared read-only query. Submission waits for the latest query, and a failed query requires retrying the selection. A multiple `choice` may be `required: true` to prevent an empty selection.

`ai-status` shows the host's current provider, endpoint host, and model before sending; optional `model` names the plugin's override. It never receives the API key or calls the provider.

## Save lifecycle and Undo

Mounted views expose asynchronous `flush()`, which drains autosave requests (including edits made while a request was running) and returns false when a save fails. Host navigation waits for it; closing a sheet with a failed save reopens the editor. `form.autosave` reports local save state and offers Retry save. Background flushing is best effort; native operating-system termination is not guaranteed to wait for it.

A successful action may declare `undo: {op: "restore", args: {id: "{{row.id}}"}, success: "Restored"}`. The toast offers Undo for ten seconds. Undo uses the original item's arguments and original space bridge; plugins must supply a supported reversal operation. Trash remains available after the toast expires.

Selection actions may also declare `undo`. After a bulk run, Undo runs one bulk call on the items that succeeded. The undo operation must also be bulk-capable, and `undo.args` is resolved once per run, so `{{row.*}}` is not available there.

## Pages and widget grid

Home and any pages the user creates (Edit page, New page) are grids of widget placements owned by the host and synced between devices. Users can place any declared widget, more than once, then move and resize it on a 12-column grid. `home.order` and `home.hidden` only choose the first Home layout; after that the user's page decides. The host provides dragging, resizing, keyboard movement and saving; plugins do not implement positioning.

A widget template may declare how it fits on a page:

```json
"page": {
  "size": { "default": [6, 4], "min": [3, 3], "max": [12, 10] },
  "config": {
    "filter": { "type": "enum", "label": "Show", "values": ["today", "open"], "labels": ["Due today", "All open"], "default": "today" }
  }
}
```

- `size`: `[columns, rows]` on the 12-column grid (rows are about 72 px). Defaults are `[4, 3]`, min `[2, 2]`, max `[12, 12]`. Content that does not fit scrolls inside the card; narrow screens stack cards at their natural height.
- `config`: settings the user can change for each placement, of type `enum` (`values`, optional `labels`), `text` (`max_length`, default 200), `boolean` or `number` (`min`, `max`). Read them as `{{$config.name}}`; declared `default`s apply everywhere the widget appears, including the Companion rail. The host keeps only declared, valid values.
- `{{$size}}` is `compact` (≤ 3 columns or ≤ 2 rows), `regular` or `expanded` (≥ 8 columns), for templates that show less or more.

Settings are data for bindings only: they reach your own queries and operations, never another plugin's. Keep widget IDs and setting names stable between releases; removed plugins keep their placements (shown as Unavailable) until they return.

The built-in View widget lets people choose an app, one viewable query from that app, and a layout (`list`, `table`, `board`, or `calendar`). They can also configure sort order, grouping ("Columns from" for board) and date columns, and filtering can follow the page focus. The widget only reads through the query declared in that app's manifest; it never reaches another plugin's data.
## Declared events

Declare events in the manifest before calling `terrarium_guest::events::emit`:

```json
"events": {
  "pinged": {
    "description": "A ping was recorded",
    "payload": {"n": {"type": "integer", "required": true}}
  }
}
```

```rust,ignore
terrarium_guest::events::emit("pinged", &serde_json::json!({"n": 1}))?;
```

The payload must be an object matching its ParamSpecs. Unknown fields and
missing/null required fields are rejected. Optional missing/null fields are
allowed. Event and field names match `[a-z][a-z0-9_]{0,39}`; declare at most
32 events, 32 fields each, and descriptions of at most 512 UTF-8 bytes.
Each serialized payload is limited to 16 KiB. At most 50 emissions are accepted
per operation and across its whole interop call tree. Every attempt counts as a
host call. Failed/trapped/rolled-back/dry-run calls discard their own events;
an independently committed interop child retains its events if its parent fails.
The host determines the source plugin and space. Never emit secrets, audio,
notes or note bodies. Event declarations disclose future encrypted replication
for automations through install/update capabilities bound to their payload
contract. This contract stage buffers events only; delivery is a later stage.

The API remains `terrarium:plugin@0.1.0`. Upgrade **every host/device before**
installing event-enabled packages: older hosts reject the new manifest field
and cannot link components importing `events`. New hosts still accept old
manifests and components. Do not strip declarations to accommodate old hosts.

### Automation rules

The host owns the synced `_rules` store. Plugins cannot query it or manage
automations. The `rule` plugin id is reserved. Command clients can read
`rule.list` and paged `rule.runs` in an open space; creating, editing, reviewing
and approving rules is available only through the trusted app services. Rule
history never contains event payloads.

Automation targets need typed `params`, write effect, `cli: true` (the default),
no confirmation and a name outside the `on_` callback namespace. Whole-field
bindings must preserve declared types and requiredness; enum bindings may only
narrow the destination's allowed values. Each execution device must approve the
exact mapping and package identities locally. Changing a mapping or either
package requires review again; renaming does not. Upgrade all devices before
installing event-enabled packages, as described above; older peers park the
unknown `_rules` store without interpreting it. Rule execution is added
separately from this configuration contract.

### Deferred HTTP requests

Declare `"permissions": ["http"]` with `"hosts": ["api.example.com"]` (1–20
patterns), then call `terrarium_guest::http::fetch(&request, "on_fetched")`.
Mark the declared callback operation `cli: false`. Requests contain required
uppercase `method` and HTTPS `url`, plus optional `headers`, text `body`,
`auth: {"account":"name"}` and JSON `context`. GET/HEAD reject bodies; methods
are GET, HEAD, POST, PUT, PATCH and DELETE. Unknown fields are errors.

Exact hosts match only themselves; `*.example.org` matches subdomains but not
the apex. A standalone `*` requests every public HTTPS website. Hostnames are
IDNA-normalized and lowercase; IPs, local/special-use names, ports in patterns,
paths and schemes are forbidden. URLs reject userinfo, fragments and ports
other than 443. Header names are limited to Accept, Accept-Language,
Content-Type, If-None-Match, If-Modified-Since, Idempotency-Key and safe X-*
headers; credentials belong to host-managed accounts. See
[network access](../design/network-access.md) for exact policy and limits.

`fetch` returns an opaque request id after validation and queue reservation.
Failed/trapped operations, commit failures and dry runs cancel their requests,
including nested interop requests. Network execution and reply delivery are
enforced by the host HTTP worker. Desktop and Android run requests only while
the app is open (Android must be in the foreground). Preview supports
unauthenticated requests with the same policy and isolated counters/audit;
`terrarium serve` has no worker and reports unavailable.
`available()` is false and `fetch` returns `unavailable` when no worker queue
is installed (including headless serve). In-memory requests are lost on restart;
remote writes must use provider-supported idempotency keys when appropriate.
The host follows at most five GET/HEAD redirects, all to approved public
HTTPS hosts; writes return 3xx without replay. Replies are strict UTF-8 text,
limited to 2 MiB and 32 KiB of response headers; escaped callbacks are capped
at 4 MiB. The whole network job has a 30-second deadline, with 10-second
connect/read/write inactivity limits. Only documented response headers are
returned; cookies, compression, proxies and cross-job connection reuse are off.

Device-local quotas reserve capacity before sending: 60 attempts per rolling
hour and 50 MiB of sent/received body bytes per UTC day, per space/plugin.
Rollback releases unspent reservations; a crash keeps them charged. Counters
and the bounded host-only audit are private files, never synced. Settings →
Plugins shows hosts and network activity, and can clear a plugin's audit.
Installing/updating a host permission needs local consent; `*` also needs an
explicit “I allow access to any website” checkbox.

Users manage bearer accounts in Settings → Plugins → Accounts. A request's
`auth.account` names an account in the current space and plugin. The host
injects its bearer header only for the account's exact selected API hosts,
which must still fit the current manifest. Wildcards never expand account
recipients. An absent/unbound account fails with `denied`; an unreadable or
locally unapproved account fails with `unavailable` and asks the user to sign
in again. Synced credentials require “Use this account on this device” before
use. Plugins never receive credentials or request headers, and cannot manage
accounts through operations, CLI/MCP or template actions. Preview remains
unauthenticated. OAuth browser sign-in is HTTP-04b work.

Authenticated redirects cannot change origin, even between bound hosts.
OAuth refresh, when a host-provisioned account is present, consumes the same
attempt/byte budget and deadline; only GET/HEAD can refresh and retry once
after a 401. Mutating requests expose their original 401 without replay.
Removing an account cancels related work and deletes its synced row. Removed
account names cannot be reused because the store uses terminal delete-wins
tombstones; choose a new name. Provider revocation remains the user's action.

Replies preserve context and have `{request, ok, status?, headers?, body?,
error?, context}`; failures use `{code, message, temporary}`. Never put secrets
in logs or errors.

API remains `terrarium:plugin@0.1.0`. Old manifests default `hosts` to empty.
Old components load on the new linker; unused HTTP imports are removed by
release builds. Older hosts reject new fields/permissions or fail to link HTTP
imports: `available()` cannot handle a link failure. Upgrade **every device**
consuming the synced plugin set before enabling HTTP packages; keep unsupported
packages inactive and do not downgrade while they remain enabled.

OAuth providers may be declared with optional `oauth` (at most four account
kinds). Each definition has only `auth_url`, `token_url`, `scopes` and
`api_hosts`, for example:

```json
{"oauth":{"work":{"auth_url":"https://login.example.com/authorize","token_url":"https://tokens.example.com/token","scopes":["items.read"],"api_hosts":["api.example.com"]}}}
```

Both provider URLs require HTTPS on port 443, without userinfo, query or
fragment, and their exact hosts must be literal entries in `hosts`.
`api_hosts` contains 1–20 exact public DNS hosts covered by `hosts`; it is
also the explicit list of token recipients shown in Settings. There are at
most 20 nonempty scopes, each at most 256 bytes without whitespace.
Endpoint, scope or API-host changes require renewed package consent.

On a computer, Settings → Plugins → Accounts opens the system browser with
state and S256 PKCE. Register a native client with the provider, using the
loopback redirect `http://127.0.0.1:<random port>/callback`; enter your client
ID and optional secret in Settings. Tokens, client material and provider
configuration are sealed together. Sign-in can be cancelled and expires
after five minutes. Phones must sync, then explicitly approve the account
on that device. Plugins receive only an account reference, never tokens.
Sign out removes the Terrarium account; revoke provider access separately.
Old manifests default `oauth` to empty. Upgrade all devices before enabling
a plugin declaring OAuth; older hosts reject the new manifest field.
