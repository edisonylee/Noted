import { useEffect, useRef, useState } from "react";
import { CalendarDays, Camera, Loader, Pencil, Plus, X } from "lucide-react";
import { api, type EntityCandidate, type NoteRow } from "./api";
import { fileToImg, type Img } from "./image";

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

// A single-line time label: "10:40 AM", "8:30–10:30 AM" (shared meridiem), or
// "11:40 AM–12:15 PM" when the range crosses noon.
function fmtRange(start?: string, end?: string): string {
  const s = toMinutes(start);
  if (s == null) return "";
  if (!end) return fmtTime(start);
  const e = toMinutes(end);
  if (e == null) return fmtTime(start);
  const sameHalf = s < 720 === e < 720; // both AM or both PM
  if (sameHalf) return `${fmtTime(start).replace(/\s?(AM|PM)$/i, "")}–${fmtTime(end)}`;
  return `${fmtTime(start)}–${fmtTime(end)}`;
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

// "0830" minutes-of-day -> "08:30". Wraps into [0,1440).
function minToStr(m: number): string {
  const mm = ((Math.round(m) % 1440) + 1440) % 1440;
  return `${String(Math.floor(mm / 60)).padStart(2, "0")}:${String(mm % 60).padStart(2, "0")}`;
}

// Resolve a clock value (hour, minute, am/pm) to minutes-of-day. With no am/pm we
// assume the day moves forward (monotonic): pick the smallest reading that isn't
// before `prevMin`, so a bare "2:00" after noon lands at 14:00, not 02:00.
function pickMonotonic(hour: number, min: number, ap: "a" | "p" | null, prevMin: number): number {
  if (ap === "a") return (hour === 12 ? 0 : hour) * 60 + min;
  if (ap === "p") return (hour === 12 ? 12 : hour + 12) * 60 + min;
  if (hour >= 13) return hour * 60 + min; // already 24h, e.g. "18:30"
  const base = (hour % 12) * 60 + min; // 12 -> 0
  if (base >= prevMin) return base;
  if (base + 720 >= prevMin) return base + 720;
  return base + 720;
}

// Parse one leading time token ("8", "8:30", "8:30pm", "930", "1145").
function resolveTime(tok: string, prevMin: number): number | null {
  const t = tok.trim();
  const m = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm|a|p)?$/i.exec(t);
  if (m) {
    const hour = Number(m[1]);
    const min = m[2] ? Number(m[2]) : 0;
    if (hour > 23 || min > 59) return null;
    const ap = m[3] ? (m[3][0].toLowerCase() as "a" | "p") : null;
    return pickMonotonic(hour, min, ap, prevMin);
  }
  const d = /^(\d{3,4})$/.exec(t); // colon-less "930" / "1145"
  if (d) {
    const n = d[1];
    const hh = Number(n.slice(0, n.length - 2));
    const mm = Number(n.slice(-2));
    if (hh > 23 || mm > 59) return null;
    return pickMonotonic(hh, mm, null, prevMin);
  }
  return null;
}

// am/pm (or bare a/p) only counts as a meridiem when not glued to a word, so
// "2:00 profound" keeps its "p" instead of reading it as "2:00 p.m.".
const TIME_TOK = "\\d{1,2}(?::\\d{2})?(?:\\s*(?:am|pm|a|p)(?![a-z]))?|\\d{3,4}";
const LINE_RE = new RegExp(`^\\s*~?\\s*(${TIME_TOK})\\s*(?:(?:[-–—]|to)\\s*~?\\s*(${TIME_TOK}))?`, "i");

// Turn a typed/transcribed schedule into ordered blocks — deterministic, no LLM.
// Lines that start with a clock time become timed blocks; everything else is an
// "Anytime" task. The whole point is reliability: this never hard-fails.
function parseSchedule(text: string): Block[] {
  const blocks: Block[] = [];
  let prev = -1;
  for (const raw of text.split("\n")) {
    const line = raw.trim().replace(/^[-*•]\s+/, "");
    if (!line) continue;
    const m = LINE_RE.exec(line);
    const start = m?.[1] ? resolveTime(m[1], prev) : null;
    if (m && start != null) {
      const end = m[2] ? resolveTime(m[2], start) : null;
      const task = line.slice(m[0].length).replace(/^[\s:.,–—-]+/, "").trim();
      if (!task) continue;
      const b: Block = { task, start: minToStr(start) };
      if (end != null && end > start) {
        b.end = minToStr(end);
        b.duration_min = end - start;
      }
      blocks.push(b);
      prev = end ?? start;
    } else {
      blocks.push({ task: line });
    }
  }
  return blocks;
}

const PLACEHOLDER = `Lay out your day, however messy:

woke up 7:30, gym 8–9
deep work on the today view 10–12
lunch w/ sam 12:30
errands 2–4, call the dentist sometime
dinner 7, wind down by 10`;

