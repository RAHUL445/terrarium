# Design: signed plugin registry

Status: design for ECO-02; no registry implementation or production endpoint
is enabled by this document.

Review: pending user review (approved to proceed)

Goal: browse a small signed catalog and discover updates, then reuse the
existing package review. Catalog authenticity never grants publisher trust
or permission to execute a plugin. This design adds no plugin manifest
fields, WIT imports, synced columns or plugin network permission.

## 1. What exists today

- `crates/runtime/src/package.rs` reads `.terrapkg` ZIPs, bounds compressed
  and expanded content to 32 MiB and files to 256, checks paths and verifies
  Ed25519 signatures. `Package::digest()` hashes the sorted paths, sizes
  and file hashes, excluding `signature.json`; it is **not the ZIP hash**.
  Package signatures cover UTF-8 `terrapkg-sig-v1:<digest>`.
- `crates/runtime/src/packages.rs` stores `installed`, `blobs`, and trusted
  publisher `signers` in synced `_packages`. Previous version/digest support
  rollback. `package-approvals.json` is device-local, keyed by
  `id:digest:signer`. `materialize_approved` requires current publisher
  trust and exact-package local consent even after sync or rollback.
- `apps/tauri/src-tauri/src/install.rs` stages Folder, File, Upload, Git
  and Synced sources. `stage_install` returns trust, signer fingerprint,
  capabilities, added capabilities and `shadowed_by_dev`. `confirm_install`
  consumes a token, expires reviews after 600 seconds, requires explicit
  trust for unknown publishers, requires approval of added capabilities,
  and records local consent. Production rejects unsigned packages.
- Plugin releases currently use `../terrarium-plugins/scripts/build-release.py`
  and `bundle.json`; `scripts/import-plugins.py` verifies individual file
  hashes against `plugins.lock.json`. Bundled installs use the sentinel
  digest `bundled`, not a package digest. Separate binary distribution is
  not yet enabled by `../terrarium-plugins/docs/releasing.md`.
- `manifest.rs` requires exact `API_VERSION` equality, currently
  `terrarium:plugin@0.1.0`. Host HTTP clients in `crates/ai` and `crates/sync`
  already use `ureq` 2 with TLS. Registry fetches are host operations;
  plugin HTTP (09-http) is unrelated.

## 2. Proposal

### 2.1 Wire format and URL layout

Format 1 is UTF-8 JSON without a BOM. All fields shown below are required;
reject unknown fields at every object level, duplicate object keys, wrong
JSON types, nulls, unsupported formats and out-of-bound values. No JSON
comments or trailing commas. Array order has no semantic meaning. Do not
parse and reserialize bytes before signature verification.

Exact example index (synthetic metadata; not a released package):

```json
{
  "format": 1,
  "registry": "terrarium-official",
  "generated_ms": 1794355200000,
  "expires_ms": 1794960000000,
  "sequence": 42,
  "plugins": [
    {
      "id": "feeds",
      "name": "Feeds",
      "description": "Keep up with your feeds.",
      "icon": "inbox",
      "publisher": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
      "versions": [
        {
          "version": "0.1.0",
          "api": "terrarium:plugin@0.1.0",
          "url": "https://plugins.terrarium.example/v1/packages/feeds/0.1.0/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.terrapkg",
          "size": 123,
          "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "capabilities": [],
          "min_host": "0.1.0-alpha.1",
          "yanked": false
        }
      ]
    }
  ]
}
```

The `.example` origin is a documentation fixture, never a default network
target. Before activation the owner supplies the real origin and package
base path in a reviewed host release. Concrete layout relative to that
compiled origin:

- `/v1/index.json`: latest index bytes.
- `/v1/index.sig.json`: detached signature for exactly those bytes.
- `/v1/packages/<id>/<version>/<digest>.terrapkg`: immutable package.

Signature JSON has exactly these fields:

```json
{
  "format": 1,
  "key": "jlk84PS9QVc4jRN2Uo2TppbWEbZXpcC2xyvLx4h2G4w=",
  "sig": "rUJF8EfN6ycDkvxTPSzEsmUouQmEoaZtj082q8TEwyOao6shAbhYXMRCk1GzRLKBi2Rj5i4b2AKKXxOavRlyAw=="
}
```

