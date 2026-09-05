import { convertFileSrc, invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { ThemeModePreference, ThemePack } from "./themes/types";

// Product builds run inside a Tauri shell and use its IPC boundary. The retired
// LAN browser bridge must not be revived here: it exposed a desktop-sized RPC
// surface and persisted bearer credentials in URLs and browser storage.
export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Playable URL for a locally retained file (meeting audio) via the Tauri
// asset protocol. Desktop only — the phone bridge doesn't serve raw files.
export function localFileUrl(path: string | null | undefined): string | null {
  return isDesktop && path ? convertFileSrc(path) : null;
}

// Retained temporarily for the dormant legacy phone UI's error boundary. New
// native mobile code must model sync/enrollment state instead of URL tokens.
export class TokenError extends Error {
  constructor() {
    super("Connection expired — re-scan the QR code shown in noted on your Mac.");
    this.name = "TokenError";
  }
}

// A plain browser build has no product transport. Desktop and future native iOS
// shells use Tauri IPC; sync availability will be modeled separately on iOS.
export class OfflineError extends Error {
  constructor() {
    super("Reconnecting to your Mac…");
    this.name = "OfflineError";
  }
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isDesktop) return tauriInvoke<T>(cmd, args as Record<string, unknown>);
  throw new OfflineError();
}

// One extracted observation within a note (a category + its structured data).
export type Proposal = {
  category: string;
  is_new_category: boolean;
  description: string;
  routed_by?: "header" | "classifier"; // how the category was decided
  data: Record<string, unknown>;
};

// A candidate knowledge-graph entity surfaced from a note (proposed, pre-save).
// For a person the note may also yield a curated `fact` and `relationship`.
export type EntityCandidate = {
  name: string;
  type: string;
  fact?: string;
  relationship?: string;
};

// A stored entity node (for the graph view / management).
export type EntityRow = {
  id: number;
  name: string;
  type: string;
  mention_count: number;
  last_seen?: string | null;
  suggested_name?: string | null;
};

// A likely-duplicate entity pair from the retro merge scan (People view panel).
export type MergeSuggestion = {
  a_id: number;
  a_name: string;
  a_mentions: number;
  b_id: number;
  b_name: string;
  b_mentions: number;
  etype: string;
  similarity: number;
};

// Knowledge-graph shapes. Work-graph nodes also carry `vault`.
export type GraphNode = {
  id: number;
  name: string;
  type: string;
  mention_count: number;
  last_seen?: string | null;
  suggested_name?: string | null;
  vault?: string;
};
export type GraphEdge = { source: number; target: number; weight: number };
export type GraphData = { nodes: GraphNode[]; edges: GraphEdge[] };

// A registered Obsidian "brain" vault mirrored into the KG, with live counts.
export type BrainVaultStatus = {
  vault: string;
  root_path: string;
  direction: string;
  last_git_sha: string | null;
  last_synced_at: string | null;
  enabled: boolean;
  note_count: number;
  entity_count: number;
};
export type BrainSyncReport = {
  vault: string;
  scanned: number;
  imported: number;
  unchanged: number;
  entities_created: number;
  mentions_added: number;
  errors: string[];
};
// One brain note write-back would change (managed region: before -> after).
export type BrainWritePreview = {
  vault: string;
  path: string;
  entity: string;
  before: string | null;
  after: string;
};
export type BrainWriteReport = {
  files_written: number;
  commits: { vault: string; sha: string; files: number }[];
  errors: string[];
};
// A brain note related to in-progress capture text (proactive surfacing).
export type RelatedBrain = {
  note_id: number;
  vault: string;
  entity_id: number | null;
  name: string | null;
  snippet: string;
};
export type EntityMention = { note_id: number; event_date: string; snippet: string };

// A dated, curated fact about a person, drawn from one note mention.
export type PersonMention = { date: string; text: string; note_id: number };

// A person's accumulated profile for the People view.
export type PersonProfile = {
  id: number;
  name: string;
  relationship: string | null;
  mention_count: number;
  first_seen: string | null;
  last_seen: string | null;
  aliases: string[];
  suggested_name: string | null;
  mentions: PersonMention[];
};

// Full profile for ANY entity (person/place/topic/…): header + the complete,
// uncapped mention timeline. Backs the per-entity page.
export type EntityProfile = {
  id: number;
  name: string;
  type: string;
  relationship: string | null;
  mention_count: number;
  first_seen: string | null;
  last_seen: string | null;
  aliases: string[];
  mentions: PersonMention[]; // {date, text, note_id}, newest-first, uncapped
};

// What categorize/categorizePhoto return: the note-level envelope wrapping one
// or more entries (a single note can fill several categories) plus the entities
// the note refers to.
export type Envelope = {
  raw_text?: string; // original text, or the transcription on the photo path
  event_date: string; // canonical day (YYYY-MM-DD), extracted or defaulted to today
  date_was_extracted?: boolean; // true if read from the note, false if defaulted
  entries: Proposal[];
  entities: EntityCandidate[];
};

export type NoteEntry = { id?: number; category: string | null; data: Record<string, unknown> | null };

export type NoteRow = {
  id: number;
  title: string;
  raw_text: string;
  document_json: string | null;
  note_kind: "capture" | "document";
  source: string;
  entries: NoteEntry[];
  event_date: string;
  created_at: string;
  updated_at: string;
  trashed_at?: string | null;
};

export type CategoryInfo = {
  id: number;
  name: string;
  description: string;
  schema: { shape?: unknown; field_freq?: Record<string, number> };
  entry_count: number;
};

export type NoteFolderInfo = {
  id: number;
  parent_id: number | null;
  name: string;
  kind: "space" | "folder";
  auto_rule: "" | "daily_standup";
  note_ids: number[];
  explicit_filings: NoteFolderItemInfo[];
};

export type NoteFolderItemInfo = {
  note_id: number;
  filing_context: "work" | "personal" | null;
  source: "context" | "rule" | "manual" | "undo";
  reason: string;
  event_id: number | null;
};

export type NoteFilingReceipt = {
  event_id: number;
  note_id: number;
  folder_id: number | null;
  previous_folder_id: number | null;
  filing_context: "work" | "personal" | null;
  previous_context: "work" | "personal" | null;
  source: "context" | "rule" | "manual" | "undo";
  reason: string;
};

