// The Notes workspace separates model-generated topics from user-owned
// organization. Root spaces scope the whole library; folders remain the
// visible hierarchy inside the selected space.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  ArrowLeft,
  ArrowUpDown,
  AudioLines,
  BookOpen,
  CalendarDays,
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Inbox,
  ListFilter,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  PenLine,
  Plus,
  Search,
  Trash2,
  X,
} from "lucide-react";
import {
  api,
  type CategoryInfo,
  type MeetingListRow,
  type NoteSortOrder,
  type NoteFolderInfo,
  type NoteRow,
  type TranscriptSearchFacets,
  type TranscriptSearchFilters,
  type TranscriptSearchHit,
} from "./api";
import { DataView } from "./DataView";
import { MeetingPage } from "./MeetingPage";
import { easternDay, formatDay, relativeDay } from "./day";

type CreateTarget = {
  parentId: number;
  label: string;
};

type MeetingTarget = {
  id: number;
  segmentId?: number;
};

type LibraryDragItem =
  | { kind: "note"; noteId: number; label: string }
  | { kind: "folder"; folderId: number; label: string };

type FolderDropPlacement = "inside" | "before" | "after";

type FolderDropTarget = {
  folder: NoteFolderInfo;
  placement: FolderDropPlacement;
};

type FolderMoveNotice = {
  kind: "success" | "error";
  message: string;
};

type ActiveFolderPointer = {
  item: LibraryDragItem;
  pointerId: number;
  startX: number;
  startY: number;
  moved: boolean;
};

type SearchInstrument = "filters" | null;

const SORT_ORDERS: NoteSortOrder[] = [
  "date_desc",
  "date_asc",
  "title_asc",
  "title_desc",
];

const SORT_LABELS: Record<NoteSortOrder, string> = {
  date_desc: "Newest first",
  date_asc: "Oldest first",
  title_asc: "Title A–Z",
  title_desc: "Title Z–A",
};

function isNoteSortOrder(value: string | null): value is NoteSortOrder {
  return value != null && SORT_ORDERS.includes(value as NoteSortOrder);
}

const EMPTY_TRANSCRIPT_FILTERS: TranscriptSearchFilters = {
  people: [],
  folderIds: [],
  meetingTypes: [],
};

