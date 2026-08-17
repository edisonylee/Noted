//! Desktop portable-record registry and shadow sync journal.
//!
//! Existing domain tables remain authoritative. This module assigns stable
//! public identities, retains immutable portable snapshots, and records future
//! synchronization work without turning the snapshot tables into a second
//! writable source of truth.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

use crate::migrations::{self, DatabaseStamp, MigrationDescriptor, MigrationOutcome};
use crate::portable::{
    canonical_json, canonical_sha256, deterministic_backfill_uuid_v7, is_uuid_v7, new_uuid_v7,
    AuthorityKind, ContextRecordV1, LifecycleState, RecordAuthority, RecordLifecycle, RecordScope,
    ScopeClass,
};

pub const PORTABLE_SCHEMA_VERSION: u32 = 2;
pub const PORTABLE_MIGRATION: MigrationDescriptor<'static> = MigrationDescriptor::new(
    PORTABLE_SCHEMA_VERSION,
    "desktop-portable-note-registry",
    "83b1770f488106ade4bfac158f4bd5580bd7555b0b2dd20e6f7eae5dae3cd552",
);
pub const PORTABLE_SCHEMA_STAMP: DatabaseStamp =
    DatabaseStamp::new(PORTABLE_SCHEMA_VERSION, 1, PORTABLE_SCHEMA_VERSION);

const PORTABLE_SCHEMA: &str = r#"
CREATE TABLE libraries (
  library_id             TEXT PRIMARY KEY,
  authority_generation   INTEGER NOT NULL CHECK(authority_generation > 0),
  purge_generation       INTEGER NOT NULL CHECK(purge_generation >= 0),
  current_key_epoch      INTEGER NOT NULL CHECK(current_key_epoch >= 0),
  owner_device_id        TEXT,
  enrollment_state       TEXT NOT NULL CHECK(enrollment_state IN ('local', 'enrolled')),
  created_at             TEXT NOT NULL
);

CREATE TABLE library_scopes (
  scope_id     TEXT PRIMARY KEY,
  library_id   TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  scope_class  TEXT NOT NULL CHECK(scope_class IN ('work', 'personal', 'unknown')),
  created_at   TEXT NOT NULL,
  UNIQUE(library_id, scope_class)
);

CREATE TABLE portable_devices (
  device_id                 TEXT PRIMARY KEY,
  library_id                TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  device_kind               TEXT NOT NULL,
  display_name              TEXT NOT NULL,
  role                      TEXT NOT NULL CHECK(role IN ('authority', 'replica')),
  enrollment_state          TEXT NOT NULL CHECK(enrollment_state IN ('active', 'revoked')),
  capabilities_json         TEXT NOT NULL,
  public_signing_key        BLOB,
  public_encryption_key     BLOB,
  last_transaction_counter  INTEGER NOT NULL DEFAULT 0 CHECK(last_transaction_counter >= 0),
  created_at                TEXT NOT NULL,
  enrolled_at               TEXT,
  revoked_at                TEXT
);
CREATE UNIQUE INDEX portable_devices_one_local_authority
  ON portable_devices(library_id) WHERE role = 'authority' AND enrollment_state = 'active';

CREATE TABLE portable_records (
  record_id             TEXT PRIMARY KEY,
  library_id            TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  kind                  TEXT NOT NULL,
  record_schema_version INTEGER NOT NULL CHECK(record_schema_version > 0),
  source_table          TEXT NOT NULL,
  source_row_id         INTEGER NOT NULL,
  scope_id              TEXT NOT NULL REFERENCES library_scopes(scope_id) ON DELETE RESTRICT,
  sensitivity           TEXT NOT NULL CHECK(sensitivity IN ('standard', 'sensitive', 'restricted')),
  authority_kind        TEXT NOT NULL CHECK(authority_kind IN ('noted', 'external', 'derived')),
  authority_origin      TEXT,
  write_policy          TEXT NOT NULL CHECK(write_policy IN ('read_write', 'read_only', 'proposal_only')),
  lifecycle_state       TEXT NOT NULL CHECK(lifecycle_state IN ('active', 'trash', 'tombstone')),
  trashed_at            TEXT,
  tombstoned_at         TEXT,
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL,
  UNIQUE(source_table, source_row_id)
);
CREATE INDEX portable_records_library_kind ON portable_records(library_id, kind, record_id);

CREATE TABLE change_transactions (
  transaction_id             TEXT PRIMARY KEY,
  library_id                 TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  device_id                  TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  device_transaction_counter INTEGER NOT NULL CHECK(device_transaction_counter > 0),
  member_count               INTEGER NOT NULL CHECK(member_count > 0),
  manifest_digest            TEXT NOT NULL CHECK(length(manifest_digest) = 64),
  commit_marker              INTEGER NOT NULL CHECK(commit_marker = 1),
  created_at                 TEXT NOT NULL,
  UNIQUE(device_id, device_transaction_counter)
);

CREATE TABLE record_versions (
  version_id        TEXT PRIMARY KEY,
  record_id         TEXT NOT NULL REFERENCES portable_records(record_id) ON DELETE RESTRICT,
  revision          INTEGER NOT NULL CHECK(revision > 0),
  content_hash      TEXT NOT NULL CHECK(length(content_hash) = 64),
  snapshot_json     TEXT NOT NULL,
  source_device_id  TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  transaction_id    TEXT REFERENCES change_transactions(transaction_id) ON DELETE RESTRICT,
  created_at        TEXT NOT NULL,
  accepted_at       TEXT NOT NULL,
  UNIQUE(record_id, version_id),
  UNIQUE(record_id, revision)
);

CREATE TRIGGER record_versions_no_update BEFORE UPDATE ON record_versions BEGIN
  SELECT RAISE(ABORT, 'record_versions is immutable');
END;
CREATE TRIGGER record_versions_no_delete BEFORE DELETE ON record_versions BEGIN
  SELECT RAISE(ABORT, 'record_versions is immutable');
END;

CREATE TABLE record_heads (
  record_id             TEXT PRIMARY KEY REFERENCES portable_records(record_id) ON DELETE RESTRICT,
  accepted_revision     INTEGER NOT NULL CHECK(accepted_revision > 0),
  accepted_version_id   TEXT NOT NULL,
  content_hash          TEXT NOT NULL CHECK(length(content_hash) = 64),
  authority_generation  INTEGER NOT NULL CHECK(authority_generation > 0),
  accepted_at           TEXT NOT NULL,
  FOREIGN KEY(record_id, accepted_version_id)
    REFERENCES record_versions(record_id, version_id) ON DELETE RESTRICT
);

CREATE TABLE change_log (
  local_sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
  mutation_id             TEXT NOT NULL UNIQUE,
  transaction_id          TEXT NOT NULL REFERENCES change_transactions(transaction_id) ON DELETE RESTRICT,
  transaction_member_index INTEGER NOT NULL CHECK(transaction_member_index >= 0),
  record_id               TEXT NOT NULL REFERENCES portable_records(record_id) ON DELETE RESTRICT,
  record_kind             TEXT NOT NULL,
  base_revision           INTEGER NOT NULL CHECK(base_revision >= 0),
  base_version_id         TEXT,
  proposed_revision       INTEGER NOT NULL CHECK(proposed_revision > 0),
  version_id              TEXT NOT NULL REFERENCES record_versions(version_id) ON DELETE RESTRICT,
  mutation_digest         TEXT NOT NULL CHECK(length(mutation_digest) = 64),
  authority_generation    INTEGER NOT NULL CHECK(authority_generation > 0),
  state                   TEXT NOT NULL CHECK(state IN ('accepted_local', 'accepted_remote', 'rejected')),
  created_at              TEXT NOT NULL,
  UNIQUE(transaction_id, transaction_member_index)
);
CREATE TRIGGER change_log_no_update BEFORE UPDATE ON change_log BEGIN
  SELECT RAISE(ABORT, 'change_log is immutable');
END;
CREATE TRIGGER change_log_no_delete BEFORE DELETE ON change_log BEGIN
  SELECT RAISE(ABORT, 'change_log is immutable');
END;

CREATE TABLE sync_outbox (
  mutation_id       TEXT PRIMARY KEY REFERENCES change_log(mutation_id) ON DELETE RESTRICT,
  payload_json      TEXT NOT NULL,
  payload_hash      TEXT NOT NULL CHECK(length(payload_hash) = 64),
  state             TEXT NOT NULL CHECK(state IN ('shadow_pending', 'sending', 'acknowledged', 'failed')),
  attempts          INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
  next_attempt_at   TEXT,
  acknowledged_at  TEXT,
  last_error        TEXT,
  created_at        TEXT NOT NULL
);

-- Media identity and semantic ownership are portable. Filesystem locations
-- are deliberately isolated in media_local_paths and are never serialized in
-- a ContextRecord snapshot or outbox envelope.
CREATE TABLE media_objects (
  media_id         TEXT PRIMARY KEY,
  library_id       TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  media_kind       TEXT NOT NULL CHECK(media_kind IN ('image', 'audio', 'video', 'file', 'unknown')),
  content_hash     TEXT CHECK(content_hash IS NULL OR length(content_hash) = 64),
  byte_size        INTEGER CHECK(byte_size IS NULL OR byte_size >= 0),
  mime_type        TEXT,
  lifecycle_state TEXT NOT NULL CHECK(lifecycle_state IN ('active', 'trash', 'tombstone')),
  created_at       TEXT NOT NULL
);

CREATE TABLE media_refs (
  owner_record_id  TEXT NOT NULL REFERENCES portable_records(record_id) ON DELETE RESTRICT,
  media_id         TEXT NOT NULL REFERENCES media_objects(media_id) ON DELETE RESTRICT,
  semantic_role    TEXT NOT NULL,
  json_pointer     TEXT NOT NULL,
  created_at       TEXT NOT NULL,
  PRIMARY KEY(owner_record_id, media_id, semantic_role, json_pointer)
);
CREATE INDEX media_refs_media ON media_refs(media_id, owner_record_id);

CREATE TABLE media_local_paths (
  mapping_id     TEXT PRIMARY KEY,
  media_id       TEXT NOT NULL REFERENCES media_objects(media_id) ON DELETE RESTRICT,
  device_id      TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  source_table   TEXT NOT NULL,
  source_row_id  INTEGER NOT NULL,
  json_pointer   TEXT NOT NULL,
  local_path     TEXT NOT NULL,
  path_digest    TEXT NOT NULL CHECK(length(path_digest) = 64),
  availability   TEXT NOT NULL CHECK(availability IN ('unknown', 'available', 'missing')),
  created_at     TEXT NOT NULL,
  UNIQUE(device_id, source_table, source_row_id, json_pointer, path_digest)
);
CREATE INDEX media_local_paths_media ON media_local_paths(media_id, device_id);

CREATE TABLE portable_quarantine (
  quarantine_id   TEXT PRIMARY KEY,
  source_table    TEXT NOT NULL,
  source_row_id   INTEGER NOT NULL,
  reason          TEXT NOT NULL,
  details_json    TEXT NOT NULL,
  quarantined_at  TEXT NOT NULL,
  UNIQUE(source_table, source_row_id, reason)
);
CREATE TRIGGER portable_quarantine_no_update BEFORE UPDATE ON portable_quarantine BEGIN
  SELECT RAISE(ABORT, 'portable_quarantine is immutable');
END;
CREATE TRIGGER portable_quarantine_no_delete BEFORE DELETE ON portable_quarantine BEGIN
  SELECT RAISE(ABORT, 'portable_quarantine is immutable');
END;
"#;

#[derive(Debug, Clone)]
struct LocalIdentity {
    library_id: String,
    device_id: String,
    authority_generation: u64,
    scopes: BTreeMap<&'static str, String>,
}

#[derive(Debug)]
struct NoteSource {
    id: i64,
    title: String,
    raw_text: String,
    source: String,
    category_id: Option<i64>,
    created_at: String,
    origin: String,
    source_path: Option<String>,
    image_path: Option<String>,
    filing_context: Option<String>,
    trashed_at: Option<String>,
    meeting_count: i64,
}

