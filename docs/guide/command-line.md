# Command line

The `terrarium` command line works with your data in the running app on
your Mac: it lists what you can do, runs plugin queries and actions (add a
to-do, edit a note, arrange a page), searches every plugin, shows pages in
the app window and follows changes as they happen. Scripts and AI agents can
use it the same way you do.

## Turn it on

1. In Terrarium, open **Settings → Developer → Command-line control** and
   turn on **Allow command-line control on this computer**. Until you do,
   nothing listens.
2. Under **Clients**, type a name (for example `Claude` or `my scripts`),
   choose what it may do, and choose **Add client**:
   - **Read only**: queries, search and `watch`.
   - **Read and change**: also actions that add, change or remove data,
     arrange pages and spaces, and show pages or messages in the app
     window. The app asks you to confirm before adding one.
3. Copy the token it shows. It is shown once; the app keeps only a
   fingerprint of it. If you lose it, remove the client and add a new one.

Each client has its own token, so you can see what each one did and remove
one without affecting the others.

## Install the command

The command ships inside the app at
`Terrarium.app/Contents/MacOS/terrarium`. The **Command-line control** card
shows its full path. Link it into a folder on your `PATH`:

```sh
mkdir -p ~/.local/bin
ln -s /Applications/Terrarium.app/Contents/MacOS/terrarium ~/.local/bin/terrarium
```

If you build from source, it is also at `target/release/terrarium` after
`cargo build --release -p terrarium-cli`.

## Log in

```sh
terrarium login
```

Paste the token when asked. It is saved to `~/.config/terrarium/token`,
readable only by you. Scripts can set `TERRA_TOKEN` instead.
`terrarium logout` deletes the saved token.

## Read

```sh
terrarium status                              # app version, this client, spaces, plugins
terrarium commands                            # everything this client can run
terrarium commands todo.                      # only To-do's commands
terrarium query todo.tasks --filter upcoming  # run a query
terrarium query todo.counts --space work      # in another space
terrarium search "milk"                       # search every plugin
terrarium search "milk" --all-spaces --plugin notes
terrarium watch                               # print events as data changes
```

`terrarium search` ranks results across plugins and shows a `kind` column
for indexed hits, such as Task or Note. Indexed search requires all words
and matches the last typed word as a prefix.

With `--json` (or when piped), hits include `kind` and `snippet` (both null
on fallback hits), plus `ref` (`plugin/table/id`) on indexed hits. The result
also includes `truncated` (some matches were left out) and `indexing` (plugin
ids whose indexes are still rebuilding). Existing `results` and `errors`
fields remain available.

### Read item references and links

These illustrative examples use placeholder IDs. Replace `<task-id>` and `<note-id>` with real item IDs in
this space. Keep the quotes: `refs` is a JSON array, while `ref` is a string.
All five link/ref commands operate in the default space unless you pass
`--space <space-id>`.

```sh
terrarium query ref.resolve --refs '["todo/tasks/<task-id>"]'
terrarium query ref.backlinks --ref 'todo/tasks/<task-id>'
terrarium query link.list --ref 'notes/notes/<note-id>'
```

`ref.resolve` accepts at most 200 refs and returns descriptions with state
`ok`, `missing`, `trash` or `unknown`; live descriptions include the title,
kind, done state and open target. `ref.backlinks` returns
`{mentions: [ref, …], indexing: [plugin, …]}` from indexed text, catching
up first; an unfinished index can return fewer mentions. `link.list`
returns stored relations in either direction (at most 500), with `other`
and `direction` (`out` or `in`). It does not include text mentions.

## Change

These need a **Read and change** client.

```sh
terrarium run todo.create --title "Call Sam tomorrow 6pm #work"
terrarium run todo.update --id <id> --priority 2
terrarium run todo.create --title "Try it" --dry-run   # show what would change, keep nothing
terrarium run todo.delete --id <id>                    # asks first; --yes to skip the question
terrarium run todo.toggle --done true --ids a,b,c
terrarium run todo.toggle --done true --ids @todo-ids.txt
terrarium run todo.toggle --done true --where tasks --where-params '{"filter":"today"}'

terrarium page list
terrarium page create Planning
terrarium page place <page> todo/today                 # add a widget, below the others
terrarium page place <page> _host/text --args '{"config": {"text": "Hello"}}'
terrarium page move <page> <placement> --x 6 --y 0 --w 6
terrarium page unplace <page> <placement>
terrarium page rename <page> "Weekly plan"
terrarium page remove <page>

terrarium space list
terrarium space create Work
terrarium space rename <id> "Side projects"
terrarium space remove <id>                            # data is kept; `space restore` brings it back

terrarium open todo --item <id>                        # show it in the app window
terrarium notify "Report is ready"
```

For the same illustrative placeholder IDs, add or remove a stored relation:

```sh
terrarium run link.add --source 'notes/notes/<note-id>' --target 'todo/tasks/<task-id>'
terrarium run link.remove --source 'notes/notes/<note-id>' --target 'todo/tasks/<task-id>'
```

`link.add` returns the existing live relation when the same directional
link is already present. Optional `--kind related` is the default and the
only supported kind. `link.remove` asks **Remove this link?** and trashes
all matching relations in either direction, returning `{removed: count}`.
It leaves both items untouched. Without an interactive terminal, removal
stops with exit code 7; pass `--yes` only after approving that removal.

