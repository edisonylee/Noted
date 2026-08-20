import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  connectMobileDeepLinkConsumer,
  MOBILE_DEEP_LINK_ERROR_EVENT,
  MOBILE_OPEN_NOTE_EVENT,
} from "./mobileDeepLinks";
import "./MobileShell.css";

export { MOBILE_OPEN_NOTE_EVENT } from "./mobileDeepLinks";

export const MOBILE_NOTES_COMMANDS = {
  health: "mobile_health",
  workspace: "get_mobile_notes_workspace",
  listLegacy: "list_mobile_notes",
  create: "create_mobile_note",
  update: "update_mobile_note",
  trash: "trash_mobile_note",
  trashLegacy: "delete_mobile_note",
  restore: "restore_mobile_note",
  file: "file_mobile_note",
  undoFiling: "undo_mobile_note_filing",
  resolveConflict: "resolve_mobile_note_conflict",
  sync: "mobile_sync_now",
} as const;

export type LifecycleState = "active" | "trashed";
export type NoteSyncState = "local" | "not_enrolled" | "pending" | "syncing" | "synced" | "offline" | "error";
export type WorkspaceSyncState = NoteSyncState;
type ConflictResolution = "keepAsCopy" | "useRemote";

export type MobileNote = {
  recordId: string;
  title: string;
  body: string;
  createdAt: number;
  updatedAt: number;
  folderId: string | null;
  folderName: string | null;
  lifecycleState: LifecycleState;
  needsFiling: boolean;
  syncState: NoteSyncState;
  conflictOf: string | null;
  hasOpenConflict: boolean;
  readOnly: boolean;
};

export type MobileFolder = {
  folderId: string;
  name: string;
  parentId: string | null;
  path: string | null;
  noteCount: number;
};

export type MobileCapabilities = {
  filing: boolean;
  undoFiling: boolean;
  trash: boolean;
  restore: boolean;
  conflictResolution: boolean;
  legacyTrash: boolean;
};

export type MobileWorkspace = {
  notes: MobileNote[];
  folders: MobileFolder[];
  capabilities: MobileCapabilities;
  sync: {
    state: WorkspaceSyncState;
    pendingCount: number;
    lastSyncedAt: number | null;
  };
  counts: {
    inbox: number | null;
    needsFiling: number | null;
    trash: number | null;
  };
};

type LibraryLocation =
  | { kind: "inbox" }
  | { kind: "needsFiling" }
  | { kind: "folder"; folderId: string; label: string }
  | { kind: "trash" };

type WorkspaceView = LibraryLocation["kind"] | "all";

type WorkspaceRequest = {
  query: string | null;
  view: WorkspaceView;
  folderId: string | null;
};

type NoteDraft = Pick<
  MobileNote,
  "title" | "body" | "folderId" | "folderName" | "lifecycleState" | "needsFiling" | "syncState" | "conflictOf" | "hasOpenConflict" | "readOnly"
> & {
  recordId: string | null;
  originalTitle: string;
  originalBody: string;
};

type MobileNoteWire = Omit<Partial<MobileNote>, "lifecycleState" | "syncState"> &
  Pick<MobileNote, "recordId" | "title" | "body" | "createdAt" | "updatedAt"> & {
    lifecycleState?: string;
    syncState?: string;
  };

type InvokeCommand = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export type MobileNotesClient = {
  workspace(request: WorkspaceRequest): Promise<MobileWorkspace>;
  create(title: string, body: string): Promise<MobileNote>;
  update(recordId: string, title: string, body: string): Promise<MobileNote>;
  trash(recordId: string, useLegacyCommand: boolean): Promise<void>;
  restore(recordId: string): Promise<MobileNote>;
  file(recordId: string, folderId: string): Promise<MobileNote>;
  undoFiling(recordId: string): Promise<MobileNote>;
  resolveConflict(recordId: string, resolution: ConflictResolution): Promise<MobileNote>;
  sync(manualAddress?: string): Promise<void>;
};

const EMPTY_WORKSPACE: MobileWorkspace = {
  notes: [],
  folders: [],
  capabilities: {
    filing: false,
    undoFiling: false,
    trash: false,
    restore: false,
    conflictResolution: false,
    legacyTrash: true,
  },
  sync: { state: "local", pendingCount: 0, lastSyncedAt: null },
  counts: { inbox: null, needsFiling: null, trash: null },
};

