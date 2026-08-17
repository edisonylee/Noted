use crate::portable::{
    canonical_sha256, deterministic_backfill_uuid_v7, is_uuid, is_uuid_v7, new_uuid_v7,
    AcceptedHead, AuthorityKind, LifecycleState, LocalBranch, LocalBranchState, RecordAuthority,
    RecordLifecycle, RecordScope, ScopeClass,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const PORTABLE_SCHEMA_VERSION: i64 = 2;
const PORTABLE_SCHEMA_V1_CHECKSUM: &str =
    "d6d8377525aa80d91e9e7cb22d4eff4da5cf7998abc8968a5457c1fc86e84b7b";
const PORTABLE_SCHEMA_V2_CHECKSUM: &str =
    "838992191ee7053706d16154b41f08b1e041526101d496d1d762848a614b8e45";
const PORTABLE_MIGRATION_V1_NAME: &str = "iphone-notes-portability";
const PORTABLE_MIGRATION_V2_NAME: &str = "iphone-note-lifecycle-and-transaction-groups";
const MOBILE_APPLICATION_ID: i64 = 0x4e4f_5449; // ASCII `NOTI`.
const MOBILE_NOTES_EXPORT_FORMAT: &str = "noted.mobile-notes.export.v1";
const MOBILE_NOTES_EXPORT_VERSION: u32 = 1;
const MAX_MOBILE_NOTES_EXPORT_BYTES: usize = 256 * 1024 * 1024;
static RECOVERY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileNote {
    pub record_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MobileNotesExportEnvelope {
    format: String,
    format_version: u32,
    payload: MobileNotesExportPayload,
    payload_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MobileNotesExportPayload {
    replica: MobileReplicaExport,
    notes: Vec<MobileNoteExport>,
    outbox: Vec<MobileOutboxExport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MobileReplicaExport {
    library_id: String,
    device_id: String,
    install_id: String,
    default_scope_id: String,
    library_state: String,
    next_transaction_counter: i64,
    created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MobileNoteExport {
    library_id: String,
    record_id: String,
    record_kind: String,
    record_schema_version: i64,
    title: String,
    body: String,
    created_at: i64,
    updated_at: i64,
    accepted_revision: i64,
    accepted_version_id: Option<String>,
    accepted_content_hash: Option<String>,
    working_revision: i64,
    working_branch_id: String,
    working_version_id: String,
    working_base_revision: i64,
    pending_mutation_id: String,
    sync_state: String,
    lifecycle_state: String,
    trashed_at: Option<i64>,
    tombstoned_at: Option<i64>,
    canonical_hash: String,
    authority: String,
    scope: String,
    scope_id: String,
    scope_class: String,
    sensitivity: String,
    provenance: serde_json::Value,
    origin_device_id: String,
    last_modified_device_id: String,
    origin_install_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MobileOutboxExport {
    mutation_id: String,
    transaction_id: String,
    device_transaction_counter: i64,
    transaction_member_index: i64,
    transaction_member_count: i64,
    library_id: String,
    device_id: String,
    install_id: String,
    scope_id: String,
    scope_class: String,
    record_id: String,
    record_kind: String,
    operation: String,
    base_revision: i64,
    base_version_id: Option<String>,
    proposed_revision: i64,
    local_revision: i64,
    branch_id: String,
    version_id: String,
    canonical_hash: String,
    payload: serde_json::Value,
    state: String,
    eligible_for_sync: bool,
    superseded_at: Option<i64>,
    attempts: i64,
    created_at: i64,
    acknowledged_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct ReplicaIdentity {
    library_id: String,
    device_id: String,
    install_id: String,
    default_scope_id: String,
    library_state: String,
}

#[derive(Debug)]
struct PortableState {
    record_id: String,
    accepted_revision: i64,
    working_revision: i64,
    working_branch_id: String,
    accepted_version_id: Option<String>,
    accepted_content_hash: Option<String>,
    created_at: i64,
    lifecycle_state: String,
    trashed_at: Option<i64>,
    authority: String,
    provenance_json: String,
    scope_id: String,
    scope_class: String,
}

#[derive(Debug)]
struct Mutation<'a> {
    operation: &'a str,
    record_id: &'a str,
    title: &'a str,
    body: &'a str,
    base_revision: i64,
    proposed_revision: i64,
    local_revision: i64,
    version_id: &'a str,
    branch_id: &'a str,
    base_version_id: Option<&'a str>,
    accepted_content_hash: Option<&'a str>,
    mutation_id: &'a str,
    canonical_hash: &'a str,
    lifecycle_state: &'a str,
    trashed_at: Option<i64>,
    tombstoned_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
    provenance_json: &'a str,
    scope_id: &'a str,
    scope_class: &'a str,
}

#[derive(Debug)]
struct OutboxTransaction {
    transaction_id: String,
    device_transaction_counter: i64,
    member_count: i64,
}

#[derive(Serialize)]
struct MutationPayload<'a> {
    mutation_contract_version: &'static str,
    operation: &'a str,
    proposed_revision: i64,
    proposed_record: ProposedRecordPayload<'a>,
}

#[derive(Serialize)]
struct ProposedRecordPayload<'a> {
    proposal_contract_version: &'static str,
    library_id: &'a str,
    record_id: &'a str,
    kind: &'static str,
    record_schema_version: u32,
    created_at: String,
    updated_at: String,
    scope: RecordScope,
    sensitivity: &'static str,
    authority: RecordAuthority,
    content: serde_json::Value,
    content_hash: &'a str,
    provenance: serde_json::Value,
    lifecycle: RecordLifecycle,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted_head: Option<AcceptedHead>,
    local_branch: LocalBranch,
}

pub struct MobileStore {
    connection: Mutex<Connection>,
}

impl MobileStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| error.to_string())?;
        let recovery_path = prepare_mobile_migration_recovery(path, &connection)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(|error| error.to_string())?;
        migrate_portable_notes(&mut connection, recovery_path.as_deref())?;
        ensure_mobile_search_schema(&mut connection)?;
        verify_mobile_search_schema(&connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Internal recovery artifact created before the one-time prototype
    /// migration. A future Settings flow can expose an explicit export/reset
    /// choice without guessing a filesystem location.
    #[allow(dead_code)]
    pub fn migration_recovery_path(&self) -> Result<Option<String>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        connection
            .query_row(
                "SELECT migration_recovery_path FROM mobile_schema_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(|error| error.to_string())
    }

    pub fn list(&self, query: Option<&str>) -> Result<Vec<MobileNote>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let trimmed = query.map(str::trim).filter(|value| !value.is_empty());

        if let Some(query) = trimmed {
            let pattern = format!("%{}%", escape_like(query));
            let fts_query = mobile_fts_query(query);
            if fts_query.is_none() {
                let mut statement = connection
                    .prepare(
                        "SELECT record_id, title, body, created_at, updated_at
                         FROM mobile_notes
                         WHERE lifecycle_state = 'active'
                           AND deleted_at IS NULL
                           AND (title LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                             OR body LIKE ?1 ESCAPE '\\' COLLATE NOCASE)
                         ORDER BY updated_at DESC, id DESC",
                    )
                    .map_err(|error| error.to_string())?;
                let rows = statement
                    .query_map([pattern], note_from_row)
                    .map_err(|error| error.to_string())?;
                return rows
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string());
            }
            let mut statement = connection
                .prepare(
                    "SELECT notes.record_id, notes.title, notes.body,
                            notes.created_at, notes.updated_at
                     FROM mobile_notes_fts
                     JOIN mobile_notes AS notes
                       ON notes.record_id = mobile_notes_fts.record_id
                     WHERE mobile_notes_fts MATCH ?1
                       AND notes.lifecycle_state = 'active'
                       AND notes.deleted_at IS NULL
                       AND (notes.title LIKE ?2 ESCAPE '\\' COLLATE NOCASE
                         OR notes.body LIKE ?2 ESCAPE '\\' COLLATE NOCASE)
                     ORDER BY notes.updated_at DESC, notes.id DESC",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map(
                    params![fts_query.expect("checked above"), pattern],
                    note_from_row,
                )
                .map_err(|error| error.to_string())?;
            return rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string());
        }

        let mut statement = connection
            .prepare(
                "SELECT record_id, title, body, created_at, updated_at
                 FROM mobile_notes
                 WHERE lifecycle_state = 'active' AND deleted_at IS NULL
                 ORDER BY updated_at DESC, id DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], note_from_row)
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn create(&self, title: &str, body: &str) -> Result<MobileNote, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let identity = replica_identity(&transaction)?;
        let timestamp = next_timestamp(&transaction)?;
        let record_id = new_uuid_v7();
        let working_branch_id = new_uuid_v7();
        let working_version_id = new_uuid_v7();
        let mutation_id = new_uuid_v7();
        let outbox_transaction = begin_outbox_transaction(&transaction, 1)?;
        let title = title.trim();
        let canonical_hash = note_content_hash(title, body);
        let provenance_json = r#"{"source":"iphone_native"}"#;

        transaction
            .execute(
                "INSERT INTO mobile_notes (
                   title, body, created_at, updated_at, deleted_at,
                   library_id, record_id, record_kind, record_schema_version,
                   accepted_revision, accepted_version_id, accepted_content_hash,
                   working_revision, working_branch_id, working_version_id,
                   working_base_revision,
                   pending_mutation_id, sync_state, lifecycle_state, tombstoned_at,
                   canonical_hash, authority, scope, scope_id, scope_class,
                   sensitivity, provenance_json,
                   origin_device_id, last_modified_device_id, origin_install_id
                 ) VALUES (
                   ?1, ?2, ?3, ?3, NULL,
                   ?4, ?5, 'note', 1,
                   0, NULL, NULL,
                   1, ?6, ?7, 0,
                   ?8, 'pending', 'active', NULL,
                   ?9, 'noted', 'personal', ?10, 'personal',
                   'standard', ?11,
                   ?12, ?12, ?13
                 )",
                params![
                    title,
                    body,
                    timestamp,
                    identity.library_id,
                    record_id,
                    working_branch_id,
                    working_version_id,
                    mutation_id,
                    canonical_hash,
                    identity.default_scope_id,
                    provenance_json,
                    identity.device_id,
                    identity.install_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        enqueue_mutation(
            &transaction,
            &identity,
            &outbox_transaction,
            0,
            Mutation {
                operation: "create",
                record_id: &record_id,
                title,
                body,
                base_revision: 0,
                proposed_revision: 1,
                local_revision: 1,
                version_id: &working_version_id,
                branch_id: &working_branch_id,
                base_version_id: None,
                accepted_content_hash: None,
                mutation_id: &mutation_id,
                canonical_hash: &canonical_hash,
                lifecycle_state: "active",
                trashed_at: None,
                tombstoned_at: None,
                created_at: timestamp,
                updated_at: timestamp,
                provenance_json,
                scope_id: &identity.default_scope_id,
                scope_class: "personal",
            },
        )?;
        transaction.commit().map_err(|error| error.to_string())?;

        Ok(MobileNote {
            record_id,
            title: title.to_string(),
            body: body.to_string(),
            created_at: timestamp,
            updated_at: timestamp,
        })
    }

    pub fn update(&self, record_id: &str, title: &str, body: &str) -> Result<MobileNote, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let identity = replica_identity(&transaction)?;
        if !is_uuid_v7(record_id) {
            return Err("note record_id must be a canonical UUIDv7".to_string());
        }
        let state = portable_state(&transaction, record_id)?
            .filter(|state| state.lifecycle_state == "active")
            .ok_or_else(|| format!("note {record_id} does not exist"))?;
        ensure_noted_authority(&state)?;
        let timestamp = next_timestamp(&transaction)?;
        let working_revision = state.working_revision.saturating_add(1);
        let working_version_id = new_uuid_v7();
        let mutation_id = new_uuid_v7();
        let outbox_transaction = begin_outbox_transaction(&transaction, 1)?;
        let title = title.trim();
        let canonical_hash = note_content_hash(title, body);

        let changed = transaction
            .execute(
                "UPDATE mobile_notes
                 SET title = ?1,
                     body = ?2,
                     updated_at = ?3,
                     working_revision = ?4,
                     working_version_id = ?5,
                     working_base_revision = accepted_revision,
                     pending_mutation_id = ?6,
                     sync_state = 'pending',
                     canonical_hash = ?7,
                     last_modified_device_id = ?8
                 WHERE record_id = ?9 AND lifecycle_state = 'active'",
                params![
                    title,
                    body,
                    timestamp,
                    working_revision,
                    working_version_id,
                    mutation_id,
                    canonical_hash,
                    identity.device_id,
                    record_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err(format!("note {record_id} does not exist"));
        }

        enqueue_mutation(
            &transaction,
            &identity,
            &outbox_transaction,
            0,
            Mutation {
                operation: "update",
                record_id: &state.record_id,
                title,
                body,
                base_revision: state.accepted_revision,
                proposed_revision: state.accepted_revision.saturating_add(1),
                local_revision: working_revision,
                version_id: &working_version_id,
                branch_id: &state.working_branch_id,
                base_version_id: state.accepted_version_id.as_deref(),
                accepted_content_hash: state.accepted_content_hash.as_deref(),
                mutation_id: &mutation_id,
                canonical_hash: &canonical_hash,
                lifecycle_state: "active",
                trashed_at: None,
                tombstoned_at: None,
                created_at: state.created_at,
                updated_at: timestamp,
                provenance_json: &state.provenance_json,
                scope_id: &state.scope_id,
                scope_class: &state.scope_class,
            },
        )?;

        let note = transaction
            .query_row(
                "SELECT record_id, title, body, created_at, updated_at
                 FROM mobile_notes WHERE record_id = ?1",
                [record_id],
                note_from_row,
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(note)
    }

    pub fn delete(&self, record_id: &str) -> Result<(), String> {
        self.set_lifecycle(record_id, "trash", "trash")
    }

    #[allow(dead_code)]
    pub fn restore(&self, record_id: &str) -> Result<MobileNote, String> {
        self.set_lifecycle(record_id, "active", "restore")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        connection
            .query_row(
                "SELECT record_id, title, body, created_at, updated_at
                 FROM mobile_notes WHERE record_id = ?1 AND lifecycle_state = 'active'",
                [record_id],
                note_from_row,
            )
            .map_err(|error| error.to_string())
    }

    /// Permanently removes the record's content from normal product surfaces
    /// while retaining its portable tombstone. No row is physically deleted.
    #[allow(dead_code)]
    pub fn tombstone(&self, record_id: &str) -> Result<(), String> {
        self.set_lifecycle(record_id, "tombstone", "tombstone")
    }

    /// Recreates the disposable full-text index from authoritative note rows.
    /// This never changes a portable record, revision, or outbox mutation.
    #[allow(dead_code)]
    pub fn rebuild_search_index(&self) -> Result<(), String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        rebuild_mobile_search_schema(&mut connection)?;
        verify_mobile_search_schema(&connection)
    }

    /// Produces a deterministic, checksummed JSON backup containing portable
    /// UUID references only. SQLite row IDs and local filesystem paths are not
    /// part of this format.
    #[allow(dead_code)]
    pub fn export_notes(&self) -> Result<String, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        verify_current_mobile_schema(&connection)?;
        verify_mobile_search_schema(&connection)?;
        let payload = read_mobile_notes_export(&connection)?;
        validate_mobile_notes_export(&payload)?;
        let payload_value = serde_json::to_value(&payload).map_err(|error| error.to_string())?;
        serde_json::to_string_pretty(&MobileNotesExportEnvelope {
            format: MOBILE_NOTES_EXPORT_FORMAT.to_string(),
            format_version: MOBILE_NOTES_EXPORT_VERSION,
            payload,
            payload_sha256: canonical_sha256(&payload_value),
        })
        .map_err(|error| error.to_string())
    }

    /// Restores an export into an empty initialized store. Portable record
    /// identity is retained, while this installation keeps its newly generated
    /// device identity and requires explicit re-enrollment before sync.
    #[allow(dead_code)]
    pub fn restore_notes_export(&self, export_json: &str) -> Result<usize, String> {
        if export_json.len() > MAX_MOBILE_NOTES_EXPORT_BYTES {
            return Err("mobile notes export exceeds the restore size limit".to_string());
        }
        let envelope: MobileNotesExportEnvelope = serde_json::from_str(export_json)
            .map_err(|error| format!("invalid mobile notes export: {error}"))?;
        if envelope.format != MOBILE_NOTES_EXPORT_FORMAT
            || envelope.format_version != MOBILE_NOTES_EXPORT_VERSION
        {
            return Err("unsupported mobile notes export format".to_string());
        }
        let payload_value =
            serde_json::to_value(&envelope.payload).map_err(|error| error.to_string())?;
        if !is_sha256(&envelope.payload_sha256)
            || canonical_sha256(&payload_value) != envelope.payload_sha256
        {
            return Err("mobile notes export payload checksum does not match".to_string());
        }
        validate_mobile_notes_export(&envelope.payload)?;

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        verify_current_mobile_schema(&connection)?;
        verify_mobile_search_schema(&connection)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let occupied: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mobile_notes)
                     OR EXISTS(SELECT 1 FROM mobile_note_outbox)",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if occupied {
            return Err(
                "mobile notes restore requires an empty store and will not overwrite existing records"
                    .to_string(),
            );
        }
        let fresh_replica = replica_identity(&transaction)?;
        let restored_at = envelope
            .payload
            .notes
            .iter()
            .map(|note| note.updated_at)
            .chain(envelope.payload.outbox.iter().flat_map(|outbox| {
                [
                    Some(outbox.created_at),
                    outbox.superseded_at,
                    outbox.acknowledged_at,
                ]
                .into_iter()
                .flatten()
            }))
            .max()
            .unwrap_or(0)
            .max(now_millis()?);
        write_mobile_notes_export(
            &transaction,
            &envelope.payload,
            &fresh_replica.device_id,
            &fresh_replica.install_id,
            restored_at,
        )?;
        validate_replica_identity(&replica_identity(&transaction)?)?;
        validate_portable_notes(&transaction)?;
        validate_outbox_transaction_groups(&transaction)?;
        validate_restored_export_links(&transaction)?;
        transaction.commit().map_err(|error| error.to_string())?;
        verify_mobile_search_schema(&connection)?;
        Ok(envelope.payload.notes.len())
    }

    /// Attach an unpaired phone's staging records to the library and default
    /// scope proven by the first pairing handshake. Record IDs are retained;
    /// the staging-only scope is remapped to the Mac's canonical scope ID.
    #[allow(dead_code)]
    pub fn adopt_staging_library(
        &self,
        mac_library_id: &str,
        mac_default_scope_id: &str,
    ) -> Result<usize, String> {
        if !is_uuid(mac_library_id) || !is_uuid(mac_default_scope_id) {
            return Err(
                "paired Mac library_id and default scope_id must be canonical UUIDs".to_string(),
            );
        }

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let staging_identity = replica_identity(&transaction)?;
        match staging_identity.library_state.as_str() {
            "paired"
                if staging_identity.library_id == mac_library_id
                    && staging_identity.default_scope_id == mac_default_scope_id =>
            {
                return Ok(0)
            }
            "paired" => {
                return Err(
                    "this iPhone is already paired to a different Noted library or scope"
                        .to_string(),
                )
            }
            "local_staging" => {}
            state => return Err(format!("unsupported mobile library state {state}")),
        }

        let has_externally_observed_state: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_notes
                   WHERE accepted_revision > 0
                      OR accepted_version_id IS NOT NULL
                      OR accepted_content_hash IS NOT NULL
                      OR sync_state IN ('sending', 'acknowledged', 'conflict')
                   UNION ALL
                   SELECT 1 FROM mobile_note_outbox
                   WHERE state IN ('sending', 'acknowledged', 'conflict')
                      OR attempts > 0
                      OR acknowledged_at IS NOT NULL
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if has_externally_observed_state {
            return Err(
                "staging-library adoption is forbidden after a record has been accepted or exposed to sync"
                    .to_string(),
            );
        }

        struct AdoptionNote {
            id: i64,
            record_id: String,
            title: String,
            body: String,
            created_at: i64,
            updated_at: i64,
            working_revision: i64,
            working_branch_id: String,
            canonical_hash: String,
            lifecycle_state: String,
            trashed_at: Option<i64>,
            tombstoned_at: Option<i64>,
            provenance_json: String,
            scope_id: String,
            scope_class: String,
        }

        let notes = {
            let mut statement = transaction
                .prepare(
                    "SELECT id, record_id, title, body, created_at, updated_at,
                            working_revision, working_branch_id, canonical_hash, lifecycle_state,
                            trashed_at, tombstoned_at, provenance_json, scope_id, scope_class
                     FROM mobile_notes ORDER BY id",
                )
                .map_err(|error| error.to_string())?;
            let mapped = statement
                .query_map([], |row| {
                    Ok(AdoptionNote {
                        id: row.get(0)?,
                        record_id: row.get(1)?,
                        title: row.get(2)?,
                        body: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                        working_revision: row.get(6)?,
                        working_branch_id: row.get(7)?,
                        canonical_hash: row.get(8)?,
                        lifecycle_state: row.get(9)?,
                        trashed_at: row.get(10)?,
                        tombstoned_at: row.get(11)?,
                        provenance_json: row.get(12)?,
                        scope_id: row.get(13)?,
                        scope_class: row.get(14)?,
                    })
                })
                .map_err(|error| error.to_string())?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };

        // Staging operations have never crossed a transport boundary. Preserve
        // them as an audit trail, mark them ineligible, and emit one new create
        // representing each record's current state in the paired Mac library.
        let adoption_time = next_timestamp(&transaction)?;
        transaction
            .execute(
                "UPDATE mobile_note_outbox
                 SET state = 'superseded', eligible_for_sync = 0, superseded_at = ?1
                 WHERE library_id = ?2 AND eligible_for_sync = 1",
                params![adoption_time, staging_identity.library_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_replica
                 SET library_id = ?1, default_scope_id = ?2, library_state = 'paired'
                 WHERE singleton = 1 AND library_state = 'local_staging'",
                params![mac_library_id, mac_default_scope_id],
            )
            .map_err(|error| error.to_string())?;
        let paired_identity = replica_identity(&transaction)?;

        let outbox_transaction = if notes.is_empty() {
            None
        } else {
            Some(begin_outbox_transaction(&transaction, notes.len())?)
        };
        for (member_index, note) in notes.iter().enumerate() {
            let scope_id = if note.scope_id == staging_identity.default_scope_id {
                mac_default_scope_id
            } else {
                &note.scope_id
            };
            let working_revision = note.working_revision.saturating_add(1);
            let working_version_id = new_uuid_v7();
            let mutation_id = new_uuid_v7();
            transaction
                .execute(
                    "UPDATE mobile_notes
                     SET library_id = ?1,
                         accepted_revision = 0,
                         accepted_version_id = NULL,
                         accepted_content_hash = NULL,
                         working_revision = ?2,
                    working_version_id = ?3,
                         working_base_revision = 0,
                         pending_mutation_id = ?4,
                         sync_state = 'pending',
                         scope_id = ?5
                     WHERE id = ?6",
                    params![
                        mac_library_id,
                        working_revision,
                        working_version_id,
                        mutation_id,
                        scope_id,
                        note.id
                    ],
                )
                .map_err(|error| error.to_string())?;
            enqueue_mutation(
                &transaction,
                &paired_identity,
                outbox_transaction
                    .as_ref()
                    .expect("non-empty adoption has an outbox transaction"),
                i64::try_from(member_index)
                    .map_err(|_| "outbox transaction has too many members".to_string())?,
                Mutation {
                    operation: "create",
                    record_id: &note.record_id,
                    title: &note.title,
                    body: &note.body,
                    base_revision: 0,
                    proposed_revision: 1,
                    local_revision: working_revision,
                    version_id: &working_version_id,
                    branch_id: &note.working_branch_id,
                    base_version_id: None,
                    accepted_content_hash: None,
                    mutation_id: &mutation_id,
                    canonical_hash: &note.canonical_hash,
                    lifecycle_state: &note.lifecycle_state,
                    trashed_at: note.trashed_at,
                    tombstoned_at: note.tombstoned_at,
                    created_at: note.created_at,
                    updated_at: note.updated_at,
                    provenance_json: &note.provenance_json,
                    scope_id,
                    scope_class: &note.scope_class,
                },
            )?;
        }

        transaction.commit().map_err(|error| error.to_string())?;
        Ok(notes.len())
    }

    fn set_lifecycle(
        &self,
        record_id: &str,
        lifecycle: &str,
        operation: &str,
    ) -> Result<(), String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let identity = replica_identity(&transaction)?;
        if !is_uuid_v7(record_id) {
            return Err("note record_id must be a canonical UUIDv7".to_string());
        }
        let state = portable_state(&transaction, record_id)?
            .ok_or_else(|| format!("note {record_id} does not exist"))?;
        ensure_noted_authority(&state)?;
        let expected_state = match lifecycle {
            "trash" => "active",
            "active" => "trash",
            "tombstone" => "trash",
            state => return Err(format!("unsupported mobile note lifecycle {state}")),
        };
        if state.lifecycle_state != expected_state {
            return Err(format!("note {record_id} does not exist"));
        }

        let (title, body): (String, String) = transaction
            .query_row(
                "SELECT title, body FROM mobile_notes WHERE record_id = ?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let timestamp = next_timestamp(&transaction)?;
        let working_revision = state.working_revision.saturating_add(1);
        let working_version_id = new_uuid_v7();
        let mutation_id = new_uuid_v7();
        let outbox_transaction = begin_outbox_transaction(&transaction, 1)?;
        let (trashed_at, tombstoned_at) =
            match lifecycle {
                "active" => (None, None),
                "trash" => (Some(timestamp), None),
                "tombstone" => (
                    Some(state.trashed_at.ok_or_else(|| {
                        format!("note {record_id} is missing its trash timestamp")
                    })?),
                    Some(timestamp),
                ),
                _ => unreachable!("lifecycle was validated above"),
            };
        let canonical_hash = note_content_hash(&title, &body);

        let changed = transaction
            .execute(
                "UPDATE mobile_notes
                 SET updated_at = ?1,
                     deleted_at = ?2,
                     lifecycle_state = ?3,
                     trashed_at = ?2,
                     tombstoned_at = ?4,
                     working_revision = ?5,
                     working_version_id = ?6,
                     working_base_revision = accepted_revision,
                     pending_mutation_id = ?7,
                     sync_state = 'pending',
                     canonical_hash = ?8,
                     last_modified_device_id = ?9
                 WHERE record_id = ?10 AND lifecycle_state = ?11",
                params![
                    timestamp,
                    trashed_at,
                    lifecycle,
                    tombstoned_at,
                    working_revision,
                    working_version_id,
                    mutation_id,
                    canonical_hash,
                    identity.device_id,
                    record_id,
                    expected_state,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err(format!("note {record_id} does not exist"));
        }

        enqueue_mutation(
            &transaction,
            &identity,
            &outbox_transaction,
            0,
            Mutation {
                operation,
                record_id: &state.record_id,
                title: &title,
                body: &body,
                base_revision: state.accepted_revision,
                proposed_revision: state.accepted_revision.saturating_add(1),
                local_revision: working_revision,
                version_id: &working_version_id,
                branch_id: &state.working_branch_id,
                base_version_id: state.accepted_version_id.as_deref(),
                accepted_content_hash: state.accepted_content_hash.as_deref(),
                mutation_id: &mutation_id,
                canonical_hash: &canonical_hash,
                lifecycle_state: lifecycle,
                trashed_at,
                tombstoned_at,
                created_at: state.created_at,
                updated_at: timestamp,
                provenance_json: &state.provenance_json,
                scope_id: &state.scope_id,
                scope_class: &state.scope_class,
            },
        )?;
        transaction.commit().map_err(|error| error.to_string())
    }
}

fn ensure_mobile_search_schema(connection: &mut Connection) -> Result<(), String> {
    let has_index: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'mobile_notes_fts'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    create_mobile_search_objects(&transaction)?;
    if !has_index {
        populate_mobile_search_index(&transaction)?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn rebuild_mobile_search_schema(connection: &mut Connection) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "DROP TRIGGER IF EXISTS mobile_notes_fts_insert;
             DROP TRIGGER IF EXISTS mobile_notes_fts_update;
             DROP TRIGGER IF EXISTS mobile_notes_fts_delete;
             DROP TABLE IF EXISTS mobile_notes_fts;",
        )
        .map_err(|error| error.to_string())?;
    create_mobile_search_objects(&transaction)?;
    populate_mobile_search_index(&transaction)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn create_mobile_search_objects(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS mobile_notes_fts USING fts5(
               record_id UNINDEXED,
               title,
               body,
               tokenize = 'unicode61 remove_diacritics 2'
             );
             CREATE TRIGGER IF NOT EXISTS mobile_notes_fts_insert
             AFTER INSERT ON mobile_notes
             WHEN NEW.lifecycle_state = 'active' AND NEW.deleted_at IS NULL
             BEGIN
               INSERT INTO mobile_notes_fts(record_id, title, body)
               VALUES (NEW.record_id, NEW.title, NEW.body);
             END;
             CREATE TRIGGER IF NOT EXISTS mobile_notes_fts_update
             AFTER UPDATE OF record_id, title, body, lifecycle_state, deleted_at ON mobile_notes
             BEGIN
               DELETE FROM mobile_notes_fts WHERE record_id = OLD.record_id;
               INSERT INTO mobile_notes_fts(record_id, title, body)
               SELECT NEW.record_id, NEW.title, NEW.body
               WHERE NEW.lifecycle_state = 'active' AND NEW.deleted_at IS NULL;
             END;
             CREATE TRIGGER IF NOT EXISTS mobile_notes_fts_delete
             AFTER DELETE ON mobile_notes
             BEGIN
               DELETE FROM mobile_notes_fts WHERE record_id = OLD.record_id;
             END;",
        )
        .map_err(|error| format!("create mobile full-text search index: {error}"))
}

fn populate_mobile_search_index(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO mobile_notes_fts(record_id, title, body)
             SELECT record_id, title, body
             FROM mobile_notes
             WHERE lifecycle_state = 'active' AND deleted_at IS NULL
             ORDER BY record_id",
            [],
        )
        .map(|_| ())
        .map_err(|error| format!("populate mobile full-text search index: {error}"))
}

