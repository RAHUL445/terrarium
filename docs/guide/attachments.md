# Attachments

In Notes, open a note in **Edit** mode and paste or drop a file into the
Markdown editor. Images appear in preview; other files appear as links or
file cards. Other apps can offer file fields too. Each file can be up to
**25 MiB**. SVG files are not shown as inline images.

Terrarium keeps the file in your space and inserts a `terrarium:blob/...`
reference into the text. Keep that reference to keep the attachment. A plain
Markdown copy contains the reference, not the file bytes. Where a file card
has **Save**, you can save a copy on a computer; saving files is unavailable
on Android in this build.

## Sync and backups

Attachments sync encrypted with the space. Items and file bytes can arrive
at different times. **Not on this device yet** means the reference is here
but the file is not; connect your paired devices or Drive and sync again.
Upgrade all paired devices before installing apps with attachment fields.

[Backups](sync-and-devices.md#backups) include attachments. Older app versions
cannot restore the newer attachment backup format. A readable space export
is not a replacement for an encrypted backup of your files.

## Storage and cleanup

Open **Settings → Devices and sync** and find **Attachments**. It shows
local usage and **Quota**. The default is **2 GB**; the menu offers limits from
**512 MB** to **10 GB**. When the quota is full, adding or fetching more files
can fail while item references remain. Free space or raise the quota, then
try again.

Removing a link does not delete its file immediately. Automatic local cleanup
waits 30 days after a file is first recorded as unreferenced. **Clean up now**
uses a shorter one-day wait. Files referenced by rows in trash remain in use.
Cleanup can wait while spaces are closed so their references are not missed.
It removes local unused files, not copies on every peer or in old backups,
and does not promise secure erasure.
