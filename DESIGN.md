# noted — Design

> **Crisp Data Canvas** — a focused, lightweight analytics aesthetic.
> Theme: **light**. Adapted from the Seline Analytics style reference.

This file documents the **theme** for noted — the colors, type, spacing, and
feel. It is intentionally *not* a component spec: no button/navbar/card recipes
live here. Build components however the screen needs; just stay inside these
tokens and principles. The CSS variables in `src/App.css` are the
implementation; this file is the intent behind them.

---

## 1. Atmosphere

A crisp, monochromatic base with a single vivid blue for active states and brand
accent. Surfaces are airy with soft, low-opacity shadows — elements feel like
they barely lift off the page. Typography is precise and utilitarian, prioritizing
readability over flair. The overall feeling is **clarity and organization**:
content and data do the talking, styling stays out of the way.

**Key characteristics**
- Near-white canvas with a warm achromatic undertone (`#fafaf9` over `#ffffff`)
- One accent only — Chartwell Blue — reserved for action and active states
- Diffused, sub-0.1 opacity shadows; never heavy or dark
- Soft rounded corners; pill shapes for the smallest chips
- Compact, dense, functional — analytics-dashboard density

---

## 2. Color — tokens & roles

### Surfaces & text
| Name | Value | Token | Role |
|------|-------|-------|------|
| Cloud White | `#ffffff` | `--color-cloud-white` | Primary surface — cards, raised UI |
| Canvas Fog | `#fafaf9` | `--color-canvas-fog` | Page background / base canvas |
| Slate Text | `#0c0a09` | `--color-slate-text` | Primary text, headings |
| Ash Gray | `#78716c` | `--color-ash-gray` | Secondary / muted text, icons |
| Steel Gray | `#a8a29e` | `--color-steel-gray` | Tertiary text, subtle icons |

### Borders & lines
| Name | Value | Token | Role |
|------|-------|-------|------|
| Stone Border | `#e5e7eb` | `--color-stone-border` | Default borders, dividers |
| Platinum Outline | `#d6d3d1` | `--color-platinum-outline` | Input borders, light separators |
| Hover Stone | `#c9c5c2` | `--color-hover-stone` | Subtle hover on text/borders |

### Accent
| Name | Value | Token | Role |
|------|-------|-------|------|
| Chartwell Blue | `#3ba6f1` | `--color-chartwell-blue` | **The only accent.** Primary action, active state, key data points, brand |
| Sky Tint | `#c1e1f7` | `--color-sky-tint` | Soft cool tint for subtle highlight backgrounds |
| Ghost Ink | `#1c1917` | `--color-ghost-ink` | Near-black for dark-hover affordances |

> **One-accent rule:** Chartwell Blue is precious. Use it only where you want the
> eye to go. Everything else is the monochrome stone/slate scale.

---

## 3. Typography

| Token | Family | Use |
|-------|--------|-----|
| `--font-inter` | Inter (→ system-ui) | Body, UI labels, captions, nav, descriptions |
| `--font-roobert` | roobert (→ sans-serif) | Headings & display |

Weights: Inter **400 / 500 / 600**, roobert **400 / 500**. Don't go heavier — the
system stays lightweight on purpose.

### Type scale
| Role | Size | Line height | Letter spacing | Token |
|------|------|-------------|----------------|-------|
| Caption | 12px | 1.5 | +0.048px | `--text-caption` |
| Heading sm | 18px | 1.25 | −0.016px | `--text-heading-sm` |
| Heading | 20px | 1.2 | −0.017px | `--text-heading` |
| Heading lg | 32px | 1.12 | −0.021px | `--text-heading-lg` |
| Display | 52px | 1.0 | −0.025px | `--text-display` |

Body text: Inter 14–16px, line-height ~1.5. Tighten tracking as size grows.

---

## 4. Spacing, radius & elevation

**Base unit 4px. Compact density.** Spacing tokens: `--spacing-4` … `--spacing-160`
(4, 8, 12, 16, 24, 32, 40, 48, 64, 80, 96, 160).

Rhythm: section gap **48px**, card padding **24px**, element gap **8px**.

### Radius
| Token | Value | Feel |
|-------|-------|------|
| `--radius-md` | 4px | Inputs, tight controls |
| `--radius-lg` | 10px | Cards, default containers |
| `--radius-2xl` | 16px | Prominent feature blocks |
| `--radius-full` | 9999px | Pills, tags, smallest chips |

