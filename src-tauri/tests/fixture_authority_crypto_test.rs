#![cfg(feature = "sanitized-development-fixtures")]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::{Connection, OpenFlags};
use tauri_app_lib::{
    direct_sync::{DirectSyncAuthority, DirectSyncEnrollment},
    durable_direct_sync::{FixtureAuthorityClock, SqliteDirectSyncAuthority},
    fixture_authority_runtime::{
        provision_sanitized_fixture_authority, verify_generated_fixture_seed_mutation,
        FixtureAuthorityError,
    },
    pairing_protocol::Environment,
    portable::new_uuid_v7,
};

const NOW_MS: i64 = 1_786_968_000_000;

struct TestDirectory {
    directory: PathBuf,
    database: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let temp_root = fs::canonicalize(std::env::temp_dir()).unwrap();
        let directory = temp_root.join(format!(
            "noted-{label}-{}-{}",
            std::process::id(),
            new_uuid_v7()
        ));
        fs::create_dir(&directory).unwrap();
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
    .unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    connection
}

struct TestClock;

impl FixtureAuthorityClock for TestClock {
    fn now_ms(&self) -> Result<i64, ()> {
        Ok(NOW_MS)
    }
}

#[test]
fn provisioned_nrc1_seed_and_revoked_writer_key_survive_restart() {
    let test = TestDirectory::new("fixture-real-record-crypto");
    let descriptor = provision_sanitized_fixture_authority(&test.database).unwrap();
    let authority = SqliteDirectSyncAuthority::open_sanitized_fixture(
        &test.database,
        &descriptor.library_id,
        Arc::new(TestClock),
    )
    .unwrap();
    let first = authority.bootstrap().unwrap();
    assert!(!first.records.is_empty());
    assert!(first.records.iter().all(|record| {
        record.mutation.ciphertext.starts_with(b"NRC1")
            && !record
                .mutation
                .ciphertext
                .windows(b"contractVersion".len())
                .any(|window| window == b"contractVersion")
            && verify_generated_fixture_seed_mutation(&record.mutation)
    }));

    let connection = open(&test.database);
    let (seed_writer_id, state, stored_key): (String, String, Vec<u8>) = connection
        .query_row(
            "SELECT device_id, enrollment_state, public_signing_key
             FROM portable_devices
             WHERE library_id = ?1 AND device_kind = 'fixture_seed'",
            [&descriptor.library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "revoked");
    assert_eq!(stored_key.len(), 65);
    assert_eq!(stored_key[0], 0x04);
    drop(connection);
    assert_eq!(
        DirectSyncEnrollment::historical_writer_signing_public_key(
            &authority,
            &seed_writer_id,
            &descriptor.library_id,
            Environment::Development,
            descriptor.authority_generation,
        )
        .unwrap(),
        stored_key
    );

    drop(authority);
    assert_eq!(
        provision_sanitized_fixture_authority(&test.database).unwrap(),
        descriptor
    );
    let reopened = SqliteDirectSyncAuthority::open_sanitized_fixture(
        &test.database,
        &descriptor.library_id,
        Arc::new(TestClock),
    )
    .unwrap();
    assert_eq!(reopened.bootstrap().unwrap(), first);
}

#[test]
fn seed_ciphertext_signature_and_writer_directory_tampering_fail_closed() {
    let test = TestDirectory::new("fixture-real-record-tamper");
    let descriptor = provision_sanitized_fixture_authority(&test.database).unwrap();
    let authority = SqliteDirectSyncAuthority::open_sanitized_fixture(
        &test.database,
        &descriptor.library_id,
        Arc::new(TestClock),
    )
    .unwrap();
    let snapshot = authority.bootstrap().unwrap();
    let mut tampered_ciphertext = snapshot.records[0].mutation.clone();
    let final_byte = tampered_ciphertext.ciphertext.last_mut().unwrap();
    *final_byte ^= 0x01;
    assert!(!verify_generated_fixture_seed_mutation(
        &tampered_ciphertext
    ));
    let mut tampered_outer_signature = snapshot.records[0].mutation.clone();
    tampered_outer_signature.signature[0] ^= 0x01;
    assert!(!verify_generated_fixture_seed_mutation(
        &tampered_outer_signature
    ));
    drop(authority);

    let connection = open(&test.database);
    let mut replacement_key = vec![0x7a_u8; 65];
    replacement_key[0] = 0x04;
    connection
        .execute(
            "UPDATE portable_devices SET public_signing_key = ?1
             WHERE library_id = ?2 AND device_kind = 'fixture_seed'",
            rusqlite::params![replacement_key, descriptor.library_id],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        provision_sanitized_fixture_authority(&test.database),
        Err(FixtureAuthorityError::Integrity(_))
    ));
}
