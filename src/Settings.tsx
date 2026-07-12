import { useEffect, useState } from "react";
import { X, Check, Loader2, Wifi, WifiOff, CalendarCheck, CalendarX, RefreshCw, Trash2, FolderPlus } from "lucide-react";
import { api, type BrainVaultStatus, type GcalStatus, type ProviderMode, type ProviderSettings } from "./api";

// Live connection status, shown as a persistent badge so "is Gemini actually
// reachable?" is never a mystery — checked on open and after every save/test.
type Conn =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "ok"; msg: string }
  | { state: "err"; msg: string };

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

  useEffect(() => {
    api.getProviderSettings().then((cfg) => {
      setS(cfg);
      setMode(cfg.mode);
      setTextModel(cfg.gemini_text_model);
      setVisionModel(cfg.gemini_vision_model);
      // If a key is already stored, verify it's actually live on open so the
      // user sees "Connected" without having to remember to click Test.
      if (cfg.mode === "balanced" && cfg.has_gemini_key) checkConnection();
    });
    api.gcalAuthStatus().then(setGcal);
    api.brainListVaults().then(setVaults).catch(() => {});
    api.brainGetAuto().then(setAutoProp).catch(() => {});
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
    if (!vaultPath.trim()) return;
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

  // Persist the current fields, then hit Gemini and reflect the real result in
  // the status badge. Shared by Save, Test, and the on-open auto-check.
  async function checkConnection() {
    setConn({ state: "checking" });
    try {
      await api.setProviderSettings({
        mode,
        // only send the key if the user typed a new one (blank = leave as-is)
        gemini_api_key: key.trim() ? key.trim() : undefined,
        gemini_text_model: textModel.trim() || undefined,
        gemini_vision_model: visionModel.trim() || undefined,
      });
      const msg = await api.testProvider();
      setConn({ state: "ok", msg });
      setS((prev) => (prev ? { ...prev, has_gemini_key: true } : prev));
    } catch (e) {
      setConn({ state: "err", msg: String(e) });
    }
  }

  async function save() {
    setSaving(true);
    try {
      await api.setProviderSettings({
        mode,
        gemini_api_key: key.trim() ? key.trim() : undefined,
        gemini_text_model: textModel.trim() || undefined,
        gemini_vision_model: visionModel.trim() || undefined,
      });
      // Confirm the save landed against a working connection before leaving —
      // surfaces a bad key instead of closing on a silent failure.
      if (mode === "balanced" && (key.trim() || s?.has_gemini_key)) {
        await checkConnection();
      } else {
        onClose();
      }
    } finally {
      setSaving(false);
    }
  }

  // Explicitly clear the stored key (blank field = "keep", so removal needs its
  // own action). Sends "" which the backend reads as "delete from Keychain".
  async function removeKey() {
    await api.setProviderSettings({ mode, gemini_api_key: "" });
    setKey("");
    setConn({ state: "idle" });
    setS((prev) => (prev ? { ...prev, has_gemini_key: false } : prev));
  }

  const hasKey = s?.has_gemini_key || key.trim().length > 0;
  const testing = conn.state === "checking";

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

            <button className="ghost-btn test-btn" onClick={checkConnection} disabled={testing || !hasKey}>
              {testing ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
              Test connection
            </button>
          </div>
        )}

        <h3 className="settings-section">Google Calendar</h3>
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
                Choose which calendars show up from the filter inside the Calendar view. To
                reconnect an expired account, just add it again.
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

        <h3 className="settings-section">Brain vaults</h3>
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
              disabled={vaultBusy !== "" || !vaultPath.trim()}
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
