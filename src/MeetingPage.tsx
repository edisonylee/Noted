// The meeting page: pre-meeting prep → live recording (notes front and center,
// transcript hidden behind a toggle, Granola-style) → summary tabs (PLAUD's
// multidimensional model: "+" regenerates with another template as a new tab).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  AudioLines,
  Check,
  ChevronDown,
  Copy,
  Download,
  FileDown,
  Folder,
  Loader,
  Mic,
  Pause,
  PenLine,
  Play,
  Plus,
  RotateCcw,
  Search,
  Share2,
  Sparkles,
  Square,
  Trash2,
  Users,
  Video,
} from "lucide-react";
import { listen } from "./events";
import { openPath } from "@tauri-apps/plugin-opener";
import { joinUrl } from "./joinUrl";
import { openExternalUrl } from "./openExternalUrl";
import {
  api,
  isDesktop,
  localFileUrl,
  type MeetingConversation,
  type MeetingDetail,
  type MeetingMicAec,
  type MeetingSegment,
  type MeetingSpeaker,
  type MeetingTemplate,
  type NoteFolderInfo,
  type PersonProfile,
  type RangeEvent,
} from "./api";
import { releaseProfile } from "./releaseProfile";
import { easternDay, formatDay } from "./day";
import { DocumentEditor } from "./editor/DocumentEditor";
import {
  documentPlainText,
  emptyDocument,
  storedDocumentOrPlainText,
  type StructuredDocument,
} from "./editor/document";
import {
  onFilingContextChange,
  readFilingContext,
  writeFilingContext,
  type FilingContext,
} from "./filingContext";
import { meetingMarkdown, transcriptMarkdown } from "./meetingMarkdown";

