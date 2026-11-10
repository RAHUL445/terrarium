# Threat model

Assets: notes/tasks/habits/audio, database and backup contents, device identity and encryption keys, trusted publisher keys, signing credentials, and release artifacts.

Trust boundaries:
- Local main-window shell → Tauri IPC → privileged Rust services.
- Untrusted plugin manifests/templates/WASM → host storage, interop, AI, speech and scheduler APIs.
- Package files/Git/uploads/synced blobs → parser → install consent → executable cache.
- Paired network peers → sync ingestion → local stores and plugin set.
- Local command-line clients → control socket → command registry (desktop only, off by default).
- Build dependencies/CI → signed desktop and Android releases.

Assume malicious plugins, tampered archives, compromised peers, and malformed IPC/network input. A signed plugin identifies its publisher; it does not establish that its behavior is safe. Same-user native malware and compromised OS are not fully contained by a WASM sandbox. The shell is privileged: main-window ACL alone does not isolate individual plugins rendered inside it. Plugin templates must remain declarative and Rust must enforce every plugin API boundary.


## Implemented controls

- Plugins link only against `terrarium:*` interfaces, run with fuel, time,
  memory and host-call limits, and write only through the logged storage API.
- Packages are signed; installs and capability-expanding updates require local
  consent. Unsigned packages are rejected outside development builds.
- Tauri commands are enumerated and granted only to the local main window.
- Sync batches are encrypted and signed; peer HTTP requests carry a device proof
  and single-use challenge ([details](peer-http-authentication.md)).
- Speech workspaces are private and cleaned up ([details](private-audio.md)).
- Command-line control is off until turned on in Settings. The socket is a
  `0600` file in the data folder, not a network port. Each client has its own
  token (only its SHA-256 is stored) scoped to read only or read and change,
  and every command it runs is audited, with the rows it changed but without
  arguments or values. Destructive actions need an explicit confirmation from
  the caller, and dry runs roll back every store they touch. Outside callers'
  arguments are checked against the command's declared parameters before any
  plugin code runs, and the recovery phrase, AI keys, pairing and backup
  restore have no commands ([design](../design/cli-control.md)).
  `terrarium mcp` is only a client of the same socket with the agent's own
  token, so an agent gets exactly that client's scope; destructive tools
  ask the user through the agent when it supports elicitation, and
  otherwise rely on the agent asking before passing `confirmed: true`.
- Dependency advisories are tracked with exposure guards
  ([review](dependency-advisory-review.md)).

## Known gaps

Complete compromised-device revocation is not active
([design](device-revocation.md), [milestone](membership-v2-milestone.md)).
Retention controls, hard quotas, fuzzing and production-signed artifacts remain
open; see the [roadmap](../roadmap.md). An agent whose client cannot ask its
user can pass `confirmed: true` on its own; a **Read only** token is the way
to rule that out, and deleted items stay in the trash.