function messageFrom(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

function isMissingCommand(reason: unknown, command: string) {
  const message = messageFrom(reason).toLowerCase();
  return message.includes(command.toLowerCase()) && (
    message.includes("not found") ||
    message.includes("unknown") ||
    message.includes("does not exist") ||
    message.includes("not allowed")
  );
}

function normalizeLifecycle(value?: string): LifecycleState {
  return value === "trashed" || value === "trash" ? "trashed" : "active";
}

function normalizeSyncState(value: string | undefined, localOnly: boolean): NoteSyncState {
  if (value === "notEnrolled") return "not_enrolled";
  if (value === "restorePending" || value === "restore_pending") return "pending";
  if (value === "clean" || value === "acknowledged") return "synced";
  if (value === "sending") return "syncing";
  if (value === "conflict") return "error";
  if (value === "local" || value === "not_enrolled" || value === "pending" || value === "syncing" || value === "synced" || value === "offline" || value === "error") return value;
  return localOnly ? "local" : "pending";
}

function normalizeNote(note: MobileNoteWire, localOnly = false): MobileNote {
  const legacyConflict = note.syncState === "conflict";
  return {
    recordId: note.recordId,
    title: note.title,
    body: note.body,
    createdAt: note.createdAt,
    updatedAt: note.updatedAt,
    folderId: note.folderId ?? null,
    folderName: note.folderName ?? null,
    lifecycleState: normalizeLifecycle(note.lifecycleState),
    needsFiling: note.needsFiling ?? !note.folderId,
    syncState: normalizeSyncState(note.syncState, localOnly),
    conflictOf: note.conflictOf ?? (legacyConflict ? note.recordId : null),
    hasOpenConflict: Boolean(note.hasOpenConflict ?? legacyConflict),
    readOnly: Boolean(note.readOnly),
  };
}

type MobileWorkspaceWire = {
  notes?: MobileNoteWire[];
  folders?: MobileFolder[];
  capabilities?: Partial<Omit<MobileCapabilities, "legacyTrash">>;
  counts?: Partial<Record<keyof MobileWorkspace["counts"], number>>;
  sync?: Partial<Omit<MobileWorkspace["sync"], "state">> & { state?: string };
};

function normalizeWorkspace(raw: MobileWorkspaceWire): MobileWorkspace {
  const notes = (raw.notes ?? []).map((note) => normalizeNote(note));
  return {
    notes,
    folders: (raw.folders ?? []).map((folder) => ({
      folderId: folder.folderId,
      name: folder.name,
      parentId: folder.parentId ?? null,
      path: folder.path ?? null,
      noteCount: folder.noteCount ?? 0,
    })),
    capabilities: {
      filing: Boolean(raw.capabilities?.filing),
      undoFiling: Boolean(raw.capabilities?.undoFiling),
      trash: Boolean(raw.capabilities?.trash),
      restore: Boolean(raw.capabilities?.restore),
      conflictResolution: Boolean(raw.capabilities?.conflictResolution),
      legacyTrash: false,
    },
    sync: {
      state: normalizeSyncState(raw.sync?.state, false),
      pendingCount: raw.sync?.pendingCount ?? 0,
      lastSyncedAt: raw.sync?.lastSyncedAt ?? null,
    },
    counts: {
      inbox: raw.counts?.inbox ?? null,
      needsFiling: raw.counts?.needsFiling ?? null,
      trash: raw.counts?.trash ?? null,
    },
  };
}

export function createMobileNotesClient(invokeCommand: InvokeCommand): MobileNotesClient {
  let workspaceCommandAvailable: boolean | null = null;

  async function legacyWorkspace(request: WorkspaceRequest): Promise<MobileWorkspace> {
    const [legacyNotes, health] = await Promise.all([
      invokeCommand<MobileNoteWire[]>(
        MOBILE_NOTES_COMMANDS.listLegacy,
        { query: request.query },
      ),
      invokeCommand<{ sync?: WorkspaceSyncState }>(MOBILE_NOTES_COMMANDS.health).catch(() => ({ sync: "local" as const })),
    ]);
    const allNotes = legacyNotes.map((note) => normalizeNote(note, true));
    const notes = allNotes.filter((note) => {
      if (request.view === "all") return true;
      if (request.view === "trash") return note.lifecycleState === "trashed";
      if (request.view === "needsFiling") return note.lifecycleState === "active" && note.needsFiling;
      if (request.view === "folder") return note.lifecycleState === "active" && note.folderId === request.folderId;
      return note.lifecycleState === "active";
    });
    return {
      ...EMPTY_WORKSPACE,
      notes,
      sync: { state: health.sync ?? "local", pendingCount: 0, lastSyncedAt: null },
      counts: { inbox: allNotes.length, needsFiling: allNotes.length, trash: 0 },
    };
  }

  return {
    async workspace(request) {
      if (workspaceCommandAvailable === false) return legacyWorkspace(request);
      try {
        const result = await invokeCommand<MobileWorkspaceWire>(
          MOBILE_NOTES_COMMANDS.workspace,
          { query: request.query, view: request.view, folderId: request.folderId },
        );
        workspaceCommandAvailable = true;
        return normalizeWorkspace(result);
      } catch (reason) {
        if (!isMissingCommand(reason, MOBILE_NOTES_COMMANDS.workspace)) throw reason;
        workspaceCommandAvailable = false;
        return legacyWorkspace(request);
      }
    },
    create(title, body) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.create, { title, body }).then((note) => normalizeNote(note));
    },
    update(recordId, title, body) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.update, { recordId, title, body }).then((note) => normalizeNote(note));
    },
    trash(recordId, useLegacyCommand) {
      return invokeCommand<void>(useLegacyCommand ? MOBILE_NOTES_COMMANDS.trashLegacy : MOBILE_NOTES_COMMANDS.trash, { recordId });
    },
    restore(recordId) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.restore, { recordId }).then((note) => normalizeNote(note));
    },
    file(recordId, folderId) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.file, { recordId, folderId }).then((note) => normalizeNote(note));
    },
    undoFiling(recordId) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.undoFiling, { recordId }).then((note) => normalizeNote(note));
    },
    resolveConflict(recordId, resolution) {
      return invokeCommand<MobileNoteWire>(MOBILE_NOTES_COMMANDS.resolveConflict, { recordId, resolution }).then((note) => normalizeNote(note));
    },
    sync(manualAddress) {
      return invokeCommand<void>(MOBILE_NOTES_COMMANDS.sync, { manualAddress: manualAddress ?? null });
    },
  };
}

