import { useEffect, useRef, useState } from "react";
import { Check, FileText, Loader2, Palette, RotateCcw, Sparkles, Trash2, Upload } from "lucide-react";
import { api } from "./api";
import { useTheme, validateThemePack, type ThemeMode, type ThemeModePreference, type ThemePack } from "./useTheme";

function resolvedMode(mode: ThemeModePreference): ThemeMode {
  if (mode !== "system") return mode;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function ThemeSwatches({ pack, mode }: { pack: ThemePack; mode: ThemeMode }) {
  const colors = pack[mode].colors;
  return (
    <span className="theme-swatches" aria-hidden>
      {[colors.canvas, colors.surface, colors.ink, colors.accent, pack[mode].charts[2]].map((color, index) => (
        <i key={`${color}-${index}`} style={{ background: color }} />
      ))}
    </span>
  );
}

export function ThemesSettings() {
  const {
    modePreference,
    setModePreference,
    activeThemeId,
    themes,
    isPreviewing,
    previewTheme,
    clearPreview,
    activateTheme,
    registerTheme,
    removeTheme,
  } = useTheme();
  const [selectedId, setSelectedId] = useState(activeThemeId);
  const [designMd, setDesignMd] = useState("");
  const [themeName, setThemeName] = useState("");
  const [pendingPack, setPendingPack] = useState<ThemePack | null>(null);
  const [compiling, setCompiling] = useState(false);
  const [applying, setApplying] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const mode = resolvedMode(modePreference);
  const displayThemes = pendingPack ? [...themes, pendingPack] : themes;

  useEffect(() => {
    setSelectedId(activeThemeId);
  }, [activeThemeId]);

  useEffect(
    () => () => {
      clearPreview();
    },
    [clearPreview],
  );

  function selectTheme(pack: ThemePack) {
    setSelectedId(pack.id);
    setMessage(null);
    previewTheme(pendingPack?.id === pack.id ? pack : pack.id, mode);
  }

  function chooseMode(next: ThemeModePreference) {
    setModePreference(next);
    if (selectedId !== activeThemeId) {
      previewTheme(pendingPack?.id === selectedId ? pendingPack : selectedId, resolvedMode(next));
    }
  }

  async function applySelected() {
    setApplying(true);
    setMessage(null);
    try {
      if (pendingPack && pendingPack.id === selectedId) {
        const savedPack = await api.themeSave(pendingPack);
        registerTheme(savedPack);
      }
      await api.themeActivate(selectedId, modePreference);
      if (!activateTheme(selectedId, false)) throw new Error("That theme is no longer available.");
      setPendingPack(null);
      setDesignMd("");
      setThemeName("");
      setMessage("Theme saved for this Mac and phone. An open client refreshes it when focused.");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setApplying(false);
    }
  }

  function cancelPreview() {
    clearPreview();
    setPendingPack(null);
    setSelectedId(activeThemeId);
    setMessage(null);
  }

  async function compileDesign() {
    if (!designMd.trim()) return;
    setCompiling(true);
    setMessage(null);
    try {
      const compiled = await api.themeCompileDesign(designMd, themeName.trim() || undefined);
      const checked = validateThemePack(compiled);
      if (!checked.ok) throw new Error(checked.errors.join(" "));
      const pack = themes.some((theme) => theme.id === checked.value.id)
        ? { ...checked.value, id: `${checked.value.id.slice(0, 34)}-${Date.now().toString(36)}` }
        : checked.value;
      setPendingPack(pack);
      setSelectedId(pack.id);
      previewTheme(pack, mode);
      setMessage("Local preview ready. Nothing has been saved yet.");
    } catch (error) {
      setMessage(`Couldn’t create that theme locally: ${error}`);
    } finally {
      setCompiling(false);
    }
  }

  async function readFile(file?: File | null) {
    if (!file) return;
    if (file.size > 80_000) {
      setMessage("That file is larger than the 80 KB local import limit.");
      if (fileRef.current) fileRef.current.value = "";
      return;
    }
    try {
      setDesignMd(await file.text());
      if (!themeName) setThemeName(file.name.replace(/\.md$/i, "").replace(/[-_]+/g, " "));
      setMessage(`Loaded ${file.name}. Review it, then create a local preview.`);
    } catch (error) {
      setMessage(`Couldn’t read that file: ${error}`);
    } finally {
      if (fileRef.current) fileRef.current.value = "";
    }
  }

  async function deleteTheme(pack: ThemePack) {
    if (pack.source.kind === "builtin") return;
    if (pendingPack?.id === pack.id) {
      clearPreview();
      setPendingPack(null);
      setSelectedId(activeThemeId);
      setMessage(`Removed the “${pack.name}” preview.`);
      return;
    }
    try {
      await api.themeDelete(pack.id);
      removeTheme(pack.id);
      setSelectedId(activeThemeId === pack.id ? "noted-warm" : activeThemeId);
      setMessage(`Deleted “${pack.name}”.`);
    } catch (error) {
      setMessage(String(error));
    }
  }

  return (
    <>
      <h3>Themes</h3>
      <p className="settings-sub">
        Swap Noted’s visual system without changing its layout. Everything here is free: presets
        are bundled, and DESIGN.md files are interpreted by your local Ollama model. Refero MCP is
        not required.
      </p>

      <div className="theme-mode-row" role="group" aria-label="Color mode">
        {(["system", "light", "dark"] as const).map((item) => (
          <button
            key={item}
            className={"pill" + (modePreference === item ? " on" : "")}
            onClick={() => chooseMode(item)}
            aria-pressed={modePreference === item}
          >
            {item[0].toUpperCase() + item.slice(1)}
          </button>
        ))}
      </div>

      <div className="theme-gallery" role="group" aria-label="Installed themes">
        {displayThemes.map((pack) => {
          const selected = selectedId === pack.id;
          const active = activeThemeId === pack.id && !isPreviewing;
          return (
            <div className={"theme-card" + (selected ? " selected" : "")} key={pack.id}>
              <button className="theme-card-main" onClick={() => selectTheme(pack)} aria-pressed={selected}>
                <ThemeSwatches pack={pack} mode={mode} />
                <span className="theme-card-copy">
                  <strong>{pack.name}</strong>
                  <span>{pack.description}</span>
                </span>
                {active && <Check size={15} className="theme-active-check" aria-label="Active" />}
              </button>
              {pack.source.kind !== "builtin" && (
                <button className="theme-delete" onClick={() => void deleteTheme(pack)} aria-label={`Delete ${pack.name}`}>
                  <Trash2 size={13} />
                </button>
              )}
            </div>
          );
        })}
      </div>

      {(isPreviewing || selectedId !== activeThemeId) && (
        <div className="theme-preview-bar">
          <span><Palette size={14} /> Previewing {displayThemes.find((theme) => theme.id === selectedId)?.name ?? "theme"}</span>
          <button className="ghost-btn" onClick={cancelPreview}>Cancel</button>
          <button className="primary" onClick={() => void applySelected()} disabled={applying}>
            {applying ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
            Apply
          </button>
        </div>
      )}

      <div className="theme-import">
        <div className="theme-import-head">
          <span className="assistant-mark"><Sparkles size={14} /></span>
          <div>
            <strong>Import DESIGN.md</strong>
            <span>Paste a style from Refero Styles or upload the Markdown file.</span>
          </div>
        </div>
        <label className="field">
          <span className="field-label">Theme name <em>optional</em></span>
          <input
            value={themeName}
            onChange={(event) => setThemeName(event.target.value)}
            placeholder="e.g. Quiet Cupertino"
            maxLength={80}
          />
        </label>
        <label className="field">
          <span className="field-label">DESIGN.md</span>
          <textarea
            className="theme-design-input"
            value={designMd}
            onChange={(event) => setDesignMd(event.target.value)}
            placeholder="Paste the DESIGN.md contents here…"
            spellCheck={false}
            maxLength={80_000}
          />
        </label>
        <input
          ref={fileRef}
          type="file"
          accept=".md,text/markdown,text/plain"
          hidden
          onChange={(event) => void readFile(event.target.files?.[0])}
        />
        <div className="field-row">
          <button className="ghost-btn" onClick={() => fileRef.current?.click()}>
            <Upload size={14} /> Upload .md
          </button>
          <button
            className="primary"
            onClick={() => void compileDesign()}
            disabled={compiling || designMd.trim().length < 20}
          >
            {compiling ? <Loader2 size={14} className="spin" /> : <FileText size={14} />}
            {compiling ? "Creating locally…" : "Create preview"}
          </button>
        </div>
        <p className="field-hint">
          Imported prose becomes validated semantic tokens. Raw CSS, remote fonts, scripts, and
          layout overrides are never installed.
        </p>
      </div>

      <button
        className="theme-reset link"
        onClick={() => {
          setSelectedId("noted-warm");
          previewTheme("noted-warm", mode);
        }}
      >
        <RotateCcw size={13} /> Restore Noted Warm
      </button>
      {message && <div className="field-hint theme-message" role="status" aria-live="polite">{message}</div>}
    </>
  );
}
