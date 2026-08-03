import { useEffect, useState } from "react";
import { X, Check, Loader2, Wifi, WifiOff, CalendarCheck, CalendarX, Download, Mic, Plus, RefreshCw, Trash2, FolderPlus, Laptop, Gauge, Cloud, KeyRound } from "lucide-react";
import { api, type BrainVaultStatus, type ByokConfig, type CloudProvider, type GcalStatus, type MeetingsCfg, type MeetingModelStatus, type MeetingTemplate, type ProviderId, type ProviderMode, type ProviderSettings } from "./api";
import { ThemesSettings } from "./ThemesSettings";
import { TranscriptVocabularySettings } from "./TranscriptVocabularySettings";
import { releaseProfile } from "./releaseProfile";

// Live connection status, shown as a persistent badge so "is Gemini actually
// reachable?" is never a mystery — checked on open and after every save/test.
type Conn =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "ok"; msg: string }
  | { state: "err"; msg: string };

type SettingsSection = "models" | "themes" | "calendar" | "vaults" | "meetings";

// Model-provider settings. noted runs 100% local by default; the internally
// named "balanced" mode sends only new captures to a cloud extract/OCR model.
// Storage, embeddings, chat, meetings, and Brain Vault stay local.
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
  const [byok, setByok] = useState<ByokConfig | null>(null);
  const [groqKey, setGroqKey] = useState("");
  const [compatibleKey, setCompatibleKey] = useState("");
  const [discoveredModels, setDiscoveredModels] = useState<Record<string, string[]>>({});
  const [discovering, setDiscovering] = useState("");

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
  const [videoPermissionBusy, setVideoPermissionBusy] = useState(false);
  const [videoPermissionMsg, setVideoPermissionMsg] = useState<string | null>(null);

  async function runCaptureProbe() {
    setProbing(true);
    setProbeMsg("Recording 8 seconds — talk, and play some audio…");
    try {
      const r = (await api.meetingCaptureProbe(8)) as unknown as {
        me?: { seconds: number; duration_ratio: number; rms: number };
        them?: { seconds: number; duration_ratio: number; rms: number };
        tap_supported?: boolean;
      };
      const fmt = (c?: { seconds: number; duration_ratio: number; rms: number }) =>
        !c || c.seconds < 0.5
          ? "no audio ✗"
          : c.duration_ratio > 1.25
            ? `format error ✗ (${c.seconds.toFixed(1)}s captured in 8s)`
            : c.rms > 0.002
              ? "signal ✓"
              : "captured, but silent";
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

  async function requestVideoPermission() {
    setVideoPermissionBusy(true);
    setVideoPermissionMsg(null);
    try {
      const granted = await api.meetingVideoRequestPermission();
      const status = await api.meetingModelStatus();
      setMModel(status);
      setVideoPermissionMsg(
        granted || status.video_authorized
          ? "Video permission is ready."
          : "macOS did not grant access. Enable noted in Privacy & Security → Screen & System Audio Recording, then reopen the app."
      );
    } catch (e) {
      setVideoPermissionMsg(String(e));
    } finally {
      setVideoPermissionBusy(false);
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
      setByok(cfg.byok);
      if (cfg.mode === "hosted" && !cfg.has_hosted_key) {
        setConn({
          state: "err",
          msg: "Hosted activation is missing from macOS Keychain. Choose Local and save, or restore activation before using Hosted.",
        });
      }
      // If a key is already stored, verify it's actually live on open so the
      // user sees "Connected" without having to remember to click Test.
      const hasActiveKey =
        (cfg.cloud_provider === "openai" && cfg.has_openai_key) ||
        (cfg.cloud_provider === "anthropic" && cfg.has_anthropic_key) ||
        ((cfg.cloud_provider ?? "gemini") === "gemini" && cfg.has_gemini_key);
      if ((cfg.mode === "balanced" && hasActiveKey) || (cfg.mode === "hosted" && cfg.has_hosted_key)) {
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
      if (active === "gemini") setKey("");
      if (active === "openai") setOpenaiKey("");
      if (active === "anthropic") setAnthropicKey("");
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
      if (mode === "byok" && byok) {
        setConn({ state: "checking" });
        const results = await api.testByokSettings(byok, {
          openaiApiKey: openaiKey.trim() || undefined,
          geminiApiKey: key.trim() || undefined,
          anthropicApiKey: anthropicKey.trim() || undefined,
          groqApiKey: groqKey.trim() || undefined,
          openaiCompatibleApiKey: compatibleKey.trim() || undefined,
        });
        await api.setProviderSettings(settingsPayload({ mode: s?.mode ?? "local" }));
        try {
          await api.setByokSettings(byok, groqKey.trim() || undefined, compatibleKey.trim() || undefined);
        } catch (e) {
          if (!String(e).includes("EMBEDDING_REBUILD_REQUIRED") ||
              !window.confirm("Changing embedding models requires rebuilding semantic search. Your notes stay intact. Rebuild now?")) throw e;
          await api.setByokSettings(byok, groqKey.trim() || undefined, compatibleKey.trim() || undefined, true);
          await api.reindex();
        }
        setConn({ state: "ok", msg: Object.entries(results).map(([cap, result]) => `${cap}: ${result}`).join(" · ") });
        setOpenaiKey("");
        setKey("");
        setAnthropicKey("");
        setGroqKey("");
        setCompatibleKey("");
        setS(await api.getProviderSettings());
      } else {
        try {
          await api.setProviderSettings(settingsPayload());
        } catch (e) {
          if (!String(e).includes("EMBEDDING_REBUILD_REQUIRED") ||
              !window.confirm("Changing model profiles requires rebuilding semantic search. Your notes stay intact. Rebuild now?")) throw e;
          await api.setProviderSettings({ ...settingsPayload(), confirm_embedding_rebuild: true });
          await api.reindex();
        }
      }
      // Confirm the save landed against a working connection before leaving —
      // surfaces a bad key instead of closing on a silent failure.
      if ((mode === "balanced" && hasKey) || (mode === "hosted" && s?.has_hosted_key)) {
        await checkConnection();
      } else if (page) {
        setSavedHint(true);
        window.setTimeout(() => setSavedHint(false), 2000);
      } else {
        onClose();
      }
    } catch (e) {
      setConn({ state: "err", msg: String(e) });
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

  async function removeByokKey(providerId: ProviderId) {
    if (!byok) return;
    if (providerId === "groq" || providerId === "openai_compatible") {
      await api.setByokSettings(byok, providerId === "groq" ? "" : undefined, providerId === "openai_compatible" ? "" : undefined);
    } else {
      await api.setProviderSettings({
        mode: "byok",
        openai_api_key: providerId === "openai" ? "" : undefined,
        gemini_api_key: providerId === "gemini" ? "" : undefined,
        anthropic_api_key: providerId === "anthropic" ? "" : undefined,
      });
    }
    setS(await api.getProviderSettings());
  }

  const hasKey =
    cloudProvider === "openai"
      ? Boolean(s?.has_openai_key) || openaiKey.trim().length > 0
      : cloudProvider === "anthropic"
        ? Boolean(s?.has_anthropic_key) || anthropicKey.trim().length > 0
        : Boolean(s?.has_gemini_key) || key.trim().length > 0;
  const testing = conn.state === "checking";
  const isEmbeddingModel = (name: string) => /embed|nomic/i.test(name);
  const isVisionModel = (name: string) => /vl(?=[:_.-]|$)|vision|llava|bakllava|moondream/i.test(name);
  const localTextModels = [...new Set([localTextModel, ...installedModels.filter((name) => !isEmbeddingModel(name) && !isVisionModel(name))])].filter(Boolean);
  const localVisionModels = [...new Set([localVisionModel, ...installedModels.filter((name) => !isEmbeddingModel(name) && isVisionModel(name))])].filter(Boolean);

  const speechEngineField = (label: string) => (
    <label className="field">
      <span className="field-label">{label}</span>
      <select
        value={mcfg?.asr_engine ?? "whisper"}
        onChange={(e) =>
          mcfg &&
          saveMcfg({ ...mcfg, asr_engine: e.target.value as "whisper" | "parakeet" | "hosted" })
        }
      >
        <option value="whisper">
          {mModel?.turbo
            ? "Whisper large-v3-turbo — accuracy-first"
            : mModel?.base
              ? "Whisper Base English — lighter fallback"
              : "Whisper large-v3-turbo — accuracy-first (not installed)"}
        </option>
        <option value="parakeet" disabled={!mModel?.parakeet}>
          Parakeet TDT 0.6B — speed-first, English only
          {mModel?.parakeet ? "" : " (not installed)"}
        </option>
        {releaseProfile.notedHosted && (
          <option value="hosted" disabled={!mModel?.hosted}>
            Hosted Parakeet — cloud transcription
            {mModel?.hosted ? "" : " (activation required)"}
          </option>
        )}
      </select>
      <span className="field-hint">Applies to quick dictation and meeting transcripts.</span>
    </label>
  );

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
          {releaseProfile.notedHosted
            ? "Choose Hosted for the simplest setup, use your own API keys, or keep inference private and local."
            : "Keep inference private and local, or route each capability through your own API keys."}
        </p>

        <div className="provider-profiles" role="radiogroup" aria-label="Model profile">
          <button role="radio" className={"provider-profile" + (mode === "local" ? " on" : "")} onClick={() => setMode("local")} aria-checked={mode === "local"}>
            <span className="provider-profile-icon"><Laptop size={17} /></span>
            <span className="provider-profile-copy"><strong>Local</strong><small>Private on this Mac</small></span>
            {mode === "local" && <Check className="provider-profile-check" size={15} />}
          </button>
          {releaseProfile.balancedInference && (
            <button role="radio" className={"provider-profile" + (mode === "balanced" ? " on" : "")} onClick={() => setMode("balanced")} aria-checked={mode === "balanced"}>
              <span className="provider-profile-icon"><Gauge size={17} /></span>
              <span className="provider-profile-copy"><strong>Cloud-assisted capture</strong><small>Cloud extraction, local library</small></span>
              {mode === "balanced" && <Check className="provider-profile-check" size={15} />}
            </button>
          )}
          {releaseProfile.notedHosted && (
            <button
              className={"provider-profile recommended" + (mode === "hosted" ? " on" : "")}
              onClick={() => setMode("hosted")}
              disabled={!s?.has_hosted_key}
              title={s?.has_hosted_key ? "Use your Noted hosted account" : "Activate a Noted account first"}
              role="radio"
              aria-checked={mode === "hosted"}
            >
              <span className="provider-profile-icon"><Cloud size={17} /></span>
              <span className="provider-profile-copy"><strong>Hosted <em>Recommended</em></strong><small>Everything included</small></span>
              {mode === "hosted" && <Check className="provider-profile-check" size={15} />}
            </button>
          )}
          <button role="radio" className={"provider-profile" + (mode === "byok" ? " on" : "")} onClick={() => setMode("byok")} aria-checked={mode === "byok"}>
            <span className="provider-profile-icon"><KeyRound size={17} /></span>
            <span className="provider-profile-copy"><strong>My API keys</strong><small>Choose every provider</small></span>
            {mode === "byok" && <Check className="provider-profile-check" size={15} />}
          </button>
        </div>

        {(mode === "local" || mode === "balanced") && <div className="settings-fields">
          <div className="field-row">
            <label className="field">
              <span className="field-label">Local text model</span>
              <select value={localTextModel} onChange={(e) => setLocalTextModel(e.target.value)}>
                {localTextModels.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
              </select>
              <span className="field-hint">Classifies notes, extracts structure, and powers local chat.</span>
            </label>
            <label className="field">
              <span className="field-label">Local vision model</span>
              <select value={localVisionModel} onChange={(e) => setLocalVisionModel(e.target.value)}>
                {localVisionModels.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
              </select>
              <span className="field-hint">Reads photos, screenshots, and handwriting.</span>
            </label>
          </div>
          <div className="field-row">
            <span className="field-hint">
              <Check size={13} /> Semantic search: nomic-embed-text (fixed, 768 dimensions)
            </span>
            <span className="field-hint">
              Ollama currently reports {installedModels.length} installed model{installedModels.length === 1 ? "" : "s"}.
            </span>
          </div>
          {speechEngineField("Local speech model")}
          <div className="field-row">
            <span className="field-hint">
              {mModel?.turbo || mModel?.base ? <Check size={13} /> : null} Whisper {mModel?.turbo ? "large-v3-turbo installed" : mModel?.base ? "Base English installed" : "not installed"}
            </span>
            <span className="field-hint">
              {mModel?.parakeet ? <Check size={13} /> : null} Parakeet TDT 0.6B {mModel?.parakeet ? "installed" : "not installed"}
            </span>
          </div>
          {releaseProfile.diarization && (
            <span className="field-hint">
              {mModel?.speaker ? <Check size={13} /> : null} Speaker separation {mModel?.speaker ? "installed" : "not installed"} — separates voices; one-on-ones use calendar attendees when available.
            </span>
          )}
        </div>}

        {releaseProfile.notedHosted && mode === "hosted" && (
          <div className="settings-fields">
            <div className={"conn-status " + conn.state}>
              {conn.state === "checking" && <Loader2 size={13} className="spin" />}
              {conn.state === "ok" && <Wifi size={13} />}
              {conn.state !== "ok" && conn.state !== "checking" && <WifiOff size={13} />}
              <span className="conn-label">
                {conn.state === "checking"
                  ? "Checking hosted connection…"
                  : conn.state === "ok"
                    ? conn.msg
                    : s?.has_hosted_key
                      ? "Ready to connect"
                      : "Hosted activation missing"}
              </span>
            </div>
            {conn.state === "err" && <div className="conn-detail">{conn.msg}</div>}
            <span className="field-hint">
              Your activation credential is stored in macOS Keychain. No Ollama, Gemma, Parakeet,
              Whisper, or embedding-model download is required.
            </span>
            <button className="ghost-btn test-btn" onClick={() => checkConnection({ mode: "hosted" })} disabled={!s?.has_hosted_key}>
              Test hosted connection
            </button>
          </div>
        )}

        {releaseProfile.balancedInference && mode === "balanced" && (
          <div className="settings-fields">
            <span className="field-hint">
              <strong>Privacy boundary:</strong> only new note text and photos are sent to this provider for extraction and OCR.
              Your database, semantic-search index, journal, Ask context, meetings, and Brain Vault stay on this Mac.
              Ask itself remains local; context-free cloud questions are not enabled yet.
            </span>
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

        {mode === "byok" && byok && (
          <div className="settings-fields">
            <strong>Capability routing</strong>
            <span className="field-hint">
              Each kind of data goes only to the provider selected below. Keys are stored in macOS Keychain;
              model IDs and routing preferences are stored locally. Anthropic cannot be selected for embeddings or transcription.
            </span>
            {([
              ["intelligence", "Chat, notes, summaries and recaps"],
              ["vision", "Photos and handwriting"],
              ["embeddings", "Semantic search index"],
              ["transcription", "Dictation and meeting audio"],
            ] as const).map(([slot, label]) => {
              const allowed: ProviderId[] = (slot === "embeddings"
                ? ["openai", "gemini", "openai_compatible", "noted_hosted"]
                : slot === "transcription"
                  ? ["openai", "gemini", "groq", "openai_compatible", "noted_hosted"]
                  : ["openai", "gemini", "anthropic", "groq", "openai_compatible", "noted_hosted"]
              ).filter((provider) => releaseProfile.notedHosted || provider !== "noted_hosted") as ProviderId[];
              const choice = byok[slot];
              const update = (patch: Partial<typeof choice>) => setByok({ ...byok, [slot]: { ...choice, ...patch } });
              return <div className="field-row" key={slot}>
                <label className="field">
                  <span className="field-label">{label}</span>
                  <select value={choice.provider} onChange={(e) => update({ provider: e.target.value as ProviderId })}>
                    {allowed.map((p) => <option key={p} value={p}>{p.replace(/_/g, " ")}</option>)}
                  </select>
                </label>
                <label className="field">
                  <span className="field-label">Model ID</span>
                  <input list={`models-${slot}`} value={choice.model} onChange={(e) => update({ model: e.target.value })} spellCheck={false} />
                  <datalist id={`models-${slot}`}>{(discoveredModels[slot] ?? []).map((m) => <option key={m} value={m} />)}</datalist>
                  <button type="button" className="field-clear" disabled={discovering === slot} onClick={async () => {
                    setDiscovering(slot);
                    try {
                      const models = await api.listByokModels(choice.provider, choice.base_url);
                      setDiscoveredModels((prev) => ({ ...prev, [slot]: models }));
                    }
                    catch (e) { setConn({ state: "err", msg: `Model discovery: ${String(e)}` }); }
                    finally { setDiscovering(""); }
                  }}>{discovering === slot ? "loading…" : "discover models"}</button>
                </label>
                {choice.provider === "openai_compatible" && <label className="field">
                  <span className="field-label">Base URL</span>
                  <input value={choice.base_url} onChange={(e) => update({ base_url: e.target.value })} placeholder="https://…/v1" spellCheck={false} />
                </label>}
              </div>;
            })}
            <label className="field">
              <span className="field-label">OpenAI key {s?.has_openai_key && <>· saved <button type="button" className="field-clear" onClick={() => removeByokKey("openai")}>remove</button></>}</span>
              <input type="password" value={openaiKey} onChange={(e) => setOpenaiKey(e.target.value)} placeholder="Leave blank to keep" autoComplete="off" />
            </label>
            <label className="field">
              <span className="field-label">Gemini key {s?.has_gemini_key && <>· saved <button type="button" className="field-clear" onClick={() => removeByokKey("gemini")}>remove</button></>}</span>
              <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="Leave blank to keep" autoComplete="off" />
            </label>
            <label className="field">
              <span className="field-label">Anthropic key {s?.has_anthropic_key && <>· saved <button type="button" className="field-clear" onClick={() => removeByokKey("anthropic")}>remove</button></>}</span>
              <input type="password" value={anthropicKey} onChange={(e) => setAnthropicKey(e.target.value)} placeholder="Leave blank to keep" autoComplete="off" />
            </label>
            <label className="field">
              <span className="field-label">Groq key {s?.has_groq_key && <>· saved <button type="button" className="field-clear" onClick={() => removeByokKey("groq")}>remove</button></>}</span>
              <input type="password" value={groqKey} onChange={(e) => setGroqKey(e.target.value)} placeholder="Leave blank to keep" autoComplete="off" />
            </label>
            <label className="field">
              <span className="field-label">OpenAI-compatible key {s?.has_openai_compatible_key && <>· saved <button type="button" className="field-clear" onClick={() => removeByokKey("openai_compatible")}>remove</button></>}</span>
              <input type="password" value={compatibleKey} onChange={(e) => setCompatibleKey(e.target.value)} placeholder="Leave blank to keep" autoComplete="off" />
            </label>
            <div className="conn-detail">
              Audio → {byok.transcription.provider} · Summaries → {byok.intelligence.provider} · Photos → {byok.vision.provider} · Search indexing → {byok.embeddings.provider}
            </div>
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
          <strong>Experimental.</strong> Registered Obsidian vaults import Markdown and wikilinks into
          your knowledge graph. Work vaults are one-way by default; the personal vault is generated
          by noted. Write-back only changes noted-managed blocks. Git history is created only when
          the vault is already a git repository.
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
                using noted-managed blocks. Import + embed always run regardless; writes are
                committed only in vaults that already use git.
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
          noted records meetings bot-free using your mic + system audio. Choose private local
          transcription{releaseProfile.notedHosted ? " or hosted Parakeet with no model download" : " or your own transcription provider"}.
          Nothing is captured unless you accept a prompt or hit Record. Speaker separation groups
          voices; participant names are assigned manually instead of recognized from voiceprints.
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
          {releaseProfile.videoCapture && (
            <>
              <label className="vault-auto">
                <input
                  type="checkbox"
                  checked={mcfg?.record_video ?? true}
                  onChange={(e) => mcfg && saveMcfg({ ...mcfg, record_video: e.target.checked })}
                />
                <span>
                  Record the meeting window as video
                  <em>
                    Captures the call app's window itself (macOS 15+, one-time Screen Recording
                    permission) — covering it with other apps or switching Spaces doesn't
                    interrupt it. Videos auto-delete after{" "}
                    <input
                      className="inline-days"
                      type="number"
                      min={0}
                      max={365}
                      value={mcfg?.video_keep_days ?? 14}
                      onClick={(e) => e.preventDefault()}
                      onChange={(e) =>
                        mcfg &&
                        saveMcfg({
                          ...mcfg,
                          video_keep_days: Math.max(0, Number(e.target.value) || 0),
                        })
                      }
                    />{" "}
                    days to save space (0 = keep forever); transcripts and summaries are kept.
                  </em>
                </span>
              </label>
              {mcfg?.record_video && mModel?.video_supported && (
                <div className="meeting-video-permission">
                  <div className={"conn-status " + (mModel.video_authorized ? "ok" : "idle")}>
                    {mModel.video_authorized ? <Check size={13} /> : <Laptop size={13} />}
                    {mModel.video_authorized
                      ? "Window recording permission ready"
                      : "Window recording is paused until permission is granted"}
                  </div>
                  {!mModel.video_authorized && (
                    <button
                      type="button"
                      className="ghost-btn test-btn"
                      onClick={requestVideoPermission}
                      disabled={videoPermissionBusy}
                    >
                      {videoPermissionBusy ? (
                        <Loader2 size={14} className="spin" />
                      ) : (
                        <Laptop size={14} />
                      )}
                      Allow video recording once
                    </button>
                  )}
                  <span className="field-hint">
                    Noted will not ask during a meeting. If access is off, audio and transcription
                    continue and video is skipped.
                  </span>
                  {videoPermissionMsg && (
                    <span className="field-hint">{videoPermissionMsg}</span>
                  )}
                </div>
              )}
            </>
          )}
          <label className="vault-auto">
            <input
              type="checkbox"
              checked={mcfg?.mic_aec ?? true}
              onChange={(e) => mcfg && saveMcfg({ ...mcfg, mic_aec: e.target.checked })}
            />
            <span>
              Echo cancellation on the microphone
              <em>
                macOS voice processing removes what your speakers are playing from the mic, so
                the other side of a call never shows up as you — essential without headphones.
                Applies from the next recording.
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
              Preferred terms for new transcripts (comma-separated)
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
            <span className="field-hint">
              Helps the speech model recognize names and jargon before correction rules are applied.
            </span>
          </label>
          <TranscriptVocabularySettings />
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
          {releaseProfile.diarization && <div className="field-row">
            {mModel?.speaker ? (
              <span className="field-hint">
                <Check size={13} /> Speaker separation ready — rename speakers in each transcript
              </span>
            ) : (
              <button
                className="ghost-btn test-btn"
                onClick={downloadSpeakerModel}
                disabled={sDownloading}
                title="Separates voices into neutral speaker labels; it does not recognize people across meetings"
              >
                {sDownloading ? <Loader2 size={14} className="spin" /> : <Download size={14} />}
                {sDownloading ? "Downloading (27 MB)…" : "Download speaker separation model (27 MB)"}
              </button>
            )}
          </div>}
          {speechEngineField("Transcription engine")}
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
                title="NVIDIA Parakeet-TDT 0.6B — speed-first local transcription. English only."
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
