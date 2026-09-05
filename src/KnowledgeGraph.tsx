import { useEffect, useMemo, useRef, useState } from "react";
import ForceGraph2D from "react-force-graph-2d";
import { Loader2, RefreshCw, Search, X } from "lucide-react";
import { api, type EntityProfile, type GraphData } from "./api";
import { colorForType, TYPE_COLORS } from "./entityColors";
import { relativeDay } from "./day";

// The Knowledge home: an Obsidian-style force canvas over the meeting-fed
// entity graph. Nodes are entities (people / projects / topics / orgs), edges
// are co-mentions within the same note, and everything glows a little.

function cssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

// The meeting-fed lens: what the graph shows by default. "all" opens it up to
// every entity type the older note pipeline created.
const LENSES: { key: string; label: string; types: string[] | null }[] = [
  { key: "meetings", label: "Meetings", types: ["person", "project", "topic", "org"] },
  { key: "people", label: "People", types: ["person"] },
  { key: "work", label: "Projects", types: ["project", "org"] },
  { key: "topics", label: "Topics", types: ["topic"] },
  { key: "all", label: "Everything", types: null },
];

const RANGES: { key: string; label: string; days: number | null }[] = [
  { key: "all", label: "All time", days: null },
  { key: "30d", label: "30 days", days: 30 },
  { key: "7d", label: "7 days", days: 7 },
];

type FGNode = {
  id: number;
  name: string;
  type: string;
  mention_count: number;
  x?: number;
  y?: number;
};

