import { useEffect, useState } from "react";
import { X, Check, Loader2 } from "lucide-react";
import { api, type ProviderMode, type ProviderSettings } from "./api";

// Model-provider settings. noted runs 100% local by default; "Balanced" sends
// only the latency-sensitive extract/OCR calls to Gemini so a busy local model
// never leaves a note stuck on "reading". Embeddings + chat stay local.
export function SettingsModal({ onClose }: { onClose: () => void }) {
  const [s, setS] = useState<ProviderSettings | null>(null);
  const [mode, setMode] = useState<ProviderMode>("local");
  const [key, setKey] = useState("");
  const [textModel, setTextModel] = useState("");
  const [visionModel, setVisionModel] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; msg: string } | null>(null);

  useEffect(() => {
    api.getProviderSettings().then((cfg) => {
      setS(cfg);
      setMode(cfg.mode);
      setTextModel(cfg.gemini_text_model);
      setVisionModel(cfg.gemini_vision_model);
    });
  }, []);

  async function save() {
    setSaving(true);
    try {
      await api.setProviderSettings({
        mode,
        // only send the key if the user typed a new one (blank = leave as-is)
        gemini_api_key: key.trim() ? key.trim() : undefined,
        gemini_text_model: textModel.trim() || undefined,
        gemini_vision_model: visionModel.trim() || undefined,
      });
      onClose();
    } finally {
      setSaving(false);
    }
  }

  async function runTest() {
    setTesting(true);
    setTest(null);
    try {
      // persist first so the backend tests against the current key/model
      await api.setProviderSettings({
        mode,
        gemini_api_key: key.trim() ? key.trim() : undefined,
        gemini_text_model: textModel.trim() || undefined,
        gemini_vision_model: visionModel.trim() || undefined,
      });
      const msg = await api.testProvider();
      setTest({ ok: true, msg });
      setS((prev) => (prev ? { ...prev, has_gemini_key: true } : prev));
    } catch (e) {
      setTest({ ok: false, msg: String(e) });
    } finally {
      setTesting(false);
    }
  }

  const hasKey = s?.has_gemini_key || key.trim().length > 0;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
        <button className="icon-btn modal-close" onClick={onClose} aria-label="Close">
          <X size={16} />
        </button>
        <h3>Models</h3>
        <p className="settings-sub">
          noted runs entirely on your machine by default — free and private. Switch to Balanced
          if local feels slow: only note extraction and photo OCR go to Gemini (fast + cheap);
          your embeddings and chat stay local.
        </p>

        <div className="pill-group settings-seg">
          <button className={"pill" + (mode === "local" ? " on" : "")} onClick={() => setMode("local")}>
            Local <em>$0 · private</em>
          </button>
          <button className={"pill" + (mode === "balanced" ? " on" : "")} onClick={() => setMode("balanced")}>
            Balanced <em>Gemini hot path</em>
          </button>
        </div>

        {mode === "balanced" && (
          <div className="settings-fields">
            <label className="field">
              <span className="field-label">
                Gemini API key{" "}
                {s?.has_gemini_key && <em className="field-saved">saved</em>}
              </span>
              <input
                type="password"
                placeholder={s?.has_gemini_key ? "•••••••• (leave blank to keep)" : "AIza…"}
                value={key}
                onChange={(e) => setKey(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
              <span className="field-hint">
                Stored in your macOS Keychain — never written to disk.{" "}
                <a href="https://aistudio.google.com/apikey" target="_blank" rel="noreferrer">
                  Get a free key
                </a>
              </span>
            </label>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Extract model</span>
                <input value={textModel} onChange={(e) => setTextModel(e.target.value)} spellCheck={false} />
              </label>
              <label className="field">
                <span className="field-label">OCR model</span>
                <input value={visionModel} onChange={(e) => setVisionModel(e.target.value)} spellCheck={false} />
              </label>
            </div>

            <button className="ghost-btn test-btn" onClick={runTest} disabled={testing || !hasKey}>
              {testing ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
              Test connection
            </button>
            {test && (
              <div className={"test-result " + (test.ok ? "ok" : "err")}>{test.msg}</div>
            )}
          </div>
        )}

        <div className="settings-actions">
          <button className="ghost-btn" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" onClick={save} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
