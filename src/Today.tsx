import { useEffect, useRef, useState } from "react";
import { CalendarCheck, CalendarDays, CalendarX, Camera, Check, Loader, Pencil, Plus, Trash2, X } from "lucide-react";
import { api, type CalEvent, type EntityCandidate, type NoteRow } from "./api";
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

// Strip a leading list/checkbox marker so checklist lines parse like plain ones:
// "- ", "* ", "• ", box/checkbox glyphs (□ ☐ ☑ ☒ ■ ✓ ✗ …), and ASCII [ ] [x] ( ).
const MARKER_RE =
  /^\s*(?:[-*•·▪●]|[□☐▢◻◽◾■☑☒✅✓✔✗✘]|\[\s*[xX]?\s*\]|\(\s*[xX]?\s*\))\s+/;
function stripMarker(line: string): string {
  return line.replace(MARKER_RE, "");
}

// If a block lacks a usable start time, treat its task text as a raw schedule
// line and recover start/end/duration the same way parseSchedule does — the
// model sometimes fails to split a line, leaving the time inside `task`
// ("□ 11:30-12:30 : Workout") so the block would otherwise fall to "Anytime".
// `prev` threads the monotonic cursor across blocks for am/pm inference.
function salvageBlock(b: Block, prev: number): Block {
  if (toMinutes(b.start) != null) {
    // Already timed; just clean any stray marker glyph off the task.
    const task = stripMarker(b.task.trim());
    return task === b.task ? b : { ...b, task };
  }
  const line = stripMarker(b.task.trim());
  const m = LINE_RE.exec(line);
  const start = m?.[1] ? resolveTime(m[1], prev) : null;
  if (m && start != null) {
    const task = line.slice(m[0].length).replace(/^[\s:.,–—-]+/, "").trim();
    if (task) {
      const end = m[2] ? resolveTime(m[2], start) : null;
      const out: Block = { task, start: minToStr(start) };
      if (end != null && end > start) {
        out.end = minToStr(end);
        out.duration_min = end - start;
      }
      return out;
    }
  }
  // No leading time — keep the task, minus any marker glyph.
  return line === b.task ? b : { ...b, task: line };
}