export function KnowledgeGraph({
  theme,
  onOpenEntity,
}: {
  theme: string;
  onOpenEntity?: (id: number) => void;
}) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const fgRef = useRef<any>(null);
  const [size, setSize] = useState({ w: 800, h: 600 });
  const [data, setData] = useState<GraphData | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [lens, setLens] = useState("meetings");
  const [range, setRange] = useState("all");
  const [query, setQuery] = useState("");
  const [hover, setHover] = useState<number | null>(null);
  const [selected, setSelected] = useState<EntityProfile | null>(null);
  const [rebuilding, setRebuilding] = useState(false);
  const [rebuildMsg, setRebuildMsg] = useState<string | null>(null);
  const fitted = useRef(false);

  const load = () => api.entityGraph().then(setData).catch((e) => setErr(String(e)));
  useEffect(() => {
    load();
  }, []);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() =>
      setSize({ w: el.clientWidth, h: el.clientHeight })
    );
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Colors re-read every render so theme flips repaint the canvas correctly.
  const dark = theme === "dark";
  const colors = {
    bg: cssVar("--canvas", dark ? "#141210" : "#faf8f5"),
    ink: cssVar("--ink", dark ? "#e8e4de" : "#26221c"),
    line: cssVar("--line", dark ? "#3a352e" : "#e4ded4"),
    accent: cssVar("--accent", "#b25e43"),
    font: cssVar("--font", "system-ui"),
  };

  const graph = useMemo(() => {
    if (!data) return { nodes: [] as FGNode[], links: [] as any[] };
    const lensTypes = LENSES.find((l) => l.key === lens)?.types ?? null;
    const days = RANGES.find((r) => r.key === range)?.days ?? null;
    let cutoff = "";
    if (days != null) {
      const d = new Date();
      d.setDate(d.getDate() - days);
      cutoff = d.toISOString().slice(0, 10);
    }
    let nodes = data.nodes.filter(
      (n) =>
        (lensTypes == null || lensTypes.includes(n.type)) &&
        (days == null || (n.last_seen ?? "") >= cutoff)
    );
    // Quality floor: one-off entities stay hidden until the graph is big enough
    // for them to be noise rather than the whole picture.
    const recurring = nodes.filter((n) => n.mention_count >= 2);
    if (recurring.length >= 8) nodes = recurring;
    const q = query.trim().toLowerCase();
    if (q) nodes = nodes.filter((n) => n.name.toLowerCase().includes(q));
    const keep = new Set(nodes.map((n) => n.id));
    const links = data.edges
      .filter((e) => keep.has(e.source) && keep.has(e.target))
      .map((e) => ({ ...e }));
    return { nodes: nodes.map((n) => ({ ...n })) as FGNode[], links };
  }, [data, lens, range, query]);

  // Re-fit whenever the visible set changes. Switching lens or range rebuilds
  // the graph, and keeping the previous viewport leaves the new nodes framed
  // badly — or entirely off-screen when the set shrinks.
  useEffect(() => {
    fitted.current = false;
  }, [graph]);

  // Neighborhood map for hover dimming.
  const neighbors = useMemo(() => {
    const m = new Map<number, Set<number>>();
    for (const l of graph.links) {
      const s = typeof l.source === "object" ? l.source.id : l.source;
      const t = typeof l.target === "object" ? l.target.id : l.target;
      if (!m.has(s)) m.set(s, new Set());
      if (!m.has(t)) m.set(t, new Set());
      m.get(s)!.add(t);
      m.get(t)!.add(s);
    }
    return m;
  }, [graph]);

  const inNeighborhood = (id: number) =>
    hover == null || id === hover || (neighbors.get(hover)?.has(id) ?? false);

  // Spread the layout a bit further apart than the library default.
  useEffect(() => {
    const fg = fgRef.current;
    if (!fg) return;
    fg.d3Force("charge")?.strength(-160);
    fg.d3Force("link")?.distance((l: any) => 60 / Math.min(3, l.weight || 1) + 30);
  }, [graph]);

  async function openNode(node: FGNode) {
    try {
      const p = await api.entityProfile(node.id);
      setSelected(p);
    } catch (e) {
      setErr(String(e));
    }
    if (node.x != null && node.y != null && fgRef.current) {
      fgRef.current.centerAt(node.x, node.y, 500);
      fgRef.current.zoom(3, 500);
    }
  }

  async function rebuild() {
    if (rebuilding) return;
    setRebuilding(true);
    setRebuildMsg(null);
    try {
      const r = await api.kgReindexMeetings();
      setRebuildMsg(
        r.mentions > 0
          ? `Mined ${r.mentions} new mention${r.mentions === 1 ? "" : "s"} from ${r.meetings} meeting${r.meetings === 1 ? "" : "s"}.`
          : "Graph is up to date with your meetings."
      );
      await load();
    } catch (e) {
      setRebuildMsg(String(e));
    } finally {
      setRebuilding(false);
    }
  }

  const empty = data != null && graph.nodes.length === 0;

  return (
    <div className="kn-graph-wrap">
      <div className="kn-graph-bar">
        <div className="kn-graph-lenses">
          {LENSES.map((l) => (
            <button
              key={l.key}
              className={"chip" + (lens === l.key ? " on" : "")}
              onClick={() => setLens(l.key)}
            >
              {l.label}
            </button>
          ))}
          <span className="kn-graph-sep" />
          {RANGES.map((r) => (
            <button
              key={r.key}
              className={"chip" + (range === r.key ? " on" : "")}
              onClick={() => setRange(r.key)}
            >
              {r.label}
            </button>
          ))}
        </div>
        <div className="kn-graph-tools">
          <div className="kn-graph-search">
            <Search size={13} />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter graph…"
            />
            {query && (
              <button className="icon-btn" onClick={() => setQuery("")}>
                <X size={12} />
              </button>
            )}
          </div>
          <button className="ghost-btn" onClick={rebuild} disabled={rebuilding} title="Mine all recorded meetings for people, projects and topics">
            {rebuilding ? <Loader2 size={13} className="spin" /> : <RefreshCw size={13} />}
            Rebuild from meetings
          </button>
        </div>
      </div>
      {rebuildMsg && <div className="muted small kn-graph-msg">{rebuildMsg}</div>}
      {err && <div className="error">{err}</div>}

      <div className="kn-graph" ref={wrapRef}>
        <ForceGraph2D
          ref={fgRef}
          width={size.w}
          height={size.h}
          graphData={graph}
          backgroundColor={colors.bg}
          cooldownTicks={200}
          onEngineStop={() => {
            if (!fitted.current && graph.nodes.length > 1 && fgRef.current) {
              fgRef.current.zoomToFit(400, 80);
              fitted.current = true;
            }
          }}
          nodeLabel={() => ""}
          linkColor={(l: any) => {
            const s = typeof l.source === "object" ? l.source.id : l.source;
            const t = typeof l.target === "object" ? l.target.id : l.target;
            if (hover != null && (s === hover || t === hover)) return colors.accent;
            return colors.line;
          }}
          linkWidth={(l: any) => {
            const s = typeof l.source === "object" ? l.source.id : l.source;
            const t = typeof l.target === "object" ? l.target.id : l.target;
            const w = Math.min(3.5, 0.6 + (l.weight || 1) * 0.6);
            return hover != null && (s === hover || t === hover) ? w + 0.8 : w;
          }}
          onNodeHover={(n: any) => setHover(n ? n.id : null)}
          onNodeClick={(n: any) => openNode(n)}
          onBackgroundClick={() => setSelected(null)}
          nodeCanvasObject={(node: any, ctx, scale) => {
            const dim = !inNeighborhood(node.id);
            const r = Math.max(2.5, Math.sqrt(node.mention_count || 1) * 2.6);
            const color = colorForType(node.type);
            ctx.globalAlpha = dim ? 0.12 : 1;
            // Obsidian-style soft glow — stronger for hubs, calmer in light mode.
            ctx.shadowColor = color;
            ctx.shadowBlur = dim ? 0 : (dark ? 14 : 8) + r;
            ctx.beginPath();
            ctx.arc(node.x, node.y, r, 0, 2 * Math.PI);
            ctx.fillStyle = color;
            ctx.fill();
            ctx.shadowBlur = 0;
            // Labels fade in with zoom (hubs first), always on for the hovered
            // neighborhood.
            const hubBonus = Math.min(1.2, (node.mention_count || 1) / 8);
            const zoomAlpha = Math.max(0, Math.min(1, (scale - 1.4 + hubBonus) / 1.2));
            const hovered = hover != null && inNeighborhood(node.id);
            const alpha = hovered ? 1 : zoomAlpha;
            if (alpha > 0.02 && !dim) {
              const label =
                node.name.length > 22 ? node.name.slice(0, 21) + "…" : node.name;
              ctx.globalAlpha = alpha;
              ctx.font = `${Math.max(3.2, 11 / scale)}px ${colors.font}`;
              ctx.textAlign = "center";
              ctx.textBaseline = "top";
              ctx.fillStyle = colors.ink;
              ctx.fillText(label, node.x, node.y + r + 2);
            }
            ctx.globalAlpha = 1;
          }}
          nodePointerAreaPaint={(node: any, color, ctx) => {
            const r = Math.max(2.5, Math.sqrt(node.mention_count || 1) * 2.6) + 4;
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(node.x, node.y, r, 0, 2 * Math.PI);
            ctx.fill();
          }}
        />

        {empty && (
          <div className="kn-graph-empty">
            <p className="muted">
              Nothing on the map yet. Record a meeting — or mine the ones you already
              have with <strong>Rebuild from meetings</strong>.
            </p>
          </div>
        )}

        <div className="graph-legend kn-graph-legend">
          {Object.entries(TYPE_COLORS)
            .filter(([t]) =>
              (LENSES.find((l) => l.key === lens)?.types ?? Object.keys(TYPE_COLORS)).includes(t)
            )
            .map(([t]) => (
              <span key={t}>
                <i style={{ background: colorForType(t) }} /> {t}
              </span>
            ))}
        </div>

        {selected && (
          <aside className="entity-panel kn-graph-panel">
            <div className="entity-panel-head">
              <span className="ldot" style={{ background: colorForType(selected.type) }} />
              <strong>{selected.name}</strong>
              <button className="icon-btn" onClick={() => setSelected(null)}>
                <X size={13} />
              </button>
            </div>
            <div className="muted small">
              {selected.type} · {selected.mention_count}{" "}
              {selected.mention_count === 1 ? "mention" : "mentions"}
              {selected.last_seen && <> · last {relativeDay(selected.last_seen)}</>}
            </div>
            <ul className="entity-panel-mentions">
              {selected.mentions.slice(0, 5).map((m, i) => (
                <li key={i}>
                  <span className="fact-date">{relativeDay(m.date)}</span>
                  <span className="fact-text">{m.text}</span>
                </li>
              ))}
            </ul>
            <button className="ghost-btn" onClick={() => onOpenEntity?.(selected.id)}>
              Open full page
            </button>
          </aside>
        )}
      </div>
    </div>
  );
}
