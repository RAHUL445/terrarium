# Export and import

## Export a space

On a computer, switch to the space you want, then open **Settings → Spaces**
and choose **Export this space**. The ZIP file goes to
`Documents/Terrarium Exports`. When it finishes, **Show in folder** reveals it.
Export is available on computers only.

**The file is not encrypted. Anyone with it can read its contents.** Keep it
somewhere safe and delete it when you no longer need it. For encrypted files
that you can restore, use [backups](sync-and-devices.md#backups).

Inside the ZIP:

- `manifest.json` records the space, export time, apps, versions and counts.
- `data/<app>/<table>.json` contains each app table as an array of JSON items.
- `files/<app>/…` contains Markdown and CSV copies where the app offers them:
  To-do exports `tasks.csv`, and Notes exports Markdown grouped by project.
- `README.txt` explains the contents and the privacy warning.

Items in the trash are excluded from JSON unless you tick **Include items in
the trash**. Readable copies follow each app's export query; To-do and Notes
exclude trash from those copies even when the checkbox is ticked.
The JSON export is for reading or moving data elsewhere; it is not a restore
format. It does not replace a backup.

## Import tasks or notes

Switch to the space that should receive the items, then open
**Settings → Spaces → Import**. The choices depend on your installed apps.
With To-do and Notes installed, they are:

| What to import | Input |
| --- | --- |
| To-do · Tasks from a CSV file | One CSV file with all the headers below, even if some cells are empty |
| To-do · Task titles from a CSV file | One CSV file with a `title` header |
| Notes · Notes from a folder of Markdown files | A folder; available on computers only |

The full To-do CSV headers are:

```csv
title,notes,due,remind_time,priority,repeat,tags,done,in_progress,urgent,important
```

Headers ignore case and surrounding spaces; extra columns are ignored. Use
`YYYY-MM-DD` for `due`, `HH:MM` for `remind_time`, and 0–3 for `priority`.
Repeat values are `never`, `daily`, `weekdays`, `weekly` or `monthly`.
Boolean columns accept `true`, `yes`, `1`, `x` or `false`, `no`, `0`
(case does not matter). Leave optional cells empty. Rows whose mapped cells
are all blank are skipped. Titles are kept literally, including dates or
`#tags` in the title; put those values in their own columns if wanted.

For Notes, choose the folder containing your Markdown files. Each `.md` or
`.markdown` file becomes a note: the file name without its extension becomes
the title, its text becomes the body, and its parent path inside the chosen
folder becomes the project. For example, `Work/Plan.md` becomes **Plan** in
**Work**; a file directly in the chosen folder has no project. Other file
types and hidden file names are skipped.

Choose **Check** to try the import and see errors without keeping anything.
Choose **Import** to add the items. Each run accepts at most **2,000 items**
and **16 MB of text**. Split larger inputs into smaller files or folders.

Imports add new items; nothing is replaced or matched to existing items.
Importing the same input again creates duplicates. A failed item does not
undo successful items. The result shows imported, failed and skipped counts,
and lists up to ten failure reasons in the app. Fix those items and import
only them again to avoid duplicating the successes.

For scripts, see the [command-line guide](command-line.md#export-and-import),
which also explains the smaller limit over the control socket.
