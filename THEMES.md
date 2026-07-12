# Noted Themes

Noted themes change the app's visual language without changing its layout,
calendar geometry, responsive behavior, or product logic. Themes work locally;
they do not require Refero MCP, a cloud model, or a paid API.

## Ways to add a theme

- Search and preview 50 built-in themes in **Settings → Themes**.
- Paste or upload a `DESIGN.md`. Noted's local Ollama model converts it into a
  constrained theme pack, then shows a preview before anything is applied.
- Ask the assistant for a visual direction such as “give Noted an Apple-styled
  retheme.” It selects from the installed themes and asks for confirmation.

Imported Markdown is never injected as CSS. Noted converts it to a validated,
versioned JSON pack containing only supported semantic tokens. Remote font URLs,
arbitrary selectors, scripts, and layout overrides are not accepted.

## Theme pack contract

Theme packs use `schemaVersion: 1` and provide `light` and `dark` modes:

```json
{
  "schemaVersion": 1,
  "id": "cupertino",
  "name": "Cupertino",
  "description": "A crisp system-inspired theme.",
  "source": { "kind": "builtin", "label": "Included with Noted" },
  "light": {
    "colors": {
      "canvas": "#f5f5f7",
      "surface": "#ffffff",
      "ink": "#1d1d1f",
      "accent": "#007aff"
    },
    "typography": {
      "fontUi": "-apple-system, BlinkMacSystemFont, sans-serif",
      "fontDisplay": "-apple-system, BlinkMacSystemFont, sans-serif",
      "fontMono": "SFMono-Regular, monospace",
      "baseSize": "15px",
      "scale": 1
    },
    "shape": {
      "radiusSm": "10px",
      "radiusMd": "14px",
      "radiusLg": "18px",
      "radiusXl": "24px",
      "borderWidth": "1px"
    },
    "elevation": {
      "shadowSm": "0 1px 2px rgba(0, 0, 0, 0.04)",
      "shadowMd": "0 10px 30px rgba(0, 0, 0, 0.10)",
      "shadowLg": "0 24px 70px rgba(0, 0, 0, 0.18)",
      "blur": "18px"
    },
    "motion": {
      "durationFast": "140ms",
      "durationNormal": "200ms",
      "durationSlow": "360ms",
      "easing": "cubic-bezier(0.22, 1, 0.36, 1)"
    },
    "charts": ["#007aff", "#34c759", "#ff9f0a", "#af52de", "#5ac8fa", "#ff375f", "#5856d6", "#8e8e93"]
  },
  "dark": { "...": "the same token groups for dark mode" }
}
```

Colors are semantic rather than component-specific: `canvas`, `surface`,
`surface2`, `ink`, `inkSoft`, `muted`, `faint`, `line`, `lineStrong`, `accent`,
`accentTint`, `good`, `bad`, and the related interaction/error roles. This keeps
every screen consistent while allowing Noted to add new components without
requiring old themes to contain selectors for them.

## Storage and privacy

Custom theme packs live under the Tauri app-data directory. The pasted source
Markdown is used only for local compilation and is not retained. The active
theme and color-mode preference are stored in app-data as well, so the desktop
and phone clients share one selection. A validated pack copy is cached in local
storage only to prevent a flash of the default palette during startup.

Fonts are limited to bundled or system font stacks. This preserves Noted's
offline behavior and prevents a theme from making hidden network requests.
