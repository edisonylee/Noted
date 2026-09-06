import { describe, expect, test } from 'bun:test';
import { createNeonThemePack, getNeonTokens, NEON_ACCENTS, neonCssVariables, type NeonAccent } from '../src/design-system/tokens';
import { validateThemePack } from '../src/themes/runtime';

function luminance(hex: string) {
  return [0, 2, 4].map(index => parseInt(hex.slice(1 + index, 3 + index), 16) / 255)
    .map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    .reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
}
function contrast(a: string, b: string) {
  const [bright, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (bright + 0.05) / (dark + 0.05);
}

describe('Neon semantic color contracts', () => {
  for (const accent of Object.keys(NEON_ACCENTS) as NeonAccent[]) {
    for (const mode of ['light', 'dark'] as const) {
      test(`${accent}/${mode}: text and focus stay readable`, () => {
        const t = getNeonTokens(accent, mode);
        for (const surface of [t.canvas, t.surface, t.raised]) {
          for (const text of [t.ink, t.secondary, t.muted, t.accentInk, t.danger, t.success]) {
            expect(contrast(text, surface)).toBeGreaterThanOrEqual(4.5);
          }
          expect(contrast(t.focus, surface)).toBeGreaterThanOrEqual(3);
          expect(contrast(t.controlLine, surface)).toBeGreaterThanOrEqual(3);
        }
        expect(contrast(t.onAccent, t.accent)).toBeGreaterThanOrEqual(4.5);
        expect(contrast(t.accentInk, t.accentSoft)).toBeGreaterThanOrEqual(4.5);
        expect(contrast(t.danger, t.dangerSoft)).toBeGreaterThanOrEqual(4.5);
        expect(contrast(t.success, t.successSoft)).toBeGreaterThanOrEqual(4.5);
        expect(contrast(t.sidebarMuted, t.sidebar)).toBeGreaterThanOrEqual(4.5);
      });
      test(`${accent}/${mode}: replacing the accent preserves neutrals and status`, () => {
        const base = getNeonTokens('citron', mode);
        const candidate = getNeonTokens(accent, mode);
        for (const role of ['canvas', 'ink', 'surface', 'raised', 'line', 'sidebar', 'danger', 'success'] as const) {
          expect(candidate[role]).toBe(base[role]);
        }
        const variables = neonCssVariables(accent, mode);
        expect(variables['--nd-on-accent']).toBe(candidate.onAccent);
        expect(variables['--nd-accent-ink']).toBe(candidate.accentInk);
        expect(variables['--nd-accent-soft']).toBe(candidate.accentSoft);
        expect(Object.values(variables).every(Boolean)).toBe(true);
      });
    }
    test(`${accent}: adapter passes the existing theme-pack validator`, () => {
      const result = validateThemePack(createNeonThemePack(accent));
      expect(result.ok).toBe(true);
    });
  }
});

describe('Product theme integration', () => {
  test('citron is the default and existing themes remain selectable', async () => {
    const { DEFAULT_THEME, BUILT_IN_THEMES, getBuiltInTheme } = await import('../src/themes/presets');
    expect(DEFAULT_THEME.id).toBe('noted-neon-citron');
    expect(BUILT_IN_THEMES[0]).toBe(DEFAULT_THEME);
    expect(getBuiltInTheme('noted-warm')?.source.kind).toBe('builtin');
    expect(getBuiltInTheme('paper')?.source.kind).toBe('builtin');
    expect(new Set(BUILT_IN_THEMES.map(pack => pack.id)).size).toBe(BUILT_IN_THEMES.length);
  });
  test('real runtime exports vivid fills separately from readable accent text', async () => {
    const { NEON_THEMES, NOTED_WARM } = await import('../src/themes/presets');
    const { themeModeToCssVariables } = await import('../src/themes/runtime');
    for (const pack of NEON_THEMES) {
      expect(validateThemePack(pack).ok).toBe(true);
      for (const mode of ['light', 'dark'] as const) {
        const vars = themeModeToCssVariables(pack[mode], pack.id, mode);
        expect(contrast(vars['--on-accent-fill'], vars['--accent-fill'])).toBeGreaterThanOrEqual(4.5);
        expect(contrast(vars['--accent'], vars['--surface'])).toBeGreaterThanOrEqual(4.5);
        expect(contrast(vars['--accent-focus'], vars['--canvas'])).toBeGreaterThanOrEqual(3);
        const other = themeModeToCssVariables(NOTED_WARM[mode], NOTED_WARM.id, mode);
        expect(other['--accent-fill']).toBe(NOTED_WARM[mode].colors.accent);
        expect(other['--accent']).toBe(NOTED_WARM[mode].colors.accent);
      }
    }
  });
  test('frontend and native backend recognize the same built-in themes', async () => {
    const { BUILT_IN_THEMES, DEFAULT_THEME } = await import('../src/themes/presets');
    const backend = await Bun.file(new URL('../src-tauri/src/themes.rs', import.meta.url)).text();
    const defaultId = backend.match(/DEFAULT_THEME_ID: &str = "([^"]+)"/)![1];
    const registry = backend.split('const BUILTIN_IDS: &[&str] = &[')[1].split('];')[0];
    const nativeIds = [defaultId, ...[...registry.matchAll(/"([a-z0-9-]+)"/g)].map(match => match[1])];
    expect(defaultId).toBe(DEFAULT_THEME.id);
    expect(nativeIds.sort()).toEqual(BUILT_IN_THEMES.map(pack => pack.id).sort());
  });
});
