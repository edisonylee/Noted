use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
    thread,
    time::Duration,
};

use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use tauri_app_lib::direct_authority_store::{
    AckOutcome, AcknowledgeCheckpoint, ActivateEnrollment, ActivateOutcome, AttemptOutcome,
    CheckpointOutcome, ConfirmEnrollment, ConfirmOutcome, ConsumeInvitation, ConsumeOutcome,
    DirectAuthorityStore, FixtureAcceptedChange, FixtureChangeOutcome, InvitationRegistration,
    IssueCheckpoint, NewInvitation, RevokeOutcome, StoreError, StoreResult,
    DIRECT_AUTHORITY_SCHEMA_VERSION,
};

const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000001";
const MAC_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000002";
const INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000003";
const HELLO_ID: &str = "018f47a0-7b80-7000-8000-000000000004";
const RECEIPT_ID: &str = "018f47a0-7b80-7000-8000-000000000005";
const PHONE_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000006";
const FINISH_ID: &str = "018f47a0-7b80-7000-8000-000000000007";
const TRANSACTION_ONE_ID: &str = "018f47a0-7b80-7000-8000-000000000008";
const TRANSACTION_TWO_ID: &str = "018f47a0-7b80-7000-8000-000000000009";
const NOW_MS: i64 = 1_776_000_000_000;
const AUTHORITY_GENERATION: u64 = 7;
const PURGE_GENERATION: u64 = 2;
const KEY_EPOCH: u64 = 1;
const MAX_FIXTURE_ATTEMPTS: u8 = 5;

static NEXT_TEST_DATABASE: AtomicU64 = AtomicU64::new(1);

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_TEST_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noted-direct-authority-{}-{sequence}.sqlite3",
            std::process::id()
        ));
        Self { path }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.path.as_os_str().to_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
    }
}

fn open(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open fixture authority database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    connection
        .busy_timeout(Duration::from_secs(10))
        .expect("set busy timeout");
    connection
}

