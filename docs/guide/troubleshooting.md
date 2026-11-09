# Troubleshooting

## The app build stops with a plugin bundle error

`Import a plugin bundle first` means no plugin bundle was imported.
`Imported plugin bundle differs from the approved lock` means the imported
bundle doesn't match `plugins.lock.json`. For a local build, build and import a
bundle with `--update-lock`, then build again (see
[Install on macOS](install-macos.md#3-build-the-plugins-and-the-app)).

## Devices don't sync

- Check that both devices are signed in to the same Tailscale network and that
  Tailscale is connected.
- On the phone, choose **Sync now** in **Devices and sync** and read the error,
  if any.
- The Mac must be awake with Terrarium open, unless
  [Google Drive](sync-and-devices.md#google-drive) is connected.
- A pairing code works once and expires after 5 minutes. Show a new one if
  joining fails.

## A change I made seems lost

Terrarium merges edits per field. If two devices edit the same field while
apart, the later edit wins. Notes merge text edits from both devices.

**Settings → Developer** has a **Conflict history**, **Trash notices** for
items edited on one device and deleted on another, and the **Change log**.

## Reminders don't appear

Allow notifications for Terrarium in macOS **System Settings → Notifications**
or Android **Settings → Apps → Terrarium → Notifications**. A reminder is shown
only on the device it is set for (phone, desktop or both).

## Voice captures are not transcribed

Transcription is off until it is set up on a Mac; see
[voice transcription](ai-and-schedules.md#voice-transcription). Captures are
kept and can be transcribed later.

## Start over

> **Warning:** this permanently deletes all Terrarium data on that device.
> Make a [backup](sync-and-devices.md#backups) and keep your
> [recovery phrase](sync-and-devices.md#recovery-phrase) first if you want to
> keep anything.

**macOS:** quit Terrarium, then move
`~/Library/Application Support/io.github.rahul445.terrarium/` to the Trash.
Release builds also keep keys in the login Keychain; delete the items named
`io.github.rahul445.terrarium/…` in Keychain Access.

**Android:** uninstall the app, or clear its storage in **Settings → Apps →
Terrarium → Storage**.

## Reporting a problem

Open an issue on [GitHub](https://github.com/RAHUL445/terrarium/issues) with
your platform, the steps and what you expected. Don't attach real data, keys
or recovery phrases. Report security problems privately; see
[SECURITY.md](../../SECURITY.md).
