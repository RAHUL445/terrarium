# Design: global search and links

Status: built (§2 search and §3 links).

Goal: one search box that finds anything in a space, ranked well, and a
way to point from one item to another (a note mentioning a task, a task
belonging to a project note) that every plugin can use. Both keep the
plugin boundary: plugin code never reads another plugin's data.

## 1. Starting point

This section describes search before §2 was built and the original problems
it addressed. The storage facts below still apply.

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

The fallback path in `search()` gives each plugin a fair share:

- Run every plugin's query, keep at most `ceil(limit / plugins)` hits per
  plugin first, capped by the remaining total limit, then fill unused room
  from plugins that had more hits, in plugin order.
- Order hits inside the merged list: exact title match, title starts with
  `q`, title contains `q`, other. Stable within each group.
- Report `truncated: true` when any plugin had more hits than were kept.

This remains the fallback for plugins without an index and while an index
is being rebuilt.

### 2.2 Search index

**As built:** the host owns a full-text index for each space, populated by
queries declared by each plugin. Compared with the original proposal:

- Rebuilds walk row ids in chunks of at most 500, never `ids: null`; plugin
  reads remain bounded.
- `docs` records `tbl` (the table name) and `changed_ms` (when indexed).
- Background indexing uses periodic bounded steps, rather than a
  `data-changed` listener; local UI writes do not emit that event.
- Indexed hits carry `ref` (`plugin/table/id`).
- Every search first catches up by at most 2,000 rows per plugin and space,
  then uses fallback search for indexes still rebuilding.

**Manifest:** for example, To-do declares:

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

- `table`: a declared table whose changes trigger re-indexing. Each table
  may be listed once, with at most 8 entries per manifest.
- `query`: a declared query taking exactly `ids`, a JSON array of row ids.
  It returns `id`, `title` and optional `body`, `keywords`, `subtitle`.
  Requested ids the query leaves out (trashed, cleared or hidden) are
  removed from the index. Use `"cli": false` for this internal query.