const mobileNotesClient = createMobileNotesClient((command, args) => invoke(command, args));

function noteTitle(draft: Pick<NoteDraft, "title" | "body">) {
  const explicitTitle = draft.title.trim();
  if (explicitTitle) return explicitTitle;
  return draft.body.split("\n").find((line) => line.trim())?.trim().slice(0, 80) || "Untitled note";
}

function notePreview(note: MobileNote) {
  return note.body.replace(/\s+/g, " ").trim() || "No additional text";
}

function formatUpdated(timestamp: number) {
  const date = new Date(timestamp);
  if (!Number.isFinite(date.getTime())) return "Unknown";
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return new Intl.DateTimeFormat(undefined, sameDay
    ? { hour: "numeric", minute: "2-digit" }
    : { month: "short", day: "numeric" }).format(date);
}

function formatLastSync(timestamp: number) {
  const date = new Date(timestamp);
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(date);
}

function syncSummary(workspace: MobileWorkspace) {
  const { state, pendingCount, lastSyncedAt } = workspace.sync;
  if (state === "not_enrolled" || state === "local") return "Only on this iPhone";
  if (state === "offline") return pendingCount ? `${pendingCount} waiting for your Mac` : "Mac unavailable";
  if (state === "syncing") return "Syncing with your Mac";
  if (state === "pending") return `${pendingCount || 1} waiting for your Mac`;
  if (state === "error") return "Sync needs attention";
  if (lastSyncedAt) return `Up to date at ${formatLastSync(lastSyncedAt)}`;
  return "Up to date on both devices";
}

function noteStatus(note: Pick<MobileNote, "syncState" | "conflictOf" | "hasOpenConflict" | "readOnly">) {
  if (note.readOnly) return "Read-only mirror";
  if (note.hasOpenConflict) return "Conflict needs review";
  if (note.conflictOf) return "Conflict copy";
  if (note.syncState === "pending") return "Waiting for Mac";
  if (note.syncState === "syncing") return "Sending to Mac";
  if (note.syncState === "synced") return "On Mac and iPhone";
  if (note.syncState === "offline") return "Saved; Mac unavailable";
  if (note.syncState === "error") return "Sync needs attention";
  return "Only on this iPhone";
}

function locationTitle(location: LibraryLocation) {
  if (location.kind === "needsFiling") return "Needs filing";
  if (location.kind === "folder") return location.label;
  if (location.kind === "trash") return "Trash";
  return "Inbox";
}

function draftFromNote(note: MobileNote): NoteDraft {
  return {
    recordId: note.recordId,
    title: note.title,
    body: note.body,
    folderId: note.folderId,
    folderName: note.folderName,
    lifecycleState: note.lifecycleState,
    needsFiling: note.needsFiling,
    syncState: note.syncState,
    conflictOf: note.conflictOf,
    hasOpenConflict: note.hasOpenConflict,
    readOnly: note.readOnly,
    originalTitle: note.title,
    originalBody: note.body,
  };
}

function newDraft(): NoteDraft {
  return {
    recordId: null,
    title: "",
    body: "",
    folderId: null,
    folderName: null,
    lifecycleState: "active",
    needsFiling: true,
    syncState: "local",
    conflictOf: null,
    hasOpenConflict: false,
    readOnly: false,
    originalTitle: "",
    originalBody: "",
  };
}

function BackIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 5.2c-2.2 2.3-4.5 4.5-6.8 6.9 2 2 4.3 4.5 6.5 6.7" /></svg>;
}

function SearchIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M16.1 10.4c0 3.3-2.4 5.8-5.7 5.8-3.2 0-5.6-2.2-5.6-5.5 0-3.5 2.3-5.9 5.7-5.9 3.2 0 5.6 2.3 5.6 5.6Z" /><path d="m14.7 14.8 4.6 4.4" /></svg>;
}

function ComposeIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M13.2 5.2H6.5c-1.2 0-2 .9-2 2.1v10.2c0 1.2.8 2 2.1 2h10.2c1.2 0 2-.8 2-2.1v-6.7" /><path d="m10.8 13.3 1.1-3.8 5.9-5.8c.8-.8 1.8-.7 2.5 0 .7.8.6 1.8-.1 2.5l-5.8 5.9-3.6 1.2Z" /></svg>;
}

function IndexIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 6.5c4-.5 9.3-.5 14 .1M5.2 11.9c3.6-.4 8.7-.3 13.5.1M5 17.4c4.2-.4 9.5-.4 13.8.1" /></svg>;
}

function CloseIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6.4 6.1 11.2 11.7M17.8 6.4 6.2 17.6" /></svg>;
}

function TrashIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5.4 7.4c4.2.2 8.9.1 13.2 0M9 7.2 9.2 5h5.9l.2 2.4M7.4 7.7c.2 3.5.5 7.2.8 10.7 2.4.2 5 .2 7.5 0 .4-3.5.7-7.2.9-10.7M10.2 10.5l.2 5M13.9 10.4l-.1 5.1" /></svg>;
}

function RestoreIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.4 8.2V4.6M6.3 8.1l3.8-.1" /><path d="M6.8 7.2a7.1 7.1 0 1 1-1.4 7.2" /></svg>;
}

function FolderIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3.8 7.1c3.1-.2 5.6-.1 7.6.1l1.5 1.8c2.4-.1 4.8 0 7.4.2-.2 3.2-.5 6.5-1.1 9.5-4.9.4-9.8.3-14.5 0-.5-4.1-.8-7.9-.9-11.6Z" /></svg>;
}

function useModalFocus(onClose: () => void, returnFocusId?: string) {
  const dialogRef = useRef<HTMLElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    const focusable = () => Array.from(dialog.querySelectorAll<HTMLElement>(
      "button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex='-1'])",
    ));
    focusable()[0]?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab") return;

      const controls = focusable();
      if (!controls.length) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.body.style.overflow = previousOverflow;
      (returnFocusId ? document.getElementById(returnFocusId) : previousFocus)?.focus();
    };
  }, []);

  return dialogRef;
}

function NoteState({ note }: { note: MobileNote }) {
  const location = note.lifecycleState === "trashed"
    ? "Trash"
    : note.folderName ?? (note.needsFiling ? "Needs filing" : "Inbox");
  return (
    <span className="note-row__state" data-state={note.hasOpenConflict || note.conflictOf ? "conflict" : note.syncState}>
      <span>{location}</span>
      <span aria-hidden="true">·</span>
      <span>{noteStatus(note)}</span>
    </span>
  );
}

function LibraryIndex({
  location,
  workspace,
  onChoose,
  onSync,
  syncing,
  onClose,
}: {
  location: LibraryLocation;
  workspace: MobileWorkspace;
  onChoose: (location: LibraryLocation) => void;
  onSync: (manualAddress?: string) => void;
  syncing: boolean;
  onClose: () => void;
}) {
  const dialogRef = useModalFocus(onClose, "mobile-notes-library-button");

  function selected(kind: LibraryLocation["kind"], folderId?: string) {
    return location.kind === kind && (kind !== "folder" || (location.kind === "folder" && location.folderId === folderId));
  }

  const canOpenSpaces = workspace.capabilities.filing || workspace.capabilities.undoFiling;
  const canOpenTrash = workspace.capabilities.restore;
  const enrolled = workspace.sync.state !== "local" && workspace.sync.state !== "not_enrolled";

  return (
    <div className="library-drawer-layer">
      <button className="library-drawer__scrim" type="button" tabIndex={-1} aria-label="Close notebook index" onClick={onClose} />
      <aside ref={dialogRef} className="library-drawer" id="mobile-notes-library" role="dialog" aria-modal="true" aria-label="Notebook index" tabIndex={-1}>
        <header className="library-drawer__header">
          <div>
            <strong>Notes</strong>
            <span>{syncSummary(workspace)}</span>
          </div>
          <button className="bare-icon" type="button" onClick={onClose} aria-label="Close notebook index">
            <CloseIcon />
          </button>
        </header>

        <nav className="library-drawer__nav" aria-label="Note collections">
          <button type="button" aria-current={selected("inbox") ? "page" : undefined} onClick={() => onChoose({ kind: "inbox" })}>
            <span>Inbox</span>{workspace.counts.inbox !== null && <span>{workspace.counts.inbox}</span>}
          </button>
          <button type="button" aria-current={selected("needsFiling") ? "page" : undefined} onClick={() => onChoose({ kind: "needsFiling" })}>
            <span>Needs filing</span>{workspace.counts.needsFiling !== null && <span>{workspace.counts.needsFiling}</span>}
          </button>

          <div className="library-drawer__section">
            <p>Spaces</p>
            {canOpenSpaces && workspace.folders.length ? workspace.folders.map((folder) => (
              <button
                type="button"
                key={folder.folderId}
                className="library-drawer__folder"
                aria-current={selected("folder", folder.folderId) ? "page" : undefined}
                onClick={() => onChoose({ kind: "folder", folderId: folder.folderId, label: folder.name })}
              >
                <span>{folder.path || folder.name}</span><span>{folder.noteCount}</span>
              </button>
            )) : <span className="library-drawer__empty">{canOpenSpaces ? "No spaces on this iPhone yet." : "Spaces become available after this notebook is enrolled."}</span>}
          </div>

          {canOpenTrash && (
            <button type="button" aria-current={selected("trash") ? "page" : undefined} onClick={() => onChoose({ kind: "trash" })}>
              <span>Trash</span>{workspace.counts.trash !== null && <span>{workspace.counts.trash}</span>}
            </button>
          )}

          {enrolled && (
            <div className="library-drawer__section library-drawer__connection">
              <p>Mac connection</p>
              <button type="button" disabled={syncing} onClick={() => onSync()}>
                <span>{syncing ? "Looking for your Mac…" : "Sync now"}</span>
              </button>
              <button
                type="button"
                disabled={syncing}
                onClick={() => {
                  const address = window.prompt("Mac address", "192.168.1.2:43123")?.trim();
                  if (address) onSync(address);
                }}
              >
                <span>Connect by address</span>
              </button>
              <span className="library-drawer__empty">Use a numeric private address only. Your saved pairing still verifies the Mac.</span>
            </div>
          )}
        </nav>
      </aside>
    </div>
  );
}

