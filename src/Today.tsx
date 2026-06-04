import { useEffect, useRef, useState } from "react";
import { CalendarCheck, CalendarDays, Camera, Check, Loader, Pencil, Plus, Trash2, X } from "lucide-react";
import { api, type EntityCandidate, type NoteRow } from "./api";
import { fileToImg, type Img } from "./image";
import { APP_TZ, easternDay, easternMinutes } from "./day";

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
export function parseBlocks(data: Record<string, unknown> | null | undefined): Block[] {
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
    // Drop empties and date-only header blocks. Filtering on load (not just on
    // parse) also cleans schedules saved before date-stripping existed, so a
    // stored "June 4, 2026" stops showing up under "Anytime".
    .filter((b) => b.task && !isDateOnly(b.task));
}

const isSchedule = (cat: string | null) => cat?.toLowerCase() === "schedule";

// Lay timed blocks out in the order they were authored, rolling any backward
// time-jump forward across midnight: a small clock value written after a later
// one ("12am" or "1am" after "10pm") is the next morning, not the start of
// today, so "bedtime" stays at the bottom where it was typed instead of sorting
// 00:00 to the very top. Returns absolute start/effEnd minutes. The stored array
// is already in authored order, so no separate sort is needed and it survives
// save/reload.
export function layoutRows(blocks: Block[]): { b: Block; start: number; effEnd: number }[] {
  const timed = blocks
    .map((b) => ({ b, clock: toMinutes(b.start) }))
    .filter((x): x is { b: Block; clock: number } => x.clock != null);

  let carry = 0; // minutes of accumulated day rollover
  let prevStart = -1;
  return timed
    .map((x) => {
      if (carry + x.clock < prevStart) carry += 1440;
      const start = carry + x.clock;
      prevStart = start;
      // End shares the start's day; if it reads earlier it crosses midnight too.
      const endClock = toMinutes(x.b.end);
      const end =
        endClock == null ? null : endClock < x.clock ? carry + endClock + 1440 : carry + endClock;
      return { b: x.b, start, end };
    })
    .map((x, i, arr) => {
      // Effective end: explicit end, else next block's start, else +1h — used to
      // decide which block is "now" and which have already passed.
      const nextStart = arr[i + 1]?.start ?? null;
      const effEnd = x.end ?? nextStart ?? x.start + 60;
      return { b: x.b, start: x.start, effEnd };
    });
}

// Serialize blocks back to the text form parseSchedule understands, so inline
// edits keep raw_text (and therefore the text/photo editor) in sync with the
// structured blocks. "HH:MM" round-trips cleanly through resolveTime.
function blocksToText(blocks: Block[]): string {
  return blocks
    .map((b) => {
      if (!b.start) return b.task;
      const time = b.end ? `${b.start}-${b.end}` : b.start;
      return `${time} ${b.task}`;
    })
    .join("\n");
}

// Inline editing manipulates one explicit day, so order timed blocks by clock
// (untimed kept last, in their existing order). This is plain chronological
// sort — the next-day rollover in layoutRows is a heuristic for ambiguous typed
// text, not for times the user set directly with a picker.
function sortBlocks(blocks: Block[]): Block[] {
  const at = (b: Block) => toMinutes(b.start);
  const timed = blocks.filter((b) => at(b) != null).sort((a, z) => at(a)! - at(z)!);
  const untimed = blocks.filter((b) => at(b) == null);
  return [...timed, ...untimed];
}

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

// Longest-first so the alternation matches full names before abbreviations.
const WEEKDAY =
  "(?:monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues|tue|weds|wed|thurs|thur|thu|fri|sat|sun)";
const MONTH =
  "(?:january|february|march|april|may|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sept|sep|oct|nov|dec)";