fn verify_mobile_search_schema(connection: &Connection) -> Result<(), String> {
    let table_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'mobile_notes_fts'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if table_sql
        .as_deref()
        .is_none_or(|sql| !sql.to_ascii_lowercase().contains("using fts5"))
    {
        return Err("mobile full-text search index is missing or invalid".to_string());
    }
    let trigger_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger'
               AND name IN (
                 'mobile_notes_fts_insert',
                 'mobile_notes_fts_update',
                 'mobile_notes_fts_delete'
               )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if trigger_count != 3 {
        return Err("mobile full-text search maintenance triggers are missing".to_string());
    }
    let divergence: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM (
                  SELECT record_id, title, body FROM mobile_notes_fts
                  EXCEPT
                  SELECT record_id, title, body FROM mobile_notes
                  WHERE lifecycle_state = 'active' AND deleted_at IS NULL
                ))
               +
               (SELECT COUNT(*) FROM (
                  SELECT record_id, title, body FROM mobile_notes
                  WHERE lifecycle_state = 'active' AND deleted_at IS NULL
                  EXCEPT
                  SELECT record_id, title, body FROM mobile_notes_fts
                ))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let duplicate_records: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (
               SELECT record_id FROM mobile_notes_fts
               GROUP BY record_id HAVING COUNT(*) != 1
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if divergence != 0 || duplicate_records != 0 {
        return Err("mobile full-text search index diverged from active notes".to_string());
    }
    Ok(())
}