export type Health = {
  models: string[];
  vec_version: string;
  assistant_shortcut_enabled: boolean;
  assistant_shortcut_registered: boolean;
};

// Model-provider settings. "local" = 100% Ollama; "balanced" routes the
// extract/OCR hot path to a chosen cloud provider; "hosted" routes every
// model-dependent feature through the authenticated Noted API.
export type ProviderMode = "local" | "balanced" | "hosted" | "byok";
export type CloudProvider = "gemini" | "openai" | "anthropic";
export type ProviderId =
  | "noted_hosted"
  | "local"
  | "openai"
  | "gemini"
  | "anthropic"
  | "openai_compatible"
  | "groq"
  | "system";
export type CapabilityChoice = { provider: ProviderId; model: string; base_url: string };
export type ByokConfig = {
  intelligence: CapabilityChoice;
  vision: CapabilityChoice;
  embeddings: CapabilityChoice;
  transcription: CapabilityChoice;
  speech: CapabilityChoice;
};
export type ProviderSettings = {
  version: number;
  mode: ProviderMode;
  cloud_provider: CloudProvider;
  text_model: string; // local Ollama text model (any pulled model)
  vision_model: string; // local Ollama vision model
  gemini_text_model: string;
  gemini_vision_model: string;
  openai_base_url: string; // any OpenAI-compatible endpoint
  openai_text_model: string;
  openai_vision_model: string;
  anthropic_text_model: string;
  anthropic_vision_model: string;
  has_gemini_key: boolean;
  has_openai_key: boolean;
  has_anthropic_key: boolean;
  has_hosted_key: boolean;
  byok: ByokConfig;
  has_groq_key: boolean;
  has_openai_compatible_key: boolean;
};

export type ThemeState = {
  schemaVersion: 1;
  activeThemeId: string;
  colorMode: ThemeModePreference;
};
export type ThemeCandidate = { id: string; name: string; description: string };
export type ThemeSuggestion = { themeId: string; summary: string };

// Google Calendar. noted pushes the day's schedule one-way into a dedicated
// "noted" calendar in the first account, and the Calendar view reads/writes
// events across every connected account; auth is OAuth (tokens live in the
// macOS Keychain, one refresh token per account).
export type GcalCalendarInfo = {
  id: string;
  name: string;
  color: string; // Google's calendar hex
  enabled: boolean; // shown in the Calendar view
  primary: boolean;
  access: string; // owner/writer = event create/edit allowed
};
export type GcalAccountInfo = {
  email: string;
  connected: boolean; // false = session expired, needs a reconnect
  calendars: GcalCalendarInfo[];
};
export type GcalStatus = {
  connected: boolean; // at least one account has a refresh token
  has_client: boolean; // the OAuth client id + secret are set
  account_email: string | null; // first account
  sync_account: string | null; // account hosting the "noted" push calendar
  calendar_id: string | null;
  accounts: GcalAccountInfo[];
};

/**
 * Apple Calendar (EventKit) — read-only, and local: no OAuth, no tokens.
 *
 * `access` is the macOS permission state. `write_only` is a real EventKit
 * state that cannot read events, so it is no more useful here than `denied`;
 * `unsupported` means this build is not on macOS.
 */
export type AppleCalAccess =
  | "granted"
  | "denied"
  | "restricted"
  | "write_only"
  | "not_determined"
  | "unsupported";

export type AppleCalendarInfo = {
  id: string;
  name: string;
  color: string;
  source: string; // the EventKit account: iCloud, Google, On My Mac…
  enabled: boolean;
};

export type AppleCalStatus = {
  access: AppleCalAccess;
  account: string; // the label Apple events carry in RangeEvent.account
  calendars: AppleCalendarInfo[];
};

// Deterministic meeting filing. Rules match exact Google identities and run
// in priority order; they never rely on model inference or broad domains.
export type MeetingFilingRule = {
  email: string;
  folder_id: number | null;
  folder_name: string | null;
  folder_path: string | null;
  priority: number;
  enabled: boolean;
};
export type MeetingFilingBackfillItem = {
  meeting_id: number;
  note_id: number;
  title: string;
  status: string;
  folder_id: number | null;
  folder_name: string | null;
  folder_path: string | null;
  email: string | null;
  via: string;
};
export type MeetingFilingBackfillPreview = {
  token: string;
  eligible: number;
  would_file: number;
  needs_filing: number;
  already_filed: number;
  manual: number;
  items: MeetingFilingBackfillItem[];
};
export type MeetingFilingBackfillApply = {
  reviewed: number;
  filed: number;
  needs_filing: number;
  skipped: number;
};
// One remembered guest for event-form autocomplete (harvested from events).
export type GcalContact = { email: string; name: string };
export type SyncReport = {
  created: number;
  updated: number;
  skipped: number; // legacy untimed blocks, not pushable as events
  duplicates: number; // blocks already on another calendar at the same start+end
  deleted: number; // events for blocks removed since the last sync
  errors: string[];
};
// One event read back from Google Calendar for Today. Times are "HH:MM" in the configured zone
// wall-clock; all-day events have start/end null and all_day true. Meeting and
// account metadata lets a uniquely matching schedule row join in one click.
export type CalEvent = {
  id: string;
  task: string;
  start: string | null;
  end: string | null;
  all_day: boolean;
  calendar: string;
  calendar_id: string;
  color: string;
  account: string;
  meet_link: string | null;
  html_link: string | null;
};
// One event in the Calendar view's range feed. Timed events position by
// start_min/end_min — minutes from `date`'s configured-zone midnight, where end_min
// > 1440 means it crosses midnight. All-day events carry `end_date` instead
// (Google's EXCLUSIVE end day).
export type RangeEvent = {
  id: string;
  title: string;
  date: string; // YYYY-MM-DD start day in the configured zone
  end_date: string | null;
  start_min: number | null;
  end_min: number | null;
  all_day: boolean;
  calendar: string;
  calendar_id: string;
  color: string;
  account: string;
  location: string | null;
  description: string | null;
  declined: boolean;
  google_meet: boolean; // the link is Google conference data (edit can keep/remove it)
  meet_link: string | null; // Meet/Zoom/Teams join URL (conference data, or found in location/description)
  html_link: string | null; // open the event in Google Calendar's web UI
  organizer: string | null;
  organizer_email?: string | null;
  creator_email?: string | null;
  attendees: { name: string; email: string; status: string; self: boolean }[]; // capped at 12, rooms excluded
  attendee_emails?: string[]; // uncapped normalized identities used for deterministic meeting filing
  associated_emails?: string[]; // source account + organizer + creator + attendee identities
  attendee_count: number; // real total (rooms excluded)
  ical_uid?: string | null;
  recurring_event_id?: string | null; // Google series id; present on recurring instances
};
// Fields the Calendar view's create/edit forms submit.
export type EventInput = {
  account: string;
  calendarId: string;
  title: string;
  date: string; // YYYY-MM-DD
  start?: string; // "HH:MM"; absent = all-day
  end?: string;
  endDate?: string; // all-day only: inclusive last day
  location?: string;
  description?: string;
  addMeet?: boolean; // create only: attach a Google Meet conference
  guests?: string[]; // create only: attendee emails (they get invites)
};

