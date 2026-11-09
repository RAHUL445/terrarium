# Third-party source notices

The root Apache-2.0 license applies to original Terrarium code, not a
relicensing of third-party materials.

- `vendor/loro-internal/`: Loro 1.17.1 source, MIT. Its MIT license and existing
  copyright notices remain in `vendor/loro-internal/LICENSE` and source files.
  The host modifies the collection dependency and version-vector adapter;
  `vendor/loro-internal/PATCH.md` records provenance and exact changes.
- `vendor/loro-internal/src/diff/`: retains its separately supplied Apache-2.0
  license at `src/diff/LICENSE` and upstream source notices.
- Cargo dependencies, downloaded tools and separately installed speech/AI models
  retain their own licenses. Cargo.lock fixes versions/checksums; it does not
  grant redistribution rights or constitute a complete binary license report.

Keep these notices and all embedded upstream notices when redistributing source.
Before distributing compiled apps or bundled model files, produce the complete
artifact-specific dependency/license inventory and include required license texts
and notices. That binary-distribution work is tracked in `docs/roadmap.md`.
