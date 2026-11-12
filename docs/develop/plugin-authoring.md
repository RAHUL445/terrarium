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
| `manifest.json` | id, name, version, `api` (`terrarium:plugin@0.1.0`), `storage` schema, `operations`, `queries`, `pages`, `widgets`, optional `limits`, `provides`, `uses`, `jobs`, `reminders`, `permissions`, `open`, `refs`, `export`, `import` |
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
- `ai`: text generation with the provider the user set up in Settings → AI. The plugin never sees the provider, key or network. `ai::submit(&json!({"system": …, "prompt": …, "context": …}), "on_done")` queues a request and returns its id; it runs after your operation commits (and is dropped if it rolls back). The host then calls your operation `on_done` with `{request, ok, text, model, error: {code, message, temporary}, context}`. `ai::available()` says whether a request would be accepted now; `submit` fails with `unavailable` and the reason otherwise.

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

Changes apply when the operation commits. A due run calls the operation on whichever device is open, with `args.$run` = `{id, schedule, slot_ms, date, time, tz, previous_ms, attempt, manual}`. Make a run with the same id harmless to repeat (two devices may both run a slot while offline). Return error code `retry` for a temporary failure (tried again after 15 min, then 1 h) or `skip`; any other error fails the run and rolls back. To keep your changes and still end the run as failed, return `{"$run": {"status": "failed", "message": "…"}}`. A run that submits an AI request ends when the reply has been delivered. To tell the user a run produced something, return `{"$run": {"notify": "Weekly report is ready."}}` (from the operation, or from the AI `on-done` operation); it is shown as a notification when "Notify me when a run has something ready" is on. `list`, `runs`, `run_now`, `retry` and `remove` cover the rest.

The user also sees what `uses`, `provides`, `jobs`, `reminders` and raised `limits` allow. An update that adds any of these asks for consent again.

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

The host fills in `today`, `weekday`, `now_ms`, `utc_offset_ms` and `tz` for queries when a caller leaves them out; don't give them types. It hides operations named `on_…` (AI callbacks) from outside callers automatically. Mark other internal operations, such as job targets nobody should run by hand, `"cli": false`. The plugin ids `app`, `plugin`, `space`, `page`, `search`, `schedule`, `theme`, `sync`, `backup`, `ui`, `ref` and `link` are reserved for host commands.

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
