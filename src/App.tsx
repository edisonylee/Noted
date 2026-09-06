import { useOutsideDismiss } from "./ui/useDismissal";
import { AppUpdateIndicator } from "./AppUpdateIndicator";
import brandWordmark from "./design-system/assets/wordmark.png";
import type { TeamNotificationTarget } from "./teams/types";
import { useMentionNotifications } from "./teams/useMentionNotifications";
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "./events";
import { BookOpen, Camera, Check, ChevronUp, Loader, Mic, Moon, PenLine, Settings, Smartphone, Square, Sun, Video } from "lucide-react";
import { SettingsModal } from "./Settings";
import { startRecording, type Recorder } from "./audio";
import { fileToImg, type Img } from "./image";
import { useIsMobile } from "./useIsMobile";
import { useConnection } from "./useConnection";
import { MobileCapture } from "./MobileCapture";
import { BottomNav, type MobileTab } from "./BottomNav";
import { useTheme } from "./useTheme";
import { api, TokenError, OfflineError, type CategoryInfo, type EntityCandidate, type Envelope, type NoteFolderInfo, type NoteRow, type RangeEvent, type RelatedBrain } from "./api";
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
import { DocumentsView, LibraryView } from "./NotesView";
import { AskView } from "./AskView";
import { AgentContextApproval } from "./AgentContextApproval";
import { WeatherHome } from "./Weather";
import { APP_TZ, easternDay, easternHour } from "./day";
import {
  filingContextLabel,
  hasStoredFilingContext,
  isFilingContext,
  onFilingContextChange,
  readFilingContext,
  writeFilingContext,
  type FilingContext,
} from "./filingContext";
import "./App.css";
import { FloatingDock } from "./FloatingDock";
import { PRIMARY_DESTINATIONS } from "./navigation";
import { isDocumentNote } from "./library";

const TeamWorkspace = lazy(() => import("./teams/TeamWorkspace").then(module => ({ default: module.TeamWorkspace })));

type Phase = "idle" | "thinking" | "review";
type View = "team" | "today" | "ask" | "capture" | "documents" | "library" | "calendar" | "journal" | "knowledge" | "settings";

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

type SavedFilingMessage = {
  path: string;
  spaceId: number | null;
  reason: string;
  eventId: number | null;
  canUndo: boolean;
};

function folderIsWithin(folder: NoteFolderInfo, rootId: number, folders: NoteFolderInfo[]): boolean {
  let current: NoteFolderInfo | undefined = folder;
  const seen = new Set<number>();
  while (current && !seen.has(current.id)) {
    if (current.id === rootId) return true;
    seen.add(current.id);
    const parentId: number | null = current.parent_id;
    current = parentId == null
      ? undefined
      : folders.find((candidate) => candidate.id === parentId);
  }
  return false;
}

function folderPathLabel(folderId: number, folders: NoteFolderInfo[]): string {
  const names: string[] = [];
  let current = folders.find((folder) => folder.id === folderId);
  const seen = new Set<number>();
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    names.unshift(current.name);
    const parentId: number | null = current.parent_id;
    current = parentId == null
      ? undefined
      : folders.find((folder) => folder.id === parentId);
  }
  const path = names.join(" / ");
  const target = folders.find((folder) => folder.id === folderId);
  return target?.kind === "space" ? `${path} / Inbox` : path;
}

function folderSpaceId(folderId: number, folders: NoteFolderInfo[]): number | null {
  let current = folders.find((folder) => folder.id === folderId);
  const seen = new Set<number>();
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    if (current.parent_id == null) return current.kind === "space" ? current.id : null;
    current = folders.find((folder) => folder.id === current!.parent_id);
  }
  return null;
}

