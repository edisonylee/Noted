import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CalendarDays,
  Bold,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleEllipsis,
  CloudSun,
  FileText,
  Filter,
  Folder,
  Home,
  ImagePlus,
  Inbox,
  Italic,
  Laptop,
  ListChecks,
  List,
  ListOrdered,
  Menu,
  Mic,
  Paperclip,
  Pilcrow,
  Plus,
  Search,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
  Underline,
  Video,
  AudioWaveform,
  X,
  Link,
} from "lucide-react";
import type {
  MobileNote,
  MobileNotesClient,
  MobileWorkspace,
  WorkspaceSyncState,
} from "./MobileShell";
import "./DesktopMatchedMobile.css";

type PrimaryTab = "home" | "schedule" | "calendar" | "notes";
type NotesView = "all" | "inbox" | "meetings" | "needsFiling" | "trash";

type PairingStatus = {
  state: string;
  confirmation: { receiptId: string; verificationCode: string; grantedScopes: string[] } | null;
  activation: unknown | null;
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
    legacyTrash: false,
  },
  sync: { state: "local", pendingCount: 0, lastSyncedAt: null },
  counts: { inbox: null, needsFiling: null, trash: null },
};

const SCHEDULE = [
  { start: "8:00", end: "8:15 AM", title: "Daily Stand Up", duration: "", live: false, video: true },
  { start: "8:30", end: "9:30 AM", title: "gym", duration: "1h", live: false },
  { start: "9:45", end: "11:30 AM", title: "Work block", duration: "1h 45m", live: false },
  { start: "11:45", end: "12:45 PM", title: "Get to office, work", duration: "1h", live: false },
  { start: "12:45", end: "1:30 PM", title: "lunch", duration: "45m", live: false },
  { start: "2:00", end: "4:30 PM", title: "Google x a16z Speedrun", duration: "2h 30m", live: true },
  { start: "4:30", end: "6:45 PM", title: "work block", duration: "2h 15m", live: false },
];

const CALENDAR_DAYS = [
  { weekday: "SUN", day: 16 },
  { weekday: "MON", day: 17 },
  { weekday: "TUE", day: 18 },
  { weekday: "WED", day: 19 },
  { weekday: "THU", day: 20, active: true },
  { weekday: "FRI", day: 21 },
  { weekday: "SAT", day: 22 },
];

const TASKS = [
  "E2E Ad personalization design pass",
  "influencer ads go straight to cloned websites",
  "E2E Email Marketing",
  "Set up Pupsday Email Marketing",
];

function syncCopy(state: WorkspaceSyncState, pendingCount: number) {
  if (state === "local" || state === "not_enrolled") return "Connect Mac";
  if (state === "syncing") return "Syncing";
  if (state === "offline") return pendingCount ? `${pendingCount} waiting` : "Mac offline";
  if (state === "error") return "Sync needs attention";
  if (state === "pending") return `${pendingCount || 1} waiting`;
  return "Mac up to date";
}

function noteSubtitle(note: MobileNote) {
  if (note.folderName) return note.folderName;
  if (note.needsFiling) return "Needs filing";
  return "notes";
}

function NoteEditor({ note, onClose }: { note: MobileNote; onClose: () => void }) {
  return (
    <section className="dm-note-detail" aria-label={note.title}>
      <header className="dm-note-detail__bar">
        <button type="button" onClick={onClose} aria-label="Back to notes"><ChevronLeft /></button>
        <span>{note.folderName || "Work"}</span>
        <button type="button" aria-label="More note actions"><CircleEllipsis /></button>
      </header>
      <div className="dm-note-detail__content">
        <p className="dm-eyebrow">{noteSubtitle(note)}</p>
        <h1>{note.title}</h1>
        <p className="dm-note-detail__meta">Updated {new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(new Date(note.updatedAt))}</p>
        <div className="dm-note-detail__body">{note.body || "No additional text"}</div>
      </div>
    </section>
  );
}

function BottomNav({ tab, onChange }: { tab: PrimaryTab; onChange: (tab: PrimaryTab) => void }) {
  const items: Array<{ tab: PrimaryTab; label: string; icon: typeof Home }> = [
    { tab: "home", label: "Home", icon: Home },
    { tab: "schedule", label: "Schedule", icon: ListChecks },
    { tab: "calendar", label: "Calendar", icon: CalendarDays },
    { tab: "notes", label: "Notes", icon: FileText },
  ];
  return (
    <nav className="dm-bottom-nav" aria-label="Primary">
      {items.map((item) => {
        const Icon = item.icon;
        return (
          <button
            type="button"
            key={item.tab}
            className={tab === item.tab ? "active" : ""}
            aria-current={tab === item.tab ? "page" : undefined}
            onClick={() => onChange(item.tab)}
          >
            <Icon />
            <span>{item.label}</span>
          </button>
        );
      })}
    </nav>
  );
}

