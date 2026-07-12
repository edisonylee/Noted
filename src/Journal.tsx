import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUp,
  Check,
  ChevronLeft,
  ChevronRight,
  Loader,
  LockKeyhole,
  NotebookPen,
  Sparkles,
} from "lucide-react";
import { api, type NoteRow } from "./api";
import { APP_TZ, easternDay, easternHour, formatDay, relativeDay } from "./day";

type Msg = {
  id: string;
  role: "you" | "journal";
  text: string;
  createdAt?: string;
  meta?: string;
};

const JOURNAL_DRAFT_KEY = "noted-journal-draft";
const isJournal = (cat: string | null) => cat?.toLowerCase() === "journal";

// Time-adaptive opening prompt, mirroring the home greeting's tone.
function journalPrompt(): string {
  const h = easternHour();
  if (h < 11) return "What's on your mind this morning?";
  if (h < 17) return "How is today actually going?";
  return "How did today feel? What stayed with you?";
}

function notesForDay(notes: NoteRow[], day: string): NoteRow[] {
  return notes
    .filter((note) => note.event_date === day && note.entries.some((entry) => isJournal(entry.category)))
    .sort((a, b) => a.created_at.localeCompare(b.created_at));
}

function notesToMessages(notes: NoteRow[], day: string): Msg[] {
  return notesForDay(notes, day)
    .map((note) => ({
      id: `note-${note.id}`,
      role: "you" as const,
      text: note.raw_text ?? "",
      createdAt: note.created_at,
      meta: "Saved privately on this Mac",
    }))
    .filter((message) => message.text.trim());
}

function pageDate(day: string): { weekday: string; date: string } {
  return {
    weekday: formatDay(day, { weekday: "long" }),
    date: formatDay(day, { month: "long", day: "numeric", year: "numeric" }),
  };
}

// Calendar-day movement anchored in UTC, so stepping across a daylight-saving
// boundary still lands on the immediately previous or next YYYY-MM-DD.
function shiftDay(day: string, amount: number): string {
  const [year, month, date] = day.split("-").map(Number);
  const shifted = new Date(Date.UTC(year, month - 1, date + amount, 12));
  return [
    shifted.getUTCFullYear(),
    String(shifted.getUTCMonth() + 1).padStart(2, "0"),
    String(shifted.getUTCDate()).padStart(2, "0"),
  ].join("-");
}

function entryTime(createdAt?: string): string {
  if (!createdAt) return "Just now";
  const instant = new Date(createdAt);
  if (Number.isNaN(instant.getTime())) return "Entry";
  return instant.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
    timeZone: APP_TZ,
  });
}

function preview(text: string): string {
  const singleLine = text.replace(/\s+/g, " ").trim();
  return singleLine.length > 54 ? `${singleLine.slice(0, 54).trimEnd()}…` : singleLine;
}

const startingLines = [
  { label: "A moment", text: "A moment I want to remember today was…" },
  { label: "What's lingering", text: "Something that's been lingering in my mind is…" },
  { label: "A small good thing", text: "One small thing I'm grateful for today is…" },
];

