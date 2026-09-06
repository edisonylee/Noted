import type { ThemeMode, ThemeModeTokens, ThemePack } from '../themes/types';

export const NEON_ACCENTS = {
  citron: { name: 'Citron', vivid: '#DFFF00', lightInk: '#526000', lightTint: '#F4F8D9', darkTint: '#282D13' },
  blue: { name: 'Electric blue', vivid: '#38C8FF', lightInk: '#006389', lightTint: '#E2F5FD', darkTint: '#102B36' },
  pink: { name: 'Hot pink', vivid: '#FF64CE', lightInk: '#9D166F', lightTint: '#FDE9F7', darkTint: '#35172C' },
  green: { name: 'Acid green', vivid: '#79FF5C', lightInk: '#286514', lightTint: '#EAFBE5', darkTint: '#1B3016' },
} as const;

export type NeonAccent = keyof typeof NEON_ACCENTS;
export const DEFAULT_NEON_ACCENT: NeonAccent = 'citron';
export const FOUNDATIONS = {
  font: '"Geist Variable", ui-sans-serif, system-ui, sans-serif',
  spacing: [4, 8, 12, 16, 24, 32, 48, 64],
  type: { caption: 12, label: 13, body: 15, reading: 17, section: 22, title: 36 },
  radii: { control: 6, panel: 10, sheet: 16 },
  motion: { fast: '120ms', normal: '180ms', easing: 'cubic-bezier(0.2, 0, 0, 1)' },
} as const;

export function getNeonTokens(accent: NeonAccent, mode: ThemeMode) {
  const color = NEON_ACCENTS[accent];
  const dark = mode === 'dark';
  return {
    canvas: dark ? '#0A0A0A' : '#FFFFFF',
    surface: dark ? '#151515' : '#F7F7F7',
    raised: dark ? '#202020' : '#FFFFFF',
    ink: dark ? '#F5F5F5' : '#0A0A0A',
    secondary: dark ? '#BBBBBB' : '#555555',
    muted: dark ? '#A0A0A0' : '#707070',
    line: dark ? '#303030' : '#E6E6E6',
    controlLine: dark ? '#858585' : '#858585',
    hover: dark ? '#252525' : '#F0F0F0',
    accent: color.vivid,
    onAccent: '#0A0A0A',
    accentInk: dark ? color.vivid : color.lightInk,
    accentSoft: dark ? color.darkTint : color.lightTint,
    focus: dark ? color.vivid : color.lightInk,
    sidebar: '#0A0A0A',
    sidebarInk: '#F5F5F5',
    sidebarMuted: '#A0A0A0',
    danger: dark ? '#FF9696' : '#AD2020',
    dangerSoft: dark ? '#351919' : '#FFF0F0',
    success: dark ? '#A3E4B5' : '#28623C',
    successSoft: dark ? '#192A1E' : '#EBF5EE',
  } as const;
}

export function neonCssVariables(accent: NeonAccent, mode: ThemeMode): Record<string, string> {
  const values: Record<string, string> = Object.fromEntries(
    Object.entries(getNeonTokens(accent, mode)).map(([key, value]) => [
      `--nd-${key.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`)}`, value,
    ]),
  );
  values['--nd-font'] = FOUNDATIONS.font;
  values['--nd-fast'] = FOUNDATIONS.motion.fast;
  values['--nd-duration'] = FOUNDATIONS.motion.normal;
  values['--nd-ease'] = FOUNDATIONS.motion.easing;
  return values;
}

// The legacy pack uses accent in text as well as fills. Map it to the readable
// companion; new components use the vivid and on-accent semantic pair instead.
export function createNeonThemePack(accent: NeonAccent): ThemePack {
  function modeTokens(mode: ThemeMode): ThemeModeTokens {
    const t = getNeonTokens(accent, mode);
    return {
      colors: {
        canvas: t.canvas, surface: t.raised, surface2: t.surface, ink: t.ink,
        inkSoft: t.secondary, muted: t.muted, faint: t.muted, line: t.line,
        lineStrong: t.controlLine, accent: t.accentInk, accentTint: t.accentSoft,
        good: t.success, bad: t.danger, onInk: t.canvas,
        inkHover: mode === 'dark' ? '#DDDDDD' : '#303030', hoverSoft: t.hover,
        scrollThumb: t.controlLine, scrollThumbHover: t.secondary,
        errorBg: t.dangerSoft, errorBorder: t.danger, errorInk: t.danger,
      },
      typography: { fontUi: FOUNDATIONS.font, fontDisplay: FOUNDATIONS.font,
        fontMono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', baseSize: '15px', scale: 1 },
      shape: { radiusSm: '6px', radiusMd: '10px', radiusLg: '16px', radiusXl: '20px', borderWidth: '1px' },
      elevation: { shadowSm: 'none', shadowMd: '0 4px 12px rgba(0,0,0,0.12)', shadowLg: '0 12px 32px rgba(0,0,0,0.18)', blur: '0px' },
      motion: { durationFast: '120ms', durationNormal: '180ms', durationSlow: '240ms', easing: FOUNDATIONS.motion.easing },
      charts: [t.accentInk, t.ink, t.secondary, t.muted, '#858585', '#999999', '#AAAAAA', '#BBBBBB'],
    };
  }
  return { schemaVersion: 1, id: `noted-neon-${accent}`, name: `Noted / ${NEON_ACCENTS[accent].name}`,
    description: 'Monochrome foundation with a vivid, interchangeable accent.',
    source: { kind: 'custom', label: 'Noted design system' }, light: modeTokens('light'), dark: modeTokens('dark') };
}

export function neonAccentForTheme(themeId: string): NeonAccent | null {
  const accent = themeId.replace(/^noted-neon-/, '');
  return themeId.startsWith('noted-neon-') && Object.prototype.hasOwnProperty.call(NEON_ACCENTS, accent)
    ? accent as NeonAccent : null;
}
