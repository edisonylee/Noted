import { convertFileSrc, invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { ThemeModePreference, ThemePack } from "./themes/types";

// noted runs in two places: the desktop Tauri shell, and a phone browser that
// loads the same UI over the LAN HTTPS server (see src-tauri/src/phone.rs).
// On desktop we use Tauri's IPC; in the browser we POST to /api/<cmd> with the
// access token from the launch URL (?t=…), cached so it survives reloads.
export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Playable URL for a locally retained file (meeting audio) via the Tauri
// asset protocol. Desktop only — the phone bridge doesn't serve raw files.
export function localFileUrl(path: string | null | undefined): string | null {
  return isDesktop && path ? convertFileSrc(path) : null;
}

function webToken(): string {
  const fromUrl = new URLSearchParams(window.location.search).get("t");
  if (fromUrl) {
    localStorage.setItem("noted_token", fromUrl);
    return fromUrl;
  }
  return localStorage.getItem("noted_token") ?? "";
}

// Thrown when the server rejects our token (403) — the app catches this to show
// a friendly "re-scan the QR" state instead of a raw "forbidden".
export class TokenError extends Error {
  constructor() {
    super("Connection expired — re-scan the QR code shown in noted on your Mac.");
    this.name = "TokenError";
  }
}

// Thrown when the Mac server is simply unreachable (connection refused / network
// drop) — typically while it's restarting mid-rebuild. Distinct from TokenError:
// the token is fine, the server just isn't up yet, so the app waits and retries
// instead of asking the user to re-pair.
export class OfflineError extends Error {
  constructor() {
    super("Reconnecting to your Mac…");
    this.name = "OfflineError";
  }
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isDesktop) return tauriInvoke<T>(cmd, args as Record<string, unknown>);
  let res: Response;
  try {
    res = await fetch(`/api/${cmd}?t=${encodeURIComponent(webToken())}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(args ?? {}),
    });
  } catch {
    // fetch rejects (TypeError) on connection refused / DNS / TLS / network drop:
    // the server is unreachable, not refusing us. Surface as offline so the app
    // shows a "reconnecting" state and polls, rather than a hard error.
    throw new OfflineError();
  }
  if (res.status === 403) {
    // Stale/empty token — drop it so a relaunch from the (tokened) home-screen
    // icon re-captures a fresh one from the URL.
    localStorage.removeItem("noted_token");
    throw new TokenError();
  }
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
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
  raw_text: string;
  source: string;
  entries: NoteEntry[];
  event_date: string;
  created_at: string;
};

export type CategoryInfo = {
  id: number;
  name: string;
  description: string;
  schema: { shape?: unknown; field_freq?: Record<string, number> };
  entry_count: number;
};

export type Health = { models: string[]; vec_version: string };

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
// One remembered guest for event-form autocomplete (harvested from events).
export type GcalContact = { email: string; name: string };
export type SyncReport = {
  created: number;
  updated: number;
  skipped: number; // untimed ("Anytime") blocks, not pushable as events
  duplicates: number; // blocks already on another calendar at the same start+end
  deleted: number; // events for blocks removed since the last sync
  errors: string[];
};
// One event read back from Google Calendar for the Today empty state. Times are
// "HH:MM" Eastern wall-clock; all-day events have start/end null and all_day true.
export type CalEvent = {
  task: string;
  start: string | null;
  end: string | null;
  all_day: boolean;
  calendar: string;
};
// One event in the Calendar view's range feed. Timed events position by
// start_min/end_min — minutes from `date`'s Eastern midnight, where end_min
// > 1440 means it crosses midnight. All-day events carry `end_date` instead
// (Google's EXCLUSIVE end day).
export type RangeEvent = {
  id: string;
  title: string;
  date: string; // YYYY-MM-DD start day (Eastern)
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
  attendees: { name: string; email: string; status: string; self: boolean }[]; // capped at 12, rooms excluded
  attendee_count: number; // real total (rooms excluded)
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
  channel: "me" | "them";
  t0_ms: number;
  t1_ms: number;
  text: string;
  speaker: string | null; // diarized name once available; null = channel default
};
export type MeetingSummary = {
  id: number;
  template: string;
  content_md: string;
  created_at: string;
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
};
// A diarized voice in a meeting. `suggested` is an unconfirmed LLM-mined name
// (confirming = renaming); label "Them" is the lone-unrecognized-voice case.
export type MeetingSpeaker = {
  label: string;
  suggested: string | null;
  seg_count: number;
};
export type MeetingDetail = MeetingListRow & {
  event_id: string | null;
  raw_notes: string;
  audio_me_path: string | null;
  audio_them_path: string | null;
  video_path: string | null; // window recording; null = off/expired/deleted
  segments: MeetingSegment[];
  summaries: MeetingSummary[];
  talk_ms: { me: number; them: number };
  speakers: MeetingSpeaker[];
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
  parakeet: boolean; // Parakeet-TDT ASR engine files present
  hosted: boolean; // scoped Noted API key exists in macOS Keychain
  tap_supported: boolean;
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
  /** macOS voice-processing (AEC) on the mic — strips speaker playback from the mic signal. */
  mic_aec: boolean;
  /** Record the meeting app's window as video (ScreenCaptureKit, macOS 15+). */
  record_video: boolean;
  /** Days before the launch-time sweep deletes window videos; 0 = keep forever. */
  video_keep_days: number;
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
  health: () => invoke<Health>("health"),
  categorize: (text: string) => invoke<Envelope>("categorize_note", { text }),
  categorizePhoto: (imageBase64: string) =>
    invoke<Envelope>("categorize_photo", { imageBase64 }),
  // Transcription only (no extraction) — for flows that re-parse the text
  // themselves, like the Today schedule editor.
  ocrPhoto: (imageBase64: string) => invoke<string>("ocr_photo", { imageBase64 }),
  saveImage: (imageBase64: string, ext: string) =>
    invoke<string>("save_image", { imageBase64, ext }),
  save: (args: {
    raw_text: string;
    source?: string;
    image_path?: string | null;
    event_date: string;
    entries: { category: string; description?: string; data: Record<string, unknown> }[];
    entities?: EntityCandidate[];
  }) => invoke<number>("save_entry", { args }),
  // Instant capture: queue raw text/photo on the Mac (no LLM wait); the Mac
  // categorizes + files it in the background. Returns a pending id.
  quickCapture: (rawText: string, source?: string, imagePath?: string, eventDate?: string) =>
    invoke<number>("quick_capture", { rawText, source, imagePath, eventDate }),
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
  listCategories: () => invoke<CategoryInfo[]>("list_categories"),
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
  exportDb: () => invoke<string>("export_db"),
  phoneInfo: () => invoke<PhoneInfo>("phone_info"),
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
  // Meetings (local Granola). Capture commands are desktop-only — the phone
  // bridge returns a clean error for them; reads work everywhere.
  meetingModelStatus: () => invoke<MeetingModelStatus>("meeting_model_status"),
  downloadMeetingModel: () => invoke<boolean>("download_meeting_model"),
  downloadSpeakerModel: () => invoke<boolean>("download_speaker_model"),
  downloadParakeetModel: () => invoke<boolean>("download_parakeet_model"),
  meetingStart: (args: {
    title?: string;
    eventId?: string;
    eventJson?: unknown;
    retainAudio?: boolean;
    sourceBundle?: string;
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
  meetingGet: (id: number) => invoke<MeetingDetail>("meeting_get", { id }),
  meetingSetNotes: (id: number, notes: string) =>
    invoke<void>("meeting_set_notes", { id, notes }),
  meetingSummarize: (id: number, template?: string) =>
    invoke<string>("meeting_summarize", { id, template }),
  // Rename propagates to the transcript and updates the voiceprint so future
  // meetings auto-label this person; confirming a suggestion is just a rename.
  meetingRenameSpeaker: (id: number, from: string, to: string) =>
    invoke<void>("meeting_rename_speaker", { id, from, to }),
  meetingSuggestSpeakers: (id: number) =>
    invoke<number>("meeting_suggest_speakers", { id }),
  // Rebuild speaker labels from the retained audio — for meetings recorded
  // before diarization existed or interrupted by a crash. Resolves to the
  // voice count (0 = nothing to do).
  meetingRediarize: (id: number) => invoke<number>("meeting_rediarize", { id }),
  // Delete the window video now instead of waiting out the retention window.
  meetingVideoDelete: (id: number) => invoke<void>("meeting_video_delete", { id }),
  // Live Assist: answer a question against this meeting's transcript-so-far.
  // Uses the active text provider; works mid-recording and on finished meetings.
  meetingAssist: (id: number, question: string) =>
    invoke<{ answer: string }>("meeting_assist", { id, question }),
  // Writes "<date> <title>.md" into ~/Downloads; resolves to the path.
  meetingExportMd: (id: number) => invoke<string>("meeting_export_md", { id }),
  meetingExportPdf: (id: number) => invoke<string>("meeting_export_pdf", { id }),
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
