import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  CalendarDays,
  Check,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Loader,
  MapPin,
  Pencil,
  Plus,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
  Users,
  Video,
  X,
} from "lucide-react";
import {
  api,
  type EventInput,
  type GcalContact,
  type GcalStatus,
  type RangeEvent,
} from "./api";
import { dayDiff, easternDay, easternMinutes, formatDay } from "./day";

const HOUR_PX_MAX = 48; // grid scale ceiling: one hour of the day
// The default visible window is the working day, 8 AM – 9 PM. It's a FIXED
// anchor — the view never scrolls itself to follow the current time.
const VIEW_START_H = 8;
const VIEW_HOURS = 13;
const DAY_MIN = 1440;

// "YYYY-MM-DD" + n days. Noon-UTC anchor so DST can't shift the day.
function addDays(d: string, n: number): string {
  return new Date(Date.parse(d + "T12:00:00Z") + n * 86_400_000).toISOString().slice(0, 10);
}

// Snap to the week's Sunday (Google's default week start).
function weekStart(d: string): string {
  return addDays(d, -new Date(d + "T12:00:00Z").getUTCDay());
}

function minToHHMM(m: number): string {
  const mm = ((Math.round(m) % DAY_MIN) + DAY_MIN) % DAY_MIN;
  return `${String(Math.floor(mm / 60)).padStart(2, "0")}:${String(mm % 60).padStart(2, "0")}`;
}

// "9 AM" / "9:30 AM" — wraps past midnight for cross-day ends.
function fmtMin(m: number): string {
  const mm = ((m % DAY_MIN) + DAY_MIN) % DAY_MIN;
  const h = Math.floor(mm / 60);
  const min = mm % 60;
  const ap = h < 12 ? "AM" : "PM";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return min ? `${h12}:${String(min).padStart(2, "0")} ${ap}` : `${h12} ${ap}`;
}

