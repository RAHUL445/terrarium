# Design: global search and links

Status: proposal. Nothing in this document is built yet unless a section
says "as built".

Goal: one search box that finds anything in a space, ranked well, and a
way to point from one item to another (a note mentioning a task, a task
belonging to a project note) that every plugin can use. Both keep the
plugin boundary: plugin code never reads another plugin's data.

## 1. What exists today

### Search

- A plugin may name a `search` query in its manifest. It takes `q` and
  returns `id`, `title` and optionally `subtitle`. Every recommended plugin
  with searchable text (todo, notes, bookmarks, voice) does this with
  `LIKE '%' || ?1 || '%'`. Habits and reports have no search.
- The host command `search.query` (`crates/runtime/src/commands.rs`, `search()`)
  runs each plugin's query, one plugin after another, in one or every
  space, and appends the rows. ⌘K, the `terrarium` CLI and MCP all use it.
- Each hit carries `open: {plugin, page, item}` from the manifest's `open`
  field (`OpenDef`), and `ui.open` shows that item.

Problems:

1. **Starvation.** The loop stops at `limit` (default 200) in plugin order,
   so a plugin with many matches hides every plugin after it.
2. **No ranking.** Hits are grouped by plugin, not by how well they match.
3. **Slow on large data.** `LIKE '%…%'` scans every row of every plugin on
   every keystroke, sequentially.
4. **Weak matching.** No prefix-as-you-type, no word boundaries, no
   matching several words in different columns.

### Links

None. Plugins are isolated; the only cross-plugin path is `interop.call`
through declared interfaces. One ad-hoc link exists: todo's `tasks.note_id`
holds a notes row id, and todo calls notes through interop to use it.

### Facts the design relies on

- Every change, local or imported by sync, is recorded in the store's
  `_pp_changes` table with an increasing `lseq` and its `(tbl, row_id)`
  (`crates/core/src/store.rs`, `apply_change`). A cursor on `lseq` finds
  every row that changed since the last look, whatever device made it.
- `text` columns keep a plain-text copy in the visible column, so SQL sees
  ordinary text.
- The bundled SQLite is built with `SQLITE_ENABLE_FTS5`
  (`libsqlite3-sys`, feature `bundled`), on macOS and Android.
- Manifests use `deny_unknown_fields`: a plugin that uses a new field fails
  to load on an older host. Fields added before 1.0 accept that, as the
  CLI work did with `open`, `params` and `effect`.

## 2. Global search

### 2.1 Quick fix (no new contract)

Before the index exists, `search()` stops starving plugins:

- Run every plugin's query, keep at most `ceil(limit / plugins)` hits per
  plugin first, then fill the rest of `limit` from plugins that had more.
- Order hits inside the merged list: exact title match, title starts with
  `q`, title contains `q`, other. Stable within each group.
- Report `truncated: true` when any plugin had more hits than were kept.

This ships alone and stays as the fallback for plugins without an index.

### 2.2 Search index

The host keeps one full-text index per space, built from what plugins
declare. Plugins choose what is searchable and how it reads; the host owns
storage, ranking and the query.

**Manifest:**

```json
"index": [
  { "table": "tasks", "query": "index_tasks", "kind": "Task" }
],
"queries": {
  "index_tasks": {
    "sql": "SELECT id, title, notes AS body, tags AS keywords, CASE WHEN done = 1 THEN 'Done' ELSE due END AS subtitle FROM tasks WHERE _deleted IS NULL AND cleared_at IS NULL AND id IN (SELECT value FROM json_each(?1))",
    "params": ["ids"]
  }
}
```

- `table`: a declared table whose changes trigger re-indexing.
- `query`: one of the plugin's own queries. Param `ids` is a JSON array of
  row ids; `null` means every row (full rebuild). It returns `id`,
  `title` and optional `body`, `keywords`, `subtitle`. A changed id that
  the query does not return (trashed, cleared, filtered out) is removed
  from the index, so plugins keep their own visibility rules in SQL.
