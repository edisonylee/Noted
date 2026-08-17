use anyhow::anyhow;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_app_lib::{backup, db, migrations};

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

fn recovery_artifacts(path: &Path) -> Vec<PathBuf> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let prefix = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("noted");
    let mut artifacts = std::fs::read_dir(parent.join("migration-recovery"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(prefix)
                .then_some(entry.path())
        })
        .collect::<Vec<_>>();
    artifacts.sort();
    artifacts
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

fn stamp_as_v1(path: &Path) {
    let mut connection = Connection::open(path).unwrap();
    migrations::stamp_converged_schema(
        &mut connection,
        migrations::MigrationDescriptor::new(
            1,
            "legacy-additive-baseline",
            "92aed657051490183fd931d523bc2146522f0e6094d176da9479b0be91d61659",
        ),
        migrations::DatabaseStamp::new(1, 1, 1),
        "v1-fixture",
    )
    .unwrap();
}

fn portable_identity_inventory(connection: &Connection) -> (String, Vec<(String, String, String)>) {
    let library_id = connection
        .query_row("SELECT library_id FROM libraries", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap();
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT p.record_id, h.accepted_version_id, v.snapshot_json
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 ORDER BY p.source_table, p.source_row_id",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    (library_id, records)
}

#[test]
fn unversioned_database_is_snapshotted_before_convergence_and_stamped_once() {
    let path = temp_db("legacy");
    cleanup(&path);
    seed_legacy_database(&path);

    let live = db::init(&path).expect("migrate legacy database");
    assert_eq!(
        migrations::read_database_stamp(&live).unwrap(),
        migrations::DatabaseStamp::new(2, 1, 2)
    );
    let application_id: i64 = live
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    assert_eq!(application_id, i64::from(migrations::NOTED_APPLICATION_ID));
    let user_version: i64 = live
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 2);
    assert_eq!(
        live.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        2
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
        2
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

#[test]
fn v1_upgrade_is_snapshotted_and_restore_recreates_portable_ids() {
    let path = temp_db("v1_upgrade");
    let restored_path = temp_db("v1_restored");
    cleanup(&path);
    cleanup(&restored_path);
    seed_legacy_database(&path);
    stamp_as_v1(&path);

    let upgraded = db::init(&path).expect("upgrade stamped v1 database");
    assert_eq!(
        migrations::read_database_stamp(&upgraded).unwrap(),
        migrations::DatabaseStamp::new(2, 1, 2)
    );
    let recovery_path: PathBuf = upgraded
        .query_row(
            "SELECT value FROM app_metadata
             WHERE key = 'last_pre_migration_recovery_path'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(PathBuf::from)
        .unwrap();
    let recovery = Connection::open(&recovery_path).unwrap();
    assert_eq!(
        migrations::read_database_stamp(&recovery).unwrap(),
        migrations::DatabaseStamp::new(1, 1, 1)
    );
    assert!(!recovery
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'libraries'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    drop(recovery);

    let expected = portable_identity_inventory(&upgraded);
    let original_device_id: String = upgraded
        .query_row(
            "SELECT device_id FROM portable_devices WHERE role = 'authority'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(upgraded);
    std::fs::copy(&recovery_path, &restored_path).unwrap();
    let restored = db::init(&restored_path).expect("restore and re-upgrade v1 recovery");
    assert_eq!(portable_identity_inventory(&restored), expected);
    let restored_device_id: String = restored
        .query_row(
            "SELECT device_id FROM portable_devices WHERE role = 'authority'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(restored_device_id, original_device_id);

    drop(restored);
    cleanup(&path);
    cleanup(&restored_path);
    let _ = std::fs::remove_file(recovery_path);
}

#[test]
fn v1_migration_defers_before_writing_when_a_meeting_capture_is_active() {
    let path = temp_db("active_recording");
    cleanup(&path);
    seed_legacy_database(&path);
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meetings (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   title TEXT NOT NULL,
                   status TEXT NOT NULL,
                   created_at TEXT NOT NULL
                 );
                 INSERT INTO meetings(title, status, created_at)
                 VALUES ('Do not interrupt', 'recording', '2026-08-01T12:05:00Z');",
            )
            .unwrap();
    }
    stamp_as_v1(&path);
    let exact_before = std::fs::read(&path).unwrap();
    assert!(recovery_artifacts(&path).is_empty());

    let error = db::init(&path).unwrap_err().to_string();
    assert!(error.contains("migration is deferred"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), exact_before);
    assert!(recovery_artifacts(&path).is_empty());

    let untouched = Connection::open(&path).unwrap();
    assert_eq!(
        migrations::read_database_stamp(&untouched).unwrap(),
        migrations::DatabaseStamp::new(1, 1, 1)
    );
    assert_eq!(
        untouched
            .query_row("SELECT status FROM meetings", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "recording"
    );
    assert!(!untouched
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'libraries'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());

    drop(untouched);
    cleanup(&path);
}

#[test]
fn pre_migration_snapshot_includes_committed_rows_still_in_the_wal() {
    let path = temp_db("active_wal");
    cleanup(&path);
    seed_legacy_database(&path);
    let wal_writer = Connection::open(&path).unwrap();
    wal_writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    wal_writer
        .execute(
            "INSERT INTO notes
             (raw_text, source, image_path, category_id, created_at)
             VALUES ('Committed only through WAL.', 'text', NULL, 1,
                     '2026-08-01T12:10:00Z')",
            [],
        )
        .unwrap();
    let note_id = wal_writer.last_insert_rowid();
    wal_writer
        .execute(
            "INSERT INTO entries
             (note_id, category_id, data_json, created_at)
             VALUES (?1, 1, '{\"text\":\"Committed only through WAL.\"}',
                     '2026-08-01T12:10:00Z')",
            [note_id],
        )
        .unwrap();
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    assert!(wal_path.metadata().unwrap().len() > 0);

    let migrated = db::init(&path).expect("migrate while committed WAL frames are active");
    let recovery_path: PathBuf = migrated
        .query_row(
            "SELECT value FROM app_metadata
             WHERE key = 'last_pre_migration_recovery_path'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(PathBuf::from)
        .unwrap();
    let recovery = Connection::open(&recovery_path).unwrap();
    assert_eq!(
        recovery
            .query_row(
                "SELECT raw_text FROM notes WHERE id = ?1",
                [note_id],
                |row| { row.get::<_, String>(0) }
            )
            .unwrap(),
        "Committed only through WAL."
    );
    assert_eq!(
        recovery
            .query_row(
                "SELECT COUNT(*) FROM entries WHERE note_id = ?1",
                [note_id],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        1
    );

    drop(recovery);
    drop(migrated);
    drop(wal_writer);
    cleanup(&path);
    let _ = std::fs::remove_file(recovery_path);
}

#[test]
fn injected_migration_failure_rolls_back_and_verified_recovery_restores_v1() {
    let path = temp_db("failure_live");
    let recovery_path = temp_db("failure_recovery");
    let restored_path = temp_db("failure_restored");
    cleanup(&path);
    cleanup(&recovery_path);
    cleanup(&restored_path);
    seed_legacy_database(&path);
    stamp_as_v1(&path);

    let mut live = Connection::open(&path).unwrap();
    backup::create_pre_migration_snapshot(&live, &recovery_path).unwrap();
    let v2 = migrations::MigrationDescriptor::new(2, "injected-v2", "injected-v2");
    let error = migrations::apply_migration(
        &mut live,
        v2,
        migrations::DatabaseStamp::new(2, 1, 2),
        "failure-fixture",
        |transaction| {
            transaction.execute_batch(
                "CREATE TABLE must_rollback (id INTEGER PRIMARY KEY);
                 UPDATE notes SET raw_text = 'must roll back' WHERE id = 1;",
            )?;
            Err(anyhow!("injected migration failure"))
        },
    )
    .unwrap_err();
    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("injected migration failure"),
        "{error_chain}"
    );
    assert_eq!(
        migrations::read_database_stamp(&live).unwrap(),
        migrations::DatabaseStamp::new(1, 1, 1)
    );
    assert_eq!(
        live.query_row("SELECT raw_text FROM notes WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "Preserve this exact legacy note."
    );
    assert!(!live
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'must_rollback'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    drop(live);

    std::fs::copy(&recovery_path, &restored_path).unwrap();
    let restored = Connection::open(&restored_path).unwrap();
    assert_eq!(
        migrations::read_database_stamp(&restored).unwrap(),
        migrations::DatabaseStamp::new(1, 1, 1)
    );
    assert_eq!(
        restored
            .query_row("SELECT raw_text FROM notes WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "Preserve this exact legacy note."
    );

    drop(restored);
    cleanup(&path);
    cleanup(&recovery_path);
    cleanup(&restored_path);
}
