# Host collection backport

Base: crates.io `loro-internal` 1.17.1, registry archive checksum
`7e533c1688430aa4fcdd933f00861e331c169a4158ee6d27d054e0558892d447`.
Upstream: https://github.com/loro-dev/loro/ . MIT license copied from the
same-version `loro` wrapper because the internal registry package omitted it;
existing third-party notices, including `src/diff/LICENSE`, are retained.

Changes from the registry source:

- Cargo.toml and Cargo.toml.orig: alias `im` to exactly `imbl` 7.0.2 with serde.
- src/version.rs: select imbl's GenericHashMap with the existing FxBuildHasher
  and default shared pointer; return an opaque iterator with the same item type.
- Encoding.md: strip two inherited trailing spaces for repository whitespace checks.
- Add this patch notice and the project MIT license. No encoding, import,
  replay, merge, device IDs or host data migration code changed.

This removes the archived im/sized-chunks chain from the host dependency graph;
it does not replace Loro. A local patch creates an ongoing maintenance duty:
rebase deliberately, review the diff, refresh the source digest, rerun old/new
compatibility, native/Android checks, and the strict audit. Do not bypass
advisories or use the exposed internal crate as a stable host SDK API.

Proof: `python3 scripts/check_crdt_compat.py` from the host root and
`cargo test --locked -p platform-core --test crdt_compat`.
See `docs/security/dependency-remediation-2026-11-07.md` in the host repository.
