# Composable views: phases 1–4

Branch: `feature/composable-views-phases-1-4` in host and plugins.
Bases: host `794f64e`, plugins `c600592`.

## Scope and acceptance

1. **Composition** — plugin widgets embed the same host view renderer used by pages. Query access stays scoped to the plugin. Refresh, disposal and draft snapshots work. Existing specialized widgets remain available.
2. **Contracts** — declared and validated temporal filters, server sorting, bounded pagination and row actions. Unknown required host features fail closed. Actions retain plugin operations and follow-up behavior. Test data must exceed the old 500/300 limits.
3. **Customization** — one editor for local and shared views; local edits apply only on confirmation, shared edits are explicit, copies become independent, refresh preserves drafts. Filters, fields, density and layout use the same definitions.
4. **Weekly review** — a user-created page combines completed Tasks, created or edited Notes, reflection text and quick capture. A timezone-aware page period supplies half-open timestamp bounds to opted-in views. Views retain independent display settings.

Each phase receives focused validation and a milestone commit. Final validation includes cross-repository Node tests, Rust contract/storage tests and plugin SQL fixtures. No app installation, merge or publication is part of this change.

## Boundaries

- Host owns structure and presentation; plugins own data and actions.
- Saved views are explicit shared definitions. Presets seed independent configuration.
- Pagination uses bounded offsets with deterministic SQL tie-breaking. Refresh resets pagination after edits; concurrent writes do not provide snapshot isolation.
- Required-feature rejection protects older strict manifest parsers, but mixed-version device rollout remains a deployment concern. Do not install new bundles on older hosts.
- Folders, pins, general page redesign and additional plugin migrations belong to later phases.

## Validation and milestones

Completed all four phases on the feature branches.

| Milestone | Host | Plugins |
| --- | --- | --- |
| 1: composition | `8b2f4d0` | `2a2f8ae` |
| 2: query and action contracts | `274466f` | `92e3805` |
| 3: unified customization | `b3a0eba` | Host-owned editor; no plugin change |
| 4: weekly review | Commit containing this report | `dbc4176` |

Verified on 2026-11-13:

- Full UI suite: **330 passed** (`node --test ui/tests/*.test.cjs`, with `TERRA_PLUGIN_SOURCE` pointing at sibling `packages` and `TERRA_HOST_SOURCE` at this checkout).
- Runtime library: **196 passed** (`cargo test --offline --locked -p terrarium-runtime --lib`). Includes page-store v4 migration and view-input validation.
- Plugin SQL fixtures: **45 To-do and 4 Notes passed** (`python3 -m unittest discover -s packages/<plugin>/tests -p '*_query.py'`). Covers more than 500 tasks/300 notes, half-open boundaries and deterministic timestamp ties.
- CLI build: `cargo build --offline --locked -p terra` passed. `terra check` loaded To-do 0.7.0 and Notes 0.4.0 with the new contracts.
- Browser preview used isolated scratch data. Created a weekly review, changed Previous week to This week, saw the matching task and newly created note, and saved a local table layout while Notes remained a list. The native installed app and Android were not modified or tested.
- `git diff --check` passed. The pre-existing untracked `ui-consistency-plan.md` was preserved.

## Implementation limits

The existing specialized task lists retain their 500-row default; reusable host views can page past it. Legacy saved display sorts on the workspace query must use its SQL Sort filter; unsupported display sorts show a repair message. Review queries declare their own server sorts. Calendar and board pages explicitly describe the current batch; they do not imply complete month or lane totals. Page-period changes and placement edits use the existing separate page metadata/layout save calls.

Reflection uses the existing editable Text widget. Optional completion followups open the task detail, where the done note can be entered. No automatic publication, installation, merging or device rollout was performed.
