# Building from source

For using the app, the [install guides](../../README.md#use-the-app) are enough.
This page covers working on the host and plugins.

## Prerequisites

- Rust stable with the `wasm32-unknown-unknown` target (pinned in `rust-toolchain.toml`)
- Node.js 22+ and Python 3
- Tauri CLI: `cargo install tauri-cli --version 2.12.4 --locked`
- macOS with Xcode command line tools for the desktop app
- For Android: Android SDK, Java and an NDK supported by Tauri; see
  [Install on Android](../guide/install-android.md#1-install-the-tools)

Clone [terrarium](https://github.com/RAHUL445/terrarium) and
[terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins) side by side.
Commands below run from the `terrarium` folder.

## Try the UI in a browser

Clone both repositories side by side, then build the host CLI and run it with
the recommended plugins:

```sh
cargo build --locked -p terra
for p in ../terrarium-plugins/packages/*/; do ./target/debug/terra build "$p"; done
./target/debug/terra preview ../terrarium-plugins/packages/* --port 8767 --data /tmp/terrarium-preview
```

Open <http://localhost:8767>.

## Isolated native test profiles

Set `TERRA_PROFILE_DIR` to an absolute, throwaway directory before launching the
native app. Host data, backups (`backups/`) and exports (`exports/`) stay in that
profile. Without a profile, release builds keep their usual storage locations.
If Documents is unavailable, backups fall back to the app data `backups/` folder;
Settings → Backups shows the active folder and how to allow Documents access.

Profile runs and debug builds also use separate WebView storage, shared between
the main window and Quick capture. The pinned Tauri 2.12.5 supports a
[`data_store_identifier`](https://docs.rs/tauri/2.12.5/tauri/webview/struct.WebviewWindowBuilder.html#method.data_store_identifier)
on macOS 14 and later; WebKit manages the physical location of these stores.
Windows and Linux use the profile's `webview/` directory (app data `webview/`
for debug runs without a profile). On older macOS, WebKit uses its default data
store, so UI preferences use keys prefixed by a stable profile ID exposed as
`window.terrariumProfileId`. All native profile/debug runs use that prefix,
including theme, space, saved views and layout preferences. Release builds
without a profile retain their existing keys. This older-macOS fallback isolates
UI preferences; it does not isolate WebKit cookies or caches.

## Build the desktop app

The app embeds an imported, digest-pinned bundle of the recommended plugins:

```sh
cargo build --locked -p terra
python3 ../terrarium-plugins/scripts/build-release.py \
  --terra "$PWD/target/debug/terra" \
  --output ../terrarium-plugins/releases/dev-001
python3 scripts/import-plugins.py ../terrarium-plugins/releases/dev-001 --update-lock
cargo tauri build --debug --bundles app --config '{"bundle":{"active":true}}'
```

For Android, use the same imported bundle:

```sh
cargo tauri android build --debug --target aarch64
```

The Android Studio project in `apps/tauri/src-tauri/gen/android` contains
project-specific native configuration; do not regenerate it blindly.

During plugin development, set `TERRA_PLUGINS` to built package directories
(separated by the platform path separator) to override the bundled versions.

## Checks

Host-only checks, no plugins checkout needed:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --exclude terrarium-app
node --test ui/tests/*.test.cjs
python3 -m unittest discover -s scripts/tests -v
```

Cross-repository checks:

```sh
export TERRA_PLUGIN_SOURCE="$(cd ../terrarium-plugins/packages && pwd)"
export TERRA_HOST_SOURCE="$PWD"
cargo test --locked --workspace --features terrarium-runtime/plugin-integration
node --test ui/tests/*.test.cjs ../terrarium-plugins/tests/*.test.cjs
python3 -m unittest discover -s ../terrarium-plugins/packages/notes/tests -p '*_query.py'
```

Integration suites fail, rather than skip, when the sources are missing.

## Plugin bundle lock

`plugins.lock.json` pins the approved plugin commit, SDK and artifact digests.
The importer verifies API version, package identity, required files, component
headers and SHA-256 digests, and stages only declared files under the ignored
`bundled-plugins/`. Approving a new bundle means reviewing it, importing with
`--update-lock` and committing the new lock. Digests prove integrity, not
publisher identity; signed `.terrapkg` packages cover trusted distribution.

## SDK

`crates/guest` and `wit/` are canonical here. To update the plugins repository:

```sh
python3 scripts/export-sdk.py --output /tmp/terrarium-sdk-export
```

Copy the export's `vendor/terrarium-sdk/` and `sdk.lock.json` into the plugins
repository together; never hand-edit the vendored copy. API changes need a
compatibility review, plugin rebuilds and the cross-repository checks.
