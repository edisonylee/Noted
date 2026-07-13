import { useEffect, useState } from "react";
import { X, Check, Loader2, Wifi, WifiOff, CalendarCheck, CalendarX, Download, Mic, Plus, RefreshCw, Trash2, FolderPlus } from "lucide-react";
import { api, type BrainVaultStatus, type CloudProvider, type GcalStatus, type MeetingsCfg, type MeetingModelStatus, type MeetingTemplate, type ProviderMode, type ProviderSettings } from "./api";
import { ThemesSettings } from "./ThemesSettings";

// Live connection status, shown as a persistent badge so "is Gemini actually
// reachable?" is never a mystery — checked on open and after every save/test.
type Conn =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "ok"; msg: string }
  | { state: "err"; msg: string };

type SettingsSection = "models" | "themes" | "calendar" | "vaults" | "meetings";

// Model-provider settings. noted runs 100% local by default; "Balanced" sends
// only the latency-sensitive extract/OCR calls to Gemini so a busy local model
// never leaves a note stuck on "reading". Embeddings + chat stay local.
// Rendered two ways: `page` (desktop — a real Settings view with a section
// nav) or as the compact modal (mobile).
export function SettingsModal({ onClose, page = false }: { onClose: () => void; page?: boolean }) {
  const [section, setSection] = useState<SettingsSection>("models");
  const [savedHint, setSavedHint] = useState(false);
  const [s, setS] = useState<ProviderSettings | null>(null);
  const [mode, setMode] = useState<ProviderMode>("local");
  const [key, setKey] = useState("");
  const [textModel, setTextModel] = useState("");
  const [visionModel, setVisionModel] = useState("");
  // Balanced-mode cloud provider: Gemini, any OpenAI-compatible endpoint, or
  // Anthropic. Each keeps its own key (Keychain) + model pair.
  const [cloudProvider, setCloudProvider] = useState<CloudProvider>("gemini");
  const [openaiBase, setOpenaiBase] = useState("");
  const [openaiKey, setOpenaiKey] = useState("");
  const [openaiText, setOpenaiText] = useState("");
  const [openaiVision, setOpenaiVision] = useState("");
  const [anthropicKey, setAnthropicKey] = useState("");
  const [anthropicText, setAnthropicText] = useState("");
  const [anthropicVision, setAnthropicVision] = useState("");
  // Local (Ollama) models — dropdowns fed by the health check's pulled list.
  const [localTextModel, setLocalTextModel] = useState("");
  const [localVisionModel, setLocalVisionModel] = useState("");
  const [installedModels, setInstalledModels] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [conn, setConn] = useState<Conn>({ state: "idle" });

  // Google Calendar sync (one-way push to a dedicated "noted" calendar).
  const [gcal, setGcal] = useState<GcalStatus | null>(null);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [gcalBusy, setGcalBusy] = useState<"" | "saving" | "connecting">("");
  const [gcalMsg, setGcalMsg] = useState<{ kind: "ok" | "err"; text: string } | null>(null);

  // Brain vaults (Obsidian ↔ noted sync).
  const [vaults, setVaults] = useState<BrainVaultStatus[]>([]);
  const [vaultPath, setVaultPath] = useState("");
  const [vaultBusy, setVaultBusy] = useState(""); // "" | "adding" | "sync:<vault>" | "sync:all" | "rm:<vault>"
  const [vaultMsg, setVaultMsg] = useState<string | null>(null);
  const [autoProp, setAutoProp] = useState(true);

  // Meetings (recorder + detection). Changes save immediately, like autoProp.
  const [mcfg, setMcfg] = useState<MeetingsCfg | null>(null);
  const [mModel, setMModel] = useState<MeetingModelStatus | null>(null);
  const [mTemplates, setMTemplates] = useState<MeetingTemplate[]>([]);
  // Template editor: which template row is expanded, and its draft state.
  // A null editTpl with a non-null draft = creating a new template.
  const [editTpl, setEditTpl] = useState<string | null>(null);
  const [tplDraft, setTplDraft] = useState<{ name: string; prompt: string } | null>(null);
  const [tplBusy, setTplBusy] = useState(false);
  const [ignoreText, setIgnoreText] = useState("");
  const [vocabText, setVocabText] = useState("");
  const [mDownloading, setMDownloading] = useState(false);
  const [sDownloading, setSDownloading] = useState(false);
  const [pDownloading, setPDownloading] = useState(false);
  const [probeMsg, setProbeMsg] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);

  async function runCaptureProbe() {
    setProbing(true);
    setProbeMsg("Recording 8 seconds — talk, and play some audio…");
    try {
      const r = (await api.meetingCaptureProbe(8)) as unknown as {
        me?: { seconds: number; rms: number };
        them?: { seconds: number; rms: number };
        tap_supported?: boolean;
      };
      const fmt = (c?: { seconds: number; rms: number }) =>
        !c || c.seconds < 0.5 ? "no audio ✗" : c.rms > 0.002 ? "signal ✓" : "captured, but silent";
      setProbeMsg(
        `Mic: ${fmt(r.me)} · System audio: ${fmt(r.them)}` +
          (r.them && r.them.seconds < 0.5
            ? " — check System Settings → Privacy & Security → Screen & System Audio Recording"
            : "")
      );
    } catch (e) {
      setProbeMsg(String(e));
    } finally {
      setProbing(false);
    }
  }

  async function saveMcfg(next: MeetingsCfg) {
    setMcfg(next);
    try {
      await api.meetingsSettingsSet(next);
    } catch {
      /* re-read on next open */
    }
  }

  async function downloadMeetingModel() {
    setMDownloading(true);
    try {
      await api.downloadMeetingModel();
      setMModel(await api.meetingModelStatus());
    } catch {
      /* status stays; user can retry */
    } finally {
      setMDownloading(false);
    }
  }

  async function saveTemplate() {
    if (!tplDraft || !tplDraft.name.trim() || !tplDraft.prompt.trim()) return;
    setTplBusy(true);
    try {
      await api.meetingTemplateSave(tplDraft.name.trim(), tplDraft.prompt.trim());
      setMTemplates(await api.meetingTemplates());
      setEditTpl(null);
      setTplDraft(null);
    } catch {
      /* keep the draft so nothing is lost; user can retry */
    } finally {
      setTplBusy(false);
    }
  }

  async function deleteTemplate(name: string) {
    setTplBusy(true);
    try {
      await api.meetingTemplateDelete(name);
      setMTemplates(await api.meetingTemplates());
      setEditTpl(null);
      setTplDraft(null);
    } catch {
      /* builtins can't be deleted; nothing to do */
    } finally {
      setTplBusy(false);
    }
  }

  async function downloadSpeakerModel() {
    setSDownloading(true);
    try {
      await api.downloadSpeakerModel();
      setMModel(await api.meetingModelStatus());
    } catch {
      /* status stays; user can retry */
    } finally {
      setSDownloading(false);
    }
  }

  async function downloadParakeet() {
    setPDownloading(true);
    try {
      await api.downloadParakeetModel();
      setMModel(await api.meetingModelStatus());
      // Downloading it is the intent to use it — switch the engine over.
      if (mcfg) await saveMcfg({ ...mcfg, asr_engine: "parakeet" });
    } catch {
      /* partial downloads resume on retry */
    } finally {
      setPDownloading(false);
    }
  }

  useEffect(() => {
    api.getProviderSettings().then((cfg) => {
      setS(cfg);
      setMode(cfg.mode);
      setTextModel(cfg.gemini_text_model);
      setVisionModel(cfg.gemini_vision_model);
      setCloudProvider(cfg.cloud_provider ?? "gemini");
      setOpenaiBase(cfg.openai_base_url ?? "");
      setOpenaiText(cfg.openai_text_model ?? "");
      setOpenaiVision(cfg.openai_vision_model ?? "");
      setAnthropicText(cfg.anthropic_text_model ?? "");
      setAnthropicVision(cfg.anthropic_vision_model ?? "");
      setLocalTextModel(cfg.text_model ?? "");
      setLocalVisionModel(cfg.vision_model ?? "");
      // If a key is already stored, verify it's actually live on open so the
      // user sees "Connected" without having to remember to click Test.
      const hasActiveKey =
        (cfg.cloud_provider === "openai" && cfg.has_openai_key) ||
        (cfg.cloud_provider === "anthropic" && cfg.has_anthropic_key) ||
        ((cfg.cloud_provider ?? "gemini") === "gemini" && cfg.has_gemini_key);
      if (cfg.mode === "balanced" && hasActiveKey) {
        // Pass overrides: this closure captured first-render state, and
        // saving with a stale mode would silently flip Balanced off.
        checkConnection({ mode: cfg.mode, cloud_provider: cfg.cloud_provider ?? "gemini" });
      }
    });
    // Pulled Ollama models feed the local-model dropdowns; Ollama being down
    // just leaves the current values as the only options.
    api.health().then((h) => setInstalledModels(h.models)).catch(() => {});
    api.gcalAuthStatus().then(setGcal);
    api.brainListVaults().then(setVaults).catch(() => {});
    api.brainGetAuto().then(setAutoProp).catch(() => {});
    api
      .meetingsSettingsGet()
      .then((c) => {
        setMcfg(c);
        setIgnoreText(c.ignore_bundles.join(", "));
        setVocabText((c.vocabulary ?? []).join(", "));
      })
      .catch(() => {});
    api.meetingModelStatus().then(setMModel).catch(() => {});
    api.meetingTemplates().then(setMTemplates).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function toggleAutoProp(on: boolean) {
    setAutoProp(on);
    try {
      await api.brainSetAuto(on);
    } catch {
      /* revert handled by next open */
    }
  }

  function reloadVaults() {
    api.brainListVaults().then(setVaults).catch(() => {});
  }
  async function addVault() {
    if (!vaultPath.trim()) {
      setVaultMsg("Type the vault's folder path into the field above first — e.g. /Users/edison/Brain/work");
      return;
    }
    setVaultBusy("adding");
    setVaultMsg(null);
    try {
      setVaults(await api.brainAddVault(vaultPath.trim()));
      setVaultPath("");
    } catch (e) {
      setVaultMsg(String(e));
    } finally {
      setVaultBusy("");
    }
  }
  async function removeVault(v: string) {
    setVaultBusy("rm:" + v);
    try {
      await api.brainRemoveVault(v);
      reloadVaults();
    } finally {
      setVaultBusy("");
    }
  }
  async function syncVault(v?: string) {
    setVaultBusy(v ? "sync:" + v : "sync:all");
    setVaultMsg(null);
    try {
      const reports = await api.brainSync(v);
      const imported = reports.reduce((s, r) => s + r.imported, 0);
      setVaultMsg(`Synced — ${imported} note(s) updated.`);
      reloadVaults();
    } catch (e) {
      setVaultMsg(String(e));
    } finally {
      setVaultBusy("");
    }
  }

  // Save the OAuth client (id + secret) so Connect can run.
  async function saveGcalClient() {
    if (!clientId.trim() || !clientSecret.trim()) return;
    setGcalBusy("saving");
    setGcalMsg(null);
    try {
      await api.gcalSetClient(clientId.trim(), clientSecret.trim());
      setGcal(await api.gcalAuthStatus());
      setClientSecret(""); // don't keep the secret in component state
      setGcalMsg({ kind: "ok", text: "Credentials saved — now click Connect." });
    } catch (e) {
      setGcalMsg({ kind: "err", text: String(e) });
    } finally {
      setGcalBusy("");
    }
  }

  // Run the OAuth consent flow (opens the browser, catches the loopback redirect).
  // One OAuth run per account: each click opens Google's account picker, so
  // adding a second (work) account is just running this again.
  async function connectGcal() {
    setGcalBusy("connecting");
    setGcalMsg(null);
    try {
      const st = await api.gcalBeginAuth();
      setGcal(st);
      setGcalMsg(
        st.connected
          ? { kind: "ok", text: "Connected to Google Calendar." }
          : { kind: "err", text: "Not connected — please try again." }
      );
    } catch (e) {
      setGcalMsg({ kind: "err", text: String(e) });
    } finally {
      setGcalBusy("");
    }
  }

  async function removeGcalAccount(email: string) {
    try {
      setGcal(await api.gcalRemoveAccount(email));
      setGcalMsg(null);
    } catch (e) {
      setGcalMsg({ kind: "err", text: String(e) });
    }
  }

  async function setGcalSyncAccount(email: string) {
    try {
      setGcal(await api.gcalSetSyncAccount(email));
      setGcalMsg(null);
    } catch (e) {
      setGcalMsg({ kind: "err", text: String(e) });
    }
  }

  // Every provider field the UI edits, in one payload. Blank fields map to
  // undefined = "leave unchanged" on the backend.
  function settingsPayload(overrides?: { mode?: ProviderMode; cloud_provider?: CloudProvider }) {
    return {
      mode: overrides?.mode ?? mode,
      cloud_provider: overrides?.cloud_provider ?? cloudProvider,
      // only send a key if the user typed a new one (blank = leave as-is)
      gemini_api_key: key.trim() ? key.trim() : undefined,
      gemini_text_model: textModel.trim() || undefined,
      gemini_vision_model: visionModel.trim() || undefined,
      openai_base_url: openaiBase.trim() || undefined,
      openai_api_key: openaiKey.trim() ? openaiKey.trim() : undefined,
      openai_text_model: openaiText.trim() || undefined,
      openai_vision_model: openaiVision.trim() || undefined,
      anthropic_api_key: anthropicKey.trim() ? anthropicKey.trim() : undefined,
      anthropic_text_model: anthropicText.trim() || undefined,
      anthropic_vision_model: anthropicVision.trim() || undefined,
      text_model: localTextModel.trim() || undefined,
      vision_model: localVisionModel.trim() || undefined,
    };
  }

  // Persist the current fields, then hit the active cloud provider and reflect
  // the real result in the status badge. Shared by Save, Test, and the on-open
  // auto-check (which passes overrides — first-render state is stale there).
  async function checkConnection(overrides?: { mode?: ProviderMode; cloud_provider?: CloudProvider }) {
    const active = overrides?.cloud_provider ?? cloudProvider;
    setConn({ state: "checking" });
    try {
      await api.setProviderSettings(settingsPayload(overrides));
      const msg = await api.testProvider();
      setConn({ state: "ok", msg });
      setS((prev) =>
        prev
          ? {
              ...prev,
              has_gemini_key: active === "gemini" ? true : prev.has_gemini_key,
              has_openai_key: active === "openai" ? true : prev.has_openai_key,
              has_anthropic_key: active === "anthropic" ? true : prev.has_anthropic_key,
            }
          : prev
      );
    } catch (e) {
      setConn({ state: "err", msg: String(e) });
    }
  }

  async function save() {
    setSaving(true);
    try {
      await api.setProviderSettings(settingsPayload());
      // Confirm the save landed against a working connection before leaving —
      // surfaces a bad key instead of closing on a silent failure.
      if (mode === "balanced" && hasKey) {
        await checkConnection();
      } else if (page) {
        setSavedHint(true);
        window.setTimeout(() => setSavedHint(false), 2000);
      } else {
        onClose();
      }
    } finally {
      setSaving(false);
    }
  }

  // Explicitly clear the stored key for the active provider (blank field =
  // "keep", so removal needs its own action). Sends "" = delete from Keychain.
  async function removeKey() {
    await api.setProviderSettings({
      mode,
      gemini_api_key: cloudProvider === "gemini" ? "" : undefined,
      openai_api_key: cloudProvider === "openai" ? "" : undefined,
      anthropic_api_key: cloudProvider === "anthropic" ? "" : undefined,
    });
    if (cloudProvider === "gemini") setKey("");
    if (cloudProvider === "openai") setOpenaiKey("");
    if (cloudProvider === "anthropic") setAnthropicKey("");
    setConn({ state: "idle" });
    setS((prev) =>
      prev
        ? {
            ...prev,
            has_gemini_key: cloudProvider === "gemini" ? false : prev.has_gemini_key,
            has_openai_key: cloudProvider === "openai" ? false : prev.has_openai_key,
            has_anthropic_key: cloudProvider === "anthropic" ? false : prev.has_anthropic_key,
          }
        : prev
    );
  }

  const hasKey =
    cloudProvider === "openai"
      ? Boolean(s?.has_openai_key) || openaiKey.trim().length > 0
      : cloudProvider === "anthropic"
        ? Boolean(s?.has_anthropic_key) || anthropicKey.trim().length > 0
        : Boolean(s?.has_gemini_key) || key.trim().length > 0;
  const testing = conn.state === "checking";

  const sections: [SettingsSection, string][] = [
    ["models", "Models"],
    ["themes", "Themes"],
    ["calendar", "Google Calendar"],
    ["vaults", "Brain vaults"],
    ["meetings", "Meetings"],
  ];

  const inner = (
    <div className="settings-layout">
      <nav className="settings-nav">
        {sections.map(([id, label]) => (
          <button key={id} className={section === id ? "on" : ""} onClick={() => setSection(id)}>
            {label}
          </button>
        ))}
      </nav>
      <div className="settings-body">
        {section === "models" && (
          <>
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

        <div className="settings-fields">
          <div className="field-row">
            <label className="field">
              <span className="field-label">Local text model</span>
              <select value={localTextModel} onChange={(e) => setLocalTextModel(e.target.value)}>
                {[...new Set([localTextModel, ...installedModels])]
                  .filter(Boolean)
                  .map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
              </select>
            </label>
            <label className="field">
              <span className="field-label">Local vision model</span>
              <select value={localVisionModel} onChange={(e) => setLocalVisionModel(e.target.value)}>
                {[...new Set([localVisionModel, ...installedModels])]
                  .filter(Boolean)
                  .map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
              </select>
            </label>
          </div>
          <span className="field-hint">
            Any Ollama model you've pulled works (recommended: qwen2.5:7b-instruct + qwen2.5vl:7b).
            Embeddings stay on nomic-embed-text — the search index is built with it.
          </span>
        </div>

        {mode === "balanced" && (
          <div className="settings-fields">
            <div className={"conn-status " + conn.state}>
              {conn.state === "checking" && <Loader2 size={13} className="spin" />}
              {conn.state === "ok" && <Wifi size={13} />}
              {conn.state === "err" && <WifiOff size={13} />}
              {conn.state === "idle" && <WifiOff size={13} />}
              <span className="conn-label">
                {conn.state === "checking" && "Checking connection…"}
                {conn.state === "ok" && (conn.msg || "Connected")}
                {conn.state === "err" && "Couldn’t connect"}
                {conn.state === "idle" &&
                  (hasKey ? "Not tested yet" : "Add a key to connect")}
              </span>
            </div>
            {conn.state === "err" && <div className="conn-detail">{conn.msg}</div>}

            <label className="field">
              <span className="field-label">Cloud provider</span>
              <select
                value={cloudProvider}
                onChange={(e) => {
                  setCloudProvider(e.target.value as CloudProvider);
                  setConn({ state: "idle" });
                }}
              >
                <option value="gemini">Google Gemini</option>
                <option value="anthropic">Anthropic (Claude)</option>
                <option value="openai">OpenAI-compatible (OpenAI, OpenRouter, LM Studio…)</option>
              </select>
            </label>

            {cloudProvider === "gemini" && (
              <>
                <label className="field">
                  <span className="field-label">
                    Gemini API key{" "}
                    {s?.has_gemini_key && (
                      <button type="button" className="field-clear" onClick={removeKey}>
                        remove
                      </button>
                    )}
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
              </>
            )}

            {cloudProvider === "anthropic" && (
              <>
                <label className="field">
                  <span className="field-label">
                    Anthropic API key{" "}
                    {s?.has_anthropic_key && (
                      <button type="button" className="field-clear" onClick={removeKey}>
                        remove
                      </button>
                    )}
                  </span>
                  <input
                    type="password"
                    placeholder={s?.has_anthropic_key ? "•••••••• (leave blank to keep)" : "sk-ant-…"}
                    value={anthropicKey}
                    onChange={(e) => setAnthropicKey(e.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <span className="field-hint">
                    Stored in your macOS Keychain — never written to disk.{" "}
                    <a href="https://platform.claude.com" target="_blank" rel="noreferrer">
                      Get a key
                    </a>
                  </span>
                </label>
                <div className="field-row">
                  <label className="field">
                    <span className="field-label">Extract model</span>
                    <input
                      value={anthropicText}
                      onChange={(e) => setAnthropicText(e.target.value)}
                      spellCheck={false}
                    />
                  </label>
                  <label className="field">
                    <span className="field-label">OCR model</span>
                    <input
                      value={anthropicVision}
                      onChange={(e) => setAnthropicVision(e.target.value)}
                      spellCheck={false}
                    />
                  </label>
                </div>
              </>
            )}

            {cloudProvider === "openai" && (
              <>
                <label className="field">
                  <span className="field-label">Base URL</span>
                  <input
                    value={openaiBase}
                    onChange={(e) => setOpenaiBase(e.target.value)}
                    placeholder="https://api.openai.com/v1"
                    spellCheck={false}
                    autoComplete="off"
                  />
                  <span className="field-hint">
                    Any OpenAI-compatible endpoint: api.openai.com, OpenRouter, LM Studio,
                    llama.cpp server, vLLM…
                  </span>
                </label>
                <label className="field">
                  <span className="field-label">
                    API key{" "}
                    {s?.has_openai_key && (
                      <button type="button" className="field-clear" onClick={removeKey}>
                        remove
                      </button>
                    )}
                  </span>
                  <input
                    type="password"
                    placeholder={s?.has_openai_key ? "•••••••• (leave blank to keep)" : "sk-…"}
                    value={openaiKey}
                    onChange={(e) => setOpenaiKey(e.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <span className="field-hint">Stored in your macOS Keychain — never written to disk.</span>
                </label>
                <div className="field-row">
                  <label className="field">
                    <span className="field-label">Extract model</span>
                    <input value={openaiText} onChange={(e) => setOpenaiText(e.target.value)} spellCheck={false} />
                  </label>
                  <label className="field">
                    <span className="field-label">OCR model</span>
                    <input
                      value={openaiVision}
                      onChange={(e) => setOpenaiVision(e.target.value)}
                      spellCheck={false}
                    />
                  </label>
                </div>
              </>
            )}

            <button className="ghost-btn test-btn" onClick={() => checkConnection()} disabled={testing || !hasKey}>
              {testing ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
              Test connection
            </button>
          </div>
        )}

        <div className="settings-actions">
          {savedHint && (
            <span className="field-hint">
              <Check size={13} /> Saved
            </span>
          )}
          {!page && (
            <button className="ghost-btn" onClick={onClose}>
              Cancel
            </button>
          )}
          <button className="primary" onClick={save} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
          </>
        )}

        {section === "themes" && <ThemesSettings />}

        {section === "calendar" && (
          <>
        <h3>Google Calendar</h3>
        <p className="settings-sub">
          Connect one or more Google accounts (work + personal) and the Calendar view consolidates
          every calendar in one place. Your daily schedule also pushes one-way into a dedicated
          “noted” calendar in the first account — other calendars are never touched by the sync.
        </p>

        <div className="settings-fields">
          {gcalMsg && (
            <div className={gcalMsg.kind === "err" ? "conn-detail" : "field-hint"}>{gcalMsg.text}</div>
          )}

          {(gcal?.accounts ?? []).length > 0 && (
            <div className="gcal-accounts">
              {gcal!.accounts.map((a) => (
                <div className="gcal-account" key={a.email}>
                  {a.connected ? (
                    <CalendarCheck size={14} className="gcal-acct-ok" />
                  ) : (
                    <CalendarX size={14} className="gcal-acct-bad" />
                  )}
                  <span className="gcal-acct-email">{a.email}</span>
                  {!a.connected && <span className="gcal-acct-warn">reconnect needed</span>}
                  {a.email === gcal!.sync_account ? (
                    <span className="gcal-acct-sync" title="Your daily schedule pushes into the 'noted' calendar in this account">
                      schedule syncs here
                    </span>
                  ) : (
                    a.connected && (
                      <button
                        className="gcal-acct-synchere"
                        onClick={() => setGcalSyncAccount(a.email)}
                        title="Push the daily schedule into this account instead"
                      >
                        sync here
                      </button>
                    )
                  )}
                  <button
                    className="gcal-acct-x"
                    onClick={() => removeGcalAccount(a.email)}
                    title={`Remove ${a.email}`}
                    aria-label={`Remove ${a.email}`}
                  >
                    <X size={13} />
                  </button>
                </div>
              ))}
              <span className="field-hint">
                The daily schedule pushes one-way into a calendar named “noted” inside the account
                marked above — never into your real calendars. Choose which calendars show up from
                the filter inside the Calendar view. To reconnect an expired account, just add it
                again.
              </span>
            </div>
          )}

          {(!gcal?.has_client || (gcal?.accounts ?? []).length === 0) && (
            <>
              <label className="field">
                <span className="field-label">OAuth client ID</span>
                <input
                  placeholder={gcal?.has_client ? "•••• (saved)" : "…apps.googleusercontent.com"}
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span className="field-label">OAuth client secret</span>
                <input
                  type="password"
                  placeholder={gcal?.has_client ? "•••••••• (leave blank to keep)" : "GOCSPX-…"}
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                />
                <span className="field-hint">
                  Create a “Desktop app” OAuth client in the{" "}
                  <a
                    href="https://console.cloud.google.com/apis/credentials"
                    target="_blank"
                    rel="noreferrer"
                  >
                    Google Cloud Console
                  </a>{" "}
                  (enable the Calendar API). Add each Google account you’ll connect as a Test user
                  on the consent screen, or you’ll need to reconnect weekly. Secret is stored in
                  your macOS Keychain — never on disk. One client works for all your accounts.
                </span>
              </label>

              <button
                className="ghost-btn test-btn"
                onClick={saveGcalClient}
                disabled={gcalBusy !== "" || !clientId.trim() || !clientSecret.trim()}
              >
                {gcalBusy === "saving" ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
                Save credentials
              </button>
            </>
          )}

          <button
            className={(gcal?.accounts ?? []).length === 0 ? "primary" : "ghost-btn test-btn"}
            onClick={connectGcal}
            disabled={gcalBusy !== "" || !gcal?.has_client}
          >
            {gcalBusy === "connecting" ? (
              <>
                <Loader2 size={14} className="spin" /> Waiting for Google…
              </>
            ) : (gcal?.accounts ?? []).length === 0 ? (
              "Connect Google account"
            ) : (
              "Add another account"
            )}
          </button>
        </div>

          </>
        )}

        {section === "vaults" && (
          <>
        <h3>Brain vaults</h3>
        <p className="settings-sub">
          Obsidian vaults under <code>~/Brain</code> sync into your knowledge graph (visible in
          Knowledge → Work). Work vaults import one-way; the personal vault is generated by noted.
          noted only writes inside a managed block, and every write is a git commit you can revert.
        </p>

        <div className="settings-fields">
          <label className="vault-auto">
            <input
              type="checkbox"
              checked={autoProp}
              onChange={(e) => toggleAutoProp(e.target.checked)}
            />
            <span>
              Auto-propagate
              <em>
                Every 10 min, write captures back into your vaults and refresh the personal vault
                (git-committed). Import + embed always run regardless.
              </em>
            </span>
          </label>
          {vaults.length === 0 && <div className="field-hint">No vaults registered.</div>}
          {vaults.map((v) => (
            <div className="vault-row" key={v.vault}>
              <div className="vault-id">
                <span className="vault-name">{v.vault}</span>
                <span className="vault-meta">
                  {v.direction} · {v.note_count} notes · {v.entity_count} entities
                  {v.last_synced_at ? ` · ${v.last_synced_at.slice(0, 10)}` : ""}
                </span>
              </div>
              <button
                className="ghost-btn vault-sync"
                onClick={() => syncVault(v.vault)}
                disabled={vaultBusy !== ""}
                title="Sync this vault now"
              >
                {vaultBusy === "sync:" + v.vault ? (
                  <Loader2 size={13} className="spin" />
                ) : (
                  <RefreshCw size={13} />
                )}
              </button>
              <button
                className="icon-btn"
                onClick={() => removeVault(v.vault)}
                disabled={vaultBusy !== ""}
                title="Stop tracking this vault"
              >
                <Trash2 size={14} />
              </button>
            </div>
          ))}
          {vaultMsg && <div className="field-hint">{vaultMsg}</div>}

          <label className="field">
            <span className="field-label">Add a vault (folder path)</span>
            <input
              placeholder="/Users/you/Brain/another-vault"
              value={vaultPath}
              onChange={(e) => setVaultPath(e.target.value)}
              spellCheck={false}
              autoComplete="off"
            />
          </label>
          <div className="field-row">
            <button
              className="ghost-btn test-btn"
              onClick={addVault}
              disabled={vaultBusy !== ""}
            >
              {vaultBusy === "adding" ? <Loader2 size={14} className="spin" /> : <FolderPlus size={14} />}
              Add vault
            </button>
            <button className="ghost-btn" onClick={() => syncVault(undefined)} disabled={vaultBusy !== ""}>
              {vaultBusy === "sync:all" ? <Loader2 size={14} className="spin" /> : <RefreshCw size={14} />}
              Sync all
            </button>
          </div>
        </div>

          </>
        )}

        {section === "meetings" && (
          <>
        <h3>Meetings</h3>
        <p className="settings-sub">
          noted records meetings bot-free: your mic + system audio, transcribed and summarized
          100% on this Mac. Nothing is captured unless you accept a prompt or hit Record.
        </p>

        <div className="settings-fields">
          {mModel && !mModel.tap_supported && (
            <div className="field-hint">
              System-audio capture needs macOS 14.4+ — recordings here would be mic-only.
            </div>
          )}
          <label className="vault-auto">
            <input
              type="checkbox"
              checked={mcfg?.auto_prompt ?? true}
              onChange={(e) => mcfg && saveMcfg({ ...mcfg, auto_prompt: e.target.checked })}
            />
            <span>
              Offer to record meetings
              <em>
                A small prompt appears 1 minute before calendar meetings and when a call app
                starts using your microphone. Ignoring it records nothing.
              </em>
            </span>
          </label>
          <label className="vault-auto">
            <input
              type="checkbox"
              checked={mcfg?.retain_audio ?? true}
              onChange={(e) => mcfg && saveMcfg({ ...mcfg, retain_audio: e.target.checked })}
            />
            <span>
              Keep audio recordings
              <em>
                Store each meeting's audio locally so you can verify the transcript later.
                Off = transcribe-and-discard, like Granola.
              </em>
            </span>
          </label>
          <label className="field">
            <span className="field-label">Default summary template</span>
            <select
              value={mcfg?.default_template ?? "Meeting"}
              onChange={(e) => mcfg && saveMcfg({ ...mcfg, default_template: e.target.value })}
            >
              {(mTemplates.length ? mTemplates : [{ name: "Meeting", prompt: "", builtin: true }]).map(
                (t) => (
                  <option key={t.name} value={t.name}>
                    {t.name}
                  </option>
                )
              )}
            </select>
          </label>
          <div className="field">
            <span className="field-label">Summary templates</span>
            <div className="tpl-list">
              {mTemplates.map((t) => (
                <div key={t.name} className="tpl-row">
                  <button
                    className="tpl-head"
                    onClick={() => {
                      if (editTpl === t.name) {
                        setEditTpl(null);
                        setTplDraft(null);
                      } else {
                        setEditTpl(t.name);
                        setTplDraft({ name: t.name, prompt: t.prompt });
                      }
                    }}
                  >
                    {t.name}
                    {t.builtin && <em>built-in</em>}
                  </button>
                  {editTpl === t.name && tplDraft && (
                    <div className="tpl-editor">
                      <textarea
                        value={tplDraft.prompt}
                        readOnly={t.builtin}
                        rows={5}
                        spellCheck={false}
                        onChange={(e) => setTplDraft({ ...tplDraft, prompt: e.target.value })}
                      />
                      <div className="tpl-actions">
                        {t.builtin ? (
                          <>
                            <span className="field-hint">
                              Built-in templates reset on launch — duplicate to customize.
                            </span>
                            <button
                              className="ghost-btn"
                              onClick={() => {
                                setEditTpl(null);
                                setTplDraft({ name: `${t.name} (mine)`, prompt: t.prompt });
                              }}
                            >
                              Duplicate & edit
                            </button>
                          </>
                        ) : (
                          <>
                            <button className="ghost-btn" onClick={saveTemplate} disabled={tplBusy}>
                              {tplBusy ? <Loader2 size={13} className="spin" /> : <Check size={13} />} Save
                            </button>
                            <button
                              className="ghost-btn danger"
                              onClick={() => deleteTemplate(t.name)}
                              disabled={tplBusy}
                            >
                              <Trash2 size={13} /> Delete
                            </button>
                          </>
                        )}
                      </div>
                    </div>
                  )}
                </div>
              ))}
              {tplDraft && editTpl === null ? (
                <div className="tpl-editor">
                  <input
                    placeholder="Template name"
                    value={tplDraft.name}
                    autoFocus
                    onChange={(e) => setTplDraft({ ...tplDraft, name: e.target.value })}
                  />
                  <textarea
                    placeholder="Describe the sections to produce, in order — e.g. 'Summary' — one paragraph. 'Decisions' — tight bullets…"
                    value={tplDraft.prompt}
                    rows={5}
                    spellCheck={false}
                    onChange={(e) => setTplDraft({ ...tplDraft, prompt: e.target.value })}
                  />
                  <div className="tpl-actions">
                    <button
                      className="ghost-btn"
                      onClick={saveTemplate}
                      disabled={tplBusy || !tplDraft.name.trim() || !tplDraft.prompt.trim()}
                    >
                      {tplBusy ? <Loader2 size={13} className="spin" /> : <Check size={13} />} Save
                    </button>
                    <button className="ghost-btn" onClick={() => setTplDraft(null)}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <button
                  className="ghost-btn tpl-new"
                  onClick={() => {
                    setEditTpl(null);
                    setTplDraft({ name: "", prompt: "" });
                  }}
                >
                  <Plus size={13} /> New template
                </button>
              )}
            </div>
          </div>
          <label className="field">
            <span className="field-label">
              Custom vocabulary — names and jargon the transcriber mishears (comma-separated)
            </span>
            <input
              value={vocabText}
              onChange={(e) => setVocabText(e.target.value)}
              onBlur={() =>
                mcfg &&
                saveMcfg({
                  ...mcfg,
                  vocabulary: vocabText
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean),
                })
              }
              placeholder="a16z, Anthropic, Tauri, SOC 2"
              spellCheck={false}
              autoComplete="off"
            />
          </label>
          <label className="field">
            <span className="field-label">
              Never prompt for these apps (comma-separated bundle-id fragments)
            </span>
            <input
              value={ignoreText}
              onChange={(e) => setIgnoreText(e.target.value)}
              onBlur={() =>
                mcfg &&
                saveMcfg({
                  ...mcfg,
                  ignore_bundles: ignoreText
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean),
                })
              }
              spellCheck={false}
              autoComplete="off"
            />
          </label>
          <div className="field-row">
            {mModel?.turbo ? (
              <span className="field-hint">
                <Check size={13} /> Meeting model ready (whisper large-v3-turbo)
              </span>
            ) : (
              <button
                className="ghost-btn test-btn"
                onClick={downloadMeetingModel}
                disabled={mDownloading}
              >
                {mDownloading ? <Loader2 size={14} className="spin" /> : <Download size={14} />}
                {mDownloading
                  ? "Downloading (1.6 GB)…"
                  : mModel?.base
                    ? "Upgrade meeting transcription (1.6 GB)"
                    : "Download meeting model (1.6 GB)"}
              </button>
            )}
            <button
              className="ghost-btn"
              onClick={runCaptureProbe}
              disabled={probing}
              title="Record 8s of mic + system audio — triggers the macOS permission prompts on first use"
            >
              {probing ? <Loader2 size={14} className="spin" /> : <Mic size={14} />} Test capture
            </button>
          </div>
          <div className="field-row">
            {mModel?.speaker ? (
              <span className="field-hint">
                <Check size={13} /> Speaker ID ready — transcripts label who's speaking
              </span>
            ) : (
              <button
                className="ghost-btn test-btn"
                onClick={downloadSpeakerModel}
                disabled={sDownloading}
                title="Voice-embedding model that tells call participants apart (labels appear when the meeting ends)"
              >
                {sDownloading ? <Loader2 size={14} className="spin" /> : <Download size={14} />}
                {sDownloading ? "Downloading (29 MB)…" : "Download speaker ID model (29 MB)"}
              </button>
            )}
          </div>
          <label className="field">
            <span className="field-label">Transcription engine</span>
            <select
              value={mcfg?.asr_engine ?? "whisper"}
              onChange={(e) =>
                mcfg &&
                saveMcfg({ ...mcfg, asr_engine: e.target.value as "whisper" | "parakeet" })
              }
            >
              <option value="whisper">Whisper</option>
              <option value="parakeet" disabled={!mModel?.parakeet}>
                Parakeet — faster, better with names
                {mModel?.parakeet ? "" : " (download below)"}
              </option>
            </select>
          </label>
          <div className="field-row">
            {mModel?.parakeet ? (
              <span className="field-hint">
                <Check size={13} /> Parakeet ready (NVIDIA Parakeet-TDT 0.6B)
              </span>
            ) : (
              <button
                className="ghost-btn test-btn"
                onClick={downloadParakeet}
                disabled={pDownloading}
                title="NVIDIA Parakeet-TDT 0.6B — noticeably faster than whisper large-v3-turbo and stronger on names and jargon. English only."
              >
                {pDownloading ? <Loader2 size={14} className="spin" /> : <Download size={14} />}
                {pDownloading ? "Downloading (660 MB)…" : "Download Parakeet engine (660 MB)"}
              </button>
            )}
          </div>
          {probeMsg && <div className="field-hint">{probeMsg}</div>}
        </div>
          </>
        )}
      </div>
    </div>
  );

  if (page) {
    return (
      <section className="settings-page">
        <h2 className="settings-title">Settings</h2>
        {inner}
      </section>
    );
  }
  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
        <button className="icon-btn modal-close" onClick={onClose} aria-label="Close">
          <X size={16} />
        </button>
        {inner}
      </div>
    </div>
  );
}
