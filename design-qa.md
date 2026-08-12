# Schedule Redesign QA

## Artifacts

- Source visual truth: `/Users/edison/.codex/generated_images/019fa3f1-8568-7fa2-8155-5b445b1e4825/exec-559ca515-7f24-4880-a036-58c779413e8e.png`
- Final implementation screenshot: `/private/tmp/noted-schedule-qa/09-final-1487x760-tasks-open.jpeg`
- Full-view comparison: `/private/tmp/noted-schedule-qa/10-final-reference-vs-implementation.png`
- Lower-schedule screenshot: `/private/tmp/noted-schedule-qa/11-final-lower-schedule.jpeg`
- Focused lower-schedule comparison: `/private/tmp/noted-schedule-qa/12-final-lower-reference-vs-implementation.png`

## Viewport and normalization

- Source pixels: 1487 × 1058.
- Implementation viewport: 1487 × 760 logical pixels in the native Tauri window.
- Full comparison normalization: the source was cropped from the top to 1487 × 760; the implementation remained at native 1:1 pixels.
- Focused comparison normalization: matching 1487 × 478 regions were cropped around the 5:00 PM–Up next portion of each image.
- State: light theme, Schedule selected, Thursday August 6, task drawer open, real local schedule/task data. The task drawer and timeline were compared in the same open state.

## Full-view comparison evidence

- The final wide layout matches the selected hierarchy: Daily Schedule and date first, calendar/tasks/edit controls on the same header line, hourly agenda as the primary surface, and the task drawer beside it.
- Empty hours restore the selected planner rhythm on windows at least 1400 px wide. Smaller windows intentionally collapse to event-only rows so the schedule remains usable without excessive scrolling.
- Event rows use start times, right-aligned plain durations, a light current-event wash, and the selected `NOW` + end-time subline.
- The task drawer begins below the page header, stays available while the agenda scrolls, extends to the lower app chrome, and uses the selected leading-plus task affordance.

## Focused comparison evidence

- The lower comparison confirms the 5:00–8:00 PM empty-hour rows, Anytime row, and compact Up next card preserve the source order and spacing.
- Up next contains a real non-`noted` Google Calendar event and remains below the full daily schedule instead of replacing it.

## Findings

- No actionable P0, P1, or P2 mismatch remains in the Schedule implementation.
- Existing app chrome remains intentionally product-native: the established collapsible 206 px sidebar and fixed assistant/backup bar were not globally redesigned for a single screen.
- The implementation uses the current time, so rows that have passed are muted dynamically; this state difference is expected and not a fidelity defect.

## Comparison history

1. Initial implementation: P1 — empty hours were omitted, collapsing the day; P1 — event rows used ranges and inline pills; P2 — the wide task drawer began at the top of the header and ended at its content.
2. First correction: added responsive 8:00 AM–8:00 PM empty-hour guide rows and moved the wide drawer below the header.
3. Second correction: changed event anatomy to start time + plain duration, added the active-event end-time subline, softened the active fill, extended the drawer, and removed wide-only add-row clutter.
4. Final polish: changed the empty task affordance to one leading plus and confirmed the full and focused comparisons at matching width and state.

## Interactions verified

- Opened and closed Tasks; Escape closes it and returns focus to the Tasks toggle.
- Opened Calendar sync and confirmed today's complete event list and sync action.
- Opened Edit day, confirmed the schedule editor, and cancelled without writing data.
- Scrolled the wide agenda and confirmed empty hours, Anytime, Up next, and the sticky task drawer.
- Confirmed the compact 920 px layout hides empty guide rows and uses the overlay task drawer.
- Confirmed Up next loads the next future event and no Recent Recordings section appears on Schedule.

## Build verification

- `git diff --check` passed.
- `bunx vite build` passed in the active worktree.
- `bun run build` (TypeScript + Vite) passed in an isolated QA checkout using the current Schedule sources while excluding an unrelated in-progress `NotesView.tsx` edit.
- `bun run tauri build --debug` passed in the isolated QA checkout and produced the native app used for interaction and screenshot verification.

final result: passed

---

# Inline checklist alignment QA — August 10, 2026

## Artifacts

- Source defect screenshot: `/var/folders/2z/4wqr492559s0wvbzwwy0w62h0000gn/T/TemporaryItems/NSIRD_screencaptureui_24TTyJ/Screenshot 2026-08-10 at 2.34.35 PM.png`
- Corrected native implementation screenshot: `/private/tmp/noted-inline-checklist-fixed.png`

## Viewport, state, and comparison evidence

