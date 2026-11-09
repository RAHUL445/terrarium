# Loro collection backport and remaining advisories

Status: partial remediation, not release approval.

## Decision and alternatives

Loro remains 1.17.1. Its released internal crate still uses archived `im`;
there is no compatible released fix for the im/sized-chunks advisories.
Replacing the CRDT would impose an unnecessary format/recovery migration.
Ignoring advisories would hide the problem. Instead the host owns a narrow,
source-pinned backport of `loro-internal` that uses maintained imbl 7.0.2.
This introduces a maintenance obligation and needs independent review before
production deployment. No complete soundness claim is made for imbl.

Upstream registry source is preserved under `vendor/loro-internal`, with
`PATCH.md` describing the two manifest edits, the version-vector type/iterator
adaptation, provenance/checksum and license. Wire encoding/merge logic is not
edited. The main lockfile has no im/sized-chunks/bitmaps dependency. The original
upstream manifests/lock and isolated legacy-reader test lock are evidence for
upstream/compatibility testing, not application dependency inputs.

## Compatibility and rollback proof

Before installing the backport, unmodified registry Loro generated the synthetic
fixtures under `crates/core/tests/fixtures/loro-1.17.1` with fixed peer IDs and
Unicode text. No real records are included. Three automated tests cover old
snapshot/update import, duplicate update idempotence, concurrent offline Unicode
edits, reopened snapshots and malformed import preserving existing text.

`python3 scripts/check_crdt_compat.py` additionally constructs an isolated
unpatched registry-Loro program using its own locked dependencies. It reads the
new snapshot/update and converges with the patched writer in the reverse import
order. The same existing format is readable by the old code on this scoped
fixture; this is not universal CRDT-format or full-app downgrade proof.

Rollback: remove the workspace patch and restore the preceding Cargo.lock, then
rebuild. No data conversion or automatic rewrite accompanies this change.
Exercise application backup/reopen and physical paired-device sync before
shipping. The compatibility tool defaults to cached dependencies and temporary
synthetic data (CI explicitly uses --fetch-legacy for its pinned old reader); it never opens normal application storage.

The drift guard pins both upstream and vendored Rust/manifest source, reviewed
features and the exact imbl dependency chain. Reintroduction of im/sized-chunks,
replacement of the local backport by a registry copy, or active glib on a reviewed
target fails the guard. This remains separate from cargo-audit.

## Audit result and remaining blocker

The strict audit was refreshed against RustSec on 2026-11-07. Unsoundness findings
fell from three to **one**: `glib` 0.18.5 / RUSTSEC-2024-0429. Maintenance warnings
fell from eight to **five**: atomic-polyfill, proc-macro-error, zbus_names,
zvariant and zvariant_derive. There were zero entries in the scanner's separate
vulnerability list; that does not negate the unsoundness finding. The audit
still exits nonzero. No ignores, altered advisory database or risk waiver added.

glib remains absent from the reviewed arm64 macOS and Android active graphs.
It belongs to Tauri's Linux GTK dependency chain. Moving to glib >=0.20 is not
a compatible lock-only update. A correct backport to 0.18 would still match the
advisory's version range, so it cannot honestly be described as a clean strict
scan merely by changing local source. Resolve through a supported upstream stack
or an explicitly reviewed patch/risk policy; Linux release remains unsupported
by this assessment. Five maintenance warnings also need owned upstream paths.

Primary references:
- https://docs.rs/crate/imbl/7.0.2
- https://docs.rs/crate/loro-internal/1.17.1/source/Cargo.toml.orig
- https://rustsec.org/advisories/RUSTSEC-2023-0126.html
- https://rustsec.org/advisories/RUSTSEC-2026-0255.html
- https://rustsec.org/advisories/RUSTSEC-2024-0429.html

## Verification

Run the cross-repository checks in the README with the integration feature.
Both arm64 exposure guards run in CI via `scripts/dependency_exposure.py`.

## Remaining platform findings

No new vendor fork or dependency version change was
introduced merely to remove inactive-platform packages from a cross-platform
lockfile. The full strict audit and its failure status remain intact.

Inspected `cargo tree --locked --offline --target TARGET --edges normal,build
--prefix none --format '{p}|{f}'` on 2026-11-07:

- aarch64-apple-darwin: 415 distinct package names; none of the six remaining
  advisory package names present.
- aarch64-linux-android: 420 distinct package names; none of the six remaining
  advisory package names present.

This includes normal/build dependencies of the workspace's selected/default
features, not only runtime imports. It does not cover dev-only dependencies,
alternate feature sets, Intel macOS, other Android architectures, Linux releases,
final signed binaries or future vulnerabilities. Counts refer to distinct names,
not name/version pairs, and are a dated snapshot rather than a pinned total.

Remaining full-lock findings and the owning upgrade paths:

| Package/finding | Locked dependency path | Remediation ownership |
| --- | --- | --- |
| glib / RUSTSEC-2024-0429 | Tauri GTK/WebKit stack -> glib 0.18.5 | Needs a supported GTK/WebKit/Tauri stack with patched glib, or separately reviewed source backport and audit policy. A blind 0.20 substitution breaks compatibility. |
| atomic-polyfill / RUSTSEC-2023-0089 | heapless 0.7.17, via postcard and Loro collection dependencies | Embedded-target paths need an upstream compatible heapless/atomic migration. No use was found in either reviewed graph. |
| proc-macro-error / RUSTSEC-2024-0370 | glib-macros / gtk3-macros | Owned by the GTK macro stack; update through that stack. |
| zbus_names / RUSTSEC-2026-0371 | zbus / zbus_macros 5.19.0 | Parent notification/opener integrations need a compatible zbus 6 migration; names moved into zbus. |
| zvariant / RUSTSEC-2026-0370 | zbus / zbus_macros / zbus_names | Parent integrations need a compatible zbus 6 migration; wire types moved into zbus. |
| zvariant_derive / RUSTSEC-2026-0372 | zvariant | Follow the same parent integration migration; derive support moved into zbus. |

The RustSec zbus advisories describe a move into zbus 6, not patched standalone
versions. Replacing transitive versions in Cargo.lock cannot implement that API
migration. Sources:
- https://rustsec.org/advisories/RUSTSEC-2026-0370.html
- https://rustsec.org/advisories/RUSTSEC-2026-0371.html
- https://rustsec.org/advisories/RUSTSEC-2026-0372.html
- https://github.com/gtk-rs/gtk-rs-core/pull/1343

The exposure guard now rejects **all six** packages if they appear in either
reviewed graph, including build dependencies. A regression was demonstrated
first: the preceding guard accepted five of the six packages. The new negative
subtests fail on the old guard and pass after the patch. All 24 Python tests and
both actual target guard runs pass. No Rust/app code changed, so the earlier
runtime/build/compatibility evidence was not rerun. CI now fetches locked sources
for both targets and invokes this guard; hosted execution has not been verified.

**This is not globally closed.** The full-lock strict audit remains one
unsoundness finding plus five maintenance warnings. The guard adds target
exposure protection and upgrade ownership, not an advisory waiver, a clean audit
claim or permission to publish. Source backports would also need independent
review and relevant platform tests; do not add untested GTK forks to a Mac and
Android product solely to turn the scanner green.
