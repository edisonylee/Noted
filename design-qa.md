# Design QA

## Adaptive daily schedule

## Evidence

- Source visual truth: retained with the original QA task artifacts
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
---

## Mobile companion

## Comparison target

- Source visual truth artifacts:
  - Home
  - Schedule
  - Notes
  - `/private/tmp/noted-desktop-calendar.jpeg` (Calendar desktop source)
- Implementation screenshot paths:
  - `/private/tmp/noted-mobile-home-final.png`
  - `/private/tmp/noted-mobile-schedule-final-2.png`
  - `/private/tmp/noted-mobile-calendar-final.png`
  - `/private/tmp/noted-mobile-notes-final.png`
- Combined comparison evidence retained with the original QA task artifacts:
  - Home and first Schedule pass
  - Revised Schedule pass
  - Notes
  - Calendar desktop-to-mobile adaptation

## Viewport and normalization

- Device: iPhone 17 Pro simulator, iOS 26.5.
- CSS viewport: 402 × 874 points; implementation capture: 1206 × 2622 pixels at 3× density.
- Generated mobile sources: 853 × 1844 pixels. The comparison boards normalize the source and implementation to equal displayed height while preserving aspect ratio; no findings were filed from pixel-density or device-frame differences.
- State: dark mobile shell. Home and Schedule use realistic schedule fixtures; Notes intentionally shows the pre-pairing empty local library; Calendar is the day adaptation of the desktop week source.

## Full-view comparison evidence

- Home retains the atmospheric blue-gray canvas, Geist hierarchy, black capture and content cards, blue active state, and four-destination bottom navigation.
- Schedule retains the black timeline, blue section label, all-day event, compact event surfaces, task sheet, and bottom navigation.
- Calendar converts the desktop week grid into a legible day agenda while preserving the month/date controls, blue active date, all-day event, black canvas, and event-card vocabulary.
- Notes retains the black workspace, blue Work scope, All Notes hierarchy, search/filter row, gray library drawer, and blue Notes tab. The implementation's zero count is expected until Mac pairing completes.

## Focused-region comparison evidence

- Focused regions were required for Home capture controls, Schedule task tools, Notes library structure, and Calendar date/event controls; each is readable in the combined comparison boards.
- Typography: Geist Variable is bundled and used consistently. Display weights, compact metadata, letter spacing, truncation, and antialiasing follow the desktop direction.
- Spacing and layout: 16–22 point primary margins, 11–18 point radii, bottom-safe-area navigation, and 44-point-or-larger primary tap targets are consistent. The two-line weather metadata layout is an intentional readability adaptation for the narrower physical viewport.
- Colors and tokens: `#050505` black, charcoal surfaces, muted grays, `#4c83ff` blue, and the cyan all-day accent map cleanly to the desktop palette.
- Image quality: the generated atmospheric weather raster is bundled as a real PNG, cover-cropped without stretching, and remains sharp at 3× density. No emoji, handcrafted SVG art, CSS drawings, or placeholder asset substitutes are used; Lucide supplies interface icons.
- Copy/content: app-specific labels match the desktop product vocabulary. Pairing copy states the same-Wi-Fi and Mac-authority constraints plainly.
- Accessibility and behavior: semantic buttons/labels, focus-visible controls, reduced-motion handling, safe-area padding, practical tap targets, task completion toggles, day/week selection, Notes search/drawer/detail, capture, and Connect Mac states were exercised.

## Findings

- No actionable P0/P1/P2 visual findings remain.
- [P3] Home weather metadata uses two rows rather than the source's single row. This prevents cramped type at the 402-point viewport and preserves every datum.
- [P3] Notes is visually sparse before pairing because it truthfully renders the empty local library instead of shipping fake notes.

## Comparison history

1. Initial simulator pass
   - Earlier finding [P1]: iOS Simulator rejected its own test SQLite protection metadata and then failed Keychain access in an unsigned bundle, preventing rendered QA.
   - Fix: accept the simulator's file-protection attribute limitation only under `targetEnvironment(simulator)` while keeping backup-exclusion verification; build the simulator app with its normal debug signature.
   - Post-fix evidence: all four screens launched and were captured from the signed iPhone 17 Pro simulator.
2. First visual pass
   - Earlier finding [P1]: the Home weather asset was absent because the mobile Vite build intentionally disables `publicDir`.
   - Fix: import the PNG through the CSS asset graph so Vite fingerprints and bundles it.
   - Post-fix evidence: `/private/tmp/noted-mobile-home-final.png` and the Home comparison row in `exec-86b11702-335d-481f-9123-46d3babfefe0.png`.
   - Earlier finding [P2]: the Home container clipped content after the Recent card.
   - Fix: replace blanket overflow clipping with horizontal-only clipping so the lower Connect Mac state remains reachable.
3. Schedule fidelity pass
   - Earlier finding [P2]: the first implementation omitted the task-formatting toolbar visible in the selected Schedule target.
   - Fix: add the eight-control Lucide toolbar with selected state and adjust the task-list height.
   - Post-fix evidence: `/private/tmp/noted-mobile-schedule-final-2.png` and `exec-d4b33271-faac-4136-a78e-af67323688d6.png`.

## Implementation checklist

- [x] Shared desktop-matched mobile tokens and navigation
- [x] Home, Schedule, Calendar, and Notes screens
- [x] Notes library drawer and note-detail state
- [x] Task interactions and formatting controls
- [x] Connect Mac pairing sheet and matching desktop authority panel
- [x] Signed simulator build and four-screen visual capture
- [x] Source/implementation combined comparisons
- [x] Frontend, iOS shell, and deterministic Rust verification

## Follow-up polish

- Pair and sync a real library to replace the expected pre-pairing Notes empty state during the physical-device proof.
final result: passed
