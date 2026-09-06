import type {
  ThemeColors,
  ThemeElevation,
  ThemeModeTokens,
  ThemeMotion,
  ThemePack,
  ThemeShape,
  ThemeTypography,
} from "./types";
import { ADDITIONAL_THEMES } from "./catalog";
import { createNeonThemePack, NEON_ACCENTS, type NeonAccent } from "../design-system/tokens";

const geist = '"Geist Variable", ui-sans-serif, system-ui, sans-serif';
const system = '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif';
const serif = 'Iowan Old Style, "Palatino Linotype", Palatino, Georgia, serif';
const mono = '"SFMono-Regular", Consolas, "Liberation Mono", monospace';

const baseTypography: ThemeTypography = {
  fontUi: geist,
  fontDisplay: geist,
  fontMono: mono,
  baseSize: "15px",
  scale: 1,
};

const baseShape: ThemeShape = {
  radiusSm: "9px",
  radiusMd: "12px",
  radiusLg: "16px",
  radiusXl: "22px",
  borderWidth: "1px",
};

const baseMotion: ThemeMotion = {
  durationFast: "140ms",
  durationNormal: "200ms",
  durationSlow: "360ms",
  easing: "cubic-bezier(0.22, 1, 0.36, 1)",
};

const lightElevation: ThemeElevation = {
  shadowSm: "0 1px 2px rgba(27, 25, 22, 0.04)",
  shadowMd: "0 8px 24px -8px rgba(27, 25, 22, 0.12), 0 2px 6px rgba(27, 25, 22, 0.05)",
  shadowLg: "0 24px 60px -16px rgba(27, 25, 22, 0.22)",
  blur: "10px",
};

const darkElevation: ThemeElevation = {
  shadowSm: "0 1px 2px rgba(0, 0, 0, 0.3)",
  shadowMd: "0 8px 24px -8px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.35)",
  shadowLg: "0 24px 60px -16px rgba(0, 0, 0, 0.6)",
  blur: "10px",
};

const warmLight: ThemeColors = {
  canvas: "#f7f5f1", surface: "#ffffff", surface2: "#fcfbf8", ink: "#1b1916",
  inkSoft: "#46413a", muted: "#6f6960", faint: "#7c756a", line: "#e9e5dd",
  lineStrong: "#ddd8cd", accent: "#3d79bd", accentTint: "#e1eaf4", good: "#3f7d5b",
  bad: "#be463b", onInk: "#f7f5f1", inkHover: "#322d26", hoverSoft: "#efece5",
  scrollThumb: "#ddd7cb", scrollThumbHover: "#cfc8ba", errorBg: "#fbf0ee",
  errorBorder: "#f0d4cf", errorInk: "#9c3a31",
};

const warmDark: ThemeColors = {
  canvas: "#17150f", surface: "#211e18", surface2: "#1b1813", ink: "#f3efe7",
  inkSoft: "#cdc6ba", muted: "#a8a08f", faint: "#8e8677", line: "#322e26",
  lineStrong: "#423c31", accent: "#5797df", accentTint: "#19232f", good: "#6fb78c",
  bad: "#e57163", onInk: "#1b1916", inkHover: "#e9e3d8", hoverSoft: "#2a261f",
  scrollThumb: "#3a352c", scrollThumbHover: "#4a4338", errorBg: "#2b1a16",
  errorBorder: "#542f29", errorInk: "#f0a99f",
};

type ModeOverrides = {
  colors?: Partial<ThemeColors>;
  typography?: Partial<ThemeTypography>;
  shape?: Partial<ThemeShape>;
  elevation?: Partial<ThemeElevation>;
  motion?: Partial<ThemeMotion>;
  charts?: string[];
};

function mode(dark: boolean, overrides: ModeOverrides = {}): ThemeModeTokens {
  const charts = overrides.charts ?? (dark
    ? ["#5797df", "#b8b0a2", "#6fb78c", "#d6a45f", "#83a7af", "#c0b193", "#cc8576", "#a99bb8"]
    : ["#3d79bd", "#46413a", "#3f7d5b", "#b8893f", "#5e7e86", "#9b8b6e", "#8a5a4a", "#7a6c84"]);
  return {
    colors: { ...(dark ? warmDark : warmLight), ...overrides.colors },
    typography: { ...baseTypography, ...overrides.typography },
    shape: { ...baseShape, ...overrides.shape },
    elevation: { ...(dark ? darkElevation : lightElevation), ...overrides.elevation },
    motion: { ...baseMotion, ...overrides.motion },
    charts: charts.slice(0, 8) as ThemeModeTokens["charts"],
  };
}

