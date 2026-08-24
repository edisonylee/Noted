use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::{Connection, OpenFlags};
use tauri_app_lib::{
    db,
    direct_authority_store::InvitationRegistration,
    direct_pairing::{AuthorityBindings, AuthorityClock, AuthorityClockError},
    direct_sync::{DirectEndpoint, DirectSyncAuthority, DirectSyncCrypto},
    durable_direct_sync::{FixtureAuthorityClock, SqliteDirectSyncAuthority},
    fixture_authority_runtime::{
        provision_sanitized_fixture_authority, verify_generated_fixture_seed_mutation,
        FixtureAuthorityError, SanitizedFixtureAuthorityRuntime,
    },
    pairing_protocol::{
        AuthenticatedHpkeSeal, BootstrapMetadataV1, Environment, FreshValuePurpose, Invitation,
        LibraryDataClass, LocalHpkeKey, LocalSigningKey, PairingCrypto, PairingRole, RecordKind,
        PAIRING_PROTOCOL, PAIRING_SUITE,
    },
    portable::new_uuid_v7,
    sync_protocol::MutationEnvelope,
};

const NOW_MS: i64 = 1_786_968_000_000;

struct TestDirectory {
    directory: PathBuf,
    database: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let temp_root = fs::canonicalize(std::env::temp_dir()).expect("canonical temp root");
        let directory = temp_root.join(format!(
            "noted-{label}-{}-{}",
            std::process::id(),
            new_uuid_v7()
        ));
        fs::create_dir(&directory).expect("create isolated test directory");
        Self {
            database: directory.join("sanitized-fixture.sqlite"),
            directory,
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        for path in [
            self.database.clone(),
            sidecar(&self.database, "-wal"),
            sidecar(&self.database, "-shm"),
            sidecar(&self.database, "-journal"),
        ] {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn open(path: &Path) -> Connection {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open fixture database");
    connection
        .execute_batch("PRAGMA foreign_keys=ON;")
        .expect("enable foreign keys");
    connection
}

#[derive(Clone)]
struct TestClock;

impl FixtureAuthorityClock for TestClock {
    fn now_ms(&self) -> Result<i64, ()> {
        Ok(NOW_MS)
    }
}

#[test]
fn provisioner_publishes_only_generated_notes_slice_with_bootstrap_ciphertext() {
    let test = TestDirectory::new("fixture-provision");
    let descriptor = provision_sanitized_fixture_authority(&test.database).expect("provision");
    let published_files = fs::read_dir(&test.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(published_files, vec![test.database.file_name().unwrap()]);

    assert_eq!(descriptor.database_path, test.database);
    assert_eq!(descriptor.authority_generation, 1);
    assert_eq!(descriptor.purge_generation, 0);
    assert_eq!(descriptor.key_epoch, 1);
    assert_eq!(descriptor.environment, Environment::Development);
    assert_eq!(
        descriptor.library_data_class,
        LibraryDataClass::SanitizedFixture
    );
    assert_eq!(
        descriptor
            .capabilities
            .record_kinds
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["category", "folder", "note"])
    );
    for capability in descriptor.capabilities.record_kinds.values() {
        assert_eq!(capability.max_read_schema_version, 1);
        assert_eq!(capability.max_write_schema_version, 1);
    }

    let connection = open(&test.database);
    let default_scope: String = connection
        .query_row(
            "SELECT scope_class FROM library_scopes WHERE scope_id = ?1",
            [&descriptor.default_scope_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default_scope, "unknown");
    let fixture_note: (String, String, Option<String>) = connection
        .query_row(
            "SELECT title, raw_text, image_path FROM notes
             WHERE source = 'sanitized_fixture'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("generated note");
    assert_eq!(fixture_note.0, "Generated phone sync fixture");
    assert!(fixture_note
        .1
        .contains("Generated development-only content"));
    assert_eq!(fixture_note.2, None);
    let category_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM categories WHERE name = 'Sanitized Fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(category_count, 1);
    drop(connection);

    let authority = SqliteDirectSyncAuthority::open_sanitized_fixture(
        &test.database,
        &descriptor.library_id,
        Arc::new(TestClock),
    )
    .expect("open durable authority");
    let snapshot = authority.bootstrap().expect("validated bootstrap");
    assert_eq!(snapshot.high_water_cursor, 1);
    assert!(!snapshot.records.is_empty());
    #[cfg(feature = "sanitized-development-fixtures")]
    {
        assert!(snapshot
            .records
            .iter()
            .all(|record| record.mutation.ciphertext.starts_with(b"NRC1")));
        assert!(snapshot.records.iter().all(|record| !record
            .mutation
            .ciphertext
            .windows(b"fixture-json:".len())
            .any(|window| window == b"fixture-json:")));
    }
    #[cfg(not(feature = "sanitized-development-fixtures"))]
    assert!(snapshot
        .records
        .iter()
        .all(|record| record.mutation.ciphertext.starts_with(b"fixture-json:")));
    assert!(snapshot
        .records
        .iter()
        .all(|record| verify_generated_fixture_seed_mutation(&record.mutation)));
    assert_eq!(
        snapshot
            .records
            .iter()
            .map(|record| record.mutation.record_kind.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["category", "folder", "note"])
    );
    snapshot.validate().expect("bootstrap contract");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&test.database).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn reprovision_is_read_only_and_restart_stable() {
    let test = TestDirectory::new("fixture-restart");
    let first = provision_sanitized_fixture_authority(&test.database).expect("first provision");
    let connection = open(&test.database);
    let before: (i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM notes),
               (SELECT COUNT(*) FROM categories),
               (SELECT COUNT(*) FROM note_folders),
               (SELECT COUNT(*) FROM direct_authority_transactions),
               (SELECT COUNT(*) FROM direct_sync_checkpoints)",
            [],
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
        .unwrap();
    drop(connection);

    let second = provision_sanitized_fixture_authority(&test.database).expect("verify existing");
    assert_eq!(first, second);
    let connection = open(&test.database);
    let after: (i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM notes),
               (SELECT COUNT(*) FROM categories),
               (SELECT COUNT(*) FROM note_folders),
               (SELECT COUNT(*) FROM direct_authority_transactions),
               (SELECT COUNT(*) FROM direct_sync_checkpoints)",
            [],
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
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn existing_unmarked_database_is_rejected_without_migration_or_repair() {
    let test = TestDirectory::new("fixture-existing");
    let connection = db::init(&test.database).expect("ordinary desktop database");
    connection.close().unwrap();
    let connection = open(&test.database);
    let before_epoch: i64 = connection
        .query_row("SELECT current_key_epoch FROM libraries", [], |row| {
            row.get(0)
        })
        .unwrap();
    let before_profiles: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM direct_authority_profiles",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        provision_sanitized_fixture_authority(&test.database),
        Err(FixtureAuthorityError::ExistingDatabaseNotFixture)
    ));
    let connection = open(&test.database);
    assert_eq!(
        connection
            .query_row("SELECT current_key_epoch FROM libraries", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        before_epoch
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM direct_authority_profiles",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        before_profiles
    );
    let marker_exists: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'fixture_authority_runtime_v1'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!marker_exists);
}

#[test]
fn marked_database_tampering_fails_closed_and_is_not_repaired() {
    let test = TestDirectory::new("fixture-tamper");
    let descriptor = provision_sanitized_fixture_authority(&test.database).expect("provision");
    let connection = open(&test.database);
    connection
        .execute(
            "UPDATE libraries SET current_key_epoch = 2 WHERE library_id = ?1",
            [&descriptor.library_id],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        provision_sanitized_fixture_authority(&test.database),
        Err(FixtureAuthorityError::Integrity(_))
    ));
    assert_eq!(
        open(&test.database)
            .query_row(
                "SELECT current_key_epoch FROM libraries WHERE library_id = ?1",
                [&descriptor.library_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}

struct PairingClock;

impl AuthorityClock for PairingClock {
    fn now_ms(&self) -> Result<i64, AuthorityClockError> {
        Ok(NOW_MS)
    }
}

struct AcceptingPairingCrypto;

impl PairingCrypto for AcceptingPairingCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        _public_key: &[u8],
        _message: &[u8],
        _signature: &[u8],
    ) -> Result<(), ()> {
        Ok(())
    }

    fn sign(&self, _key: LocalSigningKey, _message: &[u8]) -> Result<Vec<u8>, ()> {
        Err(())
    }

    fn seal_authenticated(
        &self,
        _sender_key: LocalHpkeKey,
        _recipient_public_key: &[u8],
        _info: &[u8],
        _associated_data: &[u8],
        _plaintext: &[u8],
        _exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        Err(())
    }

    fn seal_bootstrap_key_package(
        &self,
        _sender_key: LocalHpkeKey,
        _recipient_public_key: &[u8],
        _info: &[u8],
        _associated_data: &[u8],
        _metadata: &BootstrapMetadataV1,
        _exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        Err(())
    }

    fn fresh_bytes(&self, _purpose: FreshValuePurpose, _length: usize) -> Result<Vec<u8>, ()> {
        Err(())
    }

    fn fresh_uuid_v7(&self, _purpose: FreshValuePurpose) -> Result<String, ()> {
        Err(())
    }
}

struct RejectingSyncCrypto;

impl DirectSyncCrypto for RejectingSyncCrypto {
    fn verify_request_signature(
        &self,
        _endpoint: DirectEndpoint,
        _device_id: &str,
        _signing_bytes: &[u8],
        _signature: &[u8],
    ) -> Result<(), ()> {
        Err(())
    }

    fn verify_mutation_ciphertext(
        &self,
        _device_id: &str,
        _mutation: &MutationEnvelope,
    ) -> Result<(), ()> {
        Err(())
    }

    fn authenticate_response(
        &self,
        _endpoint: DirectEndpoint,
        _signing_bytes: &[u8],
    ) -> Result<Vec<u8>, ()> {
        Err(())
    }
}

#[test]
fn runtime_derives_pairing_and_sync_identity_from_one_verified_fixture() {
    let test = TestDirectory::new("fixture-runtime");
    let provisioned =
        provision_sanitized_fixture_authority(&test.database).expect("provision fixture");
    let mut p256 = [0x21_u8; 65];
    p256[0] = 4;
    let mut mac_p256 = [0x22_u8; 65];
    mac_p256[0] = 4;
    let bindings = AuthorityBindings {
        authority_signing_public_key: p256,
        mac_pairing_signing_public_key: mac_p256,
        mac_pairing_hpke_public_key: [0x23; 32],
        tls_spki_sha256: [0x24; 32],
    };
    let runtime = SanitizedFixtureAuthorityRuntime::open(
        &test.database,
        AcceptingPairingCrypto,
        PairingClock,
        RejectingSyncCrypto,
        Arc::new(TestClock),
        bindings.clone(),
    )
    .expect("open shared authority runtime");
    assert_eq!(runtime.descriptor(), &provisioned);

    let invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: new_uuid_v7(),
        invitation_nonce: vec![0x31; 32],
        authority_signing_public_key: bindings.authority_signing_public_key.to_vec(),
        mac_pairing_signing_public_key: bindings.mac_pairing_signing_public_key.to_vec(),
        mac_pairing_hpke_public_key: bindings.mac_pairing_hpke_public_key.to_vec(),
        tls_spki_sha256: bindings.tls_spki_sha256.to_vec(),
        library_id: runtime.descriptor().library_id.clone(),
        authority_generation: runtime.descriptor().authority_generation,
        scope_ceiling: BTreeSet::from([RecordKind::Note, RecordKind::Category, RecordKind::Folder]),
        created_at_ms: NOW_MS,
        expires_at_ms: NOW_MS + 60_000,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: vec![0x32; 64],
    };
    assert_eq!(
        runtime
            .register_invitation(&invitation)
            .expect("register invitation through shared gate"),
        InvitationRegistration::Registered
    );
    let connection = open(&test.database);
    let stored: (String, i64, Vec<u8>) = connection
        .query_row(
            "SELECT library_id, authority_generation, tls_spki_sha256
             FROM direct_pairing_invitations WHERE invitation_id = ?1",
            [&invitation.invitation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(stored.0, provisioned.library_id);
    assert_eq!(stored.1, provisioned.authority_generation as i64);
    assert_eq!(stored.2, bindings.tls_spki_sha256);
}
