# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Plugin renames (`former_ids`)

- Plugins can be renamed: a manifest's `former_ids` lists earlier ids. The first one stays the storage and sync id (database file, sync and backup store, events, schedules, reminders), and every former id still resolves in commands, refs, backlinks, links, deep links, page widgets, saved views, automations and installed-package records. MCP keeps the old tool names as deprecated aliases.
- To-do is now **Task manager** (`tasks`, formerly `todo`); its data stays in `todo.db`. The app moves this device's pins, layout, last place, Today and quick-capture choices to the new id once, keeping the old entries.

### Today

- Settings lists Plugins again on desktop, so the plugin manager is one click from Settings.
- The first rail item is called **Home** again (home icon, ⌘1); it opens the Today list. The extra gap under it in the left rail is gone.
- The left rail's Collapse button lines up with the other rail rows, and the right rail's Customize button closes the panel on a second click (unapplied changes are discarded, like Cancel).
- Toasts (like "Removed from today · Undo") are smaller and follow the light or dark theme; on phones they sit above the tab bar instead of covering it.
- A new Today view is the first navigation item (⌘1 / Ctrl+1).
- Today is now the home page and the app always opens on it. The old Home widget page is now called Dashboard and is pinned in the left rail; unpin it if you don't need it. The Open on start setting is gone.
- Pinned pages now use ⌘2–⌘9, because ⌘1 opens Today.
- Folders can be pinned to the left rail (from Pages and folders or Manage pins); a pinned folder opens in place to list its pages and subfolders, and in the icon rail it opens Pages and folders at that folder.
- Today shows My Day, Due, Habits, To sort, Catch up, Wrap up and Done from installed apps in one column, with a live summary line.
- Each item appears once, in its earliest section; long sections show "▸ N more", and Due marks the current time.
- Rows show quick actions on hover or focus, with Undo toasts; J/K/↑/↓, X, T, S, Enter, C and Esc work on Today.
- The capture bar files text into the Inbox, a list or My Day, sends links to Bookmarks and `note:` text to Notes, and records audio for Voice; on phones it fits the screen and row details wrap under the title.
- Customize hides or reorders Today sections, hides individual apps and switches row density, saved per space on this device.
- Plugins can contribute Today rows through the opt-in `today-v1` manifest contract (`docs/design/today-v1.md`); an invalid entry is skipped without blocking the plugin, and `terra check` reports it.

### Slim To-do page

- Collapsible page sidebars (To-do) have one toggle before the page title, collapse to an icon rail with count badges, toggle with ⌘\ / Ctrl+\, and remember the choice per page on this device.
- Plugin pages can use an inner view sidebar (`workspace` with `nav` groups, counts and hover ⋯ menus) that collapses to a "View ▾" menu below 900px.
- Plugin pages can use one list toolbar (`view-toolbar`): inline search, Filter with removable chips and "Temporary · Save view · Reset", Sort, Group, layout and Display menus.
- Plugin pages can use button menus (`menu`), single-line rows with hover actions (`item.layout: "line"`), 36/44px row density, a 22px page title and a capture box with its destination inside.
- Saved views can open inside a plugin page from its own sidebar, and a new view can start from the page's current filters.
- Task rows (To-do lists, widgets and Today) use a light round done control with a tick on hover, and show dates in the normal font as `10 Dec · 2pm`; plugin chips can be icon-only (`icon_only`, `label`) or use `format: "short-time"`, and a chip repeating its group heading is hidden.

### Docked details

- On windows 1200px and wider, a record opened from a list or from Today shows in a panel docked on the right instead of over the page, when the list keeps at least 560px beside it (otherwise, e.g. with the navigation rail or companion open, it opens as the usual sheet and switches as the room changes); the list stays usable and the open row is highlighted.
- With a record docked, J/K open the next or previous row's record (saving the current one first) and Esc closes the panel; narrower windows and phones keep the existing sheet.
- Notes opens its editor beside the note list on wide windows.
- Autosaving forms also save as soon as you leave a field; in a sheet their Saved / Saving… note shows once in the sheet header, so fields keep normal spacing.
- Plugin sheets can have a ⋯ menu in their header (`sheet.menu`), and collapsible sections can remember being open on this device (`section.remember`).
- Templates can read the current time as `{{$now}}`.
- A new saved view started from a page layout the view library lacks (say Kanban) can map it to a board layout (`draft.layouts`); an enum view column with `board: "matrix"` (four values) draws its board as a 2×2 matrix.
- The floating "Pin pages & apps" and "Pin app" buttons are gone; pin from the app or page ⋯ menu, or right-click a page or app in the rail or pinned tabs.
- The pinned-tabs bar above page content is gone; pinned pages and apps show in the left rail.
- Page content is centered in wide windows instead of hugging the left edge; Home and other widget pages can use up to 1600px.
- Capture boxes keep their chosen destination after adding an entry.

### Fixed in UI review round 2

