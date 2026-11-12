# Home-screen widgets

Status: proposed design, OSI-04. Written 2026-11-11.
Human reviewer: pending. Review date: pending; this is not a reviewed design yet.

## 1. Goal and non-goals

Show a few non-private plugin records on the Android home screen. Recommend
Kotlin `AppWidgetProvider` with fixed `RemoteViews` layouts first. Recommend
**later** for macOS desktop / Notification Center widgets: the current bundle
has no widget extension or shared-container signing arrangement.

The host produces plain snapshots. Native widget code reads those snapshots;
it never opens plugin databases, runs Wasm, or loads `ui/renderer.js` or a
WebView. Version 1 supports only tap-to-open, including row taps. No completion
buttons, editing, background plugin actions, arbitrary layouts, or networking.
iOS and reuse of the in-app page grid are outside this proposal.

Source baseline: [architecture](../develop/architecture.md#declarative-ui),
[plugin widgets](../develop/plugin-authoring.md#pages-and-widget-grid),
`crates/core/src/store.rs`, `crates/runtime/src/registry.rs`, and the reminder
query consumer in `crates/host/src/jobs.rs`. Queries execute SQL through
`PluginHost::query_in` in `registry.rs`, not in `runtime/src/host.rs`.
The brief's `links-and-search.md` is absent; the implemented OSI-01 contract
is documented in plugin authoring and `apps/tauri/src-tauri/src/links.rs`.
All new files and behavior named below are proposals, not existing support.

## 2. Manifest declaration

Keep OS widgets distinct from the existing `widgets` template declarations.
Example fragment for a plugin with an eligible declared `widget_today` query:

```json
{
  "os_widgets": [{
    "id": "today",
    "title": "Today",
    "query": "widget_today",
    "fields": {"title": "title", "subtitle": "due", "tags": "tags"},
    "open": {"page": "main", "item": "id"}
  }]
}
```

`fields` values and `open.item` are exact result-column names, not expressions
or template interpolation. `title`, `tags`, `open.page`, and `open.item` are
required; `subtitle` is optional. The query returns individual source records,
including each record's full tags and stable ID. No aggregates, joined private
content, note bodies, media, or URLs supplied by a plugin. Host text layouts
are fixed. Version 1 derives an optional “N shown” count after filtering and
truncation; a plugin-supplied aggregate `count` field is deliberately deferred
because the host cannot remove private contributions from an aggregate.

Future validation belongs in `crates/runtime/src/manifest.rs`:

- Add `os_widgets` with `#[serde(default)]`; nested structs also use
  `deny_unknown_fields`. Old manifests load without any declarations.
- At most eight declarations per plugin; IDs unique and using the existing
  target-ID grammar; titles nonempty and at most 80 characters.
- Resolve `query` only against that plugin's declared queries. Parameters may
  only be a subset of `CONTEXT_PARAMS`: `today`, `weekday`, `now_ms`,
  `utc_offset_ms`, `tz`. Like `badge`, no user-entered query arguments;
  widget code cannot supply SQL or an alternate plugin/space.
- Resolve `open.page` against declared pages; `open.item` must map to a stable
  ID. Validate row types and IDs at snapshot generation, since query columns
  are not statically typed. Missing tags or malformed rows fail closed.

The publisher builds `terrarium://open/<plugin>/<page>?item=<id>&space=<id>`
using OSI-01's validation and length rules. The host supplies plugin and space
from the saved placement, never from row contents. Header taps omit `item`.
The existing app link queue in `apps/tauri/src-tauri/src/links.rs` and navigation
in `ui/app.js` resolve the target again; removed targets show an unavailable
message rather than silently opening another space.

Upgrade policy: optional does not mean old hosts accept new fields. Older
hosts reject `os_widgets` under `deny_unknown_fields`, and the plugin set
syncs. Ship host support to every participating device before publishing a
plugin manifest containing the field. Until then keep the old plugin bundle;
do not silently strip declarations or bypass signature checks. Gate future
plugin distribution on minimum host support, or require manual all-device
upgrade if no enforceable version gate exists. No WIT or database migration
is needed. Snapshot format has a separate `version: 1`; unknown versions render
an unavailable placeholder and require a host update, not a best-effort parse.

## 3. Data path

Propose `crates/host/src/os_widgets.rs` as the single snapshot publisher,
sharing `Plugins::with_host` from `crates/host/src/plugins.rs`. It calls
`query_in(space, plugin, query, context)` in `crates/runtime/src/registry.rs`.
This is the same read-only path the command registry exposes as
`<plugin>.<query>` in `crates/runtime/src/commands.rs`; no external CLI token
or new public service is needed. Hidden queries (`cli: false`) remain usable
internally. All reads stay within the selected plugin's per-space store.

Propose a device-local placement registry owned by that publisher: random
placement ID, OS instance ID, plugin ID, declaration ID, space ID, and consent
generation. Consent and placements are not synced. Each active
`(space, plugin, declaration)` has one JSON snapshot shared by its placements.
Paths use host-generated opaque keys, not raw names or IDs from callers.

1. The native configuration flow asks the app host to list eligible
   declarations and open spaces, then saves a placement after consent.
   Tauri wiring belongs in proposed `apps/tauri/src-tauri/src/os_widgets.rs`.
2. On startup, after plugin/space reconciliation, the publisher builds all
   opted-in snapshots. Changes from actions, sync and plugin reconciliation
   invalidate them. Hook `data-changed`, `spaces-changed`, `plugins-changed`
   at host/native event sources, including `apps/tauri/src-tauri/src/shell.rs`
   and `sync.rs`; do not depend on JavaScript listeners or a visible window.
3. Debounce ordinary data changes for one second. Extend the jobs tick in
   `crates/host/src/jobs.rs` to refresh at most every 30 minutes while the host
   runs, and at local midnight / time-zone changes for context queries.
   Revocation and removal bypass debouncing. Headless integration belongs in
   `apps/tauri/src-tauri/src/headless.rs`; it uses the same publisher.
4. Under the host lock, query, filter private records, validate and project
   fields, then publish only if the placement's consent generation still
   matches. Removal invalidates that generation first, so an in-flight query
   cannot recreate a removed snapshot. Serialize publication with removal.
5. Write atomically using `crates/host/src/private.rs`'s owner-only temporary
   file / rename approach. Android destination is
   `Context.filesDir/os-widgets/`; the provider uses the same app UID and
   default process, without a content provider or external-storage export.
   Later macOS destination is `os-widgets/` under the App Group URL obtained
   by native code, not a guessed `~/Library/Group Containers` path.
6. After rename, the proposed Tauri native bridge asks Android's provider to
   call `AppWidgetManager.updateAppWidget` for affected instances. Later the
   macOS bridge requests `WidgetCenter.reloadTimelines(ofKind:)`. Native
   readers check version, size, identity and consent generation before use.

Snapshot envelope: `version`, opaque key, plugin/declaration/space IDs,
consent generation, `generated_at_ms`, `expires_at_ms`, `status`, and projected
`rows` containing only title, optional subtitle and host-built open URL.
Do not include raw query results, tags, SQL, credentials, or error details.
Use limits of ten active placements per device, five visible rows per
snapshot, 120 characters per title, 160 per subtitle, 2,048 bytes per link,
and 32 KiB UTF-8 JSON per snapshot, with a 320 KiB directory budget.
Stop adding rows before the byte cap; do not write oversized JSON. Native
readers impose the same byte cap before allocation. Existing query execution
still has its 10,000-row / 4 MB bound in `crates/core/src/store.rs`; eligible
queries should select a small ordered result. Never report a truncated count
as the total number of records.

Display “Updated …” from the generation timestamp. A successful empty result
shows “Nothing to show.” Query, validation or publication failure invalidates
the old content and requests an unavailable placeholder; do not retain a
previous row that may since have become private. Set expiry to 24 hours after
generation; expired, missing or corrupt snapshots show “Open Terrarium to
update.” A backward clock jump also makes a snapshot unavailable.

If Android kills the app, the provider can read its last snapshot without
initializing Rust/Tauri. A unique periodic `WorkManager` worker, proposed in
`gen/android/app/src/main/java/io/github/rahul445/terrarium/widgets/`, may
re-render cached data and expire it; it does not query SQLite or boot another
host. Fresh data while Rust is absent is deferred. WorkManager and the system
timer are not substitutes for a live query publisher. Force-stop can prevent
background work until the user opens the app. Reconcile OS instance IDs on
restart, including restored IDs; restored placements require fresh consent.

On desktop, existing `crates/host/src/jobs.rs` runs only inside the app or
`terrarium serve`; `headless.rs` retains the existing exclusive data-folder
lock. There is no always-running system job. If neither process runs,
WidgetKit later reads cached data and schedules an expired placeholder;
it cannot make plugin data fresh. Neither platform guarantees immediate
redraw or erasure of OS-cached content when Terrarium is stopped.

## 4. Android

Use `AppWidgetProvider` and `RemoteViews`: the existing hand-maintained Gradle
project has no Compose setup, and five plain rows need no Compose dependency.
Glance offers Kotlin composition but requires Compose configuration and has
its own widget UI elements; it does not make the app's templates reusable.
See [Jetpack Glance](https://developer.android.com/develop/ui/compose/glance).
Revisit Glance if several native layouts become necessary.

Add proposed `widgets/TerrariumWidgetProvider.kt`,
`widgets/WidgetConfigurationActivity.kt`, layout XML and provider metadata
under `apps/tauri/src-tauri/gen/android/app/src/main/`. Edit that project
directly; never regenerate it. Supply compact and expanded fixed text layouts,
light/dark colors, accessible labels and text scaling. Use static synthetic
picker previews; no snapshot contents in previews.

The configuration activity initializes its result as cancelled, obtains the
OS widget ID, asks the running app for plugin/widget and space choices, shows
a content preview and the exposure notice, then saves only on confirmation.
If the host is unavailable, launch the app's setup flow and require successful
configuration before returning success. Validate the OS ID belongs to this
provider; never accept arbitrary snapshot paths. Publish the initial view
explicitly before returning the widget ID. Support reconfiguration and deletion.
See [configuration lifecycle](https://developer.android.com/develop/ui/views/appwidgets/configuration).

Use explicit immutable activity `PendingIntent`s to `MainActivity.kt`, with
distinct identities per placement/row and the host-built OSI-01 URI. A tap
starts the normal app and passes through `links.rs`; it executes no action.
Provider callbacks do bounded file reads and native rendering only. Long
work belongs in the worker, not the broadcast receiver.

Set `updatePeriodMillis` to 0. Push after app changes; use one best-effort
30-minute WorkManager job only while placements exist, for cached rendering
and expiry. Android's system update timer has a 30-minute floor; it does not
limit app-initiated pushes. WorkManager remains subject to power restrictions,
so none of these intervals promise freshness. See Android's
[update guidance](https://developer.android.com/develop/ui/views/appwidgets/advanced).
Test process death, reboot, force-stop, resizing, multiple spaces, missing
plugins, cancellation and stale snapshots on physical devices before release.

## 5. macOS

Decision: **later**. Feasible in principle outside the App Store, but not
supported by the current bundle. `apps/tauri/src-tauri/tauri.conf.json` enables
hardened runtime; `Entitlements.plist` currently grants only microphone access.
Neither defines a WidgetKit target or App Group. Hardened runtime alone does
not grant extension discovery or container sharing.

Propose a separate native build under `apps/macos-widgets/`: an Xcode Widget
Extension target with its own bundle identifier, Info.plist and sandbox
entitlements. A Swift package can hold snapshot decoding and SwiftUI views,
but cannot replace the extension target and packaging. Build with Xcode
outside Tauri; embed the resulting `.appex` into the final app's
`Contents/PlugIns` before final signing. See Apple's
[extension setup](https://developer.apple.com/documentation/widgetkit/creating-a-widget-extension).

Give the containing app and extension the same App Group entitlement, using
a team-scoped group and native `FileManager.containerURL` lookup. Keep plugin
databases and keys outside it. App Groups can connect sandboxed extensions
to nonsandboxed macOS apps. See
[App Groups](https://developer.apple.com/documentation/xcode/configuring-app-groups).

For independent distribution, replace ad hoc signing with a consistent
Developer ID Application identity. Sign nested code before the containing
app, preserve appropriate sandbox / hardened-runtime entitlements, then
notarize and staple the distributable. Team-prefixed macOS App Groups do not
need profiles merely for that group; restricted entitlements may need their
own distribution profiles. Verify target-specific requirements rather than
assuming one host profile covers every executable. See
[distribution signing](https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac),
[container authorization](https://developer.apple.com/documentation/xcode/accessing-app-group-containers)
and [notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).
This is a release-pipeline change; clean-machine installation, widget gallery
discovery and shared-file access must pass before claiming support.

The extension's timeline provider reads snapshots only, producing a current
entry and a future unavailable entry at expiry. The containing app's proposed
`os_widgets.rs` bridge requests reloads after publication. Reload requests
are budgeted and timelines are scheduled by the system; a requested date is
not a real-time guarantee. See
[WidgetKit freshness](https://developer.apple.com/documentation/widgetkit/keeping-a-widget-up-to-date).
Use SwiftUI links / widget URLs for tap-to-open and privacy-sensitive views;
no App Intent mutations in v1. Choose supported macOS versions and prove
row-link behavior for each family in the later feasibility task.

## 6. Privacy and security

Every OS instance requires explicit, device-local opt-in for that plugin
widget and space, even if an in-app widget is already placed. Explain before
confirmation: “Widget text is visible outside Terrarium, including on shared
screens. Copies are stored on this device.” Offer removing it in the app.

Snapshots are plaintext. Local SQLite is also plaintext; only sync batches
are encrypted. App-private storage, owner-only files and App Group membership
limit access but do not encrypt widget contents or prevent screenshots,
launcher caches, backup copies or shoulder surfing. Keep snapshots and
placement consent excluded from transfer/backup: the existing Android rules
in `gen/android/app/src/main/res/xml/data_extraction_rules.xml` already exclude
the whole app files domain. Verify this remains true when adding widgets;
exclude them from later macOS exports too. Do not sync or log their contents
or links.

Treat home-screen, lock-screen, screen-sharing and Notification Center
visibility as exposure. Request home-screen-only placement on Android and
no lock-screen families in v1, but do not promise every launcher/OS respects
that boundary. Future SwiftUI privacy redaction is an additional defense,
not permission to export private text.

The publisher filters `#private` before projection, count calculation or file
creation, matching the case-insensitive tag-token rule in
`crates/runtime/src/registry.rs`'s Today attachment. Queries must return the
full tags field; missing, null or wrongly typed tags reject the snapshot.
An empty string means no tags. Content marked private in supported title/tag
syntax must also be excluded. No aggregate queries in v1, because hidden
records must not affect a count or subtitle. Query authors must preserve
source privacy metadata; the host cannot infer omitted or falsified tags
from arbitrary SQL. Review eligible query declarations and synthetic private
record tests before shipping a plugin's opt-in surface.

Removing a placement invalidates consent, cancels its pending work/intents
and requests a blank native view. Delete its snapshot when the last instance
using that key is removed; another opted-in instance keeps its own access.
Space removal, plugin removal, declaration disappearance, or explicit revoke
invalidates all affected placements and removes their snapshots immediately
through proposed `crates/host/src/os_widgets.rs`. Native deletion callbacks
must perform cleanup even when Rust is absent, using the saved registry;
the publisher reconciles stale files and OS IDs at next startup. Serialize
native cleanup and Rust publication with the same placement lock/generation.
Do not resurrect consent on restore or plugin reinstall. Already captured
screenshots and OS caches cannot be recalled; request replacement promptly
without claiming secure erasure or immediate redraw.

## 7. Effort and phasing

These are proposed future task briefs, not authorization to implement code.
Each should follow `tasks/CONVENTIONS.md`, include acceptance criteria and
synthetic tests, and stop at its stated boundary. Android comes first.

1. **OSW-01: Snapshot contract and privacy** — M, strong; host. Finalize
   manifest validation, old-manifest compatibility, minimum-host rollout,
   row privacy rules and snapshot decoding. Depends on human design review.
2. **OSW-02: Host snapshot publisher** — M, strong; host. Add the publisher,
   local placements, event/jobs hooks, atomic bounded publication, consent
   generations and removal. Test isolation, midnight, error/expiry handling
   and removal races. Depends on OSW-01.
3. **OSW-03: Android widget and setup** — M, strong; host Android project.
   Add provider, native bridge, configuration, two layouts, PendingIntents,
   cached worker and backup exclusions. Prove cold-start taps, instance
   identity and process-death behavior. Depends on OSW-02 and OSI-01.
4. **OSW-04: First plugin declaration and device checks** — S, strong;
   plugins plus host verification. Add one reviewed Todo query with private
   fixtures after all-device host upgrade. Verify physical-device launcher,
   reboot, revoke and stale-data behavior. Depends on OSW-03.
5. **OSW-05: macOS signing and packaging feasibility** — M, strong; host.
   Prove a synthetic-data extension, App Group, independent Developer ID
   distribution and clean-machine discovery. No user data; stop with evidence
   if signing/bundling is unavailable. Schedule after Android validation.
6. **OSW-06: macOS snapshot widget** — M, strong; host. Add Swift decoder,
   timelines, consent configuration, links, cleanup and signed release checks
   only if OSW-05 succeeds. Depends on OSW-02 and OSW-05.

## 8. Open questions

- Who reviews this design, and on what date? Record actual approval above
  before calling OSI-04 complete.
- Is showing cached Android content until the app runs again acceptable,
  or should a separate lifecycle-safe headless query service be designed?
- Are five rows, ten placements and 24-hour expiry appropriate? Expiry is a
  display policy, not a guarantee that the OS erases a cached frame on time.
- Should first release narrow eligibility to reviewed Todo queries until a
  stronger generic privacy/provenance contract exists?
- Will distribution enforce minimum host versions, or use a manual
  all-device upgrade gate before syncing new manifests?
- Is Developer ID signing available, and which macOS versions / widget
  families must the later feasibility work support?

Platform references checked 2026-11-11. Runtime/device behavior above remains
proposed and untested; this task changes documentation only.
