import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { CalendarCheck, CalendarDays, CalendarX, Camera, Check, ChevronLeft, ChevronRight, Ellipsis, GripHorizontal, ListTodo, Loader, Pencil, Plus, Trash2, Video, X } from "lucide-react";
import {
  api,
  type CalEvent,
  type EntityCandidate,
  type GcalContact,
  type GcalStatus,
  type NoteRow,
} from "./api";
import {
  CalendarEventForm,
  defaultCalendarEventKey,
  parseCalendarGuests,
  splitCalendarEventKey,
  type CalendarEventFormState,
} from "./Calendar";
import { fileToImg, type Img } from "./image";
import { easternDay, easternMinutes, formatDay } from "./day";
import { joinUrl } from "./joinUrl";
import { openExternalUrl } from "./openExternalUrl";
import {
  buildScheduleGrid,
  isCurrentInterval,
  scheduleEndFromResizeDelta,
  scheduleMinuteFromGridOffset,
  scheduleStartFromResizeDelta,
} from "./scheduleLayout";
import { DocumentEditor } from "./editor/DocumentEditor";
import {
  TASK_DOCUMENT_VERSION,
  countOpenDocumentTasks,
  documentFingerprint,
  documentPlainText,
  extractDocumentTasks,
  normalizeTaskDocument,
  type DocumentTask,
  type StructuredDocument,
} from "./editor/document";

export type Block = {
  task: string;
  start?: string; // HH:MM (24h)
  end?: string; // HH:MM (24h)
  duration_min?: number;
};

type Todo = DocumentTask;

type GridDraftEvent = {
  task: string;
  start: number;
  end: number;
};

type GridResizeState = {
  index: number | null;
  edge: "start" | "end";
  initialStart: number;
  initialEnd: number;
  currentStart: number;
  currentEnd: number;
  originClientY: number;
  pointerId: number;
};

// Schedule blocks intentionally remain lightweight. Join behavior is resolved
// from the live calendar feed so a stale meeting URL is never persisted into a
// hand-edited day. Ambiguous duplicate events stay ordinary editable rows.
function matchingCalendarEvent(block: Block, events: CalEvent[]): CalEvent | null {
  const exact = events.filter(
    (event) =>
      !event.all_day &&
      event.task.trim() === block.task.trim() &&
      (event.start ?? "") === (block.start ?? "") &&
      (!block.end || (event.end ?? "") === block.end)
  );
  if (exact.length === 1) return exact[0];

  // Schedules created before configurable time zones stored Eastern wall-clock
  // strings. Once the live calendar is available, a unique title is enough to
  // reconnect that row to the same event and show its current-zone time. Keep
  // duplicate titles ambiguous so we never silently bind the wrong meeting.
  const titleMatches = events.filter(
    (event) => !event.all_day && event.start && event.task.trim() === block.task.trim()
  );
  return titleMatches.length === 1 ? titleMatches[0] : null;
}

export function reconcileScheduleBlocks(blocks: Block[], events: CalEvent[]): Block[] {
  return blocks.map((block) => {
    const event = matchingCalendarEvent(block, events);
    if (!event?.start) return block;
    return {
      ...block,
      start: event.start,
      ...(event.end ? { end: event.end } : {}),
    };
  });
}

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

// Keep same-meridiem ranges compact while making the actual end unambiguous.
function fmtRange(start?: string, end?: string): string {
  const startMinutes = toMinutes(start);
  const endMinutes = toMinutes(end);
  if (startMinutes == null) return "";
  if (endMinutes == null) return fmtTime(start);
  const sameMeridiem = startMinutes < 720 === endMinutes < 720;
  if (sameMeridiem) return `${fmtTime(start).replace(/\s?(AM|PM)$/i, "")}–${fmtTime(end)}`;
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

// Calendar-day movement is anchored at UTC noon so stepping across daylight
// saving boundaries always lands on the immediately adjacent YYYY-MM-DD.
function shiftDay(day: string, amount: number): string {
  const shifted = new Date(Date.parse(`${day}T12:00:00Z`) + amount * 86_400_000);
  return shifted.toISOString().slice(0, 10);
}

// Strip a leading list/checkbox marker so checklist lines parse like plain ones:
// "- ", "* ", "• ", box/checkbox glyphs (□ ☐ ☑ ☒ ■ ✓ ✗ …), and ASCII [ ] [x] ( ).
const MARKER_RE =
  /^\s*(?:[-*•·▪●]|[□☐▢◻◽◾■☑☒✅✓✔✗✘]|\[\s*[xX]?\s*\]|\(\s*[xX]?\s*\))\s+/;
function stripMarker(line: string): string {
  return line.replace(MARKER_RE, "");
}

// Resolved start (+ optional end) → a timed Block, and the monotonic-cursor
// value the next line should resolve against. An end at-or-before the start
// folds across midnight ("10pm–1am" is 3h) — layoutRows and the Rust gcal push
// already read end<start that way, so the parse side now agrees instead of
// silently dropping the end. Shared by parseSchedule and salvageBlock so both
// build blocks identically.
function buildTimed(task: string, start: number, end: number | null): { block: Block; next: number } {
  const s = ((start % 1440) + 1440) % 1440;
  const block: Block = { task, start: minToStr(s) };
  let next = start;
  if (end != null) {
    const e = ((end % 1440) + 1440) % 1440;
    if (e !== s) {
      block.end = minToStr(e);
      block.duration_min = e > s ? e - s : e + 1440 - s;
      next = start + block.duration_min;
    }
  }
  return { block, next };
}

// If a block lacks a usable start time, treat its task text as a raw schedule
// line and recover start/end/duration the same way parseSchedule does — the
// model sometimes fails to split a line, leaving the time inside `task`
// ("□ 11:30-12:30 : Workout") so the block would otherwise remain untimed.
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
      return buildTimed(task, start, end).block;
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
    // stored "June 4, 2026" stops showing up as an untimed row.
    .filter((b) => b.task && !isDateOnly(b.task));
}

// Checklist items live beside schedule blocks in the same entry. Be generous
// when reading older/hand-edited data, but always write the canonical shape.
export function parseTodos(data: Record<string, unknown> | null | undefined): Todo[] {
  const raw = data?.todos;
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item, index): Todo | null => {
      if (typeof item === "string") {
        const text = item.trim();
        return text ? { id: `legacy-${index}-${text}`, text, completed: false } : null;
      }
      if (!item || typeof item !== "object") return null;
      const value = item as Record<string, unknown>;
      const text =
        typeof value.text === "string"
          ? value.text.trim()
          : typeof value.task === "string"
            ? value.task.trim()
            : "";
      if (!text) return null;
      return {
        id: typeof value.id === "string" && value.id ? value.id : `legacy-${index}-${text}`,
        text,
        completed: value.completed === true || value.done === true,
      };
    })
    .filter((item): item is Todo => item != null);
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

// Serialize blocks into the human-facing editor format. The app keeps canonical
// HH:MM values internally for storage, sorting, and calendar sync, but the edit
// surface should always speak in an explicit US 12-hour clock. Both endpoints
// carry their own meridiem so parseSchedule can round-trip ranges across noon.
function blocksToText(blocks: Block[]): string {
  return blocks
    .map((b) => {
      if (!b.start) return b.task;
      const time = b.end ? `${fmtTime(b.start)}–${fmtTime(b.end)}` : fmtTime(b.start);
      return `${time} ${b.task}`;
    })
    .join("\n");
}

// Prefill an existing schedule from structured, timed blocks only. In
// particular, do not fall back to the note's raw text: task-only daily entries
// store their checklist there, and treating that text as a schedule is what
// allowed open tasks to leak into the builder.
export function scheduleEditorSeed(blocks: Block[]): string {
  return blocksToText(blocks.filter((block) => toMinutes(block.start) != null));
}

// Calendar-imported events are the only external source allowed to prefill a
// new schedule. All-day events remain in Calendar because the schedule itself
// requires a concrete start time.
export function calendarEventsToScheduleBlocks(events: CalEvent[]): Block[] {
  return events
    .filter((event) => !event.all_day && event.task.trim() && toMinutes(event.start ?? undefined) != null)
    .map((event) => ({
      task: event.task,
      start: event.start!,
      ...(toMinutes(event.end ?? undefined) != null ? { end: event.end! } : {}),
    }));
}

// Calendar events are part of the day by default. Reconcile any existing rows
// first (including schedules saved before local-time calendar values), then add
// only genuinely missing timed events. Manual schedule rows remain untouched.
export function mergeScheduleWithCalendar(blocks: Block[], events: CalEvent[]): Block[] {
  const merged = reconcileScheduleBlocks(blocks, events);
  let added = false;
  for (const calendarBlock of calendarEventsToScheduleBlocks(events)) {
    const alreadyPresent = merged.some(
      (block) =>
        block.task.trim() === calendarBlock.task.trim() &&
        (block.start ?? "") === (calendarBlock.start ?? "") &&
        (!block.end || !calendarBlock.end || block.end === calendarBlock.end),
    );
    if (!alreadyPresent) {
      merged.push(calendarBlock);
      added = true;
    }
  }
  // Avoid rewriting an existing hand-authored order merely because Calendar
  // loaded. New events use the schedule's normal chronological write order.
  return added ? sortBlocks(merged) : merged;
}

function scheduleBlocksKey(blocks: Block[]): string {
  return JSON.stringify(
    blocks.map((block) => [
      block.task.trim(),
      block.start ?? "",
      block.end ?? "",
      block.duration_min ?? null,
    ]),
  );
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
  if (hour === 12) {
    // An unqualified "12" means noon by human convention ("12-1 lunch"), never
    // 00:xx; once noon has passed it's the coming midnight instead.
    const noon = 720 + min;
    return noon >= prevMin ? noon : 1440 + min;
  }
  const base = hour * 60 + min;
  if (base >= prevMin) return base;
  if (base + 720 >= prevMin) return base + 720;
  return base + 720;
}

// Parse one leading time token ("8", "8:30", "8:30pm", "930", "1145", "noon").
function resolveTime(tok: string, prevMin: number): number | null {
  const t = tok.trim();
  // Word clocks — the only ones people actually write in schedules.
  if (/^(?:noon|midday)$/i.test(t)) return 720;
  // "midnight" opening a schedule is 00:00; anywhere later it's the coming one.
  if (/^midnight$/i.test(t)) return prevMin > 0 ? 1440 : 0;
  const m = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm|a|p)?$/i.exec(t);
  if (m) {
    const hour = Number(m[1]);
    const min = m[2] ? Number(m[2]) : 0;
    if (hour > 23 || min > 59) return null;
    const ap = m[3] ? (m[3][0].toLowerCase() as "a" | "p") : null;
    return pickMonotonic(hour, min, ap, prevMin);
  }
  const d = /^(\d{3,4})\s*(am|pm|a|p)?$/i.exec(t); // colon-less "930" / "1145" / "930pm"
  if (d) {
    const n = d[1];
    const hh = Number(n.slice(0, n.length - 2));
    const mm = Number(n.slice(-2));
    if (hh > 23 || mm > 59) return null;
    const ap = d[2] ? (d[2][0].toLowerCase() as "a" | "p") : null;
    return pickMonotonic(hh, mm, ap, prevMin);
  }
  return null;
}

