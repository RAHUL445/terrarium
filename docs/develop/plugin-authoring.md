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
| `manifest.json` | id, name, version, `api` (`terrarium:plugin@0.1.0`), `storage` schema, `operations`, `queries`, `pages`, `widgets`, optional `limits`, `provides`, `uses`, `jobs`, `reminders`, `permissions`, `open` |
| `src/lib.rs` | `impl terrarium_guest::Plugin` with `handle(operation, args)`, then `terrarium_guest::plugin!(Type)` |
| `ui/*.json` | page and widget templates |
| `dist/plugin.wasm` | built component (`terra build`) |

## Storage

Tables and columns come from `storage` in the manifest. Every table gets `id` (UUIDv7) and `_deleted` (set while a row is in trash). Write through `storage::insert` / `update` / `increment` / `delete` / `restore`; read with `storage::query` (read-only SQL, only your tables). The rules in [the architecture](architecture.md#storage) apply:

- migrations may only add tables and columns
- no UNIQUE constraints or foreign keys
- counters only take `increment`

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

Core components: `stack`, `card`, `section`, `heading`, `text`, `badge`, `stat`, `divider`, `empty`, `button`, `checkbox`, `tabs`, `search-field`, `form`, `text-field`, `text-area`, `date`, `list`. Any component accepts `if`, `grow` and `basis`.

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
| `list.layout: "plain"` | No box or row dividers, e.g. one featured card or lines of text |
| `list.layout: "board"` / `"matrix"` | Groups one query result into three columns / two columns. Supply `column` (row key), `columns: [{value, label, description?}]` and an accessible `label`. Every row must match a declared column value. Empty columns retain their headings and counts; both layouts stack on narrow screens. Filtering, limits, actions and refresh use the normal list contract. |
| Lists in list items | A `list` or `bar` inside a list item loads with it; its `params` can use `{{row.*}}` from the item (or the sheet) |
| `sheet.refresh` | Redraws the sheet when its row changes (say after a Refresh button inside), unless a field in it has focus |
| `json_each` | Queries may unpack JSON stored in a text column: `SELECT json_extract(j.value, '$.day') AS day FROM t, json_each(t.daily) j WHERE t.id = ?1` |
| `action.then` | `{"state.open": ""}`: state set after the operation succeeds. `success` and `then` can use `{{result.*}}`, the operation's return value |
| `{{!path}}`, `{{path == word}}`, `{{path != word}}` | Whole-string bindings for not and text comparison |
| `markdown-editor` | Form field for Markdown: toolbar (heading, bold, italic, lists, checklist), Enter continues a list, live preview with tickable checkboxes, word count. `start: "preview"` opens in preview; `actions: [{label, icon, op, arg, success}]` send the selection or current line to an operation |
| `form.autosave` | Saves a moment after typing stops and when its sheet closes; no button. Status shows next to the editor's word count |
| `sheet.size: "large"`, `sheet.on_close` | Wide panel (full screen on phones); an action run after closing, with the sheet's `row` (e.g. drop an empty note) |
| `list.layout: "chips"`, `button.variant: "chip"`, `button.pressed` | A row of filter chips (scrolls sideways on phones) |
| `text-field.suggestions` | `{query, column}`: offers existing values while typing |
| `$utc_offset_ms` | Add to a UTC ms timestamp in SQL to get local time: `date((ts + ?n) / 1000, 'unixepoch')` |

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
| `search` | A declared query taking `q` and returning `id`, `title` and optionally `subtitle`, for ⌘K search (this space or all spaces) and the host's `search.query` command. |
| `open` | `{"page": "main"}`: the page a search hit opens, with the hit's `id` as the item. Defaults to the first page. |
| `quick_actions` | `[{"label": "New task", "op": "create", "arg": "title"}]`: offered in search, quick add and the Apps screen; the typed text goes to `op` as `arg`. |

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
| `effect` | `read`, `write` (the default) or `destructive` (removes, hides, or changes many items). A read-only client can only run `read` commands. |
| `confirm` | Text a person approves before a destructive command runs. |
| `params` / `types` | Argument types: `string`, `text`, `integer`, `number`, `boolean`, `date` (`YYYY-MM-DD`), `time` (`HH:MM`), `id`, `enum` (with `values`), `json`. `required`, `description`, and `min`/`max` for numbers. An empty string is accepted for optional fields, meaning "clear". |
| `result` | Fields of the returned object. |
| `cli` | `false` keeps an operation or query away from outside callers. The UI, schedules and interop still reach it. |

Outside callers can't pass arguments you didn't declare, and their types and required fields are checked before your code runs. Your own UI templates aren't checked, so a template can still pass extra fields.

The host fills in `today`, `weekday`, `now_ms`, `utc_offset_ms` and `tz` for queries when a caller leaves them out; don't give them types. It hides operations named `on_…` (AI callbacks) from outside callers automatically. Mark other internal operations, such as job targets nobody should run by hand, `"cli": false`. The plugin ids `app`, `plugin`, `space`, `page`, `search`, `schedule`, `theme`, `sync`, `backup` and `ui` are reserved for host commands.

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
