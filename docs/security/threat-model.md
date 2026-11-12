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
- Attachments are encrypted-at-rest blobs keyed from the space key. During sync,
  received blob bytes are accepted only if decrypt+id verification succeeds.
  Attachment quotas and size caps limit storage and transfer growth.
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
  `terrarium serve` runs the same services without a window, behind an
  exclusive lock on the data folder that the app also takes, so only one
  process writes the data; it has no access to the device keys, so it
  neither syncs nor sends AI requests.
- Events must be declared typed objects; names, fields, sizes and emission
  counts are bounded, and imported records are validated again. Package
  capabilities disclose replicated event fields. Plugin code cannot read
  events/rules or subscribe; the host transfers only explicitly mapped fields.
- Automation consent is device-local and bound to rule content, rule/space id
  and exact source/target package digests and signers. Review tokens detect
  stale sheets. Only trusted app services can approve, enable local execution
  or arm a space; CLI/MCP expose read-only rules/history. Synced flags cannot
  authorize execution. Keyless headless serve cannot execute automations.
- Local claims are durable before invocation, survive synced history pruning
  and prevent replay on that device. Chains stop at depth 3, fan-out is capped
  at ten, and local rates cap attempts at 60/rule/hour and 600/profile/hour.
  The 100,000-claim ceiling stops actions rather than evicting replay protection.
- Local `automation-audit.jsonl` records bounded identifiers, time, status,
  fixed error code and change count, without payloads, arguments, names, raw
  results or plugin errors. It uses private permissions and trims to the newest
  200 entries after 512 KiB. Audit preflight failure prevents action calls;
  failure after an action does not retry it. See the
  [as-built design](../design/events-and-automations.md).
- Dependency advisories are tracked with exposure guards
  ([review](dependency-advisory-review.md)).
- Blob images only render trusted raster types (PNG/JPEG/GIF/WebP). SVG
  attachments are treated as files, not inline images.

### Network

HTTP-02 through HTTP-05 implement the [network access design](../design/network-access.md).
The following controls apply to plugin HTTP, accounts and feed rendering.

- Malicious plugins may exfiltrate any data they can read to approved
  websites. Per-host install/update consent, an explicit “Reach any website”
  approval for arbitrary feeds, and device-local package approval disclose
  and bound that authority; they do not inspect outgoing data for privacy.
- The host mediates HTTPS requests after operation commit. Public-address
  filtering blocks LAN, loopback, link-local and CGNAT/Tailscale peers,
  including IPv6 and embedded-address escapes. Checked DNS addresses are
  pinned through connection, TLS verifies the original hostname, redirects
  are revalidated, and proxies/cookies are disabled. This prevents DNS
  rebinding and redirect escapes within the stated routing trust boundary.
- Bounded queues, concurrency, persisted attempt/byte budgets, header/body
  caps, disabled compression and deadlines covering DNS through network
  completion limit resource abuse, response bombs and slowloris. In-memory jobs are
  lost on process death; remote effects cannot be rolled back or safely
  replayed automatically. Headless serve sends no plugin HTTP requests;
  Android runs requests only while the app is open/foreground.
- Host-only sealed accounts are bound to space, plugin, exact recipient
  hosts and approved provider configuration. Plugins cannot read credentials
  or borrow another plugin's account. Local activation prevents peer-injected
  account/grant metadata from silently authorizing use on an honest device;
  authenticated cross-origin redirects cannot replay tokens elsewhere.
  A compromised peer holding the space root can decrypt shared credentials.
  A hostile approved API can echo tokens in its response. Neither is
  prevented by sealing or the WASM boundary. Complete key rotation and
  compromised-device revocation remain SEC-02 work.
- Desktop OAuth uses the system browser, loopback-only callback, PKCE,
  state and bounded one-use exchange. Consent names the provider to expose
  phishing risk; manifest signing does not establish provider trust.
- Device-local bounded audit records contain host, method, result, byte
  counts and timing, never URL paths/queries, headers, bodies or tokens.
  Internet feed data remains untrusted: declarative templates, safe markdown,
  inert unsafe links and no automatic remote images must prevent content
  from becoming script, privileged IPC or an unmediated network channel.

## Known gaps

Complete compromised-device revocation is not active
([design](device-revocation.md), [milestone](membership-v2-milestone.md)).
Event types cannot prove that a plugin's business event is true or prevent a
malicious plugin encoding sensitive data in declared strings. A compromised
paired peer can forge events that trigger an already approved rule, suppress
work through fabricated history, or falsify synced audit history. Local consent
prevents new mappings, not forged activity within an approved mapping. Payloads
are visible to authorized peers; there is no private-event mode or signed-event
attestation.

Event persistence is best effort across separate plugin/event transactions;
crashes can lose delivery or claimed work, and actions are never automatically
retried. Offline devices with stale primary settings can both act; converging
history is not exactly-once delivery. Seven-day event and 30-day run retention
are logical deletion, not physical erasure of tombstones or encrypted batches.
Durable outboxes, distributed leadership, safe retries and claim-ledger
compaction remain deferred.

Credentials remain sealed under the current space key until complete SEC-02
rotation/revocation. Network attempt/byte budgets do not supply a hard disk
quota (SEC-04). Retention controls and complete diagnostic-log review remain
SEC-03 work. Fuzzing and production-signed artifacts also remain open; see the
[roadmap](../roadmap.md). An agent whose client cannot ask its user can pass `confirmed: true` on its own; a **Read only** token is the way
to rule that out, and deleted items stay in the trash.

## Plugin account credentials

HTTP accounts live in the synced host-only `_vault` store. Plugins, the command
registry, CLI/MCP and declarative template actions cannot list, write, approve or
read credentials. Only the local main-window Settings service manages accounts.
The encrypted envelope binds the space, plugin, account, exact API hosts, token
kind and OAuth endpoint/client configuration; AAD binds a versioned tuple of
space/plugin/account. Metadata edits fail closed. Local activation binds the
sealed row digest and device-local row revision to the approved loaded package
identity (installed cache paths include package digest and signer). Peer-created,
changed or replayed credentials need explicit approval on this device. Only
fixed “Saved” hints are exposed. Preview has no credential-management services.

Refresh runs outside the host lock. The existing per-space/plugin worker slot
serializes refreshes for an account. Generation and activation are checked before
sending and storing; deletion/replacement cannot be undone by a late refresh.
401 replay is restricted to one GET/HEAD retry, sharing the deadline and private
attempt/byte budgets. Account removal retires callbacks and propagates deletion;
it does not revoke a provider token or erase change logs/backups. A trusted API
can echo a received token in its response, which remains provider-controlled data.

A locally available old sealing root is used to migrate readable credentials to
a new root. Unreadable ciphertext is retained and requires sign-in again. Full
space-key rotation and compromised-device revocation remain SEC-02 work; paired
peers with the space root can decrypt shared credentials directly. Desktop
OAuth browser sign-in and provider-manifest consent are implemented, with
PKCE, state, loopback-only one-use callbacks and bounded token exchange.
