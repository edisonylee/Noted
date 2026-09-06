# Noted neon design system

Status: integrated into the Mac product. Citron is the default for new selections and reset/fallback. Blue, pink, and green are built-in alternatives. Existing saved themes remain selectable; applying citron changes the current workspace. The separate preview remains available for future design work.

## Open the preview

```sh
bunx vite --config vite.design-system.config.ts
```

Open http://127.0.0.1:4176/design-system.html. **Workspace** explores capture, note reading, and source navigation. **Design system** shows the shared palette, controls, typography, spacing, and selected brand reference. The toolbar changes accent and light/dark mode across both views.

The notes and transcript are sample content. Search, filters, task checkboxes, source passages, and note creation work in browser memory. Reloading resets them. This preview does not read or write the user's real library.

## Visual foundation

Keep the selected **noted / Soft Cut** identity: compact, softly squared lowercase lettering. Geist supplies readable product typography. The orbital archive imagery belongs in campaign and brand moments; the reading interface carries its connecting-thread idea through highlights and citations.

Use white and near-black surfaces, neutral grays, fine separators, and one interchangeable neon accent. Avoid colored page backgrounds, glow effects, and multicolor decoration. The default is citron; the same product structure works with any of the four palettes.

| Accent | Vivid fill | Readable text on light surfaces |
| --- | --- | --- |
| Citron | `#DFFF00` | `#526000` |
| Electric blue | `#38C8FF` | `#006389` |
| Hot pink | `#FF64CE` | `#9D166F` |
| Acid green | `#79FF5C` | `#286514` |

These are digital sRGB colors. The physical fluorescence suggested by the brand reference is a print/material treatment, not a screen color capability.

## Color roles

The source of truth is [`tokens.ts`](../../src/design-system/tokens.ts).

- `accent` + `onAccent`: bright emphasis with near-black text, including key-thought highlights and the capture action.
- `accentInk`: readable links, icons, and emphasized text. Uses the companion shade in light mode and vivid color in dark mode.
- `accentSoft`: selection backgrounds and source context; pair with `accentInk`.
- `focus`: a high-contrast outline independent of the fill.
- `canvas`, `surface`, `raised`, `ink`, `secondary`, `muted`, `line`: neutrals that stay fixed when the accent changes.
- `danger` and `success`: stable functional meaning, paired with words/icons. They do not change with brand color.

Never put white text on these neon fills. Never use the vivid citron as small text on white. A separator can be subtle; an interactive control boundary must remain discernible.

## Type, space, and behavior

Geist uses a 12/13/15/17/22/36 px foundation for captions, labels, body, reading text, sections, and titles. Compact metadata in the preview has an 11 px exception. Spacing follows 4/8/12/16/24/32/48/64 px. Corners use 6 px for controls, 10 px for panels, and 16 px for sheets. Transitions use 120–180 ms and respect reduced motion.

Desktop places navigation, note library, and document beside each other. A source panel opens on demand. Below 1240 px the source stacks beneath the document; below 620 px the library and reader become separate views with a back action.

## Reuse and extend

[`components.tsx`](../../src/design-system/components.tsx) exports `Button`, `TextField`, `CheckField`, and `Citation`, with scoped styles in `primitives.css`.

```tsx
import type { CSSProperties } from 'react';
import { Button } from './design-system/components';
import { neonCssVariables } from './design-system/tokens';

<div className="nd-system" style={neonCssVariables('pink', 'light') as CSSProperties}>
  <Button variant="accent" onClick={startCapture}>New note</Button>
</div>
```

Load the bundled Geist font at the application entry. Add an accent by extending `NEON_ACCENTS` with `name`, `vivid`, `lightInk`, `lightTint`, and `darkTint`. The preview picker derives its options from this registry. Run the contrast tests after every palette change; a single arbitrary hex is insufficient to guarantee usable text and focus states.

`createNeonThemePack(accent)` produces a pack accepted by the existing ThemePack validator. It intentionally maps the legacy `accent` field to the readable companion because existing screens use that field for text as well as fills. The four neon packs are registered in `themes/presets.ts` and recognized by the native backend. The runtime adds `--accent-fill`, `--on-accent-fill`, and `--accent-focus`; the legacy `--accent` remains readable text. `product.css` applies the shared product treatment only when `data-design="neon"`. Appearance offers the four accents together above the existing theme catalog, with preview, cancel, and persistent Apply. Custom packs retain their own design treatment.

The wordmark asset is a raster concept derived from the selected reference, suitable for this preview. Final vector lettering and monogram sizing remain future artwork work. Product integration is based on current master and covers the Mac shell, Home, Documents, Library, document reading, meetings, scheduling, calendar, settings, dialogs, Team workspace and messaging, and shared assistant/knowledge/journal surfaces. Teams and service implementations are retained from master; the accent treatment is applied through semantic tokens and scoped CSS. The separate native iOS companion uses its own shell and is outside this Mac rollout.

## Validation

```sh
bun test tests/neon-design-system.test.ts
bunx tsc --noEmit
bunx vite build --config vite.design-system.config.ts
```

The token tests check text, accent fill, selection, focus, control boundaries, mode invariants, and compatibility with the current theme schema. See [preview QA](design-qa.md) for browser checks and [product QA](product-qa.md) for installed Mac verification and limitations.


## Product rollout

- Default: `noted-neon-citron` in both frontend and Rust. Deleting an active custom pack falls back to citron.
- Persistence: existing theme-state and fast-cache mechanisms; imported packs are retained. Explicit saved choices are not silently overwritten on upgrade.
- Identity: Soft Cut wordmark in the application; a solid black sidebar in either mode.
- Accent: vivid fills for capture/recording and new document actions, readable companion shades for links and focus, stable functional status colors.
- Home: weather remains available in a compact row; neon themes replace the atmospheric backdrop with a solid reading canvas.
- Native launch: updater plugin registration now requires updater configuration, so standard local builds can start without release-only signing settings.

## Install baseline

Build from a checkout containing `origin/master`. The standard install and automatic update scripts refuse or skip older branches so a design build cannot silently remove newer product features. This check uses the locally fetched remote ref; fetch before an intentional install. The active integration checkout for this rollout is `/Users/edison/Noted-neon-current`, based on `267a57b`.