// am/pm (or bare a/p) only counts as a meridiem when not glued to a word, so
// "2:00 profound" keeps its "p" instead of reading it as "2:00 p.m.". Word
// clocks (noon/midday/midnight) count too, with the same not-glued guard.
// Colon-less "930"/"0830" must come FIRST: alternation is leftmost-wins, so
// with \d{1,2} in front it would eat "08" and leave "30 run" as the task.
const TIME_TOK =
  "\\d{3,4}(?:\\s*(?:am|pm|a|p)(?![a-z]))?|\\d{1,2}(?::\\d{2})?(?:\\s*(?:am|pm|a|p)(?![a-z]))?|(?:noon|midday|midnight)(?![a-z])";
const LINE_RE = new RegExp(`^\\s*~?\\s*(${TIME_TOK})\\s*(?:(?:[-–—]|to)\\s*~?\\s*(${TIME_TOK}))?`, "i");

// Longest-first so the alternation matches full names before abbreviations.
const WEEKDAY =
  "(?:monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues|tue|weds|wed|thurs|thur|thu|fri|sat|sun)";
const MONTH =
  "(?:january|february|march|april|may|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sept|sep|oct|nov|dec)";

// A line that is *only* a date ("Thursday, June 4th", "6/4/2026", "2026-06-04").
// These ride along in photos to confirm the day — they're a sanity check, not a
// task, so we drop them rather than parking them as untimed rows. We require a
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
// Lines that start with a clock time become timed blocks; everything else remains
// untimed. The whole point is reliability: this never hard-fails.
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
      const { block, next } = buildTimed(task, start, end);
      blocks.push(block);
      prev = next;
    } else {
      blocks.push({ task: line });
    }
  }
  return blocks;
}

const PLACEHOLDER = `Lay out your day, however messy:

7:30 AM Wake up
8:00 AM–9:00 AM Gym
10:00 AM–12:00 PM Deep work
12:30 PM Lunch with Sam
2:00 PM–4:00 PM Errands
7:00 PM Dinner`;

