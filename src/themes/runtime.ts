import {
  THEME_COLOR_KEYS,
  THEME_SCHEMA_VERSION,
  type ThemeMode,
  type ThemeModeTokens,
  type ThemePack,
  type ThemeValidationResult,
} from "./types";

const COLOR_TO_CSS: Record<(typeof THEME_COLOR_KEYS)[number], string> = {
  canvas: "--canvas",
  surface: "--surface",
  surface2: "--surface-2",
  ink: "--ink",
  inkSoft: "--ink-soft",
  muted: "--muted",
  faint: "--faint",
  line: "--line",
  lineStrong: "--line-strong",
  accent: "--accent",
  accentTint: "--accent-tint",
  good: "--good",
  bad: "--bad",
  onInk: "--on-ink",
  inkHover: "--ink-hover",
  hoverSoft: "--hover-soft",
  scrollThumb: "--scroll-thumb",
  scrollThumbHover: "--scroll-thumb-hover",
  errorBg: "--error-bg",
  errorBorder: "--error-border",
  errorInk: "--error-ink",
};

export const THEME_CSS_VARIABLES = [
  ...Object.values(COLOR_TO_CSS),
  "--accent-rgb",
  "--r-sm",
  "--r-md",
  "--r-lg",
  "--r-xl",
  "--border-width",
  "--shadow-1",
  "--shadow-2",
  "--shadow-3",
  "--surface-blur",
  "--font",
  "--font-display",
  "--font-mono",
  "--base-size",
  "--type-scale",
  "--duration-fast",
  "--duration-normal",
  "--duration-slow",
  "--ease",
  ...Array.from({ length: 8 }, (_, index) => `--chart-${index + 1}`),
] as const;

const asRecord = (value: unknown): Record<string, unknown> | null =>
  typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;

function safeString(value: unknown, maxLength = 200): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= maxLength && !/[\u0000-\u001f]/.test(value);
}

function cssSupports(property: string, value: string): boolean {
  return typeof CSS === "undefined" || typeof CSS.supports !== "function" || CSS.supports(property, value);
}

