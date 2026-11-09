# Install on Android

The Android app is built from source and sideloaded; it is not on any app
store. It runs on 64-bit ARM phones with Android 7.0 or later.

A phone works on its own, but it is most useful paired with a Mac running
Terrarium: the Mac hosts the space, and the phone syncs with it (see
[sync and devices](sync-and-devices.md)).

## 1. Install the tools

Everything from [Install on macOS](install-macos.md#1-install-the-tools), plus:

- [Android Studio](https://developer.android.com/studio), or the Android SDK
  command line tools with a JDK 17.
- In the SDK Manager: an Android SDK platform, platform tools (`adb`) and an
  NDK supported by Tauri.
- The Android Rust target:

  ```sh
  rustup target add aarch64-linux-android
  ```

Set the environment variables Tauri needs, for example in `~/.zshrc`:

```sh
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME="$ANDROID_HOME/ndk/<version>"
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
```

See Tauri's [Android prerequisites](https://v2.tauri.app/start/prerequisites/#android)
if anything is missing.

## 2. Build the APK

Clone both repositories and import a plugin bundle exactly as in
[Install on macOS](install-macos.md#2-get-the-source) (steps 2 and the first
three commands of step 3). Then, from the `terrarium` folder:

```sh
cargo tauri android build --debug --target aarch64
```

The APK is written under
`apps/tauri/src-tauri/gen/android/app/build/outputs/apk/`.

Debug builds are signed with Android's debug key. That is fine for your own
phone, but don't share them as releases.

Don't regenerate `apps/tauri/src-tauri/gen/android` with `cargo tauri android
init`; the project has hand-written native configuration.

## 3. Install on your phone

**With a cable (`adb`):**

1. On the phone, enable **Developer options** (tap **Build number** seven times
   in **Settings → About phone**), then turn on **USB debugging**.
2. Connect the phone and accept the prompt.
3. Install:

   ```sh
   adb install -r "$(find apps/tauri/src-tauri/gen/android/app/build/outputs/apk -name '*debug.apk' | head -1)"
   ```

**Without a cable:** copy the APK to the phone, open it from a file manager,
and allow installs from that app when Android asks.

## 4. First launch

- Open Terrarium. It creates the **Personal** space with the bundled plugins.
- For reminders, allow notifications. If none appear, turn them on in
  **Settings → Apps → Terrarium → Notifications**.
- Voice captures need microphone access. If recording fails, allow it in
  **Settings → Apps → Terrarium → Permissions**.
- To sync with your Mac, install [Tailscale](https://tailscale.com) on both and
  [pair the phone](sync-and-devices.md#pair-a-device).

## Updating

Build a new APK and install it with `adb install -r` (or open it on the
phone). Data is kept as long as the APK is signed with the same key, which is
true for debug builds from the same computer.

## Where your data lives

App data stays in the app's private storage and is excluded from Android
backups. Secret keys are encrypted with a key held in the Android Keystore.
Uninstalling the app deletes everything on the phone; data that was synced to
your Mac or saved in a backup is not affected.
