# Releasing

Terrarium is published as an experimental source preview. Releases are tags on
`main`; no signed binaries are distributed yet. See [the roadmap](roadmap.md)
for the gates that must close before a binary or store release.

## Cut a source prerelease

1. Make sure `main` is green. The `strict audit (informational, no ignores)`
   job in Security checks does not block CI; copy every finding it reports
   into the release notes. Known findings are listed, not waived.
2. Run the `Pinned plugin integration` workflow with the plugins repository
   (`RAHUL445/terrarium-plugins`) and the **full commit SHA recorded in
   `plugins.lock.json`**, never a branch name.
3. Update [CHANGELOG.md](../CHANGELOG.md) and the workspace version in
   `Cargo.toml`.
4. Tag and push, for example `v0.1.0-alpha.1`, and create a GitHub prerelease
   whose notes link the changelog and known security limits.

## Rebuild from the two repositories

Follow the README with both checkouts. Match the plugins commit to
`plugins.lock.json`, build `terra`, build a plugin release with the plugins
repository's release builder, then import it. Wasm digests can differ with
source paths or toolchains; review regenerated files before updating the lock.
A digest mismatch must never be bypassed silently.

Imported `bundled-plugins/`, build outputs, user databases, keys and signing
files are not source inputs and are never committed.

## Event-enabled packages

Upgrade the host on **every paired device** before publishing or refreshing a
synced set containing To-do 0.5.0 or Habits 0.3.0. The API stays
`terrarium:plugin@0.1.0`, but old hosts reject the `events` manifest field and
cannot link the new WIT import. New hosts still load pre-event packages.
Review expanded event-sharing capabilities on install/update, then review each
automation again on its selected execution device and explicitly start there.
Rollback to an old host requires compatible packages or another host upgrade;
never remove synced tables/columns or silently strip event declarations.
See [plugin authoring](develop/plugin-authoring.md#events).

## Binary builds

Do not label debug-key Android APKs or unsigned, non-notarized macOS builds as
production releases. Binary prerequisites (SBOM, signing, notarization, store
review) are tracked as `REL-*` items in the roadmap.
