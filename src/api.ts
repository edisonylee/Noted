import { invoke } from "@tauri-apps/api/core";

// One extracted observation within a note (a category + its structured data).
export type Proposal = {
  category: string;
  is_new_category: boolean;
  description: string;
  routed_by?: "header" | "classifier"; // how the category was decided
  data: Record<string, unknown>;
};

// What categorize/categorizePhoto return: the note-level envelope wrapping one
// or more entries (a single note can fill several categories).
export type Envelope = {
  raw_text?: string; // original text, or the transcription on the photo path
  event_date: string; // canonical day (YYYY-MM-DD), extracted or defaulted to today
  date_was_extracted?: boolean; // true if read from the note, false if defaulted
  entries: Proposal[];
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

export type AskSource = {
  note_id: number;
  category: string | null;
  event_date: string;
  snippet: string;
};
export type AskResult = { answer: string; sources: AskSource[] };

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
  }) => invoke<number>("save_entry", { args }),
  listNotes: () => invoke<NoteRow[]>("list_notes"),
  listCategories: () => invoke<CategoryInfo[]>("list_categories"),
  chat: (question: string, history: { role: string; content: string }[]) =>
    invoke<AskResult>("chat", { question, history }),
  speak: (text: string) => invoke<void>("speak", { text }),
  stopSpeaking: () => invoke<void>("stop_speaking"),
  reindex: () => invoke<number>("reindex"),
  categoryTrends: (category: string) => invoke<Trends>("category_trends", { category }),
  voiceStatus: () => invoke<{ ready: boolean }>("voice_status"),
  downloadVoiceModel: () => invoke<boolean>("download_voice_model"),
  transcribe: (audioB64: string, sampleRate: number) =>
    invoke<string>("transcribe", { audioB64, sampleRate }),
  generateRecap: (period: "day" | "week") => invoke<Recap>("generate_recap", { period }),
  listRecaps: () => invoke<RecapRow[]>("list_recaps"),
  exportDb: () => invoke<string>("export_db"),
  phoneInfo: () => invoke<PhoneInfo>("phone_info"),
  readInboxImage: (path: string) =>
    invoke<{ base64: string; ext: string }>("read_inbox_image", { path }),
};
