//! Ordered SQLite schema migration primitives shared by every Noted runtime.
//!
//! This module deliberately does not know how to build the legacy schema or
//! create recovery points. Callers first converge an unversioned database to a
//! known schema, create any required recovery point, and then use
//! [`stamp_converged_schema`] to make that baseline explicit. Later migrations
//! should use [`apply_migration`] so their schema changes and compatibility
//! stamp commit atomically.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

/// ASCII `NOTD`, used to reject accidentally opened non-Noted SQLite files.
pub const NOTED_APPLICATION_ID: u32 = 0x4e4f_5444;

const SCHEMA_VERSION_KEY: &str = "schema_version";
const MIN_READER_VERSION_KEY: &str = "min_reader_version";
const MIN_WRITER_VERSION_KEY: &str = "min_writer_version";
const PRODUCT_VERSION_KEY: &str = "schema_product_version";

const TRACKING_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS app_metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY CHECK (version > 0),
  name TEXT NOT NULL CHECK (length(name) > 0),
  checksum TEXT NOT NULL CHECK (length(checksum) > 0),
  applied_at TEXT NOT NULL,
  product_version TEXT NOT NULL CHECK (length(product_version) > 0)
);

CREATE TRIGGER IF NOT EXISTS schema_migrations_no_update
BEFORE UPDATE ON schema_migrations
BEGIN
  SELECT RAISE(ABORT, 'schema_migrations is append-only');
END;

CREATE TRIGGER IF NOT EXISTS schema_migrations_no_delete
BEFORE DELETE ON schema_migrations
BEGIN
  SELECT RAISE(ABORT, 'schema_migrations is append-only');
END;
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseStamp {
    pub schema_version: u32,
    pub min_reader_version: u32,
    pub min_writer_version: u32,
}

impl DatabaseStamp {
    pub const fn new(
        schema_version: u32,
        min_reader_version: u32,
        min_writer_version: u32,
    ) -> Self {
        Self {
            schema_version,
            min_reader_version,
            min_writer_version,
        }
    }