This signature verifies the exact example index block above, including its
final newline. It uses a disposable synthetic registry key, distinct from
the example publisher, and is not a production trust anchor. The example
package metadata does not identify an actual artifact.
`key` must be standard padded base64 of a valid 32-byte Ed25519 public key;
`sig` must be standard padded base64 of 64 signature bytes. Reject alternate
base64 spellings by decoding then checking re-encoding equality. Publisher
keys use the same canonical encoding. Compare full decoded keys, never
short fingerprints.

Let `H = lowercase_hex(SHA256(exact_index_bytes))`. Sign and verify exactly
UTF-8 `terrarium-registry-index-v1:` followed by the 64 ASCII characters of
`H`, with no trailing newline. This domain is distinct from package signing.
Registry identity, times and sequence are inside the signed bytes. The
signature's key is a selector for the compiled trust set, never a trust
anchor supplied by the server. One detached signature is sufficient for v1.

Validation bounds and semantics:

| Item | Rule |
| --- | --- |
| Index | At most 1,048,576 bytes, at most 16 levels JSON nesting |
| Signature JSON | At most 4,096 bytes, at most 4 levels nesting |
| Time/sequence numbers | Unsigned JSON integers, 0 through 9,007,199,254,740,991; sequence starts at 1 |
| Freshness | `generated_ms <= now_ms + 300000`, `expires_ms > now_ms`, and `0 < expires_ms - generated_ms <= 604800000` (7 days) |
| Registry | Exact compiled id `terrarium-official`, no server-selected registry |
| Plugins | 0–1,000 entries; unique ids matching `[a-z][a-z0-9-]*`, at most 63 ASCII bytes |
| Text | Name 1–100 UTF-8 bytes; description 0–2,000 bytes; no control characters; render as text, never HTML |
| Icon | 1–64 ASCII bytes matching `[a-z][a-z0-9-]*`; host icon lookup, unknown names use the host's generic plugin icon; never a URL |
| Versions | 1–100 per plugin; unique version strings; strict stable SemVer `major.minor.patch`, no prerelease or build suffix; each numeric component 0–4,294,967,295, no leading zeros |
| Compatibility | `api` 1–128 ASCII bytes; compare to `API_VERSION` exactly; `min_host` is strict SemVer 2, at most 128 ASCII bytes, with prerelease/build identifiers allowed |
| Size | Integer compressed ZIP byte count 1–33,554,432; exact downloaded length must match |
| Digest | Exactly 64 lowercase hex characters; package content digest, excluding signature |
| Capabilities | 0–128 unique strings, each 1–256 UTF-8 bytes without controls; sorted for output; capability **ids** from `capabilities(manifest)`, not labels |
| URL | At most 2,048 ASCII bytes, exact approved origin/path rules below |
| Yanked | Boolean; publisher retains old versions as tombstones rather than reusing their identity |

One publisher key per plugin id is deliberately restrictive. All versions
for that id must verify under it. Publisher key migration is out of v1:
freeze publication for that id until a separate reviewed migration protocol
exists, or publish under a new id with a fresh trust review. Changing a key
in an index is not proof of publisher continuity.

### 2.2 Registry trust, rotation and revocation

Ship one official registry configuration: id, HTTPS origin, package base,
active public keys, revoked keys and a minimum sequence. `REGISTRY_KEYS`
is empty until the owner supplies real keys; empty means **no requests**,
including from timers. No user-added registries, remote trust metadata,
trust-on-first-use, keys from `_packages.signers`, or keys from DNS. A
registry key cannot automatically become a trusted package publisher.
A debug-only environment override may supply a fixture origin/key in an
isolated test profile; release builds ignore it entirely and production
cache/state must never be reused with debug overrides.

Registry signing and publisher signing use separate keys and custody.
Rotation requires a host release shipping old and new public keys, then
switching the single detached signature to the new key, then a later host
release removing the old key. Clients without the new key stop refreshing
and expire safely. Do not accept a replacement key merely because the old
registry key signs it.

Compromise response is an expedited host release that revokes/removes the
key, pins a clean key and raises the minimum sequence beyond all known
accepted compromised snapshots. Explicit revocation overrides an active
entry and invalidates cached signatures at startup. A malicious accepted
sequence near the integer maximum requires a new registry id in that host
release; do not reset the old high-water mark. An old/offline host cannot
learn revocation immediately: its remaining exposure is bounded by expiry
only with a trustworthy clock; a compromised signer can issue fresh lies
until the host is updated. This is not a full TUF-style root/threshold
system. Publisher checks and consent still limit installation.

