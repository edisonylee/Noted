import { useEffect, useMemo, useRef, useState } from "react";
import ForceGraph2D from "react-force-graph-2d";
import { ArrowRight, X } from "lucide-react";
import { api, type EntityMention, type GraphData, type GraphNode } from "./api";
import { TYPE_COLORS, colorForType } from "./entityColors";

function cssVar(name: string, fallback: string) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

export function SelfView({
  theme,
  onOpenEntity,
  // The graph source defaults to the full entity graph; the Work lens passes a
  // vault-scoped loader. Remount (via `key`) to reload when the source changes.
  fetchGraph,
  emptyTitle = "Your graph is empty",
  emptyBody = "It grows as you log people, places, foods and activities. Capture a few notes through the app and watch yourself take shape.",
}: {
  theme: string;
  onOpenEntity?: (id: number) => void;
  fetchGraph?: () => Promise<GraphData>;
  emptyTitle?: string;
  emptyBody?: string;
}) {
  const [data, setData] = useState<GraphData | null>(null);
  const [hover, setHover] = useState<number | null>(null);
  const [selected, setSelected] = useState<GraphNode | null>(null);
  const [detail, setDetail] = useState<EntityMention[]>([]);
  const wrapRef = useRef<HTMLDivElement>(null);
  const fgRef = useRef<any>(null);
  const [size, setSize] = useState({ w: 800, h: 600 });

  useEffect(() => {
    (fetchGraph ?? api.entityGraph)().then(setData).catch(() => setData({ nodes: [], edges: [] }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // theme-aware canvas colors (recompute when the theme flips)
  const colors = useMemo(
    () => ({
      bg: cssVar("--canvas", "#f7f5f1"),
      ink: cssVar("--ink", "#1b1916"),
      line: cssVar("--line", "#e9e5dd"),
      accent: cssVar("--accent", "#3d79bd"),
    }),
    [theme]
  );

  const graph = useMemo(() => {
    if (!data) return { nodes: [], links: [] };
    return {
      nodes: data.nodes.map((n) => ({ ...n })),
      links: data.edges.map((e) => ({ source: e.source, target: e.target, weight: e.weight })),
    };
  }, [data]);

  const neighbors = useMemo(() => {
    const m = new Map<number, Set<number>>();
    for (const e of data?.edges ?? []) {
      (m.get(e.source) ?? m.set(e.source, new Set()).get(e.source)!).add(e.target);
      (m.get(e.target) ?? m.set(e.target, new Set()).get(e.target)!).add(e.source);
    }
    return m;
  }, [data]);

  function openNode(node: any) {
    setSelected(node);
    api.entityDetail(node.id).then(setDetail).catch(() => setDetail([]));
    if (node.x != null) {
      fgRef.current?.centerAt(node.x, node.y, 500);
      fgRef.current?.zoom(2.4, 500);
    }
  }

  const empty = data && data.nodes.length === 0;

  return (
    <div className="self">
      <div className="self-graph" ref={wrapRef}>
        {empty ? (
          <div className="self-empty">
            <h2>{emptyTitle}</h2>
            <p>{emptyBody}</p>
          </div>
        ) : data ? (
          <ForceGraph2D
            ref={fgRef}
            width={size.w}
            height={size.h}
            graphData={graph}
            backgroundColor={colors.bg}
            cooldownTicks={140}
            nodeRelSize={5}
            nodeLabel={(n: any) => `${n.name} · ${n.type}`}
            linkColor={(l: any) => {
              if (hover == null) return colors.line;
              const s = typeof l.source === "object" ? l.source.id : l.source;
              const t = typeof l.target === "object" ? l.target.id : l.target;
              return s === hover || t === hover ? colors.accent : colors.line;
            }}
            linkWidth={(l: any) => Math.min(4, 1 + (l.weight || 1) * 0.5)}
            onNodeHover={(n: any) => setHover(n ? n.id : null)}
            onNodeClick={openNode}
            nodeCanvasObject={(node: any, ctx: CanvasRenderingContext2D, scale: number) => {
              const r = Math.max(3, Math.sqrt(node.mention_count || 1) * 3);
              const lit = hover != null && (node.id === hover || neighbors.get(hover)?.has(node.id));
              ctx.globalAlpha = hover != null && !lit ? 0.22 : 1;
              ctx.beginPath();
              ctx.arc(node.x, node.y, r, 0, 2 * Math.PI);
              ctx.fillStyle = colorForType(node.type);
              ctx.fill();
              if (scale > 1.3 || (node.mention_count || 0) >= 2 || node.id === hover) {
                const fs = Math.max(11 / scale, 2.5);
                ctx.font = `${fs}px "Geist Variable", sans-serif`;
                ctx.fillStyle = colors.ink;
                ctx.textAlign = "center";
                ctx.textBaseline = "top";
                ctx.fillText(node.name, node.x, node.y + r + 2);
              }
              ctx.globalAlpha = 1;
            }}
          />
        ) : (
          <div className="self-empty">
            <p className="muted">drawing your graph…</p>
          </div>
        )}

        {!empty && (
          <div className="graph-legend">
            {Object.entries(TYPE_COLORS).map(([t, c]) => (
              <span className="legend-item" key={t}>
                <span className="ldot" style={{ background: c }} />
                {t}
              </span>
            ))}
          </div>
        )}
      </div>

      {selected && (
        <aside className="entity-panel">
          <div className="entity-panel-head">
            <span className="ldot big" style={{ background: colorForType(selected.type) }} />
            <div className="entity-id">
              <div className="entity-name">{selected.name}</div>
              <div className="entity-meta">
                {selected.type} · {selected.mention_count} mention{selected.mention_count === 1 ? "" : "s"}
              </div>
            </div>
            <button className="icon-btn" onClick={() => setSelected(null)} title="Close">
              <X size={16} />
            </button>
          </div>
          {onOpenEntity && (
            <button className="entity-fullpage" onClick={() => onOpenEntity(selected.id)}>
              View full timeline <ArrowRight size={13} />
            </button>
          )}
          <div className="entity-mentions">
            {detail.length === 0 && <p className="muted small">No notes yet.</p>}
            {detail.map((m, i) => (
              <div className="entity-mention" key={i}>
                <time>{m.event_date}</time>
                <p>{m.snippet}</p>
              </div>
            ))}
          </div>
        </aside>
      )}
    </div>
  );
}
