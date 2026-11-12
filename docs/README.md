# Terrarium documentation

Terrarium is an extensible, local-first app host for macOS and Android. These
pages cover using the app, building plugins and themes, and the project's
security and release process. Start with the [project README](../README.md)
for an overview.

## User guide

| Page | What it covers |
| --- | --- |
| [Install on macOS](guide/install-macos.md) | Build the app with the recommended plugins, install it, update it, where data lives |
| [Install on Android](guide/install-android.md) | Build the APK, sideload it, permissions, updates |
| [First steps](guide/first-steps.md) | Home, pages, global search, mentions and backlinks, quick capture, deep links, Android share, bulk selection and undo |
| [Sync and devices](guide/sync-and-devices.md) | Pairing over Tailscale, Google Drive, recovery phrase, backups |
| [Export and import](guide/export-and-import.md) | Save readable copies of a space, import tasks from CSV and notes from Markdown |
| [Views on pages](guide/views.md) | List, table, board and calendar widgets, sorting, grouping and page focus |
| [Attachments](guide/attachments.md) | Add files, sync, storage limits, backups and cleanup |
| [Version history](guide/history.md) | Preview and restore versions, choose history visibility periods |
| [Plugins](guide/plugins.md) | Install, review, catalog browsing, website permissions, accounts, updates and rollback |
| [AI, schedules and voice](guide/ai-and-schedules.md) | AI provider, scheduled runs, reminders, local transcription |
| [Automations](guide/automations.md) | Connect app events to actions, review consent, choose a device and troubleshoot runs |
| [Appearance](guide/appearance.md) | Light and dark mode, themes, app icons, workspace layout |
| [Command line](guide/command-line.md) | Read and change your data from the `terrarium` command line or an AI agent (MCP), with or without the app window |
| [Troubleshooting](guide/troubleshooting.md) | Build errors, sync problems, conflicts, starting over |

## Develop

| Page | What it covers |
| --- | --- |
| [Architecture](develop/architecture.md) | How the host, plugins, storage and sync fit together |
| [Writing a plugin](develop/plugin-authoring.md) | Manifest, storage, queries, templates, permissions, schedules, events, signing |
| [Making a theme](develop/themes.md) | Theme file format, tokens and contrast checks |
| [Building from source](develop/building-from-source.md) | Debug builds, checks, plugin bundle lock, SDK export |
| [Design: command-line control](design/cli-control.md) | The command registry, control socket and CLI, by phase |
| [Design: global search and links](design/links-and-search.md) | Ranked search index across plugins, references between items, backlinks |
| [Design: attachments](design/attachments.md) | Encrypted files, transfer, quota and cleanup contracts |
| [Design: events and automations](design/events-and-automations.md) | Typed events, consent and execution |
| [Design: network access](design/network-access.md) | Plugin HTTPS, token vault and browser sign-in |
| [Design: version history](design/version-history.md) | Replay, restoration and retention |
| [Design: registry](design/registry.md) | Signed catalogs, review and production activation gates |
| [Design: declarative plugins](design/declarative-plugins.md) | Manifest operations without guest code |
| [Proposal: Home Screen widgets](design/home-widgets.md) | Native OS widgets, not shipped |
| [Research: JavaScript guests](design/js-guest-spike.md) | Experimental toolchain findings, not shipped |

The recommended plugins live in
[terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins), with their
own docs.

## Security

- [Threat model](security/threat-model.md)
- [Security policy](../SECURITY.md) and [privacy](../PRIVACY.md)
- [Peer HTTP authentication](security/peer-http-authentication.md)
- [Device revocation](security/device-revocation.md)
- [Membership v2 milestone](security/membership-v2-milestone.md)
- [Private audio](security/private-audio.md)
- [Dependency advisory review](security/dependency-advisory-review.md) and
  [Loro backport](security/loro-backport.md)

## Project

- [Roadmap](roadmap.md)
- [Releasing](releasing.md)
- [Contributing](../CONTRIBUTING.md) and [changelog](../CHANGELOG.md)