// ── Meetings (local Granola) ────────────────────────────────────────────────
// A meeting is a recorded capture session; its transcript is Me (mic) / Them
// (system audio) segments, and each summarize run adds a template-named tab.
export type MeetingSegment = {
  id: number;
  // Physical capture source, not participant identity. In-person meetings use
  // the local mic for everyone and distinguish people with `speaker`.
  channel: "me" | "them";
  t0_ms: number;
  t1_ms: number;
  voiced_ms: number | null; // speech-only VAD frames; null on historical rows
  text: string;
  speaker: string | null; // diarized name once available; null = channel default
};
export type MeetingSummary = {
  id: number;
  template: string;
  content_md: string;
  content_json?: Record<string, unknown> | null;
  created_at: string;
};
export type MeetingSpeakerUpdateResult = {
  speakers_updated: number;
  summaries_refreshed: number;
  summary_refresh_error: string | null;
};
export type MeetingStatus = "recording" | "summarizing" | "done" | "failed";
export type MeetingListRow = {
  id: number;
  title: string;
  started_at: string | null;
  ended_at: string | null;
  status: MeetingStatus;
  note_id: number | null;
  event_json: Partial<RangeEvent> | null;
  segment_count: number;
  summary_count: number;
  trashed_at: string | null;
  meeting_type?: "daily_standup" | "one_on_one" | "group" | "other";
  route_status?: "matched" | "manual" | "needs_filing";
  route_folder_id?: number | null;
  route_email?: string | null;
  route_via?: string | null;
  filing_context?: "work" | "personal" | null;
  capture_mode: "online" | "in_person";
};
export type TranscriptSearchHit = {
  segment_id: number;
  meeting_id: number;
  meeting_title: string;
  started_at: string | null;
  t0_ms: number;
  speaker: string;
  text: string;
};
export type TranscriptFacetValue = {
  value: string;
  label: string;
  count: number;
};
export type TranscriptSearchFacets = {
  people: TranscriptFacetValue[];
  folders: TranscriptFacetValue[];
  meeting_types: TranscriptFacetValue[];
};
export type TranscriptSearchFilters = {
  people: string[];
  folderIds: number[];
  meetingTypes: string[];
};
export type NoteSortOrder = "date_desc" | "date_asc" | "title_asc" | "title_desc";
export type TranscriptVocabularyRule = {
  id: number;
  heard: string;
  preferred: string;
  created_at: string;
  updated_at: string;
  last_batch_id: number | null;
  last_changed_segments: number | null;
  last_applied_at: string | null;
};
export type TranscriptVocabularyPreview = {
  matching_segments: number;
  occurrences: number;
};
export type TranscriptVocabularyApplyResult = {
  rule: TranscriptVocabularyRule;
  batch_id: number | null;
  changed_segments: number;
  changed_occurrences: number;
};
export type TranscriptVocabularyUndoResult = {
  restored_segments: number;
  skipped_segments: number;
};
// A diarized voice in a meeting. `suggested` is an unconfirmed LLM-mined name
// (confirming = renaming); label "Them" is the lone-unrecognized-voice case.
export type MeetingSpeaker = {
  label: string;
  suggested: string | null;
  seg_count: number;
};
export type MeetingConversationParticipant = {
  label: string;
  channel: "me" | "them";
  talk_ms: number;
  share_pct: number;
  words: number;
  pace_wpm: number | null;
  speech_bursts: number;
  median_speech_burst_ms: number;
};
export type MeetingConversation = {
  available: boolean;
  timing_basis: "voice_activity" | "segment_bounds";
  speaker_time_ms: number;
  transcript_words: number;
  expected_remote_speakers: number | null;
  detected_remote_speakers: number;
  speaker_coverage_pct: number | null;
  unattributed_remote_ms: number;
  unattributed_remote_pct: number | null;
  speaker_detail_available: boolean;
  speaker_detail_reason:
    | "available"
    | "not_enough_speech"
    | "no_remote_speech"
    | "low_attribution"
    | "speaker_count_mismatch";
  channels: MeetingConversationParticipant[];
  speakers: MeetingConversationParticipant[];
};
export type MeetingDetail = MeetingListRow & {
  public_id: string;
  event_id: string | null;
  raw_notes: string;
  notes_document_json: string | null;
  audio_me_path: string | null;
  audio_them_path: string | null;
  video_path: string | null; // window recording; null = off/expired/deleted
  asr_engine: string | null; // resolved engine used when this meeting started
  asr_model: string | null; // exact local artifact or provider model used
  summary_error: string | null; // persisted generation failure or saved-quality warning
  segments: MeetingSegment[];
  summaries: MeetingSummary[];
  talk_ms: { me: number; them: number };
  speakers: MeetingSpeaker[];
  conversation?: MeetingConversation;
};
export type MeetingLiveState = {
  active: boolean;
  meetingId?: number;
  title?: string;
  elapsed_ms?: number;
  last_signal_ms_ago?: number | null;
};
export type MeetingTemplate = { name: string; prompt: string; builtin: boolean };
export type MeetingModelStatus = {
  turbo: boolean;
  base: boolean;
  speaker: boolean; // voice-embedding model for per-speaker labels
  in_person_supported: boolean; // FluidAudio requires macOS 14+
  in_person_diarizer: boolean; // FluidAudio offline diarization models are ready
  parakeet: boolean; // Parakeet-TDT ASR engine files present
  hosted: boolean; // scoped Noted API key exists in macOS Keychain
  tap_supported: boolean;
  video_supported: boolean;
  video_authorized: boolean;
};
// What the record-prompt popup shows: calendar T-60s / mic-in-use detection,
// or a transient buttonless "status" card (e.g. "Meeting saved").
export type PromptPayload = {
  kind: "calendar" | "mic" | "status";
  title: string;
  app: string | null;
  bundleId: string | null;
  meetingTitle: string;
  event: Partial<RangeEvent> | null;
};
export type MeetingsCfg = {
  auto_prompt: boolean;
  retain_audio: boolean;
  ignore_bundles: string[];
  default_template: string;
  vocabulary: string[];
  asr_engine: "whisper" | "parakeet" | "hosted";
  /**
   * macOS voice-processing (AEC) on the mic — strips speaker playback from the
   * mic signal. Defaults off: voice processing seizes the input device, so a
   * call app sharing the mic would record silence. Recording yields to a live
   * call even when this is on (see `MicAecState`).
   */
  mic_aec: boolean;
  /** Record the meeting app's window as video (ScreenCaptureKit, macOS 15+). */
  record_video: boolean;
  /** Days before the launch-time sweep deletes window videos; 0 = keep forever. */
  video_keep_days: number;
};

