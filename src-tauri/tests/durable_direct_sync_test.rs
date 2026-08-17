use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

use tauri_app_lib::direct_authority_store::DirectAuthorityStore;
use tauri_app_lib::direct_sync::{
    request_signing_bytes, AckRequest, AckResponse, AuthorityStoreError, BootstrapRequest,
    BootstrapResponse, CheckpointRequest, CheckpointResponse, DirectEndpoint, DirectRequest,
    DirectResponse, DirectSyncAuthority, DirectSyncConfig, DirectSyncCrypto, DirectSyncLimits,
    DirectSyncService, NegotiateRequest, NegotiateResponse, PullRequest, PullResponse, PushRequest,
    PushResponse, SecureTransportEvidence, SignedSyncRequest, SignedSyncResponse,
};
use tauri_app_lib::direct_sync_transport::DirectSyncRequestHandler;
use tauri_app_lib::durable_direct_sync::{
    DurableAuthorityError, FixtureAuthorityClock, PreparePushOutcome, SqliteDirectSyncAuthority,
};
use tauri_app_lib::pairing_protocol::{Environment, LibraryDataClass};
use tauri_app_lib::portable::{
    AuthorityKind, ContextRecordV1, LifecycleState, RecordAuthority, RecordLifecycle, RecordScope,
    ScopeClass,
};
use tauri_app_lib::sync_protocol::{
    MutationDraft, ProtocolCapabilities, ProtocolError, ReceiptDisposition, RecordKindCapability,
    SignedTransaction, SubmitOutcome, TerminalRejection, TransactionHeader,
    DEFAULT_MAX_TRANSACTION_BYTES, SYNC_PROTOCOL_VERSION,
};

const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000001";
const MAC_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000002";
const PHONE_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000003";
const SCOPE_ID: &str = "018f47a0-7b80-7000-8000-000000000004";
const INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000005";
const RECEIPT_ID: &str = "018f47a0-7b80-7000-8000-000000000006";
const AUTHORITY_GENERATION: u64 = 7;
const NOW: i64 = 1_776_000_000_000;

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "noted-durable-direct-sync-{}-{sequence}.sqlite3",
            std::process::id()
        )))
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
    }
}

struct TestClock(AtomicI64);