function isSafeCssValue(value: unknown, property: string, maxLength = 240): value is string {
  return safeString(value, maxLength)
    && !/[;{}]/.test(value)
    && !/(?:url|image|expression|javascript|@import)\s*\(/i.test(value)
    && cssSupports(property, value);
}

function isSafeColor(value: unknown): value is string {
  return isSafeCssValue(value, "color", 100) && !/\bvar\s*\(/i.test(value);
}

function isSafeFont(value: unknown): value is string {
  return safeString(value)
    && !/[;{}@]/.test(value)
    && !/(?:url|expression|javascript)\s*\(/i.test(value);
}

function isSafeLength(value: unknown, min: number, max: number): value is string {
  if (!safeString(value, 20)) return false;
  const match = value.match(/^(-?(?:\d+\.?\d*|\.\d+))(px|rem|em)$/);
  if (!match) return false;
  const n = Number(match[1]);
  const multiplier = match[2] === "px" ? 1 : 16;
  return Number.isFinite(n) && n * multiplier >= min && n * multiplier <= max;
}

function isSafeDuration(value: unknown): value is string {
  if (!safeString(value, 20)) return false;
  const match = value.match(/^(\d+(?:\.\d+)?)(ms|s)$/);
  if (!match) return false;
  const milliseconds = Number(match[1]) * (match[2] === "s" ? 1000 : 1);
  return milliseconds >= 0 && milliseconds <= 2000;
}

function validateMode(value: unknown, path: string, errors: string[]): ThemeModeTokens | null {
  const mode = asRecord(value);
  if (!mode) {
    errors.push(`${path} must be an object.`);
    return null;
  }

  const colors = asRecord(mode.colors);
  const typography = asRecord(mode.typography);
  const shape = asRecord(mode.shape);
  const elevation = asRecord(mode.elevation);
  const motion = asRecord(mode.motion);
  const charts = mode.charts;

  if (!colors) errors.push(`${path}.colors must be an object.`);
  if (!typography) errors.push(`${path}.typography must be an object.`);
  if (!shape) errors.push(`${path}.shape must be an object.`);
  if (!elevation) errors.push(`${path}.elevation must be an object.`);
  if (!motion) errors.push(`${path}.motion must be an object.`);

  if (colors) {
    for (const key of THEME_COLOR_KEYS) {
      if (!isSafeColor(colors[key])) errors.push(`${path}.colors.${key} must be a safe CSS color.`);
    }
  }
  if (typography) {
    if (!isSafeFont(typography.fontUi)) errors.push(`${path}.typography.fontUi is invalid.`);
    if (!isSafeFont(typography.fontDisplay)) errors.push(`${path}.typography.fontDisplay is invalid.`);
    if (!isSafeFont(typography.fontMono)) errors.push(`${path}.typography.fontMono is invalid.`);
    if (!isSafeLength(typography.baseSize, 13, 18)) errors.push(`${path}.typography.baseSize must be 13–18px.`);
    if (typeof typography.scale !== "number" || typography.scale < 0.8 || typography.scale > 1.25) {
      errors.push(`${path}.typography.scale must be between 0.8 and 1.25.`);
    }
  }
  if (shape) {
    for (const key of ["radiusSm", "radiusMd", "radiusLg", "radiusXl"] as const) {
      if (!isSafeLength(shape[key], 0, 64)) errors.push(`${path}.shape.${key} must be between 0 and 64px.`);
    }
    if (!isSafeLength(shape.borderWidth, 0, 3)) errors.push(`${path}.shape.borderWidth must be between 0 and 3px.`);
  }
  if (elevation) {
    for (const key of ["shadowSm", "shadowMd", "shadowLg"] as const) {
      if (!isSafeCssValue(elevation[key], "box-shadow")) errors.push(`${path}.elevation.${key} is invalid.`);
    }
    if (!isSafeLength(elevation.blur, 0, 32)) errors.push(`${path}.elevation.blur must be between 0 and 32px.`);
  }
  if (motion) {
    for (const key of ["durationFast", "durationNormal", "durationSlow"] as const) {
      if (!isSafeDuration(motion[key])) errors.push(`${path}.motion.${key} must be between 0 and 2000ms.`);
    }
    if (!isSafeCssValue(motion.easing, "transition-timing-function", 100)) {
      errors.push(`${path}.motion.easing is invalid.`);
    }
  }
  if (!Array.isArray(charts) || charts.length !== 8 || !charts.every(isSafeColor)) {
    errors.push(`${path}.charts must contain exactly eight safe CSS colors.`);
  }

  if (errors.length > 0 || !colors || !typography || !shape || !elevation || !motion || !Array.isArray(charts)) {
    return null;
  }

  return {
    colors: Object.fromEntries(THEME_COLOR_KEYS.map((key) => [key, colors[key]])) as ThemeModeTokens["colors"],
    typography: {
      fontUi: typography.fontUi as string,
      fontDisplay: typography.fontDisplay as string,
      fontMono: typography.fontMono as string,
      baseSize: typography.baseSize as string,
      scale: typography.scale as number,
    },
    shape: {
      radiusSm: shape.radiusSm as string,
      radiusMd: shape.radiusMd as string,
      radiusLg: shape.radiusLg as string,
      radiusXl: shape.radiusXl as string,
      borderWidth: shape.borderWidth as string,
    },
    elevation: {
      shadowSm: elevation.shadowSm as string,
      shadowMd: elevation.shadowMd as string,
      shadowLg: elevation.shadowLg as string,
      blur: elevation.blur as string,
    },
    motion: {
      durationFast: motion.durationFast as string,
      durationNormal: motion.durationNormal as string,
      durationSlow: motion.durationSlow as string,
      easing: motion.easing as string,
    },
    charts: charts as ThemeModeTokens["charts"],
  };
}

export function validateThemePack(value: unknown): ThemeValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];
  const pack = asRecord(value);
  if (!pack) return { ok: false, errors: ["Theme pack must be an object."] };

  if (pack.schemaVersion !== THEME_SCHEMA_VERSION) {
    errors.push(`schemaVersion must be ${THEME_SCHEMA_VERSION}.`);
  }
  if (!safeString(pack.id, 128) || !/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(pack.id)) {
    errors.push("id may contain only letters, numbers, dots, underscores and hyphens.");
  }
  if (!safeString(pack.name, 80)) errors.push("name must be 1–80 characters.");
  if (pack.description !== undefined && !safeString(pack.description, 300)) {
    errors.push("description must be at most 300 characters.");
  }

  const source = asRecord(pack.source);
  const sourceKinds = new Set(["builtin", "imported", "assistant", "custom"]);
  if (!source || !sourceKinds.has(String(source.kind))) errors.push("source.kind is invalid.");
  if (source?.label !== undefined && !safeString(source.label, 120)) errors.push("source.label is invalid.");

  const light = validateMode(pack.light, "light", errors);
  const dark = validateMode(pack.dark, "dark", errors);
  if (errors.length > 0 || !light || !dark || !source) return { ok: false, errors };

  if (light.typography.baseSize !== dark.typography.baseSize) {
    warnings.push("Light and dark modes use different base sizes; switching modes may reflow content.");
  }

  return {
    ok: true,
    value: {
      schemaVersion: THEME_SCHEMA_VERSION,
      id: pack.id as string,
      name: pack.name as string,
      ...(pack.description ? { description: pack.description as string } : {}),
      source: {
        kind: source.kind as ThemePack["source"]["kind"],
        ...(source.label ? { label: source.label as string } : {}),
      },
      light,
      dark,
    },
    warnings,
  };
}

export function parseThemePackJson(text: string): ThemeValidationResult {
  try {
    return validateThemePack(JSON.parse(text));
  } catch {
    return { ok: false, errors: ["Theme pack is not valid JSON."] };
  }
}

function rgbChannels(color: string): string {
  const hex = color.match(/^#([\da-f]{3}|[\da-f]{6})$/i)?.[1];
  if (hex) {
    const full = hex.length === 3 ? [...hex].map((c) => c + c).join("") : hex;
    return `${parseInt(full.slice(0, 2), 16)}, ${parseInt(full.slice(2, 4), 16)}, ${parseInt(full.slice(4, 6), 16)}`;
  }
  const rgb = color.match(/^rgba?\(\s*(\d+(?:\.\d+)?)\D+(\d+(?:\.\d+)?)\D+(\d+(?:\.\d+)?)/i);
  if (rgb) return `${Math.round(Number(rgb[1]))}, ${Math.round(Number(rgb[2]))}, ${Math.round(Number(rgb[3]))}`;
  if (typeof document !== "undefined") {
    const probe = document.createElement("span");
    probe.style.color = color;
    probe.style.display = "none";
    document.documentElement.appendChild(probe);
    const computed = getComputedStyle(probe).color;
    probe.remove();
    const normalized = computed.match(/rgba?\(\s*(\d+)\D+(\d+)\D+(\d+)/i);
    if (normalized) return `${normalized[1]}, ${normalized[2]}, ${normalized[3]}`;
  }
  return "61, 121, 189";
}

export function themeModeToCssVariables(tokens: ThemeModeTokens): Record<string, string> {
  const variables: Record<string, string> = {};
  for (const key of THEME_COLOR_KEYS) variables[COLOR_TO_CSS[key]] = tokens.colors[key];
  Object.assign(variables, {
    "--accent-rgb": rgbChannels(tokens.colors.accent),
    "--r-sm": tokens.shape.radiusSm,
    "--r-md": tokens.shape.radiusMd,
    "--r-lg": tokens.shape.radiusLg,
    "--r-xl": tokens.shape.radiusXl,
    "--border-width": tokens.shape.borderWidth,
    "--shadow-1": tokens.elevation.shadowSm,
    "--shadow-2": tokens.elevation.shadowMd,
    "--shadow-3": tokens.elevation.shadowLg,
    "--surface-blur": tokens.elevation.blur,
    "--font": tokens.typography.fontUi,
    "--font-display": tokens.typography.fontDisplay,
    "--font-mono": tokens.typography.fontMono,
    "--base-size": tokens.typography.baseSize,
    "--type-scale": String(tokens.typography.scale),
    "--duration-fast": tokens.motion.durationFast,
    "--duration-normal": tokens.motion.durationNormal,
    "--duration-slow": tokens.motion.durationSlow,
    "--ease": tokens.motion.easing,
  });
  tokens.charts.forEach((color, index) => { variables[`--chart-${index + 1}`] = color; });
  return variables;
}

export function applyThemeToDocument(pack: ThemePack, mode: ThemeMode, target?: HTMLElement): Record<string, string> {
  const root = target ?? (typeof document !== "undefined" ? document.documentElement : undefined);
  const variables = themeModeToCssVariables(pack[mode]);
  if (!root) return variables;

  root.dataset.theme = mode;
  root.dataset.themeId = pack.id;
  root.style.colorScheme = mode;
  for (const [name, value] of Object.entries(variables)) root.style.setProperty(name, value);

  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  meta?.setAttribute("content", pack[mode].colors.canvas);
  return variables;
}
