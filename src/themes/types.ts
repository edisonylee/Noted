export const THEME_SCHEMA_VERSION = 1 as const;

export type ThemeMode = "light" | "dark";
export type ThemeModePreference = ThemeMode | "system";

export type ThemeSourceKind = "builtin" | "imported" | "assistant" | "custom";

export interface ThemeSource {
  kind: ThemeSourceKind;
  label?: string;
}

export const THEME_COLOR_KEYS = [
  "canvas",
  "surface",
  "surface2",
  "ink",
  "inkSoft",
  "muted",
  "faint",
  "line",
  "lineStrong",
  "accent",
  "accentTint",
  "good",
  "bad",
  "onInk",
  "inkHover",
  "hoverSoft",
  "scrollThumb",
  "scrollThumbHover",
  "errorBg",
  "errorBorder",
  "errorInk",
] as const;

export type ThemeColorKey = (typeof THEME_COLOR_KEYS)[number];
export type ThemeColors = Record<ThemeColorKey, string>;

export interface ThemeTypography {
  fontUi: string;
  fontDisplay: string;
  fontMono: string;
  baseSize: string;
  scale: number;
}

export interface ThemeShape {
  radiusSm: string;
  radiusMd: string;
  radiusLg: string;
  radiusXl: string;
  borderWidth: string;
}

export interface ThemeElevation {
  shadowSm: string;
  shadowMd: string;
  shadowLg: string;
  blur: string;
}

export interface ThemeMotion {
  durationFast: string;
  durationNormal: string;
  durationSlow: string;
  easing: string;
}

export interface ThemeModeTokens {
  colors: ThemeColors;
  typography: ThemeTypography;
  shape: ThemeShape;
  elevation: ThemeElevation;
  motion: ThemeMotion;
  charts: [string, string, string, string, string, string, string, string];
}

export interface ThemePack {
  schemaVersion: typeof THEME_SCHEMA_VERSION;
  id: string;
  name: string;
  description?: string;
  source: ThemeSource;
  light: ThemeModeTokens;
  dark: ThemeModeTokens;
}

export interface ThemeSelection {
  themeId: string;
  mode: ThemeModePreference;
}

export interface ThemeValidationSuccess {
  ok: true;
  value: ThemePack;
  warnings: string[];
}

export interface ThemeValidationFailure {
  ok: false;
  errors: string[];
}

export type ThemeValidationResult = ThemeValidationSuccess | ThemeValidationFailure;
