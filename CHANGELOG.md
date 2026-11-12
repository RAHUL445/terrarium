# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