impl TestClock {
    fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl FixtureAuthorityClock for TestClock {
    fn now_ms(&self) -> Result<i64, ()> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn id(number: u64) -> String {
    format!("018f47a0-7b80-7000-8000-{number:012x}")
}

fn notes_capabilities() -> ProtocolCapabilities {
    let mut capabilities = ProtocolCapabilities::new(
        1,
        1,
        BTreeMap::from([
            ("note".to_owned(), RecordKindCapability::new(1, 1)),
            ("category".to_owned(), RecordKindCapability::new(1, 1)),
            ("folder".to_owned(), RecordKindCapability::new(1, 1)),
        ]),
    );
    capabilities.max_transaction_bytes = 512 * 1024;
    capabilities
}

fn open(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open fixture database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
        .expect("configure fixture database");
    connection
}

fn seed(database: &TestDatabase) {
    let mut connection = open(&database.0);
    connection
        .execute_batch(
            "CREATE TABLE libraries (
               library_id TEXT PRIMARY KEY,
               authority_generation INTEGER NOT NULL,
               purge_generation INTEGER NOT NULL,
               current_key_epoch INTEGER NOT NULL,
               owner_device_id TEXT,
               enrollment_state TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE library_scopes (
               scope_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL REFERENCES libraries(library_id),
               scope_class TEXT NOT NULL,
               created_at TEXT NOT NULL,
               UNIQUE(library_id, scope_class)
             );
             CREATE TABLE portable_devices (
               device_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL REFERENCES libraries(library_id),
               device_kind TEXT NOT NULL,
               display_name TEXT NOT NULL,
               role TEXT NOT NULL,
               enrollment_state TEXT NOT NULL,
               capabilities_json TEXT NOT NULL,
               public_signing_key BLOB,
               public_encryption_key BLOB,
               last_transaction_counter INTEGER NOT NULL DEFAULT 0,
               created_at TEXT NOT NULL,
               enrolled_at TEXT,
               revoked_at TEXT
             );
             CREATE UNIQUE INDEX portable_devices_one_local_authority
               ON portable_devices(library_id)
               WHERE role = 'authority' AND enrollment_state = 'active';
             CREATE TABLE portable_records (
               record_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL REFERENCES libraries(library_id),
               kind TEXT NOT NULL,
               record_schema_version INTEGER NOT NULL,
               source_table TEXT NOT NULL,
               source_row_id INTEGER NOT NULL,
               scope_id TEXT NOT NULL REFERENCES library_scopes(scope_id),
               sensitivity TEXT NOT NULL,
               authority_kind TEXT NOT NULL,
               authority_origin TEXT,
               write_policy TEXT NOT NULL,
               lifecycle_state TEXT NOT NULL,
               trashed_at TEXT,
               tombstoned_at TEXT,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               UNIQUE(source_table, source_row_id)
             );
             CREATE TABLE change_transactions (
               transaction_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL REFERENCES libraries(library_id),
               device_id TEXT NOT NULL REFERENCES portable_devices(device_id),
               device_transaction_counter INTEGER NOT NULL,
               member_count INTEGER NOT NULL,
               manifest_digest TEXT NOT NULL,
               commit_marker INTEGER NOT NULL,
               created_at TEXT NOT NULL,
               UNIQUE(device_id, device_transaction_counter)
             );
             CREATE TABLE record_versions (
               version_id TEXT PRIMARY KEY,
               record_id TEXT NOT NULL REFERENCES portable_records(record_id),
               revision INTEGER NOT NULL,
               content_hash TEXT NOT NULL,
               snapshot_json TEXT NOT NULL,
               source_device_id TEXT NOT NULL REFERENCES portable_devices(device_id),
               transaction_id TEXT REFERENCES change_transactions(transaction_id),
               created_at TEXT NOT NULL,
               accepted_at TEXT NOT NULL,
               UNIQUE(record_id, version_id),
               UNIQUE(record_id, revision)
             );
             CREATE TABLE record_heads (
               record_id TEXT PRIMARY KEY REFERENCES portable_records(record_id),
               accepted_revision INTEGER NOT NULL,
               accepted_version_id TEXT NOT NULL,
               content_hash TEXT NOT NULL,
               authority_generation INTEGER NOT NULL,
               accepted_at TEXT NOT NULL,
               FOREIGN KEY(record_id, accepted_version_id)
                 REFERENCES record_versions(record_id, version_id)
             );
             CREATE TABLE change_log (
               local_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               mutation_id TEXT NOT NULL UNIQUE,
               transaction_id TEXT NOT NULL REFERENCES change_transactions(transaction_id),
               transaction_member_index INTEGER NOT NULL,
               record_id TEXT NOT NULL REFERENCES portable_records(record_id),
               record_kind TEXT NOT NULL,
               base_revision INTEGER NOT NULL,
               base_version_id TEXT,
               proposed_revision INTEGER NOT NULL,
               version_id TEXT NOT NULL REFERENCES record_versions(version_id),
               mutation_digest TEXT NOT NULL,
               authority_generation INTEGER NOT NULL,
               state TEXT NOT NULL,
               created_at TEXT NOT NULL,
               UNIQUE(transaction_id, transaction_member_index)
             );",
        )
        .expect("create portable Notes schema");
    connection
        .execute(
            "INSERT INTO libraries VALUES (?1, ?2, 0, 1, ?3, 'local', ?4)",
            params![
                LIBRARY_ID,
                AUTHORITY_GENERATION,
                MAC_DEVICE_ID,
                "2026-08-17T00:00:00Z"
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_scopes VALUES (?1, ?2, 'unknown', ?3)",
            params![SCOPE_ID, LIBRARY_ID, "2026-08-17T00:00:00Z"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO portable_devices
             (device_id, library_id, device_kind, display_name, role,
              enrollment_state, capabilities_json, last_transaction_counter,
              created_at, enrolled_at)
             VALUES (?1, ?2, 'macos', 'Fixture Mac', 'authority', 'active',
                     '{}', 0, ?3, ?3),
                    (?4, ?2, 'ios', 'Fixture iPhone', 'replica', 'active',
                     ?5, 0, ?3, ?3)",
            params![
                MAC_DEVICE_ID,
                LIBRARY_ID,
                "2026-08-17T00:00:00Z",
                PHONE_DEVICE_ID,
                serde_json::to_string(&notes_capabilities()).unwrap(),
            ],
        )
        .unwrap();

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    DirectAuthorityStore::install_schema(&transaction).unwrap();
    DirectAuthorityStore::initialize_fixture_profile(
        &transaction,
        LIBRARY_ID,
        AUTHORITY_GENERATION,
        &serde_json::to_string(&notes_capabilities()).unwrap(),
        NOW,
    )
    .unwrap();
    transaction
        .execute(
            "INSERT INTO direct_pairing_invitations
             (invitation_id, library_id, authority_generation, invitation_digest,
              nonce_hash, mac_pairing_signing_public_key,
              mac_pairing_hpke_public_key, tls_spki_sha256, scope_ceiling_json,
              environment, created_at_ms, expires_at_ms, failed_attempts, state,
              state_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[\"note\",\"category\",\"folder\"]',
                     'development', ?9, ?10, 0, 'active', 2)",
            params![
                INVITATION_ID,
                LIBRARY_ID,
                AUTHORITY_GENERATION,
                vec![0x11_u8; 32],
                vec![0x12_u8; 32],
                {
                    let mut key = vec![0x13_u8; 65];
                    key[0] = 4;
                    key
                },
                vec![0x14_u8; 32],
                vec![0x15_u8; 32],
                NOW - 1_000,
                NOW + 299_000,
            ],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO direct_enrollment_receipts
             (receipt_id, invitation_id, library_id, device_id, display_name,
              app_version, build_version, authority_generation, receipt_json,
              granted_scopes_json, capabilities_json,
              client_signing_public_key, client_hpke_public_key,
              begin_response_bytes, verification_code, confirmation_digest,
              bootstrap_envelope_bytes, bootstrap_envelope_digest,
              bootstrap_response_bytes, failed_finish_attempts, state,
              server_finish_bytes, created_at_ms, expires_at_ms, activated_at_ms,
              state_revision)
             VALUES (?1, ?2, ?3, ?4, 'Fixture iPhone', '1', '1', ?5,
                     '{}', '[\"note\",\"category\",\"folder\"]', ?6, ?7, ?8, x'01', NULL, ?9,
                     x'02', ?10, x'03', 0, 'active', x'04', ?11, ?12, ?13, 2)",
            params![
                RECEIPT_ID,
                INVITATION_ID,
                LIBRARY_ID,
                PHONE_DEVICE_ID,
                AUTHORITY_GENERATION,
                serde_json::to_string(&notes_capabilities()).unwrap(),
                {
                    let mut key = vec![0x16_u8; 65];
                    key[0] = 4;
                    key
                },
                vec![0x17_u8; 32],
                vec![0x18_u8; 32],
                vec![0x19_u8; 32],
                NOW - 500,
                NOW + 299_000,
                NOW,
            ],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO direct_device_sync_state
             (device_id, library_id, authority_generation, last_seen_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![PHONE_DEVICE_ID, LIBRARY_ID, AUTHORITY_GENERATION, NOW],
        )
        .unwrap();
    transaction.commit().unwrap();
    DirectAuthorityStore::verify_schema(&connection).unwrap();
}

fn adapter(database: &TestDatabase, clock: Arc<TestClock>) -> SqliteDirectSyncAuthority {
    SqliteDirectSyncAuthority::open_sanitized_fixture(&database.0, LIBRARY_ID, clock).unwrap()
}

fn fixture_record(record_number: u64, version_number: u64, revision: u64) -> ContextRecordV1 {
    ContextRecordV1::new(
        LIBRARY_ID.to_owned(),
        id(record_number),
        "note".to_owned(),
        1,
        revision,
        id(version_number),
        "2026-08-17T00:00:00Z".to_owned(),
        format!("2026-08-17T00:00:{revision:02}Z"),
        None,
        RecordScope {
            scope_id: SCOPE_ID.to_owned(),
            class: ScopeClass::Unknown,
        },
        "standard".to_owned(),
        RecordAuthority {
            kind: AuthorityKind::Noted,
            origin: None,
        },
        serde_json::json!({ "title": format!("Fixture {revision}"), "body": "sanitized" }),
        serde_json::json!({ "source": "fixture" }),
        RecordLifecycle {
            state: LifecycleState::Active,
            trashed_at: None,
            tombstoned_at: None,
        },
    )
    .unwrap()
}

fn transaction(
    counter: u64,
    transaction_number: u64,
    mutation_number: u64,
    record: ContextRecordV1,
    base_revision: u64,
    base_version_id: Option<String>,
    purge_generation: u64,
) -> SignedTransaction {
    transaction_expiring(
        counter,
        transaction_number,
        mutation_number,
        record,
        base_revision,
        base_version_id,
        purge_generation,
        u64::try_from(NOW + 200_000).unwrap(),
    )
}

#[allow(clippy::too_many_arguments)]
fn transaction_expiring(
    counter: u64,
    transaction_number: u64,
    mutation_number: u64,
    record: ContextRecordV1,
    base_revision: u64,
    base_version_id: Option<String>,
    purge_generation: u64,
    expires_at: u64,
) -> SignedTransaction {
    let mut ciphertext = b"fixture-json:".to_vec();
    ciphertext.extend(serde_json::to_vec(&record).unwrap());
    SignedTransaction::prepare(
        TransactionHeader {
            protocol_version: 1,
            library_id: LIBRARY_ID.to_owned(),
            transaction_id: id(transaction_number),
            device_id: PHONE_DEVICE_ID.to_owned(),
            device_transaction_counter: counter,
            authority_generation: AUTHORITY_GENERATION,
            purge_generation,
            key_epoch: 1,
        },
        vec![MutationDraft {
            mutation_id: id(mutation_number),
            record_id: record.record_id.clone(),
            record_kind: record.kind.clone(),
            record_schema_version: record.record_schema_version,
            base_head_revision: base_revision,
            base_head_version_id: base_version_id,
            proposed_revision: record.revision,
            version_id: record.version_id.clone(),
            ciphertext,
        }],
        expires_at,
    )
    .unwrap()
    .attach_signatures(vec![vec![0x44; 64]])
    .unwrap()
}

fn request_digest(label: &[u8]) -> [u8; 32] {
    Sha256::digest(label).into()
}

fn prepare(
    authority: &SqliteDirectSyncAuthority,
    request_number: u64,
    digest_label: &[u8],
    transaction: SignedTransaction,
    now: u64,
) -> tauri_app_lib::durable_direct_sync::PreparedPush {
    match authority
        .prepare_push(
            &id(request_number),
            request_digest(digest_label),
            transaction,
            now,
        )
        .unwrap()
    {
        PreparePushOutcome::NeedsFinalization(prepared) => {
            assert_eq!(prepared.request_id(), id(request_number));
            prepared
        }
        PreparePushOutcome::ExactReplay(_) => panic!("expected a candidate"),
    }
}

#[test]
fn prepared_push_survives_restart_and_exact_replay_survives_later_cursor() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let first = transaction(1, 100, 101, fixture_record(200, 201, 1), 0, None, 0);

    let prepared = prepare(
        &adapter(&database, Arc::clone(&clock)),
        300,
        b"request-one",
        first.clone(),
        NOW as u64,
    );
    assert_eq!(prepared.receipt().high_water_cursor, 1);
    drop(prepared);

    let restarted = adapter(&database, Arc::clone(&clock));
    let prepared = prepare(
        &restarted,
        300,
        b"request-one",
        first.clone(),
        NOW as u64 + 1,
    );
    restarted
        .finalize_push(&prepared, 200, b"signed-response-one", NOW as u64 + 1)
        .unwrap();
    let snapshot = restarted.bootstrap().unwrap();
    assert_eq!(snapshot.high_water_cursor, 1);
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(restarted.pull(0, 10).unwrap().changes.len(), 1);

    let second = transaction(2, 102, 103, fixture_record(202, 203, 1), 0, None, 0);
    let second_prepared = prepare(&restarted, 301, b"request-two", second, NOW as u64 + 2);
    restarted
        .finalize_push(
            &second_prepared,
            200,
            b"signed-response-two",
            NOW as u64 + 2,
        )
        .unwrap();

    let reopened = adapter(&database, clock);
    let replay = reopened
        .prepare_push(
            &id(300),
            request_digest(b"request-one"),
            first,
            NOW as u64 + 3,
        )
        .unwrap();
    let PreparePushOutcome::ExactReplay(replay) = replay else {
        panic!("expected exact response replay");
    };
    assert_eq!(replay.exact_response_bytes, b"signed-response-one");
    assert_eq!(replay.receipt.high_water_cursor, 1);
    assert_eq!(reopened.pull(0, 10).unwrap().high_water_cursor, 2);
}

#[test]
fn conflict_and_rejection_are_terminal_durable_and_never_enter_pull_or_heads() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let authority = adapter(&database, Arc::clone(&clock));
    let first_record = fixture_record(400, 401, 1);
    let first = transaction(1, 402, 403, first_record.clone(), 0, None, 0);
    let first_prepared = prepare(&authority, 404, b"first", first, NOW as u64);
    authority
        .finalize_push(&first_prepared, 200, b"accepted", NOW as u64)
        .unwrap();

    let stale = transaction(2, 405, 406, fixture_record(400, 407, 1), 0, None, 0);
    let conflict = prepare(&authority, 408, b"conflict", stale.clone(), NOW as u64 + 1);
    assert!(matches!(
        conflict.receipt().disposition,
        ReceiptDisposition::Conflict { .. }
    ));
    authority
        .finalize_push(&conflict, 200, b"conflict-response", NOW as u64 + 1)
        .unwrap();

    let pending = transaction(3, 409, 410, fixture_record(411, 412, 1), 0, None, 0);
    let _accepted_candidate = prepare(
        &authority,
        413,
        b"generation-change",
        pending.clone(),
        NOW as u64 + 2,
    );
    open(&database.0)
        .execute(
            "UPDATE libraries SET purge_generation = 1 WHERE library_id = ?1",
            [LIBRARY_ID],
        )
        .unwrap();
    let rejected = prepare(
        &authority,
        413,
        b"generation-change",
        pending.clone(),
        NOW as u64 + 3,
    );
    assert!(matches!(
        rejected.receipt().disposition,
        ReceiptDisposition::Rejected {
            code: TerminalRejection::PurgeGenerationChanged
        }
    ));
    authority
        .finalize_push(&rejected, 200, b"rejected-response", NOW as u64 + 3)
        .unwrap();

    let reopened = adapter(&database, clock);
    assert_eq!(reopened.pull(0, 10).unwrap().changes.len(), 1);
    assert_eq!(reopened.bootstrap().unwrap().records.len(), 1);
    let conflict_replay = reopened
        .prepare_push(&id(408), request_digest(b"conflict"), stale, NOW as u64 + 4)
        .unwrap();
    assert!(matches!(
        conflict_replay,
        PreparePushOutcome::ExactReplay(_)
    ));
    let rejected_replay = reopened
        .prepare_push(
            &id(413),
            request_digest(b"generation-change"),
            pending,
            NOW as u64 + 4,
        )
        .unwrap();
    assert!(matches!(
        rejected_replay,
        PreparePushOutcome::ExactReplay(_)
    ));
}

#[test]
fn historical_checkpoint_ack_survives_reopen_and_advances_monotonically() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let mut authority = adapter(&database, Arc::clone(&clock));
    let first = transaction(1, 500, 501, fixture_record(502, 503, 1), 0, None, 0);
    let outcome = authority.push(first, NOW as u64).unwrap();
    assert!(matches!(outcome, SubmitOutcome::Terminal(_)));
    let first_checkpoint = authority.checkpoint().unwrap();

    let second = transaction(2, 504, 505, fixture_record(506, 507, 1), 0, None, 0);
    authority.push(second, NOW as u64 + 1).unwrap();
    let second_checkpoint = authority.checkpoint().unwrap();
    assert_eq!(first_checkpoint.high_water_cursor, 1);
    assert_eq!(second_checkpoint.high_water_cursor, 2);

    clock.set(NOW + 10);
    let first_ack = authority
        .acknowledge(
            PHONE_DEVICE_ID,
            first_checkpoint.high_water_cursor,
            &first_checkpoint.checkpoint_digest,
        )
        .unwrap();
    drop(authority);

    let mut reopened = adapter(&database, Arc::clone(&clock));
    assert_eq!(
        reopened
            .acknowledge(
                PHONE_DEVICE_ID,
                first_checkpoint.high_water_cursor,
                &first_checkpoint.checkpoint_digest,
            )
            .unwrap(),
        first_ack
    );
    reopened
        .acknowledge(
            PHONE_DEVICE_ID,
            second_checkpoint.high_water_cursor,
            &second_checkpoint.checkpoint_digest,
        )
        .unwrap();
    let acknowledged: i64 = open(&database.0)
        .query_row(
            "SELECT acknowledged_cursor FROM direct_device_sync_state WHERE device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(acknowledged, 2);
}

#[test]
fn revocation_blocks_new_push_replay_and_ack_after_commit() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let mut authority = adapter(&database, Arc::clone(&clock));
    let first = transaction(1, 600, 601, fixture_record(602, 603, 1), 0, None, 0);
    let prepared = prepare(&authority, 604, b"before-revoke", first.clone(), NOW as u64);
    authority
        .finalize_push(&prepared, 200, b"before-revoke-response", NOW as u64)
        .unwrap();
    let checkpoint = authority.checkpoint().unwrap();
    clock.set(NOW + 1);
    authority.revoke_device(PHONE_DEVICE_ID).unwrap();

    let replay = authority.prepare_push(
        &id(604),
        request_digest(b"before-revoke"),
        first,
        NOW as u64 + 2,
    );
    assert_eq!(
        replay,
        Err(DurableAuthorityError::Protocol(
            ProtocolError::DeviceRevoked
        ))
    );
    assert!(matches!(
        authority.acknowledge(
            PHONE_DEVICE_ID,
            checkpoint.high_water_cursor,
            &checkpoint.checkpoint_digest,
        ),
        Err(AuthorityStoreError::Protocol(ProtocolError::DeviceRevoked))
    ));
    drop(authority);

    let connection = open(&database.0);
    let states: (String, String) = connection
        .query_row(
            "SELECT d.enrollment_state, r.state FROM portable_devices d
             JOIN direct_enrollment_receipts r ON r.device_id = d.device_id
             WHERE d.device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(states, ("revoked".to_owned(), "revoked".to_owned()));
    DirectAuthorityStore::verify_schema(&connection).unwrap();
}

#[test]
fn adapter_transaction_limit_matches_direct_service_contract() {
    assert!(notes_capabilities().max_transaction_bytes <= DEFAULT_MAX_TRANSACTION_BYTES);
    fn assert_authority<T: DirectSyncAuthority>() {}
    assert_authority::<SqliteDirectSyncAuthority>();
}

#[test]
fn retained_replay_evidence_never_permanently_exhausts_the_rate_window() {
    let database = TestDatabase::new();
    seed(&database);
    let connection = open(&database.0);
    for request_number in 0..128_u64 {
        connection
            .execute(
                "INSERT INTO direct_request_replays
                 (device_id, request_id, endpoint, request_digest, status_code,
                  exact_response_bytes, created_at_ms)
                 VALUES (?1, ?2, '/sync/v1/push', ?3, 200, ?4, ?5)",
                params![
                    PHONE_DEVICE_ID,
                    id(10_000 + request_number),
                    vec![request_number as u8; 32],
                    format!("retained-response-{request_number}").into_bytes(),
                    NOW,
                ],
            )
            .unwrap();
    }
    drop(connection);

    let later = NOW + 300_001;
    let authority = adapter(&database, Arc::new(TestClock::new(later)));
    let signed = transaction_expiring(
        1,
        800,
        801,
        fixture_record(802, 803, 1),
        0,
        None,
        0,
        u64::try_from(later + 60_000).unwrap(),
    );
    let prepared = prepare(&authority, 804, b"after-window", signed, later as u64);
    authority
        .finalize_push(&prepared, 200, b"after-window-response", later as u64)
        .unwrap();

    let retained: i64 = open(&database.0)
        .query_row("SELECT COUNT(*) FROM direct_request_replays", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, 129, "immutable replay evidence is retained");
}

#[derive(Clone)]
struct RouteCrypto {
    response_tag: u8,
    fail_response_endpoint: Option<DirectEndpoint>,
}

impl RouteCrypto {
    fn request_signature(
        endpoint: DirectEndpoint,
        device_id: &str,
        signing_bytes: &[u8],
    ) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"noted.durable-route-test/request");
        hasher.update(endpoint.path().as_bytes());
        hasher.update(device_id.as_bytes());
        hasher.update(signing_bytes);
        let digest = hasher.finalize();
        [digest.as_slice(), digest.as_slice()].concat()
    }
}

impl DirectSyncCrypto for RouteCrypto {
    fn verify_request_signature(
        &self,
        endpoint: DirectEndpoint,
        device_id: &str,
        signing_bytes: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        (signature == Self::request_signature(endpoint, device_id, signing_bytes))
            .then_some(())
            .ok_or(())
    }

    fn verify_mutation_ciphertext(
        &self,
        device_id: &str,
        mutation: &tauri_app_lib::sync_protocol::MutationEnvelope,
    ) -> Result<(), ()> {
        (mutation.device_id == device_id
            && mutation.ciphertext.starts_with(b"fixture-json:")
            && !mutation.signature.is_empty())
        .then_some(())
        .ok_or(())
    }

    fn authenticate_response(
        &self,
        endpoint: DirectEndpoint,
        signing_bytes: &[u8],
    ) -> Result<Vec<u8>, ()> {
        if self.fail_response_endpoint == Some(endpoint) {
            return Err(());
        }
        let mut hasher = Sha256::new();
        hasher.update(b"noted.durable-route-test/response");
        hasher.update([self.response_tag]);
        hasher.update(endpoint.path().as_bytes());
        hasher.update(signing_bytes);
        let digest = hasher.finalize();
        Ok([digest.as_slice(), digest.as_slice()].concat())
    }
}

type DurableRouteService =
    DirectSyncService<SqliteDirectSyncAuthority, SqliteDirectSyncAuthority, RouteCrypto>;

fn durable_route_service(
    database: &TestDatabase,
    clock: Arc<TestClock>,
    response_tag: u8,
    fail_response_endpoint: Option<DirectEndpoint>,
) -> DurableRouteService {
    DirectSyncService::new(
        adapter(database, Arc::clone(&clock)),
        adapter(database, clock),
        RouteCrypto {
            response_tag,
            fail_response_endpoint,
        },
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: AUTHORITY_GENERATION,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x15; 32],
            limits: DirectSyncLimits::default(),
        },
    )
    .unwrap()
}

