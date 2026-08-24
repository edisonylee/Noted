// SQLite access layer for noted.
// Holds the single connection behind a Mutex in Tauri state. Also owns the
// "emergent schema" logic: each category grows an additive shape + field
// frequency map from the notes the user actually saves.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{ffi::sqlite3_auto_extension, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlite_vec::sqlite3_vec_init;

/// Managed Tauri state. rusqlite's `Connection` is `Send` but not `Sync`,
/// so we wrap it in a `Mutex` to make the state `Send + Sync`.
pub struct Db(pub Mutex<Connection>);

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS categories (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT UNIQUE NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  schema_json TEXT NOT NULL DEFAULT '{"shape":{},"field_freq":{}}',
  entry_count INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS notes (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  title        TEXT NOT NULL DEFAULT '',
  raw_text     TEXT NOT NULL,
  source       TEXT NOT NULL DEFAULT 'text',
  image_path   TEXT,
  category_id  INTEGER REFERENCES categories(id),
  created_at   TEXT NOT NULL,
  -- Provenance: 'capture' for notes the user logged; 'brain:<vault>' for notes
  -- mirrored from an Obsidian brain vault. Capture-listing views filter to
  -- capture-origin so imported brain notes never pollute the daily log/trends.
  origin       TEXT NOT NULL DEFAULT 'capture',
  source_path  TEXT,   -- vault-relative file path (brain notes); sync key
  content_hash TEXT,   -- last-synced content hash (change detection + echo suppression)
  synced_at    TEXT,
  filing_context TEXT,                 -- work|personal; NULL for legacy/imported notes
  trashed_at   TEXT                    -- reversible removal for ordinary capture notes
);

CREATE TABLE IF NOT EXISTS entries (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  note_id     INTEGER NOT NULL REFERENCES notes(id),
  category_id INTEGER NOT NULL REFERENCES categories(id),
  data_json   TEXT NOT NULL,
  event_date  TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

-- User-owned organization is deliberately separate from model-generated
-- categories. A space is a root node; folders can nest beneath spaces or other
-- folders. Memberships are only organizational and never alter note content.
CREATE TABLE IF NOT EXISTS note_folders (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  parent_id   INTEGER REFERENCES note_folders(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK(kind IN ('space', 'folder')),
  auto_rule   TEXT NOT NULL DEFAULT '',
  position    INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_note_folders_parent_name
  ON note_folders(COALESCE(parent_id, 0), name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS note_folder_items (
  folder_id INTEGER NOT NULL REFERENCES note_folders(id) ON DELETE CASCADE,
  note_id   INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  source    TEXT NOT NULL DEFAULT 'manual',
  reason    TEXT NOT NULL DEFAULT 'Previously filed by you.',
  event_id  INTEGER REFERENCES note_filing_events(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (folder_id, note_id)
);
CREATE INDEX IF NOT EXISTS idx_note_folder_items_note ON note_folder_items(note_id);

-- Every filing decision is immutable history. note_folder_items is only the
-- current one-home projection; from_event_id connects a transition to the
-- exact state it replaced, and undoes_event_id makes reversals auditable.
CREATE TABLE IF NOT EXISTS note_filing_events (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  note_id          INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  from_folder_id   INTEGER REFERENCES note_folders(id) ON DELETE SET NULL,
  to_folder_id     INTEGER REFERENCES note_folders(id) ON DELETE SET NULL,
  from_path        TEXT,
  to_path          TEXT,
  from_context     TEXT,
  to_context       TEXT,
  source           TEXT NOT NULL CHECK(source IN ('context', 'rule', 'manual', 'undo')),
  reason           TEXT NOT NULL,
  from_event_id    INTEGER REFERENCES note_filing_events(id) ON DELETE SET NULL,
  undoes_event_id  INTEGER REFERENCES note_filing_events(id) ON DELETE SET NULL,
  created_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_note_filing_events_note
  ON note_filing_events(note_id, id DESC);

-- Deterministic meeting filing. Email identities are exact, normalized keys;
-- priority decides which destination wins when an event spans identities.
-- A deleted destination leaves the rule visible but disabled (folder_id NULL)
-- instead of silently redirecting future meetings somewhere else.
CREATE TABLE IF NOT EXISTS meeting_filing_rules (
  email      TEXT PRIMARY KEY COLLATE NOCASE,
  folder_id  INTEGER REFERENCES note_folders(id) ON DELETE SET NULL,
  priority   INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_meeting_filing_rules_priority
  ON meeting_filing_rules(priority, email);

CREATE VIRTUAL TABLE IF NOT EXISTS embeddings USING vec0(
  note_id   INTEGER PRIMARY KEY,
  embedding FLOAT[768]
);

CREATE TABLE IF NOT EXISTS app_metadata (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS recaps (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  period       TEXT NOT NULL,            -- 'day' | 'week'
  period_start TEXT NOT NULL,
  period_end   TEXT NOT NULL,
  content      TEXT NOT NULL,
  entry_count  INTEGER NOT NULL,
  created_at   TEXT NOT NULL
);

-- Knowledge graph (Phase 2): entities are typed nouns surfaced from notes;
-- mentions link them to the note/entry they appeared in. Edges are derived
-- from co-mention at query time (no edge table).
CREATE TABLE IF NOT EXISTS entities (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  name          TEXT NOT NULL,           -- canonical display name ("Planet Fitness")
  norm          TEXT NOT NULL,           -- dedup key (lowercased, trimmed)
  type          TEXT NOT NULL,           -- person|place|activity|food|item|org|topic
  aliases       TEXT NOT NULL DEFAULT '[]',   -- JSON array of alternate spellings
  relationship  TEXT,                    -- how a person relates to the author (latest stated)
  first_seen    TEXT,
  last_seen     TEXT,
  mention_count INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL,
  home_note_id  INTEGER REFERENCES notes(id), -- the brain note that DEFINES this entity (NULL for capture-only)
  UNIQUE(norm, type)
);

CREATE TABLE IF NOT EXISTS entity_mentions (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  entity_id  INTEGER NOT NULL REFERENCES entities(id),
  note_id    INTEGER NOT NULL REFERENCES notes(id),
  entry_id   INTEGER REFERENCES entries(id),
  context    TEXT,
  event_date TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mention_entity ON entity_mentions(entity_id);
CREATE INDEX IF NOT EXISTS idx_mention_note   ON entity_mentions(note_id);

CREATE VIRTUAL TABLE IF NOT EXISTS entity_embeddings USING vec0(
  entity_id INTEGER PRIMARY KEY,
  embedding FLOAT[768]
);

-- Merge suggestions the user rejected ("not the same"): (lo, hi) entity-id
-- pairs excluded from future suggest_merges passes so a dismissal sticks.
CREATE TABLE IF NOT EXISTS dismissed_merges (
  a INTEGER NOT NULL,
  b INTEGER NOT NULL,
  PRIMARY KEY (a, b)
);

-- Quick-capture queue: a note captured (e.g. from the phone) that hasn't been
-- categorized yet. A background worker runs extraction, writes it as a real
-- note + entries, then deletes the row. `attempts` caps retries on poison rows.
CREATE TABLE IF NOT EXISTS pending_captures (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  raw_text    TEXT NOT NULL,
  source      TEXT NOT NULL DEFAULT 'text',
  image_path  TEXT,
  event_date  TEXT,
  filing_context TEXT,                   -- work|personal; NULL for legacy/unknown
  created_at  TEXT NOT NULL,
  error       TEXT,
  attempts    INTEGER NOT NULL DEFAULT 0
);

-- Registered Obsidian "brain" vaults synced into the knowledge graph. Each is a
-- git repo on disk; `last_git_sha` lets a re-sync diff only what changed.
CREATE TABLE IF NOT EXISTS brain_vaults (
  vault          TEXT PRIMARY KEY,        -- "baro" | "profound" | "personal"
  root_path      TEXT NOT NULL,           -- absolute path to the vault
  direction      TEXT NOT NULL DEFAULT 'import', -- 'import' | 'export' | 'bidi'
  last_git_sha   TEXT,
  last_synced_at TEXT,
  enabled        INTEGER NOT NULL DEFAULT 1
);

-- Meeting recorder. A meeting is a capture session (mic + system audio, two
-- streams); its transcript lives in meeting_segments (large, append-heavy —
-- deliberately NOT in entries.data_json). A searchable projection containing
-- the primary summary plus user-authored notes is filed under the 'meetings'
-- category; note_id links that projection back to this canonical meeting.
CREATE TABLE IF NOT EXISTS meetings (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  public_id      TEXT,                    -- stable UUIDv7 used outside the database boundary
  title          TEXT NOT NULL,
  event_id       TEXT,                    -- gcal event id when calendar-matched
  event_json     TEXT,                    -- event snapshot: attendees, meet_link, times
  started_at     TEXT,
  ended_at       TEXT,
  status         TEXT NOT NULL DEFAULT 'recording', -- recording|summarizing|done|failed
  raw_notes      TEXT NOT NULL DEFAULT '',-- canonical user-authored notes owned by this meeting
  notes_document_json TEXT,                -- formatting for the same notes; never a separate note record
  audio_me_path  TEXT,                    -- retained WAVs (verifiability); NULL if off
  audio_them_path TEXT,
  capture_mode   TEXT NOT NULL DEFAULT 'online', -- online (mic + system) | in_person (room mic)
  asr_engine     TEXT,                    -- resolved engine used for this recording
  asr_model      TEXT,                    -- exact model artifact/provider model at start
  note_id        INTEGER REFERENCES notes(id), -- searchable meeting projection; not a second source of truth
  filing_context TEXT,                   -- work|personal snapshot taken at recording start
  route_folder_id INTEGER REFERENCES note_folders(id) ON DELETE SET NULL,
  route_email    TEXT,                    -- normalized rule identity that matched
  route_via      TEXT,                    -- source_account|organizer|creator|attendee|manual
  route_status   TEXT NOT NULL DEFAULT 'needs_filing', -- matched|needs_filing|manual
  route_updated_at TEXT,
  trashed_at     TEXT,                    -- reversible removal; NULL = visible
  created_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meeting_segments (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  channel    TEXT NOT NULL,               -- 'me' (mic) | 'them' (system audio)
  t0_ms      INTEGER NOT NULL,
  t1_ms      INTEGER NOT NULL,
  voiced_ms  INTEGER,                     -- active VAD frames; NULL on legacy rows
  text       TEXT NOT NULL,
  speaker    TEXT                         -- NULL = channel default; diarization fills later
);
CREATE INDEX IF NOT EXISTS idx_segment_meeting ON meeting_segments(meeting_id, t0_ms);

-- A content-linked FTS index keeps transcript lookup off the recording table.
-- The triggers make each completed ASR segment searchable immediately while
-- avoiding a full transcript scan for every character typed in the UI.
CREATE VIRTUAL TABLE IF NOT EXISTS meeting_segments_fts USING fts5(
  text,
  content='meeting_segments',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS meeting_segments_fts_insert AFTER INSERT ON meeting_segments BEGIN
  INSERT INTO meeting_segments_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS meeting_segments_fts_delete AFTER DELETE ON meeting_segments BEGIN
  INSERT INTO meeting_segments_fts(meeting_segments_fts, rowid, text)
  VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS meeting_segments_fts_update AFTER UPDATE OF text ON meeting_segments BEGIN
  INSERT INTO meeting_segments_fts(meeting_segments_fts, rowid, text)
  VALUES ('delete', old.id, old.text);
  INSERT INTO meeting_segments_fts(rowid, text) VALUES (new.id, new.text);
END;

-- User-owned transcript vocabulary. Rules are applied deterministically after
-- ASR so a preferred spelling survives every transcription engine. Bulk edits
-- keep the prior text per segment, making the latest correction reversible.
CREATE TABLE IF NOT EXISTS transcript_vocabulary (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  heard      TEXT NOT NULL COLLATE NOCASE UNIQUE,
  preferred  TEXT NOT NULL,
  enabled    INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS transcript_correction_batches (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  vocabulary_id       INTEGER REFERENCES transcript_vocabulary(id) ON DELETE SET NULL,
  heard               TEXT NOT NULL,
  preferred           TEXT NOT NULL,
  changed_segments    INTEGER NOT NULL,
  changed_occurrences INTEGER NOT NULL,
  created_at          TEXT NOT NULL,
  undone_at           TEXT
);

CREATE TABLE IF NOT EXISTS transcript_correction_items (
  batch_id    INTEGER NOT NULL REFERENCES transcript_correction_batches(id) ON DELETE CASCADE,
  segment_id  INTEGER NOT NULL REFERENCES meeting_segments(id) ON DELETE CASCADE,
  before_text TEXT NOT NULL,
  after_text  TEXT NOT NULL,
  PRIMARY KEY (batch_id, segment_id)
);
CREATE INDEX IF NOT EXISTS idx_transcript_correction_segment
  ON transcript_correction_items(segment_id);

-- One row per generated summary tab (PLAUD-style multidimensional summaries:
-- regenerating with another template adds a tab, never overwrites).
CREATE TABLE IF NOT EXISTS meeting_summaries (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  template   TEXT NOT NULL,
  content_md TEXT NOT NULL,
  content_json TEXT,
  created_at TEXT NOT NULL
);

-- One row per diarized voice in a meeting: the cluster centroid (f32-le blob)
-- is kept so a later rename can seed/update a persistent voice profile even
-- after the audio is gone. `suggested` holds an unconfirmed LLM-mined name;
-- confirming it is just a rename.
CREATE TABLE IF NOT EXISTS meeting_speakers (
  meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  label      TEXT NOT NULL,               -- current display label ("Speaker 2" or a name)
  centroid   BLOB NOT NULL,
  seg_count  INTEGER NOT NULL,
  suggested  TEXT,
  PRIMARY KEY (meeting_id, label)
);

-- Legacy voiceprint rows retained for additive-schema compatibility. Current
-- meeting labeling never reads or writes this table: group-call identities are
-- manual, while a true calendar 1:1 uses its sole external attendee.
CREATE TABLE IF NOT EXISTS speaker_profiles (
  name       TEXT PRIMARY KEY,
  embedding  BLOB NOT NULL,
  samples    INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);

-- A template is a name + one free-text prompt (PLAUD's model): the prompt
-- describes the sections to extract. builtin rows are re-seeded on startup.
CREATE TABLE IF NOT EXISTS meeting_templates (
  name    TEXT PRIMARY KEY,
  prompt  TEXT NOT NULL,
  builtin INTEGER NOT NULL DEFAULT 0
);

-- Metadata-only record of an approved or denied disclosure to a registered
-- local agent. Context Pass plaintext is never stored in SQLite.
CREATE TABLE IF NOT EXISTS agent_context_receipts (
  id              TEXT PRIMARY KEY,
  request_id      TEXT NOT NULL,
  pass_id         TEXT,
  client_id       TEXT NOT NULL,
  client_name     TEXT NOT NULL,
  runtime_name    TEXT,
  purpose         TEXT NOT NULL,
  resource_uri    TEXT,
  resource_title  TEXT,
  source_revision TEXT,
  packet_hash     TEXT,
  included_json   TEXT NOT NULL DEFAULT '{}',
  status          TEXT NOT NULL,
  total_bytes     INTEGER NOT NULL DEFAULT 0,
  delivered_bytes INTEGER NOT NULL DEFAULT 0,
  requested_at    TEXT NOT NULL,
  decided_at      TEXT,
  completed_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_agent_context_receipts_created
  ON agent_context_receipts(requested_at DESC);
"#;

/// Generate a UUIDv7-compatible public identifier without exposing a SQLite
/// row id. The 48-bit timestamp keeps creation order while 74 random bits make
/// identifiers unguessable for the local-agent boundary.
pub fn new_public_id() -> String {
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let timestamp = timestamp_ms.to_be_bytes();
    bytes[..6].copy_from_slice(&timestamp[2..]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Register sqlite-vec as an auto extension (process-wide, must happen before
/// the connection is opened) and create the schema. Called once at startup.
pub fn init(db_path: &Path) -> Result<Connection> {
    // SAFETY: standard sqlite-vec registration. Must run before Connection::open.
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    // Migrations for DBs created before a column existed (additive only).
    ensure_column(&conn, "entries", "event_date", "TEXT")?;
    ensure_column(&conn, "entities", "relationship", "TEXT")?;
    ensure_column(&conn, "notes", "title", "TEXT")?;
    // Brain-sync columns (additive; legacy rows read as capture-origin via COALESCE).
    ensure_column(&conn, "notes", "origin", "TEXT")?;
    ensure_column(&conn, "notes", "source_path", "TEXT")?;
    ensure_column(&conn, "notes", "content_hash", "TEXT")?;
    ensure_column(&conn, "notes", "synced_at", "TEXT")?;
    ensure_column(&conn, "notes", "filing_context", "TEXT")?;
    ensure_column(&conn, "notes", "trashed_at", "TEXT")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_notes_trashed_at ON notes(trashed_at)",
        [],
    )?;
    ensure_column(
        &conn,
        "note_folder_items",
        "source",
        "TEXT NOT NULL DEFAULT 'manual'",
    )?;
    ensure_column(
        &conn,
        "note_folder_items",
        "reason",
        "TEXT NOT NULL DEFAULT 'Previously filed by you.'",
    )?;
    ensure_column(
        &conn,
        "note_folder_items",
        "event_id",
        "INTEGER REFERENCES note_filing_events(id) ON DELETE SET NULL",
    )?;
    // The former schema allowed more than one explicit membership per note,
    // even though every UI move already replaced the prior membership. Repair
    // any hand-edited/imported duplicates before enforcing the one-home model.
    conn.execute(
        "DELETE FROM note_folder_items AS older
         WHERE EXISTS (
           SELECT 1 FROM note_folder_items AS newer
           WHERE newer.note_id = older.note_id
             AND (newer.created_at > older.created_at
               OR (newer.created_at = older.created_at AND newer.rowid > older.rowid))
         )",
        [],
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_note_folder_items_one_home
         ON note_folder_items(note_id)",
        [],
    )?;
    ensure_column(&conn, "pending_captures", "filing_context", "TEXT")?;
    ensure_column(&conn, "entities", "home_note_id", "INTEGER")?;
    // Person naming: AI-proposed display name awaiting the user's confirm
    // (people filed from meeting attendees start out named by raw email).
    ensure_column(&conn, "entities", "suggested_name", "TEXT")?;
    ensure_column(&conn, "meetings", "video_path", "TEXT")?;
    ensure_column(&conn, "meetings", "public_id", "TEXT")?;
    ensure_column(&conn, "meetings", "trashed_at", "TEXT")?;
    ensure_column(&conn, "meetings", "asr_engine", "TEXT")?;
    ensure_column(&conn, "meetings", "asr_model", "TEXT")?;
    ensure_column(&conn, "meetings", "notes_document_json", "TEXT")?;
    ensure_column(
        &conn,
        "meetings",
        "capture_mode",
        "TEXT NOT NULL DEFAULT 'online'",
    )?;
    ensure_column(&conn, "meetings", "filing_context", "TEXT")?;
    ensure_column(
        &conn,
        "meetings",
        "route_folder_id",
        "INTEGER REFERENCES note_folders(id) ON DELETE SET NULL",
    )?;
    ensure_column(&conn, "meetings", "route_email", "TEXT")?;
    ensure_column(&conn, "meetings", "route_via", "TEXT")?;
    ensure_column(
        &conn,
        "meetings",
        "route_status",
        "TEXT NOT NULL DEFAULT 'needs_filing'",
    )?;
    ensure_column(&conn, "meetings", "route_updated_at", "TEXT")?;
    // Future conversation pace uses speech-only VAD time. Historical rows stay
    // NULL so the UI can withhold pace instead of presenting padded spans as
    // precise articulation timing.
    ensure_column(&conn, "meeting_segments", "voiced_ms", "INTEGER")?;
    // Meeting Pack v2 keeps a structured source of truth next to the Markdown
    // projection used by search, older clients, and user edits.
    ensure_column(&conn, "meeting_summaries", "content_json", "TEXT")?;
    backfill_meeting_public_ids(&conn)?;
    initialize_meeting_filing_provenance(&conn)?;
    initialize_meeting_transcript_index(&conn)?;
    seed_note_folders(&conn)?;
    seed_note_folder_structure_v2(&conn)?;
    initialize_semantic_folder_rules(&conn)?;
    crate::meeting::store::initialize_one_on_one_speakers(&conn)?;
    initialize_embedding_fingerprint(&conn, &crate::provider::active_embedding_fingerprint())?;
    // Note: the reserved catch-all "misc" is not pre-seeded — the classifier is
    // told about it by name in the prompt, and it's created on first real use
    // (so an unused misc never clutters the catalog/UI).
    Ok(conn)
}

fn backfill_meeting_public_ids(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT id FROM meetings WHERE public_id IS NULL OR trim(public_id) = '' ORDER BY id",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for id in ids {
        conn.execute(
            "UPDATE meetings SET public_id = ?2 WHERE id = ?1",
            rusqlite::params![id, new_public_id()],
        )?;
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_meetings_public_id ON meetings(public_id)",
        [],
    )?;
    Ok(())
}

/// Existing explicit folder memberships predate route provenance and are
/// necessarily user-owned. Mark them manual once so backfill can never move
/// those notes. Everything else starts in the reviewable needs-filing state.
fn initialize_meeting_filing_provenance(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE meetings
         SET route_folder_id = (
               SELECT i.folder_id FROM note_folder_items i
               WHERE i.note_id = meetings.note_id
               ORDER BY i.created_at DESC, i.folder_id LIMIT 1
             ),
             route_email = NULL,
             route_via = 'manual',
             route_status = 'manual',
             route_updated_at = COALESCE(route_updated_at, created_at)
         WHERE route_via IS NULL
           AND note_id IS NOT NULL
           AND EXISTS (SELECT 1 FROM note_folder_items i WHERE i.note_id = meetings.note_id)",
        [],
    )?;
    conn.execute(
        "UPDATE meetings SET route_status = 'needs_filing'
         WHERE route_status IS NULL OR route_status = ''",
        [],
    )?;
    Ok(())
}

/// Existing databases predate the transcript FTS table. Rebuild it once from
/// saved segments; afterward the schema triggers keep it current in real time.
fn initialize_meeting_transcript_index(conn: &Connection) -> Result<()> {
    let indexed: Option<String> = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'meeting_transcripts_fts_v1'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if indexed.is_some() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO meeting_segments_fts(meeting_segments_fts) VALUES ('rebuild')",
        [],
    )?;
    tx.execute(
        "INSERT INTO app_metadata (key, value) VALUES ('meeting_transcripts_fts_v1', '1')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Give the notes library a useful first filing tree once. The metadata
/// marker means deleting or renaming any of these later is respected.
fn seed_note_folders(conn: &Connection) -> Result<()> {
    let seeded: Option<String> = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'note_folders_v1_seeded'",
            [],
            |r| r.get(0),
        )
        .ok();
    if seeded.is_some() {
        return Ok(());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO note_folders (parent_id, name, kind, auto_rule, position, created_at)
         VALUES (NULL, 'Work', 'space', '', 0, ?1)",
        [&now],
    )?;
    let work_id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO note_folders (parent_id, name, kind, auto_rule, position, created_at)
         VALUES (NULL, 'Personal', 'space', '', 1, ?1)",
        [&now],
    )?;
    tx.execute(
        "INSERT INTO note_folders (parent_id, name, kind, auto_rule, position, created_at)
         VALUES (?1, 'Baro', 'folder', '', 0, ?2)",
        rusqlite::params![work_id, now],
    )?;
    let baro_id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO note_folders (parent_id, name, kind, auto_rule, position, created_at)
         VALUES (?1, 'Daily Standup Meeting Notes', 'folder', 'daily_standup', 0, ?2)",
        rusqlite::params![baro_id, now],
    )?;
    tx.execute(
        "INSERT INTO app_metadata (key, value) VALUES ('note_folders_v1_seeded', '1')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Add the agreed Work and Personal organization once. Folder placement stays
/// user-owned: this creates empty destinations but does not guess where notes
/// belong. The marker also means later renames or deletions are respected.
fn seed_note_folder_structure_v2(conn: &Connection) -> Result<()> {
    let seeded: Option<String> = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'note_folders_v2_seeded'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if seeded.is_some() {
        return Ok(());
    }

    let work_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Work' COLLATE NOCASE",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let personal_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal' COLLATE NOCASE",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let has_all_roots = work_id.is_some() && personal_id.is_some();

    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    for (parent_id, names) in [
        (work_id, &["Symphony", "Side Projects", "Career"][..]),
        (
            personal_id,
            &[
                "Health",
                "Finances",
                "Home",
                "Relationships",
                "Travel",
                "Personal Learning",
            ][..],
        ),
    ] {
        let Some(parent_id) = parent_id else {
            continue;
        };
        for name in names {
            tx.execute(
                "INSERT OR IGNORE INTO note_folders
                   (parent_id, name, kind, auto_rule, position, created_at)
                 VALUES (
                   ?1,
                   ?2,
                   'folder',
                   '',
                   (SELECT COALESCE(MAX(position), -1) + 1
                    FROM note_folders WHERE parent_id IS ?1),
                   ?3
                 )",
                rusqlite::params![parent_id, name, now],
            )?;
        }
    }
    if has_all_roots {
        tx.execute(
            "INSERT INTO app_metadata (key, value) VALUES ('note_folders_v2_seeded', '1')",
            [],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Give conventional meeting folders durable semantics once, without making
/// their display names part of every future routing decision. Renaming one of
/// these folders preserves the rule; deleting it removes the destination.
fn initialize_semantic_folder_rules(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE note_folders SET auto_rule = 'one_on_one'
         WHERE auto_rule = '' AND kind = 'folder'
           AND lower(replace(replace(name, '-', ' '), '_', ' '))
               IN ('one on ones', 'one on one', '1:1s', '1:1')",
        [],
    )?;
    conn.execute(
        "UPDATE note_folders SET auto_rule = 'external_partner'
         WHERE auto_rule = '' AND kind = 'folder'
           AND lower(replace(replace(name, '-', ' '), '_', ' '))
               IN ('partner meetings', 'partner meeting', 'partners')",
        [],
    )?;
    Ok(())
}

fn inferred_folder_rule(name: &str) -> &'static str {
    let normalized = name.trim().to_lowercase().replace(['-', '_'], " ");
    match normalized.as_str() {
        "one on ones" | "one on one" | "1:1s" | "1:1" => "one_on_one",
        "partner meetings" | "partner meeting" | "partners" => "external_partner",
        _ => "",
    }
}

/// Add a column if a pre-existing table is missing it. No-op on fresh DBs where
/// the CREATE TABLE already includes it. ALTER adds it nullable; new writes always
/// populate it and reads COALESCE legacy NULLs.
fn ensure_column(conn: &Connection, table: &str, col: &str, decl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|x| x.ok())
        .any(|name| name == col);
    drop(stmt);
    if !has {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl};"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Read helpers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CategoryInfo {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub entry_count: i64,
}

pub fn list_categories(conn: &Connection) -> Result<Vec<CategoryInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, schema_json, entry_count
         FROM categories ORDER BY entry_count DESC, name",
    )?;
    let rows = stmt.query_map([], |r| {
        let schema_str: String = r.get(3)?;
        Ok(CategoryInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            schema: serde_json::from_str(&schema_str).unwrap_or_else(|_| json!({})),
            entry_count: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Compact catalog string injected into the categorize prompt so the model
/// reuses existing categories and only invents a name when nothing fits.
pub fn category_catalog(conn: &Connection) -> Result<String> {
    let cats = list_categories(conn)?;
    if cats.is_empty() {
        return Ok("(none yet — this is the first note)".to_string());
    }
    let mut out = String::new();
    for c in cats {
        let shape = c.schema.get("shape").cloned().unwrap_or_else(|| json!({}));
        out.push_str(&format!(
            "- {}: {}. shape: {}\n",
            c.name,
            if c.description.is_empty() {
                "(no description)"
            } else {
                &c.description
            },
            serde_json::to_string(&shape).unwrap_or_default()
        ));
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct NoteEntry {
    pub id: Option<i64>,
    pub category: Option<String>,
    pub data: Value,
}

#[derive(Serialize)]
pub struct NoteRow {
    pub id: i64,
    pub title: String,
    pub raw_text: String,
    pub source: String,
    pub entries: Vec<NoteEntry>,
    pub event_date: String,
    pub created_at: String,
    pub trashed_at: Option<String>,
}

/// Parse a `json_group_array(json_object('category',..,'data',..))` string into
/// entries, dropping the all-null placeholder a LEFT JOIN emits for a note that
/// somehow has no entries.
fn parse_note_entries(s: &str) -> Vec<NoteEntry> {
    let arr: Vec<Value> = serde_json::from_str(s).unwrap_or_default();
    arr.into_iter()
        .filter_map(|v| {
            let id = v.get("id").and_then(|i| i.as_i64());
            let category = v.get("category").and_then(|c| c.as_str()).map(String::from);
            let data = v.get("data").cloned().unwrap_or(Value::Null);
            if category.is_none() && data.is_null() {
                None
            } else {
                Some(NoteEntry { id, category, data })
            }
        })
        .collect()
}

fn list_notes_by_trash(conn: &Connection, trashed: bool) -> Result<Vec<NoteRow>> {
    // One row per note; its entries (category + data) aggregated into a JSON
    // array. Ordered by the day the thing happened (latest entry event_date),
    // falling back to the save day for any legacy rows without one. Meeting
    // notes keep their separate meeting-owned Trash lifecycle.
    let mut stmt = conn.prepare(
        "SELECT n.id, COALESCE(n.title, ''), n.raw_text, n.source,
                COALESCE(MAX(e.event_date), date(n.created_at)) AS event_date,
                json_group_array(json_object('id', e.id, 'category', c.name, 'data', json(e.data_json))) AS entries,
                n.created_at, n.trashed_at
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE (n.origin = 'capture' OR n.origin IS NULL)
           AND (
             (?1 = 0 AND n.trashed_at IS NULL AND NOT EXISTS (
               SELECT 1 FROM meetings m
               WHERE m.note_id = n.id AND m.trashed_at IS NOT NULL
             ))
             OR
             (?1 = 1 AND n.trashed_at IS NOT NULL AND NOT EXISTS (
               SELECT 1 FROM meetings m WHERE m.note_id = n.id
             ))
           )
         GROUP BY n.id
         ORDER BY CASE WHEN ?1 = 1 THEN n.trashed_at ELSE event_date END DESC,
                  n.id DESC",
    )?;
    let rows = stmt.query_map([trashed], |r| {
        let entries_str: String = r.get(5)?;
        Ok(NoteRow {
            id: r.get(0)?,
            title: r.get(1)?,
            raw_text: r.get(2)?,
            source: r.get(3)?,
            event_date: r.get(4)?,
            entries: parse_note_entries(&entries_str),
            created_at: r.get(6)?,
            trashed_at: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_notes(conn: &Connection) -> Result<Vec<NoteRow>> {
    list_notes_by_trash(conn, false)
}

pub fn list_trashed_notes(conn: &Connection) -> Result<Vec<NoteRow>> {
    list_notes_by_trash(conn, true)
}

fn ordinary_note_state(
    conn: &Connection,
    note_id: i64,
) -> Result<(Option<String>, Option<String>)> {
    let row = conn
        .query_row(
            "SELECT n.trashed_at, n.image_path,
                    COALESCE(n.origin, 'capture'),
                    EXISTS(SELECT 1 FROM meetings m WHERE m.note_id = n.id)
             FROM notes n WHERE n.id = ?1",
            [note_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("note not found"))?;
    if row.2 != "capture" {
        return Err(anyhow::anyhow!("only captured notes can be moved to Trash"));
    }
    if row.3 {
        return Err(anyhow::anyhow!(
            "meeting notes must use the meeting Trash lifecycle"
        ));
    }
    Ok((row.0, row.1))
}

/// Refresh cached counts against visible notes while leaving the underlying
/// rows intact so moving a note out of Trash restores its knowledge context.
fn refresh_visible_note_aggregates(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE categories SET entry_count =
           (SELECT COUNT(*)
            FROM entries e JOIN notes n ON n.id = e.note_id
            WHERE e.category_id = categories.id AND n.trashed_at IS NULL)",
        [],
    )?;
    conn.execute(
        "UPDATE entities SET
           mention_count =
             (SELECT COUNT(*)
              FROM entity_mentions m JOIN notes n ON n.id = m.note_id
              WHERE m.entity_id = entities.id AND n.trashed_at IS NULL),
           first_seen =
             (SELECT MIN(m.event_date)
              FROM entity_mentions m JOIN notes n ON n.id = m.note_id
              WHERE m.entity_id = entities.id AND n.trashed_at IS NULL),
           last_seen =
             (SELECT MAX(m.event_date)
              FROM entity_mentions m JOIN notes n ON n.id = m.note_id
              WHERE m.entity_id = entities.id AND n.trashed_at IS NULL)",
        [],
    )?;
    Ok(())
}

pub fn trash_note(conn: &Connection, note_id: i64, now: &str) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let (trashed_at, _) = ordinary_note_state(&tx, note_id)?;
    if trashed_at.is_some() {
        return Ok(false);
    }
    tx.execute(
        "UPDATE notes SET trashed_at = ?2 WHERE id = ?1",
        rusqlite::params![note_id, now],
    )?;
    refresh_visible_note_aggregates(&tx)?;
    tx.commit()?;
    Ok(true)
}

pub fn restore_note(conn: &Connection, note_id: i64) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let (trashed_at, _) = ordinary_note_state(&tx, note_id)?;
    if trashed_at.is_none() {
        return Ok(false);
    }
    tx.execute(
        "UPDATE notes SET trashed_at = NULL WHERE id = ?1",
        [note_id],
    )?;
    refresh_visible_note_aggregates(&tx)?;
    tx.commit()?;
    Ok(true)
}

#[derive(Debug)]
pub struct DeletedNote {
    pub image_path: Option<String>,
}

/// Permanently remove an ordinary note only after it has entered Trash. Folder
/// memberships and filing history cascade from `notes`; the remaining derived
/// data has explicit cleanup because its foreign keys intentionally do not.
pub fn delete_note_forever(conn: &mut Connection, note_id: i64) -> Result<Option<DeletedNote>> {
    let tx = conn.transaction()?;
    let (trashed_at, image_path) = ordinary_note_state(&tx, note_id)?;
    if trashed_at.is_none() {
        return Ok(None);
    }

    tx.execute(
        "UPDATE entities SET home_note_id = NULL WHERE home_note_id = ?1",
        [note_id],
    )?;
    tx.execute("DELETE FROM entity_mentions WHERE note_id = ?1", [note_id])?;
    tx.execute("DELETE FROM embeddings WHERE note_id = ?1", [note_id])?;
    tx.execute("DELETE FROM entries WHERE note_id = ?1", [note_id])?;
    tx.execute("DELETE FROM notes WHERE id = ?1", [note_id])?;
    let image_path = match image_path {
        Some(path) => {
            let still_referenced: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE image_path = ?1)",
                [&path],
                |row| row.get(0),
            )?;
            (!still_referenced).then_some(path)
        }
        None => None,
    };
    refresh_visible_note_aggregates(&tx)?;
    tx.commit()?;

    Ok(Some(DeletedNote { image_path }))
}

// ---------------------------------------------------------------------------
// Spaces and folders. These are user-owned organization, independent from the
// extraction categories above. New captures materialize one primary filing;
// the computed rule path below remains only for untouched legacy notes.
// ---------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct NoteFolderItemInfo {
    pub note_id: i64,
    pub filing_context: Option<String>,
    pub source: String,
    pub reason: String,
    pub event_id: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct NoteFolderInfo {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub kind: String,
    pub auto_rule: String,
    pub note_ids: Vec<i64>,
    pub explicit_filings: Vec<NoteFolderItemInfo>,
}

fn matches_daily_standup<'a>(raw_text: &str, categories: impl Iterator<Item = &'a str>) -> bool {
    fn matches(value: &str) -> bool {
        let lower = value.to_lowercase();
        let spaced = lower.replace(['-', '_'], " ");
        lower.contains("standup")
            || lower.contains("stand-up")
            || spaced.contains("daily stand up")
            || spaced.contains("stand up meeting")
            || spaced.contains("daily scrum")
    }

    let categories: Vec<&str> = categories.collect();
    if categories
        .iter()
        .any(|category| category.trim().eq_ignore_ascii_case("schedule"))
    {
        return false;
    }

    matches(raw_text) || categories.into_iter().any(matches)
}

pub fn is_daily_standup(note: &NoteRow) -> bool {
    matches_daily_standup(
        &note.raw_text,
        note.entries
            .iter()
            .filter_map(|entry| entry.category.as_deref()),
    )
}

pub fn list_note_folders(conn: &Connection) -> Result<Vec<NoteFolderInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, name, kind, auto_rule
         FROM note_folders
         ORDER BY parent_id IS NOT NULL, parent_id, position, name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(NoteFolderInfo {
            id: r.get(0)?,
            parent_id: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            auto_rule: r.get(4)?,
            note_ids: Vec::new(),
            explicit_filings: Vec::new(),
        })
    })?;
    let mut folders = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let auto_notes = if folders
        .iter()
        .any(|folder| folder.auto_rule == "daily_standup")
    {
        list_notes(conn)?
    } else {
        Vec::new()
    };
    let decided_note_ids: HashSet<i64> = conn
        .prepare(
            "SELECT note_id FROM note_folder_items
             UNION
             SELECT note_id FROM note_filing_events",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;

    for folder in &mut folders {
        let mut item_stmt = conn.prepare(
            "SELECT i.note_id, n.filing_context, COALESCE(i.source, 'manual'),
                    COALESCE(i.reason, 'Previously filed by you.'), i.event_id
             FROM note_folder_items i
             JOIN notes n ON n.id = i.note_id
             WHERE i.folder_id = ?1 AND n.trashed_at IS NULL
               AND NOT EXISTS (
                 SELECT 1 FROM meetings m
                 WHERE m.note_id = n.id AND m.trashed_at IS NOT NULL
               )
             ORDER BY i.note_id DESC",
        )?;
        let items = item_stmt.query_map([folder.id], |row| {
            Ok(NoteFolderItemInfo {
                note_id: row.get(0)?,
                filing_context: row.get(1)?,
                source: row.get(2)?,
                reason: row.get(3)?,
                event_id: row.get(4)?,
            })
        })?;
        folder.explicit_filings = items.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut note_ids = folder
            .explicit_filings
            .iter()
            .map(|item| item.note_id)
            .collect::<Vec<_>>();

        if folder.auto_rule == "daily_standup" {
            note_ids.extend(
                auto_notes
                    .iter()
                    .filter(|note| !decided_note_ids.contains(&note.id) && is_daily_standup(note))
                    .map(|note| note.id),
            );
        }
        note_ids.sort_unstable();
        note_ids.dedup();
        folder.note_ids = note_ids;
    }
    Ok(folders)
}

pub fn create_note_folder(
    conn: &Connection,
    parent_id: Option<i64>,
    name: &str,
    kind: &str,
    auto_rule: &str,
    now: &str,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow::anyhow!("folder name cannot be empty"));
    }
    if !matches!(
        auto_rule,
        "" | "daily_standup" | "one_on_one" | "external_partner"
    ) {
        return Err(anyhow::anyhow!("unknown folder rule"));
    }
    let auto_rule = if auto_rule.is_empty() {
        inferred_folder_rule(name)
    } else {
        auto_rule
    };
    match parent_id {
        None if kind != "space" => {
            return Err(anyhow::anyhow!("a root item must be a space"));
        }
        Some(parent) => {
            let exists = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
                [parent],
                |r| r.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(anyhow::anyhow!("parent folder not found"));
            }
            if kind != "folder" {
                return Err(anyhow::anyhow!("only folders can be nested"));
            }
        }
        None => {}
    }

    let position: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1
         FROM note_folders
         WHERE parent_id IS ?1",
        [parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO note_folders (parent_id, name, kind, auto_rule, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![parent_id, name, kind, auto_rule, position, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_note_folder(conn: &Connection, folder_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow::anyhow!("folder name cannot be empty"));
    }
    let changed = conn.execute(
        "UPDATE note_folders SET name = ?1 WHERE id = ?2",
        rusqlite::params![name, folder_id],
    )?;
    if changed == 0 {
        return Err(anyhow::anyhow!("folder not found"));
    }
    Ok(())
}

/// Move a folder under a new parent and place it before a sibling. A null
/// `before_id` appends it to the end of the destination. Positions are
/// normalized in both the old and new parents so later reads have one stable
/// order, even after repeated drag operations.
pub fn move_note_folder(
    conn: &Connection,
    folder_id: i64,
    parent_id: Option<i64>,
    before_id: Option<i64>,
) -> Result<()> {
    let (kind, old_parent): (String, Option<i64>) = conn
        .query_row(
            "SELECT kind, parent_id FROM note_folders WHERE id = ?1",
            [folder_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => anyhow::anyhow!("folder not found"),
            other => other.into(),
        })?;
    if kind != "folder" {
        return Err(anyhow::anyhow!("only folders can be moved"));
    }

    if let Some(parent_id) = parent_id {
        let parent_exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
            [parent_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !parent_exists {
            return Err(anyhow::anyhow!("destination folder not found"));
        }

        let creates_cycle = conn.query_row(
            "WITH RECURSIVE descendants(id) AS (
               SELECT id FROM note_folders WHERE id = ?1
               UNION ALL
               SELECT child.id
               FROM note_folders child
               JOIN descendants parent ON child.parent_id = parent.id
             )
             SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)",
            rusqlite::params![folder_id, parent_id],
            |row| row.get::<_, bool>(0),
        )?;
        if creates_cycle {
            return Err(anyhow::anyhow!("a folder cannot be moved inside itself"));
        }
    }

    let destination_parent = parent_id.ok_or_else(|| {
        anyhow::anyhow!("folders must remain inside the Work or Personal context")
    })?;
    let source_context = folder_filing_context(conn, folder_id)?;
    let destination_context = folder_filing_context(conn, destination_parent)?;
    if source_context != destination_context {
        return Err(anyhow::anyhow!(
            "folders cannot move between Work and Personal"
        ));
    }

    if let Some(before_id) = before_id {
        if before_id == folder_id {
            return Err(anyhow::anyhow!("a folder cannot be placed before itself"));
        }
        let before_parent: Option<i64> = conn
            .query_row(
                "SELECT parent_id FROM note_folders WHERE id = ?1 AND kind = 'folder'",
                [before_id],
                |row| row.get(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    anyhow::anyhow!("destination sibling not found")
                }
                other => other.into(),
            })?;
        if before_parent != parent_id {
            return Err(anyhow::anyhow!(
                "destination sibling has a different parent"
            ));
        }
    }

    let ordered_children = |parent: Option<i64>| -> Result<Vec<i64>> {
        let mut stmt = conn.prepare(
            "SELECT id FROM note_folders
             WHERE parent_id IS ?1 AND kind = 'folder'
             ORDER BY position, name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([parent], |row| row.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };

    let mut destination = ordered_children(parent_id)?;
    destination.retain(|id| *id != folder_id);
    let insert_at = match before_id {
        Some(before_id) => destination
            .iter()
            .position(|id| *id == before_id)
            .ok_or_else(|| anyhow::anyhow!("destination sibling not found"))?,
        None => destination.len(),
    };
    destination.insert(insert_at, folder_id);

    let mut source = if old_parent == parent_id {
        Vec::new()
    } else {
        ordered_children(old_parent)?
    };
    source.retain(|id| *id != folder_id);

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE note_folders SET parent_id = ?1 WHERE id = ?2",
        rusqlite::params![parent_id, folder_id],
    )?;
    for (position, id) in source.iter().enumerate() {
        tx.execute(
            "UPDATE note_folders SET position = ?1 WHERE id = ?2",
            rusqlite::params![position as i64, id],
        )?;
    }
    for (position, id) in destination.iter().enumerate() {
        tx.execute(
            "UPDATE note_folders SET position = ?1 WHERE id = ?2",
            rusqlite::params![position as i64, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_note_folder(conn: &Connection, folder_id: i64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    let kind = tx
        .query_row(
            "SELECT kind FROM note_folders WHERE id = ?1",
            [folder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("folder not found"))?;
    if kind == "space" {
        return Err(anyhow::anyhow!(
            "Work and Personal contexts cannot be deleted"
        ));
    }

    let affected_notes = {
        let mut stmt = tx.prepare(
            "WITH RECURSIVE subtree(id) AS (
               SELECT id FROM note_folders WHERE id = ?1
               UNION ALL
               SELECT child.id FROM note_folders child
               JOIN subtree parent ON child.parent_id = parent.id
             )
             SELECT DISTINCT i.note_id, i.folder_id
             FROM note_folder_items i JOIN subtree s ON s.id = i.folder_id",
        )?;
        let rows = stmt.query_map([folder_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let deleted_path = note_folder_path(&tx, folder_id)?;
    for (note_id, previous_folder_id) in affected_notes {
        let context = tx.query_row(
            "SELECT filing_context FROM notes WHERE id = ?1",
            [note_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        let context = match context {
            Some(context) => normalized_filing_context(&context)?,
            // Old explicit memberships predate persisted context. Their
            // actual folder ancestry is still authoritative at deletion time.
            None => folder_filing_context(&tx, previous_folder_id)?,
        };
        let inbox_id = context_space_id(&tx, &context)?;
        let reason = format!(
            "Moved to {} / Inbox because {deleted_path} was deleted.",
            if context == "work" {
                "Work"
            } else {
                "Personal"
            }
        );
        filing_transition(
            &tx,
            note_id,
            Some(inbox_id),
            "context",
            &reason,
            Some(&context),
            None,
            &now,
        )?;
        sync_linked_meeting_route(&tx, note_id, Some(inbox_id), "context", &now)?;
    }

    // Keep disabled rules visible for repair, and make unresolved automatic
    // routes honest. Manual provenance remains manual even though deleting its
    // destination naturally removes the folder membership via CASCADE.
    tx.execute(
        "WITH RECURSIVE subtree(id) AS (
           SELECT id FROM note_folders WHERE id = ?1
           UNION ALL
           SELECT child.id FROM note_folders child JOIN subtree ON child.parent_id = subtree.id
         )
         UPDATE meeting_filing_rules SET folder_id = NULL, updated_at = ?2
         WHERE folder_id IN (SELECT id FROM subtree)",
        rusqlite::params![folder_id, now],
    )?;
    tx.execute(
        "WITH RECURSIVE subtree(id) AS (
           SELECT id FROM note_folders WHERE id = ?1
           UNION ALL
           SELECT child.id FROM note_folders child JOIN subtree ON child.parent_id = subtree.id
         )
         UPDATE meetings SET route_folder_id = NULL,
                route_via = 'destination_missing', route_status = 'needs_filing',
                route_updated_at = ?2
         WHERE route_folder_id IN (SELECT id FROM subtree)
           AND COALESCE(route_status, 'needs_filing') <> 'manual'",
        rusqlite::params![folder_id, now],
    )?;
    tx.execute("DELETE FROM note_folders WHERE id = ?1", [folder_id])?;
    tx.commit()?;
    Ok(())
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct NoteFilingReceipt {
    pub event_id: i64,
    pub note_id: i64,
    pub folder_id: Option<i64>,
    pub previous_folder_id: Option<i64>,
    pub filing_context: Option<String>,
    pub previous_context: Option<String>,
    pub source: String,
    pub reason: String,
}

#[derive(Debug)]
struct CurrentFiling {
    folder_id: Option<i64>,
    event_id: Option<i64>,
    path: Option<String>,
    context: Option<String>,
}

fn note_folder_path(conn: &Connection, folder_id: i64) -> Result<String> {
    let mut names = Vec::new();
    let mut current = Some(folder_id);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(anyhow::anyhow!("folder hierarchy contains a cycle"));
        }
        let row = conn
            .query_row(
                "SELECT name, parent_id FROM note_folders WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        let Some((name, parent_id)) = row else {
            return Err(anyhow::anyhow!("filing target not found"));
        };
        names.push(name);
        current = parent_id;
    }
    names.reverse();
    Ok(names.join(" / "))
}

fn normalized_filing_context(value: &str) -> Result<String> {
    let context = value.trim().to_lowercase();
    if !matches!(context.as_str(), "work" | "personal") {
        return Err(anyhow::anyhow!("filing context must be work or personal"));
    }
    Ok(context)
}

fn context_space_id(conn: &Connection, context: &str) -> Result<i64> {
    let context = normalized_filing_context(context)?;
    conn.query_row(
        "SELECT id FROM note_folders
         WHERE parent_id IS NULL AND kind = 'space' AND name = ?1 COLLATE NOCASE",
        [context],
        |row| row.get(0),
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => anyhow::anyhow!("filing context not found"),
        other => other.into(),
    })
}

pub(crate) fn folder_filing_context(conn: &Connection, folder_id: i64) -> Result<String> {
    let mut current = folder_id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return Err(anyhow::anyhow!("folder hierarchy contains a cycle"));
        }
        let (name, kind, parent_id) = conn
            .query_row(
                "SELECT name, kind, parent_id FROM note_folders WHERE id = ?1",
                [current],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => anyhow::anyhow!("filing target not found"),
                other => other.into(),
            })?;
        match parent_id {
            Some(parent_id) => current = parent_id,
            None if kind == "space" => return normalized_filing_context(&name),
            None => return Err(anyhow::anyhow!("filing target is outside a context")),
        }
    }
}

/// Resolve the Work/Personal root that owns a filing destination.
pub fn note_folder_context(conn: &Connection, folder_id: i64) -> Result<String> {
    folder_filing_context(conn, folder_id)
}

fn folder_is_in_context(conn: &Connection, folder_id: i64, context: &str) -> Result<bool> {
    Ok(folder_filing_context(conn, folder_id)? == normalized_filing_context(context)?)
}

fn current_filing(conn: &Connection, note_id: i64) -> Result<Option<CurrentFiling>> {
    let item = conn
        .query_row(
            "SELECT i.folder_id, i.event_id, n.filing_context
             FROM note_folder_items i
             JOIN notes n ON n.id = i.note_id
             WHERE i.note_id = ?1 LIMIT 1",
            [note_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((folder_id, event_id, context)) = item {
        return Ok(Some(CurrentFiling {
            folder_id: Some(folder_id),
            event_id,
            path: Some(note_folder_path(conn, folder_id)?),
            context,
        }));
    }

    let event = conn
        .query_row(
            "SELECT to_folder_id, id, to_path, to_context
             FROM note_filing_events WHERE note_id = ?1 ORDER BY id DESC LIMIT 1",
            [note_id],
            |row| {
                Ok(CurrentFiling {
                    folder_id: row.get(0)?,
                    event_id: Some(row.get(1)?),
                    path: row.get(2)?,
                    context: row.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(event)
}

pub(crate) fn filing_transition(
    conn: &Connection,
    note_id: i64,
    folder_id: Option<i64>,
    source: &str,
    reason: &str,
    filing_context: Option<&str>,
    undoes_event_id: Option<i64>,
    now: &str,
) -> Result<NoteFilingReceipt> {
    if !matches!(source, "context" | "rule" | "manual" | "undo") {
        return Err(anyhow::anyhow!("unknown filing source"));
    }
    let current = current_filing(conn, note_id)?;
    let previous_folder_id = current.as_ref().and_then(|item| item.folder_id);
    let previous_path = current.as_ref().and_then(|item| item.path.as_deref());
    let previous_event_id = current.as_ref().and_then(|item| item.event_id);
    let previous_context = current.as_ref().and_then(|item| item.context.as_deref());
    let target_path = folder_id.map(|id| note_folder_path(conn, id)).transpose()?;

    conn.execute(
        "INSERT INTO note_filing_events
           (note_id, from_folder_id, to_folder_id, from_path, to_path,
            from_context, to_context, source, reason, from_event_id,
            undoes_event_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            note_id,
            previous_folder_id,
            folder_id,
            previous_path,
            target_path,
            previous_context,
            filing_context,
            source,
            reason,
            previous_event_id,
            undoes_event_id,
            now,
        ],
    )?;
    let event_id = conn.last_insert_rowid();

    conn.execute(
        "UPDATE notes SET filing_context = ?2 WHERE id = ?1",
        rusqlite::params![note_id, filing_context],
    )?;

    conn.execute(
        "DELETE FROM note_folder_items WHERE note_id = ?1",
        [note_id],
    )?;
    if let Some(folder_id) = folder_id {
        conn.execute(
            "INSERT INTO note_folder_items
               (folder_id, note_id, source, reason, event_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![folder_id, note_id, source, reason, event_id, now],
        )?;
    }

    Ok(NoteFilingReceipt {
        event_id,
        note_id,
        folder_id,
        previous_folder_id,
        filing_context: filing_context.map(String::from),
        previous_context: previous_context.map(String::from),
        source: source.to_string(),
        reason: reason.to_string(),
    })
}

fn sync_linked_meeting_route(
    conn: &Connection,
    note_id: i64,
    folder_id: Option<i64>,
    filing_source: &str,
    now: &str,
) -> Result<()> {
    let (route_via, route_status) = match filing_source {
        "rule" => ("filing_rule", "matched"),
        "context" => ("context_inbox", "needs_filing"),
        "manual" => ("manual", "manual"),
        _ if folder_id.is_some() => ("undo", "manual"),
        _ => ("undo", "needs_filing"),
    };
    conn.execute(
        "UPDATE meetings SET filing_context =
                    (SELECT filing_context FROM notes WHERE id = ?1),
                route_folder_id = ?2, route_email = NULL,
                route_via = ?3, route_status = ?4, route_updated_at = ?5
         WHERE note_id = ?1",
        rusqlite::params![note_id, folder_id, route_via, route_status, now],
    )?;
    Ok(())
}

/// Move a note's one explicit filing. Every manual choice appends an audit
/// event, even when the destination is unchanged, because confirming an
/// automatic destination is itself a sticky human decision.
pub fn file_note(
    conn: &Connection,
    note_id: i64,
    folder_id: Option<i64>,
    now: &str,
) -> Result<NoteFilingReceipt> {
    let note_exists = conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM notes
           WHERE id = ?1 AND (origin = 'capture' OR origin IS NULL)
             AND trashed_at IS NULL
         )",
        [note_id],
        |r| r.get::<_, bool>(0),
    )?;
    if !note_exists {
        return Err(anyhow::anyhow!("note not found"));
    }
    if let Some(folder_id) = folder_id {
        let is_target = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
            [folder_id],
            |r| r.get::<_, bool>(0),
        )?;
        if !is_target {
            return Err(anyhow::anyhow!("filing target not found"));
        }
    }

    let tx = conn.unchecked_transaction()?;
    let filing_context = match folder_id {
        Some(folder_id) => Some(folder_filing_context(&tx, folder_id)?),
        None => tx
            .query_row(
                "SELECT filing_context FROM notes WHERE id = ?1",
                [note_id],
                |row| row.get::<_, Option<String>>(0),
            )?
            .map(|value| normalized_filing_context(&value))
            .transpose()?,
    };
    let reason = match folder_id {
        Some(folder_id) => format!("Moved to {} by you.", note_folder_path(&tx, folder_id)?),
        None => "Removed from folders by you.".to_string(),
    };
    let receipt = filing_transition(
        &tx,
        note_id,
        folder_id,
        "manual",
        &reason,
        filing_context.as_deref(),
        None,
        now,
    )?;
    sync_linked_meeting_route(&tx, note_id, folder_id, "manual", now)?;
    tx.commit()?;
    Ok(receipt)
}

/// Reverse exactly the filing event the caller observed. A stale receipt never
/// rewinds a newer manual move; the current event and destination must match.
pub fn undo_note_filing(conn: &Connection, event_id: i64, now: &str) -> Result<NoteFilingReceipt> {
    let tx = conn.unchecked_transaction()?;
    let event = tx
        .query_row(
            "SELECT note_id, from_folder_id, to_folder_id, from_path, to_path,
                    from_context, to_context, reason, from_event_id
             FROM note_filing_events WHERE id = ?1",
            [event_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("filing event not found"))?;
    let (
        note_id,
        from_folder_id,
        to_folder_id,
        from_path,
        _to_path,
        from_context,
        to_context,
        old_reason,
        from_event_id,
    ) = event;

    let note_is_trashed: bool = tx.query_row(
        "SELECT trashed_at IS NOT NULL FROM notes WHERE id = ?1",
        [note_id],
        |row| row.get(0),
    )?;
    if note_is_trashed {
        return Err(anyhow::anyhow!(
            "restore the note before changing its filing"
        ));
    }

    let current = current_filing(&tx, note_id)?
        .ok_or_else(|| anyhow::anyhow!("filing changed since this action"))?;
    if current.event_id != Some(event_id)
        || current.folder_id != to_folder_id
        || current.context != to_context
    {
        return Err(anyhow::anyhow!("filing changed since this action"));
    }
    if from_folder_id.is_none() && from_path.is_some() {
        return Err(anyhow::anyhow!("the previous folder no longer exists"));
    }
    if let Some(folder_id) = from_folder_id {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM note_folders WHERE id = ?1)",
            [folder_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(anyhow::anyhow!("the previous folder no longer exists"));
        }
    }

    let restored_label = from_path.as_deref().unwrap_or("Unfiled");
    let reason = format!("Restored {restored_label} by undoing: {old_reason}");
    let receipt = filing_transition(
        &tx,
        note_id,
        from_folder_id,
        "undo",
        &reason,
        from_context.as_deref(),
        Some(event_id),
        now,
    )?;
    let restored_source = from_event_id
        .and_then(|id| {
            tx.query_row(
                "SELECT source FROM note_filing_events WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
        })
        .unwrap_or_else(|| {
            if from_folder_id.is_some() {
                "manual".to_string()
            } else {
                "context".to_string()
            }
        });
    sync_linked_meeting_route(&tx, note_id, from_folder_id, &restored_source, now)?;
    tx.commit()?;
    Ok(receipt)
}

/// Refresh the human-readable body of a generated note. Its semantic index
/// and knowledge mentions are derived data, so discard both before rebuilding
/// them from the corrected content.
pub fn refresh_note_text(conn: &Connection, note_id: i64, raw_text: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE notes SET raw_text = ?2 WHERE id = ?1 AND trashed_at IS NULL",
        rusqlite::params![note_id, raw_text],
    )?;
    if changed == 0 {
        return Err(anyhow::anyhow!("note not found"));
    }
    conn.execute("DELETE FROM embeddings WHERE note_id = ?1", [note_id])?;
    clear_note_mentions(conn, note_id)?;
    Ok(())
}

/// Save user-owned note fields without rewriting captured text to manufacture a
/// display title. Body edits invalidate derived search and knowledge data;
/// title-only edits only invalidate the semantic index.
pub fn update_note(conn: &Connection, note_id: i64, title: &str, raw_text: &str) -> Result<()> {
    let previous = conn
        .query_row(
            "SELECT raw_text FROM notes
             WHERE id = ?1 AND (origin = 'capture' OR origin IS NULL)
               AND trashed_at IS NULL",
            [note_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Err(anyhow::anyhow!("note not found"));
    };
    conn.execute(
        "UPDATE notes SET title = ?2, raw_text = ?3 WHERE id = ?1",
        rusqlite::params![note_id, title.trim(), raw_text],
    )?;
    conn.execute("DELETE FROM embeddings WHERE note_id = ?1", [note_id])?;
    if previous != raw_text {
        clear_note_mentions(conn, note_id)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Semantic search (sqlite-vec). Vectors are stored as JSON text (sqlite-vec
// parses it); we L2-normalize before insert + query so the default L2 distance
// ranks the same as cosine similarity.
// ---------------------------------------------------------------------------

pub fn insert_embedding(conn: &Connection, note_id: i64, vec: &[f32]) -> Result<()> {
    let json = serde_json::to_string(vec)?;
    conn.execute(
        "INSERT OR REPLACE INTO embeddings(note_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![note_id, json],
    )?;
    Ok(())
}

pub fn embedding_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))?)
}

pub fn embedding_fingerprint(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM app_metadata WHERE key = 'embedding_space'")?;
    let mut rows = stmt.query([])?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

pub fn initialize_embedding_fingerprint(conn: &Connection, fingerprint: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO app_metadata(key, value) VALUES ('embedding_space', ?1)",
        [fingerprint],
    )?;
    Ok(())
}

fn ensure_embedding_space_ready(conn: &Connection) -> Result<()> {
    let expected = crate::provider::active_embedding_fingerprint();
    if embedding_fingerprint(conn)?.as_deref() != Some(&expected) {
        return Err(anyhow::anyhow!(
            "semantic search is rebuilding for the selected embedding model"
        ));
    }
    Ok(())
}

pub fn replace_embedding_space(
    conn: &mut Connection,
    fingerprint: &str,
    notes: &[(i64, Vec<f32>)],
    entities: &[(i64, Vec<f32>)],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM embeddings", [])?;
    tx.execute("DELETE FROM entity_embeddings", [])?;
    for (id, embedding) in notes {
        let encoded = serde_json::to_string(embedding)?;
        tx.execute(
            "INSERT INTO embeddings(note_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id, encoded],
        )?;
    }
    for (id, embedding) in entities {
        let encoded = serde_json::to_string(embedding)?;
        tx.execute(
            "INSERT INTO entity_embeddings(entity_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![id, encoded],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO app_metadata(key, value) VALUES ('embedding_space', ?1)",
        [fingerprint],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn all_note_embedding_inputs(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT n.id,
                COALESCE(n.title, '') || char(10)
                  || COALESCE(group_concat(DISTINCT c.name), '') || char(10) || n.raw_text || char(10)
                  || COALESCE(group_concat(e.data_json, char(10)), '')
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE n.trashed_at IS NULL
         GROUP BY n.id ORDER BY n.id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn all_entity_embedding_inputs(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, name || char(10) || type || char(10) || COALESCE(aliases, '[]')
         FROM entities ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Notes that don't yet have an embedding, with the text to embed for each.
pub fn notes_missing_embeddings(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT n.id,
                COALESCE(n.title, '') || char(10)
                  || COALESCE(group_concat(DISTINCT c.name), '') || char(10) || n.raw_text || char(10)
                  || COALESCE(group_concat(e.data_json, char(10)), '')
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE n.trashed_at IS NULL
           AND n.id NOT IN (SELECT note_id FROM embeddings)
         GROUP BY n.id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The text we embed for ONE note (category names + raw text + entry data) —
/// same composition as `notes_missing_embeddings`. Used to re-embed a note after
/// the chat agent edits one of its entries.
pub fn note_embed_text(conn: &Connection, note_id: i64) -> Result<String> {
    let text: String = conn.query_row(
        "SELECT COALESCE(n.title, '') || char(10)
                || COALESCE(group_concat(DISTINCT c.name), '') || char(10) || n.raw_text || char(10)
                || COALESCE(group_concat(e.data_json, char(10)), '')
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE n.id = ?1 AND n.trashed_at IS NULL
         GROUP BY n.id",
        [note_id],
        |r| r.get(0),
    )?;
    Ok(text)
}

/// (event_date, data) for every entry in a category, oldest first — feeds trends.
pub fn category_entries(conn: &Connection, category: &str) -> Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(e.event_date, date(e.created_at)), e.data_json
         FROM entries e
         JOIN categories c ON c.id = e.category_id
         JOIN notes n ON n.id = e.note_id
         WHERE c.name = ?1 AND n.trashed_at IS NULL
         ORDER BY COALESCE(e.event_date, date(e.created_at))",
    )?;
    let rows = stmt.query_map([category], |r| {
        let date: String = r.get(0)?;
        let data_str: String = r.get(1)?;
        Ok((date, serde_json::from_str(&data_str).unwrap_or(Value::Null)))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// (note_id, event_date, raw_text, data) for every entry — feeds the entity
/// backfill, which re-derives people from each entry's data and links them to
/// their note. Joined to `notes` so the backfill has snippet context.
pub fn all_entry_data(conn: &Connection) -> Result<Vec<(i64, String, String, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT e.note_id, COALESCE(e.event_date, date(e.created_at)), n.raw_text, e.data_json
         FROM entries e JOIN notes n ON n.id = e.note_id
         WHERE n.trashed_at IS NULL
         ORDER BY e.note_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let note_id: i64 = r.get(0)?;
        let date: String = r.get(1)?;
        let raw: String = r.get(2)?;
        let data_str: String = r.get(3)?;
        Ok((
            note_id,
            date,
            raw,
            serde_json::from_str(&data_str).unwrap_or(Value::Null),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The latest `schedule` entry's blocks for one day (`YYYY-MM-DD`). Mirrors the
/// "newest note wins" selection in Today.tsx (a re-captured schedule supersedes
/// earlier ones), so this feeds Google Calendar sync the same blocks the UI shows.
/// Returns an empty vec when there's no schedule for that day.
pub fn schedule_blocks_for(conn: &Connection, event_date: &str) -> Result<Vec<Value>> {
    let data_str: Option<String> = conn
        .query_row(
            "SELECT e.data_json
             FROM entries e
             JOIN categories c ON c.id = e.category_id
             JOIN notes n ON n.id = e.note_id
             WHERE c.name = 'schedule' AND e.event_date = ?1
               AND n.trashed_at IS NULL
             ORDER BY e.id DESC LIMIT 1",
            [event_date],
            |r| r.get(0),
        )
        .ok();
    let blocks = data_str
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("blocks").and_then(|b| b.as_array()).cloned())
        .unwrap_or_default();
    Ok(blocks)
}

/// One entry of a note, with its row id — feeds the chat agent's edit targeting
/// (the agent needs a stable `entry_id` to point at) and the edit preview.
#[derive(Serialize)]
pub struct EntryRow {
    pub entry_id: i64,
    pub category: Option<String>,
    pub event_date: String,
    pub data: Value,
}

pub fn note_entries(conn: &Connection, note_id: i64) -> Result<Vec<EntryRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, c.name, COALESCE(e.event_date, date(e.created_at)), e.data_json
         FROM entries e
         JOIN notes n ON n.id = e.note_id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE e.note_id = ?1 AND n.trashed_at IS NULL
         ORDER BY e.id",
    )?;
    let rows = stmt.query_map([note_id], |r| {
        let data_str: String = r.get(3)?;
        Ok(EntryRow {
            entry_id: r.get(0)?,
            category: r.get(1)?,
            event_date: r.get(2)?,
            data: serde_json::from_str(&data_str).unwrap_or(Value::Null),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Serialize)]
pub struct SearchHit {
    pub note_id: i64,
    pub distance: f32,
    pub category: Option<String>,
    pub event_date: String,
    pub raw_text: String,
    pub data: Option<Value>,
    pub origin: Option<String>, // 'capture' | 'brain:<vault>' — for source provenance
}

/// Most-recent entries by event date — complements semantic search so the Q&A
/// can answer "yesterday" / "today" / "last workout" style questions.
pub fn recent_entries(conn: &Connection, limit: i64) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, COALESCE(MAX(e.event_date), date(n.created_at)) AS d, pc.name, n.raw_text,
                json_group_array(json(e.data_json)) AS data, n.origin
         FROM notes n
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries e ON e.note_id = n.id
         WHERE (n.origin = 'capture' OR n.origin IS NULL)
           AND n.trashed_at IS NULL
         GROUP BY n.id
         ORDER BY d DESC, n.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        let data_str: Option<String> = r.get(4)?;
        Ok(SearchHit {
            note_id: r.get(0)?,
            distance: 0.0,
            event_date: r.get(1)?,
            category: r.get(2)?,
            raw_text: r.get(3)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            origin: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Capture notes with an entry dated `day` — backs day-scoped chat questions
/// ("what's on my schedule today?") so the answer can't drift to other dates.
/// The inner JOIN filters before grouping, so `data` carries only that day's
/// entries even when a multi-section note spans several dates.
pub fn notes_on_date(conn: &Connection, day: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, ?1, pc.name, n.raw_text,
                json_group_array(json(e.data_json)) AS data, n.origin
         FROM notes n
         LEFT JOIN categories pc ON pc.id = n.category_id
         JOIN entries e ON e.note_id = n.id
         WHERE (n.origin = 'capture' OR n.origin IS NULL)
           AND n.trashed_at IS NULL
           AND COALESCE(e.event_date, date(e.created_at)) = ?1
         GROUP BY n.id
         ORDER BY n.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![day, limit], |r| {
        let data_str: Option<String> = r.get(4)?;
        Ok(SearchHit {
            note_id: r.get(0)?,
            distance: 0.0,
            event_date: r.get(1)?,
            category: r.get(2)?,
            raw_text: r.get(3)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            origin: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Semantic search restricted to a single origin (e.g. 'brain:baro') — backs
/// vault-scoped chat. Pulls a wide KNN candidate pool, then filters to the
/// origin and ranks. (Fine at personal-KB scale; widen the pool if it grows.)
pub fn search_notes_scoped(
    conn: &Connection,
    qvec: &[f32],
    k: i64,
    origin: &str,
) -> Result<Vec<SearchHit>> {
    ensure_embedding_space_ready(conn)?;
    let json = serde_json::to_string(qvec)?;
    let mut stmt = conn.prepare(
        "SELECT e.note_id, e.distance, pc.name,
                COALESCE(MAX(en.event_date), date(n.created_at)), n.raw_text,
                json_group_array(json(en.data_json)) AS data, n.origin
         FROM (
            SELECT note_id, distance FROM embeddings
            WHERE embedding MATCH ?1 ORDER BY distance LIMIT 200
         ) e
         JOIN notes n ON n.id = e.note_id
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries en ON en.note_id = n.id
         WHERE n.origin = ?3 AND n.trashed_at IS NULL
         GROUP BY e.note_id
         ORDER BY e.distance
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![json, k, origin], |r| {
        let data_str: Option<String> = r.get(5)?;
        Ok(SearchHit {
            note_id: r.get(0)?,
            distance: r.get(1)?,
            category: r.get(2)?,
            event_date: r.get(3)?,
            raw_text: r.get(4)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            origin: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Semantic search across ALL brain notes (any vault) — backs proactive
/// surfacing ("related in your brain" as you capture).
pub fn search_notes_brain(conn: &Connection, qvec: &[f32], k: i64) -> Result<Vec<SearchHit>> {
    ensure_embedding_space_ready(conn)?;
    let json = serde_json::to_string(qvec)?;
    let mut stmt = conn.prepare(
        "SELECT e.note_id, e.distance, pc.name,
                COALESCE(MAX(en.event_date), date(n.created_at)), n.raw_text,
                json_group_array(json(en.data_json)) AS data, n.origin
         FROM (
            SELECT note_id, distance FROM embeddings
            WHERE embedding MATCH ?1 ORDER BY distance LIMIT 200
         ) e
         JOIN notes n ON n.id = e.note_id
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries en ON en.note_id = n.id
         WHERE n.origin LIKE 'brain:%' AND n.trashed_at IS NULL
         GROUP BY e.note_id
         ORDER BY e.distance
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![json, k], |r| {
        let data_str: Option<String> = r.get(5)?;
        Ok(SearchHit {
            note_id: r.get(0)?,
            distance: r.get(1)?,
            category: r.get(2)?,
            event_date: r.get(3)?,
            raw_text: r.get(4)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            origin: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every note that mentions an entity (brain home note + capture mentions),
/// newest first — backs entity/item-scoped asking. Combines the curated brain
/// profile with the live capture stream for that one item.
pub fn notes_for_entity(conn: &Connection, entity_id: i64, limit: i64) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, COALESCE(MAX(en.event_date), date(n.created_at)) AS d, pc.name, n.raw_text,
                json_group_array(json(en.data_json)) AS data, n.origin
         FROM entity_mentions m
         JOIN notes n ON n.id = m.note_id
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries en ON en.note_id = n.id
         WHERE m.entity_id = ?1 AND n.trashed_at IS NULL
         GROUP BY n.id
         ORDER BY d DESC, n.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![entity_id, limit], |r| {
        let data_str: Option<String> = r.get(4)?;
        Ok(SearchHit {
            note_id: r.get(0)?,
            distance: 0.0,
            category: r.get(2)?,
            event_date: r.get(1)?,
            raw_text: r.get(3)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            origin: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// An entity's display name + type, for labeling a scoped answer.
pub fn entity_name_type(conn: &Connection, entity_id: i64) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT name, type FROM entities WHERE id = ?1",
            [entity_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok())
}

pub fn search_notes(conn: &Connection, qvec: &[f32], k: i64) -> Result<Vec<SearchHit>> {
    ensure_embedding_space_ready(conn)?;
    let json = serde_json::to_string(qvec)?;
    // sqlite-vec applies LIMIT while finding nearest neighbors. Retained Trash
    // vectors must not consume the caller's visible result budget, so include
    // enough extra candidates to cover every currently trashed vector and
    // apply the requested limit again after filtering.
    let trashed_vectors: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM embeddings v JOIN notes n ON n.id = v.note_id
         WHERE n.trashed_at IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    let visible_limit = k.max(0);
    let candidate_limit = visible_limit.saturating_add(trashed_vectors);
    let mut stmt = conn.prepare(
        "SELECT e.note_id, e.distance, pc.name,
                COALESCE(MAX(en.event_date), date(n.created_at)), n.raw_text,
                json_group_array(json(en.data_json)) AS data, n.origin
         FROM (
            SELECT note_id, distance FROM embeddings
            WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2
         ) e
         JOIN notes n ON n.id = e.note_id
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries en ON en.note_id = n.id
         WHERE n.trashed_at IS NULL
         GROUP BY e.note_id
         ORDER BY e.distance
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![json, candidate_limit, visible_limit],
        |r| {
            let data_str: Option<String> = r.get(5)?;
            Ok(SearchHit {
                note_id: r.get(0)?,
                distance: r.get(1)?,
                category: r.get(2)?,
                event_date: r.get(3)?,
                raw_text: r.get(4)?,
                data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
                origin: r.get(6)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// Recaps (M5): period summaries grounded in entries within a date range.
// ---------------------------------------------------------------------------

/// (event_date, category, data) for entries whose day is within [start, end].
pub fn entries_between(
    conn: &Connection,
    start: &str,
    end: &str,
) -> Result<Vec<(String, String, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(e.event_date, date(e.created_at)) AS d, COALESCE(c.name,''), e.data_json
         FROM entries e
         JOIN notes n ON n.id = e.note_id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE d BETWEEN ?1 AND ?2 AND n.trashed_at IS NULL
         ORDER BY d",
    )?;
    let rows = stmt.query_map([start, end], |r| {
        let date: String = r.get(0)?;
        let cat: String = r.get(1)?;
        let data_str: String = r.get(2)?;
        Ok((
            date,
            cat,
            serde_json::from_str(&data_str).unwrap_or(Value::Null),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Serialize)]
pub struct RecapRow {
    pub id: i64,
    pub period: String,
    pub period_start: String,
    pub period_end: String,
    pub content: String,
    pub entry_count: i64,
    pub created_at: String,
}

/// Does a recap already exist for this exact period + range?
pub fn recap_exists(conn: &Connection, period: &str, start: &str, end: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recaps WHERE period = ?1 AND period_start = ?2 AND period_end = ?3",
        rusqlite::params![period, start, end],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Store a recap, replacing any existing one for the same period + range.
pub fn upsert_recap(
    conn: &Connection,
    period: &str,
    start: &str,
    end: &str,
    content: &str,
    entry_count: i64,
    now: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM recaps WHERE period = ?1 AND period_start = ?2 AND period_end = ?3",
        rusqlite::params![period, start, end],
    )?;
    conn.execute(
        "INSERT INTO recaps (period, period_start, period_end, content, entry_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![period, start, end, content, entry_count, now],
    )?;
    Ok(())
}

pub fn list_recaps(conn: &Connection, limit: i64) -> Result<Vec<RecapRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, period, period_start, period_end, content, entry_count, created_at
         FROM recaps ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok(RecapRow {
            id: r.get(0)?,
            period: r.get(1)?,
            period_start: r.get(2)?,
            period_end: r.get(3)?,
            content: r.get(4)?,
            entry_count: r.get(5)?,
            created_at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// Quick-capture queue: raw notes awaiting background categorization.
// ---------------------------------------------------------------------------

pub struct PendingCapture {
    pub id: i64,
    pub raw_text: String,
    pub source: String,
    pub image_path: Option<String>,
    pub event_date: Option<String>,
    pub filing_context: Option<String>,
}

/// Queue a raw capture for later categorization. Returns its id.
pub fn insert_pending(
    conn: &Connection,
    raw_text: &str,
    source: &str,
    image_path: Option<&str>,
    event_date: Option<&str>,
    filing_context: Option<&str>,
    now: &str,
) -> Result<i64> {
    let filing_context = filing_context.map(normalized_filing_context).transpose()?;
    conn.execute(
        "INSERT INTO pending_captures
           (raw_text, source, image_path, event_date, filing_context, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            raw_text,
            source,
            image_path,
            event_date,
            filing_context,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Pending captures still worth processing (under the retry cap), oldest first.
pub fn list_pending(conn: &Connection, max_attempts: i64) -> Result<Vec<PendingCapture>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_text, source, image_path, event_date,
                COALESCE(filing_context, '')
         FROM pending_captures WHERE attempts < ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([max_attempts], |row| {
            let filing_context = row.get::<_, String>(5)?;
            Ok(PendingCapture {
                id: row.get(0)?,
                raw_text: row.get(1)?,
                source: row.get(2)?,
                image_path: row.get(3)?,
                event_date: row.get(4)?,
                filing_context: (!filing_context.is_empty()).then_some(filing_context),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_pending(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM pending_captures WHERE id = ?1", [id])?;
    Ok(())
}

/// Record a processing failure and bump the attempt count (for retry + visibility).
pub fn set_pending_error(conn: &Connection, id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE pending_captures SET error = ?2, attempts = attempts + 1 WHERE id = ?1",
        rusqlite::params![id, error],
    )?;
    Ok(())
}

/// Count captures that have exhausted their retries (for a "needs attention" badge).
pub fn count_pending_errors(conn: &Connection, max_attempts: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM pending_captures WHERE attempts >= ?1",
        [max_attempts],
        |r| r.get(0),
    )?)
}

// ---------------------------------------------------------------------------
// Write path: save a reviewed proposal, creating/evolving the category.
// ---------------------------------------------------------------------------

/// One extracted observation: a category + its structured data.
pub struct EntryInput {
    pub category: String,
    pub description: String,
    pub data: Value,
}

pub struct SaveInput {
    pub raw_text: String,
    pub source: String,
    pub image_path: Option<String>,
    pub event_date: String, // canonical day (YYYY-MM-DD) the thing happened
    pub entries: Vec<EntryInput>,
}

/// Write one note plus its entries (one per category), creating/evolving each
/// category. The note's `category_id` points at the first ("primary") entry's
/// category for back-compat with single-category reads. Returns the note id.
fn save_note_in_transaction(
    tx: &rusqlite::Transaction,
    input: &SaveInput,
    now: &str,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO notes (raw_text, source, image_path, category_id, created_at)
         VALUES (?1, ?2, ?3, NULL, ?4)",
        rusqlite::params![
            input.raw_text.as_str(),
            input.source.as_str(),
            input.image_path.as_deref(),
            now
        ],
    )?;
    let note_id = tx.last_insert_rowid();

    let mut primary_cat: Option<i64> = None;
    for entry in &input.entries {
        let cat_id = upsert_category(&tx, &entry.category, &entry.description, &entry.data, now)?;
        primary_cat.get_or_insert(cat_id);
        tx.execute(
            "INSERT INTO entries (note_id, category_id, data_json, event_date, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                note_id,
                cat_id,
                entry.data.to_string(),
                input.event_date.as_str(),
                now
            ],
        )?;
    }
    if let Some(pc) = primary_cat {
        tx.execute(
            "UPDATE notes SET category_id = ?1 WHERE id = ?2",
            rusqlite::params![pc, note_id],
        )?;
    }

    Ok(note_id)
}

pub fn save_note(conn: &mut Connection, input: SaveInput, now: &str) -> Result<i64> {
    let tx = conn.transaction()?;
    let note_id = save_note_in_transaction(&tx, &input, now)?;

    tx.commit()?;
    Ok(note_id)
}

fn approved_daily_standup_folder(conn: &Connection, work_space_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM note_folders WHERE id = ?1
           UNION ALL
           SELECT child.id FROM note_folders child
           JOIN descendants parent ON child.parent_id = parent.id
         )
         SELECT folder.id
         FROM note_folders folder JOIN descendants d ON d.id = folder.id
         WHERE folder.auto_rule = 'daily_standup'
         ORDER BY folder.position, folder.id LIMIT 1",
        [work_space_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Refine a broad identity destination with the approved deterministic rule
/// beneath it. Today the only materialized rule is Daily Standup; keeping this
/// resolution in the DB layer makes new saves and historical meeting backfills
/// agree on the same final folder.
pub(crate) fn automatic_rule_destination_for_note(
    conn: &Connection,
    note_id: i64,
    requested_folder_id: i64,
) -> Result<i64> {
    if folder_filing_context(conn, requested_folder_id)? != "work" {
        return Ok(requested_folder_id);
    }
    let raw_text = conn.query_row(
        "SELECT raw_text FROM notes WHERE id = ?1",
        [note_id],
        |row| row.get::<_, String>(0),
    )?;
    let categories = conn
        .prepare(
            "SELECT c.name FROM entries e
             JOIN categories c ON c.id = e.category_id
             WHERE e.note_id = ?1 ORDER BY e.id",
        )?
        .query_map([note_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !matches_daily_standup(&raw_text, categories.iter().map(String::as_str)) {
        return Ok(requested_folder_id);
    }
    Ok(approved_daily_standup_folder(conn, requested_folder_id)?.unwrap_or(requested_folder_id))
}

/// Save a new capture and its reviewed placement as one transaction. Every
/// note first enters the selected context Inbox. A reviewed destination wins;
/// otherwise only the approved Work Daily Standup rule may move it deeper.
pub fn save_note_with_initial_filing_source(
    conn: &mut Connection,
    input: SaveInput,
    filing_context: &str,
    requested_folder_id: Option<i64>,
    requested_source: &str,
    requested_reason: Option<&str>,
    now: &str,
) -> Result<i64> {
    let filing_context = normalized_filing_context(filing_context)?;
    if !matches!(requested_source, "manual" | "rule") {
        return Err(anyhow::anyhow!(
            "reviewed filing source must be manual or rule"
        ));
    }
    let is_standup = matches_daily_standup(
        &input.raw_text,
        input.entries.iter().map(|entry| entry.category.as_str()),
    );
    let tx = conn.transaction()?;
    let context_id = context_space_id(&tx, &filing_context)?;
    if let Some(folder_id) = requested_folder_id {
        if !folder_is_in_context(&tx, folder_id, &filing_context)? {
            return Err(anyhow::anyhow!(
                "reviewed folder must be inside the selected context"
            ));
        }
    }
    let broad_requested_folder_id = requested_folder_id;
    let requested_folder_id =
        if requested_source == "rule" && filing_context == "work" && is_standup {
            requested_folder_id
                .map(|folder_id| {
                    approved_daily_standup_folder(&tx, folder_id)
                        .map(|approved| approved.unwrap_or(folder_id))
                })
                .transpose()?
        } else {
            requested_folder_id
        };

    let note_id = save_note_in_transaction(&tx, &input, now)?;
    let context_label = if filing_context == "work" {
        "Work"
    } else {
        "Personal"
    };
    filing_transition(
        &tx,
        note_id,
        Some(context_id),
        "context",
        &format!("Saved to {context_label} / Inbox because {context_label} was selected."),
        Some(&filing_context),
        None,
        now,
    )?;

    if let Some(folder_id) = requested_folder_id {
        let path = note_folder_path(&tx, folder_id)?;
        let reason = if broad_requested_folder_id != Some(folder_id) {
            format!(
                "Filed in {path} because it matched your approved Daily Standup rule within the meeting account destination."
            )
        } else {
            requested_reason.map(String::from).unwrap_or_else(|| {
                if requested_source == "rule" {
                    format!("Filed in {path} by an approved rule.")
                } else {
                    format!("Chosen before saving: {path}.")
                }
            })
        };
        filing_transition(
            &tx,
            note_id,
            Some(folder_id),
            requested_source,
            &reason,
            Some(&filing_context),
            None,
            now,
        )?;
    } else if filing_context == "work" && is_standup {
        if let Some(folder_id) = approved_daily_standup_folder(&tx, context_id)? {
            let path = note_folder_path(&tx, folder_id)?;
            filing_transition(
                &tx,
                note_id,
                Some(folder_id),
                "rule",
                &format!("Filed in {path} because it matched your approved Daily Standup rule."),
                Some(&filing_context),
                None,
                now,
            )?;
        }
    }

    tx.commit()?;
    Ok(note_id)
}

/// Reviewed capture wrapper: an explicitly selected destination is a sticky
/// manual decision. Meeting/calendar rules use the source-aware variant above.
pub fn save_note_with_initial_filing(
    conn: &mut Connection,
    input: SaveInput,
    filing_context: &str,
    requested_folder_id: Option<i64>,
    now: &str,
) -> Result<i64> {
    save_note_with_initial_filing_source(
        conn,
        input,
        filing_context,
        requested_folder_id,
        "manual",
        None,
        now,
    )
}

/// Upsert a category and evolve its schema additively from this entry's data.
/// Returns the category id.
fn upsert_category(
    tx: &rusqlite::Transaction,
    name: &str,
    description: &str,
    data: &Value,
    now: &str,
) -> Result<i64> {
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, schema_json FROM categories WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    let id = match existing {
        Some((id, schema_str)) => {
            let mut schema: Value =
                serde_json::from_str(&schema_str).unwrap_or_else(|_| default_schema());
            evolve_schema(&mut schema, data);
            tx.execute(
                "UPDATE categories
                 SET schema_json = ?1, entry_count = entry_count + 1,
                     description = CASE WHEN ?2 != '' THEN ?2 ELSE description END
                 WHERE id = ?3",
                rusqlite::params![schema.to_string(), description, id],
            )?;
            id
        }
        None => {
            let mut schema = default_schema();
            evolve_schema(&mut schema, data);
            tx.execute(
                "INSERT INTO categories (name, description, schema_json, entry_count, created_at)
                 VALUES (?1, ?2, ?3, 1, ?4)",
                rusqlite::params![name, description, schema.to_string(), now],
            )?;
            tx.last_insert_rowid()
        }
    };
    Ok(id)
}

/// Create a category by name if it doesn't already exist, with no entries yet
/// (entry_count 0). Returns the category id (existing or new). Used by the chat
/// agent's `create_category` action — unlike `upsert_category`, this is standalone
/// (not tied to saving a note) and never bumps a count.
pub fn create_category(conn: &Connection, name: &str, description: &str, now: &str) -> Result<i64> {
    if let Ok(id) = conn.query_row("SELECT id FROM categories WHERE name = ?1", [name], |r| {
        r.get::<_, i64>(0)
    }) {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO categories (name, description, schema_json, entry_count, created_at)
         VALUES (?1, ?2, ?3, 0, ?4)",
        rusqlite::params![name, description, default_schema().to_string(), now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Overwrite one entry's structured data in place — the only mutation path for
/// entries (writes are otherwise append-only). Returns the entry's `note_id` so
/// the caller can re-embed the note. Used by the chat agent's `edit_entry` action.
pub fn update_entry_data(conn: &Connection, entry_id: i64, data: &Value) -> Result<i64> {
    let (note_id, cur): (i64, String) = conn
        .query_row(
            "SELECT e.note_id, e.data_json
             FROM entries e JOIN notes n ON n.id = e.note_id
             WHERE e.id = ?1 AND n.trashed_at IS NULL",
            [entry_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| anyhow::anyhow!("entry {entry_id} not found"))?;
    // Shallow-merge the correction over the existing data: if the model returns a
    // partial object (only the changed key), untouched top-level fields survive.
    let merged = match (serde_json::from_str::<Value>(&cur).ok(), data) {
        (Some(Value::Object(mut base)), Value::Object(patch)) => {
            for (k, v) in patch {
                base.insert(k.clone(), v.clone());
            }
            Value::Object(base)
        }
        _ => data.clone(),
    };
    conn.execute(
        "UPDATE entries SET data_json = ?1 WHERE id = ?2",
        rusqlite::params![merged.to_string(), entry_id],
    )?;
    Ok(note_id)
}

fn default_schema() -> Value {
    json!({ "shape": {}, "field_freq": {} })
}

/// Grow a category's schema additively from a saved data object:
///  - `shape`: deep-merge so the structure is the union of everything seen
///  - `field_freq`: bump a count for every dot-path present in this entry
fn evolve_schema(schema: &mut Value, data: &Value) {
    let obj = schema.as_object_mut().expect("schema is object");

    let mut shape = obj.get("shape").cloned().unwrap_or_else(|| json!({}));
    merge_shape(&mut shape, data);
    obj.insert("shape".into(), shape);

    let mut freq: Map<String, Value> = obj
        .get("field_freq")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut paths = Vec::new();
    collect_paths(data, String::new(), &mut paths);
    for p in paths {
        let n = freq.get(&p).and_then(|v| v.as_i64()).unwrap_or(0) + 1;
        freq.insert(p, json!(n));
    }
    obj.insert("field_freq".into(), Value::Object(freq));
}

/// Deep-merge `incoming` into `target` to build a representative example shape.
/// Objects union their keys; arrays merge element shapes into the first slot;
/// scalars are kept as a sample value (first writer wins).
fn merge_shape(target: &mut Value, incoming: &Value) {
    match (target, incoming) {
        (Value::Object(t), Value::Object(i)) => {
            for (k, v) in i {
                merge_shape(t.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (Value::Array(t), Value::Array(i)) => {
            if t.is_empty() {
                t.push(Value::Null);
            }
            for item in i {
                merge_shape(&mut t[0], item);
            }
        }
        (t @ Value::Null, incoming) => {
            *t = incoming.clone();
        }
        // scalar already present — keep existing sample
        _ => {}
    }
}

/// Flatten an object's leaf paths into dot notation, collapsing array indices
/// (so `exercises[0].name` and `exercises[1].name` both count as `exercises.name`).
fn collect_paths(v: &Value, prefix: String, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collect_paths(val, p, out);
            }
        }
        Value::Array(a) => {
            for item in a {
                collect_paths(item, prefix.clone(), out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push(prefix);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Knowledge graph (Phase 2): entity storage. Resolution/normalization lives in
// entities.rs; this module is pure storage and takes already-normalized keys.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct EntityRow {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub mention_count: i64,
    pub last_seen: Option<String>,
    pub suggested_name: Option<String>,
}

/// All entities, most-mentioned first.
pub fn list_entities(conn: &Connection) -> Result<Vec<EntityRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, type, mention_count, last_seen, suggested_name
         FROM entities ORDER BY mention_count DESC, name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(EntityRow {
            id: r.get(0)?,
            name: r.get(1)?,
            r#type: r.get(2)?,
            mention_count: r.get(3)?,
            last_seen: r.get(4)?,
            suggested_name: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Serialize)]
pub struct GraphEdge {
    pub source: i64,
    pub target: i64,
    pub weight: i64,
}

/// Co-mention edges for the knowledge graph: two entities are linked when they
/// appear in the same note, weighted by how many distinct notes they share.
pub fn entity_edges(conn: &Connection) -> Result<Vec<GraphEdge>> {
    let mut stmt = conn.prepare(
        "SELECT a.entity_id, b.entity_id, COUNT(DISTINCT a.note_id) AS w
         FROM entity_mentions a
         JOIN entity_mentions b ON a.note_id = b.note_id AND a.entity_id < b.entity_id
         JOIN notes n ON n.id = a.note_id
         WHERE n.trashed_at IS NULL
         GROUP BY a.entity_id, b.entity_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(GraphEdge {
            source: r.get(0)?,
            target: r.get(1)?,
            weight: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Cross-type KNN over entity embeddings — question→entity matching for the
/// graph-aware chat. Returns (entity_id, L2 distance), nearest first.
pub fn nearest_entities_any(conn: &Connection, qvec: &[f32], k: i64) -> Result<Vec<(i64, f32)>> {
    ensure_embedding_space_ready(conn)?;
    let json = serde_json::to_string(qvec)?;
    let mut stmt = conn.prepare(
        "SELECT entity_id, distance FROM entity_embeddings
         WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![json, k], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// An entity's strongest co-mention neighbors: (name, type, shared-note count),
/// heaviest edge first — the "linked to" line in the chat's graph digest.
pub fn entity_neighbors(
    conn: &Connection,
    entity_id: i64,
    k: i64,
) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT ent.name, ent.type, COUNT(DISTINCT a.note_id) AS w
         FROM entity_mentions a
         JOIN entity_mentions b ON a.note_id = b.note_id AND b.entity_id != a.entity_id
         JOIN entities ent ON ent.id = b.entity_id
         JOIN notes n ON n.id = a.note_id
         WHERE a.entity_id = ?1 AND n.trashed_at IS NULL
         GROUP BY b.entity_id
         ORDER BY w DESC, ent.mention_count DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![entity_id, k], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every entity with its aliases: (id, name, type, aliases). The catalog is
/// small (a personal KB), so chat-question matching happens in memory.
pub fn entities_for_matching(conn: &Connection) -> Result<Vec<(i64, String, String, Vec<String>)>> {
    let mut stmt = conn.prepare("SELECT id, name, type, aliases FROM entities")?;
    let rows = stmt.query_map([], |r| {
        let aliases: String = r.get(3)?;
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            serde_json::from_str(&aliases).unwrap_or_default(),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Serialize)]
pub struct EntityMentionRow {
    pub note_id: i64,
    pub event_date: String,
    pub snippet: String,
}

/// Notes that mention an entity, newest first — for the graph's detail panel.
pub fn entity_detail(
    conn: &Connection,
    entity_id: i64,
    limit: i64,
) -> Result<Vec<EntityMentionRow>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT m.note_id, m.event_date, n.raw_text
         FROM entity_mentions m JOIN notes n ON n.id = m.note_id
         WHERE m.entity_id = ?1 AND n.trashed_at IS NULL
         ORDER BY m.event_date DESC, m.note_id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![entity_id, limit], |r| {
        let raw: String = r.get(2)?;
        Ok(EntityMentionRow {
            note_id: r.get(0)?,
            event_date: r.get(1)?,
            snippet: raw.replace('\n', " ").chars().take(140).collect(),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Find an existing entity by exact normalized key OR alias match, within a type.
pub fn entity_exact(conn: &Connection, norm: &str, etype: &str) -> Result<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM entities
             WHERE type = ?1 AND (norm = ?2 OR EXISTS (
                 SELECT 1 FROM json_each(entities.aliases) WHERE lower(json_each.value) = ?2
             ))
             LIMIT 1",
            rusqlite::params![etype, norm],
            |r| r.get::<_, i64>(0),
        )
        .ok();
    Ok(id)
}

/// Nearest existing entity of the same type by embedding distance (for merge
/// suggestions). Returns (entity_id, L2 distance) or None.
pub fn nearest_entity(conn: &Connection, qvec: &[f32], etype: &str) -> Result<Option<(i64, f32)>> {
    ensure_embedding_space_ready(conn)?;
    let json = serde_json::to_string(qvec)?;
    let hit = conn
        .query_row(
            "SELECT e.entity_id, e.distance FROM (
                 SELECT entity_id, distance FROM entity_embeddings
                 WHERE embedding MATCH ?1 ORDER BY distance LIMIT 5
             ) e
             JOIN entities ent ON ent.id = e.entity_id
             WHERE ent.type = ?2
             ORDER BY e.distance LIMIT 1",
            rusqlite::params![json, etype],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f32>(1)?)),
        )
        .ok();
    Ok(hit)
}

/// Create a new entity. `aliases` is a JSON array string. Returns its id.
pub fn create_entity(
    conn: &Connection,
    name: &str,
    norm: &str,
    etype: &str,
    aliases: &str,
    event_date: &str,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO entities (name, norm, type, aliases, first_seen, last_seen, mention_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, ?6)",
        rusqlite::params![name, norm, etype, aliases, event_date, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Whether this entity is already linked to this note. `add_mention` has no
/// dedupe (it always inserts + bumps the count), so the backfill checks this
/// first to stay idempotent across re-runs.
pub fn mention_exists(conn: &Connection, entity_id: i64, note_id: i64) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entity_mentions WHERE entity_id = ?1 AND note_id = ?2",
        rusqlite::params![entity_id, note_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Record a mention of an entity, bumping its count and extending its date span.
pub fn add_mention(
    conn: &Connection,
    entity_id: i64,
    note_id: i64,
    entry_id: Option<i64>,
    context: &str,
    event_date: &str,
    now: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO entity_mentions (entity_id, note_id, entry_id, context, event_date, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![entity_id, note_id, entry_id, context, event_date, now],
    )?;
    conn.execute(
        "UPDATE entities SET
             mention_count = mention_count + 1,
             first_seen = MIN(COALESCE(first_seen, ?2), ?2),
             last_seen  = MAX(COALESCE(last_seen, ?2), ?2)
         WHERE id = ?1",
        rusqlite::params![entity_id, event_date],
    )?;
    Ok(())
}

pub fn insert_entity_embedding(conn: &Connection, entity_id: i64, vec: &[f32]) -> Result<()> {
    let json = serde_json::to_string(vec)?;
    conn.execute(
        "INSERT OR REPLACE INTO entity_embeddings(entity_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![entity_id, json],
    )?;
    Ok(())
}

#[derive(Serialize)]
pub struct MergeSuggestionRow {
    pub a_id: i64,
    pub a_name: String,
    pub a_mentions: i64,
    pub b_id: i64,
    pub b_name: String,
    pub b_mentions: i64,
    pub etype: String,
    pub similarity: f32,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Same-type entity pairs whose embeddings sit at/above `threshold` cosine
/// similarity — likely duplicates ("Sara" / "Sarah") accumulated before the
/// capture-time resolver could catch them. Pairs the user dismissed are
/// excluded. Brute-force over in-memory vectors: a personal KB has hundreds of
/// entities, not millions, so O(n²) here beats plumbing a KNN index query per
/// entity.
pub fn suggest_merges(
    conn: &Connection,
    threshold: f32,
    limit: usize,
) -> Result<Vec<MergeSuggestionRow>> {
    use std::collections::{HashMap, HashSet};

    let mut meta_stmt = conn.prepare("SELECT id, name, type, mention_count FROM entities")?;
    let meta: HashMap<i64, (String, String, i64)> = meta_stmt
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // vec0 hands vectors back as little-endian f32 blobs.
    let mut emb_stmt = conn.prepare("SELECT entity_id, embedding FROM entity_embeddings")?;
    let embs: Vec<(i64, Vec<f32>)> = emb_stmt
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            let vec = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            Ok((id, vec))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let dismissed: HashSet<(i64, i64)> = conn
        .prepare("SELECT a, b FROM dismissed_merges")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut out: Vec<MergeSuggestionRow> = Vec::new();
    for i in 0..embs.len() {
        for j in (i + 1)..embs.len() {
            let (ia, va) = &embs[i];
            let (ib, vb) = &embs[j];
            let (Some(ma), Some(mb)) = (meta.get(ia), meta.get(ib)) else {
                continue; // embedding row orphaned from its entity
            };
            if ma.1 != mb.1 {
                continue; // different types never merge
            }
            if dismissed.contains(&((*ia).min(*ib), (*ia).max(*ib))) {
                continue;
            }
            let sim = cosine(va, vb);
            if sim >= threshold {
                out.push(MergeSuggestionRow {
                    a_id: *ia,
                    a_name: ma.0.clone(),
                    a_mentions: ma.2,
                    b_id: *ib,
                    b_name: mb.0.clone(),
                    b_mentions: mb.2,
                    etype: ma.1.clone(),
                    similarity: sim,
                });
            }
        }
    }
    out.sort_by(|x, y| y.similarity.total_cmp(&x.similarity));
    out.truncate(limit);
    Ok(out)
}

/// "Not the same" — remember the pair (order-normalized) so it stops being
/// suggested.
pub fn dismiss_merge(conn: &Connection, a: i64, b: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO dismissed_merges(a, b) VALUES (?1, ?2)",
        rusqlite::params![a.min(b), a.max(b)],
    )?;
    Ok(())
}

/// Merge `drop_id` into `keep_id`: reassign its mentions, union its aliases +
/// name into keep's aliases, recompute keep's count, then delete the dropped
/// entity (and its embedding). Derived co-mention edges recompute automatically.
pub fn merge_entities(conn: &mut Connection, keep_id: i64, drop_id: i64) -> Result<()> {
    if keep_id == drop_id {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE entity_mentions SET entity_id = ?1 WHERE entity_id = ?2",
        rusqlite::params![keep_id, drop_id],
    )?;
    // fold the dropped name + aliases into keep's alias list
    let dropped_name: String =
        tx.query_row("SELECT name FROM entities WHERE id = ?1", [drop_id], |r| {
            r.get(0)
        })?;
    let keep_aliases: String = tx.query_row(
        "SELECT aliases FROM entities WHERE id = ?1",
        [keep_id],
        |r| r.get(0),
    )?;
    let dropped_aliases: String = tx.query_row(
        "SELECT aliases FROM entities WHERE id = ?1",
        [drop_id],
        |r| r.get(0),
    )?;
    let mut set: Vec<String> = serde_json::from_str(&keep_aliases).unwrap_or_default();
    set.push(dropped_name);
    if let Ok(extra) = serde_json::from_str::<Vec<String>>(&dropped_aliases) {
        set.extend(extra);
    }
    set.sort();
    set.dedup();
    tx.execute(
        "UPDATE entities SET aliases = ?1,
             mention_count =
               (SELECT COUNT(*)
                FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                WHERE m.entity_id = ?2 AND n.trashed_at IS NULL),
             first_seen =
               (SELECT MIN(m.event_date)
                FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                WHERE m.entity_id = ?2 AND n.trashed_at IS NULL),
             last_seen =
               (SELECT MAX(m.event_date)
                FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                WHERE m.entity_id = ?2 AND n.trashed_at IS NULL)
         WHERE id = ?2",
        rusqlite::params![serde_json::to_string(&set)?, keep_id],
    )?;
    tx.execute(
        "DELETE FROM entity_embeddings WHERE entity_id = ?1",
        [drop_id],
    )?;
    tx.execute("DELETE FROM entities WHERE id = ?1", [drop_id])?;
    tx.commit()?;
    Ok(())
}

/// Set/refresh a person's relationship to the author. Latest non-empty wins;
/// an empty/blank value is ignored so a later note without a relationship never
/// clobbers a known one.
pub fn set_entity_relationship(conn: &Connection, id: i64, rel: &str) -> Result<()> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE entities SET relationship = ?2 WHERE id = ?1",
        rusqlite::params![id, rel],
    )?;
    Ok(())
}

/// Store (or clear) an AI-proposed display name for an entity. Suggestions are
/// advisory only — the entity's real name changes exclusively through
/// `rename_entity` on the user's explicit confirm.
pub fn set_suggested_name(conn: &Connection, id: i64, name: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE entities SET suggested_name = ?2 WHERE id = ?1",
        rusqlite::params![id, name],
    )?;
    Ok(())
}

/// Rename an entity: the old name joins the alias list (so future filings by
/// the old key — e.g. an attendee email — still resolve here), the suggestion
/// is consumed, and any alias equal to the new norm is dropped. The caller
/// resolves UNIQUE(norm, type) collisions (via `merge_entities`) first.
pub fn rename_entity(conn: &Connection, id: i64, new_name: &str, new_norm: &str) -> Result<()> {
    let (old_name, aliases): (String, String) = conn.query_row(
        "SELECT name, aliases FROM entities WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut set: Vec<String> = serde_json::from_str(&aliases).unwrap_or_default();
    set.push(old_name);
    set.retain(|a| a.to_lowercase() != new_norm);
    set.sort();
    set.dedup();
    conn.execute(
        "UPDATE entities SET name = ?2, norm = ?3, aliases = ?4, suggested_name = NULL
         WHERE id = ?1",
        rusqlite::params![id, new_name, new_norm, serde_json::to_string(&set)?],
    )?;
    Ok(())
}

/// Person entities still named by a raw email address and without a pending
/// suggestion — the pool the name-suggestion pass works through.
pub fn email_named_people(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, name FROM entities
         WHERE type = 'person' AND name LIKE '%@%' AND suggested_name IS NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// People view: person-typed entities + their dated mentions (curated facts).
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct PersonMention {
    pub date: String,
    pub text: String,
    pub note_id: i64,
}

#[derive(Serialize)]
pub struct PersonProfile {
    pub id: i64,
    pub name: String,
    pub relationship: Option<String>,
    pub mention_count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub aliases: Vec<String>,
    pub suggested_name: Option<String>,
    pub mentions: Vec<PersonMention>,
}

/// All mentions of an entity, most recent first (the curated `fact` lives in the
/// `context` column).
pub fn mentions_for(conn: &Connection, entity_id: i64) -> Result<Vec<PersonMention>> {
    let mut stmt = conn.prepare(
        "SELECT m.event_date, COALESCE(m.context, ''), m.note_id
         FROM entity_mentions m JOIN notes n ON n.id = m.note_id
         WHERE m.entity_id = ?1 AND n.trashed_at IS NULL
         ORDER BY m.event_date DESC, m.id DESC",
    )?;
    let rows = stmt.query_map([entity_id], |r| {
        Ok(PersonMention {
            date: r.get(0)?,
            text: r.get(1)?,
            note_id: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Serialize)]
pub struct EntityProfile {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub relationship: Option<String>,
    pub mention_count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub aliases: Vec<String>,
    pub mentions: Vec<PersonMention>,
}

/// Full profile for ANY entity (person/place/topic/…): header fields + every
/// dated mention, newest-first and uncapped. Unlike `mentions_for`, mention text
/// falls back to a note snippet when there's no curated `context` (non-person
/// entities rarely have one), so the per-entity page always shows something.
pub fn entity_profile(conn: &Connection, entity_id: i64) -> Result<EntityProfile> {
    let (name, etype, relationship, mention_count, first_seen, last_seen, aliases): (
        String,
        String,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        String,
    ) = conn.query_row(
        "SELECT name, type, relationship, mention_count, first_seen, last_seen, aliases
         FROM entities WHERE id = ?1",
        [entity_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        },
    )?;

    let mut stmt = conn.prepare(
        "SELECT m.event_date,
                COALESCE(NULLIF(m.context, ''), substr(replace(n.raw_text, char(10), ' '), 1, 140)),
                m.note_id
         FROM entity_mentions m JOIN notes n ON n.id = m.note_id
         WHERE m.entity_id = ?1 AND n.trashed_at IS NULL
         ORDER BY m.event_date DESC, m.id DESC",
    )?;
    let mentions = stmt
        .query_map([entity_id], |r| {
            Ok(PersonMention {
                date: r.get(0)?,
                text: r.get(1)?,
                note_id: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(EntityProfile {
        id: entity_id,
        name,
        r#type: etype,
        relationship,
        mention_count,
        first_seen,
        last_seen,
        aliases: serde_json::from_str(&aliases).unwrap_or_default(),
        mentions,
    })
}

/// Every `person` entity with its dated mentions — the People view's data.
/// Most-mentioned first, mirroring `list_entities`.
pub fn person_profiles(conn: &Connection) -> Result<Vec<PersonProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, relationship, mention_count, first_seen, last_seen, aliases, suggested_name
         FROM entities WHERE type = 'person'
         ORDER BY mention_count DESC, last_seen DESC, name",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let aliases: String = r.get(6)?;
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                aliases,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(rows.len());
    for (id, name, relationship, mention_count, first_seen, last_seen, aliases, suggested_name) in
        rows
    {
        out.push(PersonProfile {
            id,
            name,
            relationship,
            mention_count,
            first_seen,
            last_seen,
            aliases: serde_json::from_str(&aliases).unwrap_or_default(),
            suggested_name,
            mentions: mentions_for(conn, id)?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Brain sync: registered Obsidian vaults + storage for the notes they mirror.
// A brain note is a `notes` row with origin = 'brain:<vault>', category_id NULL,
// and NO entries (its content lives in raw_text; its links become mentions).
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct BrainVaultStatus {
    pub vault: String,
    pub root_path: String,
    pub direction: String,
    pub last_git_sha: Option<String>,
    pub last_synced_at: Option<String>,
    pub enabled: bool,
    pub note_count: i64,   // brain notes mirrored from this vault
    pub entity_count: i64, // distinct entities mentioned by this vault's notes
}

/// Registered vaults with live counts (notes mirrored + entities touched).
pub fn list_brain_vaults(conn: &Connection) -> Result<Vec<BrainVaultStatus>> {
    let mut stmt = conn.prepare(
        "SELECT vault, root_path, direction, last_git_sha, last_synced_at, enabled
         FROM brain_vaults ORDER BY vault",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(rows.len());
    for (vault, root_path, direction, last_git_sha, last_synced_at, enabled) in rows {
        let origin = format!("brain:{vault}");
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE origin = ?1 AND trashed_at IS NULL",
            [&origin],
            |r| r.get(0),
        )?;
        let entity_count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT m.entity_id) FROM entity_mentions m
             JOIN notes n ON n.id = m.note_id
             WHERE n.origin = ?1 AND n.trashed_at IS NULL",
            [&origin],
            |r| r.get(0),
        )?;
        out.push(BrainVaultStatus {
            vault,
            root_path,
            direction,
            last_git_sha,
            last_synced_at,
            enabled,
            note_count,
            entity_count,
        });
    }
    Ok(out)
}

/// Register a vault (or update its root/direction). Idempotent on `vault`.
pub fn upsert_brain_vault(
    conn: &Connection,
    vault: &str,
    root_path: &str,
    direction: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO brain_vaults (vault, root_path, direction) VALUES (?1, ?2, ?3)
         ON CONFLICT(vault) DO UPDATE SET root_path = ?2, direction = ?3",
        rusqlite::params![vault, root_path, direction],
    )?;
    Ok(())
}

pub fn remove_brain_vault(conn: &Connection, vault: &str) -> Result<()> {
    conn.execute("DELETE FROM brain_vaults WHERE vault = ?1", [vault])?;
    Ok(())
}

/// Record a completed sync (advances the git checkpoint for next-time diffing).
pub fn set_vault_synced(
    conn: &Connection,
    vault: &str,
    git_sha: Option<&str>,
    now: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE brain_vaults SET last_git_sha = ?2, last_synced_at = ?3 WHERE vault = ?1",
        rusqlite::params![vault, git_sha, now],
    )?;
    Ok(())
}

/// The stored content hash for a brain note, if we've mirrored this file before.
/// Lets a sync skip files whose content is unchanged.
pub fn brain_note_hash(
    conn: &Connection,
    origin: &str,
    source_path: &str,
) -> Result<Option<String>> {
    let h: Option<Option<String>> = conn
        .query_row(
            "SELECT content_hash FROM notes WHERE origin = ?1 AND source_path = ?2",
            rusqlite::params![origin, source_path],
            |r| r.get(0),
        )
        .ok();
    Ok(h.flatten())
}

/// Insert or refresh the `notes` row mirroring one brain file. Keyed by
/// (origin, source_path). Returns the note id.
pub fn upsert_brain_note(
    conn: &Connection,
    origin: &str,
    source_path: &str,
    raw_text: &str,
    hash: &str,
    now: &str,
) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM notes WHERE origin = ?1 AND source_path = ?2",
            rusqlite::params![origin, source_path],
            |r| r.get(0),
        )
        .ok();
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE notes SET raw_text = ?1, content_hash = ?2, synced_at = ?3 WHERE id = ?4",
                rusqlite::params![raw_text, hash, now, id],
            )?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO notes (raw_text, source, image_path, category_id, created_at, origin, source_path, content_hash, synced_at)
                 VALUES (?1, 'brain', NULL, NULL, ?2, ?3, ?4, ?5, ?2)",
                rusqlite::params![raw_text, now, origin, source_path, hash],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Drop all mentions for a note and fix the affected entities' counts — used
/// before re-inserting a changed brain note's links so removed `[[wikilinks]]`
/// don't leave stale edges.
pub fn clear_note_mentions(conn: &Connection, note_id: i64) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt =
            conn.prepare("SELECT DISTINCT entity_id FROM entity_mentions WHERE note_id = ?1")?;
        let v = stmt
            .query_map([note_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    conn.execute("DELETE FROM entity_mentions WHERE note_id = ?1", [note_id])?;
    for id in ids {
        conn.execute(
            "UPDATE entities SET
               mention_count =
                 (SELECT COUNT(*)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = ?1 AND n.trashed_at IS NULL),
               first_seen =
                 (SELECT MIN(m.event_date)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = ?1 AND n.trashed_at IS NULL),
               last_seen =
                 (SELECT MAX(m.event_date)
                  FROM entity_mentions m JOIN notes n ON n.id = m.note_id
                  WHERE m.entity_id = ?1 AND n.trashed_at IS NULL)
             WHERE id = ?1",
            [id],
        )?;
    }
    Ok(())
}

/// Mark which brain note DEFINES an entity (first definition wins; a later note
/// or a capture never steals an entity's home).
pub fn set_entity_home(conn: &Connection, entity_id: i64, note_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE entities SET home_note_id = ?2 WHERE id = ?1 AND home_note_id IS NULL",
        rusqlite::params![entity_id, note_id],
    )?;
    Ok(())
}

/// A node in the Work graph — an entity touched by a brain vault, tagged with the
/// vault that defines it (its home note's vault).
#[derive(Serialize)]
pub struct WorkNode {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub mention_count: i64,
    pub vault: String, // defining vault ("" if the entity has no brain home)
}

/// A brain note that write-back would touch: its defining entity has ≥1 capture
/// mention, which we mirror into the note's managed region.
pub struct WriteTarget {
    pub entity_id: i64,
    pub entity_name: String,
    pub home_note_id: i64,
    pub source_path: String, // vault-relative path of the home note
    pub origin: String,      // "brain:<vault>"
    pub captures: Vec<(String, String)>, // (event_date, fact/snippet), newest first
}

/// Entities defined by a brain note (have a `home_note_id`) that also carry
/// capture-origin mentions — i.e. the daily-capture stream noted should write
/// back into each one's brain note. Optionally scoped to one vault.
pub fn write_targets(conn: &Connection, vault: Option<&str>) -> Result<Vec<WriteTarget>> {
    let bases = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, e.home_note_id, n.source_path, n.origin
             FROM entities e JOIN notes n ON n.id = e.home_note_id
             WHERE n.origin LIKE 'brain:%' AND (?1 IS NULL OR n.origin = 'brain:' || ?1)
               AND n.trashed_at IS NULL
               AND n.source_path IS NOT NULL
               AND EXISTS (
                 SELECT 1 FROM entity_mentions m JOIN notes cn ON cn.id = m.note_id
                 WHERE m.entity_id = e.id
                   AND (cn.origin = 'capture' OR cn.origin IS NULL)
                   AND cn.trashed_at IS NULL
               )
             ORDER BY e.name",
        )?;
        let v = stmt
            .query_map(rusqlite::params![vault], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut out = Vec::with_capacity(bases.len());
    for (entity_id, entity_name, home_note_id, source_path, origin) in bases {
        let captures = {
            let mut stmt = conn.prepare(
                "SELECT m.event_date,
                        COALESCE(NULLIF(m.context, ''), substr(replace(cn.raw_text, char(10), ' '), 1, 160))
                 FROM entity_mentions m JOIN notes cn ON cn.id = m.note_id
                 WHERE m.entity_id = ?1
                   AND (cn.origin = 'capture' OR cn.origin IS NULL)
                   AND cn.trashed_at IS NULL
                 ORDER BY m.event_date DESC, m.id DESC",
            )?;
            let v = stmt
                .query_map([entity_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        out.push(WriteTarget {
            entity_id,
            entity_name,
            home_note_id,
            source_path,
            origin,
            captures,
        });
    }
    Ok(out)
}

/// A capture-derived person to export into the personal vault.
pub struct PersonExport {
    pub id: i64,
    pub name: String,
    pub relationship: Option<String>,
    pub mentions: Vec<(String, String)>, // (event_date, fact/snippet), newest first
}

/// People worth exporting to the personal vault: person entities NOT already
/// defined by a brain note (so work contacts owned by a work vault stay there),
/// seen at least `min_mentions` times (filters one-off noise).
pub fn people_for_export(conn: &Connection, min_mentions: i64) -> Result<Vec<PersonExport>> {
    let ids = {
        let mut stmt = conn.prepare(
            "SELECT id, name, relationship FROM entities
             WHERE type = 'person' AND home_note_id IS NULL AND mention_count >= ?1
             ORDER BY mention_count DESC, name",
        )?;
        let v = stmt
            .query_map([min_mentions], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut out = Vec::with_capacity(ids.len());
    for (id, name, relationship) in ids {
        let mentions = mentions_for(conn, id)?
            .into_iter()
            .map(|m| (m.date, m.text))
            .collect();
        out.push(PersonExport {
            id,
            name,
            relationship,
            mentions,
        });
    }
    Ok(out)
}

/// After a write-back rewrites a brain file, sync the mirror row to the new
/// content + hash so the next import sees it unchanged (echo suppression).
pub fn update_brain_note_content(
    conn: &Connection,
    note_id: i64,
    raw_text: &str,
    hash: &str,
    now: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE notes SET raw_text = ?1, content_hash = ?2, synced_at = ?3 WHERE id = ?4",
        rusqlite::params![raw_text, hash, now, note_id],
    )?;
    Ok(())
}

/// The Work-tab graph: a lens over the same KG, scoped to entities a brain vault
/// touches (optionally one vault) plus the co-mention edges among them that come
/// from brain notes. Capture-only entities are excluded; people who appear in
/// both a brain and daily captures are included (they carry a brain mention).
pub fn work_graph(
    conn: &Connection,
    vault: Option<&str>,
) -> Result<(Vec<WorkNode>, Vec<GraphEdge>)> {
    let nodes = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT e.id, e.name, e.type, e.mention_count,
                    COALESCE((SELECT substr(n2.origin, 7) FROM notes n2 WHERE n2.id = e.home_note_id), '') AS vault
             FROM entities e
             JOIN entity_mentions m ON m.entity_id = e.id
             JOIN notes n ON n.id = m.note_id
             WHERE n.origin LIKE 'brain:%'
               AND n.trashed_at IS NULL
               AND (?1 IS NULL OR n.origin = 'brain:' || ?1)
             ORDER BY e.mention_count DESC, e.name",
        )?;
        let v = stmt
            .query_map(rusqlite::params![vault], |r| {
                Ok(WorkNode {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    r#type: r.get(2)?,
                    mention_count: r.get(3)?,
                    vault: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let edges = {
        let mut stmt = conn.prepare(
            "SELECT a.entity_id, b.entity_id, COUNT(DISTINCT a.note_id) AS w
             FROM entity_mentions a
             JOIN entity_mentions b ON a.note_id = b.note_id AND a.entity_id < b.entity_id
             JOIN notes n ON n.id = a.note_id
             WHERE n.origin LIKE 'brain:%'
               AND n.trashed_at IS NULL
               AND (?1 IS NULL OR n.origin = 'brain:' || ?1)
             GROUP BY a.entity_id, b.entity_id",
        )?;
        let v = stmt
            .query_map(rusqlite::params![vault], |r| {
                Ok(GraphEdge {
                    source: r.get(0)?,
                    target: r.get(1)?,
                    weight: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    Ok((nodes, edges))
}
