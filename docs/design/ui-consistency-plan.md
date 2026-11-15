# Terrarium UI consistency plan

Date: 2026-11-13. Status: proposed; implementation has not started.

## Outcome

Make Terrarium feel like one application across Home, pages, plugins, search,
settings, and item details. Preserve user control over organization, saved views,
layouts, visible fields, density, themes, and navigation. Optimize To-do for
two jobs: capture something quickly and choose/finish the next piece of work.

## Evidence and limits

This plan is based on current source, not a fresh visual audit of the installed
app. Inspect actual screens before declaring specific layout defects resolved.

- `ui/app.css` already provides shared color and font tokens, but spacing,
  dimensions, typography sizes, and radii are mostly literal values. Successive
  shell overrides make the effective design harder to maintain.
- `ui/index.html` defines the workspace bar, plugin header/tabs, page header,
  navigation rails, and page-edit controls. These need a single hierarchy and
  explicit action ownership; removing all secondary navigation would lose context.
- Host sheets, renderer sheets, page configuration, confirmations, and history
  use separate styles. Their purposes differ, but common anatomy should not.
- `ui/view-library.js` already implements temporary Apply, Save changes,
  Save as new, Reset, searchable filter choices, density, and field selection.
  Preserve those semantics rather than replacing working behavior.
- The To-do milestone record says display preferences apply to saved views and
  page references, while capture/focus retains a separate presentation. Details
  use a sheet; a persistent inspector would be new behavior.

## Design direction

A quiet, practical workspace: aligned rows, restrained borders, clear selection,
readable labels, and one prominent action per working context. Retain the system
font and installed themes. Avoid turning every group of controls into a card.

Starting default palette preserves existing tokens: background #f7f7f8,
surface #ffffff, text #18181b, secondary text #52525b, border #e4e4e7,
accent #4338ca. These are defaults, not replacements for user themes. Error,
warning, success, and custom status colors retain distinct semantic roles.

Proposed shared measurements, to validate in prototypes:

| Foundation | Rule |
| --- | --- |
| Type | 12px metadata, 14px body/controls, 16px section title, 22px page title |
| Spacing | 4, 8, 12, 16, 24, 32px; explicit exceptions for dense data |
| Corners | 6px controls, 10px surfaces, 14px dialogs; pills only for chips |
| Controls | 32px compact desktop, 36px comfortable; touch targets at least 44px |
| Icons | One shared family; 16px in controls, 20px for navigation |
| State | Consistent hover, selected, focus, disabled, pending, error, and success |

Host owns shared visual primitives, navigation, view controls, and overlay
behavior. Plugins supply content, capabilities, labels, and domain operations.
Do not hard-code To-do fields into generic host components.

## Screen structure and interaction rules

```text
Space / current location                          Search / global tools
Navigation       View name                        New task
                 Search tasks   Filter (3)   Sort   Group   Display
                 List: L1, L2   Group: S1, S2   Tag: T1
                 Temporary changes                  Save… / Reset
                 Task content                     Details, when opened
```

This is a conceptual desktop arrangement, not a decision to add another rail.
Keep the existing collapsible host navigation. Place plugin-specific navigation
only where it serves a distinct purpose. Page widgets use a compact version of
the same toolbar rather than a full nested application header.

- Separate **Filter** (which items), **Sort**, **Group by**, and **Display**
  (layout, fields, density). Show active filter summaries without opening an editor.
- Keep group as the organization noun and status as workflow state. Do not label
  workflow states as groups or reintroduce “section” in To-do organization.
- Preserve OR within a filter and AND across filters; expose tags any/all.
  Show unavailable selections explicitly so filters never silently broaden.
- Apply remains temporary. Save changes updates the shared view; Save as new
  creates a separate view. Show where a shared view is used before saving changes.
  Widget position/size stays local to the page.
- Use the same task title, status, date, tag, selection, and overflow patterns in
  lists, tables, boards, and widgets. Let each layout retain its useful structure.