function preset(
  id: string,
  name: string,
  description: string,
  light: ModeOverrides,
  dark: ModeOverrides,
): ThemePack {
  return {
    schemaVersion: 1,
    id,
    name,
    description,
    source: { kind: "builtin", label: "Included with Noted" },
    light: mode(false, light),
    dark: mode(true, dark),
  };
}

export const NOTED_WARM = preset(
  "noted-warm",
  "Noted Warm",
  "The original calm, warm and near-monochrome Noted design.",
  {},
  {},
);

const CUPERTINO = preset(
  "cupertino",
  "Cupertino",
  "A crisp, restrained system aesthetic inspired by macOS.",
  {
    colors: {
      canvas: "#f5f5f7", surface: "#ffffff", surface2: "#f9f9fb", ink: "#1d1d1f",
      inkSoft: "#3a3a3c", muted: "#6e6e73", faint: "#75757a", line: "#e5e5ea",
      lineStrong: "#d1d1d6", accent: "#007aff", accentTint: "#e5f1ff", good: "#248a3d",
      bad: "#d70015", onInk: "#ffffff", inkHover: "#343437", hoverSoft: "#ededf0",
      scrollThumb: "#c7c7cc", scrollThumbHover: "#aeaeb2", errorBg: "#fff0f1",
      errorBorder: "#ffd1d5", errorInk: "#b50012",
    },
    typography: { fontUi: system, fontDisplay: system },
    shape: { radiusSm: "10px", radiusMd: "14px", radiusLg: "18px", radiusXl: "24px" },
    elevation: {
      shadowSm: "0 1px 2px rgba(0, 0, 0, 0.04)",
      shadowMd: "0 10px 30px rgba(0, 0, 0, 0.10)",
      shadowLg: "0 24px 70px rgba(0, 0, 0, 0.18)",
      blur: "18px",
    },
    charts: ["#007aff", "#34c759", "#ff9f0a", "#af52de", "#5ac8fa", "#ff375f", "#5856d6", "#8e8e93"],
  },
  {
    colors: {
      canvas: "#000000", surface: "#1c1c1e", surface2: "#111113", ink: "#f5f5f7",
      inkSoft: "#d1d1d6", muted: "#a1a1a6", faint: "#848489", line: "#2c2c2e",
      lineStrong: "#3a3a3c", accent: "#0a84ff", accentTint: "#0b2945", good: "#30d158",
      bad: "#ff453a", onInk: "#111113", inkHover: "#e5e5ea", hoverSoft: "#2c2c2e",
      scrollThumb: "#3a3a3c", scrollThumbHover: "#545458", errorBg: "#351416",
      errorBorder: "#682126", errorInk: "#ff9a94",
    },
    typography: { fontUi: system, fontDisplay: system },
    shape: { radiusSm: "10px", radiusMd: "14px", radiusLg: "18px", radiusXl: "24px" },
    elevation: { blur: "18px" },
    charts: ["#0a84ff", "#30d158", "#ff9f0a", "#bf5af2", "#64d2ff", "#ff375f", "#5e5ce6", "#aeaeb2"],
  },
);

const LINEAR_MIDNIGHT = preset(
  "linear-midnight",
  "Linear Midnight",
  "Cool precision, violet energy and deep graphite surfaces.",
  {
    colors: {
      canvas: "#f6f5fa", surface: "#ffffff", surface2: "#faf9fc", ink: "#202024",
      inkSoft: "#45434d", muted: "#706d7a", faint: "#777481", line: "#e7e5ec",
      lineStrong: "#d8d5e0", accent: "#6558d9", accentTint: "#ece9ff", good: "#27845a",
      bad: "#c53e55", onInk: "#ffffff", inkHover: "#34323a", hoverSoft: "#efedf4",
      scrollThumb: "#d6d2df", scrollThumbHover: "#bfbacb", errorBg: "#fff0f3",
      errorBorder: "#f4cfd7", errorInk: "#a82f45",
    },
    shape: { radiusSm: "7px", radiusMd: "9px", radiusLg: "12px", radiusXl: "16px" },
    charts: ["#6558d9", "#2589bd", "#27845a", "#b16a15", "#ad477c", "#4e6cad", "#7b6b9d", "#65626d"],
  },
  {
    colors: {
      canvas: "#0f1015", surface: "#17181f", surface2: "#121319", ink: "#f1f1f4",
      inkSoft: "#c8c7ce", muted: "#9997a2", faint: "#82808b", line: "#292a33",
      lineStrong: "#383943", accent: "#8b7cf6", accentTint: "#25213e", good: "#5bc794",
      bad: "#ed6a7f", onInk: "#111218", inkHover: "#e2e1e7", hoverSoft: "#22232b",
      scrollThumb: "#33343e", scrollThumbHover: "#494a55", errorBg: "#301a21",
      errorBorder: "#5b2b38", errorInk: "#f29aaa",
    },
    shape: { radiusSm: "7px", radiusMd: "9px", radiusLg: "12px", radiusXl: "16px" },
    charts: ["#8b7cf6", "#55bde8", "#5bc794", "#e0a259", "#df77ae", "#7898ec", "#aa92cf", "#a4a2aa"],
  },
);

