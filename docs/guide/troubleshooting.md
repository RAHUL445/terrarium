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

## Backups cannot use Documents

Open **Settings → Backups** and read the folder shown below the backup list.
If Documents access is denied, Terrarium says **Terrarium can't use the
Documents backup folder** and stores backups in the app data folder's
`backups` directory instead.

On macOS, allow Documents access in **System Settings → Privacy & Security →
Files and Folders**, then try **Back up now** again. If the fallback is also
unavailable, check the app data folder's permissions. With `TERRA_PROFILE_DIR`,
backups stay in that profile's `backups` folder; a profile permission error
requires fixing that folder. See [test profiles](../develop/building-from-source.md).

## An attachment or an old version is missing

**Not on this device yet** means attachment bytes have not arrived. Sync
again and check the storage quota in **Settings → Devices and sync**. See
[Attachments](attachments.md) for limits and cleanup.

History periods hide older versions without erasing them. Check **Settings →
Spaces → Keep history for**. A listed **Current** entry can be too old for a
historical preview; open the item normally. See [Version history](history.md).

## Browse shows no plugin list

Production registry discovery is not enabled in this source preview. Install
from a package file under **Installed**. See [Plugins](plugins.md#browse-the-plugin-registry).

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