const EMPTY_TRANSCRIPT_FACETS: TranscriptSearchFacets = {
  people: [],
  folders: [],
  meeting_types: [],
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

function displayedNoteTitle(note: NoteRow, standupNoteIds: Set<number>): string {
  if (isScheduleNote(note)) return scheduleNoteTitle(note);
  if (standupNoteIds.has(note.id)) return datedNoteTitle(note);
  return noteTitle(note);
}

function compareDates(
  left: string | null,
  right: string | null,
  oldestFirst: boolean
): number {
  const leftTime = left == null ? Number.NaN : Date.parse(left);
  const rightTime = right == null ? Number.NaN : Date.parse(right);
  const leftKnown = Number.isFinite(leftTime);
  const rightKnown = Number.isFinite(rightTime);
  if (!leftKnown && !rightKnown) return 0;
  if (!leftKnown) return 1;
  if (!rightKnown) return -1;
  return oldestFirst ? leftTime - rightTime : rightTime - leftTime;
}

function compareNoteRows(
  left: NoteRow,
  right: NoteRow,
  sortOrder: NoteSortOrder,
  standupNoteIds: Set<number>
): number {
  if (sortOrder === "title_asc" || sortOrder === "title_desc") {
    const titleOrder = displayedNoteTitle(left, standupNoteIds).localeCompare(
      displayedNoteTitle(right, standupNoteIds),
      undefined,
      { numeric: true, sensitivity: "base" }
    );
    if (titleOrder !== 0) return sortOrder === "title_asc" ? titleOrder : -titleOrder;
  }
  const dateOrder = compareDates(
    `${left.event_date}T12:00:00Z`,
    `${right.event_date}T12:00:00Z`,
    sortOrder === "date_asc"
  );
  if (dateOrder !== 0) return dateOrder;
  return sortOrder === "date_asc" ? left.id - right.id : right.id - left.id;
}

function compareMeetingRows(
  left: MeetingListRow,
  right: MeetingListRow,
  sortOrder: NoteSortOrder
): number {
  if (sortOrder === "title_asc" || sortOrder === "title_desc") {
    const titleOrder = left.title.localeCompare(right.title, undefined, {
      numeric: true,
      sensitivity: "base",
    });
    if (titleOrder !== 0) return sortOrder === "title_asc" ? titleOrder : -titleOrder;
  }
  const dateOrder = compareDates(
    left.started_at,
    right.started_at,
    sortOrder === "date_asc"
  );
  if (dateOrder !== 0) return dateOrder;
  return sortOrder === "date_asc" ? left.id - right.id : right.id - left.id;
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
  const [searchInstrument, setSearchInstrument] = useState<SearchInstrument>(null);
  const [transcriptFacets, setTranscriptFacets] = useState<TranscriptSearchFacets>(
    EMPTY_TRANSCRIPT_FACETS
  );
  const [transcriptFilters, setTranscriptFilters] = useState<TranscriptSearchFilters>(
    EMPTY_TRANSCRIPT_FILTERS
  );
  const [sortOrder, setSortOrderState] = useState<NoteSortOrder>(() => {
    const saved = localStorage.getItem("noted-note-sort");
    return isNoteSortOrder(saved) ? saved : "date_desc";
  });
  const [folders, setFolders] = useState<NoteFolderInfo[]>([]);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [creating, setCreating] = useState<CreateTarget | null>(null);
  const [newFolderName, setNewFolderName] = useState("");
  const [menuFolder, setMenuFolder] = useState<number | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [filing, setFiling] = useState(false);
  const [filingMsg, setFilingMsg] = useState<string | null>(null);
  const [draggingItem, setDraggingItem] = useState<LibraryDragItem | null>(null);
  const [folderDropTarget, setFolderDropTargetState] = useState<{
    folderId: number;
    placement: FolderDropPlacement;
  } | null>(null);
  const [folderDragPoint, setFolderDragPoint] = useState<{ x: number; y: number } | null>(null);
  const [folderMoveNotice, setFolderMoveNotice] = useState<FolderMoveNotice | null>(null);
  const [activeSpaceId, setActiveSpaceIdState] = useState<number | null>(() => {
    const saved = Number(localStorage.getItem("noted-active-space"));
    return Number.isFinite(saved) && saved > 0 ? saved : null;
  });
  const [spaceMenuOpen, setSpaceMenuOpen] = useState(false);
  const [topicsOpen, setTopicsOpenState] = useState(
    () => localStorage.getItem("noted-topics") === "open"
  );
  const activeFolderPointer = useRef<ActiveFolderPointer | null>(null);
  const folderDropTargetRef = useRef<FolderDropTarget | null>(null);
  const spaceSwitcherRef = useRef<HTMLDivElement | null>(null);
  const suppressRowOpen = useRef(false);
  const folderExpandTimer = useRef<number | null>(null);
  const folderNoticeTimer = useRef<number | null>(null);
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

  const setSortOrder = (next: NoteSortOrder) => {
    setSortOrderState(next);
    localStorage.setItem("noted-note-sort", next);
  };

  const setTopicsOpen = (open: boolean) => {
    setTopicsOpenState(open);
    localStorage.setItem("noted-topics", open ? "open" : "closed");
  };

  useEffect(() => {
    if (!spaceMenuOpen) return;
    const dismiss = (event: MouseEvent) => {
      if (!spaceSwitcherRef.current?.contains(event.target as Node)) {
        setSpaceMenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSpaceMenuOpen(false);
    };
    document.addEventListener("mousedown", dismiss);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("mousedown", dismiss);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [spaceMenuOpen]);

  useEffect(
    () => () => {
      if (folderExpandTimer.current != null) window.clearTimeout(folderExpandTimer.current);
      if (folderNoticeTimer.current != null) window.clearTimeout(folderNoticeTimer.current);
    },
    []
  );

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

  const loadTranscriptFacets = useCallback(async () => {
    try {
      setTranscriptFacets(await api.meetingSearchFacets());
    } catch {
      // Search itself remains available if an older phone build has not yet
      // learned the management endpoints.
    }
  }, []);

  useEffect(() => {
    loadMeetings();
    loadFolders();
    loadTranscriptFacets();
  }, [loadFolders, loadMeetings, loadTranscriptFacets]);

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
        .meetingSearchTranscripts(search, transcriptFilters, sortOrder)
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
  }, [query, selection, sortOrder, transcriptFilters]);

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
            name.toLowerCase() === "work" ? 0 : name.toLowerCase() === "personal" ? 1 : 2;
          return rank(a.name) - rank(b.name);
        }),
    [folderChildren]
  );

  const workspaceSpace = rootSpaces.find((folder) => folder.name.toLowerCase() === "work");
  const activeSpace =
    rootSpaces.find((folder) => folder.id === activeSpaceId) ?? workspaceSpace ?? rootSpaces[0];
  const activeSpaceLabel =
    activeSpace?.name.toLowerCase() === "work"
      ? "My Workspace"
      : activeSpace?.name.toLowerCase() === "personal"
        ? "My Personal Space"
        : activeSpace
          ? `My ${activeSpace.name} Space`
          : "My Workspace";
  const activeSpaceDescription =
    activeSpace?.name.toLowerCase() === "personal" ? "Personal notes" : "Work notes";
  const defaultFolderParent = activeSpace;

  useEffect(() => {
    if (!activeSpace || activeSpace.id === activeSpaceId) return;
    setActiveSpaceIdState(activeSpace.id);
    localStorage.setItem("noted-active-space", String(activeSpace.id));
  }, [activeSpace, activeSpaceId]);

  const topLevelFolders = useMemo(
    () =>
      activeSpace == null
        ? []
        : (folderChildren.get(activeSpace.id) ?? []).filter(
            (folder) => folder.kind === "folder"
          ),
    [activeSpace, folderChildren]
  );

  const allSpaceNoteIds = useMemo(() => {
    const ids = new Set<number>();
    for (const space of rootSpaces) {
      for (const noteId of folderNoteIds.get(space.id) ?? []) ids.add(noteId);
    }
    return ids;
  }, [folderNoteIds, rootSpaces]);

  const activeSpaceNoteIds = useMemo(() => {
    const explicit = new Set(activeSpace ? folderNoteIds.get(activeSpace.id) ?? [] : []);
    // Before spaces were visible, most captures were intentionally left
    // unfiled. Preserve them in the existing Work library until the user or a
    // future routing rule gives them a more specific home.
    if (activeSpace?.name.toLowerCase() === "work") {
      for (const note of notes) {
        if (!allSpaceNoteIds.has(note.id)) explicit.add(note.id);
      }
    }
    return explicit;
  }, [activeSpace, allSpaceNoteIds, folderNoteIds, notes]);

  const scopedNotes = useMemo(
    () => (activeSpace ? notes.filter((note) => activeSpaceNoteIds.has(note.id)) : notes),
    [activeSpace, activeSpaceNoteIds, notes]
  );

  const successfulMeetings = useMemo(
    () =>
      meetings.filter(
        (meeting) =>
          (meeting.status !== "failed" ||
            meeting.segment_count > 0 ||
            meeting.summary_count > 0 ||
            meeting.note_id != null) &&
          (meeting.note_id != null
            ? activeSpaceNoteIds.has(meeting.note_id)
            : activeSpace?.name.toLowerCase() === "work")
      ),
    [activeSpace, activeSpaceNoteIds, meetings]
  );

  const scheduleNotes = useMemo(() => {
    const seenDays = new Set<string>();
    return scopedNotes.filter((note) => {
      if (!isScheduleNote(note) || seenDays.has(note.event_date)) return false;
      seenDays.add(note.event_date);
      return true;
    });
  }, [scopedNotes]);
  const libraryNotes = useMemo(
    () => scopedNotes.filter((note) => !isScheduleNote(note)),
    [scopedNotes]
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
  const selectedFolderParentWithinSpace = selectedFolderParent
    .split(" / ")
    .filter((name) => name && name.toLowerCase() !== activeSpace?.name.toLowerCase())
    .join(" / ");

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
    return [...rows].sort((left, right) =>
      compareNoteRows(left, right, sortOrder, standupNoteIds)
    );
  }, [libraryNotes, query, scheduleNotes, selectedFolder, selection, sortOrder, standupNoteIds]);

  const meetingRows = useMemo(() => {
    const rows = selection === "meeting-trash" ? trashedMeetings : successfulMeetings;
    const normalizedQuery = query.trim().toLowerCase();
    const filtered = normalizedQuery
      ? rows.filter((meeting) =>
          `${meeting.title} ${meeting.status}`.toLowerCase().includes(normalizedQuery)
        )
      : rows;
    return [...filtered].sort((left, right) => compareMeetingRows(left, right, sortOrder));
  }, [query, selection, sortOrder, successfulMeetings, trashedMeetings]);

  const activeMeetingIds = useMemo(
    () => new Set(successfulMeetings.map((meeting) => meeting.id)),
    [successfulMeetings]
  );
  const visibleTranscriptHits = useMemo(
    () => transcriptHits.filter((hit) => activeMeetingIds.has(hit.meeting_id)),
    [activeMeetingIds, transcriptHits]
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

  function selectSpace(space: NoteFolderInfo) {
    setActiveSpaceIdState(space.id);
    localStorage.setItem("noted-active-space", String(space.id));
    setSelection("all");
    setQuery("");
    setSearchInstrument(null);
    setCreating(null);
    setNewFolderName("");
    setMenuFolder(null);
    setFolderError(null);
    setSpaceMenuOpen(false);
  }

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

  function cancelFolderCreation() {
    setCreating(null);
    setNewFolderName("");
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

  function showFolderMoveNotice(notice: FolderMoveNotice) {
    setFolderMoveNotice(notice);
    if (folderNoticeTimer.current != null) window.clearTimeout(folderNoticeTimer.current);
    folderNoticeTimer.current = window.setTimeout(() => {
      setFolderMoveNotice(null);
      folderNoticeTimer.current = null;
    }, notice.kind === "success" ? 3200 : 5600);
  }

  function clearFolderDrag() {
    if (folderExpandTimer.current != null) {
      window.clearTimeout(folderExpandTimer.current);
      folderExpandTimer.current = null;
    }
    activeFolderPointer.current = null;
    folderDropTargetRef.current = null;
    setDraggingItem(null);
    setFolderDropTargetState(null);
    setFolderDragPoint(null);
  }

  function setFolderDropTarget(target: FolderDropTarget | null) {
    const previous = folderDropTargetRef.current;
    if (
      previous?.folder.id === target?.folder.id &&
      previous?.placement === target?.placement
    ) {
      return;
    }
    folderDropTargetRef.current = target;
    setFolderDropTargetState(
      target ? { folderId: target.folder.id, placement: target.placement } : null
    );
    if (folderExpandTimer.current != null) {
      window.clearTimeout(folderExpandTimer.current);
      folderExpandTimer.current = null;
    }
    if (!target || target.placement !== "inside") return;
    const children = folderChildren.get(target.folder.id) ?? [];
    if (children.length > 0 && !expanded.has(target.folder.id)) {
      folderExpandTimer.current = window.setTimeout(() => {
        setExpanded(new Set(expanded).add(target.folder.id));
        folderExpandTimer.current = null;
      }, 650);
    }
  }

  function beginFolderPointer(
    event: ReactPointerEvent<HTMLElement>,
    item: LibraryDragItem
  ) {
    if (event.button !== 0) return;
    activeFolderPointer.current = {
      item,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function folderIsInside(folderId: number, possibleAncestorId: number): boolean {
    const seen = new Set<number>();
    let current = folders.find((folder) => folder.id === folderId);
    while (current && !seen.has(current.id)) {
      if (current.id === possibleAncestorId) return true;
      seen.add(current.id);
      current = current.parent_id == null
        ? undefined
        : folders.find((folder) => folder.id === current?.parent_id);
    }
    return false;
  }

  function folderAtPoint(x: number, y: number): FolderDropTarget | null {
    const element = document
      .elementFromPoint(x, y)
      ?.closest<HTMLElement>("[data-folder-drop-id]");
    const id = element == null ? Number.NaN : Number(element.dataset.folderDropId);
    const folder = Number.isFinite(id)
      ? folders.find((item) => item.id === id)
      : undefined;
    const dragged = activeFolderPointer.current?.item;
    if (!element || !folder || !dragged) return null;

    if (dragged.kind === "note") return { folder, placement: "inside" };

    const rect = element.getBoundingClientRect();
    const relativeY = rect.height > 0 ? (y - rect.top) / rect.height : 0.5;
    const placement: FolderDropPlacement =
      relativeY < 0.3 ? "before" : relativeY > 0.7 ? "after" : "inside";
    if (folder.id === dragged.folderId) return null;

    const destinationParent = placement === "inside" ? folder.id : folder.parent_id;
    if (
      destinationParent != null &&
      folderIsInside(destinationParent, dragged.folderId)
    ) {
      return null;
    }
    return { folder, placement };
  }

  function moveFolderPointer(event: ReactPointerEvent<HTMLElement>) {
    const active = activeFolderPointer.current;
    if (!active || active.pointerId !== event.pointerId) return;
    const distance = Math.hypot(event.clientX - active.startX, event.clientY - active.startY);
    if (!active.moved && distance < 6) return;
    if (!active.moved) {
      active.moved = true;
      suppressRowOpen.current = true;
      setDraggingItem(active.item);
      setFolderMoveNotice(null);
      window.getSelection()?.removeAllRanges();
    }
    event.preventDefault();
    setFolderDragPoint({ x: event.clientX, y: event.clientY });
    setFolderDropTarget(folderAtPoint(event.clientX, event.clientY));
  }

  function endFolderPointer(event: ReactPointerEvent<HTMLElement>) {
    const active = activeFolderPointer.current;
    if (!active || active.pointerId !== event.pointerId) return;
    const moved = active.moved;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    const target =
      folderAtPoint(event.clientX, event.clientY) ?? folderDropTargetRef.current;
    clearFolderDrag();
    if (!moved) return;
    event.preventDefault();
    window.setTimeout(() => {
      suppressRowOpen.current = false;
    }, 0);
    if (target) void performFolderDrop(active.item, target);
  }

  function cancelFolderPointer(event: ReactPointerEvent<HTMLElement>) {
    const active = activeFolderPointer.current;
    if (!active || active.pointerId !== event.pointerId) return;
    clearFolderDrag();
    suppressRowOpen.current = false;
  }

  function openUnlessDragged(openItem: () => void) {
    if (suppressRowOpen.current) return;
    openItem();
  }

  async function performFolderDrop(item: LibraryDragItem, target: FolderDropTarget) {
    try {
      if (item.kind === "note") {
        await api.fileNote(item.noteId, target.folder.id);
      } else {
        const destinationParentId =
          target.placement === "inside" ? target.folder.id : target.folder.parent_id;
        let beforeId: number | null = null;
        if (target.placement === "before") {
          beforeId = target.folder.id;
        } else if (target.placement === "after") {
          const siblings = (folderChildren.get(destinationParentId) ?? []).filter(
            (folder) => folder.kind === "folder" && folder.id !== item.folderId
          );
          const targetIndex = siblings.findIndex((folder) => folder.id === target.folder.id);
          beforeId = targetIndex >= 0 ? siblings[targetIndex + 1]?.id ?? null : null;
        }
        await api.moveNoteFolder(item.folderId, destinationParentId, beforeId);
        if (target.placement === "inside") {
          setExpanded(new Set(expanded).add(target.folder.id));
        }
      }
      await Promise.all([loadFolders(), loadTranscriptFacets()]);
      showFolderMoveNotice({
        kind: "success",
        message:
          item.kind === "note"
            ? `Moved “${item.label}” to ${folderPath(target.folder.id, folders)}.`
            : target.placement === "inside"
              ? `Moved “${item.label}” into “${target.folder.name}”.`
              : `Moved “${item.label}” ${target.placement} “${target.folder.name}”.`,
      });
    } catch (error) {
      showFolderMoveNotice({
        kind: "error",
        message: `Could not move “${item.label}”: ${String(error)}`,
      });
    }
  }

  function toggleStringTranscriptFilter(
    key: "people" | "meetingTypes",
    value: string
  ) {
    setTranscriptFilters((current) => {
      const values = current[key];
      return {
        ...current,
        [key]: values.includes(value)
          ? values.filter((item) => item !== value)
          : [...values, value],
      };
    });
  }

  function toggleFolderTranscriptFilter(folderId: number) {
    setTranscriptFilters((current) => ({
      ...current,
      folderIds: current.folderIds.includes(folderId)
        ? current.folderIds.filter((id) => id !== folderId)
        : [...current.folderIds, folderId],
    }));
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
    const filingTargets = folders.filter(
      (folder) => folder.kind === "space" || folder.kind === "folder"
    );
    const filedIn = filingTargets.filter((folder) => folder.note_ids.includes(openNote.id));
    const filingTargetLabel = (folder: NoteFolderInfo) => {
      if (folder.kind !== "space") return folderPath(folder.id, folders);
      if (folder.name.toLowerCase() === "work") return "My Workspace · Inbox";
      if (folder.name.toLowerCase() === "personal") return "My Personal Space · Inbox";
      return `${folder.name} · Inbox`;
    };
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
                {filedIn.map(filingTargetLabel).join(" · ")}
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
                  {filingTargets.map((folder) => (
                    <option key={folder.id} value={folder.id}>
                      {filingTargetLabel(folder)}
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

  const renderNoteRow = (note: NoteRow) => {
    const canMove = !isScheduleNote(note);
    const label = displayedNoteTitle(note, standupNoteIds);
    return (
    <button
      key={note.id}
      className={`note-row${canMove ? " can-drag" : ""}${
        draggingItem?.kind === "note" && draggingItem.noteId === note.id ? " dragging" : ""
      }`}
      onClick={() => openUnlessDragged(() => open(note))}
      onPointerDown={canMove ? (event) => beginFolderPointer(event, { kind: "note", noteId: note.id, label }) : undefined}
      onPointerMove={canMove ? moveFolderPointer : undefined}
      onPointerUp={canMove ? endFolderPointer : undefined}
      onPointerCancel={canMove ? cancelFolderPointer : undefined}
      title={canMove ? "Drag to a folder" : undefined}
    >
      {isScheduleNote(note) ? (
        <CalendarDays size={14} className="note-row-icon" />
      ) : meetingIdOf(note) != null ? (
        <AudioLines size={14} className="note-row-icon" />
      ) : (
        <FileText size={14} className="note-row-icon" />
      )}
      <span className="note-row-title">{label}</span>
      {!isScheduleNote(note) && (
        <span className="note-row-categories">{noteCats(note).slice(0, 2).join(" · ")}</span>
      )}
      <span className="note-row-date">{relativeDay(note.event_date)}</span>
    </button>
    );
  };

  const transcriptSearchActive =
    (selection === "all" || selection === "meetings") && query.trim().length >= 2;
  const activeTranscriptFilterCount =
    transcriptFilters.people.length +
    transcriptFilters.folderIds.length +
    transcriptFilters.meetingTypes.length;
  const transcriptFiltersActive = activeTranscriptFilterCount > 0;

  const renderFacetGroup = (
    label: string,
    values: TranscriptSearchFacets["people"],
    selected: (value: string) => boolean,
    toggle: (value: string) => void
  ) => (
    <fieldset className="search-filter-group">
      <legend>{label}</legend>
      <div className="search-filter-values">
        {values.length === 0 ? (
          <span className="search-filter-empty">No values yet</span>
        ) : (
          values.map((facet) => (
            <label key={facet.value} className="search-filter-value">
              <input
                type="checkbox"
                checked={selected(facet.value)}
                onChange={() => toggle(facet.value)}
              />
              <span>{facet.label}</span>
              <small>{facet.count}</small>
            </label>
          ))
        )}
      </div>
    </fieldset>
  );

  const renderSearchInstrument = () => {
    if (searchInstrument === "filters") {
      return (
        <section className="search-instrument" aria-label="Transcript filters">
          <header className="search-instrument-head">
            <div>
              <h2>Filter transcript matches</h2>
              <p>Choose within a group for either/or; groups combine together.</p>
            </div>
            <div className="search-instrument-actions">
              {transcriptFiltersActive && (
                <button
                  onClick={() => setTranscriptFilters(EMPTY_TRANSCRIPT_FILTERS)}
                >
                  Clear all
                </button>
              )}
              <button
                className="search-instrument-close"
                onClick={() => setSearchInstrument(null)}
                aria-label="Close transcript filters"
              >
                <X size={15} />
              </button>
            </div>
          </header>
          <div className="search-filter-grid">
            {renderFacetGroup(
              "People",
              transcriptFacets.people,
              (value) => transcriptFilters.people.includes(value),
              (value) => toggleStringTranscriptFilter("people", value)
            )}
            {renderFacetGroup(
              "Folders and companies",
              transcriptFacets.folders,
              (value) => transcriptFilters.folderIds.includes(Number(value)),
              (value) => toggleFolderTranscriptFilter(Number(value))
            )}
            {renderFacetGroup(
              "Meeting type",
              transcriptFacets.meeting_types,
              (value) => transcriptFilters.meetingTypes.includes(value),
              (value) => toggleStringTranscriptFilter("meetingTypes", value)
            )}
          </div>
        </section>
      );
    }
    return null;
  };

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
                : `${visibleTranscriptHits.length === 200 ? "200+" : visibleTranscriptHits.length} ${
                    visibleTranscriptHits.length === 1 ? "line" : "lines"
                  }`}
          </span>
        </header>
        {transcriptSearchError ? (
          <p className="transcript-results-error">Transcript search is temporarily unavailable.</p>
        ) : (
          <div className="transcript-result-list">
            {visibleTranscriptHits.map((hit) => (
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
        className={`note-row folder-content-row can-drag${
          draggingItem ? " drop-available" : ""
        }${
          draggingItem?.kind === "folder" && draggingItem.folderId === folder.id
            ? " dragging"
            : ""
        }${
          folderDropTarget?.folderId === folder.id
            ? ` drop-${folderDropTarget.placement}`
            : ""
        }`}
        onClick={() => openUnlessDragged(() => setSelection(`folder:${folder.id}`))}
        onPointerDown={(event) =>
          beginFolderPointer(event, {
            kind: "folder",
            folderId: folder.id,
            label: folder.name,
          })
        }
        onPointerMove={moveFolderPointer}
        onPointerUp={endFolderPointer}
        onPointerCancel={cancelFolderPointer}
        title="Drag to reorder or move into another folder"
        data-folder-drop-id={folder.id}
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
          className={`folder-tree-row${isSelected ? " on" : ""}${
            draggingItem ? " drop-available" : ""
          }${
            draggingItem?.kind === "folder" && draggingItem.folderId === folder.id
              ? " dragging"
              : ""
          }${
            folderDropTarget?.folderId === folder.id
              ? ` drop-${folderDropTarget.placement}`
              : ""
          }`}
          style={{ "--folder-depth": depth } as CSSProperties}
          data-folder-drop-id={folder.id}
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
                className="folder-main can-drag"
                title={`Drag to reorder ${folder.name} or move it into another folder`}
                onClick={() => {
                  openUnlessDragged(() => {
                    setSelection(`folder:${folder.id}`);
                    setMenuFolder(null);
                  });
                }}
                onPointerDown={(event) =>
                  beginFolderPointer(event, {
                    kind: "folder",
                    folderId: folder.id,
                    label: folder.name,
                  })
                }
                onPointerMove={moveFolderPointer}
                onPointerUp={endFolderPointer}
                onPointerCancel={cancelFolderPointer}
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
                setCreating({
                  parentId: folder.id,
                  label: `Folder in ${folder.name}`,
                });
                setNewFolderName("");
              }}
            >
              New subfolder
            </button>
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
        {creating?.parentId === folder.id && (
          <form
            className="folder-create nested"
            style={{ "--folder-depth": depth } as CSSProperties}
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
                if (event.key === "Escape") cancelFolderCreation();
              }}
              placeholder={creating.label}
              aria-label={creating.label}
            />
            <button
              type="button"
              className="folder-create-cancel"
              onClick={cancelFolderCreation}
              aria-label="Cancel new subfolder"
              title="Cancel"
            >
              <X size={13} />
            </button>
          </form>
        )}
        {isExpanded && children.map((child) => renderFolder(child, depth + 1))}
      </div>
    );
  };

  const listedCount = transcriptSearchActive && transcriptFiltersActive
    ? 0
    : meetingView
      ? meetingRows.length
      : selectedFolder
        ? visibleFolderChildren.length + list.length
        : list.length;
  const visibleCount = listedCount + (transcriptSearchActive ? visibleTranscriptHits.length : 0);
  const countLabel = transcriptSearchActive
    ? `${visibleCount}${visibleTranscriptHits.length === 200 ? "+" : ""} ${
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
    : transcriptSearchActive && transcriptFiltersActive
      ? "No transcript lines match this search and filter combination."
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
      {draggingItem && folderDragPoint && (
        <div
          className="folder-drag-preview"
          style={{ left: folderDragPoint.x + 14, top: folderDragPoint.y + 12 }}
          aria-hidden="true"
        >
          {draggingItem.kind === "folder" ? <FolderOpen size={13} /> : <FileText size={13} />}
          <strong>{draggingItem.label}</strong>
        </div>
      )}
      <div className="notes-library-shell">
        <button
          className="notes-library-toggle icon-btn"
          onClick={() => setLibraryOpen(!libraryOpen)}
          title={`${libraryOpen ? "Collapse" : "Show"} library`}
          aria-label={`${libraryOpen ? "Collapse" : "Show"} library`}
          aria-expanded={libraryOpen}
        >
          {libraryOpen ? <PanelLeftClose size={15} /> : <PanelLeftOpen size={15} />}
        </button>

        {libraryOpen && (
          <aside className="spaces" aria-label="Notes library">
          <div className="space-switcher" ref={spaceSwitcherRef}>
            <button
              className="space-switcher-trigger"
              onClick={() => setSpaceMenuOpen((open) => !open)}
              aria-haspopup="menu"
              aria-expanded={spaceMenuOpen}
            >
              <span className="space-switcher-copy">
                <strong>{activeSpaceLabel}</strong>
                <small>{activeSpaceDescription}</small>
              </span>
              <ChevronDown size={15} aria-hidden="true" />
            </button>
            {spaceMenuOpen && (
              <div className="space-switcher-menu" role="menu" aria-label="Switch space">
                {rootSpaces.map((space) => {
                  const isWork = space.name.toLowerCase() === "work";
                  const isPersonal = space.name.toLowerCase() === "personal";
                  const label = isWork
                    ? "My Workspace"
                    : isPersonal
                      ? "My Personal Space"
                      : `My ${space.name} Space`;
                  return (
                    <button
                      key={space.id}
                      role="menuitemradio"
                      aria-checked={space.id === activeSpace?.id}
                      className={space.id === activeSpace?.id ? "on" : ""}
                      onClick={() => selectSpace(space)}
                    >
                      <span>
                        <strong>{label}</strong>
                        <small>{isPersonal ? "Personal notes" : "Work notes"}</small>
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
          <div className="library-saved-label">Saved views</div>
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
            {!creating && (
              <button
                className="library-add"
                disabled={!defaultFolderParent}
                onClick={() => {
                  if (!defaultFolderParent) return;
                  setCreating({
                    parentId: defaultFolderParent.id,
                    label: `Folder in ${activeSpaceLabel}`,
                  });
                  setNewFolderName("");
                }}
                aria-label={`New folder in ${activeSpaceLabel}`}
                title="New folder"
              >
                <Plus size={14} />
              </button>
            )}
          </div>
          <div className="folder-tree">
            {topLevelFolders.map((folder) => renderFolder(folder, 0))}
          </div>
          {folderMoveNotice && (
            <div
              className={`folder-move-notice ${folderMoveNotice.kind}`}
              role={folderMoveNotice.kind === "error" ? "alert" : "status"}
              aria-live="polite"
            >
              {folderMoveNotice.message}
            </div>
          )}
          {creating && creating.parentId === activeSpace?.id && (
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
                  if (event.key === "Escape") cancelFolderCreation();
                }}
                placeholder={creating.label}
                aria-label={creating.label}
              />
              <button
                type="button"
                className="folder-create-cancel"
                onClick={cancelFolderCreation}
                aria-label="Cancel new folder"
                title="Cancel"
              >
                <X size={13} />
              </button>
            </form>
          )}
          <div className="library-topics">
            <button
              className="library-topics-toggle"
              onClick={() => setTopicsOpen(!topicsOpen)}
              aria-expanded={topicsOpen}
            >
              {topicsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              <span>Topics</span>
              <small>Automatic</small>
            </button>
            {topicsOpen && (
              <nav className="library-categories" aria-label="Topics">
                {categories.length > 0 ? (
                  categories.map((category) => (
                    <button
                      key={category.id}
                      className={selection === category.id ? "on" : ""}
                      onClick={() => setSelection(category.id)}
                    >
                      <FileText size={13} />
                      <span className="space-label">{category.label}</span>
                      <span className="space-n">{category.count}</span>
                    </button>
                  ))
                ) : (
                  <span className="library-topics-empty">No topics in this space yet.</span>
                )}
              </nav>
            )}
          </div>

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
      </div>

      <main className="notes-list">
        <div className="notes-context">
          <div>
            <div className="notes-breadcrumb">
              {activeSpaceLabel}
              {selectedFolderParentWithinSpace
                ? ` / ${selectedFolderParentWithinSpace}`
                : ""}
            </div>
            <h1>{currentLabel}</h1>
            {selectedFolder?.auto_rule === "daily_standup" && (
              <p>Stand-up notes are filed here automatically.</p>
            )}
          </div>
          <span className="notes-context-count">{countLabel}</span>
        </div>
        <div className="notes-list-head">
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
          <div className="search-tool-buttons">
            {(selection === "all" || selection === "meetings") && (
              <button
                className={
                  searchInstrument === "filters" || transcriptFiltersActive ? "on" : ""
                }
                onClick={() =>
                  setSearchInstrument((current) =>
                    current === "filters" ? null : "filters"
                  )
                }
                aria-expanded={searchInstrument === "filters"}
              >
                <ListFilter size={14} />
                <span>Filter</span>
                {activeTranscriptFilterCount > 0 && (
                  <small>{activeTranscriptFilterCount}</small>
                )}
              </button>
            )}
            <label className="notes-sort">
              <ArrowUpDown size={14} aria-hidden="true" />
              <span className="notes-sort-label">Sort by</span>
              <select
                aria-label="Sort notes by"
                value={sortOrder}
                onChange={(event) => setSortOrder(event.target.value as NoteSortOrder)}
              >
                {SORT_ORDERS.map((order) => (
                  <option value={order} key={order}>
                    {SORT_LABELS[order]}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>

        {(selection === "all" || selection === "meetings") && renderSearchInstrument()}
        {renderTranscriptResults()}

        {visibleCount === 0 ? (
          <p className="quiet-empty">{emptyMessage}</p>
        ) : transcriptSearchActive && transcriptFiltersActive ? null : meetingView ? (
          meetingRows.map((meeting) => (
            <button
              key={meeting.id}
              className={`note-row${
                meeting.note_id != null &&
                draggingItem?.kind === "note" &&
                draggingItem.noteId === meeting.note_id
                  ? " dragging"
                  : ""
              }${selection !== "meeting-trash" && meeting.note_id != null ? " can-drag" : ""}`}
              onClick={() => openUnlessDragged(() => setOpenMeeting({ id: meeting.id }))}
              onPointerDown={
                selection !== "meeting-trash" && meeting.note_id != null
                  ? (event) =>
                      beginFolderPointer(event, {
                        kind: "note",
                        noteId: meeting.note_id as number,
                        label: meeting.title,
                      })
                  : undefined
              }
              onPointerMove={
                selection !== "meeting-trash" && meeting.note_id != null
                  ? moveFolderPointer
                  : undefined
              }
              onPointerUp={
                selection !== "meeting-trash" && meeting.note_id != null
                  ? endFolderPointer
                  : undefined
              }
              onPointerCancel={
                selection !== "meeting-trash" && meeting.note_id != null
                  ? cancelFolderPointer
                  : undefined
              }
              title={
                selection !== "meeting-trash" && meeting.note_id != null
                  ? "Drag to a folder"
                  : undefined
              }
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
              <section className="folder-index">
                <header className="folder-index-head">
                  <h2>Folders</h2>
                  <span>{visibleFolderChildren.length}</span>
                </header>
                <div className="folder-index-list">
                  {visibleFolderChildren.map(renderMainFolderRow)}
                </div>
              </section>
            )}
            {list.map(renderNoteRow)}
          </>
        ) : (
          list.map(renderNoteRow)
        )}
      </main>
    </div>
  );
}