### 2.3 Verification, freshness and local state

`registry_index` and `registry_stage` share the following rules:

1. Check desktop/platform and nonempty configuration before networking.
   Load the registry's high-water state and compiled sequence floor.
2. Fetch both bounded bodies, verify detached signature against an active,
   non-revoked compiled key over the raw index bytes, then strictly parse
   and validate the entire index. Reject a wrong registry id. Never expose
   partially validated rows.
3. Enforce time limits above. Reject `now_ms + 300000 < last_verified_ms`
   as clock rollback and offer a clock error; never silently extend expiry.
4. Reject sequence below `max(compiled_floor, high_water.sequence)`.
   Equal sequence is accepted **only if the raw index hash is identical**
   to that sequence's stored hash. A different hash at equal sequence is
   equivocation. Greater sequence replaces it; republishing identical
   metadata with later expiry still requires an increment.
5. Atomically persist the verified raw bytes/signature and the updated
   high-water state before reporting a fresh list or allowing staging.
   Persistence errors fail closed; never advance on invalid, expired,
   mismatched or partially fetched data. Serialize concurrent refreshes and
   compare sequence again at commit so a slow older fetch cannot win.
6. Reverify the cache under current compiled keys, floor, time and state on
   every use, including startup and immediately before download/staging.
   HTTP status, ETag and local `verified: true` flags are not trust proofs.

Device-local `<data>/registry-cache.json` is a single atomic envelope
containing `format: 1`, `registry`, `index_base64`, `signature_base64`,
`sequence`, `index_sha256`, `last_verified_ms`, `last_successful_check_ms`.
These are local bookkeeping, not additional signed fields. Envelope limit
is 1,500,000 bytes; decode with individual wire limits. Validate that the
stored sequence/hash agree with the signed index. Write a temporary file
in the same directory, flush and sync it, rename and sync the directory;
if durability fails, no fresh result. Keep the previous file on failure.
No two-file sequence/cache transaction is needed.

Clearing cached payloads must retain sequence/hash/clock high-water fields
in the envelope (base64 payloads may be absent only in this local cleared
state). Settings provides no reset of these fields. Corrupt or inconsistent
state disables registry actions until restored from a known-good local
backup or repaired with a reviewed host migration. A genuinely new profile
starts at the compiled floor, with no prior hash; that floor alone cannot
identify equivocation at an unseen sequence. Loss of all local state or a
malicious local user can defeat local rollback protection; it is not a
trusted hardware counter.

On failure keep the previous verified list for display only while it
remains fresh. Once expired, show cached entries as out of date with every
install/update action disabled and no update-count badge. An invalid
refresh never authorizes using its bytes. An unexpired prior snapshot can
still be replayed until expiry (up to seven days); offline clients cannot
prove that a newer sequence exists. Manual-only checking can leave the
list expired indefinitely; it never extends validity.

### 2.4 HTTP and package staging

Use a dedicated host `ureq` client with normal TLS certificate/hostname
validation. Only GET; no cookies, credentials, authorization headers,
referrer, telemetry or user/space/device identifiers. Request
`Accept-Encoding: identity`; reject any non-identity content encoding.
Accept HTTP 200 only; no 304 shortcuts or range/resume in v1. Request
JSON for index/signature and octet-stream for packages; do not trust MIME
or Content-Length instead of byte validation.

Limits: 10 seconds connect, 10 seconds per blocking read, 30 seconds total
wall time for each index/signature/package request including DNS, TLS and
body. Enforce the total deadline during streaming, independent of trickle
reads; cancel and delete temporary bytes when exceeded. Bound reads at
limit + 1 even without Content-Length; reject oversized declared lengths
early. The package limit is `MAX_PACKAGE_BYTES`; normal ZIP expansion and
path checks remain mandatory. At most one refresh and one package download
per profile concurrently; work runs off the UI thread, outside the
plugin-host lock. No automatic download retries; partial releases retry on
the next check or user action, never by weakening verification.

