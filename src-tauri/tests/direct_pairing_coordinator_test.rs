use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc, Barrier,
    },
    thread,
};

use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use tauri_app_lib::{
    direct_authority_store::{DirectAuthorityStore, InvitationRegistration, StoreError},
    direct_pairing::{
        AuthorityBindings, AuthorityClock, AuthorityClockError, CoordinatorError,
        DirectPairingCoordinator, OwnerConfirmationResult,
    },
    pairing_protocol::{
        canonical_client_finish_unsigned, canonical_client_hello_unsigned,
        canonical_invitation_unsigned, enrollment_confirmation_digest, invitation_nonce_proof,
        AuthenticatedHpkeEnvelope, AuthenticatedHpkeSeal, BootstrapEnvelope, BootstrapMetadataV1,
        ClientFinish, ClientHello, Environment, FreshValuePurpose, Invitation, KindCapability,
        LibraryDataClass, LocalHpkeKey, LocalSigningKey, PairingCrypto, PairingError,
        PairingPolicy, PairingRole, RecordKind, ScopeClass, ServerHello, TransportEvidence,
        BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES, HPKE_EXPORTER_SECRET_BYTES, PAIRING_PROTOCOL,
        PAIRING_SUITE,
    },
    sync_protocol::SYNC_PROTOCOL_VERSION,
};
use zeroize::Zeroizing;

const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000101";
const MAC_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000102";
const PHONE_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000103";
const NOW_MS: i64 = 1_776_100_000_000;
const AUTHORITY_GENERATION: u64 = 9;
const UNKNOWN_SCOPE_ID: &str = "018f47a0-7b80-7000-8000-000000000104";

const AUTHORITY_KEY: [u8; 65] = [0xa1; 65];
const MAC_SIGNING_KEY: [u8; 65] = [0xb2; 65];
const MAC_HPKE_KEY: [u8; 32] = [0xc3; 32];
const TLS_PIN: [u8; 32] = [0xd4; 32];
const CLIENT_SIGNING_KEY: [u8; 65] = [0xe5; 65];
const CLIENT_HPKE_KEY: [u8; 32] = [0xf6; 32];

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

fn fixture_bootstrap_key_package(key_epoch: u64) -> Zeroizing<Vec<u8>> {
    let mut package = Zeroizing::new(Vec::with_capacity(48));
    package.extend_from_slice(b"NBK1");
    package.extend_from_slice(&1_u32.to_be_bytes());
    package.extend_from_slice(&key_epoch.to_be_bytes());
    package.extend_from_slice(&[0xa7; 32]);
    package
}

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir().join(format!(
                "noted-direct-pairing-coordinator-{}-{sequence}.sqlite3",
                std::process::id()
            )),
        }
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
    let connection = Connection::open(path).expect("open fixture pairing database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
        .expect("configure fixture pairing database");
    connection
}

fn seed(database: &TestDatabase) {
    let mut connection = open(&database.path);
    connection
        .execute_batch(
            "CREATE TABLE libraries (
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
               WHERE role = 'authority' AND enrollment_state = 'active';
             CREATE TABLE library_scopes (
               scope_id TEXT PRIMARY KEY,
               library_id TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE CASCADE,
               scope_class TEXT NOT NULL CHECK(scope_class IN ('work', 'personal', 'unknown')),
               created_at TEXT NOT NULL,
               UNIQUE(library_id, scope_class)
             );",
        )
        .expect("install portable fixture anchors");
    connection
        .execute(
            "INSERT INTO libraries
             (library_id, authority_generation, purge_generation,
              current_key_epoch, owner_device_id, enrollment_state, created_at)
             VALUES (?1, ?2, 0, 1, ?3, 'local', '2026-08-17T00:00:00Z')",
            params![LIBRARY_ID, AUTHORITY_GENERATION, MAC_DEVICE_ID],
        )
        .expect("seed fixture library");
    connection
        .execute(
            "INSERT INTO portable_devices
             (device_id, library_id, device_kind, display_name, role,
              enrollment_state, capabilities_json, last_transaction_counter,
              created_at, enrolled_at)
             VALUES (?1, ?2, 'macos', 'Sanitized Fixture Mac', 'authority',
                     'active', '{}', 0, '2026-08-17T00:00:00Z',
                     '2026-08-17T00:00:00Z')",
            params![MAC_DEVICE_ID, LIBRARY_ID],
        )
        .expect("seed fixture authority");
    connection
        .execute(
            "INSERT INTO library_scopes(scope_id, library_id, scope_class, created_at)
             VALUES (?1, ?2, 'unknown', '2026-08-17T00:00:00Z')",
            params![UNKNOWN_SCOPE_ID, LIBRARY_ID],
        )
        .expect("seed fixture unknown scope");
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("start fixture schema transaction");
    DirectAuthorityStore::install_schema(&transaction).expect("install direct authority schema");
    DirectAuthorityStore::initialize_fixture_profile(
        &transaction,
        LIBRARY_ID,
        AUTHORITY_GENERATION,
        r#"{"fixtureRecords":true}"#,
        NOW_MS,
    )
    .expect("initialize fixture profile");
    transaction.commit().expect("commit fixture schema");
}