// Parse a user-facing US time while keeping the rest of the schedule pipeline
// canonical. Blank is a valid "no time" value; null means the text is invalid.
// Requiring AM/PM avoids silently turning an ambiguous "2:00" into 2 AM.
export function parseTime12(value: string): string | null {
  const text = value.trim().replace(/\./g, "");
  if (!text) return "";
  if (/^noon$/i.test(text)) return "12:00";
  if (/^midnight$/i.test(text)) return "00:00";
  const match = /^(\d{1,2})(?::(\d{1,2}))?\s*([ap])m?$/i.exec(text);
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = match[2] ? Number(match[2]) : 0;
  if (hour < 1 || hour > 12 || minute > 59) return null;
  const hour24 = (hour % 12) + (match[3].toLowerCase() === "p" ? 12 : 0);
  return `${String(hour24).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

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
  const [start, setStart] = useState(init.start ? fmtTime(init.start) : "");
  const [end, setEnd] = useState(init.end ? fmtTime(init.end) : "");
  const [task, setTask] = useState(init.task ?? "");
  const initialStart = toMinutes(init.start);
  const initialEnd = toMinutes(init.end);
  const [endsNextDay, setEndsNextDay] = useState(
    initialStart != null && initialEnd != null && initialEnd < initialStart
  );
  const [timeError, setTimeError] = useState<{
    field: "start" | "end" | "range";
    message: string;
  } | null>(null);
  const timeErrorId = useId();

  function normalizeTime(
    value: string,
    setValue: (next: string) => void,
    field: "start" | "end"
  ) {
    const parsed = parseTime12(value);
    if (parsed == null) {
      if (value.trim()) setTimeError({ field, message: "Use a time like 9:30 AM." });
      return;
    }
    setValue(parsed ? fmtTime(parsed) : "");
    setTimeError(null);
  }

  function commit() {
    const t = task.trim();
    if (!t) return;
    const parsedStart = parseTime12(start);
    const parsedEnd = parseTime12(end);
    if (!parsedStart) {
      setTimeError({ field: "start", message: "Add a start time like 9:30 AM. Use Tasks for anything flexible." });
      return;
    }
    if (parsedEnd == null) {
      setTimeError({ field: "end", message: "Use an end time like 10:30 AM." });
      return;
    }
    if (parsedStart && parsedEnd && parsedStart === parsedEnd) {
      setTimeError({ field: "range", message: "Start and end time need to be different." });
      return;
    }
    const startMinutes = parsedStart ? toMinutes(parsedStart) : null;
    const endMinutes = parsedEnd ? toMinutes(parsedEnd) : null;
    if (
      startMinutes != null &&
      endMinutes != null &&
      endMinutes < startMinutes &&
      !endsNextDay
    ) {
      setTimeError({
        field: "range",
        message: "The end is earlier than the start. Check “Ends next day” if that’s intentional.",
      });
      return;
    }
    const b: Block = { task: t };
    if (parsedStart) {
      b.start = parsedStart;
      if (startMinutes != null && endMinutes != null) {
        b.end = parsedEnd;
        b.duration_min = (endMinutes - startMinutes + 1440) % 1440;
      }
    }
    onCommit(b);
  }

  const onEditorKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") commit();
    else if (e.key === "Escape") onCancel();
  };
  const currentStart = parseTime12(start);
  const currentEnd = parseTime12(end);
  const currentStartMinutes = currentStart ? toMinutes(currentStart) : null;
  const currentEndMinutes = currentEnd ? toMinutes(currentEnd) : null;
  const crossesMidnight =
    currentStartMinutes != null &&
    currentEndMinutes != null &&
    currentEndMinutes < currentStartMinutes;

  return (
    <div className="today-block today-rowedit" role="listitem">
      <div className="today-rowedit-times">
        <label className="today-timefield">
          <span>Start</span>
          <input
            type="text"
            className={"today-timeinput" + (timeError?.field === "start" || timeError?.field === "range" ? " invalid" : "")}
            value={start}
            disabled={busy}
            placeholder="9:00 AM"
            autoCapitalize="characters"
            onChange={(e) => {
              const next = e.target.value;
              setStart(next);
              if (!next.trim()) setEnd("");
              setEndsNextDay(false);
              setTimeError(null);
            }}
            onBlur={() => normalizeTime(start, setStart, "start")}
            onKeyDown={onEditorKeyDown}
            aria-label="Start time, for example 9:00 AM"
            aria-invalid={timeError?.field === "start" || timeError?.field === "range"}
            aria-describedby={timeError ? timeErrorId : undefined}
          />
        </label>
        <span className="today-rowedit-dash">–</span>
        <label className="today-timefield">
          <span>End</span>
          <input
            type="text"
            className={"today-timeinput" + (timeError?.field === "end" || timeError?.field === "range" ? " invalid" : "")}
            value={end}
            disabled={busy || !start.trim()}
            placeholder="10:00 AM"
            autoCapitalize="characters"
            onChange={(e) => {
              setEnd(e.target.value);
              setEndsNextDay(false);
              setTimeError(null);
            }}
            onBlur={() => normalizeTime(end, setEnd, "end")}
            onKeyDown={onEditorKeyDown}
            aria-label="End time, for example 10:00 AM"
            aria-invalid={timeError?.field === "end" || timeError?.field === "range"}
            aria-describedby={timeError ? timeErrorId : undefined}
          />
        </label>
      </div>
      {crossesMidnight && (
        <label className="today-overnight">
          <input
            type="checkbox"
            checked={endsNextDay}
            disabled={busy}
            onChange={(e) => {
              setEndsNextDay(e.target.checked);
              setTimeError(null);
            }}
          />
          Ends next day
        </label>
      )}
      <input
        className="today-taskinput"
        value={task}
        disabled={busy}
        autoFocus
        placeholder="What's happening?"
        onChange={(e) => setTask(e.target.value)}
        onKeyDown={onEditorKeyDown}
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
      {timeError && <div id={timeErrorId} className="today-rowedit-error" role="alert">{timeError.message}</div>}
    </div>
  );
}

export function TodayView({
  notes,
  onSaved,
  onOpenSettings,
  lead,
}: {
  notes: NoteRow[];
  onSaved: () => void | Promise<void>;
  onOpenSettings?: () => void;
  lead?: ReactNode;
}) {
  // Re-render each minute so the "now" highlight stays accurate through the day.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  const currentDay = easternDay(now);
  const [selectedDay, setSelectedDay] = useState(() => currentDay);
  const previousCurrentDayRef = useRef(currentDay);
  useEffect(() => {
    const previousCurrentDay = previousCurrentDayRef.current;
    if (previousCurrentDay === currentDay) return;
    previousCurrentDayRef.current = currentDay;
    setSelectedDay((day) => (day === previousCurrentDay ? currentDay : day));
  }, [currentDay]);

  const isToday = selectedDay === currentDay;
  const dateLine = formatDay(selectedDay, {
    weekday: "long",
    month: "long",
    day: "numeric",
    ...(selectedDay.slice(0, 4) === currentDay.slice(0, 4) ? {} : { year: "numeric" }),
  });
  const compactDateLine = formatDay(selectedDay, {
    weekday: "short",
    month: "short",
    day: "numeric",
    ...(selectedDay.slice(0, 4) === currentDay.slice(0, 4) ? {} : { year: "numeric" }),
  });
  const selectedDayNavLabel = isToday
    ? "Today"
    : formatDay(selectedDay, { weekday: "short" });

  // notes are newest-first (db.rs orders by event_date, id DESC), so the first
  // match is the selected day's latest schedule — re-captures naturally win,
  // with no merging.
  const note = notes.find(
    (n) => n.event_date === selectedDay && n.entries.some((e) => isSchedule(e.category))
  );
  const entry = note?.entries.find((e) => isSchedule(e.category));
  const blocks = parseBlocks(entry?.data);
  const storedTodos = parseTodos(entry?.data);
  const incomingTaskDocument = normalizeTaskDocument(entry?.data?.task_doc, storedTodos);
  const incomingTaskFingerprint = documentFingerprint(incomingTaskDocument);

  // Inline create/edit state — the schedule is made right here, not in Log.
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  // Two-step inline confirm for clearing a day — WKWebView silently swallows
  // window.confirm() (returns false), so we arm/confirm in-app instead.
  const [clearArmed, setClearArmed] = useState(false);
  const [photo, setPhoto] = useState<Img | null>(null);
  const [photoReading, setPhotoReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const photoRequestRef = useRef(0);
  const photoTextRef = useRef("");
  const calWrapRef = useRef<HTMLDivElement>(null); // anchor for the calendar peek popover
  const addWrapRef = useRef<HTMLDivElement>(null);

  // Inline row editing on the agenda: which block index is open, an "add new"
  // flag, and a save-in-flight flag shared by both.
  const [editIdx, setEditIdx] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const [rowBusy, setRowBusy] = useState(false);
  const [gridDraft, setGridDraft] = useState<GridDraftEvent | null>(null);
  const [selectedIdx, setSelectedIdx] = useState<number | null>(null);
  const [gridCursorMinute, setGridCursorMinute] = useState<number | null>(null);
  const [gridResize, setGridResize] = useState<GridResizeState | null>(null);
  const [gridDetailsOpen, setGridDetailsOpen] = useState(false);
  const [gridDetailsBusy, setGridDetailsBusy] = useState(false);
  const [gridDetailsError, setGridDetailsError] = useState<string | null>(null);
  const [gridDetailsContacts, setGridDetailsContacts] = useState<GcalContact[]>([]);
  const timeGridRef = useRef<HTMLDivElement>(null);
  const gridDraftInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!gridDraft) return;
    const frame = window.requestAnimationFrame(() => gridDraftInputRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [gridDraft?.start]);

  // Tasks use the same structured document shape that future standalone notes
  // can reuse. Older todo arrays are lifted into a task-list document in memory
  // and written back in both shapes the first time the document changes.
  const [taskDocument, setTaskDocument] = useState<StructuredDocument>(() => incomingTaskDocument);
  const [taskSaveState, setTaskSaveState] = useState<"idle" | "dirty" | "saving" | "saved" | "error">("idle");
  const taskDocumentRef = useRef(taskDocument);
  const taskIncomingFingerprintRef = useRef(incomingTaskFingerprint);
  const taskChangeRevisionRef = useRef(0);
  const taskPersistedRevisionRef = useRef(0);
  const taskSaveTimerRef = useRef<number | null>(null);
  const taskSaveInFlightRef = useRef(false);
  const taskSaveWaitersRef = useRef<Array<() => void>>([]);
  const taskSavePendingRef = useRef(false);
  const taskAwaitingEntryRef = useRef(false);
  const taskEntryIdRef = useRef<number | null>(entry?.id ?? null);
  const taskBlocksRef = useRef(blocks);
  const taskDayRef = useRef(selectedDay);
  const taskOnSavedRef = useRef(onSaved);
  const taskFlushRef = useRef<() => Promise<void>>(async () => {});
  const taskMountedRef = useRef(true);
  const todoDateRef = useRef(selectedDay);
  const agendaRef = useRef<HTMLDivElement>(null);
  const [agendaHeight, setAgendaHeight] = useState<number | null>(null);
  // Keep the accepted desktop planner composition intact on first open: the
  // checklist is docked beside the schedule. Compact/mobile layouts still
  // begin with it closed so the schedule keeps the full viewport.
  const [todoOpen, setTodoOpen] = useState(
    () => typeof window !== "undefined" && window.matchMedia?.("(min-width: 840px)").matches === true
  );
  const [todoError, setTodoError] = useState<string | null>(null);
  const todoToggleRef = useRef<HTMLButtonElement>(null);

  taskDocumentRef.current = taskDocument;
  taskBlocksRef.current = blocks;
  taskDayRef.current = selectedDay;
  taskOnSavedRef.current = onSaved;
  if (entry?.id != null) {
    taskEntryIdRef.current = entry.id;
    taskAwaitingEntryRef.current = false;
  } else if (!taskAwaitingEntryRef.current) {
    taskEntryIdRef.current = null;
  }

  const todos = extractDocumentTasks(taskDocument);
  const remainingTodos = countOpenDocumentTasks(taskDocument);
  const taskPanelStyle = (
    agendaHeight == null ? undefined : { "--today-agenda-height": `${agendaHeight}px` }
  ) as CSSProperties | undefined;

  // Google Calendar sync: whether we're connected, and the last sync's outcome.
  const [gcalConnected, setGcalConnected] = useState<boolean | null>(null);
  const [gcalStatus, setGcalStatus] = useState<GcalStatus | null>(null);
  // The selected day's events pulled back from Google Calendar — used for the empty-state
  // starting point, calendar peek, and direct join links on matching rows.
  const [calEvents, setCalEvents] = useState<CalEvent[] | null>(null);
  const [calEventsDay, setCalEventsDay] = useState<string | null>(null);
  const [calLoading, setCalLoading] = useState(false);
  const [calOpen, setCalOpen] = useState(false); // peek popover open?
  const [addOpen, setAddOpen] = useState(false);
  const [calendarAutoState, setCalendarAutoState] = useState<"idle" | "saving" | "error">("idle");
  const [calendarAutoRetry, setCalendarAutoRetry] = useState(0);
  const calendarAutoAttemptRef = useRef<string | null>(null);
  const [sync, setSync] = useState<{
    state: "idle" | "syncing" | "clearing" | "ok" | "err";
    msg: string;
  }>({
    state: "idle",
    msg: "",
  });

  const automaticScheduleBlocks = mergeScheduleWithCalendar(blocks, calEvents ?? []);
  const storedBlocksKey = scheduleBlocksKey(blocks);
  const automaticBlocksKey = scheduleBlocksKey(automaticScheduleBlocks);
  const calendarNeedsUpdate = storedBlocksKey !== automaticBlocksKey;
  taskBlocksRef.current = automaticScheduleBlocks;

  useEffect(() => {
    api
      .gcalAuthStatus()
      .then((status) => {
        setGcalStatus(status);
        setGcalConnected(status.connected);
      })
      .catch(() => {
        setGcalStatus(null);
        setGcalConnected(false);
      });
  }, []);

  // Only timed blocks can become calendar events; untimed rows are skipped.
  const hasTimed = automaticScheduleBlocks.some((b) => toMinutes(b.start) != null);

  useLayoutEffect(() => {
    if (!todoOpen || !agendaRef.current) return;
    const agenda = agendaRef.current;
    const measure = () => setAgendaHeight(Math.ceil(agenda.getBoundingClientRect().height));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(agenda);
    return () => observer.disconnect();
  }, [blocks.length, note?.id, todoOpen]);

  function closeTodoDrawer(returnFocus = true) {
    setTodoOpen(false);
    if (returnFocus) window.requestAnimationFrame(() => todoToggleRef.current?.focus());
  }

  useEffect(() => {
    if (!todoOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeTodoDrawer();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [todoOpen]);

  useEffect(() => {
    // Reset date-bound editor state after navigation or midnight rollover. Do
    // not run on initial mount, which would undo the desktop-open default.
    if (todoDateRef.current === selectedDay) return;
    todoDateRef.current = selectedDay;
    if (taskSaveTimerRef.current != null) window.clearTimeout(taskSaveTimerRef.current);
    taskSaveTimerRef.current = null;
    taskChangeRevisionRef.current = 0;
    taskPersistedRevisionRef.current = 0;
    taskSavePendingRef.current = false;
    taskAwaitingEntryRef.current = false;
    taskEntryIdRef.current = entry?.id ?? null;
    taskIncomingFingerprintRef.current = incomingTaskFingerprint;
    taskDocumentRef.current = incomingTaskDocument;
    setTaskDocument(incomingTaskDocument);
    setTaskSaveState("idle");
    setTodoError(null);
    setGridDraft(null);
    setSelectedIdx(null);
    setGridCursorMinute(null);
    setGridResize(null);
    setGridDetailsOpen(false);
    setGridDetailsError(null);
    calendarAutoAttemptRef.current = null;
    setCalendarAutoState("idle");
  }, [selectedDay, entry?.id, incomingTaskDocument, incomingTaskFingerprint]);

  // Pull in a newly loaded or externally updated document only while there are
  // no local edits waiting to be saved. Normal autosave refreshes therefore do
  // not disturb the selection or move the user's caret.
  useEffect(() => {
    if (todoDateRef.current !== selectedDay) return;
    if (taskIncomingFingerprintRef.current === incomingTaskFingerprint) return;
    taskIncomingFingerprintRef.current = incomingTaskFingerprint;
    if (taskChangeRevisionRef.current !== taskPersistedRevisionRef.current) return;
    taskDocumentRef.current = incomingTaskDocument;
    setTaskDocument(incomingTaskDocument);
  }, [selectedDay, incomingTaskDocument, incomingTaskFingerprint]);

  useEffect(() => {
    taskMountedRef.current = true;
    return () => {
      taskMountedRef.current = false;
      if (taskSaveTimerRef.current != null) window.clearTimeout(taskSaveTimerRef.current);
      if (taskChangeRevisionRef.current !== taskPersistedRevisionRef.current) {
        void taskFlushRef.current();
      }
    };
  }, []);

  // Self-heal: when parseBlocks salvaged times the model left buried in `task`
  // (e.g. "□ 11:30-12:30 : Workout" with no start), rewrite the stored row once
  // so the DB holds clean start/end. This persists the schedule across reloads
  // and lets Google Calendar sync — which reads the stored times — actually push.
  const healedRef = useRef<number | null>(null);
  // In-flight heal write, so a Sync clicked right after capture waits for the
  // healed times to land before the backend reads blocks out of the DB.
  const healWriteRef = useRef<Promise<void> | null>(null);
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
    healWriteRef.current = api
      .updateEntry(entry.id, { blocks })
      .then(() => onSaved())
      .catch(() => {})
      .finally(() => {
        healWriteRef.current = null;
      });
  }, [entry?.id, blocks]);

  // Pull the selected day's real Google Calendar events. Errors fall back to an empty list
  // so schedule editing remains fully useful when Calendar is unavailable.
  async function loadCalEvents() {
    const requestedDay = selectedDay;
    setCalLoading(true);
    try {
      const events = await api.gcalListEvents(requestedDay);
      if (taskDayRef.current === requestedDay) {
        setCalEvents(events);
        setCalEventsDay(requestedDay);
      }
    } catch {
      if (taskDayRef.current === requestedDay) {
        setCalEvents([]);
        setCalEventsDay(requestedDay);
      }
    } finally {
      if (taskDayRef.current === requestedDay) setCalLoading(false);
    }
  }

  // Forget cached events when the selected day changes so dates never share a
  // stale calendar response.
  useEffect(() => {
    setCalEvents(null);
    setCalEventsDay(null);
    setCalLoading(false);
    setCalOpen(false);
    setAddOpen(false);
  }, [selectedDay]);

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

  useEffect(() => {
    if (!addOpen) return;
    const onDown = (event: MouseEvent) => {
      if (addWrapRef.current && !addWrapRef.current.contains(event.target as Node)) setAddOpen(false);
    };
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && setAddOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [addOpen]);

  // Auto-load whenever Calendar is connected: populated schedules need the
  // live meeting metadata too, otherwise their rows cannot become joinable.
  useEffect(() => {
    if (gcalConnected && calEvents == null && !calLoading) loadCalEvents();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gcalConnected, calEvents, calLoading]);

  // Push the selected day's schedule to Google Calendar. Not connected → open Settings.
  async function syncToGcal() {
    if (!gcalConnected) {
      onOpenSettings?.();
      return;
    }
    if (sync.state === "syncing") return;
    setSync({ state: "syncing", msg: "" });
    try {
      // The backend reads blocks from the DB, so let any in-flight self-heal
      // write finish first — otherwise salvaged times miss this push.
      if (healWriteRef.current) await healWriteRef.current;
      const r = await api.gcalSync(selectedDay);
      const pushed = r.created + r.updated;
      const parts = [`Synced ${pushed} block${pushed === 1 ? "" : "s"}`];
      if (r.skipped) {
        parts.push(`${r.skipped} unscheduled item${r.skipped === 1 ? "" : "s"} skipped`);
      }
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
  // Clear the selected day: empty its schedule in noted (the editor text box +
  // the stored blocks) and, when Google is connected, also remove that day's events
  // from the noted calendar. Events that live only in the user's *other*
  // calendars are never deleted from Google — clearing them here just drops
  // noted's local copy of the day.
  async function clearDay() {
    if (sync.state === "syncing" || sync.state === "clearing") return;
    // First click arms; second click (while armed) actually clears. The button
    // label flips to "Confirm clear" and disarms after a few seconds.
    if (!clearArmed) {
      setClearArmed(true);
      window.setTimeout(() => setClearArmed(false), 4000);
      return;
    }
    setClearArmed(false);
    setSync({ state: "clearing", msg: "" });
    try {
      // Local: blank the editor box and persist an empty schedule. (Saving an
      // empty textarea via saveDraft is a no-op, so clear the stored blocks here.)
      setDraft("");
      if (entry?.id != null) await api.updateEntry(entry.id, { blocks: [] });
      // Google: drop that day's noted-calendar events, if connected.
      let removed = 0;
      if (gcalConnected) removed = await api.gcalClearDay(selectedDay);
      await onSaved();
      setSync({
        state: "ok",
        msg:
          gcalConnected && removed > 0
            ? `Schedule cleared · ${removed} calendar event${removed === 1 ? "" : "s"} removed.`
            : "Schedule cleared.",
      });
    } catch (e) {
      setSync({ state: "err", msg: String(e) });
    }
  }

  function openEditor() {
    // Structured blocks are authoritative. Normalizing them here both prevents
    // stale raw capture text from overwriting inline edits and guarantees the
    // editor opens with explicit 12-hour times.
    photoRequestRef.current += 1;
    photoTextRef.current = "";
    setDraft(scheduleEditorSeed(automaticScheduleBlocks));
    setPhoto(null);
    setPhotoReading(false);
    setEditError(null);
    setEditIdx(null);
    setAdding(false);
    setGridDraft(null);
    setSelectedIdx(null);
    setGridResize(null);
    setGridDetailsOpen(false);
    setGridDetailsError(null);
    closeTodoDrawer(false);
    setSync({ state: "idle", msg: "" }); // clear stale sync/clear feedback
    setEditing(true);
  }

  async function attachPhoto(file: File | undefined) {
    if (!file) return;
    const requestId = ++photoRequestRef.current;
    try {
      const nextPhoto = await fileToImg(file);
      if (requestId !== photoRequestRef.current) return;
      // Photo capture is a first-class way into the schedule builder. Opening
      // the editor here means the visible photo card works without an Edit step.
      if (!editing) {
        setDraft(scheduleEditorSeed(automaticScheduleBlocks));
        setSync({ state: "idle", msg: "" });
        closeTodoDrawer(false);
        setEditing(true);
      }
      const previousPhotoText = photoTextRef.current.trim();
      if (previousPhotoText) {
        setDraft((current) => {
          const trimmed = current.trim();
          if (trimmed === previousPhotoText) return "";
          const suffix = `\n${previousPhotoText}`;
          return trimmed.endsWith(suffix) ? trimmed.slice(0, -suffix.length).trimEnd() : current;
        });
        photoTextRef.current = "";
      }
      setPhoto(nextPhoto);
      setEditError(null);
      setPhotoReading(true);
      try {
        const transcription = (await api.ocrPhoto(nextPhoto.base64)).trim();
        if (requestId !== photoRequestRef.current) return;
        if (!transcription) {
          setEditError("No schedule text was found in that photo. You can still type it below.");
        } else {
          const photoBlocks = parseSchedule(transcription);
          if (!photoBlocks.length) {
            setEditError("No schedule items were found in that photo. Try another image or type them below.");
          } else {
            const normalized = blocksToText(photoBlocks).trim();
            photoTextRef.current = normalized;
            setDraft((current) => [current.trim(), normalized].filter(Boolean).join("\n"));
          }
        }
      } catch (e) {
        if (requestId === photoRequestRef.current) {
          setEditError(`Couldn’t read that photo: ${String(e)}`);
        }
      } finally {
        if (requestId === photoRequestRef.current) setPhotoReading(false);
      }
    } catch (e) {
      if (requestId !== photoRequestRef.current) return;
      if (!editing) {
        setDraft(scheduleEditorSeed(automaticScheduleBlocks));
        setEditing(true);
      }
      setEditError(String(e));
    }
  }

  function removePhoto() {
    photoRequestRef.current += 1;
    photoTextRef.current = "";
    setPhotoReading(false);
    setPhoto(null);
  }

  function closeEditor() {
    photoRequestRef.current += 1;
    photoTextRef.current = "";
    setPhotoReading(false);
    setEditing(false);
    setEditError(null);
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

      // Photo OCR is inserted into the visible editor as soon as the image is
      // selected. Save only persists the reviewed text and local image here.
      if (photo) {
        source = "photo";
        image_path = await api.saveImage(photo.base64, photo.ext);
      }

      if (!body) {
        setEditError("Add at least one schedule item before saving.");
        return;
      }

      // Deterministic parse — instant, offline, never hard-fails on the model.
      const parsedBlocks = parseSchedule(body);
      if (!parsedBlocks.length) {
        setEditError('Couldn’t find anything to schedule. Try lines like "9:00 gym" or "2–4pm errands".');
        setBusy(false);
        return;
      }

      const unscheduled = parsedBlocks.filter((block) => toMinutes(block.start) == null);
      if (unscheduled.length) {
        setEditError(
          `Add a time to ${unscheduled.length === 1 ? `“${unscheduled[0].task}”` : "every schedule item"}. Use Tasks for anything flexible.`
        );
        return;
      }
      // Preserve older hidden untimed rows without allowing new ones into a
      // view that no longer has an Anytime section.
      const legacyUntimed = blocks.filter((block) => toMinutes(block.start) == null);
      const nextBlocks = sortBlocks([...parsedBlocks, ...legacyUntimed]);

      await api.save({
        raw_text: body,
        source,
        image_path,
        event_date: selectedDay,
        entries: [{
          category: "schedule",
          description: "daily schedule",
          data: {
            blocks: nextBlocks,
            todos,
            task_doc_version: TASK_DOCUMENT_VERSION,
            task_doc: taskDocument,
          },
        }],
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

  // Persist inline edits. If this day's schedule entry already exists we update it
  // IN PLACE (update_entry) so the same DB row changes — Today and the Timeline
  // reflect the edit with no duplicate note, and the note is re-embedded. Only
  // when there's no schedule yet do we insert a fresh note.
  async function persistBlocks(next: Block[]): Promise<boolean> {
    if (rowBusy) return false;
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
          event_date: selectedDay,
          entries: [{
            category: "schedule",
            description: "daily schedule",
            data: {
              blocks: cleaned,
              todos,
              task_doc_version: TASK_DOCUMENT_VERSION,
              task_doc: taskDocument,
            },
          }],
        });
      }
      setEditIdx(null);
      setAdding(false);
      await onSaved();
      return true;
    } catch (e) {
      setEditError(String(e));
      return false;
    } finally {
      setRowBusy(false);
    }
  }

  const commitEdit = (idx: number, b: Block) => persistBlocks(blocks.map((x, i) => (i === idx ? b : x)));
  const deleteBlock = (idx: number) => persistBlocks(blocks.filter((_, i) => i !== idx));
  const commitAdd = (b: Block) => persistBlocks([...blocks, b]);
  const beginAdd = () => {
    setAddOpen(false);
    setEditIdx(null);
    setEditError(null);
    setGridDraft(null);
    setSelectedIdx(null);
    setGridResize(null);
    setGridDetailsOpen(false);
    setGridDetailsError(null);
    closeTodoDrawer(false);
    setAdding(true);
  };
  const beginEdit = (idx: number) => {
    setAdding(false);
    setEditError(null);
    setGridDraft(null);
    setSelectedIdx(idx);
    setGridResize(null);
    setGridDetailsOpen(false);
    setGridDetailsError(null);
    setEditIdx(idx);
  };

  async function flushTaskDocument() {
    if (taskSaveInFlightRef.current) {
      taskSavePendingRef.current = true;
      await new Promise<void>((resolve) => taskSaveWaitersRef.current.push(resolve));
      if (taskChangeRevisionRef.current !== taskPersistedRevisionRef.current) {
        await taskFlushRef.current();
      }
      return;
    }
    if (taskChangeRevisionRef.current === taskPersistedRevisionRef.current) return;

    // A first task creates this day's schedule entry. If React has not delivered
    // the refreshed entry id yet, wait for that rather than inserting a second
    // note while the user keeps typing.
    if (taskEntryIdRef.current == null && taskAwaitingEntryRef.current) {
      try {
        await taskOnSavedRef.current();
      } catch (error) {
        if (taskMountedRef.current) {
          setTodoError(String(error));
          setTaskSaveState("error");
        }
        return;
      }
      taskSavePendingRef.current = true;
      taskSaveTimerRef.current = window.setTimeout(() => void taskFlushRef.current(), 220);
      return;
    }

    const snapshot = taskDocumentRef.current;
    const revision = taskChangeRevisionRef.current;
    const targetDate = taskDayRef.current;
    const text = documentPlainText(snapshot);
    const nextTodos = extractDocumentTasks(snapshot);

    // There is no reason to create an otherwise empty daily note, but clearing
    // an existing document must still be persisted.
    if (taskEntryIdRef.current == null && !text) {
      taskPersistedRevisionRef.current = revision;
      taskSavePendingRef.current = false;
      if (taskMountedRef.current) setTaskSaveState("idle");
      return;
    }

    taskSaveInFlightRef.current = true;
    taskSavePendingRef.current = false;
    if (taskMountedRef.current) {
      setTodoError(null);
      setTaskSaveState("saving");
    }

    try {
      const entryId = taskEntryIdRef.current;
      const data = {
        task_doc_version: TASK_DOCUMENT_VERSION,
        task_doc: snapshot,
        todos: nextTodos,
      };
      if (entryId != null) {
        await api.updateEntry(entryId, data);
      } else {
        taskAwaitingEntryRef.current = true;
        await api.save({
          raw_text: text,
          source: "text",
          event_date: targetDate,
          entries: [
            {
              category: "schedule",
              description: "daily schedule and tasks",
              data: { blocks: taskBlocksRef.current, ...data },
            },
          ],
        });
        await taskOnSavedRef.current();
      }
      taskPersistedRevisionRef.current = revision;
      if (taskChangeRevisionRef.current === revision) {
        if (taskMountedRef.current && taskDayRef.current === targetDate) setTaskSaveState("saved");
      } else {
        taskSavePendingRef.current = true;
      }
    } catch (error) {
      taskSavePendingRef.current = false;
      if (taskMountedRef.current && taskDayRef.current === targetDate) {
        setTodoError(String(error));
        setTaskSaveState("error");
      }
    } finally {
      taskSaveInFlightRef.current = false;
      const waiters = taskSaveWaitersRef.current.splice(0);
      waiters.forEach((resolve) => resolve());
      if (
        taskSavePendingRef.current ||
        taskChangeRevisionRef.current !== taskPersistedRevisionRef.current
      ) {
        taskSaveTimerRef.current = window.setTimeout(() => void taskFlushRef.current(), 220);
      }
    }
  }
  taskFlushRef.current = flushTaskDocument;

  function handleTaskDocumentChange(next: StructuredDocument) {
    if (documentFingerprint(next) === documentFingerprint(taskDocumentRef.current)) return;
    taskDocumentRef.current = next;
    taskChangeRevisionRef.current += 1;
    taskSavePendingRef.current = true;
    setTaskDocument(next);
    setTodoError(null);
    setTaskSaveState("dirty");
    if (taskSaveTimerRef.current != null) window.clearTimeout(taskSaveTimerRef.current);
    taskSaveTimerRef.current = window.setTimeout(() => void taskFlushRef.current(), 750);
  }

  const [dayNavigating, setDayNavigating] = useState(false);
  const dayNavigationDisabled =
    dayNavigating ||
    editing ||
    busy ||
    photoReading ||
    rowBusy ||
    editIdx != null ||
    adding ||
    gridDraft != null ||
    gridResize != null ||
    gridDetailsOpen ||
    gridDetailsBusy;

  // Once the selected day's live calendar has loaded, make its timed events
  // part of the saved schedule without asking the user to assemble them. The
  // attempted-content key prevents retry loops when persistence fails; the
  // explicit retry below is reserved for that exceptional state.
  useEffect(() => {
    if (
      !gcalConnected ||
      calEvents == null ||
      calEventsDay !== selectedDay ||
      !calendarNeedsUpdate ||
      dayNavigationDisabled ||
      taskSaveInFlightRef.current
    ) {
      return;
    }

    const attemptKey = `${selectedDay}:${automaticBlocksKey}`;
    if (calendarAutoAttemptRef.current === attemptKey) return;
    calendarAutoAttemptRef.current = attemptKey;
    setCalendarAutoState("saving");
    const targetDay = selectedDay;

    void persistBlocks(automaticScheduleBlocks).then((saved) => {
      if (taskDayRef.current !== targetDay) return;
      setCalendarAutoState(saved ? "idle" : "error");
    });
    // The block arrays are represented by stable content keys above; depending
    // on the freshly parsed arrays themselves would rerun this effect forever.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    gcalConnected,
    calEvents,
    calEventsDay,
    selectedDay,
    calendarNeedsUpdate,
    dayNavigationDisabled,
    automaticBlocksKey,
    calendarAutoRetry,
  ]);

  function retryAutomaticCalendar() {
    calendarAutoAttemptRef.current = null;
    setCalendarAutoState("idle");
    setCalendarAutoRetry((revision) => revision + 1);
  }

  async function navigateToDay(nextDay: string) {
    if (nextDay === selectedDay || dayNavigationDisabled) return;
    setDayNavigating(true);
    try {
      if (taskSaveTimerRef.current != null) window.clearTimeout(taskSaveTimerRef.current);
      taskSaveTimerRef.current = null;
      await taskFlushRef.current();
      // Keep the user on this day when its notes could not be saved. The
      // editor already exposes the actionable error beside the document.
      if (taskChangeRevisionRef.current !== taskPersistedRevisionRef.current) return;
      setCalOpen(false);
      setAddOpen(false);
      setClearArmed(false);
      setSync({ state: "idle", msg: "" });
      setSelectedDay(nextDay);
    } finally {
      setDayNavigating(false);
    }
  }

  const renderTodoDrawer = () => {
    if (!todoOpen) return null;
    return (
      <>
        <button
          type="button"
          className="today-task-scrim"
          tabIndex={-1}
          aria-label="Close tasks"
          onClick={() => closeTodoDrawer()}
        />
        <aside
          id="today-task-drawer"
          className="today-task-drawer"
          style={taskPanelStyle}
          role="dialog"
          aria-modal="false"
          aria-labelledby="today-tasks-title"
        >
          <div className="today-task-drawer-head">
            <div className="today-task-title">
              <h2 id="today-tasks-title">
                Tasks <span>· {remainingTodos}</span>
              </h2>
              {taskSaveState !== "idle" && (
                <span className={`today-task-save-state ${taskSaveState}`} aria-live="polite">
                  {taskSaveState === "dirty" && "Unsaved"}
                  {taskSaveState === "saving" && <><Loader size={11} className="spin" /> Saving</>}
                  {taskSaveState === "saved" && "Saved"}
                  {taskSaveState === "error" && "Not saved"}
                </span>
              )}
            </div>
            <button
              type="button"
              className="today-task-close"
              onClick={() => closeTodoDrawer()}
              aria-label="Close tasks"
            >
              <X size={17} />
            </button>
          </div>

          <section className="today-todos" aria-label={`Tasks and notes for ${dateLine}`}>
            <DocumentEditor
              value={taskDocument}
              onChange={handleTaskDocumentChange}
              placeholder="Type a task, note, or idea…"
              ariaLabel={`Tasks and notes for ${dateLine}`}
            />
            {todoError && <div className="error today-task-error">{todoError}</div>}
          </section>
        </aside>
      </>
    );
  };

  const renderScheduleShell = (content: ReactNode) => (
    <div className={"today-shell" + (todoOpen ? " tasks-open" : "")}>
      <div className="today today-shell-head">{Head(true)}</div>
      {isToday && lead ? <div className="today-shell-lead">{lead}</div> : null}
      <div className="today today-shell-body">{content}</div>
      {renderTodoDrawer()}
    </div>
  );

  const Head = (withEdit: boolean) => (
    <header className="today-head">
      <div className="today-headrow">
        <div className="today-date-block">
          <div className="today-date-topline">
            <div className="today-eyebrow">Daily schedule</div>
            <nav className="today-day-nav" aria-label="Browse schedule days">
              <button
                type="button"
                onClick={() => void navigateToDay(shiftDay(selectedDay, -1))}
                disabled={dayNavigationDisabled}
                aria-label="Previous day"
                title="Previous day"
              >
                <ChevronLeft size={16} />
              </button>
              <span className="today-day-nav-label" aria-current="date">
                {selectedDayNavLabel}
              </span>
              <button
                type="button"
                onClick={() => void navigateToDay(shiftDay(selectedDay, 1))}
                disabled={dayNavigationDisabled}
                aria-label="Next day"
                title="Next day"
              >
                <ChevronRight size={16} />
              </button>
            </nav>
          </div>
          <h1 className="today-date" aria-label={dateLine}>
            <time dateTime={selectedDay}>
              <span className="today-date-full" aria-hidden="true">{dateLine}</span>
              <span className="today-date-compact" aria-hidden="true">{compactDateLine}</span>
            </time>
          </h1>
        </div>
        {withEdit && (
          <div className="today-headbtns">
            <div className="today-calwrap" ref={calWrapRef}>
              <button
                className={"today-edit" + (calOpen ? " active" : "")}
                onClick={() => {
                  if (gcalConnected === false) {
                    onOpenSettings?.();
                    return;
                  }
                  setAddOpen(false);
                  setCalOpen((open) => !open);
                  if (calEvents == null && !calLoading) void loadCalEvents();
                }}
                aria-expanded={calOpen}
                title={gcalConnected === false ? "Connect Google Calendar" : "View and sync Google Calendar"}
              >
                <CalendarDays size={14} /> {gcalConnected === false ? "Connect calendar" : "Calendar sync"}
              </button>
              {gcalConnected && calOpen && (
                <div className="today-calpop" role="dialog" aria-label="Calendar sync">
                  <div className="today-calpop-head">
                    <span>Calendar sync</span>
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
                    <div className="today-calpop-msg">
                      {isToday ? "Nothing on your calendar today." : "Nothing on your calendar for this day."}
                    </div>
                  ) : (
                    <ul className="today-calpop-list">
                      {calEvents.map((e, i) => {
                        const meetingUrl = e.meet_link ? joinUrl(e.meet_link, e.account) : null;
                        return (
                          <li
                            key={e.id || i}
                            className={"today-calpop-row" + (meetingUrl ? " joinable" : "")}
                          >
                            {meetingUrl && (
                              <a
                                className="today-cal-event-hit"
                                href={meetingUrl}
                                target="_blank"
                                rel="noreferrer"
                                onClick={(event) => {
                                  event.preventDefault();
                                  openExternalUrl(meetingUrl);
                                }}
                                aria-label={`Join ${e.task}`}
                              />
                            )}
                            <span className="today-cal-time">
                              {e.all_day ? "All day" : fmtTime(e.start ?? undefined)}
                            </span>
                            <span className="today-cal-task">{e.task}</span>
                            {meetingUrl && <Video size={13} className="today-cal-join-icon" aria-hidden="true" />}
                          </li>
                        );
                      })}
                    </ul>
                  )}
                  <button
                    type="button"
                    className="today-calpop-sync"
                    onClick={() => void syncToGcal()}
                    disabled={sync.state === "syncing" || sync.state === "clearing" || rowBusy || !hasTimed}
                  >
                    {sync.state === "syncing" ? <Loader size={14} className="spin" /> : <CalendarCheck size={14} />}
                    {isToday ? "Sync today’s schedule" : "Sync this day’s schedule"}
                  </button>
                </div>
              )}
            </div>
            <button
              ref={todoToggleRef}
              type="button"
              className="today-edit today-tasks-toggle"
              onClick={() => (todoOpen ? closeTodoDrawer() : setTodoOpen(true))}
              aria-expanded={todoOpen}
              aria-controls="today-task-drawer"
            >
              <ListTodo size={14} /> Tasks · {remainingTodos}
            </button>
            <div className="today-calwrap" ref={addWrapRef}>
              <button
                type="button"
                className={"today-edit today-day-options" + (addOpen ? " active" : "")}
                onClick={() => {
                  setCalOpen(false);
                  setAddOpen((open) => !open);
                }}
                aria-expanded={addOpen}
                aria-haspopup="menu"
                aria-label="Schedule options"
                title="Schedule options"
              >
                <Ellipsis size={16} />
              </button>
              {addOpen && (
                <div className="today-addpop" role="menu" aria-label="Schedule options">
                  {!isToday && (
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => void navigateToDay(currentDay)}
                    >
                      <CalendarDays size={14} /> Go to today
                    </button>
                  )}
                  <button type="button" role="menuitem" onClick={beginAdd}>
                    <Plus size={14} /> Add an event
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    disabled={busy || photoReading}
                    onClick={() => {
                      setAddOpen(false);
                      setEditIdx(null);
                      setAdding(false);
                      fileRef.current?.click();
                    }}
                  >
                    <Camera size={14} /> From photo
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setAddOpen(false);
                      openEditor();
                    }}
                  >
                    <Pencil size={14} /> Edit as text
                  </button>
                </div>
              )}
            </div>
          </div>
        )}
      </div>
      <input
        ref={fileRef}
        type="file"
        accept="image/*,.heic,.heif"
        capture="environment"
        hidden
        onChange={(e) => {
          void attachPhoto(e.currentTarget.files?.[0]);
          e.currentTarget.value = "";
        }}
      />
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
          <div className="today-editor-intro">
            <div>
              <div className="today-editor-kicker">Schedule builder</div>
              <h2>
                {blocks.length
                  ? isToday ? "Edit today’s schedule" : "Edit this day’s schedule"
                  : isToday ? "Create today’s schedule" : "Create a schedule for this day"}
              </h2>
            </div>
            <p>Use one item per line. Write times as 9:00 AM or 2:00 PM–4:00 PM.</p>
          </div>
          <textarea
            className="today-textarea"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={PLACEHOLDER}
            autoFocus
            disabled={busy || photoReading}
          />

          {photo && (
            <div className="today-photo">
              <img src={photo.dataUrl} alt="schedule" />
              <button className="today-photo-x" onClick={removePhoto} aria-label="Remove photo">
                <X size={14} />
              </button>
            </div>
          )}

          {photoReading && (
            <div className="today-photo-reading" role="status">
              <Loader size={14} className="spin" /> Reading the schedule into the editor…
            </div>
          )}

          {editError && <div className="error">{editError}</div>}

          {(sync.state === "ok" || sync.state === "err") && (
            <div className={"today-syncmsg " + sync.state}>{sync.msg}</div>
          )}

          <div className="today-editor-actions">
            <button
              className="today-photo-btn"
              onClick={() => fileRef.current?.click()}
              disabled={busy || photoReading}
              title="Snap a handwritten schedule"
            >
              <Camera size={16} /> {photo ? "Replace photo" : "Add photo"}
            </button>
            <button
              className={"today-photo-btn" + (clearArmed ? " armed" : "")}
              onClick={clearDay}
              disabled={busy || photoReading || sync.state === "clearing"}
              title={
                gcalConnected
                  ? `Clear ${isToday ? "today's" : "this day's"} schedule in noted and its events from the noted calendar`
                  : `Clear ${isToday ? "today's" : "this day's"} schedule in noted`
              }
            >
              {sync.state === "clearing" ? (
                <Loader size={16} className="spin" />
              ) : (
                <CalendarX size={16} />
              )}
              {clearArmed ? "Confirm clear" : isToday ? "Clear today" : "Clear this day"}
            </button>
            <span className="today-spacer" />
            <button
              className="today-cancel"
              onClick={closeEditor}
              disabled={busy}
            >
              Cancel
            </button>
            <button
              className="today-save"
              onClick={saveDraft}
              disabled={busy || photoReading || !draft.trim()}
            >
              {photoReading ? (
                <>
                  <Loader size={15} className="spin" /> Reading photo…
                </>
              ) : busy ? (
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

  // An empty day uses the same schedule canvas as a populated day. Calendar
  // context can sit above it, but it must never replace the creation surface.
  const calendarPending =
    !blocks.length &&
    (gcalConnected == null ||
      (gcalConnected && (calLoading || calEvents == null || calEventsDay !== selectedDay)));
  const showEmptyCalendar = Boolean(
    !blocks.length && !calendarPending && gcalConnected && calEvents && calEvents.length > 0,
  );
  const hasTimedCalendarEvents = calendarEventsToScheduleBlocks(calEvents ?? []).length > 0;

  // ---- Agenda ----
  const nowMin = easternMinutes(now);
  const scheduleHourPx = 44;
  const scheduleGridPad = 16;
  const displayBlocks = reconcileScheduleBlocks(blocks, calEvents ?? []);
  const grid = buildScheduleGrid(displayBlocks, { pixelsPerHour: scheduleHourPx });
  const gridStepMinutes = 15;
  const gridMinEventHeight = 40;

  const clampGridStart = (minute: number) =>
    Math.min(Math.max(grid.start, minute), Math.max(grid.start, grid.end - 60));

  const minuteAtGridY = (clientY: number, rectTop: number) =>
    clampGridStart(
      scheduleMinuteFromGridOffset(clientY - rectTop - scheduleGridPad, {
        gridStart: grid.start,
        pixelsPerHour: scheduleHourPx,
        stepMinutes: gridStepMinutes,
      }),
    );

  const paintedEventHeight = (start: number, end: number) =>
    Math.max(gridMinEventHeight, ((end - start) / 60) * scheduleHourPx - 3);

  const beginGridDraftAt = (start: number) => {
    if (rowBusy || editIdx != null || gridResize) return;
    setAddOpen(false);
    setAdding(false);
    setEditIdx(null);
    setEditError(null);
    setSelectedIdx(null);
    setGridCursorMinute(null);
    setGridDetailsOpen(false);
    setGridDetailsError(null);
    setGridDraft({ task: "", start, end: start + 60 });
    window.setTimeout(() => gridDraftInputRef.current?.focus({ preventScroll: true }), 40);
  };

  const handleGridPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || gridResize || gridDraft) return;
    setGridCursorMinute(minuteAtGridY(event.clientY, event.currentTarget.getBoundingClientRect().top));
  };

  const handleGridClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.button !== 0 || event.target !== event.currentTarget || gridDraft) return;
    beginGridDraftAt(minuteAtGridY(event.clientY, event.currentTarget.getBoundingClientRect().top));
  };

  const handleGridKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || gridDraft || gridResize) return;
    const current = gridCursorMinute ?? grid.start;
    let next = current;
    if (event.key === "ArrowUp") next = current - gridStepMinutes;
    else if (event.key === "ArrowDown") next = current + gridStepMinutes;
    else if (event.key === "PageUp") next = current - 60;
    else if (event.key === "PageDown") next = current + 60;
    else if (event.key === "Home") next = grid.start;
    else if (event.key === "End") next = grid.end - 60;
    else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      beginGridDraftAt(clampGridStart(current));
      return;
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setSelectedIdx(null);
      setGridCursorMinute(null);
      return;
    } else {
      return;
    }
    event.preventDefault();
    setGridCursorMinute(clampGridStart(next));
  };

  const saveGridDraft = async () => {
    if (!gridDraft?.task.trim() || rowBusy) return;
    const block: Block = {
      task: gridDraft.task.trim(),
      start: minToStr(gridDraft.start),
      end: minToStr(gridDraft.end),
      duration_min: gridDraft.end - gridDraft.start,
    };
    const nextBlocks = sortBlocks([...blocks, block]);
    const nextIndex = nextBlocks.findIndex((candidate) => candidate === block);
    if (await persistBlocks(nextBlocks)) {
      setGridDraft(null);
      setSelectedIdx(nextIndex);
      window.requestAnimationFrame(() => timeGridRef.current?.focus({ preventScroll: true }));
    }
  };

  const openGridDetails = () => {
    if (!gridDraft) return;
    if (!gcalStatus?.connected) {
      setEditError("Connect Google Calendar from Calendar sync to add Meet, guests, and event details.");
      return;
    }
    setEditError(null);
    setGridDetailsError(null);
    setGridDetailsOpen(true);
    api.gcalContacts().then(setGridDetailsContacts).catch(() => setGridDetailsContacts([]));
  };

  const saveGridDetails = async (form: CalendarEventFormState) => {
    if (!gridDraft || gridDetailsBusy) return;
    const { guests, invalid } = parseCalendarGuests(form.guests);
    if (invalid.length) {
      setGridDetailsError(`That doesn't look like a valid email: ${invalid.join(", ")}.`);
      return;
    }
    const start = toMinutes(form.start);
    const endClock = toMinutes(form.end);
    if (start == null || endClock == null || start === endClock) {
      setGridDetailsError("Choose different start and end times for this event.");
      return;
    }
    const end = endClock <= start ? endClock + 1440 : endClock;
    const block: Block = {
      task: form.title.trim(),
      start: minToStr(start),
      end: minToStr(end),
      duration_min: end - start,
    };
    const nextBlocks = sortBlocks([...blocks, block]);
    const nextIndex = nextBlocks.findIndex((candidate) => candidate === block);
    setGridDetailsBusy(true);
    setGridDetailsError(null);
    try {
      if (!(await persistBlocks(nextBlocks))) {
        setGridDetailsError(`Couldn't save this event to ${isToday ? "today's" : "this day's"} schedule.`);
        return;
      }
      const [account, calendarId] = splitCalendarEventKey(form.calKey);
      await api.gcalCreateEvent({
        account,
        calendarId,
        title: block.task,
        date: selectedDay,
        start: block.start,
        end: block.end,
        location: form.location.trim(),
        description: form.description.trim(),
        addMeet: form.meet === "add",
        ...(guests.length ? { guests } : {}),
      });
      localStorage.setItem("cal.lastCal", form.calKey);
      setGridDraft(null);
      setGridDetailsOpen(false);
      setSelectedIdx(nextIndex);
      await loadCalEvents();
      window.requestAnimationFrame(() => timeGridRef.current?.focus({ preventScroll: true }));
    } catch (error) {
      setGridDraft(null);
      setGridDetailsOpen(false);
      setEditError(`Saved to your schedule, but Google Calendar couldn't create the event: ${String(error)}`);
    } finally {
      setGridDetailsBusy(false);
    }
  };

  const persistGridResize = async (index: number, start: number, end: number) => {
    const block = blocks[index];
    if (!block) return;
    await commitEdit(index, {
      ...block,
      start: minToStr(start),
      end: minToStr(end),
      duration_min: end - start,
    });
  };

  const beginGridResize = (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number | null,
    edge: "start" | "end",
    start: number,
    end: number,
  ) => {
    if (rowBusy) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    setSelectedIdx(index);
    setGridCursorMinute(null);
    setGridResize({
      index,
      edge,
      initialStart: start,
      initialEnd: end,
      currentStart: start,
      currentEnd: end,
      originClientY: event.clientY,
      pointerId: event.pointerId,
    });
  };

  const updateGridResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!gridResize || event.pointerId !== gridResize.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const delta = event.clientY - gridResize.originClientY;
    if (gridResize.edge === "start") {
      const currentStart = scheduleStartFromResizeDelta(
        gridResize.initialStart,
        gridResize.initialEnd,
        delta,
        {
          pixelsPerHour: scheduleHourPx,
          stepMinutes: gridStepMinutes,
          minStart: grid.start,
        },
      );
      setGridResize({ ...gridResize, currentStart });
    } else {
      const currentEnd = scheduleEndFromResizeDelta(
        gridResize.initialStart,
        gridResize.initialEnd,
        delta,
        {
          pixelsPerHour: scheduleHourPx,
          stepMinutes: gridStepMinutes,
          maxEnd: grid.end,
        },
      );
      setGridResize({ ...gridResize, currentEnd });
    }
  };

  const finishGridResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!gridResize || event.pointerId !== gridResize.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const completed = gridResize;
    setGridResize(null);
    if (completed.index == null) {
      setGridDraft((draft) =>
        draft
          ? { ...draft, start: completed.currentStart, end: completed.currentEnd }
          : draft,
      );
    } else {
      void persistGridResize(completed.index, completed.currentStart, completed.currentEnd);
    }
  };

  const cancelGridResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!gridResize || event.pointerId !== gridResize.pointerId) return;
    event.stopPropagation();
    setGridResize(null);
  };

  const resizeWithKeyboard = (
    index: number | null,
    edge: "start" | "end",
    start: number,
    end: number,
    direction: -1 | 1,
  ) => {
    const nextStart =
      edge === "start"
        ? Math.max(grid.start, Math.min(end - gridStepMinutes, start + direction * gridStepMinutes))
        : start;
    const nextEnd =
      edge === "end"
        ? Math.min(grid.end, Math.max(start + gridStepMinutes, end + direction * gridStepMinutes))
        : end;
    if (index == null) {
      setGridDraft((draft) =>
        draft ? { ...draft, start: nextStart, end: nextEnd } : draft,
      );
    } else {
      void persistGridResize(index, nextStart, nextEnd);
    }
  };

  const eventState = (item: (typeof grid.items)[number]) => {
    if (!isToday) return { isNow: false, isPast: false };
    const isNow = isCurrentInterval(item, nowMin);
    return { isNow, isPast: !isNow && nowMin >= item.end };
  };

  const meetingFor = (block: Block) => {
    const calendarEvent = matchingCalendarEvent(block, calEvents ?? []);
    return calendarEvent?.meet_link
      ? joinUrl(calendarEvent.meet_link, calendarEvent.account)
      : null;
  };

  const eventClasses = (
    base: string,
    item: (typeof grid.items)[number],
    meetingUrl: string | null,
  ) => {
    const { isNow, isPast } = eventState(item);
    return (
      base +
      (meetingUrl ? " joinable" : "") +
      (isNow ? " now" : "") +
      (isPast ? " past" : "")
    );
  };

  const draftStart = gridDraft
    ? gridResize?.index === null
      ? gridResize.currentStart
      : gridDraft.start
    : null;
  const draftEnd = gridDraft
    ? gridResize?.index === null
      ? gridResize.currentEnd
      : gridDraft.end
    : null;

  return renderScheduleShell(
    <>
      {!blocks.length && calendarPending && (
        <div className="today-empty-context today-empty-loading" role="status" aria-live="polite">
          <Loader size={14} className="spin" /> Checking calendar…
        </div>
      )}
      {showEmptyCalendar && (
        <div className="today-empty-context">
          <div className="today-cal">
            <div className="today-cal-head">
              <CalendarDays size={15} /> From your calendar
            </div>
            <ul className="today-cal-list">
              {calEvents!.map((event, i) => {
                const meetingUrl = event.meet_link ? joinUrl(event.meet_link, event.account) : null;
                return (
                  <li key={event.id || i} className={"today-cal-row" + (meetingUrl ? " joinable" : "")}>
                    {meetingUrl && (
                      <a
                        className="today-cal-event-hit"
                        href={meetingUrl}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(event) => {
                          event.preventDefault();
                          openExternalUrl(meetingUrl);
                        }}
                        aria-label={`Join ${event.task}`}
                      />
                    )}
                    <span className="today-cal-time">
                      {event.all_day ? "All day" : fmtTime(event.start ?? undefined)}
                    </span>
                    <span className="today-cal-task">{event.task}</span>
                    {meetingUrl && <Video size={13} className="today-cal-join-icon" aria-hidden="true" />}
                  </li>
                );
              })}
            </ul>
            {hasTimedCalendarEvents ? (
              calendarAutoState === "error" ? (
                <div className="today-cal-auto is-error" role="alert">
                  <span>Couldn’t update the schedule.</span>
                  <button type="button" onClick={retryAutomaticCalendar}>Try again</button>
                </div>
              ) : (
                <div className="today-cal-auto" role="status" aria-live="polite">
                  <Loader size={14} className="spin" /> Updating schedule…
                </div>
              )
            ) : (
              <div className="today-cal-auto">No timed events</div>
            )}
          </div>
        </div>
      )}
      <div className="today-agenda" ref={agendaRef}>
        {adding && (
          <ScheduleRowForm
            init={{ task: "" }}
            busy={rowBusy}
            onCommit={commitAdd}
            onCancel={() => setAdding(false)}
          />
        )}
        <div
          className="today-time-grid"
          style={{ height: `${grid.heightPx + scheduleGridPad * 2}px` }}
          role="region"
          aria-label="Daily schedule, proportional time grid"
        >
          <div className="today-grid-hours" aria-hidden="true">
            {grid.hourMarks.map((minute) => (
              <div
                className="today-grid-hour"
                key={minute}
                style={{ top: `${scheduleGridPad + ((minute - grid.start) / 60) * scheduleHourPx}px` }}
              >
                <span>{fmtTime(minToStr(minute)).replace(":00", "")}</span>
              </div>
            ))}
          </div>
          <div
            ref={timeGridRef}
            className="today-grid-events"
            role="list"
            tabIndex={gridDraft ? -1 : 0}
            aria-label="Schedule canvas. Use the arrow keys to choose a time, then press Enter to add an event."
            onPointerMove={handleGridPointerMove}
            onClick={handleGridClick}
            onPointerLeave={() => !gridResize && setGridCursorMinute(null)}
            onKeyDown={handleGridKeyDown}
          >
            {!grid.items.length && !gridDraft && (
              <div className="today-grid-empty" role="status">
                Click any time to add an event
              </div>
            )}
            {gridCursorMinute != null && !gridDraft && (
              <div
                className="today-grid-cursor"
                aria-hidden="true"
                style={{
                  top: `${scheduleGridPad + ((gridCursorMinute - grid.start) / 60) * scheduleHourPx}px`,
                }}
              >
                <span>{fmtTime(minToStr(gridCursorMinute))}</span>
              </div>
            )}
            {gridDraft && draftStart != null && draftEnd != null && (
              <div
                className="today-grid-event today-grid-draft selected"
                style={{
                  top: `${scheduleGridPad + ((draftStart - grid.start) / 60) * scheduleHourPx}px`,
                  height: `${paintedEventHeight(draftStart, draftEnd)}px`,
                  left: 0,
                  width: "100%",
                }}
                onPointerDown={(event) => event.stopPropagation()}
                onPointerEnter={() => setGridCursorMinute(null)}
                role="listitem"
                aria-label={`New event, ${fmtRange(minToStr(draftStart), minToStr(draftEnd))}`}
              >
                <button
                  type="button"
                  className="today-grid-resize today-grid-resize-start"
                  onPointerDown={(event) => beginGridResize(event, null, "start", draftStart, draftEnd)}
                  onPointerMove={updateGridResize}
                  onPointerUp={finishGridResize}
                  onPointerCancel={cancelGridResize}
                  onKeyDown={(event) => {
                    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                    event.preventDefault();
                    event.stopPropagation();
                    resizeWithKeyboard(
                      null,
                      "start",
                      draftStart,
                      draftEnd,
                      event.key === "ArrowUp" ? -1 : 1,
                    );
                  }}
                  aria-label={`Adjust new event start. Currently starts at ${fmtTime(minToStr(draftStart))}. Use the up and down arrow keys in 15-minute steps.`}
                  title={`Adjust start · ${fmtTime(minToStr(draftStart))}`}
                >
                  <GripHorizontal size={14} />
                </button>
                <div className="today-grid-event-content">
                  <span className="today-grid-event-time">
                    {fmtRange(minToStr(draftStart), minToStr(draftEnd))}
                  </span>
                  <input
                    ref={gridDraftInputRef}
                    className="today-grid-title-input"
                    value={gridDraft.task}
                    autoFocus
                    placeholder="Event title"
                    disabled={rowBusy}
                    onPointerDown={(event) => event.stopPropagation()}
                    onChange={(event) => setGridDraft({ ...gridDraft, task: event.target.value })}
                    onKeyDown={(event) => {
                      event.stopPropagation();
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void saveGridDraft();
                      } else if (event.key === "Escape") {
                        setGridDraft(null);
                      }
                    }}
                    aria-label="Event title"
                  />
                </div>
                <div className="today-grid-draft-actions">
                  <button
                    type="button"
                    onClick={() => void saveGridDraft()}
                    disabled={rowBusy || !gridDraft.task.trim()}
                    aria-label="Save event"
                    title="Save event"
                  >
                    <Check size={13} />
                  </button>
                  <button
                    type="button"
                    onClick={openGridDetails}
                    disabled={rowBusy || !gridDraft.task.trim()}
                    aria-label="Add calendar details, guests, and Google Meet"
                    title={
                      gcalConnected
                        ? "Calendar details and Google Meet"
                        : "Connect Google Calendar to add event details"
                    }
                  >
                    <CalendarDays size={13} />
                  </button>
                  <button
                    type="button"
                    onClick={() => setGridDraft(null)}
                    disabled={rowBusy}
                    aria-label="Cancel new event"
                    title="Cancel"
                  >
                    <X size={13} />
                  </button>
                </div>
                <button
                  type="button"
                  className="today-grid-resize today-grid-resize-end"
                  onPointerDown={(event) => beginGridResize(event, null, "end", draftStart, draftEnd)}
                  onPointerMove={updateGridResize}
                  onPointerUp={finishGridResize}
                  onPointerCancel={cancelGridResize}
                  onKeyDown={(event) => {
                    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                    event.preventDefault();
                    event.stopPropagation();
                    resizeWithKeyboard(
                      null,
                      "end",
                      draftStart,
                      draftEnd,
                      event.key === "ArrowUp" ? -1 : 1,
                    );
                  }}
                  aria-label={`Adjust new event end. Currently ends at ${fmtTime(minToStr(draftEnd))}. Use the up and down arrow keys in 15-minute steps.`}
                  title={`Adjust end · ${fmtTime(minToStr(draftEnd))}`}
                >
                  <GripHorizontal size={14} />
                </button>
              </div>
            )}
            {grid.items.map((item) => {
              const idx = item.index;
              const block = item.block;
              if (idx === editIdx) {
                return (
                  <div
                    className="today-grid-edit"
                    key={`grid-edit-${idx}`}
                    style={{ top: `${scheduleGridPad + item.topPx}px` }}
                  >
                    <ScheduleRowForm
                      init={block}
                      busy={rowBusy}
                      onCommit={(next) => commitEdit(idx, next)}
                      onCancel={() => setEditIdx(null)}
                      onDelete={() => deleteBlock(idx)}
                    />
                  </div>
                );
              }

              const meetingUrl = meetingFor(block);
              const { isNow } = eventState(item);
              const previewStart = gridResize?.index === idx ? gridResize.currentStart : item.start;
              const previewEnd = gridResize?.index === idx ? gridResize.currentEnd : item.end;
              const previewHeight = paintedEventHeight(previewStart, previewEnd);
              const previewDuration = previewEnd - previewStart;
              const compact = previewHeight < 52;
              const range = fmtRange(minToStr(previewStart), minToStr(previewEnd));
              const selected = selectedIdx === idx;
              const laneGap = 4;
              const left = item.leftFraction * 100;
              const width = item.widthFraction * 100;
              const laneOffset = (item.lane * laneGap) / item.laneCount;
              const laneReduction = ((item.laneCount - 1) * laneGap) / item.laneCount;
              return (
                <div
                  key={`grid-event-${idx}`}
                  className={
                    eventClasses("today-grid-event editable", item, meetingUrl) +
                    (compact ? " compact" : "") +
                    (selected ? " selected" : "") +
                    (gridResize?.index === idx ? " resizing" : "")
                  }
                  style={{
                    top: `${scheduleGridPad + ((previewStart - grid.start) / 60) * scheduleHourPx}px`,
                    height: `${previewHeight}px`,
                    left: `calc(${left}% + ${laneOffset}px)`,
                    width: `calc(${width}% - ${laneReduction}px)`,
                  }}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (!gridDraft && !gridResize) setSelectedIdx(idx);
                  }}
                  onDoubleClick={(event) => {
                    event.stopPropagation();
                    beginEdit(idx);
                  }}
                  onPointerEnter={() => setGridCursorMinute(null)}
                  title="Click to select · double-click to edit"
                  role="listitem"
                  tabIndex={0}
                  onFocus={() => !gridDraft && setSelectedIdx(idx)}
                  onKeyDown={(event) => {
                    if (event.target !== event.currentTarget) return;
                    if (event.key === "Enter") {
                      event.preventDefault();
                      event.stopPropagation();
                      beginEdit(idx);
                    } else if (event.key === "Escape") {
                      event.preventDefault();
                      event.stopPropagation();
                      setSelectedIdx(null);
                    }
                  }}
                  aria-label={`${block.task}, ${range}${isNow ? ", now" : ""}`}
                >
                  <div className="today-grid-event-content">
                    <span className="today-grid-event-time">{range}</span>
                    <span className="today-task">{block.task}</span>
                    {!compact && (
                      <span className="today-grid-event-meta">
                        {isNow && <span className="today-nowtag">now</span>}
                        <span>{fmtDur(previewDuration)}</span>
                      </span>
                    )}
                  </div>
                  <div className="today-grid-event-actions">
                    {meetingUrl && (
                      <a
                        className="today-grid-event-action today-grid-join"
                        href={meetingUrl}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(event) => {
                          event.preventDefault();
                          event.stopPropagation();
                          openExternalUrl(meetingUrl);
                        }}
                        aria-label={`Join ${block.task}, ${range}`}
                        title="Join meeting"
                      >
                        <Video size={13} aria-hidden="true" />
                      </a>
                    )}
                    <button
                      type="button"
                      className="today-row-edit-trigger"
                      onClick={(event) => {
                        event.stopPropagation();
                        beginEdit(idx);
                      }}
                      aria-label={`Edit ${block.task}`}
                      title="Edit schedule item"
                    >
                      <Pencil size={13} />
                    </button>
                  </div>
                  {selected && (
                    <>
                      <button
                        type="button"
                        className="today-grid-resize today-grid-resize-start"
                        onPointerDown={(event) =>
                          beginGridResize(event, idx, "start", previewStart, previewEnd)
                        }
                        onPointerMove={updateGridResize}
                        onPointerUp={finishGridResize}
                        onPointerCancel={cancelGridResize}
                        onKeyDown={(event) => {
                          if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                          event.preventDefault();
                          event.stopPropagation();
                          resizeWithKeyboard(
                            idx,
                            "start",
                            previewStart,
                            previewEnd,
                            event.key === "ArrowUp" ? -1 : 1,
                          );
                        }}
                        aria-label={`Adjust ${block.task} start. Currently starts at ${fmtTime(minToStr(previewStart))}. Use the up and down arrow keys in 15-minute steps.`}
                        title={`Adjust start · ${fmtTime(minToStr(previewStart))}`}
                      >
                        <GripHorizontal size={14} />
                      </button>
                      <button
                        type="button"
                        className="today-grid-resize today-grid-resize-end"
                        onPointerDown={(event) =>
                          beginGridResize(event, idx, "end", previewStart, previewEnd)
                        }
                        onPointerMove={updateGridResize}
                        onPointerUp={finishGridResize}
                        onPointerCancel={cancelGridResize}
                        onKeyDown={(event) => {
                          if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                          event.preventDefault();
                          event.stopPropagation();
                          resizeWithKeyboard(
                            idx,
                            "end",
                            previewStart,
                            previewEnd,
                            event.key === "ArrowUp" ? -1 : 1,
                          );
                        }}
                        aria-label={`Adjust ${block.task} end. Currently ends at ${fmtTime(minToStr(previewEnd))}. Use the up and down arrow keys in 15-minute steps.`}
                        title={`Adjust end · ${fmtTime(minToStr(previewEnd))}`}
                      >
                        <GripHorizontal size={14} />
                      </button>
                    </>
                  )}
                </div>
              );
            })}
          </div>
        </div>

        <div className="today-ledger" role="list" aria-label="Daily schedule">
          {grid.items.map((item) => {
            const idx = item.index;
            const block = item.block;
            if (idx === editIdx) {
              return (
                <ScheduleRowForm
                  key={`ledger-edit-${idx}`}
                  init={block}
                  busy={rowBusy}
                  onCommit={(next) => commitEdit(idx, next)}
                  onCancel={() => setEditIdx(null)}
                  onDelete={() => deleteBlock(idx)}
                />
              );
            }
            const meetingUrl = meetingFor(block);
            const { isNow } = eventState(item);
            const range = fmtRange(block.start, minToStr(item.end));
            return (
              <div
                key={`ledger-event-${idx}`}
                className={eventClasses("today-block editable", item, meetingUrl)}
                onDoubleClick={meetingUrl ? undefined : () => beginEdit(idx)}
                title={meetingUrl ? `Join ${block.task}` : "Double-click to edit"}
                role="listitem"
                aria-label={`${block.task}, ${range}${isNow ? ", now" : ""}`}
              >
                {meetingUrl && (
                  <a
                    className="today-event-hit"
                    href={meetingUrl}
                    target="_blank"
                    rel="noreferrer"
                    aria-label={`Join ${block.task}, ${range}`}
                  />
                )}
                <div className="today-time" aria-hidden="true">
                  <span>{fmtTime(block.start)}</span>
                  <span className="today-time-end">{fmtTime(minToStr(item.end))}</span>
                </div>
                <div className="today-info">
                  <div className="today-task-stack">
                    <span className="today-task">{block.task}</span>
                    {isNow && (
                      <span className="today-current-meta">
                        <span className="today-nowtag">now</span>
                        <span>Ends at {fmtTime(minToStr(item.end))}</span>
                      </span>
                    )}
                    {meetingUrl && (
                      <span className="today-event-join">
                        <Video size={12} aria-hidden="true" /> Join meeting
                      </span>
                    )}
                  </div>
                  <span className="today-dur">{fmtDur(item.durationMinutes)}</span>
                  <button
                    type="button"
                    className="today-row-edit-trigger"
                    onClick={(event) => {
                      event.stopPropagation();
                      beginEdit(idx);
                    }}
                    aria-label={`Edit ${block.task}`}
                    title="Edit schedule item"
                  >
                    <Pencil size={13} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>

      </div>

      {editError && !editing && <div className="error today-rowerror">{editError}</div>}
      {gridDetailsOpen && gridDraft && (
        <CalendarEventForm
          key={`${gridDraft.start}-${gridDraft.end}`}
          heading="New calendar event"
          mode="create"
          init={{
            title: gridDraft.task,
            date: selectedDay,
            allDay: false,
            start: minToStr(gridDraft.start),
            end: minToStr(gridDraft.end),
            endDate: selectedDay,
            location: "",
            description: "",
            calKey: defaultCalendarEventKey(gcalStatus),
            meet: "none",
            guests: "",
          }}
          status={gcalStatus}
          contacts={gridDetailsContacts}
          lockDate
          allowAllDay={false}
          busy={gridDetailsBusy}
          error={gridDetailsError}
          onSave={(form) => void saveGridDetails(form)}
          onCancel={() => {
            if (gridDetailsBusy) return;
            setGridDetailsOpen(false);
            setGridDetailsError(null);
          }}
        />
      )}
    </>
  );
}
