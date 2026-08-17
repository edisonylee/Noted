# Design QA — Adaptive daily schedule

## Evidence

- Source visual truth: `/Users/edison/.codex/generated_images/01a000c2-3176-7b50-ab1b-71855b66c62a/exec-69df5d3b-df3b-47e3-b94b-c8c882a668d1.png`
- Implementation screenshot: `/private/tmp/noted-adaptive-schedule-implementation.png`
- Focused source crop: `/private/tmp/noted-adaptive-schedule-source-morning.png`
- Focused implementation crop: `/private/tmp/noted-adaptive-schedule-implementation-morning.png`
- Browser URL used for QA: `http://127.0.0.1:1420/?designPreview=1` with a temporary, removed preview fixture
- Viewport: 1794 × 1200 CSS px, desktop, device scale factor 1
- Source pixels: 1431 × 1099; implementation pixels: 1794 × 1200
- Density normalization: the source is an image-generation output with no declared CSS density. The implementation was captured at 1×. Full views were compared by composition, and the morning schedule was compared again with content-focused crops rather than treating raw pixel dimensions as equivalent.
- State: dark theme, task drawer open, six screenshot-matching events. The implementation fixture was intentionally calendar-disconnected, so its header says “Connect calendar”; the source shows the connected label. The source's 1:45 PM line and the implementation's live current-time line represent different clock states and were not treated as a layout mismatch.

## Full-view comparison evidence

The final implementation preserves the selected adaptive composition: a readable morning focus band, a labeled `10:30 AM–2:30 PM` four-hour free-time band, a true two-column overlap from 5:00–5:30 PM, and a labeled evening free-time band. The production header and task editor remain the existing Noted components and tokens.

## Focused region comparison evidence

The morning crops confirm that 8:00, 9:00, 9:45, and 10:00 events all occupy the full schedule width. The 9:45 and 10:00 cards are separated by 56 px from start to start; their painted heights are 53 px and 48 px, leaving visible breathing room without inventing an overlap column. Thirty-minute lines and labels remain legible behind the events.

## Findings

- No actionable P0, P1, or P2 findings remain.
- [P3] The selected mock uses a clay live-state accent while the captured Noted Warm theme uses its current blue `--accent` token. This is an expected design-system constraint; the schedule correctly inherits the active theme rather than hard-coding a mock-specific color.
- [P3] The implementation retains Noted's eyebrow, day navigation, and calendar connection state. These are existing product controls and do not interfere with the adaptive schedule hierarchy.

## Required fidelity surfaces

- Fonts and typography: Geist Variable is preserved. Event time, title, duration, half-hour labels, and free-time copy retain the existing Noted type hierarchy and remain readable without unintended wrapping.
- Spacing and layout rhythm: short events use a 48 px minimum with an 8 px clearance target; focus bands use 30-minute guides; long gaps compress to 58–74 px; true overlaps alone split into columns.
- Colors and visual tokens: canvas, surfaces, borders, text, hover, selection, and current-time states use existing theme variables. No mock-only color values were introduced.
- Image quality and asset fidelity: no raster or decorative image assets are required by this screen. Existing Lucide icons remain intact; no inline SVG, CSS drawing, emoji, or placeholder asset was added.
- Copy and content: event titles, time ranges, duration labels, task copy, and free-time ranges match the intended schedule content. The connection-label difference is fixture state, not copy drift.

## Comparison history

1. Initial comparison found three P2 fidelity issues: the midday gap began at 10:45 and read `3h 45m`; half-hour guides were unlabeled; and 40 px short cards with 4 px clearance still felt tight.
2. Fixes applied: short-event focus windows now end on the next 30-minute boundary; all focus marks display 30-minute labels; short cards use a 48 px minimum with 8 px clearance; events of 30 minutes or less retain compact content even when the adaptive scale grants more height.
3. Post-fix evidence: final full-view and morning crops listed above. The midday band now reads `10:30 AM–2:30 PM · Free time · 4h`; all morning events are full width; the only two-column pair is Team Bonding/Gym; and the focused comparison shows no remaining P0/P1/P2 drift.

## Interaction and runtime checks

- Clicking the middle of the compressed midday band created a draft at 1:30 PM with 15-minute snapping, confirming reversible compressed-time mapping.
- Draft cancellation returned to the neutral schedule state.
- Final measured widths: all four morning events 570 px; the two real-overlap events 283 px each.
- Final measured short-event heights: 48, 48, 53, and 48 px.
- Browser console warnings/errors: none.
- Production checks: `bun run build` and `bun test tests/schedule-layout.test.ts` pass.

## Implementation checklist

- [x] Use real event intervals for lane assignment.
- [x] Expand dense focus bands enough for readable short events.
- [x] Compress and label long free-time ranges.
- [x] Add 30-minute guides and labels in active bands.
- [x] Preserve direct creation and resize mapping across the piecewise scale.
- [x] Verify the screenshot-matching desktop state and focused morning region.

## Follow-up polish

- If desired later, test the same fixture across the optional theme catalog; the implementation already inherits each theme's tokens.

final result: passed
