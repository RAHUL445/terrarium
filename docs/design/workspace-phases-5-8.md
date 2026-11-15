# Workspace phases 5–8

Continuation of phases 1–4 on `feature/composable-views-phases-1-4` in host and plugins. Research source: the composable workspace roadmap (N1/N2, P1, M5 and M6). Existing untracked UI-consistency research is preserved.

| Phase | Deliverable | Acceptance |
| --- | --- | --- |
| 5 | Nested folders, recoverable page/folder Trash, unified device-local page/app pins | Same-space and cycle validation; deterministic recovered projection; stable IDs through moves/restores; explicit legacy app pin migration, reorder and unpin without deleting targets |
| 6 | Flow block editing alongside the existing grid | Separate block order preserves grid coordinates; keyboard moves, duplicate/remove/undo; draft Apply/Cancel behavior; layouts survive reload |
| 7 | Shared-view widgets for Notes, Bookmarks and Feeds | Reuse host view controls and styles; preserve existing specialized tools and old placements; opt-in compatible migration; query/action checks |
| 8 | Integration and release verification | Node/Rust/SQL checks, native compile where available, scratch browser flow, migration and recovery evidence; explicitly report untested device/fleet behavior |

## Ownership and constraints

Host owns navigation, folder/page metadata, pins and layout. Plugins own their source records, domain actions and declared view capabilities. No data copies or cross-plugin SQL joins. New widgets are optional, old widget IDs remain valid, and existing page placements are not rewritten automatically.

Folders and pages archive into recoverable Trash while existing terminal-delete storage semantics stay intact. Root is an explicit move destination. Invalid or cyclic parent edges project to a recovered location rather than mutating records during reads. Existing core schema-version buffering protects older readers from applying unknown-schema changes, but an older client cannot display the new workspace correctly; upgrade all participating clients before rollout.

Pins stay device-local and space-scoped. Explicit existing app choices migrate once; unconfigured defaults and existing pages are not automatically pinned. Unavailable pins remain visible and removable. Page browser search includes names and folder paths.

Flow order is independent of grid coordinates. A mode change is a page draft until saved. Shared-view duplication retains an explicit reference; use the view editor's independent-copy action to detach it.

No merge, publish, installation or production-data migration is authorized by this work. Native/Android installation and mixed-version device rollout require a separate release step.

## Results

Implementation complete on the feature branches; no merge or install performed.

- Host phase 5 foundation: `928aad7` (schema, lifecycle commands, folder browser, local pin storage).
- Host phase 6 integration: `bdb717b` (folder/pin navigation and flow editor).
- Phase 7 adds optional Notes/Bookmarks/Feeds widgets and object-filter support (`03891ba` in plugins, `e4e875b` in host).
- Phase 8 records integration evidence and release limits below.

### Verified

- Runtime: 198 library tests and 8 page integration tests pass. Covers v1/v4/v5 migration, stable-ID restore with placements, parent cycles, cross-space rejection, deterministic recovery projection, same-space restore batches, position overflow and active-folder limits. An older schema buffers new-schema changes until upgrade in the local core test.
- UI: full combined host/plugin Node suite passes all 347 tests. Flow tests cover independent order, duplication with vacant grid positions, Cancel restoring changed configs, and correct mode-specific instructions.
- SQL: 45 To-do tests and 17 Notes/Bookmarks/Feeds tests pass, including paging beyond old caps.
- Package validation: `terra check` passes for Notes, Bookmarks and Feeds with existing WASM. No Rust plugin operation changes were needed.
- Final CLI builds offline; macOS Tauri app passes `cargo check --offline --locked -p terrarium-app`.
- Browser, isolated scratch data: create Personal / Reviews nested folders; create and pin Weekly review; save flow order and reload; duplicate/reorder then Cancel; move page into nested folder; Trash and restore page with pin preserved; pin Notes and reorder mixed pins; capture a note and display it in review and the optional Notes view; apply table layout and a project filter through named controls.

### Limits and release notes

The folder service permits multiple nesting levels, bounded by 200 active folders per space rather than a fixed depth. Positions are bounded to 0–1,000,000. `page.folder_move` is the public command for moving a page into a folder; existing `page.move` continues to move widget placements. The service name is `page_move`. Non-subtree removal promotes direct child folders and pages atomically.

The preview uses `/private/tmp/workspace58-preview`; installed data is untouched. Native app installation, Android/device interaction, real multi-device sync and mixed-version fleet rollout were not tested. The native check is a compile check, not a packaged release. Upgrade participating clients together before deploying schema 6. No automatic migration of old widget placements is performed.


### Reproducing the checks

Set `TERRA_PLUGIN_SOURCE` to the plugin checkout's `packages` directory and `TERRA_HOST_SOURCE` to this checkout before `node --test ui/tests/*.test.cjs`. Run `cargo test --offline --locked -p terrarium-runtime --lib --test pages` for the runtime boundary. In plugins, use `python3 -m unittest discover -s packages/<plugin>/tests -p '*query.py'` for each of todo, notes, bookmarks, and feeds (discovery includes review tests without a standalone main).

Browser evidence: `/Users/rahul.pan/.codex/visualizations/2026/11/09/01a1c089-06ce-7681-ac18-20c85cdd8233/workspace-phases-5-8.png`. The scratch preview process was stopped after verification.
