# Command line

The `terrarium` command line reads your data in the running app on your
Mac: it lists what you can do, runs plugin queries, searches every plugin and
follows changes as they happen. Scripts and AI agents can use it the same
way you do.

For now clients can only read. Adding to-dos, editing pages and other
changes come in a later release.

## Turn it on

1. In Terrarium, open **Settings → Developer → Command-line control** and
   turn on **Allow command-line control on this computer**. Until you do,
   nothing listens.
2. Under **Clients**, type a name (for example `Claude` or `my scripts`)
   and choose **Add read-only client**.
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

## Use it

```sh
terrarium status                              # app version, this client, spaces, plugins
terrarium commands                            # everything this client can run
terrarium commands todo.                      # only To-do's commands
terrarium query todo.tasks --filter all       # run a query
terrarium query todo.counts --space work      # in another space
terrarium query page.list                     # pages of a space
terrarium search "milk"                       # search every plugin
terrarium search "milk" --all-spaces --plugin notes
terrarium watch                               # print events as data changes
```

- **Arguments** come from the command's description: `--name value`, or
  `--args '{"name": "value"}'` (or `--args @file.json`). Run
  `terrarium commands` to see each command's parameters; a `*` marks
  required ones. Dates (`today`) and similar clock values are filled in for
  you.
- **Output** is a table in a terminal and JSON when piped. Add `--json`
  to always get JSON.
- **Errors** go to stderr, as JSON when piped:
  `{"error": {"code": "...", "message": "..."}}`.

| Exit code | Meaning |
| --- | --- |
| 0 | Done |
| 1 | Failed |
| 2 | Invalid arguments or usage |
| 3 | No such command, item or space |
| 4 | The item is in the trash |
| 5 | Not allowed: unknown token, or the client may only read |
| 6 | Can't reach the app: it isn't open, or command-line control is off |

## What the app records

**Settings → Developer → Recent activity** lists what clients did: when,
which client, the command and whether it worked. Arguments aren't recorded.
The list and the client settings stay on this Mac; they don't sync.

## Security

- The app listens only on a socket file in its data folder that only your
  macOS user can open. There is no network port.
- A client token lets a program read everything in Terrarium. Treat it like
  a password, and remove clients you no longer use.
- Turning command-line control off, or removing a client, closes its open
  connections at once.
- Some things are never available from the command line: your recovery
  phrase, AI keys, pairing codes, restoring a backup, and trusting plugin
  publishers.
