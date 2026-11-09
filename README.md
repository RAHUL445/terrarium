<p align="center">
  <img src="branding/terrarium-logo.svg" alt="Terrarium logo: six tiles inside a rounded frame" width="128">
</p>

<h1 align="center">Terrarium</h1>

<p align="center">
  An extensible, local-first app host for macOS and Android. Every feature is a sandboxed WebAssembly plugin.
</p>

<p align="center">
  <a href="https://github.com/RAHUL445/terrarium/actions/workflows/checks.yml"><img src="https://github.com/RAHUL445/terrarium/actions/workflows/checks.yml/badge.svg" alt="Checks"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License: Apache-2.0"></a>
  <img src="https://img.shields.io/badge/status-experimental-orange.svg" alt="Status: experimental">
</p>

Terrarium is a host, not an app with a fixed feature set. It provides what
every personal-data app needs: storage, end-to-end encrypted sync between your
devices, a sandbox, a UI shell, a scheduler and AI access. Features arrive as
plugins that you install or write yourself. Every device keeps a full copy
of its data and works offline, with no project server.

> **Status:** experimental source preview. It is not a production release and
> has known security gaps; read [SECURITY.md](SECURITY.md) before using it with
> real data.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/home-dark.png">
  <img src="docs/screenshots/home-light.png" alt="Terrarium home page with widgets from the recommended plugins: today's tasks, quick voice capture, habits and notes">
</picture>

<table>
  <tr>
    <td><img src="docs/screenshots/todo.png" alt="To-do plugin listing today's tasks with tags, reminders and repeats"></td>
    <td><img src="docs/screenshots/habits.png" alt="Habits plugin showing daily progress, streaks and a week of check-ins"></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/notes.png" alt="Notes plugin listing Markdown notes grouped by project"></td>
    <td><img src="docs/screenshots/bookmarks.png" alt="Bookmarks plugin with pinned links, projects and tags"></td>
  </tr>
</table>

<sub>Terrarium running the <a href="#recommended-plugins">recommended plugins</a>. Screenshots use synthetic demo data.</sub>

## What the host provides

Plugins get these for free, so a plugin is mostly a schema, a few operations
and some templates.

- **Sandboxed plugins.** Each plugin is a WebAssembly component with its own
  SQLite database, capability-gated host APIs and per-call fuel, time and
  memory limits. Packages are signed; installs and permission increases need
  your consent. Plugin UI is declarative templates, so plugins cannot run code
  in the app shell.
- **Sync for every plugin.** Plugins declare per-column merge rules
  (last-writer-wins, counters, append-only, Loro text CRDT) and the host syncs
  their data. Batches are encrypted and signed, and travel directly between
  devices over Tailscale or through your own Google Drive app folder. Deletes
  go to trash by default.
- **One UI for desktop and phone.** A shared web shell renders plugin
  templates, custom pages with a widget grid where plugins share one screen,
  a search palette and themes.
- **Spaces.** Keep Personal, Work and other spaces apart while sharing one
  plugin list and pairing.
- **Scheduler and AI.** Plugins can schedule jobs and reminders and generate
  text through any OpenAI-compatible provider you configure, local or remote.
- **Developer CLI.** `terra` scaffolds, builds, checks, hot-reloads, signs and
  previews plugins in a browser.

See [architecture](docs/develop/architecture.md) for how the pieces fit together and
[writing a plugin](docs/develop/plugin-authoring.md) to build your own.

## Recommended plugins

The recommended plugins live in their own repository,
[terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins), and are
built against the same public SDK as any third-party plugin. The desktop and
Android builds ship them as a digest-pinned bundle. You can install more
plugins as signed `.terrapkg` packages from a file or a Git repository, and
roll back or remove any plugin; see the [plugins guide](docs/guide/plugins.md).

| Plugin | Features |
| --- | --- |
| **To-do** | Tasks with due dates, reminders, priorities, repeats and steps; list, board and matrix layouts |
| **Notes** | Markdown notes in projects with checklists and autosave; edits on different devices merge |
| **Habits** | Scheduled habits with check-ins, streaks, reminders and sleep logs |
| **Bookmarks** | Links with projects, tags and notes; pin the ones you use most |
| **Voice** | Record or type quick captures, transcribed locally, then send them to To-do or Notes |
| **Reports** | AI-written reviews of your tasks, habits and notes, on demand or on a schedule |

## Repositories

| Repository | Contents |
| --- | --- |
| **terrarium** (this one) | The host: runtime, storage, sync, scheduler, AI, shared UI, Tauri apps, `terra` CLI, guest SDK and WIT contract |
| [terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins) | The recommended plugins, built against a vendored SDK snapshot |

## Use the app

Terrarium has no downloadable release yet. You build it from source on a Mac,
which takes a few commands and bundles the recommended plugins.

**macOS**

1. Install Rust, Node.js 22+, Python 3, the Xcode command line tools and the
   Tauri CLI.
2. Clone this repository and
   [terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins) side by side.
3. Build the plugin bundle and the app, then drag `Terrarium.app` into
   `/Applications`.

Full steps: [Install on macOS](docs/guide/install-macos.md).

**Android** (64-bit ARM, Android 7.0+)

1. Add the Android SDK, NDK and Rust Android target to the macOS setup.
2. Build the APK with `cargo tauri android build`.
3. Install it with `adb install` or by opening the APK on the phone.
4. Pair the phone with your Mac over Tailscale to sync.

Full steps: [Install on Android](docs/guide/install-android.md).

Then read [first steps](docs/guide/first-steps.md) and
[sync and devices](docs/guide/sync-and-devices.md). The
[user guide](docs/README.md#user-guide) covers plugins, AI, schedules, themes,
backups and troubleshooting.

### Try it in a browser

To look around without building the app, run the host CLI's preview server with
the recommended plugins (no sync or device features):

```sh
cargo build --locked -p terra
for p in ../terrarium-plugins/packages/*/; do ./target/debug/terra build "$p"; done
./target/debug/terra preview ../terrarium-plugins/packages/* --port 8767 --data /tmp/terrarium-preview
```

Open <http://localhost:8767>.

## Development

[Building from source](docs/develop/building-from-source.md) covers debug
builds, plugin development with `TERRA_PLUGINS`, host and cross-repository
checks, the plugin bundle lock and the SDK export. Quick host-only checks:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --exclude terrarium-app
node --test ui/tests/*.test.cjs
```

## Documentation

All documentation is in [docs/](docs/README.md):

- **User guide:** [install on macOS](docs/guide/install-macos.md),
  [install on Android](docs/guide/install-android.md),
  [first steps](docs/guide/first-steps.md),
  [sync and devices](docs/guide/sync-and-devices.md),
  [plugins](docs/guide/plugins.md),
  [AI, schedules and voice](docs/guide/ai-and-schedules.md),
  [appearance](docs/guide/appearance.md),
  [troubleshooting](docs/guide/troubleshooting.md)
- **Develop:** [architecture](docs/develop/architecture.md),
  [writing a plugin](docs/develop/plugin-authoring.md),
  [making a theme](docs/develop/themes.md),
  [building from source](docs/develop/building-from-source.md)
- **Security and project:** [threat model](docs/security/threat-model.md),
  [security policy](SECURITY.md), [privacy](PRIVACY.md),
  [roadmap](docs/roadmap.md), [releasing](docs/releasing.md)

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Original code is licensed under [Apache-2.0](LICENSE). Vendored third-party
code keeps its own license; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
