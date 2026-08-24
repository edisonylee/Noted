use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use tauri_app_lib::db;

fn temp_db(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "noted_phase0_{label}_{}_{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

fn stamp(conn: &mut Connection, version: u32, min_reader: u32, name: &str) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS phase0_schema_migrations (
           version INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           checksum TEXT NOT NULL,
           applied_at TEXT NOT NULL
         );
         PRAGMA application_id = 1313821764;",
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO phase0_schema_migrations
         (version, name, checksum, applied_at)
         VALUES (?1, ?2, ?3, '2026-08-06T12:00:00Z')",
        params![version, name, format!("phase0-checksum-{version}")],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO app_metadata(key, value)
         VALUES ('phase0_schema_version', ?1)",
        [version.to_string()],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO app_metadata(key, value)
         VALUES ('phase0_min_reader_version', ?1)",
        [min_reader.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

fn check_reader_compatibility(
    conn: &Connection,
    reader_version: u32,
    max_schema: u32,
) -> Result<()> {
    let schema: u32 = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'phase0_schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse()?;
    let min_reader: u32 = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = 'phase0_min_reader_version'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse()?;
    if reader_version < min_reader || schema > max_schema {
        bail!(
            "incompatible database: schema={schema}, min_reader={min_reader}, reader={reader_version}, max_schema={max_schema}"
        );
    }
    Ok(())
}

#[test]
fn recovery_point_and_reader_floor_support_a_safe_binary_downgrade() {
    let live_path = temp_db("migration_live");
    let recovery_path = temp_db("migration_recovery");
    cleanup(&live_path);
    cleanup(&recovery_path);

    let mut live = db::init(&live_path).unwrap();
    live.execute(
        "INSERT INTO notes
         (title, raw_text, source, category_id, created_at, origin, filing_context)
         VALUES ('Baseline record', 'Preserve me across recovery.', 'text', NULL,
                 '2026-08-06T12:00:00Z', 'capture', 'personal')",
        [],
    )
    .unwrap();
    stamp(&mut live, 1, 1, "stamp-legacy-baseline").unwrap();
    check_reader_compatibility(&live, 1, 1).unwrap();

    let integrity: String = live
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    live.execute("VACUUM INTO ?1", [recovery_path.to_string_lossy().as_ref()])
        .expect("create a consistent recovery point without copying the live DB file");
    assert!(std::fs::metadata(&recovery_path).unwrap().len() > 0);

    stamp(&mut live, 2, 2, "incompatible-example").unwrap();
    assert!(
        check_reader_compatibility(&live, 1, 1).is_err(),
        "an old reader refuses the newer reader floor"
    );
    check_reader_compatibility(&live, 2, 2).unwrap();
    drop(live);

    // A binary downgrade restores the matching closed recovery dataset. Copying
    // is safe here because both SQLite databases are closed; live backup creation
    // above used VACUUM INTO.
    std::fs::copy(&recovery_path, &live_path).unwrap();
    let restored = db::init(&live_path).unwrap();
    check_reader_compatibility(&restored, 1, 1).unwrap();
    let body: String = restored
        .query_row(
            "SELECT raw_text FROM notes WHERE title = 'Baseline record'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, "Preserve me across recovery.");
    drop(restored);

    cleanup(&live_path);
    cleanup(&recovery_path);
}
