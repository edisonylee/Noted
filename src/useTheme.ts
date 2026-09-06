import {
  createContext,
  createElement,
  useContext,
  useEffect,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { api } from "./api";
import { BUILT_IN_THEMES, getBuiltInTheme, DEFAULT_THEME } from "./themes/presets";
import { applyThemeToDocument, themeModeToCssVariables, validateThemePack } from "./themes/runtime";
import type {
  ThemeMode,
  ThemeModePreference,
  ThemePack,
  ThemeSelection,
} from "./themes/types";

// Backwards-compatible name used by chart and graph views.
export type Theme = ThemeMode;
export type { ThemeMode, ThemeModePreference, ThemePack, ThemeSelection } from "./themes/types";
export { BUILT_IN_THEMES } from "./themes/presets";
export { parseThemePackJson, validateThemePack } from "./themes/runtime";

const LEGACY_MODE_KEY = "noted-theme";
const SELECTION_KEY = "noted-theme-selection-v1";
const CUSTOM_THEMES_KEY = "noted-custom-themes-v1";
const FAST_CACHE_KEY = "noted-theme-cache-v1";

interface ThemeState {
  selection: ThemeSelection;
  resolvedMode: ThemeMode;
  activeTheme: ThemePack;
  themes: readonly ThemePack[];
  preview: { theme: ThemePack; mode: ThemeMode } | null;
}

function systemMode(): ThemeMode {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function resolveMode(preference: ThemeModePreference): ThemeMode {
  return preference === "system" ? systemMode() : preference;
}

function readSelection(): ThemeSelection {
  if (typeof localStorage === "undefined") return { themeId: DEFAULT_THEME.id, mode: "system" };
  try {
    const cached = JSON.parse(localStorage.getItem(SELECTION_KEY) ?? "null") as Partial<ThemeSelection> | null;
    if (
      cached
      && typeof cached.themeId === "string"
      && (cached.mode === "light" || cached.mode === "dark" || cached.mode === "system")
    ) {
      return { themeId: cached.themeId, mode: cached.mode };
    }
    const legacy = localStorage.getItem(LEGACY_MODE_KEY);
    return {
      themeId: DEFAULT_THEME.id,
      mode: legacy === "light" || legacy === "dark" ? legacy : "system",
    };
  } catch {
    return { themeId: DEFAULT_THEME.id, mode: "system" };
  }
}

function readCustomThemes(): ThemePack[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const items = JSON.parse(localStorage.getItem(CUSTOM_THEMES_KEY) ?? "[]") as unknown;
    if (!Array.isArray(items)) return [];
    return items.flatMap((item) => {
      const result = validateThemePack(item);
      return result.ok && !getBuiltInTheme(result.value.id) ? [result.value] : [];
    });
  } catch {
    return [];
  }
}

class ThemeStore {
  private registry = new Map<string, ThemePack>();
  private listeners = new Set<() => void>();
  private state: ThemeState;
  private media: MediaQueryList | null = null;

  constructor() {
    for (const pack of BUILT_IN_THEMES) this.registry.set(pack.id, pack);
    for (const pack of readCustomThemes()) this.registry.set(pack.id, pack);

    const requested = readSelection();
    const selection = this.registry.has(requested.themeId)
      ? requested
      : { ...requested, themeId: DEFAULT_THEME.id };
    const activeTheme = this.registry.get(selection.themeId) ?? DEFAULT_THEME;
    const resolvedMode = resolveMode(selection.mode);
    this.state = {
      selection,
      resolvedMode,
      activeTheme,
      themes: [...this.registry.values()],
      preview: null,
    };
    applyThemeToDocument(activeTheme, resolvedMode);
    this.persist();

    if (typeof window !== "undefined" && window.matchMedia) {
      this.media = window.matchMedia("(prefers-color-scheme: dark)");
      this.media.addEventListener("change", this.handleSystemChange);
    }
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): ThemeState => this.state;

  private emit(next: ThemeState): void {
    this.state = next;
    for (const listener of this.listeners) listener();
  }

  private handleSystemChange = (): void => {
    if (this.state.selection.mode !== "system" || this.state.preview) return;
    const resolvedMode = systemMode();
    applyThemeToDocument(this.state.activeTheme, resolvedMode);
    this.emit({ ...this.state, resolvedMode });
    this.persist();
  };

  private persist(): void {
    if (typeof localStorage === "undefined") return;
    try {
      const { selection, resolvedMode, activeTheme } = this.state;
      localStorage.setItem(SELECTION_KEY, JSON.stringify(selection));
      localStorage.setItem(LEGACY_MODE_KEY, resolvedMode);
      localStorage.setItem(
        CUSTOM_THEMES_KEY,
        JSON.stringify([...this.registry.values()].filter((theme) => !getBuiltInTheme(theme.id))),
      );
      localStorage.setItem(FAST_CACHE_KEY, JSON.stringify({
        schemaVersion: 1,
        themeId: activeTheme.id,
        modePreference: selection.mode,
        resolvedMode,
        cssVariables: themeModeToCssVariables(activeTheme[resolvedMode], activeTheme.id, resolvedMode),
      }));
    } catch {
      // Local persistence is a startup optimization; the live theme still works.
    }
  }

  setModePreference = (mode: ThemeModePreference, syncBackend = true): void => {
    const resolvedMode = resolveMode(mode);
    const selection = { ...this.state.selection, mode };
    applyThemeToDocument(this.state.activeTheme, resolvedMode);
    this.emit({ ...this.state, selection, resolvedMode, preview: null });
    this.persist();
    if (syncBackend) void api.themeSetColorMode(mode).catch(() => {});
  };

  toggle = (): void => {
    this.setModePreference(this.state.resolvedMode === "dark" ? "light" : "dark");
  };

  activateTheme = (themeId: string, syncBackend = true): boolean => {
    const activeTheme = this.registry.get(themeId);
    if (!activeTheme) return false;
    const selection = { ...this.state.selection, themeId };
    const resolvedMode = resolveMode(selection.mode);
    applyThemeToDocument(activeTheme, resolvedMode);
    this.emit({ ...this.state, selection, resolvedMode, activeTheme, preview: null });
    this.persist();
    if (syncBackend) void api.themeActivate(themeId, selection.mode).catch(() => {});
    return true;
  };

  registerTheme = (candidate: ThemePack): ThemePack => {
    const result = validateThemePack(candidate);
    if (!result.ok) throw new Error(result.errors.join(" "));
    if (getBuiltInTheme(result.value.id)) throw new Error("Built-in themes cannot be replaced.");
    this.registry.set(result.value.id, result.value);
    this.emit({ ...this.state, themes: [...this.registry.values()] });
    this.persist();
    return result.value;
  };

  removeTheme = (themeId: string): boolean => {
    if (getBuiltInTheme(themeId) || !this.registry.delete(themeId)) return false;
    if (this.state.activeTheme.id === themeId) {
      const selection = { ...this.state.selection, themeId: DEFAULT_THEME.id };
      const activeTheme = DEFAULT_THEME;
      const resolvedMode = resolveMode(selection.mode);
      applyThemeToDocument(activeTheme, resolvedMode);
      this.emit({
        ...this.state,
        selection,
        resolvedMode,
        activeTheme,
        themes: [...this.registry.values()],
        preview: null,
      });
    } else if (this.state.preview?.theme.id === themeId) {
      const resolvedMode = resolveMode(this.state.selection.mode);
      applyThemeToDocument(this.state.activeTheme, resolvedMode);
      this.emit({
        ...this.state,
        resolvedMode,
        themes: [...this.registry.values()],
        preview: null,
      });
    } else {
      this.emit({ ...this.state, themes: [...this.registry.values()] });
    }
    this.persist();
    return true;
  };

  previewTheme = (themeOrId: ThemePack | string, mode?: ThemeMode): boolean => {
    let theme: ThemePack | undefined;
    if (typeof themeOrId === "string") {
      theme = this.registry.get(themeOrId);
    } else {
      const result = validateThemePack(themeOrId);
      if (result.ok) theme = result.value;
    }
    if (!theme) return false;
    const previewMode = mode ?? this.state.resolvedMode;
    applyThemeToDocument(theme, previewMode);
    this.emit({ ...this.state, resolvedMode: previewMode, preview: { theme, mode: previewMode } });
    return true;
  };

  clearPreview = (): void => {
    if (!this.state.preview) return;
    const resolvedMode = resolveMode(this.state.selection.mode);
    applyThemeToDocument(this.state.activeTheme, resolvedMode);
    this.emit({ ...this.state, resolvedMode, preview: null });
  };
}

export const themeStore = new ThemeStore();
const ThemeContext = createContext(themeStore);
const EMPTY_THEMES: ThemePack[] = [];

export function ThemeProvider({ children, themes = EMPTY_THEMES }: { children: ReactNode; themes?: ThemePack[] }) {
  useEffect(() => {
    for (const theme of themes) {
      try {
        themeStore.registerTheme(theme);
      } catch {
        // Invalid reconciled themes are ignored; Settings can surface validation details.
      }
    }
  }, [themes]);
  useEffect(() => {
    let cancelled = false;
    async function syncFromBackend() {
      if (themeStore.getSnapshot().preview) return;
      const before = themeStore.getSnapshot().selection;
      try {
        const [savedThemes, savedState] = await Promise.all([api.themeList(), api.themeState()]);
        if (cancelled) return;
        const current = themeStore.getSnapshot();
        if (current.preview || current.selection.themeId !== before.themeId || current.selection.mode !== before.mode) return;
        for (const theme of savedThemes) {
          try {
            themeStore.registerTheme(theme);
          } catch {
            // The backend also validates packs; ignore stale/incompatible files.
          }
        }
        themeStore.setModePreference(savedState.colorMode, false);
        themeStore.activateTheme(savedState.activeThemeId, false);
      } catch {
        // Keep the fast local cache while desktop/phone transport reconnects.
      }
    }
    const onVisible = () => {
      if (document.visibilityState === "visible") void syncFromBackend();
    };
    void syncFromBackend();
    window.addEventListener("focus", syncFromBackend);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", syncFromBackend);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
  return createElement(ThemeContext.Provider, { value: themeStore }, children);
}

export function resolveInitialTheme(): Theme {
  return themeStore.getSnapshot().resolvedMode;
}

export function useTheme() {
  const store = useContext(ThemeContext);
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return {
    // Existing API
    theme: state.resolvedMode,
    toggle: store.toggle,
    // Theme engine API
    modePreference: state.selection.mode,
    setModePreference: store.setModePreference,
    activeThemeId: state.activeTheme.id,
    activeTheme: state.activeTheme,
    themes: state.themes,
    isPreviewing: state.preview !== null,
    previewTheme: store.previewTheme,
    clearPreview: store.clearPreview,
    activateTheme: store.activateTheme,
    registerTheme: store.registerTheme,
    removeTheme: store.removeTheme,
  };
}
