use crate::portable::{
    canonical_json, canonical_sha256, deterministic_backfill_uuid_v7, is_uuid, is_uuid_v7,
    new_uuid_v7, AcceptedHead, AuthorityKind, ContextRecordV1, LifecycleState, LocalBranch,
    LocalBranchState, RecordAuthority, RecordLifecycle, RecordScope, ScopeClass,
};
use crate::{
    direct_sync::{PushRequest, PushResponse, SignedSyncRequest, SignedSyncResponse},
    pairing_client::{PairingClientCheckpoint, PairingClientState},
    pairing_protocol::{
        fixture_record_capabilities, fixture_record_scopes, validate_bootstrap, BootstrapEnvelope,
        Environment, Invitation, KindCapability, LibraryDataClass, PairingRole, RecordKind,
        ServerFinish, ServerHello, BOOTSTRAP_SYNC_PROTOCOL_VERSION, MAX_PAIRING_MESSAGE_BYTES,
        PAIRING_PROTOCOL, PAIRING_SUITE, RECORD_CIPHER_SUITE,
    },
    sync_protocol::{ReceiptDisposition, TerminalRejection, SYNC_PROTOCOL_VERSION},
};
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

const PORTABLE_SCHEMA_VERSION: i64 = 8;
const PORTABLE_SCHEMA_V1_CHECKSUM: &str =
    "d6d8377525aa80d91e9e7cb22d4eff4da5cf7998abc8968a5457c1fc86e84b7b";
const PORTABLE_SCHEMA_V2_CHECKSUM: &str =
    "838992191ee7053706d16154b41f08b1e041526101d496d1d762848a614b8e45";
const PORTABLE_SCHEMA_V3_CHECKSUM: &str =
    "17914efe0d9e4d164d7f70f3ed0865184d1f560dcb028f19d39e79cb75d1f70b";
const PORTABLE_SCHEMA_V4_CHECKSUM: &str =
    "b0a8c29148518f29b2ef257ad344fe6b9bbe8fab1e02100f4ef7a4ab91e7ae8f";
const PORTABLE_SCHEMA_V5_CHECKSUM: &str =
    "c88b728d82871ba599c9b92a247e79ccd95cb60165e21d859ac6689dc8c0ea46";
const PORTABLE_SCHEMA_V6_CHECKSUM: &str =
    "116f0b2173408450630ef6a04fb9cc4d5507066c2f47f37ccae977bfa1665d39";
const PORTABLE_SCHEMA_V7_CHECKSUM: &str =
    "fe5078bed01caf0fe697b2b66287776cf8be4de91455409a40cbe7109d7f78bd";
const PORTABLE_SCHEMA_V8_CHECKSUM: &str =
    "2756ab5086b94ebeb6e29560408ed762df7047962d489b7b5ffbcd78c10d5bed";
const PORTABLE_MIGRATION_V1_NAME: &str = "iphone-notes-portability";
const PORTABLE_MIGRATION_V2_NAME: &str = "iphone-note-lifecycle-and-transaction-groups";
const PORTABLE_MIGRATION_V3_NAME: &str = "iphone-note-workspace-and-sync-state";
const PORTABLE_MIGRATION_V4_NAME: &str = "iphone-sanitized-fixture-pairing-checkpoint";
const PORTABLE_MIGRATION_V5_NAME: &str = "iphone-atomic-pairing-activation";
const PORTABLE_MIGRATION_V6_NAME: &str = "iphone-direct-sync-wire-journal-and-bootstrap-pages";
const PORTABLE_MIGRATION_V7_NAME: &str = "iphone-lossless-canonical-context-records";
const PORTABLE_MIGRATION_V8_NAME: &str = "iphone-durable-authority-revocation";
const PORTABLE_SCHEMA_V4_DDL: &str = r#"CREATE TABLE mobile_pairing_checkpoint_v1 (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               fixture_class TEXT NOT NULL CHECK (fixture_class = 'sanitized_fixture'),
               device_id TEXT NOT NULL,
               identity_handle TEXT NOT NULL,
               pending_bootstrap_handle TEXT,
               state TEXT NOT NULL CHECK (state IN (
                 'ready', 'awaiting_server_hello', 'awaiting_user_confirmation',
                 'awaiting_bootstrap', 'bootstrap_prepared', 'awaiting_server_finish',
                 'pending_activation', 'active', 'cancellation_pending', 'cancelled'
               )),
               invitation_bytes BLOB NOT NULL
                 CHECK (length(invitation_bytes) BETWEEN 1 AND 16384),
               client_hello_bytes BLOB CHECK (length(client_hello_bytes) BETWEEN 1 AND 16384),
               server_hello_bytes BLOB CHECK (length(server_hello_bytes) BETWEEN 1 AND 16384),
               bootstrap_bytes BLOB CHECK (length(bootstrap_bytes) BETWEEN 1 AND 16384),
               client_finish_bytes BLOB CHECK (length(client_finish_bytes) BETWEEN 1 AND 16384),
               server_finish_bytes BLOB CHECK (length(server_finish_bytes) BETWEEN 1 AND 16384),
               transcript_digest BLOB CHECK (length(transcript_digest) = 32),
               receipt_id TEXT,
               envelope_digest BLOB CHECK (length(envelope_digest) = 32),
               user_decision INTEGER CHECK (user_decision IN (0, 1)),
               checkpoint_json TEXT NOT NULL CHECK (length(checkpoint_json) <= 131072),
               updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
               CHECK (pending_bootstrap_handle IS NULL OR
                      (receipt_id IS NOT NULL AND envelope_digest IS NOT NULL)),
               CHECK (
                 (state IN ('awaiting_server_finish', 'pending_activation')
                    AND pending_bootstrap_handle IS NOT NULL)
                 OR
                 (state NOT IN ('awaiting_server_finish', 'pending_activation')
                    AND pending_bootstrap_handle IS NULL)
               )
             );"#;
const PORTABLE_SCHEMA_V5_DDL: &str = r#"CREATE TABLE mobile_pairing_activation_v1 (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               fixture_class TEXT NOT NULL CHECK (fixture_class = 'sanitized_fixture'),
               receipt_id TEXT NOT NULL,
               library_id TEXT NOT NULL,
               device_id TEXT NOT NULL,
               default_scope_id TEXT NOT NULL,
               authority_generation INTEGER NOT NULL CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL CHECK (purge_generation >= 0),
               key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
               sync_spki_sha256 BLOB NOT NULL CHECK (length(sync_spki_sha256) = 32),
               record_cipher_suite TEXT NOT NULL
                 CHECK (length(record_cipher_suite) BETWEEN 1 AND 128),
               granted_scopes_json TEXT NOT NULL
                 CHECK (length(granted_scopes_json) BETWEEN 1 AND 4096),
               capabilities_json TEXT NOT NULL
                 CHECK (length(capabilities_json) BETWEEN 1 AND 8192),
               activation_json TEXT NOT NULL
                 CHECK (length(activation_json) BETWEEN 1 AND 262144),
               activation_sha256 TEXT NOT NULL CHECK (length(activation_sha256) = 64),
               adopted_note_count INTEGER NOT NULL CHECK (adopted_note_count >= 0),
               finalized_at INTEGER NOT NULL CHECK (finalized_at >= 0)
             );"#;
const PORTABLE_SCHEMA_V6_DDL: &str = r#"CREATE TABLE mobile_direct_sync_push_counter_v1 (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               next_counter INTEGER NOT NULL CHECK (next_counter > 0)
             );
             INSERT INTO mobile_direct_sync_push_counter_v1 (singleton, next_counter)
             VALUES (1, 1);
             CREATE TABLE mobile_direct_sync_journal_summary_v1 (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               pruned_through_sequence INTEGER NOT NULL DEFAULT 0
                 CHECK (pruned_through_sequence >= 0),
               pruned_completed_count INTEGER NOT NULL DEFAULT 0
                 CHECK (pruned_completed_count >= 0),
               pruned_request_bytes INTEGER NOT NULL DEFAULT 0
                 CHECK (pruned_request_bytes >= 0),
               pruned_response_bytes INTEGER NOT NULL DEFAULT 0
                 CHECK (pruned_response_bytes >= 0),
               max_pruned_push_counter INTEGER NOT NULL DEFAULT 0
                 CHECK (max_pruned_push_counter >= 0),
               updated_at INTEGER NOT NULL DEFAULT 0 CHECK (updated_at >= 0)
             );
             INSERT INTO mobile_direct_sync_journal_summary_v1 (singleton) VALUES (1);
             CREATE TABLE mobile_direct_sync_request_v1 (
               local_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               request_id TEXT NOT NULL CHECK (length(request_id) = 36),
               endpoint TEXT NOT NULL CHECK (endpoint IN (
                 '/sync/v1/negotiate', '/sync/v1/bootstrap', '/sync/v1/push',
                 '/sync/v1/pull', '/sync/v1/checkpoint', '/sync/v1/ack'
               )),
               operation TEXT NOT NULL CHECK (operation IN (
                 'negotiate', 'bootstrap', 'push', 'pull', 'checkpoint', 'ack'
               )),
               purpose_json BLOB NOT NULL
                 CHECK (length(purpose_json) BETWEEN 1 AND 16384),
               purpose_sha256 TEXT NOT NULL CHECK (length(purpose_sha256) = 64),
               push_transaction_id TEXT CHECK (
                 push_transaction_id IS NULL OR length(push_transaction_id) = 36
               ),
               push_counter INTEGER CHECK (push_counter IS NULL OR push_counter > 0),
               receipt_id TEXT NOT NULL CHECK (length(receipt_id) = 36),
               activation_sha256 TEXT NOT NULL CHECK (length(activation_sha256) = 64),
               library_id TEXT NOT NULL CHECK (length(library_id) = 36),
               device_id TEXT NOT NULL CHECK (length(device_id) = 36),
               authority_generation INTEGER NOT NULL CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL CHECK (purge_generation >= 0),
               key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
               sync_spki_sha256 BLOB NOT NULL CHECK (length(sync_spki_sha256) = 32),
               request_bytes BLOB NOT NULL
                 CHECK (length(request_bytes) BETWEEN 1 AND 4194304),
               request_sha256 TEXT NOT NULL CHECK (length(request_sha256) = 64),
               request_content_type TEXT NOT NULL
                 CHECK (request_content_type = 'application/json'),
               response_status INTEGER
                 CHECK (response_status IS NULL OR response_status BETWEEN 100 AND 599),
               response_content_type TEXT
                 CHECK (response_content_type IS NULL OR length(response_content_type) BETWEEN 1 AND 128),
               response_bytes BLOB
                 CHECK (response_bytes IS NULL OR length(response_bytes) BETWEEN 1 AND 4194304),
               response_sha256 TEXT CHECK (response_sha256 IS NULL OR length(response_sha256) = 64),
               state TEXT NOT NULL DEFAULT 'pending'
                 CHECK (state IN ('pending', 'response_received', 'completed', 'quarantined')),
               attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 100),
               created_at INTEGER NOT NULL CHECK (created_at >= 0),
               updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
               last_attempt_at INTEGER CHECK (last_attempt_at IS NULL OR last_attempt_at >= created_at),
               response_received_at INTEGER
                 CHECK (response_received_at IS NULL OR response_received_at >= created_at),
               completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= created_at),
               quarantined_at INTEGER CHECK (quarantined_at IS NULL OR quarantined_at >= created_at),
               error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
               UNIQUE(request_id, endpoint),
               UNIQUE(push_transaction_id),
               UNIQUE(device_id, push_counter),
               CHECK (
                 (endpoint = '/sync/v1/negotiate' AND operation = 'negotiate') OR
                 (endpoint = '/sync/v1/bootstrap' AND operation = 'bootstrap') OR
                 (endpoint = '/sync/v1/push' AND operation = 'push') OR
                 (endpoint = '/sync/v1/pull' AND operation = 'pull') OR
                 (endpoint = '/sync/v1/checkpoint' AND operation = 'checkpoint') OR
                 (endpoint = '/sync/v1/ack' AND operation = 'ack')
               ),
               CHECK (
                 (endpoint = '/sync/v1/push') =
                 (push_transaction_id IS NOT NULL AND push_counter IS NOT NULL)
               ),
               CHECK (
                 (response_status IS NULL AND response_content_type IS NULL
                   AND response_bytes IS NULL AND response_sha256 IS NULL)
                 OR
                 (response_status IS NOT NULL AND response_content_type IS NOT NULL
                   AND response_bytes IS NOT NULL AND response_sha256 IS NOT NULL)
               ),
               CHECK (state != 'pending' OR response_bytes IS NULL),
               CHECK (state NOT IN ('response_received', 'completed') OR response_bytes IS NOT NULL),
               CHECK ((state = 'completed') = (completed_at IS NOT NULL)),
               CHECK ((state = 'quarantined') = (quarantined_at IS NOT NULL))
             );
             CREATE INDEX idx_mobile_direct_sync_request_state_counter
               ON mobile_direct_sync_request_v1(state, local_sequence);
             CREATE UNIQUE INDEX idx_mobile_direct_sync_request_open
               ON mobile_direct_sync_request_v1((1))
               WHERE state IN ('pending', 'response_received');
             CREATE TRIGGER mobile_direct_sync_request_identity_immutable
             BEFORE UPDATE OF request_id, endpoint, operation, purpose_json, purpose_sha256,
               push_transaction_id, push_counter,
               receipt_id, activation_sha256, library_id, device_id,
               authority_generation, purge_generation, key_epoch, sync_spki_sha256,
               request_bytes, request_sha256, request_content_type, created_at
             ON mobile_direct_sync_request_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync request identity is immutable');
             END;
             CREATE TRIGGER mobile_direct_sync_response_immutable
             BEFORE UPDATE OF response_status, response_content_type,
               response_bytes, response_sha256
             ON mobile_direct_sync_request_v1
             WHEN OLD.response_bytes IS NOT NULL AND (
               NEW.response_status IS NOT OLD.response_status
               OR NEW.response_content_type IS NOT OLD.response_content_type
               OR
               NEW.response_bytes IS NOT OLD.response_bytes
               OR NEW.response_sha256 IS NOT OLD.response_sha256
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync response is immutable');
             END;
             CREATE TRIGGER mobile_direct_sync_request_state_monotonic
             BEFORE UPDATE OF state ON mobile_direct_sync_request_v1
             WHEN NOT (
               NEW.state = OLD.state
               OR (OLD.state = 'pending' AND NEW.state IN ('response_received', 'quarantined'))
               OR (OLD.state = 'response_received' AND NEW.state IN ('completed', 'quarantined'))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync request state cannot roll back');
             END;
             CREATE TABLE mobile_direct_sync_push_binding_v1 (
               transaction_id TEXT PRIMARY KEY CHECK (length(transaction_id) = 36),
               request_id TEXT NOT NULL UNIQUE CHECK (length(request_id) = 36),
               push_counter INTEGER NOT NULL UNIQUE CHECK (push_counter > 0),
               request_sha256 TEXT NOT NULL CHECK (length(request_sha256) = 64),
               receipt_id TEXT NOT NULL CHECK (length(receipt_id) = 36),
               activation_sha256 TEXT NOT NULL CHECK (length(activation_sha256) = 64),
               library_id TEXT NOT NULL CHECK (length(library_id) = 36),
               device_id TEXT NOT NULL CHECK (length(device_id) = 36),
               authority_generation INTEGER NOT NULL CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL CHECK (purge_generation >= 0),
               key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
               sync_spki_sha256 BLOB NOT NULL CHECK (length(sync_spki_sha256) = 32),
               state TEXT NOT NULL CHECK (state IN (
                 'sending', 'awaiting_echo', 'acknowledged', 'conflict', 'rejected'
               )),
               created_at INTEGER NOT NULL CHECK (created_at >= 0),
               updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
               terminal_at INTEGER CHECK (terminal_at IS NULL OR terminal_at >= created_at),
               error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
               CHECK ((state IN ('acknowledged', 'conflict', 'rejected')) = (terminal_at IS NOT NULL))
             );
             CREATE TRIGGER mobile_direct_sync_push_binding_identity_immutable
             BEFORE UPDATE OF transaction_id, request_id, push_counter,
               request_sha256, receipt_id, activation_sha256, library_id,
               device_id, authority_generation, purge_generation, key_epoch,
               sync_spki_sha256, created_at
             ON mobile_direct_sync_push_binding_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync push binding identity is immutable');
             END;
             CREATE TRIGGER mobile_direct_sync_push_binding_state_monotonic
             BEFORE UPDATE OF state ON mobile_direct_sync_push_binding_v1
             WHEN NOT (
               NEW.state = OLD.state
               OR (OLD.state = 'sending' AND NEW.state IN ('awaiting_echo', 'conflict', 'rejected'))
               OR (OLD.state = 'awaiting_echo' AND NEW.state IN ('acknowledged', 'conflict'))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync push binding state cannot roll back');
             END;
             CREATE TABLE mobile_bootstrap_checkpoint_v1 (
               checkpoint_id TEXT PRIMARY KEY CHECK (length(checkpoint_id) = 36),
               contract_version TEXT NOT NULL
                 CHECK (contract_version = 'noted.sync-bootstrap.v1'),
               checkpoint_sha256 TEXT NOT NULL CHECK (length(checkpoint_sha256) = 64),
               receipt_id TEXT NOT NULL CHECK (length(receipt_id) = 36),
               activation_sha256 TEXT NOT NULL CHECK (length(activation_sha256) = 64),
               library_id TEXT NOT NULL CHECK (length(library_id) = 36),
               device_id TEXT NOT NULL CHECK (length(device_id) = 36),
               authority_generation INTEGER NOT NULL CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL CHECK (purge_generation >= 0),
               key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
               sync_spki_sha256 BLOB NOT NULL CHECK (length(sync_spki_sha256) = 32),
               start_cursor INTEGER NOT NULL CHECK (start_cursor >= 0),
               high_water_cursor INTEGER NOT NULL CHECK (high_water_cursor >= start_cursor),
               final_page_count INTEGER
                 CHECK (final_page_count IS NULL OR final_page_count BETWEEN 1 AND 64),
               final_commitment_sha256 TEXT
                 CHECK (final_commitment_sha256 IS NULL OR length(final_commitment_sha256) = 64),
               state TEXT NOT NULL DEFAULT 'receiving'
                 CHECK (state IN ('receiving', 'received', 'applied', 'aborted', 'quarantined')),
               created_at INTEGER NOT NULL CHECK (created_at >= 0),
               finalized_at INTEGER CHECK (finalized_at IS NULL OR finalized_at >= created_at),
               applied_at INTEGER CHECK (applied_at IS NULL OR applied_at >= created_at),
               terminal_at INTEGER CHECK (terminal_at IS NULL OR terminal_at >= created_at),
               error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
               CHECK (state != 'receiving' OR final_page_count IS NULL),
               CHECK (state NOT IN ('received', 'applied') OR final_page_count IS NOT NULL),
               CHECK ((final_page_count IS NULL) = (final_commitment_sha256 IS NULL)),
               CHECK ((state = 'applied') = (applied_at IS NOT NULL)),
               CHECK ((state IN ('aborted', 'quarantined')) = (terminal_at IS NOT NULL))
             );
             CREATE UNIQUE INDEX idx_mobile_bootstrap_checkpoint_open
               ON mobile_bootstrap_checkpoint_v1((1))
               WHERE state IN ('receiving', 'received');
             CREATE TABLE mobile_bootstrap_page_v1 (
               checkpoint_id TEXT NOT NULL
                 REFERENCES mobile_bootstrap_checkpoint_v1(checkpoint_id) ON DELETE CASCADE,
               page_index INTEGER NOT NULL CHECK (page_index BETWEEN 0 AND 63),
               checkpoint_sha256 TEXT NOT NULL CHECK (length(checkpoint_sha256) = 64),
               requested_after_record_id TEXT
                 CHECK (requested_after_record_id IS NULL OR length(requested_after_record_id) BETWEEN 1 AND 512),
               next_after_record_id TEXT
                 CHECK (next_after_record_id IS NULL OR length(next_after_record_id) BETWEEN 1 AND 512),
               has_more INTEGER NOT NULL CHECK (has_more IN (0, 1)),
               dependency_sha256 TEXT
                 CHECK (dependency_sha256 IS NULL OR length(dependency_sha256) = 64),
               response_bytes BLOB NOT NULL
                 CHECK (length(response_bytes) BETWEEN 1 AND 4194304),
               response_sha256 TEXT NOT NULL CHECK (length(response_sha256) = 64),
               state TEXT NOT NULL DEFAULT 'received'
                 CHECK (state IN ('received', 'applied', 'quarantined')),
               received_at INTEGER NOT NULL CHECK (received_at >= 0),
               applied_at INTEGER CHECK (applied_at IS NULL OR applied_at >= received_at),
               quarantined_at INTEGER CHECK (quarantined_at IS NULL OR quarantined_at >= received_at),
               error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
               PRIMARY KEY(checkpoint_id, page_index),
               CHECK ((page_index = 0) = (dependency_sha256 IS NULL)),
               CHECK ((page_index = 0) = (requested_after_record_id IS NULL)),
               CHECK (has_more = 0 OR next_after_record_id IS NOT NULL),
               CHECK ((state = 'applied') = (applied_at IS NOT NULL)),
               CHECK ((state = 'quarantined') = (quarantined_at IS NOT NULL))
             );
             CREATE INDEX idx_mobile_bootstrap_page_checkpoint_index
               ON mobile_bootstrap_page_v1(checkpoint_id, page_index);
             CREATE TRIGGER mobile_bootstrap_checkpoint_identity_immutable
             BEFORE UPDATE OF checkpoint_id, contract_version, checkpoint_sha256, receipt_id,
               activation_sha256, library_id, device_id, authority_generation,
               purge_generation, key_epoch, sync_spki_sha256, start_cursor,
               high_water_cursor, created_at
             ON mobile_bootstrap_checkpoint_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile bootstrap checkpoint identity is immutable');
             END;
             CREATE TRIGGER mobile_bootstrap_checkpoint_final_immutable
             BEFORE UPDATE OF final_page_count, final_commitment_sha256
             ON mobile_bootstrap_checkpoint_v1
             WHEN OLD.final_page_count IS NOT NULL AND (
               NEW.final_page_count IS NOT OLD.final_page_count
               OR NEW.final_commitment_sha256 IS NOT OLD.final_commitment_sha256
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile bootstrap final commitment is immutable');
             END;
             CREATE TRIGGER mobile_bootstrap_checkpoint_state_monotonic
             BEFORE UPDATE OF state ON mobile_bootstrap_checkpoint_v1
             WHEN NOT (
               NEW.state = OLD.state
               OR (OLD.state = 'receiving' AND NEW.state IN ('received', 'aborted', 'quarantined'))
               OR (OLD.state = 'received' AND NEW.state IN ('applied', 'aborted', 'quarantined'))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile bootstrap checkpoint state cannot roll back');
             END;
             CREATE TRIGGER mobile_bootstrap_page_identity_immutable
             BEFORE UPDATE OF checkpoint_id, page_index, checkpoint_sha256,
               requested_after_record_id, next_after_record_id, has_more,
               dependency_sha256, response_bytes, response_sha256, received_at
             ON mobile_bootstrap_page_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile bootstrap page identity is immutable');
             END;
             CREATE TRIGGER mobile_bootstrap_page_state_monotonic
             BEFORE UPDATE OF state ON mobile_bootstrap_page_v1
             WHEN NOT (
               NEW.state = OLD.state
               OR (OLD.state = 'received' AND NEW.state IN ('applied', 'quarantined'))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile bootstrap page state cannot roll back');
             END;"#;
const PORTABLE_SCHEMA_V7_DDL: &str = r#"CREATE TABLE mobile_canonical_record_v1 (
               record_id TEXT PRIMARY KEY CHECK (length(record_id) = 36),
               library_id TEXT NOT NULL CHECK (length(library_id) = 36),
               record_kind TEXT NOT NULL CHECK (record_kind IN ('note', 'category', 'folder')),
               accepted_revision INTEGER CHECK (accepted_revision IS NULL OR accepted_revision > 0),
               accepted_version_id TEXT CHECK (accepted_version_id IS NULL OR length(accepted_version_id) = 36),
               accepted_content_hash TEXT CHECK (accepted_content_hash IS NULL OR length(accepted_content_hash) = 64),
               accepted_record_json BLOB CHECK (
                 accepted_record_json IS NULL OR length(accepted_record_json) BETWEEN 1 AND 524288
               ),
               accepted_record_sha256 TEXT CHECK (
                 accepted_record_sha256 IS NULL OR length(accepted_record_sha256) = 64
               ),
               working_revision INTEGER NOT NULL CHECK (working_revision > 0),
               working_version_id TEXT NOT NULL CHECK (length(working_version_id) = 36),
               working_content_hash TEXT NOT NULL CHECK (length(working_content_hash) = 64),
               working_record_json BLOB NOT NULL
                 CHECK (length(working_record_json) BETWEEN 1 AND 524288),
               working_record_sha256 TEXT NOT NULL CHECK (length(working_record_sha256) = 64),
               backfill_provenance TEXT NOT NULL DEFAULT 'native_exact'
                 CHECK (backfill_provenance IN ('native_exact', 'v7_projection_backfill')),
               updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
               CHECK (
                 (accepted_revision IS NULL AND accepted_version_id IS NULL
                   AND accepted_content_hash IS NULL AND accepted_record_json IS NULL
                   AND accepted_record_sha256 IS NULL)
                 OR
                 (accepted_revision IS NOT NULL AND accepted_version_id IS NOT NULL
                   AND accepted_content_hash IS NOT NULL AND accepted_record_json IS NOT NULL
                   AND accepted_record_sha256 IS NOT NULL)
               )
             );
             CREATE INDEX idx_mobile_canonical_record_kind
               ON mobile_canonical_record_v1(record_kind, record_id);
             CREATE TRIGGER mobile_canonical_record_identity_immutable
             BEFORE UPDATE OF record_id, library_id, record_kind
             ON mobile_canonical_record_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile canonical record identity is immutable');
             END;
             CREATE TRIGGER mobile_canonical_accepted_head_monotonic
             BEFORE UPDATE OF accepted_revision, accepted_version_id,
               accepted_content_hash, accepted_record_json, accepted_record_sha256
             ON mobile_canonical_record_v1
             WHEN OLD.accepted_revision IS NOT NULL AND (
               NEW.accepted_revision IS NULL
               OR NEW.accepted_revision < OLD.accepted_revision
               OR (NEW.accepted_revision = OLD.accepted_revision AND (
                 NEW.accepted_version_id IS NOT OLD.accepted_version_id
                 OR NEW.accepted_content_hash IS NOT OLD.accepted_content_hash
                 OR NEW.accepted_record_json IS NOT OLD.accepted_record_json
                 OR NEW.accepted_record_sha256 IS NOT OLD.accepted_record_sha256
               ))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile canonical accepted head cannot roll back or fork');
             END;"#;
const PORTABLE_SCHEMA_V8_DDL: &str = r#"CREATE TABLE mobile_authority_revocation_v1 (
               activation_sha256 TEXT PRIMARY KEY CHECK (length(activation_sha256) = 64),
               contract_version TEXT NOT NULL
                 CHECK (contract_version = 'noted.mobile-authority-revocation.v1'),
               receipt_id TEXT NOT NULL CHECK (length(receipt_id) = 36),
               library_id TEXT NOT NULL CHECK (length(library_id) = 36),
               device_id TEXT NOT NULL CHECK (length(device_id) = 36),
               authority_generation INTEGER NOT NULL CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL CHECK (purge_generation >= 0),
               key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
               sync_spki_sha256 BLOB NOT NULL CHECK (length(sync_spki_sha256) = 32),
               request_id TEXT NOT NULL UNIQUE CHECK (length(request_id) = 36),
               endpoint TEXT NOT NULL CHECK (endpoint IN (
                 '/sync/v1/negotiate', '/sync/v1/bootstrap', '/sync/v1/push',
                 '/sync/v1/pull', '/sync/v1/checkpoint', '/sync/v1/ack'
               )),
               response_status INTEGER NOT NULL CHECK (response_status BETWEEN 100 AND 599),
               evidence_kind TEXT NOT NULL CHECK (evidence_kind IN (
                 'signed_push_receipt', 'authenticated_transport_error'
               )),
               response_bytes BLOB NOT NULL
                 CHECK (length(response_bytes) BETWEEN 1 AND 4194304),
               response_sha256 TEXT NOT NULL CHECK (length(response_sha256) = 64),
               reason TEXT NOT NULL CHECK (reason = 'device_revoked'),
               revoked_at INTEGER NOT NULL CHECK (revoked_at >= 0)
             );
             CREATE TRIGGER mobile_authority_revocation_immutable
             BEFORE UPDATE ON mobile_authority_revocation_v1 BEGIN
               SELECT RAISE(ABORT, 'mobile authority revocation evidence is immutable');
             END;
             DROP TRIGGER mobile_direct_sync_push_binding_state_monotonic;
             CREATE TRIGGER mobile_direct_sync_push_binding_state_monotonic
             BEFORE UPDATE OF state ON mobile_direct_sync_push_binding_v1
             WHEN NOT (
               NEW.state = OLD.state
               OR (OLD.state = 'sending' AND NEW.state IN ('awaiting_echo', 'conflict', 'rejected'))
               OR (OLD.state = 'awaiting_echo' AND NEW.state IN ('acknowledged', 'conflict', 'rejected'))
             ) BEGIN
               SELECT RAISE(ABORT, 'mobile direct-sync push binding state cannot roll back');
             END;"#;
const MOBILE_APPLICATION_ID: i64 = 0x4e4f_5449; // ASCII `NOTI`.
const MOBILE_NOTES_EXPORT_FORMAT: &str = "noted.mobile-notes.export.v1";
const MOBILE_NOTES_EXPORT_VERSION: u32 = 1;
const MAX_MOBILE_NOTES_EXPORT_BYTES: usize = 256 * 1024 * 1024;
const MAX_MOBILE_INBOX_BYTES: usize = 4 * 1024 * 1024;
const MAX_MOBILE_PAIRING_ACTIVATION_BYTES: usize = 256 * 1024;
const MAX_MOBILE_DIRECT_SYNC_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MOBILE_DIRECT_SYNC_PURPOSE_BYTES: usize = 16 * 1024;
const MAX_MOBILE_DIRECT_SYNC_ATTEMPTS: i64 = 100;
const MAX_MOBILE_DIRECT_SYNC_ROWS: i64 = 512;
const MAX_MOBILE_DIRECT_SYNC_OPEN_ROWS: i64 = 1;
const MAX_MOBILE_DIRECT_SYNC_TOTAL_BYTES: i64 = 64 * 1024 * 1024;
const MAX_MOBILE_BOOTSTRAP_PAGES: usize = 64;
const MAX_MOBILE_BOOTSTRAP_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MOBILE_BOOTSTRAP_TOTAL_BYTES: i64 = 32 * 1024 * 1024;
const MAX_MOBILE_BOOTSTRAP_CHECKPOINTS: i64 = 16;
const MAX_MOBILE_BOOTSTRAP_CHANGES: usize = 4096;
const MAX_MOBILE_ELIGIBLE_OUTBOX_GROUPS: usize = 16;
const MAX_MOBILE_TRANSACTION_MEMBERS: usize = 128;
// These ceilings match the direct-sync parser's per-string and aggregate
// string budgets. Enforcing them before an outbox row commits prevents a
// mutation that the phone can persist but can never upload.
const MAX_MOBILE_NOTE_TEXT_BYTES: usize = 256 * 1024;
const MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES: usize = 512 * 1024;
// NRC1 reserves its versioned container header, nonce, AEAD tag, and inner
// record signature inside the 512 KiB per-mutation ciphertext ceiling.
const MAX_MOBILE_CANONICAL_RECORD_BYTES: usize = 524_120;
const MOBILE_CANONICAL_RECORD_CIPHERTEXT_OVERHEAD_BYTES: usize =
    MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES - MAX_MOBILE_CANONICAL_RECORD_BYTES;
// AES-256-GCM adds a 16-byte tag; the v1 ciphertext field also carries a
// 12-byte nonce. Associated data and signatures live outside ciphertext.
const MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES: usize = 28;
const MAX_MOBILE_MUTATION_PAYLOAD_BYTES: usize =
    MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES - MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES;
pub const MOBILE_STORE_LOCKED_ERROR: &str = "mobile_store_locked_protected_data";
// RFC 3339's four-digit year range is also comfortably inside JavaScript's
// Date range, which keeps remote timestamps safe on product surfaces.
const MAX_PORTABLE_TIMESTAMP_MS: i64 = 253_402_300_799_999;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkspaceNote {
    pub record_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
    pub lifecycle_state: String,
    pub needs_filing: bool,
    pub sync_state: String,
    pub conflict_of: Option<String>,
    pub has_open_conflict: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkspaceFolder {
    pub folder_id: String,
    pub name: String,
    pub parent_id: Option<String>,
    /// A logical breadcrumb (for example, `Work / Project`), never a
    /// filesystem path.
    pub path: Option<String>,
    pub note_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkspaceCapabilities {
    pub filing: bool,
    pub undo_filing: bool,
    pub trash: bool,
    pub restore: bool,
    pub conflict_resolution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkspaceSync {
    pub state: String,
    pub pending_count: i64,
    pub last_synced_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkspaceCounts {
    pub inbox: i64,
    pub needs_filing: i64,
    pub trash: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileNotesWorkspace {
    pub notes: Vec<MobileWorkspaceNote>,
    pub folders: Vec<MobileWorkspaceFolder>,
    pub capabilities: MobileWorkspaceCapabilities,
    pub sync: MobileWorkspaceSync,
    pub counts: MobileWorkspaceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileStoreHealth {
    pub storage: String,
    pub sync: String,
}

/// The only pairing state allowed in SQLite. It contains exact public wire
/// messages and opaque native handles, never a private key or decrypted
/// bootstrap. The explicit table mirrors critical bindings so a corrupted
/// JSON checkpoint cannot silently redirect native recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobilePairingCheckpoint {
    pub identity_handle: String,
    pub pending_bootstrap_handle: Option<String>,
    pub client: PairingClientCheckpoint,
    pub updated_at: i64,
}

/// Public, secret-free material required to make native key activation and
/// SQLite adoption one recoverable product transition. The active checkpoint
/// is stored byte-for-byte with these bindings in the same SQLite commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobilePairingActivation {
    pub receipt_id: String,
    pub library_id: String,
    pub device_id: String,
    pub default_scope_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub sync_spki_sha256: Vec<u8>,
    pub record_cipher_suite: String,
    pub granted_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
    pub checkpoint: MobilePairingCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePairingActivationResult {
    pub adopted_note_count: usize,
    pub replayed: bool,
}

/// Recovery-oriented state intended for the runtime to combine with its
/// Keychain inventory. `native_active_pending_finalize` is the only expected
/// crash window after native key activation and before the SQLite commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePairingActivationHealth {
    pub phase: String,
    pub database_finalized: bool,
    pub receipt_id: Option<String>,
    pub library_state: String,
    pub enrollment_state: String,
}

/// Exact response bytes that have already crossed the runtime's authority
/// signature verifier and its durable response journal. The store independently
/// rebinds them to the active activation and original signed push transaction
/// before changing enrollment state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAuthorityRevocationEvidence {
    pub request_id: String,
    pub endpoint: String,
    pub exact_response_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAuthorityRevocation {
    pub activation_sha256: String,
    pub receipt_id: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub request_id: String,
    pub endpoint: String,
    pub response_status: i64,
    pub evidence_kind: String,
    pub response_sha256: String,
    pub revoked_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAuthorityRevocationResult {
    pub revocation: MobileAuthorityRevocation,
    pub retired_outbox_count: usize,
    pub quarantined_request_count: usize,
    pub replayed: bool,
}

/// One complete, still-eligible local transaction group. `payload_bytes` are
/// the exact bytes stored by the portable writer; `payload` is the same JSON
/// decoded for callers that need to build the signed/encrypted wire envelope.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEligibleOutboxMutation {
    pub mutation_id: String,
    pub transaction_id: String,
    pub device_transaction_counter: i64,
    pub transaction_member_index: i64,
    pub transaction_member_count: i64,
    pub library_id: String,
    pub device_id: String,
    pub install_id: String,
    pub scope_id: String,
    pub scope_class: String,
    pub record_id: String,
    pub record_kind: String,
    pub operation: String,
    pub base_revision: i64,
    pub base_version_id: Option<String>,
    pub proposed_revision: i64,
    pub local_revision: i64,
    pub branch_id: String,
    pub version_id: String,
    pub canonical_hash: String,
    pub payload_bytes: Vec<u8>,
    pub payload: serde_json::Value,
    pub state: String,
    pub attempts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEligibleOutboxTransactionGroup {
    pub transaction_id: String,
    /// The v1-v5 local edit-order counter. It is metadata, not the v6 signed
    /// wire counter; superseded local edits can leave gaps in this sequence.
    pub device_transaction_counter: i64,
    pub mutations: Vec<MobileEligibleOutboxMutation>,
}

/// Lossless canonical state for one supported portable record. These are the
/// exact canonical JSON bytes used by record crypto; product projections are
/// deliberately not exposed as encryption input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCanonicalRecord {
    pub record_id: String,
    pub library_id: String,
    pub record_kind: String,
    pub accepted_record_bytes: Option<Vec<u8>>,
    pub accepted_record_sha256: Option<String>,
    pub working_record_bytes: Vec<u8>,
    pub working_record_sha256: String,
    pub backfill_provenance: String,
}

/// A local mutation prepared for the real record-encryption boundary. Unlike
/// `MobileEligibleOutboxMutation::payload_bytes`, `proposed_record_bytes` are
/// a complete, validated `ContextRecordV1` and never the legacy shadow
/// proposal format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCanonicalOutboxMutation {
    pub mutation_id: String,
    pub transaction_id: String,
    pub device_transaction_counter: i64,
    pub transaction_member_index: i64,
    pub transaction_member_count: i64,
    pub library_id: String,
    pub device_id: String,
    pub record_id: String,
    pub record_kind: String,
    pub operation: String,
    pub base_revision: i64,
    pub base_version_id: Option<String>,
    pub proposed_revision: i64,
    pub version_id: String,
    pub proposed_record_bytes: Vec<u8>,
    pub proposed_record_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCanonicalOutboxTransactionGroup {
    pub transaction_id: String,
    pub device_transaction_counter: i64,
    pub mutations: Vec<MobileCanonicalOutboxMutation>,
}

/// Exact already-signed request supplied by the transport orchestrator. Push
/// requests must carry the outbox transaction id and the value returned by
/// `next_direct_sync_push_counter`; non-push requests carry neither. Preparing
/// the row atomically claims the push counter before transport is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDirectSyncRequestDraft {
    pub request_id: String,
    pub endpoint: String,
    pub operation: String,
    pub purpose_json: Vec<u8>,
    pub push_transaction_id: Option<String>,
    pub push_counter: Option<i64>,
    pub signed_request_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileDirectSyncRequest {
    pub local_sequence: i64,
    pub request_id: String,
    pub endpoint: String,
    pub operation: String,
    pub purpose_json: Vec<u8>,
    pub purpose_sha256: String,
    pub push_transaction_id: Option<String>,
    pub push_counter: Option<i64>,
    pub receipt_id: String,
    pub activation_sha256: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub sync_spki_sha256: Vec<u8>,
    pub request_bytes: Vec<u8>,
    pub request_sha256: String,
    pub request_content_type: String,
    pub response_status: Option<i64>,
    pub response_content_type: Option<String>,
    pub response_bytes: Option<Vec<u8>>,
    pub response_sha256: Option<String>,
    pub state: String,
    pub attempts: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_attempt_at: Option<i64>,
    pub response_received_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub quarantined_at: Option<i64>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileDirectSyncPrepareResult {
    pub request: MobileDirectSyncRequest,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileDirectSyncPruneResult {
    pub pruned_completed_count: usize,
    pub remaining_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileDirectSyncPushDisposition {
    AcceptedAwaitingEcho,
    Conflict,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileDirectSyncPushBinding {
    pub transaction_id: String,
    pub request_id: String,
    pub push_counter: i64,
    pub request_sha256: String,
    pub receipt_id: String,
    pub activation_sha256: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub sync_spki_sha256: Vec<u8>,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub terminal_at: Option<i64>,
    pub error_code: Option<String>,
}

/// Opaque signed/encrypted response page. Persistence validates only public
/// ordering, digest, activation, and checkpoint bindings; it never decrypts
/// or parses `response_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBootstrapPageDraft {
    pub checkpoint_id: String,
    pub contract_version: String,
    pub checkpoint_sha256: String,
    pub library_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub page_index: usize,
    pub high_water_cursor: i64,
    pub requested_after_record_id: Option<String>,
    pub next_after_record_id: Option<String>,
    pub has_more: bool,
    pub dependency_sha256: Option<String>,
    pub response_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileBootstrapPage {
    pub checkpoint_id: String,
    pub page_index: i64,
    pub checkpoint_sha256: String,
    pub requested_after_record_id: Option<String>,
    pub next_after_record_id: Option<String>,
    pub has_more: bool,
    pub dependency_sha256: Option<String>,
    pub response_bytes: Vec<u8>,
    pub response_sha256: String,
    pub state: String,
    pub received_at: i64,
    pub applied_at: Option<i64>,
    pub quarantined_at: Option<i64>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileBootstrapCheckpoint {
    pub checkpoint_id: String,
    pub contract_version: String,
    pub checkpoint_sha256: String,
    pub receipt_id: String,
    pub activation_sha256: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub key_epoch: i64,
    pub sync_spki_sha256: Vec<u8>,
    pub start_cursor: i64,
    pub high_water_cursor: i64,
    pub final_page_count: Option<i64>,
    pub final_commitment_sha256: Option<String>,
    pub state: String,
    pub created_at: i64,
    pub finalized_at: Option<i64>,
    pub applied_at: Option<i64>,
    pub terminal_at: Option<i64>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileBootstrapRecovery {
    pub checkpoint: MobileBootstrapCheckpoint,
    pub pages: Vec<MobileBootstrapPage>,
}

/// Decrypted, protocol-validated current-head projections for one committed
/// bootstrap checkpoint. This is not an acceptance-log history: per-head
/// checkpoints may be sparse and cursor publication jumps to the high-water.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBootstrapSnapshot {
    pub checkpoint_sha256: String,
    pub head_batches: Vec<MobileInboxChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileBootstrapStageResult {
    pub recovery: MobileBootstrapRecovery,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileBootstrapApplyResult {
    pub checkpoint_id: String,
    pub final_cursor: i64,
    pub applied_change_count: usize,
    pub applied_record_count: usize,
    pub conflict_count: usize,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileIncomingCategory {
    pub category_id: String,
    pub name: String,
    pub schema: serde_json::Value,
    pub authority: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileIncomingFolder {
    pub folder_id: String,
    pub name: String,
    pub parent_folder_id: Option<String>,
    pub position: i64,
    pub authority: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileIncomingNote {
    pub record_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub accepted_revision: i64,
    pub accepted_version_id: String,
    pub accepted_content_hash: String,
    pub lifecycle_state: String,
    pub trashed_at: Option<i64>,
    pub tombstoned_at: Option<i64>,
    pub folder_id: Option<String>,
    pub authority: String,
    pub scope_id: String,
    pub scope_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileInboxChange {
    pub sequence: i64,
    pub transaction_id: String,
    pub transaction_digest: String,
    pub library_id: String,
    pub source_device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    #[serde(default)]
    pub categories: Vec<MobileIncomingCategory>,
    #[serde(default)]
    pub folders: Vec<MobileIncomingFolder>,
    #[serde(default)]
    pub notes: Vec<MobileIncomingNote>,
}

impl MobileInboxChange {
    /// Digest of the authenticated, decrypted transaction payload. The digest
    /// field itself is excluded so a caller cannot make a self-referential
    /// payload appear valid.
    pub fn computed_transaction_digest(&self) -> String {
        canonical_sha256(&serde_json::json!({
            "sequence": self.sequence,
            "transactionId": self.transaction_id,
            "libraryId": self.library_id,
            "sourceDeviceId": self.source_device_id,
            "authorityGeneration": self.authority_generation,
            "purgeGeneration": self.purge_generation,
            "categories": self.categories,
            "folders": self.folders,
            "notes": self.notes,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileInboxApplyResult {
    pub sequence: i64,
    pub applied_count: usize,
    pub conflict_count: usize,
    pub state: String,
}

/// Records decrypted from one already-authenticated authority transaction.
/// Each member must be the exact canonical JSON plaintext recovered from its
/// encrypted mutation. `transaction_digest` remains the digest of the signed
/// authority transaction and is therefore safe for ordered replay binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileCanonicalPullChange {
    pub sequence: i64,
    pub transaction_id: String,
    pub transaction_digest: String,
    pub library_id: String,
    pub source_device_id: String,
    pub authority_generation: i64,
    pub purge_generation: i64,
    pub record_bytes: Vec<Vec<u8>>,
}

/// Decrypted current heads for an exact, fully staged bootstrap checkpoint.
/// Publishing these records, their UI projections, the checkpoint state, and
/// both cursors is one SQLite transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileCanonicalBootstrapSnapshot {
    pub checkpoint_sha256: String,
    pub record_bytes: Vec<Vec<u8>>,
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
struct ExistingSyncNote {
    title: String,
    body: String,
    accepted_revision: i64,
    accepted_version_id: Option<String>,
    accepted_content_hash: Option<String>,
    working_branch_id: String,
    working_version_id: String,
    pending_mutation_id: String,
    lifecycle_state: String,
    canonical_hash: String,
    folder_id: Option<String>,
    conflict_of: Option<String>,
}

#[derive(Debug)]
struct ConflictSnapshot {
    conflict_id: String,
    record_id: String,
    local_title: String,
    local_body: String,
    local_canonical_hash: String,
    local_created_at: i64,
    local_updated_at: i64,
    local_lifecycle_state: String,
    local_trashed_at: Option<i64>,
    local_tombstoned_at: Option<i64>,
    local_folder_id: Option<String>,
    local_authority: String,
    local_scope: String,
    local_scope_id: String,
    local_scope_class: String,
    local_provenance_json: String,
    accepted_revision: i64,
    accepted_version_id: String,
    accepted_content_hash: String,
    remote_title: String,
    remote_body: String,
    remote_created_at: i64,
    remote_updated_at: i64,
    remote_lifecycle_state: String,
    remote_trashed_at: Option<i64>,
    remote_tombstoned_at: Option<i64>,
    remote_folder_id: Option<String>,
    remote_authority: String,
    remote_scope_id: String,
    remote_scope_class: String,
}

#[derive(Debug)]
struct Mutation<'a> {
    operation: &'a str,
    /// Whether this user action owns the note's title/body fields. Filing,
    /// lifecycle, pairing, and conflict-copy mutations must not project their
    /// normalized SQLite strings back over exact canonical content.
    patch_title_body: bool,
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
    authority: &'a str,
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

#[derive(Debug)]
enum InboxApplyError {
    /// Authenticated bytes are internally inconsistent with deterministic
    /// domain rules and may be quarantined so the ordered stream can advance.
    Semantic(String),
    /// Storage, clock, locking, or other local failures are retryable and must
    /// never consume the authenticated sequence.
    Operational(String),
}

impl InboxApplyError {
    fn semantic(message: impl Into<String>) -> Self {
        Self::Semantic(message.into())
    }

    fn operational(message: impl Into<String>) -> Self {
        Self::Operational(message.into())
    }

    fn into_string(self) -> String {
        match self {
            Self::Semantic(message) | Self::Operational(message) => message,
        }
    }
}

fn inbox_sql_error(context: &str, error: rusqlite::Error) -> InboxApplyError {
    let message = format!("{context}: {error}");
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            InboxApplyError::semantic(message)
        }
        _ => InboxApplyError::operational(message),
    }
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
    path: PathBuf,
    connection: ProtectedConnection,
}

struct ProtectedConnection {
    slot: Mutex<Option<Connection>>,
}

impl ProtectedConnection {
    fn new(connection: Connection) -> Self {
        Self {
            slot: Mutex::new(Some(connection)),
        }
    }

    fn closed() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    fn lock(&self) -> Result<MobileConnectionGuard<'_>, String> {
        let guard = self
            .slot
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        if guard.is_none() {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        Ok(MobileConnectionGuard { guard })
    }
}

struct MobileConnectionGuard<'a> {
    guard: MutexGuard<'a, Option<Connection>>,
}

impl Deref for MobileConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.guard
            .as_ref()
            .expect("mobile connection guard is constructed only when open")
    }
}

impl DerefMut for MobileConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard
            .as_mut()
            .expect("mobile connection guard is constructed only when open")
    }
}

impl MobileStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = open_mobile_connection(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            connection: ProtectedConnection::new(connection),
        })
    }

    /// Construct the managed store without touching the protected path. iOS
    /// uses this state when launching before first unlock or while locked.
    pub fn closed(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            connection: ProtectedConnection::closed(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Canonical replica identity that native signing/HPKE key creation must
    /// bind to before a pairing transcript is constructed.
    pub fn replica_device_id(&self) -> Result<String, String> {
        let connection = self.lock_connection()?;
        Ok(replica_identity(&connection)?.device_id)
    }

    pub fn load_pairing_checkpoint(&self) -> Result<Option<MobilePairingCheckpoint>, String> {
        let connection = self.lock_connection()?;
        load_mobile_pairing_checkpoint(&connection)
    }

    pub fn save_pairing_checkpoint(
        &self,
        checkpoint: &MobilePairingCheckpoint,
    ) -> Result<(), String> {
        validate_mobile_pairing_checkpoint(checkpoint)?;
        if checkpoint.client.state == PairingClientState::Active {
            return Err(
                "active pairing checkpoints must be committed by finalize_pairing_activation"
                    .to_string(),
            );
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let already_finalized: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mobile_pairing_activation_v1 WHERE singleton = 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if already_finalized {
            return Err("a finalized pairing checkpoint cannot be rolled back".to_string());
        }
        let replica = replica_identity(&transaction)?;
        if checkpoint.client.identity.device_id != replica.device_id {
            return Err(
                "pairing checkpoint identity is not bound to the mobile replica".to_string(),
            );
        }
        write_mobile_pairing_checkpoint(&transaction, checkpoint)?;
        transaction.commit().map_err(|error| error.to_string())
    }

    fn lock_connection(&self) -> Result<MobileConnectionGuard<'_>, String> {
        self.connection.lock()
    }

    /// Called by the native protected-data lifecycle adapter. Taking and
    /// dropping the connection under the same mutex serializes the close after
    /// every in-flight operation and makes subsequent commands fail closed.
    pub fn protected_data_became_unavailable(&self) -> Result<(), String> {
        let mut connection = self
            .connection
            .slot
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        connection.take();
        Ok(())
    }

    /// Reopens and re-verifies the store only after iOS reports that protected
    /// data is available again. A failed reopen leaves the store locked.
    pub fn protected_data_became_available(&self) -> Result<(), String> {
        let mut connection = self
            .connection
            .slot
            .lock()
            .map_err(|_| "mobile note store lock was poisoned".to_string())?;
        if connection.is_some() {
            return Ok(());
        }
        let reopened = open_mobile_connection(&self.path)?;
        *connection = Some(reopened);
        Ok(())
    }

    pub fn protected_data_is_available(&self) -> Result<bool, String> {
        self.connection
            .slot
            .lock()
            .map(|connection| connection.is_some())
            .map_err(|_| "mobile note store lock was poisoned".to_string())
    }

    pub fn health(&self) -> Result<MobileStoreHealth, String> {
        let connection = self.lock_connection()?;
        let (library_state, enrollment_state, stored_sync_state): (String, String, String) =
            connection
                .query_row(
                    "SELECT replica.library_state,
                            sync.enrollment_state, sync.sync_state
                     FROM mobile_replica AS replica
                     JOIN mobile_sync_state AS sync ON sync.singleton = 1
                     WHERE replica.singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| error.to_string())?;
        let (pending, has_conflict): (bool, bool) = connection
            .query_row(
                "SELECT
                   EXISTS(
                     SELECT 1 FROM mobile_note_outbox
                     WHERE eligible_for_sync = 1
                   ),
                   EXISTS(
                     SELECT 1 FROM mobile_note_conflicts WHERE state = 'open'
                   )",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let sync = if library_state == "local_staging" {
            "local"
        } else if enrollment_state != "active" {
            "not_enrolled"
        } else if has_conflict {
            "error"
        } else if pending {
            "pending"
        } else {
            match stored_sync_state.as_str() {
                "idle" => "synced",
                "pending" => "pending",
                "syncing" => "syncing",
                "error" | "conflict" | "revoked" => "error",
                _ => "not_enrolled",
            }
        };
        Ok(MobileStoreHealth {
            storage: "ready".to_string(),
            sync: sync.to_string(),
        })
    }
}

fn open_mobile_connection(path: &Path) -> Result<Connection, String> {
    let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;")
        .map_err(|error| error.to_string())?;
    let recovery_path = prepare_mobile_migration_recovery(path, &connection)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;",
        )
        .map_err(|error| error.to_string())?;
    migrate_portable_notes(&mut connection, recovery_path.as_deref())?;
    recover_interrupted_inbox(&connection)?;
    ensure_mobile_search_schema(&mut connection)?;
    verify_mobile_search_schema(&connection)?;

    Ok(connection)
}

struct PairingCheckpointMirrors {
    state: &'static str,
    transcript_digest: Option<Vec<u8>>,
    receipt_id: Option<String>,
    envelope_digest: Option<Vec<u8>>,
}

fn pairing_state_name(state: PairingClientState) -> &'static str {
    match state {
        PairingClientState::Ready => "ready",
        PairingClientState::AwaitingServerHello => "awaiting_server_hello",
        PairingClientState::AwaitingUserConfirmation => "awaiting_user_confirmation",
        PairingClientState::AwaitingBootstrap => "awaiting_bootstrap",
        PairingClientState::BootstrapPrepared => "bootstrap_prepared",
        PairingClientState::AwaitingServerFinish => "awaiting_server_finish",
        PairingClientState::PendingActivation => "pending_activation",
        PairingClientState::Active => "active",
        PairingClientState::CancellationPending => "cancellation_pending",
        PairingClientState::Cancelled => "cancelled",
    }
}

fn pairing_checkpoint_mirrors(
    checkpoint: &MobilePairingCheckpoint,
) -> Result<PairingCheckpointMirrors, String> {
    let server = checkpoint
        .client
        .server_hello_bytes
        .as_deref()
        .map(|bytes| {
            serde_json::from_slice::<ServerHello>(bytes)
                .map_err(|error| format!("decode checkpoint ServerHello: {error}"))
        })
        .transpose()?;
    let bootstrap = checkpoint
        .client
        .bootstrap_bytes
        .as_deref()
        .map(|bytes| {
            serde_json::from_slice::<BootstrapEnvelope>(bytes)
                .map_err(|error| {
                    let is_legacy_fixture = serde_json::from_slice::<serde_json::Value>(bytes)
                        .ok()
                        .and_then(|value| value.as_object().cloned())
                        .is_some_and(|object| {
                            object.contains_key("sealed_bootstrap")
                                && !object.contains_key("metadata")
                        });
                    if is_legacy_fixture {
                        "legacy fixture pairing checkpoint has no authenticated bootstrap metadata; discard the pending native bootstrap and reset pairing before schema v5 migration"
                            .to_string()
                    } else {
                        format!("decode checkpoint BootstrapEnvelope: {error}")
                    }
                })
        })
        .transpose()?;
    if let (Some(server), Some(bootstrap)) = (&server, &bootstrap) {
        if bootstrap.receipt_id != server.receipt.receipt_id {
            return Err(
                "pairing checkpoint bootstrap receipt does not match transcript".to_string(),
            );
        }
    }
    Ok(PairingCheckpointMirrors {
        state: pairing_state_name(checkpoint.client.state),
        transcript_digest: server
            .as_ref()
            .map(|value| value.receipt.transcript_digest.clone()),
        receipt_id: server
            .as_ref()
            .map(|value| value.receipt.receipt_id.clone()),
        envelope_digest: bootstrap.map(|value| value.envelope_digest),
    })
}

fn write_mobile_pairing_checkpoint(
    connection: &Connection,
    checkpoint: &MobilePairingCheckpoint,
) -> Result<(), String> {
    let mirrored = pairing_checkpoint_mirrors(checkpoint)?;
    let checkpoint_json = serde_json::to_string(&checkpoint.client)
        .map_err(|error| format!("serialize mobile pairing checkpoint: {error}"))?;
    let decision = checkpoint.client.user_decision.map(i64::from);
    connection
        .execute(
            "INSERT INTO mobile_pairing_checkpoint_v1 (
               singleton, fixture_class, device_id, identity_handle,
               pending_bootstrap_handle, state, invitation_bytes,
               client_hello_bytes, server_hello_bytes, bootstrap_bytes,
               client_finish_bytes, server_finish_bytes, transcript_digest,
               receipt_id, envelope_digest, user_decision, checkpoint_json,
               updated_at
             ) VALUES (
               1, 'sanitized_fixture', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
               ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )
             ON CONFLICT(singleton) DO UPDATE SET
               fixture_class = excluded.fixture_class,
               device_id = excluded.device_id,
               identity_handle = excluded.identity_handle,
               pending_bootstrap_handle = excluded.pending_bootstrap_handle,
               state = excluded.state,
               invitation_bytes = excluded.invitation_bytes,
               client_hello_bytes = excluded.client_hello_bytes,
               server_hello_bytes = excluded.server_hello_bytes,
               bootstrap_bytes = excluded.bootstrap_bytes,
               client_finish_bytes = excluded.client_finish_bytes,
               server_finish_bytes = excluded.server_finish_bytes,
               transcript_digest = excluded.transcript_digest,
               receipt_id = excluded.receipt_id,
               envelope_digest = excluded.envelope_digest,
               user_decision = excluded.user_decision,
               checkpoint_json = excluded.checkpoint_json,
               updated_at = excluded.updated_at",
            params![
                checkpoint.client.identity.device_id,
                checkpoint.identity_handle,
                checkpoint.pending_bootstrap_handle,
                mirrored.state,
                checkpoint.client.invitation_bytes,
                checkpoint.client.client_hello_bytes,
                checkpoint.client.server_hello_bytes,
                checkpoint.client.bootstrap_bytes,
                checkpoint.client.client_finish_bytes,
                checkpoint.client.server_finish_bytes,
                mirrored.transcript_digest,
                mirrored.receipt_id,
                mirrored.envelope_digest,
                decision,
                checkpoint_json,
                checkpoint.updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate_mobile_pairing_checkpoint(checkpoint: &MobilePairingCheckpoint) -> Result<(), String> {
    if checkpoint.client.version != 1
        || checkpoint.client.config.environment != Environment::Development
        || checkpoint.client.config.library_data_class != LibraryDataClass::SanitizedFixture
    {
        return Err("only sanitized fixture pairing checkpoints are accepted".to_string());
    }
    if !is_uuid(&checkpoint.identity_handle)
        || checkpoint
            .pending_bootstrap_handle
            .as_deref()
            .is_some_and(|handle| !is_uuid(handle))
        || !is_uuid_v7(&checkpoint.client.identity.device_id)
        || checkpoint.updated_at < 0
    {
        return Err("mobile pairing checkpoint contains an invalid public identifier".to_string());
    }
    for bytes in [
        Some(checkpoint.client.invitation_bytes.as_slice()),
        checkpoint.client.client_hello_bytes.as_deref(),
        checkpoint.client.server_hello_bytes.as_deref(),
        checkpoint.client.bootstrap_bytes.as_deref(),
        checkpoint.client.client_finish_bytes.as_deref(),
        checkpoint.client.server_finish_bytes.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if bytes.is_empty() || bytes.len() > MAX_PAIRING_MESSAGE_BYTES {
            return Err("mobile pairing checkpoint contains invalid wire bytes".to_string());
        }
    }
    let mirrors = pairing_checkpoint_mirrors(checkpoint)?;
    let state_requires_pending_handle = matches!(
        checkpoint.client.state,
        PairingClientState::AwaitingServerFinish | PairingClientState::PendingActivation
    );
    if checkpoint.pending_bootstrap_handle.is_some() != state_requires_pending_handle {
        return Err(
            "mobile pairing checkpoint pending handle does not match its durable state".to_string(),
        );
    }
    if checkpoint.pending_bootstrap_handle.is_some()
        && (mirrors.receipt_id.is_none() || mirrors.envelope_digest.is_none())
    {
        return Err(
            "pending native bootstrap handle is missing its public receipt binding".to_string(),
        );
    }
    Ok(())
}

fn verify_mobile_pairing_checkpoint_schema(connection: &Connection) -> Result<(), String> {
    let all_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mobile_pairing_checkpoint_v1')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let required_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mobile_pairing_checkpoint_v1')
             WHERE name IN (
               'singleton', 'fixture_class', 'device_id', 'identity_handle',
               'pending_bootstrap_handle', 'state', 'invitation_bytes',
               'client_hello_bytes', 'server_hello_bytes', 'bootstrap_bytes',
               'client_finish_bytes', 'server_finish_bytes', 'transcript_digest',
               'receipt_id', 'envelope_digest', 'user_decision', 'checkpoint_json',
               'updated_at'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if all_columns != 18 || required_columns != 18 {
        return Err("mobile pairing checkpoint schema is incomplete".to_string());
    }
    if let Some(checkpoint) = load_mobile_pairing_checkpoint(connection)? {
        validate_mobile_pairing_checkpoint(&checkpoint)?;
        if checkpoint.client.identity.device_id != replica_identity(connection)?.device_id {
            return Err(
                "mobile pairing checkpoint identity is not bound to the mobile replica".to_string(),
            );
        }
    }
    Ok(())
}

fn verify_mobile_pairing_activation_schema(connection: &Connection) -> Result<(), String> {
    let all_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mobile_pairing_activation_v1')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let required_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('mobile_pairing_activation_v1')
             WHERE name IN (
               'singleton', 'fixture_class', 'receipt_id', 'library_id', 'device_id',
               'default_scope_id', 'authority_generation', 'purge_generation', 'key_epoch',
               'sync_spki_sha256', 'record_cipher_suite', 'granted_scopes_json',
               'capabilities_json', 'activation_json', 'activation_sha256',
               'adopted_note_count', 'finalized_at'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if all_columns != 17 || required_columns != 17 {
        return Err("mobile pairing activation schema is incomplete".to_string());
    }
    let checkpoint = load_mobile_pairing_checkpoint(connection)?;
    let Some(stored) = load_mobile_pairing_activation(connection)? else {
        if checkpoint
            .as_ref()
            .is_some_and(|value| value.client.state == PairingClientState::Active)
        {
            return Err(
                "SQLite cannot report Active without an atomic pairing activation record"
                    .to_string(),
            );
        }
        let identity = replica_identity(connection)?;
        let enrollment_state: String = connection
            .query_row(
                "SELECT enrollment_state FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if identity.library_state != "local_staging" || enrollment_state != "not_enrolled" {
            return Err(
                "unfinalized v5 pairing state must remain local_staging and not_enrolled"
                    .to_string(),
            );
        }
        return Ok(());
    };
    let identity = replica_identity(connection)?;
    let sync: (String, i64, i64) = connection
        .query_row(
            "SELECT enrollment_state, authority_generation, purge_generation
             FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    let revocation = if mobile_authority_revocation_schema_exists(connection)? {
        load_mobile_authority_revocation_by_activation(connection, &stored.activation_sha256)?
    } else {
        None
    };
    match sync.0.as_str() {
        "active" => {
            if checkpoint.as_ref() != Some(&stored.activation.checkpoint) {
                return Err(
                    "finalized activation does not match the exact Active checkpoint".to_string(),
                );
            }
            if revocation.is_some() {
                return Err(
                    "an active mobile activation has durable revocation evidence".to_string(),
                );
            }
        }
        "revoked" => {
            let revoked = revocation.as_ref().ok_or_else(|| {
                "revoked mobile enrollment is missing durable authority evidence".to_string()
            })?;
            if revoked.public.receipt_id != stored.activation.receipt_id
                || revoked.public.library_id != stored.activation.library_id
                || revoked.public.device_id != stored.activation.device_id
                || revoked.public.authority_generation != stored.activation.authority_generation
                || revoked.public.purge_generation != stored.activation.purge_generation
                || revoked.public.key_epoch != stored.activation.key_epoch
                || revoked.sync_spki_sha256 != stored.activation.sync_spki_sha256
            {
                return Err(
                    "durable authority revocation does not match the finalized activation"
                        .to_string(),
                );
            }
            if let Some(pending) = checkpoint.as_ref() {
                if pending != &stored.activation.checkpoint {
                    let invitation: Invitation =
                        serde_json::from_slice(&pending.client.invitation_bytes)
                            .map_err(|error| format!("decode re-enrollment Invitation: {error}"))?;
                    if pending.client.state == PairingClientState::Active
                        || pending.client.identity.device_id != stored.activation.device_id
                        || invitation.library_id != stored.activation.library_id
                        || i64::try_from(invitation.authority_generation).ok()
                            <= Some(stored.activation.authority_generation)
                    {
                        return Err("revoked enrollment has an invalid re-enrollment checkpoint"
                            .to_string());
                    }
                }
            }
        }
        _ => {
            return Err(
                "a finalized mobile activation must be either active or durably revoked"
                    .to_string(),
            )
        }
    }
    if identity.library_state != "paired"
        || identity.library_id != stored.activation.library_id
        || identity.device_id != stored.activation.device_id
        || identity.default_scope_id != stored.activation.default_scope_id
        || sync.1 != stored.activation.authority_generation
        || sync.2 != stored.activation.purge_generation
    {
        return Err(
            "finalized pairing activation is not atomically reflected by replica and enrollment state"
                .to_string(),
        );
    }
    let wrong_default_scope_class: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM mobile_notes
                WHERE scope_id = ?1 AND scope_class != 'unknown')
             + (SELECT COUNT(*) FROM mobile_note_outbox
                WHERE eligible_for_sync = 1 AND scope_id = ?1 AND scope_class != 'unknown')",
            [&stored.activation.default_scope_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if wrong_default_scope_class != 0 {
        return Err("paired default scope is not classified as unknown".to_string());
    }
    if stored.finalized_at < stored.activation.checkpoint.updated_at {
        return Err(
            "pairing activation finalization time predates its Active checkpoint".to_string(),
        );
    }
    Ok(())
}

fn load_mobile_pairing_checkpoint(
    connection: &Connection,
) -> Result<Option<MobilePairingCheckpoint>, String> {
    struct Stored {
        device_id: String,
        identity_handle: String,
        pending_bootstrap_handle: Option<String>,
        state: String,
        invitation_bytes: Vec<u8>,
        client_hello_bytes: Option<Vec<u8>>,
        server_hello_bytes: Option<Vec<u8>>,
        bootstrap_bytes: Option<Vec<u8>>,
        client_finish_bytes: Option<Vec<u8>>,
        server_finish_bytes: Option<Vec<u8>>,
        transcript_digest: Option<Vec<u8>>,
        receipt_id: Option<String>,
        envelope_digest: Option<Vec<u8>>,
        user_decision: Option<i64>,
        checkpoint_json: String,
        updated_at: i64,
    }
    let stored = connection
        .query_row(
            "SELECT device_id, identity_handle, pending_bootstrap_handle, state,
                    invitation_bytes, client_hello_bytes, server_hello_bytes,
                    bootstrap_bytes, client_finish_bytes, server_finish_bytes,
                    transcript_digest, receipt_id, envelope_digest, user_decision,
                    checkpoint_json, updated_at
             FROM mobile_pairing_checkpoint_v1 WHERE singleton = 1",
            [],
            |row| {
                Ok(Stored {
                    device_id: row.get(0)?,
                    identity_handle: row.get(1)?,
                    pending_bootstrap_handle: row.get(2)?,
                    state: row.get(3)?,
                    invitation_bytes: row.get(4)?,
                    client_hello_bytes: row.get(5)?,
                    server_hello_bytes: row.get(6)?,
                    bootstrap_bytes: row.get(7)?,
                    client_finish_bytes: row.get(8)?,
                    server_finish_bytes: row.get(9)?,
                    transcript_digest: row.get(10)?,
                    receipt_id: row.get(11)?,
                    envelope_digest: row.get(12)?,
                    user_decision: row.get(13)?,
                    checkpoint_json: row.get(14)?,
                    updated_at: row.get(15)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let client: PairingClientCheckpoint = serde_json::from_str(&stored.checkpoint_json)
        .map_err(|error| format!("decode mobile pairing checkpoint: {error}"))?;
    let checkpoint = MobilePairingCheckpoint {
        identity_handle: stored.identity_handle,
        pending_bootstrap_handle: stored.pending_bootstrap_handle,
        client,
        updated_at: stored.updated_at,
    };
    validate_mobile_pairing_checkpoint(&checkpoint)?;
    let mirrors = pairing_checkpoint_mirrors(&checkpoint)?;
    let replica_device_id = replica_identity(connection)?.device_id;
    if stored.device_id != replica_device_id
        || stored.device_id != checkpoint.client.identity.device_id
        || stored.state != mirrors.state
        || stored.invitation_bytes != checkpoint.client.invitation_bytes
        || stored.client_hello_bytes != checkpoint.client.client_hello_bytes
        || stored.server_hello_bytes != checkpoint.client.server_hello_bytes
        || stored.bootstrap_bytes != checkpoint.client.bootstrap_bytes
        || stored.client_finish_bytes != checkpoint.client.client_finish_bytes
        || stored.server_finish_bytes != checkpoint.client.server_finish_bytes
        || stored.transcript_digest != mirrors.transcript_digest
        || stored.receipt_id != mirrors.receipt_id
        || stored.envelope_digest != mirrors.envelope_digest
        || stored.user_decision != checkpoint.client.user_decision.map(i64::from)
    {
        return Err("mobile pairing checkpoint mirror mismatch".to_string());
    }
    Ok(Some(checkpoint))
}

struct StoredMobilePairingActivation {
    activation: MobilePairingActivation,
    activation_json: String,
    activation_sha256: String,
    adopted_note_count: usize,
    finalized_at: i64,
}

fn validate_mobile_pairing_activation(activation: &MobilePairingActivation) -> Result<(), String> {
    validate_mobile_pairing_checkpoint(&activation.checkpoint)?;
    if activation.checkpoint.client.state != PairingClientState::Active
        || activation.checkpoint.pending_bootstrap_handle.is_some()
    {
        return Err("pairing activation requires an exact Active checkpoint".to_string());
    }
    if !is_uuid_v7(&activation.receipt_id)
        || !is_uuid_v7(&activation.library_id)
        || !is_uuid_v7(&activation.device_id)
        || !is_uuid_v7(&activation.default_scope_id)
        || activation.authority_generation <= 0
        || activation.purge_generation < 0
        || activation.key_epoch <= 0
        || activation.sync_spki_sha256.len() != 32
    {
        return Err("mobile pairing activation contains an invalid public binding".to_string());
    }
    if activation.granted_scopes != fixture_record_scopes()
        || activation.capabilities != fixture_record_capabilities()
        || activation.record_cipher_suite != RECORD_CIPHER_SUITE
    {
        return Err(
            "mobile pairing activation requires the exact fixture scope, capability, and cipher suite"
                .to_string(),
        );
    }
    let client = &activation.checkpoint.client;
    if client.config.environment != Environment::Development
        || client.config.library_data_class != LibraryDataClass::SanitizedFixture
        || client.config.requested_scopes != activation.granted_scopes
        || client.config.capabilities != activation.capabilities
        || client.identity.device_id != activation.device_id
        || client.user_decision != Some(true)
    {
        return Err("mobile pairing activation is not bound to its fixture client".to_string());
    }
    let invitation: Invitation = serde_json::from_slice(&client.invitation_bytes)
        .map_err(|error| format!("decode activation Invitation: {error}"))?;
    let server_hello: ServerHello = serde_json::from_slice(
        client
            .server_hello_bytes
            .as_deref()
            .ok_or_else(|| "activation checkpoint is missing ServerHello".to_string())?,
    )
    .map_err(|error| format!("decode activation ServerHello: {error}"))?;
    let bootstrap: BootstrapEnvelope = serde_json::from_slice(
        client
            .bootstrap_bytes
            .as_deref()
            .ok_or_else(|| "activation checkpoint is missing BootstrapEnvelope".to_string())?,
    )
    .map_err(|error| format!("decode activation BootstrapEnvelope: {error}"))?;
    let server_finish: ServerFinish = serde_json::from_slice(
        client
            .server_finish_bytes
            .as_deref()
            .ok_or_else(|| "activation checkpoint is missing ServerFinish".to_string())?,
    )
    .map_err(|error| format!("decode activation ServerFinish: {error}"))?;
    let client_activation = client
        .activation
        .as_ref()
        .ok_or_else(|| "Active checkpoint is missing its public activation".to_string())?;
    if client_activation.activated_at_ms < 0
        || client_activation.activated_at_ms > MAX_PORTABLE_TIMESTAMP_MS
        || activation.checkpoint.updated_at < client_activation.activated_at_ms
        || activation.checkpoint.updated_at > MAX_PORTABLE_TIMESTAMP_MS
    {
        return Err("mobile pairing activation contains an invalid timestamp".to_string());
    }
    let receipt = &client_activation.receipt;
    validate_bootstrap(&bootstrap, receipt)
        .map_err(|error| format!("invalid authenticated bootstrap metadata: {error}"))?;
    if receipt.protocol != PAIRING_PROTOCOL
        || receipt.suite != PAIRING_SUITE
        || receipt.mac_role != PairingRole::MacAuthority
        || receipt.client_role != PairingRole::IphoneCompanion
        || receipt.client_signing_key_fingerprint
            != Sha256::digest(&client.identity.signing_public_key).to_vec()
        || receipt.client_hpke_key_fingerprint
            != Sha256::digest(&client.identity.hpke_public_key).to_vec()
        || receipt.mac_signing_key_fingerprint
            != Sha256::digest(&invitation.mac_pairing_signing_public_key).to_vec()
        || receipt.mac_hpke_key_fingerprint
            != Sha256::digest(&invitation.mac_pairing_hpke_public_key).to_vec()
        || invitation.protocol != PAIRING_PROTOCOL
        || invitation.suite != PAIRING_SUITE
        || invitation.authority_role != PairingRole::MacAuthority
        || invitation.intended_client_role != PairingRole::IphoneCompanion
        || invitation.scope_ceiling != activation.granted_scopes
        || server_hello.protocol != PAIRING_PROTOCOL
        || server_hello.suite != PAIRING_SUITE
        || server_hello.sender_role != PairingRole::MacAuthority
        || server_hello.recipient_role != PairingRole::IphoneCompanion
        || server_finish.protocol != PAIRING_PROTOCOL
        || server_finish.suite != PAIRING_SUITE
        || server_finish.sender_role != PairingRole::MacAuthority
        || server_finish.recipient_role != PairingRole::IphoneCompanion
        || server_hello.receipt != *receipt
        || server_finish.receipt != *receipt
        || server_finish.activated_at_ms != client_activation.activated_at_ms
        || bootstrap.receipt_id != receipt.receipt_id
        || invitation.invitation_id != receipt.invitation_id
        || invitation.library_id != receipt.library_id
        || invitation.authority_generation != receipt.authority_generation
        || invitation.environment != Environment::Development
        || invitation.library_data_class != LibraryDataClass::SanitizedFixture
    {
        return Err("mobile pairing activation transcript bindings do not match".to_string());
    }
    let metadata = &bootstrap.metadata;
    let authority_generation = i64::try_from(receipt.authority_generation)
        .map_err(|_| "pairing authority generation exceeds SQLite range".to_string())?;
    let purge_generation = i64::try_from(metadata.purge_generation)
        .map_err(|_| "pairing purge generation exceeds SQLite range".to_string())?;
    let key_epoch = i64::try_from(metadata.key_epoch)
        .map_err(|_| "pairing key epoch exceeds SQLite range".to_string())?;
    if receipt.receipt_id != activation.receipt_id
        || receipt.library_id != activation.library_id
        || receipt.device_id != activation.device_id
        || receipt.environment != Environment::Development
        || receipt.granted_scopes != activation.granted_scopes
        || receipt.capabilities != activation.capabilities
        || authority_generation != activation.authority_generation
        || metadata.environment != Environment::Development
        || metadata.library_data_class != LibraryDataClass::SanitizedFixture
        || metadata.sync_protocol_version != BOOTSTRAP_SYNC_PROTOCOL_VERSION
        || metadata.receipt_id != activation.receipt_id
        || metadata.library_id != activation.library_id
        || metadata.device_id != activation.device_id
        || metadata.default_scope_id != activation.default_scope_id
        || metadata.default_scope_class != ScopeClass::Unknown
        || purge_generation != activation.purge_generation
        || key_epoch != activation.key_epoch
        || metadata.durable_sync_spki_sha256 != activation.sync_spki_sha256
        || metadata.granted_scopes != activation.granted_scopes
        || metadata.capabilities != activation.capabilities
        || metadata.record_cipher_suite != activation.record_cipher_suite
        || metadata.transcript_digest != receipt.transcript_digest
    {
        return Err(
            "mobile pairing activation does not match authenticated bootstrap metadata".to_string(),
        );
    }
    if client.confirmation.as_ref().is_none_or(|confirmation| {
        confirmation.receipt_id != activation.receipt_id
            || confirmation.granted_scopes != activation.granted_scopes
    }) {
        return Err("mobile pairing activation is missing the exact user confirmation".to_string());
    }
    Ok(())
}

fn serialized_mobile_pairing_activation(
    activation: &MobilePairingActivation,
) -> Result<(String, String, String, String), String> {
    let activation_json = serde_json::to_string(activation).map_err(|error| error.to_string())?;
    if activation_json.is_empty() || activation_json.len() > MAX_MOBILE_PAIRING_ACTIVATION_BYTES {
        return Err("mobile pairing activation exceeds its durable size limit".to_string());
    }
    let value = serde_json::to_value(activation).map_err(|error| error.to_string())?;
    let digest = canonical_sha256(&value);
    let scopes =
        serde_json::to_string(&activation.granted_scopes).map_err(|error| error.to_string())?;
    let capabilities =
        serde_json::to_string(&activation.capabilities).map_err(|error| error.to_string())?;
    Ok((activation_json, digest, scopes, capabilities))
}

fn load_mobile_pairing_activation(
    connection: &Connection,
) -> Result<Option<StoredMobilePairingActivation>, String> {
    type StoredRow = (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        Vec<u8>,
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
    );
    let stored: Option<StoredRow> = connection
        .query_row(
            "SELECT fixture_class, receipt_id, library_id, device_id, default_scope_id,
                    authority_generation, purge_generation, key_epoch, sync_spki_sha256,
                    record_cipher_suite, granted_scopes_json, capabilities_json,
                    activation_json, activation_sha256, adopted_note_count, finalized_at
             FROM mobile_pairing_activation_v1 WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let activation: MobilePairingActivation = serde_json::from_str(&stored.12)
        .map_err(|error| format!("decode mobile pairing activation: {error}"))?;
    validate_mobile_pairing_activation(&activation)?;
    let (activation_json, digest, scopes, capabilities) =
        serialized_mobile_pairing_activation(&activation)?;
    if stored.0 != "sanitized_fixture"
        || stored.1 != activation.receipt_id
        || stored.2 != activation.library_id
        || stored.3 != activation.device_id
        || stored.4 != activation.default_scope_id
        || stored.5 != activation.authority_generation
        || stored.6 != activation.purge_generation
        || stored.7 != activation.key_epoch
        || stored.8 != activation.sync_spki_sha256
        || stored.9 != activation.record_cipher_suite
        || stored.10 != scopes
        || stored.11 != capabilities
        || stored.12 != activation_json
        || stored.13 != digest
        || stored.14 < 0
        || stored.15 < 0
    {
        return Err("mobile pairing activation mirror mismatch".to_string());
    }
    Ok(Some(StoredMobilePairingActivation {
        activation,
        activation_json,
        activation_sha256: digest,
        adopted_note_count: usize::try_from(stored.14)
            .map_err(|_| "adopted note count exceeds platform range".to_string())?,
        finalized_at: stored.15,
    }))
}

#[derive(Debug, Clone)]
struct MobileDirectSyncBinding {
    receipt_id: String,
    activation_sha256: String,
    library_id: String,
    device_id: String,
    authority_generation: i64,
    purge_generation: i64,
    key_epoch: i64,
    sync_spki_sha256: Vec<u8>,
}

struct StoredMobileAuthorityRevocation {
    public: MobileAuthorityRevocation,
    sync_spki_sha256: Vec<u8>,
    response_bytes: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MobileDirectSyncErrorBody {
    error: MobileDirectSyncErrorCode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MobileDirectSyncErrorCode {
    code: String,
}

struct BoundMobileAuthorityRevocationEvidence {
    request: MobileDirectSyncRequest,
    evidence_kind: &'static str,
}

fn exact_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn mobile_authority_revocation_schema_exists(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'mobile_authority_revocation_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn durable_direct_sync_binding(connection: &Connection) -> Result<MobileDirectSyncBinding, String> {
    let stored = load_mobile_pairing_activation(connection)?
        .ok_or_else(|| "direct sync requires a finalized pairing activation".to_string())?;
    Ok(MobileDirectSyncBinding {
        receipt_id: stored.activation.receipt_id,
        activation_sha256: stored.activation_sha256,
        library_id: stored.activation.library_id,
        device_id: stored.activation.device_id,
        authority_generation: stored.activation.authority_generation,
        purge_generation: stored.activation.purge_generation,
        key_epoch: stored.activation.key_epoch,
        sync_spki_sha256: stored.activation.sync_spki_sha256,
    })
}

fn active_direct_sync_binding(connection: &Connection) -> Result<MobileDirectSyncBinding, String> {
    let binding = durable_direct_sync_binding(connection)?;
    verify_mobile_pairing_activation_schema(connection)?;
    let enrollment_state: String = connection
        .query_row(
            "SELECT enrollment_state FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if enrollment_state != "active" {
        return Err("direct sync is disabled because mobile enrollment is not active".to_string());
    }
    Ok(binding)
}

fn direct_sync_binding_for_activation_sha(
    connection: &Connection,
    activation_sha256: &str,
) -> Result<MobileDirectSyncBinding, String> {
    let current = durable_direct_sync_binding(connection)?;
    if current.activation_sha256 == activation_sha256 {
        return Ok(current);
    }
    if !mobile_authority_revocation_schema_exists(connection)? {
        return Err("direct-sync history is bound to an unknown activation".to_string());
    }
    let revoked = load_mobile_authority_revocation_by_activation(connection, activation_sha256)?
        .ok_or_else(|| "direct-sync history is bound to an unknown activation".to_string())?;
    Ok(MobileDirectSyncBinding {
        receipt_id: revoked.public.receipt_id,
        activation_sha256: revoked.public.activation_sha256,
        library_id: revoked.public.library_id,
        device_id: revoked.public.device_id,
        authority_generation: revoked.public.authority_generation,
        purge_generation: revoked.public.purge_generation,
        key_epoch: revoked.public.key_epoch,
        sync_spki_sha256: revoked.sync_spki_sha256,
    })
}

fn load_mobile_authority_revocation_by_request(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<StoredMobileAuthorityRevocation>, String> {
    load_mobile_authority_revocation(connection, "request_id", request_id)
}

fn load_mobile_authority_revocation_by_activation(
    connection: &Connection,
    activation_sha256: &str,
) -> Result<Option<StoredMobileAuthorityRevocation>, String> {
    load_mobile_authority_revocation(connection, "activation_sha256", activation_sha256)
}

fn load_mobile_authority_revocation(
    connection: &Connection,
    selector: &str,
    value: &str,
) -> Result<Option<StoredMobileAuthorityRevocation>, String> {
    if !matches!(selector, "request_id" | "activation_sha256") {
        return Err("invalid authority revocation selector".to_string());
    }
    let query = format!(
        "SELECT activation_sha256, receipt_id, library_id, device_id,
                authority_generation, purge_generation, key_epoch,
                sync_spki_sha256, request_id, endpoint, response_status,
                evidence_kind, response_bytes, response_sha256, revoked_at
         FROM mobile_authority_revocation_v1 WHERE {selector} = ?1"
    );
    let stored = connection
        .query_row(&query, [value], |row| {
            Ok(StoredMobileAuthorityRevocation {
                public: MobileAuthorityRevocation {
                    activation_sha256: row.get(0)?,
                    receipt_id: row.get(1)?,
                    library_id: row.get(2)?,
                    device_id: row.get(3)?,
                    authority_generation: row.get(4)?,
                    purge_generation: row.get(5)?,
                    key_epoch: row.get(6)?,
                    request_id: row.get(8)?,
                    endpoint: row.get(9)?,
                    response_status: row.get(10)?,
                    evidence_kind: row.get(11)?,
                    response_sha256: row.get(13)?,
                    revoked_at: row.get(14)?,
                },
                sync_spki_sha256: row.get(7)?,
                response_bytes: row.get(12)?,
            })
        })
        .optional()
        .map_err(|error| error.to_string())?;
    stored
        .map(validate_stored_mobile_authority_revocation)
        .transpose()
}

fn validate_stored_mobile_authority_revocation(
    stored: StoredMobileAuthorityRevocation,
) -> Result<StoredMobileAuthorityRevocation, String> {
    let revocation = &stored.public;
    if !is_sha256(&revocation.activation_sha256)
        || !is_uuid_v7(&revocation.receipt_id)
        || !is_uuid_v7(&revocation.library_id)
        || !is_uuid_v7(&revocation.device_id)
        || revocation.authority_generation <= 0
        || revocation.purge_generation < 0
        || revocation.key_epoch <= 0
        || stored.sync_spki_sha256.len() != 32
        || !is_uuid_v7(&revocation.request_id)
        || !valid_direct_sync_endpoint(&revocation.endpoint)
        || !(100..=599).contains(&revocation.response_status)
        || !is_sha256(&revocation.response_sha256)
        || exact_sha256(&stored.response_bytes) != revocation.response_sha256
        || revocation.revoked_at < 0
    {
        return Err("stored mobile authority revocation evidence is invalid".to_string());
    }
    match revocation.evidence_kind.as_str() {
        "signed_push_receipt" => {
            let signed: SignedSyncResponse<PushResponse> =
                serde_json::from_slice(&stored.response_bytes).map_err(|error| {
                    format!("decode stored authority revocation response: {error}")
                })?;
            if revocation.endpoint != "/sync/v1/push"
                || revocation.response_status != 200
                || signed.protocol_version != SYNC_PROTOCOL_VERSION
                || signed.request_id != revocation.request_id
                || signed.library_id != revocation.library_id
                || signed.device_id != revocation.device_id
                || i64::try_from(signed.authority_generation).ok()
                    != Some(revocation.authority_generation)
                || signed.payload.receipt.library_id != revocation.library_id
                || signed.payload.receipt.device_id != revocation.device_id
                || i64::try_from(signed.payload.receipt.authority_generation).ok()
                    != Some(revocation.authority_generation)
                || i64::try_from(signed.payload.receipt.purge_generation).ok()
                    != Some(revocation.purge_generation)
                || !matches!(
                    signed.payload.receipt.disposition,
                    ReceiptDisposition::Rejected {
                        code: TerminalRejection::DeviceRevoked
                    }
                )
                || signed.signature.len() != 64
            {
                return Err("stored signed authority revocation evidence is invalid".to_string());
            }
        }
        "authenticated_transport_error" => {
            let parsed: MobileDirectSyncErrorBody = serde_json::from_slice(&stored.response_bytes)
                .map_err(|error| {
                    format!("decode stored authority revocation error response: {error}")
                })?;
            if !(400..=599).contains(&revocation.response_status)
                || parsed.error.code != "device_revoked"
            {
                return Err(
                    "stored authenticated authority revocation error is invalid".to_string()
                );
            }
        }
        _ => return Err("stored mobile authority revocation evidence kind is invalid".to_string()),
    }
    Ok(stored)
}

fn valid_direct_sync_endpoint(value: &str) -> bool {
    matches!(
        value,
        "/sync/v1/negotiate"
            | "/sync/v1/bootstrap"
            | "/sync/v1/push"
            | "/sync/v1/pull"
            | "/sync/v1/checkpoint"
            | "/sync/v1/ack"
    )
}

fn valid_direct_sync_operation(endpoint: &str, operation: &str) -> bool {
    matches!(
        (endpoint, operation),
        ("/sync/v1/negotiate", "negotiate")
            | ("/sync/v1/bootstrap", "bootstrap")
            | ("/sync/v1/push", "push")
            | ("/sync/v1/pull", "pull")
            | ("/sync/v1/checkpoint", "checkpoint")
            | ("/sync/v1/ack", "ack")
    )
}

fn validate_direct_sync_purpose(
    purpose_json: &[u8],
    endpoint: &str,
    operation: &str,
    push_transaction_id: Option<&str>,
    push_counter: Option<i64>,
) -> Result<(), String> {
    use crate::mobile_sync_runtime::ExactRequestPurpose;

    if purpose_json.is_empty() || purpose_json.len() > MAX_MOBILE_DIRECT_SYNC_PURPOSE_BYTES {
        return Err("mobile direct-sync purpose is empty or oversized".to_string());
    }
    let value: serde_json::Value = serde_json::from_slice(purpose_json)
        .map_err(|error| format!("decode mobile direct-sync purpose: {error}"))?;
    if canonical_json(&value).as_bytes() != purpose_json {
        return Err("mobile direct-sync purpose is not canonical JSON".to_string());
    }
    let purpose: ExactRequestPurpose = serde_json::from_value(value)
        .map_err(|error| format!("decode typed mobile direct-sync purpose: {error}"))?;
    if purpose.endpoint().path() != endpoint || !valid_direct_sync_operation(endpoint, operation) {
        return Err("mobile direct-sync purpose does not match its endpoint".to_string());
    }
    match purpose {
        ExactRequestPurpose::Negotiate {
            capabilities_sha256,
        } if !is_sha256(&capabilities_sha256) => {
            Err("mobile negotiate purpose is invalid".to_string())
        }
        ExactRequestPurpose::Negotiate { .. } => Ok(()),
        ExactRequestPurpose::Bootstrap {
            requested_record_kinds,
            checkpoint_digest,
            after_record_id,
            limit,
        } => {
            if requested_record_kinds.is_empty()
                || limit == 0
                || checkpoint_digest
                    .as_deref()
                    .is_some_and(|digest| !is_sha256(digest))
                || after_record_id
                    .as_deref()
                    .is_some_and(|record_id| !is_uuid_v7(record_id))
                || (after_record_id.is_some() && checkpoint_digest.is_none())
            {
                Err("mobile bootstrap request purpose is invalid".to_string())
            } else {
                Ok(())
            }
        }
        ExactRequestPurpose::Push {
            transaction_id,
            transaction_digest,
            device_transaction_counter,
        } => {
            if !is_uuid_v7(&transaction_id)
                || !is_sha256(&transaction_digest)
                || device_transaction_counter == 0
                || device_transaction_counter > i64::MAX as u64
                || push_transaction_id != Some(transaction_id.as_str())
                || push_counter != i64::try_from(device_transaction_counter).ok()
            {
                Err("mobile push request purpose is invalid or misbound".to_string())
            } else {
                Ok(())
            }
        }
        ExactRequestPurpose::Pull {
            requested_cursor,
            limit,
            requested_record_kinds,
        } => {
            if requested_cursor > i64::MAX as u64 || limit == 0 || requested_record_kinds.is_empty()
            {
                Err("mobile pull request purpose is invalid".to_string())
            } else {
                Ok(())
            }
        }
        ExactRequestPurpose::Checkpoint { known_cursor } => {
            if known_cursor.is_some_and(|cursor| cursor > i64::MAX as u64) {
                Err("mobile checkpoint request purpose is invalid".to_string())
            } else {
                Ok(())
            }
        }
        ExactRequestPurpose::Ack {
            high_water_cursor,
            checkpoint_digest,
        } => {
            if high_water_cursor > i64::MAX as u64 || !is_sha256(&checkpoint_digest) {
                Err("mobile ack request purpose is invalid".to_string())
            } else {
                Ok(())
            }
        }
    }
}

fn valid_mobile_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b':')
        })
}

fn load_direct_sync_request(
    connection: &Connection,
    request_id: &str,
    endpoint: &str,
) -> Result<Option<MobileDirectSyncRequest>, String> {
    connection
        .query_row(
            "SELECT local_sequence, request_id, endpoint, operation,
                    purpose_json, purpose_sha256,
                    push_transaction_id, push_counter, receipt_id,
                    activation_sha256, library_id, device_id,
                    authority_generation, purge_generation, key_epoch,
                    sync_spki_sha256, request_bytes, request_sha256, request_content_type,
                    response_status, response_content_type, response_bytes,
                    response_sha256, state, attempts,
                    created_at, updated_at, last_attempt_at, response_received_at,
                    completed_at, quarantined_at, error_code
             FROM mobile_direct_sync_request_v1
             WHERE request_id = ?1 AND endpoint = ?2",
            params![request_id, endpoint],
            |row| {
                Ok(MobileDirectSyncRequest {
                    local_sequence: row.get(0)?,
                    request_id: row.get(1)?,
                    endpoint: row.get(2)?,
                    operation: row.get(3)?,
                    purpose_json: row.get(4)?,
                    purpose_sha256: row.get(5)?,
                    push_transaction_id: row.get(6)?,
                    push_counter: row.get(7)?,
                    receipt_id: row.get(8)?,
                    activation_sha256: row.get(9)?,
                    library_id: row.get(10)?,
                    device_id: row.get(11)?,
                    authority_generation: row.get(12)?,
                    purge_generation: row.get(13)?,
                    key_epoch: row.get(14)?,
                    sync_spki_sha256: row.get(15)?,
                    request_bytes: row.get(16)?,
                    request_sha256: row.get(17)?,
                    request_content_type: row.get(18)?,
                    response_status: row.get(19)?,
                    response_content_type: row.get(20)?,
                    response_bytes: row.get(21)?,
                    response_sha256: row.get(22)?,
                    state: row.get(23)?,
                    attempts: row.get(24)?,
                    created_at: row.get(25)?,
                    updated_at: row.get(26)?,
                    last_attempt_at: row.get(27)?,
                    response_received_at: row.get(28)?,
                    completed_at: row.get(29)?,
                    quarantined_at: row.get(30)?,
                    error_code: row.get(31)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn load_direct_sync_push_binding(
    connection: &Connection,
    transaction_id: &str,
) -> Result<Option<MobileDirectSyncPushBinding>, String> {
    connection
        .query_row(
            "SELECT transaction_id, request_id, push_counter, request_sha256,
                    receipt_id, activation_sha256, library_id, device_id,
                    authority_generation, purge_generation, key_epoch,
                    sync_spki_sha256, state, created_at, updated_at,
                    terminal_at, error_code
             FROM mobile_direct_sync_push_binding_v1 WHERE transaction_id = ?1",
            [transaction_id],
            |row| {
                Ok(MobileDirectSyncPushBinding {
                    transaction_id: row.get(0)?,
                    request_id: row.get(1)?,
                    push_counter: row.get(2)?,
                    request_sha256: row.get(3)?,
                    receipt_id: row.get(4)?,
                    activation_sha256: row.get(5)?,
                    library_id: row.get(6)?,
                    device_id: row.get(7)?,
                    authority_generation: row.get(8)?,
                    purge_generation: row.get(9)?,
                    key_epoch: row.get(10)?,
                    sync_spki_sha256: row.get(11)?,
                    state: row.get(12)?,
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                    terminal_at: row.get(15)?,
                    error_code: row.get(16)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn validate_direct_sync_push_binding(
    binding_row: &MobileDirectSyncPushBinding,
    binding: &MobileDirectSyncBinding,
) -> Result<(), String> {
    if !is_uuid(&binding_row.transaction_id)
        || !is_uuid_v7(&binding_row.request_id)
        || binding_row.push_counter <= 0
        || !is_sha256(&binding_row.request_sha256)
        || binding_row.receipt_id != binding.receipt_id
        || binding_row.activation_sha256 != binding.activation_sha256
        || binding_row.library_id != binding.library_id
        || binding_row.device_id != binding.device_id
        || binding_row.authority_generation != binding.authority_generation
        || binding_row.purge_generation != binding.purge_generation
        || binding_row.key_epoch != binding.key_epoch
        || binding_row.sync_spki_sha256 != binding.sync_spki_sha256
        || !matches!(
            binding_row.state.as_str(),
            "sending" | "awaiting_echo" | "acknowledged" | "conflict" | "rejected"
        )
        || binding_row.created_at < 0
        || binding_row.updated_at < binding_row.created_at
        || binding_row
            .terminal_at
            .is_some_and(|value| value < binding_row.created_at)
        || binding_row
            .error_code
            .as_deref()
            .is_some_and(|value| !valid_mobile_error_code(value))
        || matches!(
            binding_row.state.as_str(),
            "acknowledged" | "conflict" | "rejected"
        ) != binding_row.terminal_at.is_some()
    {
        return Err("mobile direct-sync push binding is invalid".to_string());
    }
    Ok(())
}

fn request_matches_binding(
    request: &MobileDirectSyncRequest,
    binding: &MobileDirectSyncBinding,
) -> bool {
    request.receipt_id == binding.receipt_id
        && request.activation_sha256 == binding.activation_sha256
        && request.library_id == binding.library_id
        && request.device_id == binding.device_id
        && request.authority_generation == binding.authority_generation
        && request.purge_generation == binding.purge_generation
        && request.key_epoch == binding.key_epoch
        && request.sync_spki_sha256 == binding.sync_spki_sha256
}

fn validate_direct_sync_request_row(
    request: &MobileDirectSyncRequest,
    binding: &MobileDirectSyncBinding,
) -> Result<(), String> {
    validate_direct_sync_purpose(
        &request.purpose_json,
        &request.endpoint,
        &request.operation,
        request.push_transaction_id.as_deref(),
        request.push_counter,
    )?;
    let response_tuple = request
        .response_status
        .zip(request.response_content_type.as_ref())
        .zip(request.response_bytes.as_ref())
        .zip(request.response_sha256.as_ref());
    let is_push = request.endpoint == "/sync/v1/push";
    if !is_uuid_v7(&request.request_id)
        || !valid_direct_sync_endpoint(&request.endpoint)
        || !valid_direct_sync_operation(&request.endpoint, &request.operation)
        || request.purpose_sha256 != exact_sha256(&request.purpose_json)
        || request.local_sequence <= 0
        || is_push != request.push_transaction_id.is_some()
        || is_push != request.push_counter.is_some()
        || request
            .push_transaction_id
            .as_deref()
            .is_some_and(|value| !is_uuid_v7(value))
        || request.push_counter.is_some_and(|value| value <= 0)
        || !request_matches_binding(request, binding)
        || request.request_bytes.is_empty()
        || request.request_bytes.len() > MAX_MOBILE_DIRECT_SYNC_REQUEST_BYTES
        || request.request_sha256 != exact_sha256(&request.request_bytes)
        || request.request_content_type != "application/json"
        || ![
            request.response_status.is_some(),
            request.response_content_type.is_some(),
            request.response_bytes.is_some(),
            request.response_sha256.is_some(),
        ]
        .windows(2)
        .all(|pair| pair[0] == pair[1])
        || response_tuple.is_some_and(|(((status, content_type), bytes), digest)| {
            !(100..=599).contains(&status)
                || content_type != "application/json"
                || bytes.is_empty()
                || bytes.len() > MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES
                || digest != &exact_sha256(bytes)
        })
        || request.response_bytes.as_ref().is_some_and(|bytes| {
            bytes.is_empty() || bytes.len() > MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES
        })
        || !matches!(
            request.state.as_str(),
            "pending" | "response_received" | "completed" | "quarantined"
        )
        || request.attempts < 0
        || request.attempts > MAX_MOBILE_DIRECT_SYNC_ATTEMPTS
        || request.created_at < 0
        || request.updated_at < request.created_at
        || request
            .last_attempt_at
            .is_some_and(|value| value < request.created_at)
        || request
            .response_received_at
            .is_some_and(|value| value < request.created_at)
        || request
            .completed_at
            .is_some_and(|value| value < request.created_at)
        || request
            .quarantined_at
            .is_some_and(|value| value < request.created_at)
        || request
            .error_code
            .as_deref()
            .is_some_and(|value| !valid_mobile_error_code(value))
    {
        return Err("mobile direct-sync request journal row is invalid".to_string());
    }
    let has_response = response_tuple.is_some();
    if (request.state == "pending" && has_response)
        || (matches!(request.state.as_str(), "response_received" | "completed") && !has_response)
        || (request.state == "completed") != request.completed_at.is_some()
        || (request.state == "quarantined") != request.quarantined_at.is_some()
        || (has_response != request.response_received_at.is_some())
    {
        return Err("mobile direct-sync request state does not match its exact bytes".to_string());
    }
    Ok(())
}

fn validate_authenticated_revocation_evidence(
    connection: &Connection,
    binding: &MobileDirectSyncBinding,
    evidence: &MobileAuthorityRevocationEvidence,
) -> Result<BoundMobileAuthorityRevocationEvidence, String> {
    if !is_uuid_v7(&evidence.request_id)
        || !valid_direct_sync_endpoint(&evidence.endpoint)
        || evidence.exact_response_bytes.is_empty()
        || evidence.exact_response_bytes.len() > MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES
    {
        return Err("authority revocation evidence is malformed".to_string());
    }
    let request = load_direct_sync_request(connection, &evidence.request_id, &evidence.endpoint)?
        .ok_or_else(|| {
        "authority revocation has no durable authenticated response".to_string()
    })?;
    validate_direct_sync_request_row(&request, binding)?;
    let response_status = request.response_status.ok_or_else(|| {
        "authority revocation has no durable authenticated response status".to_string()
    })?;
    let response_sha256 = exact_sha256(&evidence.exact_response_bytes);
    if request.response_content_type.as_deref() != Some("application/json")
        || request.response_bytes.as_deref() != Some(evidence.exact_response_bytes.as_slice())
        || request.response_sha256.as_deref() != Some(response_sha256.as_str())
        || !matches!(
            request.state.as_str(),
            "response_received" | "completed" | "quarantined"
        )
    {
        return Err(
            "authority revocation bytes do not match the authenticated response journal"
                .to_string(),
        );
    }

    if response_status != 200 {
        let parsed: MobileDirectSyncErrorBody =
            serde_json::from_slice(&evidence.exact_response_bytes)
                .map_err(|error| format!("decode authority revocation error: {error}"))?;
        if !(400..=599).contains(&response_status) || parsed.error.code != "device_revoked" {
            return Err(
                "authenticated transport response does not report device_revoked".to_string(),
            );
        }
        // The transport actor may only record this row after the pinned
        // VerifiedDirectSyncSession returns the response. The exact journal
        // identity and activation mirror are rechecked here so an unrelated or
        // stale response cannot revoke the current replica.
        return Ok(BoundMobileAuthorityRevocationEvidence {
            request,
            evidence_kind: "authenticated_transport_error",
        });
    }

    if evidence.endpoint != "/sync/v1/push" {
        return Err(
            "a successful authority revocation response must be a signed push receipt".to_string(),
        );
    }

    let signed_request: SignedSyncRequest<PushRequest> =
        serde_json::from_slice(&request.request_bytes)
            .map_err(|error| format!("decode revocation push request: {error}"))?;
    let signed_response: SignedSyncResponse<PushResponse> =
        serde_json::from_slice(&evidence.exact_response_bytes)
            .map_err(|error| format!("decode authority revocation response: {error}"))?;
    let transaction = &signed_request.payload.transaction;
    let receipt = &signed_response.payload.receipt;
    let mutation_ids = transaction
        .members
        .iter()
        .map(|member| member.mutation_id.clone())
        .collect::<Vec<_>>();
    if signed_request.protocol_version != SYNC_PROTOCOL_VERSION
        || signed_request.request_id != request.request_id
        || signed_request.library_id != binding.library_id
        || signed_request.device_id != binding.device_id
        || i64::try_from(signed_request.authority_generation).ok()
            != Some(binding.authority_generation)
        || signed_request.signature.len() != 64
        || transaction.manifest.library_id != binding.library_id
        || transaction.manifest.device_id != binding.device_id
        || i64::try_from(transaction.manifest.authority_generation).ok()
            != Some(binding.authority_generation)
        || i64::try_from(transaction.manifest.purge_generation).ok()
            != Some(binding.purge_generation)
        || i64::try_from(transaction.manifest.key_epoch).ok() != Some(binding.key_epoch)
        || request.push_transaction_id.as_deref()
            != Some(transaction.manifest.transaction_id.as_str())
        || request.push_counter
            != i64::try_from(transaction.manifest.device_transaction_counter).ok()
        || signed_response.protocol_version != SYNC_PROTOCOL_VERSION
        || signed_response.request_id != request.request_id
        || signed_response.library_id != binding.library_id
        || signed_response.device_id != binding.device_id
        || i64::try_from(signed_response.authority_generation).ok()
            != Some(binding.authority_generation)
        || signed_response.signature.len() != 64
        || receipt.library_id != binding.library_id
        || receipt.device_id != binding.device_id
        || receipt.transaction_id != transaction.manifest.transaction_id
        || receipt.transaction_digest != transaction.signed_digest()
        || receipt.mutation_ids != mutation_ids
        || receipt.device_transaction_counter != transaction.manifest.device_transaction_counter
        || i64::try_from(receipt.authority_generation).ok() != Some(binding.authority_generation)
        || i64::try_from(receipt.purge_generation).ok() != Some(binding.purge_generation)
        || !matches!(
            receipt.disposition,
            ReceiptDisposition::Rejected {
                code: TerminalRejection::DeviceRevoked
            }
        )
    {
        return Err(
            "authenticated authority revocation is not bound to its activation and push"
                .to_string(),
        );
    }
    Ok(BoundMobileAuthorityRevocationEvidence {
        request,
        evidence_kind: "signed_push_receipt",
    })
}

fn load_bootstrap_checkpoint(
    connection: &Connection,
    checkpoint_id: &str,
) -> Result<Option<MobileBootstrapCheckpoint>, String> {
    connection
        .query_row(
            "SELECT checkpoint_id, contract_version, checkpoint_sha256,
                    receipt_id, activation_sha256,
                    library_id, device_id, authority_generation, purge_generation,
                    key_epoch, sync_spki_sha256, start_cursor, high_water_cursor,
                    final_page_count, final_commitment_sha256, state, created_at,
                    finalized_at, applied_at, terminal_at, error_code
             FROM mobile_bootstrap_checkpoint_v1 WHERE checkpoint_id = ?1",
            [checkpoint_id],
            |row| {
                Ok(MobileBootstrapCheckpoint {
                    checkpoint_id: row.get(0)?,
                    contract_version: row.get(1)?,
                    checkpoint_sha256: row.get(2)?,
                    receipt_id: row.get(3)?,
                    activation_sha256: row.get(4)?,
                    library_id: row.get(5)?,
                    device_id: row.get(6)?,
                    authority_generation: row.get(7)?,
                    purge_generation: row.get(8)?,
                    key_epoch: row.get(9)?,
                    sync_spki_sha256: row.get(10)?,
                    start_cursor: row.get(11)?,
                    high_water_cursor: row.get(12)?,
                    final_page_count: row.get(13)?,
                    final_commitment_sha256: row.get(14)?,
                    state: row.get(15)?,
                    created_at: row.get(16)?,
                    finalized_at: row.get(17)?,
                    applied_at: row.get(18)?,
                    terminal_at: row.get(19)?,
                    error_code: row.get(20)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn load_bootstrap_pages(
    connection: &Connection,
    checkpoint_id: &str,
) -> Result<Vec<MobileBootstrapPage>, String> {
    connection
        .prepare(
            "SELECT checkpoint_id, page_index, checkpoint_sha256,
                    requested_after_record_id, next_after_record_id, has_more,
                    dependency_sha256, response_bytes, response_sha256,
                    state, received_at, applied_at, quarantined_at, error_code
             FROM mobile_bootstrap_page_v1
             WHERE checkpoint_id = ?1 ORDER BY page_index",
        )
        .and_then(|mut statement| {
            statement
                .query_map([checkpoint_id], |row| {
                    Ok(MobileBootstrapPage {
                        checkpoint_id: row.get(0)?,
                        page_index: row.get(1)?,
                        checkpoint_sha256: row.get(2)?,
                        requested_after_record_id: row.get(3)?,
                        next_after_record_id: row.get(4)?,
                        has_more: row.get::<_, i64>(5)? == 1,
                        dependency_sha256: row.get(6)?,
                        response_bytes: row.get(7)?,
                        response_sha256: row.get(8)?,
                        state: row.get(9)?,
                        received_at: row.get(10)?,
                        applied_at: row.get(11)?,
                        quarantined_at: row.get(12)?,
                        error_code: row.get(13)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())
}

fn bootstrap_recovery(
    connection: &Connection,
    checkpoint_id: &str,
) -> Result<Option<MobileBootstrapRecovery>, String> {
    let Some(checkpoint) = load_bootstrap_checkpoint(connection, checkpoint_id)? else {
        return Ok(None);
    };
    let pages = load_bootstrap_pages(connection, checkpoint_id)?;
    Ok(Some(MobileBootstrapRecovery { checkpoint, pages }))
}

fn prune_completed_direct_sync_in_transaction(
    transaction: &Transaction<'_>,
    retain_recent_completed: usize,
) -> Result<usize, String> {
    let rows = transaction
        .prepare(
            "SELECT local_sequence, length(request_bytes) + length(purpose_json),
                    COALESCE(length(response_bytes), 0), COALESCE(push_counter, 0),
                    push_transaction_id
             FROM mobile_direct_sync_request_v1
             WHERE state = 'completed'
               AND (
                 push_transaction_id IS NULL OR EXISTS (
                   SELECT 1 FROM mobile_direct_sync_push_binding_v1 AS binding
                   WHERE binding.transaction_id = mobile_direct_sync_request_v1.push_transaction_id
                     AND binding.state IN ('acknowledged', 'conflict', 'rejected')
                 )
               )
               AND local_sequence NOT IN (
                 SELECT local_sequence FROM mobile_direct_sync_request_v1
                 WHERE state = 'completed'
                   AND (
                     push_transaction_id IS NULL OR EXISTS (
                       SELECT 1 FROM mobile_direct_sync_push_binding_v1 AS binding
                       WHERE binding.transaction_id = mobile_direct_sync_request_v1.push_transaction_id
                         AND binding.state IN ('acknowledged', 'conflict', 'rejected')
                     )
                   )
                 ORDER BY local_sequence DESC LIMIT ?1
               )
             ORDER BY local_sequence",
        )
        .and_then(|mut statement| {
            statement
                .query_map([retain_recent_completed as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok(0);
    }
    let through_sequence = rows.last().map(|row| row.0).unwrap_or(0);
    let request_bytes = rows.iter().try_fold(0_i64, |total, row| {
        total
            .checked_add(row.1)
            .ok_or_else(|| "direct-sync prune byte count overflowed".to_string())
    })?;
    let response_bytes = rows.iter().try_fold(0_i64, |total, row| {
        total
            .checked_add(row.2)
            .ok_or_else(|| "direct-sync prune byte count overflowed".to_string())
    })?;
    let max_push_counter = rows.iter().map(|row| row.3).max().unwrap_or(0);
    for (sequence, _, _, _, transaction_id) in &rows {
        let deleted = transaction
            .execute(
                "DELETE FROM mobile_direct_sync_request_v1
                 WHERE local_sequence = ?1 AND state = 'completed'",
                [sequence],
            )
            .map_err(|error| error.to_string())?;
        if deleted != 1 {
            return Err("completed direct-sync row changed during compaction".to_string());
        }
        if let Some(transaction_id) = transaction_id {
            let deleted_binding = transaction
                .execute(
                    "DELETE FROM mobile_direct_sync_push_binding_v1
                     WHERE transaction_id = ?1
                       AND state IN ('acknowledged', 'conflict', 'rejected')",
                    [transaction_id],
                )
                .map_err(|error| error.to_string())?;
            if deleted_binding != 1 {
                return Err(
                    "completed direct-sync push lost its terminal lifecycle binding".to_string(),
                );
            }
        }
    }
    let updated_at = now_millis()?;
    let changed = transaction
        .execute(
            "UPDATE mobile_direct_sync_journal_summary_v1
             SET pruned_through_sequence = MAX(pruned_through_sequence, ?1),
                 pruned_completed_count = pruned_completed_count + ?2,
                 pruned_request_bytes = pruned_request_bytes + ?3,
                 pruned_response_bytes = pruned_response_bytes + ?4,
                 max_pruned_push_counter = MAX(max_pruned_push_counter, ?5),
                 updated_at = ?6
             WHERE singleton = 1",
            params![
                through_sequence,
                rows.len() as i64,
                request_bytes,
                response_bytes,
                max_push_counter,
                updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("direct-sync compaction summary is missing".to_string());
    }
    Ok(rows.len())
}

fn validate_bootstrap_recovery(
    recovery: &MobileBootstrapRecovery,
    binding: &MobileDirectSyncBinding,
) -> Result<(), String> {
    let checkpoint = &recovery.checkpoint;
    if !is_uuid_v7(&checkpoint.checkpoint_id)
        || checkpoint.contract_version != crate::sync_protocol::BOOTSTRAP_SNAPSHOT_VERSION
        || !is_sha256(&checkpoint.checkpoint_sha256)
        || checkpoint.receipt_id != binding.receipt_id
        || checkpoint.activation_sha256 != binding.activation_sha256
        || checkpoint.library_id != binding.library_id
        || checkpoint.device_id != binding.device_id
        || checkpoint.authority_generation != binding.authority_generation
        || checkpoint.purge_generation != binding.purge_generation
        || checkpoint.key_epoch != binding.key_epoch
        || checkpoint.sync_spki_sha256 != binding.sync_spki_sha256
        || checkpoint.start_cursor < 0
        || checkpoint.high_water_cursor < checkpoint.start_cursor
        || checkpoint
            .final_page_count
            .is_some_and(|count| count <= 0 || count > MAX_MOBILE_BOOTSTRAP_PAGES as i64)
        || checkpoint.created_at < 0
        || checkpoint
            .finalized_at
            .is_some_and(|value| value < checkpoint.created_at)
        || checkpoint
            .applied_at
            .is_some_and(|value| value < checkpoint.created_at)
        || checkpoint
            .terminal_at
            .is_some_and(|value| value < checkpoint.created_at)
        || checkpoint
            .error_code
            .as_deref()
            .is_some_and(|value| !valid_mobile_error_code(value))
        || !matches!(
            checkpoint.state.as_str(),
            "receiving" | "received" | "applied" | "aborted" | "quarantined"
        )
    {
        return Err("mobile bootstrap checkpoint is invalid or activation-mismatched".to_string());
    }
    let is_finalized = checkpoint.final_page_count.is_some();
    if is_finalized != checkpoint.final_commitment_sha256.is_some()
        || checkpoint
            .final_commitment_sha256
            .as_deref()
            .is_some_and(|digest| digest != checkpoint.checkpoint_sha256)
        || (checkpoint.state == "receiving" && is_finalized)
        || (matches!(checkpoint.state.as_str(), "received" | "applied") && !is_finalized)
        || (checkpoint.state == "applied") != checkpoint.applied_at.is_some()
        || matches!(checkpoint.state.as_str(), "aborted" | "quarantined")
            != checkpoint.terminal_at.is_some()
    {
        return Err("mobile bootstrap checkpoint state is internally inconsistent".to_string());
    }
    if recovery.pages.len() > MAX_MOBILE_BOOTSTRAP_PAGES {
        return Err("mobile bootstrap page count exceeds its bound".to_string());
    }
    let total_bytes = recovery.pages.iter().try_fold(0usize, |total, page| {
        total
            .checked_add(page.response_bytes.len())
            .ok_or_else(|| "mobile bootstrap page bytes overflowed".to_string())
    })?;
    if total_bytes > MAX_MOBILE_BOOTSTRAP_TOTAL_BYTES as usize {
        return Err("mobile bootstrap pages exceed their aggregate byte limit".to_string());
    }
    for (index, page) in recovery.pages.iter().enumerate() {
        if page.checkpoint_id != checkpoint.checkpoint_id
            || page.page_index != index as i64
            || page.checkpoint_sha256 != checkpoint.checkpoint_sha256
            || page.response_bytes.is_empty()
            || page.response_bytes.len() > MAX_MOBILE_BOOTSTRAP_PAGE_BYTES
            || page.response_sha256 != exact_sha256(&page.response_bytes)
            || page
                .requested_after_record_id
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 512)
            || page
                .next_after_record_id
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 512)
            || (index == 0) != page.requested_after_record_id.is_none()
            || (index == 0) != page.dependency_sha256.is_none()
            || (page.has_more && page.next_after_record_id.is_none())
            || page.received_at < checkpoint.created_at
            || page
                .applied_at
                .is_some_and(|value| value < page.received_at)
            || page
                .quarantined_at
                .is_some_and(|value| value < page.received_at)
            || page
                .error_code
                .as_deref()
                .is_some_and(|value| !valid_mobile_error_code(value))
            || !matches!(page.state.as_str(), "received" | "applied" | "quarantined")
            || (page.state == "applied") != page.applied_at.is_some()
            || (page.state == "quarantined") != page.quarantined_at.is_some()
        {
            return Err("mobile bootstrap page is invalid".to_string());
        }
        if let Some(previous) = index
            .checked_sub(1)
            .and_then(|value| recovery.pages.get(value))
        {
            if !previous.has_more
                || page.requested_after_record_id != previous.next_after_record_id
                || page.dependency_sha256.as_deref() != Some(previous.response_sha256.as_str())
            {
                return Err("mobile bootstrap page chain is discontinuous".to_string());
            }
        }
    }
    if let Some(final_count) = checkpoint.final_page_count {
        if final_count as usize != recovery.pages.len()
            || recovery.pages.last().is_none_or(|page| page.has_more)
        {
            return Err("mobile bootstrap final page count is inconsistent".to_string());
        }
    } else if recovery.pages.last().is_some_and(|page| !page.has_more) {
        return Err("mobile bootstrap final page is missing its atomic commitment".to_string());
    }
    match checkpoint.state.as_str() {
        "receiving" | "received" => {
            if recovery.pages.iter().any(|page| page.state != "received") {
                return Err("open bootstrap pages have an invalid state".to_string());
            }
        }
        "applied" => {
            if recovery.pages.iter().any(|page| page.state != "applied") {
                return Err("applied bootstrap has unapplied pages".to_string());
            }
        }
        "quarantined" => {
            if recovery
                .pages
                .iter()
                .any(|page| page.state != "quarantined")
            {
                return Err("quarantined bootstrap has unquarantined pages".to_string());
            }
        }
        "aborted" => {
            if recovery.pages.iter().any(|page| page.state != "received") {
                return Err("aborted bootstrap evidence has an invalid state".to_string());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn quarantine_bootstrap_in_transaction(
    transaction: &Transaction<'_>,
    checkpoint_id: &str,
    error_code: &str,
) -> Result<(), String> {
    if !is_uuid_v7(checkpoint_id) || !valid_mobile_error_code(error_code) {
        return Err("bootstrap quarantine target or reason is invalid".to_string());
    }
    let checkpoint = load_bootstrap_checkpoint(transaction, checkpoint_id)?
        .ok_or_else(|| "bootstrap checkpoint does not exist".to_string())?;
    if checkpoint.state == "quarantined" {
        return if checkpoint.error_code.as_deref() == Some(error_code) {
            Ok(())
        } else {
            Err("bootstrap quarantine reason cannot be rewritten".to_string())
        };
    }
    if matches!(checkpoint.state.as_str(), "applied" | "aborted") {
        return Err("terminal bootstrap checkpoint cannot be quarantined".to_string());
    }
    let quarantined_at = now_millis()?;
    transaction
        .execute(
            "UPDATE mobile_bootstrap_page_v1
             SET state = 'quarantined', quarantined_at = ?1, error_code = ?2
             WHERE checkpoint_id = ?3 AND state = 'received'",
            params![quarantined_at, error_code, checkpoint_id],
        )
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE mobile_bootstrap_checkpoint_v1
             SET state = 'quarantined', terminal_at = ?1, error_code = ?2
             WHERE checkpoint_id = ?3 AND state IN ('receiving', 'received')",
            params![quarantined_at, error_code, checkpoint_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("bootstrap checkpoint could not be quarantined atomically".to_string());
    }
    Ok(())
}

fn finish_bootstrap_terminal(
    store: &MobileStore,
    checkpoint_id: &str,
    terminal_state: &str,
    error_code: &str,
) -> Result<MobileBootstrapRecovery, String> {
    if !matches!(terminal_state, "aborted" | "quarantined")
        || !is_uuid_v7(checkpoint_id)
        || !valid_mobile_error_code(error_code)
    {
        return Err("bootstrap terminal transition is invalid".to_string());
    }
    let mut connection = store.lock_connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let binding = active_direct_sync_binding(&transaction)?;
    let before = bootstrap_recovery(&transaction, checkpoint_id)?
        .ok_or_else(|| "bootstrap checkpoint does not exist".to_string())?;
    validate_bootstrap_recovery(&before, &binding)?;
    if before.checkpoint.state == terminal_state {
        if before.checkpoint.error_code.as_deref() != Some(error_code) {
            return Err("bootstrap terminal reason cannot be rewritten".to_string());
        }
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(before);
    }
    if matches!(
        before.checkpoint.state.as_str(),
        "applied" | "aborted" | "quarantined"
    ) {
        return Err("bootstrap checkpoint is already terminal".to_string());
    }
    if terminal_state == "quarantined" {
        quarantine_bootstrap_in_transaction(&transaction, checkpoint_id, error_code)?;
    } else {
        let terminal_at = now_millis()?;
        let changed = transaction
            .execute(
                "UPDATE mobile_bootstrap_checkpoint_v1
                 SET state = 'aborted', terminal_at = ?1, error_code = ?2
                 WHERE checkpoint_id = ?3 AND state IN ('receiving', 'received')",
                params![terminal_at, error_code, checkpoint_id],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("bootstrap checkpoint could not be aborted atomically".to_string());
        }
    }
    let result = bootstrap_recovery(&transaction, checkpoint_id)?
        .ok_or_else(|| "terminal bootstrap checkpoint disappeared".to_string())?;
    validate_bootstrap_recovery(&result, &binding)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(result)
}

fn pending_checkpoint_precedes_activation(
    pending: &MobilePairingCheckpoint,
    active: &MobilePairingCheckpoint,
) -> bool {
    if pending.identity_handle != active.identity_handle
        || pending.pending_bootstrap_handle.is_none()
        || active.pending_bootstrap_handle.is_some()
        || pending.updated_at > active.updated_at
    {
        return false;
    }
    let mut expected = active.client.clone();
    expected.state = PairingClientState::PendingActivation;
    expected.activation = None;
    pending.client == expected
}

fn adopt_staging_for_pairing_activation(
    transaction: &Transaction<'_>,
    library_id: &str,
    default_scope_id: &str,
) -> Result<usize, String> {
    let staging_identity = replica_identity(transaction)?;
    if staging_identity.library_state != "local_staging" {
        return Err(
            "atomic pairing activation requires the untouched local_staging replica".to_string(),
        );
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
        authority: String,
    }
    let notes = transaction
        .prepare(
            "SELECT id, record_id, title, body, created_at, updated_at,
                    working_revision, working_branch_id, canonical_hash, lifecycle_state,
                    trashed_at, tombstoned_at, provenance_json, scope_id, scope_class, authority
             FROM mobile_notes ORDER BY id",
        )
        .and_then(|mut statement| {
            statement
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
                        authority: row.get(15)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;

    let adoption_time = next_timestamp(transaction)?;
    transaction
        .execute(
            "UPDATE mobile_note_outbox
             SET state = 'superseded', eligible_for_sync = 0, superseded_at = ?1
             WHERE library_id = ?2 AND eligible_for_sync = 1",
            params![adoption_time, staging_identity.library_id],
        )
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE mobile_replica
             SET library_id = ?1, default_scope_id = ?2, library_state = 'paired'
             WHERE singleton = 1 AND library_state = 'local_staging'",
            params![library_id, default_scope_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("mobile replica changed during atomic pairing activation".to_string());
    }
    transaction
        .execute(
            "UPDATE mobile_note_folders SET library_id = ?1 WHERE library_id = ?2",
            params![library_id, staging_identity.library_id],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE mobile_note_categories SET library_id = ?1 WHERE library_id = ?2",
            params![library_id, staging_identity.library_id],
        )
        .map_err(|error| error.to_string())?;
    rebind_staging_canonical_records(
        transaction,
        &staging_identity.library_id,
        library_id,
        &staging_identity.default_scope_id,
        default_scope_id,
    )?;
    let paired_identity = replica_identity(transaction)?;

    struct PlannedAdoption {
        note_index: usize,
        scope_id: String,
        scope_class: String,
        working_revision: i64,
        working_version_id: String,
        mutation_id: String,
        ciphertext_bytes: usize,
    }
    let mut plans = Vec::with_capacity(notes.len());
    for (note_index, note) in notes.iter().enumerate() {
        let remaps_default_scope = note.scope_id == staging_identity.default_scope_id;
        let scope_id = if remaps_default_scope {
            default_scope_id.to_string()
        } else {
            note.scope_id.clone()
        };
        let scope_class = if remaps_default_scope {
            "unknown".to_string()
        } else {
            note.scope_class.clone()
        };
        let working_revision = note
            .working_revision
            .checked_add(1)
            .ok_or_else(|| "mobile note working revision overflowed".to_string())?;
        let working_version_id = new_uuid_v7();
        let mutation_id = new_uuid_v7();
        let payload_json = serialize_mutation_payload(
            &paired_identity,
            &Mutation {
                operation: "create",
                patch_title_body: false,
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
                authority: &note.authority,
                provenance_json: &note.provenance_json,
                scope_id: &scope_id,
                scope_class: &scope_class,
            },
        )?;
        plans.push(PlannedAdoption {
            note_index,
            scope_id,
            scope_class,
            working_revision,
            working_version_id,
            mutation_id,
            ciphertext_bytes: payload_json
                .len()
                .checked_add(MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES)
                .ok_or_else(|| "mobile note mutation ciphertext size overflowed".to_string())?,
        });
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_bytes = 0usize;
    for (plan_index, plan) in plans.iter().enumerate() {
        let needs_new_group = groups.last().is_some_and(|group| {
            group.len() >= MAX_MOBILE_TRANSACTION_MEMBERS
                || group_bytes
                    .checked_add(plan.ciphertext_bytes)
                    .is_none_or(|total| total > MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES)
        });
        if groups.is_empty() || needs_new_group {
            groups.push(Vec::new());
            group_bytes = 0;
        }
        groups.last_mut().expect("group exists").push(plan_index);
        group_bytes = group_bytes
            .checked_add(plan.ciphertext_bytes)
            .ok_or_else(|| "mobile outbox transaction size overflowed".to_string())?;
    }
    for group in groups {
        let outbox_transaction = begin_outbox_transaction(transaction, group.len())?;
        for (member_index, plan_index) in group.into_iter().enumerate() {
            let plan = &plans[plan_index];
            let note = &notes[plan.note_index];
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
                         scope = ?6,
                         scope_id = ?5,
                         scope_class = ?6
                     WHERE id = ?7",
                    params![
                        library_id,
                        plan.working_revision,
                        plan.working_version_id,
                        plan.mutation_id,
                        plan.scope_id,
                        plan.scope_class,
                        note.id,
                    ],
                )
                .map_err(|error| error.to_string())?;
            enqueue_mutation(
                transaction,
                &paired_identity,
                &outbox_transaction,
                i64::try_from(member_index)
                    .map_err(|_| "outbox transaction has too many members".to_string())?,
                Mutation {
                    operation: "create",
                    patch_title_body: false,
                    record_id: &note.record_id,
                    title: &note.title,
                    body: &note.body,
                    base_revision: 0,
                    proposed_revision: 1,
                    local_revision: plan.working_revision,
                    version_id: &plan.working_version_id,
                    branch_id: &note.working_branch_id,
                    base_version_id: None,
                    accepted_content_hash: None,
                    mutation_id: &plan.mutation_id,
                    canonical_hash: &note.canonical_hash,
                    lifecycle_state: &note.lifecycle_state,
                    trashed_at: note.trashed_at,
                    tombstoned_at: note.tombstoned_at,
                    created_at: note.created_at,
                    updated_at: note.updated_at,
                    authority: &note.authority,
                    provenance_json: &note.provenance_json,
                    scope_id: &plan.scope_id,
                    scope_class: &plan.scope_class,
                },
            )?;
        }
    }
    Ok(notes.len())
}

fn activate_sync_enrollment_in_transaction(
    transaction: &Transaction<'_>,
    activation: &MobilePairingActivation,
) -> Result<(), String> {
    let identity = replica_identity(transaction)?;
    if identity.library_state != "paired"
        || identity.library_id != activation.library_id
        || identity.device_id != activation.device_id
        || identity.default_scope_id != activation.default_scope_id
    {
        return Err("mobile replica adoption does not match pairing activation".to_string());
    }
    let (enrollment_state, current_authority, current_purge): (String, i64, i64) = transaction
        .query_row(
            "SELECT enrollment_state, authority_generation, purge_generation
             FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if enrollment_state != "not_enrolled"
        || activation.authority_generation < current_authority
        || activation.purge_generation < current_purge
    {
        return Err("mobile sync enrollment is not an untouched monotonic activation".to_string());
    }
    let pending: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE mobile_sync_state
             SET enrollment_state = 'active', sync_state = ?1,
                 authority_generation = ?2, purge_generation = ?3,
                 last_error_code = NULL
             WHERE singleton = 1 AND enrollment_state = 'not_enrolled'",
            params![
                if pending { "pending" } else { "idle" },
                activation.authority_generation,
                activation.purge_generation,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("mobile sync enrollment changed during pairing activation".to_string());
    }
    Ok(())
}

impl MobileStore {
    /// Internal recovery artifact created before the one-time prototype
    /// migration. A future Settings flow can expose an explicit export/reset
    /// choice without guessing a filesystem location.
    #[allow(dead_code)]
    pub fn migration_recovery_path(&self) -> Result<Option<String>, String> {
        let connection = self.lock_connection()?;
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
        let connection = self.lock_connection()?;
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

    /// Confirms that a public-ID navigation target belongs to this replica and
    /// is still visible to the mobile product. Deep links never bypass the
    /// repository's library or lifecycle boundaries.
    pub fn verify_note_link(&self, library_id: &str, record_id: &str) -> Result<(), String> {
        if !is_uuid_v7(library_id) || !is_uuid_v7(record_id) {
            return Err("mobile note link requires canonical UUIDv7 identifiers".to_string());
        }
        let connection = self.lock_connection()?;
        let current_library_id: String = connection
            .query_row(
                "SELECT library_id FROM mobile_replica WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if current_library_id != library_id {
            return Err("mobile note link belongs to a different notebook".to_string());
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_notes
                   WHERE library_id = ?1 AND record_id = ?2
                     AND lifecycle_state IN ('active', 'trash')
                 )",
                params![library_id, record_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            return Err("mobile note link is not available in this notebook".to_string());
        }
        Ok(())
    }

    pub fn workspace(
        &self,
        query: Option<&str>,
        view: Option<&str>,
        folder_id: Option<&str>,
    ) -> Result<MobileNotesWorkspace, String> {
        let connection = self.lock_connection()?;
        let library_id: String = connection
            .query_row(
                "SELECT library_id FROM mobile_replica WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let view = view
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("inbox");
        if !matches!(view, "inbox" | "all" | "needsFiling" | "folder" | "trash") {
            return Err(format!("unsupported mobile notes view {view}"));
        }
        let folder_id = folder_id.map(str::trim).filter(|value| !value.is_empty());
        if folder_id.is_some_and(|value| !is_uuid_v7(value)) {
            return Err("folderId must be a canonical UUIDv7".to_string());
        }
        if view == "folder" && folder_id.is_none() {
            return Err("folder view requires folderId".to_string());
        }
        if let Some(folder_id) = folder_id {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM mobile_note_folders
                       WHERE folder_id = ?1 AND library_id = ?2
                         AND lifecycle_state = 'active'
                     )",
                    params![folder_id, library_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !exists {
                return Err(format!("folder {folder_id} does not exist"));
            }
        }
        let query_pattern = query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", escape_like(value)));
        let mut statement = connection
            .prepare(
                "SELECT notes.record_id, notes.title, notes.body,
                        notes.created_at, notes.updated_at,
                        filing.folder_id, folders.name,
                        notes.lifecycle_state, notes.sync_state,
                        notes.conflict_of, notes.authority,
                        EXISTS(
                          SELECT 1 FROM mobile_note_conflicts AS conflicts
                          WHERE conflicts.record_id = notes.record_id
                            AND conflicts.state = 'open'
                        ),
                        EXISTS(
                          SELECT 1 FROM mobile_sync_state
                          WHERE singleton = 1 AND enrollment_state = 'active'
                        )
                 FROM mobile_notes AS notes
                 LEFT JOIN mobile_note_filing AS filing
                   ON filing.record_id = notes.record_id
                 LEFT JOIN mobile_note_folders AS folders
                   ON folders.folder_id = filing.folder_id
                  AND folders.lifecycle_state = 'active'
                 WHERE (
                   (?1 = 'trash' AND notes.lifecycle_state = 'trash')
                   OR (?1 != 'trash' AND notes.lifecycle_state = 'active')
                 )
                   AND (?1 != 'needsFiling' OR filing.folder_id IS NULL)
                   AND (?1 != 'folder' OR filing.folder_id = ?2)
                   AND (?3 IS NULL
                        OR notes.title LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                        OR notes.body LIKE ?3 ESCAPE '\\' COLLATE NOCASE)
                   AND notes.library_id = ?4
                 ORDER BY notes.updated_at DESC, notes.record_id",
            )
            .map_err(|error| error.to_string())?;
        let notes = statement
            .query_map(params![view, folder_id, query_pattern, library_id], |row| {
                let folder_id: Option<String> = row.get(5)?;
                Ok(MobileWorkspaceNote {
                    record_id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    folder_id: folder_id.clone(),
                    folder_name: row.get(6)?,
                    lifecycle_state: public_lifecycle_state(&row.get::<_, String>(7)?),
                    needs_filing: folder_id.is_none(),
                    sync_state: public_note_sync_state(&row.get::<_, String>(8)?, row.get(12)?),
                    conflict_of: row.get(9)?,
                    has_open_conflict: row.get(11)?,
                    read_only: row.get::<_, String>(10)? != "noted",
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;

        let raw_folders = connection
            .prepare(
                "SELECT folders.folder_id, folders.name, folders.parent_folder_id,
                        folders.position,
                        COUNT(CASE WHEN notes.lifecycle_state = 'active' THEN 1 END)
                 FROM mobile_note_folders AS folders
                 LEFT JOIN mobile_note_filing AS filing
                   ON filing.folder_id = folders.folder_id
                 LEFT JOIN mobile_notes AS notes
                   ON notes.record_id = filing.record_id
                 WHERE folders.lifecycle_state = 'active'
                   AND folders.library_id = ?1
                 GROUP BY folders.folder_id, folders.name,
                          folders.parent_folder_id, folders.position
                 ORDER BY folders.position, folders.name, folders.folder_id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([&library_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        let folder_index = raw_folders
            .iter()
            .map(|(folder_id, name, parent_id, _, _)| {
                (folder_id.clone(), (name.clone(), parent_id.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let folders = raw_folders
            .into_iter()
            .map(|(folder_id, name, parent_id, _, note_count)| {
                Ok(MobileWorkspaceFolder {
                    path: Some(logical_folder_path(&folder_id, &folder_index)?),
                    folder_id,
                    name,
                    parent_id,
                    note_count,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let (enrollment_state, stored_sync_state, last_synced_at, library_state): (
            String,
            String,
            Option<i64>,
            String,
        ) = connection
            .query_row(
                "SELECT sync.enrollment_state, sync.sync_state, sync.last_synced_at,
                        replica.library_state
                 FROM mobile_sync_state AS sync
                 CROSS JOIN mobile_replica AS replica
                 WHERE sync.singleton = 1 AND replica.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|error| error.to_string())?;
        let pending_count: i64 = connection
            .query_row(
                "SELECT COUNT(DISTINCT transaction_id)
                 FROM mobile_note_outbox WHERE eligible_for_sync = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let has_conflict: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_conflicts WHERE state = 'open'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let (inbox_count, needs_filing_count, trash_count): (i64, i64, i64) = connection
            .query_row(
                "SELECT
                   COUNT(CASE WHEN notes.lifecycle_state = 'active' THEN 1 END),
                   COUNT(CASE WHEN notes.lifecycle_state = 'active'
                                   AND filing.folder_id IS NULL THEN 1 END),
                   COUNT(CASE WHEN notes.lifecycle_state = 'trash' THEN 1 END)
                 FROM mobile_notes AS notes
                 LEFT JOIN mobile_note_filing AS filing
                   ON filing.record_id = notes.record_id
                 WHERE notes.library_id = ?1",
                [&library_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        let enrolled = enrollment_state == "active";
        let sync_state = if !enrolled {
            if library_state == "local_staging" {
                "local".to_string()
            } else {
                "not_enrolled".to_string()
            }
        } else if has_conflict {
            "error".to_string()
        } else if pending_count > 0 {
            "pending".to_string()
        } else {
            match stored_sync_state.as_str() {
                "idle" => "synced".to_string(),
                "not_enrolled" => "not_enrolled".to_string(),
                "pending" | "syncing" | "error" => stored_sync_state,
                "conflict" | "revoked" => "error".to_string(),
                _ => "error".to_string(),
            }
        };
        Ok(MobileNotesWorkspace {
            notes,
            folders,
            capabilities: MobileWorkspaceCapabilities {
                filing: true,
                undo_filing: true,
                trash: true,
                restore: true,
                conflict_resolution: enrolled,
            },
            sync: MobileWorkspaceSync {
                state: sync_state,
                pending_count,
                last_synced_at,
            },
            counts: MobileWorkspaceCounts {
                inbox: inbox_count,
                needs_filing: needs_filing_count,
                trash: trash_count,
            },
        })
    }

    pub fn create(&self, title: &str, body: &str) -> Result<MobileNote, String> {
        let mut connection = self.lock_connection()?;
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
        let default_scope_class = if identity.library_state == "paired" {
            "unknown"
        } else {
            "personal"
        };

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
                   ?9, 'noted', ?11, ?10, ?11,
                   'standard', ?12,
                   ?13, ?13, ?14
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
                    default_scope_class,
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
                patch_title_body: true,
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
                authority: "noted",
                provenance_json,
                scope_id: &identity.default_scope_id,
                scope_class: default_scope_class,
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
        let mut connection = self.lock_connection()?;
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
        ensure_no_open_note_conflict(&transaction, record_id)?;
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
                patch_title_body: true,
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
                authority: &state.authority,
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

    pub fn file_note(
        &self,
        record_id: &str,
        folder_id: &str,
    ) -> Result<MobileWorkspaceNote, String> {
        if !is_uuid_v7(folder_id) {
            return Err("folderId must be a canonical UUIDv7".to_string());
        }
        self.set_filing(record_id, Some(folder_id), "file")
    }

    pub fn undo_note_filing(&self, record_id: &str) -> Result<MobileWorkspaceNote, String> {
        self.set_filing(record_id, None, "undoFiling")
    }

    #[allow(dead_code)]
    pub fn restore(&self, record_id: &str) -> Result<MobileNote, String> {
        self.set_lifecycle(record_id, "active", "restore")?;
        let connection = self.lock_connection()?;
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

    fn set_filing(
        &self,
        record_id: &str,
        requested_folder_id: Option<&str>,
        action: &str,
    ) -> Result<MobileWorkspaceNote, String> {
        if !is_uuid_v7(record_id) {
            return Err("note recordId must be a canonical UUIDv7".to_string());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let identity = replica_identity(&transaction)?;
        let state = portable_state(&transaction, record_id)?
            .filter(|state| state.lifecycle_state == "active")
            .ok_or_else(|| format!("note {record_id} does not exist"))?;
        ensure_noted_authority(&state)?;
        ensure_no_open_note_conflict(&transaction, record_id)?;
        let (title, body): (String, String) = transaction
            .query_row(
                "SELECT title, body FROM mobile_notes WHERE record_id = ?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let current_filing: Option<(Option<String>, Option<String>)> = transaction
            .query_row(
                "SELECT folder_id, previous_folder_id
                 FROM mobile_note_filing WHERE record_id = ?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let current_folder_id = current_filing
            .as_ref()
            .and_then(|(folder_id, _)| folder_id.clone());
        let target_folder_id = match action {
            "file" => requested_folder_id.map(str::to_string),
            "undoFiling" => current_filing
                .as_ref()
                .ok_or_else(|| format!("note {record_id} has no filing change to undo"))?
                .1
                .clone(),
            _ => return Err(format!("unsupported filing action {action}")),
        };
        if action == "file" && target_folder_id == current_folder_id {
            return workspace_note_by_id(&transaction, record_id);
        }
        if let Some(folder_id) = target_folder_id.as_deref() {
            let valid_folder: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM mobile_note_folders
                       WHERE folder_id = ?1 AND library_id = ?2
                         AND lifecycle_state = 'active'
                     )",
                    params![folder_id, identity.library_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !valid_folder {
                return Err(format!("folder {folder_id} does not exist"));
            }
        }

        let timestamp = next_timestamp(&transaction)?;
        let working_revision = state.working_revision.saturating_add(1);
        let working_version_id = new_uuid_v7();
        let mutation_id = new_uuid_v7();
        let outbox_transaction = begin_outbox_transaction(&transaction, 1)?;
        let canonical_hash = note_content_hash(&title, &body);
        transaction
            .execute(
                "INSERT INTO mobile_note_filing (
                   record_id, folder_id, previous_folder_id, filed_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(record_id) DO UPDATE SET
                   folder_id = excluded.folder_id,
                   previous_folder_id = excluded.previous_folder_id,
                   filed_at = excluded.filed_at,
                   updated_at = excluded.updated_at",
                params![record_id, target_folder_id, current_folder_id, timestamp],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_notes
                 SET updated_at = ?1,
                     working_revision = ?2,
                     working_version_id = ?3,
                     working_base_revision = accepted_revision,
                     pending_mutation_id = ?4,
                     sync_state = 'pending',
                     last_modified_device_id = ?5
                 WHERE record_id = ?6 AND lifecycle_state = 'active'",
                params![
                    timestamp,
                    working_revision,
                    working_version_id,
                    mutation_id,
                    identity.device_id,
                    record_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        enqueue_mutation(
            &transaction,
            &identity,
            &outbox_transaction,
            0,
            Mutation {
                operation: "update",
                patch_title_body: false,
                record_id,
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
                lifecycle_state: "active",
                trashed_at: None,
                tombstoned_at: None,
                created_at: state.created_at,
                updated_at: timestamp,
                authority: &state.authority,
                provenance_json: &state.provenance_json,
                scope_id: &state.scope_id,
                scope_class: &state.scope_class,
            },
        )?;
        attach_organization_payload(
            &transaction,
            &mutation_id,
            action,
            target_folder_id.as_deref(),
            current_folder_id.as_deref(),
        )?;
        let note = workspace_note_by_id(&transaction, record_id)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(note)
    }

    /// Recreates the disposable full-text index from authoritative note rows.
    /// This never changes a portable record, revision, or outbox mutation.
    #[allow(dead_code)]
    pub fn rebuild_search_index(&self) -> Result<(), String> {
        let mut connection = self.lock_connection()?;
        rebuild_mobile_search_schema(&mut connection)?;
        verify_mobile_search_schema(&connection)
    }

    /// Produces a deterministic, checksummed JSON backup containing portable
    /// UUID references only. SQLite row IDs and local filesystem paths are not
    /// part of this format.
    #[allow(dead_code)]
    pub fn export_notes(&self) -> Result<String, String> {
        let connection = self.lock_connection()?;
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

        let mut connection = self.lock_connection()?;
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
        transaction
            .execute(
                "UPDATE mobile_note_folders
                 SET library_id = ?1",
                [&envelope.payload.replica.library_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_note_categories
                 SET library_id = ?1",
                [&envelope.payload.replica.library_id],
            )
            .map_err(|error| error.to_string())?;
        // v1 exports predate exact canonical envelopes. Restore their portable
        // projections, then deterministically rebuild v7 canonical state in
        // this same transaction; the export path never pretends those bytes
        // were authority-authenticated historical records.
        transaction
            .execute("DELETE FROM mobile_canonical_record_v1", [])
            .map_err(|error| error.to_string())?;
        backfill_canonical_records_v7(&transaction)?;
        validate_replica_identity(&replica_identity(&transaction)?)?;
        validate_portable_notes(&transaction)?;
        validate_outbox_transaction_groups(&transaction)?;
        validate_restored_export_links(&transaction)?;
        transaction.commit().map_err(|error| error.to_string())?;
        verify_mobile_search_schema(&connection)?;
        Ok(envelope.payload.notes.len())
    }

    /// Commits the only durable transition from a staged phone to a paired,
    /// enrolled replica. Native code first performs its idempotent Keychain
    /// activation, then supplies the resulting exact Active checkpoint here.
    pub fn finalize_pairing_activation(
        &self,
        activation: &MobilePairingActivation,
    ) -> Result<MobilePairingActivationResult, String> {
        validate_mobile_pairing_activation(activation)?;
        let (activation_json, activation_sha256, scopes_json, capabilities_json) =
            serialized_mobile_pairing_activation(activation)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;

        if let Some(stored) = load_mobile_pairing_activation(&transaction)? {
            if stored.activation != *activation
                || stored.activation_json != activation_json
                || stored.activation_sha256 != activation_sha256
            {
                return Err(
                    "byte-different mobile pairing activation replay was rejected".to_string(),
                );
            }
            verify_mobile_pairing_activation_schema(&transaction)?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobilePairingActivationResult {
                adopted_note_count: stored.adopted_note_count,
                replayed: true,
            });
        }

        let pending = load_mobile_pairing_checkpoint(&transaction)?.ok_or_else(|| {
            "pairing activation has no durable PendingActivation checkpoint".to_string()
        })?;
        if !pending_checkpoint_precedes_activation(&pending, &activation.checkpoint) {
            return Err(
                "pairing activation does not exactly advance its durable PendingActivation checkpoint"
                    .to_string(),
            );
        }
        let identity = replica_identity(&transaction)?;
        if identity.library_state != "local_staging" || identity.device_id != activation.device_id {
            return Err(
                "pairing activation requires the matching untouched local_staging replica"
                    .to_string(),
            );
        }
        let enrollment_state: String = transaction
            .query_row(
                "SELECT enrollment_state FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if enrollment_state != "not_enrolled" {
            return Err(
                "pairing activation requires an untouched not_enrolled sync state".to_string(),
            );
        }

        let adopted_note_count = adopt_staging_for_pairing_activation(
            &transaction,
            &activation.library_id,
            &activation.default_scope_id,
        )?;
        activate_sync_enrollment_in_transaction(&transaction, activation)?;
        let finalized_at = next_timestamp(&transaction)?.max(activation.checkpoint.updated_at);
        let adopted_note_count_i64 = i64::try_from(adopted_note_count)
            .map_err(|_| "adopted note count exceeds SQLite range".to_string())?;
        transaction
            .execute(
                "INSERT INTO mobile_pairing_activation_v1 (
                   singleton, fixture_class, receipt_id, library_id, device_id,
                   default_scope_id, authority_generation, purge_generation, key_epoch,
                   sync_spki_sha256, record_cipher_suite, granted_scopes_json,
                   capabilities_json, activation_json, activation_sha256,
                   adopted_note_count, finalized_at
                 ) VALUES (
                   1, 'sanitized_fixture', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                   ?9, ?10, ?11, ?12, ?13, ?14, ?15
                 )",
                params![
                    activation.receipt_id,
                    activation.library_id,
                    activation.device_id,
                    activation.default_scope_id,
                    activation.authority_generation,
                    activation.purge_generation,
                    activation.key_epoch,
                    activation.sync_spki_sha256,
                    activation.record_cipher_suite,
                    scopes_json,
                    capabilities_json,
                    activation_json,
                    activation_sha256,
                    adopted_note_count_i64,
                    finalized_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        write_mobile_pairing_checkpoint(&transaction, &activation.checkpoint)?;
        verify_mobile_pairing_activation_schema(&transaction)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobilePairingActivationResult {
            adopted_note_count,
            replayed: false,
        })
    }

    pub fn finalized_pairing_activation(&self) -> Result<Option<MobilePairingActivation>, String> {
        let connection = self.lock_connection()?;
        let activation = load_mobile_pairing_activation(&connection)?;
        verify_mobile_pairing_activation_schema(&connection)?;
        Ok(activation.map(|value| value.activation))
    }

    pub fn authority_revocation(&self) -> Result<Option<MobileAuthorityRevocation>, String> {
        let connection = self.lock_connection()?;
        let Some(activation) = load_mobile_pairing_activation(&connection)? else {
            return Ok(None);
        };
        let revocation = load_mobile_authority_revocation_by_activation(
            &connection,
            &activation.activation_sha256,
        )?;
        Ok(revocation.map(|stored| stored.public))
    }

    /// Combines the durable checkpoint with native Keychain inventory so the
    /// runtime can distinguish the one crash-recovery window from completion.
    pub fn pairing_activation_health(
        &self,
        native_activation_is_active: bool,
    ) -> Result<MobilePairingActivationHealth, String> {
        let connection = self.lock_connection()?;
        let checkpoint = load_mobile_pairing_checkpoint(&connection)?;
        let activation = load_mobile_pairing_activation(&connection)?;
        verify_mobile_pairing_activation_schema(&connection)?;
        let identity = replica_identity(&connection)?;
        let enrollment_state: String = connection
            .query_row(
                "SELECT enrollment_state FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let phase = if activation.is_some() {
            "finalized"
        } else if checkpoint
            .as_ref()
            .is_some_and(|value| value.client.state == PairingClientState::PendingActivation)
        {
            if native_activation_is_active {
                "native_active_pending_finalize"
            } else {
                "pending_native_activation"
            }
        } else if native_activation_is_active {
            "native_active_without_pending_checkpoint"
        } else if checkpoint.is_some() {
            "pairing"
        } else {
            "not_started"
        };
        let receipt_id = activation
            .as_ref()
            .map(|value| value.activation.receipt_id.clone())
            .or_else(|| {
                checkpoint
                    .as_ref()
                    .and_then(|value| pairing_checkpoint_mirrors(value).ok()?.receipt_id)
            });
        Ok(MobilePairingActivationHealth {
            phase: phase.to_string(),
            database_finalized: activation.is_some(),
            receipt_id,
            library_state: identity.library_state,
            enrollment_state,
        })
    }

    /// Attach an unpaired phone's staging records to the library and default
    /// scope proven by the first pairing handshake. Record IDs are retained;
    /// the staging-only scope is remapped to the Mac's canonical scope ID.
    #[allow(dead_code)]
    fn adopt_staging_library(
        &self,
        mac_library_id: &str,
        mac_default_scope_id: &str,
    ) -> Result<usize, String> {
        if !is_uuid(mac_library_id) || !is_uuid(mac_default_scope_id) {
            return Err(
                "paired Mac library_id and default scope_id must be canonical UUIDs".to_string(),
            );
        }

        let mut connection = self.lock_connection()?;
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
            authority: String,
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
                            , authority
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
                        authority: row.get(15)?,
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
        transaction
            .execute(
                "UPDATE mobile_note_folders
                 SET library_id = ?1
                 WHERE library_id = ?2",
                params![mac_library_id, staging_identity.library_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_note_categories
                 SET library_id = ?1
                 WHERE library_id = ?2",
                params![mac_library_id, staging_identity.library_id],
            )
            .map_err(|error| error.to_string())?;
        rebind_staging_canonical_records(
            &transaction,
            &staging_identity.library_id,
            mac_library_id,
            &staging_identity.default_scope_id,
            mac_default_scope_id,
        )?;
        let paired_identity = replica_identity(&transaction)?;

        struct PlannedAdoption {
            note_index: usize,
            scope_id: String,
            working_revision: i64,
            working_version_id: String,
            mutation_id: String,
            ciphertext_bytes: usize,
        }

        let mut plans = Vec::with_capacity(notes.len());
        for (note_index, note) in notes.iter().enumerate() {
            let scope_id = if note.scope_id == staging_identity.default_scope_id {
                mac_default_scope_id.to_string()
            } else {
                note.scope_id.clone()
            };
            let working_revision = note
                .working_revision
                .checked_add(1)
                .ok_or_else(|| "mobile note working revision overflowed".to_string())?;
            let working_version_id = new_uuid_v7();
            let mutation_id = new_uuid_v7();
            let payload_json = serialize_mutation_payload(
                &paired_identity,
                &Mutation {
                    operation: "create",
                    patch_title_body: false,
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
                    authority: &note.authority,
                    provenance_json: &note.provenance_json,
                    scope_id: &scope_id,
                    scope_class: &note.scope_class,
                },
            )?;
            plans.push(PlannedAdoption {
                note_index,
                scope_id,
                working_revision,
                working_version_id,
                mutation_id,
                ciphertext_bytes: payload_json
                    .len()
                    .checked_add(MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES)
                    .ok_or_else(|| "mobile note mutation ciphertext size overflowed".to_string())?,
            });
        }

        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_bytes = 0usize;
        for (plan_index, plan) in plans.iter().enumerate() {
            let plan_bytes = plan.ciphertext_bytes;
            let needs_new_group = groups.last().is_some_and(|group| {
                group.len() >= MAX_MOBILE_TRANSACTION_MEMBERS
                    || group_bytes
                        .checked_add(plan_bytes)
                        .is_none_or(|total| total > MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES)
            });
            if groups.is_empty() || needs_new_group {
                groups.push(Vec::new());
                group_bytes = 0;
            }
            groups.last_mut().expect("group exists").push(plan_index);
            group_bytes = group_bytes
                .checked_add(plan_bytes)
                .ok_or_else(|| "mobile outbox transaction size overflowed".to_string())?;
        }

        for group in groups {
            let outbox_transaction = begin_outbox_transaction(&transaction, group.len())?;
            for (transaction_member_index, plan_index) in group.into_iter().enumerate() {
                let plan = &plans[plan_index];
                let note = &notes[plan.note_index];
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
                            plan.working_revision,
                            plan.working_version_id,
                            plan.mutation_id,
                            plan.scope_id,
                            note.id
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                enqueue_mutation(
                    &transaction,
                    &paired_identity,
                    &outbox_transaction,
                    i64::try_from(transaction_member_index)
                        .map_err(|_| "outbox transaction has too many members".to_string())?,
                    Mutation {
                        operation: "create",
                        patch_title_body: false,
                        record_id: &note.record_id,
                        title: &note.title,
                        body: &note.body,
                        base_revision: 0,
                        proposed_revision: 1,
                        local_revision: plan.working_revision,
                        version_id: &plan.working_version_id,
                        branch_id: &note.working_branch_id,
                        base_version_id: None,
                        accepted_content_hash: None,
                        mutation_id: &plan.mutation_id,
                        canonical_hash: &note.canonical_hash,
                        lifecycle_state: &note.lifecycle_state,
                        trashed_at: note.trashed_at,
                        tombstoned_at: note.tombstoned_at,
                        created_at: note.created_at,
                        updated_at: note.updated_at,
                        authority: &note.authority,
                        provenance_json: &note.provenance_json,
                        scope_id: &plan.scope_id,
                        scope_class: &note.scope_class,
                    },
                )?;
            }
        }

        transaction.commit().map_err(|error| error.to_string())?;
        Ok(notes.len())
    }

    /// Applies one authenticated and decrypted direct-sync transaction. The
    /// inbox receipt is committed before domain rows change, and replay is
    /// byte-bound by sequence, transaction ID, and canonical digest.
    pub fn apply_inbox_change(
        &self,
        change: &MobileInboxChange,
    ) -> Result<MobileInboxApplyResult, String> {
        validate_mobile_inbox_change(change)?;
        let payload_json = serde_json::to_string(change).map_err(|error| error.to_string())?;
        if payload_json.len() > MAX_MOBILE_INBOX_BYTES {
            return Err("mobile sync inbox transaction exceeds the 4 MiB limit".to_string());
        }

        let mut connection = self.lock_connection()?;
        let received_at = now_millis()?;
        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            validate_inbox_authority(&transaction, change).map_err(InboxApplyError::into_string)?;
            let cursors: (i64, i64) = transaction
                .query_row(
                    "SELECT downloaded_cursor, applied_cursor
                     FROM mobile_sync_state WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| error.to_string())?;
            let existing: Option<(String, String, String)> = transaction
                .query_row(
                    "SELECT transaction_id, transaction_digest, state
                     FROM mobile_sync_inbox WHERE sequence = ?1",
                    [change.sequence],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if let Some((transaction_id, digest, state)) = existing {
                if transaction_id != change.transaction_id || digest != change.transaction_digest {
                    return Err(
                        "mobile sync sequence reuse changed authenticated bytes".to_string()
                    );
                }
                if state == "applied" || state == "quarantined" {
                    transaction.commit().map_err(|error| error.to_string())?;
                    return Ok(MobileInboxApplyResult {
                        sequence: change.sequence,
                        applied_count: 0,
                        conflict_count: 0,
                        state,
                    });
                }
                if change.sequence != cursors.1.saturating_add(1) || change.sequence > cursors.0 {
                    return Err(
                        "mobile sync replay is not the next unapplied transaction".to_string()
                    );
                }
            } else {
                if change.sequence != cursors.0.saturating_add(1)
                    || change.sequence != cursors.1.saturating_add(1)
                {
                    return Err("mobile sync inbox sequence is not contiguous".to_string());
                }
                transaction
                    .execute(
                        "INSERT INTO mobile_sync_inbox (
                           sequence, transaction_id, transaction_digest,
                           payload_json, state, received_at
                         ) VALUES (?1, ?2, ?3, ?4, 'received', ?5)",
                        params![
                            change.sequence,
                            change.transaction_id,
                            change.transaction_digest,
                            payload_json,
                            received_at
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "UPDATE mobile_sync_state
                         SET downloaded_cursor = ?1, sync_state = 'syncing',
                             last_error_code = NULL
                         WHERE singleton = 1",
                        [change.sequence],
                    )
                    .map_err(|error| error.to_string())?;
            }
            transaction.commit().map_err(|error| error.to_string())?;
        }

        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let changed = transaction
                .execute(
                    "UPDATE mobile_sync_inbox
                     SET state = 'applying', apply_started_at = ?1, error_code = NULL
                     WHERE sequence = ?2 AND state = 'received'
                       AND ?2 = (SELECT applied_cursor + 1
                                 FROM mobile_sync_state WHERE singleton = 1)",
                    params![now_millis()?, change.sequence],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("mobile sync inbox transaction cannot enter applying state".to_string());
            }
            transaction.commit().map_err(|error| error.to_string())?;
        }

        let apply_result = (|| -> Result<(usize, usize), InboxApplyError> {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            validate_inbox_authority(&transaction, change)?;
            apply_incoming_categories(&transaction, change)?;
            apply_incoming_folders(&transaction, change)?;
            let mut conflicts = 0_usize;
            for note in &change.notes {
                if apply_incoming_note(&transaction, change, note)? {
                    conflicts += 1;
                }
            }
            let applied_at = now_millis().map_err(InboxApplyError::operational)?;
            let changed_row = transaction
                .execute(
                    "UPDATE mobile_sync_inbox
                     SET state = 'applied', applied_at = ?1, error_code = NULL
                     WHERE sequence = ?2 AND state = 'applying'",
                    params![applied_at, change.sequence],
                )
                .map_err(|error| {
                    InboxApplyError::operational(format!(
                        "checkpoint applied inbox transaction: {error}"
                    ))
                })?;
            if changed_row != 1 {
                return Err(InboxApplyError::operational(
                    "mobile sync inbox lost its applying checkpoint",
                ));
            }
            let pending: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1
                     )",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    InboxApplyError::operational(format!(
                        "inspect pending mobile mutations: {error}"
                    ))
                })?;
            transaction
                .execute(
                    "UPDATE mobile_sync_state
                     SET applied_cursor = ?1,
                         sync_state = ?2,
                         last_synced_at = ?3,
                         last_error_code = NULL
                     WHERE singleton = 1 AND applied_cursor + 1 = ?1",
                    params![
                        change.sequence,
                        if conflicts > 0 {
                            "conflict"
                        } else if pending {
                            "pending"
                        } else {
                            "idle"
                        },
                        applied_at
                    ],
                )
                .map_err(|error| {
                    InboxApplyError::operational(format!(
                        "advance mobile sync apply cursor: {error}"
                    ))
                })?;
            transaction
                .commit()
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            Ok((
                change.categories.len() + change.folders.len() + change.notes.len(),
                conflicts,
            ))
        })();

        match apply_result {
            Ok((applied_count, conflict_count)) => Ok(MobileInboxApplyResult {
                sequence: change.sequence,
                applied_count,
                conflict_count,
                state: if conflict_count > 0 {
                    "conflict".to_string()
                } else {
                    "applied".to_string()
                },
            }),
            Err(InboxApplyError::Semantic(error)) => {
                quarantine_inbox_change(&mut connection, change.sequence, &error)?;
                Ok(MobileInboxApplyResult {
                    sequence: change.sequence,
                    applied_count: 0,
                    conflict_count: 0,
                    state: "quarantined".to_string(),
                })
            }
            Err(InboxApplyError::Operational(error)) => {
                return_inbox_change_to_received(&mut connection, change.sequence)?;
                Err(error)
            }
        }
    }

    /// Atomically publishes exact decrypted canonical records from one
    /// authority-accepted transaction together with their UI projections and
    /// both ordered cursors. Invalid records leave no inbox or projection
    /// residue; callers retain the authenticated wire evidence for quarantine.
    pub fn apply_canonical_pull_change(
        &self,
        change: &MobileCanonicalPullChange,
    ) -> Result<MobileInboxApplyResult, String> {
        let decoded = validate_canonical_pull_change(change)?;
        let payload_json = canonical_pull_evidence_json(change)?;
        if payload_json.len() > MAX_MOBILE_INBOX_BYTES {
            return Err("canonical pull evidence exceeds the 4 MiB inbox bound".to_string());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        validate_canonical_authority_binding(
            &transaction,
            &change.library_id,
            change.authority_generation,
            change.purge_generation,
        )?;
        let cursors: (i64, i64) = transaction
            .query_row(
                "SELECT downloaded_cursor, applied_cursor
                 FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let existing: Option<(String, String, String, String)> = transaction
            .query_row(
                "SELECT transaction_id, transaction_digest, payload_json, state
                 FROM mobile_sync_inbox WHERE sequence = ?1",
                [change.sequence],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((transaction_id, digest, stored_payload, state)) = existing {
            if transaction_id != change.transaction_id
                || digest != change.transaction_digest
                || stored_payload.as_bytes() != payload_json.as_bytes()
            {
                return Err(
                    "canonical pull sequence replay changed exact authenticated data".to_string(),
                );
            }
            if state == "applied" && cursors.1 >= change.sequence {
                transaction.commit().map_err(|error| error.to_string())?;
                return Ok(MobileInboxApplyResult {
                    sequence: change.sequence,
                    applied_count: 0,
                    conflict_count: 0,
                    state,
                });
            }
            return Err("canonical pull replay has an invalid durable state".to_string());
        }
        if cursors.0 != cursors.1 || change.sequence != cursors.1.saturating_add(1) {
            return Err("canonical pull sequence is not the next fully applied cursor".to_string());
        }
        let applied_at = now_millis()?;
        transaction
            .execute(
                "INSERT INTO mobile_sync_inbox (
                   sequence, transaction_id, transaction_digest, payload_json,
                   state, received_at, apply_started_at
                 ) VALUES (?1, ?2, ?3, ?4, 'applying', ?5, ?5)",
                params![
                    change.sequence,
                    change.transaction_id,
                    change.transaction_digest,
                    payload_json,
                    applied_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        let conflict_count =
            apply_canonical_record_set(&transaction, &decoded, &change.source_device_id)?;
        let changed_inbox = transaction
            .execute(
                "UPDATE mobile_sync_inbox
                 SET state = 'applied', applied_at = ?1
                 WHERE sequence = ?2 AND state = 'applying'",
                params![applied_at, change.sequence],
            )
            .map_err(|error| error.to_string())?;
        let pending: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let changed_cursor = transaction
            .execute(
                "UPDATE mobile_sync_state
                 SET downloaded_cursor = ?1, applied_cursor = ?1,
                     sync_state = ?2, last_synced_at = ?3, last_error_code = NULL
                 WHERE singleton = 1 AND downloaded_cursor = ?4 AND applied_cursor = ?4",
                params![
                    change.sequence,
                    if conflict_count > 0 {
                        "conflict"
                    } else if pending {
                        "pending"
                    } else {
                        "idle"
                    },
                    applied_at,
                    cursors.1,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed_inbox != 1 || changed_cursor != 1 {
            return Err("canonical pull could not publish its atomic cursor".to_string());
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileInboxApplyResult {
            sequence: change.sequence,
            applied_count: decoded.len(),
            conflict_count,
            state: if conflict_count > 0 {
                "conflict".to_string()
            } else {
                "applied".to_string()
            },
        })
    }

    /// Publishes exact decrypted bootstrap heads only after all opaque pages
    /// and the final checkpoint commitment are durable. Canonical rows,
    /// projections, page/checkpoint state, and cursor jump share one commit.
    pub fn apply_canonical_bootstrap_snapshot(
        &self,
        checkpoint_id: &str,
        snapshot: &MobileCanonicalBootstrapSnapshot,
    ) -> Result<MobileBootstrapApplyResult, String> {
        if !is_sha256(&snapshot.checkpoint_sha256)
            || snapshot.record_bytes.len() > MAX_MOBILE_BOOTSTRAP_CHANGES
        {
            return Err("canonical bootstrap snapshot is invalid".to_string());
        }
        let decoded = decode_canonical_record_set(&snapshot.record_bytes)?;
        let mut connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        let recovery = bootstrap_recovery(&connection, checkpoint_id)?
            .ok_or_else(|| "bootstrap checkpoint does not exist".to_string())?;
        validate_bootstrap_recovery(&recovery, &binding)?;
        if snapshot.checkpoint_sha256 != recovery.checkpoint.checkpoint_sha256 {
            return Err("canonical bootstrap is bound to another checkpoint".to_string());
        }
        if recovery.checkpoint.state == "applied" {
            return Ok(MobileBootstrapApplyResult {
                checkpoint_id: checkpoint_id.to_string(),
                final_cursor: recovery.checkpoint.high_water_cursor,
                applied_change_count: 0,
                applied_record_count: 0,
                conflict_count: 0,
                replayed: true,
            });
        }
        if recovery.checkpoint.state != "received"
            || recovery.pages.iter().any(|page| page.state != "received")
        {
            return Err("canonical bootstrap checkpoint is not ready for apply".to_string());
        }
        for record in &decoded {
            if record.library_id != binding.library_id {
                return Err("canonical bootstrap record belongs to another library".to_string());
            }
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let conflict_count =
            apply_canonical_record_set(&transaction, &decoded, &binding.device_id)?;
        let applied_at = now_millis()?;
        let changed_pages = transaction
            .execute(
                "UPDATE mobile_bootstrap_page_v1
                 SET state = 'applied', applied_at = ?1, error_code = NULL
                 WHERE checkpoint_id = ?2 AND state = 'received'",
                params![applied_at, checkpoint_id],
            )
            .map_err(|error| error.to_string())?;
        let changed_checkpoint = transaction
            .execute(
                "UPDATE mobile_bootstrap_checkpoint_v1
                 SET state = 'applied', applied_at = ?1, error_code = NULL
                 WHERE checkpoint_id = ?2 AND state = 'received'",
                params![applied_at, checkpoint_id],
            )
            .map_err(|error| error.to_string())?;
        let pending: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let changed_cursor = transaction
            .execute(
                "UPDATE mobile_sync_state
                 SET downloaded_cursor = ?1, applied_cursor = ?1,
                     sync_state = ?2, last_synced_at = ?3, last_error_code = NULL
                 WHERE singleton = 1 AND downloaded_cursor = ?4 AND applied_cursor = ?4",
                params![
                    recovery.checkpoint.high_water_cursor,
                    if conflict_count > 0 {
                        "conflict"
                    } else if pending {
                        "pending"
                    } else {
                        "idle"
                    },
                    applied_at,
                    recovery.checkpoint.start_cursor,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed_pages != recovery.pages.len() || changed_checkpoint != 1 || changed_cursor != 1 {
            return Err("canonical bootstrap publication was not atomic".to_string());
        }
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileBootstrapApplyResult {
            checkpoint_id: checkpoint_id.to_string(),
            final_cursor: recovery.checkpoint.high_water_cursor,
            applied_change_count: recovery.pages.len(),
            applied_record_count: decoded.len(),
            conflict_count,
            replayed: false,
        })
    }

    pub fn resolve_note_conflict(
        &self,
        record_id: &str,
        resolution: &str,
    ) -> Result<MobileWorkspaceNote, String> {
        if !is_uuid_v7(record_id) {
            return Err("note recordId must be a canonical UUIDv7".to_string());
        }
        if !matches!(resolution, "keepAsCopy" | "useRemote") {
            return Err("unsupported mobile note conflict resolution".to_string());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let identity = replica_identity(&transaction)?;
        let conflict = load_open_conflict(&transaction, record_id)?
            .ok_or_else(|| format!("note {record_id} has no open conflict"))?;
        let resolved_at = next_timestamp(&transaction)?;
        retire_resolved_conflict_outbox(&transaction, record_id, resolved_at)?;

        let returned_record_id = if resolution == "keepAsCopy" {
            create_conflict_copy(&transaction, &identity, &conflict)?
        } else {
            conflict.record_id.clone()
        };
        materialize_conflict_remote(&transaction, &identity, &conflict)?;
        promote_canonical_accepted_to_working(&transaction, &conflict.record_id)?;
        transaction
            .execute(
                "UPDATE mobile_note_conflicts
                 SET state = ?1, resolved_at = ?2
                 WHERE conflict_id = ?3 AND state = 'open'",
                params![
                    if resolution == "keepAsCopy" {
                        "kept_copy"
                    } else {
                        "used_remote"
                    },
                    resolved_at,
                    conflict.conflict_id
                ],
            )
            .map_err(|error| error.to_string())?;
        let pending: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_sync_state
                 SET sync_state = ?1, last_error_code = NULL
                 WHERE singleton = 1 AND enrollment_state = 'active'",
                [if pending { "pending" } else { "idle" }],
            )
            .map_err(|error| error.to_string())?;
        let result = workspace_note_by_id(&transaction, &returned_record_id)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }

    fn set_lifecycle(
        &self,
        record_id: &str,
        lifecycle: &str,
        operation: &str,
    ) -> Result<(), String> {
        let mut connection = self.lock_connection()?;
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
        ensure_no_open_note_conflict(&transaction, record_id)?;
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
                patch_title_body: false,
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
                authority: &state.authority,
                provenance_json: &state.provenance_json,
                scope_id: &state.scope_id,
                scope_class: &state.scope_class,
            },
        )?;
        transaction.commit().map_err(|error| error.to_string())
    }
}

impl MobileStore {
    /// Returns complete eligible outbox groups without exposing SQLite to the
    /// sync orchestrator. Local edit counters are intentionally metadata only;
    /// the signed-wire journal owns its separate contiguous counter.
    pub fn eligible_outbox_transaction_groups(
        &self,
        limit: usize,
    ) -> Result<Vec<MobileEligibleOutboxTransactionGroup>, String> {
        if limit == 0 || limit > MAX_MOBILE_ELIGIBLE_OUTBOX_GROUPS {
            return Err(format!(
                "eligible outbox group limit must be between 1 and {MAX_MOBILE_ELIGIBLE_OUTBOX_GROUPS}"
            ));
        }
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        validate_outbox_transaction_groups(&connection)?;
        let groups = connection
            .prepare(
                "SELECT transaction_id, device_transaction_counter, MIN(local_sequence)
                 FROM mobile_note_outbox
                 WHERE eligible_for_sync = 1 AND state = 'pending'
                 GROUP BY transaction_id, device_transaction_counter
                 ORDER BY device_transaction_counter, MIN(local_sequence)
                 LIMIT ?1",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([limit as i64], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        let mut result = Vec::with_capacity(groups.len());
        for (transaction_id, device_transaction_counter) in groups {
            type Row = (
                String,
                String,
                i64,
                i64,
                i64,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                i64,
                Option<String>,
                i64,
                i64,
                String,
                String,
                String,
                String,
                String,
                i64,
            );
            let rows = connection
                .prepare(
                    "SELECT mutation_id, transaction_id, device_transaction_counter,
                            transaction_member_index, transaction_member_count,
                            library_id, device_id, install_id, scope_id, scope_class,
                            record_id, record_kind, operation, base_revision, base_version_id,
                            proposed_revision, local_revision, branch_id, version_id,
                            canonical_hash, payload_json, state, attempts
                     FROM mobile_note_outbox
                     WHERE transaction_id = ?1 AND eligible_for_sync = 1
                       AND state = 'pending'
                     ORDER BY transaction_member_index",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([&transaction_id], |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                                row.get(6)?,
                                row.get(7)?,
                                row.get(8)?,
                                row.get(9)?,
                                row.get(10)?,
                                row.get(11)?,
                                row.get(12)?,
                                row.get(13)?,
                                row.get(14)?,
                                row.get(15)?,
                                row.get(16)?,
                                row.get(17)?,
                                row.get(18)?,
                                row.get(19)?,
                                row.get(20)?,
                                row.get(21)?,
                                row.get(22)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<Row>>>()
                })
                .map_err(|error| error.to_string())?;
            let expected_count = rows.first().map(|row| row.4).unwrap_or(0);
            if rows.is_empty()
                || rows.len() > MAX_MOBILE_TRANSACTION_MEMBERS
                || i64::try_from(rows.len()).ok() != Some(expected_count)
            {
                return Err("eligible outbox transaction group is incomplete".to_string());
            }
            let mut payload_bytes = 0usize;
            let mut mutations = Vec::with_capacity(rows.len());
            for row in rows {
                if row.1 != transaction_id
                    || row.2 != device_transaction_counter
                    || row.5 != binding.library_id
                    || row.6 != binding.device_id
                {
                    return Err(
                        "eligible outbox transaction is not bound to the active replica"
                            .to_string(),
                    );
                }
                let bytes = row.20.into_bytes();
                payload_bytes = payload_bytes
                    .checked_add(bytes.len() + MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES)
                    .ok_or_else(|| "eligible outbox payload size overflowed".to_string())?;
                let payload = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("decode eligible outbox payload: {error}"))?;
                mutations.push(MobileEligibleOutboxMutation {
                    mutation_id: row.0,
                    transaction_id: row.1,
                    device_transaction_counter: row.2,
                    transaction_member_index: row.3,
                    transaction_member_count: row.4,
                    library_id: row.5,
                    device_id: row.6,
                    install_id: row.7,
                    scope_id: row.8,
                    scope_class: row.9,
                    record_id: row.10,
                    record_kind: row.11,
                    operation: row.12,
                    base_revision: row.13,
                    base_version_id: row.14,
                    proposed_revision: row.15,
                    local_revision: row.16,
                    branch_id: row.17,
                    version_id: row.18,
                    canonical_hash: row.19,
                    payload_bytes: bytes,
                    payload,
                    state: row.21,
                    attempts: row.22,
                });
            }
            if payload_bytes > MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES {
                return Err("eligible outbox transaction exceeds its wire ceiling".to_string());
            }
            result.push(MobileEligibleOutboxTransactionGroup {
                transaction_id,
                device_transaction_counter,
                mutations,
            });
        }
        Ok(result)
    }

    /// Returns the exact canonical plaintext records that the native crypto
    /// layer must seal. The legacy shadow payload API above remains available
    /// only to old fixture tests and is never consulted here.
    pub fn eligible_canonical_outbox_transaction_groups(
        &self,
        limit: usize,
    ) -> Result<Vec<MobileCanonicalOutboxTransactionGroup>, String> {
        if limit == 0 || limit > MAX_MOBILE_ELIGIBLE_OUTBOX_GROUPS {
            return Err(format!(
                "eligible canonical outbox group limit must be between 1 and {MAX_MOBILE_ELIGIBLE_OUTBOX_GROUPS}"
            ));
        }
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        validate_outbox_transaction_groups(&connection)?;
        verify_mobile_canonical_records(&connection)?;
        let groups = connection
            .prepare(
                "SELECT transaction_id, device_transaction_counter, MIN(local_sequence)
                 FROM mobile_note_outbox
                 WHERE eligible_for_sync = 1 AND state = 'pending'
                 GROUP BY transaction_id, device_transaction_counter
                 ORDER BY device_transaction_counter, MIN(local_sequence)
                 LIMIT ?1",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([limit as i64], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        let mut result = Vec::with_capacity(groups.len());
        for (transaction_id, device_transaction_counter) in groups {
            type Row = (
                String,
                String,
                i64,
                i64,
                i64,
                String,
                String,
                String,
                String,
                i64,
                Option<String>,
                i64,
                String,
                Vec<u8>,
                String,
            );
            let rows = connection
                .prepare(
                    "SELECT outbox.mutation_id, outbox.transaction_id,
                            outbox.device_transaction_counter,
                            outbox.transaction_member_index,
                            outbox.transaction_member_count,
                            outbox.library_id, outbox.device_id, outbox.record_id,
                            outbox.record_kind, outbox.base_revision,
                            outbox.base_version_id, outbox.proposed_revision,
                            outbox.version_id, canonical.working_record_json,
                            canonical.working_record_sha256
                     FROM mobile_note_outbox AS outbox
                     JOIN mobile_canonical_record_v1 AS canonical USING (record_id)
                     WHERE outbox.transaction_id = ?1
                       AND outbox.eligible_for_sync = 1 AND outbox.state = 'pending'
                     ORDER BY outbox.transaction_member_index",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([&transaction_id], |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                                row.get(6)?,
                                row.get(7)?,
                                row.get(8)?,
                                row.get(9)?,
                                row.get(10)?,
                                row.get(11)?,
                                row.get(12)?,
                                row.get(13)?,
                                row.get(14)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<Row>>>()
                })
                .map_err(|error| error.to_string())?;
            let expected_count = rows.first().map(|row| row.4).unwrap_or(0);
            if rows.is_empty()
                || rows.len() > MAX_MOBILE_TRANSACTION_MEMBERS
                || i64::try_from(rows.len()).ok() != Some(expected_count)
            {
                return Err("eligible canonical outbox transaction is incomplete".to_string());
            }
            let mut byte_total = 0usize;
            let mut mutations = Vec::with_capacity(rows.len());
            for row in rows {
                if row.1 != transaction_id
                    || row.2 != device_transaction_counter
                    || row.5 != binding.library_id
                    || row.6 != binding.device_id
                    || exact_sha256(&row.13) != row.14
                {
                    return Err(
                        "eligible canonical outbox transaction is not bound to the active replica"
                            .to_string(),
                    );
                }
                let record = decode_exact_canonical_context_record(&row.13)?;
                if record.library_id != row.5
                    || record.record_id != row.7
                    || record.kind != row.8
                    || record.revision != row.11 as u64
                    || record.version_id != row.12
                    || !matches!(record.authority.kind, AuthorityKind::Noted)
                {
                    return Err(
                        "eligible canonical outbox record does not match its mutation".to_string(),
                    );
                }
                byte_total = byte_total
                    .checked_add(row.13.len() + MOBILE_CANONICAL_RECORD_CIPHERTEXT_OVERHEAD_BYTES)
                    .ok_or_else(|| "canonical outbox byte size overflowed".to_string())?;
                mutations.push(MobileCanonicalOutboxMutation {
                    mutation_id: row.0,
                    transaction_id: row.1,
                    device_transaction_counter: row.2,
                    transaction_member_index: row.3,
                    transaction_member_count: row.4,
                    library_id: row.5,
                    device_id: row.6,
                    record_id: row.7,
                    record_kind: row.8,
                    operation: if row.9 == 0 {
                        "create".to_string()
                    } else if matches!(record.lifecycle.state, LifecycleState::Tombstone) {
                        "delete".to_string()
                    } else {
                        "update".to_string()
                    },
                    base_revision: row.9,
                    base_version_id: row.10,
                    proposed_revision: row.11,
                    version_id: row.12,
                    proposed_record_bytes: row.13,
                    proposed_record_sha256: row.14,
                });
            }
            if byte_total > MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES {
                return Err("canonical outbox transaction exceeds its wire ceiling".to_string());
            }
            result.push(MobileCanonicalOutboxTransactionGroup {
                transaction_id,
                device_transaction_counter,
                mutations,
            });
        }
        Ok(result)
    }

    pub fn canonical_record(
        &self,
        record_id: &str,
    ) -> Result<Option<MobileCanonicalRecord>, String> {
        if !is_uuid_v7(record_id) {
            return Err("canonical record id must be a UUIDv7".to_string());
        }
        let connection = self.lock_connection()?;
        verify_mobile_canonical_records(&connection)?;
        connection
            .query_row(
                "SELECT record_id, library_id, record_kind,
                        accepted_record_json, accepted_record_sha256,
                        working_record_json, working_record_sha256,
                        backfill_provenance
                 FROM mobile_canonical_record_v1 WHERE record_id = ?1",
                [record_id],
                |row| {
                    Ok(MobileCanonicalRecord {
                        record_id: row.get(0)?,
                        library_id: row.get(1)?,
                        record_kind: row.get(2)?,
                        accepted_record_bytes: row.get(3)?,
                        accepted_record_sha256: row.get(4)?,
                        working_record_bytes: row.get(5)?,
                        working_record_sha256: row.get(6)?,
                        backfill_provenance: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    /// Returns `(downloaded_cursor, applied_cursor)` from the same durable
    /// state advanced by canonical pull and bootstrap publication.
    pub fn canonical_sync_cursors(&self) -> Result<(i64, i64), String> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT downloaded_cursor, applied_cursor
                 FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())
    }

    /// Durable bootstrap completion marker independent of the cursor value;
    /// an empty library legitimately completes bootstrap at high-water zero.
    pub fn canonical_initial_bootstrap_applied(&self) -> Result<bool, String> {
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_bootstrap_checkpoint_v1
                   WHERE state = 'applied'
                     AND receipt_id = ?1 AND activation_sha256 = ?2
                     AND library_id = ?3 AND device_id = ?4
                     AND authority_generation = ?5 AND purge_generation = ?6
                     AND key_epoch = ?7 AND sync_spki_sha256 = ?8
                 )",
                params![
                    binding.receipt_id,
                    binding.activation_sha256,
                    binding.library_id,
                    binding.device_id,
                    binding.authority_generation,
                    binding.purge_generation,
                    binding.key_epoch,
                    binding.sync_spki_sha256,
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    /// The next authoritative push counter. Pull, bootstrap, checkpoint, and
    /// ack requests deliberately do not consume this sequence: the authority
    /// requires contiguous counters only for signed push transactions.
    pub fn next_direct_sync_push_counter(&self) -> Result<i64, String> {
        let connection = self.lock_connection()?;
        active_direct_sync_binding(&connection)?;
        connection
            .query_row(
                "SELECT next_counter FROM mobile_direct_sync_push_counter_v1 WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    /// Atomically persists exact signed bytes before transport. Push requests
    /// also claim their authoritative counter and bind it to one complete,
    /// still-eligible outbox transaction. Exact replay is idempotent.
    pub fn prepare_direct_sync_request(
        &self,
        draft: &MobileDirectSyncRequestDraft,
    ) -> Result<MobileDirectSyncPrepareResult, String> {
        let is_push = draft.endpoint == "/sync/v1/push";
        if !is_uuid_v7(&draft.request_id)
            || !valid_direct_sync_endpoint(&draft.endpoint)
            || !valid_direct_sync_operation(&draft.endpoint, &draft.operation)
            || is_push != draft.push_transaction_id.is_some()
            || is_push != draft.push_counter.is_some()
            || draft
                .push_transaction_id
                .as_deref()
                .is_some_and(|value| !is_uuid_v7(value))
            || draft.push_counter.is_some_and(|value| value <= 0)
            || draft.signed_request_bytes.is_empty()
            || draft.signed_request_bytes.len() > MAX_MOBILE_DIRECT_SYNC_REQUEST_BYTES
        {
            return Err("mobile direct-sync signed request is invalid or oversized".to_string());
        }
        validate_direct_sync_purpose(
            &draft.purpose_json,
            &draft.endpoint,
            &draft.operation,
            draft.push_transaction_id.as_deref(),
            draft.push_counter,
        )?;
        let purpose_sha256 = exact_sha256(&draft.purpose_json);
        let request_sha256 = exact_sha256(&draft.signed_request_bytes);
        let added_request_bytes = draft
            .purpose_json
            .len()
            .checked_add(draft.signed_request_bytes.len())
            .ok_or_else(|| "mobile direct-sync request byte count overflowed".to_string())?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        if let Some(existing) =
            load_direct_sync_request(&transaction, &draft.request_id, &draft.endpoint)?
        {
            validate_direct_sync_request_row(&existing, &binding)?;
            if existing.operation != draft.operation
                || existing.purpose_json != draft.purpose_json
                || existing.purpose_sha256 != purpose_sha256
                || existing.push_transaction_id != draft.push_transaction_id
                || existing.push_counter != draft.push_counter
                || existing.request_bytes != draft.signed_request_bytes
                || existing.request_sha256 != request_sha256
            {
                return Err("byte-different direct-sync request replay was rejected".to_string());
            }
            if let Some(transaction_id) = existing.push_transaction_id.as_deref() {
                let push_binding = load_direct_sync_push_binding(&transaction, transaction_id)?
                    .ok_or_else(|| {
                        "direct-sync push request lost its durable outbox binding".to_string()
                    })?;
                validate_direct_sync_push_binding(&push_binding, &binding)?;
                if push_binding.request_id != existing.request_id
                    || push_binding.push_counter != existing.push_counter.unwrap_or_default()
                    || push_binding.request_sha256 != existing.request_sha256
                {
                    return Err("direct-sync push request binding is inconsistent".to_string());
                }
            }
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobileDirectSyncPrepareResult {
                request: existing,
                replayed: true,
            });
        }
        let mut counts: (i64, i64, i64) = transaction
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(length(purpose_json) + length(request_bytes)
                                     + COALESCE(length(response_bytes), 0)), 0),
                        COALESCE(SUM(CASE WHEN state IN ('pending', 'response_received') THEN 1 ELSE 0 END), 0)
                 FROM mobile_direct_sync_request_v1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        if counts.0 >= MAX_MOBILE_DIRECT_SYNC_ROWS
            || counts
                .1
                .checked_add(added_request_bytes as i64)
                .is_none_or(|bytes| bytes > MAX_MOBILE_DIRECT_SYNC_TOTAL_BYTES)
        {
            prune_completed_direct_sync_in_transaction(&transaction, 256)?;
            counts = transaction
                .query_row(
                    "SELECT COUNT(*),
                            COALESCE(SUM(length(purpose_json) + length(request_bytes)
                                         + COALESCE(length(response_bytes), 0)), 0),
                            COALESCE(SUM(CASE WHEN state IN ('pending', 'response_received') THEN 1 ELSE 0 END), 0)
                     FROM mobile_direct_sync_request_v1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| error.to_string())?;
        }
        if counts.0 >= MAX_MOBILE_DIRECT_SYNC_ROWS
            || counts.2 >= MAX_MOBILE_DIRECT_SYNC_OPEN_ROWS
            || counts
                .1
                .checked_add(added_request_bytes as i64)
                .is_none_or(|bytes| bytes > MAX_MOBILE_DIRECT_SYNC_TOTAL_BYTES)
        {
            return Err("mobile direct-sync journal reached its durable bound".to_string());
        }
        if let (Some(transaction_id), Some(push_counter)) =
            (draft.push_transaction_id.as_deref(), draft.push_counter)
        {
            validate_outbox_transaction_groups(&transaction)?;
            let group: Option<(i64, i64, i64, String, String)> = transaction
                .query_row(
                    "SELECT COUNT(*), MIN(transaction_member_count),
                            MAX(transaction_member_count), MIN(library_id), MIN(device_id)
                     FROM mobile_note_outbox
                     WHERE transaction_id = ?1 AND eligible_for_sync = 1
                       AND state = 'pending'
                     HAVING COUNT(*) > 0",
                    [transaction_id],
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
                .optional()
                .map_err(|error| error.to_string())?;
            let Some((member_count, minimum_count, maximum_count, library_id, device_id)) = group
            else {
                return Err("push request does not name an eligible outbox transaction".to_string());
            };
            if member_count != minimum_count
                || member_count != maximum_count
                || library_id != binding.library_id
                || device_id != binding.device_id
            {
                return Err("push request outbox transaction is incomplete or misbound".to_string());
            }
            let next_counter: i64 = transaction
                .query_row(
                    "SELECT next_counter FROM mobile_direct_sync_push_counter_v1 WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if push_counter != next_counter {
                return Err(format!(
                    "signed push counter changed before journal prepare; expected {next_counter}"
                ));
            }
        }
        let created_at = now_millis()?;
        transaction
            .execute(
                "INSERT INTO mobile_direct_sync_request_v1 (
                   request_id, endpoint, operation, purpose_json, purpose_sha256,
                   push_transaction_id, push_counter,
                   receipt_id, activation_sha256, library_id, device_id,
                   authority_generation, purge_generation, key_epoch, sync_spki_sha256,
                   request_bytes, request_sha256, request_content_type,
                   state, attempts, created_at, updated_at
                 ) VALUES (
                   ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                   ?13, ?14, ?15, ?16, ?17, 'application/json', 'pending', 0, ?18, ?18
                 )",
                params![
                    draft.request_id,
                    draft.endpoint,
                    draft.operation,
                    draft.purpose_json,
                    purpose_sha256,
                    draft.push_transaction_id,
                    draft.push_counter,
                    binding.receipt_id,
                    binding.activation_sha256,
                    binding.library_id,
                    binding.device_id,
                    binding.authority_generation,
                    binding.purge_generation,
                    binding.key_epoch,
                    binding.sync_spki_sha256,
                    draft.signed_request_bytes,
                    request_sha256,
                    created_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        if let Some(push_counter) = draft.push_counter {
            let transaction_id = draft
                .push_transaction_id
                .as_deref()
                .expect("validated push transaction id");
            transaction
                .execute(
                    "INSERT INTO mobile_direct_sync_push_binding_v1 (
                       transaction_id, request_id, push_counter, request_sha256,
                       receipt_id, activation_sha256, library_id, device_id,
                       authority_generation, purge_generation, key_epoch,
                       sync_spki_sha256, state, created_at, updated_at
                     ) VALUES (
                       ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                       'sending', ?13, ?13
                     )",
                    params![
                        transaction_id,
                        draft.request_id,
                        push_counter,
                        request_sha256,
                        binding.receipt_id,
                        binding.activation_sha256,
                        binding.library_id,
                        binding.device_id,
                        binding.authority_generation,
                        binding.purge_generation,
                        binding.key_epoch,
                        binding.sync_spki_sha256,
                        created_at,
                    ],
                )
                .map_err(|error| error.to_string())?;
            let advanced = transaction
                .execute(
                    "UPDATE mobile_direct_sync_push_counter_v1
                     SET next_counter = next_counter + 1
                     WHERE singleton = 1 AND next_counter = ?1",
                    [push_counter],
                )
                .map_err(|error| error.to_string())?;
            if advanced != 1 {
                return Err("direct-sync push counter reservation was not atomic".to_string());
            }
            let expected_members: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM mobile_note_outbox
                     WHERE transaction_id = ?1 AND eligible_for_sync = 1 AND state = 'pending'",
                    [transaction_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            let claimed = transaction
                .execute(
                    "UPDATE mobile_note_outbox SET state = 'sending'
                     WHERE transaction_id = ?1 AND eligible_for_sync = 1 AND state = 'pending'",
                    [transaction_id],
                )
                .map_err(|error| error.to_string())?;
            if claimed as i64 != expected_members || claimed == 0 {
                return Err("push outbox transaction could not be claimed atomically".to_string());
            }
        }
        let request = load_direct_sync_request(&transaction, &draft.request_id, &draft.endpoint)?
            .ok_or_else(|| "prepared direct-sync request disappeared".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPrepareResult {
            request,
            replayed: false,
        })
    }

    pub fn record_direct_sync_attempt(
        &self,
        request_id: &str,
        endpoint: &str,
    ) -> Result<MobileDirectSyncRequest, String> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        let request = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync request does not exist".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        if request.state != "pending" {
            return Err("only a pending direct-sync request may start transport".to_string());
        }
        if request.attempts >= MAX_MOBILE_DIRECT_SYNC_ATTEMPTS {
            transaction.commit().map_err(|error| error.to_string())?;
            return Err(
                "direct-sync retry limit reached; exact request remains pending for manual recovery"
                    .to_string(),
            );
        }
        let attempted_at = now_millis()?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET attempts = attempts + 1, last_attempt_at = ?1,
                     updated_at = ?1, error_code = NULL
                 WHERE request_id = ?2 AND endpoint = ?3 AND state = 'pending'",
                params![attempted_at, request_id, endpoint],
            )
            .map_err(|error| error.to_string())?;
        let result = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync request disappeared after attempt".to_string())?;
        validate_direct_sync_request_row(&result, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }

    /// Persists exact response bytes before any semantic processing. The first
    /// response wins; an identical replay is idempotent and a different replay
    /// permanently quarantines the request while retaining the first bytes.
    pub fn record_direct_sync_response(
        &self,
        request_id: &str,
        endpoint: &str,
        response_status: u16,
        response_content_type: &str,
        response_bytes: &[u8],
    ) -> Result<MobileDirectSyncPrepareResult, String> {
        if !(100..=599).contains(&response_status)
            || response_content_type != "application/json"
            || response_bytes.is_empty()
            || response_bytes.len() > MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES
        {
            return Err("direct-sync response is empty or oversized".to_string());
        }
        let response_sha256 = exact_sha256(response_bytes);
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        let request = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync request does not exist".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        if let Some(stored_bytes) = request.response_bytes.as_deref() {
            if request.response_status == Some(i64::from(response_status))
                && request.response_content_type.as_deref() == Some(response_content_type)
                && stored_bytes == response_bytes
                && request.response_sha256.as_deref() == Some(response_sha256.as_str())
            {
                transaction.commit().map_err(|error| error.to_string())?;
                return Ok(MobileDirectSyncPrepareResult {
                    request,
                    replayed: true,
                });
            }
            if request.state != "completed" && request.state != "quarantined" {
                let quarantined_at = now_millis()?;
                transaction
                    .execute(
                        "UPDATE mobile_direct_sync_request_v1
                         SET state = 'quarantined', quarantined_at = ?1,
                             updated_at = ?1, error_code = 'response_replay_mismatch'
                         WHERE request_id = ?2 AND endpoint = ?3",
                        params![quarantined_at, request_id, endpoint],
                    )
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(|error| error.to_string())?;
            }
            return Err("byte-different direct-sync response replay was quarantined".to_string());
        }
        if request.state != "pending" {
            return Err("direct-sync response cannot change a terminal request".to_string());
        }
        let current_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(purpose_json) + length(request_bytes)
                                      + COALESCE(length(response_bytes), 0)), 0)
                 FROM mobile_direct_sync_request_v1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if current_bytes
            .checked_add(response_bytes.len() as i64)
            .is_none_or(|bytes| bytes > MAX_MOBILE_DIRECT_SYNC_TOTAL_BYTES)
        {
            return Err("mobile direct-sync journal byte ceiling was reached".to_string());
        }
        let received_at = now_millis()?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET response_status = ?1, response_content_type = ?2,
                     response_bytes = ?3, response_sha256 = ?4,
                     state = 'response_received', response_received_at = ?5,
                     updated_at = ?5, error_code = NULL
                 WHERE request_id = ?6 AND endpoint = ?7 AND state = 'pending'",
                params![
                    i64::from(response_status),
                    response_content_type,
                    response_bytes,
                    response_sha256,
                    received_at,
                    request_id,
                    endpoint
                ],
            )
            .map_err(|error| error.to_string())?;
        let result = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync response journal row disappeared".to_string())?;
        validate_direct_sync_request_row(&result, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPrepareResult {
            request: result,
            replayed: false,
        })
    }

    /// Applies an authenticated authority revocation as one durable local
    /// transition. The exact response must already exist in the request
    /// journal. User-authored working branches remain in the Notes tables and
    /// export surface, while every network-capable queue is made terminal.
    pub fn apply_authority_revocation(
        &self,
        evidence: &MobileAuthorityRevocationEvidence,
    ) -> Result<MobileAuthorityRevocationResult, String> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = durable_direct_sync_binding(&transaction)?;

        if let Some(stored) =
            load_mobile_authority_revocation_by_request(&transaction, &evidence.request_id)?
        {
            if stored.public.activation_sha256 != binding.activation_sha256
                || stored.public.endpoint != evidence.endpoint
                || stored.response_bytes != evidence.exact_response_bytes
            {
                return Err("byte-different authority revocation replay was rejected".to_string());
            }
            verify_current_mobile_schema(&transaction)?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobileAuthorityRevocationResult {
                revocation: stored.public,
                retired_outbox_count: 0,
                quarantined_request_count: 0,
                replayed: true,
            });
        }
        if load_mobile_authority_revocation_by_activation(&transaction, &binding.activation_sha256)?
            .is_some()
        {
            return Err("authority revocation evidence cannot be rewritten".to_string());
        }

        let bound = validate_authenticated_revocation_evidence(&transaction, &binding, evidence)?;
        let response_status = bound
            .request
            .response_status
            .ok_or_else(|| "authority revocation response status disappeared".to_string())?;
        let response_sha256 = exact_sha256(&evidence.exact_response_bytes);
        let revoked_at = next_timestamp(&transaction)?;
        transaction
            .execute(
                "INSERT INTO mobile_authority_revocation_v1 (
                   activation_sha256, contract_version, receipt_id, library_id,
                   device_id, authority_generation, purge_generation, key_epoch,
                   sync_spki_sha256, request_id, endpoint, response_status,
                   evidence_kind, response_bytes, response_sha256, reason, revoked_at
                 ) VALUES (
                   ?1, 'noted.mobile-authority-revocation.v1', ?2, ?3, ?4,
                   ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                   'device_revoked', ?15
                 )",
                params![
                    binding.activation_sha256,
                    binding.receipt_id,
                    binding.library_id,
                    binding.device_id,
                    binding.authority_generation,
                    binding.purge_generation,
                    binding.key_epoch,
                    binding.sync_spki_sha256,
                    evidence.request_id,
                    evidence.endpoint,
                    response_status,
                    bound.evidence_kind,
                    evidence.exact_response_bytes,
                    response_sha256,
                    revoked_at,
                ],
            )
            .map_err(|error| error.to_string())?;

        transaction
            .execute(
                "UPDATE mobile_notes
                 SET sync_state = 'conflict'
                 WHERE EXISTS (
                   SELECT 1 FROM mobile_note_outbox AS outbox
                   WHERE outbox.record_id = mobile_notes.record_id
                     AND outbox.eligible_for_sync = 1
                 )",
                [],
            )
            .map_err(|error| error.to_string())?;
        let retired_outbox_count = transaction
            .execute(
                "UPDATE mobile_note_outbox
                 SET state = 'conflict', eligible_for_sync = 0,
                     superseded_at = COALESCE(superseded_at, ?1)
                 WHERE eligible_for_sync = 1",
                [revoked_at],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_push_binding_v1
                 SET state = 'rejected', updated_at = MAX(updated_at, ?1),
                     terminal_at = COALESCE(terminal_at, ?1),
                     error_code = 'device_revoked'
                 WHERE state IN ('sending', 'awaiting_echo')",
                [revoked_at],
            )
            .map_err(|error| error.to_string())?;
        let quarantined_request_count = transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET state = 'quarantined', quarantined_at = COALESCE(quarantined_at, ?1),
                     updated_at = MAX(updated_at, ?1), error_code = 'device_revoked'
                 WHERE state IN ('pending', 'response_received')",
                [revoked_at],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_bootstrap_page_v1
                 SET state = 'quarantined', quarantined_at = COALESCE(quarantined_at, ?1),
                     error_code = 'device_revoked'
                 WHERE state = 'received'",
                [revoked_at],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_bootstrap_checkpoint_v1
                 SET state = 'quarantined', terminal_at = COALESCE(terminal_at, ?1),
                     error_code = 'device_revoked'
                 WHERE state IN ('receiving', 'received')",
                [revoked_at],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_sync_inbox
                 SET state = 'quarantined', error_code = 'device_revoked'
                 WHERE state IN ('received', 'applying')",
                [],
            )
            .map_err(|error| error.to_string())?;
        let enrollment_changed = transaction
            .execute(
                "UPDATE mobile_sync_state
                 SET enrollment_state = 'revoked', sync_state = 'revoked',
                     last_error_code = 'device_revoked'
                 WHERE singleton = 1 AND enrollment_state = 'active'",
                [],
            )
            .map_err(|error| error.to_string())?;
        if enrollment_changed != 1 {
            return Err("authority revocation requires the active enrollment".to_string());
        }

        verify_current_mobile_schema(&transaction)?;
        let stored = load_mobile_authority_revocation_by_activation(
            &transaction,
            &binding.activation_sha256,
        )?
        .ok_or_else(|| "authority revocation did not commit its evidence".to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileAuthorityRevocationResult {
            revocation: stored.public,
            retired_outbox_count,
            quarantined_request_count,
            replayed: false,
        })
    }

    pub fn complete_direct_sync_request(
        &self,
        request_id: &str,
        endpoint: &str,
    ) -> Result<MobileDirectSyncPrepareResult, String> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        let request = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync request does not exist".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        if request.endpoint == "/sync/v1/push" {
            return Err(
                "push completion requires complete_direct_sync_push_request disposition"
                    .to_string(),
            );
        }
        if request.state == "completed" {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobileDirectSyncPrepareResult {
                request,
                replayed: true,
            });
        }
        if request.state != "response_received" {
            return Err("direct-sync request has no durable response to complete".to_string());
        }
        let completed_at = now_millis()?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET state = 'completed', completed_at = ?1, updated_at = ?1,
                     error_code = NULL
                 WHERE request_id = ?2 AND endpoint = ?3 AND state = 'response_received'",
                params![completed_at, request_id, endpoint],
            )
            .map_err(|error| error.to_string())?;
        let result = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "completed direct-sync request disappeared".to_string())?;
        validate_direct_sync_request_row(&result, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPrepareResult {
            request: result,
            replayed: false,
        })
    }

    /// Completes a push and its outbox disposition in the same transaction.
    /// Accepted groups move to the durable `awaiting_echo` binding (their
    /// legacy outbox rows remain `sending`) until a pull echo acknowledges
    /// them. Rejected/conflicted groups retain content but become ineligible.
    pub fn complete_direct_sync_push_request(
        &self,
        request_id: &str,
        disposition: MobileDirectSyncPushDisposition,
        error_code: Option<&str>,
    ) -> Result<MobileDirectSyncPrepareResult, String> {
        let accepted = disposition == MobileDirectSyncPushDisposition::AcceptedAwaitingEcho;
        if accepted != error_code.is_none()
            || error_code.is_some_and(|value| !valid_mobile_error_code(value))
        {
            return Err("push completion disposition is invalid".to_string());
        }
        let endpoint = "/sync/v1/push";
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        let request = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync push request does not exist".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        let transaction_id = request
            .push_transaction_id
            .as_deref()
            .ok_or_else(|| "direct-sync push request lost its outbox binding".to_string())?;
        let push_binding = load_direct_sync_push_binding(&transaction, transaction_id)?
            .ok_or_else(|| "direct-sync push request lost its lifecycle binding".to_string())?;
        validate_direct_sync_push_binding(&push_binding, &binding)?;
        let expected_binding_state = match disposition {
            MobileDirectSyncPushDisposition::AcceptedAwaitingEcho => "awaiting_echo",
            MobileDirectSyncPushDisposition::Conflict => "conflict",
            MobileDirectSyncPushDisposition::Rejected => "rejected",
        };
        if request.state == "completed" {
            let exact = if accepted {
                request.error_code.is_none()
                    && matches!(
                        push_binding.state.as_str(),
                        "awaiting_echo" | "acknowledged"
                    )
            } else {
                request.error_code.as_deref() == error_code
                    && push_binding.state == expected_binding_state
            };
            if !exact {
                return Err("push completion disposition cannot be rewritten".to_string());
            }
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobileDirectSyncPrepareResult {
                request,
                replayed: true,
            });
        }
        if request.state != "response_received" {
            return Err("direct-sync push has no durable response to complete".to_string());
        }
        let group: (i64, i64, i64) = transaction
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN state = 'sending' AND eligible_for_sync = 1 THEN 1 ELSE 0 END), 0),
                        COALESCE(MAX(transaction_member_count), 0)
                 FROM mobile_note_outbox WHERE transaction_id = ?1",
                [&transaction_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        if group.0 == 0 || group.0 != group.1 || group.0 != group.2 {
            return Err("direct-sync push outbox claim is incomplete".to_string());
        }
        if !accepted {
            let changed = transaction
                .execute(
                    "UPDATE mobile_note_outbox
                     SET state = 'conflict', eligible_for_sync = 0
                     WHERE transaction_id = ?1 AND state = 'sending'
                       AND eligible_for_sync = 1",
                    [transaction_id],
                )
                .map_err(|error| error.to_string())?;
            if changed as i64 != group.0 {
                return Err(
                    "rejected direct-sync push did not retire its complete group".to_string(),
                );
            }
        }
        let completed_at = now_millis()?;
        let terminal_at = (!accepted).then_some(completed_at);
        let changed_binding = transaction
            .execute(
                "UPDATE mobile_direct_sync_push_binding_v1
                 SET state = ?1, updated_at = ?2, terminal_at = ?3, error_code = ?4
                 WHERE transaction_id = ?5 AND request_id = ?6 AND state = 'sending'",
                params![
                    expected_binding_state,
                    completed_at,
                    terminal_at,
                    error_code,
                    transaction_id,
                    request_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed_binding != 1 {
            return Err("direct-sync push lifecycle did not advance atomically".to_string());
        }
        let changed = transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET state = 'completed', completed_at = ?1, updated_at = ?1,
                     error_code = ?2
                 WHERE request_id = ?3 AND endpoint = ?4 AND state = 'response_received'",
                params![completed_at, error_code, request_id, endpoint],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("direct-sync push completion was not atomic".to_string());
        }
        let result = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "completed direct-sync push disappeared".to_string())?;
        validate_direct_sync_request_row(&result, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPrepareResult {
            request: result,
            replayed: false,
        })
    }

    pub fn quarantine_direct_sync_request(
        &self,
        request_id: &str,
        endpoint: &str,
        error_code: &str,
    ) -> Result<MobileDirectSyncPrepareResult, String> {
        if !valid_mobile_error_code(error_code) {
            return Err("direct-sync quarantine error code is invalid".to_string());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        let request = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "direct-sync request does not exist".to_string())?;
        validate_direct_sync_request_row(&request, &binding)?;
        if request.state == "quarantined" {
            if request.error_code.as_deref() != Some(error_code) {
                return Err("direct-sync quarantine reason cannot be rewritten".to_string());
            }
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(MobileDirectSyncPrepareResult {
                request,
                replayed: true,
            });
        }
        if request.state == "completed" {
            return Err("completed direct-sync request cannot be quarantined".to_string());
        }
        let quarantined_at = now_millis()?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_request_v1
                 SET state = 'quarantined', quarantined_at = ?1,
                     updated_at = ?1, error_code = ?2
                 WHERE request_id = ?3 AND endpoint = ?4",
                params![quarantined_at, error_code, request_id, endpoint],
            )
            .map_err(|error| error.to_string())?;
        let result = load_direct_sync_request(&transaction, request_id, endpoint)?
            .ok_or_else(|| "quarantined direct-sync request disappeared".to_string())?;
        validate_direct_sync_request_row(&result, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPrepareResult {
            request: result,
            replayed: false,
        })
    }

    /// Restart recovery returns only work that can still make progress. Exact
    /// response bytes are included so semantic processing never refetches.
    pub fn recover_direct_sync_requests(&self) -> Result<Vec<MobileDirectSyncRequest>, String> {
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        let keys = connection
            .prepare(
                "SELECT request_id, endpoint FROM mobile_direct_sync_request_v1
                 WHERE state IN ('pending', 'response_received')
                 ORDER BY local_sequence",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        if keys.len() as i64 > MAX_MOBILE_DIRECT_SYNC_OPEN_ROWS {
            return Err("mobile direct-sync recovery exceeds its row ceiling".to_string());
        }
        let mut requests = Vec::with_capacity(keys.len());
        for (request_id, endpoint) in keys {
            let request = load_direct_sync_request(&connection, &request_id, &endpoint)?
                .ok_or_else(|| "direct-sync recovery row disappeared".to_string())?;
            validate_direct_sync_request_row(&request, &binding)?;
            requests.push(request);
        }
        Ok(requests)
    }

    pub fn direct_sync_push_binding(
        &self,
        transaction_id: &str,
    ) -> Result<Option<MobileDirectSyncPushBinding>, String> {
        if !is_uuid(transaction_id) {
            return Err("direct-sync push transaction id is invalid".to_string());
        }
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        let result = load_direct_sync_push_binding(&connection, transaction_id)?;
        if let Some(result) = result.as_ref() {
            validate_direct_sync_push_binding(result, &binding)?;
        }
        Ok(result)
    }

    /// Compacts only completed rows. Pending, response-received, and
    /// quarantined evidence is never deleted automatically, and a summary
    /// preserves sequence/counter floors so rollback remains detectable.
    pub fn prune_completed_direct_sync_requests(
        &self,
        retain_recent_completed: usize,
    ) -> Result<MobileDirectSyncPruneResult, String> {
        if retain_recent_completed > MAX_MOBILE_DIRECT_SYNC_ROWS as usize {
            return Err("direct-sync completed retention exceeds the journal bound".to_string());
        }
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        active_direct_sync_binding(&transaction)?;
        let pruned =
            prune_completed_direct_sync_in_transaction(&transaction, retain_recent_completed)?;
        let remaining: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM mobile_direct_sync_request_v1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileDirectSyncPruneResult {
            pruned_completed_count: pruned,
            remaining_rows: usize::try_from(remaining)
                .map_err(|_| "direct-sync row count exceeds platform range".to_string())?,
        })
    }

    /// Stores one exact opaque bootstrap response. Direct-sync pagination does
    /// not reveal a page count up front, so each page chains the prior response
    /// digest and exact `after_record_id`; `has_more = false` seals the count.
    pub fn stage_bootstrap_page(
        &self,
        draft: &MobileBootstrapPageDraft,
    ) -> Result<MobileBootstrapStageResult, String> {
        if !is_uuid_v7(&draft.checkpoint_id)
            || draft.contract_version != crate::sync_protocol::BOOTSTRAP_SNAPSHOT_VERSION
            || !is_sha256(&draft.checkpoint_sha256)
            || !is_uuid_v7(&draft.library_id)
            || draft.authority_generation <= 0
            || draft.purge_generation < 0
            || draft.key_epoch <= 0
            || draft.page_index >= MAX_MOBILE_BOOTSTRAP_PAGES
            || draft.high_water_cursor < 0
            || (draft.page_index == 0) != draft.dependency_sha256.is_none()
            || (draft.page_index == 0) != draft.requested_after_record_id.is_none()
            || draft
                .dependency_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            || draft
                .requested_after_record_id
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 512)
            || draft
                .next_after_record_id
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 512)
            || (draft.has_more && draft.next_after_record_id.is_none())
            || draft.response_bytes.is_empty()
            || draft.response_bytes.len() > MAX_MOBILE_BOOTSTRAP_PAGE_BYTES
        {
            return Err("mobile bootstrap page or commitment is invalid".to_string());
        }
        let response_sha256 = exact_sha256(&draft.response_bytes);
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let binding = active_direct_sync_binding(&transaction)?;
        if draft.library_id != binding.library_id
            || draft.authority_generation != binding.authority_generation
            || draft.purge_generation != binding.purge_generation
            || draft.key_epoch != binding.key_epoch
        {
            return Err(
                "bootstrap response header does not match finalized activation".to_string(),
            );
        }
        let existing_checkpoint = load_bootstrap_checkpoint(&transaction, &draft.checkpoint_id)?;
        if let Some(existing_page) = load_bootstrap_pages(&transaction, &draft.checkpoint_id)?
            .into_iter()
            .find(|page| page.page_index == draft.page_index as i64)
        {
            let exact = existing_page.checkpoint_sha256 == draft.checkpoint_sha256
                && existing_page.requested_after_record_id == draft.requested_after_record_id
                && existing_page.next_after_record_id == draft.next_after_record_id
                && existing_page.has_more == draft.has_more
                && existing_page.dependency_sha256 == draft.dependency_sha256
                && existing_page.response_bytes == draft.response_bytes
                && existing_page.response_sha256 == response_sha256;
            if exact {
                let recovery = bootstrap_recovery(&transaction, &draft.checkpoint_id)?
                    .ok_or_else(|| "bootstrap replay checkpoint disappeared".to_string())?;
                transaction.commit().map_err(|error| error.to_string())?;
                return Ok(MobileBootstrapStageResult {
                    recovery,
                    replayed: true,
                });
            }
            quarantine_bootstrap_in_transaction(
                &transaction,
                &draft.checkpoint_id,
                "page_replay_mismatch",
            )?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Err("byte-different bootstrap page replay was quarantined".to_string());
        }

        let _checkpoint = if let Some(checkpoint) = existing_checkpoint {
            if checkpoint.receipt_id != binding.receipt_id
                || checkpoint.contract_version != draft.contract_version
                || checkpoint.activation_sha256 != binding.activation_sha256
                || checkpoint.library_id != binding.library_id
                || checkpoint.device_id != binding.device_id
                || checkpoint.authority_generation != binding.authority_generation
                || checkpoint.purge_generation != binding.purge_generation
                || checkpoint.key_epoch != binding.key_epoch
                || checkpoint.sync_spki_sha256 != binding.sync_spki_sha256
                || checkpoint.checkpoint_sha256 != draft.checkpoint_sha256
                || checkpoint.high_water_cursor != draft.high_water_cursor
            {
                return Err("bootstrap checkpoint does not match finalized activation".to_string());
            }
            if checkpoint.state != "receiving" {
                return Err("bootstrap checkpoint is not accepting pages".to_string());
            }
            checkpoint
        } else {
            if draft.page_index != 0 {
                return Err("bootstrap pages must begin at index zero".to_string());
            }
            let cursors: (i64, i64) = transaction
                .query_row(
                    "SELECT downloaded_cursor, applied_cursor
                     FROM mobile_sync_state WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| error.to_string())?;
            if cursors.0 != cursors.1 || draft.high_water_cursor < cursors.1 {
                return Err(
                    "bootstrap start cursor is not the fully applied local cursor".to_string(),
                );
            }
            let checkpoint_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM mobile_bootstrap_checkpoint_v1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if checkpoint_count >= MAX_MOBILE_BOOTSTRAP_CHECKPOINTS {
                return Err("mobile bootstrap checkpoint history reached its bound".to_string());
            }
            let created_at = now_millis()?;
            transaction
                .execute(
                    "INSERT INTO mobile_bootstrap_checkpoint_v1 (
                       checkpoint_id, contract_version, checkpoint_sha256,
                       receipt_id, activation_sha256,
                       library_id, device_id, authority_generation, purge_generation,
                       key_epoch, sync_spki_sha256, start_cursor, high_water_cursor,
                       state, created_at
                     ) VALUES (
                       ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                       ?13, 'receiving', ?14
                     )",
                    params![
                        draft.checkpoint_id,
                        draft.contract_version,
                        draft.checkpoint_sha256,
                        binding.receipt_id,
                        binding.activation_sha256,
                        binding.library_id,
                        binding.device_id,
                        binding.authority_generation,
                        binding.purge_generation,
                        binding.key_epoch,
                        binding.sync_spki_sha256,
                        cursors.1,
                        draft.high_water_cursor,
                        created_at,
                    ],
                )
                .map_err(|error| error.to_string())?;
            load_bootstrap_checkpoint(&transaction, &draft.checkpoint_id)?
                .ok_or_else(|| "bootstrap checkpoint insert disappeared".to_string())?
        };
        let pages = load_bootstrap_pages(&transaction, &draft.checkpoint_id)?;
        if pages.len() != draft.page_index {
            return Err("bootstrap page index is not contiguous".to_string());
        }
        if let Some(previous) = pages.last() {
            if !previous.has_more
                || previous.next_after_record_id != draft.requested_after_record_id
                || draft.dependency_sha256.as_deref() != Some(previous.response_sha256.as_str())
            {
                return Err(
                    "bootstrap page cursor or digest dependency is not contiguous".to_string(),
                );
            }
        }
        let total_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(response_bytes)), 0)
                 FROM mobile_bootstrap_page_v1 WHERE checkpoint_id = ?1",
                [&draft.checkpoint_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if total_bytes
            .checked_add(draft.response_bytes.len() as i64)
            .is_none_or(|bytes| bytes > MAX_MOBILE_BOOTSTRAP_TOTAL_BYTES)
        {
            return Err("mobile bootstrap pages exceed their aggregate byte limit".to_string());
        }
        let received_at = now_millis()?;
        transaction
            .execute(
                "INSERT INTO mobile_bootstrap_page_v1 (
                   checkpoint_id, page_index, checkpoint_sha256,
                   requested_after_record_id, next_after_record_id, has_more,
                   dependency_sha256, response_bytes, response_sha256,
                   state, received_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'received', ?10)",
                params![
                    draft.checkpoint_id,
                    draft.page_index as i64,
                    draft.checkpoint_sha256,
                    draft.requested_after_record_id,
                    draft.next_after_record_id,
                    i64::from(draft.has_more),
                    draft.dependency_sha256,
                    draft.response_bytes,
                    response_sha256,
                    received_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        if !draft.has_more {
            let changed = transaction
                .execute(
                    "UPDATE mobile_bootstrap_checkpoint_v1
                     SET final_page_count = ?1, final_commitment_sha256 = ?2,
                         state = 'received', finalized_at = ?3, error_code = NULL
                     WHERE checkpoint_id = ?4 AND state = 'receiving'",
                    params![
                        (draft.page_index + 1) as i64,
                        draft.checkpoint_sha256,
                        received_at,
                        draft.checkpoint_id,
                    ],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("bootstrap final-page commitment was not atomic".to_string());
            }
        }
        let recovery = bootstrap_recovery(&transaction, &draft.checkpoint_id)?
            .ok_or_else(|| "staged bootstrap checkpoint disappeared".to_string())?;
        validate_bootstrap_recovery(&recovery, &binding)?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(MobileBootstrapStageResult {
            recovery,
            replayed: false,
        })
    }

    pub fn recover_bootstrap_staging(&self) -> Result<Option<MobileBootstrapRecovery>, String> {
        let connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        let checkpoint_id: Option<String> = connection
            .query_row(
                "SELECT checkpoint_id FROM mobile_bootstrap_checkpoint_v1
                 WHERE state IN ('receiving', 'received')
                 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(checkpoint_id) = checkpoint_id else {
            return Ok(None);
        };
        let recovery = bootstrap_recovery(&connection, &checkpoint_id)?
            .ok_or_else(|| "bootstrap recovery checkpoint disappeared".to_string())?;
        validate_bootstrap_recovery(&recovery, &binding)?;
        Ok(Some(recovery))
    }

    pub fn abort_bootstrap_staging(
        &self,
        checkpoint_id: &str,
        error_code: &str,
    ) -> Result<MobileBootstrapRecovery, String> {
        finish_bootstrap_terminal(self, checkpoint_id, "aborted", error_code)
    }

    pub fn quarantine_bootstrap_staging(
        &self,
        checkpoint_id: &str,
        error_code: &str,
    ) -> Result<MobileBootstrapRecovery, String> {
        finish_bootstrap_terminal(self, checkpoint_id, "quarantined", error_code)
    }

    /// Applies decoded current-head record batches only after every exact page
    /// is durable. Bootstrap acceptance checkpoints are intentionally sparse;
    /// they are bounded by (but do not need to span) the committed high-water.
    /// Domain writes, cursors, and page states advance in one transaction.
    pub fn apply_bootstrap_snapshot(
        &self,
        checkpoint_id: &str,
        snapshot: &MobileBootstrapSnapshot,
    ) -> Result<MobileBootstrapApplyResult, String> {
        if !is_sha256(&snapshot.checkpoint_sha256) {
            return Err("bootstrap snapshot checkpoint digest is invalid".to_string());
        }
        let changes = snapshot.head_batches.as_slice();
        let record_count = changes.iter().try_fold(0usize, |count, change| {
            count
                .checked_add(change.categories.len() + change.folders.len() + change.notes.len())
                .ok_or_else(|| "bootstrap decoded record count overflowed".to_string())
        })?;
        if changes.len() > MAX_MOBILE_BOOTSTRAP_CHANGES
            || record_count > MAX_MOBILE_BOOTSTRAP_CHANGES
        {
            return Err("bootstrap decoded change count exceeds its bound".to_string());
        }
        let mut decoded_bytes = 0usize;
        let mut category_ids = BTreeSet::new();
        let mut folder_ids = BTreeSet::new();
        let mut note_ids = BTreeSet::new();
        for change in changes {
            validate_mobile_inbox_change(change)?;
            if change
                .categories
                .iter()
                .any(|category| !category_ids.insert(category.category_id.as_str()))
                || change
                    .folders
                    .iter()
                    .any(|folder| !folder_ids.insert(folder.folder_id.as_str()))
                || change
                    .notes
                    .iter()
                    .any(|note| !note_ids.insert(note.record_id.as_str()))
            {
                return Err("bootstrap snapshot contains duplicate record heads".to_string());
            }
            decoded_bytes = decoded_bytes
                .checked_add(
                    serde_json::to_vec(change)
                        .map_err(|error| error.to_string())?
                        .len(),
                )
                .ok_or_else(|| "bootstrap decoded byte size overflowed".to_string())?;
        }
        if decoded_bytes > MAX_MOBILE_BOOTSTRAP_TOTAL_BYTES as usize {
            return Err("bootstrap decoded changes exceed their bounded size".to_string());
        }

        let mut connection = self.lock_connection()?;
        let binding = active_direct_sync_binding(&connection)?;
        let recovery = bootstrap_recovery(&connection, checkpoint_id)?
            .ok_or_else(|| "bootstrap checkpoint does not exist".to_string())?;
        validate_bootstrap_recovery(&recovery, &binding)?;
        if snapshot.checkpoint_sha256 != recovery.checkpoint.checkpoint_sha256 {
            return Err("bootstrap snapshot is bound to a different checkpoint".to_string());
        }
        recovery
            .checkpoint
            .final_page_count
            .ok_or_else(|| "bootstrap final page is not committed".to_string())?;
        let final_cursor = recovery.checkpoint.high_water_cursor;
        if recovery.checkpoint.state == "applied" {
            let applied_cursor: i64 = connection
                .query_row(
                    "SELECT applied_cursor FROM mobile_sync_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if applied_cursor < final_cursor {
                return Err("applied bootstrap checkpoint is ahead of the sync cursor".to_string());
            }
            return Ok(MobileBootstrapApplyResult {
                checkpoint_id: checkpoint_id.to_string(),
                final_cursor,
                applied_change_count: 0,
                applied_record_count: 0,
                conflict_count: 0,
                replayed: true,
            });
        }
        if recovery.checkpoint.state != "received"
            || recovery.pages.iter().any(|page| page.state != "received")
        {
            return Err("bootstrap checkpoint is not ready for atomic apply".to_string());
        }
        let start_cursor = recovery.checkpoint.start_cursor;
        if changes.iter().any(|change| change.sequence > final_cursor)
            || changes
                .windows(2)
                .any(|pair| pair[1].sequence <= pair[0].sequence)
        {
            return Err(
                "bootstrap record checkpoints are unordered or exceed the committed high-water"
                    .to_string(),
            );
        }

        let apply_result = (|| -> Result<(usize, usize), InboxApplyError> {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            let cursors: (i64, i64) = transaction
                .query_row(
                    "SELECT downloaded_cursor, applied_cursor
                     FROM mobile_sync_state WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            if cursors != (start_cursor, start_cursor) {
                return Err(InboxApplyError::semantic(
                    "bootstrap apply cursor changed after page staging",
                ));
            }
            let mut conflicts = 0usize;
            for change in changes {
                validate_inbox_authority(&transaction, change)?;
            }
            if let Some(first) = changes.first() {
                let mut workspace = first.clone();
                workspace.categories = changes
                    .iter()
                    .flat_map(|change| change.categories.iter().cloned())
                    .collect();
                workspace.folders = changes
                    .iter()
                    .flat_map(|change| change.folders.iter().cloned())
                    .collect();
                workspace.notes.clear();
                apply_incoming_categories(&transaction, &workspace)?;
                apply_incoming_folders(&transaction, &workspace)?;
            }
            for change in changes {
                for note in &change.notes {
                    if apply_incoming_note(&transaction, change, note)? {
                        conflicts += 1;
                    }
                }
            }
            let applied_at = now_millis().map_err(InboxApplyError::operational)?;
            let changed_pages = transaction
                .execute(
                    "UPDATE mobile_bootstrap_page_v1
                     SET state = 'applied', applied_at = ?1, error_code = NULL
                     WHERE checkpoint_id = ?2 AND state = 'received'",
                    params![applied_at, checkpoint_id],
                )
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            if changed_pages != recovery.pages.len() {
                return Err(InboxApplyError::operational(
                    "bootstrap pages changed during atomic apply",
                ));
            }
            let changed_checkpoint = transaction
                .execute(
                    "UPDATE mobile_bootstrap_checkpoint_v1
                     SET state = 'applied', applied_at = ?1, error_code = NULL
                     WHERE checkpoint_id = ?2 AND state = 'received'",
                    params![applied_at, checkpoint_id],
                )
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            if changed_checkpoint != 1 {
                return Err(InboxApplyError::operational(
                    "bootstrap checkpoint changed during atomic apply",
                ));
            }
            let pending: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM mobile_note_outbox WHERE eligible_for_sync = 1
                     )",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            let changed_cursor = transaction
                .execute(
                    "UPDATE mobile_sync_state
                     SET downloaded_cursor = ?1, applied_cursor = ?1,
                         sync_state = ?2, last_synced_at = ?3, last_error_code = NULL
                     WHERE singleton = 1 AND downloaded_cursor = ?4 AND applied_cursor = ?4",
                    params![
                        final_cursor,
                        if conflicts > 0 {
                            "conflict"
                        } else if pending {
                            "pending"
                        } else {
                            "idle"
                        },
                        applied_at,
                        start_cursor,
                    ],
                )
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            if changed_cursor != 1 {
                return Err(InboxApplyError::operational(
                    "bootstrap cursor did not advance atomically",
                ));
            }
            transaction
                .commit()
                .map_err(|error| InboxApplyError::operational(error.to_string()))?;
            Ok((record_count, conflicts))
        })();
        match apply_result {
            Ok((applied_record_count, conflict_count)) => Ok(MobileBootstrapApplyResult {
                checkpoint_id: checkpoint_id.to_string(),
                final_cursor,
                applied_change_count: changes.len(),
                applied_record_count,
                conflict_count,
                replayed: false,
            }),
            Err(InboxApplyError::Semantic(error)) => {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|failure| failure.to_string())?;
                quarantine_bootstrap_in_transaction(
                    &transaction,
                    checkpoint_id,
                    "semantic_apply_rejected",
                )?;
                transaction
                    .commit()
                    .map_err(|failure| failure.to_string())?;
                Err(error)
            }
            Err(InboxApplyError::Operational(error)) => Err(error),
        }
    }
}

fn validate_canonical_pull_change(
    change: &MobileCanonicalPullChange,
) -> Result<Vec<ContextRecordV1>, String> {
    if change.sequence <= 0
        || !is_uuid(&change.transaction_id)
        || !is_sha256(&change.transaction_digest)
        || !is_uuid_v7(&change.library_id)
        || !is_uuid(&change.source_device_id)
        || change.authority_generation <= 0
        || change.purge_generation < 0
        || change.record_bytes.is_empty()
        || change.record_bytes.len() > MAX_MOBILE_TRANSACTION_MEMBERS
    {
        return Err("canonical pull envelope is invalid".to_string());
    }
    let records = decode_canonical_record_set(&change.record_bytes)?;
    if records
        .iter()
        .any(|record| record.library_id != change.library_id)
    {
        return Err("canonical pull record belongs to another library".to_string());
    }
    Ok(records)
}

fn decode_canonical_record_set(bytes: &[Vec<u8>]) -> Result<Vec<ContextRecordV1>, String> {
    let mut total = 0usize;
    let mut ids = BTreeSet::new();
    let mut records = Vec::with_capacity(bytes.len());
    for exact in bytes {
        total = total
            .checked_add(exact.len())
            .ok_or_else(|| "canonical record batch size overflowed".to_string())?;
        if total > MAX_MOBILE_INBOX_BYTES {
            return Err("canonical record batch exceeds the 4 MiB limit".to_string());
        }
        let record = decode_exact_canonical_context_record(exact)?;
        if !ids.insert(record.record_id.clone()) {
            return Err("canonical record batch contains duplicate heads".to_string());
        }
        records.push(record);
    }
    Ok(records)
}

fn canonical_pull_evidence_json(change: &MobileCanonicalPullChange) -> Result<String, String> {
    let evidence = serde_json::json!({
        "contractVersion": "noted.mobile-canonical-apply.v1",
        "sequence": change.sequence,
        "transactionId": change.transaction_id,
        "transactionDigest": change.transaction_digest,
        "libraryId": change.library_id,
        "sourceDeviceId": change.source_device_id,
        "authorityGeneration": change.authority_generation,
        "purgeGeneration": change.purge_generation,
        "records": change.record_bytes.iter().map(|bytes| serde_json::json!({
            "sha256": exact_sha256(bytes),
            "byteLength": bytes.len(),
        })).collect::<Vec<_>>(),
    });
    Ok(canonical_json(&evidence))
}

fn validate_canonical_authority_binding(
    connection: &Connection,
    library_id: &str,
    authority_generation: i64,
    purge_generation: i64,
) -> Result<(), String> {
    let identity = replica_identity(connection)?;
    let state: (String, i64, i64) = connection
        .query_row(
            "SELECT enrollment_state, authority_generation, purge_generation
             FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if identity.library_state != "paired"
        || state.0 != "active"
        || identity.library_id != library_id
        || state.1 != authority_generation
        || state.2 != purge_generation
    {
        return Err(
            "canonical sync input is not bound to the active library generation".to_string(),
        );
    }
    Ok(())
}

fn portable_timestamp_millis(value: &str) -> Result<i64, String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| "canonical record timestamp is invalid".to_string())?;
    let millis = parsed.timestamp_millis();
    if !(0..=MAX_PORTABLE_TIMESTAMP_MS).contains(&millis) {
        return Err("canonical record timestamp is outside the portable range".to_string());
    }
    Ok(millis)
}

fn projected_authority(authority: &RecordAuthority) -> &'static str {
    match authority.kind {
        AuthorityKind::Noted => "noted",
        AuthorityKind::External | AuthorityKind::Derived => "external",
    }
}

fn projected_scope_class(scope: &ScopeClass) -> &'static str {
    match scope {
        ScopeClass::Work => "work",
        ScopeClass::Personal => "personal",
        ScopeClass::Unknown => "unknown",
    }
}

fn lifecycle_projection(
    lifecycle: &RecordLifecycle,
) -> Result<(String, Option<i64>, Option<i64>), String> {
    Ok(match lifecycle.state {
        LifecycleState::Active => ("active".to_string(), None, None),
        LifecycleState::Trash => (
            "trash".to_string(),
            Some(portable_timestamp_millis(
                lifecycle
                    .trashed_at
                    .as_deref()
                    .ok_or_else(|| "trash record is missing trashed_at".to_string())?,
            )?),
            None,
        ),
        LifecycleState::Tombstone => (
            "tombstone".to_string(),
            Some(portable_timestamp_millis(
                lifecycle
                    .trashed_at
                    .as_deref()
                    .ok_or_else(|| "tombstone record is missing trashed_at".to_string())?,
            )?),
            Some(portable_timestamp_millis(
                lifecycle
                    .tombstoned_at
                    .as_deref()
                    .ok_or_else(|| "tombstone record is missing tombstoned_at".to_string())?,
            )?),
        ),
    })
}

fn apply_canonical_record_set(
    transaction: &Transaction<'_>,
    records: &[ContextRecordV1],
    source_device_id: &str,
) -> Result<usize, String> {
    let mut categories = records
        .iter()
        .filter(|record| record.kind == "category")
        .collect::<Vec<_>>();
    categories.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    for record in categories {
        apply_canonical_category(transaction, record)?;
    }

    let mut remaining = records
        .iter()
        .filter(|record| record.kind == "folder")
        .map(|record| (record.record_id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter_map(|(record_id, record)| {
                if matches!(record.lifecycle.state, LifecycleState::Tombstone) {
                    return Some(record_id.clone());
                }
                let parent = record
                    .content
                    .get("parentId")
                    .and_then(serde_json::Value::as_str);
                let parent_ready = parent.is_none_or(|parent_id| {
                    !remaining.contains_key(parent_id)
                        && transaction
                            .query_row(
                                "SELECT EXISTS(
                                   SELECT 1 FROM mobile_note_folders
                                   WHERE folder_id = ?1 AND lifecycle_state = 'active'
                                 )",
                                [parent_id],
                                |row| row.get::<_, bool>(0),
                            )
                            .unwrap_or(false)
                });
                parent_ready.then_some(record_id.clone())
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err("canonical folder batch has a missing parent or cycle".to_string());
        }
        for record_id in ready {
            let record = remaining
                .remove(&record_id)
                .ok_or_else(|| "canonical folder apply lost a ready record".to_string())?;
            apply_canonical_folder(transaction, record)?;
        }
    }

    let mut notes = records
        .iter()
        .filter(|record| record.kind == "note")
        .collect::<Vec<_>>();
    notes.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    let mut conflicts = 0usize;
    for record in notes {
        if apply_canonical_note(transaction, record, source_device_id)? {
            conflicts += 1;
        }
    }
    Ok(conflicts)
}

fn existing_accepted_canonical_bytes(
    connection: &Connection,
    record: &ContextRecordV1,
) -> Result<Option<Vec<u8>>, String> {
    let existing: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT accepted_revision, accepted_record_json
             FROM mobile_canonical_record_v1
             WHERE record_id = ?1 AND accepted_revision IS NOT NULL",
            [&record.record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, bytes)) = existing else {
        return Ok(None);
    };
    if record.revision < revision as u64 {
        return Err("canonical accepted head attempted a revision rollback".to_string());
    }
    if record.revision == revision as u64 {
        let incoming = canonical_context_record_bytes(record)?;
        if incoming != bytes {
            return Err(
                "canonical accepted revision was reused with different exact bytes".to_string(),
            );
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn apply_canonical_category(
    transaction: &Transaction<'_>,
    record: &ContextRecordV1,
) -> Result<(), String> {
    if existing_accepted_canonical_bytes(transaction, record)?.is_some() {
        return Ok(());
    }
    if matches!(record.lifecycle.state, LifecycleState::Trash) {
        return Err("category records cannot use the trash lifecycle".to_string());
    }
    let content = record.content.as_object().expect("shape validated");
    let name = content
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    let schema = content
        .get("schema")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let updated_at = portable_timestamp_millis(&record.updated_at)?;
    let created_at = portable_timestamp_millis(&record.created_at)?;
    let lifecycle = if matches!(record.lifecycle.state, LifecycleState::Tombstone) {
        "tombstone"
    } else {
        "active"
    };
    let changed = transaction
        .execute(
            "INSERT INTO mobile_note_categories (
               category_id, library_id, name, normalized_name, schema_json,
               authority, lifecycle_state, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(category_id) DO UPDATE SET
               name = excluded.name, normalized_name = excluded.normalized_name,
               schema_json = excluded.schema_json, authority = excluded.authority,
               lifecycle_state = excluded.lifecycle_state, updated_at = excluded.updated_at
             WHERE mobile_note_categories.library_id = excluded.library_id",
            params![
                record.record_id,
                record.library_id,
                name,
                normalized_workspace_name(name),
                canonical_json(&schema),
                projected_authority(&record.authority),
                lifecycle,
                created_at,
                updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("canonical category projection collided with another library".to_string());
    }
    write_canonical_record_row(
        transaction,
        Some(record),
        record,
        "native_exact",
        updated_at,
    )
}

fn apply_canonical_folder(
    transaction: &Transaction<'_>,
    record: &ContextRecordV1,
) -> Result<(), String> {
    if existing_accepted_canonical_bytes(transaction, record)?.is_some() {
        return Ok(());
    }
    if matches!(record.lifecycle.state, LifecycleState::Trash) {
        return Err("folder records cannot use the trash lifecycle".to_string());
    }
    let content = record.content.as_object().expect("shape validated");
    let name = content
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    let parent_id = content.get("parentId").and_then(serde_json::Value::as_str);
    let position = content
        .get("position")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if position < 0 {
        return Err("canonical folder position cannot be negative".to_string());
    }
    let updated_at = portable_timestamp_millis(&record.updated_at)?;
    let created_at = portable_timestamp_millis(&record.created_at)?;
    let lifecycle = if matches!(record.lifecycle.state, LifecycleState::Tombstone) {
        "tombstone"
    } else {
        "active"
    };
    let changed = transaction
        .execute(
            "INSERT INTO mobile_note_folders (
               folder_id, library_id, parent_folder_id, name, normalized_name,
               position, authority, lifecycle_state, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(folder_id) DO UPDATE SET
               parent_folder_id = excluded.parent_folder_id,
               name = excluded.name, normalized_name = excluded.normalized_name,
               position = excluded.position, authority = excluded.authority,
               lifecycle_state = excluded.lifecycle_state, updated_at = excluded.updated_at
             WHERE mobile_note_folders.library_id = excluded.library_id",
            params![
                record.record_id,
                record.library_id,
                parent_id,
                name,
                normalized_workspace_name(name),
                position,
                projected_authority(&record.authority),
                lifecycle,
                created_at,
                updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("canonical folder projection collided with another library".to_string());
    }
    write_canonical_record_row(
        transaction,
        Some(record),
        record,
        "native_exact",
        updated_at,
    )
}

fn canonical_note_projection(record: &ContextRecordV1) -> Result<MobileIncomingNote, String> {
    let content = record.content.as_object().expect("shape validated");
    let (lifecycle_state, trashed_at, tombstoned_at) = lifecycle_projection(&record.lifecycle)?;
    Ok(MobileIncomingNote {
        record_id: record.record_id.clone(),
        title: content
            .get("title")
            .and_then(serde_json::Value::as_str)
            .expect("shape validated")
            .to_string(),
        body: content
            .get("body")
            .and_then(serde_json::Value::as_str)
            .expect("shape validated")
            .to_string(),
        created_at: portable_timestamp_millis(&record.created_at)?,
        updated_at: portable_timestamp_millis(&record.updated_at)?,
        accepted_revision: i64::try_from(record.revision)
            .map_err(|_| "canonical note revision exceeds SQLite range".to_string())?,
        accepted_version_id: record.version_id.clone(),
        accepted_content_hash: record.content_hash.clone(),
        lifecycle_state,
        trashed_at,
        tombstoned_at,
        folder_id: content
            .get("folderId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        authority: authority_storage_value(&record.authority).to_string(),
        scope_id: record.scope.scope_id.clone(),
        scope_class: projected_scope_class(&record.scope.class).to_string(),
    })
}

fn apply_canonical_note(
    transaction: &Transaction<'_>,
    record: &ContextRecordV1,
    source_device_id: &str,
) -> Result<bool, String> {
    if existing_accepted_canonical_bytes(transaction, record)?.is_some() {
        return Ok(false);
    }
    let note = canonical_note_projection(record)?;
    ensure_incoming_folder_exists(transaction, &record.library_id, note.folder_id.as_deref())
        .map_err(InboxApplyError::into_string)?;
    let existing = load_existing_sync_note(transaction, &record.record_id)
        .map_err(InboxApplyError::into_string)?;
    let exact_bytes = canonical_context_record_bytes(record)?;
    let Some(existing_note) = existing else {
        let change = canonical_projection_change(record, source_device_id, note.clone());
        insert_remote_note(transaction, &change, &note).map_err(InboxApplyError::into_string)?;
        write_canonical_record_row(
            transaction,
            Some(record),
            record,
            "native_exact",
            note.updated_at,
        )?;
        return Ok(false);
    };
    ensure_no_open_note_conflict(transaction, &record.record_id)?;
    let local_branch: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_outbox
               WHERE record_id = ?1 AND eligible_for_sync = 1
             )",
            [&record.record_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let (stored_working, stored_backfill_provenance): (Vec<u8>, String) = transaction
        .query_row(
            "SELECT working_record_json, backfill_provenance
             FROM mobile_canonical_record_v1 WHERE record_id = ?1",
            [&record.record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    let acknowledges_local = local_branch && stored_working == exact_bytes;
    if local_branch && !acknowledges_local {
        preserve_note_conflict(transaction, &note, &existing_note)
            .map_err(InboxApplyError::into_string)?;
        let working = decode_exact_canonical_context_record(&stored_working)?;
        write_canonical_record_row(
            transaction,
            Some(record),
            &working,
            &stored_backfill_provenance,
            note.updated_at,
        )?;
        return Ok(true);
    }
    let change = canonical_projection_change(record, source_device_id, note.clone());
    materialize_remote_note(transaction, &change, &note, false)
        .map_err(InboxApplyError::into_string)?;
    if acknowledges_local {
        acknowledge_local_outbox_group(
            transaction,
            &record.record_id,
            &existing_note.pending_mutation_id,
            note.updated_at,
        )
        .map_err(InboxApplyError::into_string)?;
    }
    write_canonical_record_row(
        transaction,
        Some(record),
        record,
        "native_exact",
        note.updated_at,
    )?;
    Ok(false)
}

fn canonical_projection_change(
    record: &ContextRecordV1,
    source_device_id: &str,
    note: MobileIncomingNote,
) -> MobileInboxChange {
    MobileInboxChange {
        sequence: 1,
        transaction_id: new_uuid_v7(),
        transaction_digest: "0".repeat(64),
        library_id: record.library_id.clone(),
        source_device_id: source_device_id.to_string(),
        authority_generation: 1,
        purge_generation: 0,
        categories: Vec::new(),
        folders: Vec::new(),
        notes: vec![note],
    }
}

fn validate_mobile_inbox_change(change: &MobileInboxChange) -> Result<(), String> {
    let member_count = change.categories.len() + change.folders.len() + change.notes.len();
    if change.sequence <= 0
        || member_count == 0
        || member_count > MAX_MOBILE_TRANSACTION_MEMBERS
        || !is_uuid(&change.transaction_id)
        || !is_uuid_v7(&change.library_id)
        || !is_uuid(&change.source_device_id)
        || change.authority_generation <= 0
        || change.purge_generation < 0
        || !is_sha256(&change.transaction_digest)
    {
        return Err("mobile sync inbox envelope is invalid".to_string());
    }
    if change.computed_transaction_digest() != change.transaction_digest {
        return Err("mobile sync inbox digest does not bind its payload".to_string());
    }

    let mut category_ids = BTreeSet::new();
    for category in &change.categories {
        if !category_ids.insert(category.category_id.as_str())
            || !is_uuid_v7(&category.category_id)
            || category.name.trim().is_empty()
            || !(0..=MAX_PORTABLE_TIMESTAMP_MS).contains(&category.updated_at)
            || !matches!(category.authority.as_str(), "noted" | "external")
        {
            return Err("mobile sync category payload is invalid".to_string());
        }
    }

    let mut folder_ids = BTreeSet::new();
    for folder in &change.folders {
        if !folder_ids.insert(folder.folder_id.as_str())
            || !is_uuid_v7(&folder.folder_id)
            || folder
                .parent_folder_id
                .as_deref()
                .is_some_and(|parent| !is_uuid_v7(parent) || parent == folder.folder_id)
            || folder.name.trim().is_empty()
            || folder.position < 0
            || !(0..=MAX_PORTABLE_TIMESTAMP_MS).contains(&folder.updated_at)
            || !matches!(folder.authority.as_str(), "noted" | "external")
        {
            return Err("mobile sync folder payload is invalid".to_string());
        }
    }

    let mut note_ids = BTreeSet::new();
    for note in &change.notes {
        let lifecycle_valid = match note.lifecycle_state.as_str() {
            "active" => note.trashed_at.is_none() && note.tombstoned_at.is_none(),
            "trash" => {
                note.trashed_at
                    .is_some_and(|trashed| trashed >= note.created_at && trashed <= note.updated_at)
                    && note.tombstoned_at.is_none()
            }
            "tombstone" => {
                note.trashed_at
                    .zip(note.tombstoned_at)
                    .is_some_and(|(trashed, tombstoned)| {
                        trashed >= note.created_at
                            && trashed <= tombstoned
                            && tombstoned <= note.updated_at
                    })
            }
            _ => false,
        };
        if !note_ids.insert(note.record_id.as_str())
            || !is_uuid_v7(&note.record_id)
            || note.accepted_revision <= 0
            || !is_uuid(&note.accepted_version_id)
            || !is_sha256(&note.accepted_content_hash)
            || note.accepted_content_hash != note_content_hash(note.title.trim(), &note.body)
            || note.title.len() > MAX_MOBILE_NOTE_TEXT_BYTES
            || note.body.len() > MAX_MOBILE_NOTE_TEXT_BYTES
            || !(0..=MAX_PORTABLE_TIMESTAMP_MS).contains(&note.created_at)
            || note.updated_at > MAX_PORTABLE_TIMESTAMP_MS
            || note.updated_at < note.created_at
            || !lifecycle_valid
            || note
                .folder_id
                .as_deref()
                .is_some_and(|folder_id| !is_uuid_v7(folder_id))
            || !matches!(note.authority.as_str(), "noted" | "external")
            || !is_uuid(&note.scope_id)
            || !matches!(note.scope_class.as_str(), "personal" | "work" | "unknown")
        {
            return Err("mobile sync note payload is invalid".to_string());
        }
    }
    Ok(())
}

fn validate_inbox_authority(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
) -> Result<(), InboxApplyError> {
    let identity = replica_identity(transaction).map_err(InboxApplyError::operational)?;
    let state: (String, i64, i64) = transaction
        .query_row(
            "SELECT enrollment_state, authority_generation, purge_generation
             FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| {
            InboxApplyError::operational(format!("read mobile sync authority: {error}"))
        })?;
    if identity.library_state != "paired"
        || state.0 != "active"
        || identity.library_id != change.library_id
        || state.1 != change.authority_generation
        || state.2 != change.purge_generation
    {
        return Err(InboxApplyError::semantic(
            "mobile sync inbox is not bound to the active library generation",
        ));
    }
    Ok(())
}

fn apply_incoming_categories(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
) -> Result<(), InboxApplyError> {
    for category in &change.categories {
        let changed = transaction
            .execute(
                "INSERT INTO mobile_note_categories (
                   category_id, library_id, name, normalized_name, schema_json,
                   authority, lifecycle_state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7)
                 ON CONFLICT(category_id) DO UPDATE SET
                   name = excluded.name,
                   normalized_name = excluded.normalized_name,
                   schema_json = excluded.schema_json,
                   authority = excluded.authority,
                   lifecycle_state = 'active',
                   updated_at = excluded.updated_at
                 WHERE mobile_note_categories.library_id = excluded.library_id",
                params![
                    category.category_id,
                    change.library_id,
                    category.name.trim(),
                    normalized_workspace_name(&category.name),
                    serde_json::to_string(&category.schema)
                        .map_err(|error| InboxApplyError::semantic(error.to_string()))?,
                    category.authority,
                    category.updated_at,
                ],
            )
            .map_err(|error| inbox_sql_error("apply mobile sync category", error))?;
        if changed != 1 {
            return Err(InboxApplyError::semantic(format!(
                "mobile sync category {} collides with another library",
                category.category_id
            )));
        }
    }
    Ok(())
}

fn apply_incoming_folders(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
) -> Result<(), InboxApplyError> {
    validate_proposed_folder_graph(transaction, change)?;
    let mut known = transaction
        .prepare(
            "SELECT folder_id FROM mobile_note_folders
             WHERE library_id = ?1 AND lifecycle_state = 'active'",
        )
        .and_then(|mut statement| {
            statement
                .query_map([&change.library_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<BTreeSet<_>>>()
        })
        .map_err(|error| {
            InboxApplyError::operational(format!("load mobile folder graph: {error}"))
        })?;
    let mut remaining = change
        .folders
        .iter()
        .map(|folder| (folder.folder_id.clone(), folder))
        .collect::<BTreeMap<_, _>>();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter_map(|(folder_id, folder)| {
                if folder
                    .parent_folder_id
                    .as_ref()
                    .is_none_or(|parent| known.contains(parent))
                {
                    Some(folder_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(InboxApplyError::semantic(
                "mobile sync folder graph has a missing parent or cycle",
            ));
        }
        for folder_id in ready {
            let folder = remaining
                .remove(&folder_id)
                .expect("ready folder remains present");
            let changed = transaction
                .execute(
                    "INSERT INTO mobile_note_folders (
                       folder_id, library_id, parent_folder_id, name,
                       normalized_name, position, authority, lifecycle_state,
                       created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?8)
                     ON CONFLICT(folder_id) DO UPDATE SET
                       parent_folder_id = excluded.parent_folder_id,
                       name = excluded.name,
                       normalized_name = excluded.normalized_name,
                       position = excluded.position,
                       authority = excluded.authority,
                       lifecycle_state = 'active',
                       updated_at = excluded.updated_at
                     WHERE mobile_note_folders.library_id = excluded.library_id",
                    params![
                        folder.folder_id,
                        change.library_id,
                        folder.parent_folder_id,
                        folder.name.trim(),
                        normalized_workspace_name(&folder.name),
                        folder.position,
                        folder.authority,
                        folder.updated_at,
                    ],
                )
                .map_err(|error| inbox_sql_error("apply mobile sync folder", error))?;
            if changed != 1 {
                return Err(InboxApplyError::semantic(format!(
                    "mobile sync folder {} collides with another library",
                    folder.folder_id
                )));
            }
            known.insert(folder_id);
        }
    }
    Ok(())
}

fn validate_proposed_folder_graph(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
) -> Result<(), InboxApplyError> {
    let mut graph = transaction
        .prepare(
            "SELECT folder_id, parent_folder_id
             FROM mobile_note_folders
             WHERE library_id = ?1 AND lifecycle_state = 'active'",
        )
        .and_then(|mut statement| {
            statement
                .query_map([&change.library_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        })
        .map_err(|error| {
            InboxApplyError::operational(format!("load proposed mobile folder graph: {error}"))
        })?;
    for folder in &change.folders {
        graph.insert(folder.folder_id.clone(), folder.parent_folder_id.clone());
    }
    if graph
        .values()
        .flatten()
        .any(|parent| !graph.contains_key(parent))
    {
        return Err(InboxApplyError::semantic(
            "mobile sync folder graph has a missing parent",
        ));
    }
    for folder_id in graph.keys() {
        let mut seen = BTreeSet::new();
        let mut cursor = Some(folder_id.as_str());
        while let Some(current) = cursor {
            if !seen.insert(current) {
                return Err(InboxApplyError::semantic(
                    "mobile sync folder graph contains a cycle",
                ));
            }
            cursor = graph.get(current).and_then(Option::as_deref);
        }
    }
    Ok(())
}

fn apply_incoming_note(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
    note: &MobileIncomingNote,
) -> Result<bool, InboxApplyError> {
    ensure_incoming_folder_exists(transaction, &change.library_id, note.folder_id.as_deref())?;
    let existing = load_existing_sync_note(transaction, &note.record_id)?;
    let Some(existing) = existing else {
        insert_remote_note(transaction, change, note)?;
        return Ok(false);
    };
    if existing.conflict_of.as_deref() == Some(note.record_id.as_str()) {
        return Err(InboxApplyError::semantic(
            "mobile sync conflict-copy identity is self-referential",
        ));
    }
    let has_open_conflict: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_conflicts
               WHERE record_id = ?1 AND state = 'open'
             )",
            [&note.record_id],
            |row| row.get(0),
        )
        .map_err(|error| {
            InboxApplyError::operational(format!("inspect open mobile note conflict: {error}"))
        })?;
    if has_open_conflict {
        return Err(InboxApplyError::semantic(
            "mobile note must resolve its current conflict before another advance",
        ));
    }
    if note.accepted_revision < existing.accepted_revision {
        return Err(InboxApplyError::semantic(
            "mobile sync note attempted an accepted-head rollback",
        ));
    }
    if note.accepted_revision == existing.accepted_revision {
        if existing.accepted_version_id.as_deref() != Some(&note.accepted_version_id)
            || existing.accepted_content_hash.as_deref() != Some(&note.accepted_content_hash)
        {
            return Err(InboxApplyError::semantic(
                "mobile sync reused an accepted revision with different bytes",
            ));
        }
        return Ok(false);
    }

    let has_local_branch: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_outbox
               WHERE record_id = ?1 AND eligible_for_sync = 1
             )",
            [&note.record_id],
            |row| row.get(0),
        )
        .map_err(|error| {
            InboxApplyError::operational(format!("inspect local mobile note branch: {error}"))
        })?;
    let remote_acknowledges_local = has_local_branch
        && existing.working_version_id == note.accepted_version_id
        && existing.canonical_hash == note.accepted_content_hash;
    if has_local_branch && !remote_acknowledges_local {
        preserve_note_conflict(transaction, note, &existing)?;
        return Ok(true);
    }

    materialize_remote_note(transaction, change, note, false)?;
    if remote_acknowledges_local {
        acknowledge_local_outbox_group(
            transaction,
            &note.record_id,
            &existing.pending_mutation_id,
            note.updated_at,
        )?;
    }
    Ok(false)
}

fn load_existing_sync_note(
    transaction: &Transaction<'_>,
    record_id: &str,
) -> Result<Option<ExistingSyncNote>, InboxApplyError> {
    transaction
        .query_row(
            "SELECT notes.title, notes.body,
                    notes.accepted_revision, notes.accepted_version_id,
                    notes.accepted_content_hash, notes.working_branch_id,
                    notes.working_version_id, notes.pending_mutation_id,
                    notes.lifecycle_state, notes.canonical_hash,
                    filing.folder_id, notes.conflict_of
             FROM mobile_notes AS notes
             LEFT JOIN mobile_note_filing AS filing
               ON filing.record_id = notes.record_id
             WHERE notes.record_id = ?1",
            [record_id],
            |row| {
                Ok(ExistingSyncNote {
                    title: row.get(0)?,
                    body: row.get(1)?,
                    accepted_revision: row.get(2)?,
                    accepted_version_id: row.get(3)?,
                    accepted_content_hash: row.get(4)?,
                    working_branch_id: row.get(5)?,
                    working_version_id: row.get(6)?,
                    pending_mutation_id: row.get(7)?,
                    lifecycle_state: row.get(8)?,
                    canonical_hash: row.get(9)?,
                    folder_id: row.get(10)?,
                    conflict_of: row.get(11)?,
                })
            },
        )
        .optional()
        .map_err(|error| {
            InboxApplyError::operational(format!("load existing mobile sync note: {error}"))
        })
}

fn insert_remote_note(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
    note: &MobileIncomingNote,
) -> Result<(), InboxApplyError> {
    let provenance_json = incoming_note_provenance_json(change, note)?;
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
               origin_install_id, conflict_of
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5,
               ?6, ?7, 'note', 1,
               ?8, ?9, ?10,
               ?8, ?9, ?9,
               ?8, ?9, 'acknowledged',
               ?11, ?12, ?13, ?10,
               ?14, ?15, ?16, ?15, 'standard',
               ?17, ?18, ?18,
               ?18, NULL
             )",
            params![
                note.title.trim(),
                note.body,
                note.created_at,
                note.updated_at,
                note.trashed_at,
                change.library_id,
                note.record_id,
                note.accepted_revision,
                note.accepted_version_id,
                note.accepted_content_hash,
                note.lifecycle_state,
                note.trashed_at,
                note.tombstoned_at,
                note.authority,
                note.scope_class,
                note.scope_id,
                provenance_json,
                change.source_device_id,
            ],
        )
        .map_err(|error| inbox_sql_error("insert remote mobile note", error))?;
    set_remote_filing(
        transaction,
        &note.record_id,
        note.folder_id.as_deref(),
        note.updated_at,
    )
}

fn incoming_note_provenance_json(
    change: &MobileInboxChange,
    note: &MobileIncomingNote,
) -> Result<String, InboxApplyError> {
    let provenance = if note.authority == "external" {
        serde_json::json!({
            "source": "external_authority",
            "transport": "direct_sync",
            "source_device_id": change.source_device_id,
        })
    } else {
        serde_json::json!({
            "source": "direct_sync",
            "source_device_id": change.source_device_id,
        })
    };
    serde_json::to_string(&provenance).map_err(|error| InboxApplyError::semantic(error.to_string()))
}

fn materialize_remote_note(
    transaction: &Transaction<'_>,
    change: &MobileInboxChange,
    note: &MobileIncomingNote,
    preserve_conflict_marker: bool,
) -> Result<(), InboxApplyError> {
    let provenance_json = incoming_note_provenance_json(change, note)?;
    let changed = transaction
        .execute(
            "UPDATE mobile_notes
             SET title = ?1, body = ?2, created_at = ?3, updated_at = ?4,
                 deleted_at = ?5,
                 accepted_revision = ?6,
                 accepted_version_id = ?7,
                 accepted_content_hash = ?8,
                 working_revision = ?6,
                 working_branch_id = ?7,
                 working_version_id = ?7,
                 working_base_revision = ?6,
                 pending_mutation_id = ?7,
                 sync_state = 'acknowledged',
                 lifecycle_state = ?9,
                 trashed_at = ?10,
                 tombstoned_at = ?11,
                 canonical_hash = ?8,
                 authority = ?12,
                 scope = ?13,
                 scope_id = ?14,
                 scope_class = ?13,
                 provenance_json = ?15,
                 last_modified_device_id = ?16,
                 conflict_of = CASE WHEN ?17 THEN conflict_of ELSE NULL END
             WHERE record_id = ?18 AND library_id = ?19",
            params![
                note.title.trim(),
                note.body,
                note.created_at,
                note.updated_at,
                note.trashed_at,
                note.accepted_revision,
                note.accepted_version_id,
                note.accepted_content_hash,
                note.lifecycle_state,
                note.trashed_at,
                note.tombstoned_at,
                note.authority,
                note.scope_class,
                note.scope_id,
                provenance_json,
                change.source_device_id,
                preserve_conflict_marker,
                note.record_id,
                change.library_id,
            ],
        )
        .map_err(|error| inbox_sql_error("materialize remote mobile note", error))?;
    if changed != 1 {
        return Err(InboxApplyError::semantic(format!(
            "mobile sync note {} does not exist",
            note.record_id
        )));
    }
    set_remote_filing(
        transaction,
        &note.record_id,
        note.folder_id.as_deref(),
        note.updated_at,
    )
}

fn preserve_note_conflict(
    transaction: &Transaction<'_>,
    note: &MobileIncomingNote,
    local: &ExistingSyncNote,
) -> Result<(), InboxApplyError> {
    let conflict_id = new_uuid_v7();
    transaction
        .execute(
            "INSERT INTO mobile_note_conflicts (
               conflict_id, record_id, local_branch_id, local_version_id,
               local_title, local_body, local_canonical_hash,
               local_lifecycle_state, local_folder_id,
               accepted_revision, accepted_version_id, accepted_content_hash,
               remote_title, remote_body, remote_created_at, remote_updated_at,
               remote_lifecycle_state, remote_trashed_at, remote_tombstoned_at,
               remote_folder_id, remote_authority, remote_scope_id,
               remote_scope_class, state, created_at
             ) VALUES (
               ?1, ?2, ?3, ?4,
               ?5, ?6, ?7,
               ?8, ?9,
               ?10, ?11, ?12,
               ?13, ?14, ?15, ?16,
               ?17, ?18, ?19,
               ?20, ?21, ?22,
               ?23, 'open', ?24
             )",
            params![
                conflict_id,
                note.record_id,
                local.working_branch_id,
                local.working_version_id,
                local.title,
                local.body,
                local.canonical_hash,
                local.lifecycle_state,
                local.folder_id,
                note.accepted_revision,
                note.accepted_version_id,
                note.accepted_content_hash,
                note.title.trim(),
                note.body,
                note.created_at,
                note.updated_at,
                note.lifecycle_state,
                note.trashed_at,
                note.tombstoned_at,
                note.folder_id,
                note.authority,
                note.scope_id,
                note.scope_class,
                now_millis().map_err(InboxApplyError::operational)?,
            ],
        )
        .map_err(|error| inbox_sql_error("preserve mobile note conflict", error))?;
    transaction
        .execute(
            "UPDATE mobile_notes
             SET accepted_revision = ?1,
                 accepted_version_id = ?2,
                 accepted_content_hash = ?3,
                 sync_state = 'conflict'
             WHERE record_id = ?4",
            params![
                note.accepted_revision,
                note.accepted_version_id,
                note.accepted_content_hash,
                note.record_id
            ],
        )
        .map_err(|error| inbox_sql_error("mark conflicted mobile note", error))?;
    mark_local_outbox_group_conflict(transaction, &note.record_id, &local.pending_mutation_id)
}

fn acknowledge_local_outbox_group(
    transaction: &Transaction<'_>,
    record_id: &str,
    pending_mutation_id: &str,
    acknowledged_at: i64,
) -> Result<(), InboxApplyError> {
    let transaction_id: Option<String> = transaction
        .query_row(
            "SELECT transaction_id FROM mobile_note_outbox
             WHERE record_id = ?1 AND mutation_id = ?2",
            params![record_id, pending_mutation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            InboxApplyError::operational(format!("load acknowledged outbox group: {error}"))
        })?;
    if let Some(transaction_id) = transaction_id {
        let direct_state: Option<String> = transaction
            .query_row(
                "SELECT state FROM mobile_direct_sync_push_binding_v1
                 WHERE transaction_id = ?1",
                [&transaction_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                InboxApplyError::operational(format!(
                    "load acknowledged direct-sync push binding: {error}"
                ))
            })?;
        if direct_state
            .as_deref()
            .is_some_and(|state| state != "awaiting_echo" && state != "acknowledged")
        {
            return Err(InboxApplyError::semantic(
                "pull echo arrived before its direct-sync push was completed",
            ));
        }
        transaction
            .execute(
                "UPDATE mobile_note_outbox
                 SET state = 'acknowledged', eligible_for_sync = 0,
                     acknowledged_at = ?1
                 WHERE transaction_id = ?2 AND eligible_for_sync = 1",
                params![acknowledged_at, transaction_id],
            )
            .map_err(|error| {
                InboxApplyError::operational(format!("acknowledge local outbox group: {error}"))
            })?;
        if direct_state.as_deref() == Some("awaiting_echo") {
            let changed = transaction
                .execute(
                    "UPDATE mobile_direct_sync_push_binding_v1
                     SET state = 'acknowledged', updated_at = MAX(created_at, ?1),
                         terminal_at = MAX(created_at, ?1), error_code = NULL
                     WHERE transaction_id = ?2 AND state = 'awaiting_echo'",
                    params![acknowledged_at, transaction_id],
                )
                .map_err(|error| {
                    InboxApplyError::operational(format!(
                        "acknowledge direct-sync push binding: {error}"
                    ))
                })?;
            if changed != 1 {
                return Err(InboxApplyError::operational(
                    "direct-sync push echo did not settle atomically",
                ));
            }
        }
    }
    Ok(())
}

fn mark_local_outbox_group_conflict(
    transaction: &Transaction<'_>,
    record_id: &str,
    pending_mutation_id: &str,
) -> Result<(), InboxApplyError> {
    let transaction_id: Option<String> = transaction
        .query_row(
            "SELECT transaction_id FROM mobile_note_outbox
             WHERE record_id = ?1 AND mutation_id = ?2",
            params![record_id, pending_mutation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            InboxApplyError::operational(format!("load conflicted outbox group: {error}"))
        })?;
    if let Some(transaction_id) = transaction_id {
        let conflicted_at = now_millis().map_err(InboxApplyError::operational)?;
        transaction
            .execute(
                "UPDATE mobile_note_outbox
                 SET state = 'conflict', eligible_for_sync = 0
                 WHERE transaction_id = ?1 AND eligible_for_sync = 1",
                [&transaction_id],
            )
            .map_err(|error| {
                InboxApplyError::operational(format!("mark local outbox group conflict: {error}"))
            })?;
        transaction
            .execute(
                "UPDATE mobile_direct_sync_push_binding_v1
                 SET state = 'conflict', updated_at = ?1, terminal_at = ?1,
                     error_code = 'pull_head_conflict'
                 WHERE transaction_id = ?2 AND state = 'awaiting_echo'",
                params![conflicted_at, transaction_id],
            )
            .map_err(|error| {
                InboxApplyError::operational(format!(
                    "mark direct-sync push binding conflict: {error}"
                ))
            })?;
    }
    Ok(())
}

fn retire_resolved_conflict_outbox(
    transaction: &Transaction<'_>,
    record_id: &str,
    resolved_at: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE mobile_note_outbox
             SET state = 'conflict', eligible_for_sync = 0,
                 superseded_at = COALESCE(superseded_at, ?1)
             WHERE record_id = ?2
               AND (eligible_for_sync = 1 OR state IN ('pending', 'sending'))",
            params![resolved_at, record_id],
        )
        .map_err(|error| error.to_string())?;
    let has_unretired_branch: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_outbox
               WHERE record_id = ?1 AND eligible_for_sync = 1
             )",
            [record_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_unretired_branch {
        Err(format!(
            "note {record_id} conflict resolution left an eligible local branch"
        ))
    } else {
        Ok(())
    }
}

fn set_remote_filing(
    transaction: &Transaction<'_>,
    record_id: &str,
    folder_id: Option<&str>,
    updated_at: i64,
) -> Result<(), InboxApplyError> {
    transaction
        .execute(
            "INSERT INTO mobile_note_filing (
               record_id, folder_id, previous_folder_id, filed_at, updated_at
             ) VALUES (?1, ?2, NULL, ?3, ?3)
             ON CONFLICT(record_id) DO UPDATE SET
               folder_id = excluded.folder_id,
               previous_folder_id = NULL,
               filed_at = excluded.filed_at,
               updated_at = excluded.updated_at",
            params![record_id, folder_id, updated_at],
        )
        .map_err(|error| inbox_sql_error("apply remote mobile note filing", error))?;
    Ok(())
}

fn ensure_incoming_folder_exists(
    transaction: &Transaction<'_>,
    library_id: &str,
    folder_id: Option<&str>,
) -> Result<(), InboxApplyError> {
    let Some(folder_id) = folder_id else {
        return Ok(());
    };
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_folders
               WHERE folder_id = ?1 AND library_id = ?2
                 AND lifecycle_state = 'active'
             )",
            params![folder_id, library_id],
            |row| row.get(0),
        )
        .map_err(|error| {
            InboxApplyError::operational(format!("inspect incoming mobile note folder: {error}"))
        })?;
    if exists {
        Ok(())
    } else {
        Err(InboxApplyError::semantic(format!(
            "mobile sync note references missing folder {folder_id}"
        )))
    }
}

fn quarantine_inbox_change(
    connection: &mut Connection,
    sequence: i64,
    _reason: &str,
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE mobile_sync_inbox
             SET state = 'quarantined', applied_at = ?1,
                 error_code = 'semantic_validation_failed'
             WHERE sequence = ?2 AND state IN ('received', 'applying')",
            params![now_millis()?, sequence],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE mobile_sync_state
             SET applied_cursor = ?1, sync_state = 'error',
                 last_error_code = 'semantic_validation_failed'
             WHERE singleton = 1 AND applied_cursor + 1 = ?1",
            [sequence],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn return_inbox_change_to_received(
    connection: &mut Connection,
    sequence: i64,
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE mobile_sync_inbox
             SET state = 'received', apply_started_at = NULL,
                 error_code = 'transient_apply_failed'
             WHERE sequence = ?1 AND state = 'applying'",
            [sequence],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("mobile sync inbox could not preserve its retry checkpoint".to_string());
    }
    transaction
        .execute(
            "UPDATE mobile_sync_state
             SET sync_state = 'error', last_error_code = 'transient_apply_failed'
             WHERE singleton = 1",
            [],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn load_open_conflict(
    transaction: &Transaction<'_>,
    record_id: &str,
) -> Result<Option<ConflictSnapshot>, String> {
    transaction
        .query_row(
            "SELECT conflicts.conflict_id, conflicts.record_id,
                    conflicts.local_title, conflicts.local_body,
                    conflicts.local_canonical_hash,
                    notes.created_at, notes.updated_at,
                    conflicts.local_lifecycle_state,
                    notes.trashed_at, notes.tombstoned_at,
                    conflicts.local_folder_id,
                    notes.authority, notes.scope, notes.scope_id,
                    notes.scope_class, notes.provenance_json,
                    conflicts.accepted_revision,
                    conflicts.accepted_version_id,
                    conflicts.accepted_content_hash,
                    conflicts.remote_title, conflicts.remote_body,
                    conflicts.remote_created_at, conflicts.remote_updated_at,
                    conflicts.remote_lifecycle_state,
                    conflicts.remote_trashed_at,
                    conflicts.remote_tombstoned_at,
                    conflicts.remote_folder_id, conflicts.remote_authority,
                    conflicts.remote_scope_id, conflicts.remote_scope_class
             FROM mobile_note_conflicts AS conflicts
             JOIN mobile_notes AS notes
               ON notes.record_id = conflicts.record_id
             WHERE conflicts.record_id = ?1 AND conflicts.state = 'open'",
            [record_id],
            |row| {
                Ok(ConflictSnapshot {
                    conflict_id: row.get(0)?,
                    record_id: row.get(1)?,
                    local_title: row.get(2)?,
                    local_body: row.get(3)?,
                    local_canonical_hash: row.get(4)?,
                    local_created_at: row.get(5)?,
                    local_updated_at: row.get(6)?,
                    local_lifecycle_state: row.get(7)?,
                    local_trashed_at: row.get(8)?,
                    local_tombstoned_at: row.get(9)?,
                    local_folder_id: row.get(10)?,
                    local_authority: row.get(11)?,
                    local_scope: row.get(12)?,
                    local_scope_id: row.get(13)?,
                    local_scope_class: row.get(14)?,
                    local_provenance_json: row.get(15)?,
                    accepted_revision: row.get(16)?,
                    accepted_version_id: row.get(17)?,
                    accepted_content_hash: row.get(18)?,
                    remote_title: row.get(19)?,
                    remote_body: row.get(20)?,
                    remote_created_at: row.get(21)?,
                    remote_updated_at: row.get(22)?,
                    remote_lifecycle_state: row.get(23)?,
                    remote_trashed_at: row.get(24)?,
                    remote_tombstoned_at: row.get(25)?,
                    remote_folder_id: row.get(26)?,
                    remote_authority: row.get(27)?,
                    remote_scope_id: row.get(28)?,
                    remote_scope_class: row.get(29)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn create_conflict_copy(
    transaction: &Transaction<'_>,
    identity: &ReplicaIdentity,
    conflict: &ConflictSnapshot,
) -> Result<String, String> {
    let record_id = new_uuid_v7();
    let branch_id = new_uuid_v7();
    let version_id = new_uuid_v7();
    let mutation_id = new_uuid_v7();
    let timestamp = next_timestamp(transaction)?.max(conflict.local_updated_at.saturating_add(1));
    let outbox_transaction = begin_outbox_transaction(transaction, 1)?;
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
               origin_install_id, conflict_of
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5,
               ?6, ?7, 'note', 1,
               0, NULL, NULL,
               1, ?8, ?9,
               0, ?10, 'pending',
               ?11, ?12, ?13, ?14,
               ?15, ?16, ?17, ?18, 'standard',
               ?19, ?20, ?20,
               ?21, ?22
             )",
            params![
                conflict.local_title,
                conflict.local_body,
                conflict.local_created_at,
                timestamp,
                conflict.local_trashed_at,
                identity.library_id,
                record_id,
                branch_id,
                version_id,
                mutation_id,
                conflict.local_lifecycle_state,
                conflict.local_trashed_at,
                conflict.local_tombstoned_at,
                conflict.local_canonical_hash,
                conflict.local_authority,
                conflict.local_scope,
                conflict.local_scope_id,
                conflict.local_scope_class,
                conflict.local_provenance_json,
                identity.device_id,
                identity.install_id,
                conflict.record_id,
            ],
        )
        .map_err(|error| error.to_string())?;
    let folder_id = if let Some(folder_id) = conflict.local_folder_id.as_deref() {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_note_folders
                   WHERE folder_id = ?1 AND library_id = ?2
                     AND lifecycle_state = 'active'
                 )",
                params![folder_id, identity.library_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        exists.then_some(folder_id)
    } else {
        None
    };
    set_remote_filing(transaction, &record_id, folder_id, timestamp)
        .map_err(InboxApplyError::into_string)?;
    if canonical_record_table_exists(transaction)? {
        let local_bytes: Vec<u8> = transaction
            .query_row(
                "SELECT working_record_json FROM mobile_canonical_record_v1
                 WHERE record_id = ?1",
                [&conflict.record_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let mut local = decode_exact_canonical_context_record(&local_bytes)?;
        local.record_id.clone_from(&record_id);
        local.library_id.clone_from(&identity.library_id);
        local.revision = 1;
        local.version_id.clone_from(&version_id);
        local.updated_at = rfc3339_from_millis(timestamp);
        local.content_hash = canonical_sha256(&local.content);
        local.validate()?;
        write_canonical_record_row(transaction, None, &local, "native_exact", timestamp)?;
    }
    enqueue_mutation(
        transaction,
        identity,
        &outbox_transaction,
        0,
        Mutation {
            operation: "create",
            patch_title_body: false,
            record_id: &record_id,
            title: &conflict.local_title,
            body: &conflict.local_body,
            base_revision: 0,
            proposed_revision: 1,
            local_revision: 1,
            version_id: &version_id,
            branch_id: &branch_id,
            base_version_id: None,
            accepted_content_hash: None,
            mutation_id: &mutation_id,
            canonical_hash: &conflict.local_canonical_hash,
            lifecycle_state: &conflict.local_lifecycle_state,
            trashed_at: conflict.local_trashed_at,
            tombstoned_at: conflict.local_tombstoned_at,
            created_at: conflict.local_created_at,
            updated_at: timestamp,
            authority: &conflict.local_authority,
            provenance_json: &conflict.local_provenance_json,
            scope_id: &conflict.local_scope_id,
            scope_class: &conflict.local_scope_class,
        },
    )?;
    Ok(record_id)
}

fn promote_canonical_accepted_to_working(
    transaction: &Transaction<'_>,
    record_id: &str,
) -> Result<(), String> {
    if !canonical_record_table_exists(transaction)? {
        return Ok(());
    }
    let (accepted_bytes, updated_at): (Vec<u8>, i64) = transaction
        .query_row(
            "SELECT accepted_record_json, updated_at
             FROM mobile_canonical_record_v1
             WHERE record_id = ?1 AND accepted_record_json IS NOT NULL",
            [record_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    let accepted = decode_exact_canonical_context_record(&accepted_bytes)?;
    write_canonical_record_row(
        transaction,
        Some(&accepted),
        &accepted,
        "native_exact",
        updated_at,
    )
}

fn materialize_conflict_remote(
    transaction: &Transaction<'_>,
    identity: &ReplicaIdentity,
    conflict: &ConflictSnapshot,
) -> Result<(), String> {
    ensure_incoming_folder_exists(
        transaction,
        &identity.library_id,
        conflict.remote_folder_id.as_deref(),
    )
    .map_err(InboxApplyError::into_string)?;
    let provenance_json = serde_json::to_string(&if conflict.remote_authority == "external" {
        serde_json::json!({
            "source": "external_authority",
            "transport": "direct_sync",
        })
    } else {
        serde_json::json!({"source": "direct_sync"})
    })
    .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE mobile_notes
             SET title = ?1, body = ?2, created_at = ?3, updated_at = ?4,
                 deleted_at = ?5,
                 accepted_revision = ?6,
                 accepted_version_id = ?7,
                 accepted_content_hash = ?8,
                 working_revision = ?6,
                 working_branch_id = ?7,
                 working_version_id = ?7,
                 working_base_revision = ?6,
                 pending_mutation_id = ?7,
                 sync_state = 'acknowledged',
                 lifecycle_state = ?9,
                 trashed_at = ?10,
                 tombstoned_at = ?11,
                 canonical_hash = ?8,
                 authority = ?12,
                 scope = ?13,
                 scope_id = ?14,
                 scope_class = ?13,
                 provenance_json = ?15,
                 conflict_of = NULL
             WHERE record_id = ?16 AND library_id = ?17",
            params![
                conflict.remote_title,
                conflict.remote_body,
                conflict.remote_created_at,
                conflict.remote_updated_at,
                conflict.remote_trashed_at,
                conflict.accepted_revision,
                conflict.accepted_version_id,
                conflict.accepted_content_hash,
                conflict.remote_lifecycle_state,
                conflict.remote_trashed_at,
                conflict.remote_tombstoned_at,
                conflict.remote_authority,
                conflict.remote_scope_class,
                conflict.remote_scope_id,
                provenance_json,
                conflict.record_id,
                identity.library_id,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(format!(
            "conflicted note {} no longer exists",
            conflict.record_id
        ));
    }
    set_remote_filing(
        transaction,
        &conflict.record_id,
        conflict.remote_folder_id.as_deref(),
        conflict.remote_updated_at,
    )
    .map_err(InboxApplyError::into_string)
}

fn normalized_workspace_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
    } else if user_version == 2 {
        verify_mobile_schema_v2(connection)?;
    } else if user_version == 3 {
        verify_mobile_schema_v3(connection)?;
    } else if user_version == 4 {
        verify_mobile_schema_v4(connection)?;
    } else if user_version == 5 {
        verify_mobile_schema_v5(connection)?;
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

fn canonical_context_record_bytes(record: &ContextRecordV1) -> Result<Vec<u8>, String> {
    record.validate()?;
    let value = serde_json::to_value(record)
        .map_err(|error| format!("serialize canonical context record: {error}"))?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.is_empty() || bytes.len() > MAX_MOBILE_CANONICAL_RECORD_BYTES {
        return Err("canonical context record exceeds the 512 KiB plaintext bound".to_string());
    }
    Ok(bytes)
}

fn decode_exact_canonical_context_record(bytes: &[u8]) -> Result<ContextRecordV1, String> {
    if bytes.is_empty() || bytes.len() > MAX_MOBILE_CANONICAL_RECORD_BYTES {
        return Err("canonical context record has an invalid byte length".to_string());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("decode canonical context record: {error}"))?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(
            "context record plaintext must be exact canonical JSON without duplicate fields"
                .to_string(),
        );
    }
    let record: ContextRecordV1 = serde_json::from_value(value)
        .map_err(|error| format!("decode ContextRecordV1: {error}"))?;
    record.validate()?;
    if !matches!(record.kind.as_str(), "note" | "category" | "folder") {
        return Err("mobile canonical storage does not support this record kind".to_string());
    }
    if record.record_schema_version != 1 {
        return Err("mobile canonical storage only supports record schema v1".to_string());
    }
    validate_canonical_record_projection_shape(&record)?;
    Ok(record)
}

fn validate_canonical_record_projection_shape(record: &ContextRecordV1) -> Result<(), String> {
    let content = record
        .content
        .as_object()
        .ok_or_else(|| "canonical record content must be an object".to_string())?;
    match record.kind.as_str() {
        "note" => {
            let title = content
                .get("title")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "canonical note content requires a string title".to_string())?;
            let body = content
                .get("body")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "canonical note content requires a string body".to_string())?;
            if title.len() > MAX_MOBILE_NOTE_TEXT_BYTES
                || body.len() > MAX_MOBILE_NOTE_TEXT_BYTES
                || content
                    .get("folderId")
                    .filter(|value| !value.is_null())
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|folder_id| !is_uuid_v7(folder_id))
                || content
                    .get("folderId")
                    .is_some_and(|value| !value.is_null() && value.as_str().is_none())
            {
                return Err("canonical note projection fields are invalid".to_string());
            }
        }
        "category" => {
            if content
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|name| name.trim().is_empty())
                || !content.contains_key("schema")
                || matches!(record.lifecycle.state, LifecycleState::Trash)
            {
                return Err("canonical category projection fields are invalid".to_string());
            }
        }
        "folder" => {
            if content
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|name| name.trim().is_empty())
                || content
                    .get("parentId")
                    .filter(|value| !value.is_null())
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|parent_id| !is_uuid_v7(parent_id))
                || content
                    .get("parentId")
                    .is_some_and(|value| !value.is_null() && value.as_str().is_none())
                || content.get("parentId").and_then(serde_json::Value::as_str)
                    == Some(record.record_id.as_str())
                || content
                    .get("position")
                    .and_then(serde_json::Value::as_i64)
                    .is_none_or(|position| position < 0)
                || matches!(record.lifecycle.state, LifecycleState::Trash)
            {
                return Err("canonical folder projection fields are invalid".to_string());
            }
        }
        _ => return Err("unsupported mobile canonical record kind".to_string()),
    }
    Ok(())
}

fn authority_from_storage(value: &str) -> Result<RecordAuthority, String> {
    let kind = match value {
        "noted" => AuthorityKind::Noted,
        "external" => AuthorityKind::External,
        "derived" => AuthorityKind::Derived,
        _ => return Err(format!("unsupported canonical record authority {value}")),
    };
    Ok(RecordAuthority {
        kind,
        origin: Some(if value == "noted" {
            "noted".to_string()
        } else {
            "mobile_projection_backfill".to_string()
        }),
    })
}

fn authority_storage_value(authority: &RecordAuthority) -> &'static str {
    match authority.kind {
        AuthorityKind::Noted => "noted",
        AuthorityKind::External => "external",
        AuthorityKind::Derived => "derived",
    }
}

fn scope_class_from_storage(value: &str) -> Result<ScopeClass, String> {
    scope_class(value)
}

fn lifecycle_from_projection(
    state: &str,
    trashed_at: Option<i64>,
    tombstoned_at: Option<i64>,
) -> Result<RecordLifecycle, String> {
    let lifecycle = match state {
        "active" => RecordLifecycle {
            state: LifecycleState::Active,
            trashed_at: None,
            tombstoned_at: None,
        },
        "trash" => RecordLifecycle {
            state: LifecycleState::Trash,
            trashed_at: Some(rfc3339_from_millis(
                trashed_at.ok_or_else(|| "trash projection is missing trashed_at".to_string())?,
            )),
            tombstoned_at: None,
        },
        "tombstone" => RecordLifecycle {
            state: LifecycleState::Tombstone,
            trashed_at: Some(rfc3339_from_millis(trashed_at.ok_or_else(|| {
                "tombstone projection is missing trashed_at".to_string()
            })?)),
            tombstoned_at: Some(rfc3339_from_millis(tombstoned_at.ok_or_else(|| {
                "tombstone projection is missing tombstoned_at".to_string()
            })?)),
        },
        _ => return Err(format!("unsupported canonical lifecycle {state}")),
    };
    Ok(lifecycle)
}

#[allow(clippy::too_many_arguments)]
fn synthesized_context_record(
    library_id: &str,
    record_id: &str,
    kind: &str,
    revision: i64,
    version_id: &str,
    created_at: i64,
    updated_at: i64,
    scope_id: &str,
    scope_class: &str,
    sensitivity: &str,
    authority: &str,
    content: serde_json::Value,
    provenance: serde_json::Value,
    lifecycle: RecordLifecycle,
) -> Result<ContextRecordV1, String> {
    ContextRecordV1::new(
        library_id.to_string(),
        record_id.to_string(),
        kind.to_string(),
        1,
        u64::try_from(revision).map_err(|_| "canonical revision is invalid".to_string())?,
        version_id.to_string(),
        rfc3339_from_millis(created_at),
        rfc3339_from_millis(updated_at),
        None,
        RecordScope {
            scope_id: scope_id.to_string(),
            class: scope_class_from_storage(scope_class)?,
        },
        sensitivity.to_string(),
        authority_from_storage(authority)?,
        content,
        provenance,
        lifecycle,
    )
}

fn write_canonical_record_row(
    connection: &Connection,
    accepted: Option<&ContextRecordV1>,
    working: &ContextRecordV1,
    backfill_provenance: &str,
    updated_at: i64,
) -> Result<(), String> {
    working.validate()?;
    if let Some(accepted) = accepted {
        accepted.validate()?;
        if accepted.library_id != working.library_id
            || accepted.record_id != working.record_id
            || accepted.kind != working.kind
            || accepted.record_schema_version != working.record_schema_version
        {
            return Err("canonical accepted and working records are inconsistent".to_string());
        }
    }
    let accepted_bytes = accepted.map(canonical_context_record_bytes).transpose()?;
    let working_bytes = canonical_context_record_bytes(working)?;
    let accepted_revision = accepted
        .map(|record| {
            i64::try_from(record.revision)
                .map_err(|_| "canonical accepted revision exceeds SQLite range".to_string())
        })
        .transpose()?;
    let working_revision = i64::try_from(working.revision)
        .map_err(|_| "canonical working revision exceeds SQLite range".to_string())?;
    let accepted_sha256 = accepted_bytes.as_deref().map(exact_sha256);
    let working_sha256 = exact_sha256(&working_bytes);
    connection
        .execute(
            "INSERT INTO mobile_canonical_record_v1 (
               record_id, library_id, record_kind,
               accepted_revision, accepted_version_id, accepted_content_hash,
               accepted_record_json, accepted_record_sha256,
               working_revision, working_version_id, working_content_hash,
               working_record_json, working_record_sha256,
               backfill_provenance, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
               ?9, ?10, ?11, ?12, ?13, ?14, ?15
             )
             ON CONFLICT(record_id) DO UPDATE SET
               accepted_revision = excluded.accepted_revision,
               accepted_version_id = excluded.accepted_version_id,
               accepted_content_hash = excluded.accepted_content_hash,
               accepted_record_json = excluded.accepted_record_json,
               accepted_record_sha256 = excluded.accepted_record_sha256,
               working_revision = excluded.working_revision,
               working_version_id = excluded.working_version_id,
               working_content_hash = excluded.working_content_hash,
               working_record_json = excluded.working_record_json,
               working_record_sha256 = excluded.working_record_sha256,
               backfill_provenance = excluded.backfill_provenance,
               updated_at = excluded.updated_at",
            params![
                working.record_id,
                working.library_id,
                working.kind,
                accepted_revision,
                accepted.map(|record| record.version_id.as_str()),
                accepted.map(|record| record.content_hash.as_str()),
                accepted_bytes,
                accepted_sha256,
                working_revision,
                working.version_id,
                working.content_hash,
                working_bytes,
                working_sha256,
                backfill_provenance,
                updated_at,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn rebind_staging_canonical_records(
    connection: &Connection,
    old_library_id: &str,
    new_library_id: &str,
    old_default_scope_id: &str,
    new_default_scope_id: &str,
) -> Result<(), String> {
    if !canonical_record_table_exists(connection)? {
        return Ok(());
    }
    let rows = connection
        .prepare(
            "SELECT record_id, accepted_record_json, working_record_json,
                    backfill_provenance, updated_at
             FROM mobile_canonical_record_v1 WHERE library_id = ?1
             ORDER BY record_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([old_library_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    for (record_id, accepted_bytes, working_bytes, provenance, updated_at) in rows {
        let mut accepted = accepted_bytes
            .as_deref()
            .map(decode_exact_canonical_context_record)
            .transpose()?;
        let mut working = decode_exact_canonical_context_record(&working_bytes)?;
        for record in accepted.iter_mut().chain(std::iter::once(&mut working)) {
            record.library_id = new_library_id.to_string();
            if record.scope.scope_id == old_default_scope_id {
                record.scope.scope_id = new_default_scope_id.to_string();
                record.scope.class = ScopeClass::Unknown;
            }
            record.validate()?;
        }
        connection
            .execute(
                "DELETE FROM mobile_canonical_record_v1
                 WHERE record_id = ?1 AND library_id = ?2",
                params![record_id, old_library_id],
            )
            .map_err(|error| error.to_string())?;
        write_canonical_record_row(
            connection,
            accepted.as_ref(),
            &working,
            &provenance,
            updated_at,
        )?;
    }
    Ok(())
}

fn legacy_outbox_record_content(
    connection: &Connection,
    record_id: &str,
    version_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM mobile_note_outbox
             WHERE record_id = ?1 AND version_id = ?2
             ORDER BY local_sequence DESC LIMIT 1",
            params![record_id, version_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|error| format!("decode legacy outbox payload: {error}"))?;
    Ok(value
        .get("proposed_record")
        .or_else(|| value.get("proposedRecord"))
        .and_then(|record| record.get("content"))
        .cloned())
}

fn backfill_canonical_records_v7(connection: &Connection) -> Result<(), String> {
    let identity = replica_identity(connection)?;
    let notes = connection
        .prepare(
            "SELECT notes.record_id, notes.title, notes.body,
                    notes.created_at, notes.updated_at,
                    notes.accepted_revision, notes.accepted_version_id,
                    notes.accepted_content_hash, notes.working_revision,
                    notes.working_version_id, notes.lifecycle_state,
                    notes.trashed_at, notes.tombstoned_at, notes.authority,
                    notes.scope_id, notes.scope_class, notes.sensitivity,
                    notes.provenance_json, filing.folder_id
             FROM mobile_notes AS notes
             LEFT JOIN mobile_note_filing AS filing ON filing.record_id = notes.record_id
             ORDER BY notes.record_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                        row.get::<_, Option<i64>>(12)?,
                        row.get::<_, String>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, String>(15)?,
                        row.get::<_, String>(16)?,
                        row.get::<_, String>(17)?,
                        row.get::<_, Option<String>>(18)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    for row in notes {
        let (
            record_id,
            title,
            body,
            created_at,
            updated_at,
            accepted_revision,
            accepted_version_id,
            accepted_content_hash,
            _working_revision,
            working_version_id,
            lifecycle_state,
            trashed_at,
            tombstoned_at,
            authority,
            scope_id,
            scope_class,
            sensitivity,
            provenance_json,
            folder_id,
        ) = row;
        let provenance: serde_json::Value = serde_json::from_str(&provenance_json)
            .map_err(|error| format!("decode mobile note provenance: {error}"))?;
        let lifecycle = lifecycle_from_projection(&lifecycle_state, trashed_at, tombstoned_at)?;
        let mut working_content = note_content(&title, &body);
        if let (Some(content), Some(folder_id)) =
            (working_content.as_object_mut(), folder_id.as_ref())
        {
            content.insert("folderId".to_string(), serde_json::json!(folder_id));
        }
        let pending: Option<(i64, String)> = connection
            .query_row(
                "SELECT proposed_revision, version_id FROM mobile_note_outbox
                 WHERE record_id = ?1 AND eligible_for_sync = 1
                 ORDER BY local_sequence DESC LIMIT 1",
                [&record_id],
                |outbox| Ok((outbox.get(0)?, outbox.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;

        let accepted = if accepted_revision > 0 {
            let accepted_version_id = accepted_version_id
                .as_deref()
                .ok_or_else(|| format!("accepted note {record_id} has no accepted version"))?;
            let expected_hash = accepted_content_hash
                .as_deref()
                .ok_or_else(|| format!("accepted note {record_id} has no accepted content hash"))?;
            let current_legacy_content = note_content(&title, &body);
            let accepted_content = if canonical_sha256(&current_legacy_content) == expected_hash {
                current_legacy_content
            } else if let Some(content) =
                legacy_outbox_record_content(connection, &record_id, accepted_version_id)?
            {
                content
            } else {
                let remote: Option<(String, String)> = connection
                    .query_row(
                        "SELECT remote_title, remote_body FROM mobile_note_conflicts
                         WHERE record_id = ?1 AND accepted_version_id = ?2
                         ORDER BY created_at DESC LIMIT 1",
                        params![record_id, accepted_version_id],
                        |conflict| Ok((conflict.get(0)?, conflict.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| error.to_string())?;
                remote
                    .map(|(title, body)| note_content(&title, &body))
                    .ok_or_else(|| {
                        format!(
                            "cannot losslessly reconstruct accepted note {record_id}; restore the v6 recovery snapshot or pull the accepted head again"
                        )
                    })?
            };
            if canonical_sha256(&accepted_content) != expected_hash {
                return Err(format!(
                    "accepted note {record_id} recovery evidence does not match its content hash"
                ));
            }
            Some(synthesized_context_record(
                &identity.library_id,
                &record_id,
                "note",
                accepted_revision,
                accepted_version_id,
                created_at,
                updated_at,
                &scope_id,
                &scope_class,
                &sensitivity,
                &authority,
                accepted_content,
                serde_json::json!({
                    "source": "mobile_v7_projection_backfill",
                    "legacyProvenance": provenance,
                }),
                lifecycle.clone(),
            )?)
        } else {
            None
        };
        let (working_revision, working_version_id) =
            pending.as_ref().cloned().unwrap_or_else(|| {
                (
                    accepted_revision.max(1),
                    accepted_version_id
                        .clone()
                        .unwrap_or_else(|| working_version_id.clone()),
                )
            });
        let accepted_matches_working = accepted
            .as_ref()
            .is_some_and(|record| record.content == working_content);
        let working = if pending.is_none() && accepted_matches_working {
            accepted
                .clone()
                .expect("accepted_matches_working requires an accepted record")
        } else {
            synthesized_context_record(
                &identity.library_id,
                &record_id,
                "note",
                working_revision,
                &working_version_id,
                created_at,
                updated_at,
                &scope_id,
                &scope_class,
                &sensitivity,
                &authority,
                working_content.clone(),
                provenance.clone(),
                lifecycle.clone(),
            )?
        };
        write_canonical_record_row(
            connection,
            accepted.as_ref(),
            &working,
            "v7_projection_backfill",
            updated_at,
        )?;
        connection
            .execute(
                "UPDATE mobile_notes SET canonical_hash = ?1 WHERE record_id = ?2",
                params![working.content_hash, record_id],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "UPDATE mobile_note_outbox SET canonical_hash = ?1
                 WHERE record_id = ?2 AND eligible_for_sync = 1",
                params![working.content_hash, record_id],
            )
            .map_err(|error| error.to_string())?;
    }

    let categories = connection
        .prepare(
            "SELECT category_id, name, schema_json, authority, created_at, updated_at,
                    lifecycle_state FROM mobile_note_categories ORDER BY category_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    for (record_id, name, schema_json, authority, created_at, updated_at, lifecycle) in categories {
        let schema: serde_json::Value = serde_json::from_str(&schema_json)
            .map_err(|error| format!("decode mobile category schema: {error}"))?;
        let version_id = deterministic_backfill_uuid_v7(
            updated_at.max(0) as u64,
            "noted.iphone-canonical-category.v7",
            &record_id,
        );
        let record = synthesized_context_record(
            &identity.library_id,
            &record_id,
            "category",
            1,
            &version_id,
            created_at,
            updated_at,
            &identity.default_scope_id,
            "unknown",
            "standard",
            &authority,
            serde_json::json!({"name": name, "schema": schema}),
            serde_json::json!({"source": "mobile_v7_projection_backfill"}),
            lifecycle_from_projection(&lifecycle, None, None)?,
        )?;
        write_canonical_record_row(
            connection,
            Some(&record),
            &record,
            "v7_projection_backfill",
            updated_at,
        )?;
    }

    let folders = connection
        .prepare(
            "SELECT folder_id, name, parent_folder_id, position, authority,
                    created_at, updated_at, lifecycle_state
             FROM mobile_note_folders ORDER BY folder_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    for (record_id, name, parent_id, position, authority, created_at, updated_at, lifecycle) in
        folders
    {
        let version_id = deterministic_backfill_uuid_v7(
            updated_at.max(0) as u64,
            "noted.iphone-canonical-folder.v7",
            &record_id,
        );
        let record = synthesized_context_record(
            &identity.library_id,
            &record_id,
            "folder",
            1,
            &version_id,
            created_at,
            updated_at,
            &identity.default_scope_id,
            "unknown",
            "standard",
            &authority,
            serde_json::json!({
                "name": name, "folderType": "manual", "parentId": parent_id,
                "autoRule": "", "position": position,
            }),
            serde_json::json!({"source": "mobile_v7_projection_backfill"}),
            lifecycle_from_projection(&lifecycle, None, None)?,
        )?;
        write_canonical_record_row(
            connection,
            Some(&record),
            &record,
            "v7_projection_backfill",
            updated_at,
        )?;
    }
    Ok(())
}

fn verify_mobile_canonical_records(connection: &Connection) -> Result<(), String> {
    let rows = connection
        .prepare(
            "SELECT record_id, library_id, record_kind,
                    accepted_revision, accepted_version_id, accepted_content_hash,
                    accepted_record_json, accepted_record_sha256,
                    working_revision, working_version_id, working_content_hash,
                    working_record_json, working_record_sha256, backfill_provenance
             FROM mobile_canonical_record_v1 ORDER BY record_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<Vec<u8>>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Vec<u8>>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, String>(13)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let identity = replica_identity(connection)?;
    for row in rows {
        let (
            record_id,
            library_id,
            kind,
            accepted_revision,
            accepted_version_id,
            accepted_content_hash,
            accepted_bytes,
            accepted_sha,
            working_revision,
            working_version_id,
            working_content_hash,
            working_bytes,
            working_sha,
            backfill_provenance,
        ) = row;
        if library_id != identity.library_id
            || !matches!(
                backfill_provenance.as_str(),
                "native_exact" | "v7_projection_backfill"
            )
            || exact_sha256(&working_bytes) != working_sha
        {
            return Err("mobile canonical record metadata or digest is invalid".to_string());
        }
        let working = decode_exact_canonical_context_record(&working_bytes)?;
        if working.record_id != record_id
            || working.library_id != library_id
            || working.kind != kind
            || working.revision != working_revision as u64
            || working.version_id != working_version_id
            || working.content_hash != working_content_hash
        {
            return Err(
                "mobile canonical working record does not match its indexed head".to_string(),
            );
        }
        if let Some(bytes) = accepted_bytes {
            if accepted_sha.as_deref() != Some(exact_sha256(&bytes).as_str()) {
                return Err("mobile canonical accepted record digest is invalid".to_string());
            }
            let accepted = decode_exact_canonical_context_record(&bytes)?;
            if accepted.record_id != record_id
                || accepted.library_id != library_id
                || accepted.kind != kind
                || Some(accepted.revision as i64) != accepted_revision
                || Some(accepted.version_id.as_str()) != accepted_version_id.as_deref()
                || Some(accepted.content_hash.as_str()) != accepted_content_hash.as_deref()
            {
                return Err(
                    "mobile canonical accepted record does not match its indexed head".to_string(),
                );
            }
        } else if accepted_revision.is_some()
            || accepted_version_id.is_some()
            || accepted_content_hash.is_some()
            || accepted_sha.is_some()
        {
            return Err("mobile canonical accepted record is only partially present".to_string());
        }
    }
    let coverage: (i64, i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM mobile_notes),
               (SELECT COUNT(*) FROM mobile_canonical_record_v1 WHERE record_kind = 'note'),
               (SELECT COUNT(*) FROM mobile_note_categories),
               (SELECT COUNT(*) FROM mobile_canonical_record_v1 WHERE record_kind = 'category'),
               (SELECT COUNT(*) FROM mobile_note_folders),
               (SELECT COUNT(*) FROM mobile_canonical_record_v1 WHERE record_kind = 'folder')",
            [],
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
        .map_err(|error| error.to_string())?;
    if coverage.0 != coverage.1 || coverage.2 != coverage.3 || coverage.4 != coverage.5 {
        return Err("mobile canonical records do not cover every product projection".to_string());
    }
    let writable_violation: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_note_outbox AS outbox
             JOIN mobile_canonical_record_v1 AS canonical USING (record_id)
             WHERE outbox.eligible_for_sync = 1
               AND json_extract(CAST(canonical.working_record_json AS TEXT), '$.authority.kind')
                   != 'noted'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if writable_violation != 0 {
        return Err(
            "external or derived canonical records entered the writable outbox".to_string(),
        );
    }
    Ok(())
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
    if !(1..=PORTABLE_SCHEMA_VERSION).contains(&target_version) {
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
    if user_version > target_version {
        return Err(format!(
            "mobile database schema {user_version} cannot migrate backward to {target_version}"
        ));
    }
    if user_version == PORTABLE_SCHEMA_VERSION {
        return verify_current_mobile_schema(connection);
    }
    if user_version < 0 {
        return Err("mobile database schema version cannot be negative".to_string());
    }
    if user_version == target_version {
        return match user_version {
            1 => verify_mobile_schema_v1(connection),
            2 => verify_mobile_schema_v2(connection),
            3 => verify_mobile_schema_v3(connection),
            4 => verify_mobile_schema_v4(connection),
            5 => verify_mobile_schema_v5(connection),
            6 => verify_mobile_schema_v6(connection),
            7 => verify_mobile_schema_v7(connection),
            _ => verify_current_mobile_schema(connection),
        };
    }
    if user_version == 7 {
        verify_mobile_schema_v7(connection)?;
        migrate_mobile_schema_v8(connection, recovery_path)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 6 {
        verify_mobile_schema_v6(connection)?;
        migrate_mobile_schema_v7(connection, recovery_path)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 5 {
        verify_mobile_schema_v5(connection)?;
        migrate_mobile_schema_v6(connection, recovery_path)?;
        if target_version == 6 {
            return verify_mobile_schema_v6(connection);
        }
        migrate_mobile_schema_v7(connection, None)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 4 {
        verify_mobile_schema_v4(connection)?;
        migrate_mobile_schema_v5(connection, recovery_path)?;
        if target_version == 5 {
            return verify_mobile_schema_v5(connection);
        }
        migrate_mobile_schema_v6(connection, None)?;
        if target_version == 6 {
            return verify_mobile_schema_v6(connection);
        }
        migrate_mobile_schema_v7(connection, None)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 3 {
        verify_mobile_schema_v3(connection)?;
        migrate_mobile_schema_v4(connection, recovery_path)?;
        if target_version == 4 {
            return verify_mobile_schema_v4(connection);
        }
        migrate_mobile_schema_v5(connection, None)?;
        if target_version == 5 {
            return verify_mobile_schema_v5(connection);
        }
        migrate_mobile_schema_v6(connection, None)?;
        if target_version == 6 {
            return verify_mobile_schema_v6(connection);
        }
        migrate_mobile_schema_v7(connection, None)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 2 {
        verify_mobile_schema_v2(connection)?;
        migrate_mobile_schema_v3(connection, recovery_path)?;
        if target_version == 3 {
            return verify_mobile_schema_v3(connection);
        }
        migrate_mobile_schema_v4(connection, None)?;
        if target_version == 4 {
            return verify_mobile_schema_v4(connection);
        }
        migrate_mobile_schema_v5(connection, None)?;
        if target_version == 5 {
            return verify_mobile_schema_v5(connection);
        }
        migrate_mobile_schema_v6(connection, None)?;
        if target_version == 6 {
            return verify_mobile_schema_v6(connection);
        }
        migrate_mobile_schema_v7(connection, None)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
        return verify_current_mobile_schema(connection);
    }
    if user_version == 1 {
        verify_mobile_schema_v1(connection)?;
        migrate_mobile_schema_v2(connection, recovery_path)?;
        if target_version == 2 {
            return verify_mobile_schema_v2(connection);
        }
        migrate_mobile_schema_v3(connection, None)?;
        if target_version == 3 {
            return verify_mobile_schema_v3(connection);
        }
        migrate_mobile_schema_v4(connection, None)?;
        if target_version == 4 {
            return verify_mobile_schema_v4(connection);
        }
        migrate_mobile_schema_v5(connection, None)?;
        if target_version == 5 {
            return verify_mobile_schema_v5(connection);
        }
        migrate_mobile_schema_v6(connection, None)?;
        if target_version == 6 {
            return verify_mobile_schema_v6(connection);
        }
        migrate_mobile_schema_v7(connection, None)?;
        if target_version == 7 {
            return verify_mobile_schema_v7(connection);
        }
        migrate_mobile_schema_v8(connection, None)?;
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
    if target_version == 2 {
        return verify_mobile_schema_v2(connection);
    }
    migrate_mobile_schema_v3(connection, None)?;
    if target_version == 3 {
        return verify_mobile_schema_v3(connection);
    }
    migrate_mobile_schema_v4(connection, None)?;
    if target_version == 4 {
        return verify_mobile_schema_v4(connection);
    }
    migrate_mobile_schema_v5(connection, None)?;
    if target_version == 5 {
        return verify_mobile_schema_v5(connection);
    }
    migrate_mobile_schema_v6(connection, None)?;
    if target_version == 6 {
        return verify_mobile_schema_v6(connection);
    }
    migrate_mobile_schema_v7(connection, None)?;
    if target_version == 7 {
        return verify_mobile_schema_v7(connection);
    }
    migrate_mobile_schema_v8(connection, None)?;
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

fn migrate_mobile_schema_v3(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v2(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    ensure_columns(&transaction, "mobile_notes", &[("conflict_of", "TEXT")])?;
    transaction
        .execute_batch(
            "CREATE TABLE mobile_note_categories (
               category_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL,
               name TEXT NOT NULL CHECK (length(trim(name)) > 0),
               normalized_name TEXT NOT NULL CHECK (length(normalized_name) > 0),
               schema_json TEXT NOT NULL DEFAULT '{}',
               authority TEXT NOT NULL CHECK (authority IN ('noted', 'external')),
               lifecycle_state TEXT NOT NULL DEFAULT 'active'
                 CHECK (lifecycle_state IN ('active', 'tombstone')),
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               UNIQUE(library_id, normalized_name)
             );
             CREATE TABLE mobile_note_folders (
               folder_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL,
               parent_folder_id TEXT REFERENCES mobile_note_folders(folder_id),
               name TEXT NOT NULL CHECK (length(trim(name)) > 0),
               normalized_name TEXT NOT NULL CHECK (length(normalized_name) > 0),
               position INTEGER NOT NULL DEFAULT 0,
               authority TEXT NOT NULL CHECK (authority IN ('noted', 'external')),
               lifecycle_state TEXT NOT NULL DEFAULT 'active'
                 CHECK (lifecycle_state IN ('active', 'tombstone')),
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               CHECK (parent_folder_id IS NULL OR parent_folder_id != folder_id),
               UNIQUE(library_id, parent_folder_id, normalized_name)
             );
             CREATE TABLE mobile_note_filing (
               record_id TEXT PRIMARY KEY REFERENCES mobile_notes(record_id),
               folder_id TEXT REFERENCES mobile_note_folders(folder_id),
               previous_folder_id TEXT REFERENCES mobile_note_folders(folder_id),
               filed_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               CHECK (folder_id IS NULL OR previous_folder_id IS NULL
                      OR folder_id != previous_folder_id)
             );
             CREATE TABLE mobile_sync_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               enrollment_state TEXT NOT NULL DEFAULT 'not_enrolled'
                 CHECK (enrollment_state IN ('not_enrolled', 'active', 'revoked')),
               sync_state TEXT NOT NULL DEFAULT 'not_enrolled'
                 CHECK (sync_state IN ('not_enrolled', 'idle', 'pending', 'syncing', 'conflict', 'error', 'revoked')),
               authority_generation INTEGER NOT NULL DEFAULT 1 CHECK (authority_generation > 0),
               purge_generation INTEGER NOT NULL DEFAULT 0 CHECK (purge_generation >= 0),
               downloaded_cursor INTEGER NOT NULL DEFAULT 0 CHECK (downloaded_cursor >= 0),
               applied_cursor INTEGER NOT NULL DEFAULT 0 CHECK (applied_cursor >= 0),
               last_synced_at INTEGER,
               last_error_code TEXT,
               CHECK (applied_cursor <= downloaded_cursor)
             );
             CREATE TABLE mobile_sync_inbox (
               sequence INTEGER PRIMARY KEY CHECK (sequence > 0),
               transaction_id TEXT NOT NULL UNIQUE,
               transaction_digest TEXT NOT NULL UNIQUE CHECK (length(transaction_digest) = 64),
               payload_json TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'received'
                 CHECK (state IN ('received', 'applying', 'applied', 'quarantined')),
               received_at INTEGER NOT NULL,
               apply_started_at INTEGER,
               applied_at INTEGER,
               error_code TEXT
             );
             CREATE TABLE mobile_note_conflicts (
               conflict_id TEXT PRIMARY KEY,
               record_id TEXT NOT NULL REFERENCES mobile_notes(record_id),
               local_branch_id TEXT NOT NULL,
               local_version_id TEXT NOT NULL,
               local_title TEXT NOT NULL,
               local_body TEXT NOT NULL,
               local_canonical_hash TEXT NOT NULL CHECK (length(local_canonical_hash) = 64),
               local_lifecycle_state TEXT NOT NULL,
               local_folder_id TEXT REFERENCES mobile_note_folders(folder_id),
               accepted_revision INTEGER NOT NULL CHECK (accepted_revision > 0),
               accepted_version_id TEXT NOT NULL,
               accepted_content_hash TEXT NOT NULL CHECK (length(accepted_content_hash) = 64),
               remote_title TEXT NOT NULL,
               remote_body TEXT NOT NULL,
               remote_created_at INTEGER NOT NULL,
               remote_updated_at INTEGER NOT NULL,
               remote_lifecycle_state TEXT NOT NULL,
               remote_trashed_at INTEGER,
               remote_tombstoned_at INTEGER,
               remote_folder_id TEXT REFERENCES mobile_note_folders(folder_id),
               remote_authority TEXT NOT NULL,
               remote_scope_id TEXT NOT NULL,
               remote_scope_class TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'open'
                 CHECK (state IN ('open', 'kept_copy', 'used_remote')),
               created_at INTEGER NOT NULL,
               resolved_at INTEGER
             );
             CREATE UNIQUE INDEX idx_mobile_note_conflicts_open
               ON mobile_note_conflicts(record_id) WHERE state = 'open';
             CREATE INDEX idx_mobile_note_folders_parent_position
               ON mobile_note_folders(parent_folder_id, position, name);
             CREATE INDEX idx_mobile_note_filing_folder
               ON mobile_note_filing(folder_id, record_id);
             CREATE INDEX idx_mobile_sync_inbox_state_sequence
               ON mobile_sync_inbox(state, sequence);",
        )
        .map_err(|error| error.to_string())?;

    let identity = replica_identity(&transaction)?;
    let (replica_created_at, migration_time): (i64, i64) = (
        transaction
            .query_row(
                "SELECT created_at FROM mobile_replica WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?,
        now_millis()?,
    );
    let default_folder_id = deterministic_backfill_uuid_v7(
        u64::try_from(replica_created_at.max(0)).unwrap_or(0),
        &format!("noted.iphone-folder.{}", identity.library_id),
        "notes",
    );
    transaction
        .execute(
            "INSERT INTO mobile_note_folders (
               folder_id, library_id, parent_folder_id, name, normalized_name,
               position, authority, lifecycle_state, created_at, updated_at
             ) VALUES (?1, ?2, NULL, 'Notes', 'notes', 0, 'noted', 'active', ?3, ?3)",
            params![
                default_folder_id,
                identity.library_id,
                replica_created_at.max(0)
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO mobile_sync_state (
               singleton, enrollment_state, sync_state, authority_generation,
               purge_generation, downloaded_cursor, applied_cursor
             ) VALUES (1, 'not_enrolled', 'not_enrolled', 1, 0, 0, 0)",
            [],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (3, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V3_NAME,
                PORTABLE_SCHEMA_V3_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 3,
                 min_reader_version = 3,
                 min_writer_version = 3,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 2",
            params![
                PORTABLE_SCHEMA_V3_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v3 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 3)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn migrate_mobile_schema_v4(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v3(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(PORTABLE_SCHEMA_V4_DDL)
        .map_err(|error| error.to_string())?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (4, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V4_NAME,
                PORTABLE_SCHEMA_V4_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v4 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 4,
                 min_reader_version = 4,
                 min_writer_version = 4,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 3",
            params![
                PORTABLE_SCHEMA_V4_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v4 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 4)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn migrate_mobile_schema_v5(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v4(connection)?;
    if load_mobile_pairing_checkpoint(connection)?
        .is_some_and(|checkpoint| checkpoint.client.state == PairingClientState::Active)
    {
        return Err(
            "an already-active v4 fixture pairing has no atomic activation record; reset pairing before schema v5 migration"
                .to_string(),
        );
    }
    let identity = replica_identity(connection)?;
    let enrollment_state: String = connection
        .query_row(
            "SELECT enrollment_state FROM mobile_sync_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if identity.library_state != "local_staging" || enrollment_state != "not_enrolled" {
        return Err(
            "a non-atomic v4 paired/enrolled fixture requires reset before schema v5 migration"
                .to_string(),
        );
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(PORTABLE_SCHEMA_V5_DDL)
        .map_err(|error| error.to_string())?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (5, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V5_NAME,
                PORTABLE_SCHEMA_V5_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v5 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 5,
                 min_reader_version = 5,
                 min_writer_version = 5,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 4",
            params![
                PORTABLE_SCHEMA_V5_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v5 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 5)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn migrate_mobile_schema_v6(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v5(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(PORTABLE_SCHEMA_V6_DDL)
        .map_err(|error| error.to_string())?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (6, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V6_NAME,
                PORTABLE_SCHEMA_V6_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v6 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 6,
                 min_reader_version = 6,
                 min_writer_version = 6,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 5",
            params![
                PORTABLE_SCHEMA_V6_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v6 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 6)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn migrate_mobile_schema_v7(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v6(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(PORTABLE_SCHEMA_V7_DDL)
        .map_err(|error| error.to_string())?;
    backfill_canonical_records_v7(&transaction)?;
    verify_mobile_canonical_records(&transaction)?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (7, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V7_NAME,
                PORTABLE_SCHEMA_V7_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v7 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 7,
                 min_reader_version = 7,
                 min_writer_version = 7,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 6",
            params![
                PORTABLE_SCHEMA_V7_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v7 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 7)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn migrate_mobile_schema_v8(
    connection: &mut Connection,
    recovery_path: Option<&Path>,
) -> Result<(), String> {
    verify_mobile_schema_v7(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(PORTABLE_SCHEMA_V8_DDL)
        .map_err(|error| error.to_string())?;

    let migration_time = now_millis()?;
    let inserted_history = transaction
        .execute(
            "INSERT INTO mobile_schema_migrations
               (version, name, checksum, migrated_at, product_version)
             VALUES (8, ?1, ?2, ?3, ?4)",
            params![
                PORTABLE_MIGRATION_V8_NAME,
                PORTABLE_SCHEMA_V8_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
            ],
        )
        .map_err(|error| error.to_string())?;
    if inserted_history != 1 {
        return Err("mobile schema v8 could not append its migration history".to_string());
    }
    let updated_state = transaction
        .execute(
            "UPDATE mobile_schema_state
             SET schema_version = 8,
                 min_reader_version = 8,
                 min_writer_version = 8,
                 migration_checksum = ?1,
                 migrated_at = ?2,
                 product_version = ?3,
                 migration_recovery_path = COALESCE(?4, migration_recovery_path)
             WHERE singleton = 1 AND schema_version = 7",
            params![
                PORTABLE_SCHEMA_V8_CHECKSUM,
                migration_time,
                env!("CARGO_PKG_VERSION"),
                recovery_path.map(|path| path.to_string_lossy().into_owned()),
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated_state != 1 {
        return Err("mobile schema v8 could not advance its compatibility stamp".to_string());
    }
    transaction
        .pragma_update(None, "user_version", 8)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn verify_mobile_schema_v2(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 2 {
        return Err(format!(
            "mobile schema v2 verifier expected user_version 2, found {user_version}"
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
    if state.0 != 2 {
        return Err(format!(
            "mobile schema stamp {} does not match user_version 2",
            state.0
        ));
    }
    if state.1 != 2 {
        return Err(format!(
            "mobile database reader protocol floor {} does not match schema 2",
            state.1
        ));
    }
    if state.2 != 2 {
        return Err(format!(
            "mobile database writer protocol floor {} does not match schema 2",
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
    if history_count != 2 {
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

fn verify_mobile_schema_v3(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 3 {
        return Err(format!(
            "mobile schema v3 verifier expected user_version 3, found {user_version}"
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
    if state.0 != 3 || state.1 != 3 || state.2 != 3 {
        return Err("mobile schema v3 compatibility floor is invalid".to_string());
    }
    if state.3 != PORTABLE_SCHEMA_V3_CHECKSUM {
        return Err("mobile schema v3 checksum does not match this binary".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history.0 != expected_name || history.1 != expected_checksum {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 3 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    let required_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'mobile_note_categories', 'mobile_note_folders', 'mobile_note_filing',
               'mobile_sync_state', 'mobile_sync_inbox', 'mobile_note_conflicts'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_conflict_of: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'conflict_of'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_tables != 6 || !has_conflict_of {
        return Err("mobile schema v3 is missing workspace or sync-state storage".to_string());
    }
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)
}

fn verify_mobile_schema_v4(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 4 {
        return Err(format!(
            "mobile schema v4 verifier expected user_version 4, found {user_version}"
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
    if state.1 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} is newer than this binary's {}",
            state.1, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.2 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} is newer than this binary's {}",
            state.2, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.0 != 4 || state.1 != 4 || state.2 != 4 {
        return Err("mobile schema v4 compatibility floor is invalid".to_string());
    }
    if state.3 != PORTABLE_SCHEMA_V4_CHECKSUM {
        return Err("mobile schema v4 checksum does not match this binary".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
        (
            4_i64,
            PORTABLE_MIGRATION_V4_NAME,
            PORTABLE_SCHEMA_V4_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history.0 != expected_name || history.1 != expected_checksum {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 4 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    let required_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'mobile_note_categories', 'mobile_note_folders', 'mobile_note_filing',
               'mobile_sync_state', 'mobile_sync_inbox', 'mobile_note_conflicts',
               'mobile_pairing_checkpoint_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_conflict_of: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'conflict_of'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_tables != 7 || !has_conflict_of {
        return Err(
            "mobile schema v4 is missing workspace, sync-state, or pairing storage".to_string(),
        );
    }
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)?;
    verify_mobile_pairing_checkpoint_schema(connection)
}

fn verify_mobile_schema_v5(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 5 {
        return Err(format!(
            "mobile schema v5 verifier expected user_version 5, found {user_version}"
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
    if state.1 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} is newer than this binary's {}",
            state.1, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.2 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} is newer than this binary's {}",
            state.2, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.0 != 5 || state.1 != 5 || state.2 != 5 {
        return Err("mobile schema v5 compatibility floor is invalid".to_string());
    }
    if state.3 != PORTABLE_SCHEMA_V5_CHECKSUM {
        return Err("mobile schema v5 checksum does not match this binary".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
        (
            4_i64,
            PORTABLE_MIGRATION_V4_NAME,
            PORTABLE_SCHEMA_V4_CHECKSUM,
        ),
        (
            5_i64,
            PORTABLE_MIGRATION_V5_NAME,
            PORTABLE_SCHEMA_V5_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history.0 != expected_name || history.1 != expected_checksum {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 5 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    let required_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'mobile_note_categories', 'mobile_note_folders', 'mobile_note_filing',
               'mobile_sync_state', 'mobile_sync_inbox', 'mobile_note_conflicts',
               'mobile_pairing_checkpoint_v1', 'mobile_pairing_activation_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_conflict_of: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'conflict_of'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_tables != 8 || !has_conflict_of {
        return Err(
            "mobile schema v5 is missing workspace, sync-state, or pairing activation storage"
                .to_string(),
        );
    }
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)?;
    verify_mobile_pairing_checkpoint_schema(connection)?;
    verify_mobile_pairing_activation_schema(connection)
}

fn verify_mobile_direct_sync_schema(connection: &Connection) -> Result<(), String> {
    let required_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'mobile_direct_sync_push_counter_v1',
               'mobile_direct_sync_journal_summary_v1',
               'mobile_direct_sync_request_v1',
               'mobile_direct_sync_push_binding_v1',
               'mobile_bootstrap_checkpoint_v1',
               'mobile_bootstrap_page_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let required_triggers: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'trigger' AND name IN (
               'mobile_direct_sync_request_identity_immutable',
               'mobile_direct_sync_response_immutable',
               'mobile_direct_sync_request_state_monotonic',
               'mobile_direct_sync_push_binding_identity_immutable',
               'mobile_direct_sync_push_binding_state_monotonic',
               'mobile_bootstrap_checkpoint_identity_immutable',
               'mobile_bootstrap_checkpoint_final_immutable',
               'mobile_bootstrap_checkpoint_state_monotonic',
               'mobile_bootstrap_page_identity_immutable',
               'mobile_bootstrap_page_state_monotonic'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_tables != 6 || required_triggers != 10 {
        return Err("mobile direct-sync schema is incomplete or tampered".to_string());
    }
    let counter_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_direct_sync_push_counter_v1
             WHERE singleton = 1 AND next_counter > 0",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let summary: (i64, i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT pruned_through_sequence, pruned_completed_count,
                    pruned_request_bytes, pruned_response_bytes,
                    max_pruned_push_counter, updated_at
             FROM mobile_direct_sync_journal_summary_v1 WHERE singleton = 1",
            [],
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
        .map_err(|error| format!("mobile direct-sync summary is invalid: {error}"))?;
    let summary_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_direct_sync_journal_summary_v1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if counter_rows != 1
        || summary_rows != 1
        || [
            summary.0, summary.1, summary.2, summary.3, summary.4, summary.5,
        ]
        .iter()
        .any(|value| *value < 0)
    {
        return Err("mobile direct-sync counter or compaction summary is invalid".to_string());
    }
    let stats: (i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(length(purpose_json) + length(request_bytes)
                                 + COALESCE(length(response_bytes), 0)), 0),
                    COALESCE(SUM(CASE WHEN state IN ('pending', 'response_received') THEN 1 ELSE 0 END), 0),
                    COALESCE(MAX(push_counter), 0), COALESCE(MAX(local_sequence), 0)
             FROM mobile_direct_sync_request_v1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(|error| error.to_string())?;
    if stats.0 > MAX_MOBILE_DIRECT_SYNC_ROWS
        || stats.1 > MAX_MOBILE_DIRECT_SYNC_TOTAL_BYTES
        || stats.2 > MAX_MOBILE_DIRECT_SYNC_OPEN_ROWS
    {
        return Err("mobile direct-sync journal exceeds its durable bounds".to_string());
    }
    let next_counter: i64 = connection
        .query_row(
            "SELECT next_counter FROM mobile_direct_sync_push_counter_v1 WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if next_counter != stats.3.max(summary.4) + 1 {
        return Err("mobile direct-sync push counter was rolled back or skipped".to_string());
    }
    let sqlite_sequence: i64 = connection
        .query_row(
            "SELECT COALESCE((
               SELECT seq FROM sqlite_sequence
               WHERE name = 'mobile_direct_sync_request_v1'
             ), 0)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if sqlite_sequence < stats.4.max(summary.0) {
        return Err("mobile direct-sync journal sequence was rolled back".to_string());
    }

    let request_keys = connection
        .prepare(
            "SELECT request_id, endpoint FROM mobile_direct_sync_request_v1
             ORDER BY local_sequence",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let checkpoint_ids = connection
        .prepare("SELECT checkpoint_id FROM mobile_bootstrap_checkpoint_v1 ORDER BY created_at")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let push_transaction_ids = connection
        .prepare(
            "SELECT transaction_id FROM mobile_direct_sync_push_binding_v1
             ORDER BY push_counter",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if push_transaction_ids.len() as i64 > MAX_MOBILE_DIRECT_SYNC_ROWS {
        return Err("mobile direct-sync push binding history exceeds its bound".to_string());
    }
    if checkpoint_ids.len() as i64 > MAX_MOBILE_BOOTSTRAP_CHECKPOINTS {
        return Err("mobile bootstrap checkpoint history exceeds its bound".to_string());
    }
    let open_checkpoints: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mobile_bootstrap_checkpoint_v1
             WHERE state IN ('receiving', 'received')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if open_checkpoints > 1 {
        return Err("mobile bootstrap has competing open checkpoints".to_string());
    }
    if request_keys.is_empty() && checkpoint_ids.is_empty() && push_transaction_ids.is_empty() {
        return Ok(());
    }
    for (request_id, endpoint) in request_keys {
        let request =
            load_direct_sync_request(connection, &request_id, &endpoint)?.ok_or_else(|| {
                "mobile direct-sync request disappeared during verification".to_string()
            })?;
        let binding =
            direct_sync_binding_for_activation_sha(connection, &request.activation_sha256)?;
        validate_direct_sync_request_row(&request, &binding)?;
        if let Some(transaction_id) = request.push_transaction_id.as_deref() {
            let push_binding = load_direct_sync_push_binding(connection, transaction_id)?
                .ok_or_else(|| {
                    "mobile direct-sync push request has no lifecycle binding".to_string()
                })?;
            validate_direct_sync_push_binding(&push_binding, &binding)?;
            if push_binding.request_id != request.request_id
                || push_binding.push_counter != request.push_counter.unwrap_or_default()
                || push_binding.request_sha256 != request.request_sha256
            {
                return Err("mobile direct-sync push request binding is inconsistent".to_string());
            }
        }
    }
    for transaction_id in push_transaction_ids {
        let push_binding = load_direct_sync_push_binding(connection, &transaction_id)?
            .ok_or_else(|| "mobile direct-sync push binding disappeared".to_string())?;
        let binding =
            direct_sync_binding_for_activation_sha(connection, &push_binding.activation_sha256)?;
        validate_direct_sync_push_binding(&push_binding, &binding)?;
        let request_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM mobile_direct_sync_request_v1
                   WHERE request_id = ?1 AND endpoint = '/sync/v1/push'
                     AND push_transaction_id = ?2 AND push_counter = ?3
                     AND request_sha256 = ?4
                 )",
                params![
                    push_binding.request_id,
                    push_binding.transaction_id,
                    push_binding.push_counter,
                    push_binding.request_sha256,
                ],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !request_exists {
            return Err("mobile direct-sync push binding lost its exact request".to_string());
        }
    }
    for checkpoint_id in checkpoint_ids {
        let recovery = bootstrap_recovery(connection, &checkpoint_id)?.ok_or_else(|| {
            "mobile bootstrap checkpoint disappeared during verification".to_string()
        })?;
        let binding = direct_sync_binding_for_activation_sha(
            connection,
            &recovery.checkpoint.activation_sha256,
        )?;
        validate_bootstrap_recovery(&recovery, &binding)?;
    }
    Ok(())
}

fn verify_mobile_schema_v6(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 6 {
        return Err(format!(
            "mobile schema v6 verifier expected user_version 6, found {user_version}"
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
    let state: (i64, i64, i64, String) = connection
        .query_row(
            "SELECT schema_version, min_reader_version, min_writer_version,
                    migration_checksum
             FROM mobile_schema_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| format!("mobile database compatibility stamp is invalid: {error}"))?;
    if state.1 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} is newer than this binary's {}",
            state.1, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.2 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} is newer than this binary's {}",
            state.2, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.0 != 6 || state.1 != 6 || state.2 != 6 {
        return Err("mobile schema v6 compatibility floor is invalid".to_string());
    }
    if state.3 != PORTABLE_SCHEMA_V6_CHECKSUM {
        return Err("mobile schema v6 checksum does not match this binary".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
        (
            4_i64,
            PORTABLE_MIGRATION_V4_NAME,
            PORTABLE_SCHEMA_V4_CHECKSUM,
        ),
        (
            5_i64,
            PORTABLE_MIGRATION_V5_NAME,
            PORTABLE_SCHEMA_V5_CHECKSUM,
        ),
        (
            6_i64,
            PORTABLE_MIGRATION_V6_NAME,
            PORTABLE_SCHEMA_V6_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history.0 != expected_name || history.1 != expected_checksum {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 6 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    let required_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'mobile_note_categories', 'mobile_note_folders', 'mobile_note_filing',
               'mobile_sync_state', 'mobile_sync_inbox', 'mobile_note_conflicts',
               'mobile_pairing_checkpoint_v1', 'mobile_pairing_activation_v1',
               'mobile_direct_sync_push_counter_v1',
               'mobile_direct_sync_journal_summary_v1',
               'mobile_direct_sync_request_v1', 'mobile_direct_sync_push_binding_v1',
               'mobile_bootstrap_checkpoint_v1',
               'mobile_bootstrap_page_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if required_tables != 14 {
        return Err(
            "mobile schema v6 is missing workspace, pairing, or direct-sync storage".to_string(),
        );
    }
    let has_conflict_of: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('mobile_notes') WHERE name = 'conflict_of'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_conflict_of {
        return Err("mobile schema v6 is missing workspace storage".to_string());
    }
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)?;
    verify_mobile_pairing_checkpoint_schema(connection)?;
    verify_mobile_pairing_activation_schema(connection)?;
    verify_mobile_direct_sync_schema(connection)
}

fn verify_mobile_schema_v7(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 7 {
        return Err(format!(
            "mobile schema v7 verifier expected user_version 7, found {user_version}"
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
    let state: (i64, i64, i64, String) = connection
        .query_row(
            "SELECT schema_version, min_reader_version, min_writer_version,
                    migration_checksum
             FROM mobile_schema_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| format!("mobile database compatibility stamp is invalid: {error}"))?;
    if state.1 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} is newer than this binary's {}",
            state.1, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.2 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} is newer than this binary's {}",
            state.2, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state != (7, 7, 7, PORTABLE_SCHEMA_V7_CHECKSUM.to_string()) {
        return Err("mobile schema v7 compatibility stamp is invalid".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
        (
            4_i64,
            PORTABLE_MIGRATION_V4_NAME,
            PORTABLE_SCHEMA_V4_CHECKSUM,
        ),
        (
            5_i64,
            PORTABLE_MIGRATION_V5_NAME,
            PORTABLE_SCHEMA_V5_CHECKSUM,
        ),
        (
            6_i64,
            PORTABLE_MIGRATION_V6_NAME,
            PORTABLE_SCHEMA_V6_CHECKSUM,
        ),
        (
            7_i64,
            PORTABLE_MIGRATION_V7_NAME,
            PORTABLE_SCHEMA_V7_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history != (expected_name.to_string(), expected_checksum.to_string()) {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 7 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    let canonical_schema: (i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM pragma_table_info('mobile_canonical_record_v1')),
               (SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger' AND name IN (
                 'mobile_canonical_record_identity_immutable',
                 'mobile_canonical_accepted_head_monotonic'
               ))",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    if canonical_schema != (15, 2) {
        return Err("mobile schema v7 canonical-record storage is incomplete".to_string());
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)?;
    verify_mobile_pairing_checkpoint_schema(connection)?;
    verify_mobile_pairing_activation_schema(connection)?;
    verify_mobile_direct_sync_schema(connection)?;
    verify_mobile_canonical_records(connection)
}

fn verify_mobile_schema_v8(connection: &Connection) -> Result<(), String> {
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if user_version != 8 {
        return Err(format!(
            "mobile schema v8 verifier expected user_version 8, found {user_version}"
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
    let state: (i64, i64, i64, String) = connection
        .query_row(
            "SELECT schema_version, min_reader_version, min_writer_version,
                    migration_checksum
             FROM mobile_schema_state WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| format!("mobile database compatibility stamp is invalid: {error}"))?;
    if state.1 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database reader protocol floor {} is newer than this binary's {}",
            state.1, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state.2 > PORTABLE_SCHEMA_VERSION {
        return Err(format!(
            "mobile database writer protocol floor {} is newer than this binary's {}",
            state.2, PORTABLE_SCHEMA_VERSION
        ));
    }
    if state != (8, 8, 8, PORTABLE_SCHEMA_V8_CHECKSUM.to_string()) {
        return Err("mobile schema v8 compatibility stamp is invalid".to_string());
    }
    for (version, expected_name, expected_checksum) in [
        (
            1_i64,
            PORTABLE_MIGRATION_V1_NAME,
            PORTABLE_SCHEMA_V1_CHECKSUM,
        ),
        (
            2_i64,
            PORTABLE_MIGRATION_V2_NAME,
            PORTABLE_SCHEMA_V2_CHECKSUM,
        ),
        (
            3_i64,
            PORTABLE_MIGRATION_V3_NAME,
            PORTABLE_SCHEMA_V3_CHECKSUM,
        ),
        (
            4_i64,
            PORTABLE_MIGRATION_V4_NAME,
            PORTABLE_SCHEMA_V4_CHECKSUM,
        ),
        (
            5_i64,
            PORTABLE_MIGRATION_V5_NAME,
            PORTABLE_SCHEMA_V5_CHECKSUM,
        ),
        (
            6_i64,
            PORTABLE_MIGRATION_V6_NAME,
            PORTABLE_SCHEMA_V6_CHECKSUM,
        ),
        (
            7_i64,
            PORTABLE_MIGRATION_V7_NAME,
            PORTABLE_SCHEMA_V7_CHECKSUM,
        ),
        (
            8_i64,
            PORTABLE_MIGRATION_V8_NAME,
            PORTABLE_SCHEMA_V8_CHECKSUM,
        ),
    ] {
        let history = connection
            .query_row(
                "SELECT name, checksum FROM mobile_schema_migrations WHERE version = ?1",
                [version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("mobile migration v{version} history is invalid: {error}"))?;
        if history != (expected_name.to_string(), expected_checksum.to_string()) {
            return Err(format!(
                "mobile migration v{version} history does not match this binary"
            ));
        }
    }
    let history_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if history_count != 8 {
        return Err("mobile migration history is not contiguous".to_string());
    }
    let revocation_schema: (i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM pragma_table_info('mobile_authority_revocation_v1')),
               (SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger' AND name IN (
                 'mobile_authority_revocation_immutable',
                 'mobile_direct_sync_push_binding_state_monotonic'
               ))",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    if revocation_schema != (17, 2) {
        return Err("mobile schema v8 authority-revocation storage is incomplete".to_string());
    }
    let revocation_activations = connection
        .prepare(
            "SELECT activation_sha256 FROM mobile_authority_revocation_v1
             ORDER BY activation_sha256",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    for activation_sha256 in revocation_activations {
        load_mobile_authority_revocation_by_activation(connection, &activation_sha256)?
            .ok_or_else(|| {
                "mobile authority revocation disappeared during verification".to_string()
            })?;
    }
    verify_migration_history_guards(connection)?;
    verify_mobile_database_integrity(connection)?;
    validate_replica_identity(&replica_identity(connection)?)?;
    validate_portable_notes(connection)?;
    validate_outbox_transaction_groups(connection)?;
    validate_mobile_workspace_state(connection)?;
    verify_mobile_pairing_checkpoint_schema(connection)?;
    verify_mobile_pairing_activation_schema(connection)?;
    verify_mobile_direct_sync_schema(connection)?;
    verify_mobile_canonical_records(connection)
}

fn verify_current_mobile_schema(connection: &Connection) -> Result<(), String> {
    verify_mobile_schema_v8(connection)
}

fn validate_mobile_workspace_state(connection: &Connection) -> Result<(), String> {
    let identity = replica_identity(connection)?;
    let sync_state: (String, String, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT enrollment_state, sync_state, authority_generation,
                    purge_generation, downloaded_cursor, applied_cursor
             FROM mobile_sync_state WHERE singleton = 1",
            [],
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
        .map_err(|error| format!("mobile sync state is invalid: {error}"))?;
    if !matches!(sync_state.0.as_str(), "not_enrolled" | "active" | "revoked")
        || !matches!(
            sync_state.1.as_str(),
            "not_enrolled" | "idle" | "pending" | "syncing" | "conflict" | "error" | "revoked"
        )
        || sync_state.2 <= 0
        || sync_state.3 < 0
        || sync_state.4 < 0
        || sync_state.5 < 0
        || sync_state.5 > sync_state.4
    {
        return Err("mobile sync state violates its generation or cursor floors".to_string());
    }

    let invalid_links: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM mobile_note_filing AS filing
                LEFT JOIN mobile_notes AS notes ON notes.record_id = filing.record_id
                WHERE notes.record_id IS NULL)
             + (SELECT COUNT(*) FROM mobile_note_conflicts AS conflicts
                LEFT JOIN mobile_notes AS notes ON notes.record_id = conflicts.record_id
                WHERE notes.record_id IS NULL)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid_links != 0 {
        return Err("mobile workspace contains orphaned note relationships".to_string());
    }
    let wrong_library_rows: i64 = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM mobile_note_categories WHERE library_id != ?1)
             + (SELECT COUNT(*) FROM mobile_note_folders WHERE library_id != ?1)",
            [&identity.library_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if wrong_library_rows != 0 {
        return Err("mobile organization rows belong to another library".to_string());
    }
    for (table, id_column) in [
        ("mobile_note_categories", "category_id"),
        ("mobile_note_folders", "folder_id"),
        ("mobile_note_conflicts", "conflict_id"),
    ] {
        let query = format!("SELECT {id_column} FROM {table}");
        let ids = connection
            .prepare(&query)
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        if ids.iter().any(|identifier| !is_uuid_v7(identifier)) {
            return Err(format!("{table} contains a non-UUIDv7 public identity"));
        }
    }
    let conflict_of_ids = connection
        .prepare("SELECT record_id, conflict_of FROM mobile_notes WHERE conflict_of IS NOT NULL")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if conflict_of_ids
        .iter()
        .any(|(record_id, conflict_of)| !is_uuid_v7(conflict_of) || record_id == conflict_of)
    {
        return Err("mobile conflict-copy references are invalid".to_string());
    }
    let folder_rows = connection
        .prepare(
            "SELECT folder_id, name, parent_folder_id
             FROM mobile_note_folders WHERE lifecycle_state = 'active'",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    let folder_index = folder_rows
        .into_iter()
        .map(|(folder_id, name, parent_id)| (folder_id, (name, parent_id)))
        .collect::<BTreeMap<_, _>>();
    for folder_id in folder_index.keys() {
        logical_folder_path(folder_id, &folder_index)?;
    }
    let inbox = connection
        .prepare("SELECT transaction_id, transaction_digest FROM mobile_sync_inbox")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| error.to_string())?;
    if inbox
        .iter()
        .any(|(transaction_id, digest)| !is_uuid(transaction_id) || !is_sha256(digest))
    {
        return Err("mobile sync inbox contains an invalid public identity or digest".to_string());
    }
    Ok(())
}

fn recover_interrupted_inbox(connection: &Connection) -> Result<(), String> {
    let recovered = connection
        .execute(
            "UPDATE mobile_sync_inbox
             SET state = 'received', apply_started_at = NULL,
                 error_code = 'interrupted_apply_recovered'
             WHERE state = 'applying'",
            [],
        )
        .map_err(|error| error.to_string())?;
    if recovered > 0 {
        connection
            .execute(
                "UPDATE mobile_sync_state
                 SET sync_state = CASE
                   WHEN enrollment_state = 'active' THEN 'pending'
                   ELSE sync_state
                 END,
                     last_error_code = 'interrupted_apply_recovered'
                 WHERE singleton = 1",
                [],
            )
            .map_err(|error| error.to_string())?;
    }
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
                  OR SUM(length(CAST(payload_json AS BLOB)) + ?1) > ?2
             )",
            params![
                MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES as i64,
                MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES as i64
            ],
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
                    patch_title_body: true,
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
                    authority: "noted",
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

fn ensure_no_open_note_conflict(connection: &Connection, record_id: &str) -> Result<(), String> {
    let has_open_conflict: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM mobile_note_conflicts
               WHERE record_id = ?1 AND state = 'open'
             )",
            [record_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_open_conflict {
        Err(format!(
            "note {record_id} must resolve its current conflict before it can change"
        ))
    } else {
        Ok(())
    }
}

fn begin_outbox_transaction(
    transaction: &Transaction<'_>,
    member_count: usize,
) -> Result<OutboxTransaction, String> {
    if member_count == 0 || member_count > MAX_MOBILE_TRANSACTION_MEMBERS {
        return Err(format!(
            "outbox transaction must contain between 1 and {MAX_MOBILE_TRANSACTION_MEMBERS} members"
        ));
    }
    let member_count = i64::try_from(member_count)
        .map_err(|_| "outbox transaction has too many members".to_string())?;
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

fn serialize_mutation_payload(
    identity: &ReplicaIdentity,
    mutation: &Mutation<'_>,
) -> Result<String, String> {
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
    let authority_kind = match mutation.authority {
        "noted" => AuthorityKind::Noted,
        "external" => AuthorityKind::External,
        "derived" => AuthorityKind::Derived,
        authority => return Err(format!("unsupported mobile note authority {authority}")),
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
            kind: authority_kind,
            origin: Some(
                provenance
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("iphone_native")
                    .to_string(),
            ),
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
    let ciphertext_bytes = payload_json
        .len()
        .checked_add(MOBILE_MUTATION_CIPHERTEXT_OVERHEAD_BYTES)
        .ok_or_else(|| "mobile note mutation ciphertext size overflowed".to_string())?;
    if ciphertext_bytes > MAX_MOBILE_TRANSACTION_CIPHERTEXT_BYTES {
        return Err(format!(
            "mobile note mutation exceeds the {MAX_MOBILE_MUTATION_PAYLOAD_BYTES}-byte upload ceiling after encryption reserve"
        ));
    }
    Ok(payload_json)
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
    if mutation.title.len() > MAX_MOBILE_NOTE_TEXT_BYTES
        || mutation.body.len() > MAX_MOBILE_NOTE_TEXT_BYTES
    {
        return Err(format!(
            "mobile note title and body must each be at most {MAX_MOBILE_NOTE_TEXT_BYTES} UTF-8 bytes"
        ));
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

    let payload_json = serialize_mutation_payload(identity, &mutation)?;

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
    update_canonical_working_note_for_mutation(transaction, identity, &mutation)?;
    Ok(())
}

fn canonical_record_table_exists(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'mobile_canonical_record_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn update_canonical_working_note_for_mutation(
    transaction: &Transaction<'_>,
    identity: &ReplicaIdentity,
    mutation: &Mutation<'_>,
) -> Result<(), String> {
    // enqueue_mutation also runs while old schemas are being upgraded. v7
    // backfills those rows after the table is created; only live v7 writes
    // enter this exact-record path.
    if !canonical_record_table_exists(transaction)? {
        return Ok(());
    }
    let existing: Option<(Option<Vec<u8>>, Vec<u8>, String)> = transaction
        .query_row(
            "SELECT accepted_record_json, working_record_json, backfill_provenance
             FROM mobile_canonical_record_v1 WHERE record_id = ?1",
            [mutation.record_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let accepted = existing
        .as_ref()
        .and_then(|(bytes, _, _)| bytes.as_deref())
        .map(decode_exact_canonical_context_record)
        .transpose()?;
    let backfill_provenance = existing
        .as_ref()
        .map(|(_, _, provenance)| provenance.as_str())
        .unwrap_or("native_exact");
    let mut working = if let Some((_, bytes, _)) = existing.as_ref() {
        decode_exact_canonical_context_record(bytes)?
    } else {
        synthesized_context_record(
            &identity.library_id,
            mutation.record_id,
            "note",
            mutation.proposed_revision,
            mutation.version_id,
            mutation.created_at,
            mutation.updated_at,
            mutation.scope_id,
            mutation.scope_class,
            "standard",
            mutation.authority,
            note_content(mutation.title, mutation.body),
            serde_json::from_str(mutation.provenance_json)
                .map_err(|error| format!("decode note provenance: {error}"))?,
            lifecycle_from_projection(
                mutation.lifecycle_state,
                mutation.trashed_at,
                mutation.tombstoned_at,
            )?,
        )?
    };
    if !matches!(working.authority.kind, AuthorityKind::Noted) {
        return Err("external and derived canonical records are write-blocked".to_string());
    }
    if mutation.patch_title_body {
        let content = working
            .content
            .as_object_mut()
            .ok_or_else(|| "canonical note content must remain an object".to_string())?;
        content.insert("title".to_string(), serde_json::json!(mutation.title));
        content.insert("body".to_string(), serde_json::json!(mutation.body));
    }
    working.revision = u64::try_from(mutation.proposed_revision)
        .map_err(|_| "canonical proposed revision is invalid".to_string())?;
    working.version_id = mutation.version_id.to_string();
    working.updated_at = rfc3339_from_millis(mutation.updated_at);
    working.lifecycle = lifecycle_from_projection(
        mutation.lifecycle_state,
        mutation.trashed_at,
        mutation.tombstoned_at,
    )?;
    working.content_hash = canonical_sha256(&working.content);
    working.validate()?;
    write_canonical_record_row(
        transaction,
        accepted.as_ref(),
        &working,
        backfill_provenance,
        mutation.updated_at,
    )?;
    transaction
        .execute(
            "UPDATE mobile_notes SET canonical_hash = ?1 WHERE record_id = ?2",
            params![working.content_hash, mutation.record_id],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE mobile_note_outbox SET canonical_hash = ?1
             WHERE mutation_id = ?2",
            params![working.content_hash, mutation.mutation_id],
        )
        .map_err(|error| error.to_string())?;
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

fn workspace_note_by_id(
    connection: &Connection,
    record_id: &str,
) -> Result<MobileWorkspaceNote, String> {
    connection
        .query_row(
            "SELECT notes.record_id, notes.title, notes.body,
                    notes.created_at, notes.updated_at,
                    filing.folder_id, folders.name,
                    notes.lifecycle_state, notes.sync_state,
                    notes.conflict_of, notes.authority,
                    EXISTS(
                      SELECT 1 FROM mobile_note_conflicts AS conflicts
                      WHERE conflicts.record_id = notes.record_id
                        AND conflicts.state = 'open'
                    ),
                    EXISTS(
                      SELECT 1 FROM mobile_sync_state
                      WHERE singleton = 1 AND enrollment_state = 'active'
                    )
             FROM mobile_notes AS notes
             LEFT JOIN mobile_note_filing AS filing
               ON filing.record_id = notes.record_id
             LEFT JOIN mobile_note_folders AS folders
               ON folders.folder_id = filing.folder_id
             WHERE notes.record_id = ?1",
            [record_id],
            |row| {
                let folder_id: Option<String> = row.get(5)?;
                Ok(MobileWorkspaceNote {
                    record_id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    folder_id: folder_id.clone(),
                    folder_name: row.get(6)?,
                    lifecycle_state: public_lifecycle_state(&row.get::<_, String>(7)?),
                    needs_filing: folder_id.is_none(),
                    sync_state: public_note_sync_state(&row.get::<_, String>(8)?, row.get(12)?),
                    conflict_of: row.get(9)?,
                    has_open_conflict: row.get(11)?,
                    read_only: row.get::<_, String>(10)? != "noted",
                })
            },
        )
        .map_err(|error| error.to_string())
}

fn public_lifecycle_state(stored: &str) -> String {
    match stored {
        "trash" => "trashed".to_string(),
        state => state.to_string(),
    }
}

fn public_note_sync_state(stored: &str, enrolled: bool) -> String {
    match stored {
        "conflict" => "conflict".to_string(),
        "restore_pending" => "restore_pending".to_string(),
        _ if !enrolled => "local".to_string(),
        "sending" | "syncing" => "syncing".to_string(),
        "acknowledged" | "synced" => "synced".to_string(),
        "pending" => "pending".to_string(),
        _ => "local".to_string(),
    }
}

fn logical_folder_path(
    folder_id: &str,
    folders: &BTreeMap<String, (String, Option<String>)>,
) -> Result<String, String> {
    let mut components = Vec::new();
    let mut visited = BTreeSet::new();
    let mut current = Some(folder_id.to_string());
    while let Some(current_id) = current.take() {
        if !visited.insert(current_id.clone()) {
            return Err(format!(
                "mobile folder hierarchy contains a cycle at {current_id}"
            ));
        }
        let (name, parent_id) = folders
            .get(&current_id)
            .ok_or_else(|| format!("mobile folder {current_id} has a missing ancestor"))?;
        components.push(name.clone());
        current.clone_from(parent_id);
    }
    components.reverse();
    Ok(components.join(" / "))
}

fn attach_organization_payload(
    transaction: &Transaction<'_>,
    mutation_id: &str,
    action: &str,
    folder_id: Option<&str>,
    previous_folder_id: Option<&str>,
) -> Result<(), String> {
    let payload_json: String = transaction
        .query_row(
            "SELECT payload_json FROM mobile_note_outbox WHERE mutation_id = ?1",
            [mutation_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut payload: serde_json::Value =
        serde_json::from_str(&payload_json).map_err(|error| error.to_string())?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "mobile mutation payload is not an object".to_string())?;
    object.insert(
        "organization".to_string(),
        serde_json::json!({
            "action": action,
            "folderId": folder_id,
            "previousFolderId": previous_folder_id,
        }),
    );
    transaction
        .execute(
            "UPDATE mobile_note_outbox SET payload_json = ?1 WHERE mutation_id = ?2",
            params![
                serde_json::to_string(&payload).map_err(|error| error.to_string())?,
                mutation_id
            ],
        )
        .map_err(|error| error.to_string())?;
    if canonical_record_table_exists(transaction)? {
        let (record_id, working_bytes): (String, Vec<u8>) = transaction
            .query_row(
                "SELECT canonical.record_id, canonical.working_record_json
                 FROM mobile_note_outbox AS outbox
                 JOIN mobile_canonical_record_v1 AS canonical USING (record_id)
                 WHERE outbox.mutation_id = ?1",
                [mutation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let mut working = decode_exact_canonical_context_record(&working_bytes)?;
        let content = working
            .content
            .as_object_mut()
            .ok_or_else(|| "canonical note content must remain an object".to_string())?;
        content.insert(
            "folderId".to_string(),
            folder_id.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        );
        working.content_hash = canonical_sha256(&working.content);
        let (accepted_bytes, backfill_provenance): (Option<Vec<u8>>, String) = transaction
            .query_row(
                "SELECT accepted_record_json, backfill_provenance
                 FROM mobile_canonical_record_v1
                 WHERE record_id = ?1",
                [&record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| error.to_string())?;
        let accepted = accepted_bytes
            .as_deref()
            .map(decode_exact_canonical_context_record)
            .transpose()?;
        write_canonical_record_row(
            transaction,
            accepted.as_ref(),
            &working,
            &backfill_provenance,
            now_millis()?,
        )?;
        transaction
            .execute(
                "UPDATE mobile_notes SET canonical_hash = ?1 WHERE record_id = ?2",
                params![working.content_hash, record_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE mobile_note_outbox SET canonical_hash = ?1
                 WHERE mutation_id = ?2",
                params![working.content_hash, mutation_id],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
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
    use crate::pairing_client::{
        ClientPublicIdentity, PairingActivation, PairingClientConfig, PairingConfirmation,
    };
    use crate::pairing_protocol::{
        bootstrap_envelope_digest, fixture_bootstrap_metadata, AuthenticatedHpkeEnvelope,
        EnrollmentReceipt, KindCapability, PairingRole, RecordKind,
        BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES, HPKE_ENCAPSULATED_KEY_BYTES, PAIRING_PROTOCOL,
        PAIRING_SUITE,
    };

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
            .lock_connection()
            .expect("open store")
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
    fn closed_store_does_not_touch_disk_and_opens_only_after_availability() {
        let path = temporary_path("closed-before-first-unlock");
        let store = MobileStore::closed(&path);

        assert_eq!(store.path(), path.as_path());
        assert!(!path.exists());
        assert_eq!(store.list(None), Err(MOBILE_STORE_LOCKED_ERROR.to_string()));

        store
            .protected_data_became_available()
            .expect("open after protected data becomes available");
        assert!(path.exists());
        assert!(store
            .protected_data_is_available()
            .expect("read available state"));
        let device_id = store
            .replica_device_id()
            .expect("load canonical replica identity for native key binding");
        assert!(is_uuid_v7(&device_id));

        drop(store);
        remove_database(&path);
    }

    fn fixture_pairing_checkpoint(store: &MobileStore) -> MobilePairingCheckpoint {
        let device_id = store
            .replica_device_id()
            .expect("fixture replica device ID");
        MobilePairingCheckpoint {
            identity_handle: "018f47a0-7b80-4000-8000-000000000001".to_string(),
            pending_bootstrap_handle: None,
            client: PairingClientCheckpoint {
                version: 1,
                config: PairingClientConfig {
                    environment: Environment::Development,
                    library_data_class: LibraryDataClass::SanitizedFixture,
                    requested_scopes: BTreeSet::from([RecordKind::Note]),
                    capabilities: BTreeMap::from([(
                        RecordKind::Note,
                        KindCapability {
                            reader_version: 1,
                            writer_version: Some(1),
                        },
                    )]),
                    display_name: "Fixture iPhone".to_string(),
                    app_version: "0.1.0".to_string(),
                    build_version: "1".to_string(),
                },
                state: PairingClientState::Ready,
                invitation_bytes: br#"{"fixture":"sanitized"}"#.to_vec(),
                identity: ClientPublicIdentity {
                    device_id,
                    signing_public_key: vec![4; 65],
                    hpke_public_key: vec![7; 32],
                },
                client_hello_bytes: None,
                server_hello_bytes: None,
                confirmation: None,
                user_decision: None,
                bootstrap_bytes: None,
                client_finish_bytes: None,
                server_finish_bytes: None,
                activation: None,
            },
            updated_at: 1_725_000_000_000,
        }
    }

    fn fixture_pairing_activation(store: &MobileStore) -> MobilePairingActivation {
        let device_id = store
            .replica_device_id()
            .expect("fixture replica device ID");
        let receipt_id = new_uuid_v7();
        let library_id = new_uuid_v7();
        let default_scope_id = new_uuid_v7();
        let invitation_id = new_uuid_v7();
        let scopes = fixture_record_scopes();
        let capabilities = fixture_record_capabilities();
        let receipt = EnrollmentReceipt {
            protocol: PAIRING_PROTOCOL.to_string(),
            suite: PAIRING_SUITE.to_string(),
            receipt_id: receipt_id.clone(),
            invitation_id: invitation_id.clone(),
            library_id: library_id.clone(),
            device_id: device_id.clone(),
            client_signing_key_fingerprint: Sha256::digest(vec![4_u8; 65]).to_vec(),
            client_hpke_key_fingerprint: Sha256::digest(vec![7_u8; 32]).to_vec(),
            mac_signing_key_fingerprint: Sha256::digest(vec![5_u8; 65]).to_vec(),
            mac_hpke_key_fingerprint: Sha256::digest(vec![6_u8; 32]).to_vec(),
            granted_scopes: scopes.clone(),
            capabilities: capabilities.clone(),
            authority_generation: 2,
            created_at_ms: 1_725_000_000_000,
            expires_at_ms: 1_725_000_060_000,
            transcript_digest: vec![5; 32],
            environment: Environment::Development,
            mac_role: PairingRole::MacAuthority,
            client_role: PairingRole::IphoneCompanion,
        };
        let invitation = Invitation {
            protocol: PAIRING_PROTOCOL.to_string(),
            suite: PAIRING_SUITE.to_string(),
            invitation_id,
            invitation_nonce: vec![6; 32],
            authority_signing_public_key: vec![4; 65],
            mac_pairing_signing_public_key: vec![5; 65],
            mac_pairing_hpke_public_key: vec![6; 32],
            tls_spki_sha256: vec![7; 32],
            library_id: library_id.clone(),
            authority_generation: 2,
            scope_ceiling: scopes.clone(),
            created_at_ms: 1_725_000_000_000,
            expires_at_ms: 1_725_000_060_000,
            environment: Environment::Development,
            authority_role: PairingRole::MacAuthority,
            intended_client_role: PairingRole::IphoneCompanion,
            library_data_class: LibraryDataClass::SanitizedFixture,
            authority_signature: vec![8; 64],
        };
        let server_hello = ServerHello {
            protocol: PAIRING_PROTOCOL.to_string(),
            suite: PAIRING_SUITE.to_string(),
            server_nonce: vec![9; 32],
            receipt: receipt.clone(),
            challenge: AuthenticatedHpkeEnvelope {
                encapsulated_key: vec![10; HPKE_ENCAPSULATED_KEY_BYTES],
                ciphertext: vec![11; 32],
            },
            sender_role: PairingRole::MacAuthority,
            recipient_role: PairingRole::IphoneCompanion,
            proof_signature: vec![12; 64],
        };
        let sync_spki_sha256 = vec![13; 32];
        let metadata =
            fixture_bootstrap_metadata(&receipt, 3, 4, &default_scope_id, &sync_spki_sha256)
                .expect("fixture bootstrap metadata");
        let mut bootstrap = BootstrapEnvelope {
            protocol: PAIRING_PROTOCOL.to_string(),
            receipt_id: receipt_id.clone(),
            metadata,
            sealed_key_package: AuthenticatedHpkeEnvelope {
                encapsulated_key: vec![14; HPKE_ENCAPSULATED_KEY_BYTES],
                ciphertext: vec![15; BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES],
            },
            envelope_digest: Vec::new(),
        };
        bootstrap.envelope_digest = bootstrap_envelope_digest(&bootstrap);
        let activated_at_ms = 1_725_000_020_000;
        let server_finish = ServerFinish {
            protocol: PAIRING_PROTOCOL.to_string(),
            suite: PAIRING_SUITE.to_string(),
            receipt: receipt.clone(),
            activated_at_ms,
            sender_role: PairingRole::MacAuthority,
            recipient_role: PairingRole::IphoneCompanion,
            signature: vec![16; 64],
        };
        let checkpoint = MobilePairingCheckpoint {
            identity_handle: "018f47a0-7b80-4000-8000-000000000001".to_string(),
            pending_bootstrap_handle: None,
            client: PairingClientCheckpoint {
                version: 1,
                config: PairingClientConfig {
                    environment: Environment::Development,
                    library_data_class: LibraryDataClass::SanitizedFixture,
                    requested_scopes: scopes.clone(),
                    capabilities: capabilities.clone(),
                    display_name: "Fixture iPhone".to_string(),
                    app_version: "0.1.0".to_string(),
                    build_version: "1".to_string(),
                },
                state: PairingClientState::Active,
                invitation_bytes: serde_json::to_vec(&invitation).expect("encode invitation"),
                identity: ClientPublicIdentity {
                    device_id: device_id.clone(),
                    signing_public_key: vec![4; 65],
                    hpke_public_key: vec![7; 32],
                },
                client_hello_bytes: Some(br#"{"fixture":"client-hello"}"#.to_vec()),
                server_hello_bytes: Some(
                    serde_json::to_vec(&server_hello).expect("encode server hello"),
                ),
                confirmation: Some(PairingConfirmation {
                    receipt_id: receipt_id.clone(),
                    verification_code: "12345678".to_string(),
                    granted_scopes: scopes.clone(),
                }),
                user_decision: Some(true),
                bootstrap_bytes: Some(serde_json::to_vec(&bootstrap).expect("encode bootstrap")),
                client_finish_bytes: Some(br#"{"fixture":"client-finish"}"#.to_vec()),
                server_finish_bytes: Some(
                    serde_json::to_vec(&server_finish).expect("encode server finish"),
                ),
                activation: Some(PairingActivation {
                    receipt,
                    activated_at_ms,
                }),
            },
            updated_at: 1_725_000_030_000,
        };
        MobilePairingActivation {
            receipt_id,
            library_id,
            device_id,
            default_scope_id,
            authority_generation: 2,
            purge_generation: 3,
            key_epoch: 4,
            sync_spki_sha256,
            record_cipher_suite: RECORD_CIPHER_SUITE.to_string(),
            granted_scopes: scopes,
            capabilities,
            checkpoint,
        }
    }

    fn save_pending_predecessor(store: &MobileStore, activation: &MobilePairingActivation) {
        let mut pending = activation.checkpoint.clone();
        pending.client.state = PairingClientState::PendingActivation;
        pending.client.activation = None;
        pending.pending_bootstrap_handle = Some("018f47a0-7b80-4000-8000-000000000002".to_string());
        pending.updated_at -= 1;
        store
            .save_pairing_checkpoint(&pending)
            .expect("save pending activation predecessor");
    }

    fn activate_fixture_store(store: &MobileStore) -> MobilePairingActivation {
        let activation = fixture_pairing_activation(store);
        save_pending_predecessor(store, &activation);
        store
            .finalize_pairing_activation(&activation)
            .expect("finalize fixture pairing activation");
        activation
    }

    fn purpose_json(value: serde_json::Value) -> Vec<u8> {
        canonical_json(&value).into_bytes()
    }

    #[test]
    fn direct_sync_journal_persists_exact_wire_bytes_status_and_restart_state() {
        let path = temporary_path("direct-sync-exact-journal");
        let store = MobileStore::open(&path).expect("open direct-sync store");
        activate_fixture_store(&store);
        assert_eq!(
            store.next_direct_sync_push_counter().expect("push counter"),
            1
        );

        let request_id = new_uuid_v7();
        let request_bytes = br#"{"request":"signed-pull"}"#.to_vec();
        let draft = MobileDirectSyncRequestDraft {
            request_id: request_id.clone(),
            endpoint: "/sync/v1/pull".to_string(),
            operation: "pull".to_string(),
            purpose_json: purpose_json(serde_json::json!({
                "operation": "pull",
                "requested_cursor": 0,
                "limit": 1,
                "requested_record_kinds": ["note"]
            })),
            push_transaction_id: None,
            push_counter: None,
            signed_request_bytes: request_bytes.clone(),
        };
        let prepared = store
            .prepare_direct_sync_request(&draft)
            .expect("prepare exact request");
        assert!(!prepared.replayed);
        assert_eq!(prepared.request.attempts, 0);
        assert_eq!(prepared.request.request_bytes, request_bytes);
        assert_eq!(prepared.request.request_content_type, "application/json");

        let replay = store
            .prepare_direct_sync_request(&draft)
            .expect("replay exact request");
        assert!(replay.replayed);
        let mut changed_purpose = draft.clone();
        changed_purpose.purpose_json = purpose_json(serde_json::json!({
            "operation": "pull",
            "requested_cursor": 0,
            "limit": 2,
            "requested_record_kinds": ["note"]
        }));
        assert!(store
            .prepare_direct_sync_request(&changed_purpose)
            .expect_err("purpose-different request replay must fail")
            .contains("byte-different"));
        let mut changed = draft.clone();
        changed.signed_request_bytes.push(b' ');
        assert!(store
            .prepare_direct_sync_request(&changed)
            .expect_err("byte-different request replay must fail")
            .contains("byte-different"));
        let second = MobileDirectSyncRequestDraft {
            request_id: new_uuid_v7(),
            ..draft.clone()
        };
        assert!(store
            .prepare_direct_sync_request(&second)
            .expect_err("only one exact request may be unresolved")
            .contains("durable bound"));
        assert_eq!(
            store
                .record_direct_sync_attempt(&request_id, "/sync/v1/pull")
                .expect("record actual send")
                .attempts,
            1
        );
        store
            .lock_connection()
            .expect("lock retry fixture")
            .execute(
                "UPDATE mobile_direct_sync_request_v1 SET attempts = 100
                 WHERE request_id = ?1",
                [&request_id],
            )
            .expect("reach retry pause ceiling");
        assert!(store
            .record_direct_sync_attempt(&request_id, "/sync/v1/pull")
            .expect_err("retry ceiling pauses rather than terminally changing state")
            .contains("remains pending"));
        assert_eq!(
            store
                .recover_direct_sync_requests()
                .expect("recover paused exact request")[0]
                .state,
            "pending"
        );
        drop(store);

        let reopened = MobileStore::open(&path).expect("recover exact request after restart");
        let recovered = reopened
            .recover_direct_sync_requests()
            .expect("load unresolved journal");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].request_bytes, request_bytes);
        let response = br#"{"error":{"code":"fixture_denied"}}"#;
        reopened
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/pull",
                403,
                "application/json",
                response,
            )
            .expect("persist authenticated HTTP response");
        drop(reopened);

        let reopened = MobileStore::open(&path).expect("recover stored response after restart");
        let recovered = reopened
            .recover_direct_sync_requests()
            .expect("recover exact response");
        assert_eq!(recovered[0].response_status, Some(403));
        assert_eq!(
            recovered[0].response_content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(
            recovered[0].response_bytes.as_deref(),
            Some(response.as_slice())
        );
        reopened
            .complete_direct_sync_request(&request_id, "/sync/v1/pull")
            .expect("complete semantically handled response");
        let pruned = reopened
            .prune_completed_direct_sync_requests(0)
            .expect("compact completed request");
        assert_eq!(pruned.pruned_completed_count, 1);
        assert_eq!(pruned.remaining_rows, 0);
        assert_eq!(
            reopened
                .next_direct_sync_push_counter()
                .expect("push counter"),
            1,
            "non-push requests must not consume the authority counter"
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn authenticated_revocation_atomically_disables_sync_and_preserves_export() {
        let path = temporary_path("durable-authority-revocation");
        let store = MobileStore::open(&path).expect("open revocation store");
        let note = store
            .create("Offline branch", "preserve me after revocation")
            .expect("create offline note");
        activate_fixture_store(&store);

        let request_id = new_uuid_v7();
        store
            .prepare_direct_sync_request(&MobileDirectSyncRequestDraft {
                request_id: request_id.clone(),
                endpoint: "/sync/v1/negotiate".to_string(),
                operation: "negotiate".to_string(),
                purpose_json: purpose_json(serde_json::json!({
                    "operation": "negotiate",
                    "capabilities_sha256": exact_sha256(b"fixture capabilities")
                })),
                push_transaction_id: None,
                push_counter: None,
                signed_request_bytes: br#"{"request":"signed-negotiate"}"#.to_vec(),
            })
            .expect("prepare revocation-bound request");
        let response = br#"{"error":{"code":"device_revoked"}}"#.to_vec();
        store
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/negotiate",
                403,
                "application/json",
                &response,
            )
            .expect("journal authenticated revocation response");

        let evidence = MobileAuthorityRevocationEvidence {
            request_id: request_id.clone(),
            endpoint: "/sync/v1/negotiate".to_string(),
            exact_response_bytes: response.clone(),
        };
        let applied = store
            .apply_authority_revocation(&evidence)
            .expect("apply durable authority revocation");
        assert!(!applied.replayed);
        assert_eq!(applied.retired_outbox_count, 1);
        assert_eq!(applied.quarantined_request_count, 1);
        assert_eq!(applied.revocation.request_id, request_id);
        assert_eq!(applied.revocation.response_sha256, exact_sha256(&response));

        let exported = store.export_notes().expect("export revoked offline branch");
        assert!(exported.contains(&note.record_id));
        assert!(exported.contains("preserve me after revocation"));
        assert!(store
            .recover_direct_sync_requests()
            .expect_err("revoked enrollment cannot recover network work")
            .contains("not active"));
        {
            let connection = store.lock_connection().expect("inspect revoked store");
            let state: (String, String, i64, String) = connection
                .query_row(
                    "SELECT sync.enrollment_state, sync.sync_state,
                            outbox.eligible_for_sync, request.state
                     FROM mobile_sync_state AS sync
                     JOIN mobile_note_outbox AS outbox
                     JOIN mobile_direct_sync_request_v1 AS request
                     WHERE sync.singleton = 1 AND request.request_id = ?1",
                    [&request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("load atomic revocation state");
            assert_eq!(
                state,
                ("revoked".into(), "revoked".into(), 0, "quarantined".into())
            );
        }

        let replay = store
            .apply_authority_revocation(&evidence)
            .expect("replay exact authority revocation");
        assert!(replay.replayed);
        let mut changed = evidence.clone();
        changed.exact_response_bytes.push(b' ');
        assert!(store
            .apply_authority_revocation(&changed)
            .expect_err("reject byte-different revocation replay")
            .contains("byte-different"));
        drop(store);

        let reopened = MobileStore::open(&path).expect("verify revoked store after restart");
        assert!(reopened
            .export_notes()
            .expect("export after revoked restart")
            .contains(&note.record_id));
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn canonical_pull_round_trips_unknown_fields_and_local_edit_patches_only_owned_fields() {
        let store = store();
        let activation = activate_fixture_store(&store);
        let record_id = new_uuid_v7();
        let version_id = new_uuid_v7();
        let mut remote = ContextRecordV1::new(
            activation.library_id.clone(),
            record_id.clone(),
            "note".to_string(),
            1,
            1,
            version_id,
            "2026-08-17T10:00:00.000Z".to_string(),
            "2026-08-17T10:00:00.000Z".to_string(),
            None,
            RecordScope {
                scope_id: activation.default_scope_id.clone(),
                class: ScopeClass::Unknown,
            },
            "standard".to_string(),
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: Some("noted".to_string()),
            },
            serde_json::json!({
                "title": "Remote",
                "body": "Exact",
                "futureContent": {"nested": [1, 2, {"kept": true}]},
            }),
            serde_json::json!({
                "source": "fixture",
                "futureProvenance": {"opaque": "keep"},
            }),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )
        .expect("construct canonical remote note");
        remote.extensions.insert(
            "example.test/future".to_string(),
            serde_json::json!({"unknown": ["a", "b"]}),
        );
        remote.validate().expect("validate extension record");
        let exact_remote = canonical_context_record_bytes(&remote).expect("canonical remote bytes");
        let change = MobileCanonicalPullChange {
            sequence: 1,
            transaction_id: new_uuid_v7(),
            transaction_digest: exact_sha256(b"authenticated fixture transaction"),
            library_id: activation.library_id.clone(),
            source_device_id: new_uuid_v7(),
            authority_generation: activation.authority_generation,
            purge_generation: activation.purge_generation,
            record_bytes: vec![exact_remote.clone()],
        };
        let applied = store
            .apply_canonical_pull_change(&change)
            .expect("apply canonical pull");
        assert_eq!(applied.applied_count, 1);
        assert_eq!(
            store.canonical_sync_cursors().expect("canonical cursors"),
            (1, 1)
        );
        assert_eq!(
            store
                .canonical_record(&record_id)
                .expect("read canonical record")
                .expect("canonical record exists")
                .accepted_record_bytes,
            Some(exact_remote)
        );

        store
            .update(&record_id, "Edited locally", "Owned fields only")
            .expect("edit canonical note");
        let stored = store
            .canonical_record(&record_id)
            .expect("read edited canonical record")
            .expect("edited canonical record exists");
        let edited = decode_exact_canonical_context_record(&stored.working_record_bytes)
            .expect("decode edited canonical record");
        assert_eq!(edited.content["title"], "Edited locally");
        assert_eq!(edited.content["body"], "Owned fields only");
        assert_eq!(
            edited.content["futureContent"],
            serde_json::json!({"nested": [1, 2, {"kept": true}]})
        );
        assert_eq!(
            edited.provenance["futureProvenance"],
            serde_json::json!({"opaque": "keep"})
        );
        assert_eq!(
            edited.extensions["example.test/future"],
            serde_json::json!({"unknown": ["a", "b"]})
        );
        let groups = store
            .eligible_canonical_outbox_transaction_groups(1)
            .expect("load canonical outbox");
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].mutations[0].proposed_record_bytes,
            stored.working_record_bytes
        );
        assert_eq!(groups[0].mutations[0].operation, "update");

        let mut noncanonical = groups[0].mutations[0].proposed_record_bytes.clone();
        noncanonical.push(b' ');
        let bad_change = MobileCanonicalPullChange {
            sequence: 2,
            transaction_id: new_uuid_v7(),
            transaction_digest: exact_sha256(b"bad canonical fixture transaction"),
            library_id: activation.library_id,
            source_device_id: new_uuid_v7(),
            authority_generation: activation.authority_generation,
            purge_generation: activation.purge_generation,
            record_bytes: vec![noncanonical],
        };
        assert!(store.apply_canonical_pull_change(&bad_change).is_err());
        assert_eq!(
            store.canonical_sync_cursors().expect("unchanged cursors"),
            (1, 1)
        );
    }

    #[test]
    fn canonical_external_and_derived_records_are_write_blocked() {
        for authority in [AuthorityKind::External, AuthorityKind::Derived] {
            let store = store();
            let activation = activate_fixture_store(&store);
            let record_id = new_uuid_v7();
            let record = ContextRecordV1::new(
                activation.library_id.clone(),
                record_id.clone(),
                "note".to_string(),
                1,
                1,
                new_uuid_v7(),
                "2026-08-17T10:00:00.000Z".to_string(),
                "2026-08-17T10:00:00.000Z".to_string(),
                None,
                RecordScope {
                    scope_id: activation.default_scope_id.clone(),
                    class: ScopeClass::Unknown,
                },
                "standard".to_string(),
                RecordAuthority {
                    kind: authority,
                    origin: Some("fixture.external/source".to_string()),
                },
                note_content("Read only", "Authoritative elsewhere"),
                serde_json::json!({"source": "fixture"}),
                RecordLifecycle {
                    state: LifecycleState::Active,
                    trashed_at: None,
                    tombstoned_at: None,
                },
            )
            .expect("construct read-only record");
            store
                .apply_canonical_pull_change(&MobileCanonicalPullChange {
                    sequence: 1,
                    transaction_id: new_uuid_v7(),
                    transaction_digest: exact_sha256(record_id.as_bytes()),
                    library_id: activation.library_id,
                    source_device_id: new_uuid_v7(),
                    authority_generation: activation.authority_generation,
                    purge_generation: activation.purge_generation,
                    record_bytes: vec![
                        canonical_context_record_bytes(&record).expect("canonical read-only bytes")
                    ],
                })
                .expect("apply read-only record");
            let error = store
                .update(&record_id, "No", "Mutation")
                .expect_err("read-only canonical record must reject local edits");
            assert!(
                error.contains("authority") || error.contains("write-blocked"),
                "{error}"
            );
        }
    }

    #[test]
    fn direct_sync_response_replay_is_exact_and_wire_caps_are_enforced() {
        let store = store();
        activate_fixture_store(&store);
        let request_id = new_uuid_v7();
        let maximum_request = vec![b'r'; MAX_MOBILE_DIRECT_SYNC_REQUEST_BYTES];
        let draft = MobileDirectSyncRequestDraft {
            request_id: request_id.clone(),
            endpoint: "/sync/v1/checkpoint".to_string(),
            operation: "checkpoint".to_string(),
            purpose_json: purpose_json(serde_json::json!({
                "operation": "checkpoint",
                "known_cursor": null
            })),
            push_transaction_id: None,
            push_counter: None,
            signed_request_bytes: maximum_request,
        };
        store
            .prepare_direct_sync_request(&draft)
            .expect("four-MiB direct-sync request is persistable");
        let maximum_response = vec![b's'; MAX_MOBILE_DIRECT_SYNC_RESPONSE_BYTES];
        store
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/checkpoint",
                200,
                "application/json",
                &maximum_response,
            )
            .expect("four-MiB direct-sync response is persistable");
        assert!(store
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/checkpoint",
                403,
                "application/json",
                br#"{"error":"different"}"#,
            )
            .expect_err("different second response must fail closed")
            .contains("quarantined"));
        let stored = load_direct_sync_request(
            &store.lock_connection().expect("lock replay store"),
            &request_id,
            "/sync/v1/checkpoint",
        )
        .expect("load response replay row")
        .expect("response replay row");
        assert_eq!(stored.state, "quarantined");
        assert_eq!(stored.response_status, Some(200));
        assert_eq!(stored.response_bytes, Some(maximum_response));

        let mut oversized = draft;
        oversized.request_id = new_uuid_v7();
        oversized.signed_request_bytes = vec![0; MAX_MOBILE_DIRECT_SYNC_REQUEST_BYTES + 1];
        assert!(store
            .prepare_direct_sync_request(&oversized)
            .expect_err("oversized direct-sync request must fail before persistence")
            .contains("oversized"));
    }

    #[test]
    fn direct_sync_purpose_must_be_canonical_and_match_its_stored_digest() {
        let store = store();
        activate_fixture_store(&store);
        let request_id = new_uuid_v7();
        let draft = MobileDirectSyncRequestDraft {
            request_id: request_id.clone(),
            endpoint: "/sync/v1/pull".to_string(),
            operation: "pull".to_string(),
            purpose_json: purpose_json(serde_json::json!({
                "operation": "pull",
                "requested_cursor": 0,
                "limit": 1,
                "requested_record_kinds": ["note"]
            })),
            push_transaction_id: None,
            push_counter: None,
            signed_request_bytes: br#"{"request":"purpose-bound"}"#.to_vec(),
        };
        let mut noncanonical = draft.clone();
        noncanonical.request_id = new_uuid_v7();
        noncanonical.purpose_json =
            br#"{ "operation": "pull", "requested_cursor": 0, "limit": 1, "requested_record_kinds": ["note"] }"#
                .to_vec();
        assert!(store
            .prepare_direct_sync_request(&noncanonical)
            .expect_err("non-canonical purpose must fail before persistence")
            .contains("not canonical"));

        store
            .prepare_direct_sync_request(&draft)
            .expect("persist canonical purpose");
        {
            let connection = store.lock_connection().expect("lock purpose tamper store");
            connection
                .execute(
                    "DROP TRIGGER mobile_direct_sync_request_identity_immutable",
                    [],
                )
                .expect("disable immutability only for tamper fixture");
            connection
                .execute(
                    "UPDATE mobile_direct_sync_request_v1
                     SET purpose_sha256 = ?1 WHERE request_id = ?2",
                    params!["0".repeat(64), request_id],
                )
                .expect("simulate purpose digest corruption");
        }
        assert!(store
            .recover_direct_sync_requests()
            .expect_err("purpose digest corruption must fail closed")
            .contains("journal row is invalid"));
    }

    #[test]
    fn direct_sync_push_claim_survives_crash_until_pull_echo_then_compacts() {
        let path = temporary_path("direct-sync-push-lifecycle");
        let store = MobileStore::open(&path).expect("open push lifecycle store");
        let note = store
            .create("Offline edit", "must upload once")
            .expect("create staging note");
        activate_fixture_store(&store);
        let group = store
            .eligible_outbox_transaction_groups(1)
            .expect("load eligible outbox group")
            .into_iter()
            .next()
            .expect("eligible group");
        let request_id = new_uuid_v7();
        let draft = MobileDirectSyncRequestDraft {
            request_id: request_id.clone(),
            endpoint: "/sync/v1/push".to_string(),
            operation: "push".to_string(),
            purpose_json: purpose_json(serde_json::json!({
                "operation": "push",
                "transaction_id": group.transaction_id.clone(),
                "transaction_digest": exact_sha256(b"push transaction fixture"),
                "device_transaction_counter": 1
            })),
            push_transaction_id: Some(group.transaction_id.clone()),
            push_counter: Some(1),
            signed_request_bytes: br#"{"request":"signed-push"}"#.to_vec(),
        };
        store
            .prepare_direct_sync_request(&draft)
            .expect("claim push group and counter");
        assert!(store
            .eligible_outbox_transaction_groups(1)
            .expect("reload eligible groups")
            .is_empty());
        assert_eq!(
            store
                .direct_sync_push_binding(&group.transaction_id)
                .expect("load push binding")
                .expect("push binding")
                .state,
            "sending"
        );
        store
            .record_direct_sync_attempt(&request_id, "/sync/v1/push")
            .expect("record push send");
        drop(store);

        let reopened = MobileStore::open(&path).expect("recover push after crash");
        let pending = reopened
            .recover_direct_sync_requests()
            .expect("recover exact push");
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].push_transaction_id.as_deref(),
            Some(group.transaction_id.as_str())
        );
        reopened
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/push",
                200,
                "application/json",
                br#"{"receipt":"accepted"}"#,
            )
            .expect("store accepted push response");
        reopened
            .complete_direct_sync_push_request(
                &request_id,
                MobileDirectSyncPushDisposition::AcceptedAwaitingEcho,
                None,
            )
            .expect("commit accepted push disposition");
        assert_eq!(
            reopened
                .direct_sync_push_binding(&group.transaction_id)
                .expect("load awaiting echo")
                .expect("awaiting echo binding")
                .state,
            "awaiting_echo"
        );
        assert_eq!(
            reopened
                .prune_completed_direct_sync_requests(0)
                .expect("try compaction before echo")
                .pruned_completed_count,
            0,
            "completed push evidence must survive until its pull echo"
        );
        let pending_mutation_id = group.mutations[0].mutation_id.clone();
        {
            let mut connection = reopened.lock_connection().expect("lock echo store");
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .expect("begin echo transaction");
            acknowledge_local_outbox_group(
                &transaction,
                &note.record_id,
                &pending_mutation_id,
                now_millis().expect("echo timestamp"),
            )
            .expect("settle pull echo");
            transaction.commit().expect("commit pull echo");
        }
        assert_eq!(
            reopened
                .direct_sync_push_binding(&group.transaction_id)
                .expect("load acknowledged binding")
                .expect("acknowledged binding")
                .state,
            "acknowledged"
        );
        assert_eq!(
            reopened
                .prune_completed_direct_sync_requests(0)
                .expect("compact settled push")
                .pruned_completed_count,
            1
        );
        assert!(reopened
            .direct_sync_push_binding(&group.transaction_id)
            .expect("read compacted binding")
            .is_none());
        assert_eq!(
            reopened
                .next_direct_sync_push_counter()
                .expect("next push counter"),
            2
        );

        drop(reopened);
        MobileStore::open(&path).expect("compacted push summaries survive restart");
        remove_database(&path);
    }

    #[test]
    fn rejected_push_retires_the_group_without_discarding_its_content() {
        let store = store();
        store
            .create("Rejected edit", "content remains local")
            .expect("create rejected staging note");
        activate_fixture_store(&store);
        let group = store
            .eligible_outbox_transaction_groups(1)
            .expect("load rejected group")
            .remove(0);
        let request_id = new_uuid_v7();
        store
            .prepare_direct_sync_request(&MobileDirectSyncRequestDraft {
                request_id: request_id.clone(),
                endpoint: "/sync/v1/push".to_string(),
                operation: "push".to_string(),
                purpose_json: purpose_json(serde_json::json!({
                    "operation": "push",
                    "transaction_id": group.transaction_id.clone(),
                    "transaction_digest": exact_sha256(b"rejected push fixture"),
                    "device_transaction_counter": 1
                })),
                push_transaction_id: Some(group.transaction_id.clone()),
                push_counter: Some(1),
                signed_request_bytes: br#"{"request":"rejected-push"}"#.to_vec(),
            })
            .expect("prepare rejected push fixture");
        store
            .record_direct_sync_response(
                &request_id,
                "/sync/v1/push",
                200,
                "application/json",
                br#"{"receipt":"rejected"}"#,
            )
            .expect("store rejected receipt bytes");
        store
            .complete_direct_sync_push_request(
                &request_id,
                MobileDirectSyncPushDisposition::Rejected,
                Some("authority_rejected"),
            )
            .expect("commit rejected push disposition");
        assert_eq!(
            store
                .direct_sync_push_binding(&group.transaction_id)
                .expect("load rejected binding")
                .expect("rejected binding")
                .state,
            "rejected"
        );
        let rejected_outbox: (String, bool) = store
            .lock_connection()
            .expect("lock rejected outbox")
            .query_row(
                "SELECT state, eligible_for_sync FROM mobile_note_outbox
                 WHERE transaction_id = ?1 LIMIT 1",
                [&group.transaction_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read rejected outbox state");
        assert_eq!(rejected_outbox, ("conflict".to_string(), false));
        assert_eq!(
            store.list(None).expect("read retained content")[0].body,
            "content remains local"
        );
        assert_eq!(
            store
                .prune_completed_direct_sync_requests(0)
                .expect("compact rejected push evidence")
                .pruned_completed_count,
            1
        );
    }

    #[test]
    fn bootstrap_pages_stream_without_known_count_and_publish_high_water_atomically() {
        let path = temporary_path("bootstrap-page-staging");
        let store = MobileStore::open(&path).expect("open bootstrap store");
        let activation = activate_fixture_store(&store);
        let checkpoint_id = new_uuid_v7();
        let checkpoint_sha256 = exact_sha256(b"fixture bootstrap checkpoint");
        let next_record = new_uuid_v7();
        let first_bytes = br#"{"page":0}"#.to_vec();
        let first = MobileBootstrapPageDraft {
            checkpoint_id: checkpoint_id.clone(),
            contract_version: crate::sync_protocol::BOOTSTRAP_SNAPSHOT_VERSION.to_string(),
            checkpoint_sha256: checkpoint_sha256.clone(),
            library_id: activation.library_id.clone(),
            authority_generation: activation.authority_generation,
            purge_generation: activation.purge_generation,
            key_epoch: activation.key_epoch,
            page_index: 0,
            high_water_cursor: 7,
            requested_after_record_id: None,
            next_after_record_id: Some(next_record.clone()),
            has_more: true,
            dependency_sha256: None,
            response_bytes: first_bytes.clone(),
        };
        let staged = store
            .stage_bootstrap_page(&first)
            .expect("stage first unknown-count page");
        assert_eq!(staged.recovery.checkpoint.final_page_count, None);
        assert_eq!(staged.recovery.pages.len(), 1);
        drop(store);

        let reopened = MobileStore::open(&path).expect("recover staged first page");
        assert_eq!(
            reopened
                .recover_bootstrap_staging()
                .expect("recover bootstrap")
                .expect("open bootstrap")
                .pages[0]
                .response_bytes,
            first_bytes
        );
        let final_page = MobileBootstrapPageDraft {
            page_index: 1,
            requested_after_record_id: Some(next_record),
            next_after_record_id: None,
            has_more: false,
            dependency_sha256: Some(exact_sha256(&first.response_bytes)),
            response_bytes: br#"{"page":1}"#.to_vec(),
            ..first.clone()
        };
        let finalized = reopened
            .stage_bootstrap_page(&final_page)
            .expect("stage final page");
        assert_eq!(finalized.recovery.checkpoint.final_page_count, Some(2));
        assert_eq!(finalized.recovery.checkpoint.state, "received");
        assert!(
            reopened
                .stage_bootstrap_page(&final_page)
                .expect("exact final-page replay")
                .replayed
        );

        let snapshot = MobileBootstrapSnapshot {
            checkpoint_sha256,
            head_batches: Vec::new(),
        };
        let applied = reopened
            .apply_bootstrap_snapshot(&checkpoint_id, &snapshot)
            .expect("publish bootstrap snapshot");
        assert_eq!(applied.final_cursor, 7);
        let cursors: (i64, i64) = reopened
            .lock_connection()
            .expect("lock applied bootstrap")
            .query_row(
                "SELECT downloaded_cursor, applied_cursor FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read atomic bootstrap cursors");
        assert_eq!(cursors, (7, 7));
        assert!(
            reopened
                .apply_bootstrap_snapshot(&checkpoint_id, &snapshot)
                .expect("replay applied snapshot")
                .replayed
        );
        drop(reopened);
        MobileStore::open(&path).expect("applied bootstrap survives restart");
        remove_database(&path);
    }

    #[test]
    fn canonical_bootstrap_atomically_publishes_exact_note_category_and_folder_heads() {
        let path = temporary_path("canonical-bootstrap-heads");
        let store = MobileStore::open(&path).expect("open canonical bootstrap store");
        let activation = activate_fixture_store(&store);
        let checkpoint_id = new_uuid_v7();
        let checkpoint_sha256 = exact_sha256(b"canonical bootstrap checkpoint");
        store
            .stage_bootstrap_page(&MobileBootstrapPageDraft {
                checkpoint_id: checkpoint_id.clone(),
                contract_version: crate::sync_protocol::BOOTSTRAP_SNAPSHOT_VERSION.to_string(),
                checkpoint_sha256: checkpoint_sha256.clone(),
                library_id: activation.library_id.clone(),
                authority_generation: activation.authority_generation,
                purge_generation: activation.purge_generation,
                key_epoch: activation.key_epoch,
                page_index: 0,
                high_water_cursor: 0,
                requested_after_record_id: None,
                next_after_record_id: None,
                has_more: false,
                dependency_sha256: None,
                response_bytes: br#"{"canonical":"page"}"#.to_vec(),
            })
            .expect("stage canonical bootstrap page");

        let category_id = new_uuid_v7();
        let folder_id = new_uuid_v7();
        let note_id = new_uuid_v7();
        let make_record =
            |record_id: String, kind: &str, content: serde_json::Value| -> ContextRecordV1 {
                let mut record = ContextRecordV1::new(
                    activation.library_id.clone(),
                    record_id,
                    kind.to_string(),
                    1,
                    1,
                    new_uuid_v7(),
                    "2026-08-17T10:00:00.000Z".to_string(),
                    "2026-08-17T10:00:00.000Z".to_string(),
                    None,
                    RecordScope {
                        scope_id: activation.default_scope_id.clone(),
                        class: ScopeClass::Unknown,
                    },
                    "standard".to_string(),
                    RecordAuthority {
                        kind: AuthorityKind::Noted,
                        origin: Some("noted".to_string()),
                    },
                    content,
                    serde_json::json!({"source": "canonical_bootstrap_fixture"}),
                    RecordLifecycle {
                        state: LifecycleState::Active,
                        trashed_at: None,
                        tombstoned_at: None,
                    },
                )
                .expect("construct bootstrap record");
                record.extensions.insert(
                    "example.test/bootstrap".to_string(),
                    serde_json::json!({"kind": kind}),
                );
                record
            };
        let category = make_record(
            category_id.clone(),
            "category",
            serde_json::json!({
                "name": "Project",
                "schema": {"futureSchema": {"type": "opaque"}},
                "unknownCategoryField": [1, 2, 3],
            }),
        );
        let folder = make_record(
            folder_id.clone(),
            "folder",
            serde_json::json!({
                "name": "Mobile",
                "parentId": null,
                "position": 3,
                "futureFolderField": {"kept": true},
            }),
        );
        let note = make_record(
            note_id.clone(),
            "note",
            serde_json::json!({
                "title": "Bootstrapped",
                "body": "Exact record",
                "folderId": folder_id,
                "futureNoteField": {"kept": true},
            }),
        );
        let exact = [&category, &folder, &note]
            .into_iter()
            .map(|record| canonical_context_record_bytes(record).expect("canonical head"))
            .collect::<Vec<_>>();
        let applied = store
            .apply_canonical_bootstrap_snapshot(
                &checkpoint_id,
                &MobileCanonicalBootstrapSnapshot {
                    checkpoint_sha256,
                    record_bytes: exact.clone(),
                },
            )
            .expect("apply exact canonical bootstrap");
        assert_eq!(applied.applied_record_count, 3);
        assert_eq!(applied.final_cursor, 0);
        assert!(store
            .canonical_initial_bootstrap_applied()
            .expect("durable bootstrap marker"));
        for (record_id, bytes) in [
            (category_id, exact[0].clone()),
            (folder.record_id.clone(), exact[1].clone()),
            (note_id, exact[2].clone()),
        ] {
            assert_eq!(
                store
                    .canonical_record(&record_id)
                    .expect("read bootstrap head")
                    .expect("bootstrap head exists")
                    .accepted_record_bytes,
                Some(bytes)
            );
        }
        drop(store);
        let reopened = MobileStore::open(&path).expect("reopen canonical bootstrap store");
        assert!(reopened
            .canonical_initial_bootstrap_applied()
            .expect("bootstrap marker survives restart"));
        assert_eq!(
            reopened.canonical_sync_cursors().expect("zero cursors"),
            (0, 0)
        );
        remove_database(&path);
    }

    #[test]
    fn bootstrap_page_replay_mismatch_quarantines_without_cursor_advance() {
        let store = store();
        let activation = activate_fixture_store(&store);
        let checkpoint_id = new_uuid_v7();
        let checkpoint_sha256 = exact_sha256(b"mismatch checkpoint");
        let page = MobileBootstrapPageDraft {
            checkpoint_id: checkpoint_id.clone(),
            contract_version: crate::sync_protocol::BOOTSTRAP_SNAPSHOT_VERSION.to_string(),
            checkpoint_sha256,
            library_id: activation.library_id,
            authority_generation: activation.authority_generation,
            purge_generation: activation.purge_generation,
            key_epoch: activation.key_epoch,
            page_index: 0,
            high_water_cursor: 9,
            requested_after_record_id: None,
            next_after_record_id: None,
            has_more: false,
            dependency_sha256: None,
            response_bytes: br#"{"page":"first"}"#.to_vec(),
        };
        store
            .stage_bootstrap_page(&page)
            .expect("stage exact bootstrap page");
        let mut changed = page.clone();
        changed.response_bytes = br#"{"page":"changed"}"#.to_vec();
        assert!(store
            .stage_bootstrap_page(&changed)
            .expect_err("changed page replay must quarantine")
            .contains("quarantined"));
        assert!(store
            .recover_bootstrap_staging()
            .expect("no open recovery after quarantine")
            .is_none());
        let state: (String, String, i64, i64) = store
            .lock_connection()
            .expect("lock quarantined bootstrap")
            .query_row(
                "SELECT checkpoint.state, page.state,
                        sync.downloaded_cursor, sync.applied_cursor
                 FROM mobile_bootstrap_checkpoint_v1 AS checkpoint
                 JOIN mobile_bootstrap_page_v1 AS page USING (checkpoint_id)
                 CROSS JOIN mobile_sync_state AS sync
                 WHERE checkpoint.checkpoint_id = ?1 AND sync.singleton = 1",
                [&checkpoint_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read quarantined bootstrap state");
        assert_eq!(
            state,
            ("quarantined".to_string(), "quarantined".to_string(), 0, 0)
        );

        let mut oversized = page;
        oversized.checkpoint_id = new_uuid_v7();
        oversized.response_bytes = vec![0; MAX_MOBILE_BOOTSTRAP_PAGE_BYTES + 1];
        assert!(store
            .stage_bootstrap_page(&oversized)
            .expect_err("oversized bootstrap page must fail before staging")
            .contains("invalid"));
    }

    #[test]
    fn direct_sync_counter_and_exact_bytes_tampering_fail_closed_on_reopen() {
        let path = temporary_path("direct-sync-counter-tamper");
        let store = MobileStore::open(&path).expect("open tamper store");
        store.create("Counter", "rollback").expect("create note");
        activate_fixture_store(&store);
        let group = store
            .eligible_outbox_transaction_groups(1)
            .expect("load tamper group")
            .remove(0);
        store
            .prepare_direct_sync_request(&MobileDirectSyncRequestDraft {
                request_id: new_uuid_v7(),
                endpoint: "/sync/v1/push".to_string(),
                operation: "push".to_string(),
                purpose_json: purpose_json(serde_json::json!({
                    "operation": "push",
                    "transaction_id": group.transaction_id.clone(),
                    "transaction_digest": exact_sha256(b"counter-bound push fixture"),
                    "device_transaction_counter": 1
                })),
                push_transaction_id: Some(group.transaction_id),
                push_counter: Some(1),
                signed_request_bytes: br#"{"request":"counter-bound"}"#.to_vec(),
            })
            .expect("prepare counter-bound push");
        store
            .lock_connection()
            .expect("lock tamper store")
            .execute(
                "UPDATE mobile_direct_sync_push_counter_v1 SET next_counter = 1 WHERE singleton = 1",
                [],
            )
            .expect("simulate restored counter rollback");
        drop(store);
        let error = MobileStore::open(&path)
            .err()
            .expect("counter rollback must fail closed");
        assert!(
            error.contains("rolled back") || error.contains("skipped"),
            "{error}"
        );
        remove_database(&path);
    }

    #[test]
    fn pairing_checkpoint_round_trips_exact_public_bytes_and_opaque_handles() {
        let path = temporary_path("pairing-checkpoint");
        let store = MobileStore::open(&path).expect("open fixture store");
        let checkpoint = fixture_pairing_checkpoint(&store);
        store
            .save_pairing_checkpoint(&checkpoint)
            .expect("save pairing checkpoint");
        assert_eq!(
            store
                .load_pairing_checkpoint()
                .expect("load pairing checkpoint"),
            Some(checkpoint.clone())
        );
        drop(store);

        let reopened = MobileStore::open(&path).expect("reopen fixture store");
        assert_eq!(
            reopened
                .load_pairing_checkpoint()
                .expect("load durable pairing checkpoint"),
            Some(checkpoint)
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn production_pairing_checkpoint_is_rejected_before_persistence_and_after_reopen() {
        let path = temporary_path("production-pairing-checkpoint");
        let store = MobileStore::open(&path).expect("open fixture store");
        let mut checkpoint = fixture_pairing_checkpoint(&store);
        checkpoint.client.config.environment = Environment::Production;
        let error = store
            .save_pairing_checkpoint(&checkpoint)
            .expect_err("production checkpoint must not cross the fixture persistence boundary");
        assert!(error.contains("sanitized fixture"), "{error}");
        assert_eq!(
            store
                .load_pairing_checkpoint()
                .expect("load rejected checkpoint store"),
            None
        );
        drop(store);

        let reopened = MobileStore::open(&path).expect("reopen fixture store");
        assert_eq!(
            reopened
                .load_pairing_checkpoint()
                .expect("load reopened checkpoint store"),
            None
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn pairing_checkpoint_mirror_tampering_fails_closed() {
        let path = temporary_path("pairing-checkpoint-tamper");
        let store = MobileStore::open(&path).expect("open fixture store");
        let checkpoint = fixture_pairing_checkpoint(&store);
        store
            .save_pairing_checkpoint(&checkpoint)
            .expect("save pairing checkpoint");
        store
            .lock_connection()
            .expect("lock fixture store")
            .execute(
                "UPDATE mobile_pairing_checkpoint_v1 SET state = 'active' WHERE singleton = 1",
                [],
            )
            .expect("tamper checkpoint mirror");
        assert!(store
            .load_pairing_checkpoint()
            .expect_err("mirror mismatch must fail closed")
            .contains("mirror mismatch"));
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn pairing_activation_is_one_atomic_commit_and_exact_replay_survives_reopen() {
        let path = temporary_path("atomic-pairing-activation");
        let store = MobileStore::open(&path).expect("open fixture store");
        let note = store
            .create("Offline", "created before pairing")
            .expect("create staged note");
        let activation = fixture_pairing_activation(&store);
        save_pending_predecessor(&store, &activation);
        assert_eq!(
            store
                .pairing_activation_health(false)
                .expect("pending-native health")
                .phase,
            "pending_native_activation"
        );
        assert_eq!(
            store
                .pairing_activation_health(true)
                .expect("native-active recovery health")
                .phase,
            "native_active_pending_finalize"
        );

        let result = store
            .finalize_pairing_activation(&activation)
            .expect("finalize activation");
        assert_eq!(result.adopted_note_count, 1);
        assert!(!result.replayed);
        assert_eq!(
            store
                .load_pairing_checkpoint()
                .expect("load Active checkpoint"),
            Some(activation.checkpoint.clone())
        );
        let connection = store.lock_connection().expect("lock finalized store");
        let state: (String, String, String, String, String) = connection
            .query_row(
                "SELECT replica.library_state, sync.enrollment_state,
                        notes.library_id, notes.scope_id, notes.scope_class
                 FROM mobile_replica AS replica
                 JOIN mobile_sync_state AS sync ON sync.singleton = replica.singleton
                 JOIN mobile_notes AS notes ON notes.record_id = ?1",
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
            .expect("read atomic activation state");
        assert_eq!(state.0, "paired");
        assert_eq!(state.1, "active");
        assert_eq!(state.2, activation.library_id);
        assert_eq!(state.3, activation.default_scope_id);
        assert_eq!(state.4, "unknown");
        drop(connection);
        drop(store);

        let reopened = MobileStore::open(&path).expect("reopen finalized activation");
        assert_eq!(
            reopened
                .pairing_activation_health(true)
                .expect("read finalized health")
                .phase,
            "finalized"
        );
        assert_eq!(
            reopened
                .finalize_pairing_activation(&activation)
                .expect("exact activation replay"),
            MobilePairingActivationResult {
                adopted_note_count: 1,
                replayed: true,
            }
        );
        let different = fixture_pairing_activation(&reopened);
        assert!(reopened
            .finalize_pairing_activation(&different)
            .expect_err("different valid replay must fail")
            .contains("different"));
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn pairing_activation_failure_rolls_back_every_domain_boundary() {
        let path = temporary_path("pairing-activation-rollback");
        let store = MobileStore::open(&path).expect("open fixture store");
        store
            .create("Staged", "must remain staged")
            .expect("create note");
        let activation = fixture_pairing_activation(&store);
        save_pending_predecessor(&store, &activation);
        store
            .lock_connection()
            .expect("lock fixture store")
            .execute_batch(
                "CREATE TRIGGER fail_pairing_activation
                 BEFORE INSERT ON mobile_pairing_activation_v1
                 BEGIN
                   SELECT RAISE(ABORT, 'injected activation commit failure');
                 END;",
            )
            .expect("install failure trigger");
        let error = store
            .finalize_pairing_activation(&activation)
            .expect_err("injected activation failure must roll back");
        assert!(
            error.contains("injected activation commit failure"),
            "{error}"
        );
        drop(store);

        let reopened = MobileStore::open(&path).expect("reopen rolled-back activation");
        let health = reopened
            .pairing_activation_health(true)
            .expect("read recovery health");
        assert_eq!(health.phase, "native_active_pending_finalize");
        assert_eq!(health.library_state, "local_staging");
        assert_eq!(health.enrollment_state, "not_enrolled");
        assert_eq!(
            reopened
                .finalized_pairing_activation()
                .expect("load activation"),
            None
        );
        assert_eq!(
            reopened
                .load_pairing_checkpoint()
                .expect("load pending checkpoint")
                .expect("pending checkpoint")
                .client
                .state,
            PairingClientState::PendingActivation
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn pairing_activation_rejects_transport_attempt_and_bad_fixture_bindings_without_changes() {
        let store = store();
        store
            .create("Attempted", "must not move")
            .expect("create note");
        let activation = fixture_pairing_activation(&store);
        save_pending_predecessor(&store, &activation);
        store
            .lock_connection()
            .expect("lock store")
            .execute(
                "UPDATE mobile_note_outbox SET attempts = 1 WHERE eligible_for_sync = 1",
                [],
            )
            .expect("simulate transport attempt");
        assert!(store
            .finalize_pairing_activation(&activation)
            .expect_err("transport attempt must fail")
            .contains("forbidden"));
        let health = store
            .pairing_activation_health(false)
            .expect("unchanged health");
        assert_eq!(health.library_state, "local_staging");
        assert_eq!(health.enrollment_state, "not_enrolled");

        let mut production = activation.clone();
        production.checkpoint.client.config.environment = Environment::Production;
        assert!(store
            .finalize_pairing_activation(&production)
            .expect_err("production activation must fail")
            .contains("sanitized fixture"));
        let mut personal = activation.clone();
        personal.checkpoint.client.config.library_data_class = LibraryDataClass::Personal;
        assert!(store
            .finalize_pairing_activation(&personal)
            .expect_err("personal activation must fail")
            .contains("sanitized fixture"));
        let mut wrong_scopes = activation;
        wrong_scopes.granted_scopes.remove(&RecordKind::Folder);
        assert!(store
            .finalize_pairing_activation(&wrong_scopes)
            .expect_err("partial scopes must fail")
            .contains("exact fixture"));
        let mut bad_identifier = wrong_scopes;
        bad_identifier.granted_scopes = fixture_record_scopes();
        bad_identifier.library_id = "not-a-uuid".to_string();
        assert!(store
            .finalize_pairing_activation(&bad_identifier)
            .expect_err("invalid library id must fail")
            .contains("invalid public binding"));
    }

    #[test]
    fn pairing_activation_rejects_non_staging_replica_without_partial_enrollment() {
        let store = store();
        let activation = fixture_pairing_activation(&store);
        save_pending_predecessor(&store, &activation);
        store
            .lock_connection()
            .expect("lock fixture store")
            .execute(
                "UPDATE mobile_replica
                 SET library_state = 'paired', library_id = ?1, default_scope_id = ?2
                 WHERE singleton = 1",
                params![activation.library_id, activation.default_scope_id],
            )
            .expect("simulate prior non-atomic adoption");
        let error = store
            .finalize_pairing_activation(&activation)
            .expect_err("non-staging activation must fail");
        assert!(error.contains("local_staging"), "{error}");
        let connection = store.lock_connection().expect("lock rejected store");
        let state: (String, i64) = connection
            .query_row(
                "SELECT enrollment_state,
                        (SELECT COUNT(*) FROM mobile_pairing_activation_v1)
                 FROM mobile_sync_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read rejected activation state");
        assert_eq!(state, ("not_enrolled".to_string(), 0));
    }

    #[test]
    fn active_checkpoint_cannot_be_saved_outside_atomic_finalizer() {
        let store = store();
        let activation = fixture_pairing_activation(&store);
        let error = store
            .save_pairing_checkpoint(&activation.checkpoint)
            .expect_err("standalone Active checkpoint must fail");
        assert!(error.contains("finalize_pairing_activation"), "{error}");
        assert_eq!(
            store.load_pairing_checkpoint().expect("load checkpoint"),
            None
        );
    }

    #[test]
    fn pairing_activation_tampering_fails_closed_on_read_and_reopen() {
        let path = temporary_path("pairing-activation-tamper");
        let store = MobileStore::open(&path).expect("open fixture store");
        let activation = fixture_pairing_activation(&store);
        save_pending_predecessor(&store, &activation);
        store
            .finalize_pairing_activation(&activation)
            .expect("finalize fixture activation");
        store
            .lock_connection()
            .expect("lock activation store")
            .execute(
                "UPDATE mobile_pairing_activation_v1 SET key_epoch = key_epoch + 1",
                [],
            )
            .expect("tamper activation mirror");
        assert!(store
            .finalized_pairing_activation()
            .expect_err("tampered activation must fail closed")
            .contains("mirror mismatch"));
        drop(store);
        let error = MobileStore::open(&path)
            .err()
            .expect("tampered activation must fail reopen");
        assert!(error.contains("mirror mismatch"), "{error}");
        remove_database(&path);
    }

    #[test]
    fn protected_data_lifecycle_closes_fails_closed_and_reopens_the_replica() {
        let path = temporary_path("protected-data");
        let store = MobileStore::open(&path).expect("open protected-data store");
        let note = store
            .create("Protected", "survives a locked-device transition")
            .expect("create protected note");

        store
            .protected_data_became_unavailable()
            .expect("close protected store");
        assert!(!store
            .protected_data_is_available()
            .expect("read protected-data state"));
        assert_eq!(store.list(None), Err(MOBILE_STORE_LOCKED_ERROR.to_string()));
        assert_eq!(
            store.create("Blocked", "must not write while locked"),
            Err(MOBILE_STORE_LOCKED_ERROR.to_string())
        );
        store
            .protected_data_became_unavailable()
            .expect("repeat close is idempotent");

        store
            .protected_data_became_available()
            .expect("reopen protected store");
        assert!(store
            .protected_data_is_available()
            .expect("read reopened state"));
        assert_eq!(store.list(None).expect("list reopened store")[0], note);
        let temp_store: i64 = store
            .lock_connection()
            .expect("lock reopened store")
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .expect("read temp-store policy");
        assert_eq!(temp_store, 2);
        store
            .protected_data_became_available()
            .expect("repeat reopen is idempotent");

        drop(store);
        remove_database(&path);
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
        let export = source.export_notes().expect("export portable notes");
        let decoded: serde_json::Value =
            serde_json::from_str(&export).expect("parse exported JSON");
        let source_envelope: MobileNotesExportEnvelope =
            serde_json::from_str(&export).expect("decode source export");
        assert_eq!(decoded["format"], MOBILE_NOTES_EXPORT_FORMAT);
        assert_eq!(decoded["formatVersion"], MOBILE_NOTES_EXPORT_VERSION);
        assert_eq!(
            source_envelope.payload.replica.library_state,
            "local_staging"
        );
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
                (2, PORTABLE_SCHEMA_V2_CHECKSUM.to_string()),
                (3, PORTABLE_SCHEMA_V3_CHECKSUM.to_string()),
                (4, PORTABLE_SCHEMA_V4_CHECKSUM.to_string()),
                (5, PORTABLE_SCHEMA_V5_CHECKSUM.to_string()),
                (6, PORTABLE_SCHEMA_V6_CHECKSUM.to_string()),
                (7, PORTABLE_SCHEMA_V7_CHECKSUM.to_string()),
                (8, PORTABLE_SCHEMA_V8_CHECKSUM.to_string())
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
    fn schema_v3_upgrades_through_v6_once_with_an_exact_recovery_snapshot() {
        let path = temporary_path("v3-to-v6-direct-sync");
        {
            let mut connection = Connection::open(&path).expect("open v3 fixture database");
            migrate_portable_notes_to_version(&mut connection, None, 3)
                .expect("construct the exact v3 schema");
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("read v3 version"),
                3
            );
            assert!(!connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM sqlite_schema
                       WHERE type = 'table' AND name = 'mobile_pairing_checkpoint_v1'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect v3 pairing schema"));
        }

        let (recovery_path, v6_migrated_at) = {
            let store = MobileStore::open(&path).expect("upgrade v3 database to v6");
            let recovery_path = PathBuf::from(
                store
                    .migration_recovery_path()
                    .expect("read v4 recovery state")
                    .expect("v3 recovery snapshot path"),
            );
            let snapshot = Connection::open_with_flags(
                &recovery_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("open v3 recovery snapshot");
            assert_eq!(
                snapshot
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("read snapshot version"),
                3
            );
            assert!(!snapshot
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM sqlite_schema
                       WHERE type = 'table' AND name = 'mobile_pairing_checkpoint_v1'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect snapshot pairing schema"));
            drop(snapshot);

            let connection = store.connection.lock().expect("lock upgraded store");
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .expect("count v6 migration history"),
                8
            );
            assert!(connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM sqlite_schema
                       WHERE type = 'table' AND name = 'mobile_pairing_checkpoint_v1'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect v4 pairing schema"));
            assert!(connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM sqlite_schema
                       WHERE type = 'table' AND name = 'mobile_pairing_activation_v1'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("inspect v5 pairing activation schema"));
            let migrated_at = connection
                .query_row(
                    "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 6",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read v6 migration instant");
            drop(connection);
            (recovery_path, migrated_at)
        };

        let reopened = MobileStore::open(&path).expect("reopen v6 database idempotently");
        assert_eq!(
            reopened
                .migration_recovery_path()
                .expect("read stable recovery path")
                .as_deref(),
            recovery_path.to_str()
        );
        let connection = reopened.connection.lock().expect("lock reopened v6 store");
        assert_eq!(
            connection
                .query_row(
                    "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 6",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read stable v6 migration instant"),
            v6_migrated_at
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count stable migration history"),
            8
        );
        drop(connection);
        drop(reopened);
        remove_database(&path);
        let _ = std::fs::remove_file(recovery_path);
    }

    #[test]
    fn schema_v4_checksum_is_the_sha256_of_the_exact_migration_ddl() {
        use sha2::{Digest as _, Sha256};

        assert_eq!(
            format!("{:x}", Sha256::digest(PORTABLE_SCHEMA_V4_DDL.as_bytes())),
            PORTABLE_SCHEMA_V4_CHECKSUM
        );
    }

    #[test]
    fn schema_v5_checksum_is_the_sha256_of_the_exact_atomic_activation_ddl() {
        use sha2::{Digest as _, Sha256};

        assert_eq!(
            format!("{:x}", Sha256::digest(PORTABLE_SCHEMA_V5_DDL.as_bytes())),
            PORTABLE_SCHEMA_V5_CHECKSUM
        );
    }

    #[test]
    fn schema_v6_checksum_is_the_sha256_of_the_exact_direct_sync_ddl() {
        use sha2::{Digest as _, Sha256};

        assert_eq!(
            format!("{:x}", Sha256::digest(PORTABLE_SCHEMA_V6_DDL.as_bytes())),
            PORTABLE_SCHEMA_V6_CHECKSUM
        );
    }

    #[test]
    fn schema_v7_checksum_is_the_sha256_of_the_exact_canonical_record_ddl() {
        use sha2::{Digest as _, Sha256};

        assert_eq!(
            format!("{:x}", Sha256::digest(PORTABLE_SCHEMA_V7_DDL.as_bytes())),
            PORTABLE_SCHEMA_V7_CHECKSUM
        );
    }

    #[test]
    fn schema_v8_checksum_is_the_sha256_of_the_exact_revocation_ddl() {
        use sha2::{Digest as _, Sha256};

        assert_eq!(
            format!("{:x}", Sha256::digest(PORTABLE_SCHEMA_V8_DDL.as_bytes())),
            PORTABLE_SCHEMA_V8_CHECKSUM
        );
    }

    #[test]
    fn schema_v6_to_v7_backfill_has_exact_recovery_and_is_deterministic() {
        let path = temporary_path("v6-to-v7-canonical");
        let (note_id, folder_id) = {
            let mut connection = Connection::open(&path).expect("open v6 fixture");
            migrate_portable_notes_to_version(&mut connection, None, 6)
                .expect("construct exact v6 schema");
            let fixture = MobileStore {
                path: path.clone(),
                connection: ProtectedConnection::new(connection),
            };
            let note = fixture
                .create("Deterministic", "v6 projection")
                .expect("seed v6 local note");
            let folder_id = fixture
                .workspace(None, Some("all"), None)
                .expect("read v6 workspace")
                .folders
                .into_iter()
                .next()
                .expect("default v6 folder")
                .folder_id;
            fixture
                .file_note(&note.record_id, &folder_id)
                .expect("seed v6 filing projection");
            drop(fixture);
            (note.record_id, folder_id)
        };
        let (recovery_path, first_exact, first_migrated_at) = {
            let store = MobileStore::open(&path).expect("migrate v6 fixture to v7");
            let recovery_path = PathBuf::from(
                store
                    .migration_recovery_path()
                    .expect("read v7 recovery path")
                    .expect("v6 recovery snapshot exists"),
            );
            let snapshot = Connection::open_with_flags(
                &recovery_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("open v6 recovery snapshot");
            assert_eq!(
                snapshot
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("read recovery schema"),
                6
            );
            assert!(!snapshot
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM sqlite_schema
                       WHERE type = 'table' AND name = 'mobile_canonical_record_v1'
                     )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("recovery excludes v7 table"));
            let canonical = store
                .canonical_record(&note_id)
                .expect("read backfilled note")
                .expect("backfilled canonical note");
            assert_eq!(canonical.backfill_provenance, "v7_projection_backfill");
            let exact = canonical.working_record_bytes;
            let decoded =
                decode_exact_canonical_context_record(&exact).expect("validate exact backfill");
            assert_eq!(decoded.content["folderId"], folder_id);
            store
                .update(&note_id, "Edited after migration", "keep v6 filing")
                .expect("edit projection-backed canonical note");
            let edited = store
                .canonical_record(&note_id)
                .expect("read edited backfill")
                .expect("edited backfill exists");
            assert_eq!(edited.backfill_provenance, "v7_projection_backfill");
            assert_eq!(
                decode_exact_canonical_context_record(&edited.working_record_bytes)
                    .expect("decode edited backfill")
                    .content["folderId"],
                folder_id
            );
            store
                .undo_note_filing(&note_id)
                .expect("edit projection-backed filing");
            let unfiled = store
                .canonical_record(&note_id)
                .expect("read unfiled backfill")
                .expect("unfiled backfill exists");
            assert_eq!(unfiled.backfill_provenance, "v7_projection_backfill");
            assert!(
                decode_exact_canonical_context_record(&unfiled.working_record_bytes)
                    .expect("decode unfiled backfill")
                    .content["folderId"]
                    .is_null()
            );
            let migrated_at = store
                .lock_connection()
                .expect("lock v7 store")
                .query_row(
                    "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 7",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read v7 migration time");
            (recovery_path, exact, migrated_at)
        };

        let restored_path = temporary_path("v6-to-v7-canonical-restored");
        std::fs::copy(&recovery_path, &restored_path).expect("restore exact v6 snapshot");
        let restored = MobileStore::open(&restored_path).expect("rerun v7 migration");
        assert_eq!(
            restored
                .canonical_record(&note_id)
                .expect("read restored backfill")
                .expect("restored canonical note")
                .working_record_bytes,
            first_exact
        );
        drop(restored);

        let reopened = MobileStore::open(&path).expect("reopen v7 idempotently");
        assert_eq!(
            reopened
                .lock_connection()
                .expect("lock reopened v7")
                .query_row(
                    "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 7",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read stable v7 migration time"),
            first_migrated_at
        );
        drop(reopened);
        remove_database(&path);
        remove_database(&restored_path);
        let _ = std::fs::remove_file(recovery_path);
    }

    #[test]
    fn canonical_record_digest_tampering_fails_closed_on_reopen() {
        let path = temporary_path("v7-canonical-digest-tamper");
        let record_id = {
            let store = MobileStore::open(&path).expect("create v7 tamper store");
            let note = store.create("Tamper", "detected").expect("create note");
            store
                .lock_connection()
                .expect("lock canonical tamper store")
                .execute(
                    "UPDATE mobile_canonical_record_v1
                     SET working_record_sha256 = ?1 WHERE record_id = ?2",
                    params!["0".repeat(64), note.record_id],
                )
                .expect("tamper canonical digest");
            note.record_id
        };
        let error = MobileStore::open(&path)
            .err()
            .expect("canonical digest tampering must fail closed");
        assert!(
            error.contains("canonical record metadata or digest"),
            "{error}"
        );
        assert!(is_uuid_v7(&record_id));
        remove_database(&path);
    }

    #[test]
    fn schema_v4_upgrades_to_v6_once_with_exact_history_and_recovery() {
        let path = temporary_path("v4-to-v6-direct-sync");
        {
            let mut connection = Connection::open(&path).expect("open v4 fixture database");
            migrate_portable_notes_to_version(&mut connection, None, 4)
                .expect("construct exact v4 schema");
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .expect("read v4 version"),
                4
            );
        }
        let store = MobileStore::open(&path).expect("upgrade v4 database to v6");
        let recovery_path = PathBuf::from(
            store
                .migration_recovery_path()
                .expect("read v6 recovery state")
                .expect("v4 recovery snapshot path"),
        );
        let snapshot =
            Connection::open_with_flags(&recovery_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open v4 recovery snapshot");
        assert_eq!(
            snapshot
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("read recovery version"),
            4
        );
        assert!(!snapshot
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_schema
                   WHERE type = 'table' AND name = 'mobile_pairing_activation_v1'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("inspect recovery activation schema"));
        drop(snapshot);
        let v6_migrated_at = store
            .lock_connection()
            .expect("lock upgraded store")
            .query_row(
                "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 6",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("read v6 migration time");
        drop(store);

        let reopened = MobileStore::open(&path).expect("reopen v6 store idempotently");
        let connection = reopened.lock_connection().expect("lock reopened store");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count ordered history"),
            8
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT migrated_at FROM mobile_schema_migrations WHERE version = 6",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("read stable v6 migration time"),
            v6_migrated_at
        );
        drop(connection);
        drop(reopened);
        remove_database(&path);
        let _ = std::fs::remove_file(recovery_path);
    }

    #[test]
    fn active_v4_fixture_requires_explicit_reset_instead_of_synthetic_v5_activation() {
        let path = temporary_path("active-v4-reset-required");
        let fixture_store = store();
        let activation = fixture_pairing_activation(&fixture_store);
        {
            let mut connection = Connection::open(&path).expect("open v4 fixture database");
            migrate_portable_notes_to_version(&mut connection, None, 4)
                .expect("construct exact v4 schema");
            connection
                .execute(
                    "UPDATE mobile_replica SET device_id = ?1 WHERE singleton = 1",
                    [&activation.device_id],
                )
                .expect("bind v4 replica device");
            write_mobile_pairing_checkpoint(&connection, &activation.checkpoint)
                .expect("seed active v4 fixture checkpoint");
            verify_mobile_schema_v4(&connection).expect("verify seeded v4 fixture");
        }
        let error = MobileStore::open(&path)
            .err()
            .expect("active v4 fixture cannot synthesize v5 activation");
        assert!(error.contains("reset pairing"), "{error}");
        let connection = Connection::open(&path).expect("inspect rejected v4 store");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("read unchanged schema version"),
            4
        );
        assert!(!connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_schema
                   WHERE type = 'table' AND name = 'mobile_pairing_activation_v1'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("confirm no synthesized activation table"));
        drop(connection);
        remove_database(&path);
        let recovery_directory = path
            .parent()
            .expect("fixture database parent")
            .join("migration-recovery");
        let recovery_prefix = format!(
            "{}-pre-schema-v5-",
            path.file_stem()
                .and_then(|value| value.to_str())
                .expect("fixture database stem")
        );
        if let Ok(entries) = std::fs::read_dir(recovery_directory) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&recovery_prefix))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    #[test]
    fn legacy_v4_bootstrap_shape_fails_with_precise_discard_recovery_signal() {
        let path = temporary_path("legacy-v4-bootstrap-reset");
        let fixture_store = store();
        let activation = fixture_pairing_activation(&fixture_store);
        let mut pending = activation.checkpoint.clone();
        pending.client.state = PairingClientState::PendingActivation;
        pending.client.activation = None;
        pending.pending_bootstrap_handle = Some("018f47a0-7b80-4000-8000-000000000002".to_string());
        pending.client.bootstrap_bytes = Some(
            serde_json::to_vec(&serde_json::json!({
                "protocol": PAIRING_PROTOCOL,
                "receipt_id": activation.receipt_id.clone(),
                "sealed_bootstrap": {
                    "encapsulated_key": vec![1_u8; 32],
                    "ciphertext": vec![2_u8; 64]
                },
                "envelope_digest": vec![3_u8; 32]
            }))
            .expect("encode legacy bootstrap"),
        );
        {
            let mut connection = Connection::open(&path).expect("open v4 fixture database");
            migrate_portable_notes_to_version(&mut connection, None, 4)
                .expect("construct exact v4 schema");
            connection
                .execute(
                    "UPDATE mobile_replica SET device_id = ?1 WHERE singleton = 1",
                    [&activation.device_id],
                )
                .expect("bind v4 replica device");
            let server: ServerHello = serde_json::from_slice(
                pending
                    .client
                    .server_hello_bytes
                    .as_deref()
                    .expect("server bytes"),
            )
            .expect("decode server hello");
            let checkpoint_json =
                serde_json::to_string(&pending.client).expect("encode legacy checkpoint");
            connection
                .execute(
                    "INSERT INTO mobile_pairing_checkpoint_v1 (
                       singleton, fixture_class, device_id, identity_handle,
                       pending_bootstrap_handle, state, invitation_bytes,
                       client_hello_bytes, server_hello_bytes, bootstrap_bytes,
                       client_finish_bytes, server_finish_bytes, transcript_digest,
                       receipt_id, envelope_digest, user_decision, checkpoint_json,
                       updated_at
                     ) VALUES (
                       1, 'sanitized_fixture', ?1, ?2, ?3, 'pending_activation', ?4,
                       ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14
                     )",
                    params![
                        activation.device_id,
                        pending.identity_handle,
                        pending.pending_bootstrap_handle,
                        pending.client.invitation_bytes,
                        pending.client.client_hello_bytes,
                        pending.client.server_hello_bytes,
                        pending.client.bootstrap_bytes,
                        pending.client.client_finish_bytes,
                        pending.client.server_finish_bytes,
                        server.receipt.transcript_digest,
                        server.receipt.receipt_id,
                        vec![3_u8; 32],
                        checkpoint_json,
                        pending.updated_at,
                    ],
                )
                .expect("seed legacy v4 checkpoint");
        }
        let error = MobileStore::open(&path)
            .err()
            .expect("legacy bootstrap must require recovery");
        assert!(
            error.contains("discard the pending native bootstrap"),
            "{error}"
        );
        let connection = Connection::open(&path).expect("inspect unchanged v4 store");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("read unchanged v4 version"),
            4
        );
        drop(connection);
        remove_database(&path);
    }

    #[test]
    fn stamped_v6_pairing_schema_is_never_silently_recreated_after_drop() {
        let path = temporary_path("v6-pairing-schema-drop");
        {
            let store = MobileStore::open(&path).expect("create v6 store");
            store
                .connection
                .lock()
                .expect("lock v6 store")
                .execute("DROP TABLE mobile_pairing_activation_v1", [])
                .expect("drop pairing activation table fixture");
        }

        let error = MobileStore::open(&path)
            .err()
            .expect("dropped pairing table must fail closed");
        assert!(
            error.contains("pairing") || error.contains("schema v6"),
            "{error}"
        );
        let connection = Connection::open(&path).expect("inspect rejected v4 store");
        assert!(!connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_schema
                   WHERE type = 'table' AND name = 'mobile_pairing_activation_v1'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("confirm pairing table remains absent"));
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mobile_schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count untouched migration history"),
            8
        );
        drop(connection);
        remove_database(&path);
    }

    #[test]
    fn stamped_v6_direct_sync_migration_checksum_tampering_fails_closed() {
        let path = temporary_path("v6-direct-sync-checksum-tamper");
        {
            let store = MobileStore::open(&path).expect("create v6 store");
            store
                .connection
                .lock()
                .expect("lock v6 store")
                .execute(
                    "UPDATE mobile_schema_state
                     SET migration_checksum = '0000000000000000000000000000000000000000000000000000000000000000'
                     WHERE singleton = 1",
                    [],
                )
                .expect("tamper v4 schema checksum fixture");
        }

        let error = MobileStore::open(&path)
            .err()
            .expect("tampered v6 checksum must fail closed");
        assert!(error.contains("v8 compatibility stamp"), "{error}");
        remove_database(&path);
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
    fn deep_link_targets_are_bound_to_the_local_library_and_visible_lifecycle() {
        let store = store();
        let note = store.create("Linked", "local target").expect("create note");
        let library_id = {
            let connection = store.connection.lock().expect("lock store");
            replica_identity(&connection).expect("identity").library_id
        };

        store
            .verify_note_link(&library_id, &note.record_id)
            .expect("active local note should open");
        assert!(store
            .verify_note_link(&new_uuid_v7(), &note.record_id)
            .expect_err("foreign library must fail")
            .contains("different notebook"));
        assert!(store
            .verify_note_link(&library_id, &new_uuid_v7())
            .expect_err("unknown note must fail")
            .contains("not available"));

        store.delete(&note.record_id).expect("trash note");
        store
            .verify_note_link(&library_id, &note.record_id)
            .expect("trash is a visible lifecycle");
        store.tombstone(&note.record_id).expect("tombstone note");
        assert!(store
            .verify_note_link(&library_id, &note.record_id)
            .expect_err("tombstone must not open")
            .contains("not available"));
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