Allow only canonical `https` URLs with exact compiled hostname, default
port 443, no userinfo, query or fragment. Index paths are fixed constants;
package paths must exactly match the layout constructed from validated id,
version and digest. Reject IP literals, localhost/private fixture targets
in production configuration, backslashes, escaped path separators,
traversal and alternate normalized spellings. Disable automatic redirects
and reject **all** 3xx responses, including same-host redirects. This makes
redirect-to-unlisted-host refusal deterministic. Do not add `*.github.com`
or a shared download CDN to the allowlist. GitHub Pages can host the static
layout on an explicitly pinned origin; a GitHub Releases redirect chain
would require a later narrowly reviewed policy and is not supported in v1.

On Install/Update the backend receives only id and version, never a
caller-supplied URL, digest, publisher or capability list. It resolves that
selection against its verified fresh snapshot, rejects missing/yanked or
incompatible versions, and downloads into bounded temporary storage.
Then, before `stage_install`:

1. Require exact ZIP length `size`; parse via `Package::from_zip` (including
   signature verification). Registry packages are signed in debug too.
2. Match `Package::digest()` to `digest`, full signer key to `publisher`,
   and manifest id, version and API to the entry. Require the manifest
   capability id set to equal the advertised set; on mismatch reject the
   misleading listing. The manifest remains the authority for review.
3. Recheck freshness and that the selection still exists, is not yanked
   and has the same tuple in the latest local snapshot. Reject a concurrent
   refresh that changes it. Pass the **same checked bytes**, not a path
   that can change, through `InstallSource::Upload` with a host-generated
   name such as `Registry: feeds 0.1.0`. Do not add a trust bypass.
4. Keep registry context with the pending review (registry id, sequence,
   entry identity). Immediately before confirmation, recheck that context
   against the current fresh snapshot and compatibility. A local yank,
   key revocation or expiry invalidates the pending registry review. No
   extra network request is required, so an unseen remote yank has the
   same bounded freshness limitation as the catalog.
5. Show the existing review. `Trust::Unknown` still requires `trust: true`;
   added capabilities still require `approve_capabilities: true`.
   `confirm_install` records `approve_local`, then installs through the
   existing store. Synced review uses `approve_synced`; synced publisher
   trust never grants local execution consent. Token expiry and cancel
   behavior remain unchanged.

Even an update with no added capabilities requires an explicit review and
confirmation of its exact files and signer. Neither index verification nor
a badge writes `_packages` or `package-approvals.json`. An authenticated
publisher can ship malicious code: review, Wasm isolation and permission
limits remain necessary; signing proves identity, not safety. A compromised
registry can lie about listings but cannot forge a different publisher's
package. If it substitutes a different publisher, that full key determines trust;
an unknown key needs trust approval, and even a previously trusted key
cannot reuse another signer's exact-package consent. Show the changed
fingerprint in the review.

### 2.5 Updates, bundled plugins and development folders

On desktop, checking defaults to on once a registry is configured. At
startup check if no successful check exists or 24 hours elapsed; while
running check when the next 24-hour deadline arrives. Coalesce refreshes
across spaces/windows; no timer per plugin. A failed automatic check records
an attempt and waits 24 hours before retrying, avoiding restart storms;
manual Check now bypasses this delay. Store attempt time and the
`automatic_checks` preference in separate device-local settings, never
sync them. Turning automatic checks off cancels scheduling; opening Browse
reads cache only and offers Check now. Empty keys or an unsupported platform
skip all requests regardless of this preference.

Select the highest stable numeric SemVer strictly greater than the
installed version, with `yanked == false`, exact API match and
`running_host >= min_host`. Filter compatibility before choosing the
highest version. No lexical comparison, downgrades, prereleases or same
version replacements. Installed plugin versions outside the stable grammar receive no automatic
update suggestion; show manual compatibility information. Parse running host
and `min_host` as strict SemVer 2, including alpha releases (the current
workspace is `0.1.0-alpha.1`). Compare major/minor/patch numerically, then
prerelease identifiers: a stable release exceeds its prereleases, numeric
identifiers compare numerically and sort before nonnumeric identifiers,
nonnumeric identifiers compare in ASCII order, and a longer otherwise-equal
identifier list sorts later. Ignore build metadata for precedence. Reject
leading zeros in numeric identifiers and invalid/overflowing components;
bound core numbers as above. An unparseable host version disables registry
installs/update suggestions with the compatibility message. `min_host` is
required publishing metadata, not inferred from API equality. A future format, manifest field or WIT change must raise the
minimum compatible host; new API versions require host upgrades first.
Old hosts reject unknown formats/fields safely rather than guessing.

