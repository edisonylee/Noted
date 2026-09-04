// Library is the cross-source organizer; Documents is the focused authoring
// workspace. They share filing and editor behavior without sharing navigation
// hierarchy or inferring content type from formatting.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
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
  ChevronLeft,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Inbox,
  ListFilter,
  MoreHorizontal,
  PenLine,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import {
  api,
  type CategoryInfo,
  type MeetingListRow,
  type NoteSortOrder,
  type NoteFilingReceipt,
  type NoteFolderInfo,
  type NoteRow,
  type TranscriptSearchFacets,
  type TranscriptSearchFilters,
  type TranscriptSearchHit,
} from "./api";
import { DataView } from "./DataView";
import { MeetingPage } from "./MeetingPage";
import { NoteDocumentEditor } from "./NoteDocumentEditor";
import { easternDay, formatDay, relativeDay } from "./day";
import { emptyDocument } from "./editor/document";
import { isDocumentNote } from "./library";
import {
  hasStoredFilingContext,
  isFilingContext,
  onFilingContextChange,
  readFilingContext,
  writeFilingContext,
  type FilingContext,
} from "./filingContext";

type CreateTarget = {
  parentId: number;
  label: string;
};

type MeetingTarget = {
  id: number;
  segmentId?: number;
};

type MixedLibraryRow =
  | { kind: "meeting"; meeting: MeetingListRow }
  | { kind: "note"; note: NoteRow };

type NoteContextTarget = {
  kind: "meeting" | "note";
  id: number;
  noteId: number | null;
  label: string;
  trashed: boolean;
  canTrash: boolean;
  x: number;
  y: number;
};

type LibraryDragItem =
  | {
      kind: "note";
      noteId: number;
      label: string;
      trashTarget: Omit<NoteContextTarget, "x" | "y">;
    }
  | { kind: "folder"; folderId: number; label: string };

type FolderDropPlacement = "inside" | "before" | "after";

type FolderDropTarget = {
  folder: NoteFolderInfo;
  placement: FolderDropPlacement;
};

type FolderMoveNotice = {
  kind: "success" | "error";
  message: string;
  undo?: NoteFilingReceipt;
};

type FolderContextPoint = {
  folderId: number;
  x: number;
  y: number;
};

type ActiveFolderPointer = {
  item: LibraryDragItem;
  pointerId: number;
  startX: number;
  startY: number;
  moved: boolean;
};

type SearchInstrument = "filters" | null;
type LibraryWorkspaceMode = "library" | "documents";

type LibraryWorkspaceProps = {
  notes: NoteRow[];
  cats: CategoryInfo[];
  onChanged?: () => void | Promise<void>;
};

function selectionSearchesTranscripts(selection: string): boolean {
  return (
    selection === "all" ||
    selection === "meetings" ||
    selection === "needs-filing"
  );
}

// Schedule already has an app-level home. Journal is parked until it has a
// dedicated Personal-only workflow. Keep Library focused on its source items
// and the one content type with distinct behavior: meetings.
const SHOW_SCHEDULE_IN_LIBRARY = false;
const SHOW_JOURNAL = false;
const SHOW_TOPICS_IN_LIBRARY = false;

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

