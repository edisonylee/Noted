import { useEffect, useMemo, useState } from "react";
import { RefreshCw, UploadCloud } from "lucide-react";
import { SelfView } from "./Self";
import {
  api,
  type BrainVaultStatus,
  type BrainWritePreview,
  type BrainWriteReport,
  type GraphData,
} from "./api";
import { colorForType } from "./entityColors";

// The "Work" lens: the same knowledge graph, scoped to a brain vault (BARO /
// Profound / Personal) or all of them. Reuses the Self force-graph, adds a vault
// switcher, a re-sync button, and a Decisions (ADR) list pulled from the graph.
export function WorkView({ theme, onOpenEntity }: { theme: string; onOpenEntity?: (id: number) => void }) {
  const [vaults, setVaults] = useState<BrainVaultStatus[]>([]);
  const [vault, setVault] = useState<string | null>(null); // null = all vaults
  const [graph, setGraph] = useState<GraphData | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  // Write-back / export: preview is a dry run; null = panel closed. `mode`
  // distinguishes push-to-brain (work vaults) from export (personal vault).
  const [preview, setPreview] = useState<BrainWritePreview[] | null>(null);
  const [previewMode, setPreviewMode] = useState<"push" | "export">("push");
  const [previewing, setPreviewing] = useState(false);
  const [writing, setWriting] = useState(false);
  const [writeMsg, setWriteMsg] = useState<string | null>(null);
  const isPersonal = vault === "personal";

  function loadVaults() {
    api.brainListVaults().then(setVaults).catch(() => {});
  }
  useEffect(loadVaults, []);

  // Graph data (also backs the Decisions list); reloads on vault switch or sync.
  useEffect(() => {
    api.workGraph(vault ?? undefined).then(setGraph).catch(() => setGraph({ nodes: [], edges: [] }));
  }, [vault, refreshKey]);

  const decisions = useMemo(
    () =>
      (graph?.nodes ?? [])
        .filter((n) => n.type === "decision")
        .sort((a, b) => b.mention_count - a.mention_count),
    [graph]
  );

  async function sync() {
    setSyncing(true);
    try {
      await api.brainSync(vault ?? undefined);
      loadVaults();
      setRefreshKey((k) => k + 1);
    } finally {
      setSyncing(false);
    }
  }

  async function openPreview(mode: "push" | "export") {
    setPreviewMode(mode);
    setPreviewing(true);
    setWriteMsg(null);
    try {
      setPreview(
        mode === "export" ? await api.personalExportPreview() : await api.brainWritePreview(vault ?? undefined)
      );
    } catch (e) {
      setWriteMsg(String(e));
    } finally {
      setPreviewing(false);
    }
  }

  async function confirmWrite() {
    setWriting(true);
    try {
      const r: BrainWriteReport =
        previewMode === "export" ? await api.personalExport() : await api.brainWriteBack(vault ?? undefined);
      const commits = r.commits.map((c) => `${c.vault}@${c.sha}`).join(", ");
      setWriteMsg(
        r.files_written === 0
          ? "Nothing written."
          : `Wrote ${r.files_written} note(s)${commits ? ` · committed ${commits}` : ""}.` +
              (r.errors.length ? ` ${r.errors.length} error(s).` : "")
      );
      setPreview(null);
      loadVaults();
      setRefreshKey((k) => k + 1);
    } catch (e) {
      setWriteMsg(String(e));
    } finally {
      setWriting(false);
    }
  }

  const active = vault ? vaults.find((v) => v.vault === vault) ?? null : null;
  const totalNotes = vaults.reduce((s, v) => s + v.note_count, 0);
  const totalEntities = vaults.reduce((s, v) => s + v.entity_count, 0);

  if (vaults.length === 0) {
    return (
      <div className="self">
        <div className="self-empty">
          <h2>No brain vaults</h2>
          <p>
            Vaults under <code>~/Brain</code> sync automatically on launch. None were found — add one
            and run a sync.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="work">
      <div className="kn-tabs work-vault-tabs">
        <button className={"kn-tab" + (vault === null ? " on" : "")} onClick={() => setVault(null)}>
          All
        </button>
        {vaults.map((v) => (
          <button
            key={v.vault}
            className={"kn-tab" + (vault === v.vault ? " on" : "")}
            onClick={() => setVault(v.vault)}
          >
            {v.vault}
          </button>
        ))}
        <button className="ghost-btn work-sync" onClick={sync} disabled={syncing} title="Re-import from Obsidian">
          <RefreshCw size={14} className={syncing ? "spin" : ""} /> {syncing ? "Syncing…" : "Sync"}
        </button>
        <button
          className="ghost-btn"
          onClick={() => openPreview(isPersonal ? "export" : "push")}
          disabled={previewing}
          title={
            isPersonal
              ? "Preview the person notes noted would write into your personal vault"
              : "Preview what noted would write back into your Obsidian vault"
          }
        >
          <UploadCloud size={14} />{" "}
          {previewing ? "Checking…" : isPersonal ? "Export to personal…" : "Push to brain…"}
        </button>
      </div>

      {writeMsg && <div className="work-status">{writeMsg}</div>}

      {preview && (
        <div className="work-preview">
          {preview.length === 0 ? (
            <>
              <p className="muted small">
                {previewMode === "export"
                  ? "Nothing to export yet — no capture-only people seen often enough. As you log notes naming the same person a few times, they'll appear here to write into your personal vault."
                  : "Nothing to push — no captures mention these notes yet. As you log notes that mention people or topics from your brain, they'll show up here to write back."}
              </p>
              <button className="ghost-btn" onClick={() => setPreview(null)}>
                Close
              </button>
            </>
          ) : (
            <>
              <div className="work-preview-head">
                <strong>{preview.length}</strong>{" "}
                {previewMode === "export" ? "person note" : "note"}
                {preview.length === 1 ? "" : "s"} would be{" "}
                {previewMode === "export" ? "written to your personal vault" : "updated"} (hand-written
                text is untouched; each write is a git commit you can revert):
              </div>
              <div className="work-preview-list">
                {preview.map((p) => (
                  <div className="work-preview-item" key={`${p.vault}/${p.path}`}>
                    <div className="work-preview-title">
                      <span className="kn-result-name">{p.entity}</span>
                      <span className="kn-result-type">
                        {p.vault}/{p.path}
                      </span>
                    </div>
                    <pre className="work-preview-diff">{p.after}</pre>
                  </div>
                ))}
              </div>
              <div className="field-row">
                <button className="ghost-btn" onClick={() => setPreview(null)} disabled={writing}>
                  Cancel
                </button>
                <button className="primary" onClick={confirmWrite} disabled={writing}>
                  {writing ? "Writing…" : "Write & commit"}
                </button>
              </div>
            </>
          )}
        </div>
      )}

      <div className="work-status">
        {active ? (
          <>
            {active.note_count} notes · {active.entity_count} entities
            {active.last_synced_at ? ` · synced ${active.last_synced_at.slice(0, 10)}` : ""}
          </>
        ) : (
          <>
            {vaults.length} vaults · {totalNotes} notes · {totalEntities} entities
          </>
        )}
      </div>

      <SelfView
        key={`${vault ?? "all"}:${refreshKey}`}
        theme={theme}
        onOpenEntity={onOpenEntity}
        fetchGraph={() => api.workGraph(vault ?? undefined)}
        emptyTitle="Nothing imported yet"
        emptyBody="Sync a brain vault to see its people, projects, and decisions here."
      />

      {decisions.length > 0 && (
        <div className="work-decisions">
          <h3 className="work-decisions-title">Decisions{vault ? "" : " · all vaults"}</h3>
          <div className="kn-results">
            {decisions.map((d) => (
              <button className="kn-result" key={d.id} onClick={() => onOpenEntity?.(d.id)}>
                <span className="ldot" style={{ background: colorForType("decision") }} />
                <span className="kn-result-name">{d.name}</span>
                {d.vault && <span className="kn-result-type">{d.vault}</span>}
                <span className="kn-result-count">
                  {d.mention_count} link{d.mention_count === 1 ? "" : "s"}
                </span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
