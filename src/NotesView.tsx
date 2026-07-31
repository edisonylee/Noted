// The Notes workspace separates model-generated categories from user-owned
// organization. Spaces and folders form a persistent tree; smart folders can
// gather notes automatically while the note's category remains untouched.

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import {
  ArrowLeft,
  AudioLines,
  BookOpen,
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Inbox,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
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
} from "./api";
import { DataView } from "./DataView";
import { MeetingPage } from "./MeetingPage";
import { easternDay, formatDay, relativeDay } from "./day";

type CreateTarget = {
  parentId: number | null;
  kind: "space" | "folder";
  label: string;
};

type WeekGroup = {
  key: string;
  label: string;
  notes: NoteRow[];
};

function noteCats(note: NoteRow): string[] {
  return note.entries
    .map((entry) => (entry.category ?? "").toLowerCase())
    .filter(Boolean);
}

function isWorkNote(note: NoteRow): boolean {
  const categories = noteCats(note);
  if (categories.includes("meetings")) return true;
  const value = `${note.raw_text} ${categories.join(" ")}`.toLowerCase();
  const spaced = value.replace(/[-_]/g, " ");
  return (
    value.includes("standup") ||
    value.includes("stand-up") ||
    spaced.includes("daily stand up") ||
    spaced.includes("stand up meeting") ||
    spaced.includes("daily scrum")
  );
}

function noteTitle(note: NoteRow): string {
  const line = note.raw_text
    .split("\n")
    .map((value) => value.trim())
    .find((value) => value.length > 0);
  if (!line) return "(empty note)";
  return line.replace(/^#+\s*/, "").slice(0, 90);
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
    names.unshift(current.name);
    current = current.parent_id == null ? undefined : byId.get(current.parent_id);
  }
  return names.join(" / ");
}