const PAPER = preset(
  "paper",
  "Paper",
  "A quiet, tactile notebook palette for long-form thinking.",
  {
    colors: {
      canvas: "#f2efe7", surface: "#fffdf7", surface2: "#f8f4e9", ink: "#29251f",
      inkSoft: "#4f483d", muted: "#71695d", faint: "#7c7366", line: "#e4dccd",
      lineStrong: "#d5c9b7", accent: "#8c5b3f", accentTint: "#f0e2d6", good: "#52705b",
      bad: "#a94b42", onInk: "#fffdf7", inkHover: "#40382e", hoverSoft: "#ebe5d8",
      scrollThumb: "#d5ccbc", scrollThumbHover: "#bfb4a2", errorBg: "#f8eae6",
      errorBorder: "#eacac1", errorInk: "#893c35",
    },
    typography: { fontDisplay: serif },
    shape: { radiusSm: "5px", radiusMd: "7px", radiusLg: "10px", radiusXl: "14px" },
    elevation: {
      shadowSm: "0 1px 1px rgba(72, 57, 38, 0.05)",
      shadowMd: "0 8px 20px rgba(72, 57, 38, 0.09)",
      shadowLg: "0 20px 48px rgba(72, 57, 38, 0.16)",
    },
    charts: ["#8c5b3f", "#52705b", "#a77a31", "#725d87", "#477484", "#9c675e", "#74705c", "#4f483d"],
  },
  {
    colors: {
      canvas: "#191611", surface: "#231f18", surface2: "#1e1a14", ink: "#eee7d9",
      inkSoft: "#c9beaa", muted: "#a49a87", faint: "#908674", line: "#352f25",
      lineStrong: "#473e30", accent: "#d49a75", accentTint: "#34251c", good: "#7fb08b",
      bad: "#df7b70", onInk: "#211b15", inkHover: "#e2d8c5", hoverSoft: "#2d281f",
      scrollThumb: "#3e372b", scrollThumbHover: "#554b3a", errorBg: "#321d19",
      errorBorder: "#5b302a", errorInk: "#efa79e",
    },
    typography: { fontDisplay: serif },
    shape: { radiusSm: "5px", radiusMd: "7px", radiusLg: "10px", radiusXl: "14px" },
    charts: ["#d49a75", "#7fb08b", "#d3aa61", "#a995c2", "#78a7b2", "#c88c83", "#aaa18c", "#c9beaa"],
  },
);

const EDITORIAL = preset(
  "editorial",
  "Editorial",
  "High-contrast type and ink-like details inspired by independent magazines.",
  {
    colors: {
      canvas: "#f5f2ec", surface: "#fffefa", surface2: "#f9f6f0", ink: "#151515",
      inkSoft: "#383735", muted: "#686662", faint: "#77756f", line: "#dedad2",
      lineStrong: "#c8c3b9", accent: "#b3322b", accentTint: "#f5dfdc", good: "#3e7152",
      bad: "#b3322b", onInk: "#fffefa", inkHover: "#2a2927", hoverSoft: "#ebe7df",
      scrollThumb: "#cbc5bb", scrollThumbHover: "#aaa399", errorBg: "#f7e6e3",
      errorBorder: "#e7c2bd", errorInk: "#922721",
    },
    typography: { fontUi: system, fontDisplay: serif },
    shape: { radiusSm: "2px", radiusMd: "3px", radiusLg: "4px", radiusXl: "6px" },
    charts: ["#b3322b", "#151515", "#3e7152", "#b47a23", "#416d7e", "#72557a", "#87634c", "#686662"],
  },
  {
    colors: {
      canvas: "#111111", surface: "#191919", surface2: "#141414", ink: "#f4f0e8",
      inkSoft: "#d0cbc2", muted: "#a39d93", faint: "#878179", line: "#2d2c2a",
      lineStrong: "#41403d", accent: "#e45b50", accentTint: "#351d1b", good: "#70ae82",
      bad: "#e45b50", onInk: "#151515", inkHover: "#e6e1d8", hoverSoft: "#242321",
      scrollThumb: "#3a3936", scrollThumbHover: "#52504c", errorBg: "#301918",
      errorBorder: "#5a2a27", errorInk: "#f09a93",
    },
    typography: { fontUi: system, fontDisplay: serif },
    shape: { radiusSm: "2px", radiusMd: "3px", radiusLg: "4px", radiusXl: "6px" },
    charts: ["#e45b50", "#f4f0e8", "#70ae82", "#d5a558", "#72a4b5", "#a783ae", "#bd8c6e", "#a39d93"],
  },
);

