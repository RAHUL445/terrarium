# Dependency advisory review — 2026-11-05

Historical baseline. The [Loro backport](loro-backport.md) supersedes the im/sized-chunks findings and collection source pins below. The strict audit still fails on glib; no waiver exists.

Status: scoped exposure assessment completed; **no advisory waived, no release approval**. The full-lockfile strict RustSec gate still fails. No dependency or CRDT format changed, no registry source was edited, and no live data or credentials were accessed.

## Findings and scope

### RUSTSEC-2024-0429 — glib 0.18.5

The flaw affects `VariantStrIter` iterator methods; RustSec lists >=0.20.0 as patched. Official crates.io metadata reports 0.18.5 as the latest 0.18 release, so there is no compatible patch update on that line. Replacing it with 0.22 is not a compatible lock-only change for GTK3 consumers.

Both inspected normal/build dependency graphs (`aarch64-apple-darwin`, `aarch64-linux-android`) contain **no glib**. The lockfile contains Linux GTK/WebKit dependencies because the Tauri workspace supports multiple platforms. Current Tauri's GTK/WebKit dependencies are guarded by Linux/BSD OS configuration; this finding must be re-evaluated before adding a Linux release. This review does not certify other architectures, feature sets, dev tools, plugins, or final artifact contents.

### RUSTSEC-2023-0126 — im 15.1.0

RustSec names an aliasing violation in `OrdSet` insertion and lists no patched version. Official metadata still reports im 15.1.0 and Loro/loro-internal 1.17.1 as latest stable releases. Updating Loro within the current released line does not remove this dependency.

`Cargo.lock` shows exactly one direct im consumer: loro-internal. Review of Loro, loro-internal and loro-common Rust source found production im usage in `loro-internal/src/version.rs`: `ImVersionVector` wraps `im::HashMap<PeerID, Counter, FxBuildHasher>`. `PeerID` is u64 and `Counter` is i32, defined by loro-common. Its iterator signature uses the same HashMap. The other executable import is `im::HashSet` in `src/dag/test.rs`; an oplog mention is a comment. No OrdSet call was observed in these reviewed paths. This is source-based reachability evidence about this advisory, not proof that all im unsafe code is sound.

### RUSTSEC-2026-0255 — sized-chunks 0.6.5

The reported panic-safety flaw names `Chunk::{clear,drop_left,drop_right}`, `RingBuffer::{clear,drop_left,drop_right}` and `InlineArray::clear`, with no patched versions, including latest 0.7.0. Its documented precondition is an element destructor that panics, leaving stale container metadata and enabling repeated drops.

The only direct sized-chunks consumer is im. The reviewed HashMap implementation (`im/src/hash/map.rs`) delegates to `nodes::hamt`, whose storage uses `sized_chunks::sparse_chunk::SparseChunk`, not the three types named by this advisory. HashMap clearing replaces its root; its values here are primitive integer pairs. HashSet's HAMT path is the same. No use of the advisory's affected methods was observed in this reviewed path. SparseChunk itself contains unsafe code: this assessment neither certifies it generally nor rules out other or future flaws. Other im collections (including Vector and ordered collections) must not be assumed covered by this reasoning.

## Why no dependency replacement or advisory ignore

A blind Loro upgrade has no released fix to select. Replacing its persistent collection dependency with imbl would require changing the upstream Loro implementation and reviewing API, serialization, performance and CRDT compatibility. A private fork of im/sized-chunks or GTK backport creates a separate security maintenance obligation. Neither was introduced to make the audit pass. No `--ignore` option, advisory allowlist or filtering of cargo-audit findings was added.

For source-level remediation, prefer a maintained upstream Loro collection migration or a deliberately reviewed backport with ownership and regression proof. Any experiment must first preserve old snapshot/update fixtures, exercise old/new mixed-device sync in both directions, rollback/reopen, concurrent Unicode text, corruption/failure behavior and Android memory/latency. Existing local records and encrypted backups must not be rewritten or deleted as part of a trial. Keep the strict publication gate until that remediation or an explicitly approved risk acceptance exists.

## Repeatable drift guard

`scripts/dependency_exposure.py` checks:

- Reviewed package versions and registry checksums against Cargo.lock; duplicate versions require re-review.
- Exclusive im/sized-chunks consumers, so a new direct consumer cannot silently inherit this assessment.
- Digests of every Rust source file and Cargo.toml for the reviewed Loro/im/sized-chunks packages, so code changes, additions and removals invalidate the source review.
- Active normal/build target graphs and reviewed feature sets, rejecting active glib or a changed reviewed dependency path/features.

Pins and graph features are in `dependency-exposure.json`. The checker is offline and uses already-cached Cargo sources. It does not fetch, update dependencies, bypass the scanner or validate a signed binary. Missing sources/graphs or changed pins fail rather than producing an exposure approval. A local native process could modify both evidence and checker; this is a review/CI drift guard, not a boundary against compromised developer accounts.

Run from the host checkout, after fetching/building locked dependencies for the target:

```sh
python3 scripts/dependency_exposure.py
python3 -m unittest discover -s scripts/tests -v
cargo audit --deny unsound --deny yanked
```

The first command covers the two explicitly reviewed arm64 targets; `--target` can select one. The audit must run separately and still exits nonzero. The pinned cross-repository integration workflow checks the macOS scope after its build/test steps; ordinary Python CI discovery includes the guard's regression tests. Hosted CI execution remains pending.

Local results: both target scope checks passed. All 22 Python tests passed, including seven regressions for changed versions/checksums, duplicate packages, new consumers, active glib, changed features, missing/changed/added sources. The strict audit was repeated using the previously fetched advisory database: still three unsoundness and eight unmaintained-package findings; no yanked package findings. No Rust code or lockfile changed, so no new app build/device run was performed for this documentation/tooling milestone.

## Primary sources

- https://rustsec.org/advisories/RUSTSEC-2024-0429.html
- https://rustsec.org/advisories/RUSTSEC-2023-0126.html
- https://rustsec.org/advisories/RUSTSEC-2026-0255.html
- https://crates.io/api/v1/crates/loro
- https://crates.io/api/v1/crates/glib
- https://crates.io/api/v1/crates/im
- https://crates.io/api/v1/crates/sized-chunks
- https://docs.rs/crate/loro-internal/1.17.1/source/src/version.rs
- https://docs.rs/crate/loro-common/1.17.0/source/src/lib.rs
- https://docs.rs/crate/im/15.1.0/source/src/hash/map.rs
- https://docs.rs/crate/im/15.1.0/source/src/nodes/hamt.rs
- https://docs.rs/crate/sized-chunks/0.6.5/source/src/sparse_chunk/mod.rs
