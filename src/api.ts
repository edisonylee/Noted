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

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isDesktop) return tauriInvoke<T>(cmd, args as Record<string, unknown>);
  const res = await fetch(`/api/${cmd}?t=${encodeURIComponent(webToken())}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
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

// Knowledge-graph ("Self" view) shapes.
export type GraphNode = { id: number; name: string; type: string; mention_count: number };
export type GraphEdge = { source: number; target: number; weight: number };
export type GraphData = { nodes: GraphNode[]; edges: GraphEdge[] };
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

export type NoteEntry = { category: string | null; data: Record<string, unknown> | null };

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
export type PhoneInfo = { url: string; token: string; port: number };

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
  listEntities: () => invoke<EntityRow[]>("list_entities"),
  mergeEntities: (keep: number, drop: number) => invoke<void>("merge_entities", { keep, drop }),
  entityGraph: () => invoke<GraphData>("entity_graph"),
  entityDetail: (entityId: number) => invoke<EntityMention[]>("entity_detail", { entityId }),
  listPeople: () => invoke<PersonProfile[]>("list_people"),
  listNotes: () => invoke<NoteRow[]>("list_notes"),
  listCategories: () => invoke<CategoryInfo[]>("list_categories"),
  chat: (question: string, history: { role: string; content: string }[]) =>
    invoke<AskResult>("chat", { question, history }),
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
  }) => invoke<void>("set_provider_settings", args),
  testProvider: () => invoke<string>("test_provider"),
  readInboxImage: (path: string) =>
    invoke<{ base64: string; ext: string }>("read_inbox_image", { path }),
};