`removed` records do not count as installed for badges, do not trigger
reinstallation, and may be explicitly installed again through review.
A yanked version is never newly downloaded/staged. An already installed
yanked version keeps running subject to existing trust/consent; show a
warning and offer a newer compatible version if one exists. Yank is not
remote uninstall or execution revocation. Existing locally retained
rollback remains possible under the current package consent rules, with a
warning when its target is known yanked; no fetching of yanked versions.

Merge the loaded bundled plugin inventory with `_packages.installed` when
computing updates; some bundled plugins have no installed row yet. Treat
`bundled` solely as source identity and compare the bundled manifest's
version normally; never compare that sentinel to a registry digest. A
registry replacement requires full publisher and capability review, even
for the same plugin id. Same-version replacement is not offered. Existing
rollback's `previous_digest = bundled` restores the binary shipped by the
currently installed host, which can differ after an app upgrade; label it
as the bundled version rather than promising immutable historic bytes.

A development folder wins on its device. Exclude `shadowed_by_dev` plugins
from update badges and disable registry actions for that id with a message
to remove the development folder first. Keep the staging review's existing
warning for a folder added concurrently; the registry must not silently
replace or remove it. Never publish dev-folder paths.

### 2.6 UI, privacy and phones

Settings → Plugins has Installed and Browse tabs. Browse search filters
locally by id, name and description, with no search requests. Cards show
host icon, name, description, publisher fingerprint via `key_fingerprint`,
newest compatible version and advertised capability summary. Names and
summaries are catalog claims; the download review displays checked package
metadata. Show distinct registry authenticity and publisher trust: never
label a publisher trusted solely because it is listed.

| State | Text/action |
| --- | --- |
| No configured key | “No plugin list is set up”; no request |
| Initial/manual refresh | “Checking for plugins…”; cancel supported |
| Empty list/search | “No plugins found” |
| Fresh compatible entry | “Install” or “Update”; opens existing review |
| Already current | “Installed”; disabled |
| Refresh/network failure | “Couldn't check for plugins. Try again later.”; “Check now” |
| Invalid signature/sequence/state | “Couldn't verify the plugin list.”; retain only prior safe display |
| Expired | “This list is out of date.”; “Check now”; install/update disabled |
| Clock rollback/future timestamp | “Check your device's date and time.”; actions disabled |
| Host too old/API mismatch | “Update Terrarium to install this version.”; disabled |
| Yanked installed version | “This version was withdrawn by its publisher.” |
| Development override | “Remove the development folder to install this plugin.” |
| Download failure/mismatch | “Couldn't verify this plugin. Check for plugins and try again.”; no review token |
| Android Browse | “Plugin downloads are available on desktop.”; no fetch/install/update |

Installed rows use “Update available (0.3.0)”; Settings → Plugins badge is
the number of installed ids with an eligible update. Badge reflects only
a fresh verified cache. No popup, automatic download, installation or
publisher trust action. The settings toggle is “Check for plugin updates
automatically”; adjacent text: “The server sees your IP address and check
times. Plugins download only when you choose to install.”

The static host/CDN learns IP address, request times, generic HTTP client
metadata, and the selected package id/version/digest on download. TLS does
not hide that from the server. No telemetry endpoint, unique user agent,
account, cookies, per-device ids, installed-plugin inventory, search terms,
user data or publisher approval records are transmitted. Fetch one whole
index, not per-plugin checks. Manual-only is device-local and available
before the first configured fetch. Registry selection/configuration and
cache never sync; package bytes/install records/publisher trust still do.

Android registry browsing, downloading and staging are disabled in v1,
including backend entry points; desktop handles discovery. Sync is **not**
a policy bypass: do not enable new registry-origin package delivery or
activation on Android until REL-03 explicitly approves downloadable Wasm,
including synced bytes. The existing general plugin sync is outside this
doc's changes; official registry activation is gated on resolving how
Android treats registry packages across sync before shipping. ECO-02 must
not claim existing sync makes distribution compliant or enable a desktop
registry rollout with unreviewed phone execution. After REL-03, any allowed
phone activation still requires that phone's exact-package local review.

### 2.7 Publishing and activation gates