function datedTitle(title: string, day: string): string {
  const sameYear = day.slice(0, 4) === easternDay().slice(0, 4);
  const date = formatDay(day, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
  const prefix = `${date} · `;
  return title.startsWith(prefix) ? title : `${prefix}${title}`;
}

function datedNoteTitle(note: NoteRow): string {
  return datedTitle(noteTitle(note), note.event_date);
}

function meetingSeriesKey(meeting: MeetingListRow): string | null {
  const value = meeting.event_json?.ical_uid?.trim().toLocaleLowerCase();
  return value || null;
}

function meetingDay(meeting: MeetingListRow): string | null {
  const eventDay = meeting.event_json?.date;
  if (eventDay && /^\d{4}-\d{2}-\d{2}$/.test(eventDay)) return eventDay;
  if (!meeting.started_at) return null;
  const started = new Date(meeting.started_at);
  return Number.isFinite(started.getTime()) ? easternDay(started) : null;
}

function isRecurringMeeting(
  meeting: MeetingListRow,
  recurringSeriesKeys: ReadonlySet<string>
): boolean {
  const seriesKey = meetingSeriesKey(meeting);
  return Boolean(meeting.event_json?.recurring_event_id) ||
    (seriesKey != null && recurringSeriesKeys.has(seriesKey));
}

function displayedMeetingTitle(
  meeting: MeetingListRow,
  recurringSeriesKeys: ReadonlySet<string>,
  standupNoteIds: ReadonlySet<number>,
  notesById: ReadonlyMap<number, NoteRow>
): string {
  const linkedNote = meeting.note_id == null ? undefined : notesById.get(meeting.note_id);
  const title = linkedNote ? noteTitle(linkedNote) : meeting.title;
  const shouldDate =
    (meeting.note_id != null && standupNoteIds.has(meeting.note_id)) ||
    isRecurringMeeting(meeting, recurringSeriesKeys);
  if (!shouldDate) return meeting.title;
  const day = meetingDay(meeting) ?? linkedNote?.event_date;
  return day ? datedTitle(title, day) : title;
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

function displayedNoteTitle(
  note: NoteRow,
  standupNoteIds: ReadonlySet<number>,
  meetingByNoteId?: ReadonlyMap<number, MeetingListRow>,
  recurringSeriesKeys: ReadonlySet<string> = new Set()
): string {
  if (isScheduleNote(note)) return scheduleNoteTitle(note);
  if (standupNoteIds.has(note.id)) return datedNoteTitle(note);
  const meeting = meetingByNoteId?.get(note.id);
  if (meeting && isRecurringMeeting(meeting, recurringSeriesKeys)) {
    return datedTitle(noteTitle(note), meetingDay(meeting) ?? note.event_date);
  }
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
  standupNoteIds: Set<number>,
  useUpdatedAt = false
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
    useUpdatedAt ? left.updated_at : `${left.event_date}T12:00:00Z`,
    useUpdatedAt ? right.updated_at : `${right.event_date}T12:00:00Z`,
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

function compareMixedLibraryRows(
  left: MixedLibraryRow,
  right: MixedLibraryRow,
  sortOrder: NoteSortOrder,
  standupNoteIds: Set<number>
): number {
  const title = (row: MixedLibraryRow) =>
    row.kind === "meeting"
      ? row.meeting.title
      : displayedNoteTitle(row.note, standupNoteIds);
  if (sortOrder === "title_asc" || sortOrder === "title_desc") {
    const titleOrder = title(left).localeCompare(title(right), undefined, {
      numeric: true,
      sensitivity: "base",
    });
    if (titleOrder !== 0) return sortOrder === "title_asc" ? titleOrder : -titleOrder;
  }
  const timestamp = (row: MixedLibraryRow) =>
    row.kind === "meeting"
      ? row.meeting.started_at
      : `${row.note.event_date}T12:00:00Z`;
  const dateOrder = compareDates(
    timestamp(left),
    timestamp(right),
    sortOrder === "date_asc"
  );
  if (dateOrder !== 0) return dateOrder;
  const leftId = left.kind === "meeting" ? left.meeting.id : left.note.id;
  const rightId = right.kind === "meeting" ? right.meeting.id : right.note.id;
  return sortOrder === "date_asc" ? leftId - rightId : rightId - leftId;
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

function noteUpdatedDay(note: NoteRow): string {
  const updated = new Date(note.updated_at);
  return Number.isFinite(updated.getTime()) ? easternDay(updated) : note.event_date;
}

function quantified(count: number, singular: string, plural = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : plural}`;
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

function filingTargetPath(folder: NoteFolderInfo, folders: NoteFolderInfo[]): string {
  if (folder.kind === "space") return `${folder.name} · Inbox`;
  const byId = new Map(folders.map((item) => [item.id, item]));
  const names = [folder.name];
  const seen = new Set<number>([folder.id]);
  let parentId = folder.parent_id;
  while (parentId != null && !seen.has(parentId)) {
    const parent = byId.get(parentId);
    if (!parent) break;
    names.unshift(parent.name);
    seen.add(parent.id);
    parentId = parent.parent_id;
  }
  return names.join(" / ");
}

function LibraryWorkspace({
  notes,
  cats,
  onChanged,
  mode,
}: LibraryWorkspaceProps & { mode: LibraryWorkspaceMode }) {
  const documentsMode = mode === "documents";
  const [selection, setSelection] = useState(documentsMode ? "documents" : "all");
  const [query, setQuery] = useState("");
  const [openNote, setOpenNote] = useState<NoteRow | null>(null);
  const [openMeeting, setOpenMeeting] = useState<MeetingTarget | null>(null);
  const [meetings, setMeetings] = useState<MeetingListRow[]>([]);
  const [trashedMeetings, setTrashedMeetings] = useState<MeetingListRow[]>([]);
  const [trashedNotes, setTrashedNotes] = useState<NoteRow[]>([]);
  const [noteContextMenu, setNoteContextMenu] = useState<NoteContextTarget | null>(null);
  const [noteMoveMenuOpen, setNoteMoveMenuOpen] = useState(false);
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
  const [folderContextPoint, setFolderContextPoint] = useState<FolderContextPoint | null>(null);
  const [renaming, setRenaming] = useState<number | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [filing, setFiling] = useState(false);
  const [filingMeetingId, setFilingMeetingId] = useState<number | null>(null);
  const [filingMsg, setFilingMsg] = useState<string | null>(null);
  const [draggingItem, setDraggingItem] = useState<LibraryDragItem | null>(null);
  const [folderDropTarget, setFolderDropTargetState] = useState<{
    folderId: number;
    placement: FolderDropPlacement;
  } | null>(null);
  const [folderDragPoint, setFolderDragPoint] = useState<{ x: number; y: number } | null>(null);
  const [trashDropActive, setTrashDropActive] = useState(false);
  const [folderMoveNotice, setFolderMoveNotice] = useState<FolderMoveNotice | null>(null);
  const [filingContext, setFilingContextState] = useState<FilingContext>(readFilingContext);
  const [filingContextReady, setFilingContextReady] = useState(hasStoredFilingContext);
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
  const noteContextMenuRef = useRef<HTMLDivElement | null>(null);
  const folderMenuRef = useRef<HTMLDivElement | null>(null);
  const noteContextReturnFocus = useRef<HTMLButtonElement | null>(null);
  const suppressRowOpen = useRef(false);
  const folderExpandTimer = useRef<number | null>(null);
  const folderNoticeTimer = useRef<number | null>(null);
  const [editingNote, setEditingNote] = useState(false);
  const [editTitle, setEditTitle] = useState("");
  const [editBody, setEditBody] = useState("");
  const [savingNote, setSavingNote] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);
  const [creatingNote, setCreatingNote] = useState(false);
  const [createNoteError, setCreateNoteError] = useState<string | null>(null);
  const explorerStorageKey = documentsMode ? "noted-document-files" : "noted-library";
  const [libraryOpen, setLibraryOpenState] = useState(
    () => localStorage.getItem(explorerStorageKey) !== "closed"
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
    localStorage.setItem(explorerStorageKey, open ? "open" : "closed");
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

  useEffect(() => {
    if (!noteContextMenu) return;
    const focusFrame = window.requestAnimationFrame(() => {
      const firstAction = noteContextMenuRef.current?.querySelector<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)'
      );
      (firstAction ?? noteContextMenuRef.current)?.focus();
    });
    const dismiss = (event: MouseEvent) => {
      if (!noteContextMenuRef.current?.contains(event.target as Node)) {
        setNoteContextMenu(null);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setNoteContextMenu(null);
      window.requestAnimationFrame(() => noteContextReturnFocus.current?.focus());
    };
    const closeOnPageScroll = (event: Event) => {
      const target = event.target;
      if (
        target instanceof Node &&
        noteContextMenuRef.current?.contains(target)
      ) {
        return;
      }
      setNoteContextMenu(null);
    };
    const closeWithoutRestoringFocus = () => setNoteContextMenu(null);
    document.addEventListener("mousedown", dismiss);
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("scroll", closeOnPageScroll, true);
    window.addEventListener("blur", closeWithoutRestoringFocus);
    window.addEventListener("resize", closeWithoutRestoringFocus);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener("mousedown", dismiss);
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("scroll", closeOnPageScroll, true);
      window.removeEventListener("blur", closeWithoutRestoringFocus);
      window.removeEventListener("resize", closeWithoutRestoringFocus);
    };
  }, [noteContextMenu]);

  useEffect(() => setNoteContextMenu(null), [selection]);

  useEffect(() => {
    if (menuFolder == null) return;
    const focusFrame = folderContextPoint == null
      ? null
      : window.requestAnimationFrame(() => {
          folderMenuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
        });
    const dismiss = (event: MouseEvent) => {
      const target = event.target as Node;
      if (folderMenuRef.current?.contains(target)) return;
      if ((target as Element).closest?.(".folder-more")) return;
      setMenuFolder(null);
      setFolderContextPoint(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setMenuFolder(null);
      setFolderContextPoint(null);
    };
    const closeOnScroll = () => {
      if (folderContextPoint == null) return;
      setMenuFolder(null);
      setFolderContextPoint(null);
    };
    document.addEventListener("mousedown", dismiss);
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("scroll", closeOnScroll, true);
    return () => {
      if (focusFrame != null) window.cancelAnimationFrame(focusFrame);
      document.removeEventListener("mousedown", dismiss);
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("scroll", closeOnScroll, true);
    };
  }, [folderContextPoint, menuFolder]);

  useEffect(
    () =>
      onFilingContextChange((context) => {
        setFilingContextState(context);
        setFilingContextReady(true);
      }),
    []
  );

  useEffect(
    () => () => {
      if (folderExpandTimer.current != null) window.clearTimeout(folderExpandTimer.current);
      if (folderNoticeTimer.current != null) window.clearTimeout(folderNoticeTimer.current);
    },
    []
  );

  const loadMeetings = useCallback(async () => {
    try {
      const [active, trashed, deletedNotes] = await Promise.all([
        api.meetingList(),
        api.meetingTrashList(),
        api.noteTrashList(),
      ]);
      setMeetings(active);
      setTrashedMeetings(trashed);
      setTrashedNotes(deletedNotes);
    } catch {
      // Keep the last good projection if the desktop bridge is restarting.
    }
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

  // Background phone captures and meeting summaries update the note props
  // without remounting this view. Reload the meeting, folder, and search
  // projections together so a newly routed note appears immediately. This
  // effect also performs the initial load, avoiding a duplicate meeting fetch.
  useEffect(() => {
    loadMeetings();
    loadFolders();
    loadTranscriptFacets();
  }, [notes, loadFolders, loadMeetings, loadTranscriptFacets]);

  useEffect(() => {
    const search = query.trim();
    const searchesTranscripts = selectionSearchesTranscripts(selection);
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

  // `note_ids` also contains smart-rule matches. Context membership must come
  // only from persisted filing decisions, recursively including descendants.
  const explicitFolderNoteIds = useMemo(() => {
    const result = new Map<number, Set<number>>();
    const collect = (id: number, path: Set<number>): Set<number> => {
      const cached = result.get(id);
      if (cached) return cached;
      if (path.has(id)) return new Set();
      const nextPath = new Set(path).add(id);
      const folder = folders.find((item) => item.id === id);
      const ids = new Set(
        (folder?.explicit_filings ?? []).map((filing) => filing.note_id)
      );
      for (const child of folderChildren.get(id) ?? []) {
        for (const noteId of collect(child.id, nextPath)) ids.add(noteId);
      }
      result.set(id, ids);
      return ids;
    };
    for (const folder of folders) collect(folder.id, new Set());
    return result;
  }, [folderChildren, folders]);

  const filingTargets = useMemo(
    () => folders.filter((folder) => folder.kind === "space" || folder.kind === "folder"),
    [folders]
  );

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
  const selectedSpace = rootSpaces.find((folder) => folder.id === activeSpaceId);
  const contextSpace = rootSpaces.find(
    (folder) => folder.name.trim().toLowerCase() === filingContext
  );
  const selectedSpaceContext = selectedSpace?.name.trim().toLowerCase();
  const activeSpace = filingContextReady && isFilingContext(selectedSpaceContext)
    ? contextSpace ?? selectedSpace ?? workspaceSpace ?? rootSpaces[0]
    : selectedSpace ?? contextSpace ?? workspaceSpace ?? rootSpaces[0];
  const activeSpaceLabel = activeSpace?.name ?? "Work";
  const activeSpaceDescription =
    documentsMode ? `${activeSpaceLabel} documents` : `${activeSpaceLabel} library`;
  const defaultFolderParent = activeSpace;

  useEffect(() => {
    if (filingContextReady || rootSpaces.length === 0) return;
    const legacySpace = rootSpaces.find((folder) => folder.id === activeSpaceId);
    const legacyContext = legacySpace?.name.trim().toLowerCase();
    const nextContext = isFilingContext(legacyContext)
      ? legacyContext
      : readFilingContext();
    setFilingContextState(nextContext);
    setFilingContextReady(true);
    if (!hasStoredFilingContext() || readFilingContext() !== nextContext) {
      writeFilingContext(nextContext);
    }
  }, [activeSpaceId, filingContextReady, rootSpaces]);

  useEffect(() => {
    if (!filingContextReady) return;
    const nextSpace = rootSpaces.find(
      (folder) => folder.name.trim().toLowerCase() === filingContext
    );
    if (!nextSpace || nextSpace.id === activeSpaceId) return;
    setActiveSpaceIdState(nextSpace.id);
    localStorage.setItem("noted-active-space", String(nextSpace.id));
    setSelection(documentsMode ? "documents" : "all");
    setQuery("");
    setSearchInstrument(null);
    setCreating(null);
    setNewFolderName("");
    setMenuFolder(null);
    setFolderError(null);
    setSpaceMenuOpen(false);
  }, [activeSpaceId, documentsMode, filingContext, filingContextReady, rootSpaces]);

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

  const activeSpaceFolderIds = useMemo(() => {
    const ids = new Set<number>();
    if (!activeSpace) return ids;
    const visit = (folderId: number) => {
      if (ids.has(folderId)) return;
      ids.add(folderId);
      for (const child of folderChildren.get(folderId) ?? []) visit(child.id);
    };
    visit(activeSpace.id);
    return ids;
  }, [activeSpace, folderChildren]);

  const allSpaceNoteIds = useMemo(() => {
    const ids = new Set<number>();
    for (const space of rootSpaces) {
      for (const noteId of explicitFolderNoteIds.get(space.id) ?? []) ids.add(noteId);
    }
    return ids;
  }, [explicitFolderNoteIds, rootSpaces]);

  const activeSpaceNoteIds = useMemo(() => {
    return new Set(activeSpace ? explicitFolderNoteIds.get(activeSpace.id) ?? [] : []);
  }, [activeSpace, explicitFolderNoteIds]);

  const scopedNotes = useMemo(
    () => (activeSpace ? notes.filter((note) => activeSpaceNoteIds.has(note.id)) : []),
    [activeSpace, activeSpaceNoteIds, notes]
  );

  const availableMeetings = useMemo(
    () =>
      meetings.filter(
        (meeting) =>
          meeting.status === "recording" ||
          meeting.status === "summarizing" ||
          meeting.segment_count > 0 ||
          meeting.summary_count > 0 ||
          meeting.note_id != null
      ),
    [meetings]
  );

  // Completed meetings are scoped by their note's explicit folder ancestry.
  // A context Inbox is a real filing decision, so it must not also appear in
  // the global Needs filing view.
  // Before a meeting has a linked note, its captured context or explicit route
  // remains its home even if summary generation failed and the transcript is
  // already done. That failure must not make a valid recording disappear.
  const successfulMeetings = useMemo(
    () =>
      availableMeetings.filter((meeting) => {
        if (meeting.note_id != null) return activeSpaceNoteIds.has(meeting.note_id);
        const activeMeetingContext = activeSpace?.name.trim().toLowerCase();
        const routedByContext =
          isFilingContext(activeMeetingContext) &&
          meeting.filing_context === activeMeetingContext;
        const routedByFolder =
          (meeting.route_status === "matched" || meeting.route_status === "manual");
        return (
          routedByContext ||
          (routedByFolder &&
            meeting.route_folder_id != null &&
            activeSpaceFolderIds.has(meeting.route_folder_id))
        );
      }),
    [activeSpace, activeSpaceFolderIds, activeSpaceNoteIds, availableMeetings]
  );

  const needsFilingMeetings = useMemo(
    () =>
      availableMeetings.filter((meeting) => {
        if (meeting.note_id != null) {
          return !allSpaceNoteIds.has(meeting.note_id);
        }
        const hasCapturedContext = isFilingContext(meeting.filing_context);
        const hasFolderRoute =
          meeting.route_folder_id != null &&
          (meeting.route_status === "matched" || meeting.route_status === "manual");
        return !hasCapturedContext && !hasFolderRoute;
      }),
    [allSpaceNoteIds, availableMeetings]
  );

  const listedMeetingNoteIds = useMemo(
    () =>
      new Set(
        [...meetings, ...trashedMeetings].flatMap((meeting) =>
          meeting.note_id == null ? [] : [meeting.note_id]
        )
      ),
    [meetings, trashedMeetings]
  );

  const needsFilingNotes = useMemo(
    () =>
      notes.filter(
        (note) =>
          !isScheduleNote(note) &&
          !allSpaceNoteIds.has(note.id) &&
          !listedMeetingNoteIds.has(note.id)
      ),
    [allSpaceNoteIds, listedMeetingNoteIds, notes]
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
  const documentNotes = useMemo(
    () => libraryNotes.filter(isDocumentNote),
    [libraryNotes]
  );
  const documentNoteIds = useMemo(
    () => new Set(documentNotes.map((note) => note.id)),
    [documentNotes]
  );
  const trashedDocumentNotes = useMemo(
    () => trashedNotes.filter(isDocumentNote),
    [trashedNotes]
  );
  const inboxNoteIds = useMemo(
    () => new Set((activeSpace?.explicit_filings ?? []).map((filing) => filing.note_id)),
    [activeSpace]
  );
  const inboxNotes = useMemo(
    () => libraryNotes.filter((note) => inboxNoteIds.has(note.id)),
    [inboxNoteIds, libraryNotes]
  );
  const inboxDocuments = useMemo(
    () => documentNotes.filter((note) => inboxNoteIds.has(note.id)),
    [documentNotes, inboxNoteIds]
  );
  const libraryNoteIds = useMemo(
    () => new Set(libraryNotes.map((note) => note.id)),
    [libraryNotes]
  );
  // Smart folders remain a useful compatibility view for legacy notes that
  // predate explicit filing. Do not let those matches claim another context.
  const smartFolderVisibleNoteIds = useMemo(() => {
    const ids = new Set(libraryNoteIds);
    for (const note of notes) {
      if (!isScheduleNote(note) && !allSpaceNoteIds.has(note.id)) ids.add(note.id);
    }
    return ids;
  }, [allSpaceNoteIds, libraryNoteIds, notes]);

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
  const notesById = useMemo(
    () => new Map([...notes, ...trashedNotes].map((note) => [note.id, note])),
    [notes, trashedNotes]
  );
  const meetingByNoteId = useMemo(
    () =>
      new Map(
        [...meetings, ...trashedMeetings].flatMap((meeting) =>
          meeting.note_id == null ? [] : [[meeting.note_id, meeting] as const]
        )
      ),
    [meetings, trashedMeetings]
  );
  const recurringSeriesKeys = useMemo(() => {
    const counts = new Map<string, number>();
    for (const meeting of [...meetings, ...trashedMeetings]) {
      const key = meetingSeriesKey(meeting);
      if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return new Set(
      [...counts.entries()].filter(([, count]) => count > 1).map(([key]) => key)
    );
  }, [meetings, trashedMeetings]);
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
    if (documentsMode) {
      if (selection === "document-trash") rows = trashedDocumentNotes;
      else if (selection === "inbox") rows = inboxDocuments;
      else if (selection.startsWith("folder:")) {
        const ids = new Set(selectedFolder?.note_ids ?? []);
        rows = documentNotes.filter((note) => ids.has(note.id));
      } else rows = documentNotes;
    } else {
      if (selection === "all") rows = libraryNotes;
      else if (selection === "documents") rows = documentNotes;
      else if (selection === "meeting-trash") rows = trashedNotes;
      else if (selection === "inbox") rows = inboxNotes;
      else if (selection === "schedule") rows = scheduleNotes;
      else if (selection === "journal") {
        rows = libraryNotes.filter((note) => noteCats(note).includes("journal"));
      } else if (selection === "needs-filing") {
        rows = needsFilingNotes;
      } else if (selection.startsWith("category:")) {
        const category = selection.slice("category:".length);
        rows = libraryNotes.filter((note) => noteCats(note).includes(category));
      } else if (selection.startsWith("folder:")) {
        const ids = new Set(selectedFolder?.note_ids ?? []);
        const candidates = selectedFolder?.auto_rule === "daily_standup"
          ? notes.filter(
              (note) => !isScheduleNote(note) && smartFolderVisibleNoteIds.has(note.id)
            )
          : libraryNotes;
        rows = candidates.filter((note) => ids.has(note.id));
      } else rows = libraryNotes;
    }

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
      compareNoteRows(left, right, sortOrder, standupNoteIds, documentsMode)
    );
  }, [
    documentsMode,
    libraryNotes,
    documentNotes,
    inboxDocuments,
    inboxNotes,
    needsFilingNotes,
    query,
    scheduleNotes,
    selectedFolder,
    selection,
    smartFolderVisibleNoteIds,
    sortOrder,
    standupNoteIds,
    notes,
    trashedNotes,
    trashedDocumentNotes,
  ]);

  const meetingRows = useMemo(() => {
    if (documentsMode) return [];
    const rows =
      selection === "meeting-trash"
        ? trashedMeetings
        : selection === "needs-filing"
          ? needsFilingMeetings
          : successfulMeetings;
    const normalizedQuery = query.trim().toLowerCase();
    const filtered = normalizedQuery
      ? rows.filter((meeting) =>
          `${meeting.title} ${meeting.status}`.toLowerCase().includes(normalizedQuery)
        )
      : rows;
    return [...filtered].sort((left, right) => compareMeetingRows(left, right, sortOrder));
  }, [
    documentsMode,
    needsFilingMeetings,
    query,
    selection,
    sortOrder,
    successfulMeetings,
    trashedMeetings,
  ]);

  const needsFilingRows = useMemo<MixedLibraryRow[]>(
    () =>
      [
        ...meetingRows.map((meeting) => ({ kind: "meeting" as const, meeting })),
        ...list.map((note) => ({ kind: "note" as const, note })),
      ].sort((left, right) =>
        compareMixedLibraryRows(left, right, sortOrder, standupNoteIds)
      ),
    [list, meetingRows, sortOrder, standupNoteIds]
  );

  const trashRows = useMemo<MixedLibraryRow[]>(
    () =>
      [
        ...meetingRows.map((meeting) => ({ kind: "meeting" as const, meeting })),
        ...list.map((note) => ({ kind: "note" as const, note })),
      ].sort((left, right) =>
        compareMixedLibraryRows(left, right, sortOrder, standupNoteIds)
      ),
    [list, meetingRows, sortOrder, standupNoteIds]
  );

  const transcriptMeetingIds = useMemo(() => {
    const rows =
      selection === "needs-filing"
        ? needsFilingMeetings
        : successfulMeetings;
    return new Set(rows.map((meeting) => meeting.id));
  }, [needsFilingMeetings, selection, successfulMeetings]);
  const visibleTranscriptHits = useMemo(
    () => transcriptHits.filter((hit) => transcriptMeetingIds.has(hit.meeting_id)),
    [transcriptHits, transcriptMeetingIds]
  );

  const meetingOnlyView = selection === "meetings";
  const needsFilingView = selection === "needs-filing";
  const trashView = selection === "meeting-trash" || selection === "document-trash";

  const currentLabel = useMemo(() => {
    if (selection === "all") return "All items";
    if (selection === "documents") return "Documents";
    if (selection === "inbox") return "Inbox";
    if (selection === "meetings") return "Meetings";
    if (selection === "needs-filing") return "Needs filing";
    if (selection === "schedule") return "Schedule";
    if (selection === "journal") return "Journal";
    if (selection === "meeting-trash") return "Trash";
    if (selection === "document-trash") return "Trash";
    if (selectedFolder) return selectedFolder.name;
    return categories.find((category) => category.id === selection)?.label ?? "Library";
  }, [categories, selectedFolder, selection]);

  async function createDocumentNote() {
    if (creatingNote) return;
    setCreatingNote(true);
    setCreateNoteError(null);
    const document = emptyDocument();
    const documentJson = JSON.stringify(document);
    const activeContext = activeSpace?.name.trim().toLowerCase();
    const context = isFilingContext(activeContext) ? activeContext : filingContext;
    const folderId = selectedFolder?.kind === "folder" ? selectedFolder.id : null;
    try {
      const id = await api.createNoteDocument("", "", documentJson, context, folderId);
      const createdAt = new Date().toISOString();
      setEditingNote(false);
      setOpenNote({
        id,
        title: "",
        raw_text: "",
        document_json: documentJson,
        note_kind: "document",
        source: "text",
        entries: [],
        event_date: easternDay(),
        created_at: createdAt,
        updated_at: createdAt,
        trashed_at: null,
      });
      try {
        await onChanged?.();
      } catch {
        // The document is already open and durable. A later library refresh
        // repairs the index without interrupting writing.
      }
    } catch (error) {
      setCreateNoteError(String(error));
    } finally {
      setCreatingNote(false);
    }
  }

  function selectSpace(space: NoteFolderInfo) {
    setActiveSpaceIdState(space.id);
    localStorage.setItem("noted-active-space", String(space.id));
    const context = space.name.trim().toLowerCase();
    if (isFilingContext(context)) writeFilingContext(context);
    setSelection(documentsMode ? "documents" : "all");
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

  async function moveFolderFromMenu(
    folder: NoteFolderInfo,
    parentId: number | null,
    beforeId: number | null,
    message: string,
    expandParentId?: number
  ) {
    setMenuFolder(null);
    setFolderError(null);
    setFolderMoveNotice(null);
    try {
      await api.moveNoteFolder(folder.id, parentId, beforeId);
      if (expandParentId != null) {
        setExpanded(new Set(expanded).add(expandParentId));
      }
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      showFolderMoveNotice({ kind: "success", message });
    } catch (error) {
      const detail = String(error);
      setFolderError(detail);
      showFolderMoveNotice({
        kind: "error",
        message: `Could not move “${folder.name}”: ${detail}`,
      });
    }
  }

  async function fileOpenNote(folderId: number | null) {
    if (!openNote) return;
    setFiling(true);
    setFilingMsg(null);
    try {
      const receipt = await api.fileNote(openNote.id, folderId);
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      setFilingMsg(folderId == null ? "Filing removed." : `${receipt.reason} Undo is available below.`);
    } catch (error) {
      setFilingMsg(String(error));
    } finally {
      setFiling(false);
    }
  }

  async function fileMeetingNote(meeting: MeetingListRow, folderId: number) {
    if (meeting.note_id == null) return;
    setFilingMeetingId(meeting.id);
    setFolderMoveNotice(null);
    try {
      const receipt = await api.fileNote(meeting.note_id, folderId);
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      const destination = folders.find((folder) => folder.id === folderId);
      showFolderMoveNotice({
        kind: "success",
        undo: receipt,
        message: destination
          ? `Moved “${meeting.title}” to ${filingTargetPath(destination, folders)}.`
          : receipt.reason,
      });
    } catch (error) {
      showFolderMoveNotice({
        kind: "error",
        message: `Could not file “${meeting.title}”: ${String(error)}`,
      });
    } finally {
      setFilingMeetingId(null);
    }
  }

  async function undoFiling(eventId: number) {
    setFiling(true);
    setFilingMsg(null);
    try {
      await api.undoNoteFiling(eventId);
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      setFilingMsg("Previous filing restored.");
      setFolderMoveNotice(null);
    } catch (error) {
      setFilingMsg(String(error));
      showFolderMoveNotice({ kind: "error", message: String(error) });
    } finally {
      setFiling(false);
    }
  }

  function showFolderMoveNotice(notice: FolderMoveNotice) {
    setFolderMoveNotice(notice);
    if (folderNoticeTimer.current != null) window.clearTimeout(folderNoticeTimer.current);
    if (notice.undo) {
      folderNoticeTimer.current = null;
      return;
    }
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
    setTrashDropActive(false);
  }

  function showNoteContextMenu(
    button: HTMLButtonElement,
    target: Omit<NoteContextTarget, "x" | "y">,
    point?: { x: number; y: number }
  ) {
    clearFolderDrag();
    setMenuFolder(null);
    setSpaceMenuOpen(false);
    setNoteMoveMenuOpen(false);
    noteContextReturnFocus.current = button;
    const rect = button.getBoundingClientRect();
    const actionCount = target.trashed
      ? 2
      : target.noteId != null && filingTargets.length > 0
        ? 3
        : 2;
    const menuWidth = 194;
    // Keep every primary action, especially Trash, inside the viewport. The
    // folder destinations scroll independently in the Move to submenu.
    const menuHeight = target.trashed
      ? 10 + actionCount * 34 + (actionCount - 1) * 2
      : 10 + actionCount * 34 + 7 + actionCount * 2;
    const margin = 8;
    const anchorX = point && point.x > 0 ? point.x : rect.left + 18;
    const anchorY = point && point.y > 0 ? point.y : rect.bottom - 2;
    setNoteContextMenu({
      ...target,
      x: Math.max(margin, Math.min(anchorX, window.innerWidth - menuWidth - margin)),
      y: Math.max(margin, Math.min(anchorY, window.innerHeight - menuHeight - margin)),
    });
  }

  function openNoteContextMenu(
    event: ReactMouseEvent<HTMLButtonElement>,
    target: Omit<NoteContextTarget, "x" | "y">
  ) {
    event.preventDefault();
    event.stopPropagation();
    showNoteContextMenu(event.currentTarget, target, {
      x: event.clientX,
      y: event.clientY,
    });
  }

  function openNoteContextMenuFromKeyboard(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    target: Omit<NoteContextTarget, "x" | "y">
  ) {
    if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
    event.preventDefault();
    event.stopPropagation();
    showNoteContextMenu(event.currentTarget, target);
  }

  function beginNotePointer(
    event: ReactPointerEvent<HTMLButtonElement>,
    target: Omit<NoteContextTarget, "x" | "y">,
    dragItem?: LibraryDragItem
  ) {
    if (event.button === 2) {
      event.preventDefault();
      event.stopPropagation();
      showNoteContextMenu(event.currentTarget, target, {
        x: event.clientX,
        y: event.clientY,
      });
      return;
    }
    if (dragItem) beginFolderPointer(event, dragItem);
  }

  function navigateNoteContextMenu(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === "Tab") {
      setNoteContextMenu(null);
      return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const actions = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)'
      )
    );
    if (actions.length === 0) return;
    event.preventDefault();
    const current = actions.indexOf(document.activeElement as HTMLButtonElement);
    const next = current < 0
      ? event.key === "ArrowUp" || event.key === "End"
        ? actions.length - 1
        : 0
      : event.key === "Home"
        ? 0
        : event.key === "End"
          ? actions.length - 1
          : event.key === "ArrowDown"
            ? (current + 1 + actions.length) % actions.length
            : (current - 1 + actions.length) % actions.length;
    actions[next]?.focus();
  }

  function editFromNoteContextMenu() {
    const target = noteContextMenu;
    if (!target || target.trashed) return;
    setNoteContextMenu(null);
    setNoteMoveMenuOpen(false);
    if (target.kind === "meeting") {
      setOpenNote(null);
      setOpenMeeting({ id: target.id });
      return;
    }
    const note = notes.find((candidate) => candidate.id === target.id);
    if (!note) return;
    setOpenMeeting(null);
    setOpenNote(note);
    setEditingNote(false);
  }

  async function moveNoteFromContextMenu(folder: NoteFolderInfo) {
    const target = noteContextMenu;
    if (!target?.noteId || target.trashed) return;
    setNoteContextMenu(null);
    setNoteMoveMenuOpen(false);
    setFolderMoveNotice(null);
    try {
      const receipt = await api.fileNote(target.noteId, folder.id);
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      showFolderMoveNotice({
        kind: "success",
        undo: receipt,
        message: `Moved “${target.label}” to ${filingTargetPath(folder, folders)}.`,
      });
    } catch (error) {
      showFolderMoveNotice({
        kind: "error",
        message: `Could not move “${target.label}”: ${String(error)}`,
      });
    }
  }

  async function refreshAfterTrashChange() {
    await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
    onChanged?.();
  }

  async function runNoteContextAction(
    action: "trash" | "restore" | "delete",
    targetOverride?: Omit<NoteContextTarget, "x" | "y">,
    options: { skipConfirmation?: boolean } = {}
  ): Promise<boolean> {
    const target = targetOverride ?? noteContextMenu;
    if (!target || (action === "trash" && !target.canTrash)) return false;
    setNoteContextMenu(null);

    if (action === "delete" && target.kind === "note") {
      showFolderMoveNotice({
        kind: "error",
        message:
          "Permanent note deletion is paused until every synced device can honor the same purge generation. Leave it in Trash or restore it.",
      });
      return false;
    }

    if (action === "trash" && !options.skipConfirmation) {
      const confirmed = window.confirm(
        `Move “${target.label}” to Trash? You can restore it later.`
      );
      if (!confirmed) {
        window.requestAnimationFrame(() => noteContextReturnFocus.current?.focus());
        return false;
      }
    } else if (action === "delete") {
      const detail = target.kind === "meeting"
        ? "This removes its transcript, summaries, retained media, and generated note."
        : "This removes the note and its saved attachment, if it has one.";
      const confirmed = window.confirm(
        `Permanently delete “${target.label}”? ${detail} This cannot be undone.`
      );
      if (!confirmed) {
        window.requestAnimationFrame(() => noteContextReturnFocus.current?.focus());
        return false;
      }
    }

    setFolderMoveNotice(null);
    try {
      if (target.kind === "meeting") {
        if (action === "trash") await api.meetingTrash(target.id);
        else if (action === "restore") await api.meetingRestore(target.id);
        else await api.meetingDeleteForever(target.id);
      } else if (action === "trash") {
        await api.noteTrash(target.id);
      } else if (action === "restore") {
        await api.noteRestore(target.id);
      } else {
        await api.noteDeleteForever(target.id);
      }
      await refreshAfterTrashChange();
      showFolderMoveNotice({
        kind: "success",
        message:
          action === "trash"
            ? `Moved “${target.label}” to Trash.`
            : action === "restore"
              ? `Restored “${target.label}”.`
              : `Permanently deleted “${target.label}”.`,
      });
      return true;
    } catch (error) {
      showFolderMoveNotice({
        kind: "error",
        message: `Could not ${
          action === "trash" ? "move" : action === "restore" ? "restore" : "delete"
        } “${target.label}”: ${String(error)}`,
      });
      return false;
    }
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

  function pointerIsOverTrash(x: number, y: number): boolean {
    return Boolean(
      document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-trash-drop-target]")
    );
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
    const overTrash = pointerIsOverTrash(event.clientX, event.clientY);
    setTrashDropActive(
      overTrash && active.item.kind === "note" && active.item.trashTarget.canTrash
    );
    setFolderDropTarget(overTrash ? null : folderAtPoint(event.clientX, event.clientY));
  }

  function endFolderPointer(event: ReactPointerEvent<HTMLElement>) {
    const active = activeFolderPointer.current;
    if (!active || active.pointerId !== event.pointerId) return;
    const moved = active.moved;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    const overTrash = pointerIsOverTrash(event.clientX, event.clientY);
    const trashTarget =
      overTrash && active.item.kind === "note" && active.item.trashTarget.canTrash
        ? active.item.trashTarget
        : null;
    const target = overTrash
      ? null
      : folderAtPoint(event.clientX, event.clientY) ?? folderDropTargetRef.current;
    clearFolderDrag();
    if (!moved) return;
    event.preventDefault();
    window.setTimeout(() => {
      suppressRowOpen.current = false;
    }, 0);
    if (trashTarget) {
      void runNoteContextAction("trash", trashTarget, { skipConfirmation: true });
    } else if (target) {
      void performFolderDrop(active.item, target);
    }
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
      let undo: NoteFilingReceipt | undefined;
      if (item.kind === "note") {
        undo = await api.fileNote(item.noteId, target.folder.id);
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
      await Promise.all([loadFolders(), loadMeetings(), loadTranscriptFacets()]);
      showFolderMoveNotice({
        kind: "success",
        undo,
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
    const trashedNote = Boolean(openNote.trashed_at);
    const openItemLabel = isDocumentNote(openNote) ? "Document" : "Capture";
    const openNoteLabel = scheduleNote ? scheduleNoteTitle(openNote) : noteTitle(openNote);
    const openNoteTrashTarget: NoteContextTarget = {
      kind: "note",
      id: openNote.id,
      noteId: openNote.id,
      label: openNoteLabel,
      trashed: true,
      canTrash: false,
      x: 0,
      y: 0,
    };
    const explicitPlacement = filingTargets
      .flatMap((folder) =>
        folder.explicit_filings.map((filing) => ({ folder, filing }))
      )
      .find(({ filing }) => filing.note_id === openNote.id);
    const smartPlacement = explicitPlacement
      ? undefined
      : filingTargets.find(
          (folder) => folder.auto_rule !== "" && folder.note_ids.includes(openNote.id)
        );
    const explicitPlacementUndoIsNoop =
      explicitPlacement?.folder.kind === "space" &&
      explicitPlacement.filing.source === "manual" &&
      explicitPlacement.filing.reason.startsWith("Chosen before saving:");
    const filingTargetLabel = (folder: NoteFolderInfo) => filingTargetPath(folder, folders);

    if (!scheduleNote && !trashedNote) {
      const placement = explicitPlacement ? (
        <div className="note-filed-placement">
          <strong>{filingTargetLabel(explicitPlacement.folder)}</strong>
          <span>{explicitPlacement.filing.reason}</span>
          {explicitPlacement.filing.event_id != null &&
            explicitPlacement.filing.source !== "context" &&
            explicitPlacement.filing.source !== "undo" &&
            !explicitPlacementUndoIsNoop && (
              <button
                type="button"
                onClick={() => void undoFiling(explicitPlacement.filing.event_id!)}
                disabled={filing}
                aria-label={`Undo filing this ${openItemLabel.toLowerCase()} in ${filingTargetLabel(explicitPlacement.folder)}`}
              >
                Undo
              </button>
            )}
        </div>
      ) : smartPlacement ? (
        <div className="note-filed-placement smart">
          <strong>{filingTargetLabel(smartPlacement)}</strong>
          <span>Legacy smart match. Choose a folder to make its home explicit.</span>
        </div>
      ) : (
        <div className="note-filed-placement unfiled">
          <strong>Inbox</strong>
          <span>This {openItemLabel.toLowerCase()} stays in {activeSpaceLabel} until you move it.</span>
        </div>
      );

      return (
        <NoteDocumentEditor
          key={openNote.id}
          note={openNote}
          workspaceLabel={documentsMode ? "Documents" : "Library"}
          itemLabel={openItemLabel}
          metadata={(
            <>
              <span className="note-document-kind">
                {isDocumentNote(openNote) ? (
                  <FileText size={12} aria-hidden="true" />
                ) : (
                  <PenLine size={12} aria-hidden="true" />
                )}
                {openItemLabel}
              </span>
              <span>
                {isDocumentNote(openNote) ? "Edited " : "Saved "}
                {relativeDay(isDocumentNote(openNote) ? noteUpdatedDay(openNote) : openNote.event_date)}
              </span>
              {noteCats(openNote).map((category) => (
                <span key={category}>{category}</span>
              ))}
            </>
          )}
          placement={placement}
          controls={(
            <label className="note-file-select">
              <span className="sr-only">Move {openItemLabel.toLowerCase()} to a folder</span>
              <select
                value=""
                disabled={filing}
                onChange={(event) => {
                  const value = event.target.value;
                  if (value) void fileOpenNote(Number(value));
                }}
              >
                <option value="">Move to…</option>
                {filingTargets
                  .filter((folder) => folder.id !== explicitPlacement?.folder.id)
                  .map((folder) => (
                    <option key={folder.id} value={folder.id}>
                      {filingTargetLabel(folder)}
                    </option>
                  ))}
              </select>
            </label>
          )}
          onBack={() => {
            setEditingNote(false);
            setOpenNote(null);
          }}
          onSaved={async (updated) => {
            setOpenNote(updated);
            await onChanged?.();
          }}
        >
          {filingMsg && <div className="note-filing-message">{filingMsg}</div>}
          {openNote.entries.some(
            (entry) => entry.data && Object.keys(entry.data).length > 0
          ) && (
            <section className="note-detail-entries" aria-label="Extracted context">
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
            </section>
          )}
        </NoteDocumentEditor>
      );
    }

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
              <h2>{openNoteLabel}</h2>
            )}
            <div className="note-detail-meta">
              <span>{relativeDay(openNote.event_date)}</span>
              <span>{openNote.source}</span>
              {noteCats(openNote).map((category) => (
                <span key={category}>{category}</span>
              ))}
            </div>
            {!trashedNote && !scheduleNote && explicitPlacement && (
              <div className="note-filed-placement">
                <strong>{filingTargetLabel(explicitPlacement.folder)}</strong>
                <span>{explicitPlacement.filing.reason}</span>
                {explicitPlacement.filing.event_id != null &&
                  explicitPlacement.filing.source !== "context" &&
                  explicitPlacement.filing.source !== "undo" &&
                  !explicitPlacementUndoIsNoop && (
                    <button
                      type="button"
                      onClick={() => void undoFiling(explicitPlacement.filing.event_id!)}
                      disabled={filing}
                      aria-label={`Undo filing this note in ${filingTargetLabel(explicitPlacement.folder)}`}
                    >
                      Undo
                    </button>
                  )}
              </div>
            )}
            {!trashedNote && !scheduleNote && !explicitPlacement && smartPlacement && (
              <div className="note-filed-placement smart">
                <strong>{filingTargetLabel(smartPlacement)}</strong>
                <span>Legacy smart match. Choose a folder to make its home explicit.</span>
              </div>
            )}
            {!trashedNote && !scheduleNote && !explicitPlacement && !smartPlacement && (
              <div className="note-filed-placement unfiled">
                <strong>Needs filing</strong>
                <span>This legacy note has no Work or Personal context yet.</span>
              </div>
            )}
          </div>
          <div className="note-detail-controls">
            {trashedNote ? (
              <>
                <button
                  className="note-edit-trigger"
                  onClick={async () => {
                    if (await runNoteContextAction("restore", openNoteTrashTarget)) {
                      setOpenNote(null);
                    }
                  }}
                >
                  <RotateCcw size={14} /> Restore
                </button>
                <button
                  className="note-edit-trigger"
                  type="button"
                  disabled
                  title="Permanent deletion will return after synchronized purge protection is available."
                >
                  <Trash2 size={14} /> Kept safely in Trash
                </button>
              </>
            ) : !editingNote && (
              <button
                className="note-edit-trigger"
                onClick={() => {
                  setEditTitle(
                    openNote.title.trim() ||
                      openNoteLabel
                  );
                  setEditBody(openNote.raw_text);
                  setEditError(null);
                  setEditingNote(true);
                }}
              >
                <PenLine size={14} /> Edit
              </button>
            )}
            {!trashedNote && !scheduleNote && !editingNote && (
              <label className="note-file-select">
                <span className="sr-only">File note in a folder</span>
                <select
                  value=""
                  disabled={filing}
                  onChange={(event) => {
                    const value = event.target.value;
                    if (value) fileOpenNote(Number(value));
                  }}
                >
                  <option value="">Move to…</option>
                  {filingTargets
                    .filter((folder) => folder.id !== explicitPlacement?.folder.id)
                    .map((folder) => (
                      <option key={folder.id} value={folder.id}>
                        {filingTargetLabel(folder)}
                      </option>
                    ))}
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
    else {
      setEditingNote(false);
      setOpenNote(note);
    }
  };

  const renderNoteRow = (note: NoteRow) => {
    const inTrash = trashView;
    const linkedMeeting = meetingByNoteId.get(note.id);
    const meetingId = meetingIdOf(note) ?? linkedMeeting?.id ?? null;
    const canMove = !inTrash && !isScheduleNote(note);
    const label = displayedNoteTitle(
      note,
      standupNoteIds,
      meetingByNoteId,
      recurringSeriesKeys
    );
    const contextTarget: Omit<NoteContextTarget, "x" | "y"> = {
      kind: meetingId == null ? "note" : "meeting",
      id: meetingId ?? note.id,
      noteId: isScheduleNote(note) ? null : note.id,
      label,
      trashed: inTrash,
      canTrash:
        meetingId == null ||
        (linkedMeeting?.status !== "recording" && linkedMeeting?.status !== "summarizing"),
    };
    const noteKind = isDocumentNote(note) ? "Document" : "Capture";
    const filedIn = documentsMode
      ? filingTargets.find((folder) =>
          folder.explicit_filings.some((filing) => filing.note_id === note.id)
        )
      : undefined;
    const rowMeta = meetingId != null
      ? "Meeting"
      : documentsMode
        ? `${noteKind} · ${filedIn ? filingTargetPath(filedIn, folders) : `${activeSpaceLabel} · Inbox`}`
        : noteKind;
    return (
    <button
      key={`note:${note.id}`}
      className={`note-row${canMove ? " can-drag" : ""}${
        draggingItem?.kind === "note" && draggingItem.noteId === note.id ? " dragging" : ""
      }`}
      onClick={() => openUnlessDragged(() => open(note))}
      onContextMenu={(event) => openNoteContextMenu(event, contextTarget)}
      onKeyDown={(event) => openNoteContextMenuFromKeyboard(event, contextTarget)}
      onPointerDown={(event) =>
        beginNotePointer(
          event,
          contextTarget,
          canMove
            ? { kind: "note", noteId: note.id, label, trashTarget: contextTarget }
            : undefined
        )
      }
      onPointerMove={canMove ? moveFolderPointer : undefined}
      onPointerUp={canMove ? endFolderPointer : undefined}
      onPointerCancel={canMove ? cancelFolderPointer : undefined}
      title={
        canMove
          ? contextTarget.canTrash
            ? "Drag to a folder or Trash"
            : "Drag to a folder"
          : undefined
      }
      aria-haspopup="menu"
      aria-expanded={
        noteContextMenu?.kind === contextTarget.kind &&
        noteContextMenu.id === contextTarget.id
      }
    >
      {isScheduleNote(note) ? (
        <CalendarDays size={14} className="note-row-icon" />
      ) : meetingId != null ? (
        <AudioLines size={14} className="note-row-icon" />
      ) : isDocumentNote(note) ? (
        <FileText size={14} className="note-row-icon" />
      ) : (
        <PenLine size={14} className="note-row-icon" />
      )}
      <span className="note-row-title">{label}</span>
      {!isScheduleNote(note) && (
        <span className="note-row-categories">{rowMeta}</span>
      )}
      <span className="note-row-date">
        {relativeDay(documentsMode && isDocumentNote(note) ? noteUpdatedDay(note) : note.event_date)}
      </span>
    </button>
    );
  };

  const renderMeetingRow = (meeting: MeetingListRow) => {
    const inTrash = trashView;
    const canMove = !inTrash && meeting.note_id != null;
    const label = displayedMeetingTitle(
      meeting,
      recurringSeriesKeys,
      standupNoteIds,
      notesById
    );
    const contextTarget: Omit<NoteContextTarget, "x" | "y"> = {
      kind: "meeting",
      id: meeting.id,
      noteId: meeting.note_id,
      label,
      trashed: inTrash,
      canTrash: meeting.status !== "recording" && meeting.status !== "summarizing",
    };
    const currentFolderId = meeting.note_id == null
      ? null
      : filingTargets.find((folder) =>
          folder.explicit_filings.some((filing) => filing.note_id === meeting.note_id)
        )?.id ?? null;
    const row = (
      <button
        key={`meeting:${meeting.id}`}
        className={`note-row${
          meeting.note_id != null &&
          draggingItem?.kind === "note" &&
          draggingItem.noteId === meeting.note_id
            ? " dragging"
            : ""
        }${canMove ? " can-drag" : ""}`}
        onClick={() => openUnlessDragged(() => setOpenMeeting({ id: meeting.id }))}
        onContextMenu={(event) => openNoteContextMenu(event, contextTarget)}
        onKeyDown={(event) => openNoteContextMenuFromKeyboard(event, contextTarget)}
        onPointerDown={(event) =>
          beginNotePointer(
            event,
            contextTarget,
            canMove
              ? {
                  kind: "note",
                  noteId: meeting.note_id as number,
                  label,
                  trashTarget: contextTarget,
                }
              : undefined
          )
        }
        onPointerMove={canMove ? moveFolderPointer : undefined}
        onPointerUp={canMove ? endFolderPointer : undefined}
        onPointerCancel={canMove ? cancelFolderPointer : undefined}
        title={
          canMove
            ? contextTarget.canTrash
              ? "Drag to a folder or Trash"
              : "Drag to a folder"
            : undefined
        }
        aria-haspopup="menu"
        aria-expanded={
          noteContextMenu?.kind === "meeting" && noteContextMenu.id === meeting.id
        }
      >
        <AudioLines size={14} className="note-row-icon" />
        <span className="note-row-title">{label}</span>
        <span className="note-row-categories">
          {inTrash
            ? "Meeting · In trash"
            : meeting.status === "recording"
              ? "Meeting · Recording"
              : meeting.status === "summarizing"
                ? "Meeting · Enhancing notes"
                : needsFilingView ||
                    (meeting.route_status === "needs_filing" && currentFolderId == null)
                  ? "Meeting · Needs a folder"
                  : meeting.summary_count > 0
                    ? "Meeting · Notes"
                    : meeting.segment_count > 0
                      ? "Meeting · Transcript"
                      : "Meeting"}
        </span>
        <span className="note-row-date">
          {meeting.started_at
            ? relativeDay(easternDay(new Date(meeting.started_at)))
            : ""}
        </span>
      </button>
    );
    if (!needsFilingView || !canMove) {
      return row;
    }
    return (
      <div key={`meeting:${meeting.id}`} className="note-row-with-action">
        {row}
        <label className="note-row-file-select">
          <span className="sr-only">Move {meeting.title} to a folder</span>
          <select
            value=""
            disabled={filingMeetingId != null}
            aria-label={`Move ${meeting.title} to a folder`}
            onChange={(event) => {
              const value = event.target.value;
              if (value) void fileMeetingNote(meeting, Number(value));
            }}
          >
            <option value="">
              {filingMeetingId === meeting.id ? "Moving…" : "Move to…"}
            </option>
            {filingTargets
              .filter((folder) => folder.id !== currentFolderId)
              .map((folder) => (
                <option key={folder.id} value={folder.id}>
                  {filingTargetPath(folder, folders)}
                </option>
              ))}
          </select>
        </label>
      </div>
    );
  };

  const transcriptSearchActive =
    selectionSearchesTranscripts(selection) && query.trim().length >= 2;
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
    const visibleNoteIds = documentsMode
      ? documentNoteIds
      : folder.auto_rule === "daily_standup"
        ? smartFolderVisibleNoteIds
        : libraryNoteIds;
    const noteCount = Array.from(folderNoteIds.get(folder.id) ?? []).filter((id) =>
      visibleNoteIds.has(id)
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
          {quantified(noteCount, documentsMode ? "document" : "item")}
        </span>
        <ChevronRight size={14} className="folder-row-arrow" />
      </button>
    );
  };

  const renderFolder = (folder: NoteFolderInfo, depth: number): ReactNode => {
    const children = folderChildren.get(folder.id) ?? [];
    const siblings = (folderChildren.get(folder.parent_id) ?? []).filter(
      (item) => item.kind === "folder"
    );
    const siblingIndex = siblings.findIndex((item) => item.id === folder.id);
    const moveParentOptions = folders
      .filter(
        (candidate) =>
          activeSpaceFolderIds.has(candidate.id) &&
          candidate.id !== folder.id &&
          candidate.id !== folder.parent_id &&
          !folderIsInside(candidate.id, folder.id)
      )
      .sort((left, right) => {
        if (left.kind !== right.kind) return left.kind === "space" ? -1 : 1;
        return folderPath(left.id, folders).localeCompare(folderPath(right.id, folders));
      });
    const isExpanded = expanded.has(folder.id);
    const isSelected = selection === `folder:${folder.id}`;
    const visibleNoteIds = documentsMode
      ? documentNoteIds
      : folder.auto_rule === "daily_standup"
        ? smartFolderVisibleNoteIds
        : libraryNoteIds;
    const count = Array.from(folderNoteIds.get(folder.id) ?? []).filter((id) =>
      visibleNoteIds.has(id)
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
          onContextMenu={(event) => {
            event.preventDefault();
            event.stopPropagation();
            clearFolderDrag();
            setNoteContextMenu(null);
            setNoteMoveMenuOpen(false);
            const width = 220;
            const height = 310;
            const margin = 8;
            setMenuFolder(folder.id);
            setFolderContextPoint({
              folderId: folder.id,
              x: Math.max(margin, Math.min(event.clientX, window.innerWidth - width - margin)),
              y: Math.max(margin, Math.min(event.clientY, window.innerHeight - height - margin)),
            });
          }}
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
                onClick={() => {
                  setFolderContextPoint(null);
                  setMenuFolder(menuFolder === folder.id ? null : folder.id);
                }}
                aria-label={`Manage ${folder.name}`}
                aria-expanded={menuFolder === folder.id}
                aria-haspopup="true"
              >
                <MoreHorizontal size={14} />
              </button>
            </>
          )}
        </div>
        {menuFolder === folder.id && (
          <div
            ref={folderMenuRef}
            className={`folder-menu${
              folderContextPoint?.folderId === folder.id ? " context" : ""
            }`}
            style={
              folderContextPoint?.folderId === folder.id
                ? ({
                    "--folder-depth": depth,
                    left: folderContextPoint.x,
                    top: folderContextPoint.y,
                  } as CSSProperties)
                : ({ "--folder-depth": depth } as CSSProperties)
            }
            role="menu"
            aria-label={`Manage ${folder.name}`}
            onContextMenu={(event) => event.preventDefault()}
          >
            <button
              role="menuitem"
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
              role="menuitem"
              onClick={() => {
                setMenuFolder(null);
                setRenaming(folder.id);
                setRenameValue(folder.name);
              }}
            >
              Rename
            </button>
            <button
              role="menuitem"
              disabled={siblingIndex <= 0}
              onClick={() => {
                const previous = siblings[siblingIndex - 1];
                if (!previous) return;
                void moveFolderFromMenu(
                  folder,
                  folder.parent_id,
                  previous.id,
                  `Moved “${folder.name}” up.`
                );
              }}
            >
              Move up
            </button>
            <button
              role="menuitem"
              disabled={siblingIndex < 0 || siblingIndex >= siblings.length - 1}
              onClick={() => {
                if (siblingIndex < 0 || siblingIndex >= siblings.length - 1) return;
                void moveFolderFromMenu(
                  folder,
                  folder.parent_id,
                  siblings[siblingIndex + 2]?.id ?? null,
                  `Moved “${folder.name}” down.`
                );
              }}
            >
              Move down
            </button>
            {moveParentOptions.length > 0 && (
              <label className="folder-menu-move">
                <span>Move to</span>
                <select
                  value=""
                  aria-label={`Move ${folder.name} into another folder`}
                  onChange={(event) => {
                    const target = folders.find(
                      (candidate) => candidate.id === Number(event.target.value)
                    );
                    if (!target) return;
                    const destination = target.kind === "space"
                      ? `${target.name} top level`
                      : `“${target.name}”`;
                    void moveFolderFromMenu(
                      folder,
                      target.id,
                      null,
                      `Moved “${folder.name}” into ${destination}.`,
                      target.kind === "folder" ? target.id : undefined
                    );
                  }}
                >
                  <option value="">Choose a folder…</option>
                  {moveParentOptions.map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.kind === "space"
                        ? `${candidate.name} (top level)`
                        : folderPath(candidate.id, folders)}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <button
              role="menuitem"
              disabled
              title="Folder removal will return after synchronized lifecycle and purge protection is available."
            >
              Remove unavailable
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
    : trashView
      ? trashRows.length
    : needsFilingView
      ? meetingRows.length + list.length
    : meetingOnlyView
      ? meetingRows.length
      : selectedFolder
        ? visibleFolderChildren.length + list.length
        : list.length;
  const visibleCount = listedCount + (transcriptSearchActive ? visibleTranscriptHits.length : 0);
  const canCreateNote = documentsMode && !trashView;
  const countLabel = transcriptSearchActive
    ? `${visibleCount}${visibleTranscriptHits.length === 200 ? "+" : ""} ${
        visibleCount === 1 ? "result" : "results"
      }`
    : selectedFolder
    ? [
        visibleFolderChildren.length > 0
          ? `${visibleFolderChildren.length} ${visibleFolderChildren.length === 1 ? "folder" : "folders"}`
          : "",
        list.length > 0 ? quantified(list.length, documentsMode ? "document" : "item") : "",
      ]
        .filter(Boolean)
        .join(" · ") || (documentsMode ? "0 documents" : "0 items")
    : needsFilingView
      ? `${visibleCount} ${visibleCount === 1 ? "item" : "items"}`
    : trashView
      ? quantified(visibleCount, documentsMode ? "document" : "item")
    : meetingOnlyView
      ? `${visibleCount} ${visibleCount === 1 ? "meeting" : "meetings"}`
    : selection === "documents"
      ? `${visibleCount} ${visibleCount === 1 ? "document" : "documents"}`
    : `${visibleCount} ${visibleCount === 1 ? "item" : "items"}`;
  const emptyMessage = transcriptSearchPending
    ? "Searching transcripts…"
    : transcriptSearchActive && transcriptFiltersActive
      ? "No transcript lines match this search and filter combination."
    : query
    ? "Nothing matches."
    : selection === "meetings"
      ? "No meetings recorded yet."
      : selection === "documents"
        ? "No documents yet. Create one when you want a focused place to write."
      : selection === "inbox"
        ? documentsMode
          ? `No documents are saved directly to ${activeSpaceLabel}.`
          : `No items are saved directly to ${activeSpaceLabel}.`
        : selection === "needs-filing"
          ? "Everything has a folder."
          : selection === "schedule"
            ? "No schedules saved yet."
            : selection === "meeting-trash" || selection === "document-trash"
              ? documentsMode ? "Document Trash is empty." : "Trash is empty."
              : selectedFolder?.auto_rule === "daily_standup"
                ? "No stand-up notes yet. New ones will be filed here automatically."
                : selectedFolder
                  ? documentsMode
                    ? "This folder has no documents yet. Create one here or move one in."
                    : "This folder is empty. Move an item here when it belongs."
                  : "Your library is empty.";

  return (
    <div className={`notes-view ${documentsMode ? "documents-view" : "library-view"}`} data-tauri-drag-region>
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
      {noteContextMenu && (
        <div
          ref={noteContextMenuRef}
          className={`note-context-menu${
            noteContextMenu.x > window.innerWidth - 420 ? " submenu-left" : ""
          }${noteContextMenu.y > window.innerHeight / 2 ? " submenu-up" : ""}`}
          role="menu"
          tabIndex={-1}
          aria-label={`Actions for ${noteContextMenu.label}`}
          style={{
            left: noteContextMenu.x,
            top: noteContextMenu.y,
            "--note-context-submenu-room-below": `${Math.max(
              34,
              window.innerHeight - noteContextMenu.y - 45
            )}px`,
          } as CSSProperties}
          onKeyDown={navigateNoteContextMenu}
          onContextMenu={(event) => event.preventDefault()}
        >
          {noteContextMenu.trashed ? (
            <>
              <button
                type="button"
                role="menuitem"
                onClick={() => void runNoteContextAction("restore")}
              >
                <RotateCcw size={14} aria-hidden="true" />
                <span>Restore</span>
              </button>
              {noteContextMenu.kind === "meeting" ? (
                <button
                  type="button"
                  role="menuitem"
                  className="danger"
                  onClick={() => void runNoteContextAction("delete")}
                >
                  <Trash2 size={14} aria-hidden="true" />
                  <span>Delete permanently</span>
                </button>
              ) : (
                <button
                  type="button"
                  role="menuitem"
                  disabled
                  title="Permanent deletion will return after synchronized purge protection is available."
                >
                  <Trash2 size={14} aria-hidden="true" />
                  <span>Kept safely in Trash</span>
                </button>
              )}
            </>
          ) : (
            <>
              <button type="button" role="menuitem" onClick={editFromNoteContextMenu}>
                {noteContextMenu.kind === "meeting" ? (
                  <AudioLines size={14} aria-hidden="true" />
                ) : (
                  <PenLine size={14} aria-hidden="true" />
                )}
                <span>
                  {noteContextMenu.kind === "meeting"
                    ? "Open meeting"
                    : notesById.get(noteContextMenu.noteId ?? -1)?.note_kind === "document"
                      ? "Open document"
                      : "Open capture"}
                </span>
              </button>
              {noteContextMenu.noteId != null && filingTargets.length > 0 && (
                <>
                  <button
                    type="button"
                    role="menuitem"
                    className="note-context-move-trigger"
                    aria-haspopup="menu"
                    aria-expanded={noteMoveMenuOpen}
                    onPointerEnter={() => setNoteMoveMenuOpen(true)}
                    onClick={() => setNoteMoveMenuOpen((open) => !open)}
                    onKeyDown={(event) => {
                      if (event.key !== "ArrowRight") return;
                      event.preventDefault();
                      setNoteMoveMenuOpen(true);
                      window.requestAnimationFrame(() => {
                        noteContextMenuRef.current
                          ?.querySelector<HTMLButtonElement>(".note-context-submenu button")
                          ?.focus();
                      });
                    }}
                  >
                    <Folder size={14} aria-hidden="true" />
                    <span>Move to</span>
                    <ChevronRight className="note-context-chevron" size={13} aria-hidden="true" />
                  </button>
                  {noteMoveMenuOpen && (
                    <div
                      className="note-context-submenu"
                      role="menu"
                      aria-label={`Move ${noteContextMenu.label} to`}
                      onKeyDown={(event) => {
                        if (event.key !== "ArrowLeft") return;
                        event.preventDefault();
                        setNoteMoveMenuOpen(false);
                        noteContextMenuRef.current
                          ?.querySelector<HTMLButtonElement>(".note-context-move-trigger")
                          ?.focus();
                      }}
                    >
                      {filingTargets.map((folder) => (
                        <button
                          key={folder.id}
                          type="button"
                          role="menuitem"
                          onClick={() => void moveNoteFromContextMenu(folder)}
                        >
                          <Folder size={14} aria-hidden="true" />
                          <span>{filingTargetPath(folder, folders)}</span>
                        </button>
                      ))}
                    </div>
                  )}
                </>
              )}
              <div className="note-context-separator" role="separator" />
              <button
                type="button"
                role="menuitem"
                className="danger"
                disabled={!noteContextMenu.canTrash}
                onClick={() => void runNoteContextAction("trash")}
              >
                <Trash2 size={14} aria-hidden="true" />
                <span>
                  {noteContextMenu.canTrash ? "Move to Trash" : "Finish recording first"}
                </span>
              </button>
            </>
          )}
        </div>
      )}
      {documentsMode && <div className="notes-library-shell document-files-shell">
        {!libraryOpen && (
          <button
            className="notes-library-toggle notes-library-toggle-reveal icon-btn"
            onClick={() => setLibraryOpen(true)}
            title="Show files sidebar"
            aria-label="Show files sidebar"
            aria-controls="document-files-rail"
            aria-expanded={false}
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
        )}

        {libraryOpen && (
          <aside id="document-files-rail" className="spaces document-files" aria-label="Document files">
            <div className="spaces-scroll">
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
                  <div className="space-switcher-menu" role="menu" aria-label="Switch document space">
                    {rootSpaces.map((space) => (
                      <button
                        key={space.id}
                        role="menuitemradio"
                        aria-checked={space.id === activeSpace?.id}
                        className={space.id === activeSpace?.id ? "on" : ""}
                        onClick={() => selectSpace(space)}
                      >
                        <span>
                          <strong>{space.name}</strong>
                          <small>{space.name} documents</small>
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </div>

              <div className="document-files-label">Files</div>
              <nav className="library-main document-file-views" aria-label="Document views">
                <button
                  className={selection === "documents" ? "on" : ""}
                  onClick={() => setSelection("documents")}
                >
                  <FileText size={14} />
                  <span className="space-label">All documents</span>
                  <span className="space-n">{documentNotes.length}</span>
                </button>
                <button
                  className={selection === "inbox" ? "on" : ""}
                  onClick={() => setSelection("inbox")}
                  title={`Documents saved to ${activeSpaceLabel} without a folder`}
                >
                  <Inbox size={14} />
                  <span className="space-label">Inbox</span>
                  <span className="space-n">{inboxDocuments.length}</span>
                </button>
              </nav>

              <div className="library-section-head document-folder-head">
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
              {topLevelFolders.length === 0 && !creating && (
                <p className="document-folders-empty">Create a folder when this space needs structure.</p>
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
              {folderError && <div className="folder-error">{folderError}</div>}
            </div>

            <div className="spaces-trash-wrap">
              <button
                className={`spaces-trash${selection === "document-trash" ? " on" : ""}${
                  draggingItem?.kind === "note" ? " drop-ready" : ""
                }${trashDropActive ? " drop-active" : ""}`}
                onClick={() => setSelection("document-trash")}
                title="Open Document Trash. Drag a document here to remove it."
                aria-label={`Open Document Trash, ${quantified(trashedDocumentNotes.length, "document")}. Drag a document here to remove it.`}
                data-trash-drop-target
              >
                <Trash2 size={14} />
                <span className="spaces-trash-copy">
                  <strong>Trash</strong>
                  <small>{trashDropActive ? "Release to move here" : "Deleted documents"}</small>
                </span>
                <span className="space-n">{trashedDocumentNotes.length}</span>
              </button>
            </div>
            <button
              className="notes-library-toggle notes-library-toggle-collapse icon-btn"
              onClick={() => {
                setSpaceMenuOpen(false);
                setLibraryOpen(false);
              }}
              title="Hide files sidebar"
              aria-label="Hide files sidebar"
              aria-controls="document-files-rail"
              aria-expanded={true}
            >
              <ChevronLeft size={15} aria-hidden="true" />
            </button>
          </aside>
        )}
      </div>}
      {!documentsMode && <div className="notes-library-shell">
        {!libraryOpen && (
          <button
            className="notes-library-toggle notes-library-toggle-reveal icon-btn"
            onClick={() => setLibraryOpen(true)}
            title="Show library sidebar"
            aria-label="Show library sidebar"
            aria-controls="library-navigation-rail"
            aria-expanded={false}
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
        )}

        {libraryOpen && (
          <aside id="library-navigation-rail" className="spaces" aria-label="Library navigation">
          <div className="spaces-scroll">
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
              <div className="space-switcher-menu" role="menu" aria-label="Switch context">
                {rootSpaces.map((space) => {
                  const isWork = space.name.toLowerCase() === "work";
                  const isPersonal = space.name.toLowerCase() === "personal";
                  const label = isWork ? "Work" : isPersonal ? "Personal" : space.name;
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
                        <small>{isPersonal ? "Personal library" : "Work library"}</small>
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
              <FileText size={14} />
              <span className="space-label">All items</span>
              <span className="space-n">{libraryNotes.length}</span>
            </button>
            <button
              className={selection === "inbox" ? "on" : ""}
              onClick={() => setSelection("inbox")}
              title={`Items saved to ${activeSpaceLabel} without a folder`}
            >
              <Inbox size={14} />
              <span className="space-label">Inbox</span>
              <span className="space-n">{inboxNotes.length}</span>
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
              className={selection === "needs-filing" ? "on" : ""}
              onClick={() => setSelection("needs-filing")}
            >
              <FolderOpen size={14} />
              <span className="space-label">Needs filing</span>
              <span className="space-n">
                {needsFilingMeetings.length + needsFilingNotes.length}
              </span>
            </button>
            {SHOW_SCHEDULE_IN_LIBRARY && (
              <button
                className={selection === "schedule" ? "on" : ""}
                onClick={() => setSelection("schedule")}
              >
                <CalendarDays size={14} />
                <span className="space-label">Schedule</span>
                <span className="space-n">{scheduleNotes.length}</span>
              </button>
            )}
            {SHOW_JOURNAL && (
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
            )}
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
          {SHOW_TOPICS_IN_LIBRARY && (
            <div className="library-topics">
              <span id="library-topics-description" className="sr-only">
                Topics are automatic labels that can overlap. A note stays in its folder.
              </span>
              <button
                className="library-topics-toggle"
                onClick={() => setTopicsOpen(!topicsOpen)}
                aria-expanded={topicsOpen}
                aria-describedby="library-topics-description"
                aria-controls="library-topics-region"
              >
                {topicsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <span>Topics</span>
                <small>Automatic</small>
              </button>
              <div id="library-topics-region" hidden={!topicsOpen}>
                <p className="library-topics-description" aria-hidden="true">
                  Automatic labels that can overlap. Folders hold notes.
                </p>
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
              </div>
            </div>
          )}

          {folderError && <div className="folder-error">{folderError}</div>}
          </div>
          <div className="spaces-trash-wrap">
            <button
              className={`spaces-trash${selection === "meeting-trash" ? " on" : ""}${
                draggingItem?.kind === "note" && draggingItem.trashTarget.canTrash
                  ? " drop-ready"
                  : ""
              }${trashDropActive ? " drop-active" : ""}${
                draggingItem?.kind === "note" && !draggingItem.trashTarget.canTrash
                  ? " drop-blocked"
                  : ""
              }`}
              onClick={() => setSelection("meeting-trash")}
              title="Open Trash. Drag an item here to remove it."
              aria-label={`Open Trash, ${trashedMeetings.length + trashedNotes.length} items. Drag an item here to remove it.`}
              data-trash-drop-target
            >
              <Trash2 size={14} />
              <span className="spaces-trash-copy">
                <strong>Trash</strong>
                <small>
                  {trashDropActive
                    ? "Release to move here"
                    : draggingItem?.kind === "note" && !draggingItem.trashTarget.canTrash
                      ? "Finish recording first"
                      : "Drop items here"}
                </small>
              </span>
              <span className="space-n">{trashedMeetings.length + trashedNotes.length}</span>
            </button>
          </div>
          <button
            className="notes-library-toggle notes-library-toggle-collapse icon-btn"
            onClick={() => {
              setSpaceMenuOpen(false);
              setLibraryOpen(false);
            }}
            title="Hide library sidebar"
            aria-label="Hide library sidebar"
            aria-controls="library-navigation-rail"
            aria-expanded={true}
          >
            <ChevronLeft size={15} aria-hidden="true" />
          </button>
          </aside>
        )}
      </div>}

      {folderMoveNotice && (
        <div
          className={`folder-move-notice ${folderMoveNotice.kind}`}
          role={folderMoveNotice.kind === "error" ? "alert" : "status"}
          aria-live="polite"
        >
          <span>{folderMoveNotice.message}</span>
          {folderMoveNotice.undo && (
            <button
              type="button"
              onClick={() => void undoFiling(folderMoveNotice.undo!.event_id)}
              disabled={filing}
              aria-label={`Undo ${folderMoveNotice.message}`}
            >
              Undo
            </button>
          )}
          <button
            type="button"
            className="dismiss"
            onClick={() => setFolderMoveNotice(null)}
            aria-label="Dismiss filing message"
          >
            <X size={12} />
          </button>
        </div>
      )}

      <main className="notes-list">
        {documentsMode ? (
          <header className="documents-masthead">
            <div className="documents-masthead-copy">
              <div className="documents-breadcrumb">
                {trashView ? (
                  "All spaces"
                ) : (
                  <>
                    {activeSpaceLabel}
                    {selectedFolderParentWithinSpace
                      ? ` / ${selectedFolderParentWithinSpace}`
                      : ""}
                  </>
                )}
              </div>
              <h1>{currentLabel}</h1>
              <p>
                {trashView
                  ? "Deleted documents stay here until you restore them."
                  : selectedFolder
                    ? "Documents and subfolders filed here."
                    : selection === "inbox"
                      ? `Documents saved directly to ${activeSpaceLabel}, before you choose a folder.`
                      : "Write here, then shape your own folder structure as the work grows."}
              </p>
            </div>
            <div className="documents-masthead-actions">
              <span>{countLabel}</span>
              {canCreateNote && (
                <button
                  className="notes-new-note"
                  type="button"
                  onClick={() => void createDocumentNote()}
                  disabled={creatingNote}
                >
                  <Plus size={14} aria-hidden="true" />
                  <span>{creatingNote ? "Creating…" : "New document"}</span>
                </button>
              )}
            </div>
          </header>
        ) : (
        <div className="notes-context">
          <div>
            <div className="notes-breadcrumb">
              {needsFilingView || trashView ? (
                "All spaces"
              ) : (
                <>
                  {activeSpaceLabel}
                  {selectedFolderParentWithinSpace
                    ? ` / ${selectedFolderParentWithinSpace}`
                    : ""}
                </>
              )}
            </div>
            <h1>{currentLabel}</h1>
            {selectedFolder?.auto_rule === "daily_standup" && (
              <p>Stand-up notes are filed here automatically.</p>
            )}
            {selection === "inbox" && (
              <p>
                Items saved to {activeSpaceLabel} without a folder. Move them only
                when you want more organization.
              </p>
            )}
          </div>
          <div className="notes-context-actions">
            <span className="notes-context-count">{countLabel}</span>
            {canCreateNote && (
              <button
                className="notes-new-note"
                type="button"
                onClick={() => void createDocumentNote()}
                disabled={creatingNote}
              >
                <Plus size={14} aria-hidden="true" />
                <span>{creatingNote ? "Creating…" : "New document"}</span>
              </button>
            )}
          </div>
        </div>)}
        {createNoteError && (
          <div className="notes-create-error" role="alert">{createNoteError}</div>
        )}
        <div className="notes-list-head">
          <label className="notes-search">
            <Search size={14} />
            <input
              placeholder={
                documentsMode
                  ? "Search documents…"
                  : selection === "all"
                  ? "Search your library…"
                  : selection === "meetings"
                    ? "Search meeting names and transcripts…"
                    : selection === "needs-filing"
                      ? "Search unfiled items and transcripts…"
                    : `Search ${currentLabel}…`
              }
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <div className="search-tool-buttons">
            {selectionSearchesTranscripts(selection) && (
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
                aria-label={`Sort ${documentsMode ? "documents" : "library items"} by`}
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

        {selectionSearchesTranscripts(selection) && renderSearchInstrument()}
        {renderTranscriptResults()}

        {visibleCount === 0 ? (
          <p className="quiet-empty">{emptyMessage}</p>
        ) : transcriptSearchActive && transcriptFiltersActive ? null : trashView ? (
          trashRows.map((row) =>
            row.kind === "meeting"
              ? renderMeetingRow(row.meeting)
              : renderNoteRow(row.note)
          )
        ) : meetingOnlyView ? (
          meetingRows.map(renderMeetingRow)
        ) : needsFilingView ? (
          needsFilingRows.map((row) =>
            row.kind === "meeting"
              ? renderMeetingRow(row.meeting)
              : renderNoteRow(row.note)
          )
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

export function LibraryView(props: LibraryWorkspaceProps) {
  return <LibraryWorkspace {...props} mode="library" />;
}

export function DocumentsView(props: LibraryWorkspaceProps) {
  return <LibraryWorkspace {...props} mode="documents" />;
}