function HomeScreen({
  workspace,
  capture,
  setCapture,
  saving,
  onCapture,
  onSync,
  onConnect,
}: {
  workspace: MobileWorkspace;
  capture: string;
  setCapture: (value: string) => void;
  saving: boolean;
  onCapture: () => void;
  onSync: () => void;
  onConnect: () => void;
}) {
  return (
    <section className="dm-screen dm-home">
      <header className="dm-brandbar">
        <strong>noted<span>.</span></strong>
        <button type="button" aria-label="Settings"><Settings /></button>
      </header>
      <div className="dm-weatherbar">
        <span><CloudSun /><strong>65°</strong> Overcast</span>
        <span>Thu, Aug 20</span>
        <span>H 67° L 54°</span>
        <span>San Francisco</span>
      </div>
      <div className="dm-home__body">
        <h1>Hi Edison</h1>
        <form className="dm-capture" onSubmit={(event) => { event.preventDefault(); onCapture(); }}>
          <label htmlFor="dm-capture-input">Capture a note</label>
          <textarea
            id="dm-capture-input"
            rows={2}
            value={capture}
            onChange={(event) => setCapture(event.target.value)}
            placeholder="What’s on your mind?"
          />
          <div>
            <button type="button" aria-label="Attach a photo"><Paperclip /></button>
            <span>File in</span>
            <button type="button" className="dm-file-target">Work <ChevronDown /></button>
            <button type="button" aria-label="Dictate"><Mic /></button>
            <button type="submit" className="dm-send" disabled={saving || !capture.trim()} aria-label="Save note"><ChevronRight /></button>
          </div>
        </form>
        <p className="dm-section-label">UP NEXT</p>
        <button type="button" className="dm-home-row">
          <CalendarDays />
          <span><small>Tomorrow · 8:00 AM</small><strong>Daily Stand Up</strong></span>
          <ChevronRight />
        </button>
        <p className="dm-section-label">RECENT</p>
        <button type="button" className="dm-home-row">
          <AudioWaveform />
          <span><strong>Aug 20 · Daily Stand Up</strong><small>Meeting note · Today</small></span>
          <CircleEllipsis />
        </button>
      </div>
      <button type="button" className="dm-sync-strip" onClick={workspace.sync.state === "local" || workspace.sync.state === "not_enrolled" ? onConnect : onSync}>
        <Laptop />
        <span>{syncCopy(workspace.sync.state, workspace.sync.pendingCount)}</span>
        <small>{workspace.sync.state === "synced" ? "Updated just now" : "Same Wi-Fi required"}</small>
      </button>
    </section>
  );
}