#[derive(Debug)]
struct PreparedVersion {
    record: ContextRecordV1,
    snapshot_json: String,
    base_revision: u64,
    base_version_id: Option<String>,
    mutation_id: String,
    mutation_json: String,
    mutation_digest: String,
}

/// Apply the ordered desktop v2 migration. Its body and compatibility stamp are
/// committed together by the shared migration runner.
pub fn apply_portable_migration(conn: &mut Connection) -> Result<MigrationOutcome> {
    migrations::apply_migration(
        conn,
        PORTABLE_MIGRATION,
        PORTABLE_SCHEMA_STAMP,
        env!("CARGO_PKG_VERSION"),
        install_and_backfill,
    )
}

/// Verify the durable invariants on every reopen, including immutable history
/// triggers and accepted-head/version referential integrity.
pub fn verify_portable_schema(conn: &Connection) -> Result<()> {
    for table in [
        "libraries",
        "library_scopes",
        "portable_devices",
        "portable_records",
        "record_versions",
        "record_heads",
        "change_transactions",
        "change_log",
        "sync_outbox",
        "media_objects",
        "media_refs",
        "media_local_paths",
        "portable_quarantine",
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("portable schema is missing required table '{table}'");
        }
    }
    for trigger in [
        "record_versions_no_update",
        "record_versions_no_delete",
        "change_log_no_update",
        "change_log_no_delete",
        "portable_quarantine_no_update",
        "portable_quarantine_no_delete",
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1)",
            [trigger],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("portable schema is missing required trigger '{trigger}'");
        }
    }
    if conn
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        bail!("portable schema foreign-key validation failed");
    }

    let identity = local_identity(conn)?;
    if !is_uuid_v7(&identity.library_id) || !is_uuid_v7(&identity.device_id) {
        bail!("portable library and device identities must be UUIDv7");
    }

    let invalid_records: i64 = conn.query_row(
        "SELECT COUNT(*) FROM portable_records
         WHERE length(record_id) != 36 OR record_schema_version < 1
            OR authority_kind = 'external' AND write_policy = 'read_write'",
        [],
        |row| row.get(0),
    )?;
    if invalid_records != 0 {
        bail!("portable registry contains {invalid_records} invalid record rows");
    }

    let divergent_heads: i64 = conn.query_row(
        "SELECT COUNT(*) FROM record_heads h
         LEFT JOIN record_versions v
           ON v.record_id = h.record_id
          AND v.version_id = h.accepted_version_id
          AND v.revision = h.accepted_revision
          AND v.content_hash = h.content_hash
         WHERE v.version_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    if divergent_heads != 0 {
        bail!("portable registry contains {divergent_heads} invalid accepted heads");
    }

    let media_rows = {
        let mut statement = conn.prepare(
            "SELECT o.media_id, p.local_path, p.path_digest
             FROM media_objects o
             LEFT JOIN media_local_paths p ON p.media_id = o.media_id
             ORDER BY o.media_id, p.mapping_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (media_id, local_path, path_digest) in media_rows {
        if !is_uuid_v7(&media_id) {
            bail!("portable media identity '{media_id}' is not UUIDv7");
        }
        if let (Some(local_path), Some(path_digest)) = (local_path, path_digest) {
            if local_path.trim().is_empty() || canonical_sha256(&json!(local_path)) != path_digest {
                bail!("local media mapping for '{media_id}' has an invalid path digest");
            }
        }
    }
    verify_portable_json_column(conn, "record_versions", "snapshot_json")?;
    verify_portable_json_column(conn, "sync_outbox", "payload_json")?;
    Ok(())
}

fn verify_portable_json_column(conn: &Connection, table: &str, column: &str) -> Result<()> {
    let sql = format!("SELECT {column} FROM {table}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let raw = row?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("{table}.{column} contains invalid JSON"))?;
        validate_portable_value(&value, "")
            .with_context(|| format!("{table}.{column} contains local-only media state"))?;
    }
    Ok(())
}

/// Serialize and journal one ordinary Noted-owned note after its authoritative
/// domain mutation. Callers must invoke this before committing their outer
/// SQLite transaction. Brain mirrors and meeting projections never pass this
/// writer boundary.
pub fn journal_note_write(conn: &Connection, note_id: i64, occurred_at: &str) -> Result<()> {
    let identity = local_identity(conn)?;
    let note = read_note_source(conn, note_id)?;
    match classify_note(&note) {
        NoteClass::Ordinary => {}
        NoteClass::Brain => bail!(
            "note {note_id} is an externally authoritative Brain mirror; use a write-back proposal"
        ),
        NoteClass::MeetingProjection => bail!(
            "note {note_id} is a meeting search projection and cannot be journaled independently"
        ),
        NoteClass::MeetingOrphan => {
            bail!("note {note_id} claims meeting origin but has no canonical meeting owner")
        }
        NoteClass::UnknownOrigin => bail!(
            "note {note_id} has unknown authority origin '{}'",
            note.origin
        ),
    }

    let occurred_at = normalize_utc(occurred_at)?;
    let mut prepared = prepare_note_dependencies(conn, &identity, &note, &occurred_at)?;
    let registry = ensure_live_note_registry(conn, &identity, &note, &occurred_at)?;
    prepared.push(prepare_note_version(
        conn,
        &identity,
        &registry,
        &note,
        &occurred_at,
        false,
    )?);
    persist_shadow_transaction(conn, &identity, prepared, &occurred_at)
}

/// Serialize and journal one Noted-owned category after its authoritative
/// domain mutation. The category row and this shadow mutation must be enclosed
/// by the caller's outer SQLite transaction so any later failure rolls both
/// back together. Physical deletion is deliberately unsupported: callers must
/// retain the source row until a synchronized lifecycle transition exists.
pub fn journal_category_write(
    conn: &Connection,
    category_id: i64,
    occurred_at: &str,
) -> Result<()> {
    let identity = local_identity(conn)?;
    let occurred_at = normalize_utc(occurred_at)?;
    let prepared = prepare_live_category(conn, &identity, category_id, &occurred_at)?
        .into_iter()
        .collect();
    persist_shadow_transaction(conn, &identity, prepared, &occurred_at)
}

/// Serialize and journal one Noted-owned folder after its authoritative domain
/// mutation. Missing parent records are emitted before their children, and any
/// changed siblings in the current or prior parent are included so a normalized
/// move/reorder remains one grouped shadow transaction. The caller must invoke
/// this before committing its outer SQLite transaction; physical deletion is
/// not a valid folder mutation until synchronized lifecycle and purge semantics
/// exist.
pub fn journal_folder_write(conn: &Connection, folder_id: i64, occurred_at: &str) -> Result<()> {
    let identity = local_identity(conn)?;
    let occurred_at = normalize_utc(occurred_at)?;
    let mut prepared = Vec::new();
    for related_id in related_folder_write_ids(conn, folder_id)? {
        prepare_live_folder_chain(
            conn,
            &identity,
            related_id,
            &occurred_at,
            &mut HashSet::new(),
            &mut prepared,
        )?;
    }
    let mut emitted = HashSet::new();
    prepared.retain(|item| emitted.insert(item.record.record_id.clone()));
    persist_shadow_transaction(conn, &identity, prepared, &occurred_at)
}

fn related_folder_write_ids(conn: &Connection, folder_id: i64) -> Result<Vec<i64>> {
    let row = read_folder_source(conn, folder_id)?;
    let mut parent_ids = BTreeSet::new();
    parent_ids.insert(row.parent_id);
    if let Some(previous_parent_id) = accepted_folder_parent_source_id(conn, folder_id)? {
        parent_ids.insert(Some(previous_parent_id));
    }

    let mut folder_ids = BTreeSet::from([folder_id]);
    for parent_id in parent_ids {
        let mut statement = conn.prepare(
            "SELECT id FROM note_folders
             WHERE parent_id IS ?1
             ORDER BY position, name COLLATE NOCASE, id",
        )?;
        let rows = statement.query_map([parent_id], |row| row.get::<_, i64>(0))?;
        for row in rows {
            folder_ids.insert(row?);
        }
    }
    Ok(folder_ids.into_iter().collect())
}

fn accepted_folder_parent_source_id(conn: &Connection, folder_id: i64) -> Result<Option<i64>> {
    let snapshot = conn
        .query_row(
            "SELECT v.snapshot_json
             FROM portable_records p
             JOIN record_heads h ON h.record_id = p.record_id
             JOIN record_versions v ON v.version_id = h.accepted_version_id
             WHERE p.source_table = 'note_folders' AND p.source_row_id = ?1",
            [folder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let snapshot: Value = serde_json::from_str(&snapshot)
        .with_context(|| format!("folder {folder_id} has an invalid accepted snapshot"))?;
    let parent_id = snapshot
        .pointer("/content/parentId")
        .context("accepted folder snapshot is missing content.parentId")?;
    if parent_id.is_null() {
        return Ok(None);
    }
    let parent_record_id = parent_id
        .as_str()
        .context("accepted folder parentId must be a portable UUID")?;
    if !is_uuid_v7(parent_record_id) {
        bail!("accepted folder parentId is not a UUIDv7 record identity");
    }
    conn.query_row(
        "SELECT source_row_id FROM portable_records
         WHERE record_id = ?1 AND source_table = 'note_folders' AND kind = 'folder'",
        [parent_record_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()?
    .ok_or_else(|| anyhow::anyhow!("accepted folder parentId has no local folder mapping"))
    .map(Some)
}

fn install_and_backfill(tx: &Transaction<'_>) -> Result<()> {
    let schema_exists: bool = tx.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'libraries'
         )",
        [],
        |row| row.get(0),
    )?;
    if !schema_exists {
        tx.execute_batch(PORTABLE_SCHEMA)
            .context("create desktop portable registry")?;
    }
    let identity = ensure_local_identity(tx)?;
    backfill_categories(tx, &identity)?;
    backfill_folders(tx, &identity)?;
    backfill_notes(tx, &identity)?;
    inventory_unowned_entry_media(tx, &identity)?;
    inventory_unowned_meeting_document_media(tx, &identity)?;
    tx.execute(
        "INSERT INTO app_metadata(key, value) VALUES ('sync_shadow_outbox_mode', 'enabled')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    Ok(())
}

fn ensure_local_identity(conn: &Connection) -> Result<LocalIdentity> {
    let library_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM libraries", [], |row| row.get(0))?;
    if library_count > 1 {
        bail!("desktop portable schema must contain exactly one local library");
    }

    if library_count == 0 {
        let (created_at, timestamp_ms, fingerprint) = legacy_library_fingerprint(conn)?;
        let library_id =
            deterministic_backfill_uuid_v7(timestamp_ms, "desktop-library", &fingerprint);
        let device_id = new_uuid_v7();
        conn.execute(
            "INSERT INTO libraries
             (library_id, authority_generation, purge_generation, current_key_epoch,
              owner_device_id, enrollment_state, created_at)
             VALUES (?1, 1, 0, 0, NULL, 'local', ?2)",
            params![library_id, created_at],
        )?;
        conn.execute(
            "INSERT INTO portable_devices
             (device_id, library_id, device_kind, display_name, role,
              enrollment_state, capabilities_json, created_at, enrolled_at)
             VALUES (?1, ?2, 'macos', 'This Mac', 'authority', 'active', ?3, ?4, ?4)",
            params![
                device_id,
                library_id,
                canonical_json(&json!({
                    "contextRecordVersions": ["noted.context-record.v1"],
                    "recordKinds": {"note": 1, "category": 1, "folder": 1},
                    "writerVersion": PORTABLE_SCHEMA_VERSION
                })),
                created_at
            ],
        )?;
        conn.execute(
            "UPDATE libraries SET owner_device_id = ?1 WHERE library_id = ?2",
            params![device_id, library_id],
        )?;
        for scope_class in ["work", "personal", "unknown"] {
            let scope_id = deterministic_backfill_uuid_v7(
                timestamp_ms,
                "desktop-library-scope",
                &format!("{library_id}:{scope_class}"),
            );
            conn.execute(
                "INSERT INTO library_scopes(scope_id, library_id, scope_class, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![scope_id, library_id, scope_class, created_at],
            )?;
        }
    }
    local_identity(conn)
}

fn local_identity(conn: &Connection) -> Result<LocalIdentity> {
    let (library_id, authority_generation): (String, i64) = conn
        .query_row(
            "SELECT library_id, authority_generation FROM libraries",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("portable library identity is not initialized")?;
    let device_id: String = conn
        .query_row(
            "SELECT device_id FROM portable_devices
             WHERE library_id = ?1 AND role = 'authority' AND enrollment_state = 'active'",
            [&library_id],
            |row| row.get(0),
        )
        .context("portable authority device identity is not initialized")?;
    let mut scopes = BTreeMap::new();
    let mut statement =
        conn.prepare("SELECT scope_class, scope_id FROM library_scopes WHERE library_id = ?1")?;
    let rows = statement.query_map([&library_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (class, id) = row?;
        let key = match class.as_str() {
            "work" => "work",
            "personal" => "personal",
            "unknown" => "unknown",
            _ => bail!("portable library contains unsupported scope class '{class}'"),
        };
        scopes.insert(key, id);
    }
    if scopes.len() != 3 {
        bail!("portable library must have work, personal, and unknown scopes");
    }
    Ok(LocalIdentity {
        library_id,
        device_id,
        authority_generation: u64::try_from(authority_generation)
            .context("authority generation is negative")?,
        scopes,
    })
}

fn legacy_library_fingerprint(conn: &Connection) -> Result<(String, u64, String)> {
    let mut inventory = Vec::new();
    let mut earliest: Option<String> = None;
    for (table, select) in [
        (
            "categories",
            "SELECT id, name, created_at FROM categories ORDER BY id",
        ),
        (
            "note_folders",
            "SELECT id, name, created_at FROM note_folders ORDER BY id",
        ),
        (
            "notes",
            "SELECT id, raw_text, created_at FROM notes ORDER BY id",
        ),
    ] {
        let mut statement = conn.prepare(select)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, content, raw_created_at) = row?;
            let created_at = normalize_utc(&raw_created_at)?;
            if earliest
                .as_deref()
                .is_none_or(|current| created_at.as_str() < current)
            {
                earliest = Some(created_at.clone());
            }
            inventory.push(json!({
                "table": table,
                "legacyKey": id.to_string(),
                "contentHash": canonical_sha256(&json!(content)),
                "createdAt": created_at,
            }));
        }
    }
    let created_at = earliest.unwrap_or_else(now_utc);
    let timestamp_ms = timestamp_ms(&created_at)?;
    let fingerprint = canonical_sha256(&Value::Array(inventory));
    Ok((created_at, timestamp_ms, fingerprint))
}

fn backfill_categories(conn: &Connection, identity: &LocalIdentity) -> Result<()> {
    let rows = {
        let mut statement = conn.prepare(
            "SELECT id, name, description, schema_json, created_at
             FROM categories ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (id, name, description, raw_schema, raw_created_at) in rows {
        let created_at = normalize_utc(&raw_created_at)?;
        let schema: Value = serde_json::from_str(&raw_schema)
            .with_context(|| format!("category {id} contains invalid schema JSON"))?;
        let record_id = deterministic_backfill_uuid_v7(
            timestamp_ms(&created_at)?,
            "desktop-category",
            &id.to_string(),
        );
        insert_registry(
            conn,
            identity,
            &record_id,
            "category",
            "categories",
            id,
            "unknown",
            "standard",
            "noted",
            Some("noted"),
            "read_write",
            "active",
            None,
            &created_at,
            &created_at,
        )?;
        insert_backfill_version(
            conn,
            identity,
            &record_id,
            "category",
            &created_at,
            &created_at,
            "unknown",
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: Some("noted".to_string()),
            },
            json!({"name": name, "description": description, "schema": schema}),
            json!({"source": "legacy_desktop", "migration": "portable-v2"}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )?;
    }
    Ok(())
}

fn backfill_folders(conn: &Connection, identity: &LocalIdentity) -> Result<()> {
    let rows = folder_rows(conn)?;
    for row in &rows {
        let created_at = normalize_utc(&row.created_at)?;
        let record_id = deterministic_backfill_uuid_v7(
            timestamp_ms(&created_at)?,
            "desktop-note-folder",
            &row.id.to_string(),
        );
        let scope_class = folder_scope_class(conn, row.id)?;
        insert_registry(
            conn,
            identity,
            &record_id,
            "folder",
            "note_folders",
            row.id,
            scope_class,
            "standard",
            "noted",
            Some("noted"),
            "read_write",
            "active",
            None,
            &created_at,
            &created_at,
        )?;
    }
    for row in rows {
        let created_at = normalize_utc(&row.created_at)?;
        let record_id = registry_id(conn, "note_folders", row.id)?;
        let parent_id = row
            .parent_id
            .map(|id| registry_id(conn, "note_folders", id))
            .transpose()?;
        let scope_class = folder_scope_class(conn, row.id)?;
        insert_backfill_version(
            conn,
            identity,
            &record_id,
            "folder",
            &created_at,
            &created_at,
            scope_class,
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: Some("noted".to_string()),
            },
            json!({
                "name": row.name,
                "folderType": row.kind,
                "parentId": parent_id,
                "autoRule": row.auto_rule,
                "position": row.position,
            }),
            json!({"source": "legacy_desktop", "migration": "portable-v2"}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct FolderSource {
    id: i64,
    parent_id: Option<i64>,
    name: String,
    kind: String,
    auto_rule: String,
    position: i64,
    created_at: String,
}

#[derive(Debug)]
struct CategorySource {
    id: i64,
    name: String,
    description: String,
    schema: Value,
    created_at: String,
}

fn folder_rows(conn: &Connection) -> Result<Vec<FolderSource>> {
    let mut statement = conn.prepare(
        "SELECT id, parent_id, name, kind, auto_rule, position, created_at
         FROM note_folders ORDER BY id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(FolderSource {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                auto_rule: row.get(4)?,
                position: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn backfill_notes(conn: &Connection, identity: &LocalIdentity) -> Result<()> {
    let note_ids = {
        let mut statement = conn.prepare("SELECT id FROM notes ORDER BY id")?;
        let rows = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for note_id in note_ids {
        let note = read_note_source(conn, note_id)?;
        match classify_note(&note) {
            NoteClass::MeetingProjection => {
                if note.meeting_count > 1 {
                    quarantine_note(
                        conn,
                        &note,
                        "meeting_projection_multiple_owners",
                        json!({"ownerCount": note.meeting_count}),
                    )?;
                }
            }
            NoteClass::MeetingOrphan => quarantine_note(
                conn,
                &note,
                "meeting_origin_without_owner",
                json!({"action": "excluded_from_portable_notes"}),
            )?,
            NoteClass::UnknownOrigin => quarantine_note(
                conn,
                &note,
                "unknown_note_authority",
                json!({"origin": note.origin}),
            )?,
            NoteClass::Ordinary | NoteClass::Brain => {
                let created_at = normalize_utc(&note.created_at)?;
                let record_id = deterministic_backfill_uuid_v7(
                    timestamp_ms(&created_at)?,
                    "desktop-note",
                    &note.id.to_string(),
                );
                let authority = note_authority(&note);
                let scope_class = note_scope_class(&note);
                let updated_at = note
                    .trashed_at
                    .as_deref()
                    .map(normalize_utc)
                    .transpose()?
                    .unwrap_or_else(|| created_at.clone());
                insert_registry(
                    conn,
                    identity,
                    &record_id,
                    "note",
                    "notes",
                    note.id,
                    scope_class,
                    note_sensitivity(&note),
                    authority.0,
                    Some(authority.1),
                    authority.2,
                    if note.trashed_at.is_some() {
                        "trash"
                    } else {
                        "active"
                    },
                    note.trashed_at
                        .as_deref()
                        .map(normalize_utc)
                        .transpose()?
                        .as_deref(),
                    &created_at,
                    &updated_at,
                )?;
                let registry = registry_record(conn, "notes", note.id)?;
                let already_backfilled: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM record_heads WHERE record_id = ?1)",
                    [&registry.record_id],
                    |row| row.get(0),
                )?;
                if already_backfilled {
                    continue;
                }
                let prepared =
                    prepare_note_version(conn, identity, &registry, &note, &updated_at, true)?;
                persist_backfill_prepared(conn, identity, prepared)?;
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RegistryRecord {
    record_id: String,
    kind: String,
    scope_id: String,
    scope_class: String,
    sensitivity: String,
    authority_kind: String,
    authority_origin: Option<String>,
    write_policy: String,
    lifecycle_state: String,
    created_at: String,
}

fn prepare_note_dependencies(
    conn: &Connection,
    identity: &LocalIdentity,
    note: &NoteSource,
    occurred_at: &str,
) -> Result<Vec<PreparedVersion>> {
    let category_ids = {
        let mut statement = conn.prepare(
            "SELECT category_id FROM entries WHERE note_id = ?1
             UNION SELECT category_id FROM notes
             WHERE id = ?1 AND category_id IS NOT NULL
             ORDER BY category_id",
        )?;
        let rows = statement
            .query_map([note.id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut prepared = Vec::new();
    for category_id in category_ids {
        if let Some(version) = prepare_live_category(conn, identity, category_id, occurred_at)? {
            prepared.push(version);
        }
    }

    let folder_id = conn
        .query_row(
            "SELECT folder_id FROM note_folder_items WHERE note_id = ?1
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [note.id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(folder_id) = folder_id {
        prepare_live_folder_chain(
            conn,
            identity,
            folder_id,
            occurred_at,
            &mut HashSet::new(),
            &mut prepared,
        )?;
    }
    Ok(prepared)
}

fn prepare_live_category(
    conn: &Connection,
    identity: &LocalIdentity,
    category_id: i64,
    occurred_at: &str,
) -> Result<Option<PreparedVersion>> {
    let source = read_category_source(conn, category_id)?;
    let created_at = normalize_utc(&source.created_at)?;
    let registry = if let Some(record) = registry_record_optional(conn, "categories", category_id)?
    {
        record
    } else {
        let record_id = new_uuid_v7();
        insert_registry(
            conn,
            identity,
            &record_id,
            "category",
            "categories",
            category_id,
            "unknown",
            "standard",
            "noted",
            Some("noted"),
            "read_write",
            "active",
            None,
            &created_at,
            occurred_at,
        )?;
        registry_record(conn, "categories", category_id)?
    };
    enforce_live_noted_registry(&registry, "category", "categories", source.id)?;
    prepare_live_dependency_version(
        conn,
        identity,
        &registry,
        "category",
        occurred_at,
        json!({
            "name": source.name,
            "description": source.description,
            "schema": source.schema,
        }),
    )
}

fn prepare_live_folder_chain(
    conn: &Connection,
    identity: &LocalIdentity,
    folder_id: i64,
    occurred_at: &str,
    visited: &mut HashSet<i64>,
    prepared: &mut Vec<PreparedVersion>,
) -> Result<()> {
    if !visited.insert(folder_id) {
        bail!("note folder hierarchy contains a cycle at row {folder_id}");
    }
    let row = read_folder_source(conn, folder_id)?;
    if let Some(parent_id) = row.parent_id {
        prepare_live_folder_chain(conn, identity, parent_id, occurred_at, visited, prepared)?;
    }
    let created_at = normalize_utc(&row.created_at)?;
    let scope = folder_scope_class(conn, row.id)?;
    let registry = if let Some(record) = registry_record_optional(conn, "note_folders", row.id)? {
        record
    } else {
        let record_id = new_uuid_v7();
        insert_registry(
            conn,
            identity,
            &record_id,
            "folder",
            "note_folders",
            row.id,
            scope,
            "standard",
            "noted",
            Some("noted"),
            "read_write",
            "active",
            None,
            &created_at,
            occurred_at,
        )?;
        registry_record(conn, "note_folders", row.id)?
    };
    enforce_live_noted_registry(&registry, "folder", "note_folders", row.id)?;
    if registry.scope_class != scope {
        bail!(
            "folder {} cannot change portable scope from '{}' to '{}' without an explicit scope transition",
            row.id,
            registry.scope_class,
            scope
        );
    }
    let parent_id = row
        .parent_id
        .map(|id| registry_id(conn, "note_folders", id))
        .transpose()?;
    if let Some(version) = prepare_live_dependency_version(
        conn,
        identity,
        &registry,
        "folder",
        occurred_at,
        json!({
            "name": row.name,
            "folderType": row.kind,
            "parentId": parent_id,
            "autoRule": row.auto_rule,
            "position": row.position,
        }),
    )? {
        prepared.push(version);
    }
    Ok(())
}

fn read_category_source(conn: &Connection, category_id: i64) -> Result<CategorySource> {
    let row = conn
        .query_row(
            "SELECT id, name, description, schema_json, created_at
             FROM categories WHERE id = ?1",
            [category_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((id, name, description, raw_schema, created_at)) = row else {
        return missing_live_source_error(conn, "category", "categories", category_id);
    };
    let schema = serde_json::from_str(&raw_schema)
        .with_context(|| format!("category {category_id} contains invalid schema JSON"))?;
    Ok(CategorySource {
        id,
        name,
        description,
        schema,
        created_at,
    })
}

fn read_folder_source(conn: &Connection, folder_id: i64) -> Result<FolderSource> {
    let row = conn
        .query_row(
            "SELECT id, parent_id, name, kind, auto_rule, position, created_at
             FROM note_folders WHERE id = ?1",
            [folder_id],
            |row| {
                Ok(FolderSource {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    auto_rule: row.get(4)?,
                    position: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()?;
    match row {
        Some(row) => Ok(row),
        None => missing_live_source_error(conn, "folder", "note_folders", folder_id),
    }
}

fn missing_live_source_error<T>(
    conn: &Connection,
    kind: &str,
    source_table: &str,
    source_row_id: i64,
) -> Result<T> {
    if registry_record_optional(conn, source_table, source_row_id)?.is_some() {
        bail!(
            "{kind} {source_row_id} was physically deleted; portable {kind} records require a synchronized lifecycle transition"
        );
    }
    bail!("{kind} {source_row_id} does not exist")
}

fn enforce_live_noted_registry(
    registry: &RegistryRecord,
    expected_kind: &str,
    source_table: &str,
    source_row_id: i64,
) -> Result<()> {
    if registry.kind != expected_kind {
        bail!(
            "portable identity for {source_table} row {source_row_id} is registered as '{}' instead of '{expected_kind}'",
            registry.kind
        );
    }
    if registry.authority_kind != "noted"
        || registry.authority_origin.as_deref() != Some("noted")
        || registry.write_policy != "read_write"
    {
        bail!("portable {expected_kind} {source_row_id} is not writable under Noted authority");
    }
    if registry.lifecycle_state != "active" {
        bail!(
            "portable {expected_kind} {source_row_id} is not active; physical deletion is not a lifecycle transition"
        );
    }
    Ok(())
}

fn prepare_live_dependency_version(
    conn: &Connection,
    identity: &LocalIdentity,
    registry: &RegistryRecord,
    kind: &str,
    occurred_at: &str,
    content: Value,
) -> Result<Option<PreparedVersion>> {
    let current = conn
        .query_row(
            "SELECT accepted_revision, accepted_version_id, content_hash
             FROM record_heads WHERE record_id = ?1",
            [&registry.record_id],
            |row| {
                Ok((
                    u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if current
        .as_ref()
        .is_some_and(|(_, _, hash)| hash == &canonical_sha256(&content))
    {
        return Ok(None);
    }
    let (base_revision, base_version_id) = current
        .map(|(revision, version_id, _)| (revision, Some(version_id)))
        .unwrap_or((0, None));
    let revision = base_revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("{kind} revision space is exhausted"))?;
    let record = ContextRecordV1::new(
        identity.library_id.clone(),
        registry.record_id.clone(),
        kind.to_string(),
        1,
        revision,
        new_uuid_v7(),
        registry.created_at.clone(),
        normalize_utc(occurred_at)?,
        None,
        RecordScope {
            scope_id: registry.scope_id.clone(),
            class: scope_class(&registry.scope_class)?,
        },
        registry.sensitivity.clone(),
        RecordAuthority {
            kind: AuthorityKind::Noted,
            origin: registry.authority_origin.clone(),
        },
        content,
        json!({"source": "desktop_domain_write"}),
        RecordLifecycle {
            state: LifecycleState::Active,
            trashed_at: None,
            tombstoned_at: None,
        },
    )
    .map_err(anyhow::Error::msg)?;
    Ok(Some(prepare_version(
        record,
        base_revision,
        base_version_id,
        false,
    )?))
}

fn ensure_live_note_registry(
    conn: &Connection,
    identity: &LocalIdentity,
    note: &NoteSource,
    occurred_at: &str,
) -> Result<RegistryRecord> {
    if let Some(record) = registry_record_optional(conn, "notes", note.id)? {
        return Ok(record);
    }
    let created_at = normalize_utc(&note.created_at)?;
    let record_id = new_uuid_v7();
    let scope_class = note_scope_class(note);
    insert_registry(
        conn,
        identity,
        &record_id,
        "note",
        "notes",
        note.id,
        scope_class,
        note_sensitivity(note),
        "noted",
        Some("capture"),
        "read_write",
        if note.trashed_at.is_some() {
            "trash"
        } else {
            "active"
        },
        note.trashed_at
            .as_deref()
            .map(normalize_utc)
            .transpose()?
            .as_deref(),
        &created_at,
        occurred_at,
    )?;
    registry_record(conn, "notes", note.id)
}

fn prepare_note_version(
    conn: &Connection,
    identity: &LocalIdentity,
    registry: &RegistryRecord,
    note: &NoteSource,
    updated_at: &str,
    deterministic: bool,
) -> Result<PreparedVersion> {
    let (base_revision, base_version_id): (u64, Option<String>) = conn
        .query_row(
            "SELECT accepted_revision, accepted_version_id FROM record_heads WHERE record_id = ?1",
            [&registry.record_id],
            |row| {
                Ok((
                    u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                    row.get(1)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((0, None));
    let revision = base_revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("note revision space is exhausted"))?;
    let version_id = if deterministic {
        deterministic_backfill_uuid_v7(
            timestamp_ms(&registry.created_at)?,
            "desktop-note-version",
            &format!("{}:{revision}", registry.record_id),
        )
    } else {
        new_uuid_v7()
    };
    let authority_kind = match registry.authority_kind.as_str() {
        "noted" => AuthorityKind::Noted,
        "external" => AuthorityKind::External,
        "derived" => AuthorityKind::Derived,
        other => bail!("unsupported registry authority '{other}'"),
    };
    let lifecycle = note.trashed_at.as_deref().map(normalize_utc).transpose()?;
    let content = note_content(conn, identity, registry, note)?;
    let provenance = if matches!(authority_kind, AuthorityKind::External) {
        let source_id = deterministic_backfill_uuid_v7(
            timestamp_ms(&registry.created_at)?,
            "desktop-external-source",
            &format!(
                "{}:{}",
                note.origin,
                note.source_path.as_deref().unwrap_or("")
            ),
        );
        json!({"source": "registered_brain_mirror", "sourceId": source_id})
    } else {
        json!({"source": note.source})
    };
    let record = ContextRecordV1::new(
        identity.library_id.clone(),
        registry.record_id.clone(),
        "note".to_string(),
        1,
        revision,
        version_id,
        registry.created_at.clone(),
        normalize_utc(updated_at)?,
        None,
        RecordScope {
            scope_id: registry.scope_id.clone(),
            class: scope_class(&registry.scope_class)?,
        },
        registry.sensitivity.clone(),
        RecordAuthority {
            kind: authority_kind,
            origin: registry.authority_origin.clone(),
        },
        content,
        provenance,
        RecordLifecycle {
            state: if lifecycle.is_some() {
                LifecycleState::Trash
            } else {
                LifecycleState::Active
            },
            trashed_at: lifecycle,
            tombstoned_at: None,
        },
    )
    .map_err(anyhow::Error::msg)?;
    prepare_version(record, base_revision, base_version_id, deterministic)
}

fn note_content(
    conn: &Connection,
    identity: &LocalIdentity,
    registry: &RegistryRecord,
    note: &NoteSource,
) -> Result<Value> {
    let primary_category_id = note
        .category_id
        .map(|id| registry_id(conn, "categories", id))
        .transpose()?;
    let folder_id = conn
        .query_row(
            "SELECT p.record_id
             FROM note_folder_items i
             JOIN portable_records p
               ON p.source_table = 'note_folders' AND p.source_row_id = i.folder_id
             WHERE i.note_id = ?1
             ORDER BY i.created_at DESC, i.rowid DESC LIMIT 1",
            [note.id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut media = Vec::new();
    if let Some(path) = note
        .image_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        media.push(inventory_media_reference(
            conn,
            identity,
            Some(&registry.record_id),
            "notes",
            note.id,
            "/image_path",
            "/content/media/0",
            "note_image",
            path,
            &note.created_at,
        )?);
    }

    let entry_rows = {
        let mut statement = conn.prepare(
            "SELECT e.id, p.record_id, e.data_json, e.event_date, e.created_at
             FROM entries e
             JOIN portable_records p
               ON p.source_table = 'categories' AND p.source_row_id = e.category_id
             WHERE e.note_id = ?1 ORDER BY e.id",
        )?;
        let rows = statement
            .query_map([note.id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut entries = Vec::new();
    for (entry_index, (entry_id, category_id, raw_data, event_date, created_at)) in
        entry_rows.into_iter().enumerate()
    {
        let raw_data = serde_json::from_str::<Value>(&raw_data)
            .unwrap_or_else(|_| json!({"encoding": "raw_json", "raw": raw_data}));
        let portable_prefix = format!("/content/entries/{entry_index}/data");
        let data = sanitize_media_value(
            conn,
            identity,
            Some(&registry.record_id),
            "entries",
            entry_id,
            "",
            &portable_prefix,
            raw_data,
            &created_at,
            &mut media,
        )?;
        entries.push(json!({
            "categoryId": category_id,
            "data": data,
            "eventDate": event_date,
            "createdAt": normalize_utc(&created_at)?,
        }));
    }
    media.sort_by(|left, right| {
        left.get("jsonPointer")
            .and_then(Value::as_str)
            .cmp(&right.get("jsonPointer").and_then(Value::as_str))
            .then_with(|| {
                left.get("mediaId")
                    .and_then(Value::as_str)
                    .cmp(&right.get("mediaId").and_then(Value::as_str))
            })
    });
    media.dedup();
    Ok(json!({
        "title": note.title,
        "body": note.raw_text,
        "captureSource": note.source,
        "filingContext": note.filing_context,
        "primaryCategoryId": primary_category_id,
        "folderId": folder_id,
        "entries": entries,
        "media": media,
    }))
}

/// Register one machine-local reference while returning only a portable media
/// descriptor. The media identity is deterministic for legacy inventory, but
/// the path itself is confined to `media_local_paths`.
#[allow(clippy::too_many_arguments)]
fn inventory_media_reference(
    conn: &Connection,
    identity: &LocalIdentity,
    owner_record_id: Option<&str>,
    source_table: &str,
    source_row_id: i64,
    source_pointer: &str,
    portable_pointer: &str,
    semantic_role: &str,
    local_path: &str,
    raw_created_at: &str,
) -> Result<Value> {
    let local_path = local_path.trim();
    if local_path.is_empty() {
        bail!("cannot inventory an empty media path");
    }
    let created_at = normalize_utc(raw_created_at)?;
    let path_digest = canonical_sha256(&json!(local_path));
    let media_id = deterministic_backfill_uuid_v7(
        timestamp_ms(&created_at)?,
        "desktop-media-object",
        &format!("{source_table}:{source_row_id}:{source_pointer}:{path_digest}"),
    );
    let (media_kind, mime_type) = infer_media_type(local_path);
    conn.execute(
        "INSERT OR IGNORE INTO media_objects
         (media_id, library_id, media_kind, content_hash, byte_size, mime_type,
          lifecycle_state, created_at)
         VALUES (?1, ?2, ?3, NULL, NULL, ?4, 'active', ?5)",
        params![
            media_id,
            identity.library_id,
            media_kind,
            mime_type,
            created_at
        ],
    )?;
    let (stored_library, stored_kind): (String, String) = conn.query_row(
        "SELECT library_id, media_kind FROM media_objects WHERE media_id = ?1",
        [&media_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored_library != identity.library_id || stored_kind != media_kind {
        bail!("portable media identity drift for {source_table} row {source_row_id}");
    }

    let mapping_id = deterministic_backfill_uuid_v7(
        timestamp_ms(&created_at)?,
        "desktop-media-local-path",
        &format!(
            "{}:{media_id}:{source_table}:{source_row_id}:{source_pointer}:{path_digest}",
            identity.device_id
        ),
    );
    conn.execute(
        "INSERT OR IGNORE INTO media_local_paths
         (mapping_id, media_id, device_id, source_table, source_row_id,
          json_pointer, local_path, path_digest, availability, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'unknown', ?9)",
        params![
            mapping_id,
            media_id,
            identity.device_id,
            source_table,
            source_row_id,
            source_pointer,
            local_path,
            path_digest,
            created_at,
        ],
    )?;
    if let Some(owner_record_id) = owner_record_id {
        conn.execute(
            "INSERT OR IGNORE INTO media_refs
             (owner_record_id, media_id, semantic_role, json_pointer, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                owner_record_id,
                media_id,
                semantic_role,
                portable_pointer,
                created_at
            ],
        )?;
    }
    Ok(json!({
        "mediaId": media_id,
        "kind": media_kind,
        "role": semantic_role,
        "jsonPointer": portable_pointer,
        "contentHash": Value::Null,
        "byteSize": Value::Null,
    }))
}

#[allow(clippy::too_many_arguments)]
fn sanitize_media_value(
    conn: &Connection,
    identity: &LocalIdentity,
    owner_record_id: Option<&str>,
    source_table: &str,
    source_row_id: i64,
    source_pointer: &str,
    portable_pointer: &str,
    value: Value,
    created_at: &str,
    manifest: &mut Vec<Value>,
) -> Result<Value> {
    match value {
        Value::Object(mut object) => {
            let mut candidates = object
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().and_then(|path| {
                        if field_is_local_reference(key, path) {
                            Some((key.clone(), path.to_string()))
                        } else {
                            None
                        }
                    })
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|(left, _), (right, _)| {
                local_field_rank(left)
                    .cmp(&local_field_rank(right))
                    .then_with(|| left.cmp(right))
            });

            let mut seen_paths = HashSet::new();
            let mut media_ids = Vec::new();
            for (key, path) in candidates {
                object.remove(&key);
                if !seen_paths.insert(path.clone()) {
                    continue;
                }
                let source_path = pointer_join(source_pointer, &key);
                let portable_path = pointer_join(portable_pointer, "mediaId");
                let (kind, _) = infer_media_type(&path);
                let role = match kind {
                    "image" => "embedded_image",
                    "audio" => "embedded_audio",
                    "video" => "embedded_video",
                    _ => "embedded_file",
                };
                let descriptor = inventory_media_reference(
                    conn,
                    identity,
                    owner_record_id,
                    source_table,
                    source_row_id,
                    &source_path,
                    &portable_path,
                    role,
                    &path,
                    created_at,
                )?;
                media_ids.push(
                    descriptor
                        .get("mediaId")
                        .cloned()
                        .context("media descriptor is missing mediaId")?,
                );
                manifest.push(descriptor);
            }
            let local_only_keys = object
                .keys()
                .filter(|key| is_explicit_local_path_field(key))
                .cloned()
                .collect::<Vec<_>>();
            for key in local_only_keys {
                object.remove(&key);
            }

            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                let child = object
                    .remove(&key)
                    .expect("key collected from this JSON object");
                let source_path = pointer_join(source_pointer, &key);
                let portable_path = pointer_join(portable_pointer, &key);
                object.insert(
                    key,
                    sanitize_media_value(
                        conn,
                        identity,
                        owner_record_id,
                        source_table,
                        source_row_id,
                        &source_path,
                        &portable_path,
                        child,
                        created_at,
                        manifest,
                    )?,
                );
            }
            match media_ids.len() {
                0 => {}
                1 => {
                    object.insert("mediaId".to_string(), media_ids.remove(0));
                }
                _ => {
                    object.insert("mediaIds".to_string(), Value::Array(media_ids));
                }
            }
            Ok(Value::Object(object))
        }
        Value::Array(values) => {
            let mut portable = Vec::with_capacity(values.len());
            for (index, child) in values.into_iter().enumerate() {
                let segment = index.to_string();
                let source_path = pointer_join(source_pointer, &segment);
                let portable_path = pointer_join(portable_pointer, &segment);
                portable.push(sanitize_media_value(
                    conn,
                    identity,
                    owner_record_id,
                    source_table,
                    source_row_id,
                    &source_path,
                    &portable_path,
                    child,
                    created_at,
                    manifest,
                )?);
            }
            Ok(Value::Array(portable))
        }
        Value::String(path) if is_absolute_local_reference(&path) => {
            let (kind, _) = infer_media_type(&path);
            let role = match kind {
                "image" => "embedded_image",
                "audio" => "embedded_audio",
                "video" => "embedded_video",
                _ => "embedded_file",
            };
            let descriptor = inventory_media_reference(
                conn,
                identity,
                owner_record_id,
                source_table,
                source_row_id,
                source_pointer,
                &pointer_join(portable_pointer, "mediaId"),
                role,
                &path,
                created_at,
            )?;
            let media_id = descriptor
                .get("mediaId")
                .cloned()
                .context("media descriptor is missing mediaId")?;
            manifest.push(descriptor);
            Ok(json!({"mediaId": media_id}))
        }
        other => Ok(other),
    }
}

fn inventory_unowned_entry_media(conn: &Connection, identity: &LocalIdentity) -> Result<()> {
    let rows = {
        let mut statement = conn.prepare(
            "SELECT e.id, e.data_json, e.created_at
             FROM entries e
             LEFT JOIN portable_records p
               ON p.source_table = 'notes' AND p.source_row_id = e.note_id
             WHERE p.record_id IS NULL
             ORDER BY e.id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (entry_id, raw_data, created_at) in rows {
        let Ok(value) = serde_json::from_str::<Value>(&raw_data) else {
            continue;
        };
        let mut manifest = Vec::new();
        sanitize_media_value(
            conn,
            identity,
            None,
            "entries",
            entry_id,
            "",
            "",
            value,
            &created_at,
            &mut manifest,
        )?;
    }
    Ok(())
}

fn inventory_unowned_meeting_document_media(
    conn: &Connection,
    identity: &LocalIdentity,
) -> Result<()> {
    if !table_has_column(conn, "meetings", "notes_document_json")?
        || !table_has_column(conn, "meetings", "created_at")?
    {
        return Ok(());
    }
    let rows = {
        let mut statement = conn.prepare(
            "SELECT id, notes_document_json, created_at
             FROM meetings
             WHERE notes_document_json IS NOT NULL
               AND trim(notes_document_json) != ''
             ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (meeting_id, raw_document, created_at) in rows {
        let Ok(document) = serde_json::from_str::<Value>(&raw_document) else {
            continue;
        };
        let mut manifest = Vec::new();
        sanitize_media_value(
            conn,
            identity,
            None,
            "meetings",
            meeting_id,
            "/notes_document_json",
            "",
            document,
            &created_at,
            &mut manifest,
        )?;
    }
    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = conn.prepare(&sql)?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for candidate in columns {
        if candidate? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn field_is_local_reference(key: &str, value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let normalized = normalize_field_name(key);
    is_explicit_local_path_field(key)
        || matches!(normalized.as_str(), "src" | "url" | "uri" | "path")
            && is_absolute_local_reference(value)
        || is_absolute_local_reference(value)
}

fn is_explicit_local_path_field(key: &str) -> bool {
    matches!(
        normalize_field_name(key).as_str(),
        "localpath" | "imagepath" | "filepath" | "sourcepath" | "localuri"
    )
}

fn local_field_rank(key: &str) -> u8 {
    match normalize_field_name(key).as_str() {
        "localpath" => 0,
        "imagepath" | "filepath" | "sourcepath" | "localuri" => 1,
        "src" => 2,
        _ => 3,
    }
}

fn normalize_field_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_absolute_local_reference(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("\\\\")
        || lower.starts_with("file:")
        || lower.starts_with("blob:")
        || value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .get(2)
                .is_some_and(|separator| matches!(separator, b'/' | b'\\'))
}

fn pointer_join(base: &str, segment: &str) -> String {
    let escaped = segment.replace('~', "~0").replace('/', "~1");
    if base.is_empty() {
        format!("/{escaped}")
    } else {
        format!("{base}/{escaped}")
    }
}

fn infer_media_type(path: &str) -> (&'static str, Option<&'static str>) {
    let without_query = path.split(['?', '#']).next().unwrap_or(path);
    let extension = without_query
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "avif" => ("image", Some("image/avif")),
        "gif" => ("image", Some("image/gif")),
        "heic" => ("image", Some("image/heic")),
        "heif" => ("image", Some("image/heif")),
        "jpeg" | "jpg" => ("image", Some("image/jpeg")),
        "png" => ("image", Some("image/png")),
        "webp" => ("image", Some("image/webp")),
        "aac" => ("audio", Some("audio/aac")),
        "m4a" => ("audio", Some("audio/mp4")),
        "mp3" => ("audio", Some("audio/mpeg")),
        "wav" => ("audio", Some("audio/wav")),
        "mov" => ("video", Some("video/quicktime")),
        "mp4" => ("video", Some("video/mp4")),
        "pdf" => ("file", Some("application/pdf")),
        _ => ("unknown", None),
    }
}

fn validate_portable_value(value: &Value, pointer: &str) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_pointer = pointer_join(pointer, key);
                let normalized = normalize_field_name(key);
                if matches!(
                    normalized.as_str(),
                    "localpath" | "imagepath" | "filepath" | "sourcepath" | "localuri"
                ) {
                    bail!("portable value contains local-only field at {child_pointer}");
                }
                if normalized == "jsonpointer"
                    && child
                        .as_str()
                        .is_some_and(|pointer| pointer.starts_with('/'))
                {
                    continue;
                }
                validate_portable_value(child, &child_pointer)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                validate_portable_value(child, &pointer_join(pointer, &index.to_string()))?;
            }
        }
        Value::String(value) if is_absolute_local_reference(value) => {
            bail!("portable value contains a local filesystem reference at {pointer}");
        }
        _ => {}
    }
    Ok(())
}

fn prepare_version(
    record: ContextRecordV1,
    base_revision: u64,
    base_version_id: Option<String>,
    deterministic: bool,
) -> Result<PreparedVersion> {
    let record_value = serde_json::to_value(&record)?;
    validate_portable_value(&record_value, "")?;
    let snapshot_json = canonical_json(&record_value);
    let mutation_id = if deterministic {
        deterministic_backfill_uuid_v7(
            timestamp_ms(&record.created_at)?,
            "desktop-backfill-mutation-unused",
            &record.version_id,
        )
    } else {
        new_uuid_v7()
    };
    let mutation = json!({
        "protocolVersion": 1,
        "libraryId": record.library_id,
        "mutationId": mutation_id,
        "recordId": record.record_id,
        "recordKind": record.kind,
        "baseHeadRevision": base_revision,
        "baseHeadVersionId": base_version_id,
        "proposedRevision": record.revision,
        "versionId": record.version_id,
        "record": record_value,
    });
    validate_portable_value(&mutation, "")?;
    let mutation_json = canonical_json(&mutation);
    let mutation_digest = canonical_sha256(&mutation);
    Ok(PreparedVersion {
        record,
        snapshot_json,
        base_revision,
        base_version_id,
        mutation_id,
        mutation_json,
        mutation_digest,
    })
}

fn persist_backfill_prepared(
    conn: &Connection,
    identity: &LocalIdentity,
    prepared: PreparedVersion,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO record_versions
         (version_id, record_id, revision, content_hash, snapshot_json,
          source_device_id, transaction_id, created_at, accepted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?7)",
        params![
            prepared.record.version_id,
            prepared.record.record_id,
            i64::try_from(prepared.record.revision)?,
            prepared.record.content_hash,
            prepared.snapshot_json,
            identity.device_id,
            prepared.record.updated_at,
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO record_heads
         (record_id, accepted_revision, accepted_version_id, content_hash,
          authority_generation, accepted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            prepared.record.record_id,
            i64::try_from(prepared.record.revision)?,
            prepared.record.version_id,
            prepared.record.content_hash,
            i64::try_from(identity.authority_generation)?,
            prepared.record.updated_at,
        ],
    )?;
    Ok(())
}

fn persist_shadow_transaction(
    conn: &Connection,
    identity: &LocalIdentity,
    prepared: Vec<PreparedVersion>,
    occurred_at: &str,
) -> Result<()> {
    if prepared.is_empty() {
        return Ok(());
    }
    let transaction_id = new_uuid_v7();
    let member_digests = prepared
        .iter()
        .map(|item| Value::String(item.mutation_digest.clone()))
        .collect::<Vec<_>>();
    let manifest_digest = canonical_sha256(&json!({
        "transactionId": transaction_id,
        "memberCount": prepared.len(),
        "memberDigests": member_digests,
        "commit": true,
    }));
    let counter: i64 = conn.query_row(
        "UPDATE portable_devices
         SET last_transaction_counter = last_transaction_counter + 1
         WHERE device_id = ?1 AND enrollment_state = 'active'
         RETURNING last_transaction_counter",
        [&identity.device_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO change_transactions
         (transaction_id, library_id, device_id, device_transaction_counter,
          member_count, manifest_digest, commit_marker, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
        params![
            transaction_id,
            identity.library_id,
            identity.device_id,
            counter,
            i64::try_from(prepared.len())?,
            manifest_digest,
            occurred_at,
        ],
    )?;
    for (index, item) in prepared.into_iter().enumerate() {
        conn.execute(
            "INSERT INTO record_versions
             (version_id, record_id, revision, content_hash, snapshot_json,
              source_device_id, transaction_id, created_at, accepted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                item.record.version_id,
                item.record.record_id,
                i64::try_from(item.record.revision)?,
                item.record.content_hash,
                item.snapshot_json,
                identity.device_id,
                transaction_id,
                occurred_at,
            ],
        )?;
        conn.execute(
            "INSERT INTO record_heads
             (record_id, accepted_revision, accepted_version_id, content_hash,
              authority_generation, accepted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(record_id) DO UPDATE SET
               accepted_revision = excluded.accepted_revision,
               accepted_version_id = excluded.accepted_version_id,
               content_hash = excluded.content_hash,
               authority_generation = excluded.authority_generation,
               accepted_at = excluded.accepted_at",
            params![
                item.record.record_id,
                i64::try_from(item.record.revision)?,
                item.record.version_id,
                item.record.content_hash,
                i64::try_from(identity.authority_generation)?,
                occurred_at,
            ],
        )?;
        conn.execute(
            "UPDATE portable_records SET
               lifecycle_state = ?1, trashed_at = ?2, tombstoned_at = ?3, updated_at = ?4
             WHERE record_id = ?5",
            params![
                lifecycle_name(&item.record.lifecycle.state),
                item.record.lifecycle.trashed_at,
                item.record.lifecycle.tombstoned_at,
                occurred_at,
                item.record.record_id,
            ],
        )?;
        conn.execute(
            "INSERT INTO change_log
             (mutation_id, transaction_id, transaction_member_index, record_id,
              record_kind, base_revision, base_version_id, proposed_revision,
              version_id, mutation_digest, authority_generation, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     'accepted_local', ?12)",
            params![
                item.mutation_id,
                transaction_id,
                i64::try_from(index)?,
                item.record.record_id,
                item.record.kind,
                i64::try_from(item.base_revision)?,
                item.base_version_id,
                i64::try_from(item.record.revision)?,
                item.record.version_id,
                item.mutation_digest,
                i64::try_from(identity.authority_generation)?,
                occurred_at,
            ],
        )?;
        conn.execute(
            "INSERT INTO sync_outbox
             (mutation_id, payload_json, payload_hash, state, attempts, created_at)
             VALUES (?1, ?2, ?3, 'shadow_pending', 0, ?4)",
            params![
                item.mutation_id,
                item.mutation_json,
                item.mutation_digest,
                occurred_at
            ],
        )?;
    }
    Ok(())
}

fn insert_backfill_version(
    conn: &Connection,
    identity: &LocalIdentity,
    record_id: &str,
    kind: &str,
    created_at: &str,
    updated_at: &str,
    scope: &str,
    authority: RecordAuthority,
    content: Value,
    provenance: Value,
    lifecycle: RecordLifecycle,
) -> Result<()> {
    let version_id = deterministic_backfill_uuid_v7(
        timestamp_ms(created_at)?,
        &format!("desktop-{kind}-version"),
        &format!("{record_id}:1"),
    );
    let record = ContextRecordV1::new(
        identity.library_id.clone(),
        record_id.to_string(),
        kind.to_string(),
        1,
        1,
        version_id,
        created_at.to_string(),
        updated_at.to_string(),
        None,
        RecordScope {
            scope_id: identity.scopes[scope].clone(),
            class: scope_class(scope)?,
        },
        "standard".to_string(),
        authority,
        content,
        provenance,
        lifecycle,
    )
    .map_err(anyhow::Error::msg)?;
    let prepared = prepare_version(record, 0, None, true)?;
    persist_backfill_prepared(conn, identity, prepared)
}

#[allow(clippy::too_many_arguments)]
fn insert_registry(
    conn: &Connection,
    identity: &LocalIdentity,
    record_id: &str,
    kind: &str,
    source_table: &str,
    source_row_id: i64,
    scope_class: &str,
    sensitivity: &str,
    authority_kind: &str,
    authority_origin: Option<&str>,
    write_policy: &str,
    lifecycle_state: &str,
    trashed_at: Option<&str>,
    created_at: &str,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO portable_records
         (record_id, library_id, kind, record_schema_version, source_table,
          source_row_id, scope_id, sensitivity, authority_kind, authority_origin,
          write_policy, lifecycle_state, trashed_at, tombstoned_at,
          created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 NULL, ?13, ?14)",
        params![
            record_id,
            identity.library_id,
            kind,
            source_table,
            source_row_id,
            identity.scopes[scope_class],
            sensitivity,
            authority_kind,
            authority_origin,
            write_policy,
            lifecycle_state,
            trashed_at,
            created_at,
            updated_at,
        ],
    )?;
    let stored = registry_record(conn, source_table, source_row_id)?;
    if stored.record_id != record_id {
        bail!("portable identity drift for {source_table} row {source_row_id}");
    }
    Ok(())
}

fn registry_record(conn: &Connection, table: &str, row_id: i64) -> Result<RegistryRecord> {
    registry_record_optional(conn, table, row_id)?
        .ok_or_else(|| anyhow::anyhow!("portable registry is missing {table} row {row_id}"))
}

fn registry_record_optional(
    conn: &Connection,
    table: &str,
    row_id: i64,
) -> Result<Option<RegistryRecord>> {
    conn.query_row(
        "SELECT p.record_id, p.kind, p.scope_id, s.scope_class, p.sensitivity,
                p.authority_kind, p.authority_origin, p.write_policy,
                p.lifecycle_state, p.created_at
         FROM portable_records p JOIN library_scopes s ON s.scope_id = p.scope_id
         WHERE p.source_table = ?1 AND p.source_row_id = ?2",
        params![table, row_id],
        |row| {
            Ok(RegistryRecord {
                record_id: row.get(0)?,
                kind: row.get(1)?,
                scope_id: row.get(2)?,
                scope_class: row.get(3)?,
                sensitivity: row.get(4)?,
                authority_kind: row.get(5)?,
                authority_origin: row.get(6)?,
                write_policy: row.get(7)?,
                lifecycle_state: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn registry_id(conn: &Connection, table: &str, row_id: i64) -> Result<String> {
    Ok(registry_record(conn, table, row_id)?.record_id)
}

fn read_note_source(conn: &Connection, note_id: i64) -> Result<NoteSource> {
    conn.query_row(
        "SELECT n.id, COALESCE(n.title, ''), n.raw_text, n.source, n.category_id,
                n.created_at, COALESCE(n.origin, 'capture'), n.source_path,
                n.image_path, n.filing_context, n.trashed_at,
                (SELECT COUNT(*) FROM meetings m WHERE m.note_id = n.id)
         FROM notes n WHERE n.id = ?1",
        [note_id],
        |row| {
            Ok(NoteSource {
                id: row.get(0)?,
                title: row.get(1)?,
                raw_text: row.get(2)?,
                source: row.get(3)?,
                category_id: row.get(4)?,
                created_at: row.get(5)?,
                origin: row.get(6)?,
                source_path: row.get(7)?,
                image_path: row.get(8)?,
                filing_context: row.get(9)?,
                trashed_at: row.get(10)?,
                meeting_count: row.get(11)?,
            })
        },
    )
    .with_context(|| format!("note {note_id} does not exist"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoteClass {
    Ordinary,
    Brain,
    MeetingProjection,
    MeetingOrphan,
    UnknownOrigin,
}

fn classify_note(note: &NoteSource) -> NoteClass {
    if note.meeting_count > 0 {
        NoteClass::MeetingProjection
    } else if note.source.eq_ignore_ascii_case("meeting") {
        NoteClass::MeetingOrphan
    } else if note.origin.starts_with("brain:") || note.source.eq_ignore_ascii_case("brain") {
        NoteClass::Brain
    } else if note.origin == "capture" || note.origin.trim().is_empty() {
        NoteClass::Ordinary
    } else {
        NoteClass::UnknownOrigin
    }
}

fn note_authority(note: &NoteSource) -> (&'static str, &'static str, &'static str) {
    if matches!(classify_note(note), NoteClass::Brain) {
        ("external", "registered_brain", "proposal_only")
    } else {
        ("noted", "capture", "read_write")
    }
}

fn note_scope_class(note: &NoteSource) -> &'static str {
    if note.source.eq_ignore_ascii_case("journal") {
        return "personal";
    }
    match note.filing_context.as_deref() {
        Some("work") => "work",
        Some("personal") => "personal",
        _ => "unknown",
    }
}

fn note_sensitivity(note: &NoteSource) -> &'static str {
    if note.source.eq_ignore_ascii_case("journal") {
        "sensitive"
    } else {
        "standard"
    }
}

fn folder_scope_class(conn: &Connection, folder_id: i64) -> Result<&'static str> {
    let root_name: String = conn.query_row(
        "WITH RECURSIVE ancestors(id, parent_id, name) AS (
           SELECT id, parent_id, name FROM note_folders WHERE id = ?1
           UNION ALL
           SELECT parent.id, parent.parent_id, parent.name
           FROM note_folders parent JOIN ancestors child ON child.parent_id = parent.id
         )
         SELECT name FROM ancestors WHERE parent_id IS NULL LIMIT 1",
        [folder_id],
        |row| row.get(0),
    )?;
    Ok(if root_name.eq_ignore_ascii_case("work") {
        "work"
    } else if root_name.eq_ignore_ascii_case("personal") {
        "personal"
    } else {
        "unknown"
    })
}

fn quarantine_note(
    conn: &Connection,
    note: &NoteSource,
    reason: &str,
    details: Value,
) -> Result<()> {
    let created_at = normalize_utc(&note.created_at)?;
    let quarantine_id = deterministic_backfill_uuid_v7(
        timestamp_ms(&created_at)?,
        "desktop-portable-quarantine",
        &format!("notes:{}:{reason}", note.id),
    );
    conn.execute(
        "INSERT OR IGNORE INTO portable_quarantine
         (quarantine_id, source_table, source_row_id, reason, details_json, quarantined_at)
         VALUES (?1, 'notes', ?2, ?3, ?4, ?5)",
        params![
            quarantine_id,
            note.id,
            reason,
            canonical_json(&details),
            created_at
        ],
    )?;
    Ok(())
}

fn scope_class(value: &str) -> Result<ScopeClass> {
    match value {
        "work" => Ok(ScopeClass::Work),
        "personal" => Ok(ScopeClass::Personal),
        "unknown" => Ok(ScopeClass::Unknown),
        other => bail!("unsupported portable scope class '{other}'"),
    }
}

fn lifecycle_name(value: &LifecycleState) -> &'static str {
    match value {
        LifecycleState::Active => "active",
        LifecycleState::Trash => "trash",
        LifecycleState::Tombstone => "tombstone",
    }
}

fn normalize_utc(value: &str) -> Result<String> {
    let value = value.trim();
    let parsed = DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
                .map(|value| value.and_utc())
        })
        .or_else(|_| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map(|value| {
                value
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is valid")
                    .and_utc()
            })
        })
        .with_context(|| format!("'{value}' is not a supported UTC timestamp"))?;
    Ok(parsed.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn timestamp_ms(value: &str) -> Result<u64> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("'{value}' is not RFC 3339"))?
        .with_timezone(&Utc);
    u64::try_from(parsed.timestamp_millis()).context("timestamp predates the Unix epoch")
}

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE app_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE categories(
               id INTEGER PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL,
               schema_json TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE notes(
               id INTEGER PRIMARY KEY, title TEXT, raw_text TEXT NOT NULL,
               source TEXT NOT NULL, category_id INTEGER, created_at TEXT NOT NULL,
               origin TEXT, source_path TEXT, image_path TEXT,
               filing_context TEXT, trashed_at TEXT
             );
             CREATE TABLE entries(
               id INTEGER PRIMARY KEY, note_id INTEGER NOT NULL, category_id INTEGER NOT NULL,
               data_json TEXT NOT NULL, event_date TEXT NOT NULL, created_at TEXT NOT NULL
             );
             CREATE TABLE note_folders(
               id INTEGER PRIMARY KEY, parent_id INTEGER, name TEXT NOT NULL,
               kind TEXT NOT NULL, auto_rule TEXT NOT NULL, position INTEGER NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE note_folder_items(
               folder_id INTEGER NOT NULL, note_id INTEGER NOT NULL, created_at TEXT NOT NULL
             );
             CREATE TABLE meetings(id INTEGER PRIMARY KEY, note_id INTEGER);
             INSERT INTO categories VALUES
               (1, 'Ideas', 'Things to revisit',
                '{\"shape\":{\"idea\":\"string\"},\"field_freq\":{\"idea\":1}}',
                '2026-08-01T10:00:00Z');
             INSERT INTO note_folders VALUES
               (10, NULL, 'Personal', 'space', '', 0, '2026-08-01T09:00:00Z'),
               (11, 10, 'Inbox', 'folder', '', 0, '2026-08-01T09:01:00Z');
             INSERT INTO notes VALUES
               (20, 'First', 'portable body', 'text', 1, '2026-08-02T10:00:00Z',
                'capture', NULL, NULL, 'personal', NULL),
               (21, 'Mirror', 'external body', 'text', 1, '2026-08-03T10:00:00Z',
                'brain:private-vault', 'People/Ada.md', NULL, 'personal', NULL),
               (22, 'Projection', 'meeting projection', 'meeting', 1,
                '2026-08-04T10:00:00Z', 'capture', NULL, NULL, 'work', NULL),
               (23, 'Orphan', 'lost owner', 'meeting', 1,
                '2026-08-05T10:00:00Z', 'capture', NULL, NULL, 'work', NULL);
             INSERT INTO entries VALUES
               (30, 20, 1, '{\"idea\":\"ship it\"}', '2026-08-02', '2026-08-02T10:00:00Z'),
               (31, 21, 1, '{\"person\":\"Ada\"}', '2026-08-03', '2026-08-03T10:00:00Z');
             INSERT INTO note_folder_items VALUES
               (11, 20, '2026-08-02T10:00:00Z');
             INSERT INTO meetings VALUES (40, 22);",
        )
        .unwrap();
        conn
    }

    fn migrate_fixture(conn: &mut Connection) {
        let tx = conn.transaction().unwrap();
        install_and_backfill(&tx).unwrap();
        tx.commit().unwrap();
    }

    fn add_media_fixture(conn: &Connection) {
        conn.execute(
            "UPDATE notes
             SET image_path = '/Users/me/Pictures/private.png'
             WHERE id = 20",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE entries SET data_json = ?1 WHERE id = 30",
            [canonical_json(&json!({
                "type": "doc",
                "content": [{
                    "type": "container",
                    "content": [{
                        "type": "image",
                        "attrs": {
                            "src": "file:///Users/me/Pictures/nested.png",
                            "localPath": "file:///Users/me/Pictures/nested.png",
                            "alt": "Nested"
                        }
                    }]
                }],
                "remote": {"src": "https://cdn.example.test/public.png"}
            }))],
        )
        .unwrap();
        conn.execute(
            "ALTER TABLE meetings ADD COLUMN notes_document_json TEXT",
            [],
        )
        .unwrap();
        conn.execute("ALTER TABLE meetings ADD COLUMN created_at TEXT", [])
            .unwrap();
        conn.execute(
            "UPDATE meetings SET notes_document_json = ?1,
                                 created_at = '2026-08-04T10:00:00Z'
             WHERE id = 40",
            [canonical_json(&json!({
                "type": "doc",
                "content": [{
                    "type": "image",
                    "attrs": {
                        "src": "file:///Users/me/Pictures/meeting.png",
                        "localPath": "file:///Users/me/Pictures/meeting.png"
                    }
                }]
            }))],
        )
        .unwrap();
    }

    fn head_snapshot(conn: &Connection, source_table: &str, source_row_id: i64) -> Value {
        let raw: String = conn
            .query_row(
                "SELECT v.snapshot_json
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 WHERE p.source_table = ?1 AND p.source_row_id = ?2",
                params![source_table, source_row_id],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn journal_counts(conn: &Connection) -> (i64, i64, i64, i64) {
        (
            conn.query_row("SELECT COUNT(*) FROM record_versions", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM change_transactions", [], |row| {
                row.get(0)
            })
            .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM change_log", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row.get(0))
                .unwrap(),
        )
    }

    #[test]
    fn category_create_and_schema_evolution_are_atomic_portable_revisions() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let schema_v1 = json!({
            "shape": {"person": "string"},
            "field_freq": {"person": 1},
            "futureNamespace": {"preserve": [1, true, "yes"]}
        });
        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO categories
                 (id, name, description, schema_json, created_at)
                 VALUES (2, 'People', 'People to remember', ?1, ?2)",
                params![canonical_json(&schema_v1), "2026-08-10T10:00:00.000Z"],
            )
            .unwrap();
            journal_category_write(&tx, 2, "2026-08-10T10:00:00Z").unwrap();
            tx.commit().unwrap();
        }

        let created = head_snapshot(&conn, "categories", 2);
        assert!(is_uuid_v7(created["record_id"].as_str().unwrap()));
        assert_eq!(created["kind"], "category");
        assert_eq!(created["revision"], 1);
        assert_eq!(created["authority"]["kind"], "noted");
        assert_eq!(created["content"]["schema"], schema_v1);
        assert!(created["content"].get("id").is_none());
        assert_eq!(
            conn.query_row("SELECT member_count FROM change_transactions", [], |row| {
                row.get::<_, i64>(0)
            },)
                .unwrap(),
            1
        );

        let after_create = journal_counts(&conn);
        {
            let tx = conn.transaction().unwrap();
            journal_category_write(&tx, 2, "2026-08-10T10:01:00Z").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(journal_counts(&conn), after_create);

        let schema_v2 = json!({
            "shape": {"person": "string", "company": "string"},
            "field_freq": {"person": 2, "company": 1},
            "futureNamespace": {"preserve": [1, true, "yes"], "new": {"x": 9}}
        });
        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE categories SET description = ?1, schema_json = ?2 WHERE id = 2",
                params!["People and organizations", canonical_json(&schema_v2)],
            )
            .unwrap();
            journal_category_write(&tx, 2, "2026-08-10T10:02:00Z").unwrap();
            tx.commit().unwrap();
        }
        let evolved = head_snapshot(&conn, "categories", 2);
        assert_eq!(evolved["revision"], 2);
        assert_eq!(
            evolved["content"]["description"],
            "People and organizations"
        );
        assert_eq!(evolved["content"]["schema"], schema_v2);
        assert_eq!(journal_counts(&conn).0, after_create.0 + 1);
        assert_eq!(journal_counts(&conn).2, after_create.2 + 1);
        assert_eq!(journal_counts(&conn).3, after_create.3 + 1);
        verify_portable_schema(&conn).unwrap();
    }

    #[test]
    fn folder_create_rename_move_and_reorder_use_uuid_dependencies() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO note_folders
                 (id, parent_id, name, kind, auto_rule, position, created_at)
                 VALUES (12, 10, 'Projects', 'folder', '', 1, ?1)",
                ["2026-08-10T11:00:00Z"],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO note_folders
                 (id, parent_id, name, kind, auto_rule, position, created_at)
                 VALUES (13, 12, 'Launch', 'folder', '', 0, ?1)",
                ["2026-08-10T11:00:01Z"],
            )
            .unwrap();
            journal_folder_write(&tx, 13, "2026-08-10T11:00:02Z").unwrap();
            tx.commit().unwrap();
        }

        let (transaction_id, member_count): (String, i64) = conn
            .query_row(
                "SELECT transaction_id, member_count FROM change_transactions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(member_count, 2);
        let ordered_rows = conn
            .prepare(
                "SELECT p.source_row_id, l.transaction_member_index
                 FROM change_log l
                 JOIN portable_records p ON p.record_id = l.record_id
                 WHERE l.transaction_id = ?1
                 ORDER BY l.transaction_member_index",
            )
            .unwrap()
            .query_map([transaction_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ordered_rows, vec![(12, 0), (13, 1)]);

        let parent = head_snapshot(&conn, "note_folders", 12);
        let child = head_snapshot(&conn, "note_folders", 13);
        let parent_record_id = parent["record_id"].as_str().unwrap();
        let child_record_id = child["record_id"].as_str().unwrap();
        assert!(is_uuid_v7(parent_record_id));
        assert!(is_uuid_v7(child_record_id));
        assert_ne!(parent_record_id, child_record_id);
        assert_eq!(child["content"]["parentId"], parent_record_id);
        assert!(child["content"].get("id").is_none());

        let after_create = journal_counts(&conn);
        {
            let tx = conn.transaction().unwrap();
            journal_folder_write(&tx, 13, "2026-08-10T11:01:00Z").unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(journal_counts(&conn), after_create);

        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE note_folders SET name = 'Launch plan' WHERE id = 13",
                [],
            )
            .unwrap();
            journal_folder_write(&tx, 13, "2026-08-10T11:02:00Z").unwrap();
            tx.commit().unwrap();
        }
        let renamed = head_snapshot(&conn, "note_folders", 13);
        assert_eq!(renamed["revision"], 2);
        assert_eq!(renamed["content"]["name"], "Launch plan");

        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE note_folders SET parent_id = 10, position = 2 WHERE id = 13",
                [],
            )
            .unwrap();
            journal_folder_write(&tx, 13, "2026-08-10T11:03:00Z").unwrap();
            tx.commit().unwrap();
        }
        let moved = head_snapshot(&conn, "note_folders", 13);
        let root = head_snapshot(&conn, "note_folders", 10);
        assert_eq!(moved["revision"], 3);
        assert_eq!(moved["content"]["parentId"], root["record_id"]);
        assert_eq!(moved["content"]["position"], 2);
        assert!(is_uuid_v7(moved["content"]["parentId"].as_str().unwrap()));

        {
            let tx = conn.transaction().unwrap();
            tx.execute("UPDATE note_folders SET position = 0 WHERE id = 13", [])
                .unwrap();
            journal_folder_write(&tx, 13, "2026-08-10T11:04:00Z").unwrap();
            tx.commit().unwrap();
        }
        let reordered = head_snapshot(&conn, "note_folders", 13);
        assert_eq!(reordered["revision"], 4);
        assert_eq!(reordered["content"]["position"], 0);

        let payloads = conn
            .prepare("SELECT payload_json FROM sync_outbox")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(payloads.iter().all(|payload| {
            !payload.contains("source_row_id")
                && !payload.contains("sourceRowId")
                && validate_portable_value(&serde_json::from_str(payload).unwrap(), "").is_ok()
        }));
        verify_portable_schema(&conn).unwrap();
    }

    #[test]
    fn dependency_journals_roll_back_with_outer_transaction_failures() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let before = journal_counts(&conn);
        let counter_before: i64 = conn
            .query_row(
                "SELECT last_transaction_counter FROM portable_devices",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let injected: Result<()> = (|| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO categories
                 (id, name, description, schema_json, created_at)
                 VALUES (2, 'Rollback', '', '{}', ?1)",
                ["2026-08-10T12:00:00Z"],
            )?;
            journal_category_write(&tx, 2, "2026-08-10T12:00:01Z")?;
            tx.execute(
                "INSERT INTO note_folders
                 (id, parent_id, name, kind, auto_rule, position, created_at)
                 VALUES (12, 10, 'Rollback', 'folder', '', 1, ?1)",
                ["2026-08-10T12:00:00Z"],
            )?;
            journal_folder_write(&tx, 12, "2026-08-10T12:00:01Z")?;
            Err(anyhow::anyhow!("injected failure after shadow writes"))
        })();
        assert!(injected
            .unwrap_err()
            .to_string()
            .contains("injected failure"));
        assert_eq!(journal_counts(&conn), before);
        assert_eq!(
            conn.query_row(
                "SELECT last_transaction_counter FROM portable_devices",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            counter_before
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM categories WHERE id = 2", [], |row| {
                row.get::<_, i64>(0)
            },)
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM note_folders WHERE id = 12",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn dependency_writes_reject_authority_drift_and_physical_deletion() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let before = journal_counts(&conn);

        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE portable_records
                 SET authority_kind = 'external', authority_origin = 'other',
                     write_policy = 'proposal_only'
                 WHERE source_table = 'categories' AND source_row_id = 1",
                [],
            )
            .unwrap();
            tx.execute(
                "UPDATE categories SET description = 'drift' WHERE id = 1",
                [],
            )
            .unwrap();
            let error = journal_category_write(&tx, 1, "2026-08-10T13:00:00Z").unwrap_err();
            assert!(error
                .to_string()
                .contains("not writable under Noted authority"));
            tx.rollback().unwrap();
        }

        {
            let tx = conn.transaction().unwrap();
            tx.execute("DELETE FROM note_folders WHERE id = 11", [])
                .unwrap();
            let error = journal_folder_write(&tx, 11, "2026-08-10T13:01:00Z").unwrap_err();
            assert!(error.to_string().contains("physically deleted"));
            tx.rollback().unwrap();
        }
        assert_eq!(journal_counts(&conn), before);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM note_folders WHERE id = 11",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn deterministic_backfill_creates_accepted_heads_without_outbox_flood() {
        let mut first = fixture();
        let mut second = fixture();
        migrate_fixture(&mut first);
        migrate_fixture(&mut second);

        let identity = |conn: &Connection| {
            conn.query_row("SELECT library_id FROM libraries", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
        };
        assert_eq!(identity(&first), identity(&second));
        assert!(is_uuid_v7(&identity(&first)));

        let records = |conn: &Connection| {
            let mut statement = conn
                .prepare(
                    "SELECT source_table, source_row_id, record_id
                     FROM portable_records ORDER BY source_table, source_row_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(records(&first), records(&second));
        assert_eq!(
            first
                .query_row("SELECT COUNT(*) FROM record_heads", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            first
                .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            first
                .query_row("SELECT COUNT(*) FROM change_log", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        verify_portable_schema(&first).unwrap();
    }

    #[test]
    fn meeting_projection_is_not_a_note_and_orphan_is_quarantined() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let projection_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM portable_records
                 WHERE source_table = 'notes' AND source_row_id IN (22, 23)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_count, 0);
        let reason: String = conn
            .query_row(
                "SELECT reason FROM portable_quarantine
                 WHERE source_table = 'notes' AND source_row_id = 23",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "meeting_origin_without_owner");
    }

    #[test]
    fn multiply_linked_meeting_projection_is_quarantined_without_a_note_head() {
        let mut conn = fixture();
        conn.execute("INSERT INTO meetings VALUES (41, 22)", [])
            .unwrap();
        migrate_fixture(&mut conn);
        let reason: String = conn
            .query_row(
                "SELECT reason FROM portable_quarantine
                 WHERE source_table = 'notes' AND source_row_id = 22",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "meeting_projection_multiple_owners");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM portable_records
                 WHERE source_table = 'notes' AND source_row_id = 22",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn contradictory_brain_source_stays_external_and_meeting_source_stays_orphaned() {
        let mut conn = fixture();
        conn.execute(
            "INSERT INTO notes VALUES
             (24, 'Contradictory mirror', 'mirror', 'brain', 1,
              '2026-08-06T10:00:00Z', 'capture', '/Users/me/Brain/secret.md',
              NULL, 'work', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notes VALUES
             (25, 'Contradictory meeting', 'projection', 'meeting', 1,
              '2026-08-07T10:00:00Z', 'brain:private', 'Meeting.md',
              NULL, 'work', NULL)",
            [],
        )
        .unwrap();
        migrate_fixture(&mut conn);

        let (authority, policy): (String, String) = conn
            .query_row(
                "SELECT authority_kind, write_policy FROM portable_records
                 WHERE source_table = 'notes' AND source_row_id = 24",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (authority.as_str(), policy.as_str()),
            ("external", "proposal_only")
        );
        assert_eq!(
            conn.query_row(
                "SELECT reason FROM portable_quarantine
                 WHERE source_table = 'notes' AND source_row_id = 25",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "meeting_origin_without_owner"
        );
    }

    #[test]
    fn brain_mirror_is_external_proposal_only_and_hides_source_path() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let (authority, policy, snapshot): (String, String, String) = conn
            .query_row(
                "SELECT p.authority_kind, p.write_policy, v.snapshot_json
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 WHERE p.source_table = 'notes' AND p.source_row_id = 21",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(authority, "external");
        assert_eq!(policy, "proposal_only");
        assert!(!snapshot.contains("People/Ada.md"));
        assert!(!snapshot.contains("private-vault"));
        assert!(journal_note_write(&conn, 21, "2026-08-10T00:00:00Z").is_err());
    }

    #[test]
    fn local_media_paths_never_enter_portable_snapshots() {
        let mut conn = fixture();
        add_media_fixture(&conn);
        migrate_fixture(&mut conn);

        let snapshot: String = conn
            .query_row(
                "SELECT v.snapshot_json
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 WHERE p.source_table = 'notes' AND p.source_row_id = 20",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!snapshot.contains("/Users/"));
        assert!(!snapshot.contains("file:"));
        assert!(!snapshot.contains("localPath"));
        assert!(snapshot.contains("https://cdn.example.test/public.png"));
        let parsed: Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(parsed["content"]["media"].as_array().unwrap().len(), 2);
        assert!(is_uuid_v7(
            parsed["content"]["media"][0]["mediaId"].as_str().unwrap()
        ));
        assert!(
            parsed["content"]["entries"][0]["data"]["content"][0]["content"][0]["attrs"]["mediaId"]
                .as_str()
                .is_some()
        );

        let snapshots = conn
            .prepare("SELECT snapshot_json FROM record_versions")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(snapshots.iter().all(|snapshot| validate_portable_value(
            &serde_json::from_str(snapshot).unwrap(),
            ""
        )
        .is_ok()));

        let local_paths = conn
            .prepare(
                "SELECT source_table, source_row_id, json_pointer, local_path
                 FROM media_local_paths ORDER BY source_table, source_row_id, json_pointer",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(local_paths.len(), 3);
        assert!(local_paths
            .iter()
            .any(|(table, row, _, path)| table == "notes"
                && *row == 20
                && path == "/Users/me/Pictures/private.png"));
        assert!(local_paths
            .iter()
            .any(|(table, row, _, path)| table == "entries"
                && *row == 30
                && path == "file:///Users/me/Pictures/nested.png"));
        assert!(local_paths
            .iter()
            .any(|(table, row, _, path)| table == "meetings"
                && *row == 40
                && path == "file:///Users/me/Pictures/meeting.png"));
        assert!(local_paths
            .iter()
            .all(|(_, _, _, path)| path != "People/Ada.md"));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM media_objects", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM media_refs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        verify_portable_schema(&conn).unwrap();
    }

    #[test]
    fn media_backfill_ids_are_deterministic_and_live_outbox_remains_path_free() {
        let mut first = fixture();
        let mut second = fixture();
        add_media_fixture(&first);
        add_media_fixture(&second);
        migrate_fixture(&mut first);
        migrate_fixture(&mut second);

        let inventory = |conn: &Connection| {
            conn.prepare(
                "SELECT p.source_table, p.source_row_id, p.json_pointer, p.media_id
                 FROM media_local_paths p
                 ORDER BY p.source_table, p.source_row_id, p.json_pointer",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert_eq!(inventory(&first), inventory(&second));

        first
            .execute(
                "UPDATE entries SET data_json = ?1 WHERE id = 30",
                [canonical_json(&json!({
                    "deep": [{"deeper": {"src": "/private/tmp/live-update.jpg"}}]
                }))],
            )
            .unwrap();
        journal_note_write(&first, 20, "2026-08-10T00:00:00Z").unwrap();
        let payload: String = first
            .query_row(
                "SELECT payload_json FROM sync_outbox ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!payload.contains("/private/tmp"));
        assert!(!payload.contains("localPath"));
        assert!(payload.contains("mediaId"));
        validate_portable_value(&serde_json::from_str(&payload).unwrap(), "").unwrap();
        verify_portable_schema(&first).unwrap();
    }

    #[test]
    fn local_note_write_advances_head_and_creates_one_shadow_mutation() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        conn.execute(
            "UPDATE notes SET raw_text = 'edited body' WHERE id = 20",
            [],
        )
        .unwrap();
        journal_note_write(&conn, 20, "2026-08-10T00:00:00Z").unwrap();

        let (revision, body): (i64, String) = conn
            .query_row(
                "SELECT h.accepted_revision,
                        json_extract(v.snapshot_json, '$.content.body')
                 FROM portable_records p
                 JOIN record_heads h ON h.record_id = p.record_id
                 JOIN record_versions v ON v.version_id = h.accepted_version_id
                 WHERE p.source_table = 'notes' AND p.source_row_id = 20",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(revision, 2);
        assert_eq!(body, "edited body");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM change_log", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let payload: String = conn
            .query_row("SELECT payload_json FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert!(!payload.contains("source_row_id"));
        assert!(!payload.contains("People/Ada.md"));
    }

    #[test]
    fn portable_histories_are_immutable() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let version_id: String = conn
            .query_row(
                "SELECT version_id FROM record_versions LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let error = conn
            .execute(
                "UPDATE record_versions SET snapshot_json = '{}' WHERE version_id = ?1",
                [&version_id],
            )
            .unwrap_err();
        assert!(error.to_string().contains("immutable"));
    }

    #[test]
    fn rerunning_backfill_is_idempotent() {
        let mut conn = fixture();
        migrate_fixture(&mut conn);
        let before: (i64, i64, i64) = (
            conn.query_row("SELECT COUNT(*) FROM portable_records", [], |row| {
                row.get(0)
            })
            .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM record_versions", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM portable_quarantine", [], |row| {
                row.get(0)
            })
            .unwrap(),
        );
        let tx = conn.transaction().unwrap();
        install_and_backfill(&tx).unwrap();
        tx.commit().unwrap();
        let after: (i64, i64, i64) = (
            conn.query_row("SELECT COUNT(*) FROM portable_records", [], |row| {
                row.get(0)
            })
            .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM record_versions", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM portable_quarantine", [], |row| {
                row.get(0)
            })
            .unwrap(),
        );
        assert_eq!(before, after);
    }
}
