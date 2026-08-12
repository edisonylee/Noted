use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tauri_app_lib::db;

fn temp_db(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "noted_phase0_{label}_{}_{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn remove_sqlite_files(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("inspect table");
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .expect("read columns")
        .filter_map(Result::ok)
        .any(|name| name == column);
    found
}

#[test]
fn earliest_repository_schema_converges_without_losing_canonical_rows() {
    let path = temp_db("legacy_baseline");
    remove_sqlite_files(&path);
    let legacy = Connection::open(&path).expect("create synthetic legacy database");
    legacy
        .execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE categories (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               name TEXT UNIQUE NOT NULL,
               description TEXT NOT NULL DEFAULT '',
               schema_json TEXT NOT NULL DEFAULT '{\"shape\":{},\"field_freq\":{}}',
               entry_count INTEGER NOT NULL DEFAULT 0,
               created_at TEXT NOT NULL
             );
             CREATE TABLE notes (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               raw_text TEXT NOT NULL,
               source TEXT NOT NULL DEFAULT 'text',
               image_path TEXT,
               category_id INTEGER REFERENCES categories(id),
               created_at TEXT NOT NULL
             );
             CREATE TABLE entries (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               note_id INTEGER NOT NULL REFERENCES notes(id),
               category_id INTEGER NOT NULL REFERENCES categories(id),
               data_json TEXT NOT NULL,
               event_date TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE recaps (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               period TEXT NOT NULL,
               period_start TEXT NOT NULL,
               period_end TEXT NOT NULL,
               content TEXT NOT NULL,
               entry_count INTEGER NOT NULL,
               created_at TEXT NOT NULL
             );
             INSERT INTO categories
               (id, name, description, schema_json, entry_count, created_at)
             VALUES
               (1, 'journal', 'legacy journal', '{\"shape\":{},\"field_freq\":{}}', 1,
                '2026-06-02T10:00:00Z');
             INSERT INTO notes
               (id, raw_text, source, image_path, category_id, created_at)
             VALUES
               (1, 'A canonical note from the earliest schema.', 'text', NULL, 1,
                '2026-06-02T10:00:00Z');
             INSERT INTO entries
               (id, note_id, category_id, data_json, event_date, created_at)
             VALUES
               (1, 1, 1, '{\"mood\":\"focused\"}', '2026-06-02',
                '2026-06-02T10:00:00Z');",
        )
        .unwrap();
    drop(legacy);

    let conn = db::init(&path).expect("current initializer converges the legacy schema");
    let raw_text: String = conn
        .query_row("SELECT raw_text FROM notes WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(raw_text, "A canonical note from the earliest schema.");
    assert!(has_column(&conn, "notes", "title"));
    assert!(has_column(&conn, "notes", "origin"));
    assert!(has_column(&conn, "notes", "filing_context"));
    assert!(has_column(&conn, "meetings", "route_status"));
    let transcript_index: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'meeting_segments_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(transcript_index, 1);
    drop(conn);

    let reopened = db::init(&path).expect("initializer is idempotent after convergence");
    let note_count: i64 = reopened
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        note_count, 1,
        "reopening must not duplicate canonical content"
    );
    drop(reopened);
    remove_sqlite_files(&path);
}