ECO-02 adds `../terrarium-plugins/scripts/build-index.py`; ECO-01 adds no
script. It takes explicit `.terrapkg` artifacts plus reviewed catalog
metadata (name, description, icon, min_host, yanks), registry id, sequence,
timestamps and approved base URL. Run `terra pack` for signed artifacts;
`terra verify` currently emits human-readable output, so ECO-02 should add
`terra verify --json` with id, version, API, signer, digest and capability
ids to avoid parsing labels or reimplementing the package digest in Python.
Measure ZIP bytes for size. Fail on unsigned/unverified artifacts,
duplicates, inconsistent publisher keys, changed published identities,
unsupported versions, limits or metadata that disagrees with the package.
Same `(id, version)` always keeps its publisher/digest/size/URL; yanking
changes only `yanked`. Retain older entries within bounds; prune only in a
new sequence and never reuse versions. A new empty index is legal.

Script produces unsigned deterministic UTF-8 index bytes (sorted plugin
ids, numeric versions, capability ids; final newline included in signing).
Use explicit `terra sign-index <index.json> --key <registry-key-path>` to
validate and sign those exact bytes to `index.sig.json`, then independently
verify with the production trust configuration. Require `--key`; do not
implicitly use the developer publisher key from `TERRA_SIGNING_KEY` or
its default path. Reuse the key parser, not its default selection. Do not
load private keys in the index-building script or store them in the repo,
release metadata, stdout, logs or fixtures. Registry signing belongs to a
restricted release environment; publisher keys remain separately held.

Upload immutable packages first, verify them from the proposed serving
origin, then publish index and signature as one deployed snapshot where
possible. A mixed deployment fails verification and retains the last valid
cache; never accept mismatched bytes to improve availability. Refresh the
index at least daily with an incremented sequence and at most seven days
validity, even when no versions change. Keep publication sequence history
off the client; restored publishing backups must never roll it backward.

Before the **official registry is turned on**, require all of:

- REL-01 review of artifact licensing, SBOM, source/build provenance,
  custody/access controls for both signing roles, reproducible verification,
  key backup/rotation/compromise response and yank procedures.
- REL-03 decision covering desktop-to-Android synced packages and an
  implementation that enforces its approved platform behavior. Until then,
  keep official keys empty on all release platforms.
- Owner-provided real HTTPS origin/path and registry public keys, deployment
  tests for limits and redirects, initial sequence floor and fresh signed
  index, tested expiry/revocation recovery and ECO-02 validation below.

## 3. Alternatives and tradeoffs

| Question | Decision and tradeoff | Alternative deferred/rejected |
| --- | --- | --- |
| Encoding/signature | Raw JSON byte hash with domain-separated Ed25519; easy static serving, whitespace changes require resigning | Canonical JSON avoids whitespace sensitivity but adds cross-language canonicalization complexity |
| Trust roots | Separate compiled registry keys; no automatic publisher trust; host releases needed for rotation/revocation | Self-updating root/threshold metadata offers richer recovery at substantially larger implementation scope |
| Registries | One official configuration; predictable identity and allowlist | User registries introduce per-registry trust, conflicts and URL policy settings |
| Freshness | Local sequence/hash plus seven-day expiry; works offline until expiry | Online transparency/time service would improve freeze detection but add infrastructure and privacy costs |
| Hosting | Static exact-origin files, zero redirects | GitHub Releases is convenient but redirect/CDN policy broadens the network boundary |
| Updates | Stable versions only, manual review for every update | Automatic updates weaken meaningful per-device consent; prerelease channels need separate policy |
| Cache | Atomic device-local envelope with retained high-water fields | Syncing cache/preferences couples devices' clocks, rollout state and privacy choices |
| Phones | No registry operations until platform policy resolved; official activation also gates synced delivery | Desktop-only UI without a sync gate could still deliver unreviewed downloadable code to phones |
| Bundles/dev | Explicit reviewed replacement; development precedence remains | Matching id/name alone cannot establish publisher trust or package identity |
| Publisher rotation | One key per id, explicit future migration | Replacing the listed key would silently reassign old version authority |

## 4. ECO-02 implementation and verification

Use `registry_index.rs` in runtime; `registry.rs` already means PluginHost.
Keep pure parsing/signature/freshness/selection functions apart from HTTP
and local persistence, with injected clock, trust config and transport for
synthetic tests. Tauri commands own fetching, platform gating and staging
integration; never fetch while holding the plugin-host mutex. This is a
static catalog contract, not a new hosted API or plugin HTTP interface.