- `kind`: a short label shown on hits ("Task", "Note").
- A plugin may declare several entries (notes and their projects, say).
- Changes to other tables (todo's `steps`) don't re-index the parent in
  v1. Later: `"also": [{"table": "steps", "column": "task_id"}]`.

**Index storage:** `<data>/index/<space>.db`, host-local, never synced and
never backed up; any device can rebuild it from its stores.

```sql
CREATE TABLE docs(rowid INTEGER PRIMARY KEY, plugin TEXT, entry INTEGER,
                  item TEXT, title TEXT, subtitle TEXT, kind TEXT,
                  UNIQUE(plugin, item));
CREATE VIRTUAL TABLE docs_fts USING fts5(title, keywords, body,
                  content='', tokenize='unicode61 remove_diacritics 2');
CREATE TABLE cursors(plugin TEXT PRIMARY KEY, lseq INTEGER, fingerprint TEXT);
```

`fingerprint` hashes the plugin's version and its `index` declarations and
queries; a different fingerprint means a full rebuild for that plugin.

**Keeping it current:** `Indexer::catch_up(space, plugin)` reads
`_pp_changes WHERE lseq > cursor` for the declared tables, collects the
distinct row ids, runs the index query in chunks of 500 ids, upserts or
deletes `docs` rows, and moves the cursor, all in one index transaction.
It runs:

- before every `search.query` (usually a few rows, so cheap);
- in the background, debounced, after `data-changed`;
- for every plugin when a space opens, after the window is shown.

The indexer lives in `crates/runtime`, so the app, `terrarium serve` and
`terra preview` share it. Rebuilding a big plugin on the phone runs in
chunks and yields between them; ⌘K shows "Indexing…" and falls back to
the plugin's `search` query until that plugin's index is ready.

**Querying:**

- Split the user's text into words, quote each (so FTS5 operators typed by
  the user are plain text), add `*` to the last word for
  search-as-you-type, join with AND.
- Rank with `bm25(docs_fts, 5.0, 3.0, 1.0)` (title, keywords, body),
  across all plugins at once. Ties break by recently changed.
- Return the same hit shape as today plus `kind` and a `snippet` from the
  body, so the CLI, MCP and ⌘K keep working.
- Plugins without `index` are searched through `search` (2.1) and merged
  after indexed hits of equal quality.

### 2.3 Choices and tradeoffs

| Choice | For | Against | Decision |
| --- | --- | --- | --- |
| Plugin declares an index **query** (above) vs. a list of columns | Plugin keeps its visibility rules (trash, cleared, archived) and can compute text (domain from URL) | One more query per plugin | Query |
| Tokenizer `unicode61` vs. `trigram` | Small index, word matching, prefix search | No mid-word matches ("ookmar") | `unicode61`; revisit trigram if users ask |
| Index synced vs. host-local | Local needs no protocol change and no new data in transit | Each device spends time building it | Host-local |
| Semantic (embedding) search | Finds by meaning | A local model is heavy on Android; a cloud model sends data out | Later, opt-in, desktop-built, separate design |
| Contentless FTS (`content=''`) vs. stored copy | Less disk; the stores remain the source | Snippets need a second lookup | Contentless; snippet from `docs`/plugin row |

### 2.4 Security and privacy

- The index holds text from every plugin, so only the host reads it. It is
  never exposed to plugin code. `search.query` keeps its current callers
  (shell, CLI and MCP with a read token); no new plugin permission.
- The index is plain text on disk, like the stores. If database-at-rest
  encryption (SEC-03) lands, the index uses the same protection. Removing
  a plugin or a space deletes its rows or file; purge rules match the
  stores.
- Index queries run through the same read-only SQL policy as any plugin
  query, so a manifest can't use them to read outside its database.

## 3. Links

### 3.1 References

An item is named by a **ref**: `<plugin>/<table>/<row-id>`, for example
`todo/tasks/0193f…`. Row ids are UUIDv7, unique across devices. Refs are
always within one space; links across spaces are not supported (each space
has its own key and may not be on every device in the future).

In Markdown text a ref is a link with the `terrarium:` scheme:
`[Buy lamp](terrarium:todo/tasks/0193f…)`. The text in brackets is the
title when the link was made; the host shows the current title.

### 3.2 Resolving and opening

Each plugin declares how its items are described and opened:

```json
"refs": [
  { "table": "tasks", "resolve": "ref_tasks", "kind": "Task", "icon": "check" }
]
```

- `resolve`: one of the plugin's queries taking `ids` (JSON array) and
  returning `id`, `title`, optional `subtitle`, `done`. The host runs it
  through the plugin's own query path. A missing row shows as "Missing";
  a trashed one as "In trash".
- Opening uses the existing `open` field and `ui.open`.
- The index query (2.2) can usually serve as the resolve query; a plugin
  may name the same query in both.

The host batches resolves per plugin per render, and caches titles for the
life of a view (cleared on `data-changed`).

### 3.3 Making links

- **Mentions:** typing `@` in the Markdown editor opens a picker fed by
  `search.query`; choosing a hit inserts the Markdown link. Text fields
  that store refs use the same picker.
- **Relations:** a host action in templates,
  `{"link": {"from": "{{row.$ref}}", "to": "{{value.ref}}", "kind": "related"}}`,
  creates a typed relation (3.4) without plugin code. `$ref` is a new
  row binding the renderer fills from the template's plugin and table.

No WIT change in v1: plugins neither create nor read links in code. A
later `links` WIT interface (list and add links for the plugin's own
items) would need a capability, because it reveals which other items point
at a plugin's data.

### 3.4 Storage