- Source pixels: 584 × 1098. The source is a focused crop of the light-theme Tasks editor showing three broken checklist rows.
- Implementation pixels and native app viewport: 920 × 760 at 1:1 capture density. The light-theme Schedule screen is open with the Tasks editor and two temporary QA rows.
- The source is defect evidence rather than a target composition, so full-frame proportions are intentionally not treated as fidelity requirements. The focused comparison is the checklist anatomy: in the source, each checkbox occupies a separate line above its text; in the corrected implementation, each checkbox and first text line share one row, and wrapped copy aligns under its own text column.

## Required fidelity surfaces

- Fonts and typography: existing Geist type, size, line height, and editor hierarchy are unchanged.
- Spacing and layout rhythm: the checklist now uses explicit checkbox and content columns with a 9 px gap; multiline copy remains in the content column.
- Colors and visual tokens: existing light-theme surface, ink, muted, line, and accent tokens are unchanged.
- Image quality and asset fidelity: no raster or generated assets are part of the checklist control; the native checkbox remains intact.
- Copy and content: the QA sample confirmed short and wrapping task text without altering editor copy.

## Findings and comparison history

1. Initial P1: task-item children were not reliably forming one inline row, placing the checkbox above the paragraph. Fixed by adding stable task-list/task-item classes and an explicit two-column grid, while normalizing whitespace at the list-item boundary.
2. Post-fix: no actionable P0, P1, or P2 mismatch remains. Short and multiline tasks both align correctly.

## Interactions and build verification

- Created two checklist items in the rebuilt native app and confirmed Enter continued the checklist.
- Confirmed the long second item wrapped beneath its text rather than beneath its checkbox.
- Removed both temporary QA tasks and confirmed the editor returned to its prior empty state.
- `bun run build`, `bun test tests/document-editor.test.ts`, and `git diff --check -- src/App.css src/editor/DocumentEditor.tsx` passed.

final result: passed

---

# Schedule task-flow follow-up QA — August 6, 2026

This follow-up supersedes the earlier task-drawer and Anytime findings above.

## Artifacts

- Matched source screenshot: `/private/tmp/noted-schedule-redesign/00-source-matched.png`
- Final implementation screenshot: `/private/tmp/noted-schedule-redesign/01-implementation-final.png`
- Exact-viewport comparison: `/private/tmp/noted-schedule-redesign/02-source-implementation.png`
- Selected visual direction: `/Users/edison/.codex/generated_images/019f86ad-2893-7f11-b7a5-4e43b35ff97b/exec-2381a380-2597-4e90-aebc-19308d91608a.png`

## Viewport and state

- Source and implementation: 920 × 760 native Tauri pixels.
- State: light theme, Thursday August 6, populated schedule, task panel open, same local database and six real tasks.
- The comparison uses native 1:1 screenshots with no scaling or state substitution.

## Findings and corrections

- The source intentionally clipped the long fourth task with a three-line clamp. The final task list renders its complete text at natural height.
- The boxed drawer and rigid row dividers were replaced with one open writing margin, a quiet vertical separator, and independently scrolling task content.
- Task display is single-click editable; long edits use an auto-growing textarea. Enter commits, Shift+Enter inserts a line break, and Escape cancels without closing Tasks.
- The Add task composer remains fixed above the assistant bar while the task list itself scrolls.
- The Anytime section is absent. Older untimed rows remain preserved in storage, new untimed schedule entries receive guidance to use Tasks, and all-day calendar events stay in Calendar instead of becoming invisible agenda rows.
- Both the full-day editor and inline Add/Edit flow require a start time, closing the last path that could save an invisible untimed row.
- A uniquely matched Google Calendar event makes the full schedule row joinable. The final native run exposed `Daily Stand Up` as a `Join meeting` link using the event's owning Google account.
- Calendar-feed rows also expose the same direct join target before they are added to the schedule. Discord full and short invite hosts are recognized with exact hostname validation.
- Header controls were tightened after the first comparison pass so Calendar sync, Tasks, and Edit day remain on one line at the matched viewport.

## Interactions verified

- Clicked the long fourth task and confirmed a multiline textarea replaced the display row.
- Pressed Escape and confirmed the edit cancelled while the task panel stayed open.
- Confirmed the task composer remained visible at the bottom of the panel.
- Confirmed the populated `Daily Stand Up` row exposed a full-row join link after the connected calendar feed loaded.
- Confirmed no Anytime label or untimed row appeared in the agenda.
- Opened and cancelled the blank inline schedule row without writing data; its required start-time validation was also covered by the final source review.

## Build verification

- `bun run build` passed.
- `cargo test conference_link_prefers_hangout_then_conference_then_text` passed, including Discord and malicious-host rejection cases.
- `bun run tauri build --debug --bundles app` passed and produced the native app used for the final interaction and screenshot checks.

follow-up result: passed

---