export function NotesView({ notes, cats }: { notes: NoteRow[]; cats: CategoryInfo[] }) {
  const [selection, setSelection] = useState("all");
  const [activeSpaceId, setActiveSpaceId] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  const [openNote, setOpenNote] = useState<NoteRow | null>(null);
  const [openMeeting, setOpenMeeting] = useState<number | null>(null);
  const [meetings, setMeetings] = useState<MeetingListRow[]>([]);
  const [trashedMeetings, setTrashedMeetings] = useState<MeetingListRow[]>([]);
  const [folders, setFolders] = useState<NoteFolderInfo[]>([]);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [creating, setCreating] = useState<CreateTarget | null>(null);
  const [newFolderName, setNewFolderName] = useState("");
  const [menuFolder, setMenuFolder] = useState<number | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [filing, setFiling] = useState(false);
  const [filingMsg, setFilingMsg] = useState<string | null>(null);
  const [spacesOpen, setSpacesOpenState] = useState(
    () => localStorage.getItem("noted-spaces") !== "closed"
  );
  const [expanded, setExpandedState] = useState<Set<number>>(() => {
    try {
      const saved = localStorage.getItem("noted-folder-expanded");
      return saved ? new Set(JSON.parse(saved) as number[]) : new Set();
    } catch {
      return new Set();
    }
  });

  const setSpacesOpen = (open: boolean) => {
    setSpacesOpenState(open);
    localStorage.setItem("noted-spaces", open ? "open" : "closed");
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
      const roots = next.filter((folder) => folder.kind === "space");
      setActiveSpaceId((current) => {
        if (current != null && roots.some((folder) => folder.id === current)) return current;
        const saved = localStorage.getItem("noted-active-space")?.toLowerCase();
        return (
          roots.find((folder) => folder.name.toLowerCase() === saved)?.id ??
          roots.find((folder) => folder.name.toLowerCase() === "personal")?.id ??
          roots[0]?.id ??
          null
        );
      });
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
  const activeSpace = rootSpaces.find((folder) => folder.id === activeSpaceId);
  const workSpace = rootSpaces.find((folder) => folder.name.toLowerCase() === "work");
  const personalSpace = rootSpaces.find((folder) => folder.name.toLowerCase() === "personal");

  const spaceNotes = useMemo(() => {
    const result = new Map<number, NoteRow[]>();
    const workIds = new Set(workSpace ? folderNoteIds.get(workSpace.id) ?? [] : []);
    for (const note of notes) {
      if (isWorkNote(note)) workIds.add(note.id);
    }
    const outsidePersonal = new Set<number>(workIds);
    for (const space of rootSpaces) {
      if (space.id === personalSpace?.id) continue;
      for (const id of folderNoteIds.get(space.id) ?? []) outsidePersonal.add(id);
    }
    for (const space of rootSpaces) {
      const direct = new Set(folderNoteIds.get(space.id) ?? []);
      const rows =
        space.id === workSpace?.id
          ? notes.filter((note) => workIds.has(note.id))
          : space.id === personalSpace?.id
            ? notes.filter((note) => direct.has(note.id) || !outsidePersonal.has(note.id))
            : notes.filter((note) => direct.has(note.id));
      result.set(space.id, rows);
    }
    return result;
  }, [folderNoteIds, notes, personalSpace, rootSpaces, workSpace]);

  const activeSpaceNotes =
    activeSpace == null ? notes : (spaceNotes.get(activeSpace.id) ?? []);

  const categories = useMemo(() => {
    const count = (name: string) =>
      activeSpaceNotes.filter((note) => noteCats(note).includes(name)).length;
    return cats
      .map((category) => category.name.toLowerCase())
      .filter((name) => name !== "meetings" && name !== "journal")
      .map((name) => ({
        id: `category:${name}`,
        name,
        label: name.charAt(0).toUpperCase() + name.slice(1),
        count: count(name),
      }))
      .filter((category) => category.count > 0)
      .sort((a, b) => b.count - a.count);
  }, [activeSpaceNotes, cats]);

  const selectedFolderId = selection.startsWith("folder:")
    ? Number(selection.slice("folder:".length))
    : null;
  const selectedFolder =
    selectedFolderId == null
      ? undefined
      : folders.find((folder) => folder.id === selectedFolderId);

  const list = useMemo(() => {
    let rows: NoteRow[];
    if (selection === "all") rows = activeSpaceNotes;
    else if (selection === "journal") {
      rows = activeSpaceNotes.filter((note) => noteCats(note).includes("journal"));
    } else if (selection.startsWith("category:")) {
      const category = selection.slice("category:".length);
      rows = activeSpaceNotes.filter((note) => noteCats(note).includes(category));
    } else if (selection.startsWith("folder:")) {
      const folderId = Number(selection.slice("folder:".length));
      const ids = folderNoteIds.get(folderId) ?? new Set<number>();
      rows = activeSpaceNotes.filter((note) => ids.has(note.id));
    } else rows = activeSpaceNotes;

    const normalizedQuery = query.trim().toLowerCase();
    if (normalizedQuery) {
      rows = rows.filter(
        (note) =>
          note.raw_text.toLowerCase().includes(normalizedQuery) ||
          noteCats(note).some((category) => category.includes(normalizedQuery))
      );
    }
    return rows;
  }, [activeSpaceNotes, folderNoteIds, query, selection]);

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
  const meetingSpace = selection === "meetings" || selection === "meeting-trash";

  const currentLabel = useMemo(() => {
    if (selection === "all") return activeSpace ? `${activeSpace.name} notes` : "All notes";
    if (selection === "meetings") return "Meetings";
    if (selection === "journal") return "Journal";
    if (selection === "meeting-trash") return "Trash";
    if (selectedFolder) return selectedFolder.name;
    return categories.find((category) => category.id === selection)?.label ?? "Notes";
  }, [activeSpace, categories, selectedFolder, selection]);

  function chooseSpace(space: NoteFolderInfo) {
    setActiveSpaceId(space.id);
    localStorage.setItem("noted-active-space", space.name);
    setSelection("all");
    setQuery("");
    setCreating(null);
    setMenuFolder(null);
  }

  async function createFolder() {
    if (!creating || !newFolderName.trim()) return;
    try {
      const id = await api.createNoteFolder(
        creating.parentId,
        newFolderName.trim(),
        creating.kind
      );
      setCreating(null);
      setNewFolderName("");
      if (creating.parentId != null) {
        const next = new Set(expanded).add(creating.parentId);
        setExpanded(next);
      }
      await loadFolders();
      if (creating.kind === "space") {
        setActiveSpaceId(id);
        localStorage.setItem("noted-active-space", newFolderName.trim());
        setSelection("all");
      } else {
        setSelection(`folder:${id}`);
      }
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
        id={openMeeting}
        onBack={() => {
          setOpenMeeting(null);
          loadMeetings();
          loadFolders();
        }}
      />
    );
  }

  if (openNote) {
    const filedIn = folders.filter((folder) => folder.note_ids.includes(openNote.id));
    return (
      <div className="note-detail">
        <header className="note-detail-head">
          <button className="icon-btn" onClick={() => setOpenNote(null)} aria-label="Back">
            <ArrowLeft size={18} />
          </button>
          <div className="note-detail-heading">
            <h2>{noteTitle(openNote)}</h2>
            <div className="note-detail-meta">
              <span>{relativeDay(openNote.event_date)}</span>
              <span>{openNote.source}</span>
              {noteCats(openNote).map((category) => (
                <span key={category}>{category}</span>
              ))}
            </div>
            {filedIn.length > 0 && (
              <div className="note-filed-paths">
                {filedIn.map((folder) => folderPath(folder.id, folders)).join(" · ")}
              </div>
            )}
          </div>
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
              {folders.map((folder) => (
                <option key={folder.id} value={folder.id}>
                  {folderPath(folder.id, folders)}
                </option>
              ))}
              <option value="remove">Remove manual filing</option>
            </select>
          </label>
        </header>
        {filingMsg && <div className="note-filing-message">{filingMsg}</div>}
        <div className="note-detail-body">{openNote.raw_text}</div>
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
    if (meetingId != null) setOpenMeeting(meetingId);
    else setOpenNote(note);
  };

  const renderNoteRow = (note: NoteRow) => (
    <button key={note.id} className="note-row" onClick={() => open(note)}>
      {meetingIdOf(note) != null ? (
        <AudioLines size={14} className="note-row-icon" />
      ) : (
        <FileText size={14} className="note-row-icon" />
      )}
      <span className="note-row-title">{noteTitle(note)}</span>
      <span className="note-row-categories">{noteCats(note).slice(0, 2).join(" · ")}</span>
      <span className="note-row-date">{relativeDay(note.event_date)}</span>
    </button>
  );

  const renderFolder = (folder: NoteFolderInfo, depth: number): ReactNode => {
    const children = folderChildren.get(folder.id) ?? [];
    const isExpanded = expanded.has(folder.id);
    const isSelected = selection === `folder:${folder.id}`;
    const count = folderNoteIds.get(folder.id)?.size ?? 0;
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

  const visibleCount = meetingSpace ? meetingRows.length : list.length;
  const emptyMessage = query
    ? "Nothing matches."
    : selection === "meetings"
      ? "No meetings recorded yet."
      : selection === "meeting-trash"
        ? "Trash is empty."
        : selectedFolder?.auto_rule === "daily_standup"
          ? "No stand-up notes yet. New ones will be filed here automatically."
          : selectedFolder
            ? "This folder is empty. Open a note to file it here."
            : `No ${activeSpace?.name.toLowerCase() ?? ""} notes yet.`;

  return (
    <div className="notes-view" data-tauri-drag-region>
      {spacesOpen && (
        <aside className="spaces" aria-label="Notes library">
          <div className="spaces-head">
            <span>Library</span>
            <button
              className="icon-btn"
              onClick={() => setSpacesOpen(false)}
              title="Collapse library"
              aria-label="Collapse library"
            >
              <PanelLeftClose size={14} />
            </button>
          </div>
          <div className="library-section-head library-spaces-head">
            <span>Spaces</span>
            <button
              className="library-add"
              onClick={() => {
                setCreating({ parentId: null, kind: "space", label: "New space" });
                setNewFolderName("");
              }}
              aria-label="New space"
              title="New space"
            >
              <Plus size={14} />
            </button>
          </div>
          <nav className="library-spaces" aria-label="Note spaces">
            {rootSpaces.map((space) => (
              <button
                key={space.id}
                className={activeSpaceId === space.id ? "on" : ""}
                onClick={() => chooseSpace(space)}
                aria-pressed={activeSpaceId === space.id}
              >
                <span className="space-label">{space.name}</span>
                <span className="space-n">{spaceNotes.get(space.id)?.length ?? 0}</span>
              </button>
            ))}
          </nav>
          {creating && creating.parentId == null && (
            <form
              className="folder-create"
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
          <nav className="library-main" aria-label="Saved views">
            <button
              className={selection === "all" ? "on" : ""}
              onClick={() => setSelection("all")}
            >
              <Inbox size={14} />
              <span className="space-label">All {activeSpace?.name.toLowerCase() ?? ""} notes</span>
              <span className="space-n">{activeSpaceNotes.length}</span>
            </button>
            {activeSpace?.id === workSpace?.id && (
              <button
                className={selection === "meetings" ? "on" : ""}
                onClick={() => setSelection("meetings")}
              >
                <AudioLines size={14} />
                <span className="space-label">Meetings</span>
                <span className="space-n">{successfulMeetings.length}</span>
              </button>
            )}
            {activeSpace?.id === personalSpace?.id && (
              <button
                className={selection === "journal" ? "on" : ""}
                onClick={() => setSelection("journal")}
              >
                <BookOpen size={14} />
                <span className="space-label">Journal</span>
                <span className="space-n">
                  {activeSpaceNotes.filter((note) => noteCats(note).includes("journal")).length}
                </span>
              </button>
            )}
          </nav>

          <div className="library-section-head">
            <span>Folders</span>
            <button
              className="library-add"
              onClick={() => {
                if (!activeSpace) return;
                setCreating({
                  parentId: activeSpace.id,
                  kind: "folder",
                  label: `Folder in ${activeSpace.name}`,
                });
                setNewFolderName("");
              }}
              aria-label={`New folder in ${activeSpace?.name ?? "space"}`}
              title="New folder"
            >
              <Plus size={14} />
            </button>
          </div>
          <div className="folder-tree">
            {(activeSpace ? folderChildren.get(activeSpace.id) ?? [] : []).map((folder) =>
              renderFolder(folder, 0)
            )}
          </div>
          {selectedFolder && (
            <button
              className="new-subfolder"
              aria-label={`New folder in ${selectedFolder.name}`}
              onClick={() => {
                setCreating({
                  parentId: selectedFolder.id,
                  kind: "folder",
                  label: `Folder in ${selectedFolder.name}`,
                });
                setNewFolderName("");
              }}
            >
              <Plus size={13} /> New subfolder
            </button>
          )}
          {creating && creating.parentId != null && (
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
          {activeSpace?.id === workSpace?.id && (
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
          )}
        </aside>
      )}

      <main className="notes-list">
        <div className="notes-context">
          <div>
            {selectedFolder && (
              <div className="notes-breadcrumb">{folderPath(selectedFolder.id, folders)}</div>
            )}
            <h1>{currentLabel}</h1>
            {selectedFolder?.auto_rule === "daily_standup" && (
              <p>Stand-up notes are filed here automatically and grouped by calendar week.</p>
            )}
          </div>
          <span className="notes-context-count">
            {visibleCount} {visibleCount === 1 ? "note" : "notes"}
          </span>
        </div>
        <div className="notes-list-head">
          {!spacesOpen && (
            <button
              className="icon-btn"
              onClick={() => setSpacesOpen(true)}
              title={`Show library (viewing: ${currentLabel})`}
              aria-label="Show library"
            >
              <PanelLeftOpen size={15} />
            </button>
          )}
          <label className="notes-search">
            <Search size={14} />
            <input
              placeholder={selection === "all" ? "Search notes…" : `Search ${currentLabel}…`}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
        </div>

        {visibleCount === 0 ? (
          <p className="quiet-empty">{emptyMessage}</p>
        ) : meetingSpace ? (
          meetingRows.map((meeting) => (
            <button
              key={meeting.id}
              className="note-row"
              onClick={() => setOpenMeeting(meeting.id)}
            >
              <AudioLines size={14} className="note-row-icon" />
              <span className="note-row-title">{meeting.title}</span>
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
          weekGroups.map((group) => (
            <section className="note-week" key={group.key}>
              <header className="note-week-head">
                <h2>{group.label}</h2>
                <span>{group.notes.length}</span>
              </header>
              <div className="note-week-list">{group.notes.map(renderNoteRow)}</div>
            </section>
          ))
        ) : (
          list.map(renderNoteRow)
        )}
      </main>
    </div>
  );
}
