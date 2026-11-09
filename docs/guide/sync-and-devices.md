# Sync and devices

Every device keeps a full copy of all your spaces and works offline. Changes
leave a device only encrypted with your space key and signed by that device.
There is no Terrarium server.

Your **computer hosts** the space: it keeps the encrypted change batches and
serves them to paired devices. Phones and other computers **join** it. Find
everything below in **Settings → Devices and sync**.

## What you need

- Terrarium on a Mac (the host) and on each device you want to add.
- [Tailscale](https://tailscale.com) on all of them, signed in to the same
  network. Devices talk to each other directly over it; the Mac serves sync
  on port 7810 of its Tailscale address (and on localhost), not on your LAN.
- Optional: [Google Drive](#google-drive), so a phone can sync while the Mac is
  asleep.

## Pair a device

1. On the Mac, open **Devices and sync** and choose **Show pairing code**. The
   Mac shows its address and a one-time code that expires in 5 minutes. This
   also makes the Mac the host if it isn't yet.
2. On the phone, open **Devices and sync → Join a space**, enter the **Host
   address** and **Pairing code**, and choose **Join**.
3. The phone receives the space key, encrypted for that phone only, and starts
   syncing.

Devices sync in the background every few seconds while the app is open, and
when you choose **Sync now**. New devices are paired from the computer; a
phone cannot be a host.

## Paired devices

**Paired devices** lists every device in the space.
Removing a device blocks it at this host.

> Removing a device is local to the host. It does not erase data the device
> already downloaded, and it does not yet rotate the space key. Treat a lost
> device's copy as exposed. See the [roadmap](../roadmap.md) and
> [device revocation](../security/device-revocation.md).

## Google Drive

Optional. With Drive connected, devices also exchange encrypted batches
through a hidden app folder in your Google Drive, so the phone can sync while
the Mac is asleep or offline. Google only sees encrypted data.

Terrarium uses your own Google OAuth client, so you set one up once:

1. In [Google Cloud Console](https://console.cloud.google.com), create a project
   and enable the **Google Drive API**.
2. Configure the OAuth consent screen, and add your Google account as a test
   user.
3. Create an **OAuth client ID** of type **Desktop app**.
4. On the Mac, paste the client ID and secret into **Google Drive** and choose
   **Connect Google Drive**. Sign in to Google in the browser window that opens.

Pair at least one device first; Drive is connected on the host. Paired devices
receive the Drive credentials encrypted with the space key on their next sync.

**Disconnect** stops this device from using Drive. To cut off every device,
also remove Terrarium's access in your Google account's security settings.

## Recovery phrase

**Recovery phrase → Show** displays 24 words. They are your space key: anyone
with them can read your space, and without them, losing every device loses
your data. Write them down and keep them somewhere safe and offline.

## Backups

**Settings → Backups → Back up now** writes one encrypted `.terrabak` file with
every plugin's data and the plugin list.

- On a Mac, backups go to `~/Documents/Terrarium Backups/`. On a phone they
  stay in the app's storage, and a phone must join a computer first.
- A backup can only be opened with the space's recovery phrase.
- **Restore from a file** merges the backup in; newer changes are kept.
- On a new device, enter the recovery phrase when restoring. A new computer
  that restores a backup becomes the host of that space, so devices paired
  before keep syncing with it.

There are no automatic or scheduled backups yet.