export function JournalView({
  notes,
  onSaved,
}: {
  notes: NoteRow[];
  onSaved: () => void | Promise<void>;
}) {
  const today = easternDay();
  const journalNotes = useMemo(
    () => notes.filter((note) => note.entries.some((entry) => isJournal(entry.category))),
    [notes]
  );
  const [selectedDay, setSelectedDay] = useState(today);
  const [msgs, setMsgs] = useState<Msg[]>(() => notesToMessages(notes, today));
  const [input, setInput] = useState(() => localStorage.getItem(JOURNAL_DRAFT_KEY) ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLTextAreaElement>(null);
  const hasWrittenRef = useRef(false);

  const archiveDays = useMemo(() => {
    const grouped = new Map<string, NoteRow[]>();
    for (const note of journalNotes) {
      const dayNotes = grouped.get(note.event_date) ?? [];
      dayNotes.push(note);
      grouped.set(note.event_date, dayNotes);
    }
    if (!grouped.has(today)) grouped.set(today, []);
    return [...grouped.entries()]
      .map(([day, dayNotes]) => ({
        day,
        notes: dayNotes.sort((a, b) => a.created_at.localeCompare(b.created_at)),
      }))
      .sort((a, b) => b.day.localeCompare(a.day));
  }, [journalNotes, today]);

  const isToday = selectedDay === today;
  const visibleMsgs = isToday ? msgs : notesToMessages(journalNotes, selectedDay);
  const selectedDate = pageDate(selectedDay);
  const wordCount = input.trim() ? input.trim().split(/\s+/).length : 0;

  // Notes can arrive after this view mounts. Keep today's page in sync until
  // the user starts a live writing session (whose companion replies are local
  // to this mounted view and deliberately not persisted).
  useEffect(() => {
    if (!hasWrittenRef.current) setMsgs(notesToMessages(notes, today));
  }, [notes, today]);

  useEffect(() => {
    if (isToday) endRef.current?.scrollIntoView({ block: "end" });
  }, [msgs, busy, isToday]);

  useEffect(() => {
    if (!isToday || !boxRef.current) return;
    boxRef.current.style.height = "auto";
    boxRef.current.style.height = `${Math.min(boxRef.current.scrollHeight, 240)}px`;
  }, [input, isToday]);

  function beginToday(text?: string) {
    setSelectedDay(today);
    if (text != null) {
      setInput(text);
      localStorage.setItem(JOURNAL_DRAFT_KEY, text);
    }
    requestAnimationFrame(() => boxRef.current?.focus());
  }

  async function saveEntry() {
    const text = input.trim();
    if (!text || busy) return;
    const draftId = `draft-${Date.now()}`;
    hasWrittenRef.current = true;
    setError(null);
    setBusy(true);
    setInput("");

    // The private local model gets only this sitting's recent page context.
    const history = msgs.map((message) => ({
      role: message.role === "journal" ? "assistant" : "user",
      content: message.text,
    }));
    setMsgs((current) => [
      ...current,
      { id: draftId, role: "you", text, createdAt: new Date().toISOString() },
    ]);

    try {
      const result = await api.journalReflect(text, history);
      setMsgs((current) => {
        const next = current.map((message) =>
          message.id === draftId
            ? {
                ...message,
                id: `note-${result.note_id}`,
                meta: "Saved privately on this Mac",
              }
            : message
        );
        if (result.reply) {
          next.push({ id: `reflection-${result.note_id}`, role: "journal", text: result.reply });
        }
        return next;
      });
      localStorage.removeItem(JOURNAL_DRAFT_KEY);
      await onSaved();
    } catch (caught) {
      setError(String(caught));
      setInput(text);
      localStorage.setItem(JOURNAL_DRAFT_KEY, text);
      setMsgs((current) => current.filter((message) => message.id !== draftId));
    } finally {
      setBusy(false);
      boxRef.current?.focus();
    }
  }

  return (
    <div className="journal">
      <aside className="journal-index" aria-label="Journal entries">
        <div className="journal-index-head">
          <span className="journal-index-mark" aria-hidden>
            <NotebookPen size={17} />
          </span>
          <div>
            <h1>Journal</h1>
            <p>
              <LockKeyhole size={11} /> Private · on this Mac
            </p>
          </div>
        </div>

        <button className="journal-new" onClick={() => beginToday()}>
          <span>Write today</span>
          <NotebookPen size={14} />
        </button>

        <div className="journal-index-label">Written pages</div>
        <nav className="journal-index-list">
          {archiveDays.map(({ day, notes: dayNotes }) => (
            <button
              key={day}
              className={selectedDay === day ? "on" : ""}
              onClick={() => setSelectedDay(day)}
              aria-current={selectedDay === day ? "page" : undefined}
            >
              <span className="journal-index-date">
                <strong>{relativeDay(day)}</strong>
                <time dateTime={day}>{formatDay(day, { month: "short", day: "numeric" })}</time>
              </span>
              <span className="journal-index-count">
                {dayNotes.length || "Blank"}
              </span>
              <span className="journal-index-preview">
                {dayNotes.length ? preview(dayNotes[dayNotes.length - 1].raw_text) : "A new page"}
              </span>
            </button>
          ))}
        </nav>
      </aside>

      <section className="journal-page" aria-label={`Journal page for ${selectedDate.date}`}>
        <header className="journal-page-head">
          <div className="journal-page-overline">
            <time dateTime={selectedDay}>{selectedDate.weekday}</time>
            <div className="journal-page-tools">
              <span className="journal-page-privacy">
                <LockKeyhole size={11} /> Private entry
              </span>
              <div className="journal-day-nav" aria-label="Move between journal days">
                <button
                  type="button"
                  onClick={() => setSelectedDay((day) => shiftDay(day, -1))}
                  title="Previous day"
                  aria-label="Previous day"
                >
                  <ChevronLeft size={14} />
                </button>
                {!isToday && (
                  <button type="button" className="journal-today" onClick={() => beginToday()}>
                    Today
                  </button>
                )}
                <button
                  type="button"
                  onClick={() => setSelectedDay((day) => shiftDay(day, 1))}
                  disabled={selectedDay >= today}
                  title="Next day"
                  aria-label="Next day"
                >
                  <ChevronRight size={14} />
                </button>
              </div>
            </div>
          </div>
          <h2>{selectedDate.date}</h2>
          <p>{isToday ? journalPrompt() : "A page from your journal."}</p>
        </header>

        <div className="journal-paper">
          <div className="journal-thread">
            {visibleMsgs.length === 0 && !busy && (
              <div className="journal-empty">
                <NotebookPen size={21} />
                <h3>{isToday ? "The page is yours." : "Nothing was written on this page."}</h3>
                {isToday && (
                  <>
                    <p>Write the honest version. It can be a sentence, a memory, or the whole story.</p>
                    <div className="journal-starters" aria-label="Writing prompts">
                      {startingLines.map((starter) => (
                        <button key={starter.label} onClick={() => beginToday(starter.text)}>
                          {starter.label}
                        </button>
                      ))}
                    </div>
                  </>
                )}
              </div>
            )}

            {visibleMsgs.map((message) =>
              message.role === "you" ? (
                <article className="journal-entry" key={message.id}>
                  <div className="journal-entry-head">
                    <time>{entryTime(message.createdAt)}</time>
                    {message.meta && (
                      <span>
                        <Check size={11} /> {message.meta}
                      </span>
                    )}
                  </div>
                  <p>{message.text}</p>
                </article>
              ) : (
                <aside className="journal-reflection" key={message.id}>
                  <div className="journal-reflection-label">
                    <Sparkles size={13} /> Reflection
                  </div>
                  <p>{message.text}</p>
                </aside>
              )
            )}

            {busy && (
              <aside className="journal-reflection journal-reflecting" aria-live="polite">
                <div className="journal-reflection-label">
                  <Loader size={13} className="spin" /> Reflecting
                </div>
                <p>Your entry is being read privately on this Mac…</p>
              </aside>
            )}
            <div ref={endRef} />
          </div>

          {error && <div className="error journal-error">{error}</div>}

          {isToday ? (
            <div className="journal-composer">
              <label htmlFor="journal-entry">Continue the page</label>
              <textarea
                id="journal-entry"
                ref={boxRef}
                value={input}
                placeholder="Start writing…"
                disabled={busy}
                autoFocus
                onChange={(event) => {
                  const value = event.target.value;
                  setInput(value);
                  localStorage.setItem(JOURNAL_DRAFT_KEY, value);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault();
                    saveEntry();
                  }
                }}
              />
              <div className="journal-composer-foot">
                <span>{wordCount === 1 ? "1 word" : `${wordCount} words`} · ⌘↵ to save</span>
                <button
                  className="journal-save"
                  onClick={saveEntry}
                  disabled={busy || !input.trim()}
                >
                  {busy ? <Loader size={14} className="spin" /> : <ArrowUp size={14} />}
                  Save entry
                </button>
              </div>
            </div>
          ) : (
            <div className="journal-archive-foot">
              <span>This page is read-only.</span>
              <button onClick={() => beginToday()}>Return to today</button>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
