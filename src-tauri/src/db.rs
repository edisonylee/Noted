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
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  raw_text    TEXT NOT NULL,
  source      TEXT NOT NULL DEFAULT 'text',
  image_path  TEXT,
  category_id INTEGER REFERENCES categories(id),
  created_at  TEXT NOT NULL
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
            let category = v.get("category").and_then(|c| c.as_str()).map(String::from);
            let data = v.get("data").cloned().unwrap_or(Value::Null);
            if category.is_none() && data.is_null() {
                None
            } else {
                Some(NoteEntry { category, data })
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
                json_group_array(json_object('category', c.name, 'data', json(e.data_json))) AS entries,
                n.created_at
         FROM notes n
         LEFT JOIN entries e ON e.note_id = n.id
         LEFT JOIN categories c ON c.id = e.category_id
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

#[derive(Serialize)]
pub struct SearchHit {
    pub note_id: i64,
    pub distance: f32,
    pub category: Option<String>,
    pub event_date: String,
    pub raw_text: String,
    pub data: Option<Value>,
}

/// Most-recent entries by event date — complements semantic search so the Q&A
/// can answer "yesterday" / "today" / "last workout" style questions.
pub fn recent_entries(conn: &Connection, limit: i64) -> Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, COALESCE(MAX(e.event_date), date(n.created_at)) AS d, pc.name, n.raw_text,
                json_group_array(json(e.data_json)) AS data
         FROM notes n
         LEFT JOIN categories pc ON pc.id = n.category_id
         LEFT JOIN entries e ON e.note_id = n.id
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
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn search_notes(conn: &Connection, qvec: &[f32], k: i64) -> Result<Vec<SearchHit>> {
    let json = serde_json::to_string(qvec)?;
    let mut stmt = conn.prepare(
        "SELECT e.note_id, e.distance, pc.name,
                COALESCE(MAX(en.event_date), date(n.created_at)), n.raw_text,
                json_group_array(json(en.data_json)) AS data
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
