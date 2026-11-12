# Design: plugin network access

Status: as built for HTTP-02 through HTTP-05; HTTP-06 documents the contract.
HTTP-01 supplied the design only. Section 1 records the pre-implementation
baseline; section 3 preserves the implementation acceptance checklist, not a
claim of live-device/provider validation.

As-built source map: WIT/SDK and manifest validation in `wit/plugin.wit`,
`crates/guest/src/lib.rs`, `crates/runtime/src/manifest.rs`; deferred jobs and
transaction delivery in `crates/runtime/src/host.rs` and `registry.rs`;
transport, quotas, audit, vault and OAuth in `crates/net/src/`; foreground
workers and Settings services in `apps/tauri/src-tauri/src/`; consent,
activity and account forms in `ui/app.js`; Feeds in the sibling plugins repo.
Author reference: [Network](../develop/plugin-authoring.md#network).
User reference: [Network access](../guide/plugins.md#network-access).
Full key rotation/revocation (SEC-02) and general disk quotas (SEC-04) remain
deferred. No ambient socket access, background phone fetch or durable outbox
has been added.

Review: pending user review (approved to proceed)

Goal: plugins can fetch bounded web resources through a host-owned service,
without ambient sockets, access to other plugins' data, or credential access.
WebSockets, streaming, plugin servers and Android background fetch are out of scope.

## 1. Baseline before HTTP-02

- `wit/plugin.wit` defines JSON-based `ai.submit` and a declared callback
  operation. `crates/runtime/src/host.rs` checks permission, callback and
  request shape, queues immediately, and cancels jobs on rollback, commit
  failure and dry run. The AI queue is in memory (`MAX_QUEUED = 50`).
- `PluginHost::take_ai_job` and `deliver_ai` in
  `crates/runtime/src/registry.rs` associate jobs with scheduled runs.
  `apps/tauri/src-tauri/src/ai.rs` takes work under the host lock, releases
  the lock for network I/O, then delivers under the lock and emits
  `data-changed`. The queue cannot be taken while an operation owns the host.
- `crates/ai/src/client.rs` uses ureq 2, rustls, disabled redirects and a
  1 MiB bounded reader. `crates/sync/src/drive.rs` uses 10 s connect / 60 s
  request timeouts; these clients are precedents, not the plugin policy.
- `package::capabilities` produces ids and labels; `added_capabilities`
  diffs ids. `stage_install`/`confirm_install` require approval for additions.
  `Packages::approve_local` binds consent to exact package files and signer;
  publisher trust arriving through sync is not local execution consent.
- AI's `Sealer` uses XChaCha20-Poly1305, HKDF and name-bound authenticated
  data. AI keys live sealed in the synced `_ai.secrets` table. Tauri's
  `Sync::secrets_root` supplies a space root or a device root. Its separate
  `Secrets` backend uses Keychain, Android Keystore, or private files for
  device keys and Drive credentials. These are distinct storage mechanisms.
- Drive authorization supplies desktop loopback, state and PKCE precedents;
  it is provider-specific. `crates/host/src/control.rs` supplies a bounded,
  device-local audit pattern. `terrarium serve` has no device keys and
  sends neither AI nor proposed plugin HTTP traffic.

## 2. Network access — as built

### 2.1 Contract (question 1)

Implementation: as built.

The existing `terrarium:plugin@0.1.0` world includes this interface and
`import http;`; existing function signatures are unchanged:

```wit
/// Needs http permission and locally approved manifest hosts.
interface http {
  use types.{json, plugin-error};
  /// Permission, local approval, worker availability and queue/quota capacity.
  /// Does not promise a particular URL, account or DNS result will work.
  available: func() -> bool;
  /// Queue now; execution is possible only after the calling operation commits.
  /// Callback: {request, ok, status?, headers?, body?, error?, context}.
  fetch: func(request: json, on-done: string) -> result<string, plugin-error>;
}
```

Request example (all names are synthetic):

```json
{
  "method": "GET",
  "url": "https://api.example.com/items",
  "headers": {"Accept": "application/json", "If-None-Match": "example-etag"},
  "auth": {"account": "work"},
  "context": {"feed": "example-feed"}
}
```

`method` and `url` are required strings. Methods are exactly uppercase
GET, HEAD, POST, PUT, PATCH and DELETE. No CONNECT, TRACE or OPTIONS.
`headers` defaults to an empty string-to-string map, `body` to absent,
`auth` to absent and `context` to JSON null. Reject explicit null for
`headers`; optional `body` and `auth` may be null. `auth` is exactly
`{"account":"<name>"}`; account names are 1–60 ASCII letters, digits,
`-`, `_` or `.`. All request/auth objects reject unknown fields and
case-insensitive duplicate headers. The callback must be a declared operation;
callback authors mark it `cli: false`. Validate before accepting a job.

Bodies are UTF-8 text strings; reject bodies on GET/HEAD. Binary and base64
transfer are deferred. Response decoding is strict UTF-8, independent of
Content-Type charset; invalid text fails with `invalid-response`. HEAD and
204/304 replies have body `""`. Never interpret HTML or JSON in the host.

Success and failure callbacks, respectively:

```json
{"request":"opaque-job-id","ok":true,"status":200,"headers":{"content-type":"application/json","etag":"example-etag"},"body":"[]","context":{"feed":"example-feed"}}
```

```json
{"request":"opaque-job-id","ok":false,"error":{"code":"timeout","message":"The website did not answer in time.","temporary":true},"context":{"feed":"example-feed"}}
```

`request` is an opaque host-generated id unique across process restarts, not
an echo of the URL. Context is returned as the same JSON value. Success
always has status, headers and body; failure has none of them. All final
HTTP statuses, including 401/404/500, are successful transport results
(`ok:true`); plugins decide their application meaning. Expose only lowercase
`content-type`, `etag`, `last-modified`, `cache-control`, `expires`,
`retry-after`, `date`, `content-language`. For repeated allowed headers join
values with `, ` in wire order; never expose Set-Cookie, Location, server
headers or request headers. Do not expose the final URL.

Follow at most five redirects manually, within the original deadline.
Only GET/HEAD follow 301/302/303/307/308; preserve the method. Mutating
methods return the 3xx without following (no implicit replay or rewrite).
Resolve relative Location values, then repeat URL, host, DNS and credential
checks. Missing Location leaves a 3xx result; malformed/forbidden targets
or a sixth redirect fail `redirect-blocked`. Reject cross-origin redirects
when auth is present; for unauthenticated cross-origin GET/HEAD forward only
Accept, not custom/conditional headers. Never forward a body across origins.

### 2.2 Queue, transactions and delivery (questions 2, 10)

Implementation: as built.

The implementation uses AI's queue-at-submit/cancel-on-rollback model,
without a persistent outbox. Queueing reserves bounded capacity under the
host lock; workers cannot take it until the outer operation commits. Every request id
created by the operation is tracked, including interop/nested-call effects,
and cancelled on rollback, trap, commit failure or dry run. No worker or
network call runs inside an operation; workers use take/run/deliver.
HTTP-02 tests cover the lock boundary and rollback cancellation, including
nested calls and commit failures.

The queue is memory-only: no URLs, bodies or context persisted to a job file.
Restart/process death loses both queued and in-flight requests, and there is
**no restart callback** or automatic replay. The remote write might already
have happened. Recover scheduler runs left waiting as interrupted/failed on
startup, without replaying HTTP writes; explicit user/schedule retry is a
new request. Queued jobs older than 5 minutes fail `timeout` when next serviced.

A scheduled run stays Waiting while either AI or HTTP jobs for it are queued
or running, including requests submitted by callbacks. Track outstanding jobs
and aggregate failure/retry outcomes until the last callback; one reply must
not finish a run that still has work. Callback writes use normal transactions.
Callback failure rolls back those writes and fails the run; it never undoes a
remote action. Delivery is attempted once per in-memory job, no durable
exactly-once guarantee. Callbacks and scheduled operations must be safe to
repeat, and write APIs should use provider-supported idempotency keys.
No automatic network retries except the restricted auth retry in section 2.8.

### 2.3 Manifest, arbitrary feeds and consent (questions 3, 4)

Implementation: as built.

Use top-level `hosts`, optional/default empty for old manifests:

```json
{"permissions":["http"],"hosts":["api.example.com","*.example.org"]}
```

With http permission hosts must contain 1–20 entries; without permission
hosts must be empty/absent. Canonicalize IDNA to ASCII and lowercase before
matching, displaying and computing capability ids; reject duplicate canonical
patterns. DNS names have at least two nonempty labels, valid DNS label syntax,
labels ≤63 bytes, total ≤253 bytes, no trailing dot. Reject IP literals
(including URL-parser numeric IPv4 spellings), userinfo, ports, paths, schemes,
empty labels, embedded wildcard and special-use suffixes `localhost`, `local`,
`internal`, `home.arpa`, `test`, `invalid`, `example`, `onion` (including the
suffix itself). `example.com`/`.org` in examples are synthetic, not fetch targets.

Exact names match only that name. One leading `*.` matches one or more
subdomain labels, never the apex or a suffix lookalike. No public-suffix
inference is required: even `*.com` is broad authority explicitly displayed
as such. Requests permit HTTPS only, no userinfo or fragment, no IP host,
no non-443 port; explicit :443 is equivalent to default. Reject ambiguous
backslashes/control characters before URL parsing; use the same canonicalizer
for request, redirect, consent and vault hosts. HTTP feeds must migrate to
HTTPS; no LAN/development exception in production.

For user-typed RSS URLs choose option (a), a standalone `hosts:["*"]`:

```json
{"permissions":["http"],"hosts":["*"]}
```

`*` must be the only entry and still means only public HTTPS destinations.
Runtime per-host prompts (b) give narrower authority but need a new grant
protocol/UI and cannot prompt unattended jobs; manual Settings extensions
(c) require two steps for each subscription. Defer both. Feeds must disclose
that a malicious plugin can send its own stored data to any public website.
Typing a URL, a synced feed row, or a peer action is never a new grant.

Exact capability ids/labels:

- `http:api.example.com` → “Reach api.example.com”.
- `http:*.example.org` → “Reach any subdomain of example.org”.
- `http:*` → “Reach any website”.

Do not emit a redundant generic `http` capability. The install/update sheet
always adds: “This plugin can send data it can read to these websites.”
For `*`, add “Any website includes sites you have not added.” and an unchecked
“I allow access to any website” checkbox required by backend confirmation.
Actions are “Allow and install” / “Cancel” (updates: “Allow and update”).
One capability id per pattern makes growth appear in `added_capabilities`;
exact-to-wildcard or wildcard-to-`*` also asks again. Redundant additions may
ask again; prefer a conservative diff over proving set containment.

Approval remains device-local, bound to package digest and signer; synced
packages require local approval before loading. Recheck approval, active
package identity and current hosts at dispatch. Update/unload/removal cancels
queued work, retires in-flight generations and suppresses stale callbacks.
An already-sent request cannot be recalled. Settings → Plugins → <plugin>
shows hosts and “Network activity”; disabling/removing the plugin stops new
requests. No synced grant, token row or trusted-publisher row can bypass
local approval.

### 2.4 Devices and compatibility (questions 5, 9)

Implementation: as built.

Run on desktop and Android only while the app is open/foreground; do not
start requests in Android background. A suspended process may resume queued
work subject to the 5-minute age limit; process death loses it. Existing
`RunOn::Desktop` default stays; `Any` permits foreground phone execution.
No global cross-device request deduplication is promised. `terra preview`
runs unauthenticated HTTP with the same policy and an isolated audit/counter
folder, treating explicitly supplied dev packages as developer approval.
It has no production vault or OAuth access. `terrarium serve` has no worker:
`available()` is false and `fetch` returns `unavailable`, even without auth.

Keep API version 0.1.0 for this additive interface, not a claim of backwards
compatibility in both directions. HTTP-02 refreshed the SDK and rebuilt
plugins, with tests for old components and unused imports against the new
linker. Old manifests default hosts/oauth empty and retain their behavior.
Older hosts reject unknown fields/permissions or cannot link the http import;
`available()` cannot rescue a link failure. Upgrade every device consuming
the synced plugin set **before** enabling/updating network plugins. An older
host must leave an unsupported package inactive with an upgrade message,
never strip fields or run with reduced validation. Do not roll back a host
while such packages remain enabled. No new min-host manifest field here.

### 2.5 Limits and SSRF (questions 6–8)

Implementation: as built.

Hard defaults, not plugin-overridable manifest limits:

- Request body: 256 KiB UTF-8; context: 64 KiB serialized JSON; complete
  request JSON: 384 KiB; URL: 8 KiB; callback name must already be declared.
- Request headers: ≤32 entries, ≤16 KiB total names/values, ≤4 KiB each.
  Complete serialized callback: ≤4 MiB (including escaped body and context);
  exceeding this fails `too-large`, even if the raw body fits 2 MiB.
  Response headers: ≤32 KiB total on the wire, including unexposed fields,
  before buffering; an over-limit header block fails `too-large`.
- Response body: 2 MiB; read at most cap + 1 bytes, never unbounded allocate.
  Send Accept-Encoding: identity, compile without gzip/brotli decoding,
  reject non-identity Content-Encoding as `invalid-response`. Cap redirect
  bodies too (prefer close without reading). No compression bomb decoder.
- Connect: 10 s; whole job network deadline: 30 s including DNS, TLS,
  redirects, reads, refresh and any permitted retry. Read/write inactivity:
  ≤10 s and bounded by remaining deadline. Blocking OS DNS is not covered
  by a socket timeout: use a bounded resolver pool with a deadline and
  discard late results; no unbounded thread creation for stuck DNS.
- Per (device, space, plugin): one in flight, 20 queued; per device: four
  in flight and 50 queued across spaces/plugins. Fair dispatch across
  plugins. Queue reservations include staged jobs; cancellations free slots.
- Per (device, space, plugin): 60 network attempts per rolling hour,
  50 MiB transferred body bytes per UTC day, counting sent and received
  bytes, failures, redirects, OAuth exchanges/refresh and auth retries.
  Admission counts queued reservations to prevent overbooking. Persist
  counters/reservations in private device-local state; release unspent
  reservations on rollback. Before dispatch reserve worst-case remaining
  body budget; unused bytes are released after completion. On crash keep
  reservations charged conservatively. Read/write loops stop at remaining
  budget; day resets cannot erase rolling-hour history. Clock rollback must
  not reset counters early. Fail closed if counters cannot be persisted.

These are per-device network quotas, not disk quotas, billing guarantees or
protection against a compromised OS. SEC-04 still owns general storage quotas.
The as-built ureq transport wraps reads below header buffering with wire
header/body caps and deadline-aware socket I/O. A fixed four-worker DNS pool
with four pending lookup slots bounds resolver resources; late answers are
discarded. Body `take` alone would not enforce these controls.

SSRF policy deliberately rejects some globally reachable special-purpose
addresses to favor a small auditable rule. IPv4 rejects these entire ranges:
`0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`,
`169.254.0.0/16`, `172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`,
`192.31.196.0/24`, `192.52.193.0/24`, `192.88.99.0/24`,
`192.168.0.0/16`, `192.175.48.0/24`, `198.18.0.0/15`,
`198.51.100.0/24`, `203.0.113.0/24`, `224.0.0.0/4`, `240.0.0.0/4`
(includes limited broadcast). In particular CGNAT includes Tailscale peers.

IPv6 permits only `2000::/3`, minus `2001::/23`, `2001:db8::/32`,
`2002::/16`, `2620:4f:8000::/48`, `3fff::/20`. Everything outside
`2000::/3` is blocked, including unspecified `::/128`, loopback `::1/128`,
IPv4-compatible `::/96`, mapped `::ffff:0:0/96`, NAT64 `64:ff9b::/96`
and `64:ff9b:1::/48`, discard `100::/64`, dummy `100:0:0:1::/64`,
SRv6 `5f00::/16`, ULA `fc00::/7`, link-local `fe80::/10`, deprecated
site-local `fec0::/10`, multicast `ff00::/8` and reserved space. Reject all
mapped forms, even public mapped addresses; no zone identifiers. This also
blocks Teredo and 6to4 embedded-IP escapes. Review the tables when updating
the policy, using the [IANA IPv4 registry](https://www.iana.org/assignments/iana-ipv4-special-registry)
and [IANA IPv6 registry](https://www.iana.org/assignments/iana-ipv6-special-registry).

For every hop, resolve the canonical hostname once into a bounded answer
set (≤16 addresses); reject the whole hop if any answer is blocked. Empty
answers fail unreachable; sets exceeding 16 fail blocked. ureq 2's
`AgentBuilder::resolver` returns only these vetted socket addresses; the
default resolver cannot resolve the hostname again. Pin the checked addresses
through connection, retain hostname for TLS SNI/certificate verification and Host, disable proxy
configuration/environment discovery (`try_proxy_from_env(false)`), cookies,
automatic redirects and cross-job connection pools. Re-resolve and recheck
every redirect, including same-host redirects. No insecure TLS switch.
A test-only loopback policy may exist only in test compilation, inaccessible
from manifests, environment variables, preview options or production constructors.
Public IPs routed onto a LAN by a hostile OS/VPN are outside this DNS filter's
protection; OS routing compromise is outside the WASM boundary.

Allow plugin request headers only: Accept, Accept-Language, Content-Type,
If-None-Match, If-Modified-Since, Idempotency-Key and `X-*` except
`X-Forwarded-*`, `X-Real-IP`, `X-Api-Key`, `X-Auth-*`. Names use HTTP token
syntax; values reject control characters/CR/LF. Reject all other headers,
including Host, Cookie, Authorization, Proxy-*, Forwarded, Origin, Referer,
Connection, Upgrade, Transfer-Encoding, Content-Length, Accept-Encoding and
TE. Host sets framing and User-Agent: Terrarium. Cookie storage is disabled;
Set-Cookie is ignored. Secrets belong in host-managed bearer accounts; arbitrary
credential header schemes are deferred. A malicious plugin can still put its
own data in allowed headers or query strings; allowlisting is not DLP.

### 2.6 Errors, audit and privacy (questions 11, 12)

Implementation: as built.

Stable error codes; synchronous failures return WIT `{code,message}` and
never call back; failures after acceptance use the callback error object:

- `invalid` (false): malformed JSON/URL/method/header/auth/callback/context.
- `denied` (false): missing permission/approval, unlisted host, absent account
  or account-host mismatch.
- `unavailable` (false): worker/vault unavailable, unreadable or unapproved
  account; message “Sign in again on this device” where applicable.
- `queue-full` (true): queue reservations exhausted.
- `rate-limited` (true): hourly attempt limit; `quota` (true): daily byte limit.
- `too-large` (false): request, body or header cap exceeded.
- `blocked` (false): prohibited resolved address.
- `redirect-blocked` (false): redirect policy or hop cap.
- `timeout` (true): deadline or queue age exceeded.
- `unreachable` (true): DNS/connect/TLS/read transport failure.
- `invalid-response` (false): invalid UTF-8, encoding or protocol.
- `unauthorized` (false): OAuth refresh rejected; no API replay performed.
- `cancelled` (false): explicitly cancelled live work if callback remains
  valid; unload/update/removal suppress callbacks entirely.

`temporary` suggests a later attempt, not automatic retry and not proof a
remote mutation did not happen. Use fixed plain messages such as “This
website is not allowed”, “The reply is too large”, “Could not reach the
website”; never serialize URL/parser/transport/provider errors. The service
must not log request Debug, headers, bodies, account credentials or context.
Plugin-controlled log text and callback return values are not safe diagnostics.

Device-local `<data>/network-audit.jsonl`, private permissions, never synced:
`{at_ms, space, plugin, method, host, status?, code?, bytes_in, bytes_out, ms}`.
Host is canonical DNS name only; no port, path, query, URL, IP, header,
context, account name, response text or token. Count body bytes, exclude TLS
framing. Audit every attempted hop and pre-network accepted-job failure;
invalid submissions use only safe normalized fields (unknown host omitted).
At most 500 entries **and** 1 MiB, truncate oldest on every append/load,
bounded reads and line lengths, atomic rotation; never grow until restart.
If audit cannot be recorded, stop dispatch and surface a fixed local storage
error (`unavailable`). “Network activity” lists time, host, status/error and
sizes. Removal retains bounded historical audit for accountability; Settings
has “Clear network activity” for the plugin. No secret hints in audit.

### 2.7 Vault and compromised peers (questions 13, 14, 16)

Implementation: as built.

Accounts use a separate synced host-only `_vault` store, schema v1 with
additive migrations, distinct from `_ai.secrets` and device-local token
storage. Reuse the AI Sealer implementation/root, not its key row or permissions. Sync makes desktop
sign-in usable on phones; device-local storage would require separate phone
sign-in and contradict that first milestone. Sync necessarily trusts paired
devices with the shared space root: a compromised peer can decrypt shared
tokens. This design cannot claim to protect them from a space-key holder.

`accounts` columns: plugin, name, hosts (JSON text), kind (`bearer`/`oauth`),
sealed, key_id, hint, expires_ms, updated_ms; delete-wins. Row id is an
unambiguous encoded `(plugin,name)` tuple, not unchecked slash concatenation.
The encrypted envelope also contains space id, plugin id, account name,
exact hosts, kind, provider definition, client credentials and token set.
AAD is a versioned, length-delimited encoding of `vault`, space, plugin, name.
Verify plaintext bindings against all routing metadata; never trust mutable
synced hosts/token_url columns to expand where a decrypted token may go.
No plugin has query/storage/interop access to `_vault` or `_ai` secrets.

Account hosts are an explicit user-selected list (1–20) of **exact** public
DNS hosts within the plugin's current manifest authority; no wildcards or
implicit token/auth/API-host equivalence. Compare exact canonical hosts on
every injection. `auth.account` resolves only within current space/plugin,
never another plugin. Even a manifest `*` cannot expand an existing account's
hosts. Only the worker sees decrypted secrets; inject Authorization: Bearer
immediately before the checked TLS request. No request headers in callbacks.
A hostile allowed API may echo a credential in its response: no host transport
can guarantee otherwise; users trust that provider with the token. No heuristic
redaction can make arbitrary provider data a credential-isolation guarantee.

Synced account creation/replacement is not local consent. Maintain private
local activation records binding `(space, plugin, name, signer, sealed-value
digest, hosts, provider-config digest)` to user approval. Incoming or changed
credentials require “Use this account on this device” in Settings showing
exact API/provider hosts. Verified local refresh updates activation atomically;
peer refresh requires approval again. Validate local package approval too.
Encrypted peer injection/replay cannot create local activation; deletion still
removes availability. Existing approved tokens remain subject to normal
manifest checks. A compromised peer already holding the root/token remains
able to use it directly; activation prevents turning an honest host into a
new deputy, not compromised-device revocation (SEC-02).

Trusted main-window services only: list metadata/readability/activation,
put bearer, remove, sign in and approve local account. No command-registry,
CLI/MCP, plugin-callable service or template action can put/approve/read
credentials. Accounts form uses password fields, clears input after save,
never returns plaintext. Hint is fixed “Saved” (no token substring).
Secret types use redacted Debug, no serde plaintext exports. Validate bearer
values (nonempty, ≤8 KiB, no whitespace/control characters). Sealed envelope
≤64 KiB and ≤20 accounts/plugin, ≤200/space; no unbounded synced allocation.

On root transition, decrypt with the old root and reseal with the new root
before discarding it, preserving activation only for locally verified
resealing. If old root is unavailable or SEC-02 rotation cannot migrate,
fail closed with “Sign in again on this device”; never overwrite unreadable
ciphertext. SEC-02 owns complete rotation/revocation, not solved by key_id.

“Sign out” cancels related jobs, retires in-flight results, deletes the account
through the store API and clears local activation; sync propagates tombstones.
Uninstall does this for every plugin account; reinstall does not restore them.
Accounts removed by sync invalidate local activation. Late refresh cannot
resurrect deleted/replaced accounts: compare generation/digest before write.
Deletion cannot recall a sent token, erase old backups/change logs, or revoke
provider access. UI explains “Remove this account from Terrarium. Revoke access
at the provider to invalidate existing tokens.” No automatic provider revocation
endpoint in this milestone. Retain bounded audit as above.

### 2.8 OAuth (question 15)

Implementation: as built.

User-pasted bearer accounts are implemented by HTTP-04a. HTTP-04b implements
optional top-level `oauth`, default empty, ≤4 entries. Provider keys use 1–60
ASCII letters, digits, `-`, `_` or `.`. Example manifest
fragment, alongside `permissions:["http"]` and the three exact hosts:

```json
{
  "oauth": {
    "example": {
      "auth_url": "https://login.example.com/authorize",
      "token_url": "https://tokens.example.com/token",
      "scopes": ["items.read"],
      "api_hosts": ["api.example.com"]
    }
  }
}
```

Definitions reject unknown fields; both URLs are HTTPS/443 without userinfo,
fragment or query; exact hosts must be literal entries in manifest hosts
(not justified by `*`/wildcards). `api_hosts` is required, exact, nonempty,
≤20 and covered by manifest authority. This separates the recipient of API
tokens from the token server; do not infer API hosts from a login hostname.
Scopes ≤20, each ≤256 bytes, nonempty and no whitespace/control characters;
URLs ≤8 KiB. No manifest secret, script, discovery URL or arbitrary extra
OAuth parameters. Provider config capability id is `oauth:<kind>:<sha256>`
where the digest covers canonical auth/token URLs, sorted scopes and sorted
api_hosts as canonical JSON; label “Sign in to login.example.com for <plugin>”.
Any endpoint/path/scope/API-host expansion or change asks for local update
consent. Display token host, API hosts and scopes in the sign-in form.

User registers a provider client and supplies client id (and optional secret)
in Settings; seal client material with tokens. No bundled client secret is
confidential. Use system browser authorization code flow, S256 PKCE (fresh
32-byte verifier) and fresh ≥128-bit random state, per
[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252). Only desktop can initiate.
Phone text: “Sign in on your computer, then sync and approve this account on
this device.” No device-code/custom-scheme flow in scope.

Bind callback listener only to `127.0.0.1` random port, HTTP solely for this
host-created loopback callback (not a plugin fetch exception). Exact redirect
URI/path, GET only, bounded 8 KiB request headers/query, no duplicate state/code,
matching state and one-use code, five-minute overall sign-in deadline.
Invalid stray requests do not terminate a valid attempt; valid success/provider
denial, Cancel or timeout closes listener and frees port. Exchange only at the
locally approved token endpoint, with original redirect URI and verifier;
no redirects at token endpoints. DNS/SSRF/TLS/size/deadline rules apply to
host exchanges, ≤64 KiB token reply, strict known token fields with bounded
values, Bearer token type and sane positive expiry. Browser navigation is
user-visible and outside the host fetcher's redirect filter; it can redirect
for provider login. Do not expose a browser URL/code/provider error to plugins,
UI results or logs; show fixed status/error text. Do not reuse Drive's parser
unchecked; leave Drive behavior unchanged unless sharing is proven safe.

Refresh under a per-account single-flight lock outside the plugin host lock
when expiry is within 60 s. Token endpoint, client id and API hosts come from
the approved sealed envelope; never from plugin request or peer metadata.
Keep old refresh token if the provider omits a replacement. Store refreshed
credentials only if account generation/activation still match. On terminal
refresh denial require sign-in again. No refresh without local activation.
On a 401 allow at most one refresh and one API retry **only for GET/HEAD**;
mutating methods expose the original 401, though proactive refresh is allowed
before first send. All exchanges consume attempts/bytes and the remaining
30 s deadline; never reset it per retry. Authenticated cross-origin redirects
stay blocked, even between two bound hosts.

## 3. Phases and verification

This is the implementation acceptance checklist; HTTP-01 itself did not
run these network tests. HTTP-06 records its check results in the task brief.
Use synthetic fixtures and test-only transport policy; never fetch real sites in the automated suite.

- **HTTP-02:** exact WIT/SDK export and all plugin rebuilds; legacy manifest
  and component load; unused imports; unknown request/manifest/oauth fields;
  exact/wildcard/`*` normalization, IDNA, lookalike suffix, numeric IP,
  userinfo, port, fragment and special-use rejection; method/header/body/
  context caps; missing callback/permission; queue reservations/cancel;
  rollback, dry run, commit failure and nested interop prove zero dispatch.
- **HTTP-03:** table-test each blocked CIDR boundary and adjacent addresses,
  IPv6 mapped/transition/scoped forms, mixed DNS answer sets; resolver
  rebinding and checked-IP connection; same-host DNS change on redirect;
  proxy env disabled; TLS hostname checks; all redirect statuses, loops,
  cross-origin header stripping and no mutation replay; encoded/chunked/
  oversized bodies, huge headers, invalid UTF-8, compression rejection,
  slow DNS/TLS/drip reads and total deadline across five hops. Exercise
  per-plugin/global queue/concurrency fairness, hour/day rollover, restart,
  clock rollback and quota reservation races. Verify no headless worker,
  phone pause/queue expiry, lost queue/recovered waiting runs, multiple
  HTTP plus AI jobs, callback chaining/failure and stale-generation drop.
  Verify each host capability addition and backend `*` checkbox, local
  package approval on peer installs, audit content/rotation/I/O failure,
  activity display and safe fixed errors. No URLs/secrets in diagnostic captures.
- **HTTP-04a:** ciphertext has no synthetic plaintext; wrong key/AAD/space/
  plugin/account fails; metadata host/kind edits and peer-injected/replayed
  ciphertext never activate; bindings include signer and exact host;
  wildcard manifest cannot expand token recipients; no auth forwarding on
  cross-origin redirects. Services reject plugin/CLI callers; UI clears
  fields; Debug/log/error/audit contain no token substring. Root transition,
  unreadable vault, account limits, delete/uninstall/restore, concurrent
  remove-vs-refresh and late callback suppression. Refresh single-flight,
  rotation/omitted refresh token, 401 GET retry once, no POST replay,
  quota/deadline consumption. Phone requires explicit activation.
- **HTTP-04b:** fake provider verifies state, redirect URI and S256 verifier;
  missing/duplicate/mismatched state/code, wrong method/path, stray request,
  cancellation, denial, timeout and port cleanup; token endpoint rebinding,
  redirects, oversized/secret-echo error response; endpoint/scope/api_hosts
  edits cause consent; manifest unknown fields/old defaults; browser-only
  desktop boundary and phone guidance. Keep existing Drive tests green.
- **HTTP-05:** RSS/Atom parser caps (500 entries), malformed XML/DTD/entity
  expansion disabled, summary text stripping, URL scheme filtering, safe
  markdown and inert javascript/data links (including encoded forms), no
  remote image auto-load; host feed add/fetch/callback with context,
  conditional 304, duplicate callback/run deduplication, retention of the
  newest 200 items plus older unread items (as built),
  failed/oversized/non-UTF-8 feed and max 20 refreshes/run; `hosts:["*"]`
  consent. Preview plus parser/integration/UI tests, no bundle change.
- **HTTP-06:** relative links resolve; guide matches built WIT/manifest,
  errors, limits, activation and headless/phone behavior. Mark only verified
  controls implemented; keep SEC-02/SEC-04 and provider trust gaps explicit.

HTTP-01 itself ran all five host checks in tasks/CONVENTIONS.md, excluding
terrarium-app as directed. That design-only task had no runtime contract,
plugin or template changes, so cross-repo/build/native-app checks did not
apply to HTTP-01. Coordinator reruns
checks outside the sandbox before committing; socket restrictions are recorded
separately from code failures. Those HTTP-01 checks did not validate a
network implementation.

## 4. Decisions and differences from task files

- HTTP-02 keeps queue-at-submit with rollback cancellation; “queued only when
  commits” means visible to workers only after commit. It must cover nested
  calls and commit failure; HTTP-03 also rechecks local identity at dispatch.
- HTTP-02 permits a standalone `*` for Feeds. HTTP-03 supplies the extra
  backend checkbox and privacy disclosure. There is no runtime host grant UI.
- HTTP-03 budgets include outbound bytes, redirects and reservations, persist
  across restarts, and use a rolling hour. Add DNS/header bounds and disabled
  compression/proxies/pooling; body `take` alone does not meet this design.
  Possible transport changes stay within HTTP-03, not HTTP-01.
- HTTP-03 must aggregate scheduled AI/HTTP work and recover interrupted runs;
  no persisted request queue or restart callback. Preview is unauthenticated;
  headless has no HTTP worker. No write replay on network errors/redirects.
- HTTP-04a uses `_vault` but binds all security metadata inside ciphertext,
  uses unambiguous AAD/ids and device-local account activation. Synced tokens
  are not automatically authorized on phones. Remove token-tail hints.
  Sign-out retains bounded audit and cannot revoke tokens at the provider.
- HTTP-04a's unconditional 401 refresh/retry is narrowed to GET/HEAD;
  refresh is generation-checked and shares budgets/deadlines. SEC-02 full
  rotation remains a gap; fail closed when old secrets cannot be migrated.
- HTTP-04b adds required `api_hosts`, provider-config capability digests and
  explicit token-host/scope review. Token/auth hosts must be exact manifest
  entries. Keep Drive implementation unchanged unless proven sharing is safe.
- HTTP-05 uses `hosts:["*"]`, HTTPS/text-only feeds, no background phone
  fetch; user-supplied feed content never creates a grant or privileged IPC.
- HTTP-06 must not promise that hostile providers cannot echo tokens or that
  a compromised peer cannot decrypt space-shared tokens. These are trust limits.
- The brief's `docs/design/links-and-search.md` is absent in this worktree;
  the existing `cli-control.md` supplies the current-state/proposal/phases/
  decisions structure. `@RTK.md` was not found in either worktree or ancestor
  directories; no RTK-specific command workflow could be loaded.