const TERMINAL = preset(
  "terminal",
  "Terminal",
  "Monospace utility with phosphor accents and hard-edged surfaces.",
  {
    colors: {
      canvas: "#eef2ec", surface: "#f8fbf6", surface2: "#f1f6ef", ink: "#152018",
      inkSoft: "#304237", muted: "#5c6d61", faint: "#67766b", line: "#d3ddd4",
      lineStrong: "#becabd", accent: "#087c3e", accentTint: "#d9efe1", good: "#087c3e",
      bad: "#b43c32", onInk: "#f8fbf6", inkHover: "#26372b", hoverSoft: "#e3ebe2",
      scrollThumb: "#c2cec2", scrollThumbHover: "#a7b7a7", errorBg: "#f9e9e6",
      errorBorder: "#e7c7c1", errorInk: "#933027",
    },
    typography: { fontUi: mono, fontDisplay: mono, fontMono: mono },
    shape: { radiusSm: "1px", radiusMd: "2px", radiusLg: "2px", radiusXl: "3px" },
    motion: { easing: "linear" },
    charts: ["#087c3e", "#3367a8", "#a56a00", "#8a4da0", "#007d83", "#ac3a58", "#5d6c58", "#304237"],
  },
  {
    colors: {
      canvas: "#07100a", surface: "#0b180f", surface2: "#08130c", ink: "#d7fbe0",
      inkSoft: "#a6d8b2", muted: "#79a584", faint: "#61886b", line: "#173321",
      lineStrong: "#245033", accent: "#42e879", accentTint: "#123820", good: "#42e879",
      bad: "#ff6b62", onInk: "#07100a", inkHover: "#c2f2ce", hoverSoft: "#102719",
      scrollThumb: "#1d422a", scrollThumbHover: "#2b5f3c", errorBg: "#351512",
      errorBorder: "#64241f", errorInk: "#ffa09a",
    },
    typography: { fontUi: mono, fontDisplay: mono, fontMono: mono },
    shape: { radiusSm: "1px", radiusMd: "2px", radiusLg: "2px", radiusXl: "3px" },
    elevation: { shadowSm: "none", shadowMd: "0 0 0 1px #173321", shadowLg: "0 0 30px rgba(66, 232, 121, 0.08)" },
    motion: { easing: "linear" },
    charts: ["#42e879", "#62a8ff", "#ffd166", "#d891ef", "#4bd8df", "#ff7697", "#92b68d", "#a6d8b2"],
  },
);

const SOFT_GLASS = preset(
  "soft-glass",
  "Soft Glass",
  "Airy translucent color, generous curves and diffused depth.",
  {
    colors: {
      canvas: "#eef2f8", surface: "#fbfdff", surface2: "#f4f7fc", ink: "#202736",
      inkSoft: "#465064", muted: "#6b7487", faint: "#6b7487", line: "#dce3ed",
      lineStrong: "#cbd5e2", accent: "#5574d8", accentTint: "#e4e9fb", good: "#41856c",
      bad: "#c14f67", onInk: "#ffffff", inkHover: "#343e51", hoverSoft: "#e6ebf3",
      scrollThumb: "#cbd4e0", scrollThumbHover: "#b3bfce", errorBg: "#fceef2",
      errorBorder: "#efced7", errorInk: "#a63d54",
    },
    shape: { radiusSm: "13px", radiusMd: "17px", radiusLg: "22px", radiusXl: "30px" },
    elevation: {
      shadowSm: "0 2px 8px rgba(64, 82, 120, 0.06)",
      shadowMd: "0 14px 36px rgba(64, 82, 120, 0.13)",
      shadowLg: "0 32px 80px rgba(64, 82, 120, 0.22)",
      blur: "22px",
    },
    charts: ["#5574d8", "#45a58b", "#d18b43", "#a663c2", "#43a3bf", "#d2607a", "#7f8bb5", "#68728a"],
  },
  {
    colors: {
      canvas: "#101521", surface: "#1a2130", surface2: "#141b28", ink: "#f0f3fa",
      inkSoft: "#c8cede", muted: "#9da6ba", faint: "#7f899e", line: "#2b3548",
      lineStrong: "#3a465c", accent: "#88a2ff", accentTint: "#252f52", good: "#72c5aa",
      bad: "#ef8297", onInk: "#131927", inkHover: "#dfe5f3", hoverSoft: "#232c3d",
      scrollThumb: "#354156", scrollThumbHover: "#4a5870", errorBg: "#341d27",
      errorBorder: "#603040", errorInk: "#f5a4b3",
    },
    shape: { radiusSm: "13px", radiusMd: "17px", radiusLg: "22px", radiusXl: "30px" },
    elevation: { blur: "22px" },
    charts: ["#88a2ff", "#72c5aa", "#e8ad6c", "#c589e5", "#73c4dd", "#ef8297", "#a6b0d1", "#aab2c4"],
  },
);

