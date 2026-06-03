import { useEffect, useState } from "react";
import { api, type Recap, type RecapRow } from "./api";

export function RecapsView() {
  const [past, setPast] = useState<RecapRow[]>([]);
  const [current, setCurrent] = useState<Recap | null>(null);
  const [busy, setBusy] = useState<"day" | "week" | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const refresh = () => api.listRecaps().then(setPast).catch((e) => setErr(String(e)));
  useEffect(() => {
    refresh();
  }, []);

  async function gen(period: "day" | "week") {
    setBusy(period);
    setErr(null);
    setCurrent(null);
    try {
      setCurrent(await api.generateRecap(period));
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="recaps">
      <div className="recap-actions">
        <button className="primary" onClick={() => gen("day")} disabled={busy !== null}>
          {busy === "day" ? "Writing…" : "Recap today"}
        </button>
        <button className="primary" onClick={() => gen("week")} disabled={busy !== null}>
          {busy === "week" ? "Writing…" : "Recap past 7 days"}
        </button>
      </div>

      {err && <div className="error">{err}</div>}

      {current && (
        <article className="recap-card fresh">
          <div className="recap-head">
            <span className="recap-period">{label(current)}</span>
            <span className="muted">{current.entry_count} entries</span>
          </div>
          <p className="recap-text">{current.content}</p>
        </article>
      )}

      {past.length > 0 && <h2 className="recaps-h2">Past recaps</h2>}
      {past
        .filter((r) => !current || r.created_at !== undefined)
        .map((r) => (
          <article className="recap-card" key={r.id}>
            <div className="recap-head">
              <span className="recap-period">{label(r)}</span>
              <span className="muted">{r.entry_count} entries</span>
            </div>
            <p className="recap-text">{r.content}</p>
          </article>
        ))}

      {past.length === 0 && !current && (
        <p className="muted">No recaps yet. Generate one above — it summarizes what you logged.</p>
      )}
    </div>
  );
}

function label(r: { period: string; period_start: string; period_end: string }) {
  if (r.period === "day") return r.period_end;
  return `${r.period_start} → ${r.period_end}`;
}