fn read_mobile_notes_export(connection: &Connection) -> Result<MobileNotesExportPayload, String> {
    let replica = connection
        .query_row(
            "SELECT library_id, device_id, install_id, default_scope_id,
                    library_state, next_transaction_counter, created_at
             FROM mobile_replica WHERE singleton = 1",
            [],
            |row| {
                Ok(MobileReplicaExport {
                    library_id: row.get(0)?,
                    device_id: row.get(1)?,
                    install_id: row.get(2)?,
                    default_scope_id: row.get(3)?,
                    library_state: row.get(4)?,
                    next_transaction_counter: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(|error| error.to_string())?;
    let notes = connection
        .prepare(
            "SELECT library_id, record_id, record_kind, record_schema_version,
                    title, body, created_at, updated_at,
                    accepted_revision, accepted_version_id, accepted_content_hash,
                    working_revision, working_branch_id, working_version_id,
                    working_base_revision, pending_mutation_id, sync_state,
                    lifecycle_state, trashed_at, tombstoned_at, canonical_hash,
                    authority, scope, scope_id, scope_class, sensitivity,
                    provenance_json, origin_device_id, last_modified_device_id,
                    origin_install_id
             FROM mobile_notes ORDER BY record_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    let provenance_json: String = row.get(26)?;
                    Ok(MobileNoteExport {
                        library_id: row.get(0)?,
                        record_id: row.get(1)?,
                        record_kind: row.get(2)?,
                        record_schema_version: row.get(3)?,
                        title: row.get(4)?,
                        body: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        accepted_revision: row.get(8)?,
                        accepted_version_id: row.get(9)?,
                        accepted_content_hash: row.get(10)?,
                        working_revision: row.get(11)?,
                        working_branch_id: row.get(12)?,
                        working_version_id: row.get(13)?,
                        working_base_revision: row.get(14)?,
                        pending_mutation_id: row.get(15)?,
                        sync_state: row.get(16)?,
                        lifecycle_state: row.get(17)?,
                        trashed_at: row.get(18)?,
                        tombstoned_at: row.get(19)?,
                        canonical_hash: row.get(20)?,
                        authority: row.get(21)?,
                        scope: row.get(22)?,
                        scope_id: row.get(23)?,
                        scope_class: row.get(24)?,
                        sensitivity: row.get(25)?,
                        provenance: serde_json::from_str(&provenance_json)
                            .unwrap_or(serde_json::Value::Null),
                        origin_device_id: row.get(27)?,
                        last_modified_device_id: row.get(28)?,
                        origin_install_id: row.get(29)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let outbox = connection
        .prepare(
            "SELECT mutation_id, transaction_id, device_transaction_counter,
                    transaction_member_index, transaction_member_count,
                    library_id, device_id, install_id, scope_id, scope_class,
                    record_id, record_kind, operation, base_revision,
                    base_version_id, proposed_revision, local_revision,
                    branch_id, version_id, canonical_hash, payload_json,
                    state, eligible_for_sync, superseded_at, attempts,
                    created_at, acknowledged_at
             FROM mobile_note_outbox
             ORDER BY device_transaction_counter, transaction_member_index, mutation_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    let payload_json: String = row.get(20)?;
                    Ok(MobileOutboxExport {
                        mutation_id: row.get(0)?,
                        transaction_id: row.get(1)?,
                        device_transaction_counter: row.get(2)?,
                        transaction_member_index: row.get(3)?,
                        transaction_member_count: row.get(4)?,
                        library_id: row.get(5)?,
                        device_id: row.get(6)?,
                        install_id: row.get(7)?,
                        scope_id: row.get(8)?,
                        scope_class: row.get(9)?,
                        record_id: row.get(10)?,
                        record_kind: row.get(11)?,
                        operation: row.get(12)?,
                        base_revision: row.get(13)?,
                        base_version_id: row.get(14)?,
                        proposed_revision: row.get(15)?,
                        local_revision: row.get(16)?,
                        branch_id: row.get(17)?,
                        version_id: row.get(18)?,
                        canonical_hash: row.get(19)?,
                        payload: serde_json::from_str(&payload_json)
                            .unwrap_or(serde_json::Value::Null),
                        state: row.get(21)?,
                        eligible_for_sync: row.get::<_, i64>(22)? != 0,
                        superseded_at: row.get(23)?,
                        attempts: row.get(24)?,
                        created_at: row.get(25)?,
                        acknowledged_at: row.get(26)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    Ok(MobileNotesExportPayload {
        replica,
        notes,
        outbox,
    })
}

fn write_mobile_notes_export(
    transaction: &Transaction<'_>,
    payload: &MobileNotesExportPayload,
    restored_device_id: &str,
    restored_install_id: &str,
    restored_at: i64,
) -> Result<(), String> {
    transaction
        .execute("DELETE FROM mobile_replica WHERE singleton = 1", [])
        .map_err(|error| error.to_string())?;
    let replica = &payload.replica;
    transaction
        .execute(
            "INSERT INTO mobile_replica (
               singleton, library_id, device_id, install_id, default_scope_id,
               library_state, next_transaction_counter, created_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                replica.library_id,
                restored_device_id,
                restored_install_id,
                replica.default_scope_id,
                "local_staging",
                1_i64,
                restored_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    for note in &payload.notes {
        let provenance_json =
            serde_json::to_string(&note.provenance).map_err(|error| error.to_string())?;
        let deleted_at = match note.lifecycle_state.as_str() {
            "active" => None,
            "trash" | "tombstone" => note.trashed_at,
            _ => return Err("mobile notes export contains an invalid lifecycle".to_string()),
        };
        transaction
            .execute(
                "INSERT INTO mobile_notes (
                   title, body, created_at, updated_at, deleted_at,
                   library_id, record_id, record_kind, record_schema_version,
                   accepted_revision, accepted_version_id, accepted_content_hash,
                   working_revision, working_branch_id, working_version_id,
                   working_base_revision, pending_mutation_id, sync_state,
                   lifecycle_state, trashed_at, tombstoned_at, canonical_hash,
                   authority, scope, scope_id, scope_class, sensitivity,
                   provenance_json, origin_device_id, last_modified_device_id,
                   origin_install_id
                 ) VALUES (
                   ?1, ?2, ?3, ?4, ?5,
                   ?6, ?7, ?8, ?9,
                   ?10, ?11, ?12,
                   ?13, ?14, ?15,
                   ?16, ?17, ?18,
                   ?19, ?20, ?21, ?22,
                   ?23, ?24, ?25, ?26, ?27,
                   ?28, ?29, ?30, ?31
                 )",
                params![
                    note.title,
                    note.body,
                    note.created_at,
                    note.updated_at,
                    deleted_at,
                    note.library_id,
                    note.record_id,
                    note.record_kind,
                    note.record_schema_version,
                    note.accepted_revision,
                    note.accepted_version_id,
                    note.accepted_content_hash,
                    note.working_revision,
                    note.working_branch_id,
                    note.working_version_id,
                    note.working_base_revision,
                    note.pending_mutation_id,
                    "restore_pending",
                    note.lifecycle_state,
                    note.trashed_at,
                    note.tombstoned_at,
                    note.canonical_hash,
                    note.authority,
                    note.scope,
                    note.scope_id,
                    note.scope_class,
                    note.sensitivity,
                    provenance_json,
                    note.origin_device_id,
                    note.last_modified_device_id,
                    note.origin_install_id,
                ],
            )
            .map_err(|error| format!("restore mobile note {}: {error}", note.record_id))?;
    }
    for outbox in &payload.outbox {
        let payload_json =
            serde_json::to_string(&outbox.payload).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO mobile_note_outbox (
                   mutation_id, transaction_id, device_transaction_counter,
                   transaction_member_index, transaction_member_count,
                   library_id, device_id, install_id, scope_id, scope_class,
                   record_id, record_kind, operation, base_revision,
                   base_version_id, proposed_revision, local_revision,
                   branch_id, version_id, canonical_hash, payload_json,
                   state, eligible_for_sync, superseded_at, attempts,
                   created_at, acknowledged_at
                 ) VALUES (
                   ?1, ?2, ?3, ?4, ?5,
                   ?6, ?7, ?8, ?9, ?10,
                   ?11, ?12, ?13, ?14,
                   ?15, ?16, ?17,
                   ?18, ?19, ?20, ?21,
                   ?22, ?23, ?24, ?25,
                   ?26, ?27
                 )",
                params![
                    outbox.mutation_id,
                    outbox.transaction_id,
                    outbox.device_transaction_counter,
                    outbox.transaction_member_index,
                    outbox.transaction_member_count,
                    outbox.library_id,
                    outbox.device_id,
                    outbox.install_id,
                    outbox.scope_id,
                    outbox.scope_class,
                    outbox.record_id,
                    outbox.record_kind,
                    outbox.operation,
                    outbox.base_revision,
                    outbox.base_version_id,
                    outbox.proposed_revision,
                    outbox.local_revision,
                    outbox.branch_id,
                    outbox.version_id,
                    outbox.canonical_hash,
                    payload_json,
                    "superseded",
                    0_i64,
                    Some(restored_at),
                    outbox.attempts,
                    outbox.created_at,
                    outbox.acknowledged_at,
                ],
            )
            .map_err(|error| format!("restore mobile outbox mutation: {error}"))?;
    }
    Ok(())
}

fn validate_mobile_notes_export(payload: &MobileNotesExportPayload) -> Result<(), String> {
    let replica = &payload.replica;
    for (label, value) in [
        ("libraryId", replica.library_id.as_str()),
        ("deviceId", replica.device_id.as_str()),
        ("installId", replica.install_id.as_str()),
        ("defaultScopeId", replica.default_scope_id.as_str()),
    ] {
        if !is_uuid(value) {
            return Err(format!(
                "mobile notes export {label} is not a canonical UUID"
            ));
        }
    }
    if !matches!(replica.library_state.as_str(), "local_staging" | "paired")
        || replica.next_transaction_counter <= 0
        || replica.created_at < 0
    {
        return Err("mobile notes export replica state is invalid".to_string());
    }

    let mut record_ids = BTreeSet::new();
    for note in &payload.notes {
        if !record_ids.insert(note.record_id.as_str()) {
            return Err(format!(
                "mobile notes export repeats record {}",
                note.record_id
            ));
        }
        validate_mobile_note_export(note, replica)?;
    }

    let note_by_id = payload
        .notes
        .iter()
        .map(|note| (note.record_id.as_str(), note))
        .collect::<BTreeMap<_, _>>();
    let mut mutation_ids = BTreeSet::new();
    let mut eligible_counts = BTreeMap::<&str, usize>::new();
    let mut max_transaction_counter = 0_i64;
    for outbox in &payload.outbox {
        if !mutation_ids.insert(outbox.mutation_id.as_str()) {
            return Err("mobile notes export repeats an outbox mutation UUID".to_string());
        }
        let note = note_by_id.get(outbox.record_id.as_str()).ok_or_else(|| {
            format!(
                "mobile notes export mutation references missing record {}",
                outbox.record_id
            )
        })?;
        validate_mobile_outbox_export(outbox, replica)?;
        if outbox.eligible_for_sync {
            *eligible_counts
                .entry(outbox.record_id.as_str())
                .or_default() += 1;
            if outbox.mutation_id != note.pending_mutation_id
                || outbox.local_revision != note.working_revision
                || outbox.branch_id != note.working_branch_id
                || outbox.version_id != note.working_version_id
                || outbox.canonical_hash != note.canonical_hash
                || outbox.library_id != note.library_id
                || outbox.scope_id != note.scope_id
                || outbox.scope_class != note.scope_class
            {
                return Err(format!(
                    "mobile notes export current mutation does not match record {}",
                    note.record_id
                ));
            }
        }
        if outbox.device_id == replica.device_id {
            max_transaction_counter =
                max_transaction_counter.max(outbox.device_transaction_counter);
        }
    }
    if eligible_counts.values().any(|count| *count != 1) {
        return Err("mobile notes export has competing eligible record branches".to_string());
    }
    for note in &payload.notes {
        if note.sync_state == "pending"
            && eligible_counts.get(note.record_id.as_str()).copied() != Some(1)
        {
            return Err(format!(
                "mobile notes export pending record {} has no eligible mutation",
                note.record_id
            ));
        }
    }
    if replica.next_transaction_counter <= max_transaction_counter {
        return Err(
            "mobile notes export transaction counter would reuse an existing value".to_string(),
        );
    }
    Ok(())
}

fn validate_mobile_note_export(
    note: &MobileNoteExport,
    replica: &MobileReplicaExport,
) -> Result<(), String> {
    if note.library_id != replica.library_id
        || note.record_kind != "note"
        || note.record_schema_version != 1
        || !is_uuid_v7(&note.record_id)
        || !is_uuid(&note.working_branch_id)
        || !is_uuid(&note.working_version_id)
        || !is_uuid(&note.pending_mutation_id)
        || !is_uuid(&note.scope_id)
        || !is_uuid(&note.origin_device_id)
        || !is_uuid(&note.last_modified_device_id)
        || !is_uuid(&note.origin_install_id)
    {
        return Err(format!(
            "mobile notes export record {} has invalid portable identity",
            note.record_id
        ));
    }
    if note.created_at < 0
        || note.updated_at < note.created_at
        || note.working_revision <= 0
        || note.working_base_revision != note.accepted_revision
        || !matches!(
            note.sync_state.as_str(),
            "pending" | "sending" | "acknowledged" | "conflict" | "clean" | "restore_pending"
        )
        || !matches!(note.authority.as_str(), "noted" | "external" | "derived")
        || !matches!(note.scope_class.as_str(), "work" | "personal" | "unknown")
        || !matches!(
            note.sensitivity.as_str(),
            "standard" | "sensitive" | "restricted"
        )
        || note.scope.trim().is_empty()
        || !note.provenance.is_object()
        || !is_sha256(&note.canonical_hash)
        || note.canonical_hash != note_content_hash(&note.title, &note.body)
    {
        return Err(format!(
            "mobile notes export record {} has invalid portable state",
            note.record_id
        ));
    }
    match (
        note.accepted_revision,
        note.accepted_version_id.as_deref(),
        note.accepted_content_hash.as_deref(),
    ) {
        (0, None, None) => {}
        (revision, Some(version_id), Some(content_hash))
            if revision > 0 && is_uuid(version_id) && is_sha256(content_hash) => {}
        _ => {
            return Err(format!(
                "mobile notes export record {} has an incoherent accepted head",
                note.record_id
            ))
        }
    }
    let lifecycle_ok = match note.lifecycle_state.as_str() {
        "active" => note.trashed_at.is_none() && note.tombstoned_at.is_none(),
        "trash" => {
            note.trashed_at
                .is_some_and(|trashed_at| trashed_at >= note.created_at)
                && note.tombstoned_at.is_none()
        }
        "tombstone" => match (note.trashed_at, note.tombstoned_at) {
            (Some(trashed_at), Some(tombstoned_at)) => {
                trashed_at >= note.created_at && tombstoned_at >= trashed_at
            }
            _ => false,
        },
        _ => false,
    };
    if !lifecycle_ok {
        return Err(format!(
            "mobile notes export record {} has an invalid lifecycle",
            note.record_id
        ));
    }
    Ok(())
}

fn validate_mobile_outbox_export(
    outbox: &MobileOutboxExport,
    replica: &MobileReplicaExport,
) -> Result<(), String> {
    if !is_uuid(&outbox.mutation_id)
        || !is_uuid(&outbox.transaction_id)
        || !is_uuid(&outbox.library_id)
        || !is_uuid(&outbox.device_id)
        || !is_uuid(&outbox.install_id)
        || !is_uuid(&outbox.scope_id)
        || !is_uuid_v7(&outbox.record_id)
        || !is_uuid(&outbox.branch_id)
        || !is_uuid(&outbox.version_id)
        || outbox
            .base_version_id
            .as_deref()
            .is_some_and(|id| !is_uuid(id))
        || outbox.record_kind != "note"
        || !matches!(outbox.scope_class.as_str(), "work" | "personal" | "unknown")
        || !matches!(
            outbox.operation.as_str(),
            "create" | "update" | "trash" | "tombstone" | "restore"
        )
        || !matches!(
            outbox.state.as_str(),
            "pending" | "superseded" | "sending" | "acknowledged" | "conflict"
        )
        || outbox.device_transaction_counter <= 0
        || outbox.transaction_member_index < 0
        || outbox.transaction_member_count <= 0
        || outbox.transaction_member_index >= outbox.transaction_member_count
        || outbox.base_revision < 0
        || outbox.proposed_revision <= 0
        || outbox.local_revision <= 0
        || !is_sha256(&outbox.canonical_hash)
        || outbox.attempts < 0
        || outbox.created_at < 0
        || outbox
            .superseded_at
            .is_some_and(|timestamp| timestamp < outbox.created_at)
        || outbox
            .acknowledged_at
            .is_some_and(|timestamp| timestamp < outbox.created_at)
    {
        return Err(format!(
            "mobile notes export mutation {} is invalid",
            outbox.mutation_id
        ));
    }
    if outbox.eligible_for_sync
        && (outbox.library_id != replica.library_id
            || outbox.device_id != replica.device_id
            || outbox.install_id != replica.install_id)
    {
        return Err(format!(
            "mobile notes export current mutation {} does not belong to its replica",
            outbox.mutation_id
        ));
    }
    if outbox.base_revision == 0 && outbox.base_version_id.is_some()
        || outbox.base_revision > 0 && outbox.base_version_id.is_none()
    {
        return Err(format!(
            "mobile notes export mutation {} has an incoherent base head",
            outbox.mutation_id
        ));
    }
    validate_export_mutation_payload(outbox)
}

fn validate_export_mutation_payload(outbox: &MobileOutboxExport) -> Result<(), String> {
    let payload = outbox.payload.as_object().ok_or_else(|| {
        format!(
            "mobile notes export mutation {} payload is not an object",
            outbox.mutation_id
        )
    })?;
    let proposed = payload
        .get("proposed_record")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "mobile notes export mutation is missing proposed_record".to_string())?;
    let branch = proposed
        .get("local_branch")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "mobile notes export mutation is missing local_branch".to_string())?;
    let content = proposed
        .get("content")
        .ok_or_else(|| "mobile notes export mutation is missing content".to_string())?;
    let expected_updated_at = rfc3339_from_millis(outbox.created_at);
    let valid = payload
        .get("mutation_contract_version")
        .and_then(serde_json::Value::as_str)
        == Some("noted.mobile-note-mutation.shadow.v1")
        && payload.get("operation").and_then(serde_json::Value::as_str)
            == Some(outbox.operation.as_str())
        && payload
            .get("proposed_revision")
            .and_then(serde_json::Value::as_i64)
            == Some(outbox.proposed_revision)
        && proposed
            .get("proposal_contract_version")
            .and_then(serde_json::Value::as_str)
            == Some("noted.proposed-record.v1")
        && proposed
            .get("library_id")
            .and_then(serde_json::Value::as_str)
            == Some(outbox.library_id.as_str())
        && proposed
            .get("record_id")
            .and_then(serde_json::Value::as_str)
            == Some(outbox.record_id.as_str())
        && proposed.get("kind").and_then(serde_json::Value::as_str) == Some("note")
        && proposed
            .get("record_schema_version")
            .and_then(serde_json::Value::as_i64)
            == Some(1)
        && proposed
            .get("content_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outbox.canonical_hash.as_str())
        && canonical_sha256(content) == outbox.canonical_hash
        && proposed
            .get("updated_at")
            .and_then(serde_json::Value::as_str)
            == Some(expected_updated_at.as_str())
        && proposed
            .get("scope")
            .and_then(|scope| scope.get("scope_id"))
            .and_then(serde_json::Value::as_str)
            == Some(outbox.scope_id.as_str())
        && branch.get("branch_id").and_then(serde_json::Value::as_str)
            == Some(outbox.branch_id.as_str())
        && branch
            .get("working_version_id")
            .and_then(serde_json::Value::as_str)
            == Some(outbox.version_id.as_str())
        && branch
            .get("base_revision")
            .and_then(serde_json::Value::as_i64)
            == Some(outbox.base_revision)
        && branch
            .get("local_revision")
            .and_then(serde_json::Value::as_i64)
            == Some(outbox.local_revision)
        && branch
            .get("content_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outbox.canonical_hash.as_str());
    if !valid {
        return Err(format!(
            "mobile notes export mutation {} payload does not match its portable envelope",
            outbox.mutation_id
        ));
    }
    Ok(())
}

fn validate_restored_export_links(connection: &Connection) -> Result<(), String> {
    let invalid_current_mutations: i64 = connection
        .query_row(
            "SELECT COUNT(*)
             FROM mobile_note_outbox AS outbox
             LEFT JOIN mobile_notes AS notes ON notes.record_id = outbox.record_id
             WHERE notes.record_id IS NULL
                OR (outbox.eligible_for_sync = 1 AND (
                     notes.pending_mutation_id != outbox.mutation_id
                  OR notes.working_revision != outbox.local_revision
                  OR notes.working_branch_id != outbox.branch_id
                  OR notes.working_version_id != outbox.version_id
                  OR notes.canonical_hash != outbox.canonical_hash
                  OR notes.library_id != outbox.library_id
                  OR notes.scope_id != outbox.scope_id
                  OR notes.scope_class != outbox.scope_class
                ))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid_current_mutations != 0 {
        return Err("restored mobile notes contain invalid outbox references".to_string());
    }
    Ok(())
}

fn prepare_mobile_migration_recovery(
    database_path: &Path,
    connection: &Connection,
) -> Result<Option<PathBuf>, String> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if application_id != 0 && application_id != MOBILE_APPLICATION_ID {
        return Err(format!(
            "not an iPhone Noted database: application_id is {application_id:#010x}"
        ));
    }
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database schema {user_version} is newer than supported schema {PORTABLE_SCHEMA_VERSION}"
        ));
    }
    if user_version == PORTABLE_SCHEMA_VERSION {
        verify_current_mobile_schema(connection)?;
        return Ok(None);
    }
    if user_version < 0 {
        return Err("mobile database schema version cannot be negative".to_string());
    }
    if user_version == 1 {
        verify_mobile_schema_v1(connection)?;
    }
    if database_path == Path::new(":memory:") {
        return Ok(None);
    }

    let has_schema: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_schema {
        return Ok(None);
    }

    let parent = database_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "mobile database path has no recovery directory".to_string())?;
    let recovery_dir = parent.join("migration-recovery");
    std::fs::create_dir_all(&recovery_dir).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&recovery_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let stem = database_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("noted-iphone");
    let nonce = now_millis()?;
    let counter = RECOVERY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let destination = recovery_dir.join(format!(
        "{stem}-pre-schema-v{PORTABLE_SCHEMA_VERSION}-{}-{nonce}-{counter}.sqlite3",
        std::process::id()
    ));
    create_mobile_recovery_snapshot(connection, &destination)?;
    Ok(Some(destination))
}

fn create_mobile_recovery_snapshot(source: &Connection, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "mobile recovery destination already exists: {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "mobile recovery destination has no parent".to_string())?;
    let inventory = mobile_database_inventory(source)?;
    let staging = parent.join(format!(
        ".{}.staging-{}-{}",
        destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("noted-mobile-recovery"),
        std::process::id(),
        RECOVERY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if staging.exists() {
        return Err("mobile recovery staging path already exists".to_string());
    }
    let staging_text = staging
        .to_str()
        .ok_or_else(|| "mobile recovery path is not valid UTF-8".to_string())?;
    if let Err(error) = source.execute("VACUUM INTO ?1", [staging_text]) {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("create mobile recovery snapshot: {error}"));
    }
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        validate_mobile_recovery_snapshot(&staging, &inventory)?;
        File::open(&staging)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        std::fs::hard_link(&staging, destination).map_err(|error| error.to_string())?;
        std::fs::remove_file(&staging).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

fn validate_mobile_recovery_snapshot(
    path: &Path,
    expected_inventory: &BTreeMap<String, i64>,
) -> Result<(), String> {
    let snapshot = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    let quick_check = snapshot
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if quick_check.as_slice() != ["ok"] {
        return Err(format!(
            "mobile recovery quick_check failed: {}",
            quick_check.join("; ")
        ));
    }
    let has_foreign_key_error = snapshot
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| Ok(statement.query([])?.next()?.is_some()))
        .map_err(|error| error.to_string())?;
    if has_foreign_key_error {
        return Err("mobile recovery foreign_key_check failed".to_string());
    }
    if mobile_database_inventory(&snapshot)? != *expected_inventory {
        return Err("mobile recovery row inventory does not match source".to_string());
    }
    Ok(())
}

fn mobile_database_inventory(connection: &Connection) -> Result<BTreeMap<String, i64>, String> {
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let mut inventory = BTreeMap::new();
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let count = connection
            .query_row(&format!("SELECT COUNT(*) FROM \"{quoted}\""), [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| error.to_string())?;
        inventory.insert(table, count);
    }
    Ok(inventory)
}

fn migrate_portable_notes(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    migrate_portable_notes_to_version(connection, recovery_path, PORTABLE_SCHEMA_VERSION)
}

fn migrate_portable_notes_to_version(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
    target_version: i64,
) -> Result<(), String> {
    if !matches!(target_version, 1 | PORTABLE_SCHEMA_VERSION) {
        return Err(format!(
            "unsupported mobile migration target {target_version}"
        ));
    }
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database schema {user_version} is newer than supported schema {PORTABLE_SCHEMA_VERSION}"
        ));
    }
    if user_version == PORTABLE_SCHEMA_VERSION {
        return verify_current_mobile_schema(connection);
    }
    if user_version < 0 {
        return Err("mobile database schema version cannot be negative".to_string());
    }
    if user_version == 1 {
        verify_mobile_schema_v1(connection)?;
        migrate_mobile_schema_v2(connection, recovery_path)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version != 0 {
        return Err(format!("unsupported mobile database schema {user_version}"));
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS mobile_notes (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               title TEXT NOT NULL,
               body TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               deleted_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS mobile_replica (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               library_id TEXT NOT NULL,
               device_id TEXT NOT NULL,
               install_id TEXT NOT NULL,
               default_scope_id TEXT,
               library_state TEXT NOT NULL DEFAULT 'local_staging'
                 CHECK (library_state IN ('local_staging', 'paired')),
               next_transaction_counter INTEGER NOT NULL DEFAULT 1,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS mobile_schema_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               schema_version INTEGER NOT NULL,
               min_reader_version INTEGER NOT NULL,
               min_writer_version INTEGER NOT NULL,
               migration_checksum TEXT NOT NULL,
               migrated_at INTEGER NOT NULL,
               product_version TEXT NOT NULL,
               migration_recovery_path TEXT
             );
             CREATE TABLE IF NOT EXISTS mobile_schema_migrations (
               version INTEGER PRIMARY KEY CHECK (version > 0),
               name TEXT NOT NULL CHECK (length(name) > 0),
               checksum TEXT NOT NULL CHECK (length(checksum) = 64),
               migrated_at INTEGER NOT NULL,
               product_version TEXT NOT NULL CHECK (length(product_version) > 0)
             );
             CREATE TRIGGER IF NOT EXISTS mobile_schema_migrations_no_update
             BEFORE UPDATE ON mobile_schema_migrations BEGIN
               SELECT RAISE(ABORT, 'mobile_schema_migrations is append-only');
             END;
             CREATE TRIGGER IF NOT EXISTS mobile_schema_migrations_no_delete
             BEFORE DELETE ON mobile_schema_migrations BEGIN
               SELECT RAISE(ABORT, 'mobile_schema_migrations is append-only');
             END;",
        )
        .map_err(|error| error.to_string())?;

    ensure_replica_columns(&transaction)?;
    ensure_mobile_note_columns(&transaction)?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS mobile_note_outbox (
               local_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               mutation_id TEXT NOT NULL UNIQUE,
               transaction_id TEXT NOT NULL UNIQUE,
               device_transaction_counter INTEGER NOT NULL UNIQUE,
               library_id TEXT NOT NULL,
               device_id TEXT NOT NULL,
               install_id TEXT NOT NULL,
               scope_id TEXT NOT NULL,
               scope_class TEXT NOT NULL,
               record_id TEXT NOT NULL,
               record_kind TEXT NOT NULL DEFAULT 'note',
               operation TEXT NOT NULL CHECK (operation IN ('create', 'update', 'tombstone', 'restore')),
               base_revision INTEGER NOT NULL,
               base_version_id TEXT,
               proposed_revision INTEGER NOT NULL,
               local_revision INTEGER NOT NULL,
               branch_id TEXT NOT NULL,
               version_id TEXT NOT NULL,
               canonical_hash TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'superseded', 'sending', 'acknowledged', 'conflict')),
               eligible_for_sync INTEGER NOT NULL DEFAULT 1,
               superseded_at INTEGER,
               attempts INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL,
               acknowledged_at INTEGER,
               UNIQUE(record_id, local_revision)
             );
             CREATE INDEX IF NOT EXISTS idx_mobile_note_outbox_pending
               ON mobile_note_outbox(state, local_sequence);
             CREATE INDEX IF NOT EXISTS idx_mobile_note_outbox_record
               ON mobile_note_outbox(record_id, local_revision);
             CREATE INDEX IF NOT EXISTS idx_mobile_notes_updated
               ON mobile_notes(updated_at DESC);",
        )
        .map_err(|error| error.to_string())?;
    ensure_outbox_columns(&transaction)?;

    let migration_time = now_millis()?;
    let has_replica: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM mobile_replica WHERE singleton = 1)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_replica {
        let (library_id, default_scope_id) = legacy_staging_identity(&transaction, migration_time)?;
        let device_id = new_uuid_v7();
        let install_id = new_uuid_v7();
        transaction
            .execute(
                "INSERT INTO mobile_replica (
                   singleton, library_id, device_id, install_id, default_scope_id,
                   library_state,
                   next_transaction_counter, created_at
                 ) VALUES (1, ?1, ?2, ?3, ?4, 'local_staging', 1, ?5)",
                params![
                    library_id,
                    device_id,
                    install_id,
                    default_scope_id,
                    migration_time
                ],
            )
            .map_err(|error| error.to_string())?;
    } else {
        transaction
            .execute(
                "UPDATE mobile_replica
                 SET default_scope_id = COALESCE(NULLIF(default_scope_id, ''), ?1),
                     library_state = COALESCE(NULLIF(library_state, ''), 'local_staging')
                 WHERE singleton = 1",
                [new_uuid_v7()],
            )
            .map_err(|error| error.to_string())?;
    }
    let identity = replica_identity(&transaction)?;
    validate_replica_identity(&identity)?;

    backfill_portable_notes(&transaction, &identity)?;
    validate_portable_notes_v1(&transaction)?;
    transaction
        .execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_mobile_notes_record_id
               ON mobile_notes(record_id);
             CREATE INDEX IF NOT EXISTS idx_mobile_notes_library_lifecycle
               ON mobile_notes(library_id, lifecycle_state, updated_at DESC);",
        )
        .map_err(|error| error.to_string())?;
    let recovery_path = recovery_path.map(|path| path.to_string_lossy().into_owned());
    transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                1,
                PORTABLE_MIGRATION_V1_NAME,
                PORTABLE_SCHEMA_V1_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO mobile_schema_state (
               singleton, schema_version, min_reader_version, min_writer_version,
               migration_checksum, migrated_at, product_version,
               migration_recovery_path
             ) VALUES (1, ?1, ?1, ?1, ?2, ?3, ?4, ?5)",
            params![
                1,
                PORTABLE_SCHEMA_V1_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path,
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .pragma_update(None, "application_id", MOBILE_APPLICATION_ID)
        .map_err(|error| error.to_string())?;
    transaction
        .pragma_update(None, "user_version", 1)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    if target_version == 1 {
        return verify_mobile_schema_v1(connection);
    }
    migrate_mobile_schema_v2(connection, None)?;
    verify_current_mobile_schema(connection)
}

