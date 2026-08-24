use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_app_lib::{
    backup,
    db::{self, SaveInput},
};

fn paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "noted_backup_{label}_{}_{}",
        std::process::id(),
        nonce
    ));
    (
        root.with_extension("source.db"),
        root.with_extension("snapshot.db"),
    )
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

fn staging_artifacts(destination: &std::path::Path) -> Vec<std::path::PathBuf> {
    let prefix = format!(
        ".{}.staging-",
        destination.file_name().unwrap().to_string_lossy()
    );
    std::fs::read_dir(destination.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect()
}

fn save_fixture_note(connection: &mut rusqlite::Connection) {
    db::save_note(
        connection,
        SaveInput {
            raw_text: "bench 185".into(),
            source: "text".into(),
            image_path: None,
            event_date: "2026-06-09".into(),
            entries: vec![db::EntryInput {
                category: "gym".into(),
                description: String::new(),
                data: json!({"exercises":[{"name":"bench","weight":185}]}),
            }],
        },
        "2026-06-09T00:00:00Z",
    )
    .unwrap();
}

#[test]
fn validated_snapshot_is_consistent_private_and_independent() {
    let (source_path, snapshot_path) = paths("consistent");
    cleanup(&source_path);
    cleanup(&snapshot_path);

    let mut source = db::init(&source_path).unwrap();
    save_fixture_note(&mut source);
    source
        .execute(
            "INSERT INTO speaker_profiles (name, embedding, samples, updated_at)
             VALUES ('Sensitive fixture', X'01020304', 1, '2026-06-09T00:00:00Z')",
            [],
        )
        .unwrap();

    backup::create_database_snapshot(&source, &snapshot_path).unwrap();
    assert!(snapshot_path.is_file());

    // This interim export intentionally preserves every database row, including
    // sensitive speaker embeddings. It is plaintext and database-only, not a
    // sanitized or complete media recovery archive.
    let snapshot = rusqlite::Connection::open(&snapshot_path).unwrap();
    let body: String = snapshot
        .query_row("SELECT raw_text FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(body, "bench 185");
    let sensitive_rows: i64 = snapshot
        .query_row("SELECT COUNT(*) FROM speaker_profiles", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sensitive_rows, 1);
    let quick_check: String = snapshot
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(quick_check, "ok");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&snapshot_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    drop(snapshot);
    drop(source);
    cleanup(&source_path);
    cleanup(&snapshot_path);
}

#[test]
fn snapshot_refuses_to_overwrite_an_existing_destination() {
    let (source_path, snapshot_path) = paths("no_overwrite");
    cleanup(&source_path);
    cleanup(&snapshot_path);
    let source = db::init(&source_path).unwrap();
    std::fs::write(&snapshot_path, b"existing user file").unwrap();

    let error = backup::create_database_snapshot(&source, &snapshot_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"));
    assert_eq!(
        std::fs::read(&snapshot_path).unwrap(),
        b"existing user file"
    );

    drop(source);
    cleanup(&source_path);
    cleanup(&snapshot_path);
}

#[test]
fn invalid_foreign_keys_never_publish_a_snapshot() {
    let (source_path, snapshot_path) = paths("foreign_keys");
    cleanup(&source_path);
    cleanup(&snapshot_path);
    let source = db::init(&source_path).unwrap();
    source.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    source
        .execute(
            "INSERT INTO entries
               (note_id, category_id, data_json, event_date, created_at)
             VALUES (999999, 999999, '{}', '2026-06-09', '2026-06-09T00:00:00Z')",
            [],
        )
        .unwrap();

    let error = backup::create_database_snapshot(&source, &snapshot_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("foreign_key_check"));
    assert!(!snapshot_path.exists());
    assert!(staging_artifacts(&snapshot_path).is_empty());

    drop(source);
    cleanup(&source_path);
    cleanup(&snapshot_path);
}