/**
 * How the mic is being captured for the active recording ("meeting-mic-aec").
 *
 * - `active` — macOS voice processing is on; speaker bleed is cancelled.
 * - `off_by_choice` — the user turned echo cancellation off.
 * - `yielded` — a call app holds the mic, so voice processing was skipped to
 *   avoid muting the user in that call; `app` is that app's name.
 * - `unavailable` — it was wanted but could not run (odd device, denied
 *   component), so the raw mic is recording.
 *
 * Every state but `active` means the far side may be picked up as you when
 * recording on speakers.
 */
export type MicAecState = "active" | "off_by_choice" | "yielded" | "unavailable";

export type MeetingMicAec = {
  meetingId: number;
  state: MicAecState;
  app: string | null;
};

// ── Permissioned local agent access (vendor-neutral MCP) ───────────────────
export type AgentClient = {
  id: string;
  name: string;
  created_at: string;
  revoked_at: string | null;
  last_seen_at: string | null;
};
export type AgentAccessStatus = {
  enabled: boolean;
  clients: AgentClient[];
  pending_count: number;
  helper_command: string;
};
export type AgentClientSetup = {
  client: AgentClient;
  config_json: string;
  command: string;
};
export type AgentContextOptions = {
  include_summary: boolean;
  include_notes: boolean;
  include_transcript: boolean;
  max_bytes?: number | null;
};
export type AgentMeetingCandidate = {
  meeting_id: number;
  title: string;
  started_at: string | null;
  attendees: string[];
  segment_count: number;
  summary_available: boolean;
  notes_available: boolean;
};
export type AgentContextRequest = {
  id: string;
  client_name: string;
  runtime_name: string | null;
  purpose: string;
  query: string;
  created_at: string;
  expires_at: string;
  requested: AgentContextOptions;
  candidates: AgentMeetingCandidate[];
};
export type AgentContextPreview = {
  request_id: string;
  meeting_id: number;
  title: string;
  resource_uri: string;
  source_revision: string;
  packet_hash: string;
  content: string;
  total_bytes: number;
  estimated_tokens: number;
  included: { summary: boolean; notes: boolean; transcript: boolean };
};
export type AgentContextResolveResult = {
  status: "approved" | "denied";
  request_id: string;
  pass_id: string | null;
};
export type AgentContextReceipt = {
  id: string;
  client_name: string;
  runtime_name: string | null;
  purpose: string;
  resource_uri: string | null;
  resource_title: string | null;
  status: string;
  total_bytes: number;
  delivered_bytes: number;
  requested_at: string;
  decided_at: string | null;
  completed_at: string | null;
};

// The Journal agent's response: a companion reply (null if the local model was
// unreachable — the reflection is saved regardless) + how many knowledge-graph
// entities the reflection fed.
export type JournalReply = {
  reply: string | null;
  note_id: number;
  entity_count: number;
};

export type AskSource = {
  note_id: number;
  category: string | null;
  event_date: string;
  snippet: string;
};
// A pending action the chat agent proposes (applied only on user confirm).
export type ChatProposal =
  | { action: "create_category"; name: string; description: string; already_exists: boolean }
  | { action: "edit_entry"; entry_id: number; data: Record<string, unknown>; summary: string }
  | { action: "apply_theme"; theme_id: string; theme_name: string; summary: string }
  | {
      action: "create_event";
      title: string;
      date: string; // YYYY-MM-DD
      start: string | null; // "HH:MM"; null = all-day
      end: string | null;
      guests: string[];
      meet: boolean;
      summary: string;
    };

// An entity the graph contributed to an answer ("from the graph" chips).
export type AskEntity = { id: number; name: string; type: string };

// chat() returns either a grounded answer or a proposal awaiting confirmation.
export type AskResult =
  | { kind: "answer"; answer: string; sources: AskSource[]; entities?: AskEntity[] }
  | { kind: "proposal"; proposal: ChatProposal };

export type Recap = {
  content: string;
  period: string;
  period_start: string;
  period_end: string;
  entry_count: number;
};
export type RecapRow = Recap & { id: number; created_at: string };
export type PhoneInfo = { url: string; lan_url: string; token: string; port: number };
export type MobileAuthorityConfirmation = {
  receiptId: string;
  verificationCode: string;
  scopes: string[];
};
export type MobileAuthorityInfo = {
  active: boolean;
  address: string;
  port: number;
  invitationJson: string;
  invitationExpiresAtMs: number;
  pendingConfirmation: MobileAuthorityConfirmation | null;
};
export type SystemSettings = {
  timeZone: string;
  resolvedTimeZone: string;
  systemTimeZone: string;
  preferredName: string | null;
};

export type ReminderSettings = {
  enabled: boolean;
  lead_minutes: number;
};

export type StoredImagePayload = {
  dataBase64: string;
  mimeType: string;
};

export type TrendRow = { date: string; label: string; values: Record<string, number> };
export type Trends = {
  items_field: string | null;
  label_field: string | null;
  metrics: string[];
  labels: string[];
  rows: TrendRow[];
  count_by_date: [string, number][];
};

