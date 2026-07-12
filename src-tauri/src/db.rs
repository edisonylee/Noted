// SQLite access layer for noted.
// Holds the single connection behind a Mutex in Tauri state. Also owns the
// "emergent schema" logic: each category grows an additive shape + field
// frequency map from the notes the user actually saves.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{ffi::sqlite3_auto_extension, Connection};
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
  synced_at    TEXT
);

CREATE TABLE IF NOT EXISTS entries (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  note_id     INTEGER NOT NULL REFERENCES notes(id),
  category_id INTEGER NOT NULL REFERENCES categories(id),
  data_json   TEXT NOT NULL,
  event_date  TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS embeddings USING vec0(
  note_id   INTEGER PRIMARY KEY,
  embedding FLOAT[768]
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
-- deliberately NOT in entries.data_json). The AI summary is ALSO filed as a
-- regular note under the 'meetings' category so search/embeddings/KG see it;
-- note_id links back to that note.
CREATE TABLE IF NOT EXISTS meetings (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  title          TEXT NOT NULL,
  event_id       TEXT,                    -- gcal event id when calendar-matched
  event_json     TEXT,                    -- event snapshot: attendees, meet_link, times
  started_at     TEXT,
  ended_at       TEXT,
  status         TEXT NOT NULL DEFAULT 'recording', -- recording|summarizing|done|failed
  raw_notes      TEXT NOT NULL DEFAULT '',-- typed during the meeting; always preserved
  audio_me_path  TEXT,                    -- retained WAVs (verifiability); NULL if off
  audio_them_path TEXT,
  note_id        INTEGER REFERENCES notes(id),
  created_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meeting_segments (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  channel    TEXT NOT NULL,               -- 'me' (mic) | 'them' (system audio)
  t0_ms      INTEGER NOT NULL,
  t1_ms      INTEGER NOT NULL,
  text       TEXT NOT NULL,
  speaker    TEXT                         -- NULL = channel default; diarization fills later
);
CREATE INDEX IF NOT EXISTS idx_segment_meeting ON meeting_segments(meeting_id, t0_ms);

-- One row per generated summary tab (PLAUD-style multidimensional summaries:
-- regenerating with another template adds a tab, never overwrites).
CREATE TABLE IF NOT EXISTS meeting_summaries (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id INTEGER NOT NULL REFERENCES meetings(id),
  template   TEXT NOT NULL,
  content_md TEXT NOT NULL,
  created_at TEXT NOT NULL
);

-- A template is a name + one free-text prompt (PLAUD's model): the prompt
-- describes the sections to extract. builtin rows are re-seeded on startup.
CREATE TABLE IF NOT EXISTS meeting_templates (
  name    TEXT PRIMARY KEY,
  prompt  TEXT NOT NULL,
  builtin INTEGER NOT NULL DEFAULT 0
);
"#;

/// Register sqlite-vec as an auto extension (process-wide, must happen before
/// the connection is opened) and create the schema. Called once at startup.
pub fn init(db_path: &Path) -> Result<Connection> {
    // SAFETY: standard sqlite-vec registration. Must run before Connection::open.
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    // Migrations for DBs created before a column existed (additive only).
    ensure_column(&conn, "entries", "event_date", "TEXT")?;
    ensure_column(&conn, "entities", "relationship", "TEXT")?;
    // Brain-sync columns (additive; legacy rows read as capture-origin via COALESCE).
    ensure_column(&conn, "notes", "origin", "TEXT")?;
    ensure_column(&conn, "notes", "source_path", "TEXT")?;
    ensure_column(&conn, "notes", "content_hash", "TEXT")?;
    ensure_column(&conn, "notes", "synced_at", "TEXT")?;
    ensure_column(&conn, "entities", "home_note_id", "INTEGER")?;
    // Note: the reserved catch-all "misc" is not pre-seeded — the classifier is
    // told about it by name in the prompt, and it's created on first real use
    // (so an unused misc never clutters the catalog/UI).
    Ok(conn)
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
            if c.description.is_empty() { "(no description)" } else { &c.description },
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
    pub raw_text: String,
    pub source: String,
    pub entries: Vec<NoteEntry>,
    pub event_date: String,
    pub created_at: String,
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

pub fn list_notes(conn: &Connection) -> Result<Vec<NoteRow>> {
    // One row per note; its entries (category + data) aggregated into a JSON
    // array. Ordered by the day the thing happened (latest entry event_date),
    // falling back to the save day for any legacy rows without one.
    let mut stmt = conn.prepare(
        "SELECT n.id, n.raw_text, n.source,
                COALESCE(MAX(e.event_date), date(n.created_at)) AS event_date,
                json_group_array(json_object('id', e.id, 'category', c.name, 'data', json(e.data_json))) AS entries,
                n.created_at
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE (n.origin = 'capture' OR n.origin IS NULL)
         GROUP BY n.id
         ORDER BY event_date DESC, n.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let entries_str: String = r.get(4)?;
        Ok(NoteRow {
            id: r.get(0)?,
            raw_text: r.get(1)?,
            source: r.get(2)?,
            event_date: r.get(3)?,
            entries: parse_note_entries(&entries_str),
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

/// Notes that don't yet have an embedding, with the text to embed for each.
pub fn notes_missing_embeddings(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT n.id,
                COALESCE(group_concat(DISTINCT c.name), '') || char(10) || n.raw_text || char(10)
                  || COALESCE(group_concat(e.data_json, char(10)), '')
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE n.id NOT IN (SELECT note_id FROM embeddings)
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
        "SELECT COALESCE(group_concat(DISTINCT c.name), '') || char(10) || n.raw_text || char(10)
                || COALESCE(group_concat(e.data_json, char(10)), '')
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
         WHERE n.id = ?1
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
         FROM entries e JOIN categories c ON c.id = e.category_id
         WHERE c.name = ?1
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
         ORDER BY e.note_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let note_id: i64 = r.get(0)?;
        let date: String = r.get(1)?;
        let raw: String = r.get(2)?;
        let data_str: String = r.get(3)?;
        Ok((note_id, date, raw, serde_json::from_str(&data_str).unwrap_or(Value::Null)))
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
             FROM entries e JOIN categories c ON c.id = e.category_id
             WHERE c.name = 'schedule' AND e.event_date = ?1
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
         FROM entries e LEFT JOIN categories c ON c.id = e.category_id
         WHERE e.note_id = ?1
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

/// Semantic search restricted to a single origin (e.g. 'brain:baro') — backs
/// vault-scoped chat. Pulls a wide KNN candidate pool, then filters to the
/// origin and ranks. (Fine at personal-KB scale; widen the pool if it grows.)
pub fn search_notes_scoped(conn: &Connection, qvec: &[f32], k: i64, origin: &str) -> Result<Vec<SearchHit>> {
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
         WHERE n.origin = ?3
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
         WHERE n.origin LIKE 'brain:%'
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
         WHERE m.entity_id = ?1
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
    let json = serde_json::to_string(qvec)?;
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
         GROUP BY e.note_id
         ORDER BY e.distance",
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

// ---------------------------------------------------------------------------
// Recaps (M5): period summaries grounded in entries within a date range.
// ---------------------------------------------------------------------------

/// (event_date, category, data) for entries whose day is within [start, end].
pub fn entries_between(conn: &Connection, start: &str, end: &str) -> Result<Vec<(String, String, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(e.event_date, date(e.created_at)) AS d, COALESCE(c.name,''), e.data_json
         FROM entries e LEFT JOIN categories c ON c.id = e.category_id
         WHERE d BETWEEN ?1 AND ?2
         ORDER BY d",
    )?;
    let rows = stmt.query_map([start, end], |r| {
        let date: String = r.get(0)?;
        let cat: String = r.get(1)?;
        let data_str: String = r.get(2)?;
        Ok((date, cat, serde_json::from_str(&data_str).unwrap_or(Value::Null)))
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
}

/// Queue a raw capture for later categorization. Returns its id.
pub fn insert_pending(
    conn: &Connection,
    raw_text: &str,
    source: &str,
    image_path: Option<&str>,
    event_date: Option<&str>,
    now: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO pending_captures (raw_text, source, image_path, event_date, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![raw_text, source, image_path, event_date, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Pending captures still worth processing (under the retry cap), oldest first.
pub fn list_pending(conn: &Connection, max_attempts: i64) -> Result<Vec<PendingCapture>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_text, source, image_path, event_date
         FROM pending_captures WHERE attempts < ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map([max_attempts], |r| {
            Ok(PendingCapture {
                id: r.get(0)?,
                raw_text: r.get(1)?,
                source: r.get(2)?,
                image_path: r.get(3)?,
                event_date: r.get(4)?,
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
pub fn save_note(conn: &mut Connection, input: SaveInput, now: &str) -> Result<i64> {
    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO notes (raw_text, source, image_path, category_id, created_at)
         VALUES (?1, ?2, ?3, NULL, ?4)",
        rusqlite::params![input.raw_text, input.source, input.image_path, now],
    )?;
    let note_id = tx.last_insert_rowid();

    let mut primary_cat: Option<i64> = None;
    for entry in &input.entries {
        let cat_id = upsert_category(&tx, &entry.category, &entry.description, &entry.data, now)?;
        primary_cat.get_or_insert(cat_id);
        tx.execute(
            "INSERT INTO entries (note_id, category_id, data_json, event_date, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![note_id, cat_id, entry.data.to_string(), input.event_date, now],
        )?;
    }
    if let Some(pc) = primary_cat {
        tx.execute(
            "UPDATE notes SET category_id = ?1 WHERE id = ?2",
            rusqlite::params![pc, note_id],
        )?;
    }

    tx.commit()?;
    Ok(note_id)
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
    if let Ok(id) = conn.query_row(
        "SELECT id FROM categories WHERE name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    ) {
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
            "SELECT note_id, data_json FROM entries WHERE id = ?1",
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
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
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
}

/// All entities, most-mentioned first.
pub fn list_entities(conn: &Connection) -> Result<Vec<EntityRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, type, mention_count FROM entities ORDER BY mention_count DESC, name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(EntityRow {
            id: r.get(0)?,
            name: r.get(1)?,
            r#type: r.get(2)?,
            mention_count: r.get(3)?,
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

#[derive(Serialize)]
pub struct EntityMentionRow {
    pub note_id: i64,
    pub event_date: String,
    pub snippet: String,
}

/// Notes that mention an entity, newest first — for the graph's detail panel.
pub fn entity_detail(conn: &Connection, entity_id: i64, limit: i64) -> Result<Vec<EntityMentionRow>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT m.note_id, m.event_date, n.raw_text
         FROM entity_mentions m JOIN notes n ON n.id = m.note_id
         WHERE m.entity_id = ?1
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
    let dropped_name: String = tx.query_row("SELECT name FROM entities WHERE id = ?1", [drop_id], |r| r.get(0))?;
    let keep_aliases: String = tx.query_row("SELECT aliases FROM entities WHERE id = ?1", [keep_id], |r| r.get(0))?;
    let dropped_aliases: String = tx.query_row("SELECT aliases FROM entities WHERE id = ?1", [drop_id], |r| r.get(0))?;
    let mut set: Vec<String> = serde_json::from_str(&keep_aliases).unwrap_or_default();
    set.push(dropped_name);
    if let Ok(extra) = serde_json::from_str::<Vec<String>>(&dropped_aliases) {
        set.extend(extra);
    }
    set.sort();
    set.dedup();
    tx.execute(
        "UPDATE entities SET aliases = ?1,
             mention_count = (SELECT COUNT(*) FROM entity_mentions WHERE entity_id = ?2)
         WHERE id = ?2",
        rusqlite::params![serde_json::to_string(&set)?, keep_id],
    )?;
    tx.execute("DELETE FROM entity_embeddings WHERE entity_id = ?1", [drop_id])?;
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
    pub mentions: Vec<PersonMention>,
}

/// All mentions of an entity, most recent first (the curated `fact` lives in the
/// `context` column).
pub fn mentions_for(conn: &Connection, entity_id: i64) -> Result<Vec<PersonMention>> {
    let mut stmt = conn.prepare(
        "SELECT event_date, COALESCE(context, ''), note_id
         FROM entity_mentions WHERE entity_id = ?1
         ORDER BY event_date DESC, id DESC",
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
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
    )?;

    let mut stmt = conn.prepare(
        "SELECT m.event_date,
                COALESCE(NULLIF(m.context, ''), substr(replace(n.raw_text, char(10), ' '), 1, 140)),
                m.note_id
         FROM entity_mentions m JOIN notes n ON n.id = m.note_id
         WHERE m.entity_id = ?1
         ORDER BY m.event_date DESC, m.id DESC",
    )?;
    let mentions = stmt
        .query_map([entity_id], |r| {
            Ok(PersonMention { date: r.get(0)?, text: r.get(1)?, note_id: r.get(2)? })
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
        "SELECT id, name, relationship, mention_count, first_seen, last_seen, aliases
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
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(rows.len());
    for (id, name, relationship, mention_count, first_seen, last_seen, aliases) in rows {
        out.push(PersonProfile {
            id,
            name,
            relationship,
            mention_count,
            first_seen,
            last_seen,
            aliases: serde_json::from_str(&aliases).unwrap_or_default(),
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
        let note_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM notes WHERE origin = ?1", [&origin], |r| r.get(0))?;
        let entity_count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT m.entity_id) FROM entity_mentions m
             JOIN notes n ON n.id = m.note_id WHERE n.origin = ?1",
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
pub fn upsert_brain_vault(conn: &Connection, vault: &str, root_path: &str, direction: &str) -> Result<()> {
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
pub fn set_vault_synced(conn: &Connection, vault: &str, git_sha: Option<&str>, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE brain_vaults SET last_git_sha = ?2, last_synced_at = ?3 WHERE vault = ?1",
        rusqlite::params![vault, git_sha, now],
    )?;
    Ok(())
}

/// The stored content hash for a brain note, if we've mirrored this file before.
/// Lets a sync skip files whose content is unchanged.
pub fn brain_note_hash(conn: &Connection, origin: &str, source_path: &str) -> Result<Option<String>> {
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
        let mut stmt = conn.prepare("SELECT DISTINCT entity_id FROM entity_mentions WHERE note_id = ?1")?;
        let v = stmt
            .query_map([note_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    conn.execute("DELETE FROM entity_mentions WHERE note_id = ?1", [note_id])?;
    for id in ids {
        conn.execute(
            "UPDATE entities SET mention_count = (SELECT COUNT(*) FROM entity_mentions WHERE entity_id = ?1) WHERE id = ?1",
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
               AND n.source_path IS NOT NULL
               AND EXISTS (
                 SELECT 1 FROM entity_mentions m JOIN notes cn ON cn.id = m.note_id
                 WHERE m.entity_id = e.id AND (cn.origin = 'capture' OR cn.origin IS NULL)
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
                 WHERE m.entity_id = ?1 AND (cn.origin = 'capture' OR cn.origin IS NULL)
                 ORDER BY m.event_date DESC, m.id DESC",
            )?;
            let v = stmt
                .query_map([entity_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        out.push(WriteTarget { entity_id, entity_name, home_note_id, source_path, origin, captures });
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
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut out = Vec::with_capacity(ids.len());
    for (id, name, relationship) in ids {
        let mentions = mentions_for(conn, id)?.into_iter().map(|m| (m.date, m.text)).collect();
        out.push(PersonExport { id, name, relationship, mentions });
    }
    Ok(out)
}

/// After a write-back rewrites a brain file, sync the mirror row to the new
/// content + hash so the next import sees it unchanged (echo suppression).
pub fn update_brain_note_content(conn: &Connection, note_id: i64, raw_text: &str, hash: &str, now: &str) -> Result<()> {
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
pub fn work_graph(conn: &Connection, vault: Option<&str>) -> Result<(Vec<WorkNode>, Vec<GraphEdge>)> {
    let nodes = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT e.id, e.name, e.type, e.mention_count,
                    COALESCE((SELECT substr(n2.origin, 7) FROM notes n2 WHERE n2.id = e.home_note_id), '') AS vault
             FROM entities e
             JOIN entity_mentions m ON m.entity_id = e.id
             JOIN notes n ON n.id = m.note_id
             WHERE n.origin LIKE 'brain:%'
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
               AND (?1 IS NULL OR n.origin = 'brain:' || ?1)
             GROUP BY a.entity_id, b.entity_id",
        )?;
        let v = stmt
            .query_map(rusqlite::params![vault], |r| {
                Ok(GraphEdge { source: r.get(0)?, target: r.get(1)?, weight: r.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    Ok((nodes, edges))
}