// Render plain text with its URLs clickable (descriptions often carry links).
const URL_RE = /(https?:\/\/[^\s<>"')]+)/g;
function Linkified({ text }: { text: string }) {
  return (
    <>
      {text.split(URL_RE).map((p, i) =>
        /^https?:\/\//.test(p) ? (
          <a key={i} href={p} target="_blank" rel="noreferrer">
            {p.length > 54 ? p.slice(0, 51) + "…" : p}
          </a>
        ) : (
          p
        )
      )}
    </>
  );
}

// One timed event's slice of a single day column (events can cross midnight).
type Seg = {
  ev: RangeEvent;
  startMin: number; // clipped to [0, 1440]
  endMin: number;
  col: number; // overlap layout: which column of the cluster…
  cols: number; // …out of how many
};

function segmentsForDay(events: RangeEvent[], day: string): Seg[] {
  const out: Seg[] = [];
  for (const ev of events) {
    if (ev.all_day || ev.start_min == null) continue;
    const end = ev.end_min ?? ev.start_min + 60;
    const offset = dayDiff(day, ev.date); // how many days this column is past the event's start day
    if (offset < 0) continue;
    const s = ev.start_min - offset * DAY_MIN;
    const e = end - offset * DAY_MIN;
    if (e <= 0 || s >= DAY_MIN) continue;
    out.push({ ev, startMin: Math.max(0, s), endMin: Math.min(DAY_MIN, e), col: 0, cols: 1 });
  }
  return out;
}

// Classic calendar overlap layout: overlapping events share the column width.
// Sorted by start (longest first on ties), each segment takes the first free
// sub-column; a gap in time flushes the cluster and resets the widths.
function layoutSegs(segs: Seg[]): Seg[] {
  const sorted = [...segs].sort((a, b) => a.startMin - b.startMin || b.endMin - a.endMin);
  const colEnds: number[] = [];
  let cluster: Seg[] = [];
  let clusterEnd = -1;
  const flush = () => {
    for (const s of cluster) s.cols = colEnds.length;
  };
  for (const s of sorted) {
    if (cluster.length && s.startMin >= clusterEnd) {
      flush();
      cluster = [];
      colEnds.length = 0;
    }
    let col = colEnds.findIndex((e) => e <= s.startMin);
    if (col === -1) {
      col = colEnds.length;
      colEnds.push(0);
    }
    colEnds[col] = s.endMin;
    s.col = col;
    cluster.push(s);
    clusterEnd = Math.max(clusterEnd, s.endMin);
  }
  flush();
  return sorted;
}

// All-day banner chips: which columns each spans, stacked into lanes.
type Chip = { ev: RangeEvent; startIdx: number; endIdx: number; lane: number };

function layoutAllDay(events: RangeEvent[], daysArr: string[]): { chips: Chip[]; lanes: number } {
  const first = daysArr[0];
  const last = daysArr[daysArr.length - 1];
  const chips: Chip[] = [];
  for (const ev of events) {
    if (!ev.all_day) continue;
    // Google's end_date is exclusive; the chip's last visible day is one before.
    const lastDay = ev.end_date ? addDays(ev.end_date, -1) : ev.date;
    if (dayDiff(ev.date, last) > 0 || dayDiff(first, lastDay) > 0) continue;
    chips.push({
      ev,
      startIdx: Math.max(0, dayDiff(ev.date, first)),
      endIdx: Math.min(daysArr.length - 1, dayDiff(lastDay, first)),
      lane: 0,
    });
  }
  chips.sort((a, b) => a.startIdx - b.startIdx || b.endIdx - a.endIdx);
  const laneEnds: number[] = [];
  for (const c of chips) {
    let lane = laneEnds.findIndex((e) => e < c.startIdx);
    if (lane === -1) {
      lane = laneEnds.length;
      laneEnds.push(-1);
    }
    laneEnds[lane] = c.endIdx;
    c.lane = lane;
  }
  return { chips, lanes: laneEnds.length };
}

// ── create/edit form ─────────────────────────────────────────────────────────
type FormState = {
  title: string;
  date: string;
  allDay: boolean;
  start: string; // "HH:MM"
  end: string;
  endDate: string; // all-day: inclusive last day
  location: string;
  calKey: string; // `${account}|${calendarId}`
  // Google Meet intent: none = no conference, add = attach one, keep = leave
  // the existing one, remove = strip the existing one.
  meet: "none" | "add" | "keep" | "remove";
  guests: string; // create only: comma/space-separated emails
};

const calKey = (account: string, calendarId: string) => `${account}|${calendarId}`;
const splitKey = (key: string): [string, string] => {
  const i = key.indexOf("|");
  return [key.slice(0, i), key.slice(i + 1)];
};

// Calendars the user can actually write to, grouped for the <select>. An empty
// access role means the list was cached before roles existed — include it and
// let Google be the judge, rather than hiding a whole account from the picker.
function writableCals(status: GcalStatus | null, lockAccount?: string) {
  return (status?.accounts ?? [])
    .filter((a) => a.connected && (!lockAccount || a.email === lockAccount))
    .map((a) => ({
      email: a.email,
      cals: a.calendars.filter(
        (c) => c.access === "owner" || c.access === "writer" || c.access === ""
      ),
    }))
    .filter((a) => a.cals.length > 0);
}

function EventForm({
  heading,
  mode,
  init,
  status,
  contacts,
  lockAccount,
  busy,
  error,
  onSave,
  onCancel,
}: {
  heading: string;
  mode: "create" | "edit";
  init: FormState;
  status: GcalStatus | null;
  contacts: GcalContact[]; // guest autocomplete pool
  lockAccount?: string; // edit: Google can't move events across accounts
  busy: boolean;
  error: string | null;
  onSave: (f: FormState) => void;
  onCancel: () => void;
}) {
  const [f, setF] = useState<FormState>(init);
  const groups = writableCals(status, lockAccount);
  const set = (patch: Partial<FormState>) => setF((prev) => ({ ...prev, ...patch }));
  const canSave = !!f.title.trim() && !!f.date && !!f.calKey && (f.allDay || !!f.start);

  // Guest autocomplete: match the token being typed after the last comma
  // against people harvested from your calendars (name or email).
  const guestToken = f.guests.split(",").pop()?.trim().toLowerCase() ?? "";
  const guestMatches =
    mode === "create" && guestToken.length >= 1
      ? contacts
          .filter(
            (m) =>
              !f.guests.toLowerCase().includes(m.email) &&
              (m.email.includes(guestToken) || m.name.toLowerCase().includes(guestToken))
          )
          .slice(0, 6)
      : [];
  function pickGuest(email: string) {
    const parts = f.guests.split(",");
    parts[parts.length - 1] = email;
    set({ guests: parts.map((p) => p.trim()).filter(Boolean).join(", ") + ", " });
  }

  return (
    <div className="cal-overlay" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="cal-form" role="dialog" aria-label={heading}>
        <div className="cal-form-head">
          <span>{heading}</span>
          <button className="cal-pop-x" onClick={onCancel} aria-label="Close">
            <X size={14} />
          </button>
        </div>
        <input
          className="cal-form-title"
          value={f.title}
          placeholder="Add a title"
          autoFocus
          disabled={busy}
          onChange={(e) => set({ title: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter" && canSave) onSave(f);
            else if (e.key === "Escape") onCancel();
          }}
        />
        <div className="cal-form-row">
          <input
            type="date"
            value={f.date}
            disabled={busy}
            onChange={(e) => set({ date: e.target.value })}
            aria-label="Date"
          />
          {!f.allDay && (
            <>
              <input
                type="time"
                value={f.start}
                disabled={busy}
                onChange={(e) => set({ start: e.target.value })}
                aria-label="Start time"
              />
              <span className="cal-form-dash">–</span>
              <input
                type="time"
                value={f.end}
                disabled={busy}
                onChange={(e) => set({ end: e.target.value })}
                aria-label="End time"
              />
            </>
          )}
          {f.allDay && (
            <>
              <span className="cal-form-dash">–</span>
              <input
                type="date"
                value={f.endDate || f.date}
                min={f.date}
                disabled={busy}
                onChange={(e) => set({ endDate: e.target.value })}
                aria-label="End date"
              />
            </>
          )}
        </div>
        <label className="cal-form-allday">
          <input
            type="checkbox"
            checked={f.allDay}
            disabled={busy}
            onChange={(e) => set({ allDay: e.target.checked })}
          />
          All day
        </label>
        <div className="cal-form-row">
          <Video size={14} className="cal-form-ic" />
          {f.meet === "none" ? (
            <button className="cal-form-meetbtn" onClick={() => set({ meet: "add" })} disabled={busy}>
              Add Google Meet video conferencing
            </button>
          ) : (
            <span className="cal-form-meeton">
              {f.meet === "add"
                ? "Google Meet will be added"
                : f.meet === "keep"
                  ? "Google Meet attached"
                  : "Google Meet will be removed"}
              <button
                className="cal-form-meetx"
                disabled={busy}
                onClick={() =>
                  set({
                    meet:
                      f.meet === "add" ? "none" : f.meet === "keep" ? "remove" : "keep",
                  })
                }
              >
                {f.meet === "remove" ? "Undo" : "Remove"}
              </button>
            </span>
          )}
        </div>
        {mode === "create" && (
          <div className="cal-form-row">
            <Users size={14} className="cal-form-ic" />
            <div className="cal-form-guestwrap">
              <input
                value={f.guests}
                placeholder="Add guests — emails, comma-separated (they'll be invited)"
                disabled={busy}
                onChange={(e) => set({ guests: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && guestMatches.length) {
                    e.preventDefault(); // pick, don't save
                    pickGuest(guestMatches[0].email);
                  }
                }}
              />
              {guestMatches.length > 0 && (
                <div className="cal-form-suggest" role="listbox">
                  {guestMatches.map((m) => (
                    <button
                      key={m.email}
                      role="option"
                      // mousedown so the pick lands before the input blurs
                      onMouseDown={(e) => {
                        e.preventDefault();
                        pickGuest(m.email);
                      }}
                    >
                      <span className="cal-suggest-name">{m.name || m.email}</span>
                      {m.name && <span className="cal-suggest-email">{m.email}</span>}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
        <div className="cal-form-row">
          <MapPin size={14} className="cal-form-ic" />
          <input
            className="cal-form-grow"
            value={f.location}
            placeholder="Add location"
            disabled={busy}
            onChange={(e) => set({ location: e.target.value })}
          />
        </div>
        <div className="cal-form-row">
          <CalendarDays size={14} className="cal-form-ic" />
          <select
            className="cal-form-grow"
            value={f.calKey}
            disabled={busy}
            onChange={(e) => set({ calKey: e.target.value })}
            aria-label="Calendar"
          >
            {groups.map((g) => (
              <optgroup key={g.email} label={g.email}>
                {g.cals.map((c) => (
                  <option key={c.id} value={calKey(g.email, c.id)}>
                    {c.name}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </div>
        {error && <div className="cal-form-err">{error}</div>}
        <div className="cal-form-actions">
          <button className="cal-btn" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button className="cal-btn primary" onClick={() => onSave(f)} disabled={busy || !canSave}>
            {busy ? <Loader size={14} className="spin" /> : <Check size={14} />} Save
          </button>
        </div>
      </div>
    </div>
  );
}

// ── main view ────────────────────────────────────────────────────────────────
type ViewDays = 1 | 3 | 7;

export function CalendarView({ onOpenSettings }: { onOpenSettings?: () => void }) {
  const [days, setDays] = useState<ViewDays>(() => {
    const v = Number(localStorage.getItem("cal.days"));
    return v === 1 || v === 3 ? v : 7;
  });
  const today = easternDay();
  const [anchor, setAnchor] = useState(() => (days === 7 ? weekStart(today) : today));
  const [status, setStatus] = useState<GcalStatus | null>(null);
  const [events, setEvents] = useState<RangeEvent[] | null>(null);
  const [contacts, setContacts] = useState<GcalContact[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-render each minute so the now-line tracks.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  // Selected event popover (anchored near the click), create/edit form, filter.
  const [sel, setSel] = useState<{ ev: RangeEvent; x: number; y: number } | null>(null);
  const [delArmed, setDelArmed] = useState(false);
  const [form, setForm] = useState<{ mode: "create" | "edit"; init: FormState; ev?: RangeEvent } | null>(null);
  const [formBusy, setFormBusy] = useState(false);
  const [formErr, setFormErr] = useState<string | null>(null);
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement>(null);
  const selRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Hour height: 48px, squeezed down (32px floor) when the window is short so
  // the whole 8 AM – 9 PM band fits without scrolling. Measured once at mount.
  const [hourPx, setHourPx] = useState<number>(HOUR_PX_MAX);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && el.clientHeight > 0) {
      setHourPx(Math.max(32, Math.min(HOUR_PX_MAX, el.clientHeight / VIEW_HOURS)));
    }
  }, []);

  const daysArr = useMemo(
    () => Array.from({ length: days }, (_, i) => addDays(anchor, i)),
    [anchor, days]
  );

  useEffect(() => {
    api.gcalAuthStatus().then(setStatus).catch(() => setStatus(null));
  }, []);

  // Fetch the visible range; a sequence guard drops stale responses when the
  // user pages faster than the network answers.
  const seqRef = useRef(0);
  const load = useCallback(async (silent = false) => {
    const seq = ++seqRef.current;
    if (!silent) setLoading(true);
    try {
      const evs = await api.gcalEventsRange(anchor, addDays(anchor, days - 1));
      if (seq !== seqRef.current) return;
      setEvents(evs);
      setError(null);
      // The range fetch also grows the guest-autocomplete pool — pick it up.
      api.gcalContacts().then(setContacts).catch(() => {});
    } catch (e) {
      if (seq !== seqRef.current) return;
      setError(String(e));
    } finally {
      if (seq === seqRef.current) setLoading(false);
    }
  }, [anchor, days]);

  useEffect(() => {
    if (status?.connected) load();
  }, [status?.connected, load]);

  // Quiet background refresh so edits made elsewhere (phone, Google) appear.
  useEffect(() => {
    if (!status?.connected) return;
    const id = setInterval(() => load(true), 5 * 60_000);
    return () => clearInterval(id);
  }, [status?.connected, load]);

  // Anchor the window at 8 AM on open and on view switches. Deliberately a
  // fixed anchor — the grid does NOT scroll itself to follow the current time.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = VIEW_START_H * hourPx;
  }, [days, hourPx]);

  // Dismiss popovers on outside click / Escape.
  useEffect(() => {
    if (!sel && !filterOpen) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (sel && selRef.current && !selRef.current.contains(t)) setSel(null);
      if (filterOpen && filterRef.current && !filterRef.current.contains(t)) setFilterOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setSel(null);
        setFilterOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [sel, filterOpen]);

  useEffect(() => setDelArmed(false), [sel]);

  const setView = (d: ViewDays) => {
    localStorage.setItem("cal.days", String(d));
    setDays(d);
    setAnchor((a) => (d === 7 ? weekStart(a) : a));
  };
  const goToday = () => setAnchor(days === 7 ? weekStart(today) : today);
  const page = (dir: -1 | 1) => setAnchor((a) => addDays(a, dir * days));

  // Keyboard: ←/→ page, t today, d/x/w switch views. Skipped while typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (form || tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (e.key === "ArrowLeft") page(-1);
      else if (e.key === "ArrowRight") page(1);
      else if (e.key === "t") goToday();
      else if (e.key === "d") setView(1);
      else if (e.key === "x") setView(3);
      else if (e.key === "w") setView(7);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [form, days, today]);

  // Default calendar for new events: the first account's primary.
  const defaultCalKey = () => {
    const groups = writableCals(status);
    for (const g of groups) {
      const primary = g.cals.find((c) => c.primary);
      if (primary) return calKey(g.email, primary.id);
    }
    return groups.length ? calKey(groups[0].email, groups[0].cals[0].id) : "";
  };

  function openCreate(date: string, startMin: number | null) {
    const s = startMin ?? Math.min(23 * 60, Math.ceil(easternMinutes(now) / 30) * 30);
    setFormErr(null);
    setForm({
      mode: "create",
      init: {
        title: "",
        date,
        allDay: false,
        start: minToHHMM(s),
        end: minToHHMM(Math.min(s + 60, DAY_MIN - 1)),
        endDate: date,
        location: "",
        calKey: defaultCalKey(),
        meet: "none",
        guests: "",
      },
    });
  }

  function openEdit(ev: RangeEvent) {
    setSel(null);
    setFormErr(null);
    setForm({
      mode: "edit",
      ev,
      init: {
        title: ev.title,
        date: ev.date,
        allDay: ev.all_day,
        start: ev.start_min != null ? minToHHMM(ev.start_min) : "09:00",
        end: ev.end_min != null ? minToHHMM(ev.end_min) : "10:00",
        endDate: ev.end_date ? addDays(ev.end_date, -1) : ev.date,
        location: ev.location ?? "",
        calKey: calKey(ev.account, ev.calendar_id),
        meet: ev.google_meet ? "keep" : "none",
        guests: "",
      },
    });
  }

  async function saveForm(f: FormState) {
    if (!form) return;
    setFormBusy(true);
    setFormErr(null);
    const [account, calendarId] = splitKey(f.calKey);
    const input: EventInput = {
      account,
      calendarId,
      title: f.title.trim(),
      date: f.date,
      ...(f.allDay ? { endDate: f.endDate || f.date } : { start: f.start, end: f.end }),
      location: f.location.trim(),
    };
    try {
      if (form.mode === "create") {
        // One malformed address makes Google 400 the whole event — catch it
        // here with a readable message instead.
        const EMAIL_RE = /^[^\s@]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;
        const guests = f.guests
          .split(/[,\s]+/)
          .map((g) => g.trim())
          .filter(Boolean);
        const bad = guests.filter((g) => !EMAIL_RE.test(g));
        if (bad.length) {
          setFormErr(
            `That doesn't look like a valid email: ${bad.join(", ")} — check for typos (e.g. a double dot).`
          );
          setFormBusy(false);
          return;
        }
        await api.gcalCreateEvent({
          ...input,
          addMeet: f.meet === "add",
          ...(guests.length ? { guests } : {}),
        });
      } else if (form.ev) {
        // PATCH goes to the event's current calendar; a changed pick moves it.
        const moved = calendarId !== form.ev.calendar_id ? calendarId : undefined;
        // Only send a Meet intent when the user changed it — undefined keeps
        // whatever conference the event already has.
        const meet = f.meet === "add" ? true : f.meet === "remove" ? false : undefined;
        await api.gcalUpdateEvent(
          form.ev.id,
          { ...input, calendarId: form.ev.calendar_id },
          moved,
          meet
        );
      }
      setForm(null);
      await load(true);
    } catch (e) {
      setFormErr(String(e));
    } finally {
      setFormBusy(false);
    }
  }

  async function deleteSel() {
    if (!sel) return;
    if (!delArmed) {
      setDelArmed(true);
      window.setTimeout(() => setDelArmed(false), 4000);
      return;
    }
    const { ev } = sel;
    setSel(null);
    try {
      await api.gcalDeleteEvent(ev.account, ev.calendar_id, ev.id);
      await load(true);
    } catch (e) {
      setError(String(e));
    }
  }

  async function toggleCalendar(account: string, id: string, enabled: boolean) {
    try {
      setStatus(await api.gcalSetCalendarEnabled(account, id, enabled));
      await load(true);
    } catch (e) {
      setError(String(e));
    }
  }

  async function refreshCalendars() {
    try {
      setStatus(await api.gcalRefreshCalendars());
      await load(true);
    } catch (e) {
      setError(String(e));
    }
  }

  // Clicking an empty slot starts a half-hour-snapped draft there.
  function onColClick(e: React.MouseEvent<HTMLDivElement>, date: string) {
    if (e.target !== e.currentTarget) return; // an event block handled it
    const rect = e.currentTarget.getBoundingClientRect();
    const min = Math.max(0, Math.min(23.5 * 60, Math.floor(((e.clientY - rect.top) / hourPx) * 2) * 30));
    openCreate(date, min);
  }

  // ── derived render data ──
  const { chips, lanes } = useMemo(
    () => layoutAllDay(events ?? [], daysArr),
    [events, daysArr]
  );
  const grid = `64px repeat(${days}, minmax(0, 1fr))`;
  const nowMin = easternMinutes(now);
  const needsReconnect = (status?.accounts ?? []).filter((a) => !a.connected);

  // Range label: "July 2026" or "Jul – Aug 2026" when the view crosses months.
  const first = daysArr[0];
  const last = daysArr[daysArr.length - 1];
  const rangeLabel =
    first.slice(0, 7) === last.slice(0, 7)
      ? formatDay(first, { month: "long", year: "numeric" })
      : `${formatDay(first, { month: "short" })} – ${formatDay(last, { month: "short", year: "numeric" })}`;

  if (status && !status.connected) {
    return (
      <div className="cal">
        <div className="cal-connect">
          <CalendarDays size={30} className="cal-connect-icon" />
          <p className="cal-connect-title">Your calendar, all in one place</p>
          <p className="cal-connect-sub">
            Connect one or more Google accounts and every calendar shows up here — day, 3-day, and
            week views, across work and personal.
          </p>
          <button className="cal-btn primary" onClick={() => onOpenSettings?.()}>
            Connect Google Calendar
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="cal">
      <header className="cal-head">
        <div className="cal-head-left">
          <h1 className="cal-title">{rangeLabel}</h1>
          <div className="cal-nav">
            <button className="cal-icon" onClick={() => page(-1)} title="Previous" aria-label="Previous">
              <ChevronLeft size={16} />
            </button>
            <button className="cal-btn" onClick={goToday}>
              Today
            </button>
            <button className="cal-icon" onClick={() => page(1)} title="Next" aria-label="Next">
              <ChevronRight size={16} />
            </button>
          </div>
          {loading && <Loader size={14} className="spin cal-loading" />}
        </div>
        <div className="cal-head-right">
          <div className="cal-seg" role="tablist" aria-label="View">
            {([1, 3, 7] as const).map((d) => (
              <button
                key={d}
                className={days === d ? "on" : ""}
                onClick={() => setView(d)}
                role="tab"
                aria-selected={days === d}
              >
                {d === 1 ? "Day" : d === 3 ? "3 days" : "Week"}
              </button>
            ))}
          </div>
          <div className="cal-filterwrap" ref={filterRef}>
            <button
              className={"cal-icon" + (filterOpen ? " active" : "")}
              onClick={() => setFilterOpen((o) => !o)}
              title="Choose calendars"
              aria-label="Choose calendars"
              aria-expanded={filterOpen}
            >
              <SlidersHorizontal size={15} />
            </button>
            {filterOpen && (
              <div className="cal-pop cal-filter" role="dialog" aria-label="Calendars">
                <div className="cal-pop-head">
                  <span>Calendars</span>
                  <button className="cal-pop-x" onClick={refreshCalendars} title="Refresh calendar lists">
                    <RefreshCw size={13} />
                  </button>
                </div>
                {(status?.accounts ?? []).map((a) => (
                  <div key={a.email} className="cal-filter-acct">
                    <div className="cal-filter-email">
                      {a.email}
                      {!a.connected && <span className="cal-filter-warn">reconnect in Settings</span>}
                    </div>
                    {a.calendars.map((c) => (
                      <label key={c.id} className="cal-filter-row">
                        <input
                          type="checkbox"
                          checked={c.enabled}
                          onChange={(e) => toggleCalendar(a.email, c.id, e.target.checked)}
                        />
                        <span className="cal-dot" style={{ background: c.color }} />
                        <span className="cal-filter-name">{c.name}</span>
                      </label>
                    ))}
                  </div>
                ))}
                <button className="cal-filter-add" onClick={() => onOpenSettings?.()}>
                  <Plus size={14} /> Add a Google account
                </button>
              </div>
            )}
          </div>
          <button className="cal-icon" onClick={() => load()} title="Refresh events" aria-label="Refresh events">
            <RefreshCw size={15} />
          </button>
          <button className="cal-btn primary" onClick={() => openCreate(daysArr.includes(today) ? today : anchor, null)}>
            <Plus size={15} /> Event
          </button>
        </div>
      </header>

      {needsReconnect.length > 0 && (
        <div className="cal-notice">
          {needsReconnect.map((a) => a.email).join(", ")} disconnected —{" "}
          <button onClick={() => onOpenSettings?.()}>reconnect in Settings</button>
        </div>
      )}
      {error && (
        <div className="cal-notice err">
          {error} <button onClick={() => load()}>retry</button>
        </div>
      )}

      <div className="cal-dayheads" style={{ gridTemplateColumns: grid }}>
        <div />
        {daysArr.map((d) => (
          <div key={d} className={"cal-dayhead" + (d === today ? " today" : "")}>
            <span className="cal-dow">{formatDay(d, { weekday: "short" })}</span>
            <span className="cal-dom">{Number(d.slice(8))}</span>
          </div>
        ))}
      </div>

      {chips.length > 0 && (
        <div className="cal-allday" style={{ gridTemplateColumns: grid }}>
          <div className="cal-allday-label">all-day</div>
          <div
            className="cal-allday-lanes"
            style={{
              gridColumn: `2 / span ${days}`,
              gridTemplateColumns: `repeat(${days}, minmax(0, 1fr))`,
              gridTemplateRows: `repeat(${lanes}, 22px)`,
            }}
          >
            {chips.map((c, i) => (
              <button
                key={c.ev.id + i}
                className={"cal-chip" + (c.ev.declined ? " declined" : "")}
                style={
                  {
                    gridColumn: `${c.startIdx + 1} / ${c.endIdx + 2}`,
                    gridRow: c.lane + 1,
                    "--ev-color": c.ev.color,
                  } as React.CSSProperties
                }
                onClick={(e) => setSel({ ev: c.ev, x: e.clientX, y: e.clientY })}
                title={c.ev.title}
              >
                {c.ev.title}
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="cal-scroll" ref={scrollRef}>
        <div className="cal-grid" style={{ gridTemplateColumns: grid }}>
          <div className="cal-gutter" style={{ height: 24 * hourPx }}>
            {Array.from({ length: 23 }, (_, i) => i + 1).map((h) => (
              <span key={h} className="cal-hourlabel" style={{ top: h * hourPx }}>
                {fmtMin(h * 60)}
              </span>
            ))}
          </div>
          {daysArr.map((d) => {
            const segs = layoutSegs(segmentsForDay(events ?? [], d));
            return (
              <div
                key={d}
                className="cal-col"
                style={{
                  height: 24 * hourPx,
                  backgroundImage: `repeating-linear-gradient(to bottom, var(--line) 0 1px, transparent 1px ${hourPx}px)`,
                }}
                onClick={(e) => onColClick(e, d)}
              >
                {segs.map((s, i) => {
                  const h = Math.max(20, ((s.endMin - s.startMin) / 60) * hourPx - 2);
                  const showTime = h >= 34 && s.ev.start_min != null;
                  return (
                    <button
                      key={s.ev.id + i}
                      className={"cal-ev" + (s.ev.declined ? " declined" : "")}
                      style={
                        {
                          top: (s.startMin / 60) * hourPx,
                          height: h,
                          left: `calc(${(s.col / s.cols) * 100}% + 2px)`,
                          width: `calc(${(1 / s.cols) * 100}% - 5px)`,
                          "--ev-color": s.ev.color,
                        } as React.CSSProperties
                      }
                      onClick={(e) => {
                        e.stopPropagation();
                        setSel({ ev: s.ev, x: e.clientX, y: e.clientY });
                      }}
                      title={s.ev.title}
                    >
                      <span className="cal-ev-title">{s.ev.title}</span>
                      {showTime && (
                        <span className="cal-ev-time">
                          {fmtMin(s.ev.start_min!)} – {fmtMin(s.ev.end_min ?? s.ev.start_min! + 60)}
                        </span>
                      )}
                    </button>
                  );
                })}
                {d === today && (
                  <div className="cal-now" style={{ top: (nowMin / 60) * hourPx }}>
                    <span className="cal-now-dot" />
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {sel && (
        <div
          ref={selRef}
          className="cal-pop cal-evcard"
          style={{
            left: Math.max(8, Math.min(sel.x, window.innerWidth - 348)),
            top: Math.max(8, Math.min(sel.y + 8, window.innerHeight - 240)),
          }}
          role="dialog"
          aria-label="Event details"
        >
          <div className="cal-evcard-head">
            <span className="cal-dot" style={{ background: sel.ev.color }} />
            <span className={"cal-evcard-title" + (sel.ev.declined ? " declined" : "")}>
              {sel.ev.title}
            </span>
            <button className="cal-pop-x" onClick={() => setSel(null)} aria-label="Close">
              <X size={14} />
            </button>
          </div>
          <div className="cal-evcard-when">
            {formatDay(sel.ev.date, { weekday: "long", month: "long", day: "numeric" })}
            {sel.ev.all_day
              ? sel.ev.end_date && dayDiff(addDays(sel.ev.end_date, -1), sel.ev.date) > 0
                ? ` – ${formatDay(addDays(sel.ev.end_date, -1), { weekday: "long", month: "long", day: "numeric" })}`
                : " · All day"
              : ` · ${fmtMin(sel.ev.start_min!)} – ${fmtMin(sel.ev.end_min ?? sel.ev.start_min! + 60)}${
                  (sel.ev.end_min ?? 0) > DAY_MIN ? " (next day)" : ""
                }`}
          </div>
          {sel.ev.meet_link && (
            <a
              className="cal-btn primary cal-evcard-join"
              href={sel.ev.meet_link}
              target="_blank"
              rel="noreferrer"
            >
              <Video size={13} /> Join meeting
            </a>
          )}
          {sel.ev.declined && <div className="cal-evcard-meta">You declined this event</div>}
          {sel.ev.location && (
            <div className="cal-evcard-meta">
              <MapPin size={13} /> <Linkified text={sel.ev.location} />
            </div>
          )}
          {sel.ev.attendee_count > 0 && (
            <div className="cal-evcard-meta">
              <Users size={13} />
              <span>
                {sel.ev.attendee_count} {sel.ev.attendee_count === 1 ? "guest" : "guests"}
                {sel.ev.attendees.length > 0 && (
                  <>
                    {" — "}
                    {sel.ev.attendees
                      .slice(0, 5)
                      .map(
                        (a) =>
                          (a.self ? "you" : a.name.split("@")[0]) +
                          (a.status === "declined" ? " (declined)" : "")
                      )
                      .join(", ")}
                    {sel.ev.attendee_count > 5 ? ` +${sel.ev.attendee_count - 5} more` : ""}
                  </>
                )}
              </span>
            </div>
          )}
          {sel.ev.organizer && sel.ev.attendee_count > 0 && (
            <div className="cal-evcard-meta">organized by {sel.ev.organizer.split("@")[0]}</div>
          )}
          <div className="cal-evcard-meta">
            <CalendarDays size={13} /> {sel.ev.calendar} · {sel.ev.account}
          </div>
          {sel.ev.description && (
            <div className="cal-evcard-desc">
              <Linkified text={sel.ev.description} />
            </div>
          )}
          <div className="cal-evcard-actions">
            <button className="cal-btn" onClick={() => openEdit(sel.ev)}>
              <Pencil size={13} /> Edit
            </button>
            <button className={"cal-btn danger" + (delArmed ? " armed" : "")} onClick={deleteSel}>
              <Trash2 size={13} /> {delArmed ? "Confirm delete" : "Delete"}
            </button>
            {sel.ev.html_link && (
              <a
                className="cal-btn cal-evcard-gcal"
                href={sel.ev.html_link}
                target="_blank"
                rel="noreferrer"
                title="Open in Google Calendar"
              >
                <ExternalLink size={13} /> Open
              </a>
            )}
          </div>
        </div>
      )}

      {form && (
        <EventForm
          heading={form.mode === "create" ? "New event" : "Edit event"}
          mode={form.mode}
          init={form.init}
          status={status}
          contacts={contacts}
          lockAccount={form.mode === "edit" ? form.ev?.account : undefined}
          busy={formBusy}
          error={formErr}
          onSave={saveForm}
          onCancel={() => setForm(null)}
        />
      )}
    </div>
  );
}