fn write<T>(
    path: &Path,
    operation: impl FnOnce(&Transaction<'_>) -> StoreResult<T>,
) -> StoreResult<T> {
    let mut connection = open(path);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StoreError::from)?;
    match operation(&transaction) {
        Ok(value) => {
            transaction.commit().map_err(StoreError::from)?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

fn seed_database(database: &TestDatabase) {
    let mut connection = open(&database.path);
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE libraries (
               library_id             TEXT PRIMARY KEY,
               authority_generation   INTEGER NOT NULL CHECK(authority_generation > 0),
               purge_generation       INTEGER NOT NULL CHECK(purge_generation >= 0),
               current_key_epoch      INTEGER NOT NULL CHECK(current_key_epoch >= 0),
               owner_device_id        TEXT,
               enrollment_state       TEXT NOT NULL CHECK(enrollment_state IN ('local', 'enrolled')),
               created_at             TEXT NOT NULL
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
               ON portable_devices(library_id)
               WHERE role = 'authority' AND enrollment_state = 'active';",
        )
        .expect("install portable authority anchors");
    connection
        .execute(
            "INSERT INTO libraries
             (library_id, authority_generation, purge_generation,
              current_key_epoch, owner_device_id, enrollment_state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'local', ?6)",
            params![
                LIBRARY_ID,
                AUTHORITY_GENERATION,
                PURGE_GENERATION,
                KEY_EPOCH,
                MAC_DEVICE_ID,
                "2026-08-17T00:00:00Z"
            ],
        )
        .expect("seed library");
    connection
        .execute(
            "INSERT INTO portable_devices
             (device_id, library_id, device_kind, display_name, role,
              enrollment_state, capabilities_json, last_transaction_counter,
              created_at, enrolled_at)
             VALUES (?1, ?2, 'macos', 'Fixture Mac', 'authority', 'active',
                     '{}', 0, ?3, ?3)",
            params![MAC_DEVICE_ID, LIBRARY_ID, "2026-08-17T00:00:00Z"],
        )
        .expect("seed authority device");

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("begin schema transaction");
    DirectAuthorityStore::install_schema(&transaction).expect("install direct-authority expansion");
    DirectAuthorityStore::initialize_fixture_profile(
        &transaction,
        LIBRARY_ID,
        AUTHORITY_GENERATION,
        r#"{"fixtureRecords":true}"#,
        NOW_MS,
    )
    .expect("initialize sanitized fixture profile");
    transaction.commit().expect("commit schema transaction");

    assert_eq!(DIRECT_AUTHORITY_SCHEMA_VERSION, 4);
    DirectAuthorityStore::verify_schema(&connection).expect("verify new fixture schema");
}

fn invitation() -> NewInvitation {
    NewInvitation {
        invitation_id: INVITATION_ID.into(),
        library_id: LIBRARY_ID.into(),
        authority_generation: AUTHORITY_GENERATION,
        invitation_digest: [0x11; 32],
        nonce_hash: [0x22; 32],
        mac_pairing_signing_public_key: [0x04; 65],
        mac_pairing_hpke_public_key: [0x33; 32],
        tls_spki_sha256: [0x44; 32],
        scope_ceiling_json: r#"["note","folder"]"#.into(),
        created_at_ms: NOW_MS,
        expires_at_ms: NOW_MS + 300_000,
    }
}

fn consume_request(
    message_id: &str,
    receipt_id: &str,
    device_id: &str,
    request_digest: [u8; 32],
    exact_response: &[u8],
) -> ConsumeInvitation {
    ConsumeInvitation {
        message_id: message_id.into(),
        invitation_id: INVITATION_ID.into(),
        request_digest,
        observed_tls_spki_sha256: [0x44; 32],
        receipt_id: receipt_id.into(),
        device_id: device_id.into(),
        display_name: "Fixture iPhone".into(),
        app_version: "1.0".into(),
        build_version: "100".into(),
        receipt_json: r#"{"receipt":"fixture"}"#.into(),
        granted_scopes_json: r#"["note","folder"]"#.into(),
        capabilities_json: r#"{"fixtureSync":true}"#.into(),
        client_signing_public_key: [0x04; 65],
        client_hpke_public_key: [0x55; 32],
        exact_begin_response_bytes: exact_response.to_vec(),
        verification_code: "428173".into(),
        authority_now_ms: NOW_MS + 100,
    }
}

fn primary_consume() -> ConsumeInvitation {
    consume_request(
        HELLO_ID,
        RECEIPT_ID,
        PHONE_DEVICE_ID,
        [0x66; 32],
        b"server-hello\0fixture-v1",
    )
}

fn confirmation() -> ConfirmEnrollment {
    ConfirmEnrollment {
        receipt_id: RECEIPT_ID.into(),
        confirmation_digest: [0x76; 32],
        displayed_verification_code: "428173".into(),
        displayed_scopes_json: r#"["note","folder"]"#.into(),
        approved: true,
        bootstrap_envelope_bytes: b"sealed-bootstrap\0fixture-v1".to_vec(),
        bootstrap_envelope_digest: [0x77; 32],
        exact_bootstrap_response_bytes: b"bootstrap-response\0fixture-v1".to_vec(),
        authority_now_ms: NOW_MS + 200,
    }
}

fn activation() -> ActivateEnrollment {
    ActivateEnrollment {
        message_id: FINISH_ID.into(),
        receipt_id: RECEIPT_ID.into(),
        device_id: PHONE_DEVICE_ID.into(),
        authority_generation: AUTHORITY_GENERATION,
        request_digest: [0x88; 32],
        observed_tls_spki_sha256: [0x44; 32],
        exact_server_finish_bytes: b"server-finish\0fixture-v1".to_vec(),
        authority_now_ms: NOW_MS + 300,
    }
}

fn register(database: &TestDatabase) {
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::register_invitation(transaction, &invitation(), NOW_MS)
        })
        .expect("register invitation"),
        InvitationRegistration::Registered
    );
}

fn consume(database: &TestDatabase) {
    let request = primary_consume();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &request)
        })
        .expect("consume invitation"),
        ConsumeOutcome::Consumed(request.exact_begin_response_bytes)
    );
}

fn confirm(database: &TestDatabase) {
    let request = confirmation();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::confirm_enrollment(transaction, &request)
        })
        .expect("confirm enrollment"),
        ConfirmOutcome::Confirmed(request.exact_bootstrap_response_bytes)
    );
}

fn activate(database: &TestDatabase) {
    let request = activation();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &request)
        })
        .expect("activate enrollment"),
        ActivateOutcome::Activated(request.exact_server_finish_bytes)
    );
}

