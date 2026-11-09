# Install on macOS

Terrarium has no downloadable release yet, so you build the app from source on
your Mac. The build bundles the [recommended plugins](../../README.md#recommended-plugins).

Requirements: macOS on Apple silicon or Intel, with the Xcode command line
tools (`xcode-select --install`).

## 1. Install the tools

- [Rust](https://rustup.rs) (stable). The repository's `rust-toolchain.toml`
  adds the `wasm32-unknown-unknown` target automatically.
- Node.js 22 or later, and Python 3.
- The Tauri CLI:

  ```sh
  cargo install tauri-cli --version 2.12.4 --locked
  ```

## 2. Get the source

Clone both repositories into the same folder:

```sh
git clone https://github.com/RAHUL445/terrarium.git
git clone https://github.com/RAHUL445/terrarium-plugins.git
cd terrarium
```

## 3. Build the plugins and the app

```sh
cargo build --locked -p terra
python3 ../terrarium-plugins/scripts/build-release.py \
  --terra "$PWD/target/debug/terra" \
  --output ../terrarium-plugins/releases/local-001
python3 scripts/import-plugins.py ../terrarium-plugins/releases/local-001 --update-lock
cargo tauri build --bundles app --config '{"bundle":{"active":true}}'
```

The first build takes about five minutes on an Apple silicon Mac, plus
download time. When it finishes, the app is at
`target/release/bundle/macos/Terrarium.app`.

`--update-lock` rewrites `plugins.lock.json` for the bundle you just built.
That is expected for a local build; don't commit the change unless you mean to
approve a new bundle (see [building from source](../develop/building-from-source.md)).

Add `--debug` to `cargo tauri build` for a faster, unoptimized build at
`target/debug/bundle/macos/Terrarium.app`. Debug builds keep secret keys in
owner-only files instead of the Keychain.

## 4. Install and open

Drag `Terrarium.app` into `/Applications` and open it. An app you built
yourself opens normally. If you copy it to another Mac, macOS blocks it
because it is not signed or notarized. On that Mac, right-click the app and
choose **Open**, or run:

```sh
xattr -dr com.apple.quarantine /Applications/Terrarium.app
```

On first launch Terrarium creates the **Personal** space and installs the
bundled plugins. Continue with [first steps](first-steps.md).

## Updating

Pull both repositories, then repeat step 3 with a new output folder (for
example `local-002`). Build output folders are never overwritten. Replace the
app in `/Applications`; your data stays where it is.

## Where your data lives

| What | Where |
| --- | --- |
| Plugin data, sync state, settings | `~/Library/Application Support/io.github.rahul445.terrarium/` |
| Backups | `~/Documents/Terrarium Backups/` |
| Secret keys (release builds) | Login Keychain, items named `io.github.rahul445.terrarium/…` |
| Secret keys (debug builds) | Owner-only files inside the data folder |

To start over, see [troubleshooting](troubleshooting.md#start-over).
