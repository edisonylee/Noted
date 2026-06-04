import { useEffect, useState } from "react";
import { listen } from "./events";
import { RefreshCw } from "lucide-react";
import { api, type RecapRow } from "./api";
import { dayDiff, easternDay, formatDay } from "./day";

export function RecapsView() {
  const [recaps, setRecaps] = useState<RecapRow[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const refresh = () => api.listRecaps().then(setRecaps).catch((e) => setErr(String(e)));

  useEffect(() => {
    refresh();
    // auto-generated recaps notify us when they land
    const un = listen("recap-generated", () => refresh());
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function checkNow() {
    setBusy(true);
    setErr(null);
    try {
      await api.backfillRecaps();
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="recaps">
      <div className="recaps-head">
        <p className="muted">Recaps write themselves at the end of each day and week.</p>
        <button className="link" onClick={checkNow} disabled={busy}>
          <RefreshCw size={13} /> {busy ? "checking…" : "Check now"}
        </button>
      </div>

      {err && <div className="error">{err}</div>}

      {recaps.length === 0 && !busy && (
        <p className="muted">
          Nothing yet — once you&rsquo;ve logged a full day, yesterday&rsquo;s recap shows up here on its own.
        </p>
      )}

      {recaps.map((r, i) => (
        <article className={"recap-card" + (i === 0 ? " fresh" : "")} key={r.id}>
          <div className="recap-head">
            <span className="recap-period">{label(r)}</span>
            <span className="muted">{r.entry_count} entries</span>
          </div>
          <p className="recap-text">{r.content}</p>
        </article>
      ))}
    </div>
  );
}

function fmt(dateStr: string, opts: Intl.DateTimeFormatOptions) {
  return formatDay(dateStr, opts);
}

function relDay(dateStr: string): string {
  const diff = dayDiff(easternDay(), dateStr);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return fmt(dateStr, { weekday: "long", month: "long", day: "numeric" });
}

function label(r: { period: string; period_start: string; period_end: string }): string {
  if (r.period === "day") return relDay(r.period_end);
  const s = fmt(r.period_start, { month: "short", day: "numeric" });
  const e = fmt(r.period_end, { month: "short", day: "numeric" });
  return `Week of ${s} – ${e}`;
}
