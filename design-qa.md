# Mobile design QA

## Comparison target

- Source visual truth paths:
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-3343cbfe-0c91-4df6-9a1c-776073007a79.png` (Home)
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-687cf370-7981-4420-b2a5-9d87ee43cdf8.png` (Schedule)
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-0970f2bc-19e7-47f5-89ec-f50c1fc5ab03.png` (Notes)
  - `/private/tmp/noted-desktop-calendar.jpeg` (Calendar desktop source)
- Implementation screenshot paths:
  - `/private/tmp/noted-mobile-home-final.png`
  - `/private/tmp/noted-mobile-schedule-final-2.png`
  - `/private/tmp/noted-mobile-calendar-final.png`
  - `/private/tmp/noted-mobile-notes-final.png`
- Combined comparison evidence:
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-86b11702-335d-481f-9123-46d3babfefe0.png` (Home and first Schedule pass)
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-d4b33271-faac-4136-a78e-af67323688d6.png` (revised Schedule pass)
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-a97caf62-ad26-4a12-9816-891e91e7ae88.png` (Notes)
  - `/Users/edison/.codex/generated_images/01a000c3-c169-7f11-a6cb-aedfb6a19c01/exec-9c779c53-c49c-4678-9251-828fe1786a13.png` (Calendar desktop-to-mobile adaptation)

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
