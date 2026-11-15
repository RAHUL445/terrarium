# First steps

Terrarium is organised around **spaces**, **apps** (plugins) and **pages**.
Everything below works the same on desktop and Android; on a phone the side
navigation becomes a bottom bar.

## Today, the Dashboard and pages

**Today** is the home: Terrarium always opens on it, and ⌘1 / Ctrl+1 returns
to it. It lists what needs attention today from your apps in one column.

The **Dashboard** is a page of widgets from your apps, such as today's tasks, a
quick voice capture, habit check-ins and recent notes. It is pinned in the
sidebar; unpin it if you don't need it, and find it again under **Pages &
folders**. You can add more pages from the sidebar with **New page**.

You can pin pages, folders and apps to the sidebar. Pin a folder with **Pin to
rail** in **Pages & folders**; in the sidebar it opens in place to show its
pages and subfolders. ⌘2–⌘9 / Ctrl+2–9 open the pins in order.

To change a page, choose **Edit page**:

- **Add widget** picks a widget from any app, or a note, link or clock.
- Add a View widget with **Add widget → Built in → View**, then open its
  **Settings** to pick the app and data, and choose **Show as** List, Table,
  Board or Calendar.
- Boards let you drag cards between columns (or use the card handle with
  arrow keys) to update status quickly.
- View widgets can filter with a **Keyword** or follow the page's focus with
  **Follow page focus**.
- Drag a widget by its header to move it and by its corner to resize it. With
  the keyboard, arrows move a focused widget and Shift+arrows resize it.
- **Duplicate** or **Delete page** from the same toolbar; **Done** saves.

See [Views on pages](views.md) for table, calendar and board settings.

**Quick add** on a page creates an item in an app without opening it.

## Apps

**All apps** lists every installed plugin. Open one to use its own pages, such
as the To-do list, board and matrix views. Pin the apps you use most so they
stay in the sidebar.

## Select many items

Lists that support bulk actions show a **Select** button. In select mode:

- Click **Select**, then click rows to mark them.
- On desktop, use **⌘/Ctrl-click** to start selecting from a row.
- Use **Shift-click** to select a range.
- Use **⌘A/Ctrl+A** to select all visible rows in that list.
- Press **Escape** to leave select mode.
- On a phone, long-press a row to start select mode.

Bulk actions run up to 500 items at a time. If an action reports
"10 done, 2 failed", those two failed rows stay selected so you can retry or
change the action. When available, the toast shows **Undo** for ten seconds.

In Notes, you can add images by pasting or dropping them into the Markdown
editor. Terrarium stores attachments in your space and inserts
`terrarium:blob/...` references in the note text. See [Attachments](attachments.md)
for file limits, sync and cleanup.

## Search and commands

Press **⌘K** (Ctrl+K on other keyboards), or choose **Search**, to find items
across apps and run commands. Search can cover **This space** or **All
spaces**. Use ↑ and ↓ to move, Enter to open and Esc to close.

Search finds tasks, notes, bookmarks, voice captures, habits and reports.
Results are ranked across apps; for apps with an index, all words must match
and the last word matches as you type, so `desk lam` finds “Desk lamp”.
“Indexing…” can appear briefly after an update or on a new device while
results are still being rebuilt.

## Links

In a note, switch to **Edit** if its preview is open, then type `@` and
part of an item's name, such as `@lamp`. Choose a result with a tap or the
arrow keys and Enter. Preview shows a chip with the item's current title;
tap it to open the item. A completed task shows as done. Deleted items
show **In trash** or **Missing**.

Open a note or task to see **Linked from**: items mentioning it and items
connected by a link. In a note, **Link to…** connects an item without
adding text. **Remove link** removes that connection; it leaves the items
and any mentions in note text intact. Links stay within the current space.

## Quick capture and sharing

On macOS, press **⌥Space** from anywhere to open **Quick capture**. Type your
text, choose where to save it, then press Enter. Press Esc to close. You can
change or turn off this shortcut in **Settings → Appearance → Quick capture**.

On Android, share text or a link from another app and choose **Terrarium**.
Terrarium asks where to save it. Nothing is saved until you pick an app.

You can also open pages with links in this form:
`terrarium://open/<app>/<page>?item=<id>&space=<id>`. For example, a note or a
shortcut can open one task directly. These links only open existing items and
pages; they never change data. On macOS, open Terrarium once from
`/Applications` before using these links from other apps.

## Companion

The sparkle button opens the **Companion** panel beside any page:

- **Assistant** – chat with your AI provider to plan or shape a rough note.
  It can include today's task titles if you tick that option. It only
  suggests; it cannot change your data. Requires [AI](ai-and-schedules.md).
- **Activity** – recent background work, such as scheduled runs.
- **Quick actions** – capture into an app without leaving the page.
- **Pinned widgets** – widgets you keep at hand.

Choose what appears in the left rail, the Companion and quick actions in
**Settings → Appearance → Customize workspace**. This is set per space.

## Spaces

A space keeps its own data, for example **Personal** and **Work**. All spaces
use the same plugins and are on all your paired devices.

- Switch spaces from the space name at the top of the sidebar.
- Create one in **Settings → Spaces → Create space**.

## See earlier versions

In To-do or Notes, open an item and choose **History**. Pick an earlier
version to see its values, then choose **Restore this version** and confirm
with **Restore**. Restoring saves a new change and syncs to your paired
devices. The values you replaced stay in history, so you can go back.
Restoring an item from trash brings it back too.

Edits from the same person on one device are grouped into one version when
there is no more than two minutes between edits. A continuous editing
session can therefore be one version even when it lasts longer. Versions
say **This device** or **Another device**; the newest is marked **Current**.
Each restore that changes anything starts a separate, labelled version.
Later edits can join that version. See [Version history](history.md).

In **Settings → Spaces**, use **Keep history for** beside a space to choose
**Forever**, **1 year**, **90 days** or **30 days**. This choice syncs between
your devices. Older versions are hidden from History and can’t be restored;
they are not erased and stay in the space’s sync data on your devices.
Choosing a longer period can reveal them again. **Current** stays listed
even when it is older than the chosen period, but its historical preview
and restore are unavailable; open the item to see its current values.

## Deleting and undoing

Deleting an item moves it to that app's trash; you can restore it from there.
If one device edits an item while another deletes it, the edit is kept and the
item stays in trash with a prompt to restore it.

If a note or file row references an attachment that has not downloaded yet on
this device, Terrarium shows **Not on this device yet** until sync fetches it.

## Next

- [Sync and devices](sync-and-devices.md) to add your phone or another Mac.
- [Plugins](plugins.md) to install more apps.
- [Back up](sync-and-devices.md#backups) and write down your
  [recovery phrase](sync-and-devices.md#recovery-phrase).