function FolderSheet({
  folders,
  currentFolderId,
  busy,
  onChoose,
  onClose,
}: {
  folders: MobileFolder[];
  currentFolderId: string | null;
  busy: boolean;
  onChoose: (folder: MobileFolder) => void;
  onClose: () => void;
}) {
  const dialogRef = useModalFocus(onClose, "mobile-note-file-button");

  return (
    <div className="folder-sheet-layer">
      <button className="folder-sheet__scrim" type="button" tabIndex={-1} onClick={onClose} aria-label="Cancel filing" />
      <section ref={dialogRef} id="mobile-note-folder-sheet" className="folder-sheet" role="dialog" aria-modal="true" aria-labelledby="folder-sheet-title" tabIndex={-1}>
        <header>
          <div>
            <h2 id="folder-sheet-title">{currentFolderId ? "Move note" : "File note"}</h2>
            <p>Choose a space on your Mac and iPhone.</p>
          </div>
          <button className="bare-icon" type="button" onClick={onClose} aria-label="Cancel filing"><CloseIcon /></button>
        </header>
        <div className="folder-sheet__list">
          {folders.filter((folder) => folder.folderId !== currentFolderId).map((folder) => (
            <button type="button" key={folder.folderId} onClick={() => onChoose(folder)} disabled={busy}>
              <FolderIcon /><span>{folder.path || folder.name}</span>
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}

function NoteEditor({
  draft,
  workspace,
  activity,
  error,
  onChange,
  onClose,
  onSave,
  onTrash,
  onRestore,
  onFile,
  onUndoFiling,
  onResolveConflict,
}: {
  draft: NoteDraft;
  workspace: MobileWorkspace;
  activity: string | null;
  error: string | null;
  onChange: (draft: NoteDraft) => void;
  onClose: () => void;
  onSave: () => void;
  onTrash: () => void;
  onRestore: () => void;
  onFile: (folder: MobileFolder) => void;
  onUndoFiling: () => void;
  onResolveConflict: (resolution: ConflictResolution) => void;
}) {
  const titleRef = useRef<HTMLTextAreaElement>(null);
  const editorRef = useRef<HTMLElement>(null);
  const [showFolders, setShowFolders] = useState(false);
  const busy = activity !== null;
  const trashed = draft.lifecycleState === "trashed";
  const readOnly = draft.readOnly;
  const hasOpenConflict = draft.hasOpenConflict;
  const dirty = draft.title !== draft.originalTitle || draft.body !== draft.originalBody;
  const canFile = !readOnly && !hasOpenConflict && !trashed && draft.recordId !== null && workspace.capabilities.filing && workspace.folders.some((folder) => folder.folderId !== draft.folderId);
  const canUndoFiling = !readOnly && !hasOpenConflict && !trashed && draft.recordId !== null && draft.folderId !== null && workspace.capabilities.undoFiling;
  const canTrash = !readOnly && !hasOpenConflict && !trashed && draft.recordId !== null && (workspace.capabilities.trash || workspace.capabilities.legacyTrash);
  const canRestore = !readOnly && !hasOpenConflict && trashed && draft.recordId !== null && workspace.capabilities.restore;

  useEffect(() => {
    if (draft.recordId === null) titleRef.current?.focus();
  }, [draft.recordId]);

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
    <main ref={editorRef} className="note-editor" id="mobile-note-editor">
      <div className="note-editor__content" inert={showFolders || undefined} aria-hidden={showFolders || undefined}>
        <header className="note-editor__toolbar">
        <button className="bare-icon note-editor__back" type="button" onClick={onClose} aria-label="Close note">
          <BackIcon />
        </button>
        <span className="note-editor__save-state" aria-live="polite">
          {activity === "saving" ? "Saving locally" : dirty ? "Unsaved changes" : noteStatus(draft)}
        </span>
        {!trashed && !readOnly && !hasOpenConflict ? (
          <button id="mobile-note-save" className="text-action" type="button" onClick={onSave} disabled={busy}>
            {activity === "saving" ? "Saving" : "Done"}
          </button>
        ) : (
          <span className="note-editor__trash-label">{readOnly ? "Read-only" : hasOpenConflict ? "Needs review" : "In Trash"}</span>
        )}
        </header>

        {(hasOpenConflict || draft.conflictOf) && (
          <section className="conflict-copy" aria-label="Sync conflict">
            <div>
              <strong>{hasOpenConflict ? "This note has a sync conflict." : "This is a retained conflict copy."}</strong>
              <p>{hasOpenConflict ? "Your iPhone version stays safe while you decide." : "It remains separate from the resolved original."}</p>
            </div>
            {hasOpenConflict && !readOnly && workspace.capabilities.conflictResolution && draft.recordId && (
              <div className="conflict-copy__actions">
                <button type="button" onClick={() => onResolveConflict("keepAsCopy")} disabled={busy}>Keep this as a copy</button>
                <button type="button" className="danger-text" onClick={() => onResolveConflict("useRemote")} disabled={busy}>Use Mac version</button>
              </div>
            )}
          </section>
        )}

        <section className="note-editor__page">
          <p className="note-editor__filing-state">
            {readOnly ? "Read-only mirror from its source" : trashed ? "Restorable from Trash" : draft.folderName ? `Filed in ${draft.folderName}` : "Needs filing"}
          </p>
          <textarea
            ref={titleRef}
            id="mobile-note-title"
            className="note-editor__title"
            rows={2}
            value={draft.title}
            onChange={(event) => onChange({ ...draft, title: event.target.value })}
            placeholder="Title"
            aria-label="Note title"
            readOnly={trashed || readOnly || hasOpenConflict}
          />
          <textarea
            id="mobile-note-body"
            className="note-editor__body"
            value={draft.body}
            onChange={(event) => onChange({ ...draft, body: event.target.value })}
            placeholder="Start writing…"
            aria-label="Note body"
            readOnly={trashed || readOnly || hasOpenConflict}
          />
          {error && <p className="inline-error" role="alert">{error}</p>}
        </section>

        {(canFile || canUndoFiling || canTrash || canRestore) && (
          <footer className="note-editor__footer" aria-label="Note actions">
            <div className="note-editor__filing-actions">
              {canFile && (
                <button id="mobile-note-file-button" type="button" onClick={() => setShowFolders(true)} disabled={busy || dirty} title={dirty ? "Save this note before filing it" : undefined} aria-haspopup="dialog" aria-controls={showFolders ? "mobile-note-folder-sheet" : undefined}>
                  {draft.folderId ? "Move" : "File"}
                </button>
              )}
              {canUndoFiling && <button type="button" onClick={onUndoFiling} disabled={busy || dirty} title={dirty ? "Save this note before changing its filing" : undefined}>Undo filing</button>}
            </div>
            {canTrash && (
              <button id="mobile-note-trash" className="bare-icon danger-icon" type="button" onClick={onTrash} disabled={busy || dirty} title={dirty ? "Save this note before moving it to Trash" : undefined} aria-label="Move note to Trash">
                <TrashIcon />
              </button>
            )}
            {canRestore && (
              <button id="mobile-note-restore" className="restore-action" type="button" onClick={onRestore} disabled={busy}>
                <RestoreIcon /> Restore
              </button>
            )}
          </footer>
        )}
      </div>

      {showFolders && (
        <FolderSheet
          folders={workspace.folders}
          currentFolderId={draft.folderId}
          busy={busy}
          onChoose={(folder) => {
            onFile(folder);
            setShowFolders(false);
          }}
          onClose={() => setShowFolders(false)}
        />
      )}
    </main>
  );
}

export function MobileShell({ client = mobileNotesClient }: { client?: MobileNotesClient } = {}) {
  const [workspace, setWorkspace] = useState<MobileWorkspace>(EMPTY_WORKSPACE);
  const [location, setLocation] = useState<LibraryLocation>({ kind: "inbox" });
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<NoteDraft | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [activity, setActivity] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const request = useMemo<WorkspaceRequest>(() => ({
    query: query.trim() || null,
    view: location.kind,
    folderId: location.kind === "folder" ? location.folderId : null,
  }), [location, query]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const timer = window.setTimeout(() => {
      client.workspace(request)
        .then((result) => {
          if (!cancelled) {
            setWorkspace(result);
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
  }, [client, query, request]);

  async function refreshWorkspace() {
    const result = await client.workspace(request);
    setWorkspace(result);
    return result;
  }

  async function syncNow(manualAddress?: string) {
    if (activity) return;
    setActivity("syncing");
    setError(null);
    try {
      await client.sync(manualAddress);
      const refreshed = await refreshWorkspace();
      setAnnouncement(syncSummary(refreshed));
    } catch (reason) {
      setError(`Couldn’t sync with your Mac. ${messageFrom(reason)}`);
    } finally {
      setActivity(null);
    }
  }

  async function openNoteByRecordId(recordId: string) {
    const note = workspace.notes.find((candidate) => candidate.recordId === recordId);
    if (note) {
      setError(null);
      setDraft(draftFromNote(note));
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const allNotes = await client.workspace({ query: null, view: "all", folderId: null });
      const requestedNote = allNotes.notes.find((candidate) => candidate.recordId === recordId);
      if (requestedNote) {
        const nextLocation: LibraryLocation = requestedNote.lifecycleState === "trashed"
          ? { kind: "trash" }
          : requestedNote.folderId
            ? { kind: "folder", folderId: requestedNote.folderId, label: requestedNote.folderName || "Space" }
            : requestedNote.needsFiling
              ? { kind: "needsFiling" }
              : { kind: "inbox" };
        setQuery("");
        setLocation(nextLocation);
        setWorkspace(allNotes);
        setDraft(draftFromNote(requestedNote));
        return;
      }
      setError("That note is not available in this notebook.");
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    const openFromDeepLink = (event: Event) => {
      const recordId = (event as CustomEvent<{ recordId?: string }>).detail?.recordId;
      if (!recordId || draft?.recordId === recordId) return;
      if (activity) {
        setError("Finish the current note action before opening that link.");
        return;
      }
      const dirty = draft && (draft.title !== draft.originalTitle || draft.body !== draft.originalBody);
      if (dirty && !window.confirm("Discard these changes and open the linked note?")) return;
      void openNoteByRecordId(recordId);
    };
    const reportDeepLinkError = (event: Event) => {
      const message = (event as CustomEvent<{ message?: string }>).detail?.message;
      setError(message || "That note link could not be opened safely.");
    };
    window.addEventListener(MOBILE_OPEN_NOTE_EVENT, openFromDeepLink);
    window.addEventListener(MOBILE_DEEP_LINK_ERROR_EVENT, reportDeepLinkError);
    const disconnect = connectMobileDeepLinkConsumer();
    return () => {
      disconnect();
      window.removeEventListener(MOBILE_OPEN_NOTE_EVENT, openFromDeepLink);
      window.removeEventListener(MOBILE_DEEP_LINK_ERROR_EVENT, reportDeepLinkError);
    };
  });

  function closeEditor() {
    if (!draft || activity) return;
    const dirty = draft.title !== draft.originalTitle || draft.body !== draft.originalBody;
    if (dirty && !window.confirm("Discard the changes on this iPhone?")) return;
    setError(null);
    setDraft(null);
  }

  async function saveDraft() {
    if (!draft || draft.lifecycleState === "trashed" || draft.readOnly || draft.hasOpenConflict) return;
    if (!draft.title.trim() && !draft.body.trim()) {
      setDraft(null);
      return;
    }
    if (draft.recordId !== null && draft.title === draft.originalTitle && draft.body === draft.originalBody) {
      setDraft(null);
      return;
    }

    setActivity("saving");
    setError(null);
    try {
      const title = noteTitle(draft);
      const recordId = draft.recordId;
      if (recordId === null) {
        await client.create(title, draft.body);
      } else {
        await client.update(recordId, title, draft.body);
      }
      if (recordId === null) {
        const needsFilingWorkspace = await client.workspace({ query: null, view: "needsFiling", folderId: null });
        setQuery("");
        setLocation({ kind: "needsFiling" });
        setWorkspace(needsFilingWorkspace);
      } else {
        await refreshWorkspace();
      }
      setAnnouncement("Saved on this iPhone.");
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  async function trashDraft() {
    if (!draft?.recordId || draft.lifecycleState === "trashed" || draft.readOnly || draft.hasOpenConflict) return;
    if (draft.title !== draft.originalTitle || draft.body !== draft.originalBody) {
      setError("Save or discard your changes before moving this note to Trash.");
      return;
    }
    if (!window.confirm(`Move “${noteTitle(draft)}” to Trash? You can restore it later.`)) return;

    setActivity("trashing");
    setError(null);
    try {
      const target = { recordId: draft.recordId };
      await client.trash(target.recordId, workspace.capabilities.legacyTrash);
      await refreshWorkspace();
      setAnnouncement("Moved to Trash.");
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  async function restoreDraft() {
    if (!draft?.recordId || draft.lifecycleState !== "trashed" || draft.readOnly || draft.hasOpenConflict || !workspace.capabilities.restore) return;
    setActivity("restoring");
    setError(null);
    try {
      await client.restore(draft.recordId);
      await refreshWorkspace();
      setAnnouncement("Restored from Trash.");
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  async function fileDraft(folder: MobileFolder) {
    if (!draft?.recordId || draft.readOnly || draft.hasOpenConflict || !workspace.capabilities.filing) return;
    if (draft.title !== draft.originalTitle || draft.body !== draft.originalBody) {
      setError("Save this note before changing where it is filed.");
      return;
    }
    setActivity("filing");
    setError(null);
    try {
      const filedNote = await client.file(draft.recordId, folder.folderId);
      await refreshWorkspace();
      setDraft(draftFromNote(filedNote));
      setAnnouncement(`Filed in ${folder.name}.`);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  async function undoFiling() {
    if (!draft?.recordId || draft.readOnly || draft.hasOpenConflict || !workspace.capabilities.undoFiling) return;
    if (draft.title !== draft.originalTitle || draft.body !== draft.originalBody) {
      setError("Save this note before changing where it is filed.");
      return;
    }
    setActivity("unfiling");
    setError(null);
    try {
      const unfiledNote = await client.undoFiling(draft.recordId);
      await refreshWorkspace();
      setDraft(draftFromNote(unfiledNote));
      setAnnouncement(unfiledNote.folderName ? `Returned to ${unfiledNote.folderName}.` : "Returned to Needs filing.");
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  async function resolveConflict(resolution: ConflictResolution) {
    if (!draft?.recordId || !draft.hasOpenConflict || draft.readOnly || !workspace.capabilities.conflictResolution) return;
    if (resolution === "useRemote" && !window.confirm("Use the Mac version? This iPhone working branch will leave the note list, but it remains retained in conflict history and evidence.")) return;
    setActivity("resolving");
    setError(null);
    try {
      await client.resolveConflict(draft.recordId, resolution);
      await refreshWorkspace();
      setAnnouncement(resolution === "keepAsCopy" ? "Kept this version as a separate note." : "Using the Mac version.");
      setDraft(null);
    } catch (reason) {
      setError(messageFrom(reason));
    } finally {
      setActivity(null);
    }
  }

  if (draft) {
    return (
      <NoteEditor
        draft={draft}
        workspace={workspace}
        activity={activity}
        error={error}
        onChange={setDraft}
        onClose={closeEditor}
        onSave={saveDraft}
        onTrash={trashDraft}
        onRestore={restoreDraft}
        onFile={fileDraft}
        onUndoFiling={undoFiling}
        onResolveConflict={resolveConflict}
      />
    );
  }

  const title = locationTitle(location);
  const count = workspace.notes.length;
  const emptyTitle = query ? "No matching notes" : location.kind === "trash" ? "Trash is empty" : location.kind === "needsFiling" ? "Nothing needs filing" : location.kind === "folder" ? "This space is quiet" : "A clear page";
  const emptyBody = query ? "Try a different word or phrase." : location.kind === "trash" ? "Notes you move here can be restored." : location.kind === "needsFiling" ? "New notes wait here until you choose a space." : location.kind === "folder" ? "Move a note here when it belongs." : "Write something here. It stays on this iPhone.";

  return (
    <main className="notes-screen" id="mobile-notes-screen">
      <div className="notes-screen__content" inert={drawerOpen || undefined} aria-hidden={drawerOpen || undefined}>
        <header className="notes-header">
        <button
          id="mobile-notes-library-button"
          className="bare-icon notes-header__index"
          type="button"
          onClick={() => setDrawerOpen(true)}
          aria-label="Open notebook index"
          aria-expanded={drawerOpen}
          aria-controls="mobile-notes-library"
        >
          <IndexIcon />
        </button>
        <div className="notes-header__title">
          <h1>{title}</h1>
          <p><span>{loading ? "Opening notebook" : `${count} ${count === 1 ? "note" : "notes"}`}</span><span aria-hidden="true"> · </span><span>{syncSummary(workspace)}</span></p>
        </div>
        <button
          id="mobile-notes-compose-button"
          className="bare-icon notes-header__compose"
          type="button"
          onClick={() => setDraft(newDraft())}
          aria-label="Create note"
        >
          <ComposeIcon />
        </button>
        </header>

        <div className="search-field">
        <SearchIcon />
        <label className="sr-only" htmlFor="mobile-notes-search">Search {title}</label>
        <input
          id="mobile-notes-search"
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={`Search ${title.toLowerCase()}`}
          autoCapitalize="none"
          enterKeyHint="search"
        />
        {query && <button type="button" onClick={() => setQuery("")} aria-label="Clear search"><CloseIcon /></button>}
        </div>

        {location.kind === "needsFiling" && !query && <p className="view-context">New notes stay searchable while they wait for a space.</p>}
        {location.kind === "trash" && !query && <p className="view-context">Nothing is removed permanently from your phone here.</p>}

        {error && <p className="library-error" role="alert">Couldn’t open your local notes. {error}</p>}

        {!loading && !error && workspace.notes.length === 0 ? (
          <section className="empty-library">
            <div className="empty-library__mark" aria-hidden="true"><span /><span /><span /></div>
            <h2>{emptyTitle}</h2>
            <p>{emptyBody}</p>
            {!query && location.kind === "inbox" && (
              <button type="button" onClick={() => setDraft(newDraft())}>Create your first note</button>
            )}
          </section>
        ) : (
          <section className="note-list" aria-label={`${title} notes`} aria-busy={loading}>
            {workspace.notes.map((note) => (
              <button
                className="note-row"
                type="button"
                key={note.recordId}
                data-record-id={note.recordId}
                onClick={() => openNoteByRecordId(note.recordId)}
              >
                <span className="note-row__time">{formatUpdated(note.updatedAt)}</span>
                <span className="note-row__content">
                  <strong>{note.title}</strong>
                  <span className="note-row__preview">{notePreview(note)}</span>
                  <NoteState note={note} />
                </span>
              </button>
            ))}
          </section>
        )}

        <p className="sr-only" role="status" aria-live="polite">{announcement}</p>
      </div>

      {drawerOpen && (
        <LibraryIndex
          location={location}
          workspace={workspace}
          onChoose={(nextLocation) => {
            setLocation(nextLocation);
            setDrawerOpen(false);
          }}
          onSync={(manualAddress) => void syncNow(manualAddress)}
          syncing={activity === "syncing"}
          onClose={() => setDrawerOpen(false)}
        />
      )}
    </main>
  );
}
