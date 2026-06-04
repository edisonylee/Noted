import { useEffect, useState } from "react";
import { CalendarDays, Plus } from "lucide-react";
import type { NoteRow } from "./api";

// Local YYYY-MM-DD. NOT toISOString() — that's UTC and would roll the day over
// late at night, showing tomorrow's (empty) schedule before midnight.
function localDay(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

type Block = {
  task: string;
  start?: string; // HH:MM (24h)
  end?: string; // HH:MM (24h)
  duration_min?: number;
};

// Minutes since midnight for an "HH:MM" string, or null if absent/unparseable.
function toMinutes(hhmm?: string): number | null {
  if (!hhmm) return null;
  const m = /^(\d{1,2}):(\d{2})$/.exec(hhmm.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h > 23 || min > 59) return null;
  return h * 60 + min;
}

// "9:00 AM" — falls back to the raw value if it isn't HH:MM.
function fmtTime(hhmm?: string): string {
  const mins = toMinutes(hhmm);
  if (mins == null) return hhmm ?? "";
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  const ampm = h < 12 ? "AM" : "PM";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return `${h12}:${String(m).padStart(2, "0")} ${ampm}`;
}

// "2h", "45m", "1h 30m"
function fmtDur(min?: number): string {
  if (!min || min <= 0) return "";
  const h = Math.floor(min / 60);
  const m = min % 60;
  if (h && m) return `${h}h ${m}m`;
  if (h) return `${h}h`;
  return `${m}m`;
}

// Pull a clean Block[] out of an entry's `data.blocks`, tolerating missing/odd fields.
function parseBlocks(data: Record<string, unknown> | null | undefined): Block[] {
  const raw = data?.blocks;
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((b): b is Record<string, unknown> => !!b && typeof b === "object")
    .map((b) => ({
      task:
        typeof b.task === "string"
          ? b.task
          : typeof b.activity === "string"
            ? b.activity
            : typeof b.name === "string"
              ? b.name
              : "",
      start: typeof b.start === "string" ? b.start : undefined,
      end: typeof b.end === "string" ? b.end : undefined,
      duration_min: typeof b.duration_min === "number" ? b.duration_min : undefined,
    }))
    .filter((b) => b.task);
}

const isSchedule = (cat: string | null) => cat?.toLowerCase() === "schedule";

export function TodayView({
  notes,
  onMakeSchedule,
}: {
  notes: NoteRow[];
  onMakeSchedule: () => void;
}) {
  // Re-render each minute so the "now" highlight stays accurate through the day.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  const today = localDay(now);
  const dateLine = now.toLocaleDateString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
  });

  // notes are newest-first (db.rs orders by event_date, id DESC), so the first
  // match is today's latest schedule — re-captures naturally win, no merging.
  const note = notes.find(
    (n) => n.event_date === today && n.entries.some((e) => isSchedule(e.category))
  );
  const entry = note?.entries.find((e) => isSchedule(e.category));
  const blocks = parseBlocks(entry?.data);

  const Head = (
    <header className="today-head">
      <div className="today-eyebrow">Today</div>
      <h1 className="today-date">{dateLine}</h1>
    </header>
  );

  if (!blocks.length) {
    return (
      <div className="today">
        {Head}
        <div className="today-empty">
          <CalendarDays size={30} className="today-empty-icon" />
          <p className="today-empty-title">No schedule yet for today</p>
          <p className="today-empty-sub">
            Lay out your plan — type it, speak it, or snap a photo of a handwritten schedule.
            noted will lay it out here, time by time.
          </p>
          <button className="today-make" onClick={onMakeSchedule}>
            <Plus size={16} /> Make today&apos;s schedule
          </button>
        </div>
      </div>
    );
  }

  const nowMin = now.getHours() * 60 + now.getMinutes();

  // Timed blocks sorted by start; untimed kept aside.
  const rows = blocks
    .map((b) => ({ b, start: toMinutes(b.start) }))
    .filter((x): x is { b: Block; start: number } => x.start != null)
    .sort((a, z) => a.start - z.start)
    .map((x, i, arr) => {
      // Effective end: explicit end, else next block's start, else +1h — used to
      // decide which block is "now" and which have already passed.
      const end = toMinutes(x.b.end);
      const nextStart = arr[i + 1]?.start ?? null;
      const effEnd = end ?? nextStart ?? x.start + 60;
      return { ...x, effEnd };
    });
  const untimed = blocks.filter((b) => toMinutes(b.start) == null);

  const currentIdx = rows.findIndex((r) => nowMin >= r.start && nowMin < r.effEnd);

  return (
    <div className="today">
      {Head}

      <div className="today-agenda">
        {rows.map((r, i) => {
          const isNow = i === currentIdx;
          const isPast = !isNow && nowMin >= r.effEnd;
          return (
            <div
              key={i}
              className={"today-block" + (isNow ? " now" : "") + (isPast ? " past" : "")}
            >
              <div className="today-time">
                <span className="today-start">{fmtTime(r.b.start)}</span>
                {r.b.end && <span className="today-end">{fmtTime(r.b.end)}</span>}
              </div>
              <div className="today-rail">
                <span className="today-dot" />
              </div>
              <div className="today-info">
                <span className="today-task">{r.b.task}</span>
                <span className="today-meta">
                  {fmtDur(r.b.duration_min) && (
                    <span className="today-dur">{fmtDur(r.b.duration_min)}</span>
                  )}
                  {isNow && <span className="today-nowtag">now</span>}
                </span>
              </div>
            </div>
          );
        })}

        {untimed.length > 0 && (
          <div className="today-untimed">
            <div className="today-untimed-label">Anytime</div>
            {untimed.map((b, i) => (
              <div className="today-block untimed" key={i}>
                <div className="today-time" />
                <div className="today-rail">
                  <span className="today-dot" />
                </div>
                <div className="today-info">
                  <span className="today-task">{b.task}</span>
                  {fmtDur(b.duration_min) && (
                    <span className="today-meta">
                      <span className="today-dur">{fmtDur(b.duration_min)}</span>
                    </span>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {note?.raw_text && (
        <details className="today-notes">
          <summary>notes</summary>
          <pre>{note.raw_text}</pre>
        </details>
      )}
    </div>
  );
}