#[derive(Clone)]
struct FixtureClock(Arc<AtomicI64>);

impl FixtureClock {
    fn new(now_ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now_ms)))
    }

    fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl AuthorityClock for FixtureClock {
    fn now_ms(&self) -> Result<i64, AuthorityClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Clone)]
struct FixtureCrypto {
    next_value: Arc<AtomicU64>,
    sign_count: Arc<AtomicU64>,
    seal_count: Arc<AtomicU64>,
    fail_next_sign: Arc<AtomicBool>,
}

impl FixtureCrypto {
    fn new() -> Self {
        Self {
            next_value: Arc::new(AtomicU64::new(0x500)),
            sign_count: Arc::new(AtomicU64::new(0)),
            seal_count: Arc::new(AtomicU64::new(0)),
            fail_next_sign: Arc::new(AtomicBool::new(false)),
        }
    }

    fn operation_counts(&self) -> (u64, u64) {
        (
            self.sign_count.load(Ordering::SeqCst),
            self.seal_count.load(Ordering::SeqCst),
        )
    }

    fn fail_next_sign(&self) {
        self.fail_next_sign.store(true, Ordering::SeqCst);
    }
}

impl PairingCrypto for FixtureCrypto {
    fn verify_signature(
        &self,
        signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        (signature == fixture_signature(signer_role, public_key, message))
            .then_some(())
            .ok_or(())
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        self.sign_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_sign.swap(false, Ordering::SeqCst) {
            return Err(());
        }
        let public_key = match key {
            LocalSigningKey::MacPairing => &MAC_SIGNING_KEY,
            LocalSigningKey::MacAuthority => &AUTHORITY_KEY,
        };
        Ok(fixture_signature(
            PairingRole::MacAuthority,
            public_key,
            message,
        ))
    }

    fn seal_authenticated(
        &self,
        _sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        plaintext: &[u8],
        exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        self.seal_count.fetch_add(1, Ordering::SeqCst);
        let sequence = self.next_value.fetch_add(1, Ordering::SeqCst) as u8;
        let mut hasher = Sha256::new();
        hasher.update(recipient_public_key);
        hasher.update(info);
        hasher.update(associated_data);
        hasher.update(plaintext);
        hasher.update(exporter_context);
        hasher.update([sequence]);
        Ok(AuthenticatedHpkeSeal {
            envelope: AuthenticatedHpkeEnvelope {
                encapsulated_key: vec![sequence; 32],
                ciphertext: hasher.finalize().to_vec(),
            },
            exporter_secret: Zeroizing::new([sequence; HPKE_EXPORTER_SECRET_BYTES]),
        })
    }

    fn seal_bootstrap_key_package(
        &self,
        sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        metadata: &BootstrapMetadataV1,
        exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        let package = fixture_bootstrap_key_package(metadata.key_epoch);
        let mut seal = self.seal_authenticated(
            sender_key,
            recipient_public_key,
            info,
            associated_data,
            package.as_slice(),
            exporter_context,
        )?;
        let tag = Sha256::digest([seal.envelope.ciphertext.as_slice(), associated_data].concat());
        seal.envelope.ciphertext.extend_from_slice(&tag);
        seal.envelope
            .ciphertext
            .truncate(BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES);
        Ok(seal)
    }

    fn fresh_bytes(&self, _purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        let sequence = self.next_value.fetch_add(1, Ordering::SeqCst) as u8;
        Ok(vec![sequence; length])
    }

