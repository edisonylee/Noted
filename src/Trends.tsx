import { useEffect, useMemo, useState } from "react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api, type CategoryInfo, type Trends } from "./api";
import type { Theme } from "./useTheme";
import { groupByPillar } from "./pillars";

// muted series palette — blue accent leads, then earthy neutrals
const COLORS_LIGHT = ["#3d79bd", "#46413a", "#3f7d5b", "#b8893f", "#5e7e86", "#9b8b6e", "#8a5a4a", "#7a6c84"];
const COLORS_DARK = ["#5797df", "#b8b0a2", "#6fb78c", "#d6a45f", "#83a7af", "#c0b193", "#cc8576", "#a99bb8"];
// metrics that usually make the most interesting default chart
const PRIMARY_METRICS = ["weight", "hours", "duration_min", "minutes", "calories", "amount", "value", "count"];
type Agg = "sum" | "max" | "avg";

const CHART = {
  light: {
    colors: COLORS_LIGHT,
    axis: { stroke: "#8c857a", fontSize: 11 },
    grid: "#eee9e0",
    bar: "#3d79bd",
    tooltip: { background: "#ffffff", border: "1px solid #e9e5dd", borderRadius: 12, color: "#1b1916" },
  },
  dark: {
    colors: COLORS_DARK,
    axis: { stroke: "#a8a08f", fontSize: 11 },
    grid: "#322e26",
    bar: "#5797df",
    tooltip: { background: "#211e18", border: "1px solid #423c31", borderRadius: 12, color: "#f3efe7" },
  },
} as const;

export function TrendsView({ cats, theme }: { cats: CategoryInfo[]; theme: Theme }) {
  const c = CHART[theme];
  const [cat, setCat] = useState(cats[0]?.name ?? "");
  const [trends, setTrends] = useState<Trends | null>(null);
  const [metric, setMetric] = useState("");
  const [agg, setAgg] = useState<Agg>("max");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!cat && cats[0]) setCat(cats[0].name);
  }, [cats]);

  useEffect(() => {
    if (!cat) return;
    setLoading(true);
    api
      .categoryTrends(cat)
      .then((t) => {
        setTrends(t);
        const preferred = t.metrics.find((m) => PRIMARY_METRICS.includes(m)) ?? t.metrics[0] ?? "";
        setMetric(preferred);
      })
      .finally(() => setLoading(false));
  }, [cat]);

  // Pivot long rows -> wide-by-date for the chosen metric, one series per label.
  const { data, series } = useMemo(() => {
    if (!trends || !metric) return { data: [] as Record<string, unknown>[], series: [] as string[] };
    const byDate = new Map<string, Map<string, number[]>>();
    for (const r of trends.rows) {
      const v = r.values[metric];
      if (typeof v !== "number") continue;
      if (!byDate.has(r.date)) byDate.set(r.date, new Map());
      const lm = byDate.get(r.date)!;
      if (!lm.has(r.label)) lm.set(r.label, []);
      lm.get(r.label)!.push(v);
    }
    const seen = new Set<string>();
    const data = Array.from(byDate.entries())
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([date, lm]) => {
        const row: Record<string, unknown> = { date };
        for (const [label, vals] of lm) {
          seen.add(label);
          row[label] =
            agg === "sum"
              ? vals.reduce((a, b) => a + b, 0)
              : agg === "max"
                ? Math.max(...vals)
                : vals.reduce((a, b) => a + b, 0) / vals.length;
        }
        return row;
      });
    return { data, series: trends.labels.filter((l) => seen.has(l)) };
  }, [trends, metric, agg]);

  const countData = useMemo(
    () => (trends?.count_by_date ?? []).map(([date, count]) => ({ date, count })),
    [trends]
  );

  if (!cats.length) return <p className="muted">No categories yet — log some notes first.</p>;

  const hasMetricChart = trends && metric && data.length > 0;

  return (
    <div className="trends">
      <div className="trend-controls">
        <select
          className="cat-select"
          value={cat}
          onChange={(e) => setCat(e.target.value)}
          aria-label="category"
        >
          {groupByPillar(cats).map(({ pillar, cats: group }) => (
            <optgroup key={pillar} label={pillar}>
              {group.map((c) => (
                <option key={c.id} value={c.name}>
                  {c.name} ({c.entry_count})
                </option>
              ))}
            </optgroup>
          ))}
        </select>
        {trends && trends.metrics.length > 0 && (
          <>
            <div className="pill-group" role="group" aria-label="metric">
              {trends.metrics.map((m) => (
                <button
                  key={m}
                  className={"pill" + (metric === m ? " on" : "")}
                  onClick={() => setMetric(m)}
                >
                  {m}
                </button>
              ))}
            </div>
            <div className="pill-group" role="group" aria-label="aggregation">
              {(["max", "sum", "avg"] as const).map((a) => (
                <button
                  key={a}
                  className={"pill" + (agg === a ? " on" : "")}
                  onClick={() => setAgg(a)}
                >
                  {a}
                </button>
              ))}
            </div>
          </>
        )}
      </div>

      {loading && <p className="muted">crunching…</p>}

      {hasMetricChart ? (
        <div className="chart-card">
          <div className="chart-title">
            {metric} by {trends!.label_field ?? "item"} over time · {agg}
          </div>
          <ResponsiveContainer width="100%" height={320}>
            <LineChart data={data} margin={{ top: 8, right: 18, bottom: 0, left: -8 }}>
              <CartesianGrid stroke={c.grid} strokeDasharray="3 3" />
              <XAxis dataKey="date" {...c.axis} />
              <YAxis {...c.axis} />
              <Tooltip contentStyle={c.tooltip} />
              <Legend wrapperStyle={{ fontSize: 12 }} />
              {series.map((s, i) => (
                <Line
                  key={s}
                  type="monotone"
                  dataKey={s}
                  stroke={c.colors[i % c.colors.length]}
                  strokeWidth={2}
                  dot={{ r: 3 }}
                  connectNulls
                />
              ))}
            </LineChart>
          </ResponsiveContainer>
        </div>
      ) : trends && !loading ? (
        <div className="chart-card">
          <div className="chart-title">entries over time</div>
          <ResponsiveContainer width="100%" height={280}>
            <BarChart data={countData} margin={{ top: 8, right: 18, bottom: 0, left: -8 }}>
              <CartesianGrid stroke={c.grid} strokeDasharray="3 3" />
              <XAxis dataKey="date" {...c.axis} />
              <YAxis {...c.axis} allowDecimals={false} />
              <Tooltip contentStyle={c.tooltip} />
              <Bar dataKey="count" fill={c.bar} radius={[4, 4, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
          {trends.metrics.length === 0 && (
            <p className="muted small">No numeric fields in this category yet — showing how often you log it.</p>
          )}
        </div>
      ) : null}
    </div>
  );
}
