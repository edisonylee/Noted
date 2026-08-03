// The Notes workspace separates model-generated categories from user-owned
// organization. Folders are the visible hierarchy; legacy space roots remain
// as hidden storage anchors so existing filing data stays intact.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import {
  ArrowLeft,
  AudioLines,
  BookOpen,
  CalendarDays,
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Inbox,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  PenLine,
  Plus,
  Search,
  Trash2,
} from "lucide-react";
import {
  api,
  type CategoryInfo,
  type MeetingListRow,
  type NoteFolderInfo,
  type NoteRow,
  type TranscriptSearchHit,
} from "./api";
import { DataView } from "./DataView";
import { MeetingPage } from "./MeetingPage";
import { easternDay, formatDay, relativeDay } from "./day";

type CreateTarget = {
  parentId: number;
  label: string;
};

type WeekGroup = {
  key: string;
  label: string;
  notes: NoteRow[];
};

type MeetingTarget = {
  id: number;
  segmentId?: number;
};

function transcriptTimestamp(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function transcriptMeetingDate(startedAt: string | null): string {
  if (!startedAt) return "Date unavailable";
  const day = easternDay(new Date(startedAt));
  const sameYear = day.slice(0, 4) === easternDay().slice(0, 4);
  return formatDay(day, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

function transcriptSnippet(text: string, query: string, limit = 210): string {
  if (text.length <= limit) return text;
  const terms = query
    .split(/[^\p{L}\p{N}_]+/u)
    .map((term) => term.trim().toLocaleLowerCase())
    .filter(Boolean);
  const lower = text.toLocaleLowerCase();
  const matchAt = terms.reduce((earliest, term) => {
    const at = lower.indexOf(term);
    return at >= 0 && (earliest < 0 || at < earliest) ? at : earliest;
  }, -1);
  const center = matchAt >= 0 ? matchAt : 0;
  let start = Math.max(0, center - Math.floor(limit * 0.42));
  let end = Math.min(text.length, start + limit);
  if (end === text.length) start = Math.max(0, end - limit);
  if (start > 0) {
    const nextSpace = text.indexOf(" ", start);
    if (nextSpace > start && nextSpace < start + 24) start = nextSpace + 1;
  }
  if (end < text.length) {
    const previousSpace = text.lastIndexOf(" ", end);
    if (previousSpace > end - 24) end = previousSpace;
  }
  return `${start > 0 ? "…" : ""}${text.slice(start, end)}${end < text.length ? "…" : ""}`;
}

function highlightedTranscript(text: string, query: string): ReactNode {
  const terms = query
    .split(/[^\p{L}\p{N}_]+/u)
    .map((term) => term.trim())
    .filter(Boolean)
    .sort((a, b) => b.length - a.length);
  if (terms.length === 0) return text;
  const escaped = terms.map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const matcher = new RegExp(`(${escaped.join("|")})`, "giu");
  const normalized = new Set(terms.map((term) => term.toLocaleLowerCase()));
  return text.split(matcher).map((part, index) =>
    normalized.has(part.toLocaleLowerCase()) ? (
      <mark className="transcript-match-mark" key={`${part}-${index}`}>
        {part}
      </mark>
    ) : (
      part
    )
  );
}

function noteCats(note: NoteRow): string[] {
  return note.entries
    .map((entry) => (entry.category ?? "").toLowerCase())
    .filter(Boolean);
}

function isScheduleNote(note: NoteRow): boolean {
  return noteCats(note).includes("schedule");
}

function noteTitle(note: NoteRow): string {
  const custom = note.title.trim();
  if (custom) return custom;
  const line = note.raw_text
    .split("\n")
    .map((value) => value.trim())
    .find((value) => value.length > 0);
  if (!line) return "(empty note)";
  return line.replace(/^#+\s*/, "").slice(0, 90);
}

function datedNoteTitle(note: NoteRow): string {
  const sameYear = note.event_date.slice(0, 4) === easternDay().slice(0, 4);
  const date = formatDay(note.event_date, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
  return `${date} · ${noteTitle(note)}`;
}

function folderParentPath(folder: NoteFolderInfo, folders: NoteFolderInfo[]): string {
  return folder.parent_id == null ? "" : folderPath(folder.parent_id, folders);
}

function ordinalDay(day: number): string {
  const lastTwo = day % 100;
  if (lastTwo >= 11 && lastTwo <= 13) return `${day}th`;
  if (day % 10 === 1) return `${day}st`;
  if (day % 10 === 2) return `${day}nd`;
  if (day % 10 === 3) return `${day}rd`;
  return `${day}th`;
}

function scheduleNoteTitle(note: NoteRow): string {
  const custom = note.title.trim();
  if (custom) return custom;
  const [year, , day] = note.event_date.split("-").map(Number);
  const month = formatDay(note.event_date, { month: "long" });
  const yearLabel = String(year) === easternDay().slice(0, 4) ? "" : `, ${year}`;
  return `${month} ${ordinalDay(day)}${yearLabel} — Schedule`;
}

function meetingIdOf(note: NoteRow): number | null {
  for (const entry of note.entries) {
    if ((entry.category ?? "").toLowerCase() === "meetings") {
      const id = entry.data?.["meeting_id"];
      if (typeof id === "number") return id;
    }
  }
  return null;
}

function dateAtNoon(date: string): Date {
  return new Date(`${date}T12:00:00Z`);
}

function ymd(date: Date): string {
  return date.toISOString().slice(0, 10);
}

function isoWeek(date: string): number {
  const value = dateAtNoon(date);
  const day = value.getUTCDay() || 7;
  value.setUTCDate(value.getUTCDate() + 4 - day);
  const yearStart = new Date(Date.UTC(value.getUTCFullYear(), 0, 1, 12));
  return Math.ceil(((value.getTime() - yearStart.getTime()) / 86_400_000 + 1) / 7);
}

function weekFor(date: string): { start: string; end: string } {
  const start = dateAtNoon(date);
  const offset = (start.getUTCDay() + 6) % 7;
  start.setUTCDate(start.getUTCDate() - offset);
  const end = new Date(start);
  end.setUTCDate(end.getUTCDate() + 6);
  return { start: ymd(start), end: ymd(end) };
}

function calendarWeeks(notes: NoteRow[]): WeekGroup[] {
  const groups = new Map<string, WeekGroup>();
  for (const note of notes) {
    const { start, end } = weekFor(note.event_date);
    let group = groups.get(start);
    if (!group) {
      const startLabel = formatDay(start, { month: "short", day: "numeric" });
      const endLabel = formatDay(end, {
        month: start.slice(0, 7) === end.slice(0, 7) ? undefined : "short",
        day: "numeric",
        year: start.slice(0, 4) === end.slice(0, 4) ? undefined : "numeric",
      });
      group = {
        key: start,
        label: `Week ${isoWeek(start)} · ${startLabel} – ${endLabel}`,
        notes: [],
      };
      groups.set(start, group);
    }
    group.notes.push(note);
  }
  return Array.from(groups.values()).sort((a, b) => b.key.localeCompare(a.key));
}

function folderPath(folderId: number, folders: NoteFolderInfo[]): string {
  const byId = new Map(folders.map((folder) => [folder.id, folder]));
  const names: string[] = [];
  let current = byId.get(folderId);
  const seen = new Set<number>();
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    if (current.kind !== "space") names.unshift(current.name);
    current = current.parent_id == null ? undefined : byId.get(current.parent_id);
  }
  return names.join(" / ");
}

export function NotesView({
  notes,
  cats,
  onChanged,
}: {
  notes: NoteRow[];
  cats: CategoryInfo[];
  onChanged?: () => void;
}) {
  const [selection, setSelection] = useState("all");
  const [query, setQuery] = useState("");
  const [openNote, setOpenNote] = useState<NoteRow | null>(null);
  const [openMeeting, setOpenMeeting] = useState<MeetingTarget | null>(null);
  const [meetings, setMeetings] = useState<MeetingListRow[]>([]);
  const [trashedMeetings, setTrashedMeetings] = useState<MeetingListRow[]>([]);
  const [transcriptHits, setTranscriptHits] = useState<TranscriptSearchHit[]>([]);
  const [transcriptSearchPending, setTranscriptSearchPending] = useState(false);
  const [transcriptSearchError, setTranscriptSearchError] = useState<string | null>(null);
  const transcriptSearchRequest = useRef(0);
  const [folders, setFolders] = useState<NoteFolderInfo[]>([]);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [creating, setCreating] = useState<CreateTarget | null>(null);
  const [newFolderName, setNewFolderName] = useState("");
  const [menuFolder, setMenuFolder] = useState<number | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [filing, setFiling] = useState(false);
  const [filingMsg, setFilingMsg] = useState<string | null>(null);
  const [editingNote, setEditingNote] = useState(false);
  const [editTitle, setEditTitle] = useState("");
  const [editBody, setEditBody] = useState("");
  const [savingNote, setSavingNote] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpenState] = useState(
    () => localStorage.getItem("noted-library") !== "closed"
  );
  const [expanded, setExpandedState] = useState<Set<number>>(() => {
    try {
      const saved = localStorage.getItem("noted-folder-expanded");
      return saved ? new Set(JSON.parse(saved) as number[]) : new Set();
    } catch {
      return new Set();
    }
  });

  const setLibraryOpen = (open: boolean) => {
    setLibraryOpenState(open);
    localStorage.setItem("noted-library", open ? "open" : "closed");
  };

  const setExpanded = (next: Set<number>) => {
    setExpandedState(next);
    localStorage.setItem("noted-folder-expanded", JSON.stringify(Array.from(next)));
  };

  const loadMeetings = useCallback(() => {
    Promise.all([api.meetingList(), api.meetingTrashList()])
      .then(([active, trashed]) => {
        setMeetings(active);
        setTrashedMeetings(trashed);
      })
      .catch(() => {});
  }, []);

  const loadFolders = useCallback(async () => {
    try {
      const next = await api.listNoteFolders();
      setFolders(next);
      setFolderError(null);
      if (localStorage.getItem("noted-folder-expanded") == null) {
        setExpanded(new Set(next.map((folder) => folder.id)));
      }
    } catch (error) {
      setFolderError(String(error));
    }
  }, []);

  useEffect(() => {
    loadMeetings();
    loadFolders();
  }, [loadFolders, loadMeetings]);

  useEffect(() => {
    const search = query.trim();
    const searchesTranscripts = selection === "all" || selection === "meetings";
    const requestId = ++transcriptSearchRequest.current;
    if (!searchesTranscripts || search.length < 2) {
      setTranscriptHits([]);
      setTranscriptSearchPending(false);
      setTranscriptSearchError(null);
      return;
    }

    setTranscriptSearchPending(true);
    setTranscriptSearchError(null);
    setTranscriptHits([]);
    const timer = window.setTimeout(() => {
      api
        .meetingSearchTranscripts(search)
        .then((hits) => {
          if (requestId === transcriptSearchRequest.current) setTranscriptHits(hits);
        })
        .catch((error) => {
          if (requestId === transcriptSearchRequest.current) {
            setTranscriptHits([]);
            setTranscriptSearchError(String(error));
          }
        })
        .finally(() => {
          if (requestId === transcriptSearchRequest.current) {
            setTranscriptSearchPending(false);
          }
        });
    }, 40);
    return () => window.clearTimeout(timer);
  }, [query, selection]);

  const successfulMeetings = useMemo(
    () =>
      meetings.filter(
        (meeting) =>
          meeting.status !== "failed" ||
          meeting.segment_count > 0 ||
          meeting.summary_count > 0 ||
          meeting.note_id != null
      ),
    [meetings]
  );

  const folderChildren = useMemo(() => {
    const result = new Map<number | null, NoteFolderInfo[]>();
    for (const folder of folders) {
      const rows = result.get(folder.parent_id) ?? [];
      rows.push(folder);
      result.set(folder.parent_id, rows);
    }
    return result;
  }, [folders]);

  const folderNoteIds = useMemo(() => {
    const result = new Map<number, Set<number>>();
    const collect = (id: number, path: Set<number>): Set<number> => {
      const cached = result.get(id);
      if (cached) return cached;
      if (path.has(id)) return new Set();
      const nextPath = new Set(path).add(id);
      const folder = folders.find((item) => item.id === id);
      const ids = new Set(folder?.note_ids ?? []);
      for (const child of folderChildren.get(id) ?? []) {
        for (const noteId of collect(child.id, nextPath)) ids.add(noteId);
      }
      result.set(id, ids);
      return ids;
    };
    for (const folder of folders) collect(folder.id, new Set());
    return result;
  }, [folderChildren, folders]);

  const rootSpaces = useMemo(
    () =>
      [...(folderChildren.get(null) ?? [])]
        .filter((folder) => folder.kind === "space")
        .sort((a, b) => {
          const rank = (name: string) =>
            name.toLowerCase() === "personal" ? 0 : name.toLowerCase() === "work" ? 1 : 2;
          return rank(a.name) - rank(b.name);
        }),
    [folderChildren]
  );
  const personalSpace = rootSpaces.find((folder) => folder.name.toLowerCase() === "personal");
  const defaultFolderParent = personalSpace ?? rootSpaces[0];
  const topLevelFolders = useMemo(
    () => [
      ...(folderChildren.get(null) ?? []).filter((folder) => folder.kind === "folder"),
      ...rootSpaces.flatMap((space) =>
        (folderChildren.get(space.id) ?? []).filter((folder) => folder.kind === "folder")
      ),
    ],
    [folderChildren, rootSpaces]
  );
  const scheduleNotes = useMemo(() => {
    const seenDays = new Set<string>();
    return notes.filter((note) => {
      if (!isScheduleNote(note) || seenDays.has(note.event_date)) return false;
      seenDays.add(note.event_date);
      return true;
    });
  }, [notes]);
  const libraryNotes = useMemo(
    () => notes.filter((note) => !isScheduleNote(note)),
    [notes]
  );
  const libraryNoteIds = useMemo(
    () => new Set(libraryNotes.map((note) => note.id)),
    [libraryNotes]
  );

  const categories = useMemo(() => {
    const count = (name: string) =>
      libraryNotes.filter((note) => noteCats(note).includes(name)).length;
    return cats
      .map((category) => category.name.toLowerCase())
      .filter((name) => name !== "meetings" && name !== "schedule" && name !== "journal")
      .map((name) => ({
        id: `category:${name}`,
        name,
        label: name.charAt(0).toUpperCase() + name.slice(1),
        count: count(name),
      }))
      .filter((category) => category.count > 0)
      .sort((a, b) => b.count - a.count);
  }, [cats, libraryNotes]);

  const selectedFolderId = selection.startsWith("folder:")
    ? Number(selection.slice("folder:".length))
    : null;
  const selectedFolder =
    selectedFolderId == null
      ? undefined
      : folders.find((folder) => folder.id === selectedFolderId);
  const standupNoteIds = useMemo(
    () =>
      new Set(
        folders
          .filter((folder) => folder.auto_rule === "daily_standup")
          .flatMap((folder) => folder.note_ids)
      ),
    [folders]
  );
  const selectedFolderChildren = useMemo(
    () =>
      selectedFolder == null
        ? []
        : (folderChildren.get(selectedFolder.id) ?? []).filter(
            (folder) => folder.kind === "folder"
          ),
    [folderChildren, selectedFolder]
  );
  const visibleFolderChildren = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) return selectedFolderChildren;
    return selectedFolderChildren.filter((folder) =>
      folder.name.toLowerCase().includes(normalizedQuery)
    );
  }, [query, selectedFolderChildren]);
  const selectedFolderParent = selectedFolder
    ? folderParentPath(selectedFolder, folders)
    : "";

  const list = useMemo(() => {
    let rows: NoteRow[];
    if (selection === "all") rows = libraryNotes;
    else if (selection === "schedule") rows = scheduleNotes;
    else if (selection === "journal") {
      rows = libraryNotes.filter((note) => noteCats(note).includes("journal"));
    } else if (selection.startsWith("category:")) {
      const category = selection.slice("category:".length);
      rows = libraryNotes.filter((note) => noteCats(note).includes(category));
    } else if (selection.startsWith("folder:")) {
      const ids = new Set(selectedFolder?.note_ids ?? []);
      rows = libraryNotes.filter((note) => ids.has(note.id));
    } else rows = libraryNotes;

    const normalizedQuery = query.trim().toLowerCase();
    if (normalizedQuery) {
      rows = rows.filter(
        (note) =>
          note.title.toLowerCase().includes(normalizedQuery) ||
          note.raw_text.toLowerCase().includes(normalizedQuery) ||
          noteCats(note).some((category) => category.includes(normalizedQuery))
      );
    }
    return rows;
  }, [libraryNotes, query, scheduleNotes, selectedFolder, selection]);

  const meetingRows = useMemo(() => {
    const rows = selection === "meeting-trash" ? trashedMeetings : successfulMeetings;
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) return rows;
    return rows.filter((meeting) =>
      `${meeting.title} ${meeting.status}`.toLowerCase().includes(normalizedQuery)
    );
  }, [query, selection, successfulMeetings, trashedMeetings]);

  const weekGroups = useMemo(
    () => (selectedFolder ? calendarWeeks(list) : []),
    [list, selectedFolder]
  );
  const meetingView = selection === "meetings" || selection === "meeting-trash";

  const currentLabel = useMemo(() => {
    if (selection === "all") return "All Notes";
    if (selection === "meetings") return "Meetings";
    if (selection === "schedule") return "Schedule";
    if (selection === "journal") return "Journal";
    if (selection === "meeting-trash") return "Trash";
    if (selectedFolder) return selectedFolder.name;
    return categories.find((category) => category.id === selection)?.label ?? "Notes";
  }, [categories, selectedFolder, selection]);

  async function createFolder() {
    if (!creating || !newFolderName.trim()) return;
    try {
      const id = await api.createNoteFolder(
        creating.parentId,
        newFolderName.trim(),
        "folder"
      );
      setCreating(null);
      setNewFolderName("");
      if (creating.parentId != null) {
        const next = new Set(expanded).add(creating.parentId);
        setExpanded(next);
      }
      await loadFolders();
      setSelection(`folder:${id}`);
    } catch (error) {
      setFolderError(String(error));
    }
  }

  async function renameFolder(folder: NoteFolderInfo) {
    const name = renameValue.trim();
    if (!name) return;
    if (name === folder.name) {
      setRenaming(null);
      setRenameValue("");
      return;
    }
    try {
      await api.renameNoteFolder(folder.id, name);
      setRenaming(null);
      setRenameValue("");
      await loadFolders();
    } catch (error) {
      setFolderError(String(error));
    }
  }

  async function deleteFolder(folder: NoteFolderInfo) {
    setMenuFolder(null);
    const childCount = (folderChildren.get(folder.id) ?? []).length;
    const detail = childCount
      ? " Its nested folders will also be removed. Your notes will not be deleted."
      : " Your notes will not be deleted.";
    if (!window.confirm(`Remove “${folder.name}”?${detail}`)) return;
    try {
      await api.deleteNoteFolder(folder.id);
      if (selection === `folder:${folder.id}`) setSelection("all");
      await loadFolders();
    } catch (error) {
      setFolderError(String(error));
    }
  }

  async function fileOpenNote(folderId: number | null) {
    if (!openNote) return;
    setFiling(true);
    setFilingMsg(null);
    try {
      await api.fileNote(openNote.id, folderId);
      await loadFolders();
      setFilingMsg(folderId == null ? "Manual filing removed" : "Filed");
    } catch (error) {
      setFilingMsg(String(error));
    } finally {
      setFiling(false);
    }
  }

  if (openMeeting != null) {
    return (
      <MeetingPage
        id={openMeeting.id}
        focusSegmentId={openMeeting.segmentId}
        onBack={() => {
          setOpenMeeting(null);
          loadMeetings();
          loadFolders();
          onChanged?.();
        }}
      />
    );
  }

  if (openNote) {
    const scheduleNote = isScheduleNote(openNote);
    const visibleFolders = folders.filter((folder) => folder.kind === "folder");
    const filedIn = visibleFolders.filter((folder) => folder.note_ids.includes(openNote.id));
    return (
      <div className="note-detail">
        <header className="note-detail-head">
          <button
            className="icon-btn"
            onClick={() => {
              setEditingNote(false);
              setOpenNote(null);
            }}
            aria-label="Back"
          >
            <ArrowLeft size={18} />
          </button>
          <div className="note-detail-heading">
            {editingNote ? (
              <input
                className="note-title-editor"
                value={editTitle}
                onChange={(event) => setEditTitle(event.target.value)}
                aria-label="Note title"
                autoFocus
              />
            ) : (
              <h2>{scheduleNote ? scheduleNoteTitle(openNote) : noteTitle(openNote)}</h2>
            )}
            <div className="note-detail-meta">
              <span>{relativeDay(openNote.event_date)}</span>
              <span>{openNote.source}</span>
              {noteCats(openNote).map((category) => (
                <span key={category}>{category}</span>
              ))}
            </div>
            {!scheduleNote && filedIn.length > 0 && (
              <div className="note-filed-paths">
                {filedIn.map((folder) => folderPath(folder.id, folders)).join(" · ")}
              </div>
            )}
          </div>
          <div className="note-detail-controls">
            {!editingNote && (
              <button
                className="note-edit-trigger"
                onClick={() => {
                  setEditTitle(
                    openNote.title.trim() ||
                      (scheduleNote ? scheduleNoteTitle(openNote) : noteTitle(openNote))
                  );
                  setEditBody(openNote.raw_text);
                  setEditError(null);
                  setEditingNote(true);
                }}
              >
                <PenLine size={14} /> Edit
              </button>
            )}
            {!scheduleNote && !editingNote && (
              <label className="note-file-select">
                <span className="sr-only">File note in a folder</span>
                <select
                  value=""
                  disabled={filing}
                  onChange={(event) => {
                    const value = event.target.value;
                    if (value === "remove") fileOpenNote(null);
                    else if (value) fileOpenNote(Number(value));
                  }}
                >
                  <option value="">File in…</option>
                  {visibleFolders.map((folder) => (
                    <option key={folder.id} value={folder.id}>
                      {folderPath(folder.id, folders)}
                    </option>
                  ))}
                  <option value="remove">Remove manual filing</option>
                </select>
              </label>
            )}
          </div>
        </header>
        {filingMsg && <div className="note-filing-message">{filingMsg}</div>}
        {editingNote ? (
          <div className="note-edit-pane">
            <textarea
              className="note-body-editor"
              value={editBody}
              onChange={(event) => setEditBody(event.target.value)}
              aria-label="Note content"
              placeholder="Write your note…"
              spellCheck
            />
            {editError && <div className="note-edit-error">{editError}</div>}
            <div className="note-edit-actions">
              <button
                className="note-edit-cancel"
                onClick={() => {
                  setEditingNote(false);
                  setEditError(null);
                }}
                disabled={savingNote}
              >
                Cancel
              </button>
              <button
                className="note-edit-save"
                disabled={savingNote}
                onClick={async () => {
                  setSavingNote(true);
                  setEditError(null);
                  try {
                    await api.updateNote(openNote.id, editTitle, editBody);
                    setOpenNote({
                      ...openNote,
                      title: editTitle.trim(),
                      raw_text: editBody,
                    });
                    setEditingNote(false);
                    onChanged?.();
                  } catch (error) {
                    setEditError(String(error));
                  } finally {
                    setSavingNote(false);
                  }
                }}
              >
                {savingNote ? "Saving…" : "Save"}
              </button>
            </div>
          </div>
        ) : (
          <div className="note-detail-body">{openNote.raw_text}</div>
        )}
        {openNote.entries.some(
          (entry) => entry.data && Object.keys(entry.data).length > 0
        ) && (
          <div className="note-detail-entries">
            {openNote.entries.map(
              (entry, index) =>
                entry.data &&
                Object.keys(entry.data).length > 0 && (
                  <div key={entry.id ?? index} className="note-entry-card">
                    <span className="note-entry-label">{entry.category ?? "Entry"}</span>
                    <DataView value={entry.data} />
                  </div>
                )
            )}
          </div>
        )}
      </div>
    );
  }

  const open = (note: NoteRow) => {
    const meetingId = meetingIdOf(note);
    if (meetingId != null) setOpenMeeting({ id: meetingId });
    else setOpenNote(note);
  };

  const renderNoteRow = (note: NoteRow) => (
    <button key={note.id} className="note-row" onClick={() => open(note)}>
      {isScheduleNote(note) ? (
        <CalendarDays size={14} className="note-row-icon" />
      ) : meetingIdOf(note) != null ? (
        <AudioLines size={14} className="note-row-icon" />
      ) : (
        <FileText size={14} className="note-row-icon" />
      )}
      <span className="note-row-title">
        {isScheduleNote(note)
          ? scheduleNoteTitle(note)
          : standupNoteIds.has(note.id)
          ? datedNoteTitle(note)
          : noteTitle(note)}
      </span>
      {!isScheduleNote(note) && (
        <span className="note-row-categories">{noteCats(note).slice(0, 2).join(" · ")}</span>
      )}
      <span className="note-row-date">{relativeDay(note.event_date)}</span>
    </button>
  );

  const transcriptSearchActive =
    (selection === "all" || selection === "meetings") && query.trim().length >= 2;

  const renderTranscriptResults = () => {
    if (!transcriptSearchActive) return null;
    return (
      <section className="transcript-results" aria-label="Transcript search results">
        <header className="transcript-results-head" aria-live="polite">
          <h2>Transcript matches</h2>
          <span>
            {transcriptSearchPending
              ? "Searching…"
              : transcriptSearchError
                ? "Unavailable"
                : `${transcriptHits.length === 200 ? "200+" : transcriptHits.length} ${
                    transcriptHits.length === 1 ? "line" : "lines"
                  }`}
          </span>
        </header>
        {transcriptSearchError ? (
          <p className="transcript-results-error">Transcript search is temporarily unavailable.</p>
        ) : (
          <div className="transcript-result-list">
            {transcriptHits.map((hit) => (
              <button
                key={hit.segment_id}
                className="transcript-result"
                onClick={() =>
                  setOpenMeeting({ id: hit.meeting_id, segmentId: hit.segment_id })
                }
              >
                <span className="transcript-result-speaker">
                  <strong>{hit.speaker}</strong>
                  <span>{transcriptTimestamp(hit.t0_ms)}</span>
                </span>
                <span className="transcript-result-text">
                  {highlightedTranscript(transcriptSnippet(hit.text, query), query)}
                </span>
                <span className="transcript-result-meeting">
                  <strong>{hit.meeting_title}</strong>
                  <time>{transcriptMeetingDate(hit.started_at)}</time>
                </span>
              </button>
            ))}
          </div>
        )}
      </section>
    );
  };

  const renderMainFolderRow = (folder: NoteFolderInfo) => {
    const noteCount = Array.from(folderNoteIds.get(folder.id) ?? []).filter((id) =>
      libraryNoteIds.has(id)
    ).length;
    return (
      <button
        key={folder.id}
        className="note-row folder-content-row"
        onClick={() => setSelection(`folder:${folder.id}`)}
      >
        <Folder size={14} className="note-row-icon" />
        <span className="note-row-title">{folder.name}</span>
        <span className="note-row-categories">
          {noteCount} {noteCount === 1 ? "note" : "notes"}
        </span>
        <ChevronRight size={14} className="folder-row-arrow" />
      </button>
    );
  };

  const renderFolder = (folder: NoteFolderInfo, depth: number): ReactNode => {
    const children = folderChildren.get(folder.id) ?? [];
    const isExpanded = expanded.has(folder.id);
    const isSelected = selection === `folder:${folder.id}`;
    const count = Array.from(folderNoteIds.get(folder.id) ?? []).filter((id) =>
      libraryNoteIds.has(id)
    ).length;
    return (
      <div className="folder-tree-item" key={folder.id}>
        <div
          className={`folder-tree-row${isSelected ? " on" : ""}`}
          style={{ "--folder-depth": depth } as CSSProperties}
        >
          {children.length > 0 ? (
            <button
              className="folder-caret"
              onClick={() => {
                const next = new Set(expanded);
                if (isExpanded) {
                  next.delete(folder.id);
                  if (selectedFolderId != null && selectedFolderId !== folder.id) {
                    let current = folders.find((item) => item.id === selectedFolderId);
                    while (current?.parent_id != null) {
                      if (current.parent_id === folder.id) {
                        setSelection(`folder:${folder.id}`);
                        break;
                      }
                      current = folders.find((item) => item.id === current?.parent_id);
                    }
                  }
                } else next.add(folder.id);
                setExpanded(next);
              }}
              aria-label={isExpanded ? `Collapse ${folder.name}` : `Expand ${folder.name}`}
            >
              {isExpanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
          ) : (
            <span className="folder-caret-space" />
          )}
          {renaming === folder.id ? (
            <form
              className="folder-rename"
              onSubmit={(event) => {
                event.preventDefault();
                renameFolder(folder);
              }}
            >
              <input
                autoFocus
                value={renameValue}
                onChange={(event) => setRenameValue(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    setRenaming(null);
                    setRenameValue("");
                  }
                }}
                aria-label={`Rename ${folder.name}`}
              />
            </form>
          ) : (
            <>
              <button
                className="folder-main"
                title={folder.name}
                onClick={() => {
                  setSelection(`folder:${folder.id}`);
                  setMenuFolder(null);
                }}
              >
                {isSelected || (isExpanded && children.length > 0) ? (
                  <FolderOpen size={14} />
                ) : (
                  <Folder size={14} />
                )}
                <span className="space-label">{folder.name}</span>
                <span className="space-n">{count}</span>
              </button>
              <button
                className="folder-more"
                onClick={() => setMenuFolder(menuFolder === folder.id ? null : folder.id)}
                aria-label={`Manage ${folder.name}`}
              >
                <MoreHorizontal size={14} />
              </button>
            </>
          )}
        </div>
        {menuFolder === folder.id && (
          <div
            className="folder-menu"
            style={{ "--folder-depth": depth } as CSSProperties}
          >
            <button
              onClick={() => {
                setMenuFolder(null);
                setRenaming(folder.id);
                setRenameValue(folder.name);
              }}
            >
              Rename
            </button>
            <button className="danger" onClick={() => deleteFolder(folder)}>
              Remove
            </button>
          </div>
        )}
        {isExpanded && children.map((child) => renderFolder(child, depth + 1))}
      </div>
    );
  };

  const listedCount = meetingView
    ? meetingRows.length
    : selectedFolder
      ? visibleFolderChildren.length + list.length
      : list.length;
  const visibleCount = listedCount + (transcriptSearchActive ? transcriptHits.length : 0);
  const countLabel = transcriptSearchActive
    ? `${visibleCount}${transcriptHits.length === 200 ? "+" : ""} ${
        visibleCount === 1 ? "result" : "results"
      }`
    : selectedFolder
    ? [
        visibleFolderChildren.length > 0
          ? `${visibleFolderChildren.length} ${visibleFolderChildren.length === 1 ? "folder" : "folders"}`
          : "",
        list.length > 0 ? `${list.length} ${list.length === 1 ? "note" : "notes"}` : "",
      ]
        .filter(Boolean)
        .join(" · ") || "0 items"
    : `${visibleCount} ${visibleCount === 1 ? "note" : "notes"}`;
  const emptyMessage = transcriptSearchPending
    ? "Searching transcripts…"
    : query
    ? "Nothing matches."
    : selection === "meetings"
      ? "No meetings recorded yet."
      : selection === "schedule"
        ? "No schedules saved yet."
      : selection === "meeting-trash"
        ? "Trash is empty."
        : selectedFolder?.auto_rule === "daily_standup"
          ? "No stand-up notes yet. New ones will be filed here automatically."
          : selectedFolder
            ? "This folder is empty. Open a note to file it here."
            : "No notes yet.";

  return (
    <div className="notes-view" data-tauri-drag-region>
      {libraryOpen && (
        <aside className="spaces" aria-label="Notes library">
          <div className="spaces-head">
            <span>Library</span>
            <button
              className="icon-btn"
              onClick={() => setLibraryOpen(false)}
              title="Collapse library"
              aria-label="Collapse library"
            >
              <PanelLeftClose size={14} />
            </button>
          </div>
          <nav className="library-main" aria-label="Saved views">
            <button
              className={selection === "all" ? "on" : ""}
              onClick={() => setSelection("all")}
            >
              <Inbox size={14} />
              <span className="space-label">All Notes</span>
              <span className="space-n">{libraryNotes.length}</span>
            </button>
            <button
              className={selection === "meetings" ? "on" : ""}
              onClick={() => setSelection("meetings")}
            >
              <AudioLines size={14} />
              <span className="space-label">Meetings</span>
              <span className="space-n">{successfulMeetings.length}</span>
            </button>
            <button
              className={selection === "schedule" ? "on" : ""}
              onClick={() => setSelection("schedule")}
            >
              <CalendarDays size={14} />
              <span className="space-label">Schedule</span>
              <span className="space-n">{scheduleNotes.length}</span>
            </button>
            <button
              className={selection === "journal" ? "on" : ""}
              onClick={() => setSelection("journal")}
            >
              <BookOpen size={14} />
              <span className="space-label">Journal</span>
              <span className="space-n">
                {libraryNotes.filter((note) => noteCats(note).includes("journal")).length}
              </span>
            </button>
          </nav>

          <div className="library-section-head">
            <span>Folders</span>
            <button
              className="library-add"
              disabled={!defaultFolderParent}
              onClick={() => {
                if (!defaultFolderParent) return;
                setCreating({
                  parentId: defaultFolderParent.id,
                  label: "Folder name",
                });
                setNewFolderName("");
              }}
              aria-label="New folder"
              title="New folder"
            >
              <Plus size={14} />
            </button>
          </div>
          <div className="folder-tree">
            {topLevelFolders.map((folder) => renderFolder(folder, 0))}
          </div>
          {selectedFolder && (
            <button
              className="new-subfolder"
              aria-label={`New folder in ${selectedFolder.name}`}
              onClick={() => {
                setCreating({
                  parentId: selectedFolder.id,
                  label: `Folder in ${selectedFolder.name}`,
                });
                setNewFolderName("");
              }}
            >
              <Plus size={13} /> New subfolder
            </button>
          )}
          {creating && (
            <form
              className="folder-create nested"
              onSubmit={(event) => {
                event.preventDefault();
                createFolder();
              }}
            >
              <input
                autoFocus
                value={newFolderName}
                onChange={(event) => setNewFolderName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") setCreating(null);
                }}
                placeholder={creating.label}
                aria-label={creating.label}
              />
            </form>
          )}

          {categories.length > 0 && (
            <>
              <div className="library-section-head categories-head">Categories</div>
              <nav className="library-categories" aria-label="Categories">
                {categories.map((category) => (
                  <button
                    key={category.id}
                    className={selection === category.id ? "on" : ""}
                    onClick={() => setSelection(category.id)}
                  >
                    <FileText size={13} />
                    <span className="space-label">{category.label}</span>
                    <span className="space-n">{category.count}</span>
                  </button>
                ))}
              </nav>
            </>
          )}

          {folderError && <div className="folder-error">{folderError}</div>}
          <div className="spaces-trash-wrap">
            <button
              className={`spaces-trash${selection === "meeting-trash" ? " on" : ""}`}
              onClick={() => setSelection("meeting-trash")}
              title="Open trash"
            >
              <Trash2 size={14} />
              <span className="space-label">Trash</span>
              <span className="space-n">{trashedMeetings.length}</span>
            </button>
          </div>
        </aside>
      )}

      <main className="notes-list">
        <div className="notes-context">
          <div>
            {selectedFolderParent && (
              <div className="notes-breadcrumb">{selectedFolderParent}</div>
            )}
            <h1>{currentLabel}</h1>
            {selectedFolder?.auto_rule === "daily_standup" && (
              <p>Stand-up notes are filed here automatically and grouped by calendar week.</p>
            )}
          </div>
          <span className="notes-context-count">{countLabel}</span>
        </div>
        <div className="notes-list-head">
          {!libraryOpen && (
            <button
              className="icon-btn"
              onClick={() => setLibraryOpen(true)}
              title={`Show library (viewing: ${currentLabel})`}
              aria-label="Show library"
            >
              <PanelLeftOpen size={15} />
            </button>
          )}
          <label className="notes-search">
            <Search size={14} />
            <input
              placeholder={
                selection === "all"
                  ? "Search notes and transcripts…"
                  : selection === "meetings"
                    ? "Search meeting names and transcripts…"
                    : `Search ${currentLabel}…`
              }
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
        </div>

        {renderTranscriptResults()}

        {visibleCount === 0 ? (
          <p className="quiet-empty">{emptyMessage}</p>
        ) : meetingView ? (
          meetingRows.map((meeting) => (
            <button
              key={meeting.id}
              className="note-row"
              onClick={() => setOpenMeeting({ id: meeting.id })}
            >
              <AudioLines size={14} className="note-row-icon" />
              <span className="note-row-title">
                {meeting.note_id != null && standupNoteIds.has(meeting.note_id)
                  ? datedNoteTitle(
                      libraryNotes.find((note) => note.id === meeting.note_id) ?? {
                        id: meeting.note_id,
                        title: meeting.title,
                        raw_text: meeting.title,
                        source: "meeting",
                        event_date: meeting.started_at
                          ? easternDay(new Date(meeting.started_at))
                          : easternDay(),
                        created_at: meeting.started_at ?? "",
                        entries: [],
                      }
                    )
                  : meeting.title}
              </span>
              <span className="note-row-categories">
                {selection === "meeting-trash"
                  ? "in trash"
                  : meeting.status === "recording"
                    ? "recording"
                    : meeting.status === "summarizing"
                      ? "enhancing notes"
                      : meeting.summary_count > 0
                        ? "meeting notes"
                        : meeting.segment_count > 0
                          ? "transcript"
                          : "meeting"}
              </span>
              <span className="note-row-date">
                {meeting.started_at
                  ? relativeDay(easternDay(new Date(meeting.started_at)))
                  : ""}
              </span>
            </button>
          ))
        ) : selectedFolder ? (
          <>
            {visibleFolderChildren.length > 0 && (
              <section className="note-week folder-index">
                <header className="note-week-head">
                  <h2>Folders</h2>
                  <span>{visibleFolderChildren.length}</span>
                </header>
                <div className="note-week-list">
                  {visibleFolderChildren.map(renderMainFolderRow)}
                </div>
              </section>
            )}
            {weekGroups.map((group) => (
              <section className="note-week" key={group.key}>
                <header className="note-week-head">
                  <h2>{group.label}</h2>
                  <span>{group.notes.length}</span>
                </header>
                <div className="note-week-list">{group.notes.map(renderNoteRow)}</div>
              </section>
            ))}
          </>
        ) : (
          list.map(renderNoteRow)
        )}
      </main>
    </div>
  );
}