fn legacy_staging_identity(
    connection: &Connection,
    migration_time: i64,
) -> Result<(String, String), String> {
    let rows = connection
        .prepare(
            "SELECT id, title, body, created_at, updated_at, deleted_at
             FROM mobile_notes ORDER BY id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, i64>(0)?,
                        "title": row.get::<_, String>(1)?,
                        "body": row.get::<_, String>(2)?,
                        "created_at": row.get::<_, i64>(3)?,
                        "updated_at": row.get::<_, i64>(4)?,
                        "deleted_at": row.get::<_, Option<i64>>(5)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok((new_uuid_v7(), new_uuid_v7()));
    }
    let earliest = rows
        .iter()
        .filter_map(|row| row.get("created_at").and_then(serde_json::Value::as_i64))
        .min()
        .unwrap_or(migration_time)
        .max(0);
    let fingerprint = canonical_sha256(&serde_json::json!({"mobile_notes": rows}));
    let library_id = deterministic_backfill_uuid_v7(
        u64::try_from(earliest).unwrap_or(0),
        "noted.iphone-staging-library",
        &fingerprint,
    );
    let scope_id = deterministic_backfill_uuid_v7(
        u64::try_from(earliest).unwrap_or(0),
        &format!("noted.iphone-staging-scope.{library_id}"),
        "personal",
    );
    Ok((library_id, scope_id))
}