fn accepted_change(transaction_id: &str, counter: u64, digest_byte: &str) -> FixtureAcceptedChange {
    FixtureAcceptedChange {
        request_id: if counter == 1 {
            "018f47a0-7b80-7000-8000-000000000010".into()
        } else {
            "018f47a0-7b80-7000-8000-000000000011".into()
        },
        request_digest: [0x90 + counter as u8; 32],
        exact_response_bytes: format!("push-response-{counter}\0exact").into_bytes(),
        library_id: LIBRARY_ID.into(),
        authority_generation: AUTHORITY_GENERATION,
        transaction_id: transaction_id.into(),
        device_id: PHONE_DEVICE_ID.into(),
        device_transaction_counter: counter,
        transaction_digest: digest_byte.repeat(32),
        transaction_json: format!(r#"{{"fixtureCounter":{counter}}}"#),
        receipt_json: format!(r#"{{"acceptedCounter":{counter}}}"#),
        authority_now_ms: NOW_MS + 400 + counter as i64,
        created_at_ms: NOW_MS + 400 + counter as i64,
        expires_at_ms: NOW_MS + 60_000,
    }
}

fn checkpoint(cursor: u64, digest_byte: &str, response: &[u8]) -> IssueCheckpoint {
    IssueCheckpoint {
        library_id: LIBRARY_ID.into(),
        authority_generation: AUTHORITY_GENERATION,
        high_water_cursor: cursor,
        purge_generation: PURGE_GENERATION,
        key_epoch: KEY_EPOCH,
        checkpoint_digest: digest_byte.repeat(32),
        exact_response_bytes: response.to_vec(),
        created_at_ms: NOW_MS + 500 + cursor as i64,
    }
}

fn acknowledgement(checkpoint: &IssueCheckpoint, response: &[u8]) -> AcknowledgeCheckpoint {
    AcknowledgeCheckpoint {
        library_id: LIBRARY_ID.into(),
        device_id: PHONE_DEVICE_ID.into(),
        authority_generation: AUTHORITY_GENERATION,
        high_water_cursor: checkpoint.high_water_cursor,
        checkpoint_digest: checkpoint.checkpoint_digest.clone(),
        exact_response_bytes: response.to_vec(),
        authority_now_ms: NOW_MS + 600 + checkpoint.high_water_cursor as i64,
    }
}

#[test]
fn schema_verifier_survives_reopen_and_rejects_non_fixture_or_incomplete_state() {
    let database = TestDatabase::new();
    seed_database(&database);
    register(&database);

    for failed_attempts in 1..MAX_FIXTURE_ATTEMPTS {
        assert_eq!(
            write(&database.path, |transaction| {
                DirectAuthorityStore::record_invitation_failure(
                    transaction,
                    INVITATION_ID,
                    NOW_MS + failed_attempts as i64,
                )
            })
            .expect("persist failed invitation attempt"),
            AttemptOutcome::Recorded {
                failed_attempts,
                attempts_remaining: MAX_FIXTURE_ATTEMPTS - failed_attempts,
            }
        );
    }
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::record_invitation_failure(
                transaction,
                INVITATION_ID,
                NOW_MS + MAX_FIXTURE_ATTEMPTS as i64,
            )
        })
        .expect("persist terminal invitation attempt"),
        AttemptOutcome::Cancelled
    );

    let connection = open(&database.path);
    DirectAuthorityStore::verify_schema(&connection).expect("verify after reopen");
    let attempts: (i64, String) = connection
        .query_row(
            "SELECT failed_attempts, state FROM direct_pairing_invitations
             WHERE invitation_id = ?1",
            [INVITATION_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load durable attempt limit");
    assert_eq!(attempts, (MAX_FIXTURE_ATTEMPTS as i64, "cancelled".into()));
    connection
        .execute(
            "UPDATE direct_authority_profiles SET environment = 'production'",
            [],
        )
        .expect("simulate unsafe profile drift");
    assert_eq!(
        DirectAuthorityStore::verify_schema(&connection),
        Err(StoreError::FixtureOnly)
    );
    connection
        .execute(
            "UPDATE direct_authority_profiles SET environment = 'development'",
            [],
        )
        .expect("restore fixture profile");
    connection
        .execute_batch("DROP TRIGGER direct_sync_checkpoints_no_delete;")
        .expect("simulate incomplete expansion");
    assert_eq!(
        DirectAuthorityStore::verify_schema(&connection),
        Err(StoreError::StateUnavailable(
            "direct authority schema is incomplete"
        ))
    );
}

#[test]
fn expired_abandoned_receipt_does_not_block_fresh_device_enrollment() {
    const FRESH_INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000012";
    const FRESH_HELLO_ID: &str = "018f47a0-7b80-7000-8000-000000000013";
    const FRESH_RECEIPT_ID: &str = "018f47a0-7b80-7000-8000-000000000014";

    let database = TestDatabase::new();
    seed_database(&database);
    register(&database);
    consume(&database);

    let old_expiry = invitation().expires_at_ms;
    let mut fresh_invitation = invitation();
    fresh_invitation.invitation_id = FRESH_INVITATION_ID.into();
    fresh_invitation.invitation_digest = [0xa1; 32];
    fresh_invitation.nonce_hash = [0xa2; 32];
    fresh_invitation.created_at_ms = old_expiry;
    fresh_invitation.expires_at_ms = old_expiry + 300_000;
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::register_invitation(transaction, &fresh_invitation, old_expiry)
        })
        .expect("register fresh invitation after old receipt expiry"),
        InvitationRegistration::Registered
    );

    let mut fresh_hello = consume_request(
        FRESH_HELLO_ID,
        FRESH_RECEIPT_ID,
        PHONE_DEVICE_ID,
        [0xa3; 32],
        b"fresh-server-hello\0fixture-v1",
    );
    fresh_hello.invitation_id = FRESH_INVITATION_ID.into();
    fresh_hello.authority_now_ms = old_expiry + 1;
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &fresh_hello)
        })
        .expect("consume fresh invitation despite abandoned old receipt"),
        ConsumeOutcome::Consumed(fresh_hello.exact_begin_response_bytes.clone())
    );

    let connection = open(&database.path);
    let old_states: (String, String) = connection
        .query_row(
            "SELECT r.state, i.state
             FROM direct_enrollment_receipts r
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE r.receipt_id = ?1",
            [RECEIPT_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load retired enrollment state");
    assert_eq!(old_states, ("expired".into(), "expired".into()));
    let fresh_state: String = connection
        .query_row(
            "SELECT state FROM direct_enrollment_receipts WHERE receipt_id = ?1",
            [FRESH_RECEIPT_ID],
            |row| row.get(0),
        )
        .expect("load fresh enrollment state");
    assert_eq!(fresh_state, "pending_user_confirmation");
}

#[test]
fn committed_pairing_checkpoint_ack_and_revoke_state_survive_reopen() {
    let database = TestDatabase::new();
    seed_database(&database);
    register(&database);

    let hello = primary_consume();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &hello)
        })
        .expect("consume invitation"),
        ConsumeOutcome::Consumed(hello.exact_begin_response_bytes.clone())
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &hello)
        })
        .expect("replay ClientHello after reopen"),
        ConsumeOutcome::ExactReplay(hello.exact_begin_response_bytes.clone())
    );

    let confirmation = confirmation();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::confirm_enrollment(transaction, &confirmation)
        })
        .expect("confirm enrollment"),
        ConfirmOutcome::Confirmed(confirmation.exact_bootstrap_response_bytes.clone())
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::confirm_enrollment(transaction, &confirmation)
        })
        .expect("replay confirmation after reopen"),
        ConfirmOutcome::ExactReplay(confirmation.exact_bootstrap_response_bytes.clone())
    );
    let mut changed_confirmation = confirmation.clone();
    changed_confirmation.confirmation_digest = [0x75; 32];
    changed_confirmation.displayed_verification_code = "000000".into();
    changed_confirmation.displayed_scopes_json = "[]".into();
    changed_confirmation.approved = false;
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::confirm_enrollment(transaction, &changed_confirmation)
        })
        .expect("quarantine changed confirmation replay"),
        ConfirmOutcome::Quarantined
    );

    let finish = activation();
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &finish)
        })
        .expect("activate enrollment"),
        ActivateOutcome::Activated(finish.exact_server_finish_bytes.clone())
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &finish)
        })
        .expect("replay ClientFinish after reopen"),
        ActivateOutcome::ExactReplay(finish.exact_server_finish_bytes.clone())
    );

    let connection = open(&database.path);
    connection
        .execute(
            "UPDATE libraries SET authority_generation = 8 WHERE library_id = ?1",
            [LIBRARY_ID],
        )
        .expect("rotate authority generation");
    drop(connection);
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &hello)
        }),
        Err(StoreError::StateUnavailable("authority generation changed"))
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &finish)
        }),
        Err(StoreError::StateUnavailable("authority generation changed"))
    );
    open(&database.path)
        .execute(
            "UPDATE libraries SET authority_generation = ?2 WHERE library_id = ?1",
            params![LIBRARY_ID, AUTHORITY_GENERATION],
        )
        .expect("restore fixture generation for lifecycle test");

    let mut first_change = accepted_change(TRANSACTION_ONE_ID, 1, "1a");
    first_change.created_at_ms = first_change.authority_now_ms + 86_400_000;
    first_change.expires_at_ms = first_change.created_at_ms + 60_000;
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::append_fixture_accepted_change(transaction, &first_change)
        })
        .expect("append first fixture change"),
        FixtureChangeOutcome::Accepted {
            cursor: 1,
            exact_response_bytes: first_change.exact_response_bytes.clone(),
        }
    );
    let replay_time: i64 = open(&database.path)
        .query_row(
            "SELECT created_at_ms FROM direct_request_replays
             WHERE device_id = ?1 AND request_id = ?2",
            params![PHONE_DEVICE_ID, first_change.request_id],
            |row| row.get(0),
        )
        .expect("load authority-owned replay timestamp");
    assert_eq!(replay_time, first_change.authority_now_ms);
    let first_checkpoint = checkpoint(1, "2b", b"checkpoint-one\0exact");
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::issue_checkpoint(transaction, &first_checkpoint)
        })
        .expect("issue first checkpoint"),
        CheckpointOutcome::Issued(first_checkpoint.exact_response_bytes.clone())
    );
    let first_ack = acknowledgement(&first_checkpoint, b"ack-one\0exact");
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::acknowledge_checkpoint(transaction, &first_ack)
        })
        .expect("acknowledge first checkpoint"),
        AckOutcome::Recorded(first_ack.exact_response_bytes.clone())
    );

    let second_change = accepted_change(TRANSACTION_TWO_ID, 2, "3c");
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::append_fixture_accepted_change(transaction, &second_change)
        })
        .expect("append second fixture change"),
        FixtureChangeOutcome::Accepted {
            cursor: 2,
            exact_response_bytes: second_change.exact_response_bytes.clone(),
        }
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::append_fixture_accepted_change(transaction, &first_change)
        })
        .expect("replay accepted push after cursor advances"),
        FixtureChangeOutcome::ExactReplay(first_change.exact_response_bytes.clone())
    );
    let second_checkpoint = checkpoint(2, "4d", b"checkpoint-two\0exact");
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::issue_checkpoint(transaction, &second_checkpoint)
        })
        .expect("issue second checkpoint"),
        CheckpointOutcome::Issued(second_checkpoint.exact_response_bytes.clone())
    );
    let second_ack = acknowledgement(&second_checkpoint, b"ack-two\0exact");
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::acknowledge_checkpoint(transaction, &second_ack)
        })
        .expect("acknowledge second checkpoint"),
        AckOutcome::Recorded(second_ack.exact_response_bytes.clone())
    );

    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::issue_checkpoint(transaction, &first_checkpoint)
        })
        .expect("replay old checkpoint after cursor advances"),
        CheckpointOutcome::ExactReplay(first_checkpoint.exact_response_bytes.clone())
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::acknowledge_checkpoint(transaction, &second_ack)
        })
        .expect("replay latest acknowledgement after reopen"),
        AckOutcome::ExactReplay(second_ack.exact_response_bytes.clone())
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::acknowledge_checkpoint(transaction, &first_ack)
        }),
        Err(StoreError::AckMismatch)
    );

    let connection = open(&database.path);
    let persisted_checkpoint: Vec<u8> = connection
        .query_row(
            "SELECT exact_response_bytes FROM direct_sync_checkpoints
             WHERE library_id = ?1 AND high_water_cursor = 1",
            [LIBRARY_ID],
            |row| row.get(0),
        )
        .expect("load exact checkpoint bytes");
    assert_eq!(persisted_checkpoint, first_checkpoint.exact_response_bytes);
    let persisted_ack: Vec<u8> = connection
        .query_row(
            "SELECT last_ack_response_bytes FROM direct_device_sync_state
             WHERE device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| row.get(0),
        )
        .expect("load exact acknowledgement bytes");
    assert_eq!(persisted_ack, second_ack.exact_response_bytes);
    DirectAuthorityStore::verify_schema(&connection).expect("verify committed lifecycle");
    drop(connection);

    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::revoke_device(transaction, PHONE_DEVICE_ID, NOW_MS + 900)
        })
        .expect("revoke replica"),
        RevokeOutcome::Revoked
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::revoke_device(transaction, PHONE_DEVICE_ID, NOW_MS + 901)
        })
        .expect("repeat revocation"),
        RevokeOutcome::AlreadyRevoked
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::acknowledge_checkpoint(transaction, &second_ack)
        }),
        Err(StoreError::DeviceRevoked)
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &hello)
        }),
        Err(StoreError::DeviceRevoked)
    );
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &finish)
        }),
        Err(StoreError::DeviceRevoked)
    );

    let connection = open(&database.path);
    let states: (String, String, String) = connection
        .query_row(
            "SELECT d.enrollment_state, r.state, i.state
             FROM portable_devices d
             JOIN direct_enrollment_receipts r ON r.device_id = d.device_id
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE d.device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("load revoked cross-links after reopen");
    assert_eq!(
        states,
        ("revoked".into(), "revoked".into(), "revoked".into())
    );
    DirectAuthorityStore::verify_schema(&connection).expect("verify revoked lifecycle");
    let timestamps: (String, String) = connection
        .query_row(
            "SELECT created_at, revoked_at FROM portable_devices WHERE device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load portable timestamps");
    chrono::DateTime::parse_from_rfc3339(&timestamps.0).expect("created_at is RFC 3339");
    chrono::DateTime::parse_from_rfc3339(&timestamps.1).expect("revoked_at is RFC 3339");
    connection
        .execute(
            "UPDATE libraries
             SET authority_generation = 8, purge_generation = 3, current_key_epoch = 2
             WHERE library_id = ?1",
            [LIBRARY_ID],
        )
        .expect("advance authority epochs");
    DirectAuthorityStore::verify_schema(&connection)
        .expect("historical checkpoints remain valid after monotonic epoch rotation");
}