- `kind`: a label of 1–20 characters shown on hits, such as “Task”.
- Child-table changes (such as To-do's `steps`) do not re-index a parent in
  this version.

**Index storage:** `<data>/index/<space>.db`, where `<data>` is
`HostConfig.data_dir`, the default space's plugin database directory. The
index is local, never synced or backed up, and rebuilt from the stores.

```sql
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS docs(rowid INTEGER PRIMARY KEY, plugin TEXT NOT NULL,
    tbl TEXT NOT NULL, item TEXT NOT NULL, title TEXT NOT NULL, subtitle TEXT,
    kind TEXT NOT NULL, changed_ms INTEGER NOT NULL, UNIQUE(plugin, item));
CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(title, keywords, body,
    content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');
CREATE TABLE IF NOT EXISTS cursors(plugin TEXT PRIMARY KEY, lseq INTEGER NOT NULL,
    fingerprint TEXT NOT NULL, rebuild_entry INTEGER, rebuild_after TEXT NOT NULL DEFAULT '');
```

A SHA-256 fingerprint covers the index schema version, plugin version,
index declarations, query SQL and parameters. A changed fingerprint or a
store whose log went backwards starts a full rebuild. Cursors preserve the
current table and last row id so rebuilds resume across bounded calls.

**Keeping it current:** `PluginHost::index_catch_up(space, plugin, budget)`
follows `_pp_changes` after the saved `lseq`, groups changed row ids by
indexed table, then runs the declared queries in chunks of at most 500 ids.
Each chunk saves document changes and its cursor in one index transaction.
Bodies are capped at 20,000 characters. Queries hitting a read limit retry
with smaller id groups; a single row that still exceeds a limit is skipped.

- Before answering a search, catch-up handles at most 2,000 rows per plugin
  and space.
- The app and `terrarium serve` call `index_step` in the plugin reload loop,
  handling at most 1,000 rows per lock hold. The loop waits 800 ms normally
  and 100 ms while more work remains, releasing the lock between steps.
- `terra preview` runs the same step between requests, paced by its 500 ms
  receive timeout. New spaces join the next step automatically.

The indexer lives in `crates/runtime`. A rebuilding plugin is served by its
`search` query; `indexing` lists such plugin ids, and ⌘K shows “Indexing…”.

**Querying:**

- Consider the first 12 whitespace-separated words, dropping words with
  no alphanumeric character. Quote each word so FTS5 operators are plain
  text, append `*` to the last word for prefix matching, and join with AND.
- Rank indexed matches with `bm25(docs_fts, 5.0, 3.0, 1.0)` (title,
  keywords, body). Lower scores come first; ties use `changed_ms`, most
  recently indexed first. Scores across spaces are only roughly comparable.
- Merge indexed and fallback hits, then stably order by title match:
  exact, prefix, contains, other. Within a group, indexed hits come first.
- Keep `space`, `plugin`, `id`, `title`, `subtitle` and `open`. Add `kind`,
  `snippet` and `ref` on indexed hits; fallback hits have null `kind` and
  `snippet` and no `ref`. The host builds snippets from another lookup of
  the kept hits' bodies; contentless FTS cannot supply `snippet()`.
- Return `truncated` when hits were dropped, and `indexing` as a sorted,
  unique list of plugin ids whose indexes are still rebuilding. One
  plugin's failure is reported in `errors`; others can still answer.

### 2.3 Choices and tradeoffs

| Choice | For | Against | Decision |
| --- | --- | --- | --- |
| Plugin declares an index **query** (above) vs. a list of columns | Plugin keeps its visibility rules (trash, cleared, archived) and can compute text (domain from URL) | One more query per plugin | Query |
| Tokenizer `unicode61` vs. `trigram` | Small index, word matching, prefix search | No mid-word matches ("ookmar") | `unicode61`; revisit trigram if users ask |
| Index synced vs. host-local | Local needs no protocol change and no new data in transit | Each device spends time building it | Host-local |
| Semantic (embedding) search | Finds by meaning | A local model is heavy on Android; a cloud model sends data out | Later, opt-in, desktop-built, separate design |
| Contentless FTS (`content=''`) vs. stored copy | Less disk; the stores remain the source | Snippets need a second lookup | Contentless; host snippet from plugin row |

### 2.4 Security and privacy

- The index holds text from every plugin, so only the host reads it. It is
  never exposed to plugin code. `search.query` keeps its current callers
  (shell, CLI and MCP with a read token); no new plugin permission.
- The index is plain text on disk, like the stores. If database-at-rest
  encryption (SEC-03) lands, the index should use the same protection.
  Unloading a plugin removes its indexed rows. Closing a space closes its
  connection but leaves the local index file on disk.
- Index queries run through the same read-only SQL policy as any plugin
  query, so a manifest can't use them to read outside its database.

## 3. Links

### As built

- Refs use separate slim resolve queries (`id`, `title`, optional
  `subtitle`, `done`), rather than index queries with long bodies. Cleared
  tasks still resolve. All six first-party plugins declare refs.
- `ref.backlinks {ref}` reads mentions from the local index;
  `link.list {ref}` reads synced relations. The host's `backlinks` node
  merges and deduplicates them, grouping chips by kind. It resolves in
  sequential chunks of at most 200, selects at most 500 entries (relations
  first), and reports excess entries.
- Templates name their table with `table` on `list` or queried `sheet`,
  which supplies `{{row.$ref}}`. `ref-picker` shares the Markdown mention
  search list and sends only `value.ref` into its action.
- The per-plugin, per-space renderer bridge allowlists `ref.resolve`,
  `ref.backlinks`, `search.query`, `link.add`, `link.remove` and `link.list`.
  Resolved rows never enter plugin scope.
- `_links` is one host-owned synced store with a `space` column, schema
  version 1 and trash deletes. Mentions use local index schema version 2;
  neither the index nor mentions sync or enter backups. WIT is unchanged.
- Notes offers a link picker and backlinks; To-do shows backlinks and a
  chip for `note_id`. The existing To-do save-note flow supplies that id;
  Notes' `to_task` still forwards only task text and does not set it.

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
- First-party plugins use separate slim resolve queries so bodies and
  search-only filters do not affect chips.

The host batches resolves per plugin per render, and caches titles for the
life of a view (cleared on `data-changed`).

### 3.3 Making links

- **Mentions:** typing `@` in the Markdown editor opens a picker fed by
  `search.query`; choosing a hit inserts the Markdown link. The standalone
  `ref-picker` shares that list and runs its action with `value.ref`.
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
host-local `mentions(doc, to_ref)` table joined to indexed documents, so
backlinks need no extra pass.

**Relations** live in a host-owned, synced store `_links`, like `_pages`:

| Column | Notes |
| --- | --- |
| `id` | UUIDv7 |
| `space` | Space id |
| `source`, `target` | refs |
| `kind` | `related` in v1; later `parent`, `blocks`, … |
| `created_ms` | |

Last writer wins per column; deletes go to trash. Two devices may create
the same link; the UI shows one. The schema starts at version 1 and only
adds columns, as with every store.

### 3.5 Showing links

Renderer nodes:

| Node | What it shows |
| --- | --- |
| `ref` | A chip: kind icon, current title, done state; tap opens it |
| `backlinks` | "Linked from…" for `{{row.$ref}}`: mentions plus relations, grouped by kind, each a `ref` chip |
| `ref-picker` | Searches indexed items and runs an action with only the chosen ref |
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

- **Small kernel.** Ranked search across plugins can't be done by a plugin without reading other
  plugins' data, so the index lives in the host; see
  `docs/develop/architecture.md`.
- **Manifest.** New optional `index` and `refs`; new hosts accept old
  plugins; old hosts reject plugins using them (pre-1.0 rule).
- **WIT.** Unchanged.
- **Sync.** One new synced store, `_links`. Index and mentions are local.
- **Pages.** Page and widget focus may later hold a ref instead of a
  keyword; widgets would then show items linked to it.
- **todo's `note_id`.** Stays and is shown as a `ref` chip in the task
  sheet. Backlinks are indexed text mentions plus stored relations; the
  column by itself does not create a backlink.
- **AI.** The assistant may use the index to find context. Sending found
  items to a cloud provider needs the user's consent, as today.
- **CLI and MCP.** `search.query` keeps its shape and gains `kind` and
  `snippet`. New host commands: `link.list {ref}`, `link.add`,
  `link.remove`, `ref.resolve {refs}` and `ref.backlinks {ref}`.

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