export function TodayView({
  notes,
  onSaved,
}: {
  notes: NoteRow[];
  onSaved: () => void | Promise<void>;
}) {
  // Re-render each minute so the "now" highlight stays accurate through the day.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  // Inline create/edit state — the schedule is made right here, not in Log.
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [photo, setPhoto] = useState<Img | null>(null);
  const [busy, setBusy] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

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

  function openEditor() {
    setDraft(note?.raw_text ?? "");
    setPhoto(null);
    setEditError(null);
    setEditing(true);
  }

  async function attachPhoto(file: File | undefined) {
    if (!file) return;
    try {
      setPhoto(await fileToImg(file));
      setEditError(null);
    } catch (e) {
      setEditError(String(e));
    }
  }

  async function saveDraft() {
    if (busy) return;
    setBusy(true);
    setEditError(null);
    try {
      let body = draft.trim();
      let source = "text";
      let image_path: string | null = null;
      let entities: EntityCandidate[] = [];

      // Photo: use the vision model ONLY to transcribe the handwriting to text,
      // then parse it the same deterministic way as typed input.
      if (photo) {
        const env = await api.categorizePhoto(photo.base64);
        body = (env.raw_text ?? "").trim() || body;
        source = "photo";
        image_path = await api.saveImage(photo.base64, photo.ext);
        entities = env.entities; // keep any people the photo surfaced
      }

      if (!body) {
        setBusy(false);
        return;
      }

      // Deterministic parse — instant, offline, never hard-fails on the model.
      const blocks = parseSchedule(body);
      if (!blocks.length) {
        setEditError('Couldn’t find anything to schedule. Try lines like "9:00 gym" or "2–4pm errands".');
        setBusy(false);
        return;
      }

      await api.save({
        raw_text: body,
        source,
        image_path,
        event_date: today, // always today's schedule
        entries: [{ category: "schedule", description: "today's schedule", data: { blocks } }],
        entities,
      });

      setEditing(false);
      setDraft("");
      setPhoto(null);
      await onSaved();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const Head = (withEdit: boolean) => (
    <header className="today-head">
      <div className="today-headrow">
        <div>
          <div className="today-eyebrow">Today</div>
          <h1 className="today-date">{dateLine}</h1>
        </div>
        {withEdit && (
          <button className="today-edit" onClick={openEditor} aria-label="Edit schedule">
            <Pencil size={14} /> Edit
          </button>
        )}
      </div>
    </header>
  );

  // ---- Editor ----
  if (editing) {
    return (
      <div className="today">
        {Head(false)}
        <div className="today-editor">
          <textarea
            className="today-textarea"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={PLACEHOLDER}
            autoFocus
            disabled={busy}
          />

          {photo && (
            <div className="today-photo">
              <img src={photo.dataUrl} alt="schedule" />
              <button className="today-photo-x" onClick={() => setPhoto(null)} aria-label="Remove photo">
                <X size={14} />
              </button>
            </div>
          )}

          {editError && <div className="error">{editError}</div>}

          <div className="today-editor-actions">
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              capture="environment"
              hidden
              onChange={(e) => attachPhoto(e.target.files?.[0])}
            />
            <button
              className="today-photo-btn"
              onClick={() => fileRef.current?.click()}
              disabled={busy}
              title="Snap a handwritten schedule"
            >
              <Camera size={16} /> Photo
            </button>
            <span className="today-spacer" />
            <button
              className="today-cancel"
              onClick={() => {
                setEditing(false);
                setEditError(null);
              }}
              disabled={busy}
            >
              Cancel
            </button>
            <button
              className="today-save"
              onClick={saveDraft}
              disabled={busy || (!draft.trim() && !photo)}
            >
              {busy ? (
                <>
                  <Loader size={15} className="spin" /> Building…
                </>
              ) : (
                "Save schedule"
              )}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ---- Empty state ----
  if (!blocks.length) {
    return (
      <div className="today">
        {Head(false)}
        <div className="today-empty">
          <CalendarDays size={30} className="today-empty-icon" />
          <p className="today-empty-title">No schedule yet for today</p>
          <p className="today-empty-sub">
            Lay out your plan — type it from when you wake up to when you wind down, or snap a
            photo of a handwritten schedule. noted lays it out here, time by time.
          </p>
          <button className="today-make" onClick={openEditor}>
            <Plus size={16} /> Make today&apos;s schedule
          </button>
        </div>
      </div>
    );
  }

  // ---- Agenda ----
  const nowMin = now.getHours() * 60 + now.getMinutes();

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
      {Head(true)}

      <div className="today-agenda">
        {rows.map((r, i) => {
          const isNow = i === currentIdx;
          const isPast = !isNow && nowMin >= r.effEnd;
          return (
            <div
              key={i}
              className={"today-block" + (isNow ? " now" : "") + (isPast ? " past" : "")}
            >
              <div className="today-time">{fmtRange(r.b.start, r.b.end)}</div>
              <div className="today-rail">
                <span className="today-dot" />
              </div>
              <div className="today-info">
                <span className="today-task">{r.b.task}</span>
                {fmtDur(r.b.duration_min) && (
                  <span className="today-dur">{fmtDur(r.b.duration_min)}</span>
                )}
                {isNow && <span className="today-nowtag">now</span>}
              </div>
            </div>
          );
        })}

        {untimed.length > 0 && (
          <div className="today-untimed">
            <div className="today-untimed-label">Anytime</div>
            {untimed.map((b, i) => (
              <div className="today-block untimed" key={i}>
                <div className="today-time today-dash">—</div>
                <div className="today-rail">
                  <span className="today-dot" />
                </div>
                <div className="today-info">
                  <span className="today-task">{b.task}</span>
                  {fmtDur(b.duration_min) && (
                    <span className="today-dur">{fmtDur(b.duration_min)}</span>
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