# Image 1 task-ledger QA — August 6, 2026

This section supersedes the earlier Image 2 task-panel direction above. Image 1 is the selected visual truth.

## Artifacts

- Selected visual truth: `/private/tmp/noted-schedule-redesign/image1-selected-ledger.png`
- Final native implementation, standard window: `/private/tmp/noted-schedule-redesign/15-image1-ledger-final-native.png`
- Final native implementation, reference-matched width: `/private/tmp/noted-schedule-redesign/16-image1-ledger-final-1464x760.png`
- Full-view comparison: `/private/tmp/noted-schedule-redesign/17-image1-final-full-comparison.png`
- Focused task-ledger comparison: `/private/tmp/noted-schedule-redesign/18-image1-final-ledger-comparison.png`

## Viewport and normalization

- Source: 1464 × 1074 pixels. Implementation evidence: 920 × 760 and 1464 × 760 native Tauri pixels.
- Full comparison: source and implementation are both 1464 px wide; the source's top 760 px is compared to the implementation at native 1:1 density with no scaling.
- Focused comparison: both task panels are native 424 px-wide crops. The source uses `(1040, 65, 424, 695)` and the implementation uses `(999, 127, 424, 568)`, placed together without resizing.
- State: light theme, Schedule selected, task ledger open, same six real tasks, populated daily schedule. The source is taller than the available desktop verification viewport; the implementation intentionally uses a compact-height ledger below 900 px and the roomier Image 1 rhythm at taller viewports.

## Full-view comparison evidence

- The selected two-surface composition is preserved: the timeline remains primary and the task ledger is a distinct right-hand surface.
- At the reference width, the implementation's task ledger is 424 px wide, matching Image 1's approximately 422 px frame.
- Existing Noted chrome remains product-native: its 206 px sidebar, fixed assistant bar, schedule controls, and content top offset were not globally replaced by generated mock chrome.

## Focused comparison evidence

- The final ledger matches Image 1's complete outline, light surface, rounded corners, quiet `Tasks · 6` header, square checkboxes, full-width row rules, natural multiline copy, and inline plus composer.
- The desktop-only close button is absent, as in the selected image; the existing Tasks toggle remains the close/reopen control.
- At 760 px height all six real tasks and the composer remain visible. At taller viewports, responsive spacing increases to the selected roomier row rhythm without reintroducing clipping.

## Required fidelity surfaces

- Fonts and typography: the app keeps its existing Geist family. Task copy uses the selected plain, medium-density treatment; natural wrapping replaces truncation, with a taller-viewport size/rhythm adjustment.
- Spacing and layout rhythm: the frame width, interior padding, top-aligned checkboxes, ruled rows, and composer placement match the selected ledger. Compact-height spacing is an intentional responsive adaptation.
- Colors and visual tokens: the implementation uses existing warm canvas, surface, ink, and line tokens. The ledger has no added shadow or decorative color that is absent from Image 1.
- Image quality and asset fidelity: this task surface contains no raster artwork. Existing library icons are used for controls; no placeholder, emoji, or handmade icon substitute was introduced.
- Copy and content: `Tasks · 6` and `Add a task…` match the selected hierarchy. Real local task text is preserved verbatim, including its typed `->` characters.

## Findings and comparison history

1. Initial selected-direction mismatch: P1 — the implementation followed Image 2's open writing margin instead of Image 1's framed ledger. Fixed by restoring the complete frame, surface, radius, and reference-width column.
2. First ledger pass: P2 — the add composer was held at the panel bottom, leaving an artificial gap after the last task. Fixed by letting the independently scrollable task list shrink to content and placing the composer immediately after the final rule.
3. Wide compact-height pass: P2 — roomy wide-screen spacing pushed the sixth task partly below the visible list at 1464 × 760. Fixed with height-responsive density; the final native evidence shows all six tasks plus the composer.
4. Final comparison: no actionable P0, P1, or P2 mismatch remains. P3 — the mock's taller 1074 px viewport naturally shows more white space below the composer than the available 760 px native verification window.

## Interactions verified

- Clicked a task and confirmed its natural-height row becomes a multiline textarea.
- Pressed Escape and confirmed editing cancels without closing the ledger or changing task data.
- Confirmed the Tasks toggle remains expanded and is the desktop close/reopen affordance.
- Confirmed the add composer stays reachable after the final task and the list retains independent overflow behavior.
- Confirmed the populated `Daily Stand Up` row still exposes its direct Google Meet join link after the visual change.

## Build verification

- `git diff --check -- src/App.css design-qa.md` passed before the final native build.
- `bun run build` passed during the final Tauri build.
- `bun run tauri build --debug --bundles app` passed and produced the native app used for the final screenshots and interaction checks.

final result: passed