function PairingSheet({ onClose, onPaired }: { onClose: () => void; onPaired: () => void }) {
  const [pairingCode, setPairingCode] = useState("");
  const [manualAddress, setManualAddress] = useState("");
  const [status, setStatus] = useState<PairingStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const needsDiscard = [
    "live native identity requires recovery or explicit discard",
    "direct-sync endpoint is unavailable",
    "no private-LAN endpoint candidates",
    "a different invitation cannot replace the durable pairing transcript",
    "could not be cleared automatically",
  ].some((message) => error?.includes(message));

  async function connect() {
    if (!pairingCode.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const parsed = JSON.parse(pairingCode.trim()) as { invitationJson?: string; address?: string };
      if (!parsed.invitationJson) throw new Error("This pairing code is incomplete.");
      const endpointAddress = (manualAddress.trim() || parsed.address || "").trim();
      if (!endpointAddress) throw new Error("This pairing code has no Mac address.");
      // Retain the authenticated invitation's endpoint for the approval poll.
      // Bonjour is only a fallback and may be unavailable on managed networks.
      setManualAddress(endpointAddress);
      const next = await invoke<PairingStatus>("mobile_pairing_connect_fixture", {
        invitationJson: parsed.invitationJson,
        manualAddress: endpointAddress,
      });
      setStatus(next);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function finish() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<PairingStatus>("mobile_pairing_poll_fixture", {
        manualAddress: manualAddress.trim() || null,
      });
      setStatus(next);
      if (next.activation || next.state === "active") {
        onPaired();
        onClose();
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function discardFailedPairing() {
    if (busy) return;
    setBusy(true);
    try {
      await invoke<PairingStatus>("mobile_pairing_discard_fixture");
      setStatus(null);
      setPairingCode("");
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dm-pairing-layer" role="dialog" aria-modal="true" aria-label="Connect your Mac">
      <button type="button" className="dm-drawer-scrim" onClick={onClose} aria-label="Close pairing" />
      <section className="dm-pairing-sheet">
        <header><span><Laptop /> Connect your Mac</span><button type="button" onClick={onClose} aria-label="Close"><X /></button></header>
        {needsDiscard ? (
          <div className="dm-pairing-recovery">
            <ShieldCheck />
            <strong>Clear the unfinished connection</strong>
            <p>The previous attempt stopped before approval. Remove only that unfinished security identity, then pair again with a fresh Mac code.</p>
            <button type="button" className="dm-pairing-reset" onClick={() => void discardFailedPairing()}>Discard unfinished pairing</button>
          </div>
        ) : !status?.confirmation ? (
          <>
            <div className="dm-pairing-intro"><ShieldCheck /><div><strong>Private, local sync</strong><p>On your Mac, open Noted → iPhone, copy the pairing code, then paste it below. Both devices must be on the same Wi-Fi.</p></div></div>
            <label>Pairing code<textarea rows={4} value={pairingCode} onChange={(event) => setPairingCode(event.target.value)} placeholder="Paste from Noted on your Mac" /></label>
            <label>Mac address <span>Optional fallback</span><input value={manualAddress} onChange={(event) => setManualAddress(event.target.value)} placeholder="192.168.1.10:49152" inputMode="url" /></label>
            {error && <p className="dm-pairing-error">{error}</p>}
            <button type="button" className="dm-pairing-primary" onClick={() => void connect()} disabled={!pairingCode.trim() || busy}>{busy ? "Connecting…" : "Connect securely"}</button>
          </>
        ) : (
          <>
            <p className="dm-pairing-kicker">MATCH THIS CODE ON YOUR MAC</p>
            <strong className="dm-pairing-code">{status.confirmation.verificationCode}</strong>
            <p className="dm-pairing-copy">Noted will sync notes, folders, and categories after you approve the same code on your Mac.</p>
            {error && <p className="dm-pairing-error">{error}</p>}
            <button type="button" className="dm-pairing-primary" onClick={() => void finish()} disabled={busy}>{busy ? "Checking…" : "I approved it on my Mac"}</button>
          </>
        )}
      </section>
    </div>
  );
}

function ScheduleScreen() {
  const [tasksOpen, setTasksOpen] = useState(true);
  const [completed, setCompleted] = useState<Set<number>>(new Set());
  const [taskStyle, setTaskStyle] = useState<string | null>(null);
  return (
    <section className="dm-screen dm-dark dm-schedule">
      <header className="dm-dark-header">
        <strong>noted<span>.</span></strong>
        <button type="button"><SlidersHorizontal /> Calendar sync</button>
      </header>
      <div className="dm-date-heading">
        <p>DAILY SCHEDULE</p>
        <div><button type="button" aria-label="Previous day"><ChevronLeft /></button><span>Today</span><button type="button" aria-label="Next day"><ChevronRight /></button></div>
        <h1>Thursday, August 20</h1>
      </div>
      <button type="button" className="dm-up-next"><strong>Up next</strong><CalendarDays /><span>Tomorrow · 8:00 AM</span><b>Daily Stand Up</b><ChevronRight /></button>
      <div className="dm-all-day"><span><CalendarDays /> ALL DAY <b>1</b></span><button type="button"><Plus /> Add</button></div>
      <div className="dm-all-day-event"><i /><span>Stay at Retro-vibe Chinatown condo with SF skyline views</span></div>
      <div className="dm-timeline">
        {SCHEDULE.map((event) => (
          <div className={`dm-time-event${event.live ? " live" : ""}`} key={`${event.start}-${event.title}`}>
            <time>{event.start}</time>
            <article>
              <small>{event.start}–{event.end}</small>
              <strong>{event.title}</strong>
              {event.live && <em>NOW</em>}
              {event.duration && <span>{event.duration}</span>}
              {event.video && <Video />}
            </article>
          </div>
        ))}
      </div>
      <section className={`dm-tasks-sheet${tasksOpen ? " open" : ""}`}>
        <button className="dm-tasks-sheet__handle" type="button" onClick={() => setTasksOpen((value) => !value)}>
          <strong>Tasks <span>· 4</span></strong><ChevronDown />
        </button>
        {tasksOpen && <><div className="dm-task-toolbar" aria-label="Task formatting">
          {[Pilcrow, List, ListOrdered, ImagePlus, Bold, Italic, Underline, Link].map((Icon, index) => {
            const id = `format-${index}`;
            return <button type="button" key={id} className={taskStyle === id ? "active" : ""} onClick={() => setTaskStyle((current) => current === id ? null : id)} aria-label={`Task format ${index + 1}`}><Icon /></button>;
          })}
        </div><div className="dm-task-list">{TASKS.map((task, index) => (
          <button
            type="button"
            key={task}
            className={completed.has(index) ? "done" : ""}
            onClick={() => setCompleted((previous) => {
              const next = new Set(previous);
              if (next.has(index)) next.delete(index); else next.add(index);
              return next;
            })}
          ><span>{completed.has(index) && <Check />}</span>{task}</button>
        ))}</div></>}
      </section>
    </section>
  );
}

function CalendarScreen() {
  const [mode, setMode] = useState<"day" | "week">("day");
  return (
    <section className="dm-screen dm-dark dm-calendar">
      <header className="dm-calendar__header">
        <div><strong>August 2026</strong><button type="button" aria-label="Previous"><ChevronLeft /></button><button type="button">Today</button><button type="button" aria-label="Next"><ChevronRight /></button></div>
        <div className="dm-segmented"><button type="button" className={mode === "day" ? "active" : ""} onClick={() => setMode("day")}>Day</button><button type="button" className={mode === "week" ? "active" : ""} onClick={() => setMode("week")}>Week</button></div>
      </header>
      <div className="dm-week-strip">{CALENDAR_DAYS.map((day) => <button type="button" key={day.day} className={day.active ? "active" : ""}><span>{day.weekday}</span><strong>{day.day}</strong></button>)}</div>
      <div className="dm-calendar-all-day"><span>ALL-DAY</span><button type="button">Stay at Retro-vibe Chinatown condo with SF skyline views</button></div>
      <div className="dm-day-agenda">
        {SCHEDULE.slice(0, 6).map((event) => <div key={`${event.start}-${event.title}`} className={event.live ? "live" : ""}><time>{event.start}</time><article><strong>{event.title}</strong><span>{event.start}–{event.end}</span></article></div>)}
      </div>
      <button className="dm-fab" type="button" aria-label="New event"><Plus /></button>
    </section>
  );
}

function NotesDrawer({ workspace, view, onView, onClose }: { workspace: MobileWorkspace; view: NotesView; onView: (view: NotesView) => void; onClose: () => void }) {
  const views: Array<{ id: NotesView; label: string; icon: typeof FileText; count: number | null }> = [
    { id: "all", label: "All Notes", icon: FileText, count: workspace.notes.length },
    { id: "inbox", label: "Inbox", icon: Inbox, count: workspace.counts.inbox },
    { id: "meetings", label: "Meetings", icon: AudioWaveform, count: workspace.notes.filter((note) => note.folderName?.toLowerCase().includes("meeting")).length },
    { id: "needsFiling", label: "Needs filing", icon: Folder, count: workspace.counts.needsFiling },
  ];
  return (
    <div className="dm-drawer-layer">
      <button className="dm-drawer-scrim" type="button" onClick={onClose} aria-label="Close library" />
      <aside className="dm-notes-drawer">
        <header><span>Library</span><button type="button" onClick={onClose} aria-label="Close library"><X /></button></header>
        {views.map((item) => { const Icon = item.icon; return <button type="button" key={item.id} className={view === item.id ? "active" : ""} onClick={() => { onView(item.id); onClose(); }}><Icon /><span>{item.label}</span><small>{item.count ?? ""}</small></button>; })}
        <div className="dm-drawer-label"><span>Folders</span><Plus /></div>
        {workspace.folders.slice(0, 8).map((folder) => <button type="button" key={folder.folderId}><Folder /><span>{folder.path || folder.name}</span><small>{folder.noteCount}</small></button>)}
        <button type="button" className={view === "trash" ? "active dm-trash" : "dm-trash"} onClick={() => { onView("trash"); onClose(); }}><Trash2 /><span>Trash</span><small>{workspace.counts.trash ?? ""}</small></button>
      </aside>
    </div>
  );
}

function NotesScreen({ workspace, query, setQuery, view, setView, onRefresh, onConnect }: { workspace: MobileWorkspace; query: string; setQuery: (query: string) => void; view: NotesView; setView: (view: NotesView) => void; onRefresh: () => void; onConnect: () => void }) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [selected, setSelected] = useState<MobileNote | null>(null);
  const notes = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return workspace.notes.filter((note) => {
      if (view === "trash" && note.lifecycleState !== "trashed") return false;
      if (view !== "trash" && note.lifecycleState === "trashed") return false;
      if (view === "needsFiling" && !note.needsFiling) return false;
      if (view === "meetings" && !(`${note.folderName} ${note.title}`).toLowerCase().includes("meeting") && !note.title.toLowerCase().includes("stand up")) return false;
      if (!normalized) return true;
      return `${note.title} ${note.body}`.toLowerCase().includes(normalized);
    });
  }, [query, view, workspace.notes]);
  if (selected) return <NoteEditor note={selected} onClose={() => { setSelected(null); onRefresh(); }} />;
  return (
    <section className="dm-screen dm-dark dm-notes">
      <header className="dm-notes__top"><strong>noted<span>.</span></strong><button type="button" className="dm-mac-state" onClick={onConnect}><Laptop /> {syncCopy(workspace.sync.state, workspace.sync.pendingCount)}</button><button type="button" aria-label="New note"><FileText /></button></header>
      <div className="dm-notes__title"><button type="button" onClick={() => setDrawerOpen(true)} aria-label="Open library"><Menu /></button><div><button type="button">Work <ChevronDown /></button><h1>{view === "all" ? "All Notes" : view === "needsFiling" ? "Needs filing" : view[0].toUpperCase() + view.slice(1)}</h1></div><span>{notes.length} notes</span></div>
      <label className="dm-notes-search"><Search /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search notes and transcripts…" /></label>
      <div className="dm-notes-tools"><button type="button"><Filter /> Filter</button><button type="button">Newest first <ChevronDown /></button></div>
      <div className="dm-note-list">{notes.map((note) => <button type="button" key={note.recordId} onClick={() => setSelected(note)}><AudioWaveform /><strong>{note.title}</strong><span>{noteSubtitle(note)}</span><time>{new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(new Date(note.updatedAt))}</time></button>)}</div>
      {drawerOpen && <NotesDrawer workspace={workspace} view={view} onView={setView} onClose={() => setDrawerOpen(false)} />}
    </section>
  );
}

export function DesktopMatchedMobile({ client }: { client: MobileNotesClient }) {
  const [tab, setTab] = useState<PrimaryTab>("home");
  const [workspace, setWorkspace] = useState<MobileWorkspace>(EMPTY_WORKSPACE);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<NotesView>("all");
  const [capture, setCapture] = useState("");
  const [saving, setSaving] = useState(false);
  const [pairingOpen, setPairingOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const result = await client.workspace({ query: null, view: "all", folderId: null });
      setWorkspace(result);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [client]);

  useEffect(() => { void refresh(); }, [refresh]);

  async function captureNote() {
    const body = capture.trim();
    if (!body || saving) return;
    setSaving(true);
    try {
      const title = body.split("\n").find((line) => line.trim())?.trim().slice(0, 80) || "Untitled note";
      await client.create(title, body);
      setCapture("");
      await refresh();
      setTab("notes");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSaving(false);
    }
  }

  async function sync() {
    try {
      await client.sync();
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  return (
    <main className={`noted-mobile noted-mobile--${tab}`}>
      {error && <button className="dm-error" type="button" onClick={() => setError(null)}>{error}<X /></button>}
      {tab === "home" && <HomeScreen workspace={workspace} capture={capture} setCapture={setCapture} saving={saving} onCapture={captureNote} onSync={sync} onConnect={() => setPairingOpen(true)} />}
      {tab === "schedule" && <ScheduleScreen />}
      {tab === "calendar" && <CalendarScreen />}
      {tab === "notes" && <NotesScreen workspace={workspace} query={query} setQuery={setQuery} view={view} setView={setView} onRefresh={refresh} onConnect={() => setPairingOpen(true)} />}
      <BottomNav tab={tab} onChange={setTab} />
      {pairingOpen && <PairingSheet onClose={() => setPairingOpen(false)} onPaired={() => void refresh()} />}
    </main>
  );
}