- Title-only capture works immediately, with Inbox or an explicit destination.
  Scheduling and organization are progressively disclosed. Never infer one list
  from a view containing multiple lists.
- My Day distinguishes planned work from deadlines and reminders. Show unfinished
  earlier plans separately. Expose checklist steps inline without forcing details.
- Use anchored menus for short choices, sheets for item details/long forms,
  and dialogs for bounded decisions. Return focus to the opener; preserve drafts
  and scroll position where supported. State save behavior explicitly.
- Keep destructive actions in a consistent overflow location. Preserve the
  existing recurrence behavior; do not promise generic completion Undo.
- Mobile retains the same concepts using stacked content, accessible menus,
  and full-width detail sheets. Do not simply shrink desktop toolbars.

## Phases and acceptance gates

| Phase | Work | Reviewable result / exit gate |
| --- | --- | --- |
| 0 — Baseline | Capture Home, page widget, To-do capture/My Day/saved view/details, Notes, Search, Settings; include long names and empty/error states | Annotated screenshots and prioritized inconsistency inventory; distinguish observed issues from proposals |
| 1 — Foundation | Define semantic tokens and shared button/input/chip/menu/toolbar/sheet patterns; map existing theme tokens; document ownership | Component specimen in light/dark and compact/comfortable; keyboard focus, contrast, loading/error states demonstrated |
| 2 — Host shell | Align headers, navigation selection, action placement, page editing, search and overlay anatomy | Representative host screens use the same hierarchy; rail collapse, page switching, focus and scroll behavior verified |
| 3 — To-do pilot | Apply the shared patterns to capture, My Day, saved views, details, list/group/status management and page widgets | End-to-end capture → organize → plan → complete flow; filters and customization remain fully available |
| 4 — Other plugins | Adapt Notes, Habits, Bookmarks, Feeds, Voice and Reports to the shared components in small batches | Each retains domain-specific workflows while reusing headers, forms, actions, feedback and details patterns |
| 5 — Release verification | Check responsive behavior, themes, accessibility, persistence, host/page integration and installed native app | Screenshot comparison and functional results attached; remaining device limits stated; milestone commits ready for merge |

Start with a prototype showing the same data in the host frame, To-do table,
My Day, details, and a Home widget. Review it before broad implementation. Keep
implementation phases on a feature branch, commit each validated milestone, and
merge/install only when requested. Planning does not change the running app.

## Validation matrix

- Desktop: 1440px and narrow 1024px; mobile: 390px, plus 200% text/zoom checks.
- Light/dark defaults and representative installed themes; no hard-coded color
  that breaks theme meaning. Measure contrast rather than judging it by eye.
- Keyboard-only navigation, visible focus, Escape dismissal, focus restoration,
  screen-reader labels, reduced motion, and touch access without hover.
- Empty Inbox, long tasks, many groups/statuses, archived/missing choices, failed
  saves, pending actions, and larger result sets. Keep the current 500-row limit
  visible where relevant; this redesign does not solve pagination.
- Temporary filters never mutate shared views; saved changes propagate to page
  references; display preferences persist; multi-list capture uses the selected
  destination; repeating-task completion still creates exactly one successor.
- Preserve existing automated behavior checks; add targeted regressions for changed
  interactions. Browser evidence and native/device evidence must be reported separately.

## Boundaries and decisions

Recommended defaults: evolve the current visual identity; keep both density
options; use existing sheets first. Consider a persistent desktop inspector only
after the baseline demonstrates a repeated need to edit while scanning tasks.

Decision (FIX-11c): the owner asked for that inspector. At 1200px and wider,
the existing item sheet (the one `navigate: {item}` opens) docks on the right
beside the list or Today; it is the same sheet, content and actions, not a
second details UI. Below 1200px, and on phones, the sheet is unchanged. See
`docs/develop/plugin-authoring.md` ("Docked item sheet").

Do not combine this work with new scheduling rules, dependencies, data migrations,
sync changes, or full organization export. Those deserve separate acceptance
criteria. A visual consistency pass should not quietly change task semantics.