**Mentions** live in plugin text as written; nothing new is synced. The
indexer (2.2) also extracts `terrarium:` links from indexed text into a
host-local `mentions(from_ref, to_ref)` table in the index database, so
backlinks need no extra pass.

**Relations** live in a host-owned, synced store `_links`, like `_pages`:

| Column | Notes |
| --- | --- |
| `id` | UUIDv7 |
| `source`, `target` | refs |
| `kind` | `related` in v1; later `parent`, `blocks`, … |
| `created_ms` | |

Last writer wins per column; deletes go to trash. Two devices may create
the same link; the UI shows one. The schema starts at version 1 and only
adds columns, as with every store.

### 3.5 Showing links

New renderer nodes:

| Node | What it shows |
| --- | --- |
| `ref` | A chip: kind icon, current title, done state; tap opens it |
| `backlinks` | "Linked from…" for `{{row.$ref}}`: mentions plus relations, grouped by kind, each a `ref` chip |
| Markdown | `terrarium:` links render as `ref` chips; other links unchanged |

The host draws every chip from resolve results, so a plugin showing
backlinks never sees the linking plugin's rows.

### 3.6 Choices and tradeoffs

| Choice | For | Against | Decision |
| --- | --- | --- | --- |
| Links only in text (mentions) | Natural, kept in exports, nothing new to sync | No relation without text | Use for writing |
| Host `_links` store only | Typed, works in any plugin | Separate from plugin data; plugins can't join it in SQL | Use for structure |
| Both | Covers both needs | Two sources for backlinks | **Both**, merged by the host |
| Plugins read links through WIT | Plugins can use links in logic | Leaks cross-plugin structure; needs a capability | Not in v1 |

### 3.7 Effect on existing design

- **Small kernel.** The architecture says search is a plugin feature. Ranked
  search across plugins can't be done by a plugin without reading other
  plugins' data, so the index moves into the host. Update
  `docs/develop/architecture.md`.
- **Manifest.** New optional `index` and `refs`; new hosts accept old
  plugins; old hosts reject plugins using them (pre-1.0 rule).
- **WIT.** Unchanged.
- **Sync.** One new synced store, `_links`. Index and mentions are local.
- **Pages.** Page and widget focus may later hold a ref instead of a
  keyword; widgets would then show items linked to it.
- **todo's `note_id`.** Stays. Later it can be shown as a `ref` chip and
  listed in the note's backlinks.
- **AI.** The assistant may use the index to find context. Sending found
  items to a cloud provider needs the user's consent, as today.
- **CLI and MCP.** `search.query` keeps its shape and gains `kind` and
  `snippet`. New host commands: `link.list {ref}`, `link.add`,
  `link.remove`, `ref.resolve {refs}`.

## 4. Phases

Each phase lands on a feature branch with tests and a commit.

| Phase | Scope | Main files |
| --- | --- | --- |
| 1. Fair search | 2.1: per-plugin share, simple ordering, `truncated` | `crates/runtime/src/commands.rs` |
| 2. Index | 2.2: `index` manifest field, `Indexer` with cursors and fingerprints, FTS5 query and ranking, background catch-up, ⌘K "Indexing…"; index declarations for todo, notes, bookmarks, voice; search for habits and reports | `crates/runtime` (new `index.rs`), `manifest.rs`, `commands.rs`, `apps/tauri`, `ui/app.js`, plugin manifests |
| 3. Refs | 3.1–3.3 mentions: `refs` field, `ref.resolve`, `ref` chip, `terrarium:` links in Markdown, `@` picker | `manifest.rs`, `commands.rs`, `ui/renderer.js`, plugin manifests |
| 4. Backlinks and relations | 3.4–3.5: `mentions` table, `_links` store, `backlinks` node, `link` template action, link host commands | `crates/runtime` (new `links.rs`), `ui/renderer.js` |
| 5. Use | Focus as a ref, assistant context from the index | `ui/pages.js`, `crates/runtime/src/assistant.rs` |

### Tests

- Indexer: local writes, imported batches, trash and restore, rows the
  query filters out, fingerprint change rebuilds, removed plugin, chunked
  rebuild of 10,000 rows.
- Query building: quotes, FTS5 operators typed by the user, empty input,
  non-Latin text.
- Ranking: title beats body; hits from several plugins interleave;
  fallback plugins merge.
- Links: `_links` merge between two simulated devices, deleted targets,
  backlinks from mentions plus relations, resolve batching.
- UI (`node --test`): `ref` chip states, Markdown `terrarium:` links,
  `@` picker inserting a link.

## 5. Open questions

- Should habits and reports be searchable at all, and by what (habit
  names, report titles)?
- Trigram tokenizer later, or only if asked?
- Relation kinds beyond `related` in v1?
- Should ⌘K show recent and frequent items before anything is typed?
