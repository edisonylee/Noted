use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_app_lib::{db, migrations};

fn temp_db(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "noted_migration_startup_{label}_{}_{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    if let Some(parent) = path.parent() {
        let recovery_dir = parent.join("migration-recovery");
        if let Ok(entries) = std::fs::read_dir(&recovery_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("noted"),
                ) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn seed_legacy_database(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
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
               created_at TEXT NOT NULL
             );
             INSERT INTO categories
               (name, description, schema_json, entry_count, created_at)
             VALUES ('journal', '', '{\"shape\":{},\"field_freq\":{}}', 1,
                     '2026-08-01T12:00:00Z');
             INSERT INTO notes
               (raw_text, source, image_path, category_id, created_at)
             VALUES ('Preserve this exact legacy note.', 'text', NULL, 1,
                     '2026-08-01T12:00:00Z');
             INSERT INTO entries
               (note_id, category_id, data_json, created_at)
             VALUES (1, 1, '{\"text\":\"Preserve this exact legacy note.\"}',
                     '2026-08-01T12:00:00Z');",
        )
        .unwrap();
}

#[test]
fn unversioned_database_is_snapshotted_before_convergence_and_stamped_once() {
    let path = temp_db("legacy");
    cleanup(&path);
    seed_legacy_database(&path);

    let live = db::init(&path).expect("migrate legacy database");
    assert_eq!(
        migrations::read_database_stamp(&live).unwrap(),
        migrations::DatabaseStamp::new(1, 1, 1)
    );
    let application_id: i64 = live
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    assert_eq!(application_id, i64::from(migrations::NOTED_APPLICATION_ID));
    let user_version: i64 = live
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 1);
    assert_eq!(
        live.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    let recovery_path: String = live
        .query_row(
            "SELECT value FROM app_metadata
             WHERE key = 'last_pre_migration_recovery_path'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let recovery_path = PathBuf::from(recovery_path);
    assert!(recovery_path.is_file());

    // The recovery artifact is the exact pre-convergence schema, not a backup
    // made after migration metadata was written.
    let recovery = Connection::open(&recovery_path).unwrap();
    let history_exists: bool = recovery
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'schema_migrations'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!history_exists);
    assert_eq!(
        recovery
            .query_row("SELECT raw_text FROM notes WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "Preserve this exact legacy note."
    );
    drop(recovery);
    drop(live);

    let reopened = db::init(&path).expect("reopen migrated database");
    assert_eq!(
        reopened
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    let stored_recovery: String = reopened
        .query_row(
            "SELECT value FROM app_metadata
             WHERE key = 'last_pre_migration_recovery_path'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_recovery, recovery_path.to_string_lossy());

    drop(reopened);
    cleanup(&path);
    let _ = std::fs::remove_file(recovery_path);
}

#[test]
fn foreign_application_id_is_rejected_before_noted_schema_writes() {
    let path = temp_db("foreign");
    cleanup(&path);
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA application_id=1234;
                 CREATE TABLE foreign_data (value TEXT NOT NULL);
                 INSERT INTO foreign_data VALUES ('untouched');",
            )
            .unwrap();
    }

    let error = db::init(&path).unwrap_err().to_string();
    assert!(error.contains("not a Noted database"), "{error}");
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM foreign_data", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "untouched"
    );
    assert!(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'notes'",
            [],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_none());

    drop(connection);
    cleanup(&path);
}