#[test]
fn sqlite_abort_failpoints_roll_back_consume_activate_and_revoke_links() {
    let database = TestDatabase::new();
    seed_database(&database);
    register(&database);

    let connection = open(&database.path);
    connection
        .execute_batch(
            "CREATE TRIGGER fixture_abort_consume
             BEFORE UPDATE OF state ON direct_pairing_invitations
             WHEN NEW.state = 'consumed'
             BEGIN SELECT RAISE(ABORT, 'fixture consume failpoint'); END;",
        )
        .expect("install consume failpoint");
    drop(connection);
    let hello = primary_consume();
    assert!(matches!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::consume_invitation(transaction, &hello)
        }),
        Err(StoreError::Database(_))
    ));

    let connection = open(&database.path);
    let consume_state: (String, i64, i64) = connection
        .query_row(
            "SELECT i.state,
                    (SELECT COUNT(*) FROM direct_enrollment_receipts),
                    (SELECT COUNT(*) FROM direct_pairing_replays)
             FROM direct_pairing_invitations i WHERE i.invitation_id = ?1",
            [INVITATION_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect rolled-back consume");
    assert_eq!(consume_state, ("pending".into(), 0, 0));
    connection
        .execute_batch("DROP TRIGGER fixture_abort_consume;")
        .expect("remove consume failpoint");
    drop(connection);
    consume(&database);
    confirm(&database);

    for failed_attempts in 1..MAX_FIXTURE_ATTEMPTS {
        assert_eq!(
            write(&database.path, |transaction| {
                DirectAuthorityStore::record_finish_failure(
                    transaction,
                    RECEIPT_ID,
                    NOW_MS + 200 + failed_attempts as i64,
                )
            })
            .expect("persist failed finish attempt"),
            AttemptOutcome::Recorded {
                failed_attempts,
                attempts_remaining: MAX_FIXTURE_ATTEMPTS - failed_attempts,
            }
        );
    }
    let persisted_finish_attempts: i64 = open(&database.path)
        .query_row(
            "SELECT failed_finish_attempts FROM direct_enrollment_receipts
             WHERE receipt_id = ?1",
            [RECEIPT_ID],
            |row| row.get(0),
        )
        .expect("load failed finish attempts after reopen");
    assert_eq!(persisted_finish_attempts, 4);

    let connection = open(&database.path);
    connection
        .execute_batch(
            "CREATE TRIGGER fixture_abort_activation
             BEFORE UPDATE OF state ON direct_enrollment_receipts
             WHEN NEW.state = 'active'
             BEGIN SELECT RAISE(ABORT, 'fixture activation failpoint'); END;",
        )
        .expect("install activation failpoint");
    drop(connection);
    let finish = activation();
    assert!(matches!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::activate_enrollment(transaction, &finish)
        }),
        Err(StoreError::Database(_))
    ));

    let connection = open(&database.path);
    let activation_state: (String, String, i64, i64) = connection
        .query_row(
            "SELECT r.state, i.state,
                    (SELECT COUNT(*) FROM portable_devices WHERE device_id = ?2),
                    (SELECT COUNT(*) FROM direct_pairing_replays
                     WHERE message_kind = 'client_finish')
             FROM direct_enrollment_receipts r
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE r.receipt_id = ?1",
            params![RECEIPT_ID, PHONE_DEVICE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("inspect rolled-back activation");
    assert_eq!(
        activation_state,
        ("pending_finish".into(), "consumed".into(), 0, 0)
    );
    connection
        .execute_batch("DROP TRIGGER fixture_abort_activation;")
        .expect("remove activation failpoint");
    drop(connection);
    activate(&database);

    let connection = open(&database.path);
    connection
        .execute_batch(
            "CREATE TRIGGER fixture_abort_revocation
             BEFORE UPDATE OF state ON direct_enrollment_receipts
             WHEN NEW.state = 'revoked'
             BEGIN SELECT RAISE(ABORT, 'fixture revocation failpoint'); END;",
        )
        .expect("install revocation failpoint");
    drop(connection);
    assert!(matches!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::revoke_device(transaction, PHONE_DEVICE_ID, NOW_MS + 900)
        }),
        Err(StoreError::Database(_))
    ));

    let connection = open(&database.path);
    let revoked_state: (String, String, String) = connection
        .query_row(
            "SELECT d.enrollment_state, r.state, i.state
             FROM portable_devices d
             JOIN direct_enrollment_receipts r ON r.device_id = d.device_id
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE d.device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect rolled-back revocation");
    assert_eq!(
        revoked_state,
        ("active".into(), "active".into(), "active".into())
    );
    connection
        .execute_batch("DROP TRIGGER fixture_abort_revocation;")
        .expect("remove revocation failpoint");
    drop(connection);
    assert_eq!(
        write(&database.path, |transaction| {
            DirectAuthorityStore::revoke_device(transaction, PHONE_DEVICE_ID, NOW_MS + 901)
        })
        .expect("retry revocation"),
        RevokeOutcome::Revoked
    );
    DirectAuthorityStore::verify_schema(&open(&database.path))
        .expect("verify state after rollback retries");
}

