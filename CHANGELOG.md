# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

### Known issues

- See [SECURITY.md](SECURITY.md) and the [roadmap](docs/roadmap.md) for open
  security and release gates.