    fn validate(self) -> Result<()> {
        if self.schema_version == 0 {
            bail!("database schema version must be greater than zero");
        }
        if self.min_reader_version == 0 {
            bail!("minimum reader version must be greater than zero");
        }
        if self.min_writer_version == 0 {
            bail!("minimum writer version must be greater than zero");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientCapabilities {
    /// Newest database schema this binary knows how to interpret.
    pub max_schema_version: u32,
    /// Reader protocol implemented by this binary.
    pub reader_version: u32,
    /// Writer protocol implemented by this binary. Use zero for a read-only
    /// client.
    pub writer_version: u32,
}

impl ClientCapabilities {
    pub const fn new(max_schema_version: u32, reader_version: u32, writer_version: u32) -> Self {
        Self {
            max_schema_version,
            reader_version,
            writer_version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaAccess {
    Reject,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseSchemaState {
    Unversioned,
    Stamped(DatabaseStamp),
}

/// Inspect a database without claiming or changing it. A non-Noted application
/// ID is always rejected before schema convergence can write to the file.
pub fn inspect_database(conn: &Connection) -> Result<DatabaseSchemaState> {
    let application_id = application_id(conn)?;
    if application_id != 0 && application_id != NOTED_APPLICATION_ID {
        bail!(
            "not a Noted database: application_id is {application_id:#010x}, expected {NOTED_APPLICATION_ID:#010x}"
        );
    }

    let has_history: bool = conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_schema
           WHERE type = 'table' AND name = 'schema_migrations'
         )",
        [],
        |row| row.get(0),
    )?;
    if !has_history {
        return Ok(DatabaseSchemaState::Unversioned);
    }
    if application_id == 0 {
        bail!("versioned database is missing the Noted application_id");
    }
    Ok(DatabaseSchemaState::Stamped(read_database_stamp_from(
        conn,
    )?))
}

/// Computes the strongest access a client can safely have to a stamped DB.
///
/// A client that cannot read the schema is rejected outright. A client that
/// can read but does not meet the writer floor is intentionally downgraded to
/// read-only access instead of being allowed to emit an older record shape.
pub fn negotiate_schema(database: DatabaseStamp, client: ClientCapabilities) -> SchemaAccess {
    if database.validate().is_err()
        || client.max_schema_version < database.schema_version
        || client.reader_version < database.min_reader_version
    {
        return SchemaAccess::Reject;
    }

    if client.writer_version < database.min_writer_version {
        SchemaAccess::ReadOnly
    } else {
        SchemaAccess::ReadWrite
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationDescriptor<'a> {
    pub version: u32,
    pub name: &'a str,
    pub checksum: &'a str,
}

impl<'a> MigrationDescriptor<'a> {
    pub const fn new(version: u32, name: &'a str, checksum: &'a str) -> Self {
        Self {
            version,
            name,
            checksum,
        }
    }

    fn validate(self) -> Result<()> {
        if self.version == 0 {
            bail!("migration version must be greater than zero");
        }
        if self.name.trim().is_empty() {
            bail!("migration name must not be empty");
        }
        if self.checksum.trim().is_empty() {
            bail!("migration checksum must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    Applied,
    AlreadyApplied,
}

/// Atomically stamps a schema that the caller has already converged to a known
/// baseline. This does not execute schema DDL and therefore must only be called
/// after legacy initialization and validation have succeeded.
pub fn stamp_converged_schema(
    conn: &mut Connection,
    migration: MigrationDescriptor<'_>,
    stamp: DatabaseStamp,
    product_version: &str,
) -> Result<MigrationOutcome> {
    apply_migration(conn, migration, stamp, product_version, |_| Ok(()))
}

/// Runs one ordered migration and records its compatibility stamp in the same
/// SQLite transaction.
///
/// Migrations start at version 1 and must be applied without gaps. Re-running
/// an already-applied version with the same name and checksum is a no-op; the
/// migration body is not executed a second time. Reusing a version with a
/// different name or checksum is treated as database/code incompatibility.
pub fn apply_migration<F>(
    conn: &mut Connection,
    migration: MigrationDescriptor<'_>,
    stamp: DatabaseStamp,
    product_version: &str,
    migrate: F,
) -> Result<MigrationOutcome>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    migration.validate()?;
    stamp.validate()?;
    if stamp.schema_version != migration.version {
        bail!(
            "migration version {} does not match schema stamp {}",
            migration.version,
            stamp.schema_version
        );
    }
    if product_version.trim().is_empty() {
        bail!("product version must not be empty");
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin schema migration transaction")?;
    tx.execute_batch(TRACKING_SCHEMA)
        .context("initialize schema migration tracking")?;
    verify_or_claim_application_id(&tx)?;
    verify_contiguous_history(&tx)?;

    if let Some((existing_name, existing_checksum)) = migration_identity(&tx, migration.version)? {
        if existing_checksum != migration.checksum {
            bail!(
                "migration {} checksum mismatch: database has '{}', binary expects '{}'",
                migration.version,
                existing_checksum,
                migration.checksum
            );
        }
        if existing_name != migration.name {
            bail!(
                "migration {} name mismatch: database has '{}', binary expects '{}'",
                migration.version,
                existing_name,
                migration.name
            );
        }

        let current_stamp = read_database_stamp_from(&tx)?;
        if current_stamp.schema_version < migration.version {
            bail!(
                "migration {} is recorded ahead of database schema stamp {}",
                migration.version,
                current_stamp.schema_version
            );
        }
        if current_stamp.schema_version == migration.version && current_stamp != stamp {
            bail!(
                "migration {} is recorded with compatibility stamp {:?}, not {:?}",
                migration.version,
                current_stamp,
                stamp
            );
        }
        tx.commit()?;
        return Ok(MigrationOutcome::AlreadyApplied);
    }

    enforce_next_version(&tx, migration.version)?;
    migrate(&tx).with_context(|| {
        format!(
            "apply schema migration {} ({})",
            migration.version, migration.name
        )
    })?;

    tx.execute(
        "INSERT INTO schema_migrations
         (version, name, checksum, applied_at, product_version)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?4)",
        (
            i64::from(migration.version),
            migration.name,
            migration.checksum,
            product_version,
        ),
    )?;
    write_database_stamp(&tx, stamp, product_version)?;
    tx.commit().context("commit schema migration transaction")?;
    Ok(MigrationOutcome::Applied)
}

/// Reads and validates the compatibility stamp from a Noted database.
pub fn read_database_stamp(conn: &Connection) -> Result<DatabaseStamp> {
    verify_application_id(conn)?;
    read_database_stamp_from(conn)
}

/// Verifies the immutable identity of all migrations this binary knows about.
/// Newer database rows are left to [`negotiate_schema`] to reject based on the
/// supported schema ceiling.
pub fn verify_known_migrations(
    conn: &Connection,
    expected: &[MigrationDescriptor<'_>],
) -> Result<()> {
    verify_application_id(conn)?;
    let stamp = read_database_stamp_from(conn)?;
    verify_contiguous_history(conn)?;

    for migration in expected {
        migration.validate()?;
        if migration.version > stamp.schema_version {
            continue;
        }
        let Some((name, checksum)) = migration_identity(conn, migration.version)? else {
            bail!(
                "database schema {} is missing migration {} ({})",
                stamp.schema_version,
                migration.version,
                migration.name
            );
        };
        if checksum != migration.checksum {
            bail!(
                "migration {} checksum mismatch: database has '{}', binary expects '{}'",
                migration.version,
                checksum,
                migration.checksum
            );
        }
        if name != migration.name {
            bail!(
                "migration {} name mismatch: database has '{}', binary expects '{}'",
                migration.version,
                name,
                migration.name
            );
        }
    }
    Ok(())
}

fn verify_or_claim_application_id(conn: &Connection) -> Result<()> {
    let application_id = application_id(conn)?;
    if application_id != 0 && application_id != NOTED_APPLICATION_ID {
        bail!(
            "not a Noted database: application_id is {application_id:#010x}, expected {NOTED_APPLICATION_ID:#010x}"
        );
    }
    if application_id == 0 {
        conn.pragma_update(None, "application_id", i64::from(NOTED_APPLICATION_ID))?;
    }
    Ok(())
}

fn verify_application_id(conn: &Connection) -> Result<()> {
    let application_id = application_id(conn)?;
    if application_id != NOTED_APPLICATION_ID {
        bail!(
            "not a Noted database: application_id is {application_id:#010x}, expected {NOTED_APPLICATION_ID:#010x}"
        );
    }
    Ok(())
}

fn application_id(conn: &Connection) -> Result<u32> {
    let value: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    u32::try_from(value).context("SQLite application_id is outside the supported range")
}

fn enforce_next_version(conn: &Connection, version: u32) -> Result<()> {
    let latest: Option<u32> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?
        .map(|value| {
            u32::try_from(value).context("stored migration version is outside the supported range")
        })
        .transpose()?;

    let expected = match latest {
        Some(value) => value
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("migration version space is exhausted"))?,
        None => 1,
    };
    if version != expected {
        bail!("migration {version} is out of order; next migration must be {expected}");
    }
    Ok(())
}

fn verify_contiguous_history(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let mut rows = statement.query([])?;
    let mut expected = 1_u32;
    while let Some(row) = rows.next()? {
        let stored = u32::try_from(row.get::<_, i64>(0)?)
            .context("stored migration version is outside the supported range")?;
        if stored != expected {
            bail!(
                "schema migration history has a gap: expected version {expected}, found {stored}"
            );
        }
        expected = expected
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("migration version space is exhausted"))?;
    }
    Ok(())
}

fn migration_identity(conn: &Connection, version: u32) -> Result<Option<(String, String)>> {
    conn.query_row(
        "SELECT name, checksum FROM schema_migrations WHERE version = ?1",
        [i64::from(version)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn write_database_stamp(
    conn: &Connection,
    stamp: DatabaseStamp,
    product_version: &str,
) -> Result<()> {
    for (key, value) in [
        (SCHEMA_VERSION_KEY, stamp.schema_version),
        (MIN_READER_VERSION_KEY, stamp.min_reader_version),
        (MIN_WRITER_VERSION_KEY, stamp.min_writer_version),
    ] {
        conn.execute(
            "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, value.to_string()),
        )?;
    }
    conn.execute(
        "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (PRODUCT_VERSION_KEY, product_version),
    )?;
    conn.pragma_update(None, "user_version", i64::from(stamp.schema_version))?;
    Ok(())
}

fn read_database_stamp_from(conn: &Connection) -> Result<DatabaseStamp> {
    Ok(DatabaseStamp {
        schema_version: read_metadata_version(conn, SCHEMA_VERSION_KEY)?,
        min_reader_version: read_metadata_version(conn, MIN_READER_VERSION_KEY)?,
        min_writer_version: read_metadata_version(conn, MIN_WRITER_VERSION_KEY)?,
    })
}

fn read_metadata_version(conn: &Connection, key: &str) -> Result<u32> {
    let value: String = conn
        .query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .with_context(|| format!("database is missing compatibility metadata '{key}'"))?;
    value.parse::<u32>().with_context(|| {
        format!("compatibility metadata '{key}' is not a valid version: '{value}'")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    const V1: MigrationDescriptor<'static> =
        MigrationDescriptor::new(1, "stamp-legacy-baseline", "baseline-v1");

    #[test]
    fn negotiation_enforces_schema_reader_and_writer_floors() {
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
            negotiate_schema(stamp, ClientCapabilities::new(3, 2, 2)),
            SchemaAccess::ReadOnly
        );
        assert_eq!(
            negotiate_schema(stamp, ClientCapabilities::new(3, 2, 3)),
            SchemaAccess::ReadWrite
        );
    }

    #[test]
    fn baseline_stamp_is_transactional_and_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE legacy_note (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
             INSERT INTO legacy_note (body) VALUES ('preserve me');",
        )
        .unwrap();
        let stamp = DatabaseStamp::new(1, 1, 1);

        assert_eq!(
            stamp_converged_schema(&mut conn, V1, stamp, "0.1.0").unwrap(),
            MigrationOutcome::Applied
        );
        let applied_at: String = conn
            .query_row(
                "SELECT applied_at FROM schema_migrations WHERE version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            stamp_converged_schema(&mut conn, V1, stamp, "0.1.1").unwrap(),
            MigrationOutcome::AlreadyApplied
        );
        assert_eq!(read_database_stamp(&conn).unwrap(), stamp);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT applied_at FROM schema_migrations WHERE version = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            applied_at
        );
    }

    #[test]
    fn checksum_mismatch_fails_without_changing_the_stamp() {
        let mut conn = Connection::open_in_memory().unwrap();
        let stamp = DatabaseStamp::new(1, 1, 1);
        stamp_converged_schema(&mut conn, V1, stamp, "0.1.0").unwrap();

        let changed = MigrationDescriptor::new(1, V1.name, "rewritten-history");
        let error = stamp_converged_schema(&mut conn, changed, stamp, "0.1.1")
            .unwrap_err()
            .to_string();

        assert!(error.contains("checksum mismatch"), "{error}");
        assert_eq!(read_database_stamp(&conn).unwrap(), stamp);
    }

    #[test]
    fn migration_order_and_failure_rollback_are_enforced() {
        let mut conn = Connection::open_in_memory().unwrap();
        stamp_converged_schema(&mut conn, V1, DatabaseStamp::new(1, 1, 1), "0.1.0").unwrap();

        let skipped = MigrationDescriptor::new(3, "skip-v2", "skip-v2");
        let error = apply_migration(
            &mut conn,
            skipped,
            DatabaseStamp::new(3, 1, 1),
            "0.3.0",
            |tx| {
                tx.execute_batch("CREATE TABLE must_not_exist (id INTEGER);")?;
                Ok(())
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("out of order"), "{error}");
        assert!(conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'must_not_exist'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());

        let v2 = MigrationDescriptor::new(2, "portable-records", "portable-v2");
        let failure = apply_migration(&mut conn, v2, DatabaseStamp::new(2, 1, 2), "0.2.0", |tx| {
            tx.execute_batch("CREATE TABLE rolled_back (id INTEGER);")?;
            Err(anyhow!("injected migration failure"))
        })
        .unwrap_err()
        .to_string();
        assert!(failure.contains("apply schema migration 2"), "{failure}");
        assert!(conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rolled_back'",
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
    fn migration_history_is_append_only() {
        let mut conn = Connection::open_in_memory().unwrap();
        stamp_converged_schema(&mut conn, V1, DatabaseStamp::new(1, 1, 1), "0.1.0").unwrap();

        assert!(conn
            .execute(
                "UPDATE schema_migrations SET checksum = 'mutated' WHERE version = 1",
                [],
            )
            .is_err());
        assert!(conn
            .execute("DELETE FROM schema_migrations WHERE version = 1", [])
            .is_err());
    }

    #[test]
    fn an_older_applied_migration_remains_idempotent_after_schema_advances() {
        let mut conn = Connection::open_in_memory().unwrap();
        stamp_converged_schema(&mut conn, V1, DatabaseStamp::new(1, 1, 1), "0.1.0").unwrap();
        let v2 = MigrationDescriptor::new(2, "portable-records", "portable-v2");
        apply_migration(&mut conn, v2, DatabaseStamp::new(2, 1, 2), "0.2.0", |_| {
            Ok(())
        })
        .unwrap();

        let outcome = apply_migration(&mut conn, V1, DatabaseStamp::new(1, 1, 1), "0.2.0", |_| {
            bail!("an applied migration body must not run again")
        })
        .unwrap();
        assert_eq!(outcome, MigrationOutcome::AlreadyApplied);
        assert_eq!(
            read_database_stamp(&conn).unwrap(),
            DatabaseStamp::new(2, 1, 2)
        );
    }
}
