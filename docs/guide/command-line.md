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

## Change

These need a **Read and change** client.

```sh
terrarium run todo.create --title "Call Sam tomorrow 6pm #work"
terrarium run todo.update --id <id> --priority 2
terrarium run todo.create --title "Try it" --dry-run   # show what would change, keep nothing
terrarium run todo.delete --id <id>                    # asks first; --yes to skip the question

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

- **Results** of an action come with the rows it changed: which plugin,
  table and row, and what happened (`create`, `set`, `delete` and so on).
- **`--dry-run`** runs a plugin action, reports the result and the changes,
  then rolls everything back, including changes it made through other
  plugins. Nothing is kept, no schedules change and no AI request is sent.
- **Destructive actions** (deleting, removing, clearing) ask you to confirm.
  Without a terminal to ask in, they stop with exit code 7 unless you add
  `--yes`. Run with `--dry-run` first to see what would happen.

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
| 6 | Can't reach the app: it isn't open, or command-line control is off |
| 7 | Needs confirmation: run again with `--yes`, or try `--dry-run` |

## What the app records

**Settings → Developer → Recent activity** lists what clients did: when,
which client, the command, whether it worked, whether it was a dry run, and
how many rows it changed. The log keeps the changed rows (plugin, table,
row and operation) but never the arguments or values. The list and the
client settings stay on this Mac; they don't sync.

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