### Shadows (diffused, low opacity)
| Token | Value | Use |
|-------|-------|-----|
| `--shadow-subtle` | `rgba(0,0,0,0.05) 0px 1px 2px 0px` | Buttons, faint lift |
| `--shadow-sm` | `rgba(0,0,0,0.1) 0px 4px 6px -1px, rgba(0,0,0,0.1) 0px 2px 4px -2px` | Icons, small elements |
| `--shadow-md` | `rgba(0,0,0,0.05) 0px 4px 16px 0px` | Default card elevation |
| `--shadow-xl` | `rgba(17,12,46,0.12) 0px 12px 45px 0px` | Reserved — top-priority surfaces |

### Surfaces
- **L0 — Canvas Fog `#fafaf9`**: base page background
- **L1 — Cloud White `#ffffff`**: content surfaces sitting above the canvas

---

## 5. Do / Don't

**Do**
- Use Canvas Fog for the page, Cloud White for surfaces — keep it light and airy.
- Reserve Chartwell Blue for primary action, active state, and brand only.
- Use Slate Text for headings/body for high contrast.
- Pair Inter (body/UI) with roobert (headings).
- Keep corners soft; keep density compact.
- Use diffused, low-opacity shadows; reach for `--shadow-xl` only when something
  must dominate.

**Don't**
- Don't add a second saturated color — the palette is monochrome + one blue.
- Don't use heavy/dark section backgrounds; the system relies on light surfaces.
- Don't go above Inter 600 / roobert 500 — no heavy type.
- Don't use hard, angular edges or deep dark shadows.
- Don't drift line-heights far from the scale (Inter ~1.5, roobert ~1.25).

---

## 6. Quick start — CSS variables

Drop into `:root` in `src/App.css`. (These tokens are the theme; component
styles consume them.)

```css
:root {
  /* Colors */
  --color-cloud-white: #ffffff;
  --color-canvas-fog: #fafaf9;
  --color-slate-text: #0c0a09;
  --color-ash-gray: #78716c;
  --color-steel-gray: #a8a29e;
  --color-stone-border: #e5e7eb;
  --color-platinum-outline: #d6d3d1;
  --color-hover-stone: #c9c5c2;
  --color-ghost-ink: #1c1917;
  --color-chartwell-blue: #3ba6f1;
  --color-sky-tint: #c1e1f7;

  /* Type families */
  --font-inter: 'Inter', ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  --font-roobert: 'roobert', ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;

  /* Type scale */
  --text-caption: 12px;     --leading-caption: 1.5;     --tracking-caption: 0.048px;
  --text-heading-sm: 18px;  --leading-heading-sm: 1.25; --tracking-heading-sm: -0.016px;
  --text-heading: 20px;     --leading-heading: 1.2;     --tracking-heading: -0.017px;
  --text-heading-lg: 32px;  --leading-heading-lg: 1.12; --tracking-heading-lg: -0.021px;
  --text-display: 52px;     --leading-display: 1;       --tracking-display: -0.025px;

  --font-weight-regular: 400;
  --font-weight-medium: 500;
  --font-weight-semibold: 600;

  /* Spacing */
  --spacing-4: 4px;   --spacing-8: 8px;   --spacing-12: 12px; --spacing-16: 16px;
  --spacing-24: 24px; --spacing-32: 32px; --spacing-40: 40px; --spacing-48: 48px;
  --spacing-64: 64px; --spacing-80: 80px; --spacing-96: 96px; --spacing-160: 160px;
  --section-gap: 48px; --card-padding: 24px; --element-gap: 8px;

  /* Radius */
  --radius-md: 4px;
  --radius-lg: 10px;
  --radius-2xl: 16px;
  --radius-full: 9999px;

  /* Shadows */
  --shadow-subtle: rgba(0, 0, 0, 0.05) 0px 1px 2px 0px;
  --shadow-sm: rgba(0, 0, 0, 0.1) 0px 4px 6px -1px, rgba(0, 0, 0, 0.1) 0px 2px 4px -2px;
  --shadow-md: rgba(0, 0, 0, 0.05) 0px 4px 16px 0px;
  --shadow-xl: rgba(17, 12, 46, 0.12) 0px 12px 45px 0px;

  /* Surfaces */
  --surface-canvas-fog: #fafaf9;
  --surface-cloud-white: #ffffff;
}
```

> **Fonts:** `roobert` needs a font file to render as intended; without it, the
> stack falls back to system sans (still fine). Inter is widely available; add the
> file or webfont if you want it exact.