// A line that is *only* a date ("Thursday, June 4th", "6/4/2026", "2026-06-04").
// These ride along in photos to confirm the day — they're a sanity check, not a
// task, so we drop them rather than parking them under "Anytime". We require a
// real date signal (month/weekday/numeric date) AND that nothing survives once
// the date pieces are stripped, so "dentist on the 4th" stays a task.
function isDateOnly(line: string): boolean {
  const s = line.trim().toLowerCase();
  if (!s) return false;
  const hasSignal =
    new RegExp(`\\b${MONTH}\\b`).test(s) ||
    new RegExp(`\\b${WEEKDAY}\\b`).test(s) ||
    /\b\d{1,2}\/\d{1,2}(?:\/\d{2,4})?\b/.test(s) ||
    /\b\d{4}-\d{1,2}-\d{1,2}\b/.test(s);
  if (!hasSignal) return false;
  const rest = s
    .replace(new RegExp(`\\b${WEEKDAY}\\b`, "g"), " ")
    .replace(new RegExp(`\\b${MONTH}\\b`, "g"), " ")
    .replace(/\b\d{1,2}(?:st|nd|rd|th)?\b/g, " ") // day-of-month or ordinal
    .replace(/\b\d{4}\b/g, " ") // year
    .replace(/\b(?:of|the)\b/g, " ")
    .replace(/[/.,–—-]/g, " ")
    .replace(/\s+/g, "")
    .trim();
  return rest === "";
}