const HIGH_CONTRAST = preset(
  "high-contrast",
  "High Contrast",
  "Maximum legibility with strong borders and unmistakable focus color.",
  {
    colors: {
      canvas: "#ffffff", surface: "#ffffff", surface2: "#f4f4f4", ink: "#000000",
      inkSoft: "#161616", muted: "#3d3d3d", faint: "#555555", line: "#9b9b9b",
      lineStrong: "#4a4a4a", accent: "#0048d8", accentTint: "#dce8ff", good: "#006b35",
      bad: "#b00020", onInk: "#ffffff", inkHover: "#242424", hoverSoft: "#e8e8e8",
      scrollThumb: "#777777", scrollThumbHover: "#444444", errorBg: "#ffe8ec",
      errorBorder: "#b00020", errorInk: "#8c0019",
    },
    shape: { radiusSm: "3px", radiusMd: "4px", radiusLg: "6px", radiusXl: "8px", borderWidth: "2px" },
    elevation: { shadowSm: "none", shadowMd: "0 0 0 2px #4a4a4a", shadowLg: "0 0 0 3px #000000", blur: "0px" },
    charts: ["#0048d8", "#006b35", "#a65500", "#7900a8", "#006f7a", "#b00020", "#4c4c4c", "#000000"],
  },
  {
    colors: {
      canvas: "#000000", surface: "#000000", surface2: "#111111", ink: "#ffffff",
      inkSoft: "#f0f0f0", muted: "#d0d0d0", faint: "#b8b8b8", line: "#777777",
      lineStrong: "#b5b5b5", accent: "#65a7ff", accentTint: "#082d61", good: "#63d899",
      bad: "#ff808f", onInk: "#000000", inkHover: "#dddddd", hoverSoft: "#222222",
      scrollThumb: "#888888", scrollThumbHover: "#bbbbbb", errorBg: "#39000a",
      errorBorder: "#ff808f", errorInk: "#ffb0b9",
    },
    shape: { radiusSm: "3px", radiusMd: "4px", radiusLg: "6px", radiusXl: "8px", borderWidth: "2px" },
    elevation: { shadowSm: "none", shadowMd: "0 0 0 2px #b5b5b5", shadowLg: "0 0 0 3px #ffffff", blur: "0px" },
    charts: ["#65a7ff", "#63d899", "#ffc466", "#dc93ff", "#68dce8", "#ff808f", "#c1c1c1", "#ffffff"],
  },
);

export const NEON_THEMES: readonly ThemePack[] = (Object.keys(NEON_ACCENTS) as NeonAccent[]).map(accent => ({
  ...createNeonThemePack(accent),
  source: { kind: "builtin" as const },
}));
export const DEFAULT_THEME = NEON_THEMES[0];

export const BUILT_IN_THEMES: readonly ThemePack[] = [
  ...NEON_THEMES,
  NOTED_WARM,
  CUPERTINO,
  LINEAR_MIDNIGHT,
  PAPER,
  EDITORIAL,
  TERMINAL,
  SOFT_GLASS,
  HIGH_CONTRAST,
  ...ADDITIONAL_THEMES,
];

export const BUILT_IN_THEME_MAP = new Map(BUILT_IN_THEMES.map((theme) => [theme.id, theme]));

export function getBuiltInTheme(id: string): ThemePack | undefined {
  return BUILT_IN_THEME_MAP.get(id);
}