- Quick-entry drafts no longer stall navigation when marked unsaved; returning to the app restores their text.
- Capture inputs clear on submit and recover failed entries without replacing a newer draft.
- Automation choices and summaries share short labels; habit check-ins and the Gym example use a titled habit picker as well as event bindings.
- The automation device picker shows “This device” once.
- Plugin permissions use plain descriptions for shared events, app data and extra resources.
- Plugin account sections sit below their action buttons and hide when unavailable.
- Navigation badges use an optional plugin query noun; Feeds reports unread articles.
- Browser preview opens article links in a separate browser tab.
- Feed rows distinguish unread articles from muted read articles and support locale date-only formatting.
- Display times can follow the user locale without changing stored times.
- Today can collapse undated tasks while preserving their membership and the saved view choice.
- Habit day dots show full-date tooltips, and heatmaps include months, weekdays and a check-in legend.
- Search highlights matched words and shows both item context and matching snippets.
- Workspace customization uses singular and plural count labels.
- Phone content clears the floating add button at the bottom of a list.
- Reminder fields can stay blank and disabled until a due date is set.
- Empty capture feedback no longer leaves a blank row above inbox status.
- Wildcard network access is shown as “Any website”.

### Added

- Ranked global search across apps and spaces, with full-text indexes and
  prefix matching for indexed apps.
- Links between items, Markdown @mentions, current-title chips and grouped
  **Linked from** lists for mentions and saved connections.
- Built-in View widgets on pages: list, sortable table, calendar and board
  layouts with app, data, sort, grouping, date and page-focus settings.
  Templates can also show tables and calendars from declared queries.
- Board and matrix card moves by drag-and-drop or keyboard, where the app
  declares a move action.
- Desktop quick capture with a global shortcut, `terrarium://open/...` deep
  links to pages and items, and Android text/link sharing with a save picker.
- Bulk selection with keyboard shortcuts and phone long-press, actions on up
  to 500 items, partial-failure results and Undo where the app offers it.
- Readable space export as a ZIP of JSON and app-provided Markdown or CSV,
  plus checked CSV task and Markdown note imports in Settings and commands.
- Encrypted file attachments with paste/drop in Markdown editors, local
  previews, sync, storage quotas, orphan cleanup and attachment-aware backups.
- Typed plugin events and approved automations between apps, with a chosen
  execution device, local consent and run history.
- Plugin HTTPS requests to approved websites, network activity, encrypted
  bearer accounts and desktop browser sign-in with approval on each device.
- Item History for opted-in tables, preview and restore as a new version,
  and synced per-space history visibility periods.
- Signed plugin catalog browsing and update review on desktop. Production
  discovery remains disabled until the registry activation gates are met.
- Simple declarative plugins without Wasm, and `terra new --declarative` to
  scaffold them alongside the existing Rust plugin tools.

### Changed

- Home cards pack into columns to avoid gaps below short cards. Settings
  navigation fits medium windows, desktop actions say Click, and note
  snippets hide attachment markup.
- Backup status shows the latest backup and its folder, with clearer
  permission errors and an app data fallback when Documents is unavailable.
- `TERRA_PROFILE_DIR` isolates app files, backups and webview preferences for
  separate test profiles.

### Fixed

- Each non-empty version restore starts a separate, labelled History entry,
  preserving the version it replaced directly below it.
- Bulk checkboxes and permission review text are clearer; attachment previews
  handle missing MIME metadata more reliably.
- Attachments work in browser preview. Desktop builds and plugin builds use
  the reconciled feature set and the intended plugin build directory.
- Peer sync connections close more reliably after rejected requests.

## [0.1.0-alpha.1] - 2026-11-10

### Added

- First public source preview of the Terrarium host for macOS and Android.
- Wasm component plugin runtime with per-plugin SQLite storage, capability
  consent, signed packages and resource limits.
- End-to-end encrypted sync over direct peers (Tailscale) and Google Drive,
  with per-column merge rules and Loro text merging.
- Shared web UI with declarative plugin templates, custom pages with a widget
  grid, board and matrix task layouts, search and themes.
- Spaces, scheduler and reminders, AI reports through OpenAI-compatible
  providers, and private voice transcription.
- `terra` developer CLI and the `terrarium:plugin@0.1.0` guest SDK.
- One command registry shared by the app's UI and outside callers: host
  commands for spaces, pages, search, schedules and the window, and plugin
  queries and actions described with parameters, effects and examples.
- `terrarium` command line, off until turned on in Settings → Developer:
  named clients with read-only or read-and-change tokens, an audit log of
  what each ran, dry runs that keep nothing, and confirmation for
  destructive actions.
- `terrarium mcp` serves the same commands as MCP tools for AI agents, asking
  the user through the agent before destructive ones.
- `terrarium serve` runs Terrarium without its window until the app opens,
  with a lock so only one process writes the data.

### Known issues

- See [SECURITY.md](SECURITY.md) and the [roadmap](docs/roadmap.md) for open
  security and release gates.

[Unreleased]: https://github.com/RAHUL445/terrarium/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/RAHUL445/terrarium/releases/tag/v0.1.0-alpha.1
