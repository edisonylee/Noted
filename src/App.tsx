import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "./events";
import { BookOpen, CalendarDays, Camera, Check, ChevronUp, Download, House, ListTodo, Loader, Mic, Moon, Network, PanelLeft, PenLine, Settings, Smartphone, Square, StickyNote, Sun } from "lucide-react";
import { SettingsModal } from "./Settings";
import { startRecording, type Recorder } from "./audio";
import { fileToImg, type Img } from "./image";
import { useIsMobile } from "./useIsMobile";
import { useConnection } from "./useConnection";
import { MobileCapture } from "./MobileCapture";
import { BottomNav, type MobileTab } from "./BottomNav";
import { useTheme } from "./useTheme";
import { api, TokenError, OfflineError, type CategoryInfo, type EntityCandidate, type Envelope, type Health, type NoteFolderInfo, type NoteRow, type RangeEvent, type RelatedBrain } from "./api";
import { DataView } from "./DataView";
import { CalendarView } from "./Calendar";
import { JournalView } from "./Journal";
import { PhonePanel } from "./PhonePanel";
import { FloatingChat } from "./FloatingChat";
import { KnowledgeView } from "./Knowledge";
import { parseBlocks, TodayView } from "./Today";
import { MeetingPage } from "./MeetingPage";
import { releaseProfile } from "./releaseProfile";
import { ComingUp } from "./ComingUp";
import { NotesView } from "./NotesView";
import { AskView } from "./AskView";
import { WeatherHome } from "./Weather";
import { APP_TZ, easternDay, easternHour } from "./day";
import "./App.css";

type Phase = "idle" | "thinking" | "review";
type View = "today" | "ask" | "capture" | "notes" | "calendar" | "journal" | "knowledge" | "settings";

// Journal is parked while the meeting recorder + knowledge graph stabilize;
// the view, commands, and data all stay — this only hides the nav entry.
const SHOW_JOURNAL = false;
type Source = "text" | "photo";
// One editable review card per extracted entry.
type ReviewCard = {
  catName: string;
  dataText: string;
  description: string;
  routedBy?: "header" | "classifier";
};

// Time-adaptive home greeting: planning prompt in the morning → recap prompt at night.
function homeGreeting(): { dateLine: string; title: string } {
  const now = new Date();
  const h = easternHour(now);
  const partOfDay = h < 12 ? "morning" : h < 17 ? "afternoon" : h < 21 ? "evening" : "night";
  const dateStr = now.toLocaleDateString(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    timeZone: APP_TZ,
  });
  const title =
    h < 11
      ? "What's your schedule looking like today?"
      : h < 17
        ? "How's your day going?"
        : "What did you do today?";
  return { dateLine: `${dateStr} · ${partOfDay}`, title };
}

