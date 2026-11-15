# Data views v1

A plugin owns its SQL, operation semantics and data. The host owns the reusable presentation and editor. Page placements and plugin `data-view` nodes both mount `TerrariumViews`.

## Widget composition

```json
{"type":"data-view","id":"my-day","query":"workspace_tasks","config":{"layout":"list","filters":"{\"filter\":\"my_day\"}"}}
```

The query must be a view declared by the owning plugin. A widget cannot name another plugin's query through this node. Dynamic template bindings resolve through the normal renderer scope. Specialized capture forms and task editors can sit alongside it.

## Opt-in contracts

New query capabilities require top-level `"requires":["data-views-v1"]` and `view.version: 1`. Unsupported requirements reject loading. Legacy view declarations retain their prior contract.

A query may declare:

```json
{
  "version": 1,
  "title": "Completed tasks",
  "columns": [{"key":"title","label":"Task","type":"string"}],
  "pagination": {"page_size_param":"page_size","offset_param":"offset","default_page_size":100,"max_page_size":1000},
  "temporal": {"start_param":"start_ms","end_param":"end_ms"},
  "sorts": [{"param":"sort","values":["title"],"direction_param":"direction","directions":["asc","desc"]}],
  "row_action": {"label":"Complete","action":{"op":"toggle","args":{"id":"{{row.id}}","done":true}},"success":"{{result.message}}","followup":{"label":"Add note","if":"{{result.ask_note}}","action":{"navigate":{"item":"{{result.ask_note}}"}}}}
}
```

Pagination and temporal parameter names must reference declared optional integer parameters. Sort values are allowlisted; SQL must use static CASE expressions and a stable ID tie-breaker. Filters support declared input types, choices and multiple selection. Runtime validation happens before binding parameters to SQL. The query response remains an array of rows.

The host requests one extra row to detect a next page, and displays only the requested batch. It resets to the first page on refresh. Tables preserve SQL order; their display editor offers declared sorts. Board and calendar batches explicitly identify partial results. Offset paging does not provide snapshot isolation during concurrent writes. Page focus requires query-side support for paged sources.

Row actions go through the owning plugin's existing operation bridge. Optional follow-ups use the same operation result and toast action handling as other widgets. Navigation remains scoped to the owning plugin. Completion recurrence and done-note decisions remain plugin logic.

## Configuration ownership

- A local view is a placement's independent config. Editing it never implicitly changes a shared definition.
- A shared view is an explicit `saved_view` reference. Updating it is an explicit shared operation.
- An independent copy copies display and filter configuration, then no longer follows that reference.
- A preset supplies initial values; subsequent edits belong to the user.
- `follow_page_period` belongs to the placement, never the shared definition.

## Page review period

Pages can store `period: {mode, timezone, start?, end?}`. Modes are `previous_week`, `this_week`, and `custom`. Weeks begin Monday in the chosen IANA timezone. Custom ranges use local dates and an exclusive end, at most 366 days. The resolver converts each midnight independently to UTC milliseconds, including DST changes; invalid or nonexistent local midnights fail visibly.

An opted-in view maps the period to its declared temporal parameters. Plugin SQL applies `timestamp >= start_ms AND timestamp < end_ms` before sorting and paging. Tasks use `completed_at`; Notes use `COALESCE(updated_at, created_at)` and are labeled as created or edited notes. Page duplication preserves the period and local bindings.

The additive page-store migration preserves old records. Deploying these plugins requires the updated host on each participating device; this change does not automatically upgrade or install any device.

## Optional plugin widgets (phase 7)

Notes `host_notes`, Bookmarks `host_bookmarks`, and Feeds `host_unread` expose paged v1 sources. Each package adds an opt-in `host-view` widget using the host list/table renderer, density controls, named field presets, and ordinary filter inputs. Widget templates can pass filters as an object; serialized JSON remains supported for existing saved configurations. The host allowlists filter keys against the query declaration in both cases.

Existing widget IDs and placements remain intact. New widgets are hidden from automatic Home/rail seeding. Feeds delegates Mark read to the existing plugin operation. Notes and Bookmarks retain their domain-specific capture and editing surfaces. Habits, Voice, and Reports remain specialized widgets; this phase does not force them into tables.
