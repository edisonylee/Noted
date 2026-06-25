import { invoke as tauriInvoke } from "@tauri-apps/api/core";

// noted runs in two places: the desktop Tauri shell, and a phone browser that
// loads the same UI over the LAN HTTPS server (see src-tauri/src/phone.rs).
// On desktop we use Tauri's IPC; in the browser we POST to /api/<cmd> with the
// access token from the launch URL (?t=…), cached so it survives reloads.
export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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
export type EntityRow = { id: number; name: string; type: string; mention_count: number };

// Knowledge-graph ("Self" view) shapes. Work-graph nodes also carry `vault`.
export type GraphNode = { id: number; name: string; type: string; mention_count: number; vault?: string };
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
// extract/OCR hot path to Gemini while keeping embeddings + chat local.
export type ProviderMode = "local" | "balanced";
export type ProviderSettings = {
  mode: ProviderMode;
  gemini_text_model: string;
  gemini_vision_model: string;
  has_gemini_key: boolean;
};

// Google Calendar sync. noted pushes the day's schedule one-way into a dedicated
// "noted" calendar; auth is OAuth (tokens live in the macOS Keychain).
export type GcalStatus = {
  connected: boolean; // a refresh token is stored
  has_client: boolean; // the OAuth client id + secret are set
  account_email: string | null;
  calendar_id: string | null;
};
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

export type AskSource = {
  note_id: number;
  category: string | null;
  event_date: string;
  snippet: string;
};
// A pending action the chat agent proposes (applied only on user confirm).
export type ChatProposal =
  | { action: "create_category"; name: string; description: string; already_exists: boolean }
  | { action: "edit_entry"; entry_id: number; data: Record<string, unknown>; summary: string };

// chat() returns either a grounded answer or a proposal awaiting confirmation.
export type AskResult =
  | { kind: "answer"; answer: string; sources: AskSource[] }
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
  entityGraph: () => invoke<GraphData>("entity_graph"),
  entityDetail: (entityId: number) => invoke<EntityMention[]>("entity_detail", { entityId }),
  entityProfile: (entityId: number) => invoke<EntityProfile>("entity_profile", { entityId }),
  backfillEntities: () => invoke<number>("backfill_entities"),
  listPeople: () => invoke<PersonProfile[]>("list_people"),
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
  getProviderSettings: () => invoke<ProviderSettings>("get_provider_settings"),
  setProviderSettings: (args: {
    mode: ProviderMode;
    gemini_api_key?: string | null;
    gemini_text_model?: string;
    gemini_vision_model?: string;
  }) =>
    // Tauri maps camelCase JS args → snake_case Rust params, so the payload keys
    // MUST be camelCase. Sending snake_case silently drops them to None — which
    // is why the API key never reached the Keychain.
    invoke<void>("set_provider_settings", {
      mode: args.mode,
      geminiApiKey: args.gemini_api_key,
      geminiTextModel: args.gemini_text_model,
      geminiVisionModel: args.gemini_vision_model,
    }),
  testProvider: () => invoke<string>("test_provider"),
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