export default function App() {
  const { theme, toggle } = useTheme();
  const [view, setView] = useState<View>("ask");
  // Entity to open in Knowledge (set when a related-brain chip is clicked).
  const [knowledgeEntity, setKnowledgeEntity] = useState<number | null>(null);
  const [text, setText] = useState("");
  // Proactive surfacing: brain notes related to what's being typed.
  const [related, setRelated] = useState<RelatedBrain[]>([]);
  const [img, setImg] = useState<Img | null>(null);
  const [source, setSource] = useState<Source>("text");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  // review state: one editable card per extracted entry, plus note-level fields
  const [cards, setCards] = useState<ReviewCard[]>([]);
  const [ocrText, setOcrText] = useState(""); // editable transcription (photo path)
  const [eventDate, setEventDate] = useState(""); // canonical day, editable
  const [dateWasExtracted, setDateWasExtracted] = useState(false);
  const [entityChips, setEntityChips] = useState<EntityCandidate[]>([]); // graph entities to confirm

  const [notes, setNotes] = useState<NoteRow[]>([]);
  const [cats, setCats] = useState<CategoryInfo[]>([]);
  const [noteSpaces, setNoteSpaces] = useState<NoteFolderInfo[]>([]);
  const [captureSpaceId, setCaptureSpaceId] = useState<number | null>(null);
  const [health, setHealth] = useState<Health | null>(null);

  // Phone capture + backup
  const [showPhone, setShowPhone] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [backupMsg, setBackupMsg] = useState<string | null>(null);
  const [savedMsg, setSavedMsg] = useState<string | null>(null);
  const [needsRepair, setNeedsRepair] = useState(false); // 403 from a stale phone token

  // Mobile-first layout: phone viewports get a bottom-nav + dedicated capture
  // screen instead of the desktop topbar/review flow.
  const mobileViewport = useIsMobile();
  const isMobile = releaseProfile.phoneLan && mobileViewport;
  const [mobileTab, setMobileTab] = useState<MobileTab>("today");

  // Keep the native glass in step with the in-app theme: the theme runtime
  // stamps data-theme on <html>; we watch it and switch the vibrancy material
  // (dark HUD vs light sidebar glass) so light mode stays readable.
  useEffect(() => {
    const sync = () =>
      api.setChromeTheme(document.documentElement.dataset.theme === "dark").catch(() => {});
    sync();
    const mo = new MutationObserver(sync);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, []);

  // Phone captures are categorized by a background worker — refresh the lists
  // the moment one files (or fails) so it appears without switching views.
  const refreshRef = useRef<() => void>(() => {});
  useEffect(() => {
    refreshRef.current = () => refresh().catch(handleErr);
  });
  useEffect(() => {
    const subs = [
      listen("note-filed", () => refreshRef.current()),
      listen("capture-needs-attention", () => refreshRef.current()),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, []);

  // Sidebar collapse (one click or ⌘B), remembered across launches.
  const [sideOpen, setSideOpenState] = useState(
    () => localStorage.getItem("noted-sidebar") !== "closed"
  );
  const setSideOpen = (o: boolean) => {
    setSideOpenState(o);
    localStorage.setItem("noted-sidebar", o ? "open" : "closed");
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "b") {
        e.preventDefault();
        setSideOpenState((o) => {
          localStorage.setItem("noted-sidebar", o ? "closed" : "open");
          return !o;
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Meetings: the open meeting page (a today sub-view) + the global
  // recording indicator in the topbar.
  const [meetingOpen, setMeetingOpen] = useState<{
    id: number | null;
    event?: Partial<RangeEvent>;
  } | null>(null);
  const [recMeeting, setRecMeeting] = useState<{ id: number; title: string } | null>(null);
  const [meetingControlAction, setMeetingControlAction] = useState<"starting" | "stopping" | null>(null);
  const [meetingControlError, setMeetingControlError] = useState<string | null>(null);
  useEffect(() => {
    api
      .meetingState()
      .then((s) => {
        if (s.active && s.meetingId != null)
          setRecMeeting({ id: s.meetingId, title: s.title ?? "Meeting" });
      })
      .catch(() => {});
    const subs = [
      listen<{ meetingId: number; title: string }>("meeting-started", (e) => {
        setMeetingControlError(null);
        setRecMeeting({ id: e.payload.meetingId, title: e.payload.title });
        // Granola opens the note when recording starts — so do we.
        setView("today");
        setMeetingOpen({ id: e.payload.meetingId });
      }),
      listen<{ meetingId: number }>("meeting-stopped", () => setRecMeeting(null)),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, []);

  // The sidebar control is the always-available manual path: it starts an
  // event-less recording immediately, then becomes the matching stop button.
  // Calendar meetings still keep their richer pre-meeting page and metadata.
  async function toggleMeetingRecording() {
    if (meetingControlAction) return;
    const action = recMeeting ? "stopping" : "starting";
    setMeetingControlAction(action);
    setMeetingControlError(null);
    try {
      if (recMeeting) {
        await api.meetingStop();
      } else {
        const id = await api.meetingStart({ title: "Meeting" });
        setView("today");
        setMeetingOpen({ id });
      }
    } catch (e) {
      setMeetingControlError(String(e));
    } finally {
      setMeetingControlAction(null);
    }
  }

  // Once today's schedule exists, the composer's "what's your schedule?" moment
  // has passed — it collapses to a slim pill and expands on demand (or when a
  // pasted photo / an in-flight capture needs it).
  const [composerOpen, setComposerOpen] = useState(false);
  const hasSchedule = useMemo(() => {
    const today = easternDay();
    return notes.some(
      (n) =>
        n.event_date === today &&
        n.entries.some(
          (e) => e.category?.toLowerCase() === "schedule" && parseBlocks(e.data).length > 0
        )
    );
  }, [notes]);
  // A pasted image or an in-flight categorize/review always forces it open.
  const composerVisible = view === "capture" || composerOpen || !hasSchedule || phase !== "idle" || img != null;

  // Route a thrown error: a TokenError (phone 403) trips the re-pair screen; an
  // OfflineError (Mac unreachable, e.g. mid-rebuild) shows the reconnecting
  // overlay and lets the watcher auto-recover; anything else is an inline error.
  const handleErr = (e: unknown) =>
    e instanceof TokenError
      ? setNeedsRepair(true)
      : e instanceof OfflineError
        ? markOffline()
        : setError(String(e));

  // Voice (speech-to-text)
  const [voiceReady, setVoiceReady] = useState<boolean | null>(null);
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [dlModel, setDlModel] = useState(false);
  const recorderRef = useRef<Recorder | null>(null);

  const refresh = async () => {
    const [n, c, folders] = await Promise.all([
      api.listNotes(),
      api.listCategories(),
      api.listNoteFolders(),
    ]);
    setNotes(n);
    setCats(c);
    const roots = folders
      .filter((folder) => folder.kind === "space")
      .sort((a, b) => {
        const rank = (name: string) =>
          name.toLowerCase() === "personal" ? 0 : name.toLowerCase() === "work" ? 1 : 2;
        return rank(a.name) - rank(b.name);
      });
    setNoteSpaces(roots);
    setCaptureSpaceId((current) => {
      if (current != null && roots.some((space) => space.id === current)) return current;
      const saved = Number(localStorage.getItem("noted-capture-space"));
      return (
        roots.find((space) => space.id === saved)?.id ??
        roots.find((space) => space.name.toLowerCase() === "personal")?.id ??
        roots[0]?.id ??
        null
      );
    });
  };

  // Phone web client only: watch the connection to the Mac and auto-recover when
  // it comes back (e.g. after a dev rebuild) — refetch health + data, no reload.
  const { online, markOffline } = useConnection({
    onReconnect: () => {
      api.health().then(setHealth).catch(() => {});
      refresh().catch(handleErr);
    },
  });

  // Debounced "related in your brain" while composing (idle, text only).
  useEffect(() => {
    if (phase !== "idle" || img || text.trim().length < 12) {
      setRelated([]);
      return;
    }
    const t = setTimeout(() => {
      api.relatedBrain(text).then(setRelated).catch(() => {});
    }, 600);
    return () => clearTimeout(t);
  }, [text, phase, img]);

  useEffect(() => {
    api.health().then(setHealth).catch(handleErr);
    refresh().catch(handleErr);
    api.reindex().catch(() => {}); // backfill any notes missing embeddings
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => setVoiceReady(false));
  }, []);

  async function ensureVoice(): Promise<boolean> {
    if (voiceReady) return true;
    setDlModel(true);
    try {
      await api.downloadVoiceModel();
      setVoiceReady(true);
      return true;
    } catch (e) {
      setError(`voice model download failed: ${e}`);
      return false;
    } finally {
      setDlModel(false);
    }
  }

  async function onMic() {
    setError(null);
    if (!voiceReady) {
      await ensureVoice();
      return;
    }
    if (recording) {
      // stop + transcribe
      setRecording(false);
      setTranscribing(true);
      try {
        const { b64, sampleRate } = await recorderRef.current!.stop();
        const said = await api.transcribe(b64, sampleRate);
        if (said) setText((t) => (t ? t.trim() + " " : "") + said);
      } catch (e) {
        setError(`transcription failed: ${e}`);
      } finally {
        setTranscribing(false);
        recorderRef.current = null;
      }
    } else {
      try {
        recorderRef.current = await startRecording();
        setRecording(true);
      } catch (e) {
        setError(`couldn't access the microphone: ${e}`);
      }
    }
  }


  // paste an image straight from the clipboard (screenshots etc.)
  useEffect(() => {
    const onPaste = async (e: ClipboardEvent) => {
      // Ask owns its own attachment paste handler. Only the dedicated Capture
      // page should populate the note-ingestion state here.
      if (view !== "capture") return;
      const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
        i.type.startsWith("image/")
      );
      const file = item?.getAsFile();
      if (file) {
        e.preventDefault();
        setImg(await fileToImg(file));
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [view]);

  const cardParses = useMemo(
    () =>
      cards.map((c) => {
        try {
          return { ok: true as const, value: JSON.parse(c.dataText || "{}") };
        } catch (e) {
          return { ok: false as const, err: (e as Error).message };
        }
      }),
    [cards]
  );
  const allValid = cardParses.every((p) => p.ok);

  const updateCard = (i: number, patch: Partial<ReviewCard>) =>
    setCards((cs) => cs.map((c, idx) => (idx === i ? { ...c, ...patch } : c)));
  const discardCard = (i: number) => {
    const next = cards.filter((_, idx) => idx !== i);
    if (next.length === 0) resetAll();
    else setCards(next);
  };

  const updateEntity = (i: number, patch: Partial<EntityCandidate>) =>
    setEntityChips((es) => es.map((e, idx) => (idx === i ? { ...e, ...patch } : e)));
  const removeEntity = (i: number) =>
    setEntityChips((es) => es.filter((_, idx) => idx !== i));

  async function pickFile(file?: File | null) {
    if (!file) return;
    setError(null);
    setImg(await fileToImg(file));
  }

  function enterReview(env: Envelope, src: Source) {
    setCards(
      env.entries.map((e) => ({
        catName: e.category,
        dataText: JSON.stringify(e.data, null, 2),
        description: e.description,
        routedBy: e.routed_by,
      }))
    );
    setOcrText(env.raw_text ?? "");
    setEventDate(env.event_date ?? "");
    setDateWasExtracted(!!env.date_was_extracted);
    setEntityChips(env.entities ?? []);
    setSource(src);
    setPhase("review");
  }

  async function onCategorizeText() {
    if (!text.trim()) return;
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorize(text.trim()), "text");
    } catch (e) {
      setError(String(e));
      setPhase("idle");
    }
  }

  async function beginNoteReview({ text: draftText, img: draftImg }: { text: string; img: Img | null }) {
    const raw = draftText.trim();
    if (!raw && !draftImg) return;
    setError(null);
    setPhase("thinking");
    try {
      let env: Envelope;
      if (draftImg && raw) {
        const ocr = await api.ocrPhoto(draftImg.base64);
        env = await api.categorize([raw, ocr.trim()].filter(Boolean).join("\n\n"));
      } else if (draftImg) {
        env = await api.categorizePhoto(draftImg.base64);
      } else {
        env = await api.categorize(raw);
      }
      setText(raw);
      setImg(draftImg);
      enterReview(env, draftImg ? "photo" : "text");
      setMeetingOpen(null);
      setView("capture");
    } catch (e) {
      setPhase("idle");
      setError(String(e));
      throw e;
    }
  }

  async function ingestPhoto(base64: string, ext: string) {
    setMeetingOpen(null);
    setView("capture");
    setImg({ base64, ext, dataUrl: `data:image/${ext === "jpg" ? "jpeg" : ext};base64,${base64}` });
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorizePhoto(base64), "photo");
    } catch (e) {
      setError(String(e));
      setPhase("idle");
    }
  }

  async function onCategorizePhoto() {
    if (img) await ingestPhoto(img.base64, img.ext);
  }

  // Photos arriving from the phone capture server.
  useEffect(() => {
    const un = listen<{ path: string }>("photo-received", async (e) => {
      try {
        const { base64, ext } = await api.readInboxImage(e.payload.path);
        await ingestPhoto(base64, ext);
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function onBackup() {
    setBackupMsg("backing up…");
    try {
      const path = await api.exportDb();
      setBackupMsg(`backed up to ${path}`);
    } catch (e) {
      setBackupMsg(`backup failed: ${e}`);
    }
  }

  async function onSave() {
    if (!cards.length || !allValid) return;
    setError(null);
    try {
      let image_path: string | null = null;
      if (source === "photo" && img) {
        image_path = await api.saveImage(img.base64, img.ext);
      }
      const entries = cards.map((c, i) => ({
        category: c.catName.trim().toLowerCase(),
        description: c.description,
        data: (cardParses[i] as { ok: true; value: Record<string, unknown> }).value,
      }));
      const entities = entityChips
        .map((e) => ({
          name: e.name.trim(),
          type: e.type.trim().toLowerCase(),
          fact: e.fact?.trim() || undefined,
          relationship: e.relationship?.trim() || undefined,
        }))
        .filter((e) => e.name && e.type);
      const noteId = await api.save({
        raw_text: source === "photo" ? ocrText : text.trim(),
        source,
        image_path,
        event_date: eventDate,
        entries,
        entities,
      });
      if (captureSpaceId != null) await api.fileNote(noteId, captureSpaceId);
      const savedCats = Array.from(new Set(entries.map((e) => e.category))).join(", ");
      resetAll();
      await refresh();
      setSavedMsg(savedCats);
      setTimeout(() => setSavedMsg(null), 4000);
    } catch (e) {
      setError(String(e));
    }
  }

  function resetAll() {
    setText("");
    setImg(null);
    setComposerOpen(false); // filed/discarded → the pill again (if a schedule exists)
    setSource("text");
    setCards([]);
    setOcrText("");
    setEventDate("");
    setDateWasExtracted(false);
    setEntityChips([]);
    setPhase("idle");
    setView("ask");
  }

  function goHome() {
    setMeetingOpen(null);
    setView(phase === "idle" ? "ask" : "capture");
  }

  const busy = phase === "thinking";

  // Phone lost its access token (403) — show a recoverable re-pair screen
  // instead of a dead app. Relaunching from the home-screen icon (whose URL
  // carries the token) clears this on next load.
  // Non-destructive overlay shown while the Mac is unreachable (e.g. mid-rebuild).
  // `online` stays true on desktop, so this only ever appears on the phone. It
  // preserves the screen underneath and auto-dismisses when the watcher recovers
  // — distinct from the 403 re-pair screen, and never forces a reload.
  const reconnectingOverlay = !online && (
    <div className="reconnecting-overlay">
      <div className="reconnecting-card">
        <Loader size={18} className="spin" />
        <span>Reconnecting to your Mac…</span>
      </div>
    </div>
  );

  if (needsRepair) {
    return (
      <div className="app repair-screen">
        <div className="repair-card">
          <div className="brand">noted<span className="dot">.</span></div>
          <h2>Reconnect to your Mac</h2>
          <p>
            This phone’s connection expired. Open <strong>noted</strong> on your Mac, click the
            phone button, and scan the QR code again — or just relaunch noted from your Home Screen
            icon.
          </p>
          <button className="primary" onClick={() => window.location.reload()}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  // Mobile-first shell: slim top bar + bottom nav + dedicated capture screen.
  // The desktop layout below is left entirely untouched.
  if (isMobile) {
    return (
      <div className="app mobile">
        {reconnectingOverlay}
        <header className="mobile-topbar">
          <div className="brand">noted<span className="dot">.</span></div>
          <button className="icon-btn" onClick={() => setShowSettings(true)} aria-label="Settings">
            <Settings size={18} />
          </button>
        </header>

        <main className="mobile-content">
          {mobileTab === "today" && (
            <TodayView notes={notes} onSaved={() => refresh().catch(handleErr)} onOpenSettings={() => setShowSettings(true)} />
          )}
          {mobileTab === "capture" && <MobileCapture onCaptured={() => refresh().catch(handleErr)} />}
        </main>

        <BottomNav
          active={mobileTab}
          onChange={(t) => {
            setMobileTab(t);
            // Re-fetch when navigating to the schedule so a freshly auto-filed note shows.
            if (t === "today") refresh().catch(handleErr);
          }}
        />

        <FloatingChat
          variant="sheet"
          open={mobileTab === "ask"}
          onOpenChange={(o) => {
            if (!o) setMobileTab("capture");
          }}
          onMutated={() => refresh().catch(handleErr)}
        />

        {showSettings && <SettingsModal onClose={() => setShowSettings(false)} />}
      </div>
    );
  }

  return (
    <div
      className={
        "app side" +
        (sideOpen ? "" : " side-hidden") +
        (view === "ask" || view === "capture"
          ? " homemode"
          : view === "calendar"
            ? " calmode"
            : view === "journal"
              ? " journalmode"
              : "")
      }
    >
      {reconnectingOverlay}
      {/* Overlay titlebar (Codex-style): the top strip drags the window, and
          the sidebar toggle sits just right of the traffic lights — same spot
          whether the rail is open or closed. */}
      <div className="titlebar-drag" data-tauri-drag-region />
      <button
        className="side-toggle icon-btn"
        onClick={() => setSideOpen(!sideOpen)}
        title={(sideOpen ? "Collapse" : "Open") + " sidebar (⌘B)"}
        aria-label={(sideOpen ? "Collapse" : "Open") + " sidebar"}
      >
        <PanelLeft size={17} />
      </button>
      {/* The whole sidebar background drags the window (like a native app's
          source list) — buttons still click because the drag region only
          engages when the grabbed element is the one carrying the attribute. */}
      <aside className="sidebar" data-tauri-drag-region="deep">
        <div className="side-head" data-tauri-drag-region>
          <div className="brand" data-tauri-drag-region>noted<span className="dot">.</span></div>
        </div>
        <nav className="side-nav">
          <button
            className={view === "ask" || view === "capture" ? "on" : ""}
            onClick={goHome}
          >
            <House size={16} /> Home
          </button>
          <button
            className={view === "today" ? "on" : ""}
            onClick={() => {
              setMeetingOpen(null);
              setView("today");
              refresh().catch(handleErr);
            }}
          >
            <ListTodo size={16} /> Schedule
          </button>
          <button className={view === "notes" ? "on" : ""} onClick={() => setView("notes")}>
            <StickyNote size={16} /> Notes
          </button>
          <button className={view === "calendar" ? "on" : ""} onClick={() => setView("calendar")}>
            <CalendarDays size={16} /> Calendar
          </button>
          {SHOW_JOURNAL && (
            <button className={view === "journal" ? "on" : ""} onClick={() => setView("journal")}>
              <BookOpen size={16} /> Journal
            </button>
          )}
          <button className={view === "knowledge" ? "on" : ""} onClick={() => setView("knowledge")}>
            <Network size={16} /> Knowledge
          </button>
        </nav>
        <span className="spacer" data-tauri-drag-region />
        <button
          className={"rec-pill" + (recMeeting ? " active" : "")}
          onClick={toggleMeetingRecording}
          disabled={meetingControlAction != null}
          title={recMeeting ? `Stop recording: ${recMeeting.title}` : "Start a recording"}
        >
          {meetingControlAction ? (
            <Loader size={13} className="spin" />
          ) : recMeeting ? (
            <span className="bars" aria-hidden>
              <i />
              <i />
              <i />
            </span>
          ) : (
            <Mic size={14} />
          )}
          {meetingControlAction
            ? meetingControlAction === "stopping"
              ? "Stopping…"
              : "Starting…"
            : recMeeting
              ? "Stop recording"
              : "Start recording"}
        </button>
        {meetingControlError && (
          <span className="sidebar-record-error" role="alert">
            {meetingControlError}
          </span>
        )}
        <div className="side-foot">
          <button
            className="icon-btn"
            onClick={toggle}
            title={theme === "dark" ? "Switch to light" : "Switch to dark"}
            aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
          </button>
          {releaseProfile.phoneLan && (
            <button className="icon-btn" onClick={() => setShowPhone(true)} title="Capture from your phone">
              <Smartphone size={18} />
            </button>
          )}
          <button
            className={"icon-btn" + (view === "settings" ? " on" : "")}
            onClick={() => setView("settings")}
            title="Settings"
          >
            <Settings size={18} />
          </button>
        </div>
      </aside>

      {/* Empty background = window drag, like a native app. The attribute only
          fires when the grabbed element IS the background (children keep their
          own clicks/selection), so content stays fully interactive. */}
      <div className="main-col" data-tauri-drag-region>
      <main className="content" data-tauri-drag-region>
        {view === "settings" ? (
          <SettingsModal page onClose={goHome} />
        ) : view === "ask" ? (
          <WeatherHome>
            <AskView
              onMutated={() => refresh().catch(handleErr)}
              onSaveNote={beginNoteReview}
              onOpenEntity={(id) => {
                setKnowledgeEntity(id);
                setView("knowledge");
              }}
            />
          </WeatherHome>
        ) : view === "notes" ? (
          <NotesView notes={notes} cats={cats} />
        ) : view === "calendar" ? (
          <CalendarView onOpenSettings={() => setView("settings")} />
        ) : view === "journal" ? (
          <JournalView notes={notes} onSaved={() => refresh().catch(handleErr)} />
        ) : view === "knowledge" ? (
          <KnowledgeView
            theme={theme}
            notes={notes}
            openEntityId={knowledgeEntity}
            onOpenedEntity={() => setKnowledgeEntity(null)}
          />
        ) : meetingOpen ? (
          <MeetingPage
            id={meetingOpen.id}
            event={meetingOpen.event}
            onBack={() => setMeetingOpen(null)}
            onStarted={(id) => setMeetingOpen({ id })}
          />
        ) : view === "today" ? (
          <div className="schedule-page">
            <ComingUp
              onOpenEvent={(ev) => setMeetingOpen({ id: null, event: ev })}
              onOpenMeeting={(id) => setMeetingOpen({ id })}
              activeMeetingId={recMeeting?.id ?? null}
            />
            <TodayView
              notes={notes}
              onSaved={() => refresh().catch(handleErr)}
              onOpenSettings={() => setView("settings")}
            />
          </div>
        ) : (
        <WeatherHome>
        <div className="capture-workspace">
        <div className="log-hero">
        {!composerVisible ? (
          <button className="home-capturebtn" onClick={() => setComposerOpen(true)}>
            <PenLine size={15} /> Capture a note
          </button>
        ) : (
        <>
        {!hasSchedule && (
          <div className="greeting">
            <h1 className="greet-title">{homeGreeting().title}</h1>
          </div>
        )}
        <section
          className={"capture" + (dragOver ? " dragover" : "")}
          onDragOver={(e) => {
            e.preventDefault();
            setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragOver(false);
            pickFile(e.dataTransfer.files?.[0]);
          }}
        >
          {noteSpaces.length > 0 && (
            <label className="capture-space-field">
              <span>File in</span>
              <select
                value={captureSpaceId ?? ""}
                onChange={(event) => {
                  const id = Number(event.target.value);
                  setCaptureSpaceId(id);
                  localStorage.setItem("noted-capture-space", String(id));
                }}
                disabled={busy}
              >
                {noteSpaces.map((space) => (
                  <option key={space.id} value={space.id}>
                    {space.name}
                  </option>
                ))}
              </select>
            </label>
          )}
          {img ? (
            <div className="img-attached">
              <img src={img.dataUrl} alt="note" />
              <div className="img-meta">
                <span>photo attached</span>
                <button className="link" onClick={() => setImg(null)} disabled={busy}>
                  remove
                </button>
              </div>
            </div>
          ) : (
            <textarea
              value={text}
              placeholder="Brain-dump anything — your gym session, today's schedule, what you ate. Or drop / paste a photo of a handwritten note."
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onCategorizeText();
              }}
              disabled={busy}
            />
          )}

          <input
            ref={fileInput}
            type="file"
            accept="image/*,.heic,.heif"
            hidden
            onChange={(e) => pickFile(e.target.files?.[0])}
          />

          <div className="capture-actions">
            {hasSchedule && phase === "idle" && !img && (
              <button
                className="ghost"
                onClick={() => setComposerOpen(false)}
                title="Hide the composer"
                aria-label="Hide the composer"
              >
                <ChevronUp size={15} />
              </button>
            )}
            {!img && (
              <button className="ghost" onClick={() => fileInput.current?.click()} disabled={busy}>
                <Camera size={15} /> Photo
              </button>
            )}
            {!img && (
              <button
                className={"ghost" + (recording ? " recording" : "")}
                onClick={onMic}
                disabled={busy || transcribing || dlModel}
                title="Talk through your day"
              >
                {recording ? <Square size={14} strokeWidth={2.5} /> : <Mic size={15} />}
                {dlModel
                  ? "Downloading…"
                  : transcribing
                    ? "Transcribing…"
                    : recording
                      ? "Stop"
                      : voiceReady === false
                        ? "Enable voice"
                        : "Speak"}
              </button>
            )}
            <span className="hint">{img ? "review the transcription next" : "⌘↵ to file"}</span>
            {img ? (
              <button className="primary" onClick={onCategorizePhoto} disabled={busy}>
                {busy ? "Reading…" : "Read photo"}
              </button>
            ) : (
              <button className="primary" onClick={onCategorizeText} disabled={busy || !text.trim()}>
                {busy ? "Reading…" : "File it"}
              </button>
            )}
          </div>

          {phase === "idle" && related.length > 0 && (
            <div className="related-brain">
              <span className="related-brain-label">Related in your brain</span>
              <div className="related-brain-chips">
                {related.map((r) =>
                  r.entity_id != null ? (
                    <button
                      className="related-brain-chip clickable"
                      key={r.note_id}
                      title={r.snippet}
                      onClick={() => {
                        setKnowledgeEntity(r.entity_id);
                        setView("knowledge");
                      }}
                    >
                      {r.name ?? r.snippet.slice(0, 30)}
                      {r.vault && <em>{r.vault}</em>}
                    </button>
                  ) : (
                    <span className="related-brain-chip" key={r.note_id} title={r.snippet}>
                      {r.name ?? r.snippet.slice(0, 30)}
                      {r.vault && <em>{r.vault}</em>}
                    </span>
                  )
                )}
              </div>
            </div>
          )}
        </section>
        </>
        )}

        {error && <div className="error">{error}</div>}

        {phase === "review" && cards.length > 0 && (
          <section className="review">
            <div className="review-head">
              <div className="cat-edit date-edit">
                <label>Date</label>
                <input type="date" value={eventDate} onChange={(e) => setEventDate(e.target.value)} />
                <span className={"badge " + (dateWasExtracted ? "existing" : "new")}>
                  {dateWasExtracted ? "from note" : "today"}
                </span>
              </div>
              <span className="review-count">
                {cards.length} {cards.length === 1 ? "entry" : "entries"} from this note
              </span>
            </div>

            {source === "photo" && (
              <div className="transcription">
                <label>Transcription — fix any misreads</label>
                <div className="trans-row">
                  {img && <img className="trans-thumb" src={img.dataUrl} alt="note" />}
                  <textarea value={ocrText} onChange={(e) => setOcrText(e.target.value)} spellCheck={false} />
                </div>
              </div>
            )}

            <div className="review-cards">
              {cards.map((card, i) => {
                const known = cats.find((c) => c.name === card.catName.trim().toLowerCase());
                const parse = cardParses[i];
                return (
                  <div className="entry-card" key={i}>
                    <div className="entry-head">
                      <div className="cat-edit">
                        <label>Category</label>
                        <input
                          value={card.catName}
                          onChange={(e) => updateCard(i, { catName: e.target.value })}
                        />
                        {known ? (
                          <span className="badge existing">{known.entry_count} existing</span>
                        ) : (
                          <span className="badge new">new category</span>
                        )}
                        {card.routedBy === "header" && (
                          <span className="badge routed" title="You tagged this section with a header">
                            tagged
                          </span>
                        )}
                      </div>
                      {cards.length > 1 && (
                        <button className="link" onClick={() => discardCard(i)}>
                          discard
                        </button>
                      )}
                    </div>
                    {card.description && <p className="desc">{card.description}</p>}

                    <div className="review-body">
                      <div className="pane">
                        <label>Extracted data {parse.ok ? "" : "⚠︎ invalid JSON"}</label>
                        <textarea
                          className={"json " + (parse.ok ? "" : "bad")}
                          value={card.dataText}
                          onChange={(e) => updateCard(i, { dataText: e.target.value })}
                          spellCheck={false}
                        />
                      </div>
                      <div className="pane preview">
                        <label>Preview</label>
                        <div className="preview-box">
                          {parse.ok ? (
                            <DataView value={parse.value} />
                          ) : (
                            <span className="muted">fix JSON to preview</span>
                          )}
                        </div>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>

            {entityChips.length > 0 && (
              <div className="entities-review">
                <label>Entities — people, places &amp; things in this note (confirm or remove)</label>
                <div className="entity-chips">
                  {entityChips.map((e, i) => (
                    <span className="entity-chip" key={i}>
                      <input
                        className="entity-name"
                        value={e.name}
                        onChange={(ev) => updateEntity(i, { name: ev.target.value })}
                        spellCheck={false}
                      />
                      <select
                        className="entity-type"
                        value={e.type}
                        onChange={(ev) => updateEntity(i, { type: ev.target.value })}
                      >
                        {Array.from(
                          new Set(["person", "place", "activity", "food", "item", "org", "topic", e.type])
                        )
                          .filter(Boolean)
                          .map((t) => (
                            <option key={t} value={t}>
                              {t}
                            </option>
                          ))}
                      </select>
                      {e.type === "person" && (e.relationship || e.fact) && (
                        <span className="entity-fact" title="captured about this person">
                          {e.relationship ? `${e.relationship}` : ""}
                          {e.relationship && e.fact ? " · " : ""}
                          {e.fact ?? ""}
                        </span>
                      )}
                      <button className="entity-x" onClick={() => removeEntity(i)} title="remove">
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              </div>
            )}

            <div className="review-actions">
              <button onClick={resetAll}>Discard all</button>
              <button className="primary" onClick={onSave} disabled={!allValid}>
                {cards.length > 1
                  ? `Save ${cards.length} entries`
                  : `Save to ${cards[0].catName.trim().toLowerCase() || "…"}`}
              </button>
            </div>
          </section>
        )}

        </div>
        </div>
        </WeatherHome>
        )}
      </main>

      {savedMsg && (
        <button className="saved-toast" onClick={() => setView("knowledge")}>
          <Check size={15} /> Filed under <strong>{savedMsg}</strong> · view in knowledge
        </button>
      )}

      <footer className="status">
        <span className="meta">
          <span className="live-dot" />
          {health
            ? `${health.models.length} local models · ${
                health.models.some((m) => m.startsWith("qwen2.5vl")) ? "vision ready" : "no vision model"
              }`
            : "connecting to local models…"}
        </span>
        <span className="meta">
          {backupMsg && <span className="backup-msg">{backupMsg}</span>}
          <button className="link" onClick={onBackup}>
            <Download size={14} /> Back up
          </button>
        </span>
      </footer>
      </div>

      {releaseProfile.phoneLan && showPhone && <PhonePanel onClose={() => setShowPhone(false)} />}
    </div>
  );
}
