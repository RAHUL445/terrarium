# Today v1

Today is the host's default landing view: one column that answers "what needs me today?" across plugins. A plugin owns its SQL, data and operations. It contributes rows through declared queries; the host owns the layout, the merge, keyboard handling and the capture bar. No plugin reads another plugin's data, and nothing is copied between plugins.

Visual reference: `docs/design/today-mock.html`. Code: `crates/runtime/src/today.rs` (validation), `ui/today.js` (view), `ui/tests/today.test.cjs` (behaviour).

## Opting in

The contract is additive. A plugin adds `"today-v1"` to top-level `requires` and declares at most 16 sources under top-level `today`. Without the requirement, a non-empty `today` rejects the manifest (the same gate as `data-views-v1`). Plugins without `today` are unaffected.

```json
{
  "requires": ["today-v1"],
  "today": [{
    "id": "due", "section": "due", "query": "today_due",
    "params": {"today": "{{$today}}"}, "limit": 20,
    "link": {"label": "Move all to today", "each": "overdue",
             "action": {"op": "update", "args": {"id": "{{row.id}}", "due": "{{$today}}"}}},
    "row": {
      "key": "{{row.id}}", "kind": "{{row.kind}}", "title": "{{row.title}}",
      "time": "{{row.time_label}}", "sort": "{{row.sort_key}}", "tone": "{{row.tone}}",
      "chips": [{"text": "{{row.list_name}}"}],
      "check": {"action": {"op": "toggle", "args": {"id": "{{row.id}}", "done": "{{value}}"}}},
      "actions": [{"label": "Tomorrow", "key": "s", "action": {
        "op": "update", "args": {"id": "{{row.id}}", "due": "{{$tomorrow}}"},
        "success": "Moved to tomorrow",
        "undo": {"op": "update", "args": {"id": "{{row.id}}", "due": "{{row.due}}"}}}}],
      "open": {"navigate": {"item": "{{row.id}}"}}
    }
  }]
}
```

## Sections

Display order is fixed: `my_day`, `earlier`, `due`, `habits`, `sort`, `catch_up`, `wrap_up`, `done`. `earlier` is not a separate heading. It renders inside My Day as "Planned earlier, not done". An unknown section rejects that entry.

| Section | Shipped sources |
|---|---|
| `my_day` | To-do: planned for today |
| `earlier` | To-do: planned on an earlier day, not done |
| `due` | To-do: due today, overdue, today's reminders |
| `habits` | Habits: due today (chips with a streak) |
| `sort` | To-do Inbox, Voice review, Bookmarks without tags |
| `catch_up` | Feeds: unread |
| `wrap_up` | Habits: sleep prompt |
| `done` | To-do: completed today |

## Source fields

- `id`: unique within the plugin, `a-z0-9_-`, starting with a letter. A repeated id drops the later entry.
- `query`: a query the plugin declares. `params` may only name that query's parameters and must be plain values. Strings may use the context variables below.
- `limit`: 1–50, default 20. The host runs the query and shows `limit` rows; extra rows become "▸ N more".
- `count_query`: optional declared query returning `[{count}]`, used for the section's total (e.g. "Inbox 12, showing 5").
- `link`: the section's single text link, `{label, action, each?}`. With `each`, the action runs once per shown row of this source: `"all"` rows, or rows whose resolved `tone` equals the value (e.g. `"overdue"`). An `each` link is hidden when no row matches.
- `row`: maps one result row to the shared row.
  - `key` (required): identity for dedupe. `title` (required).
  - `kind`: a literal (`task`, `reminder`, `voice`, `bookmark`, `article`, `habit`, `prompt`) or one `{{row.*}}` lookup. It picks the leading icon.
  - `note`, `time`, `sort` (orders timed rows within a section), `tone` (`overdue` uses the warning token), `done` (truthy means checked).
  - `chips`: up to 4 `{text, tone?}`. `tone: "accent"` renders as a tag.
  - `check`: `{action}` with an operation. `{{value}}` is the new checked state.
  - `actions`: up to 4 `{label, key?, action}`. `key` is one unused letter. Today binds `T` and `S` to the actions with those keys.
  - `open`: what Enter or a click on the title does, usually `navigate`.

## Actions

An action is exactly one of these:

