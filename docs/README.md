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
| [First steps](guide/first-steps.md) | Home and pages, widgets, apps, search, Companion, spaces |
| [Sync and devices](guide/sync-and-devices.md) | Pairing over Tailscale, Google Drive, recovery phrase, backups |
| [Plugins](guide/plugins.md) | Install, review, update, roll back and remove plugins |
| [AI, schedules and voice](guide/ai-and-schedules.md) | AI provider, scheduled runs, reminders, local transcription |
| [Appearance](guide/appearance.md) | Light and dark mode, themes, app icons, workspace layout |
| [Command line](guide/command-line.md) | Read and change your data from the `terrarium` command line or an AI agent (MCP), with or without the app window |
| [Troubleshooting](guide/troubleshooting.md) | Build errors, sync problems, conflicts, starting over |

## Develop

| Page | What it covers |
| --- | --- |
| [Architecture](develop/architecture.md) | How the host, plugins, storage and sync fit together |
| [Writing a plugin](develop/plugin-authoring.md) | Manifest, storage, queries, templates, permissions, schedules, signing |
| [Making a theme](develop/themes.md) | Theme file format, tokens and contrast checks |
| [Building from source](develop/building-from-source.md) | Debug builds, checks, plugin bundle lock, SDK export |
| [Design: command-line control](design/cli-control.md) | The command registry, control socket and CLI, by phase |
| [Design: global search and links](design/links-and-search.md) | Ranked search index across plugins, references between items, backlinks |

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
