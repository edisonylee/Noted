#[path = "../src/migrations.rs"]
mod migrations;

use anyhow::anyhow;
use migrations::{
    apply_migration, inspect_database, negotiate_record_kind, negotiate_schema,
    read_database_stamp, stamp_converged_schema, verify_known_migrations, ClientCapabilities,
    DatabaseSchemaState, DatabaseStamp, MigrationDescriptor, MigrationOutcome,
    RecordKindCapability, SchemaAccess, NOTED_APPLICATION_ID,
};
use rusqlite::{Connection, OptionalExtension};

const BASELINE: MigrationDescriptor<'static> =
    MigrationDescriptor::new(1, "stamp-legacy-baseline", "baseline-v1");

#[test]
fn stamps_a_converged_database_and_replays_idempotently() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
         INSERT INTO notes (body) VALUES ('legacy data');",
    )
    .unwrap();
    let stamp = DatabaseStamp::new(1, 1, 1);

    assert_eq!(
        stamp_converged_schema(&mut conn, BASELINE, stamp, "0.1.0").unwrap(),
        MigrationOutcome::Applied
    );
    assert_eq!(
        stamp_converged_schema(&mut conn, BASELINE, stamp, "0.1.1").unwrap(),
        MigrationOutcome::AlreadyApplied
    );

    let application_id: i64 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    assert_eq!(application_id, i64::from(NOTED_APPLICATION_ID));
    assert_eq!(read_database_stamp(&conn).unwrap(), stamp);
    assert_eq!(
        inspect_database(&conn).unwrap(),
        DatabaseSchemaState::Stamped(stamp)
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM app_metadata WHERE key = 'schema_product_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "0.1.0"
    );
    assert_eq!(
        conn.query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "legacy data"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}

#[test]
fn inspection_distinguishes_unversioned_files_without_claiming_them() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE legacy_notes (id INTEGER PRIMARY KEY);")
        .unwrap();
    assert_eq!(
        inspect_database(&conn).unwrap(),
        DatabaseSchemaState::Unversioned
    );
    let application_id: i64 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    assert_eq!(application_id, 0);
}

#[test]
fn detects_rewritten_migration_history() {
    let mut conn = Connection::open_in_memory().unwrap();
    let stamp = DatabaseStamp::new(1, 1, 1);
    stamp_converged_schema(&mut conn, BASELINE, stamp, "0.1.0").unwrap();

    let changed = MigrationDescriptor::new(1, BASELINE.name, "different-checksum");
    let error = stamp_converged_schema(&mut conn, changed, stamp, "0.1.1")
        .unwrap_err()
        .to_string();
    assert!(error.contains("checksum mismatch"), "{error}");

    let error = verify_known_migrations(&conn, &[changed])
        .unwrap_err()
        .to_string();
    assert!(error.contains("checksum mismatch"), "{error}");

    let renamed = MigrationDescriptor::new(1, "renamed-baseline", BASELINE.checksum);
    let error = verify_known_migrations(&conn, &[renamed])
        .unwrap_err()
        .to_string();
    assert!(error.contains("name mismatch"), "{error}");
}