// Turn a typed/transcribed schedule into ordered blocks — deterministic, no LLM.
// Lines that start with a clock time become timed blocks; everything else is an
// "Anytime" task. The whole point is reliability: this never hard-fails.
export function parseSchedule(text: string): Block[] {
  const blocks: Block[] = [];
  let prev = -1;
  for (const raw of text.split("\n")) {
    const line = raw.trim().replace(/^[-*•]\s+/, "");
    if (!line) continue;
    if (isDateOnly(line)) continue;
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

// Inline row editor: start/end time pickers + task field, with save/delete/cancel.
// Used both for editing an existing block and for adding a new one. Enter saves,
// Escape cancels. Returns a fully-formed Block (duration recomputed) to onCommit.
function ScheduleRowForm({
  init,
  busy,
  onCommit,
  onCancel,
  onDelete,
}: {
  init: Block;
  busy: boolean;
  onCommit: (b: Block) => void;
  onCancel: () => void;
  onDelete?: () => void;
}) {
  const [start, setStart] = useState(init.start ?? "");
  const [end, setEnd] = useState(init.end ?? "");
  const [task, setTask] = useState(init.task ?? "");

  function commit() {
    const t = task.trim();
    if (!t) return;
    const b: Block = { task: t };
    if (start) {
      b.start = start;
      const s = toMinutes(start);
      const e = end ? toMinutes(end) : null;
      if (s != null && e != null && e > s) {
        b.end = end;
        b.duration_min = e - s;
      }
    }
    onCommit(b);
  }

  return (
    <div className="today-block today-rowedit">
      <div className="today-rowedit-times">
        <input
          type="time"
          className="today-timeinput"
          value={start}
          disabled={busy}
          onChange={(e) => setStart(e.target.value)}
          aria-label="Start time"
        />
        <span className="today-rowedit-dash">–</span>
        <input
          type="time"
          className="today-timeinput"
          value={end}
          disabled={busy || !start}
          onChange={(e) => setEnd(e.target.value)}
          aria-label="End time"
        />
      </div>
      <input
        className="today-taskinput"
        value={task}
        disabled={busy}
        autoFocus
        placeholder="What's happening?"
        onChange={(e) => setTask(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") onCancel();
        }}
      />
      <div className="today-rowedit-actions">
        <button className="today-rowbtn save" onClick={commit} disabled={busy || !task.trim()} title="Save (Enter)">
          <Check size={15} />
        </button>
        {onDelete && (
          <button className="today-rowbtn del" onClick={onDelete} disabled={busy} title="Delete">
            <Trash2 size={15} />
          </button>
        )}
        <button className="today-rowbtn" onClick={onCancel} disabled={busy} title="Cancel (Esc)">
          <X size={15} />
        </button>
      </div>
    </div>
  );
}

export function TodayView({
  notes,
  onSaved,
  onOpenSettings,
}: {
  notes: NoteRow[];
  onSaved: () => void | Promise<void>;
  onOpenSettings?: () => void;
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

  // Inline row editing on the agenda: which block index is open, an "add new"
  // flag, and a save-in-flight flag shared by both.
  const [editIdx, setEditIdx] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const [rowBusy, setRowBusy] = useState(false);

  // Google Calendar sync: whether we're connected, and the last sync's outcome.
  const [gcalConnected, setGcalConnected] = useState<boolean | null>(null);
  const [sync, setSync] = useState<{ state: "idle" | "syncing" | "ok" | "err"; msg: string }>({
    state: "idle",
    msg: "",
  });
  useEffect(() => {
    api.gcalAuthStatus().then((st) => setGcalConnected(st.connected)).catch(() => setGcalConnected(false));
  }, []);

  const today = easternDay(now);
  const dateLine = now.toLocaleDateString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    timeZone: APP_TZ,
  });

  // notes are newest-first (db.rs orders by event_date, id DESC), so the first
  // match is today's latest schedule — re-captures naturally win, no merging.
  const note = notes.find(
    (n) => n.event_date === today && n.entries.some((e) => isSchedule(e.category))
  );
  const entry = note?.entries.find((e) => isSchedule(e.category));
  const blocks = parseBlocks(entry?.data);
  // Only timed blocks can become calendar events; "Anytime" tasks are skipped.
  const hasTimed = blocks.some((b) => toMinutes(b.start) != null);

  // Push today's schedule to Google Calendar. Not connected → open Settings.
  async function syncToGcal() {
    if (!gcalConnected) {
      onOpenSettings?.();
      return;
    }
    if (sync.state === "syncing") return;
    setSync({ state: "syncing", msg: "" });
    try {
      const r = await api.gcalSync(today);
      const pushed = r.created + r.updated;
      const parts = [`Synced ${pushed} block${pushed === 1 ? "" : "s"}`];
      if (r.skipped) parts.push(`${r.skipped} anytime skipped`);
      if (r.deleted) parts.push(`${r.deleted} removed`);
      if (r.errors.length) {
        setSync({
          state: "err",
          msg: `${parts.join(" · ")} — ${r.errors.length} error${r.errors.length === 1 ? "" : "s"}: ${r.errors[0]}`,
        });
      } else {
        setSync({ state: "ok", msg: parts.join(" · ") });
      }
    } catch (e) {
      setSync({ state: "err", msg: String(e) });
    }
  }

  function openEditor() {
    // Inline edits update blocks in place but leave raw_text as the original
    // capture. If they've diverged, seed the text editor from the current blocks
    // so it can't overwrite an inline edit with stale text; otherwise keep the
    // user's original phrasing.
    const fromRaw = note?.raw_text ?? "";
    const diverged = blocksToText(parseSchedule(fromRaw)) !== blocksToText(blocks);
    setDraft(diverged ? blocksToText(blocks) : fromRaw);
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

  // Persist inline edits. If today's schedule entry already exists we update it
  // IN PLACE (update_entry) so the same DB row changes — Today and the Timeline
  // reflect the edit with no duplicate note, and the note is re-embedded. Only
  // when there's no schedule yet do we insert a fresh note.
  async function persistBlocks(next: Block[]) {
    if (rowBusy) return;
    setRowBusy(true);
    setEditError(null);
    try {
      const cleaned = sortBlocks(next.filter((b) => b.task.trim()));
      if (entry?.id != null) {
        await api.updateEntry(entry.id, { blocks: cleaned });
      } else {
        await api.save({
          raw_text: blocksToText(cleaned),
          source: note?.source ?? "text",
          event_date: today,
          entries: [{ category: "schedule", description: "today's schedule", data: { blocks: cleaned } }],
        });
      }
      setEditIdx(null);
      setAdding(false);
      await onSaved();
    } catch (e) {
      setEditError(String(e));
    } finally {
      setRowBusy(false);
    }
  }

  const commitEdit = (idx: number, b: Block) => persistBlocks(blocks.map((x, i) => (i === idx ? b : x)));
  const deleteBlock = (idx: number) => persistBlocks(blocks.filter((_, i) => i !== idx));
  const commitAdd = (b: Block) => persistBlocks([...blocks, b]);
  const beginEdit = (idx: number) => {
    setAdding(false);
    setEditError(null);
    setEditIdx(idx);
  };

  const Head = (withEdit: boolean) => (
    <header className="today-head">
      <div className="today-headrow">
        <div>
          <div className="today-eyebrow">Today</div>
          <h1 className="today-date">{dateLine}</h1>
        </div>
        {withEdit && (
          <div className="today-headbtns">
            <button
              className="today-edit"
              onClick={syncToGcal}
              disabled={sync.state === "syncing" || (!!gcalConnected && !hasTimed)}
              title={
                gcalConnected
                  ? "Push today's schedule to Google Calendar"
                  : "Connect Google Calendar in Settings"
              }
            >
              {sync.state === "syncing" ? (
                <Loader size={14} className="spin" />
              ) : (
                <CalendarCheck size={14} />
              )}
              {gcalConnected === false ? "Connect Calendar" : "Sync"}
            </button>
            <button className="today-edit" onClick={openEditor} aria-label="Edit schedule">
              <Pencil size={14} /> Edit
            </button>
          </div>
        )}
      </div>
      {withEdit && sync.state !== "idle" && sync.state !== "syncing" && (
        <div className={"today-syncmsg " + sync.state}>{sync.msg}</div>
      )}
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
              accept="image/*,.heic,.heif"
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
  const nowMin = easternMinutes(now);

  const rows = layoutRows(blocks);
  const untimed = blocks.filter((b) => toMinutes(b.start) == null);

  const currentIdx = rows.findIndex((r) => nowMin >= r.start && nowMin < r.effEnd);

  return (
    <div className="today">
      {Head(true)}

      <div className="today-agenda">
        {rows.map((r, i) => {
          const idx = blocks.indexOf(r.b);
          if (idx === editIdx) {
            return (
              <ScheduleRowForm
                key={`edit-${idx}`}
                init={r.b}
                busy={rowBusy}
                onCommit={(b) => commitEdit(idx, b)}
                onCancel={() => setEditIdx(null)}
                onDelete={() => deleteBlock(idx)}
              />
            );
          }
          const isNow = i === currentIdx;
          const isPast = !isNow && nowMin >= r.effEnd;
          return (
            <div
              key={i}
              className={"today-block editable" + (isNow ? " now" : "") + (isPast ? " past" : "")}
              onDoubleClick={() => beginEdit(idx)}
              title="Double-click to edit"
            >
              {/* Always show a connected range: each block runs to the next
                  block's start (or +1h for the last), so no row collapses to a
                  lone start time. layoutRows already folds in any explicit end. */}
              <div className="today-time">{fmtRange(r.b.start, minToStr(r.effEnd))}</div>
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
            {untimed.map((b) => {
              const idx = blocks.indexOf(b);
              if (idx === editIdx) {
                return (
                  <ScheduleRowForm
                    key={`edit-${idx}`}
                    init={b}
                    busy={rowBusy}
                    onCommit={(nb) => commitEdit(idx, nb)}
                    onCancel={() => setEditIdx(null)}
                    onDelete={() => deleteBlock(idx)}
                  />
                );
              }
              return (
                <div
                  className="today-block untimed editable"
                  key={idx}
                  onDoubleClick={() => beginEdit(idx)}
                  title="Double-click to edit"
                >
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
              );
            })}
          </div>
        )}

        {adding ? (
          <ScheduleRowForm
            init={{ task: "" }}
            busy={rowBusy}
            onCommit={commitAdd}
            onCancel={() => setAdding(false)}
          />
        ) : (
          <button
            className="today-addrow"
            onClick={() => {
              setEditIdx(null);
              setAdding(true);
            }}
          >
            <Plus size={15} /> Add to schedule
          </button>
        )}
      </div>

      {editError && !editing && <div className="error today-rowerror">{editError}</div>}

      {note?.raw_text && (
        <details className="today-notes">
          <summary>notes</summary>
          <pre>{note.raw_text}</pre>
        </details>
      )}
    </div>
  );
}