fn verify_mobile_schema_v1(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 1 {
        return Err(format!(
            "mobile schema v1 verifier received user_version {user_version}"
        ));
    }
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if application_id != MOBILE_APPLICATION_ID {
        return Err(format!(
            "mobile database is missing the expected application_id {MOBILE_APPLICATION_ID:#010x}"
        ));
    }
    let state = connection
        .query_row(
            "SELECT schema_version, min_reader_version, min_writer_version,
                    migration_checksum
             FROM mobile_schema_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|error| format!("mobile database compatibility stamp is invalid: {error}"))?;
    if state.0 != 1 || state.1 != 1 || state.2 != 1 {
        return Err("mobile schema v1 compatibility floor is invalid".to_string());
    }
    if state.3 != PORTABLE_SCHEMA_V1_CHECKSUM {
        return Err("mobile schema v1 checksum does not match this binary".to_string());
    }
    let history = connection
        .query_row(
            "SELECT name, checksum FROM mobile_schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| format!("mobile migration v1 history is invalid: {error}"))?;
    if history.0 != PORTABLE_MIGRATION_V1_NAME || history.1 != PORTABLE_SCHEMA_V1_CHECKSUM {
        return Err("mobile migration v1 history does not match this binary".to_string());
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 1 {
        return Err("mobile migration v1 history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    let identity = replica_identity(connection)?;
    validate_replica_identity(&identity)?;
    validate_portable_notes_v1(connection)?;
    let duplicate_eligible: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (
               SELECT record_id FROM mobile_note_outbox
               WHERE eligible_for_sync = 1
               GROUP BY record_id HAVING COUNT(*) > 1
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if duplicate_eligible != 0 {
        return Err("mobile outbox v1 has competing eligible branches".to_string());
    }
    Ok(())
}

fn migrate_mobile_schema_v2(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v1(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    ensure_columns(&transaction, "mobile_notes", &[("trashed_at", "INTEGER")])?;

    // V1 had one destructive-looking delete transition. Preserve those rows as
    // tombstones, reconstructing the required prior trash instant from the
    // legacy deletion instant. No content row is physically removed.
    transaction
        .execute_batch(
            "UPDATE mobile_notes
             SET lifecycle_state = 'active', deleted_at = NULL,
                 trashed_at = NULL, tombstoned_at = NULL
             WHERE lifecycle_state = 'active';
             UPDATE mobile_notes
             SET lifecycle_state = 'tombstone',
                 trashed_at = MAX(created_at, COALESCE(deleted_at, tombstoned_at, updated_at)),
                 tombstoned_at = MAX(created_at, COALESCE(tombstoned_at, deleted_at, updated_at)),
                 deleted_at = MAX(created_at, COALESCE(deleted_at, tombstoned_at, updated_at))
             WHERE lifecycle_state = 'tombstone' OR deleted_at IS NOT NULL;",
        )
        .map_err(|error| error.to_string())?;

    transaction
        .execute_batch(
            "CREATE TABLE mobile_note_outbox_v2 (
               local_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               mutation_id TEXT NOT NULL UNIQUE,
               transaction_id TEXT NOT NULL,
               device_transaction_counter INTEGER NOT NULL,
               transaction_member_index INTEGER NOT NULL CHECK (transaction_member_index >= 0),
               transaction_member_count INTEGER NOT NULL CHECK (transaction_member_count > 0),
               library_id TEXT NOT NULL,
               device_id TEXT NOT NULL,
               install_id TEXT NOT NULL,
               scope_id TEXT NOT NULL,
               scope_class TEXT NOT NULL,
               record_id TEXT NOT NULL,
               record_kind TEXT NOT NULL DEFAULT 'note',
               operation TEXT NOT NULL CHECK (operation IN ('create', 'update', 'trash', 'tombstone', 'restore')),
               base_revision INTEGER NOT NULL,
               base_version_id TEXT,
               proposed_revision INTEGER NOT NULL,
               local_revision INTEGER NOT NULL,
               branch_id TEXT NOT NULL,
               version_id TEXT NOT NULL,
               canonical_hash TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'superseded', 'sending', 'acknowledged', 'conflict')),
               eligible_for_sync INTEGER NOT NULL DEFAULT 1,
               superseded_at INTEGER,
               attempts INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL,
               acknowledged_at INTEGER,
               CHECK (transaction_member_index < transaction_member_count),
               UNIQUE(transaction_id, transaction_member_index),
               UNIQUE(device_id, device_transaction_counter, transaction_member_index),
               UNIQUE(record_id, local_revision)
             );
             INSERT INTO mobile_note_outbox_v2 (
               local_sequence, mutation_id, transaction_id, device_transaction_counter,
               transaction_member_index, transaction_member_count,
               library_id, device_id, install_id, scope_id, scope_class,
               record_id, record_kind, operation, base_revision, base_version_id,
               proposed_revision, local_revision, branch_id, version_id,
               canonical_hash, payload_json, state, eligible_for_sync,
               superseded_at, attempts, created_at, acknowledged_at
             )
             SELECT local_sequence, mutation_id, transaction_id, device_transaction_counter,
                    0, 1,
                    library_id, device_id, install_id, scope_id, scope_class,
                    record_id, record_kind, operation, base_revision, base_version_id,
                    proposed_revision, local_revision, branch_id, version_id,
                    canonical_hash, payload_json, state, eligible_for_sync,
                    superseded_at, attempts, created_at, acknowledged_at
             FROM mobile_note_outbox ORDER BY local_sequence;
             DROP TABLE mobile_note_outbox;
             ALTER TABLE mobile_note_outbox_v2 RENAME TO mobile_note_outbox;
             CREATE INDEX idx_mobile_note_outbox_pending
               ON mobile_note_outbox(state, local_sequence);
             CREATE INDEX idx_mobile_note_outbox_record
               ON mobile_note_outbox(record_id, local_revision);",
        )
        .map_err(|error| error.to_string())?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (2, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V2_NAME,
                PORTABLE_SCHEMA_V2_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v2 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 2,
                 min_reader_version = 2,
                 min_writer_version = 2,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 1",
            params![
                PORTABLE_SCHEMA_V2_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v2 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 2)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn verify_current_mobile_schema(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile schema verifier expected user_version {PORTABLE_SCHEMA_VERSION}, found {user_version}"
        ));
    }
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if application_id != MOBILE_APPLICATION_ID {
        return Err(format!(
            "mobile database is missing the expected application_id {MOBILE_APPLICATION_ID:#010x}"
        ));
    }

    let state = connection
        .query_row(
            "SELECT schema_version, min_reader_version, min_writer_version,
                    migration_checksum
             FROM mobile_schema_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|error| format!("mobile database compatibility stamp is invalid: {error}"))?;
    if state.0 != PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile schema stamp {} does not match user_version {PORTABLE_SCHEMA_VERSION}",
            state.0
        ));
    }
    if state.1 != PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} does not match schema {PORTABLE_SCHEMA_VERSION}",
            state.1
        ));
    }
    if state.2 != PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} does not match schema {PORTABLE_SCHEMA_VERSION}",
            state.2
        ));
    }
    if state.3 != PORTABLE_SCHEMA_V2_CHECKSUM {
        return Err("mobile schema-state checksum does not match this binary".to_string());
    }

    let history_v1 = connection
        .query_row(
            "SELECT name, checksum FROM mobile_schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| format!("mobile migration v1 history is invalid: {error}"))?;
    if history_v1.0 != PORTABLE_MIGRATION_V1_NAME || history_v1.1 != PORTABLE_SCHEMA_V1_CHECKSUM {
        return Err("mobile migration v1 history does not match this binary".to_string());
    }
    let history_v2 = connection
        .query_row(
            "SELECT name, checksum FROM mobile_schema_migrations WHERE version = 2",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| format!("mobile migration v2 history is invalid: {error}"))?;
    if history_v2.0 != PORTABLE_MIGRATION_V2_NAME || history_v2.1 != PORTABLE_SCHEMA_V2_CHECKSUM {
        return Err("mobile migration v2 history does not match this binary".to_string());
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != PORTABLE_SCHEMA_VERSION {
        return Err("mobile migration history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;

    verify_mobile_database_integrity(connection)?;

    let required_v2_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mobile_note_outbox')
             WHERE name IN ('transaction_member_index', 'transaction_member_count')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_trashed_at: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'trashed_at'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_v2_columns != 2 || !has_trashed_at {
        return Err(
            "mobile schema v2 is missing required lifecycle or transaction columns".to_string(),
        );
    }

    let identity = replica_identity(connection)?;
    validate_replica_identity(&identity)?;
    validate_portable_notes(connection)?;
    let duplicate_eligible: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (
               SELECT record_id FROM mobile_note_outbox
               WHERE eligible_for_sync = 1
               GROUP BY record_id HAVING COUNT(*) > 1
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if duplicate_eligible != 0 {
        return Err("mobile outbox has competing eligible branches".to_string());
    }
    validate_outbox_transaction_groups(connection)?;
    Ok(())
}

fn verify_mobile_database_integrity(connection: &Connection) -> Result<(), String> {
    let quick_check = connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if quick_check.as_slice() != ["ok"] {
        return Err(format!(
            "mobile database quick_check failed: {}",
            quick_check.join("; ")
        ));
    }
    let has_foreign_key_error = connection
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| Ok(statement.query([])?.next()?.is_some()))
        .map_err(|error| error.to_string())?;
    if has_foreign_key_error {
        return Err("mobile database foreign_key_check failed".to_string());
    }

    Ok(())
}

fn verify_migration_history_guards(connection: &Connection) -> Result<(), String> {
    let guard_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger'
               AND name IN (
                 'mobile_schema_migrations_no_update',
                 'mobile_schema_migrations_no_delete'
               )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if guard_count != 2 {
        return Err("mobile migration history append-only guards are missing".to_string());
    }
    Ok(())
}

fn validate_outbox_transaction_groups(connection: &Connection) -> Result<(), String> {
    let invalid_groups: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (
               SELECT transaction_id
               FROM mobile_note_outbox
               GROUP BY transaction_id
               HAVING COUNT(*) != MAX(transaction_member_count)
                  OR MIN(transaction_member_count) != MAX(transaction_member_count)
                  OR MIN(transaction_member_index) != 0
                  OR MAX(transaction_member_index) != MAX(transaction_member_count) - 1
                  OR COUNT(DISTINCT transaction_member_index) != COUNT(*)
                  OR MIN(device_transaction_counter) != MAX(device_transaction_counter)
                  OR MIN(device_id) != MAX(device_id)
                  OR MIN(library_id) != MAX(library_id)
                  OR MIN(install_id) != MAX(install_id)
                  OR MIN(state) != MAX(state)
                  OR MIN(eligible_for_sync) != MAX(eligible_for_sync)
                  OR MIN(attempts) != MAX(attempts)
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid_groups != 0 {
        return Err(format!(
            "mobile outbox contains {invalid_groups} incomplete or incoherent transaction groups"
        ));
    }
    Ok(())
}

fn ensure_mobile_note_columns(connection: &Connection) -> Result<(), String> {
    const COLUMNS: &[(&str, &str)] = &[
        ("deleted_at", "INTEGER"),
        ("library_id", "TEXT"),
        ("record_id", "TEXT"),
        ("record_kind", "TEXT"),
        ("record_schema_version", "INTEGER"),
        ("accepted_revision", "INTEGER"),
        ("accepted_version_id", "TEXT"),
        ("accepted_content_hash", "TEXT"),
        ("working_revision", "INTEGER"),
        ("working_branch_id", "TEXT"),
        ("working_version_id", "TEXT"),
        ("working_base_revision", "INTEGER"),
        ("pending_mutation_id", "TEXT"),
        ("sync_state", "TEXT"),
        ("lifecycle_state", "TEXT"),
        ("tombstoned_at", "INTEGER"),
        ("canonical_hash", "TEXT"),
        ("authority", "TEXT"),
        ("scope", "TEXT"),
        ("scope_id", "TEXT"),
        ("scope_class", "TEXT"),
        ("sensitivity", "TEXT"),
        ("provenance_json", "TEXT"),
        ("origin_device_id", "TEXT"),
        ("last_modified_device_id", "TEXT"),
        ("origin_install_id", "TEXT"),
    ];

    for (name, sql_type) in COLUMNS {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = ?1
                 )",
                [name],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            connection
                .execute(
                    &format!("ALTER TABLE mobile_notes ADD COLUMN {name} {sql_type}"),
                    [],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn ensure_replica_columns(connection: &Connection) -> Result<(), String> {
    ensure_columns(
        connection,
        "mobile_replica",
        &[
            ("default_scope_id", "TEXT"),
            (
                "library_state",
                "TEXT NOT NULL DEFAULT 'local_staging' CHECK (library_state IN ('local_staging', 'paired'))",
            ),
        ],
    )
}

fn ensure_outbox_columns(connection: &Connection) -> Result<(), String> {
    ensure_columns(
        connection,
        "mobile_note_outbox",
        &[
            ("scope_id", "TEXT"),
            ("scope_class", "TEXT"),
            ("base_version_id", "TEXT"),
            ("branch_id", "TEXT"),
            ("eligible_for_sync", "INTEGER NOT NULL DEFAULT 1"),
            ("superseded_at", "INTEGER"),
        ],
    )
}

fn ensure_columns(
    connection: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> Result<(), String> {
    for (name, sql_type) in columns {
        let exists: bool = connection
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)"
                ),
                [name],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            connection
                .execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {name} {sql_type}"),
                    [],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn backfill_portable_notes(
    transaction: &Transaction<'_>,
    identity: &ReplicaIdentity,
) -> Result<(), String> {
    struct LegacyRow {
        id: i64,
        title: String,
        body: String,
        created_at: i64,
        updated_at: i64,
        deleted_at: Option<i64>,
        record_id: Option<String>,
        accepted_revision: Option<i64>,
        accepted_version_id: Option<String>,
        accepted_content_hash: Option<String>,
        working_revision: Option<i64>,
        working_branch_id: Option<String>,
        working_version_id: Option<String>,
        pending_mutation_id: Option<String>,
        canonical_hash: Option<String>,
        provenance_json: Option<String>,
        scope_id: Option<String>,
        scope_class: Option<String>,
        lifecycle_state: Option<String>,
        tombstoned_at: Option<i64>,
    }

    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT id, title, body, created_at, updated_at, deleted_at,
                        record_id, accepted_revision, accepted_version_id,
                        accepted_content_hash, working_revision, working_branch_id,
                        working_version_id, pending_mutation_id, canonical_hash,
                        provenance_json, scope_id, scope_class,
                        lifecycle_state, tombstoned_at
                 FROM mobile_notes ORDER BY id",
            )
            .map_err(|error| error.to_string())?;
        let mapped = statement
            .query_map([], |row| {
                Ok(LegacyRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    deleted_at: row.get(5)?,
                    record_id: row.get(6)?,
                    accepted_revision: row.get(7)?,
                    accepted_version_id: row.get(8)?,
                    accepted_content_hash: row.get(9)?,
                    working_revision: row.get(10)?,
                    working_branch_id: row.get(11)?,
                    working_version_id: row.get(12)?,
                    pending_mutation_id: row.get(13)?,
                    canonical_hash: row.get(14)?,
                    provenance_json: row.get(15)?,
                    scope_id: row.get(16)?,
                    scope_class: row.get(17)?,
                    lifecycle_state: row.get(18)?,
                    tombstoned_at: row.get(19)?,
                })
            })
            .map_err(|error| error.to_string())?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };

    for row in rows {
        let record_id = nonempty(row.record_id).unwrap_or_else(|| {
            deterministic_backfill_uuid_v7(
                u64::try_from(row.created_at.max(0)).unwrap_or(0),
                &format!("noted.mobile-notes.{}", identity.library_id),
                &row.id.to_string(),
            )
        });
        let accepted_revision = row.accepted_revision.unwrap_or(0).max(0);
        let working_revision = row.working_revision.unwrap_or(1).max(1);
        let working_branch_id = nonempty(row.working_branch_id).unwrap_or_else(new_uuid_v7);
        let working_version_id = nonempty(row.working_version_id).unwrap_or_else(new_uuid_v7);
        let pending_mutation_id = nonempty(row.pending_mutation_id).unwrap_or_else(new_uuid_v7);
        let canonical_hash = nonempty(row.canonical_hash)
            .unwrap_or_else(|| note_content_hash(&row.title, &row.body));
        let provenance_json = nonempty(row.provenance_json)
            .unwrap_or_else(|| r#"{"source":"iphone_prototype_migration"}"#.to_string());
        let scope_id = nonempty(row.scope_id).unwrap_or_else(|| identity.default_scope_id.clone());
        let scope_class = nonempty(row.scope_class).unwrap_or_else(|| "personal".to_string());
        let was_deleted = row.deleted_at.is_some()
            || row.tombstoned_at.is_some()
            || matches!(
                row.lifecycle_state.as_deref(),
                Some("deleted" | "trash" | "tombstone")
            );
        let lifecycle_state = if was_deleted { "tombstone" } else { "active" };
        let legacy_tombstone_time = was_deleted.then(|| {
            row.deleted_at
                .or(row.tombstoned_at)
                .unwrap_or(row.updated_at)
                .max(row.created_at)
        });

        transaction
            .execute(
                "UPDATE mobile_notes SET
                   library_id = COALESCE(NULLIF(library_id, ''), ?1),
                   record_id = ?2,
                   record_kind = COALESCE(NULLIF(record_kind, ''), 'note'),
                   record_schema_version = COALESCE(record_schema_version, 1),
                   accepted_revision = ?3,
                   working_revision = ?4,
                   working_branch_id = ?5,
                   working_version_id = ?6,
                   working_base_revision = COALESCE(working_base_revision, ?3),
                   pending_mutation_id = ?7,
                   sync_state = COALESCE(NULLIF(sync_state, ''), 'pending'),
                   lifecycle_state = ?8,
                   deleted_at = ?9,
                   tombstoned_at = ?9,
                   canonical_hash = ?10,
                   authority = COALESCE(NULLIF(authority, ''), 'noted'),
                   scope = COALESCE(NULLIF(scope, ''), 'personal'),
                   scope_id = ?11,
                   scope_class = ?12,
                   sensitivity = COALESCE(NULLIF(sensitivity, ''), 'standard'),
                   provenance_json = ?13,
                   origin_device_id = COALESCE(NULLIF(origin_device_id, ''), ?14),
                   last_modified_device_id = COALESCE(NULLIF(last_modified_device_id, ''), ?14),
                   origin_install_id = COALESCE(NULLIF(origin_install_id, ''), ?15)
                 WHERE id = ?16",
                params![
                    identity.library_id,
                    record_id,
                    accepted_revision,
                    working_revision,
                    working_branch_id,
                    working_version_id,
                    pending_mutation_id,
                    lifecycle_state,
                    legacy_tombstone_time,
                    canonical_hash,
                    scope_id,
                    scope_class,
                    provenance_json,
                    identity.device_id,
                    identity.install_id,
                    row.id,
                ],
            )
            .map_err(|error| error.to_string())?;

        let has_outbox: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_outbox WHERE record_id = ?1
                 )",
                [&record_id],
                |result| result.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !has_outbox {
            let outbox_transaction = begin_outbox_transaction(transaction, 1)?;
            enqueue_mutation(
                transaction,
                identity,
                &outbox_transaction,
                0,
                Mutation {
                    operation: "create",
                    record_id: &record_id,
                    title: &row.title,
                    body: &row.body,
                    base_revision: accepted_revision,
                    proposed_revision: accepted_revision.saturating_add(1),
                    local_revision: working_revision,
                    version_id: &working_version_id,
                    branch_id: &working_branch_id,
                    base_version_id: row.accepted_version_id.as_deref(),
                    accepted_content_hash: row.accepted_content_hash.as_deref(),
                    mutation_id: &pending_mutation_id,
                    canonical_hash: &canonical_hash,
                    lifecycle_state,
                    trashed_at: legacy_tombstone_time,
                    tombstoned_at: legacy_tombstone_time,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                    provenance_json: &provenance_json,
                    scope_id: &scope_id,
                    scope_class: &scope_class,
                },
            )?;
        }
    }
    transaction
        .execute(
            "UPDATE mobile_note_outbox
             SET scope_id = COALESCE(NULLIF(scope_id, ''), ?1),
                 scope_class = COALESCE(NULLIF(scope_class, ''), 'personal'),
                 branch_id = COALESCE(
                   NULLIF(branch_id, ''),
                   (SELECT working_branch_id FROM mobile_notes
                    WHERE mobile_notes.record_id = mobile_note_outbox.record_id)
                 )",
            [&identity.default_scope_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate_portable_notes_v1(connection: &Connection) -> Result<(), String> {
    let invalid: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_notes
             WHERE library_id IS NULL OR library_id = ''
                OR record_id IS NULL OR record_id = ''
                OR record_kind != 'note'
                OR record_schema_version IS NULL
                OR accepted_revision IS NULL OR accepted_revision < 0
                OR working_revision IS NULL OR working_revision < 1
                OR working_branch_id IS NULL OR working_branch_id = ''
                OR working_version_id IS NULL OR working_version_id = ''
                OR pending_mutation_id IS NULL OR pending_mutation_id = ''
                OR sync_state IS NULL OR sync_state = ''
                OR lifecycle_state NOT IN ('active', 'tombstone')
                OR canonical_hash IS NULL OR length(canonical_hash) != 64
                OR authority IS NULL OR authority = ''
                OR scope_id IS NULL OR scope_id = ''
                OR scope_class NOT IN ('work', 'personal', 'unknown')
                OR provenance_json IS NULL OR provenance_json = ''
                OR origin_device_id IS NULL OR origin_device_id = ''
                OR last_modified_device_id IS NULL OR last_modified_device_id = ''
                OR origin_install_id IS NULL OR origin_install_id = ''
                OR (lifecycle_state = 'active' AND
                    (deleted_at IS NOT NULL OR tombstoned_at IS NOT NULL))
                OR (lifecycle_state = 'tombstone' AND
                    (deleted_at IS NULL OR tombstoned_at IS NULL))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid != 0 {
        return Err(format!(
            "portable mobile note migration left {invalid} invalid rows"
        ));
    }
    validate_portable_identifiers(connection)
}

fn validate_portable_notes(connection: &Connection) -> Result<(), String> {
    let invalid: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_notes
             WHERE library_id IS NULL OR library_id = ''
                OR record_id IS NULL OR record_id = ''
                OR record_kind != 'note'
                OR record_schema_version IS NULL
                OR accepted_revision IS NULL OR accepted_revision < 0
                OR working_revision IS NULL OR working_revision < 1
                OR working_branch_id IS NULL OR working_branch_id = ''
                OR working_version_id IS NULL OR working_version_id = ''
                OR pending_mutation_id IS NULL OR pending_mutation_id = ''
                OR sync_state IS NULL OR sync_state = ''
                OR lifecycle_state NOT IN ('active', 'trash', 'tombstone')
                OR canonical_hash IS NULL OR length(canonical_hash) != 64
                OR authority IS NULL OR authority = ''
                OR scope_id IS NULL OR scope_id = ''
                OR scope_class NOT IN ('work', 'personal', 'unknown')
                OR provenance_json IS NULL OR provenance_json = ''
                OR origin_device_id IS NULL OR origin_device_id = ''
                OR last_modified_device_id IS NULL OR last_modified_device_id = ''
                OR origin_install_id IS NULL OR origin_install_id = ''
                OR (lifecycle_state = 'active' AND
                    (deleted_at IS NOT NULL OR trashed_at IS NOT NULL OR tombstoned_at IS NOT NULL))
                OR (lifecycle_state = 'trash' AND
                    (deleted_at IS NULL OR trashed_at IS NULL OR tombstoned_at IS NOT NULL
                     OR deleted_at != trashed_at OR trashed_at < created_at))
                OR (lifecycle_state = 'tombstone' AND
                    (deleted_at IS NULL OR trashed_at IS NULL OR tombstoned_at IS NULL
                     OR deleted_at != trashed_at OR trashed_at < created_at
                     OR tombstoned_at < trashed_at))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid != 0 {
        return Err(format!(
            "portable mobile note migration left {invalid} invalid rows"
        ));
    }
    validate_portable_identifiers(connection)
}

fn validate_portable_identifiers(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare(
            "SELECT record_id, scope_id, working_branch_id, working_version_id FROM mobile_notes",
        )
        .map_err(|error| error.to_string())?;
    let identifiers = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for identifier in identifiers {
        let (record_id, scope_id, branch_id, version_id) =
            identifier.map_err(|error| error.to_string())?;
        if !is_uuid_v7(&record_id)
            || !is_uuid(&scope_id)
            || !is_uuid(&branch_id)
            || !is_uuid(&version_id)
        {
            return Err("portable mobile note migration produced an invalid UUID".to_string());
        }
    }
    Ok(())
}

fn validate_replica_identity(identity: &ReplicaIdentity) -> Result<(), String> {
    if !is_uuid(&identity.library_id)
        || !is_uuid(&identity.device_id)
        || !is_uuid(&identity.install_id)
        || !is_uuid(&identity.default_scope_id)
    {
        return Err("mobile replica identity contains an invalid UUID".to_string());
    }
    if !matches!(identity.library_state.as_str(), "local_staging" | "paired") {
        return Err("mobile replica library state is invalid".to_string());
    }
    Ok(())
}

fn replica_identity_optional(connection: &Connection) -> Result<Option<ReplicaIdentity>, String> {
    connection
        .query_row(
            "SELECT library_id, device_id, install_id, default_scope_id, library_state
             FROM mobile_replica WHERE singleton = 1",
            [],
            |row| {
                Ok(ReplicaIdentity {
                    library_id: row.get(0)?,
                    device_id: row.get(1)?,
                    install_id: row.get(2)?,
                    default_scope_id: row.get(3)?,
                    library_state: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn replica_identity(connection: &Connection) -> Result<ReplicaIdentity, String> {
    replica_identity_optional(connection)?
        .ok_or_else(|| "mobile replica identity is missing".to_string())
}

fn portable_state(
    connection: &Connection,
    record_id: &str,
) -> Result<Option<PortableState>, String> {
    connection
        .query_row(
            "SELECT record_id, accepted_revision, accepted_version_id,
                    accepted_content_hash, working_revision, working_branch_id,
                    created_at, lifecycle_state, trashed_at,
                    authority, provenance_json, scope_id, scope_class
             FROM mobile_notes WHERE record_id = ?1",
            [record_id],
            |row| {
                Ok(PortableState {
                    record_id: row.get(0)?,
                    accepted_revision: row.get(1)?,
                    accepted_version_id: row.get(2)?,
                    accepted_content_hash: row.get(3)?,
                    working_revision: row.get(4)?,
                    working_branch_id: row.get(5)?,
                    created_at: row.get(6)?,
                    lifecycle_state: row.get(7)?,
                    trashed_at: row.get(8)?,
                    authority: row.get(9)?,
                    provenance_json: row.get(10)?,
                    scope_id: row.get(11)?,
                    scope_class: row.get(12)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn ensure_noted_authority(state: &PortableState) -> Result<(), String> {
    if state.authority == "noted" {
        Ok(())
    } else {
        Err(format!(
            "note {} is owned by an external authority and is read-only on iPhone",
            state.record_id
        ))
    }
}

fn begin_outbox_transaction(
    transaction: &Transaction<'_>,
    member_count: usize,
) -> Result<OutboxTransaction, String> {
    let member_count = i64::try_from(member_count)
        .map_err(|_| "outbox transaction has too many members".to_string())?;
    if member_count <= 0 {
        return Err("outbox transaction must contain at least one member".to_string());
    }
    let device_transaction_counter: i64 = transaction
        .query_row(
            "SELECT next_transaction_counter FROM mobile_replica WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if device_transaction_counter <= 0 {
        return Err("mobile replica transaction counter is invalid".to_string());
    }
    transaction
        .execute(
            "UPDATE mobile_replica
             SET next_transaction_counter = next_transaction_counter + 1
             WHERE singleton = 1",
            [],
        )
        .map_err(|error| error.to_string())?;
    Ok(OutboxTransaction {
        transaction_id: new_uuid_v7(),
        device_transaction_counter,
        member_count,
    })
}

fn enqueue_mutation(
    transaction: &Transaction<'_>,
    identity: &ReplicaIdentity,
    outbox_transaction: &OutboxTransaction,
    member_index: i64,
    mutation: Mutation<'_>,
) -> Result<(), String> {
    if member_index < 0 || member_index >= outbox_transaction.member_count {
        return Err("outbox transaction member index is out of range".to_string());
    }
    let has_transaction_members: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_note_outbox')
               WHERE name = 'transaction_member_index'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_grouped_pending_state: bool = if has_transaction_members {
        transaction
            .query_row(
                "SELECT EXISTS(
               SELECT 1 FROM mobile_note_outbox
               WHERE record_id = ?1
                 AND eligible_for_sync = 1
                 AND transaction_member_count > 1
             )",
                [mutation.record_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?
    } else {
        false
    };
    if has_grouped_pending_state {
        return Err(
            "a pending grouped transaction must be synchronized before this record changes"
                .to_string(),
        );
    }

    let provenance = serde_json::from_str(mutation.provenance_json)
        .unwrap_or_else(|_| serde_json::json!({ "source": "unknown" }));
    let trashed_at = mutation.trashed_at.map(rfc3339_from_millis);
    let tombstoned_at = mutation.tombstoned_at.map(rfc3339_from_millis);
    let lifecycle = match mutation.lifecycle_state {
        "active" => RecordLifecycle {
            state: LifecycleState::Active,
            trashed_at: None,
            tombstoned_at: None,
        },
        "trash" => RecordLifecycle {
            state: LifecycleState::Trash,
            trashed_at,
            tombstoned_at: None,
        },
        "tombstone" => RecordLifecycle {
            state: LifecycleState::Tombstone,
            trashed_at,
            tombstoned_at,
        },
        state => return Err(format!("unsupported mobile note lifecycle {state}")),
    };
    let content = note_content(mutation.title, mutation.body);
    if canonical_sha256(&content) != mutation.canonical_hash {
        return Err("mobile note content hash diverged from proposed record".to_string());
    }
    let local_branch = LocalBranch {
        branch_id: mutation.branch_id.to_string(),
        base_revision: u64::try_from(mutation.base_revision)
            .map_err(|_| "base revision cannot be negative".to_string())?,
        base_version_id: mutation.base_version_id.map(str::to_string),
        local_revision: u64::try_from(mutation.local_revision)
            .map_err(|_| "local revision cannot be negative".to_string())?,
        working_version_id: mutation.version_id.to_string(),
        content_hash: mutation.canonical_hash.to_string(),
        state: LocalBranchState::Pending,
    };
    local_branch.validate()?;
    let accepted_head = match (
        mutation.base_revision,
        mutation.base_version_id,
        mutation.accepted_content_hash,
    ) {
        (0, None, None) => None,
        (revision, Some(version_id), Some(content_hash)) if revision > 0 => Some(AcceptedHead {
            revision: u64::try_from(revision)
                .map_err(|_| "accepted revision cannot be negative".to_string())?,
            version_id: version_id.to_string(),
            content_hash: content_hash.to_string(),
        }),
        _ => {
            return Err(
                "accepted head revision, version, and content hash must be present together"
                    .to_string(),
            )
        }
    };
    let proposed_record = ProposedRecordPayload {
        proposal_contract_version: "noted.proposed-record.v1",
        library_id: &identity.library_id,
        record_id: mutation.record_id,
        kind: "note",
        record_schema_version: 1,
        created_at: rfc3339_from_millis(mutation.created_at),
        updated_at: rfc3339_from_millis(mutation.updated_at),
        scope: RecordScope {
            scope_id: mutation.scope_id.to_string(),
            class: scope_class(mutation.scope_class)?,
        },
        sensitivity: "standard",
        authority: RecordAuthority {
            kind: AuthorityKind::Noted,
            origin: Some("iphone_native".to_string()),
        },
        content,
        content_hash: mutation.canonical_hash,
        provenance,
        lifecycle,
        accepted_head,
        local_branch,
    };
    let payload_json = serde_json::to_string(&MutationPayload {
        mutation_contract_version: "noted.mobile-note-mutation.shadow.v1",
        operation: mutation.operation,
        proposed_revision: mutation.proposed_revision,
        proposed_record,
    })
    .map_err(|error| error.to_string())?;

    transaction
        .execute(
            "UPDATE mobile_note_outbox
             SET state = 'superseded', eligible_for_sync = 0, superseded_at = ?1
             WHERE record_id = ?2 AND eligible_for_sync = 1",
            params![mutation.updated_at, mutation.record_id],
        )
        .map_err(|error| error.to_string())?;

    if has_transaction_members {
        transaction
            .execute(
                "INSERT INTO mobile_note_outbox (
                   mutation_id, transaction_id, device_transaction_counter,
                   transaction_member_index, transaction_member_count,
                   library_id, device_id, install_id, scope_id, scope_class,
                   record_id, record_kind,
                   operation, base_revision, base_version_id, proposed_revision,
                   local_revision, branch_id, version_id, canonical_hash,
                   payload_json, state, eligible_for_sync, attempts,
                   created_at, acknowledged_at
                 ) VALUES (
                   ?1, ?2, ?3, ?4, ?5,
                   ?6, ?7, ?8, ?9, ?10,
                   ?11, 'note',
                   ?12, ?13, ?14, ?15,
                   ?16, ?17, ?18, ?19,
                   ?20, 'pending', 1, 0,
                   ?21, NULL
                 )",
                params![
                    mutation.mutation_id,
                    outbox_transaction.transaction_id,
                    outbox_transaction.device_transaction_counter,
                    member_index,
                    outbox_transaction.member_count,
                    identity.library_id,
                    identity.device_id,
                    identity.install_id,
                    mutation.scope_id,
                    mutation.scope_class,
                    mutation.record_id,
                    mutation.operation,
                    mutation.base_revision,
                    mutation.base_version_id,
                    mutation.proposed_revision,
                    mutation.local_revision,
                    mutation.branch_id,
                    mutation.version_id,
                    mutation.canonical_hash,
                    payload_json,
                    mutation.updated_at,
                ],
            )
            .map_err(|error| error.to_string())?;
    } else {
        if outbox_transaction.member_count != 1 || member_index != 0 {
            return Err("mobile schema v1 cannot store grouped outbox transactions".to_string());
        }
        transaction
            .execute(
                "INSERT INTO mobile_note_outbox (
                   mutation_id, transaction_id, device_transaction_counter,
                   library_id, device_id, install_id, scope_id, scope_class,
                   record_id, record_kind,
                   operation, base_revision, base_version_id, proposed_revision,
                   local_revision, branch_id, version_id, canonical_hash,
                   payload_json, state, eligible_for_sync, attempts,
                   created_at, acknowledged_at
                 ) VALUES (
                   ?1, ?2, ?3,
                   ?4, ?5, ?6, ?7, ?8,
                   ?9, 'note',
                   ?10, ?11, ?12, ?13,
                   ?14, ?15, ?16, ?17,
                   ?18, 'pending', 1, 0,
                   ?19, NULL
                 )",
                params![
                    mutation.mutation_id,
                    outbox_transaction.transaction_id,
                    outbox_transaction.device_transaction_counter,
                    identity.library_id,
                    identity.device_id,
                    identity.install_id,
                    mutation.scope_id,
                    mutation.scope_class,
                    mutation.record_id,
                    mutation.operation,
                    mutation.base_revision,
                    mutation.base_version_id,
                    mutation.proposed_revision,
                    mutation.local_revision,
                    mutation.branch_id,
                    mutation.version_id,
                    mutation.canonical_hash,
                    payload_json,
                    mutation.updated_at,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MobileNote> {
    Ok(MobileNote {
        record_id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn mobile_fts_query(value: &str) -> Option<String> {
    let terms = value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .take(32)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn note_content(title: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "body": body,
        "title": title,
    })
}

fn note_content_hash(title: &str, body: &str) -> String {
    // Lifecycle and transport state are envelope fields, not canonical note
    // content, so they intentionally do not affect this digest.
    canonical_sha256(&note_content(title, body))
}

fn scope_class(value: &str) -> Result<ScopeClass, String> {
    match value {
        "work" => Ok(ScopeClass::Work),
        "personal" => Ok(ScopeClass::Personal),
        "unknown" => Ok(ScopeClass::Unknown),
        _ => Err(format!("unsupported scope class {value}")),
    }
}

fn now_millis() -> Result<i64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    i64::try_from(duration.as_millis()).map_err(|_| "system time is out of range".to_string())
}

fn rfc3339_from_millis(timestamp_ms: i64) -> String {
    let seconds = timestamp_ms.div_euclid(1_000);
    let milliseconds = timestamp_ms.rem_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);

    // Gregorian civil date from days since 1970-01-01. This is Howard
    // Hinnant's public-domain civil_from_days algorithm, expressed with i64s.
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    let hour = seconds_in_day / 3_600;
    let minute = (seconds_in_day % 3_600) / 60;
    let second = seconds_in_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milliseconds:03}Z")
}

fn next_timestamp(connection: &Connection) -> Result<i64, String> {
    let latest_note: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(updated_at), 0) FROM mobile_notes",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let latest_outbox: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(created_at), 0) FROM mobile_note_outbox",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(now_millis()?.max(latest_note.max(latest_outbox).saturating_add(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MobileStore {
        MobileStore::open(Path::new(":memory:")).expect("open in-memory mobile store")
    }

    fn temporary_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "noted-mobile-{label}-{}-{}.sqlite3",
            std::process::id(),
            now_millis().expect("timestamp")
        ))
    }

    fn remove_database(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    }

    #[test]
    fn notes_survive_crud_and_sort_by_recent_change() {
        let store = store();
        let first = store.create("First", "alpha").expect("create first");
        let second = store.create("Second", "beta").expect("create second");

        let updated = store
            .update(&first.record_id, "First revised", "alpha revised")
            .expect("update first");
        assert_eq!(updated.created_at, first.created_at);
        assert_eq!(
            store.list(None).expect("list")[0].record_id,
            first.record_id
        );

        store.delete(&second.record_id).expect("delete second");
        assert_eq!(store.list(None).expect("list").len(), 1);
        let (deleted_at, lifecycle, trashed_at, tombstoned_at): (
            Option<i64>,
            String,
            Option<i64>,
            Option<i64>,
        ) = store
            .connection
            .lock()
            .expect("lock store")
            .query_row(
                "SELECT deleted_at, lifecycle_state, trashed_at, tombstoned_at
                 FROM mobile_notes WHERE record_id = ?1",
                [&second.record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read trash state");
        assert!(deleted_at.is_some());
        assert_eq!(lifecycle, "trash");
        assert_eq!(trashed_at, deleted_at);
        assert_eq!(tombstoned_at, None);
    }

    #[test]
    fn trash_restore_and_tombstone_are_distinct_non_destructive_transitions() {
        let store = store();
        let note = store
            .create("Keep the row", "portable content")
            .expect("create note");

        store.delete(&note.record_id).expect("move note to trash");
        {
            let connection = store.connection.lock().expect("lock trashed store");
            let state: (String, Option<i64>, Option<i64>, Option<i64>, i64) = connection
                .query_row(
                    "SELECT lifecycle_state, deleted_at, trashed_at, tombstoned_at,
                            COUNT(*) OVER ()
                     FROM mobile_notes WHERE record_id = ?1",
                    [&note.record_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .expect("read trash state");
            assert_eq!(state.0, "trash");
            assert_eq!(state.1, state.2);
            assert!(state.2.is_some());
            assert_eq!(state.3, None);
            assert_eq!(state.4, 1);

            let payload_json: String = connection
                .query_row(
                    "SELECT payload_json FROM mobile_note_outbox
                     WHERE eligible_for_sync = 1",
                    [],
                    |row| row.get(0),
                )
                .expect("read trash payload");
            let payload: serde_json::Value =
                serde_json::from_str(&payload_json).expect("parse trash payload");
            assert_eq!(payload["operation"], "trash");
            assert_eq!(payload["proposed_record"]["lifecycle"]["state"], "trash");
            assert!(payload["proposed_record"]["lifecycle"]["trashed_at"].is_string());
            assert!(payload["proposed_record"]["lifecycle"]
                .get("tombstoned_at")
                .is_none());
        }

        store.restore(&note.record_id).expect("restore from trash");
        store.delete(&note.record_id).expect("trash again");
        store
            .tombstone(&note.record_id)
            .expect("finalize tombstone");

        let connection = store.connection.lock().expect("lock tombstoned store");
        let state: (String, String, String, i64, i64, i64) = connection
            .query_row(
                "SELECT title, body, lifecycle_state, deleted_at, trashed_at, tombstoned_at
                 FROM mobile_notes WHERE record_id = ?1",
                [&note.record_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("tombstone row must remain");
        assert_eq!(state.0, "Keep the row");
        assert_eq!(state.1, "portable content");
        assert_eq!(state.2, "tombstone");
        assert_eq!(state.3, state.4);
        assert!(state.5 >= state.4);
        let payload_json: String = connection
            .query_row(
                "SELECT payload_json FROM mobile_note_outbox WHERE eligible_for_sync = 1",
                [],
                |row| row.get(0),
            )
            .expect("read tombstone payload");
        let payload: serde_json::Value =
            serde_json::from_str(&payload_json).expect("parse tombstone payload");
        assert_eq!(payload["operation"], "tombstone");
        assert_eq!(
            payload["proposed_record"]["lifecycle"]["state"],
            "tombstone"
        );
        assert!(payload["proposed_record"]["lifecycle"]["trashed_at"].is_string());
        assert!(payload["proposed_record"]["lifecycle"]["tombstoned_at"].is_string());
        drop(connection);

        assert!(store.list(None).expect("list active notes").is_empty());
        assert!(store
            .restore(&note.record_id)
            .expect_err("tombstones cannot be restored")
            .contains("does not exist"));
    }

    #[test]
    fn search_matches_title_and_body_but_escapes_wildcards() {
        let store = store();
        store.create("Launch", "Call Sam").expect("create launch");
        store
            .create("Budget 100%", "Review")
            .expect("create budget");

        assert_eq!(store.list(Some("sam")).expect("body search").len(), 1);
        assert_eq!(store.list(Some("100%")).expect("literal search").len(), 1);
        assert!(store
            .list(Some("100_"))
            .expect("escaped underscore")
            .is_empty());
    }

    #[test]
    fn full_text_search_tracks_lifecycle_and_rebuilds_without_canonical_changes() {
        let store = store();
        let active = store
            .create("Meteor launch", "Call mission control")
            .expect("create searchable note");
        let hidden = store
            .create("Comet archive", "Hidden orbit details")
            .expect("create lifecycle note");

        assert_eq!(store.list(Some("mission")).expect("search body").len(), 1);
        store.delete(&hidden.record_id).expect("trash hidden note");
        assert!(store
            .list(Some("orbit"))
            .expect("search excludes trash")
            .is_empty());
        store
            .restore(&hidden.record_id)
            .expect("restore hidden note");
        assert_eq!(
            store
                .list(Some("orbit"))
                .expect("search restored note")
                .len(),
            1
        );
        store
            .update(&active.record_id, "Meteor launch", "Contact ground station")
            .expect("update indexed note");
        assert!(store
            .list(Some("mission"))
            .expect("old indexed text removed")
            .is_empty());
        assert_eq!(
            store
                .list(Some("ground"))
                .expect("new indexed text appears")
                .len(),
            1
        );
        store.delete(&hidden.record_id).expect("trash hidden again");
        store
            .tombstone(&hidden.record_id)
            .expect("tombstone hidden note");
        assert!(store
            .list(Some("orbit"))
            .expect("search excludes tombstone")
            .is_empty());

        let before = store.export_notes().expect("export before rebuild");
        {
            let connection = store.connection.lock().expect("lock search store");
            connection
                .execute_batch(
                    "DROP TRIGGER mobile_notes_fts_insert;
                     DROP TRIGGER mobile_notes_fts_update;
                     DROP TRIGGER mobile_notes_fts_delete;
                     DROP TABLE mobile_notes_fts;",
                )
                .expect("drop derived search objects");
        }
        store.rebuild_search_index().expect("rebuild search index");
        let after = store.export_notes().expect("export after rebuild");
        assert_eq!(
            after, before,
            "search rebuild changed canonical mobile state"
        );
        assert_eq!(
            store
                .list(Some("ground"))
                .expect("search after rebuild")
                .len(),
            1
        );
        assert!(store
            .list(Some("orbit"))
            .expect("tombstone remains excluded after rebuild")
            .is_empty());
    }

    #[test]
    fn opening_schema_v2_recreates_a_missing_search_cache() {
        let path = temporary_path("fts-reopen");
        let before = {
            let store = MobileStore::open(&path).expect("open search database");
            store
                .create("Rebuildable", "authoritative content")
                .expect("create source note");
            store.export_notes().expect("export source state")
        };
        {
            let connection = Connection::open(&path).expect("open raw schema v2 database");
            connection
                .execute_batch(
                    "DROP TRIGGER mobile_notes_fts_insert;
                     DROP TRIGGER mobile_notes_fts_update;
                     DROP TRIGGER mobile_notes_fts_delete;
                     DROP TABLE mobile_notes_fts;",
                )
                .expect("drop search cache");
        }

        let reopened = MobileStore::open(&path).expect("reopen and recreate search cache");
        assert_eq!(
            reopened.export_notes().expect("export recreated state"),
            before
        );
        assert_eq!(
            reopened
                .list(Some("authoritative"))
                .expect("search rebuilt content")
                .len(),
            1
        );
        remove_database(&path);
    }

    #[test]
    fn notes_export_restores_portable_state_but_rotates_device_identity() {
        let source = store();
        let first = source
            .create("Portable", "first revision")
            .expect("create first export note");
        source
            .update(&first.record_id, "Portable revised", "second revision")
            .expect("revise export note");
        let trashed = source
            .create("Trash", "restorable content")
            .expect("create trash export note");
        source
            .delete(&trashed.record_id)
            .expect("trash export note");
        let tombstoned = source
            .create("Tombstone", "retained tombstone content")
            .expect("create tombstone export note");
        source
            .delete(&tombstoned.record_id)
            .expect("trash before tombstone");
        source
            .tombstone(&tombstoned.record_id)
            .expect("tombstone export note");
        let library_id = new_uuid_v7();
        let scope_id = new_uuid_v7();
        assert_eq!(
            source
                .adopt_staging_library(&library_id, &scope_id)
                .expect("adopt paired library before export"),
            3
        );

        let export = source.export_notes().expect("export portable notes");
        let decoded: serde_json::Value =
            serde_json::from_str(&export).expect("parse exported JSON");
        let source_envelope: MobileNotesExportEnvelope =
            serde_json::from_str(&export).expect("decode source export");
        assert_eq!(decoded["format"], MOBILE_NOTES_EXPORT_FORMAT);
        assert_eq!(decoded["formatVersion"], MOBILE_NOTES_EXPORT_VERSION);
        assert_eq!(source_envelope.payload.replica.library_state, "paired");
        assert!(decoded["payload"]["notes"]
            .as_array()
            .expect("export notes array")
            .iter()
            .all(|note| note.get("id").is_none() && note.get("path").is_none()));
        assert!(decoded.get("migrationRecoveryPath").is_none());

        let restored = store();
        assert_eq!(
            restored
                .restore_notes_export(&export)
                .expect("restore portable notes"),
            3
        );
        let restored_export = restored.export_notes().expect("re-export restored notes");
        let mut restored_envelope: MobileNotesExportEnvelope =
            serde_json::from_str(&restored_export).expect("decode restored export");
        assert_eq!(
            restored_envelope.payload.replica.library_id,
            source_envelope.payload.replica.library_id
        );
        assert_eq!(
            restored_envelope.payload.replica.default_scope_id,
            source_envelope.payload.replica.default_scope_id
        );
        assert_ne!(
            restored_envelope.payload.replica.device_id,
            source_envelope.payload.replica.device_id
        );
        assert_ne!(
            restored_envelope.payload.replica.install_id,
            source_envelope.payload.replica.install_id
        );
        assert_eq!(
            restored_envelope.payload.replica.library_state,
            "local_staging"
        );
        assert_eq!(
            restored_envelope.payload.replica.next_transaction_counter,
            1
        );

        for (restored_note, source_note) in restored_envelope
            .payload
            .notes
            .iter_mut()
            .zip(source_envelope.payload.notes.iter())
        {
            assert_eq!(restored_note.sync_state, "restore_pending");
            restored_note.sync_state.clone_from(&source_note.sync_state);
        }
        assert_eq!(
            restored_envelope.payload.notes, source_envelope.payload.notes,
            "restore changed record IDs, revisions, branches, lifecycle, hashes, or organization"
        );
        for (restored_outbox, source_outbox) in restored_envelope
            .payload
            .outbox
            .iter_mut()
            .zip(source_envelope.payload.outbox.iter())
        {
            assert_eq!(restored_outbox.state, "superseded");
            assert!(!restored_outbox.eligible_for_sync);
            assert!(restored_outbox.superseded_at.is_some());
            restored_outbox.state.clone_from(&source_outbox.state);
            restored_outbox.eligible_for_sync = source_outbox.eligible_for_sync;
            restored_outbox.superseded_at = source_outbox.superseded_at;
        }
        assert_eq!(
            restored_envelope.payload.outbox, source_envelope.payload.outbox,
            "restore changed stable mutation IDs or branch/version history"
        );
        {
            let connection = restored.connection.lock().expect("lock restored store");
            let restored_identity = replica_identity(&connection).expect("restored identity");
            let sendable_old_identity: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM mobile_note_outbox
                     WHERE eligible_for_sync = 1
                        OR device_id = ?1
                        OR install_id = ?2",
                    params![restored_identity.device_id, restored_identity.install_id],
                    |row| row.get(0),
                )
                .expect("verify quarantined outbox identity");
            assert_eq!(sendable_old_identity, 0);
            assert_eq!(restored_identity.library_state, "local_staging");
        }
        assert_eq!(
            restored
                .list(Some("second revision"))
                .expect("search restored active note")
                .len(),
            1
        );
        assert!(restored
            .list(Some("restorable"))
            .expect("trash excluded after restore")
            .is_empty());
        assert!(restored
            .list(Some("retained tombstone"))
            .expect("tombstone excluded after restore")
            .is_empty());
        assert!(restored
            .restore_notes_export(&export)
            .expect_err("a populated store must not be overwritten")
            .contains("will not overwrite"));
    }

    #[test]
    fn notes_restore_rejects_checksum_and_semantic_tampering_atomically() {
        let source = store();
        source
            .create("Untampered", "canonical content")
            .expect("create source note");
        let export = source.export_notes().expect("export source notes");
        let mut tampered: serde_json::Value =
            serde_json::from_str(&export).expect("parse source export");
        tampered["payload"]["notes"][0]["body"] =
            serde_json::Value::String("changed after export".to_string());

        let target = store();
        let checksum_error = target
            .restore_notes_export(
                &serde_json::to_string(&tampered).expect("serialize checksum tamper"),
            )
            .expect_err("checksum tamper must fail");
        assert!(checksum_error.contains("checksum"), "{checksum_error}");
        assert!(target
            .list(None)
            .expect("empty after checksum failure")
            .is_empty());

        tampered["payloadSha256"] =
            serde_json::Value::String(canonical_sha256(&tampered["payload"]));
        let semantic_error = target
            .restore_notes_export(
                &serde_json::to_string(&tampered).expect("serialize semantic tamper"),
            )
            .expect_err("rechecksummed semantic tamper must fail");
        assert!(
            semantic_error.contains("invalid portable state"),
            "{semantic_error}"
        );
        assert!(target
            .list(None)
            .expect("empty after semantic failure")
            .is_empty());
        let connection = target.connection.lock().expect("lock untouched target");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count untouched outbox"),
            0
        );
    }

    #[test]
    fn file_backed_notes_and_replica_identity_survive_reopen() {
        let path = temporary_path("reopen");
        let (record_id, identity) = {
            let store = MobileStore::open(&path).expect("open file-backed store");
            let note = store
                .create("Persistent", "Still here")
                .expect("create persistent note");
            let connection = store.connection.lock().expect("lock store");
            let record_id = note.record_id;
            let identity = replica_identity(&connection).expect("read identity");
            (record_id, identity)
        };

        let reopened = MobileStore::open(&path).expect("reopen file-backed store");
        assert_eq!(
            reopened
                .migration_recovery_path()
                .expect("read recovery state"),
            None
        );
        let notes = reopened.list(None).expect("list reopened notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Persistent");
        let connection = reopened.connection.lock().expect("lock reopened store");
        let reopened_record_id: String = connection
            .query_row("SELECT record_id FROM mobile_notes", [], |row| row.get(0))
            .expect("read reopened record id");
        let reopened_identity = replica_identity(&connection).expect("read reopened identity");
        assert_eq!(reopened_record_id, record_id);
        assert_eq!(reopened_identity.library_id, identity.library_id);
        assert_eq!(reopened_identity.device_id, identity.device_id);
        assert_eq!(reopened_identity.install_id, identity.install_id);
        assert_eq!(
            reopened_identity.default_scope_id,
            identity.default_scope_id
        );
        assert_eq!(reopened_identity.library_state, "local_staging");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count outbox"),
            1
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))
                .expect("read application id"),
            MOBILE_APPLICATION_ID
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count migration history"),
            PORTABLE_SCHEMA_VERSION
        );
        drop(connection);
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn legacy_rows_migrate_without_changing_visible_data_or_repeating_outbox() {
        let path = temporary_path("legacy");
        {
            let connection = Connection::open(&path).expect("open legacy database");
            connection
                .execute_batch(
                    "CREATE TABLE mobile_notes (
                       id INTEGER PRIMARY KEY AUTOINCREMENT,
                       title TEXT NOT NULL,
                       body TEXT NOT NULL,
                       created_at INTEGER NOT NULL,
                       updated_at INTEGER NOT NULL,
                       deleted_at INTEGER
                     );
                     INSERT INTO mobile_notes
                       (title, body, created_at, updated_at, deleted_at)
                     VALUES
                       (' Legacy title ', 'legacy body', 100, 200, NULL),
                       ('Removed', 'preserved tombstone', 300, 400, 500);",
                )
                .expect("seed legacy database");
        }

        let (record_ids, identity, recovery_path) = {
            let store = MobileStore::open(&path).expect("migrate legacy database");
            let recovery_path = PathBuf::from(
                store
                    .migration_recovery_path()
                    .expect("read recovery state")
                    .expect("legacy migration recovery path"),
            );
            assert!(recovery_path.is_file());
            let recovery = Connection::open(&recovery_path).expect("open recovery database");
            assert_eq!(
                recovery
                    .query_row("SELECT COUNT(*) FROM mobile_notes", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .expect("count recovery rows"),
                2
            );
            assert!(!recovery
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM pragma_table_info('mobile_notes')
                       WHERE name = 'record_id'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect recovery schema"));
            drop(recovery);
            let connection = store.connection.lock().expect("lock migrated store");
            let visible: (String, String, i64, i64) = connection
                .query_row(
                    "SELECT title, body, created_at, updated_at
                     FROM mobile_notes WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read migrated visible data");
            assert_eq!(
                visible,
                (
                    " Legacy title ".to_string(),
                    "legacy body".to_string(),
                    100,
                    200
                )
            );
            let lifecycle: String = connection
                .query_row(
                    "SELECT lifecycle_state FROM mobile_notes WHERE id = 2",
                    [],
                    |row| row.get(0),
                )
                .expect("read migrated tombstone");
            assert_eq!(lifecycle, "tombstone");
            let record_ids = connection
                .prepare("SELECT record_id FROM mobile_notes ORDER BY id")
                .expect("prepare record ids")
                .query_map([], |row| row.get::<_, String>(0))
                .expect("query record ids")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect record ids");
            assert!(record_ids.iter().all(|id| is_uuid_v7(id)));
            assert_ne!(record_ids[0], record_ids[1]);
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| row
                        .get::<_, i64>(0))
                    .expect("count migrated outbox"),
                2
            );
            let identity = replica_identity(&connection).expect("identity");
            assert_eq!(
                record_ids[0],
                deterministic_backfill_uuid_v7(
                    100,
                    &format!("noted.mobile-notes.{}", identity.library_id),
                    "1"
                )
            );
            (record_ids, identity, recovery_path)
        };

        let reopened = MobileStore::open(&path).expect("reopen migrated database");
        assert_eq!(
            reopened
                .migration_recovery_path()
                .expect("read reopened recovery state")
                .as_deref(),
            recovery_path.to_str()
        );
        let connection = reopened.connection.lock().expect("lock reopened database");
        let reopened_ids = connection
            .prepare("SELECT record_id FROM mobile_notes ORDER BY id")
            .expect("prepare reopened ids")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query reopened ids")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect reopened ids");
        assert_eq!(reopened_ids, record_ids);
        assert_eq!(
            replica_identity(&connection)
                .expect("reopened identity")
                .device_id,
            identity.device_id
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count reopened outbox"),
            2
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("read user version"),
            PORTABLE_SCHEMA_VERSION
        );
        drop(connection);
        drop(reopened);
        remove_database(&path);
        let _ = std::fs::remove_file(recovery_path);
    }

    #[test]
    fn schema_v1_upgrades_in_order_with_an_exact_verified_recovery_snapshot() {
        let path = temporary_path("v1-to-v2");
        {
            let mut connection = Connection::open(&path).expect("open v1 fixture database");
            connection
                .execute_batch(
                    "CREATE TABLE mobile_notes (
                       id INTEGER PRIMARY KEY AUTOINCREMENT,
                       title TEXT NOT NULL,
                       body TEXT NOT NULL,
                       created_at INTEGER NOT NULL,
                       updated_at INTEGER NOT NULL,
                       deleted_at INTEGER
                     );
                     INSERT INTO mobile_notes
                       (title, body, created_at, updated_at, deleted_at)
                     VALUES ('V1 tombstone', 'preserve it', 100, 200, 300);",
                )
                .expect("seed pre-v1 row");
            migrate_portable_notes_to_version(&mut connection, None, 1)
                .expect("construct the exact v1 schema");
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("read v1 version"),
                1
            );
            assert!(!connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM pragma_table_info('mobile_note_outbox')
                       WHERE name = 'transaction_member_index'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect v1 outbox"));
        }

        let store = MobileStore::open(&path).expect("upgrade v1 database to v2");
        let recovery_path = PathBuf::from(
            store
                .migration_recovery_path()
                .expect("read v2 recovery state")
                .expect("v1 recovery snapshot path"),
        );
        let snapshot =
            Connection::open_with_flags(&recovery_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open v1 recovery snapshot");
        assert_eq!(
            snapshot
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("read snapshot version"),
            1
        );
        assert_eq!(
            snapshot
                .query_row(
                    "SELECT name, checksum FROM mobile_schema_migrations WHERE version = 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("read v1 migration stamp"),
            (
                PORTABLE_MIGRATION_V1_NAME.to_string(),
                PORTABLE_SCHEMA_V1_CHECKSUM.to_string()
            )
        );
        assert_eq!(
            snapshot
                .query_row(
                    "SELECT lifecycle_state, deleted_at, tombstoned_at FROM mobile_notes",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .expect("read v1 tombstone"),
            ("tombstone".to_string(), 300, 300)
        );
        assert!(!snapshot
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'trashed_at'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect snapshot lifecycle schema"));
        drop(snapshot);

        let connection = store.connection.lock().expect("lock upgraded store");
        assert_eq!(
            connection
                .prepare("SELECT version, checksum FROM mobile_schema_migrations ORDER BY version")
                .expect("prepare ordered history")
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .expect("query ordered history")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect ordered history"),
            vec![
                (1, PORTABLE_SCHEMA_V1_CHECKSUM.to_string()),
                (2, PORTABLE_SCHEMA_V2_CHECKSUM.to_string())
            ]
        );
        let lifecycle: (String, i64, i64) = connection
            .query_row(
                "SELECT lifecycle_state, trashed_at, tombstoned_at FROM mobile_notes",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read upgraded lifecycle");
        assert_eq!(lifecycle, ("tombstone".to_string(), 300, 300));
        let members: (i64, i64) = connection
            .query_row(
                "SELECT transaction_member_index, transaction_member_count
                 FROM mobile_note_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read upgraded singleton transaction");
        assert_eq!(members, (0, 1));
        drop(connection);
        drop(store);
        remove_database(&path);
        let _ = std::fs::remove_file(recovery_path);
    }

    #[test]
    fn restoring_pre_migration_snapshot_recreates_stable_library_scope_and_record_ids() {
        let path = temporary_path("restore-determinism");
        {
            let connection = Connection::open(&path).expect("open legacy database");
            connection
                .execute_batch(
                    "CREATE TABLE mobile_notes (
                       id INTEGER PRIMARY KEY AUTOINCREMENT,
                       title TEXT NOT NULL,
                       body TEXT NOT NULL,
                       created_at INTEGER NOT NULL,
                       updated_at INTEGER NOT NULL,
                       deleted_at INTEGER
                     );
                     INSERT INTO mobile_notes
                       (title, body, created_at, updated_at, deleted_at)
                     VALUES ('Restorable', 'same identity', 1234, 5678, NULL);",
                )
                .expect("seed legacy database");
        }

        let (first_identity, first_record_id, recovery_path) = {
            let store = MobileStore::open(&path).expect("first migration");
            let recovery_path = PathBuf::from(
                store
                    .migration_recovery_path()
                    .expect("read recovery")
                    .expect("recovery path"),
            );
            let connection = store.connection.lock().expect("lock first migration");
            let identity = replica_identity(&connection).expect("first identity");
            let record_id = connection
                .query_row("SELECT record_id FROM mobile_notes", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("first record id");
            (identity, record_id, recovery_path)
        };

        remove_database(&path);
        std::fs::copy(&recovery_path, &path).expect("restore pre-migration database");

        let second_recovery_path;
        {
            let restored = MobileStore::open(&path).expect("rerun migration after restore");
            second_recovery_path = PathBuf::from(
                restored
                    .migration_recovery_path()
                    .expect("read second recovery")
                    .expect("second recovery path"),
            );
            let connection = restored.connection.lock().expect("lock restored store");
            let second_identity = replica_identity(&connection).expect("second identity");
            let second_record_id = connection
                .query_row("SELECT record_id FROM mobile_notes", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("second record id");
            assert_eq!(second_identity.library_id, first_identity.library_id);
            assert_eq!(
                second_identity.default_scope_id,
                first_identity.default_scope_id
            );
            assert_eq!(second_record_id, first_record_id);
            // Restoring pre-enrollment data represents a new local install;
            // device/install IDs intentionally do not pretend to be durable.
            assert_ne!(second_identity.device_id, first_identity.device_id);
        }

        remove_database(&path);
        let _ = std::fs::remove_file(recovery_path);
        let _ = std::fs::remove_file(second_recovery_path);
    }

    #[test]
    fn stamped_mobile_schema_enforces_writer_floor_and_immutable_history() {
        let path = temporary_path("writer-floor");
        {
            let store = MobileStore::open(&path).expect("create current store");
            let connection = store.connection.lock().expect("lock current store");
            assert!(connection
                .execute(
                    "UPDATE mobile_schema_migrations SET checksum = 'rewritten' WHERE version = 1",
                    [],
                )
                .is_err());
            connection
                .execute(
                    "UPDATE mobile_schema_state SET min_writer_version = ?1 WHERE singleton = 1",
                    [PORTABLE_SCHEMA_VERSION + 1],
                )
                .expect("raise writer floor fixture");
        }

        let error = MobileStore::open(&path)
            .err()
            .expect("older writer must be rejected");
        assert!(error.contains("writer protocol"), "{error}");
        remove_database(&path);
    }

    #[test]
    fn every_mutation_advances_working_revision_and_enters_outbox() {
        let store = store();
        let note = store.create("Draft", "one").expect("create note");
        store
            .update(&note.record_id, "Draft", "two")
            .expect("update note");
        store.delete(&note.record_id).expect("trash note");
        let restored = store.restore(&note.record_id).expect("restore note");
        assert_eq!(restored.body, "two");

        let connection = store.connection.lock().expect("lock store");
        let (accepted, working, lifecycle, deleted_at): (i64, i64, String, Option<i64>) =
            connection
                .query_row(
                    "SELECT accepted_revision, working_revision, lifecycle_state, deleted_at
                 FROM mobile_notes WHERE record_id = ?1",
                    [&note.record_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read portable state");
        assert_eq!(accepted, 0);
        assert_eq!(working, 4);
        assert_eq!(lifecycle, "active");
        assert_eq!(deleted_at, None);

        let operations = connection
            .prepare(
                "SELECT operation, local_revision, base_revision, proposed_revision
                 FROM mobile_note_outbox ORDER BY local_sequence",
            )
            .expect("prepare outbox")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .expect("query outbox")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect outbox");
        assert_eq!(
            operations,
            vec![
                ("create".to_string(), 1, 0, 1),
                ("update".to_string(), 2, 0, 1),
                ("trash".to_string(), 3, 0, 1),
                ("restore".to_string(), 4, 0, 1),
            ]
        );
        let distinct_ids: i64 = connection
            .query_row(
                "SELECT COUNT(DISTINCT mutation_id) FROM mobile_note_outbox",
                [],
                |row| row.get(0),
            )
            .expect("count mutation ids");
        assert_eq!(distinct_ids, 4);
        let sendability = connection
            .prepare(
                "SELECT operation, state, eligible_for_sync
                 FROM mobile_note_outbox ORDER BY local_sequence",
            )
            .expect("prepare sendability")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .expect("query sendability")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect sendability");
        assert_eq!(
            sendability,
            vec![
                ("create".to_string(), "superseded".to_string(), 0),
                ("update".to_string(), "superseded".to_string(), 0),
                ("trash".to_string(), "superseded".to_string(), 0),
                ("restore".to_string(), "pending".to_string(), 1),
            ]
        );
        let payload_json: String = connection
            .query_row(
                "SELECT payload_json FROM mobile_note_outbox
                 ORDER BY local_sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("read portable payload");
        let payload: serde_json::Value =
            serde_json::from_str(&payload_json).expect("parse portable payload");
        assert_eq!(
            payload["mutation_contract_version"],
            "noted.mobile-note-mutation.shadow.v1"
        );
        assert!(payload.get("record").is_none());
        assert_eq!(
            payload["proposed_record"]["proposal_contract_version"],
            "noted.proposed-record.v1"
        );
        assert_eq!(
            payload["proposed_record"]["local_branch"]["state"],
            "pending"
        );
        assert_eq!(
            payload["proposed_record"]["local_branch"]["base_revision"],
            0
        );
        assert_eq!(payload["proposed_record"]["scope"]["class"], "personal");
        assert!(is_uuid(
            payload["proposed_record"]["scope"]["scope_id"]
                .as_str()
                .expect("scope id")
        ));
        assert!(payload["proposed_record"]["created_at"]
            .as_str()
            .expect("created at")
            .ends_with('Z'));
    }

    #[test]
    fn first_pairing_adopts_staging_library_without_changing_record_ids() {
        let store = store();
        let first = store.create("First", "one").expect("create first");
        store
            .update(&first.record_id, "First", "revised")
            .expect("update first");
        let second = store.create("Second", "two").expect("create second");
        store.delete(&second.record_id).expect("tombstone second");

        let (staging_library_id, staging_scope_id, record_ids, old_outbox_count) = {
            let connection = store.connection.lock().expect("lock staging store");
            let identity = replica_identity(&connection).expect("staging identity");
            assert_eq!(identity.library_state, "local_staging");
            let rows = connection
                .prepare("SELECT record_id, scope_id FROM mobile_notes ORDER BY id")
                .expect("prepare staging identifiers")
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .expect("query staging identifiers")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect staging identifiers");
            let record_ids = rows.iter().map(|row| row.0.clone()).collect::<Vec<_>>();
            assert!(rows.iter().all(|row| row.1 == identity.default_scope_id));
            let outbox_count = connection
                .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count staging outbox");
            (
                identity.library_id,
                identity.default_scope_id,
                record_ids,
                outbox_count,
            )
        };
        assert_eq!(old_outbox_count, 4);

        let mac_library_id = new_uuid_v7();
        let mac_scope_id = new_uuid_v7();
        assert_eq!(
            store
                .adopt_staging_library(&mac_library_id, &mac_scope_id)
                .expect("adopt Mac library"),
            2
        );

        {
            let connection = store.connection.lock().expect("lock adopted store");
            let identity = replica_identity(&connection).expect("paired identity");
            assert_eq!(identity.library_id, mac_library_id);
            assert_eq!(identity.default_scope_id, mac_scope_id);
            assert_eq!(identity.library_state, "paired");
            let rows = connection
                .prepare(
                    "SELECT record_id, scope_id, library_id, working_revision
                     FROM mobile_notes ORDER BY id",
                )
                .expect("prepare adopted notes")
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .expect("query adopted notes")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect adopted notes");
            assert_eq!(
                rows.iter().map(|row| row.0.clone()).collect::<Vec<_>>(),
                record_ids
            );
            assert!(rows.iter().all(|row| row.1 == mac_scope_id));
            assert!(rows.iter().all(|row| row.2 == mac_library_id));
            assert_eq!(rows.iter().map(|row| row.3).collect::<Vec<_>>(), vec![3, 3]);

            let (superseded, adopted): (i64, i64) = connection
                .query_row(
                    "SELECT
                       SUM(CASE WHEN eligible_for_sync = 0 THEN 1 ELSE 0 END),
                       SUM(CASE WHEN eligible_for_sync = 1 THEN 1 ELSE 0 END)
                     FROM mobile_note_outbox",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("count staged and adopted mutations");
            assert_eq!(superseded, old_outbox_count);
            assert_eq!(adopted, 2);
            let adopted_rows = connection
                .prepare(
                    "SELECT operation, library_id, payload_json
                     FROM mobile_note_outbox
                     WHERE eligible_for_sync = 1
                     ORDER BY local_sequence",
                )
                .expect("prepare adopted outbox")
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .expect("query adopted outbox")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect adopted outbox");
            assert!(adopted_rows
                .iter()
                .all(|row| row.0 == "create" && row.1 == mac_library_id));
            for row in adopted_rows {
                let payload: serde_json::Value =
                    serde_json::from_str(&row.2).expect("parse adopted payload");
                assert_eq!(payload["proposed_record"]["library_id"], mac_library_id);
                assert_eq!(
                    payload["proposed_record"]["scope"]["scope_id"],
                    mac_scope_id
                );
            }
            let transaction_members = connection
                .prepare(
                    "SELECT transaction_id, device_transaction_counter,
                            transaction_member_index, transaction_member_count
                     FROM mobile_note_outbox
                     WHERE eligible_for_sync = 1
                     ORDER BY transaction_member_index",
                )
                .expect("prepare adoption transaction members")
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .expect("query adoption transaction members")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect adoption transaction members");
            assert_eq!(transaction_members.len(), 2);
            assert_eq!(transaction_members[0].0, transaction_members[1].0);
            assert_eq!(transaction_members[0].1, transaction_members[1].1);
            assert_eq!(
                transaction_members
                    .iter()
                    .map(|member| (member.2, member.3))
                    .collect::<Vec<_>>(),
                vec![(0, 2), (1, 2)]
            );
            validate_outbox_transaction_groups(&connection)
                .expect("adoption transaction must be complete");
        }

        assert_eq!(
            store
                .adopt_staging_library(&mac_library_id, &mac_scope_id)
                .expect("repeat same pairing"),
            0
        );
        let other_library_id = new_uuid_v7();
        assert!(store
            .adopt_staging_library(&other_library_id, &mac_scope_id)
            .expect_err("cannot silently move paired phone")
            .contains("different"));
        let identity = {
            let connection = store.connection.lock().expect("lock paired store");
            replica_identity(&connection).expect("paired identity remains")
        };
        assert_eq!(identity.library_id, mac_library_id);
        assert_ne!(identity.library_id, staging_library_id);
        assert_ne!(identity.default_scope_id, staging_scope_id);
    }

    #[test]
    fn staging_adoption_rolls_back_if_new_create_cannot_enter_outbox() {
        let store = store();
        let first = store
            .create("First staged", "preserve me")
            .expect("create first note");
        let second = store
            .create("Second staged", "preserve me too")
            .expect("create second note");
        let staging_library_id = {
            let connection = store.connection.lock().expect("lock staging store");
            let identity = replica_identity(&connection).expect("staging identity");
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_adoption_outbox
                     BEFORE INSERT ON mobile_note_outbox
                     WHEN NEW.transaction_member_index = 1
                     BEGIN
                       SELECT RAISE(ABORT, 'injected second-member adoption failure');
                     END;",
                )
                .expect("create adoption failure trigger");
            identity.library_id
        };

        let error = store
            .adopt_staging_library(&new_uuid_v7(), &new_uuid_v7())
            .expect_err("adoption should fail atomically");
        assert!(error.contains("injected second-member adoption failure"));
        let connection = store.connection.lock().expect("lock rolled-back store");
        let identity = replica_identity(&connection).expect("rolled-back identity");
        assert_eq!(identity.library_id, staging_library_id);
        assert_eq!(identity.library_state, "local_staging");
        let states = connection
            .prepare(
                "SELECT record_id, library_id, working_revision
                 FROM mobile_notes ORDER BY id",
            )
            .expect("prepare rolled-back notes")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .expect("query rolled-back notes")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect rolled-back notes");
        assert_eq!(
            states,
            vec![
                (first.record_id, staging_library_id.clone(), 1),
                (second.record_id, staging_library_id, 1),
            ]
        );
        let outbox: (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), SUM(eligible_for_sync) FROM mobile_note_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read rolled-back outbox");
        assert_eq!(outbox, (2, 2));
        assert_eq!(
            connection
                .query_row(
                    "SELECT next_transaction_counter FROM mobile_replica WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read rolled-back transaction counter"),
            3
        );
    }

    #[test]
    fn staging_adoption_is_forbidden_after_sync_acceptance() {
        let store = store();
        let note = store
            .create("Accepted", "already observed")
            .expect("create note");
        {
            let connection = store.connection.lock().expect("lock staging store");
            connection
                .execute(
                    "UPDATE mobile_notes
                     SET accepted_revision = 1,
                         accepted_version_id = ?1,
                         accepted_content_hash = canonical_hash,
                         sync_state = 'acknowledged'
                     WHERE record_id = ?2",
                    params![new_uuid_v7(), note.record_id],
                )
                .expect("simulate accepted record");
        }

        let error = store
            .adopt_staging_library(&new_uuid_v7(), &new_uuid_v7())
            .expect_err("accepted state must prevent identity reassignment");
        assert!(error.contains("forbidden"), "{error}");
        let connection = store.connection.lock().expect("lock unchanged store");
        let identity = replica_identity(&connection).expect("read unchanged identity");
        assert_eq!(identity.library_state, "local_staging");
    }

    #[test]
    fn staging_adoption_is_forbidden_after_a_transport_attempt() {
        let store = store();
        store
            .create("Attempted", "not safe to re-home")
            .expect("create note");
        {
            let connection = store.connection.lock().expect("lock staging store");
            connection
                .execute(
                    "UPDATE mobile_note_outbox SET attempts = 1 WHERE eligible_for_sync = 1",
                    [],
                )
                .expect("simulate transport attempt");
        }

        let error = store
            .adopt_staging_library(&new_uuid_v7(), &new_uuid_v7())
            .expect_err("attempted transport must prevent identity reassignment");
        assert!(error.contains("forbidden"), "{error}");
    }

    #[test]
    fn externally_authoritative_notes_are_read_only_on_iphone() {
        let store = store();
        let note = store
            .create("Mirror", "owned elsewhere")
            .expect("create note");
        {
            let connection = store.connection.lock().expect("lock mobile store");
            connection
                .execute(
                    "UPDATE mobile_notes
                     SET authority = 'external', provenance_json = '{\"source\":\"brain\"}'
                     WHERE record_id = ?1",
                    [&note.record_id],
                )
                .expect("turn note into external mirror fixture");
        }

        let update_error = store
            .update(&note.record_id, "Changed", "must not persist")
            .expect_err("external mirror update must fail");
        assert!(update_error.contains("read-only"), "{update_error}");
        let delete_error = store
            .delete(&note.record_id)
            .expect_err("external mirror delete must fail");
        assert!(delete_error.contains("read-only"), "{delete_error}");

        let connection = store.connection.lock().expect("lock unchanged store");
        let state: (String, String, i64) = connection
            .query_row(
                "SELECT title, lifecycle_state, working_revision
                 FROM mobile_notes WHERE record_id = ?1",
                [&note.record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read unchanged mirror");
        assert_eq!(state, ("Mirror".to_string(), "active".to_string(), 1));
        let outbox_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| {
                row.get(0)
            })
            .expect("count unchanged outbox");
        assert_eq!(outbox_count, 1);
    }

    #[test]
    fn note_and_outbox_write_roll_back_together() {
        let store = store();
        {
            let connection = store.connection.lock().expect("lock store");
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_mobile_outbox
                     BEFORE INSERT ON mobile_note_outbox
                     BEGIN
                       SELECT RAISE(ABORT, 'injected outbox failure');
                     END;",
                )
                .expect("create failure trigger");
        }

        let error = store
            .create("Must roll back", "no orphan")
            .expect_err("outbox failure should fail note write");
        assert!(error.contains("injected outbox failure"));
        let connection = store.connection.lock().expect("lock store after failure");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_notes", [], |row| row
                    .get::<_, i64>(0))
                .expect("count notes"),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_note_outbox", [], |row| row
                    .get::<_, i64>(0))
                .expect("count outbox"),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT next_transaction_counter FROM mobile_replica WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read counter"),
            1
        );
    }

    #[test]
    fn failed_edit_keeps_previous_working_state_sendable() {
        let store = store();
        let note = store.create("Original", "safe").expect("create note");
        {
            let connection = store.connection.lock().expect("lock store");
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_edit_outbox
                     BEFORE INSERT ON mobile_note_outbox
                     BEGIN
                       SELECT RAISE(ABORT, 'injected edit outbox failure');
                     END;",
                )
                .expect("create edit failure trigger");
        }

        let error = store
            .update(&note.record_id, "Changed", "must roll back")
            .expect_err("edit should fail with its outbox insert");
        assert!(error.contains("injected edit outbox failure"));
        let connection = store.connection.lock().expect("lock rolled-back store");
        let current: (String, String, i64) = connection
            .query_row(
                "SELECT title, body, working_revision
                 FROM mobile_notes WHERE record_id = ?1",
                [&note.record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read rolled-back note");
        assert_eq!(current, ("Original".to_string(), "safe".to_string(), 1));
        let outbox: (i64, String, i64) = connection
            .query_row(
                "SELECT COUNT(*), state, eligible_for_sync
                 FROM mobile_note_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read surviving create mutation");
        assert_eq!(outbox, (1, "pending".to_string(), 1));
        assert_eq!(
            connection
                .query_row(
                    "SELECT next_transaction_counter FROM mobile_replica WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read rolled-back counter"),
            2
        );
    }

    #[test]
    fn record_and_replica_ids_are_uuid_v7_and_unique() {
        let store = store();
        let first = store.create("First", "one").expect("create first");
        let second = store.create("Second", "two").expect("create second");
        let connection = store.connection.lock().expect("lock store");
        let identity = replica_identity(&connection).expect("identity");
        assert!(is_uuid_v7(&identity.library_id));
        assert!(is_uuid_v7(&identity.device_id));
        assert!(is_uuid_v7(&identity.install_id));
        assert!(is_uuid_v7(&identity.default_scope_id));
        assert_eq!(identity.library_state, "local_staging");
        assert_ne!(identity.library_id, identity.device_id);
        assert_ne!(identity.device_id, identity.install_id);
        let ids = (first.record_id, second.record_id);
        assert!(is_uuid_v7(&ids.0));
        assert!(is_uuid_v7(&ids.1));
        assert_ne!(ids.0, ids.1);
    }

    #[test]
    fn canonical_hash_uses_shared_portable_hasher() {
        let first = note_content_hash("Same", "Body");
        let second = note_content_hash("Same", "Body");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert_eq!(first, canonical_sha256(&note_content("Same", "Body")));
        assert_ne!(first, note_content_hash("Same", "Different"));
    }

    #[test]
    fn future_schema_is_rejected_without_mutation() {
        let path = temporary_path("future");
        {
            let connection = Connection::open(&path).expect("open future database");
            connection
                .pragma_update(None, "user_version", PORTABLE_SCHEMA_VERSION + 1)
                .expect("stamp future version");
        }
        let error = MobileStore::open(&path)
            .err()
            .expect("future schema should be rejected");
        assert!(error.contains("newer than supported"));
        remove_database(&path);
    }
}