export const api = {
  teamStatus: () => invoke<import("./teams/types").TeamSession>("team_status"),
  teamConnect: (server: string, mode: string, secret: string, organization = "", name = "") =>
    invoke<import("./teams/types").TeamOrg[]>("team_connect", { server, mode, secret, organization, name }),
  teamDisconnect: () => invoke<void>("team_disconnect"),
  teamRequest: <T>(method: string, path: string, body?: unknown) => invoke<T>("team_request", { method, path, body }),
  teamAsk: (org: string, body: unknown) => invoke<import("./teams/types").TeamAnswer>("team_ask", { org, body }),
  teamPublishMeeting: (args: { org: string; id: number; spaceId: string; folderIds: string[]; summaryId: number | null; includeTranscript: boolean; sourceKey: string; reviewedContent: { title: string; summary: string; transcript: string; accessVersion: number } }) =>
    invoke<import("./teams/types").TeamNote>("team_publish_meeting", args),

  // Vendor-neutral local MCP clients. All content release still requires an
  // exact approval in the trusted desktop app.
  agentAccessStatus: () => invoke<AgentAccessStatus>("agent_access_status"),
  agentAccessSetEnabled: (enabled: boolean) =>
    invoke<AgentAccessStatus>("agent_access_set_enabled", { enabled }),
  agentClientCreate: (name: string) =>
    invoke<AgentClientSetup>("agent_client_create", { name }),
  agentClientRevoke: (clientId: string) =>
    invoke<AgentAccessStatus>("agent_client_revoke", { clientId }),
  agentContextPending: () =>
    invoke<AgentContextRequest[]>("agent_context_pending"),
  agentContextPreview: (
    requestId: string,
    meetingId: number,
    options: AgentContextOptions,
  ) => invoke<AgentContextPreview>("agent_context_preview", { requestId, meetingId, options }),
  agentContextResolve: (args: {
    requestId: string;
    decision: "approve" | "deny";
    meetingId?: number;
    options?: AgentContextOptions;
    previewHash?: string;
  }) => invoke<AgentContextResolveResult>("agent_context_resolve", {
    requestId: args.requestId,
    decision: args.decision,
    meetingId: args.meetingId ?? null,
    options: args.options ?? null,
    previewHash: args.previewHash ?? null,
  }),
  agentContextReceipts: () =>
    invoke<AgentContextReceipt[]>("agent_context_receipts"),
  health: () => invoke<Health>("health"),
  systemSettingsGet: () => invoke<SystemSettings>("system_settings_get"),
  systemSettingsSet: (timeZone: string, preferredName?: string) =>
    invoke<SystemSettings>("system_settings_set", {
      timeZone,
      ...(preferredName === undefined ? {} : { preferredName }),
    }),
  reminderSettingsGet: () => invoke<ReminderSettings>("reminder_settings_get"),
  reminderSettingsSet: (settings: ReminderSettings) =>
    invoke<ReminderSettings>("reminder_settings_set", { settings }),
  categorize: (text: string) => invoke<Envelope>("categorize_note", { text }),
  categorizePhoto: (imageBase64: string) =>
    invoke<Envelope>("categorize_photo", { imageBase64 }),
  // Transcription only (no extraction) — for flows that re-parse the text
  // themselves, like the Today schedule editor.
  ocrPhoto: (imageBase64: string) => invoke<string>("ocr_photo", { imageBase64 }),
  saveImage: (imageBase64: string, ext: string) =>
    invoke<string>("save_image", { imageBase64, ext }),
  loadImage: (path: string) =>
    invoke<StoredImagePayload>("load_image", { path }),
  save: (args: {
    raw_text: string;
    source?: string;
    image_path?: string | null;
    event_date: string;
    entries: { category: string; description?: string; data: Record<string, unknown> }[];
    entities?: EntityCandidate[];
    filing_context?: "work" | "personal";
    folder_id?: number | null;
  }) => invoke<number>("save_entry", { args }),
  // Instant capture: queue raw text/photo on the Mac (no LLM wait); the Mac
  // categorizes + files it in the background. Returns a pending id.
  quickCapture: (
    rawText: string,
    source?: string,
    imagePath?: string,
    eventDate?: string,
    filingContext?: "work" | "personal"
  ) => invoke<number>("quick_capture", { rawText, source, imagePath, eventDate, filingContext }),
  listEntities: () => invoke<EntityRow[]>("list_entities"),
  mergeEntities: (keep: number, drop: number) => invoke<void>("merge_entities", { keep, drop }),
  suggestEntityMerges: () => invoke<MergeSuggestion[]>("suggest_entity_merges"),
  dismissMergeSuggestion: (a: number, b: number) =>
    invoke<void>("dismiss_merge_suggestion", { a, b }),
  entityGraph: () => invoke<GraphData>("entity_graph"),
  entityDetail: (entityId: number) => invoke<EntityMention[]>("entity_detail", { entityId }),
  entityProfile: (entityId: number) => invoke<EntityProfile>("entity_profile", { entityId }),
  backfillEntities: () => invoke<number>("backfill_entities"),
  listPeople: () => invoke<PersonProfile[]>("list_people"),
  // Person naming: AI proposes display names for email-named people; the user
  // confirms (or types their own) — confirm renames + keeps the email as alias.
  suggestPersonNames: () => invoke<number>("suggest_person_names"),
  confirmPersonName: (entityId: number, name: string) =>
    invoke<void>("confirm_person_name", { entityId, name }),
  dismissPersonName: (entityId: number) => invoke<void>("dismiss_person_name", { entityId }),
  // Rebuild the meeting-fed knowledge layer over all recorded meetings.
  kgReindexMeetings: () =>
    invoke<{ meetings: number; mentions: number; name_suggestions: number }>("kg_reindex_meetings"),
  listNotes: () => invoke<NoteRow[]>("list_notes"),
  createNoteDocument: (
    title: string,
    rawText: string,
    documentJson: string,
    filingContext: "work" | "personal",
    folderId?: number | null,
  ) => invoke<number>("create_note_document", {
    title,
    rawText,
    documentJson,
    filingContext,
    folderId,
  }),
  updateNote: (noteId: number, title: string, rawText: string) =>
    invoke<void>("update_note", { noteId, title, rawText }),
  updateNoteDocument: (
    noteId: number,
    title: string,
    rawText: string,
    documentJson: string,
  ) => invoke<void>("update_note", { noteId, title, rawText, documentJson }),
  noteTrashList: () => invoke<NoteRow[]>("note_trash_list"),
  noteTrash: (noteId: number) => invoke<void>("note_trash", { noteId }),
  noteRestore: (noteId: number) => invoke<void>("note_restore", { noteId }),
  noteDeleteForever: (noteId: number) =>
    invoke<void>("note_delete_forever", { noteId }),
  listCategories: () => invoke<CategoryInfo[]>("list_categories"),
  listNoteFolders: () => invoke<NoteFolderInfo[]>("list_note_folders"),
  createNoteFolder: (parentId: number | null, name: string, kind: "space" | "folder") =>
    invoke<number>("create_note_folder", { parentId, name, kind }),
  renameNoteFolder: (folderId: number, name: string) =>
    invoke<void>("rename_note_folder", { folderId, name }),
  moveNoteFolder: (folderId: number, parentId: number | null, beforeId: number | null) =>
    invoke<void>("move_note_folder", { folderId, parentId, beforeId }),
  deleteNoteFolder: (folderId: number) =>
    invoke<void>("delete_note_folder", { folderId }),
  fileNote: (noteId: number, folderId: number | null) =>
    invoke<NoteFilingReceipt>("file_note", { noteId, folderId }),
  undoNoteFiling: (eventId: number) =>
    invoke<NoteFilingReceipt>("undo_note_filing", { eventId }),
  // `scope` (a brain vault name) restricts retrieval to that vault; `entityId`
  // pins the answer to one item (its brain note + every capture mentioning it).
  chat: (
    question: string,
    history: { role: string; content: string }[],
    scope?: string,
    entityId?: number
  ) => invoke<AskResult>("chat", { question, history, scope, entityId }),
  // Proactive surfacing: brain notes related to in-progress capture text.
  relatedBrain: (text: string) => invoke<RelatedBrain[]>("related_brain", { text }),
  // Journal: save a reflection (always) + get a companion reply and KG entities
  // (local model only — reflections never take the Balanced cloud path).
  journalReflect: (text: string, history: { role: string; content: string }[]) =>
    invoke<JournalReply>("journal_reflect", { text, history }),
  // Auto-propagation (timed write-back + export). Import + embed always run.
  brainGetAuto: () => invoke<boolean>("brain_get_auto"),
  brainSetAuto: (enabled: boolean) => invoke<void>("brain_set_auto", { enabled }),
  createCategory: (name: string, description: string) =>
    invoke<number>("create_category", { name, description }),
  updateEntry: (entryId: number, data: Record<string, unknown>) =>
    invoke<number>("update_entry", { entryId, data }),
  speak: (text: string) => invoke<void>("speak", { text }),
  stopSpeaking: () => invoke<void>("stop_speaking"),
  reindex: () => invoke<number>("reindex"),
  categoryTrends: (category: string) => invoke<Trends>("category_trends", { category }),
  voiceStatus: () => invoke<{ ready: boolean }>("voice_status"),
  downloadVoiceModel: () => invoke<boolean>("download_voice_model"),
  transcribe: (audioB64: string, sampleRate: number) =>
    invoke<string>("transcribe", { audioB64, sampleRate }),
  generateRecap: (period: "day" | "week") => invoke<Recap>("generate_recap", { period }),
  backfillRecaps: () => invoke<void>("backfill_recaps"),
  listRecaps: () => invoke<RecapRow[]>("list_recaps"),
  exportDb: (destination: string) => invoke<string>("export_db", { destination }),
  phoneInfo: () => invoke<PhoneInfo>("phone_info"),
  mobileAuthorityStart: (renew = false) =>
    invoke<MobileAuthorityInfo>("mobile_authority_start", { renew }),
  mobileAuthorityStatus: () => invoke<MobileAuthorityInfo | null>("mobile_authority_status"),
  mobileAuthorityConfirm: (receiptId: string, verificationCode: string, approved: boolean) =>
    invoke<MobileAuthorityInfo>("mobile_authority_confirm", { receiptId, verificationCode, approved }),
  // Themes are data-only, versioned token packs. DESIGN.md compilation and
  // assistant matching always use the local Ollama model, even in Balanced mode.
  themeState: () => invoke<ThemeState>("theme_state"),
  themeList: () => invoke<ThemePack[]>("theme_list"),
  themeSave: (pack: ThemePack) => invoke<ThemePack>("theme_save", { pack }),
  themeActivate: (themeId: string, colorMode?: ThemeModePreference) =>
    invoke<ThemeState>("theme_activate", { themeId, colorMode }),
  themeSetColorMode: (colorMode: ThemeModePreference) =>
    invoke<ThemeState>("theme_set_color_mode", { colorMode }),
  themeDelete: (themeId: string) => invoke<ThemeState>("theme_delete", { themeId }),
  themeCompileDesign: (designMd: string, name?: string) =>
    invoke<ThemePack>("theme_compile_design", { designMd, name }),
  themeSuggest: (prompt: string, candidates: ThemeCandidate[]) =>
    invoke<ThemeSuggestion>("theme_suggest", { prompt, candidates }),
  getProviderSettings: () => invoke<ProviderSettings>("get_provider_settings"),
  setProviderSettings: (args: {
    mode: ProviderMode;
    cloud_provider?: CloudProvider;
    gemini_api_key?: string | null;
    gemini_text_model?: string;
    gemini_vision_model?: string;
    openai_base_url?: string;
    openai_api_key?: string | null;
    openai_text_model?: string;
    openai_vision_model?: string;
    anthropic_api_key?: string | null;
    anthropic_text_model?: string;
    anthropic_vision_model?: string;
    text_model?: string;
    vision_model?: string;
    confirm_embedding_rebuild?: boolean;
  }) =>
    // Tauri maps camelCase JS args → snake_case Rust params, so the payload keys
    // MUST be camelCase. Sending snake_case silently drops them to None — which
    // is why the API key never reached the Keychain.
    invoke<void>("set_provider_settings", {
      mode: args.mode,
      confirmEmbeddingRebuild: args.confirm_embedding_rebuild ?? false,
      cloudProvider: args.cloud_provider,
      geminiApiKey: args.gemini_api_key,
      geminiTextModel: args.gemini_text_model,
      geminiVisionModel: args.gemini_vision_model,
      openaiBaseUrl: args.openai_base_url,
      openaiApiKey: args.openai_api_key,
      openaiTextModel: args.openai_text_model,
      openaiVisionModel: args.openai_vision_model,
      anthropicApiKey: args.anthropic_api_key,
      anthropicTextModel: args.anthropic_text_model,
      anthropicVisionModel: args.anthropic_vision_model,
      textModel: args.text_model,
      visionModel: args.vision_model,
    }),
  testProvider: () => invoke<string>("test_provider"),
  setByokSettings: (
    settings: ByokConfig,
    groqApiKey?: string,
    openaiCompatibleApiKey?: string,
    confirmEmbeddingRebuild = false
  ) => invoke<void>("set_byok_settings", { settings, groqApiKey, openaiCompatibleApiKey, confirmEmbeddingRebuild }),
  listByokModels: (provider: ProviderId, baseUrl = "") =>
    invoke<string[]>("list_byok_models", { provider, baseUrl }),
  testByokSettings: (settings: ByokConfig, keys: {
    openaiApiKey?: string; geminiApiKey?: string; anthropicApiKey?: string;
    groqApiKey?: string; openaiCompatibleApiKey?: string;
  }) => invoke<Record<string, string>>("test_byok_settings", { settings, ...keys }),
  readInboxImage: (path: string) =>
    invoke<{ base64: string; ext: string }>("read_inbox_image", { path }),
  // Google Calendar sync. camelCase arg keys (Tauri maps them → snake_case).
  gcalAuthStatus: () => invoke<GcalStatus>("gcal_auth_status"),
  gcalSetClient: (clientId: string, clientSecret: string) =>
    invoke<void>("gcal_set_client", { clientId, clientSecret }),
  gcalBeginAuth: () => invoke<GcalStatus>("gcal_begin_auth"),
  gcalDisconnect: () => invoke<void>("gcal_disconnect"),
  // Clear one day: delete only noted's events for that date (defaults to today).
  // Returns the number deleted. Other days/calendars are untouched.
  gcalClearDay: (eventDate?: string) => invoke<number>("gcal_clear_day", { eventDate }),
  gcalSync: (eventDate?: string) => invoke<SyncReport>("gcal_sync", { eventDate }),
  gcalListEvents: (eventDate?: string) => invoke<CalEvent[]>("gcal_list_events", { eventDate }),
  // Multi-account management + the Calendar view's feed.
  gcalRemoveAccount: (email: string) => invoke<GcalStatus>("gcal_remove_account", { email }),
  gcalSetCalendarEnabled: (account: string, calendarId: string, enabled: boolean) =>
    invoke<GcalStatus>("gcal_set_calendar_enabled", { account, calendarId, enabled }),
  gcalRefreshCalendars: () => invoke<GcalStatus>("gcal_refresh_calendars"),
  // Apple Calendar is read-only, so there is no create/update/delete pair here.
  // Its events arrive merged into gcalEventsRange.
  applecalStatus: () => invoke<AppleCalStatus>("applecal_status"),
  applecalRequestAccess: () => invoke<AppleCalStatus>("applecal_request_access"),
  applecalSetCalendarEnabled: (calendarId: string, enabled: boolean) =>
    invoke<AppleCalStatus>("applecal_set_calendar_enabled", { calendarId, enabled }),
  gcalSetSyncAccount: (email: string) => invoke<GcalStatus>("gcal_set_sync_account", { email }),
  gcalContacts: () => invoke<GcalContact[]>("gcal_contacts"),
  gcalEventsRange: (startDate: string, endDate: string) =>
    invoke<RangeEvent[]>("gcal_events_range", { startDate, endDate }),
  gcalCreateEvent: (ev: EventInput) =>
    invoke<{ id: string | null }>("gcal_create_event", { ...ev }),
  // `meet`: true attaches a Google Meet, false removes it, undefined keeps it.
  gcalUpdateEvent: (eventId: string, ev: EventInput, moveTo?: string, meet?: boolean) =>
    invoke<void>("gcal_update_event", { eventId, moveTo, meet, ...ev }),
  gcalDeleteEvent: (account: string, calendarId: string, eventId: string) =>
    invoke<void>("gcal_delete_event", { account, calendarId, eventId }),
  // Exact-identity meeting filing. The first matching rule wins; leaving
  // priority null appends a new rule behind the existing ones.
  meetingFilingRules: () => invoke<MeetingFilingRule[]>("meeting_filing_rules"),
  setMeetingFilingRule: (email: string, folderId: number, priority?: number | null) =>
    invoke<MeetingFilingRule>("meeting_filing_rule_set", {
      email,
      folderId,
      priority: priority ?? null,
    }),
  deleteMeetingFilingRule: (email: string) =>
    invoke<boolean>("meeting_filing_rule_delete", { email }),
  reorderMeetingFilingRules: (emails: string[]) =>
    invoke<MeetingFilingRule[]>("meeting_filing_rules_reorder", { emails }),
  meetingFilingBackfillPreview: () =>
    invoke<MeetingFilingBackfillPreview>("meeting_filing_backfill_preview"),
  meetingFilingBackfillApply: (token: string) =>
    invoke<MeetingFilingBackfillApply>("meeting_filing_backfill_apply", { token }),
  // Meetings (local Granola). Capture commands are desktop-only — the phone
  // bridge returns a clean error for them; reads work everywhere.
  meetingModelStatus: () => invoke<MeetingModelStatus>("meeting_model_status"),
  downloadMeetingModel: () => invoke<boolean>("download_meeting_model"),
  downloadSpeakerModel: () => invoke<boolean>("download_speaker_model"),
  downloadInPersonDiarizer: () => invoke<boolean>("download_in_person_diarizer"),
  downloadParakeetModel: () => invoke<boolean>("download_parakeet_model"),
  meetingStart: (args: {
    title?: string;
    eventId?: string;
    eventJson?: unknown;
    retainAudio?: boolean;
    sourceBundle?: string;
    filingContext?: "work" | "personal";
    captureMode?: "online" | "in_person";
  }) => invoke<number>("meeting_start", { ...args }),
  meetingPromptPayload: () => invoke<PromptPayload | null>("meeting_prompt_payload"),
  meetingDismissPrompt: (bundleId?: string) =>
    invoke<void>("meeting_dismiss_prompt", { bundleId }),
  meetingsSettingsGet: () => invoke<MeetingsCfg>("meetings_settings_get"),
  meetingsSettingsSet: (settings: MeetingsCfg) =>
    invoke<void>("meetings_settings_set", { settings }),
  hostedKeySet: (value: string) => invoke<void>("hosted_key_set", { value }),
  // Native window chrome (vibrancy material + NSAppearance) follows the theme.
  setChromeTheme: (dark: boolean) => invoke<void>("set_chrome_theme", { dark }),
  meetingStop: () => invoke<number | null>("meeting_stop"),
  meetingState: () => invoke<MeetingLiveState>("meeting_state"),
  meetingList: () => invoke<MeetingListRow[]>("meeting_list"),
  meetingSearchTranscripts: (
    query: string,
    filters?: TranscriptSearchFilters,
    sort: NoteSortOrder = "date_desc",
    limit = 200
  ) => invoke<TranscriptSearchHit[]>("meeting_search_transcripts", {
    query,
    filters,
    sort,
    limit,
  }),
  meetingSearchFacets: () =>
    invoke<TranscriptSearchFacets>("meeting_search_facets"),
  meetingTranscriptVocabularyList: () =>
    invoke<TranscriptVocabularyRule[]>("meeting_transcript_vocabulary_list"),
  meetingTranscriptVocabularyPreview: (heard: string) =>
    invoke<TranscriptVocabularyPreview>("meeting_transcript_vocabulary_preview", { heard }),
  meetingTranscriptVocabularyApply: (heard: string, preferred: string) =>
    invoke<TranscriptVocabularyApplyResult>("meeting_transcript_vocabulary_apply", {
      heard,
      preferred,
    }),
  meetingTranscriptVocabularyRemove: (id: number) =>
    invoke<void>("meeting_transcript_vocabulary_remove", { id }),
  meetingTranscriptVocabularyUndo: (batchId: number) =>
    invoke<TranscriptVocabularyUndoResult>("meeting_transcript_vocabulary_undo", { batchId }),
  meetingTrashList: () => invoke<MeetingListRow[]>("meeting_trash_list"),
  meetingGet: (id: number) => invoke<MeetingDetail>("meeting_get", { id }),
  meetingTrash: (id: number) => invoke<void>("meeting_trash", { id }),
  meetingRestore: (id: number) => invoke<void>("meeting_restore", { id }),
  meetingDeleteForever: (id: number) => invoke<void>("meeting_delete_forever", { id }),
  meetingSetNotes: (id: number, notes: string, notesDocumentJson?: string | null) =>
    invoke<void>("meeting_set_notes", { id, notes, notesDocumentJson }),
  meetingSetTitle: (id: number, title: string) =>
    invoke<void>("meeting_set_title", { id, title }),
  meetingSetFilingDestination: (id: number, folderId: number) =>
    invoke<void>("meeting_set_filing_destination", { id, folderId }),
  meetingSetSummary: (id: number, summaryId: number, contentMd: string) =>
    invoke<void>("meeting_set_summary", { id, summaryId, contentMd }),
  meetingSummarize: (id: number, template?: string) =>
    invoke<string>("meeting_summarize", { id, template }),
  // Rename propagates only within this meeting. Future meetings stay anonymous.
  meetingRenameSpeaker: (id: number, from: string, to: string) =>
    invoke<void>("meeting_rename_speaker", { id, from, to }),
  // Apply all label changes first, then refresh generated meeting notes once.
  meetingRenameSpeakers: (id: number, changes: { from: string; to: string }[]) =>
    invoke<MeetingSpeakerUpdateResult>("meeting_rename_speakers", { id, changes }),
  // Rebuild anonymous labels, then refresh generated meeting notes once.
  meetingRediarize: (id: number) =>
    invoke<MeetingSpeakerUpdateResult>("meeting_rediarize", { id }),
  // Explicit one-time macOS Screen Recording request. Meeting start never asks.
  meetingVideoRequestPermission: () => invoke<boolean>("meeting_video_request_permission"),
  // Delete the window video now instead of waiting out the retention window.
  meetingVideoDelete: (id: number) => invoke<void>("meeting_video_delete", { id }),
  // Live Assist: answer a question against this meeting's transcript-so-far.
  // Uses the active text provider; works mid-recording and on finished meetings.
  meetingAssist: (id: number, question: string) =>
    invoke<{ answer: string }>("meeting_assist", { id, question }),
  // Writes "<date> <title>.md" into ~/Downloads; resolves to the path.
  meetingExportMd: (id: number) => invoke<string>("meeting_export_md", { id }),
  meetingExportPdf: (id: number, kind: "notes" | "transcript" = "notes", summaryId?: number) =>
    invoke<string>("meeting_export_pdf", { id, kind, summaryId }),
  meetingTemplates: () => invoke<MeetingTemplate[]>("meeting_templates"),
  meetingTemplateSave: (name: string, prompt: string) =>
    invoke<void>("meeting_template_save", { name, prompt }),
  meetingTemplateDelete: (name: string) =>
    invoke<boolean>("meeting_template_delete", { name }),
  meetingCaptureProbe: (seconds?: number) =>
    invoke<Record<string, unknown>>("meeting_capture_probe", { seconds }),
  // Brain-vault sync (Obsidian ↔ noted). camelCase arg keys (Tauri → snake_case).
  brainListVaults: () => invoke<BrainVaultStatus[]>("brain_list_vaults"),
  brainAddVault: (path: string, direction?: string) =>
    invoke<BrainVaultStatus[]>("brain_add_vault", { path, direction }),
  brainRemoveVault: (vault: string) => invoke<void>("brain_remove_vault", { vault }),
  brainSync: (vault?: string) => invoke<BrainSyncReport[]>("brain_sync", { vault }),
  // The Work-tab graph — entities a brain vault touches; omit `vault` for all.
  workGraph: (vault?: string) => invoke<GraphData>("work_graph", { vault }),
  // Write-back (noted -> Obsidian). Preview is a dry run; writeBack commits.
  brainWritePreview: (vault?: string) => invoke<BrainWritePreview[]>("brain_write_preview", { vault }),
  brainWriteBack: (vault?: string) => invoke<BrainWriteReport>("brain_write_back", { vault }),
  // Personal-brain export (noted -> ~/Brain/personal). Preview is a dry run.
  personalExportPreview: () => invoke<BrainWritePreview[]>("personal_export_preview"),
  personalExport: () => invoke<BrainWriteReport>("personal_export"),
};
