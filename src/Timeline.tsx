import { useState } from "react";
import { Camera, ChevronRight } from "lucide-react";
import { DataView } from "./DataView";
import type { NoteRow } from "./api";

// "Today" / "Yesterday" / "Monday, June 1" (year only if not the current year)
function relativeDay(dateStr: string): string {
  const d = new Date(dateStr + "T00:00:00");
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diff = Math.round((today.getTime() - d.getTime()) / 86_400_000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
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
      {groups.map((g) => (
        <section className="tl-group" key={g.date}>
          <h3 className="tl-date">{relativeDay(g.date)}</h3>
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
