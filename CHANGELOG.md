# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
