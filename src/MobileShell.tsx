import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import "./MobileShell.css";

type MobileNote = {
  id: number;
  title: string;
  body: string;
  created_at: number;
  updated_at: number;
};

type NoteDraft = {
  id: number | null;
  title: string;
  body: string;
};

function messageFrom(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

function noteTitle(draft: NoteDraft) {
  const explicitTitle = draft.title.trim();
  if (explicitTitle) return explicitTitle;
  return draft.body.split("\n").find((line) => line.trim())?.trim().slice(0, 80) || "Untitled note";
}

function notePreview(note: MobileNote) {
  return note.body.replace(/\s+/g, " ").trim() || "No additional text";
}

function formatUpdated(timestamp: number) {
  const date = new Date(timestamp);
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return new Intl.DateTimeFormat(undefined, sameDay
    ? { hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric" }).format(date);
}

function BackIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M14.5 5.5 8 12l6.5 6.5" />
    </svg>
  );
}

function SearchIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="10.5" cy="10.5" r="5.75" />
      <path d="m15 15 4.25 4.25" />
    </svg>
  );
}

function ComposeIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M13.5 5.5h-7a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-7" />
      <path d="m11 13 1.2-3.7 5.9-5.9a1.75 1.75 0 0 1 2.5 2.5l-5.9 5.9L11 13Z" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5.5 7.5h13M9 7.5V5h6v2.5M7.5 7.5l.75 11h7.5l.75-11M10 10.5v5M14 10.5v5" />
    </svg>
  );
}

function NoteEditor({
  draft,
  busy,
  error,
  onChange,
  onClose,
  onSave,
  onDelete,
}: {
  draft: NoteDraft;
  busy: boolean;
  error: string | null;
  onChange: (draft: NoteDraft) => void;
  onClose: () => void;
  onSave: () => void;
  onDelete: () => void;
}) {
  const titleRef = useRef<HTMLInputElement>(null);
  const editorRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (draft.id === null) titleRef.current?.focus();
  }, [draft.id]);

  useEffect(() => {
    const viewport = window.visualViewport;
    const alignToVisibleViewport = () => {
      if (!editorRef.current) return;
      editorRef.current.style.height = `${viewport?.height ?? window.innerHeight}px`;
      editorRef.current.style.transform = `translateY(${viewport?.offsetTop ?? 0}px)`;
    };

    alignToVisibleViewport();
    viewport?.addEventListener("resize", alignToVisibleViewport);
    viewport?.addEventListener("scroll", alignToVisibleViewport);
    return () => {
      viewport?.removeEventListener("resize", alignToVisibleViewport);
      viewport?.removeEventListener("scroll", alignToVisibleViewport);
    };
  }, []);

  return (
    <main ref={editorRef} className="note-editor">
      <header className="note-editor__toolbar">
        <button className="icon-action icon-action--back" type="button" onClick={onClose} aria-label="Close note">
          <BackIcon />
        </button>
        <div className="note-editor__actions">
          {draft.id !== null && (
            <button className="icon-action icon-action--danger" type="button" onClick={onDelete} disabled={busy} aria-label="Delete note">
              <TrashIcon />
            </button>
          )}
          <button className="save-action" type="button" onClick={onSave} disabled={busy}>
            {busy ? "Saving" : "Done"}
          </button>
        </div>
      </header>

      <section className="note-editor__page">
        <input
          ref={titleRef}
          className="note-editor__title"
          value={draft.title}
          onChange={(event) => onChange({ ...draft, title: event.target.value })}
          placeholder="Title"
          aria-label="Note title"
        />
        <textarea
          className="note-editor__body"
          value={draft.body}
          onChange={(event) => onChange({ ...draft, body: event.target.value })}
          placeholder="Start writing…"
          aria-label="Note body"
        />
        {error && <p className="inline-error" role="alert">{error}</p>}
      </section>
    </main>
  );
}

export function MobileShell() {
  const [notes, setNotes] = useState<MobileNote[]>([]);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<NoteDraft | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      invoke<MobileNote[]>("list_mobile_notes", { query: query.trim() || null })
        .then((result) => {
          if (!cancelled) {
            setNotes(result);
            setError(null);
          }
        })
        .catch((reason: unknown) => {
          if (!cancelled) setError(messageFrom(reason));
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, query ? 120 : 0);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query]);

  const noteCount = useMemo(() => {
    if (loading) return "Loading";
    if (query) return `${notes.length} ${notes.length === 1 ? "result" : "results"}`;
    return `${notes.length} ${notes.length === 1 ? "note" : "notes"}`;
  }, [loading, notes.length, query]);

  async function refresh() {
    const result = await invoke<MobileNote[]>("list_mobile_notes", { query: query.trim() || null });
    setNotes(result);
  }

  async function saveDraft() {
    if (!draft) return;
    if (!draft.title.trim() && !draft.body.trim()) {
      setDraft(null);
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const payload = { title: noteTitle(draft), body: draft.body };
      if (draft.id === null) {
        await invoke("create_mobile_note", payload);
      } else {
        await invoke("update_mobile_note", { ...payload, id: draft.id });
      }
      await refresh();
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setBusy(false);
    }
  }

  async function deleteDraft() {
    if (!draft?.id) return;
    if (!window.confirm(`Delete “${noteTitle(draft)}”?`)) return;

    setBusy(true);
    setError(null);
    try {
      await invoke("delete_mobile_note", { id: draft.id });
      await refresh();
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setBusy(false);
    }
  }

  if (draft) {
    return (
      <NoteEditor
        draft={draft}
        busy={busy}
        error={error}
        onChange={setDraft}
        onClose={() => setDraft(null)}
        onSave={saveDraft}
        onDelete={deleteDraft}
      />
    );
  }

  return (
    <main className="notes-screen">
      <header className="notes-header">
        <div>
          <h1>Notes</h1>
          <p>{noteCount} on this iPhone</p>
        </div>
        <button
          className="compose-action"
          type="button"
          onClick={() => setDraft({ id: null, title: "", body: "" })}
          aria-label="Create note"
        >
          <ComposeIcon />
        </button>
      </header>

      <label className="search-field">
        <SearchIcon />
        <span className="sr-only">Search notes</span>
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search notes"
          autoCapitalize="none"
        />
      </label>

      {error && <p className="library-error" role="alert">Couldn’t open your local notes. {error}</p>}

      {!loading && !error && notes.length === 0 ? (
        <section className="empty-library">
          <div className="empty-library__mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <h2>{query ? "No matching notes" : "A clear page"}</h2>
          <p>{query ? "Try a different word or phrase." : "Write something here. It stays on this iPhone."}</p>
          {!query && (
            <button type="button" onClick={() => setDraft({ id: null, title: "", body: "" })}>
              Create your first note
            </button>
          )}
        </section>
      ) : (
        <section className="note-list" aria-label="Local notes">
          {notes.map((note) => (
            <button
              className="note-row"
              type="button"
              key={note.id}
              onClick={() => setDraft({ id: note.id, title: note.title, body: note.body })}
            >
              <span className="note-row__time">{formatUpdated(note.updated_at)}</span>
              <span className="note-row__content">
                <strong>{note.title}</strong>
                <span>{notePreview(note)}</span>
              </span>
            </button>
          ))}
        </section>
      )}
    </main>
  );
}