// Pull a clean Block[] out of an entry's `data.blocks`, tolerating missing/odd fields.
export function parseBlocks(data: Record<string, unknown> | null | undefined): Block[] {
  const raw = data?.blocks;
  if (!Array.isArray(raw)) return [];
  let prev = -1;
  return raw
    .filter((b): b is Record<string, unknown> => !!b && typeof b === "object")
    .map((b) => {
      const base: Block = {
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
      };
      // Salvage times the model left embedded in the task, then advance the
      // cursor so a later bare "2:00" reads as 14:00, not 02:00.
      const block = salvageBlock(base, prev);
      const at = toMinutes(block.start);
      if (at != null) prev = toMinutes(block.end) ?? at;
      return block;
    })
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
    const line = stripMarker(raw.trim());
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
  const calWrapRef = useRef<HTMLDivElement>(null); // anchor for the calendar peek popover

  // Inline row editing on the agenda: which block index is open, an "add new"
  // flag, and a save-in-flight flag shared by both.
  const [editIdx, setEditIdx] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const [rowBusy, setRowBusy] = useState(false);

  // Google Calendar sync: whether we're connected, and the last sync's outcome.
  const [gcalConnected, setGcalConnected] = useState<boolean | null>(null);
  // Today's events pulled back from Google Calendar — shown in the empty state
  // as a starting point, and on demand via the header "Calendar" peek button.
  const [calEvents, setCalEvents] = useState<CalEvent[] | null>(null);
  const [calLoading, setCalLoading] = useState(false);
  const [calOpen, setCalOpen] = useState(false); // peek popover open?
  const [sync, setSync] = useState<{
    state: "idle" | "syncing" | "clearing" | "ok" | "err";
    msg: string;
  }>({
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

  // Self-heal: when parseBlocks salvaged times the model left buried in `task`
  // (e.g. "□ 11:30-12:30 : Workout" with no start), rewrite the stored row once
  // so the DB holds clean start/end. This persists the timeline across reloads
  // and lets Google Calendar sync — which reads the stored times — actually push.
  const healedRef = useRef<number | null>(null);
  useEffect(() => {
    if (entry?.id == null || healedRef.current === entry.id) return;
    const stored = Array.isArray(entry.data?.blocks) ? (entry.data.blocks as unknown[]) : [];
    const changed =
      blocks.length !== stored.length ||
      blocks.some((b, i) => {
        const s = (stored[i] ?? {}) as Record<string, unknown>;
        const sStart = typeof s.start === "string" ? s.start : undefined;
        const sEnd = typeof s.end === "string" ? s.end : undefined;
        return b.task !== s.task || b.start !== sStart || b.end !== sEnd;
      });
    healedRef.current = entry.id; // claim before the await so it runs at most once
    if (!changed) return;
    api
      .updateEntry(entry.id, { blocks })
      .then(() => onSaved())
      .catch(() => {});
  }, [entry?.id, blocks]);

  // Pull today's real Google Calendar events. Used by both the empty-state card
  // and the header "Calendar" peek button, so it's a plain loader rather than an
  // effect. Errors fall back to an empty list (so the UI shows "nothing today").
  const empty = !blocks.length;
  async function loadCalEvents() {
    setCalLoading(true);
    try {
      setCalEvents(await api.gcalListEvents(today));
    } catch {
      setCalEvents([]);
    } finally {
      setCalLoading(false);
    }
  }

  // Forget cached events when the day rolls over so we never show yesterday's.
  useEffect(() => {
    setCalEvents(null);
    setCalOpen(false);
  }, [today]);

  // Dismiss the peek popover on outside-click or Escape.
  useEffect(() => {
    if (!calOpen) return;
    const onDown = (e: MouseEvent) => {
      if (calWrapRef.current && !calWrapRef.current.contains(e.target as Node)) setCalOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setCalOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [calOpen]);

  // In the empty state, auto-load so events appear without a tap.
  useEffect(() => {
    if (gcalConnected && empty && calEvents == null && !calLoading) loadCalEvents();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gcalConnected, empty, calEvents, calLoading]);

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
      if (r.duplicates) parts.push(`${r.duplicates} already on calendar`);
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

  // Wipe everything noted has pushed by deleting its dedicated calendar; the next
  // sync recreates a fresh, empty one. A quick "start over" for the synced
  // schedule — other calendars and the connection are left alone.
  async function clearGcal() {
    if (!gcalConnected) {
      onOpenSettings?.();
      return;
    }
    if (sync.state === "syncing" || sync.state === "clearing") return;
    if (
      !window.confirm(
        "Clear the “noted” calendar in Google Calendar? This removes everything noted has synced. Your other calendars aren’t touched, and your next sync starts fresh."
      )
    )
      return;
    setSync({ state: "clearing", msg: "" });
    try {
      await api.gcalReset();
      setSync({ state: "ok", msg: "Calendar cleared — sync again to repopulate." });
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
    setSync({ state: "idle", msg: "" }); // clear stale sync/clear feedback
    setEditing(true);
  }

  // Seed the editor from today's pulled calendar events: timed events become
  // timed blocks, all-day events become "Anytime" tasks. The user tweaks and
  // hits Save, which re-parses through the normal saveDraft path.
  function buildFromEvents(events: CalEvent[]) {
    const seeded: Block[] = events
      .filter((e) => e.task.trim())
      .map((e) =>
        e.start ? { task: e.task, start: e.start, ...(e.end ? { end: e.end } : {}) } : { task: e.task }
      );
    setDraft(blocksToText(seeded));
    setPhoto(null);
    setEditError(null);
    setEditing(true);
  }

  // One pulled calendar event → a schedule block (all-day → untimed "Anytime").
  const calEventToBlock = (e: CalEvent): Block =>
    e.start ? { task: e.task, start: e.start, ...(e.end ? { end: e.end } : {}) } : { task: e.task };
  // Already on today's schedule? Matched by task + start so re-adds are no-ops.
  const onSchedule = (e: CalEvent) =>
    blocks.some((b) => b.task.trim() === e.task.trim() && (b.start ?? "") === (e.start ?? ""));

  // Append calendar events to the existing schedule (peek popover). Skips any
  // already present, then persists in place via the normal inline-edit path.
  async function addCalEvents(events: CalEvent[]) {
    const add = events.filter((e) => e.task.trim() && !onSchedule(e)).map(calEventToBlock);
    if (add.length) await persistBlocks([...blocks, ...add]);
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
            {gcalConnected && (
              <div className="today-calwrap" ref={calWrapRef}>
                <button
                  className={"today-edit" + (calOpen ? " active" : "")}
                  onClick={() => {
                    setCalOpen((o) => !o);
                    if (calEvents == null && !calLoading) loadCalEvents();
                  }}
                  aria-expanded={calOpen}
                  title="See today's Google Calendar events"
                >
                  <CalendarDays size={14} /> Calendar
                </button>
                {calOpen && (
                  <div className="today-calpop" role="dialog" aria-label="Today's calendar events">
                    <div className="today-calpop-head">
                      <span>From your calendar</span>
                      <button
                        className="today-calpop-x"
                        onClick={() => setCalOpen(false)}
                        aria-label="Close"
                      >
                        <X size={14} />
                      </button>
                    </div>
                    {calLoading && calEvents == null ? (
                      <div className="today-calpop-msg">
                        <Loader size={14} className="spin" /> Loading…
                      </div>
                    ) : !calEvents || calEvents.length === 0 ? (
                      <div className="today-calpop-msg">Nothing on your calendar today.</div>
                    ) : (
                      <>
                        <ul className="today-calpop-list">
                          {calEvents.map((e, i) => {
                            const added = onSchedule(e);
                            return (
                              <li key={i} className="today-calpop-row">
                                <span className="today-cal-time">
                                  {e.all_day ? "All day" : fmtTime(e.start ?? undefined)}
                                </span>
                                <span className="today-cal-task">{e.task}</span>
                                <button
                                  className="today-calpop-add"
                                  disabled={added || rowBusy}
                                  onClick={() => addCalEvents([e])}
                                  title={added ? "Already on schedule" : "Add to schedule"}
                                  aria-label={added ? "Already on schedule" : "Add to schedule"}
                                >
                                  {added ? <Check size={14} /> : <Plus size={14} />}
                                </button>
                              </li>
                            );
                          })}
                        </ul>
                        {calEvents.some((e) => !onSchedule(e)) && (
                          <button
                            className="today-make today-calpop-all"
                            disabled={rowBusy}
                            onClick={() => addCalEvents(calEvents)}
                          >
                            <Plus size={16} /> Add all to schedule
                          </button>
                        )}
                      </>
                    )}
                  </div>
                )}
              </div>
            )}
            <button
              className="today-edit"
              onClick={syncToGcal}
              disabled={sync.state === "syncing" || sync.state === "clearing" || (!!gcalConnected && !hasTimed)}
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
      {withEdit && sync.state !== "idle" && sync.state !== "syncing" && sync.state !== "clearing" && (
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

          {(sync.state === "ok" || sync.state === "err") && (
            <div className={"today-syncmsg " + sync.state}>{sync.msg}</div>
          )}

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
            {gcalConnected && (
              <button
                className="today-photo-btn"
                onClick={clearGcal}
                disabled={busy || sync.state === "clearing"}
                title="Clear the noted calendar — removes everything noted has synced"
              >
                {sync.state === "clearing" ? (
                  <Loader size={16} className="spin" />
                ) : (
                  <CalendarX size={16} />
                )}
                Clear calendar
              </button>
            )}
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
    const showCal = gcalConnected && calEvents && calEvents.length > 0;
    return (
      <div className="today">
        {Head(false)}
        <div className="today-empty">
          {showCal ? (
            <>
              <div className="today-cal">
                <div className="today-cal-head">
                  <CalendarDays size={15} /> From your calendar
                </div>
                <ul className="today-cal-list">
                  {calEvents!.map((e, i) => (
                    <li key={i} className="today-cal-row">
                      <span className="today-cal-time">
                        {e.all_day ? "All day" : fmtTime(e.start ?? undefined)}
                      </span>
                      <span className="today-cal-task">{e.task}</span>
                      <span className="today-cal-tag">{e.calendar}</span>
                    </li>
                  ))}
                </ul>
                <button className="today-make today-cal-build" onClick={() => buildFromEvents(calEvents!)}>
                  <Plus size={16} /> Build schedule from these
                </button>
              </div>
              <div className="today-or">or</div>
              <button className="today-empty-link" onClick={openEditor}>
                Start a schedule from scratch
              </button>
            </>
          ) : (
            <>
              <CalendarDays size={30} className="today-empty-icon" />
              <p className="today-empty-title">No schedule yet for today</p>
              <p className="today-empty-sub">
                Lay out your plan — type it from when you wake up to when you wind down, or snap a
                photo of a handwritten schedule. noted lays it out here, time by time.
              </p>
              <button className="today-make" onClick={openEditor}>
                <Plus size={16} /> Make today&apos;s schedule
              </button>
              {gcalConnected === false && (
                <button className="today-empty-link" onClick={() => onOpenSettings?.()}>
                  Connect Google Calendar to see today&apos;s events
                </button>
              )}
            </>
          )}
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
