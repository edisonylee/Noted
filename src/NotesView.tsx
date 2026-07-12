// The Notes workspace: a Spaces-style left column (All / Meetings / Journal /
// every category) over the capture archive, with quick text filtering.
// Meeting notes open the full MeetingPage (transcript + summary tabs);
// everything else gets a clean read-only detail pane.

import { useMemo, useState } from "react";
import { ArrowLeft, AudioLines, BookOpen, FileText, Inbox, Search } from "lucide-react";
import { type CategoryInfo, type NoteRow } from "./api";
import { DataView } from "./DataView";
import { MeetingPage } from "./MeetingPage";
import { relativeDay } from "./day";

function noteCats(n: NoteRow): string[] {
  return n.entries
    .map((e) => (e.category ?? "").toLowerCase())
    .filter(Boolean);
}

/// A note's display title: its first real line, sans markdown heading marks.
function noteTitle(n: NoteRow): string {
  const line = n.raw_text
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0);
  if (!line) return "(empty note)";
  return line.replace(/^#+\s*/, "").slice(0, 90);
}

function meetingIdOf(n: NoteRow): number | null {
  for (const e of n.entries) {
    if ((e.category ?? "").toLowerCase() === "meetings") {
      const id = e.data?.["meeting_id"];
      if (typeof id === "number") return id;
    }
  }
  return null;
}

export function NotesView({ notes, cats }: { notes: NoteRow[]; cats: CategoryInfo[] }) {
  const [space, setSpace] = useState("all");
  const [query, setQuery] = useState("");
  const [openNote, setOpenNote] = useState<NoteRow | null>(null);
  const [openMeeting, setOpenMeeting] = useState<number | null>(null);

  // Spaces: the two first-class ones, then every other category by volume.
  const spaces = useMemo(() => {
    const count = (pred: (n: NoteRow) => boolean) => notes.filter(pred).length;
    const fixed = [
      { id: "all", label: "All notes", n: notes.length },
      { id: "meetings", label: "Meetings", n: count((x) => noteCats(x).includes("meetings")) },
      { id: "journal", label: "Journal", n: count((x) => noteCats(x).includes("journal")) },
    ];
    const rest = cats
      .map((c) => c.name.toLowerCase())
      .filter((name) => name !== "meetings" && name !== "journal")
      .map((name) => ({
        id: name,
        label: name.charAt(0).toUpperCase() + name.slice(1),
        n: count((x) => noteCats(x).includes(name)),
      }))
      .filter((s) => s.n > 0)
      .sort((a, b) => b.n - a.n);
    return [...fixed, ...rest];
  }, [notes, cats]);

  const list = useMemo(() => {
    let rows = space === "all" ? notes : notes.filter((n) => noteCats(n).includes(space));
    const q = query.trim().toLowerCase();
    if (q) rows = rows.filter((n) => n.raw_text.toLowerCase().includes(q));
    return rows;
  }, [notes, space, query]);

  if (openMeeting != null) {
    return <MeetingPage id={openMeeting} onBack={() => setOpenMeeting(null)} />;
  }

  if (openNote) {
    return (
      <div className="note-detail">
        <header className="note-detail-head">
          <button className="icon-btn" onClick={() => setOpenNote(null)} aria-label="Back">
            <ArrowLeft size={18} />
          </button>
          <div>
            <h2>{noteTitle(openNote)}</h2>
            <span className="note-detail-meta">
              {relativeDay(openNote.event_date)} · {openNote.source}
              {noteCats(openNote).map((c) => (
                <em key={c} className="note-chip">
                  {c}
                </em>
              ))}
            </span>
          </div>
        </header>
        <div className="note-detail-body">{openNote.raw_text}</div>
        {openNote.entries.some((e) => e.data && Object.keys(e.data).length > 0) && (
          <div className="note-detail-entries">
            {openNote.entries.map(
              (e, i) =>
                e.data &&
                Object.keys(e.data).length > 0 && (
                  <div key={e.id ?? i} className="note-entry-card">
                    <span className="note-chip">{e.category ?? "…"}</span>
                    <DataView value={e.data} />
                  </div>
                )
            )}
          </div>
        )}
      </div>
    );
  }

  const open = (n: NoteRow) => {
    const mid = meetingIdOf(n);
    if (mid != null) setOpenMeeting(mid);
    else setOpenNote(n);
  };

  return (
    <div className="notes-view">
      <aside className="spaces">
        {spaces.map((s) => (
          <button
            key={s.id}
            className={space === s.id ? "on" : ""}
            onClick={() => setSpace(s.id)}
          >
            {s.id === "meetings" ? (
              <AudioLines size={14} />
            ) : s.id === "journal" ? (
              <BookOpen size={14} />
            ) : s.id === "all" ? (
              <Inbox size={14} />
            ) : (
              <FileText size={14} />
            )}
            <span className="space-label">{s.label}</span>
            <span className="space-n">{s.n}</span>
          </button>
        ))}
      </aside>

      <div className="notes-list">
        <label className="notes-search">
          <Search size={14} />
          <input
            placeholder="Search notes…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        {list.length === 0 ? (
          <p className="quiet-empty">
            {query ? "Nothing matches." : "Nothing here yet — capture something."}
          </p>
        ) : (
          list.map((n) => (
            <button key={n.id} className="note-row" onClick={() => open(n)}>
              {meetingIdOf(n) != null ? (
                <AudioLines size={14} className="note-row-icon" />
              ) : (
                <FileText size={14} className="note-row-icon" />
              )}
              <span className="note-row-title">{noteTitle(n)}</span>
              <span className="note-row-chips">
                {noteCats(n)
                  .slice(0, 2)
                  .map((c) => (
                    <em key={c} className="note-chip">
                      {c}
                    </em>
                  ))}
              </span>
              <span className="note-row-date">{relativeDay(n.event_date)}</span>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