fn notes_kinds() -> BTreeSet<String> {
    ["note", "category", "folder"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn signed_route_request<T: Serialize>(
    endpoint: DirectEndpoint,
    request_number: u64,
    payload: T,
    authority_now: u64,
) -> DirectRequest {
    let mut signed = SignedSyncRequest {
        protocol_version: SYNC_PROTOCOL_VERSION,
        request_id: id(request_number),
        library_id: LIBRARY_ID.to_owned(),
        device_id: PHONE_DEVICE_ID.to_owned(),
        authority_generation: AUTHORITY_GENERATION,
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        payload,
        signature: Vec::new(),
    };
    let signing_bytes = request_signing_bytes(endpoint, &signed).unwrap();
    signed.signature = RouteCrypto::request_signature(endpoint, PHONE_DEVICE_ID, &signing_bytes);
    DirectRequest {
        method: "POST".to_owned(),
        target: endpoint.path().to_owned(),
        content_type: Some("application/json".to_owned()),
        content_encoding: None,
        body: serde_json::to_vec(&signed).unwrap(),
        authority_now,
        transport: SecureTransportEvidence {
            tls_version: "1.3".to_owned(),
            used_zero_rtt: false,
            server_spki_sha256: vec![0x15; 32],
        },
    }
}

fn send(service: &impl DirectSyncRequestHandler, request: DirectRequest) -> DirectResponse {
    service.handle_direct_sync(request)
}

fn response_payload<T: DeserializeOwned>(response: &DirectResponse) -> T {
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    serde_json::from_slice::<SignedSyncResponse<T>>(&response.body)
        .unwrap()
        .payload
}

#[test]
fn six_route_handler_survives_restart_replays_exact_push_and_ack_then_revokes() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let service = durable_route_service(&database, Arc::clone(&clock), 0xa1, None);

    let negotiate_request = signed_route_request(
        DirectEndpoint::Negotiate,
        20_000,
        NegotiateRequest {
            capabilities: notes_capabilities(),
        },
        NOW as u64,
    );
    let negotiate = send(&service, negotiate_request.clone());
    let negotiated: NegotiateResponse = response_payload(&negotiate);
    assert_eq!(negotiated.negotiated.record_kinds.len(), 3);

    let bootstrap = send(
        &service,
        signed_route_request(
            DirectEndpoint::Bootstrap,
            20_001,
            BootstrapRequest {
                requested_record_kinds: notes_kinds(),
                checkpoint_digest: None,
                after_record_id: None,
                limit: 16,
            },
            NOW as u64,
        ),
    );
    let bootstrap: BootstrapResponse = response_payload(&bootstrap);
    assert!(bootstrap.page.records.is_empty());

    let pushed_transaction = transaction(
        1,
        20_010,
        20_011,
        fixture_record(20_012, 20_013, 1),
        0,
        None,
        0,
    );
    let push_request = signed_route_request(
        DirectEndpoint::Push,
        20_002,
        PushRequest {
            transaction: pushed_transaction,
        },
        NOW as u64,
    );
    let push = send(&service, push_request.clone());
    let pushed: PushResponse = response_payload(&push);
    assert_eq!(pushed.receipt.high_water_cursor, 1);

    let pull = send(
        &service,
        signed_route_request(
            DirectEndpoint::Pull,
            20_003,
            PullRequest {
                cursor: 0,
                limit: 16,
                requested_record_kinds: notes_kinds(),
            },
            NOW as u64 + 1,
        ),
    );
    let pull: PullResponse = response_payload(&pull);
    assert_eq!(pull.page.changes.len(), 1);

    let checkpoint = send(
        &service,
        signed_route_request(
            DirectEndpoint::Checkpoint,
            20_004,
            CheckpointRequest { known_cursor: None },
            NOW as u64 + 2,
        ),
    );
    let checkpoint: CheckpointResponse = response_payload(&checkpoint);
    assert_eq!(checkpoint.checkpoint.high_water_cursor, 1);

    let ack_request = signed_route_request(
        DirectEndpoint::Ack,
        20_005,
        AckRequest {
            high_water_cursor: checkpoint.checkpoint.high_water_cursor,
            checkpoint_digest: checkpoint.checkpoint.checkpoint_digest,
        },
        NOW as u64 + 3,
    );
    let ack = send(&service, ack_request.clone());
    let acknowledged: AckResponse = response_payload(&ack);
    assert_eq!(acknowledged.receipt.high_water_cursor, 1);
    drop(service);

    let restarted = durable_route_service(&database, Arc::clone(&clock), 0xb2, None);
    let push_replay = send(&restarted, push_request.clone());
    let ack_replay = send(&restarted, ack_request.clone());
    assert_eq!(push_replay.body, push.body);
    assert_eq!(ack_replay.body, ack.body);
    assert_eq!(send(&restarted, negotiate_request.clone()).status, 200);

    restarted.revoke_device(PHONE_DEVICE_ID, NOW + 10).unwrap();
    drop(restarted);
    let revoked = durable_route_service(&database, clock, 0xc3, None);
    assert_eq!(send(&revoked, negotiate_request).status, 403);
    assert_eq!(send(&revoked, push_request).status, 403);
    assert_eq!(send(&revoked, ack_request).status, 403);
}

#[test]
fn push_signing_failure_leaves_only_prepared_state_and_retry_finalizes_once() {
    let database = TestDatabase::new();
    seed(&database);
    let clock = Arc::new(TestClock::new(NOW));
    let request = signed_route_request(
        DirectEndpoint::Push,
        21_000,
        PushRequest {
            transaction: transaction(
                1,
                21_001,
                21_002,
                fixture_record(21_003, 21_004, 1),
                0,
                None,
                0,
            ),
        },
        NOW as u64,
    );
    let failing = durable_route_service(
        &database,
        Arc::clone(&clock),
        0xd4,
        Some(DirectEndpoint::Push),
    );
    let failed = send(&failing, request.clone());
    assert_eq!(failed.status, 503);
    drop(failing);

    let before: (String, i64, i64) = open(&database.0)
        .query_row(
            "SELECT t.state,
                    (SELECT COUNT(*) FROM direct_authority_changes),
                    (SELECT COUNT(*) FROM direct_request_replays)
             FROM direct_authority_transactions t WHERE t.transaction_id = ?1",
            [id(21_001)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(before, ("prepared".to_owned(), 0, 0));

    let recovered = durable_route_service(&database, Arc::clone(&clock), 0xe5, None);
    let accepted = send(&recovered, request.clone());
    let payload: PushResponse = response_payload(&accepted);
    assert_eq!(payload.receipt.high_water_cursor, 1);
    drop(recovered);

    let after: (String, i64, i64) = open(&database.0)
        .query_row(
            "SELECT t.state,
                    (SELECT COUNT(*) FROM direct_authority_changes),
                    (SELECT COUNT(*) FROM direct_request_replays)
             FROM direct_authority_transactions t WHERE t.transaction_id = ?1",
            [id(21_001)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(after, ("accepted".to_owned(), 1, 1));

    let replaying = durable_route_service(&database, clock, 0xf6, None);
    assert_eq!(send(&replaying, request).body, accepted.body);
}