- **Results** of an action come with the rows it changed: which plugin,
  table and row, and what happened (`create`, `set`, `delete` and so on).
- **`--dry-run`** runs a plugin action, reports the result and the changes,
  then rolls everything back, including changes it made through other
  plugins. Nothing is kept, no schedules change and no AI request is sent.
- **Destructive actions** (deleting, removing, clearing) ask you to confirm.
  Without a terminal to ask in, they stop with exit code 7 unless you add
  `--yes`. Run with `--dry-run` first to see what would happen.
- **Bulk actions** use `--ids` or `--where` (never both). `--ids` may be a
  comma list, whitespace-separated text, or `@file.txt`. `--where` runs one
  declared query and uses the `id` from each row, up to 500 items.
- **Partial bulk failures** still print the failed item ids and errors. The
  summary line is the command result, and the command exits with code 1 when
  any item fails.

## Use it from an AI agent (MCP)

`terrarium mcp` offers the same commands as tools to any agent that speaks
the Model Context Protocol, such as Claude Code or Claude Desktop. Add a
client for the agent first (give each agent its own), then register the
command with the agent. For Claude Code:

```sh
claude mcp add terrarium --env TERRA_TOKEN=<token> -- terrarium mcp
```

For agents configured with JSON:

```json
{
  "mcpServers": {
    "terrarium": {
      "command": "/Applications/Terrarium.app/Contents/MacOS/terrarium",
      "args": ["mcp"],
      "env": { "TERRA_TOKEN": "<token>" }
    }
  }
}
```

Without `TERRA_TOKEN` it uses the token `terrarium login` saved.

- **Tools** are the commands with `.` turned into `_`: `todo_create`,
  `notes_list`, `search_query`, `page_place`, `ref_resolve`, `ref_backlinks`,
  `link_add`, `link_list`, `link_remove`. A query that shares its name
  with an action ends in `_query`. A **Read only** client gets only the
  tools that read.
- **Arguments** are checked the same way as on the command line. Tools that
  work in a space take `space`, and plugin actions take `dry_run` to show
  what they would change and keep nothing.
- **Destructive tools** (deleting, removing, clearing) need your yes. If
  the agent can ask you directly, Terrarium asks through it. Otherwise the
  agent has to ask you and then pass `confirmed: true`; the agent's own
  permission prompt is the place to say no.
- **Bulk tool**: `plugin_run_bulk` runs one bulk-capable plugin action on
  many items (`ids`) or on query matches (`where`). If the target action is
  destructive, it needs confirmation the same way destructive single-item
  tools do.
- **Changes** to installed plugins update the agent's tool list while it
  runs. If you quit Terrarium, the tools report that the app isn't open
  and work again once it is.

## Without the app window

`terrarium serve` runs Terrarium in the background without its window, so
scripts and agents keep working after you close the app. It uses the same
data, clients and tokens, and command-line control must be on.

```sh
terrarium serve          # runs until the app opens; Ctrl-C stops it
```

- It runs plugin queries and actions, scheduled commands and reminders.
- It doesn't sync, send AI requests or show anything in a window: those
  wait until the app is open. `terrarium open` and `terrarium notify` fail
  while it runs, and `terrarium status` says there is no window.
- Opening the app takes over: serve finishes the command it is running
  and stops, and the app answers from then on. Serve won't start while the
  app is open.
- If the command can't find the app, set `TERRA_APP_BIN` to
  `/Applications/Terrarium.app/Contents/MacOS/terrarium-app`.

## Arguments, output and errors

- **Arguments** come from the command's description: `--name value`, or
  `--args '{"name": "value"}'` (or `--args @file.json`). Run
  `terrarium commands` to see each command's parameters; a `*` marks
  required ones. Dates (`today`) and similar clock values are filled in for
  you.
- **Output** is a table in a terminal and JSON when piped. Add `--json`
  to always get JSON. An action's JSON is `{"result": …, "changes": […]}`,
  plus `"dry_run": true` on a dry run.
- **Errors** go to stderr, as JSON when piped:
  `{"error": {"code": "...", "message": "..."}}`.

| Exit code | Meaning |
| --- | --- |
| 0 | Done |
| 1 | Failed |
| 2 | Invalid arguments or usage |
| 3 | No such command, item, page or space |
| 4 | The item is in the trash |
| 5 | Not allowed: unknown token, or the client may only read |
| 6 | Can't reach the app: it isn't open (nor `terrarium serve`), or command-line control is off |
| 7 | Needs confirmation: run again with `--yes`, or try `--dry-run` |

## What the app records

**Settings → Developer → Recent activity** lists what clients did: when,
which client, the command, whether it worked, whether it was a dry run, and
how many rows it changed. A bulk run appears as one entry with the bulk
target (for example `todo.toggle`) and item count. The log keeps the changed
rows (plugin, table, row and operation) but never the arguments or values.
The list and the client settings stay on this Mac; they don't sync.

## Security

- The app listens only on a socket file in its data folder that only your
  macOS user can open. There is no network port.
- A client token lets a program read everything in Terrarium, and a
  **Read and change** token lets it change everything too. Treat tokens like
  passwords, give each program its own, and remove clients you no longer
  use.
- Turning command-line control off, or removing a client, closes its open
  connections at once.
- Some things are never available from the command line: your recovery
  phrase, AI keys, pairing codes, restoring a backup, and trusting plugin
  publishers.
