import { useEffect, useMemo, useState } from "react";
import { RefreshCw } from "lucide-react";
import { SelfView } from "./Self";
import { api, type BrainVaultStatus, type GraphData } from "./api";
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
        <button className="ghost-btn work-sync" onClick={sync} disabled={syncing} title="Re-sync from Obsidian">
          <RefreshCw size={14} className={syncing ? "spin" : ""} /> {syncing ? "Syncing…" : "Sync"}
        </button>
      </div>

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
