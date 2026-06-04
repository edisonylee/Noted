import { useEffect, useState } from "react";
import { Camera, ChevronRight, RefreshCw } from "lucide-react";
import { listen } from "./events";
import { DataView } from "./DataView";
import { api, type NoteRow, type RecapRow } from "./api";
import { dayDiff, easternDay, formatDay } from "./day";

// "Today" / "Yesterday" / "Monday, June 1" (year only if not the current year)
function relativeDay(dateStr: string): string {
  const today = easternDay();
  const diff = dayDiff(today, dateStr);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  const sameYear = dateStr.slice(0, 4) === today.slice(0, 4);
  return formatDay(dateStr, {
    weekday: "long",
    month: "long",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

// Short, dated label for a recap card woven into the feed.
function fmtDate(dateStr: string, opts: Intl.DateTimeFormatOptions): string {
  return formatDay(dateStr, opts);
}
function recapLabel(r: RecapRow): string {
  if (r.period === "week") {
    const s = fmtDate(r.period_start, { month: "short", day: "numeric" });
    const e = fmtDate(r.period_end, { month: "short", day: "numeric" });
    return `Week of ${s} – ${e}`;
  }
  return "Day recap";
}

const LABEL_KEYS = ["name", "task", "title", "label", "exercise", "activity", "item", "food", "meal"];

// A short, scannable one-liner for a collapsed entry.
function summarize(data: Record<string, unknown> | null): string {
  if (!data) return "";
  for (const v of Object.values(data)) {
    if (Array.isArray(v) && v.length && typeof v[0] === "object" && v[0]) {
      const labels = v
        .map((it) => {
          const o = it as Record<string, unknown>;
          const k = LABEL_KEYS.find((key) => typeof o[key] === "string");
          return k ? (o[k] as string) : null;
        })
        .filter(Boolean) as string[];
      if (labels.length) return labels.slice(0, 6).join(", ");
    }
  }
  const scalars = Object.entries(data)
    .filter(([, v]) => v !== null && typeof v !== "object")
    .map(([, v]) => String(v));
  return scalars.slice(0, 4).join(" · ");
}

export function TimelineView({ notes }: { notes: NoteRow[] }) {
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const toggle = (id: number) =>
    setExpanded((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  // Recaps live inline in the feed now (no separate tab). Fetch them and slot
  // each under the day it covers (period_end). They auto-generate and notify.
  const [recaps, setRecaps] = useState<RecapRow[]>([]);
  const [checking, setChecking] = useState(false);
  useEffect(() => {
    const load = () => api.listRecaps().then(setRecaps).catch(() => {});
    load();
    const un = listen("recap-generated", load);
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function checkRecaps() {
    setChecking(true);
    try {
      await api.backfillRecaps();
      setRecaps(await api.listRecaps());
    } catch {
      /* surfaced elsewhere; keep the timeline quiet */
    } finally {
      setChecking(false);
    }
  }

  // Week recaps sort before day recaps within the same day.
  const recapsByDay = new Map<string, RecapRow[]>();
  for (const r of recaps) {
    const arr = recapsByDay.get(r.period_end) ?? [];
    arr.push(r);
    arr.sort((a, b) => (a.period === b.period ? 0 : a.period === "week" ? -1 : 1));
    recapsByDay.set(r.period_end, arr);
  }

  if (!notes.length) {
    return <p className="muted">Nothing logged yet. Brain-dump a note in Log to start your timeline.</p>;
  }

  // notes arrive newest-first by event_date; group consecutive same-day runs.
  const groups: { date: string; items: NoteRow[] }[] = [];
  for (const n of notes) {
    const last = groups[groups.length - 1];
    if (last && last.date === n.event_date) last.items.push(n);
    else groups.push({ date: n.event_date, items: [n] });
  }

  return (
    <div className="tl">
      <div className="tl-top">
        <button className="link" onClick={checkRecaps} disabled={checking}>
          <RefreshCw size={13} /> {checking ? "checking…" : "Check recaps"}
        </button>
      </div>
      {groups.map((g) => (
        <section className="tl-group" key={g.date}>
          <h3 className="tl-date">{relativeDay(g.date)}</h3>
          {(recapsByDay.get(g.date) ?? []).map((r) => (
            <article className="recap-card inline" key={"recap-" + r.id}>
              <div className="recap-head">
                <span className="recap-period">{recapLabel(r)}</span>
                <span className="muted">{r.entry_count} entries</span>
              </div>
              <p className="recap-text">{r.content}</p>
            </article>
          ))}
          {g.items.map((n) => {
            const open = expanded.has(n.id);
            const summary = n.entries.map((e) => summarize(e.data)).filter(Boolean).join(" · ");
            return (
              <div className={"tl-item" + (open ? " open" : "")} key={n.id}>
                <button className="tl-head" onClick={() => toggle(n.id)}>
                  <ChevronRight size={15} className="chev" />
                  <span className="tl-cats">
                    {(n.entries.length ? n.entries : [{ category: "uncategorized", data: null }]).map(
                      (e, i) => (
                        <span className="tl-cat" key={i}>
                          {e.category ?? "uncategorized"}
                        </span>
                      )
                    )}
                  </span>
                  {summary && <span className="tl-sum">{summary}</span>}
                  {n.source === "photo" && <Camera size={12} className="tl-src" />}
                </button>
                {open && (
                  <div className="tl-body">
                    {n.entries.map((e, i) => (
                      <div className="tl-entry" key={i}>
                        {n.entries.length > 1 && (
                          <div className="tl-entry-cat">{e.category ?? "uncategorized"}</div>
                        )}
                        {e.data && <DataView value={e.data} />}
                      </div>
                    ))}
                    <details className="raw">
                      <summary>raw note</summary>
                      <pre>{n.raw_text}</pre>
                    </details>
                  </div>
                )}
              </div>
            );
          })}
        </section>
      ))}
    </div>
  );
}