- `{op, args}`: a declared, non-destructive operation, dispatched through the owning plugin's bridge. Unknown args, missing required args, or a destructive op reject the entry. Optional `success` (toast text), `undo` (a plain operation, offered as a toast Undo) and `followup` (`{label, if?, action}`, no nesting).
- `{navigate: {item?, state?}}`: opens the plugin's first page. `item` opens that row's detail, and `state` (up to 8 plain values) seeds declared page state keys, e.g. To-do `{filter: "inbox"}`. On windows 1200px and wider, `item` alone opens the page's item sheet docked beside Today instead (Today stays in view; J/K follow the rows and Esc closes it).
- `{url}` (optionally with `op`): opens an http(s) address in the system browser, then runs the op (Feeds: open and mark read).

## Templates

`{{path}}` as a whole string resolves to the raw value; otherwise it interpolates as text. Scope:

- `row.*`: the query row.
- `value`: the new check state (check actions only).
- `result.*`: the operation result (`success`, `followup`).
- `$today`, `$tomorrow` (local `YYYY-MM-DD`), `$now` (ms), `$weekday` (0 = Sunday), `$utc_offset_ms` (ms to add to UTC, positive east).

The runtime also fills omitted context parameters (`today`, `weekday`, `now_ms`, `utc_offset_ms`, `tz`) when a query declares them.

## Validation and failure

`Manifest::parse` checks the gate and the count. `today_sources(manifest)` validates each entry on its own and returns `{sources, errors}`. A bad entry is dropped with a message such as `todo: today \`due\`: query \`x\` is not declared`, and the plugin and its other sources still load. `terra check` fails on any today error and reports `N today sources`. The desktop app sends `today`, `today_errors` and `operations` (declared op names) in each plugin summary. `ui/today.js` checks each source again before it runs it. Rejected entries and query failures appear under a collapsed "Some rows couldn't load" list, and the other sources still render.

## Merge

1. Plugins keep the host's order with To-do moved first. Sources keep declaration order.
2. Rows are deduplicated by `(plugin, key)`. A row stays in its earliest section in display order (a task due today and planned today shows once, in My Day).
3. In a section fed by several sources, untimed rows come first, then timed rows by `sort`.
4. A section's total is the higher of the rows kept and the `count_query` result minus the rows dropped as duplicates.

## Host rules

- Empty sections are hidden, except My Day, which shows an empty state with its link ("+ Plan from Inbox").
- `wrap_up` is folded before 17:00 local time. `done` is always folded. The user can open either, and that choice holds for the session.
- Due shows a "now HH:MM" line between past and future timed rows.
- The summary line counts planned, due, habits, to-sort and overdue rows. Screen readers announce it (`aria-live="polite"`).
- Rows are 36px tall (44px with Comfortable density). On hover or focus, the row's actions replace its meta.
- Keys: `J`/`K`/`↓`/`↑` move between rows, `X` checks, `T`/`S` run the bound actions, Enter opens, `C` focuses the capture bar, and Esc leaves the bar or closes menus. Keys are ignored while typing or when a dialog is open.
- Today refreshes when plugin data changes. It does not poll.

## Capture bar

Typed text is routed in this order. Each route is used only when the target plugin is installed and declares the operation:

1. `note: text` → Notes `create {body}`.
2. A link → Bookmarks `create {text}`.
3. Anything else → To-do `create {title}` in the chosen destination: Inbox (default), a list (`list_id` from `todo.list_choices`), or My Day (`create`, then `update {planned_for: today}`).

When the preferred plugin is missing, the text falls through to To-do. If nothing can take it, the bar keeps the text and shows an error. The mic opens Voice's recorder (`capture_audio`) inline.

## Preferences (this device only)

- `pp-today:<space id>` stores `{version: 1, order, hidden, plugins, density}`. Customize toggles and reorders sections, hides individual apps' sources, and picks Compact or Comfortable density. Bad stored values fall back to defaults.
- The app always opens on Today; there is no start-view preference. An old `pp-open-on-start` value is ignored.
- `pp-dashboard-pinned:<space id>` records that the Dashboard (the system widget page) was pinned once to the left rail on this device, so a later unpin sticks.
- ⌘1 / Ctrl+1 opens Today. Pinned pages moved to ⌘2–⌘9.