function isDailyStandupDraft(rawText: string, cards: ReviewCard[]): boolean {
  const categories = cards.map((card) => card.catName.trim().toLowerCase());
  if (categories.includes("schedule")) return false;
  const matches = (value: string) => {
    const lower = value.toLowerCase();
    const spaced = lower.replace(/[-_]/g, " ");
    return (
      lower.includes("standup") ||
      lower.includes("stand-up") ||
      spaced.includes("daily stand up") ||
      spaced.includes("stand up meeting") ||
      spaced.includes("daily scrum")
    );
  };
  return matches(rawText) || categories.some(matches);
}

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
  useMentionNotifications();
  const [notificationTarget, setNotificationTarget] = useState<TeamNotificationTarget | null>(null);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    const open = async () => {
      try {
        const target = await api.teamNotificationTakeTarget();
        if (alive && target) { setNotificationTarget(target); setView("team"); }
      } catch { /* Browser preview has no native notification queue. */ }
    };
    void listen("team-notification-open", () => { void open(); }).then((stop) => {
      if (!alive) { stop(); return; }
      unlisten = stop;
      void open();
    });
    return () => { alive = false; unlisten?.(); };
  }, []);
  const { theme, toggle } = useTheme();
  const [appTimeZone, setAppTimeZone] = useState(APP_TZ);
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
  const [folders, setFolders] = useState<NoteFolderInfo[]>([]);
  const [filingContext, setFilingContextState] = useState<FilingContext>(readFilingContext);
  const [reviewFilingContext, setReviewFilingContext] = useState<FilingContext | null>(null);
  const [destinationFolderId, setDestinationFolderId] = useState<number | null>(null);
  const [destinationTouched, setDestinationTouched] = useState(false);
  const [assistantOpen, setAssistantOpen] = useState(false);

  // Phone capture + backup
  const [showPhone, setShowPhone] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [savedMsg, setSavedMsg] = useState<SavedFilingMessage | null>(null);
  const [undoingSave, setUndoingSave] = useState(false);
  const [savingCapture, setSavingCapture] = useState(false);
  const savingCaptureRef = useRef(false);
  const [needsRepair, setNeedsRepair] = useState(false); // 403 from a stale phone token

  useEffect(() => onFilingContextChange(setFilingContextState), []);

  function chooseFilingContext(context: FilingContext) {
    setFilingContextState(context);
    if (phase === "review") setReviewFilingContext(context);
    writeFilingContext(context);
    const matchingSpace = folders.find(
      (folder) =>
        folder.kind === "space" && folder.name.trim().toLowerCase() === context
    );
    if (matchingSpace) {
      localStorage.setItem("noted-active-space", String(matchingSpace.id));
    }
    setDestinationFolderId(null);
    setDestinationTouched(false);
  }

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
      listen("meeting-summarized", () => refreshRef.current()),
      listen("assistant-shortcut", () => setAssistantOpen(true)),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, []);

  const [sideOpen, setSideOpen] = useState(false);
  const [navigationNote, setNavigationNote] = useState<NoteRow | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === "Space") {
        e.preventDefault();
        setAssistantOpen(true);
        return;
      }

    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Meetings: the open meeting page (a today sub-view) + the persistent
  // recording control in the sidebar.
  const [meetingOpen, setMeetingOpen] = useState<{
    id: number | null;
    event?: Partial<RangeEvent>;
  } | null>(null);
  const [recMeeting, setRecMeeting] = useState<{ id: number; title: string } | null>(null);
  const [meetingControlAction, setMeetingControlAction] = useState<"starting" | "stopping" | null>(null);
  const [meetingControlError, setMeetingControlError] = useState<string | null>(null);
  const [recordModeMenu, setRecordModeMenu] = useState(false);
  const recordMenuRef = useRef<HTMLDivElement>(null);
  const recordTriggerRef = useRef<HTMLButtonElement>(null);
  useOutsideDismiss(recordModeMenu, [recordMenuRef, recordTriggerRef], () => setRecordModeMenu(false));
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

  function openRecordingMeeting() {
    if (!recMeeting) return;
    setRecordModeMenu(false);
    setView("today");
    setMeetingOpen({ id: recMeeting.id });
  }

  // The sidebar control is the always-available manual path: it starts an
  // event-less recording immediately. While recording, opening the meeting and
  // stopping it are deliberately separate actions so returning to the live
  // transcript cannot end the capture by accident.
  // Calendar meetings still keep their richer pre-meeting page and metadata.
  async function toggleMeetingRecording(captureMode?: "online" | "in_person") {
    if (meetingControlAction) return;
    if (!recMeeting && !captureMode) {
      setRecordModeMenu((open) => !open);
      return;
    }
    const action = recMeeting ? "stopping" : "starting";
    setMeetingControlAction(action);
    setMeetingControlError(null);
    try {
      if (recMeeting) {
        await api.meetingStop();
      } else {
        const id = await api.meetingStart({ title: "Meeting", filingContext, captureMode });
        setRecordModeMenu(false);
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
  }, [notes, appTimeZone]);
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
    const [n, c, f] = await Promise.all([
      api.listNotes(),
      api.listCategories(),
      api.listNoteFolders(),
    ]);
    setNotes(n);
    setCats(c);
    setFolders(f);
    // Preserve the space selected by older builds the first time the explicit
    // Work/Personal context preference is introduced.
    if (!hasStoredFilingContext()) {
      const previousSpaceId = Number(localStorage.getItem("noted-active-space"));
      const previousSpace = f.find(
        (folder) => folder.id === previousSpaceId && folder.kind === "space"
      );
      const previousContext = previousSpace?.name.trim().toLowerCase();
      const migratedContext = isFilingContext(previousContext) ? previousContext : "work";
      setFilingContextState(migratedContext);
      writeFilingContext(migratedContext);
      const migratedSpace = f.find(
        (folder) =>
          folder.kind === "space" && folder.name.trim().toLowerCase() === migratedContext
      );
      if (migratedSpace) {
        localStorage.setItem("noted-active-space", String(migratedSpace.id));
      }
    }
  };

  // Phone web client only: watch the connection to the Mac and auto-recover when
  // it comes back (e.g. after a dev rebuild) — refetch health + data, no reload.
  const { online, markOffline } = useConnection({
    onReconnect: () => {
      api.health().catch(() => {});
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
    api.health().catch(handleErr);
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
  const activeFilingContext = reviewFilingContext ?? filingContext;
  const contextSpace = useMemo(
    () =>
      folders.find(
        (folder) =>
          folder.kind === "space" && folder.name.trim().toLowerCase() === activeFilingContext
      ),
    [activeFilingContext, folders]
  );
  const contextFilingTargets = useMemo(
    () =>
      contextSpace == null
        ? []
        : folders.filter((folder) => folderIsWithin(folder, contextSpace.id, folders)),
    [contextSpace, folders]
  );
  const automaticDestination =
    activeFilingContext === "work" &&
    isDailyStandupDraft(source === "photo" ? ocrText : text, cards)
      ? contextFilingTargets.find((folder) => folder.auto_rule === "daily_standup")
      : undefined;
  const resolvedDestinationId = destinationTouched
    ? destinationFolderId
    : automaticDestination?.id ?? contextSpace?.id ?? null;
  const resolvedDestinationPath =
    resolvedDestinationId == null ? "Needs filing" : folderPathLabel(resolvedDestinationId, folders);
  const destinationReason = destinationTouched
    ? `You chose ${resolvedDestinationPath}.`
    : automaticDestination
      ? "Matched your approved Daily Standup rule."
      : contextSpace
        ? `No folder rule matched. This stays in ${filingContextLabel(activeFilingContext)} Inbox.`
        : "The selected context is unavailable. This note will stay in Needs filing.";

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

  function enterReview(env: Envelope, src: Source, capturedContext: FilingContext) {
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
    setReviewFilingContext(capturedContext);
    setDestinationFolderId(null);
    setDestinationTouched(false);
    setSource(src);
    setPhase("review");
  }

  async function onCategorizeText() {
    if (!text.trim()) return;
    const capturedContext = filingContext;
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorize(text.trim()), "text", capturedContext);
    } catch (e) {
      setError(String(e));
      setPhase("idle");
    }
  }

  async function beginNoteReview({ text: draftText, img: draftImg }: { text: string; img: Img | null }) {
    const raw = draftText.trim();
    if (!raw && !draftImg) return;
    const capturedContext = filingContext;
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
      enterReview(env, draftImg ? "photo" : "text", capturedContext);
      setMeetingOpen(null);
      setView("capture");
    } catch (e) {
      setPhase("idle");
      setError(String(e));
      throw e;
    }
  }

  async function ingestPhoto(
    base64: string,
    ext: string,
    capturedContext: FilingContext = filingContext
  ) {
    setMeetingOpen(null);
    setView("capture");
    setImg({ base64, ext, dataUrl: `data:image/${ext === "jpg" ? "jpeg" : ext};base64,${base64}` });
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorizePhoto(base64), "photo", capturedContext);
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
        await ingestPhoto(base64, ext, readFilingContext());
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function onSave() {
    if (!cards.length || !allValid || savingCaptureRef.current) return;
    savingCaptureRef.current = true;
    setSavingCapture(true);
    setError(null);
    const fallbackPath = resolvedDestinationPath;
    const fallbackReason = destinationReason;
    const fallbackSpaceId = contextSpace?.id ?? null;
    const confirmedContextInbox =
      destinationTouched && resolvedDestinationId === contextSpace?.id;
    let noteId: number;
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
      noteId = await api.save({
        raw_text: source === "photo" ? ocrText : text.trim(),
        source,
        image_path,
        event_date: eventDate,
        entries,
        entities,
        filing_context: activeFilingContext,
        folder_id: destinationTouched ? resolvedDestinationId : undefined,
      });
    } catch (e) {
      setError(String(e));
      savingCaptureRef.current = false;
      setSavingCapture(false);
      return;
    }

    // The note is committed at this point. Leave review immediately so a
    // failed follow-up read can never invite a duplicate retry.
    resetAll();
    try {
      const nextFolders = await api.listNoteFolders();
      const savedPlacement = nextFolders
        .flatMap((folder) =>
          folder.explicit_filings.map((filing) => ({ folder, filing }))
        )
        .find(({ filing }) => filing.note_id === noteId);
      setSavedMsg({
        path: savedPlacement ? folderPathLabel(savedPlacement.folder.id, nextFolders) : "Needs filing",
        spaceId: savedPlacement
          ? folderSpaceId(savedPlacement.folder.id, nextFolders)
          : null,
        reason: savedPlacement?.filing.reason ?? "No filing context was recorded.",
        eventId: savedPlacement?.filing.event_id ?? null,
        canUndo:
          !confirmedContextInbox &&
          savedPlacement?.filing.event_id != null &&
          (savedPlacement.filing.source === "rule" || savedPlacement.filing.source === "manual"),
      });
      if (
        savedPlacement?.filing.source !== "rule" &&
        savedPlacement?.filing.source !== "manual"
      ) {
        window.setTimeout(() => setSavedMsg(null), 5000);
      }
    } catch (e) {
      console.warn("note saved, but filing details could not be refreshed", e);
      setSavedMsg({
        path: fallbackPath,
        spaceId: fallbackSpaceId,
        reason: `Saved successfully. ${fallbackReason}`,
        eventId: null,
        canUndo: false,
      });
    } finally {
      await refresh().catch((e) =>
        console.warn("note saved, but the library refresh is still pending", e)
      );
      savingCaptureRef.current = false;
      setSavingCapture(false);
    }
  }

  async function undoSavedFiling() {
    if (!savedMsg?.eventId || undoingSave) return;
    setUndoingSave(true);
    try {
      await api.undoNoteFiling(savedMsg.eventId);
      await refresh();
      setSavedMsg(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setUndoingSave(false);
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
    setReviewFilingContext(null);
    setDestinationFolderId(null);
    setDestinationTouched(false);
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
          <div className="brand"><img className="brand-wordmark" src={brandWordmark} alt="noted" draggable={false} /><span className="brand-classic">noted<span className="dot">.</span></span></div>
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
          <div className="brand"><img className="brand-wordmark" src={brandWordmark} alt="noted" draggable={false} /><span className="brand-classic">noted<span className="dot">.</span></span></div>
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
        <AgentContextApproval />
      </div>
    );
  }

  const dockDestinations = PRIMARY_DESTINATIONS.map(item => ({
    ...item,
    active: view === item.id || (item.id === "ask" && view === "capture"),
    onSelect: () => {
      if (item.id === "ask") goHome();
      else if (item.id === "today") {
        setMeetingOpen(null);
        setView("today");
        refresh().catch(handleErr);
      } else setView(item.id);
    },
  }));

  return (
    <div
      className={
        "app side dock-shell" +
        (view === "ask" || view === "capture"
          ? " homemode"
          : view === "calendar"
            ? " calmode"
            : view === "journal"
              ? " journalmode"
              : view === "knowledge"
                ? " graphmode"
                : view === "today"
                  ? " schedmode"
                  : view === "team"
                    ? " teammode"
                    : "")
      }
    >
      {reconnectingOverlay}
      <div className="titlebar-drag" data-tauri-drag-region />
      <FloatingDock open={sideOpen} onOpenChange={setSideOpen} notes={notes}
        onOpenNote={note => { setNavigationNote(note); setView(isDocumentNote(note) ? "documents" : "library"); }}
        recording={!!recMeeting}
        onRecording={() => { if (recMeeting) openRecordingMeeting(); else { setSideOpen(true); setRecordModeMenu(true); } }}
        destinations={dockDestinations}>
      <aside className="mission-utilities">
        <div className="side-head" data-tauri-drag-region>
          <div className="brand" data-tauri-drag-region><img className="brand-wordmark" src={brandWordmark} alt="noted" draggable={false} /><span className="brand-classic">noted<span className="dot">.</span></span></div>
        </div>
        <nav className="side-nav">
          {dockDestinations.map(item => <button key={item.id} className={item.active ? "on" : ""} onClick={item.onSelect}>
            <item.icon size={16} />{item.label}
          </button>)}
          {SHOW_JOURNAL && <button className={view === "journal" ? "on" : ""} onClick={() => setView("journal")}><BookOpen size={16} /> Journal</button>}
        </nav>
        <span className="spacer" data-tauri-drag-region />
        {recordModeMenu && !recMeeting && (
          <div ref={recordMenuRef} className="record-mode-menu" role="menu" aria-label="Recording type">
            <button role="menuitem" onClick={() => void toggleMeetingRecording("in_person")}>
              <Mic size={15} />
              <span><strong>In-person meeting</strong><small>Room microphone · separates speakers after</small></span>
            </button>
            <button role="menuitem" onClick={() => void toggleMeetingRecording("online")}>
              <Video size={15} />
              <span><strong>Online call</strong><small>Microphone + system audio</small></span>
            </button>
          </div>
        )}
        {recMeeting ? (
          <div className="sidebar-live-meeting" role="group" aria-label={`Recording ${recMeeting.title}`}>
            <button
              type="button"
              className="sidebar-live-open"
              onClick={openRecordingMeeting}
              disabled={meetingControlAction === "stopping"}
              title={`Open meeting: ${recMeeting.title}`}
              aria-current={meetingOpen?.id === recMeeting.id ? "page" : undefined}
            >
              <span className="bars" aria-hidden>
                <i />
                <i />
                <i />
              </span>
              <span className="sidebar-live-copy">
                <strong>Open meeting</strong>
                <small>{recMeeting.title}</small>
              </span>
            </button>
            <button
              type="button"
              className="sidebar-live-stop"
              onClick={() => void toggleMeetingRecording()}
              disabled={meetingControlAction != null}
              title={meetingControlAction === "stopping" ? "Stopping recording" : `Stop recording: ${recMeeting.title}`}
              aria-label={meetingControlAction === "stopping" ? "Stopping recording" : `Stop recording: ${recMeeting.title}`}
            >
              {meetingControlAction === "stopping" ? (
                <Loader size={12} className="spin" />
              ) : (
                <Square size={10} fill="currentColor" />
              )}
              <span aria-live="polite">{meetingControlAction === "stopping" ? "Wait" : "Stop"}</span>
            </button>
          </div>
        ) : (
          <button
            ref={recordTriggerRef}
            className="rec-pill"
            onClick={() => void toggleMeetingRecording()}
            disabled={meetingControlAction != null}
            title="Start a recording"
          >
            {meetingControlAction ? (
              <Loader size={13} className="spin" />
            ) : (
              <Mic size={14} />
            )}
            {meetingControlAction
              ? "Starting…"
              : recordModeMenu ? "Close" : "Start recording"}
          </button>
        )}
        {meetingControlError && (
          <span className="sidebar-record-error" role="alert">
            {meetingControlError}
          </span>
        )}
        <AppUpdateIndicator />
        <div className="side-foot">
          <button
            className="icon-btn"
            onClick={toggle}
            title={theme === "dark" ? "Switch to light" : "Switch to dark"}
            aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          >
            {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
          </button>
          {releaseProfile.iphoneCompanion && (
            <button className="icon-btn" onClick={() => setShowPhone(true)} title="Connect iPhone" aria-label="Connect iPhone">
              <Smartphone size={18} />
            </button>
          )}
          <button
            className={"icon-btn mission-settings" + (view === "settings" ? " on" : "")}
            onClick={() => setView("settings")}
            title="Settings"
          >
            <Settings size={18} />
          </button>
        </div>
      </aside>
      </FloatingDock>

      {/* Empty background = window drag, like a native app. The attribute only
          fires when the grabbed element IS the background (children keep their
          own clicks/selection), so content stays fully interactive. */}
      <div className="main-col" data-tauri-drag-region>
      <main className="content" data-tauri-drag-region>
        {view === "settings" ? (
          <SettingsModal page onClose={goHome} onOpenTeam={() => setView("team")} />
        ) : view === "ask" ? (
          <WeatherHome onTimeZoneChange={setAppTimeZone}>
            <AskView
              onMutated={() => refresh().catch(handleErr)}
              onSaveNote={beginNoteReview}
              filingContext={filingContext}
              onFilingContextChange={chooseFilingContext}
              onOpenEntity={(id) => {
                setKnowledgeEntity(id);
                setView("knowledge");
              }}
            />
          </WeatherHome>
        ) : view === "team" ? (
          <Suspense fallback={<p role="status">Opening team workspace…</p>}>
            <TeamWorkspace onOpenLibrary={(note) => {
              // A shared document's "Open in Library" lands on the note itself,
              // through the same requested-note path search results use.
              if (note) setNavigationNote(note);
              setView(note && isDocumentNote(note) ? "documents" : "library");
            }} notificationTarget={notificationTarget} onNotificationHandled={() => setNotificationTarget(null)} />
          </Suspense>
        ) : view === "documents" ? (
          <DocumentsView key="documents" notes={notes} cats={cats} requestedNote={navigationNote} onRequestedNoteOpened={() => setNavigationNote(null)} onChanged={() => refresh().catch(handleErr)} />
        ) : view === "library" ? (
          <LibraryView key="library" notes={notes} cats={cats} requestedNote={navigationNote} onRequestedNoteOpened={() => setNavigationNote(null)} onChanged={() => refresh().catch(handleErr)} />
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
            onTitleChanged={(meetingId, title) => {
              setRecMeeting((current) =>
                current?.id === meetingId ? { ...current, title } : current
              );
            }}
          />
        ) : view === "today" ? (
          <div className="schedule-page">
            <TodayView
              notes={notes}
              onSaved={() => refresh().catch(handleErr)}
              onOpenSettings={() => setView("settings")}
              lead={(
                <ComingUp
                  onOpenEvent={(ev) => {
                    if (ev.meet_link || ev.attendee_count >= 2) {
                      setMeetingOpen({ id: null, event: ev });
                    } else {
                      setView("calendar");
                    }
                  }}
                />
              )}
            />
          </div>
        ) : (
        <WeatherHome onTimeZoneChange={setAppTimeZone}>
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
            <label className="capture-context">
              <span>File in</span>
              <select
                value={filingContext}
                onChange={(event) => chooseFilingContext(event.target.value as FilingContext)}
                disabled={busy}
                aria-label="Filing context"
              >
                <option value="work">Work</option>
                <option value="personal">Personal</option>
              </select>
            </label>
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
          <section className="review" aria-busy={savingCapture}>
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
              <div className="filing-destination">
                <div className="filing-destination-controls">
                  <label htmlFor="review-filing-context">
                    <span>Context</span>
                    <select
                      id="review-filing-context"
                      value={activeFilingContext}
                      onChange={(event) =>
                        chooseFilingContext(event.target.value as FilingContext)
                      }
                    >
                      <option value="work">Work</option>
                      <option value="personal">Personal</option>
                    </select>
                  </label>
                  <label htmlFor="review-filing-folder">
                    <span>Save in</span>
                    <select
                      id="review-filing-folder"
                      value={resolvedDestinationId ?? ""}
                      disabled={contextSpace == null}
                      aria-describedby="review-filing-reason"
                      onChange={(event) => {
                        setDestinationFolderId(Number(event.target.value));
                        setDestinationTouched(true);
                      }}
                    >
                      {contextFilingTargets.map((folder) => {
                        const fullPath = folderPathLabel(folder.id, folders);
                        const label =
                          folder.id === contextSpace?.id
                            ? "Inbox"
                            : fullPath.replace(`${contextSpace?.name} / `, "");
                        return (
                          <option key={folder.id} value={folder.id}>
                            {label}
                          </option>
                        );
                      })}
                    </select>
                  </label>
                </div>
                <p id="review-filing-reason" role="status" aria-live="polite">
                  {destinationReason}
                </p>
              </div>
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
                        <label>Topic</label>
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
                        // Grow with the name being typed. `size` rather than CSS
                        // field-sizing, which the WebKit view cannot be relied on
                        // to support; max-width keeps a long name from pushing
                        // the chip's type and remove controls off the row.
                        size={Math.min(22, Math.max(5, e.name.length + 1))}
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
              <button onClick={resetAll} disabled={savingCapture}>Discard all</button>
              <button className="primary" onClick={onSave} disabled={!allValid || savingCapture}>
                {savingCapture
                  ? "Saving…"
                  : cards.length > 1
                  ? `Save ${cards.length} entries`
                  : "Save note"}
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
        <div className="saved-toast" role="status" aria-live="polite">
          <Check size={15} />
          <span>
            Saved in <strong>{savedMsg.path}</strong>
            <small>{savedMsg.reason}</small>
          </span>
          {savedMsg.canUndo && (
            <button onClick={() => void undoSavedFiling()} disabled={undoingSave}>
              {undoingSave ? "Undoing…" : "Undo"}
            </button>
          )}
          <button
            onClick={() => {
              if (savedMsg.spaceId != null) {
                localStorage.setItem("noted-active-space", String(savedMsg.spaceId));
              }
              setView("library");
            }}
          >
            View in Library
          </button>
          <button
            className="dismiss"
            onClick={() => setSavedMsg(null)}
            aria-label="Dismiss saved note message"
          >
            ×
          </button>
        </div>
      )}


      </div>

      <FloatingChat
        open={assistantOpen}
        onOpenChange={setAssistantOpen}
        onMutated={() => refresh().catch(handleErr)}
      />

      {releaseProfile.iphoneCompanion && showPhone && <PhonePanel onClose={() => setShowPhone(false)} />}
      <AgentContextApproval />
    </div>
  );
}