    fn fresh_uuid_v7(&self, _purpose: FreshValuePurpose) -> Result<String, ()> {
        Ok(uuid(self.next_value.fetch_add(1, Ordering::SeqCst)))
    }
}

fn fixture_signature(role: PairingRole, public_key: &[u8], message: &[u8]) -> Vec<u8> {
    let role = match role {
        PairingRole::MacAuthority => b"mac".as_slice(),
        PairingRole::IphoneCompanion => b"iphone".as_slice(),
    };
    let mut first = Sha256::new();
    first.update(role);
    first.update(public_key);
    first.update(message);
    let first = first.finalize();
    let mut second = Sha256::new();
    second.update(b"fixture-signature-second-half");
    second.update(first);
    [first.as_slice(), second.finalize().as_slice()].concat()
}

fn uuid(value: u64) -> String {
    format!("018f47a0-7b80-7000-8000-{value:012x}")
}

fn scopes() -> BTreeSet<RecordKind> {
    [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
        .into_iter()
        .collect()
}

fn capabilities() -> BTreeMap<RecordKind, KindCapability> {
    [
        (
            RecordKind::Note,
            KindCapability {
                reader_version: 1,
                writer_version: Some(1),
            },
        ),
        (
            RecordKind::Category,
            KindCapability {
                reader_version: 1,
                writer_version: Some(1),
            },
        ),
        (
            RecordKind::Folder,
            KindCapability {
                reader_version: 1,
                writer_version: Some(1),
            },
        ),
    ]
    .into_iter()
    .collect()
}

fn policy() -> PairingPolicy {
    PairingPolicy {
        library_id: LIBRARY_ID.to_owned(),
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_generation: AUTHORITY_GENERATION,
        grantable_scopes: scopes(),
        capabilities: capabilities(),
    }
}

fn bindings() -> AuthorityBindings {
    AuthorityBindings {
        authority_signing_public_key: AUTHORITY_KEY,
        mac_pairing_signing_public_key: MAC_SIGNING_KEY,
        mac_pairing_hpke_public_key: MAC_HPKE_KEY,
        tls_spki_sha256: TLS_PIN,
    }
}

fn make_coordinator(
    database: &TestDatabase,
    crypto: FixtureCrypto,
    clock: FixtureClock,
) -> DirectPairingCoordinator<FixtureCrypto, FixtureClock> {
    DirectPairingCoordinator::new_fixture_only(
        open(&database.path),
        crypto,
        clock,
        policy(),
        bindings(),
    )
    .expect("construct fixture-only pairing coordinator")
}

fn invitation(index: u64, created_at_ms: i64, expires_at_ms: i64) -> Invitation {
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: uuid(0x200 + index),
        invitation_nonce: vec![0x20 + index as u8; 32],
        authority_signing_public_key: AUTHORITY_KEY.to_vec(),
        mac_pairing_signing_public_key: MAC_SIGNING_KEY.to_vec(),
        mac_pairing_hpke_public_key: MAC_HPKE_KEY.to_vec(),
        tls_spki_sha256: TLS_PIN.to_vec(),
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: AUTHORITY_GENERATION,
        scope_ceiling: scopes(),
        created_at_ms,
        expires_at_ms,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = fixture_signature(
        PairingRole::MacAuthority,
        &AUTHORITY_KEY,
        &canonical_invitation_unsigned(&invitation),
    );
    invitation
}

fn client_hello(invitation: &Invitation, index: u64, device_id: &str) -> ClientHello {
    let mut hello = ClientHello {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id: uuid(0x300 + index),
        invitation_id: invitation.invitation_id.clone(),
        nonce_proof: invitation_nonce_proof(&invitation.invitation_nonce),
        client_nonce: vec![0x31 + index as u8; 32],
        proposed_device_id: device_id.to_owned(),
        display_name: format!("Sanitized Fixture iPhone {index}"),
        client_signing_public_key: CLIENT_SIGNING_KEY.to_vec(),
        client_hpke_public_key: CLIENT_HPKE_KEY.to_vec(),
        requested_scopes: scopes(),
        capabilities: capabilities(),
        app_version: "1.0-fixture".to_owned(),
        build_version: "100".to_owned(),
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: AUTHORITY_GENERATION,
        environment: Environment::Development,
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        observed_tls_spki_sha256: TLS_PIN.to_vec(),
        proof_signature: Vec::new(),
    };
    sign_hello(&mut hello);
    hello
}

fn sign_hello(hello: &mut ClientHello) {
    hello.proof_signature = fixture_signature(
        PairingRole::IphoneCompanion,
        &CLIENT_SIGNING_KEY,
        &canonical_client_hello_unsigned(hello),
    );
}

fn finish(hello_index: u64, server: &ServerHello, bootstrap: &BootstrapEnvelope) -> ClientFinish {
    let mut finish = ClientFinish {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id: uuid(0x400 + hello_index),
        receipt_id: server.receipt.receipt_id.clone(),
        invitation_id: server.receipt.invitation_id.clone(),
        library_id: server.receipt.library_id.clone(),
        device_id: server.receipt.device_id.clone(),
        authority_generation: server.receipt.authority_generation,
        environment: Environment::Development,
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        transcript_digest: server.receipt.transcript_digest.clone(),
        bootstrap_envelope_digest: bootstrap.envelope_digest.clone(),
        proof_signature: Vec::new(),
    };
    sign_finish(&mut finish);
    finish
}

fn sign_finish(finish: &mut ClientFinish) {
    finish.proof_signature = fixture_signature(
        PairingRole::IphoneCompanion,
        &CLIENT_SIGNING_KEY,
        &canonical_client_finish_unsigned(finish),
    );
}

fn transport() -> TransportEvidence {
    TransportEvidence {
        tls_version: "1.3".to_owned(),
        used_zero_rtt: false,
        peer_spki_sha256: TLS_PIN.to_vec(),
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("serialize fixture pairing message")
}

fn begin_and_confirm(
    coordinator: &DirectPairingCoordinator<FixtureCrypto, FixtureClock>,
    invitation: &Invitation,
    index: u64,
    device_id: &str,
) -> (ClientHello, ServerHello, String, BootstrapEnvelope, Vec<u8>) {
    assert_eq!(
        coordinator
            .register_invitation(invitation)
            .expect("register invitation"),
        InvitationRegistration::Registered
    );
    let hello = client_hello(invitation, index, device_id);
    let begin = coordinator
        .process_client_hello(&encode(&hello), None, &transport())
        .expect("accept ClientHello");
    let server: ServerHello =
        serde_json::from_slice(&begin.exact_response_bytes).expect("decode ServerHello");
    let code = begin.verification_code.expect("Mac verification code");
    let bootstrap_bytes = match coordinator
        .confirm_owner(&server.receipt.receipt_id, &code, &scopes(), true)
        .expect("confirm fixture owner")
    {
        OwnerConfirmationResult::Bootstrap(bytes) => bytes,
        OwnerConfirmationResult::Cancelled => panic!("fixture confirmation was cancelled"),
    };
    let bootstrap: BootstrapEnvelope =
        serde_json::from_slice(&bootstrap_bytes).expect("decode bootstrap response");
    (hello, server, code, bootstrap, bootstrap_bytes)
}

#[test]
fn raw_connection_constructor_enables_and_verifies_foreign_keys() {
    let database = TestDatabase::new();
    seed(&database);
    let raw_connection = Connection::open(&database.path).expect("open raw SQLite connection");
    raw_connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("simulate an unconfigured raw connection");
    let initially_enabled: i64 = raw_connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("read raw foreign-key setting");
    assert_eq!(initially_enabled, 0);

    let coordinator = DirectPairingCoordinator::new_fixture_only(
        raw_connection,
        FixtureCrypto::new(),
        FixtureClock::new(NOW_MS + 10),
        policy(),
        bindings(),
    )
    .expect("constructor configures and verifies its raw connection");
    assert_eq!(
        coordinator
            .register_invitation(&invitation(1, NOW_MS, NOW_MS + 300_000))
            .expect("foreign-key protected write through configured connection"),
        InvitationRegistration::Registered
    );
}

#[test]
fn restart_cancels_only_unconsumed_invitations() {
    let database = TestDatabase::new();
    seed(&database);
    let crypto = FixtureCrypto::new();
    let clock = FixtureClock::new(NOW_MS + 10);
    let pending = invitation(1, NOW_MS, NOW_MS + 300_000);
    let consumed = invitation(2, NOW_MS, NOW_MS + 300_000);
    let coordinator = make_coordinator(&database, crypto.clone(), clock.clone());
    coordinator
        .register_invitation(&pending)
        .expect("register pending invitation");
    coordinator
        .register_invitation(&consumed)
        .expect("register consumed invitation");
    let consumed_hello = client_hello(&consumed, 2, PHONE_DEVICE_ID);
    let original = coordinator
        .process_client_hello(&encode(&consumed_hello), None, &transport())
        .expect("consume second invitation before restart");
    drop(coordinator);

    let coordinator = make_coordinator(&database, crypto, clock);
    assert_eq!(
        coordinator.process_client_hello(
            &encode(&client_hello(&pending, 1, &uuid(0x991))),
            None,
            &transport(),
        ),
        Err(CoordinatorError::Protocol(
            PairingError::InvitationCancelled
        ))
    );
    let replay = coordinator
        .process_client_hello(&encode(&consumed_hello), None, &transport())
        .expect("consumed receipt remains replayable after restart");
    assert_eq!(replay, original);
}

#[test]
fn committed_pairing_responses_replay_byte_for_byte_after_every_restart() {
    let database = TestDatabase::new();
    seed(&database);
    let crypto = FixtureCrypto::new();
    let clock = FixtureClock::new(NOW_MS + 10);
    let invite = invitation(1, NOW_MS, NOW_MS + 300_000);

    let coordinator = make_coordinator(&database, crypto.clone(), clock.clone());
    coordinator
        .register_invitation(&invite)
        .expect("register fixture invitation");
    let hello = client_hello(&invite, 1, PHONE_DEVICE_ID);
    let first_hello = coordinator
        .process_client_hello(&encode(&hello), None, &transport())
        .expect("first ClientHello");
    let counts_after_hello = crypto.operation_counts();
    drop(coordinator);

    let coordinator = make_coordinator(&database, crypto.clone(), clock.clone());
    let replayed_hello = coordinator
        .process_client_hello(&encode(&hello), None, &transport())
        .expect("restart ClientHello replay");
    assert_eq!(replayed_hello, first_hello);
    assert_eq!(crypto.operation_counts(), counts_after_hello);

    let server: ServerHello = serde_json::from_slice(&first_hello.exact_response_bytes)
        .expect("decode first ServerHello");
    let code = first_hello.verification_code.expect("verification code");
    let first_bootstrap = coordinator
        .confirm_owner(&server.receipt.receipt_id, &code, &scopes(), true)
        .expect("first owner confirmation");
    let counts_after_bootstrap = crypto.operation_counts();
    drop(coordinator);

    let coordinator = make_coordinator(&database, crypto.clone(), clock.clone());
    let replayed_bootstrap = coordinator
        .confirm_owner(&server.receipt.receipt_id, &code, &scopes(), true)
        .expect("restart owner confirmation replay");
    assert_eq!(replayed_bootstrap, first_bootstrap);
    assert_eq!(crypto.operation_counts(), counts_after_bootstrap);
    let bootstrap_bytes = match first_bootstrap {
        OwnerConfirmationResult::Bootstrap(bytes) => bytes,
        OwnerConfirmationResult::Cancelled => panic!("expected bootstrap"),
    };
    let bootstrap: BootstrapEnvelope =
        serde_json::from_slice(&bootstrap_bytes).expect("decode bootstrap");
    assert_eq!(bootstrap.metadata.purge_generation, 0);
    assert_eq!(bootstrap.metadata.key_epoch, 1);
    assert_eq!(bootstrap.metadata.default_scope_id, UNKNOWN_SCOPE_ID);
    assert_eq!(bootstrap.metadata.default_scope_class, ScopeClass::Unknown);
    assert_eq!(
        bootstrap.metadata.sync_protocol_version,
        SYNC_PROTOCOL_VERSION
    );
    assert_eq!(bootstrap.metadata.granted_scopes, scopes());
    assert_eq!(bootstrap.metadata.capabilities, capabilities());
    assert_eq!(
        bootstrap.metadata.durable_sync_spki_sha256,
        TLS_PIN.to_vec()
    );
    let client_finish = finish(1, &server, &bootstrap);
    let first_finish = coordinator
        .process_client_finish(&encode(&client_finish), None, &transport())
        .expect("first ClientFinish");
    let counts_after_finish = crypto.operation_counts();
    drop(coordinator);

    let coordinator = make_coordinator(&database, crypto.clone(), clock);
    let replayed_finish = coordinator
        .process_client_finish(&encode(&client_finish), None, &transport())
        .expect("restart ClientFinish replay");
    assert_eq!(replayed_finish, first_finish);
    assert_eq!(crypto.operation_counts(), counts_after_finish);
    let connection = open(&database.path);
    let state: String = connection
        .query_row(
            "SELECT enrollment_state FROM portable_devices WHERE device_id = ?1",
            [PHONE_DEVICE_ID],
            |row| row.get(0),
        )
        .expect("load activated fixture device");
    assert_eq!(state, "active");
}

#[test]
fn byte_different_replays_are_quarantined_for_all_three_transitions() {
    let database = TestDatabase::new();
    seed(&database);
    let coordinator = make_coordinator(
        &database,
        FixtureCrypto::new(),
        FixtureClock::new(NOW_MS + 10),
    );
    let invite = invitation(1, NOW_MS, NOW_MS + 300_000);
    coordinator
        .register_invitation(&invite)
        .expect("register fixture invitation");
    let hello = client_hello(&invite, 1, PHONE_DEVICE_ID);
    let begin = coordinator
        .process_client_hello(&encode(&hello), None, &transport())
        .expect("accept original ClientHello");
    let mut changed_hello = hello.clone();
    changed_hello.display_name.push_str(" changed");
    sign_hello(&mut changed_hello);
    assert_eq!(
        coordinator.process_client_hello(&encode(&changed_hello), None, &transport()),
        Err(CoordinatorError::Protocol(PairingError::IdReuseQuarantined))
    );

    let server: ServerHello =
        serde_json::from_slice(&begin.exact_response_bytes).expect("decode ServerHello");
    let code = begin.verification_code.expect("verification code");
    let bootstrap_bytes = match coordinator
        .confirm_owner(&server.receipt.receipt_id, &code, &scopes(), true)
        .expect("confirm original decision")
    {
        OwnerConfirmationResult::Bootstrap(bytes) => bytes,
        OwnerConfirmationResult::Cancelled => panic!("expected bootstrap"),
    };
    let connection = open(&database.path);
    let (stored_confirmation, envelope_bytes, envelope_digest): (Vec<u8>, Vec<u8>, Vec<u8>) =
        connection
            .query_row(
                "SELECT confirmation_digest, bootstrap_envelope_bytes,
                        bootstrap_envelope_digest
                 FROM direct_enrollment_receipts WHERE receipt_id = ?1",
                [&server.receipt.receipt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load canonical owner-confirmation commitment");
    assert_eq!(
        stored_confirmation,
        enrollment_confirmation_digest(
            &server.receipt,
            true,
            &code,
            &scopes(),
            &envelope_bytes,
            &envelope_digest,
            &bootstrap_bytes,
        )
    );
    drop(connection);
    assert_eq!(
        coordinator.confirm_owner(&server.receipt.receipt_id, "0000 0000", &scopes(), true,),
        Err(CoordinatorError::Protocol(PairingError::IdReuseQuarantined))
    );

    let bootstrap: BootstrapEnvelope =
        serde_json::from_slice(&bootstrap_bytes).expect("decode bootstrap");
    let original_finish = finish(1, &server, &bootstrap);
    coordinator
        .process_client_finish(&encode(&original_finish), None, &transport())
        .expect("accept original ClientFinish");
    let mut changed_finish = original_finish;
    changed_finish.transcript_digest[0] ^= 0xff;
    sign_finish(&mut changed_finish);
    assert_eq!(
        coordinator.process_client_finish(&encode(&changed_finish), None, &transport()),
        Err(CoordinatorError::Protocol(PairingError::IdReuseQuarantined))
    );

    let connection = open(&database.path);
    let quarantines: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM direct_pairing_quarantine",
            [],
            |row| row.get(0),
        )
        .expect("count quarantine evidence");
    assert_eq!(quarantines, 3);
}

#[test]
fn expired_and_revoked_enrollments_never_replay_authority_bytes() {
    let expired_database = TestDatabase::new();
    seed(&expired_database);
    let expired_clock = FixtureClock::new(NOW_MS);
    let expired_coordinator = make_coordinator(
        &expired_database,
        FixtureCrypto::new(),
        expired_clock.clone(),
    );
    let short_invite = invitation(1, NOW_MS, NOW_MS + 100);
    expired_coordinator
        .register_invitation(&short_invite)
        .expect("register short invitation");
    expired_clock.set(NOW_MS + 101);
    assert_eq!(
        expired_coordinator.process_client_hello(
            &encode(&client_hello(&short_invite, 1, PHONE_DEVICE_ID)),
            None,
            &transport(),
        ),
        Err(CoordinatorError::Protocol(PairingError::InvitationExpired))
    );

    let revoked_database = TestDatabase::new();
    seed(&revoked_database);
    let revoked_coordinator = make_coordinator(
        &revoked_database,
        FixtureCrypto::new(),
        FixtureClock::new(NOW_MS + 10),
    );
    let invite = invitation(2, NOW_MS, NOW_MS + 300_000);
    let (_hello, server, _code, bootstrap, _bootstrap_bytes) =
        begin_and_confirm(&revoked_coordinator, &invite, 2, PHONE_DEVICE_ID);
    let client_finish = finish(2, &server, &bootstrap);
    revoked_coordinator
        .process_client_finish(&encode(&client_finish), None, &transport())
        .expect("activate fixture device");
    let mut connection = open(&revoked_database.path);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("start sole revocation-path transaction");
    DirectAuthorityStore::revoke_device(&transaction, PHONE_DEVICE_ID, NOW_MS + 20)
        .expect("revoke fixture device through the authority store");
    transaction.commit().expect("commit fixture revocation");
    assert_eq!(
        revoked_coordinator.process_client_finish(&encode(&client_finish), None, &transport(),),
        Err(CoordinatorError::Store(StoreError::DeviceRevoked))
    );
}

#[test]
fn cryptographic_failure_before_commit_leaves_a_retryable_invitation() {
    let database = TestDatabase::new();
    seed(&database);
    let crypto = FixtureCrypto::new();
    let coordinator = make_coordinator(&database, crypto.clone(), FixtureClock::new(NOW_MS + 10));
    let invite = invitation(1, NOW_MS, NOW_MS + 300_000);
    coordinator
        .register_invitation(&invite)
        .expect("register retry fixture invitation");
    let hello = client_hello(&invite, 1, PHONE_DEVICE_ID);
    crypto.fail_next_sign();
    assert_eq!(
        coordinator.process_client_hello(&encode(&hello), None, &transport()),
        Err(CoordinatorError::Protocol(PairingError::CryptoUnavailable))
    );

    let connection = open(&database.path);
    let state: (String, i64, i64) = connection
        .query_row(
            "SELECT i.state, i.failed_attempts,
                    (SELECT COUNT(*) FROM direct_enrollment_receipts)
             FROM direct_pairing_invitations i WHERE i.invitation_id = ?1",
            [&invite.invitation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("load retryable pre-commit state");
    assert_eq!(state, ("pending".to_owned(), 1, 0));
    drop(connection);

    coordinator
        .process_client_hello(&encode(&hello), None, &transport())
        .expect("retry succeeds after pre-commit crypto failure");
}

#[test]
fn parallel_enrollments_for_one_device_have_one_durable_winner() {
    let database = TestDatabase::new();
    seed(&database);
    let coordinator = Arc::new(make_coordinator(
        &database,
        FixtureCrypto::new(),
        FixtureClock::new(NOW_MS + 10),
    ));
    let first_invitation = invitation(1, NOW_MS, NOW_MS + 300_000);
    let second_invitation = invitation(2, NOW_MS, NOW_MS + 300_000);
    coordinator
        .register_invitation(&first_invitation)
        .expect("register first invitation");
    coordinator
        .register_invitation(&second_invitation)
        .expect("register second invitation");
    let first = encode(&client_hello(&first_invitation, 1, PHONE_DEVICE_ID));
    let second = encode(&client_hello(&second_invitation, 2, PHONE_DEVICE_ID));
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = [first, second]
        .into_iter()
        .map(|body| {
            let coordinator = Arc::clone(&coordinator);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                coordinator.process_client_hello(&body, None, &transport())
            })
        })
        .collect();
    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("join enrollment thread"))
        .collect();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| {
                **outcome
                    == Err(CoordinatorError::Protocol(PairingError::BindingMismatch(
                        "device_id",
                    )))
            })
            .count(),
        1
    );

    let connection = open(&database.path);
    let live_receipts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM direct_enrollment_receipts
             WHERE device_id = ?1
               AND state IN ('pending_user_confirmation', 'pending_finish', 'active')",
            [PHONE_DEVICE_ID],
            |row| row.get(0),
        )
        .expect("count live fixture receipts");
    assert_eq!(live_receipts, 1);
}