Required tests for ECO-02 (in addition to all conventions checks):

- Valid exact-byte signature; changed byte, wrong/unpinned/revoked key,
  malformed/noncanonical base64, wrong registry/domain, unsigned package,
  signature JSON overflow, duplicate keys, unknown fields, invalid numeric
  types, nesting, empty list and every boundary limit.
- Expiry at `now`, future generation beyond skew, inverted/overlong
  lifetime, clock rollback, sequence rollback, identical equal-sequence
  replay, changed equal-sequence hash and compiled floor enforcement.
- Invalid responses cannot advance state; stale concurrent response cannot
  overwrite newer state; atomic-write failure, corrupted cache, restart,
  payload clearing retaining sequence, old-key cache after revocation and
  cache expiry between download, review and confirmation all fail closed.
- Oversized index/package with and without Content-Length, false size,
  ZIP expansion, trickle/total timeout, cancellation cleanup, gzip refusal,
  HTTP downgrade, traversal/encoded URLs, userinfo, unexpected ports and
  redirect to an unlisted host (and same-host redirect) refused.
- Package digest versus ZIP hash, altered package/signature, wrong id,
  version/API/publisher and misleading capability ids all refused before
  staging. Index refresh/yank during download or review invalidates staging
  or confirmation. Checked bytes are the staged bytes.
- First publisher requires trust; known publisher still reviews new exact
  version; added capabilities require backend approval; cancel/expired
  token changes nothing. Reuse security tests
  `synced_publisher_trust_does_not_transfer_execution_consent` and
  `unsigned_updates_and_changed_publishers_cannot_reuse_consent`.
- Numeric `0.10.0 > 0.9.0`, highest compatible version selection,
  incompatible newer version fallback, alpha host/minimum ordering, API mismatch,
  prerelease/invalid versions, yanked/removed entries, no downgrade or
  same-version offer, bundled-only inventory and dev-shadowed ids.
- Empty production keys and Android entry points make zero requests;
  manual-only mode has no startup/timer/Browse fetch; checks coalesce,
  failure throttles across restart; local search sends nothing; badges
  expire; registry settings/cache never enter sync.
- UI strings/states, safe text/icon rendering, disabled incompatible/yanked
  actions and existing review flow. Keep ordinary file/folder/sync install
  regression tests separate from the stricter registry source checks.
- Publisher tooling creates independently verifiable output, rejects
  changed release identity and publisher metadata, requires explicit
  registry key, never leaks secrets, and handles mixed-deploy snapshots.

## 5. Differences from task files

- `docs/design/links-and-search.md` referenced by ECO-01 is absent in this
  checkout. Follow the existing `cli-control.md` structure: current state,
  proposal, alternatives/decisions and implementation details.
- ECO-02's proposed verifier arguments need registry configuration, the
  prior hash and clock high-water state as well as last sequence. Allow
  equality only for identical bytes; sequence alone is insufficient.
- ECO-02's `updates(index, installed)` needs running host version plus
  loaded bundled/dev inventory; installed records alone omit bundled-only
  plugins and development precedence.
- Cache is an atomic raw-byte/signature/state envelope, not a trusted
  deserialized list. `registry_index(false)` obeys cache/check scheduling;
  Browse never overrides manual-only preference. Add explicit refresh for
  Check now and local settings for automatic checks/last attempt.
- Preserve registry context through pending review and confirmation, not
  only through download; Upload is the byte transport, not the authority.
- Add structured `terra verify --json` and explicit-key `sign-index` in
  ECO-02. Do not parse the current human-oriented verify output or reuse
  publisher signing defaults for registry signing.
- HTTPS allowlist has no redirects; the real origin is deliberately absent
  until owner setup. Phones cannot use sync to bypass REL-03. Empty official
  keys remain the release default until both REL-01 and REL-03 gates close.

## 6. Decisions

Settled for implementation on 2026-11-11: format 1, separate compiled trust,
raw-byte signing, retained sequence/hash, seven-day maximum validity, static
exact-origin hosting, manual install/update review, local cache/preferences,
no publisher auto-trust, no Android registry operations, and release gates
before official activation. Payments, accounts, ratings, hosted services,
threshold metadata and publisher-key migration remain out of scope.

Review: pending user review (approved to proceed)
