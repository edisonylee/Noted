#[path = "../src/migrations.rs"]
mod migrations;

use anyhow::anyhow;
use migrations::{
    apply_migration, inspect_database, negotiate_schema, read_database_stamp,
    stamp_converged_schema, verify_known_migrations, ClientCapabilities, DatabaseSchemaState,
    DatabaseStamp, MigrationDescriptor, MigrationOutcome, SchemaAccess, NOTED_APPLICATION_ID,
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