#[test]
fn stamped_version_history_and_pragma_must_align_exactly() {
    let mut user_version_ahead = Connection::open_in_memory().unwrap();
    stamp_converged_schema(
        &mut user_version_ahead,
        BASELINE,
        DatabaseStamp::new(1, 1, 1),
        "0.1.0",
    )
    .unwrap();
    user_version_ahead
        .pragma_update(None, "user_version", 2)
        .unwrap();
    let error = inspect_database(&user_version_ahead)
        .unwrap_err()
        .to_string();
    assert!(error.contains("user_version 2"), "{error}");

    let mut history_ahead = Connection::open_in_memory().unwrap();
    stamp_converged_schema(
        &mut history_ahead,
        BASELINE,
        DatabaseStamp::new(1, 1, 1),
        "0.1.0",
    )
    .unwrap();
    history_ahead
        .execute(
            "INSERT INTO schema_migrations
             (version, name, checksum, applied_at, product_version)
             VALUES (2, 'uncommitted-v2', 'uncommitted-v2', 'now', '0.2.0')",
            [],
        )
        .unwrap();
    let error = inspect_database(&history_ahead).unwrap_err().to_string();
    assert!(error.contains("history is ahead"), "{error}");

    let mut stamp_ahead = Connection::open_in_memory().unwrap();
    stamp_converged_schema(
        &mut stamp_ahead,
        BASELINE,
        DatabaseStamp::new(1, 1, 1),
        "0.1.0",
    )
    .unwrap();
    stamp_ahead
        .execute(
            "UPDATE app_metadata SET value = '2' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    stamp_ahead.pragma_update(None, "user_version", 2).unwrap();
    let error = inspect_database(&stamp_ahead).unwrap_err().to_string();
    assert!(error.contains("history is behind"), "{error}");

    let mut history_gap = Connection::open_in_memory().unwrap();
    stamp_converged_schema(
        &mut history_gap,
        BASELINE,
        DatabaseStamp::new(1, 1, 1),
        "0.1.0",
    )
    .unwrap();
    history_gap
        .execute(
            "INSERT INTO schema_migrations
             (version, name, checksum, applied_at, product_version)
             VALUES (3, 'skipped-v2', 'skipped-v2', 'now', '0.3.0')",
            [],
        )
        .unwrap();
    let error = inspect_database(&history_gap).unwrap_err().to_string();
    assert!(error.contains("history has a gap"), "{error}");
}

#[test]
fn partial_or_unknown_migration_state_is_rejected() {
    let partial = Connection::open_in_memory().unwrap();
    partial
        .execute_batch(
            "CREATE TABLE app_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO app_metadata VALUES ('schema_version', '1');",
        )
        .unwrap();
    let error = inspect_database(&partial).unwrap_err().to_string();
    assert!(error.contains("partial migration state"), "{error}");

    let mut conn = Connection::open_in_memory().unwrap();
    stamp_converged_schema(&mut conn, BASELINE, DatabaseStamp::new(1, 1, 1), "0.1.0").unwrap();
    let v2 = MigrationDescriptor::new(2, "portable-records", "portable-v2");
    apply_migration(&mut conn, v2, DatabaseStamp::new(2, 1, 2), "0.2.0", |_| {
        Ok(())
    })
    .unwrap();
    let error = verify_known_migrations(&conn, &[BASELINE])
        .unwrap_err()
        .to_string();
    assert!(error.contains("no matching descriptor"), "{error}");
}

#[test]
fn per_kind_capability_never_grants_a_lossy_write() {
    let capabilities = [
        RecordKindCapability::new("note", 2, 1),
        RecordKindCapability::new("folder", 1, 1),
    ];

    assert_eq!(
        negotiate_record_kind(SchemaAccess::ReadWrite, "note", 1, &capabilities),
        SchemaAccess::ReadWrite
    );
    assert_eq!(
        negotiate_record_kind(SchemaAccess::ReadWrite, "note", 2, &capabilities),
        SchemaAccess::ReadOnly
    );
    assert_eq!(
        negotiate_record_kind(SchemaAccess::ReadWrite, "note", 3, &capabilities),
        SchemaAccess::Reject
    );
    assert_eq!(
        negotiate_record_kind(SchemaAccess::ReadOnly, "folder", 1, &capabilities),
        SchemaAccess::ReadOnly
    );
    assert_eq!(
        negotiate_record_kind(SchemaAccess::ReadWrite, "meeting", 1, &capabilities),
        SchemaAccess::Reject
    );
}

#[test]
fn orders_migrations_and_commits_schema_with_its_writer_floor() {
    let mut conn = Connection::open_in_memory().unwrap();
    stamp_converged_schema(&mut conn, BASELINE, DatabaseStamp::new(1, 1, 1), "0.1.0").unwrap();

    let v2 = MigrationDescriptor::new(2, "portable-records", "portable-v2");
    assert_eq!(
        apply_migration(&mut conn, v2, DatabaseStamp::new(2, 1, 2), "0.2.0", |tx| {
            tx.execute_batch(
                "CREATE TABLE portable_records (
                       record_id TEXT PRIMARY KEY,
                       body TEXT NOT NULL
                     );",
            )?;
            Ok(())
        },)
        .unwrap(),
        MigrationOutcome::Applied
    );
    assert_eq!(
        negotiate_schema(
            read_database_stamp(&conn).unwrap(),
            ClientCapabilities::new(2, 1, 1),
        ),
        SchemaAccess::ReadOnly
    );
    assert_eq!(
        negotiate_schema(
            read_database_stamp(&conn).unwrap(),
            ClientCapabilities::new(2, 1, 2),
        ),
        SchemaAccess::ReadWrite
    );
    verify_known_migrations(&conn, &[BASELINE, v2]).unwrap();

    assert_eq!(
        apply_migration(
            &mut conn,
            BASELINE,
            DatabaseStamp::new(1, 1, 1),
            "0.2.0",
            |_| Err(anyhow!("baseline body was replayed")),
        )
        .unwrap(),
        MigrationOutcome::AlreadyApplied
    );
}

#[test]
fn failed_migration_rolls_back_body_history_and_stamp() {
    let mut conn = Connection::open_in_memory().unwrap();
    let baseline_stamp = DatabaseStamp::new(1, 1, 1);
    stamp_converged_schema(&mut conn, BASELINE, baseline_stamp, "0.1.0").unwrap();

    let v2 = MigrationDescriptor::new(2, "will-fail", "failure-v2");
    let error = apply_migration(&mut conn, v2, DatabaseStamp::new(2, 2, 2), "0.2.0", |tx| {
        tx.execute_batch("CREATE TABLE should_rollback (id INTEGER);")?;
        Err(anyhow!("injected failure"))
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("apply schema migration 2"), "{error}");
    assert_eq!(read_database_stamp(&conn).unwrap(), baseline_stamp);
    assert!(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'should_rollback'",
            [],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_none());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}

#[test]
fn rejects_unknown_schema_and_reader_floor_before_considering_writes() {
    let stamp = DatabaseStamp::new(3, 2, 3);

    assert_eq!(
        negotiate_schema(stamp, ClientCapabilities::new(2, 3, 3)),
        SchemaAccess::Reject
    );
    assert_eq!(
        negotiate_schema(stamp, ClientCapabilities::new(3, 1, 3)),
        SchemaAccess::Reject
    );
    assert_eq!(
        negotiate_schema(stamp, ClientCapabilities::new(3, 2, 0)),
        SchemaAccess::ReadOnly
    );
}