#[test]
fn simultaneous_client_hello_transactions_have_exactly_one_sqlite_winner() {
    let database = TestDatabase::new();
    seed_database(&database);
    register(&database);

    let first = consume_request(
        "018f47a0-7b80-7000-8000-00000000000a",
        "018f47a0-7b80-7000-8000-00000000000b",
        "018f47a0-7b80-7000-8000-00000000000c",
        [0xa1; 32],
        b"winner-a",
    );
    let second = consume_request(
        "018f47a0-7b80-7000-8000-00000000000d",
        "018f47a0-7b80-7000-8000-00000000000e",
        "018f47a0-7b80-7000-8000-00000000000f",
        [0xb2; 32],
        b"winner-b",
    );
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for request in [first, second] {
        let barrier = Arc::clone(&barrier);
        let path = database.path.clone();
        handles.push(thread::spawn(move || {
            let connection = open(&path);
            drop(connection);
            barrier.wait();
            write(&path, |transaction| {
                DirectAuthorityStore::consume_invitation(transaction, &request)
            })
        }));
    }
    barrier.wait();
    let outcomes: Vec<StoreResult<ConsumeOutcome>> = handles
        .into_iter()
        .map(|handle| handle.join().expect("consumer thread did not panic"))
        .collect();

    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(ConsumeOutcome::Consumed(_))))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(StoreError::InvitationConsumed)))
            .count(),
        1
    );

    let connection = open(&database.path);
    let durable_counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM direct_enrollment_receipts),
               (SELECT COUNT(*) FROM direct_pairing_replays
                WHERE message_kind = 'client_hello'),
               (SELECT COUNT(*) FROM direct_pairing_invitations
                WHERE state = 'consumed')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect one-winner durable state");
    assert_eq!(durable_counts, (1, 1, 1));
    DirectAuthorityStore::verify_schema(&connection).expect("verify one-winner state");
}