function mmss(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function fmtClock(min: number | null): string {
  if (min == null) return "";
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  const ampm = h >= 12 ? "pm" : "am";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return m === 0 ? `${h12}${ampm}` : `${h12}:${String(m).padStart(2, "0")}${ampm}`;
}

function fmtMeetingDay(day: string): string {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) return day;
  const sameYear = day.slice(0, 4) === easternDay().slice(0, 4);
  return formatDay(day, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

type SpeakerCandidate = {
  value: string;
  label: string;
  email: string | null;
  identities: string[];
};

function normalizedIdentity(value: string | null | undefined): string {
  return (value ?? "").trim().toLowerCase();
}

function emailDisplayName(value: string): string {
  const local = value.split("@")[0] ?? value;
  return local
    .split(/[._+\-\d]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function speakerDisplayName(value: string): string {
  if (value === "Them") return "Unassigned fragments";
  return value.includes("@") ? emailDisplayName(value) || value : value;
}

function copyWithSelection(text: string): boolean {
  const field = document.createElement("textarea");
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  field.value = text;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.appendChild(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();
  focused?.focus();
  return copied;
}

async function copyText(text: string): Promise<void> {
  // WKWebView does not consistently grant the async Clipboard API, while a
  // selection-based copy remains available inside a direct user gesture.
  if (isDesktop && copyWithSelection(text)) return;

  try {
    await navigator.clipboard.writeText(text);
  } catch (error) {
    if (copyWithSelection(text)) return;
    throw error;
  }
}

// Minimal markdown for our deterministic Meeting Pack output: hierarchy,
// tables, bullets, checkboxes, bold text, and grounded source jumps.
function MdBlock({ md, onSource }: { md: string; onSource?: (source: string) => void }) {
  const sourcePattern = /^\[(?:(\d{2,}:\d{2})(?:-\d{2,}:\d{2})?|(notes))\]$/i;
  const inline = (s: string) => {
    const parts = s.split(/(\*\*[^*]+\*\*|\[(?:\d{2,}:\d{2}(?:-\d{2,}:\d{2})?|notes)\])/gi);
    return parts.map((part, i) => {
      const bold = part.startsWith("**") && part.endsWith("**");
      const content = bold ? part.slice(2, -2) : part;
      const sourceMatch = content.match(sourcePattern);
      const source = sourceMatch?.[1] ?? sourceMatch?.[2];
      if (source) {
        const label = source.toLowerCase() === "notes" ? "My notes" : content.slice(1, -1);
        if (!onSource) {
          return <span key={i} className="summary-source static">{label}</span>;
        }
        return (
          <button
            key={i}
            type="button"
            className="summary-source"
            onClick={() => onSource(source)}
            aria-label={source.toLowerCase() === "notes" ? "Open My Notes" : `Open transcript at ${source}`}
          >
            {label}
          </button>
        );
      }
      return bold ? <strong key={i}>{content}</strong> : part;
    });
  };
  const lines = md.split("\n");
  const out: React.ReactNode[] = [];
  let list: React.ReactNode[] = [];
  let key = 0;
  const flush = () => {
    if (list.length) {
      out.push(<ul key={key++}>{list}</ul>);
      list = [];
    }
  };
  const tableCells = (line: string) => line.trim().replace(/^\||\|$/g, "").split("|").map((cell) => cell.trim());
  const tableDivider = (line: string) => {
    const cells = tableCells(line);
    return cells.length > 0 && cells.every((cell) => /^:?-{3,}:?$/.test(cell));
  };
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const line = lines[lineIndex];
    const t = line.trim();
    if (t.startsWith("|") && lines[lineIndex + 1] && tableDivider(lines[lineIndex + 1])) {
      flush();
      const headers = tableCells(t);
      const rows: string[][] = [];
      let rowIndex = lineIndex + 2;
      while (rowIndex < lines.length && lines[rowIndex].trim().startsWith("|")) {
        rows.push(tableCells(lines[rowIndex]));
        rowIndex += 1;
      }
      out.push(
        <div className="meeting-pack-table-wrap" key={key++}>
          <table className="meeting-pack-table">
            <thead><tr>{headers.map((header, index) => <th key={index}>{inline(header)}</th>)}</tr></thead>
            <tbody>{rows.map((row, rowKey) => (
              <tr key={rowKey}>{headers.map((_, cellKey) => <td key={cellKey}>{inline(row[cellKey] ?? "")}</td>)}</tr>
            ))}</tbody>
          </table>
        </div>
      );
      lineIndex = rowIndex - 1;
    } else if (t.startsWith("### ")) {
      flush();
      out.push(<h4 key={key++}>{t.slice(4)}</h4>);
    } else if (t.startsWith("## ")) {
      flush();
      out.push(<h3 key={key++}>{t.slice(3)}</h3>);
    } else if (t.startsWith("- [ ] ") || t.startsWith("- [x] ")) {
      list.push(
        <li key={key++} className="todo">
          <span className="box">{t[3] === "x" ? <Check size={11} /> : null}</span>
          <span>{inline(t.slice(6))}</span>
        </li>
      );
    } else if (t.startsWith("- ")) {
      list.push(<li key={key++}>{inline(t.slice(2))}</li>);
    } else if (t === "") {
      flush();
    } else {
      flush();
      out.push(<p key={key++}>{inline(t)}</p>);
    }
  }
  flush();
  return <div className="md">{out}</div>;
}

const TEMPLATE_COPY: Record<string, { label: string; description: string }> = {
  Meeting: { label: "General", description: "Comprehensive timeline, discussion, decisions, actions, and risks" },
  "1:1": { label: "1:1", description: "Detailed updates, feedback, support, workplan, and commitments" },
  Standup: { label: "Standup", description: "Progress, next work, blockers, and follow-ups" },
  Interview: { label: "Interview", description: "Evidence, themes, gaps, and follow-ups" },
  Lecture: { label: "Lecture", description: "Thesis, key ideas, examples, and implications" },
  "Project Update": { label: "Project update", description: "Status, milestones, decisions, and risks" },
  "Client Call": { label: "Client call", description: "Client priorities, commitments, and next steps" },
  Brainstorm: { label: "Brainstorm", description: "Ideas, tradeoffs, decisions, and experiments" },
};

function templateCopy(name: string): { label: string; description: string } {
  return TEMPLATE_COPY[name] ?? { label: name, description: "Your custom meeting-note structure" };
}

function recommendedTemplate(meetingType: MeetingDetail["meeting_type"]): string {
  if (meetingType === "daily_standup") return "Standup";
  if (meetingType === "one_on_one") return "1:1";
  return "Meeting";
}

function formatSpeechTime(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return remainder ? `${minutes}m ${remainder}s` : `${minutes}m`;
}

function segmentLabel(segment: MeetingSegment, inPerson: boolean): string {
  if (inPerson) return segment.speaker || "Unassigned";
  return segment.channel === "me"
    ? "Me"
    : releaseProfile.diarization
      ? segment.speaker || "Them"
      : "Them";
}

const ROOM_SPEAKER_TONES = 6;

function roomSpeakerTone(label: string, order: ReadonlyMap<string, number>): string {
  if (label === "Unassigned") return "room-speaker-unassigned";
  return `room-speaker-${(order.get(label) ?? 0) % ROOM_SPEAKER_TONES}`;
}

function ConversationDynamics({
  conversation,
  canRedetect,
  redetecting,
  onRedetect,
}: {
  conversation?: MeetingConversation;
  canRedetect: boolean;
  redetecting: boolean;
  onRedetect: () => void;
}) {
  const [open, setOpen] = useState(false);
  if (!conversation?.available || conversation.channels.length === 0) return null;

  const rows = conversation.speaker_detail_available
    ? conversation.speakers
    : conversation.channels;
  const you = conversation.channels.find((row) => row.channel === "me");
  const localSpeakerCount = conversation.speakers.some((row) => row.channel === "me") ? 1 : 0;
  const detailSummary = conversation.speaker_detail_available
    ? `${conversation.detected_remote_speakers + localSpeakerCount} estimated speakers · You ${Math.round(you?.share_pct ?? 0)}%`
    : `You ${Math.round(you?.share_pct ?? 0)}% · channel-level view`;
  const detailReason = (() => {
    if (conversation.speaker_detail_reason === "speaker_count_mismatch") {
      const expected = conversation.expected_remote_speakers;
      return expected == null
        ? "The detected speaker count is inconsistent, so per-speaker detail is hidden."
        : `Detected ${conversation.detected_remote_speakers} of ${expected} invited remote speakers, so per-speaker detail is hidden.`;
    }
    if (conversation.speaker_detail_reason === "low_attribution") {
      return "Too much remote speech is still unassigned, so per-speaker detail is hidden.";
    }
    if (conversation.speaker_detail_reason === "no_remote_speech") {
      return "No remote speech was captured for a participant breakdown.";
    }
    return null;
  })();

  return (
    <section className="conversation-dynamics">
      <button
        type="button"
        className="conversation-toggle"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span>
          <strong>Conversation dynamics</strong>
          <small>{detailSummary}</small>
        </span>
        <ChevronDown size={15} className={open ? "open" : ""} />
      </button>
      {open && (
        <div className="conversation-body">
          <p className="conversation-scope">Participation only — no engagement or emotion score.</p>
          {detailReason && (
            <div className="conversation-quality">
              <span>{detailReason}</span>
              {canRedetect && (
                <button type="button" onClick={onRedetect} disabled={redetecting}>
                  {redetecting ? <Loader size={12} className="spin" /> : <AudioLines size={12} />}
                  Re-detect speakers
                </button>
              )}
            </div>
          )}
          {conversation.speaker_detail_available &&
            conversation.unattributed_remote_pct != null &&
            conversation.unattributed_remote_pct > 0 && (
              <p className="conversation-attribution">
                {conversation.unattributed_remote_pct.toFixed(1)}% of remote speech couldn’t be assigned confidently.
              </p>
            )}
          <div className="conversation-rows">
            {rows.map((row) => (
              <div className="conversation-row" key={`${row.channel}:${row.label}`}>
                <div className="conversation-row-head">
                  <strong>{row.label}</strong>
                  <span>{Math.round(row.share_pct)}%</span>
                </div>
                <div className="conversation-track" aria-hidden="true">
                  <span style={{ width: `${Math.max(1, row.share_pct)}%` }} />
                </div>
                <p>
                  {formatSpeechTime(row.talk_ms)} speaking
                  {row.pace_wpm != null ? ` · ~${row.pace_wpm} words/min` : ""}
                  {` · ${row.speech_bursts} speaking ${row.speech_bursts === 1 ? "stretch" : "stretches"}`}
                </p>
              </div>
            ))}
          </div>
          <p className="conversation-method">
            {conversation.timing_basis === "voice_activity"
              ? "Share is each row’s portion of captured speaking time; overlapping channels may count the same moment. Pace appears only after at least one minute and 100 words of speech."
              : "Share is directional because this recording has only transcript segment spans. Pace is withheld without speech-only timing."}
          </p>
        </div>
      )}
    </section>
  );
}

type Tab = "notes" | "transcript" | "video" | number; // number = summary index
const EMPTY_NOTE_FOLDERS: NoteFolderInfo[] = [];

function transcriptionModelLabel(engine: string | null, model: string | null): string {
  if (!engine || !model) return "transcription model not recorded";
  if (engine === "whisper") {
    const name = model.replace(/^ggml-/, "").replace(/\.bin$/, "");
    return `transcribed with Whisper ${name}`;
  }
  if (engine === "parakeet") return "transcribed with Parakeet TDT 0.6B";
  if (engine === "hosted") return "transcribed with Hosted Parakeet";
  return `transcribed with ${engine} ${model}`;
}

function meetingFolderPath(folderId: number, folders: NoteFolderInfo[]): string {
  const names: string[] = [];
  const seen = new Set<number>();
  let current = folders.find((folder) => folder.id === folderId);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    names.unshift(current.name);
    current = current.parent_id == null
      ? undefined
      : folders.find((folder) => folder.id === current!.parent_id);
  }
  const folder = folders.find((candidate) => candidate.id === folderId);
  if (folder?.kind === "space") names.push("Inbox");
  return names.join(" / ");
}

function meetingFolderSpaceId(folderId: number, folders: NoteFolderInfo[]): number | null {
  const seen = new Set<number>();
  let current = folders.find((folder) => folder.id === folderId);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    if (current.parent_id == null) return current.kind === "space" ? current.id : null;
    current = folders.find((folder) => folder.id === current!.parent_id);
  }
  return null;
}

function displayMeetingFolderPath(path: string): string {
  return path.split(" / ").join(" › ");
}

/**
 * What to tell the user about echo cancellation during a recording.
 *
 * Returns null when voice processing is doing its job — there is nothing to warn
 * about and a permanent banner would just be noise. Otherwise it explains the
 * cost, because without it the far side of a call can be picked up by the mic
 * and attributed to you in the transcript.
 */
function micAecNotice(info: MeetingMicAec | null): string | null {
  if (!info || info.state === "active") return null;
  const consequence =
    "Speaker audio can be picked up by your mic and transcribed as you — use headphones for clean speaker labels.";
  switch (info.state) {
    case "yielded":
      return `Echo cancellation is off so ${info.app ?? "another app"} keeps your microphone — turning it on here would mute you for everyone else on the call. ${consequence}`;
    case "unavailable":
      return `Echo cancellation could not start on this microphone, so it is recording raw. ${consequence}`;
    default:
      return `Echo cancellation is off. ${consequence}`;
  }
}

export function MeetingPage({
  id,
  event,
  onBack,
  onStarted,
  onTitleChanged,
  focusSegmentId,
}: {
  id: number | null; // null = pre-meeting page for a calendar event
  event?: Partial<RangeEvent> | null;
  onBack: () => void;
  onStarted?: (id: number) => void;
  onTitleChanged?: (id: number, title: string) => void;
  focusSegmentId?: number;
}) {
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [liveSegments, setLiveSegments] = useState<MeetingSegment[]>([]);
  const [notes, setNotes] = useState("");
  const [notesDocument, setNotesDocument] = useState<StructuredDocument>(emptyDocument);
  const [notesSaveState, setNotesSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [tab, setTab] = useState<Tab>(id == null ? "notes" : "transcript");
  const [elapsed, setElapsed] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [templates, setTemplates] = useState<MeetingTemplate[]>([]);
  const [pickTemplate, setPickTemplate] = useState(false);
  const [generating, setGenerating] = useState<string | null>(null);
  const [people, setPeople] = useState<PersonProfile[]>([]);
  const [speakerEditorOpen, setSpeakerEditorOpen] = useState(false);
  const [speakerDrafts, setSpeakerDrafts] = useState<Record<string, string>>({});
  const [speakerSaving, setSpeakerSaving] = useState(false);
  const [speakerSaveMessage, setSpeakerSaveMessage] = useState<string | null>(null);
  const [rediarizing, setRediarizing] = useState(false);
  const [assistQ, setAssistQ] = useState("");
  const [assistA, setAssistA] = useState<string | null>(null);
  const [assistBusy, setAssistBusy] = useState(false);
  const [liveInsight, setLiveInsight] = useState<string | null>(null);
  const [autoAssistOn, setAutoAssistOn] = useState(true);
  const [autoAssistBusy, setAutoAssistBusy] = useState(false);
  const [autoAssistError, setAutoAssistError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [playingSeg, setPlayingSeg] = useState<number | null>(null);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  const [shareOpen, setShareOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [titleSaving, setTitleSaving] = useState(false);
  const [folders, setFolders] = useState<NoteFolderInfo[] | null>(null);
  const [folderPickerOpen, setFolderPickerOpen] = useState(false);
  const [folderQuery, setFolderQuery] = useState("");
  const [folderLoadError, setFolderLoadError] = useState(false);
  const [filingSaving, setFilingSaving] = useState(false);
  const [filingStatus, setFilingStatus] = useState<string | null>(null);
  const [editingSummary, setEditingSummary] = useState<number | null>(null);
  const [summaryDraft, setSummaryDraft] = useState("");
  const [summarySaving, setSummarySaving] = useState(false);
  const [copiedSummary, setCopiedSummary] = useState<number | null>(null);
  const [filingContext, setFilingContext] = useState<FilingContext>(readFilingContext);
  const [evidenceSegmentId, setEvidenceSegmentId] = useState<number | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const notesTimer = useRef<number | null>(null);
  const copyTimer = useRef<number | null>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const speakerInputRefs = useRef<Record<string, HTMLInputElement | null>>({});
  const folderPickerRef = useRef<HTMLDivElement>(null);
  const folderSearchRef = useRef<HTMLInputElement>(null);
  const filingStatusTimer = useRef<number | null>(null);
  const initialTabSelectedFor = useRef<number | null>(null);
  const loadedNotesFor = useRef<number | undefined>(undefined);

  useEffect(() => onFilingContextChange(setFilingContext), []);
  const assistInputRef = useRef<HTMLInputElement>(null);
  const autoAssistTimer = useRef<number | null>(null);
  const autoAssistBusyRef = useRef(false);
  const lastAutoSegment = useRef(0);
  const lastAutoAt = useRef(0);
  const activeMeetingId = useRef(id);
  activeMeetingId.current = id;

  const recording = detail?.status === "recording";
  const summarizing = detail?.status === "summarizing" || generating != null;
  // How the mic is being captured right now, reported by the capture thread.
  const [micAec, setMicAec] = useState<MeetingMicAec | null>(null);

  const load = useCallback(async () => {
    if (id == null) return;
    try {
      const d = await api.meetingGet(id);
      setDetail(d);
      setLiveSegments(d.segments);
      if (loadedNotesFor.current !== id) {
        loadedNotesFor.current = id;
        setNotes(d.raw_notes);
        setNotesDocument(storedDocumentOrPlainText(d.notes_document_json, d.raw_notes));
        setNotesSaveState("saved");
      }
      // Choose the completed-meeting landing tab once. Background refreshes and
      // speaker edits must never pull someone out of the transcript they are
      // actively reviewing.
      if (initialTabSelectedFor.current !== id) {
        initialTabSelectedFor.current = id;
        setTab((t) =>
          focusSegmentId == null && t === "transcript" && d.summaries.length > 0 && d.status === "done"
            ? 0
            : t
        );
      }
    } catch (e) {
      setError(String(e));
    }
  }, [focusSegmentId, id]);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    api.meetingTemplates().then(setTemplates).catch(() => {});
    api.listPeople().then(setPeople).catch(() => {});
    api.listNoteFolders()
      .then((items) => {
        setFolders(items);
        setFolderLoadError(false);
      })
      .catch(() => {
        setFolders([]);
        setFolderLoadError(true);
      });
  }, []);

  useEffect(() => {
    setTab(id == null ? "notes" : "transcript");
    setSpeakerEditorOpen(false);
    setSpeakerDrafts({});
    setSpeakerSaveMessage(null);
    setFolderPickerOpen(false);
    setFolderQuery("");
    setFilingStatus(null);
  }, [focusSegmentId, id]);

  useEffect(() => {
    loadedNotesFor.current = undefined;
    setNotes("");
    setNotesDocument(emptyDocument());
    setNotesSaveState("idle");
    if (notesTimer.current) window.clearTimeout(notesTimer.current);
  }, [id]);

  useEffect(() => {
    return () => {
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      if (filingStatusTimer.current) window.clearTimeout(filingStatusTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!folderPickerOpen) return;
    folderSearchRef.current?.focus();
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!folderPickerRef.current?.contains(event.target as Node)) {
        setFolderPickerOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setFolderPickerOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [folderPickerOpen]);

  // A meeting switch starts a fresh copilot session. Insights are deliberately
  // ephemeral: they are prompts for the moment, not another source of notes.
  useEffect(() => {
    setLiveInsight(null);
    setAssistA(null);
    setAutoAssistError(null);
    setAutoAssistOn(true);
    setAutoAssistBusy(false);
    autoAssistBusyRef.current = false;
    if (autoAssistTimer.current) window.clearTimeout(autoAssistTimer.current);
    lastAutoSegment.current = 0;
    lastAutoAt.current = 0;
    setEvidenceSegmentId(null);
  }, [id]);

  // Live updates: segments stream in; stopped/summarized refresh the page.
  useEffect(() => {
    setMicAec(null); // a different meeting decides its own capture strategy
    if (id == null) return;
    const subs = [
      listen<MeetingSegment & { meetingId: number }>("meeting-segment", (e) => {
        if (e.payload.meetingId !== id) return;
        setLiveSegments((prev) => [...prev, e.payload]);
      }),
      // A "me" line recognized late as the mic hearing the speakers (echo of
      // remote speech) gets deleted by the worker — drop it from the view too.
      listen<{ meetingId: number; id: number }>("meeting-segment-removed", (e) => {
        if (e.payload.meetingId !== id) return;
        setLiveSegments((prev) => prev.filter((s) => s.id !== e.payload.id));
      }),
      // Provisional speaker labels stream in as diarization clusters live;
      // the final pass at stop triggers a full reload anyway.
      listen<{ meetingId: number; labels: { id: number; label: string }[] }>(
        "meeting-speakers-updated",
        (e) => {
          if (e.payload.meetingId !== id) return;
          const byId = new Map(e.payload.labels.map((l) => [l.id, l.label]));
          setLiveSegments((prev) =>
            prev.map((s) => (byId.has(s.id) ? { ...s, speaker: byId.get(s.id)! } : s)),
          );
        },
      ),
      // Echo cancellation is decided at capture time, not from settings alone:
      // a live call keeps the mic, so the answer can differ from the toggle and
      // can change mid-recording.
      listen<MeetingMicAec>("meeting-mic-aec", (e) => {
        if (e.payload.meetingId !== id) return;
        setMicAec(e.payload);
      }),
      listen<{ meetingId: number }>("meeting-stopped", (e) => {
        if (e.payload.meetingId === id) load();
      }),
      listen<{ meetingId: number }>("meeting-summarized", (e) => {
        if (e.payload.meetingId === id) load();
      }),
      // The window video finalizes shortly after stop, off the stop path.
      listen<{ meetingId: number }>("meeting-video-ready", (e) => {
        if (e.payload.meetingId === id) load();
      }),
      listen<{ meetingId: number }>("meeting-speakers-suggested", (e) => {
        if (e.payload.meetingId === id) load();
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, [id, load]);

  // Recording clock.
  useEffect(() => {
    if (!recording) return;
    const started = detail?.started_at ? Date.parse(detail.started_at) : Date.now();
    const t = window.setInterval(() => setElapsed(Date.now() - started), 1000);
    return () => window.clearInterval(t);
  }, [recording, detail?.started_at]);

  // Rich notes autosave (debounced) once a meeting row exists. The plain-text
  // projection remains available to summarization, export, and older clients.
  const onNotesDocument = (document: StructuredDocument) => {
    const plainText = documentPlainText(document);
    setNotesDocument(document);
    setNotes(plainText);
    if (id == null) {
      setNotesSaveState("idle");
      return; // pre-meeting: passed along at start
    }
    setNotesSaveState("saving");
    if (notesTimer.current) window.clearTimeout(notesTimer.current);
    notesTimer.current = window.setTimeout(() => {
      api.meetingSetNotes(id, plainText, JSON.stringify(document))
        .then(() => {
          if (activeMeetingId.current === id) setNotesSaveState("saved");
        })
        .catch(() => {
          if (activeMeetingId.current === id) setNotesSaveState("error");
        });
    }, 800);
  };

  // Keep the live transcript pinned to the newest line when already at bottom.
  useEffect(() => {
    const el = transcriptRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [liveSegments.length]);

  useEffect(() => {
    const targetSegmentId = focusSegmentId ?? evidenceSegmentId;
    if (targetSegmentId == null || tab !== "transcript") return;
    const frame = window.requestAnimationFrame(() => {
      const transcript = transcriptRef.current;
      const line = transcript?.querySelector<HTMLElement>(
        `[data-segment-id="${targetSegmentId}"]`
      );
      if (!transcript || !line) return;
      const top =
        line.offsetTop -
        transcript.offsetTop -
        Math.max(0, (transcript.clientHeight - line.clientHeight) / 2);
      transcript.scrollTop = Math.max(0, top);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [evidenceSegmentId, focusSegmentId, liveSegments.length, tab]);

  const ev = detail?.event_json ?? event ?? null;
  const title = detail?.title ?? ev?.title ?? "Meeting";
  const inPerson = detail?.capture_mode === "in_person";
  const attendees = useMemo(() => (ev?.attendees ?? []).filter((attendee) => !attendee.self), [ev?.attendees]);
  const meetLink = ev?.meet_link ? joinUrl(ev.meet_link, ev.account) : null;

  const notesRef = useRef(notes);
  notesRef.current = notes;
  const notesDocumentRef = useRef(notesDocument);
  notesDocumentRef.current = notesDocument;

  const start = useCallback(async (captureMode: "online" | "in_person") => {
    setStarting(true);
    setError(null);
    try {
      const newId = await api.meetingStart({
        title,
        eventId: ev?.id ?? undefined,
        eventJson: ev ?? undefined,
        filingContext,
        captureMode,
      });
      const prep = notesRef.current;
      if (prep.trim()) {
        await api.meetingSetNotes(newId, prep, JSON.stringify(notesDocumentRef.current)).catch(() => {});
      }
      onStarted?.(newId);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [title, ev?.id, filingContext]);

  // Deliberately NO auto-start here. Recording begins only from an explicit
  // click — this page's Record button or the detection prompt (which fires at
  // T-60s even when this page is open, so nothing is lost). Ending is the
  // automatic half: the detector stops when the call app releases the mic.

  const stop = async () => {
    setStopping(true);
    setError(null);
    let notesSaveError: unknown = null;
    try {
      if (id != null) {
        if (notesTimer.current) window.clearTimeout(notesTimer.current);
        setNotesSaveState("saving");
        try {
          await api.meetingSetNotes(
            id,
            notesRef.current,
            JSON.stringify(notesDocumentRef.current)
          );
          setNotesSaveState("saved");
        } catch (reason) {
          notesSaveError = reason;
          setNotesSaveState("error");
        }
      }
      await api.meetingStop();
      await load();
      if (notesSaveError) {
        setError(`Recording stopped, but the latest notes could not be saved: ${String(notesSaveError)}`);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setStopping(false);
    }
  };

  const generate = async (template: string) => {
    if (id == null) return;
    setPickTemplate(false);
    setGenerating(template);
    setError(null);
    try {
      await api.meetingSummarize(id, template);
      await load();
      const existing = summaries.findIndex((s) => s.template === template);
      setTab(existing >= 0 ? existing : summaries.length);
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(null);
    }
  };

  const summaries = detail?.summaries ?? [];
  const recommendedSummaryTemplate = recommendedTemplate(detail?.meeting_type);
  const remainingTemplates = templates
    .filter((template) => !summaries.some((summary) => summary.template === template.name))
    .sort((a, b) => {
      if (a.name === recommendedSummaryTemplate) return -1;
      if (b.name === recommendedSummaryTemplate) return 1;
      if (a.builtin !== b.builtin) return a.builtin ? -1 : 1;
      return templateCopy(a.name).label.localeCompare(templateCopy(b.name).label);
    });

  const saveTitle = async () => {
    const next = titleDraft.trim();
    if (id == null || !next || titleSaving) return;
    setTitleSaving(true);
    setError(null);
    try {
      await api.meetingSetTitle(id, next);
      setDetail((current) => (current ? { ...current, title: next } : current));
      onTitleChanged?.(id, next);
      setEditingTitle(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setTitleSaving(false);
    }
  };

  const folderItems = folders ?? EMPTY_NOTE_FOLDERS;
  const destinationPath = detail?.route_folder_id != null
    ? meetingFolderPath(detail.route_folder_id, folderItems)
    : detail?.filing_context
      ? `${detail.filing_context === "work" ? "Work" : "Personal"} / Inbox`
      : "Choose a folder";
  const folderSections = useMemo(() => {
    const normalizedQuery = folderQuery.trim().toLocaleLowerCase();
    return folderItems
      .filter((folder) => folder.kind === "space" && folder.parent_id == null)
      .map((space) => ({
        space,
        folders: folderItems
          .filter((folder) => meetingFolderSpaceId(folder.id, folderItems) === space.id)
          .map((folder) => ({ folder, path: meetingFolderPath(folder.id, folderItems) }))
          .filter(({ path }) => !normalizedQuery || path.toLocaleLowerCase().includes(normalizedQuery)),
      }))
      .filter((section) => section.folders.length > 0);
  }, [folderItems, folderQuery]);

  const chooseDestination = async (folder: NoteFolderInfo, path: string) => {
    if (id == null || filingSaving) return;
    setFilingSaving(true);
    setError(null);
    try {
      await api.meetingSetFilingDestination(id, folder.id);
      const spaceId = meetingFolderSpaceId(folder.id, folderItems);
      const space = folderItems.find((candidate) => candidate.id === spaceId);
      const context = space?.name.toLocaleLowerCase() === "personal" ? "personal" : "work";
      setDetail((current) => current ? {
        ...current,
        route_folder_id: folder.id,
        route_email: null,
        route_via: "manual",
        route_status: "manual",
        filing_context: context,
      } : current);
      setFolderPickerOpen(false);
      setFolderQuery("");
      setFilingStatus(`Filed in ${displayMeetingFolderPath(path)}`);
      if (filingStatusTimer.current) window.clearTimeout(filingStatusTimer.current);
      filingStatusTimer.current = window.setTimeout(() => setFilingStatus(null), 2600);
    } catch (e) {
      setError(String(e));
    } finally {
      setFilingSaving(false);
    }
  };

  const saveSummary = async () => {
    if (id == null || editingSummary == null || summarySaving) return;
    setSummarySaving(true);
    setError(null);
    try {
      await api.meetingSetSummary(id, editingSummary, summaryDraft);
      setDetail((current) =>
        current
          ? {
              ...current,
              summaries: current.summaries.map((summary) =>
                summary.id === editingSummary
                  ? { ...summary, content_md: summaryDraft }
                  : summary
              ),
            }
          : current
      );
      setEditingSummary(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSummarySaving(false);
    }
  };

  const copySummary = async (summaryId: number, content: string) => {
    try {
      await copyText(content);
      setCopiedSummary(summaryId);
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopiedSummary(null), 1800);
    } catch (e) {
      setError(`Could not copy notes: ${String(e)}`);
    }
  };

  const storedSpeakers = detail?.speakers ?? [];
  // Old/manual/recovered recordings may have remote transcript lines without
  // a usable diarization cluster. Keep those labels in the review flow too;
  // choosing stored rows exclusively used to hide leftover "Them" fragments.
  const fallbackSpeakerCounts = new Map<string, number>();
  for (const segment of liveSegments) {
    if (!inPerson && segment.channel !== "them") continue;
    if (inPerson && !segment.speaker?.trim()) continue;
    const label = inPerson ? segment.speaker! : segment.speaker || "Them";
    fallbackSpeakerCounts.set(label, (fallbackSpeakerCounts.get(label) ?? 0) + 1);
  }
  const speakerMap = new Map<string, MeetingSpeaker>();
  for (const [label, seg_count] of fallbackSpeakerCounts) {
    speakerMap.set(label, { label, suggested: null, seg_count });
  }
  for (const speaker of storedSpeakers) {
    speakerMap.set(speaker.label, {
      ...speaker,
      seg_count: Math.max(speaker.seg_count, fallbackSpeakerCounts.get(speaker.label) ?? 0),
    });
  }
  const speakers = [...speakerMap.values()].sort(
    (a, b) => b.seg_count - a.seg_count || a.label.localeCompare(b.label)
  );
  const unnamed = (l: string) => l === "Speaker" || l.startsWith("Speaker ") || l === "Them";

  // A room recording has one physical source (the Mac microphone), but many
  // participant identities after FluidAudio diarization. Keep the source
  // channel for playback while assigning a stable visual identity in order of
  // first appearance. Treating channel="me" as identity made every person look
  // like the user in the transcript even when their speaker label was correct.
  const roomSpeakerOrder = useMemo(() => {
    const order = new Map<string, number>();
    for (const segment of [...liveSegments].sort((a, b) => a.t0_ms - b.t0_ms || a.id - b.id)) {
      const label = segmentLabel(segment, true);
      if (!order.has(label)) order.set(label, order.size);
    }
    return order;
  }, [liveSegments]);

  // Suggestions are grounded, local, and confirm-only. Calendar attendees are
  // resolved through People aliases so an invite email can appear as the
  // canonical person name; no voiceprint or model guess can assign a label.
  const speakerCandidates = useMemo<SpeakerCandidate[]>(() => {
    const out: SpeakerCandidate[] = [];
    const seen = new Set<string>();
    for (const attendee of attendees) {
      const attendeeIds = [attendee.email, attendee.name]
        .map(normalizedIdentity)
        .filter(Boolean);
      const person = people.find((candidate) => {
        const personIds = [candidate.name, ...candidate.aliases]
          .map(normalizedIdentity)
          .filter(Boolean);
        return attendeeIds.some((identity) => personIds.includes(identity));
      });
      const rawName = attendee.name?.trim() ?? "";
      const email = attendee.email?.trim() || null;
      const label =
        person?.name ||
        (rawName && !rawName.includes("@") ? rawName : "") ||
        (email ? emailDisplayName(email) : rawName) ||
        "Invitee";
      const value = person?.name || (rawName && !rawName.includes("@") ? rawName : "") || email || label;
      const identities = [...new Set(
        [value, label, email, rawName, person?.name, ...(person?.aliases ?? [])]
          .map(normalizedIdentity)
          .filter(Boolean)
      )];
      const key = normalizedIdentity(email || value);
      if (!key || seen.has(key)) continue;
      seen.add(key);
      out.push({ value, label, email, identities });
    }
    return out;
  }, [attendees, people]);

  const candidateForLabel = (label: string) => {
    const identity = normalizedIdentity(label);
    return speakerCandidates.find((candidate) => candidate.identities.includes(identity));
  };

  const speakerSamples = useMemo(() => {
    const samples = new Map<string, MeetingSegment>();
    for (const segment of [...liveSegments].sort((a, b) => a.t0_ms - b.t0_ms || a.id - b.id)) {
      if ((!inPerson && segment.channel !== "them") || !segment.text.trim()) continue;
      const label = inPerson ? segment.speaker || "Speaker" : segment.speaker || "Them";
      const current = samples.get(label);
      if (!current || (current.text.length < 28 && segment.text.length > current.text.length)) {
        samples.set(label, segment);
      }
    }
    return samples;
  }, [inPerson, liveSegments]);

  const openSpeakerEditor = (focusLabel?: string) => {
    const drafts: Record<string, string> = {};
    for (const speaker of speakers) {
      drafts[speaker.label] = unnamed(speaker.label)
        ? ""
        : candidateForLabel(speaker.label)?.value ?? speaker.label;
    }
    setSpeakerDrafts(drafts);
    setSpeakerSaveMessage(null);
    setSpeakerEditorOpen(true);
    if (focusLabel) {
      window.requestAnimationFrame(() => speakerInputRefs.current[focusLabel]?.focus());
    }
  };

  const saveSpeakerLabels = async () => {
    if (id == null || speakerSaving) return;
    const changes = speakers
      .map((speaker) => ({ from: speaker.label, to: (speakerDrafts[speaker.label] ?? "").trim() }))
      .filter((change) => change.to && change.to !== change.from);
    if (changes.length === 0) {
      setSpeakerEditorOpen(false);
      return;
    }
    setSpeakerSaving(true);
    setError(null);
    setSpeakerSaveMessage("Saving labels and updating meeting notes…");
    try {
      const result = await api.meetingRenameSpeakers(id, changes);
      await load();
      setSpeakerEditorOpen(false);
      const saved = `${result.speakers_updated} speaker ${result.speakers_updated === 1 ? "label" : "labels"} saved`;
      setSpeakerSaveMessage(
        result.summary_refresh_error
          ? `${saved}; meeting notes could not refresh`
          : result.summaries_refreshed > 0
            ? `${saved}; meeting notes updated`
            : saved,
      );
    } catch (e) {
      setSpeakerSaveMessage(null);
      setError(`Could not save every speaker label: ${String(e)}`);
    } finally {
      setSpeakerSaving(false);
    }
  };
  const speakerChangeCount = speakers.filter((speaker) => {
    const next = (speakerDrafts[speaker.label] ?? "").trim();
    return next && next !== speaker.label;
  }).length;

  // Live Assist A0: ask against this meeting's transcript-so-far (works
  // mid-recording — the model sees the rolling tail).
  const askAssist = async () => {
    const q = assistQ.trim();
    if (id == null || !q || assistBusy) return;
    setAssistBusy(true);
    try {
      const res = await api.meetingAssist(id, q);
      setAssistA(res.answer);
      setAssistQ("");
    } catch (e) {
      setAssistA(String(e));
    } finally {
      setAssistBusy(false);
    }
  };

  const requestLiveInsight = useCallback(
    async (segmentCount: number) => {
      if (id == null || autoAssistBusyRef.current) return;
      autoAssistBusyRef.current = true;
      setAutoAssistBusy(true);
      setAutoAssistError(null);
      try {
        const previous = liveInsight
          ? ` Your previous suggestion was: “${liveInsight}” Do not repeat it unless the new discussion changes it.`
          : "";
        const res = await api.meetingAssist(
          id,
          "Act as a proactive live meeting copilot. Based only on what has changed in the " +
            "latest discussion, give me ONE immediately useful insight. Prefer: (1) a direct " +
            "answer or ready-to-say response I may need next, (2) a risk, objection, or " +
            "contradiction worth flagging, (3) a decision or action item that could be missed, " +
            "or (4) a sharp follow-up question. Do not give a generic summary. Keep it to 1-3 " +
            "short sentences. Never invent a person, owner, date, fact, or commitment; call out " +
            "missing ownership explicitly. Give only the insight, with no preamble, markdown, or " +
            "offer to do more. If there is nothing meaningfully useful yet, reply exactly NO_UPDATE." +
            previous,
        );
        const answer = res.answer.trim();
        if (activeMeetingId.current === id && answer && !/^NO_UPDATE[.!]?$/i.test(answer)) {
          setLiveInsight(answer);
        }
      } catch {
        // Keep the last useful card visible; a live transient should not replace
        // it with a networking error or interrupt note-taking.
        if (activeMeetingId.current === id) {
          setAutoAssistError("Live suggestions will retry when more conversation arrives.");
        }
      } finally {
        if (activeMeetingId.current === id) {
          lastAutoSegment.current = segmentCount;
          lastAutoAt.current = Date.now();
          autoAssistBusyRef.current = false;
          setAutoAssistBusy(false);
        }
      }
    },
    [id, liveInsight],
  );

  // Debounce natural speech bursts and cap model traffic. Two meaningful
  // segments are enough to start, then at least two new segments prompt the
  // next look. The 18-second floor keeps Hosted/BYOK usage predictable.
  useEffect(() => {
    if (autoAssistTimer.current) window.clearTimeout(autoAssistTimer.current);
    if (!recording || !autoAssistOn || id == null || autoAssistBusy) return;

    const meaningful = liveSegments.filter((s) => s.text.trim().split(/\s+/).length >= 3).length;
    if (meaningful < 2) return;
    const unseen = liveSegments.length - lastAutoSegment.current;
    if (lastAutoSegment.current > 0 && unseen < 2) return;

    const sinceLast = Date.now() - lastAutoAt.current;
    const delay = Math.max(7_000, 18_000 - sinceLast);
    const segmentCount = liveSegments.length;
    autoAssistTimer.current = window.setTimeout(() => {
      autoAssistTimer.current = null;
      requestLiveInsight(segmentCount);
    }, delay);
    return () => {
      if (autoAssistTimer.current) window.clearTimeout(autoAssistTimer.current);
    };
  }, [
    autoAssistBusy,
    autoAssistOn,
    id,
    liveSegments,
    recording,
    requestLiveInsight,
  ]);

  // The panel is always visible during a recording; this shortcut is only a
  // convenience for asking without leaving the keyboard.
  useEffect(() => {
    const focusAssist = (e: KeyboardEvent) => {
      if (recording && e.metaKey && e.shiftKey && e.key.toLowerCase() === "a") {
        e.preventDefault();
        assistInputRef.current?.focus();
      }
    };
    window.addEventListener("keydown", focusAssist);
    return () => window.removeEventListener("keydown", focusAssist);
  }, [recording]);

  // Rebuild labels from retained audio. This also repairs stale or incorrect
  // labels using the current attendee-scoped naming policy.
  const rediarize = async () => {
    if (id == null) return;
    setRediarizing(true);
    setSpeakerSaveMessage("Detecting speakers and updating meeting notes…");
    try {
      const result = await api.meetingRediarize(id);
      if (result.speakers_updated > 0) await load();
      setSpeakerSaveMessage(
        result.speakers_updated === 0
          ? "No speaker groups were detected"
          : result.summary_refresh_error
            ? `${result.speakers_updated} speakers detected; meeting notes could not refresh`
            : result.summaries_refreshed > 0
              ? `${result.speakers_updated} speakers detected; meeting notes updated`
              : `${result.speakers_updated} speakers detected`,
      );
    } catch (e) {
      setSpeakerSaveMessage(null);
      setError(String(e));
    } finally {
      setRediarizing(false);
    }
  };

  const exportMd = async () => {
    if (id == null) return;
    try {
      const path = await api.meetingExportMd(id);
      setExportMsg(`Saved to ${path.split("/").slice(-2).join("/")}`);
      window.setTimeout(() => setExportMsg(null), 4000);
    } catch (e) {
      setError(String(e));
    }
  };

  const copyMarkdown = async (kind: "meeting" | "transcript") => {
    const selectedSummary = typeof tab === "number" ? summaries[tab] : summaries[0];
    const markdown = kind === "transcript"
      ? transcriptMarkdown(title, liveSegments, releaseProfile.diarization, detail?.capture_mode)
      : meetingMarkdown({
          title,
          startedAt: detail?.started_at,
          attendeeNames: attendees.map((attendee) => attendee.name || attendee.email),
          summary: selectedSummary,
          notes,
          segments: liveSegments,
          diarization: releaseProfile.diarization,
          captureMode: detail?.capture_mode,
        });
    try {
      await copyText(markdown);
      setShareOpen(false);
      setExportMsg(kind === "transcript" ? "Transcript copied as Markdown" : "Meeting copied as Markdown");
      window.setTimeout(() => setExportMsg(null), 4000);
    } catch (e) {
      setError(`Could not copy Markdown: ${String(e)}`);
    }
  };

  const exportPdf = async (kind: "notes" | "transcript") => {
    if (id == null) return;
    try {
      setExportMsg(kind === "notes" ? "Creating meeting notes PDF…" : "Creating transcript PDF…");
      const selectedSummaryId = typeof tab === "number" ? summaries[tab]?.id : summaries[0]?.id;
      const path = await api.meetingExportPdf(id, kind, selectedSummaryId);
      setExportMsg(`PDF saved to ${path.split("/").slice(-2).join("/")}`);
      try {
        await openPath(path);
      } catch {
        // the export succeeded; opening the file is best-effort
      }
      window.setTimeout(() => setExportMsg(null), 6000);
    } catch (e) {
      setExportMsg(null);
      setError(String(e));
    }
  };

  // Tap-a-line-to-seek into the retained per-channel WAVs (desktop only;
  // paths land at stop, so playback is a post-meeting affordance).
  const meAudio = localFileUrl(detail?.audio_me_path);
  const themAudio = localFileUrl(detail?.audio_them_path);
  const videoSrc = releaseProfile.videoCapture ? localFileUrl(detail?.video_path) : null;

  const deleteVideo = async () => {
    if (id == null) return;
    try {
      await api.meetingVideoDelete(id);
      setTab("transcript");
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  const moveToTrash = async () => {
    if (id == null || recording || summarizing) return;
    setRemoving(true);
    setError(null);
    try {
      await api.meetingTrash(id);
      onBack();
    } catch (e) {
      setError(String(e));
    } finally {
      setRemoving(false);
    }
  };

  const restoreMeeting = async () => {
    if (id == null) return;
    setRemoving(true);
    setError(null);
    try {
      await api.meetingRestore(id);
      onBack();
    } catch (e) {
      setError(String(e));
    } finally {
      setRemoving(false);
    }
  };

  const deleteForever = async () => {
    if (id == null) return;
    const confirmed = window.confirm(
      `Permanently delete “${title}”? This removes its transcript, summaries, retained audio/video, and generated note. This cannot be undone.`,
    );
    if (!confirmed) return;
    setRemoving(true);
    setError(null);
    try {
      await api.meetingDeleteForever(id);
      onBack();
    } catch (e) {
      setError(String(e));
    } finally {
      setRemoving(false);
    }
  };
  const playLine = (s: MeetingSegment) => {
    const src = s.channel === "me" ? meAudio : themAudio;
    const el = audioRef.current;
    if (!src || !el) return;
    if (playingSeg === s.id) {
      el.pause();
      setPlayingSeg(null);
      return;
    }
    if (!el.src.endsWith(src)) el.src = src;
    el.currentTime = s.t0_ms / 1000;
    el.play().catch(() => {});
    setPlayingSeg(s.id);
  };
  useEffect(() => {
    const el = audioRef.current;
    return () => el?.pause();
  }, []);

  const copyLine = (s: MeetingSegment) => {
    const who = segmentLabel(s, inPerson);
    void copyText(`[${mmss(s.t0_ms)}] ${who}: ${s.text}`).catch(() => {});
  };

  const q = query.trim().toLowerCase();
  const visibleSegments = useMemo(() => {
    const sorted = [...liveSegments].sort((a, b) => a.t0_ms - b.t0_ms || a.id - b.id);
    if (!q) return sorted;
    return sorted.filter(
      (s) =>
        s.text.toLowerCase().includes(q) ||
        segmentLabel(s, inPerson).toLowerCase().includes(q)
    );
  }, [inPerson, liveSegments, q]);

  const openSummarySource = (source: string) => {
    if (source.toLowerCase() === "notes") {
      setEvidenceSegmentId(null);
      setTab("notes");
      return;
    }
    const [minutes, seconds] = source.split(":").map(Number);
    if (!Number.isFinite(minutes) || !Number.isFinite(seconds)) return;
    const targetMs = (minutes * 60 + seconds) * 1000;
    const nearest = liveSegments.reduce<MeetingSegment | null>((best, segment) => {
      if (!best) return segment;
      return Math.abs(segment.t0_ms - targetMs) < Math.abs(best.t0_ms - targetMs)
        ? segment
        : best;
    }, null);
    if (!nearest) return;
    setQuery("");
    setEvidenceSegmentId(nearest.id);
    setTab("transcript");
  };

  const renderTranscriptSurface = (live = false) => (
    <>
      {liveSegments.length > 0 && (
        <div className="transcript-tools">
          <span className="tsearch">
            <Search size={13} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search transcript…"
              spellCheck={false}
              aria-label="Search this meeting transcript"
            />
          </span>
          {q && (
            <span className="tsearch-count">
              {visibleSegments.length} of {liveSegments.length}
            </span>
          )}
          {!live && (
            <button
              type="button"
              className="transcript-copy"
              onClick={() => void copyMarkdown("transcript")}
              title="Copy the full transcript as Markdown"
            >
              <Copy size={13} /> Copy Markdown
            </button>
          )}
        </div>
      )}
      <div className={`meeting-transcript${live ? " live" : ""}`} ref={transcriptRef}>
        {liveSegments.length === 0 ? (
          <p className="quiet-empty">
            {recording ? "Listening. The transcript fills in as people speak." : "No transcript."}
          </p>
        ) : visibleSegments.length === 0 ? (
          <p className="quiet-empty">No lines match "{query.trim()}".</p>
        ) : (
          visibleSegments.map((segment) => {
            const playable = segment.channel === "me" ? meAudio : themAudio;
            const speaker = segmentLabel(segment, inPerson);
            return (
              <div
                key={segment.id}
                data-segment-id={segment.id}
                data-capture-channel={segment.channel}
                className={
                  "bubble " +
                  (inPerson
                    ? `in-person ${roomSpeakerTone(speaker, roomSpeakerOrder)}`
                    : segment.channel) +
                  (playingSeg === segment.id ? " playing" : "") +
                  (focusSegmentId === segment.id || evidenceSegmentId === segment.id ? " search-focus" : "")
                }
              >
                <span className="who">
                  {inPerson && <span className="room-speaker-mark" aria-hidden="true" />}
                  {speaker} · {mmss(segment.t0_ms)}
                  <span className="line-ops">
                    {playable && !recording && (
                      <button
                        className="line-op"
                        title="Play from here"
                        onClick={() => playLine(segment)}
                      >
                        {playingSeg === segment.id ? <Pause size={11} /> : <Play size={11} />}
                      </button>
                    )}
                    <button className="line-op" title="Copy line" onClick={() => copyLine(segment)}>
                      <Copy size={11} />
                    </button>
                  </span>
                </span>
                <p>{segment.text}</p>
              </div>
            );
          })
        )}
      </div>
      <audio ref={audioRef} onEnded={() => setPlayingSeg(null)} style={{ display: "none" }} />
    </>
  );

  return (
    <div className={`meeting-page${recording ? " recording" : ""}`}>
      <header className="meeting-head">
        <button className="icon-btn" onClick={onBack} title="Back" aria-label="Back">
          <ArrowLeft size={18} />
        </button>
        <div className="meeting-title">
          <div className="meeting-title-line">
            {editingTitle ? (
              <input
                className="meeting-title-input"
                value={titleDraft}
                onChange={(event) => setTitleDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void saveTitle();
                  if (event.key === "Escape") setEditingTitle(false);
                }}
                aria-label="Meeting title"
                autoFocus
              />
            ) : (
              <h2>{title}</h2>
            )}
            {id != null && !summarizing &&
              (editingTitle ? (
                <span className="meeting-inline-actions">
                  <button onClick={() => setEditingTitle(false)} disabled={titleSaving}>
                    Cancel
                  </button>
                  <button
                    onClick={() => void saveTitle()}
                    disabled={titleSaving || !titleDraft.trim()}
                  >
                    {titleSaving ? "Saving…" : "Save"}
                  </button>
                </span>
              ) : (
                <button
                  className="meeting-title-edit"
                  onClick={() => {
                    setTitleDraft(title);
                    setEditingTitle(true);
                  }}
                  aria-label="Edit meeting title"
                  title="Edit meeting title"
                >
                  <PenLine size={13} /> Edit
                </button>
              ))}
          </div>
          <div className="meeting-meta">
            {ev?.date && <time dateTime={ev.date}>{fmtMeetingDay(ev.date)}</time>}
            {ev?.start_min != null && (
              <span>
                {fmtClock(ev.start_min)}
                {ev.end_min != null ? `–${fmtClock(ev.end_min)}` : ""}
              </span>
            )}
            {attendees.length > 0 && (
              <span title={attendees.map((a) => a.name || a.email).join(", ")}>
                <Users size={13} /> {ev?.attendee_count ?? attendees.length}
              </span>
            )}
            {detail && !recording && (
              <span title={[detail.asr_engine, detail.asr_model].filter(Boolean).join(" · ")}>
                {transcriptionModelLabel(detail.asr_engine, detail.asr_model)}
              </span>
            )}
            {id != null && detail && !detail.trashed_at && (
              <div className="meeting-destination" ref={folderPickerRef}>
                <button
                  type="button"
                  className="meeting-destination-button"
                  onClick={() => setFolderPickerOpen((open) => !open)}
                  disabled={filingSaving || summarizing}
                  aria-expanded={folderPickerOpen}
                  aria-haspopup="dialog"
                  aria-label={`Change meeting folder. Filed in ${destinationPath}`}
                  title={`Filed in ${destinationPath}`}
                >
                  {filingSaving ? <Loader size={12} className="spin" /> : <Folder size={12} />}
                  <span>{displayMeetingFolderPath(destinationPath)}</span>
                  <ChevronDown size={12} aria-hidden="true" />
                </button>
                {folderPickerOpen && (
                  <div
                    className="meeting-destination-popover"
                    role="dialog"
                    aria-label="Choose meeting folder"
                    aria-busy={filingSaving}
                  >
                    <div className="meeting-folder-search">
                      <Search size={14} aria-hidden="true" />
                      <input
                        ref={folderSearchRef}
                        value={folderQuery}
                        onChange={(event) => setFolderQuery(event.target.value)}
                        placeholder="Search folders"
                        aria-label="Search folders"
                      />
                    </div>
                    <div className="meeting-folder-list">
                      {folders == null ? (
                        <p className="meeting-folder-empty">Loading folders…</p>
                      ) : folderLoadError ? (
                        <p className="meeting-folder-empty">Folders could not be loaded.</p>
                      ) : folderSections.length === 0 ? (
                        <p className="meeting-folder-empty">No folders match “{folderQuery.trim()}”.</p>
                      ) : (
                        folderSections.map(({ space, folders: options }) => (
                          <section className="meeting-folder-section" key={space.id}>
                            <h3>{space.name}</h3>
                            {options.map(({ folder, path }) => {
                              const selected = folder.id === detail.route_folder_id;
                              const parentPath = path.split(" / ").slice(0, -1).join(" › ");
                              return (
                                <button
                                  type="button"
                                  className={selected ? "selected" : ""}
                                  key={folder.id}
                                  onClick={() => void chooseDestination(folder, path)}
                                  disabled={filingSaving}
                                  aria-current={selected ? "true" : undefined}
                                >
                                  <Folder size={14} aria-hidden="true" />
                                  <span>
                                    <strong>{folder.kind === "space" ? "Inbox" : folder.name}</strong>
                                    {parentPath && <small>{parentPath}</small>}
                                  </span>
                                  {selected && <Check size={14} aria-label="Current folder" />}
                                </button>
                              );
                            })}
                          </section>
                        ))
                      )}
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
          {filingStatus && <span className="sr-only" role="status">{filingStatus}</span>}
        </div>
        <span className="spacer" />
        {meetLink && (
          <a
            className="btn ghost"
            href={meetLink}
            target="_blank"
            rel="noreferrer"
            onClick={(event) => {
              event.preventDefault();
              openExternalUrl(meetLink);
            }}
          >
            <Video size={15} /> Join
          </a>
        )}
        {id == null && (
          <label className="meeting-context">
            <span>File in</span>
            <select
              value={filingContext}
              onChange={(event) => {
                const context = event.target.value as FilingContext;
                setFilingContext(context);
                writeFilingContext(context);
              }}
              disabled={starting}
            >
              <option value="work">Work</option>
              <option value="personal">Personal</option>
            </select>
          </label>
        )}
        {id == null ? (
          <>
            <button
              className={meetLink ? "btn rec" : "btn ghost"}
              onClick={() => start("online")}
              disabled={starting}
            >
              {starting ? <Loader size={15} className="spin" /> : <Video size={15} />} Online call
            </button>
            <button
              className={meetLink ? "btn ghost" : "btn rec"}
              onClick={() => start("in_person")}
              disabled={starting}
            >
              {starting ? <Loader size={15} className="spin" /> : <Mic size={15} />} In person
            </button>
          </>
        ) : recording ? (
          <button className="btn stop" onClick={stop} disabled={stopping}>
            {stopping ? <Loader size={15} className="spin" /> : <Square size={13} />}
            <span className="bars" aria-hidden>
              <i />
              <i />
              <i />
            </span>
            {mmss(elapsed)}
          </button>
        ) : summarizing ? (
          <span className="meeting-status">
            <Loader size={14} className="spin" /> enhancing notes…
          </span>
        ) : null}
        {id != null && !recording && (summaries.length > 0 || notes.trim().length > 0 || liveSegments.length > 0) && (
          <div className="meeting-share">
            <button
              className="btn ghost"
              onClick={() => setShareOpen((open) => !open)}
              aria-expanded={shareOpen}
              aria-haspopup="menu"
            >
              <Share2 size={15} /> Share
            </button>
            {shareOpen && (
              <div className="meeting-share-menu" role="menu">
                <button onClick={() => void copyMarkdown("meeting")}>
                  <Copy size={15} />
                  <span><strong>Copy meeting Markdown</strong><small>Current summary, notes, and transcript</small></span>
                </button>
                {(summaries.length > 0 || notes.trim().length > 0) && (
                  <button onClick={() => { setShareOpen(false); void exportPdf("notes"); }}>
                    <FileDown size={15} />
                    <span><strong>Export meeting notes PDF</strong><small>Summary and your notes</small></span>
                  </button>
                )}
                {liveSegments.length > 0 && (
                  <button onClick={() => { setShareOpen(false); void exportPdf("transcript"); }}>
                    <AudioLines size={15} />
                    <span><strong>Export transcript PDF</strong><small>Compact, speaker-labeled full record</small></span>
                  </button>
                )}
                {isDesktop && (
                  <button onClick={() => { setShareOpen(false); void exportMd(); }}>
                    <Download size={15} />
                    <span><strong>Export Markdown</strong><small>Editable plain-text copy</small></span>
                  </button>
                )}
              </div>
            )}
          </div>
        )}
        {id != null && detail?.trashed_at && (
          <>
            <button className="btn ghost" onClick={restoreMeeting} disabled={removing}>
              <RotateCcw size={15} /> Restore
            </button>
            <button className="btn ghost meeting-delete-forever" onClick={deleteForever} disabled={removing}>
              <Trash2 size={15} /> Delete permanently
            </button>
          </>
        )}
        {id != null && !detail?.trashed_at && !recording && !summarizing && (
          <button
            className="icon-btn meeting-trash-btn"
            onClick={moveToTrash}
            disabled={removing}
            title="Move meeting to Trash"
            aria-label="Move meeting to Trash"
          >
            {removing ? <Loader size={15} className="spin" /> : <Trash2 size={16} />}
          </button>
        )}
      </header>

      {exportMsg && <div className="meeting-hint meeting-export-status" role="status">{exportMsg}</div>}
      {recording && micAecNotice(micAec) && (
        <div className="meeting-hint meeting-mic-aec" role="status">
          {micAecNotice(micAec)}
        </div>
      )}
      {error && <div className="error">{error}</div>}
      {detail?.status === "failed" && (
        <div className="meeting-failure" role="status">
          {liveSegments.length === 0
            ? "Recording failed before any audio or transcript was saved. This attempt remains in your meeting history so it doesn’t disappear."
            : "Recording stopped before notes were generated. The transcript below is still available."}
        </div>
      )}
      {id == null && (
        <p className="meeting-hint">
          Jot prep notes below — recording starts when you hit Record, and your notes get
          expanded with the transcript afterwards.
        </p>
      )}

      {!recording && <nav className="meeting-tabs">
        {summaries.map((s, i) => (
          <button
            key={s.id}
            className={tab === i ? "on" : ""}
            onClick={() => {
              setEvidenceSegmentId(null);
              setTab(i);
            }}
          >
            {templateCopy(s.template).label}
          </button>
        ))}
        {id != null && (
          <button
            className={tab === "transcript" ? "on" : ""}
            onClick={() => setTab("transcript")}
          >
            <AudioLines size={13} /> Transcript
            {liveSegments.length > 0 ? ` (${liveSegments.length})` : ""}
          </button>
        )}
        <button className={tab === "notes" ? "on" : ""} onClick={() => setTab("notes")}>
          My Notes
        </button>
        {releaseProfile.videoCapture && videoSrc && (
          <button className={tab === "video" ? "on" : ""} onClick={() => setTab("video")}>
            <Video size={13} /> Video
          </button>
        )}
        {id != null && !recording && liveSegments.length > 0 && remainingTemplates.length > 0 && (
          <div className="tab-add">
            <button
              className="tab-add-button"
              onClick={() => setPickTemplate((v) => !v)}
              disabled={generating != null}
              title="Generate a summary with a template"
              aria-expanded={pickTemplate}
              aria-haspopup="menu"
            >
              {generating ? <Loader size={13} className="spin" /> : <Plus size={13} />}
              {generating ? `Generating ${templateCopy(generating).label}…` : "Add summary"}
              <ChevronDown size={12} />
            </button>
            {pickTemplate && (
              <div className="tab-menu" role="menu">
                <p>Choose what these notes should emphasize</p>
                {remainingTemplates.map((template) => {
                  const copy = templateCopy(template.name);
                  return (
                    <button
                      key={template.name}
                      onClick={() => generate(template.name)}
                      role="menuitem"
                    >
                      <span>
                        <strong>{copy.label}</strong>
                        <small>{copy.description}</small>
                      </span>
                      {template.name === recommendedSummaryTemplate && <em>Recommended</em>}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </nav>}

      {id != null && recording && (
        <section className="meeting-copilot" aria-label="Meeting copilot">
          <header className="copilot-head">
            <span className="copilot-mark"><Sparkles size={14} /></span>
            <div>
              <strong>Live copilot</strong>
              <span>
                {autoAssistOn ? "Watching for useful moments" : "Automatic suggestions paused"}
              </span>
            </div>
            <button
              className={`copilot-toggle${autoAssistOn ? " on" : ""}`}
              onClick={() => setAutoAssistOn((on) => !on)}
              aria-pressed={autoAssistOn}
              title={autoAssistOn ? "Pause automatic suggestions" : "Resume automatic suggestions"}
            >
              <span className="copilot-pulse" />
              {autoAssistOn ? "Live" : "Paused"}
            </button>
          </header>

          <div className={`copilot-insight${liveInsight ? " ready" : ""}`} aria-live="polite">
            <span className="copilot-insight-label">
              {autoAssistBusy ? (
                <><Loader size={12} className="spin" /> Thinking about the latest discussion</>
              ) : liveInsight ? (
                "Suggested now"
              ) : autoAssistOn ? (
                "Listening for enough context"
              ) : (
                "Live suggestions are paused"
              )}
            </span>
            {liveInsight ? (
              <p>{liveInsight}</p>
            ) : (
              <p className="copilot-placeholder">
                I’ll surface a response, risk, decision, or follow-up when it becomes useful.
              </p>
            )}
            {autoAssistError && <small>{autoAssistError}</small>}
          </div>

          {assistA && (
            <div className="assist-answer" aria-live="polite">
              <p>{assistA}</p>
              <button className="icon-btn" onClick={() => setAssistA(null)} aria-label="Dismiss answer">
                ×
              </button>
            </div>
          )}
          <div className="assist-input">
            <Sparkles size={13} />
            <input
              ref={assistInputRef}
              value={assistQ}
              onChange={(e) => setAssistQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") askAssist();
              }}
              placeholder="Ask about what’s happening right now…"
              spellCheck={false}
              disabled={assistBusy}
            />
            <kbd>⌘⇧A</kbd>
            <button className="chip-action" onClick={askAssist} disabled={assistBusy || !assistQ.trim()}>
              {assistBusy ? <Loader size={12} className="spin" /> : "Ask"}
            </button>
          </div>
        </section>
      )}

      {recording ? (
        <div className="meeting-live-workspace" aria-label="Live meeting workspace">
          <section className="meeting-live-pane meeting-live-transcript" aria-label="Live transcript">
            <header className="meeting-live-pane-head">
              <div>
                <strong>Transcript</strong>
                <span>
                  {liveSegments.length > 0
                    ? `${liveSegments.length} ${liveSegments.length === 1 ? "line" : "lines"}, updating live`
                    : "Listening for speech"}
                </span>
              </div>
              <AudioLines size={16} aria-hidden="true" />
            </header>
            {renderTranscriptSurface(true)}
          </section>

          <section className="meeting-live-pane meeting-live-notes" aria-label="My meeting notes">
            <header className="meeting-live-pane-head">
              <div>
                <strong>My notes</strong>
                <span>Write freely while the transcript keeps your place.</span>
              </div>
              <span className={`meeting-note-save ${notesSaveState}`} role="status">
                {notesSaveState === "saving" ? (
                  <><Loader size={11} className="spin" /> Saving</>
                ) : notesSaveState === "error" ? (
                  "Could not save"
                ) : (
                  <><Check size={11} /> Saved</>
                )}
              </span>
            </header>
            <DocumentEditor
              value={notesDocument}
              onChange={onNotesDocument}
              placeholder="Capture a decision, question, or follow-up…"
              ariaLabel="Meeting notes"
            />
          </section>
        </div>
      ) : tab === "notes" ? (
        <div className="meeting-notes-pane">
          <p className="meeting-notes-help">
            These are your notes before, during, and after the meeting. They autosave, guide the
            generated summary, and remain verbatim in exports.
          </p>
          <DocumentEditor
            value={notesDocument}
            onChange={onNotesDocument}
            placeholder="Add your own notes…"
            ariaLabel="Meeting notes"
          />
        </div>
      ) : tab === "transcript" ? (
        <>
          {releaseProfile.diarization && speakers.length > 0 && !recording && (
            <div className="speaker-review">
              <div className="speaker-bar">
                <span className="speaker-bar-label">Speakers</span>
                <span className="speaker-manual-hint">Review every label without leaving the transcript</span>
                {speakers.map((speaker) => (
                  <span
                    key={speaker.label}
                    className={`speaker-chip${inPerson ? ` room-speaker-chip ${roomSpeakerTone(speaker.label, roomSpeakerOrder)}` : ""}`}
                  >
                    <button
                      type="button"
                      className="chip-name"
                      title={speaker.label.includes("@") ? speaker.label : "Edit this speaker label"}
                      onClick={() => openSpeakerEditor(speaker.label)}
                    >
                      {speakerDisplayName(speaker.label)}
                    </button>
                  </span>
                ))}
                <button
                  type="button"
                  className="chip-action speaker-label-action"
                  onClick={() => (speakerEditorOpen ? setSpeakerEditorOpen(false) : openSpeakerEditor())}
                  aria-expanded={speakerEditorOpen}
                >
                  <PenLine size={12} /> {speakerEditorOpen ? "Close labels" : "Label speakers"}
                </button>
                {detail?.capture_mode !== "in_person" && detail?.status === "done" && detail.audio_them_path && (
                  <button
                    type="button"
                    className="chip-action"
                    onClick={rediarize}
                    disabled={rediarizing}
                    title="Rebuild anonymous speaker groups from retained audio"
                  >
                    {rediarizing ? <Loader size={12} className="spin" /> : <AudioLines size={12} />}
                    {storedSpeakers.length === 0 ? "Detect speakers" : "Re-detect speakers"}
                  </button>
                )}
              </div>

              {speakerSaveMessage && (
                <div className="speaker-save-message" role="status">
                  <Check size={12} /> {speakerSaveMessage}
                </div>
              )}

              {speakerEditorOpen && (
                <section className="speaker-editor" aria-label="Label speakers">
                  <div className="speaker-editor-head">
                    <div>
                      <h3>Label speakers</h3>
                      <p>
                        Suggestions come from this meeting’s invite and your People directory.
                        Nothing is assigned from someone’s voice.
                      </p>
                    </div>
                    <span>{speakers.length} detected</span>
                  </div>

                  <div className="speaker-editor-list">
                    {speakers.map((speaker, index) => {
                      const sample = speakerSamples.get(speaker.label);
                      const draft = speakerDrafts[speaker.label] ?? "";
                      return (
                        <div className="speaker-editor-row" key={speaker.label}>
                          <div className="speaker-editor-context">
                            <div className="speaker-editor-source">
                              <strong>{speakerDisplayName(speaker.label)}</strong>
                              <span>{speaker.seg_count} transcript {speaker.seg_count === 1 ? "line" : "lines"}</span>
                            </div>
                            {sample && (
                              <button
                                type="button"
                                className="speaker-sample"
                                onClick={() => {
                                  setQuery("");
                                  setEvidenceSegmentId(sample.id);
                                  playLine(sample);
                                }}
                              title={(inPerson ? detail?.audio_me_path : detail?.audio_them_path) ? "Play this speaker sample" : "Jump to this transcript line"}
                              >
                                {(inPerson ? detail?.audio_me_path : detail?.audio_them_path) ?
                                  (playingSeg === sample.id ? <Pause size={12} /> : <Play size={12} />) :
                                  <Search size={12} />}
                                <span>“{sample.text.length > 96 ? `${sample.text.slice(0, 96).trim()}…` : sample.text}”</span>
                                <small>{mmss(sample.t0_ms)}</small>
                              </button>
                            )}
                            {speaker.label === "Them" && (
                              <small className="speaker-fragment-warning">
                                These fragments were not confidently grouped. Leave them blank unless they all belong to one person.
                              </small>
                            )}
                          </div>

                          <div className="speaker-editor-fields">
                            <label htmlFor={`speaker-label-${index}`}>Name or email</label>
                            <input
                              id={`speaker-label-${index}`}
                              ref={(node) => { speakerInputRefs.current[speaker.label] = node; }}
                              value={draft}
                              onChange={(e) => {
                                setSpeakerDrafts((current) => ({ ...current, [speaker.label]: e.target.value }));
                                setSpeakerSaveMessage(null);
                              }}
                              onKeyDown={(e) => {
                                if (e.key === "Escape") {
                                  setSpeakerEditorOpen(false);
                                  return;
                                }
                                if (e.key !== "Enter") return;
                                e.preventDefault();
                                const next = speakers[index + 1];
                                if (next) {
                                  speakerInputRefs.current[next.label]?.focus();
                                } else {
                                  void saveSpeakerLabels();
                                }
                              }}
                              placeholder="Choose a person or type a name"
                              spellCheck={false}
                              disabled={speakerSaving}
                            />
                            {speakerCandidates.length > 0 ? (
                              <div className="speaker-candidates" aria-label={`People invited to this meeting for ${speaker.label}`}>
                                {speakerCandidates.map((candidate) => {
                                  const selected = candidate.identities.includes(normalizedIdentity(draft));
                                  const usedElsewhere = speakers.some((other) =>
                                    other.label !== speaker.label &&
                                    candidate.identities.includes(normalizedIdentity(speakerDrafts[other.label]))
                                  );
                                  return (
                                    <button
                                      type="button"
                                      key={`${speaker.label}-${candidate.email ?? candidate.value}`}
                                      className={(selected ? "selected" : "") + (usedElsewhere ? " used" : "")}
                                      onClick={() => {
                                        setSpeakerDrafts((current) => ({ ...current, [speaker.label]: candidate.value }));
                                        setSpeakerSaveMessage(null);
                                      }}
                                      aria-pressed={selected}
                                      title={candidate.email ?? candidate.label}
                                      disabled={speakerSaving}
                                    >
                                      {selected && <Check size={11} />}
                                      {candidate.label}
                                      {usedElsewhere && !selected && <small>used</small>}
                                    </button>
                                  );
                                })}
                              </div>
                            ) : (
                              <small className="speaker-no-candidates">No calendar invitees found—type a label manually.</small>
                            )}
                          </div>
                        </div>
                      );
                    })}
                  </div>

                  <div className="speaker-editor-actions">
                    <span>Enter moves to the next speaker. Nothing changes until you save.</span>
                    <button type="button" className="ghost-btn" onClick={() => setSpeakerEditorOpen(false)} disabled={speakerSaving}>
                      Cancel
                    </button>
                    <button
                      type="button"
                      className="primary"
                      onClick={() => void saveSpeakerLabels()}
                      disabled={speakerSaving || speakerChangeCount === 0}
                    >
                      {speakerSaving ? <><Loader size={12} className="spin" /> Saving…</> :
                        speakerChangeCount === 0
                          ? "Save labels"
                          : `Save ${speakerChangeCount} ${speakerChangeCount === 1 ? "label" : "labels"}`}
                    </button>
                  </div>
                </section>
              )}
            </div>
          )}
          {renderTranscriptSurface()}
        </>
      ) : releaseProfile.videoCapture && tab === "video" ? (
        <div className="meeting-video">
          {videoSrc ? (
            <>
              <video src={videoSrc} controls playsInline />
              <div className="video-tools">
                <span className="quiet-empty">
                  The call window, recorded even while covered by other apps. Auto-deletes per
                  Settings → Meetings.
                </span>
                <button
                  className="chip-action"
                  onClick={deleteVideo}
                  title="Delete the video file now to free space (transcript and summaries stay)"
                >
                  Delete video
                </button>
              </div>
            </>
          ) : (
            <p className="quiet-empty">No video for this meeting.</p>
          )}
        </div>
      ) : (
        <div className="meeting-summary">
          {summaries[tab as number] ? (
            <>
              <div className="summary-tools">
                {editingSummary === summaries[tab as number].id ? (
                  <span className="meeting-inline-actions">
                    <button onClick={() => setEditingSummary(null)} disabled={summarySaving}>
                      Cancel
                    </button>
                    <button onClick={() => void saveSummary()} disabled={summarySaving}>
                      {summarySaving ? "Saving…" : "Save"}
                    </button>
                  </span>
                ) : (
                  <>
                    <button
                      className="summary-action"
                      onClick={() => {
                        const summary = summaries[tab as number];
                        void copySummary(summary.id, summary.content_md);
                      }}
                      aria-label="Copy all summary notes"
                    >
                      {copiedSummary === summaries[tab as number].id ? (
                        <>
                          <Check size={13} /> Copied
                        </>
                      ) : (
                        <>
                          <Copy size={13} /> Copy notes
                        </>
                      )}
                    </button>
                    <button
                      className="summary-action"
                      onClick={() => {
                        const summary = summaries[tab as number];
                        setEditingSummary(summary.id);
                        setSummaryDraft(summary.content_md);
                      }}
                    >
                      <PenLine size={13} /> Edit notes
                    </button>
                    <button
                      className="summary-action"
                      onClick={() => generate(summaries[tab as number].template)}
                      disabled={generating != null}
                    >
                      {generating ? <Loader size={13} className="spin" /> : <Sparkles size={13} />}
                      Refresh
                    </button>
                  </>
                )}
              </div>
              {editingSummary === summaries[tab as number].id ? (
                <textarea
                  className="meeting-summary-editor"
                  value={summaryDraft}
                  onChange={(event) => setSummaryDraft(event.target.value)}
                  aria-label="Generated meeting notes"
                  spellCheck
                  autoFocus
                />
              ) : (
                <>
                  <MdBlock
                    md={summaries[tab as number].content_md}
                    onSource={openSummarySource}
                  />
                  {!inPerson && <ConversationDynamics
                    conversation={detail?.conversation}
                    canRedetect={Boolean(
                      releaseProfile.diarization &&
                      detail?.status === "done" &&
                      detail.audio_them_path
                    )}
                    redetecting={rediarizing}
                    onRedetect={rediarize}
                  />}
                </>
              )}
            </>
          ) : null}
        </div>
      )}
      <article className="meeting-print" aria-hidden="true">
        <header>
          <span className="print-kicker">NOTED / MEETING NOTES</span>
          <h1>{title}</h1>
          <div className="print-rule" />
          <p className="print-meta">
            {detail?.started_at ? new Date(detail.started_at).toLocaleString([], { dateStyle: "long", timeStyle: "short" }) : ""}
            {attendees.length ? ` · ${attendees.map((a) => a.name || a.email).join(", ")}` : ""}
          </p>
        </header>
        {summaries.map((s) => <section key={s.id}><h2>{templateCopy(s.template).label}</h2><MdBlock md={s.content_md} /></section>)}
        {notes.trim() && <section><h2>Notes</h2><p className="print-notes">{notes}</p></section>}
        {liveSegments.length > 0 && (
          <section className="print-transcript"><h2>Transcript</h2>
            {liveSegments.map((s) => <p key={s.id}><b>{segmentLabel(s, inPerson)}</b><span>{mmss(s.t0_ms)}</span>{s.text}</p>)}
          </section>
        )}
        <footer>{title} · Generated locally with Noted</footer>
      </article>
    </div>
  );
}
