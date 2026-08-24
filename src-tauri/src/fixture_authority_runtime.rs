//! Isolated Mac authority runtime for generated, sanitized development data.
//!
//! The module has no listener, Tauri command, personal-data constructor, or
//! caller-supplied fixture content. Provisioning builds a complete database at
//! a sibling staging path, verifies its portable/direct-sync invariants, and
//! publishes it without replacing an existing file. Pairing, sync, and
//! revocation share one operation gate once the verified fixture is opened.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::json;

#[cfg(feature = "sanitized-development-fixtures")]
use noted_apple_security::{
    decode_record_ciphertext_v1, encode_record_ciphertext_v1, BootstrapCapabilityV1,
    BootstrapMetadataV1 as NativeBootstrapMetadataV1, RecordCryptoContextV1,
    RecordCryptoOperationV1, RecordKindV1, SanitizedFixtureRecordCrypto, RECORD_CIPHER_SUITE,
    RECORD_CRYPTO_CONTEXT_VERSION,
};
#[cfg(feature = "sanitized-development-fixtures")]
use zeroize::Zeroizing;

use crate::{
    db::{self, SaveInput},
    direct_authority_store::{DirectAuthorityStore, InvitationRegistration},
    direct_pairing::{
        AuthorityBindings, AuthorityClock, ClientHelloResult, CoordinatorError,
        DirectPairingCoordinator, OwnerConfirmationResult,
    },
    direct_pairing_delivery::BootstrapPollResponse,
    direct_sync::{
        DirectRequest, DirectResponse, DirectSyncConfig, DirectSyncCrypto, DirectSyncError,
        DirectSyncLimits, DirectSyncService, DIRECT_SYNC_CONTENT_TYPE,
        MAX_DIRECT_TRANSACTION_BYTES,
        MAX_DIRECT_TRANSACTION_MEMBERS,
    },
    direct_sync_transport::{
        DirectSyncRequestHandler, FixtureAuthorityRequestHandler, PairingEndpoint,
        PairingTransportRequest, PairingTransportResponse,
    },
    durable_direct_sync::{FixtureAuthorityClock, SqliteDirectSyncAuthority},
    pairing_protocol::{
        Environment, Invitation, KindCapability, LibraryDataClass, PairingCrypto, PairingPolicy,
        RecordKind, TransportEvidence,
    },
    portable::{canonical_json, canonical_sha256, new_uuid_v7, ContextRecordV1},
    sync_protocol::{
        negotiate_capabilities, AcceptedHead, BootstrapRecord, BootstrapSnapshot, HeadAdvance,
        MutationDraft, MutationEnvelope, MutationOperation, ProtocolCapabilities,
        ReceiptDisposition, RecordKindCapability, SignedTransaction, TransactionHeader,
        TransactionReceipt, BOOTSTRAP_SNAPSHOT_VERSION, SYNC_PROTOCOL_VERSION,
    },
};

const FIXTURE_CONTRACT: &str = "noted.sanitized-fixture-authority.v1";
const FIXTURE_CLASS: &str = "sanitized_generated_development";
const FIXTURE_CONTRACT_VERSION: i64 = 1;
const FIXTURE_CREATED_AT: &str = "2026-08-17T12:00:00.000Z";
const FIXTURE_CREATED_AT_MS: i64 = 1_786_968_000_000;
const FIXTURE_EVENT_DATE: &str = "2026-08-17";
#[cfg(not(feature = "sanitized-development-fixtures"))]
const LEGACY_FIXTURE_CIPHERTEXT_PREFIX: &[u8] = b"fixture-json:";
#[cfg(feature = "sanitized-development-fixtures")]
const FIXTURE_CRYPTO_RECEIPT_ID: &str = "00000000-0000-7000-8000-0000000000f1";
#[cfg(feature = "sanitized-development-fixtures")]
const FIXTURE_CRYPTO_SCOPE_ID: &str = "00000000-0000-7000-8000-0000000000f2";
/// Publicly known development-fixture material. These values protect protocol
/// integrity and exercise the real wire format; they are intentionally not
/// credentials and are never used for personal or production data.
#[cfg(feature = "sanitized-development-fixtures")]
const FIXTURE_LIBRARY_KEY: [u8; 32] = [0x31; 32];
#[cfg(feature = "sanitized-development-fixtures")]
const FIXTURE_SEED_SIGNING_KEY: [u8; 32] = [0x51; 32];
const FIXTURE_CATEGORY_NAME: &str = "Sanitized Fixture";
const FIXTURE_FOLDER_NAME: &str = "Phone Sync Fixture";
const FIXTURE_NOTE_TITLE: &str = "Generated phone sync fixture";
const FIXTURE_NOTE_BODY: &str =
    "Generated development-only content for validating the iPhone Notes sync slice.";

const MARKER_SCHEMA: &str = r#"
CREATE TABLE fixture_authority_runtime_v1 (
  singleton                INTEGER PRIMARY KEY CHECK(singleton = 1),
  fixture_contract         TEXT NOT NULL,
  fixture_class            TEXT NOT NULL,
  contract_version         INTEGER NOT NULL CHECK(contract_version = 1),
  library_id               TEXT NOT NULL REFERENCES libraries(library_id) ON DELETE RESTRICT,
  authority_device_id      TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  default_scope_id         TEXT NOT NULL REFERENCES library_scopes(scope_id) ON DELETE RESTRICT,
  authority_generation     INTEGER NOT NULL CHECK(authority_generation > 0),
  purge_generation         INTEGER NOT NULL CHECK(purge_generation >= 0),
  key_epoch                INTEGER NOT NULL CHECK(key_epoch > 0),
  capabilities_digest      TEXT NOT NULL CHECK(length(capabilities_digest) = 64),
  descriptor_digest        TEXT NOT NULL CHECK(length(descriptor_digest) = 64),
  created_at_ms            INTEGER NOT NULL CHECK(created_at_ms >= 0)
);
CREATE TRIGGER fixture_authority_runtime_v1_no_update
BEFORE UPDATE ON fixture_authority_runtime_v1 BEGIN
  SELECT RAISE(ABORT, 'fixture authority marker is immutable');
END;
CREATE TRIGGER fixture_authority_runtime_v1_no_delete
BEFORE DELETE ON fixture_authority_runtime_v1 BEGIN
  SELECT RAISE(ABORT, 'fixture authority marker is immutable');
END;
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedFixtureAuthorityDescriptor {
    pub database_path: PathBuf,
    pub library_id: String,
    pub authority_device_id: String,
    pub default_scope_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub capabilities: ProtocolCapabilities,
}

#[derive(Debug)]
pub enum FixtureAuthorityError {
    InvalidTarget(&'static str),
    ExistingDatabaseNotFixture,
    TargetAppeared,
    Integrity(String),
    Database(String),
    Io(String),
    Pairing(CoordinatorError),
    Sync(DirectSyncError),
    StateUnavailable(&'static str),
}

impl fmt::Display for FixtureAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(reason) => write!(formatter, "invalid fixture target: {reason}"),
            Self::ExistingDatabaseNotFixture => formatter.write_str(
                "refusing to initialize an existing database without the immutable fixture marker",
            ),
            Self::TargetAppeared => formatter.write_str(
                "fixture target appeared while provisioning; no existing file was replaced",
            ),
            Self::Integrity(reason) => {
                write!(formatter, "fixture authority integrity error: {reason}")
            }
            Self::Database(reason) => {
                write!(formatter, "fixture authority database error: {reason}")
            }
            Self::Io(reason) => write!(formatter, "fixture authority filesystem error: {reason}"),
            Self::Pairing(error) => write!(formatter, "{error}"),
            Self::Sync(error) => write!(formatter, "{error}"),
            Self::StateUnavailable(reason) => {
                write!(formatter, "fixture authority state unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for FixtureAuthorityError {}

impl From<rusqlite::Error> for FixtureAuthorityError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<std::io::Error> for FixtureAuthorityError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<CoordinatorError> for FixtureAuthorityError {
    fn from(value: CoordinatorError) -> Self {
        Self::Pairing(value)
    }
}

impl From<DirectSyncError> for FixtureAuthorityError {
    fn from(value: DirectSyncError) -> Self {
        Self::Sync(value)
    }
}

/// Pairing and direct sync over one verified fixture database and one shared
/// operation gate. The generic crypto providers are deliberately mandatory;
/// this runtime never installs fallback cryptography.
pub struct SanitizedFixtureAuthorityRuntime<PC, PT, SC>
where
    PC: PairingCrypto,
    PT: AuthorityClock,
    SC: DirectSyncCrypto,
{
    operation_gate: Mutex<()>,
    descriptor: SanitizedFixtureAuthorityDescriptor,
    pairing: DirectPairingCoordinator<PC, PT>,
    sync: DirectSyncService<SqliteDirectSyncAuthority, SqliteDirectSyncAuthority, SC>,
}

impl<PC, PT, SC> SanitizedFixtureAuthorityRuntime<PC, PT, SC>
where
    PC: PairingCrypto,
    PT: AuthorityClock,
    SC: DirectSyncCrypto,
{
    pub fn open(
        database_path: impl AsRef<Path>,
        pairing_crypto: PC,
        pairing_clock: PT,
        sync_crypto: SC,
        sync_clock: Arc<dyn FixtureAuthorityClock>,
        bindings: AuthorityBindings,
    ) -> Result<Self, FixtureAuthorityError> {
        require_fixture_crypto_gate()?;
        let database_path = database_path.as_ref();
        validate_target(database_path)?;
        if !database_path.exists() {
            return Err(FixtureAuthorityError::InvalidTarget(
                "the provisioned database does not exist",
            ));
        }
        let descriptor = verify_published_fixture(database_path)?;
        let pairing_connection = open_read_write(database_path)?;
        #[cfg(feature = "sanitized-development-fixtures")]
        let record_crypto = {
            let seed_writer_id = fixture_seed_writer_id(&pairing_connection, &descriptor)?;
            Arc::new(fixture_record_crypto(
                &descriptor.library_id,
                &seed_writer_id,
                &descriptor.default_scope_id,
                descriptor.authority_generation,
                descriptor.purge_generation,
                descriptor.key_epoch,
            )?)
        };
        let pairing = DirectPairingCoordinator::new_fixture_only(
            pairing_connection,
            pairing_crypto,
            pairing_clock,
            pairing_policy(&descriptor),
            bindings.clone(),
        )?;
        #[cfg(feature = "sanitized-development-fixtures")]
        let authority = SqliteDirectSyncAuthority::open_sanitized_fixture_with_record_crypto(
            database_path,
            &descriptor.library_id,
            sync_clock,
            record_crypto,
        )
        .map_err(|error| FixtureAuthorityError::Integrity(format!("{error:?}")))?;
        #[cfg(not(feature = "sanitized-development-fixtures"))]
        let authority = SqliteDirectSyncAuthority::open_sanitized_fixture(
            database_path,
            &descriptor.library_id,
            sync_clock,
        )
        .map_err(|error| FixtureAuthorityError::Integrity(format!("{error:?}")))?;
        let sync = DirectSyncService::new(
            authority.clone(),
            authority,
            sync_crypto,
            DirectSyncConfig {
                library_id: descriptor.library_id.clone(),
                authority_generation: descriptor.authority_generation,
                environment: descriptor.environment,
                library_data_class: descriptor.library_data_class,
                server_spki_sha256: bindings.tls_spki_sha256.to_vec(),
                limits: DirectSyncLimits::default(),
            },
        )?;
        Ok(Self {
            operation_gate: Mutex::new(()),
            descriptor,
            pairing,
            sync,
        })
    }

    pub fn descriptor(&self) -> &SanitizedFixtureAuthorityDescriptor {
        &self.descriptor
    }

    pub fn register_invitation(
        &self,
        invitation: &Invitation,
    ) -> Result<InvitationRegistration, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.pairing
            .register_invitation(invitation)
            .map_err(Into::into)
    }

    pub fn process_client_hello(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
    ) -> Result<ClientHelloResult, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.pairing
            .process_client_hello(bytes, content_encoding, transport)
            .map_err(Into::into)
    }

    pub fn confirm_owner(
        &self,
        receipt_id: &str,
        displayed_verification_code: &str,
        displayed_scopes: &BTreeSet<RecordKind>,
        approved: bool,
    ) -> Result<OwnerConfirmationResult, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.pairing
            .confirm_owner(
                receipt_id,
                displayed_verification_code,
                displayed_scopes,
                approved,
            )
            .map_err(Into::into)
    }

    pub fn process_client_finish(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
    ) -> Result<Vec<u8>, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.pairing
            .process_client_finish(bytes, content_encoding, transport)
            .map_err(Into::into)
    }

    pub fn process_bootstrap_poll(
        &self,
        bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<Vec<u8>, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.pairing
            .process_bootstrap_poll(bytes, transport)
            .map_err(Into::into)
    }

    pub fn handle_sync(
        &self,
        request: DirectRequest,
    ) -> Result<DirectResponse, FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        Ok(self.sync.handle(request))
    }

    pub fn revoke_device(&self, device_id: &str, now_ms: i64) -> Result<(), FixtureAuthorityError> {
        let _operation = self.lock_operation()?;
        self.sync
            .revoke_device(device_id, now_ms)
            .map_err(Into::into)
    }

    fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, FixtureAuthorityError> {
        self.operation_gate
            .lock()
            .map_err(|_| FixtureAuthorityError::StateUnavailable("operation gate is poisoned"))
    }
}

impl<PC, PT, SC> DirectSyncRequestHandler for SanitizedFixtureAuthorityRuntime<PC, PT, SC>
where
    PC: PairingCrypto,
    PT: AuthorityClock,
    SC: DirectSyncCrypto,
{
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.handle_sync(request).unwrap_or_else(|_| DirectResponse {
            status: 503,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body: br#"{"error":{"code":"state_unavailable"}}"#.to_vec(),
        })
    }
}

impl<PC, PT, SC> FixtureAuthorityRequestHandler for SanitizedFixtureAuthorityRuntime<PC, PT, SC>
where
    PC: PairingCrypto,
    PT: AuthorityClock,
    SC: DirectSyncCrypto,
{
    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse {
        let result = match request.endpoint {
            PairingEndpoint::ClientHello => self
                .process_client_hello(&request.body, None, &request.transport)
                .map(|result| (200, result.exact_response_bytes)),
            PairingEndpoint::Bootstrap => self
                .process_bootstrap_poll(&request.body, &request.transport)
                .and_then(|bytes| {
                    let response: BootstrapPollResponse = serde_json::from_slice(&bytes)
                        .map_err(|_| FixtureAuthorityError::StateUnavailable(
                            "committed bootstrap response could not be decoded",
                        ))?;
                    Ok((response.http_status(), bytes))
                }),
            PairingEndpoint::ClientFinish => self
                .process_client_finish(&request.body, None, &request.transport)
                .map(|bytes| (200, bytes)),
        };
        match result {
            Ok((status, body)) => PairingTransportResponse { status, body },
            Err(error) => pairing_wire_error(error),
        }
    }
}

fn pairing_wire_error(error: FixtureAuthorityError) -> PairingTransportResponse {
    let (status, code) = match error {
        FixtureAuthorityError::StateUnavailable(_)
        | FixtureAuthorityError::Database(_)
        | FixtureAuthorityError::Io(_)
        | FixtureAuthorityError::Integrity(_)
        | FixtureAuthorityError::Sync(_) => (503, "state_unavailable"),
        FixtureAuthorityError::InvalidTarget(_)
        | FixtureAuthorityError::ExistingDatabaseNotFixture
        | FixtureAuthorityError::TargetAppeared
        | FixtureAuthorityError::Pairing(_) => (400, "pairing_rejected"),
    };
    PairingTransportResponse {
        status,
        body: format!(r#"{{"error":{{"code":"{code}"}}}}"#).into_bytes(),
    }
}

/// Create or verify the sole supported generated fixture authority database.
/// Existing unmarked files are never opened through the mutating desktop
/// migration path.
pub fn provision_sanitized_fixture_authority(
    database_path: impl AsRef<Path>,
) -> Result<SanitizedFixtureAuthorityDescriptor, FixtureAuthorityError> {
    require_fixture_crypto_gate()?;
    let database_path = database_path.as_ref();
    let parent = validate_target(database_path)?;
    if database_path.exists() {
        if !has_fixture_marker(database_path)? {
            return Err(FixtureAuthorityError::ExistingDatabaseNotFixture);
        }
        return verify_published_fixture(database_path);
    }

    let staging_path = unique_staging_path(&parent, database_path);
    let mut staging = StagingFile::new(staging_path.clone());
    let staging_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&staging_path)?;
    set_private_permissions(&staging_path)?;
    staging_file.sync_all()?;
    drop(staging_file);
    let mut connection = db::init(&staging_path)
        .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    seed_generated_domain_records(&mut connection)?;
    let descriptor = initialize_fixture_authority(&mut connection, &staging_path)?;
    create_marker(&mut connection, &descriptor)?;
    crate::sync_journal::verify_portable_schema(&connection)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    DirectAuthorityStore::verify_schema(&connection)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    drop(connection);

    let clock: Arc<dyn FixtureAuthorityClock> = Arc::new(ProvisioningClock);
    let authority = SqliteDirectSyncAuthority::open_sanitized_fixture(
        &staging_path,
        &descriptor.library_id,
        clock,
    )
    .map_err(|error| FixtureAuthorityError::Integrity(format!("{error:?}")))?;
    use crate::direct_sync::DirectSyncAuthority;
    authority.bootstrap().map_err(|error| {
        FixtureAuthorityError::Integrity(format!("bootstrap failed: {error:?}"))
    })?;

    let connection = open_read_write(&staging_path)?;
    verify_fixture_connection(&connection, &staging_path)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection
        .close()
        .map_err(|(_, error)| FixtureAuthorityError::Database(error.to_string()))?;

    set_private_permissions(&staging_path)?;
    OpenOptions::new()
        .read(true)
        .open(&staging_path)?
        .sync_all()?;
    match fs::hard_link(&staging_path, database_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(FixtureAuthorityError::TargetAppeared)
        }
        Err(error) => return Err(error.into()),
    }
    File::open(&parent)?.sync_all()?;
    fs::remove_file(&staging_path)?;
    staging.disarm();
    File::open(&parent)?.sync_all()?;

    verify_published_fixture(database_path)
}

fn validate_target(database_path: &Path) -> Result<PathBuf, FixtureAuthorityError> {
    if !database_path.is_absolute() {
        return Err(FixtureAuthorityError::InvalidTarget(
            "an absolute database path is required",
        ));
    }
    if database_path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(FixtureAuthorityError::InvalidTarget(
            "dot, parent, and platform-prefix components are not accepted",
        ));
    }
    let file_name = database_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(FixtureAuthorityError::InvalidTarget(
            "a normal UTF-8 filename is required",
        ))?;
    if file_name.is_empty() || file_name == ":memory:" {
        return Err(FixtureAuthorityError::InvalidTarget(
            "an on-disk filename is required",
        ));
    }
    let parent = database_path
        .parent()
        .ok_or(FixtureAuthorityError::InvalidTarget(
            "the database must have a parent directory",
        ))?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(FixtureAuthorityError::InvalidTarget(
            "the parent must be a real directory",
        ));
    }
    if fs::canonicalize(parent)? != parent {
        return Err(FixtureAuthorityError::InvalidTarget(
            "the parent path must not traverse aliases or symlinks",
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(database_path) {
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(FixtureAuthorityError::InvalidTarget(
                "an existing target must be a regular file",
            ));
        }
    }
    Ok(parent.to_path_buf())
}

fn has_fixture_marker(database_path: &Path) -> Result<bool, FixtureAuthorityError> {
    let connection = match open_read_only(database_path) {
        Ok(connection) => connection,
        Err(_) => return Ok(false),
    };
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema
               WHERE type = 'table' AND name = 'fixture_authority_runtime_v1'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FixtureAuthorityError::ExistingDatabaseNotFixture)
}

fn open_read_only(path: &Path) -> Result<Connection, FixtureAuthorityError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

fn open_read_write(path: &Path) -> Result<Connection, FixtureAuthorityError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

fn unique_staging_path(parent: &Path, database_path: &Path) -> PathBuf {
    let target_name = database_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("fixture.sqlite");
    parent.join(format!(
        ".{target_name}.staging-{}-{}",
        std::process::id(),
        new_uuid_v7()
    ))
}

struct StagingFile {
    path: PathBuf,
    armed: bool,
}

impl StagingFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = fs::remove_file(&self.path);
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = self.path.as_os_str().to_owned();
            sidecar.push(suffix);
            let _ = fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

fn set_private_permissions(path: &Path) -> Result<(), FixtureAuthorityError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn seed_generated_domain_records(connection: &mut Connection) -> Result<(), FixtureAuthorityError> {
    let personal_folder_id: i64 = connection
        .query_row(
            "SELECT id FROM note_folders
             WHERE parent_id IS NULL AND kind = 'space' AND name = 'Personal' COLLATE NOCASE",
            [],
            |row| row.get(0),
        )
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    db::create_category(
        connection,
        FIXTURE_CATEGORY_NAME,
        "Generated category used only by the direct-sync fixture authority.",
        FIXTURE_CREATED_AT,
    )
    .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    let folder_id = db::create_note_folder(
        connection,
        Some(personal_folder_id),
        FIXTURE_FOLDER_NAME,
        "folder",
        "",
        FIXTURE_CREATED_AT,
    )
    .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    let note_id = db::save_note(
        connection,
        SaveInput {
            raw_text: FIXTURE_NOTE_BODY.to_owned(),
            source: "sanitized_fixture".to_owned(),
            image_path: None,
            event_date: FIXTURE_EVENT_DATE.to_owned(),
            entries: Vec::new(),
        },
        FIXTURE_CREATED_AT,
    )
    .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    db::update_note(
        connection,
        note_id,
        FIXTURE_NOTE_TITLE,
        FIXTURE_NOTE_BODY,
        FIXTURE_CREATED_AT,
    )
    .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    db::file_note(connection, note_id, Some(folder_id), FIXTURE_CREATED_AT)
        .map_err(|error| FixtureAuthorityError::Database(error.to_string()))?;
    Ok(())
}

fn initialize_fixture_authority(
    connection: &mut Connection,
    database_path: &Path,
) -> Result<SanitizedFixtureAuthorityDescriptor, FixtureAuthorityError> {
    let (library_id, authority_device_id, authority_generation, purge_generation): (
        String,
        String,
        i64,
        i64,
    ) = connection.query_row(
        "SELECT l.library_id, l.owner_device_id, l.authority_generation, l.purge_generation
         FROM libraries l",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let default_scope_id: String = connection.query_row(
        "SELECT scope_id FROM library_scopes
         WHERE library_id = ?1 AND scope_class = 'unknown'",
        [&library_id],
        |row| row.get(0),
    )?;
    let changed = connection.execute(
        "UPDATE libraries SET current_key_epoch = 1
         WHERE library_id = ?1 AND current_key_epoch = 0",
        [&library_id],
    )?;
    if changed != 1 {
        return Err(FixtureAuthorityError::Integrity(
            "fresh fixture library did not begin at key epoch zero".to_owned(),
        ));
    }
    let descriptor = SanitizedFixtureAuthorityDescriptor {
        database_path: database_path.to_path_buf(),
        library_id,
        authority_device_id,
        default_scope_id,
        authority_generation: u64::try_from(authority_generation).map_err(|_| {
            FixtureAuthorityError::Integrity("negative authority generation".to_owned())
        })?,
        purge_generation: u64::try_from(purge_generation).map_err(|_| {
            FixtureAuthorityError::Integrity("negative purge generation".to_owned())
        })?,
        key_epoch: 1,
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        capabilities: exact_notes_capabilities(),
    };
    let capabilities_json = canonical_json(
        &serde_json::to_value(&descriptor.capabilities)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
    );
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    DirectAuthorityStore::initialize_fixture_profile(
        &transaction,
        &descriptor.library_id,
        descriptor.authority_generation,
        &capabilities_json,
        FIXTURE_CREATED_AT_MS,
    )
    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    // Accepted direct transactions are required to originate from replicas.
    // A generated one-shot writer gives the initial portable heads a valid
    // authority-log provenance without ever impersonating the Mac authority;
    // it is revoked in this same unpublished staging transaction.
    let seed_writer_id = new_uuid_v7();
    #[cfg(feature = "sanitized-development-fixtures")]
    let seed_record_crypto = fixture_record_crypto(
        &descriptor.library_id,
        &seed_writer_id,
        &descriptor.default_scope_id,
        descriptor.authority_generation,
        descriptor.purge_generation,
        descriptor.key_epoch,
    )?;
    transaction.execute(
        "INSERT INTO portable_devices
         (device_id, library_id, device_kind, display_name, role,
          enrollment_state, capabilities_json, public_signing_key,
          last_transaction_counter,
          created_at, enrolled_at)
         VALUES (?1, ?2, 'fixture_seed', 'Generated fixture seed writer',
                 'replica', 'active', ?3, ?4, 0, ?5, ?5)",
        params![
            seed_writer_id,
            descriptor.library_id,
            capabilities_json,
            {
                #[cfg(feature = "sanitized-development-fixtures")]
                {
                    seed_record_crypto.signing_public_key().to_vec()
                }
                #[cfg(not(feature = "sanitized-development-fixtures"))]
                {
                    let mut legacy_key = vec![0x41_u8; 65];
                    legacy_key[0] = 0x04;
                    legacy_key
                }
            },
            FIXTURE_CREATED_AT,
        ],
    )?;
    seed_direct_accepted_heads(
        &transaction,
        &descriptor,
        &seed_writer_id,
        #[cfg(feature = "sanitized-development-fixtures")]
        &seed_record_crypto,
    )?;
    let revoked = transaction.execute(
        "UPDATE portable_devices
         SET enrollment_state = 'revoked', revoked_at = ?2
         WHERE device_id = ?1 AND role = 'replica' AND enrollment_state = 'active'",
        params![seed_writer_id, FIXTURE_CREATED_AT],
    )?;
    if revoked != 1 {
        return Err(FixtureAuthorityError::Integrity(
            "fixture seed writer was not revoked".to_owned(),
        ));
    }
    transaction.commit()?;
    Ok(descriptor)
}

fn seed_direct_accepted_heads(
    transaction: &rusqlite::Transaction<'_>,
    descriptor: &SanitizedFixtureAuthorityDescriptor,
    seed_writer_id: &str,
    #[cfg(feature = "sanitized-development-fixtures")]
    seed_record_crypto: &SanitizedFixtureRecordCrypto,
) -> Result<(), FixtureAuthorityError> {
    let profile_cursor: i64 = transaction.query_row(
        "SELECT high_water_cursor FROM direct_authority_profiles WHERE library_id = ?1",
        [&descriptor.library_id],
        |row| row.get(0),
    )?;
    let direct_rows: i64 = transaction.query_row(
        "SELECT (SELECT COUNT(*) FROM direct_authority_transactions) +
                (SELECT COUNT(*) FROM direct_authority_mutations) +
                (SELECT COUNT(*) FROM direct_authority_changes)",
        [],
        |row| row.get(0),
    )?;
    if profile_cursor != 0 || direct_rows != 0 {
        return Err(FixtureAuthorityError::Integrity(
            "fresh fixture direct authority is not empty".to_owned(),
        ));
    }

    struct HeadRow {
        record_id: String,
        kind: String,
        schema_version: u32,
        revision: u64,
        version_id: String,
        record: ContextRecordV1,
        base_version_id: Option<String>,
    }

    let mut statement = transaction.prepare(
        "SELECT p.record_id, p.kind, p.record_schema_version,
                h.accepted_revision, h.accepted_version_id, v.snapshot_json
         FROM portable_records p
         JOIN record_heads h ON h.record_id = p.record_id
         JOIN record_versions v
           ON v.record_id = h.record_id AND v.version_id = h.accepted_version_id
         WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')
         ORDER BY p.record_id",
    )?;
    let rows = statement.query_map([&descriptor.library_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut heads = Vec::new();
    for row in rows {
        let (record_id, kind, schema_version, revision, version_id, snapshot_json) = row?;
        let revision = u64::try_from(revision)
            .map_err(|_| FixtureAuthorityError::Integrity("negative record revision".to_owned()))?;
        let schema_version = u32::try_from(schema_version).map_err(|_| {
            FixtureAuthorityError::Integrity("invalid record schema version".to_owned())
        })?;
        let record: ContextRecordV1 = serde_json::from_str(&snapshot_json)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
        if record.library_id != descriptor.library_id
            || record.record_id != record_id
            || record.kind != kind
            || record.record_schema_version != schema_version
            || record.revision != revision
            || record.version_id != version_id
        {
            return Err(FixtureAuthorityError::Integrity(
                "portable accepted head does not match its snapshot".to_owned(),
            ));
        }
        let base_version_id = if revision == 1 {
            None
        } else {
            let prior_revision = to_i64(revision - 1, "prior record revision")?;
            let prior: String = transaction
                .query_row(
                    "SELECT version_id FROM record_versions
                     WHERE record_id = ?1 AND revision = ?2",
                    params![record_id, prior_revision],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    FixtureAuthorityError::Integrity(
                        "portable head is missing its prior version".to_owned(),
                    )
                })?;
            Some(prior)
        };
        heads.push(HeadRow {
            record_id,
            kind,
            schema_version,
            revision,
            version_id,
            record,
            base_version_id,
        });
    }
    drop(statement);
    if heads.is_empty() || heads.len() > MAX_DIRECT_TRANSACTION_MEMBERS as usize {
        return Err(FixtureAuthorityError::Integrity(
            "fixture head count exceeds the direct transaction contract".to_owned(),
        ));
    }

    let transaction_id = new_uuid_v7();
    let last_counter: i64 = transaction.query_row(
        "SELECT last_transaction_counter FROM portable_devices WHERE device_id = ?1",
        [seed_writer_id],
        |row| row.get(0),
    )?;
    let counter = u64::try_from(last_counter)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            FixtureAuthorityError::Integrity("authority counter is exhausted".to_owned())
        })?;
    let drafts = heads
        .iter()
        .map(|head| {
            let mut draft = MutationDraft {
                mutation_id: new_uuid_v7(),
                operation: if head.revision == 1 && head.base_version_id.is_none() {
                    MutationOperation::Create
                } else {
                    MutationOperation::Update
                },
                record_id: head.record_id.clone(),
                record_kind: head.kind.clone(),
                record_schema_version: head.schema_version,
                base_head_revision: head.revision - 1,
                base_head_version_id: head.base_version_id.clone(),
                proposed_revision: head.revision,
                version_id: head.version_id.clone(),
                ciphertext: Vec::new(),
            };
            #[cfg(feature = "sanitized-development-fixtures")]
            {
                let context = fixture_record_context_from_draft(descriptor, &draft)?;
                let plaintext = canonical_json(
                    &serde_json::to_value(&head.record)
                        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
                )
                .into_bytes();
                let sealed = seed_record_crypto
                    .seal_record(&context, &plaintext)
                    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
                draft.ciphertext = encode_record_ciphertext_v1(&sealed, &context)
                    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
            }
            #[cfg(not(feature = "sanitized-development-fixtures"))]
            {
                draft
                    .ciphertext
                    .extend_from_slice(LEGACY_FIXTURE_CIPHERTEXT_PREFIX);
                draft.ciphertext.extend_from_slice(
                    canonical_json(
                        &serde_json::to_value(&head.record)
                            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
                    )
                    .as_bytes(),
                );
            }
            Ok(draft)
        })
        .collect::<Result<Vec<_>, FixtureAuthorityError>>()?;
    let prepared = SignedTransaction::prepare(
        TransactionHeader {
            protocol_version: SYNC_PROTOCOL_VERSION,
            library_id: descriptor.library_id.clone(),
            transaction_id: transaction_id.clone(),
            device_id: seed_writer_id.to_owned(),
            device_transaction_counter: counter,
            authority_generation: descriptor.authority_generation,
            purge_generation: descriptor.purge_generation,
            key_epoch: descriptor.key_epoch,
        },
        drafts,
        to_u64(
            FIXTURE_CREATED_AT_MS + 300_000,
            "fixture transaction expiry",
        )?,
    )
    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let signatures = prepared
        .signing_inputs()
        .into_iter()
        .map(|input| {
            #[cfg(feature = "sanitized-development-fixtures")]
            {
                seed_record_crypto
                    .sign_p256_p1363(&input.canonical_bytes)
                    .map(|signature| signature.to_vec())
                    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))
            }
            #[cfg(not(feature = "sanitized-development-fixtures"))]
            {
                Ok(legacy_fixture_seed_signature(&input.canonical_bytes))
            }
        })
        .collect::<Result<Vec<_>, FixtureAuthorityError>>()?;
    let signed = prepared
        .attach_signatures(signatures)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    if signed.manifest.byte_total > MAX_DIRECT_TRANSACTION_BYTES {
        return Err(FixtureAuthorityError::Integrity(
            "fixture ciphertext exceeds the direct transaction limit".to_owned(),
        ));
    }
    let advances = signed
        .members
        .iter()
        .map(|member| HeadAdvance {
            record_id: member.record_id.clone(),
            record_kind: member.record_kind.clone(),
            record_schema_version: member.record_schema_version,
            base_revision: member.base_head_revision,
            base_version_id: member.base_head_version_id.clone(),
            revision: member.proposed_revision,
            version_id: member.version_id.clone(),
            ciphertext_hash: member.ciphertext_hash.clone(),
        })
        .collect();
    let receipt = TransactionReceipt {
        library_id: descriptor.library_id.clone(),
        transaction_id: transaction_id.clone(),
        transaction_digest: signed.signed_digest(),
        mutation_ids: signed
            .members
            .iter()
            .map(|member| member.mutation_id.clone())
            .collect(),
        device_id: seed_writer_id.to_owned(),
        device_transaction_counter: counter,
        authority_generation: descriptor.authority_generation,
        purge_generation: descriptor.purge_generation,
        high_water_cursor: 1,
        disposition: ReceiptDisposition::Accepted { advances },
    };
    let transaction_json = canonical_json(
        &serde_json::to_value(&signed)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
    );
    let receipt_json = canonical_json(
        &serde_json::to_value(&receipt)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
    );
    transaction.execute(
        "INSERT INTO direct_authority_transactions
         (transaction_id, library_id, device_id, authority_generation,
          device_transaction_counter, signed_digest, transaction_json,
          state, receipt_json, accepted_cursor, created_at_ms, expires_at_ms,
          terminal_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'accepted', ?8, 1, ?9, ?10, ?9)",
        params![
            transaction_id,
            descriptor.library_id,
            seed_writer_id,
            to_i64(descriptor.authority_generation, "authority generation")?,
            to_i64(counter, "device transaction counter")?,
            receipt.transaction_digest,
            transaction_json,
            receipt_json,
            FIXTURE_CREATED_AT_MS,
            FIXTURE_CREATED_AT_MS + 300_000,
        ],
    )?;
    for member in &signed.members {
        let envelope_json = canonical_json(
            &serde_json::to_value(member)
                .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
        );
        transaction.execute(
            "INSERT INTO direct_authority_mutations
             (mutation_id, transaction_id, member_index, signed_digest,
              record_id, version_id, envelope_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                member.mutation_id,
                signed.manifest.transaction_id,
                i64::from(member.transaction_member_index),
                member.signed_digest(),
                member.record_id,
                member.version_id,
                envelope_json,
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO direct_authority_changes
         (library_id, sequence, transaction_id, transaction_digest, created_at_ms)
         VALUES (?1, 1, ?2, ?3, ?4)",
        params![
            descriptor.library_id,
            signed.manifest.transaction_id,
            receipt.transaction_digest,
            FIXTURE_CREATED_AT_MS,
        ],
    )?;
    let profile_changed = transaction.execute(
        "UPDATE direct_authority_profiles
         SET high_water_cursor = 1, state_revision = state_revision + 1,
             updated_at_ms = ?2
         WHERE library_id = ?1 AND high_water_cursor = 0
           AND environment = 'development'
           AND library_data_class = 'sanitized_fixture'",
        params![descriptor.library_id, FIXTURE_CREATED_AT_MS],
    )?;
    let device_changed = transaction.execute(
        "UPDATE portable_devices SET last_transaction_counter = ?2
         WHERE device_id = ?1 AND last_transaction_counter = ?3
           AND role = 'replica' AND enrollment_state = 'active'",
        params![
            seed_writer_id,
            to_i64(counter, "device transaction counter")?,
            last_counter
        ],
    )?;
    if profile_changed != 1 || device_changed != 1 {
        return Err(FixtureAuthorityError::Integrity(
            "fixture seed transaction lost its authority state race".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(not(feature = "sanitized-development-fixtures"))]
fn legacy_fixture_seed_signature(signing_bytes: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(b"noted.sanitized-fixture-authority.v1/seed-signature\0");
    digest.update(signing_bytes);
    digest.finalize().to_vec()
}

/// Verify a generated seed mutation with the implementation selected at
/// compile time. The development-fixture feature uses the complete NRC1
/// container, its inner signature, the outer P-256 signature, AEAD, and the
/// canonical portable-record binding. The legacy recognizer exists only so
/// the historical non-feature test seam remains usable.
pub fn verify_generated_fixture_seed_mutation(
    mutation: &crate::sync_protocol::MutationEnvelope,
) -> bool {
    #[cfg(feature = "sanitized-development-fixtures")]
    {
        open_generated_fixture_seed_mutation(mutation, FIXTURE_CRYPTO_SCOPE_ID).is_ok()
    }
    #[cfg(all(not(feature = "sanitized-development-fixtures"), debug_assertions))]
    {
        mutation
            .ciphertext
            .starts_with(LEGACY_FIXTURE_CIPHERTEXT_PREFIX)
            && mutation.signature == legacy_fixture_seed_signature(&mutation.signing_bytes())
    }
    #[cfg(all(not(feature = "sanitized-development-fixtures"), not(debug_assertions)))]
    {
        let _ = mutation;
        false
    }
}

fn require_fixture_crypto_gate() -> Result<(), FixtureAuthorityError> {
    #[cfg(all(not(feature = "sanitized-development-fixtures"), not(debug_assertions)))]
    {
        return Err(FixtureAuthorityError::InvalidTarget(
            "sanitized fixture authority requires the explicit development-fixture crypto feature",
        ));
    }
    #[cfg(any(feature = "sanitized-development-fixtures", debug_assertions))]
    {
        Ok(())
    }
}

#[cfg(feature = "sanitized-development-fixtures")]
fn fixture_record_crypto(
    library_id: &str,
    device_id: &str,
    default_scope_id: &str,
    authority_generation: u64,
    purge_generation: u64,
    key_epoch: u64,
) -> Result<SanitizedFixtureRecordCrypto, FixtureAuthorityError> {
    let exact_capability = BootstrapCapabilityV1 {
        reader_version: 1,
        writer_version: Some(1),
    };
    let metadata = NativeBootstrapMetadataV1 {
        version: 1,
        protocol: "noted.direct-pairing.v1".to_owned(),
        suite: "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm".to_owned(),
        sync_protocol_version: SYNC_PROTOCOL_VERSION,
        environment: "development".to_owned(),
        library_data_class: "sanitized_fixture".to_owned(),
        receipt_id: FIXTURE_CRYPTO_RECEIPT_ID.to_owned(),
        library_id: library_id.to_owned(),
        device_id: device_id.to_owned(),
        authority_generation,
        purge_generation,
        key_epoch,
        default_scope_id: default_scope_id.to_owned(),
        default_scope_class: "unknown".to_owned(),
        granted_scopes: vec![
            "note".to_owned(),
            "category".to_owned(),
            "folder".to_owned(),
        ],
        capabilities: BTreeMap::from([
            ("note".to_owned(), exact_capability),
            ("category".to_owned(), exact_capability),
            ("folder".to_owned(), exact_capability),
        ]),
        record_cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
        durable_sync_spki_sha256: [0xa5; 32],
        transcript_digest: [0xb6; 32],
    };
    SanitizedFixtureRecordCrypto::new(
        metadata,
        Zeroizing::new(FIXTURE_LIBRARY_KEY),
        Zeroizing::new(FIXTURE_SEED_SIGNING_KEY),
    )
    .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))
}

#[cfg(feature = "sanitized-development-fixtures")]
fn fixture_record_context_from_draft(
    descriptor: &SanitizedFixtureAuthorityDescriptor,
    draft: &MutationDraft,
) -> Result<RecordCryptoContextV1, FixtureAuthorityError> {
    fixture_record_context(
        &descriptor.library_id,
        descriptor.authority_generation,
        descriptor.purge_generation,
        descriptor.key_epoch,
        draft.operation,
        &draft.record_id,
        &draft.record_kind,
        draft.record_schema_version,
        draft.base_head_revision,
        draft.base_head_version_id.clone(),
        draft.proposed_revision,
        &draft.version_id,
        &draft.mutation_id,
    )
}

#[cfg(feature = "sanitized-development-fixtures")]
fn fixture_record_context_from_envelope(
    mutation: &MutationEnvelope,
) -> Result<RecordCryptoContextV1, FixtureAuthorityError> {
    fixture_record_context(
        &mutation.library_id,
        mutation.authority_generation,
        mutation.purge_generation,
        mutation.key_epoch,
        mutation.operation,
        &mutation.record_id,
        &mutation.record_kind,
        mutation.record_schema_version,
        mutation.base_head_revision,
        mutation.base_head_version_id.clone(),
        mutation.proposed_revision,
        &mutation.version_id,
        &mutation.mutation_id,
    )
}

#[cfg(feature = "sanitized-development-fixtures")]
#[allow(clippy::too_many_arguments)]
fn fixture_record_context(
    library_id: &str,
    authority_generation: u64,
    purge_generation: u64,
    key_epoch: u64,
    operation: MutationOperation,
    record_id: &str,
    record_kind: &str,
    record_schema_version: u32,
    base_head_revision: u64,
    base_head_version_id: Option<String>,
    proposed_revision: u64,
    version_id: &str,
    mutation_id: &str,
) -> Result<RecordCryptoContextV1, FixtureAuthorityError> {
    let record_kind = match record_kind {
        "note" => RecordKindV1::Note,
        "category" => RecordKindV1::Category,
        "folder" => RecordKindV1::Folder,
        _ => {
            return Err(FixtureAuthorityError::Integrity(
                "fixture record kind is unsupported".to_owned(),
            ))
        }
    };
    let operation = match operation {
        MutationOperation::Create => RecordCryptoOperationV1::Create,
        MutationOperation::Update => RecordCryptoOperationV1::Update,
        MutationOperation::Delete => RecordCryptoOperationV1::Delete,
    };
    let context = RecordCryptoContextV1 {
        version: RECORD_CRYPTO_CONTEXT_VERSION,
        cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
        library_id: library_id.to_owned(),
        record_id: record_id.to_owned(),
        record_kind,
        schema_version: record_schema_version,
        base_revision: base_head_revision,
        base_version_id: base_head_version_id,
        proposed_revision,
        version_id: version_id.to_owned(),
        mutation_id: mutation_id.to_owned(),
        authority_generation,
        purge_generation,
        key_epoch,
        operation,
    };
    context
        .validate()
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    Ok(context)
}

#[cfg(feature = "sanitized-development-fixtures")]
fn open_generated_fixture_seed_mutation(
    mutation: &MutationEnvelope,
    default_scope_id: &str,
) -> Result<ContextRecordV1, FixtureAuthorityError> {
    let crypto = fixture_record_crypto(
        &mutation.library_id,
        &mutation.device_id,
        default_scope_id,
        mutation.authority_generation,
        mutation.purge_generation,
        mutation.key_epoch,
    )?;
    if mutation.signature.len() != 64
        || !SanitizedFixtureRecordCrypto::verify_p256_p1363(
            &crypto.signing_public_key(),
            &mutation.signing_bytes(),
            &mutation.signature,
        )
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?
    {
        return Err(FixtureAuthorityError::Integrity(
            "fixture seed outer signature is invalid".to_owned(),
        ));
    }
    let context = fixture_record_context_from_envelope(mutation)?;
    let sealed = decode_record_ciphertext_v1(&mutation.ciphertext, &context)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let opened = crypto
        .open_record(&context, &sealed, &crypto.signing_public_key())
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let record: ContextRecordV1 = serde_json::from_slice(&opened.plaintext)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    record
        .validate()
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let canonical = canonical_json(
        &serde_json::to_value(&record)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?,
    );
    if canonical.as_bytes() != opened.plaintext
        || record.library_id != mutation.library_id
        || record.record_id != mutation.record_id
        || record.kind != mutation.record_kind
        || record.record_schema_version != mutation.record_schema_version
        || record.revision != mutation.proposed_revision
        || record.version_id != mutation.version_id
    {
        return Err(FixtureAuthorityError::Integrity(
            "fixture seed plaintext binding is invalid".to_owned(),
        ));
    }
    Ok(record)
}

fn create_marker(
    connection: &mut Connection,
    descriptor: &SanitizedFixtureAuthorityDescriptor,
) -> Result<(), FixtureAuthorityError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(MARKER_SCHEMA)?;
    transaction.execute(
        "INSERT INTO fixture_authority_runtime_v1
         (singleton, fixture_contract, fixture_class, contract_version,
          library_id, authority_device_id, default_scope_id,
          authority_generation, purge_generation, key_epoch,
          capabilities_digest, descriptor_digest, created_at_ms)
         VALUES (1, ?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            FIXTURE_CONTRACT,
            FIXTURE_CLASS,
            descriptor.library_id,
            descriptor.authority_device_id,
            descriptor.default_scope_id,
            to_i64(descriptor.authority_generation, "authority generation")?,
            to_i64(descriptor.purge_generation, "purge generation")?,
            to_i64(descriptor.key_epoch, "key epoch")?,
            capabilities_digest(&descriptor.capabilities)?,
            descriptor_digest(descriptor)?,
            FIXTURE_CREATED_AT_MS,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn verify_published_fixture(
    database_path: &Path,
) -> Result<SanitizedFixtureAuthorityDescriptor, FixtureAuthorityError> {
    if !has_fixture_marker(database_path)? {
        return Err(FixtureAuthorityError::ExistingDatabaseNotFixture);
    }
    let connection = open_read_only(database_path)?;
    verify_fixture_connection(&connection, database_path)
}

fn verify_fixture_connection(
    connection: &Connection,
    database_path: &Path,
) -> Result<SanitizedFixtureAuthorityDescriptor, FixtureAuthorityError> {
    crate::sync_journal::verify_portable_schema(connection)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    DirectAuthorityStore::verify_schema(connection)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    for trigger in [
        "fixture_authority_runtime_v1_no_update",
        "fixture_authority_runtime_v1_no_delete",
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1)",
            [trigger],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(FixtureAuthorityError::Integrity(format!(
                "immutable marker trigger '{trigger}' is missing"
            )));
        }
    }
    let marker: (
        String,
        String,
        i64,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT fixture_contract, fixture_class, contract_version,
                    library_id, authority_device_id, default_scope_id,
                    authority_generation, purge_generation, key_epoch,
                    capabilities_digest, descriptor_digest
             FROM fixture_authority_runtime_v1 WHERE singleton = 1",
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
                ))
            },
        )
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let marker_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM fixture_authority_runtime_v1",
        [],
        |row| row.get(0),
    )?;
    if marker_count != 1
        || marker.0 != FIXTURE_CONTRACT
        || marker.1 != FIXTURE_CLASS
        || marker.2 != FIXTURE_CONTRACT_VERSION
    {
        return Err(FixtureAuthorityError::Integrity(
            "fixture marker identity is invalid".to_owned(),
        ));
    }
    let descriptor = SanitizedFixtureAuthorityDescriptor {
        database_path: database_path.to_path_buf(),
        library_id: marker.3,
        authority_device_id: marker.4,
        default_scope_id: marker.5,
        authority_generation: to_u64(marker.6, "authority generation")?,
        purge_generation: to_u64(marker.7, "purge generation")?,
        key_epoch: to_u64(marker.8, "key epoch")?,
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        capabilities: exact_notes_capabilities(),
    };
    if marker.9 != capabilities_digest(&descriptor.capabilities)?
        || marker.10 != descriptor_digest(&descriptor)?
    {
        return Err(FixtureAuthorityError::Integrity(
            "fixture marker digest does not match its descriptor".to_owned(),
        ));
    }

    let library: Option<(i64, i64, i64, String)> = connection
        .query_row(
            "SELECT authority_generation, purge_generation, current_key_epoch,
                    owner_device_id
             FROM libraries WHERE library_id = ?1",
            [&descriptor.library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let expected_library = Some((
        to_i64(descriptor.authority_generation, "authority generation")?,
        to_i64(descriptor.purge_generation, "purge generation")?,
        to_i64(descriptor.key_epoch, "key epoch")?,
        descriptor.authority_device_id.clone(),
    ));
    if library != expected_library
        || descriptor.authority_generation != 1
        || descriptor.purge_generation != 0
        || descriptor.key_epoch != 1
    {
        return Err(FixtureAuthorityError::Integrity(
            "portable library generations or owner binding diverged".to_owned(),
        ));
    }
    let scope_class: Option<String> = connection
        .query_row(
            "SELECT scope_class FROM library_scopes
             WHERE scope_id = ?1 AND library_id = ?2",
            params![descriptor.default_scope_id, descriptor.library_id],
            |row| row.get(0),
        )
        .optional()?;
    let scope_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM library_scopes WHERE library_id = ?1",
        [&descriptor.library_id],
        |row| row.get(0),
    )?;
    if scope_class.as_deref() != Some("unknown") || scope_count != 3 {
        return Err(FixtureAuthorityError::Integrity(
            "fixture must have one unknown default within exactly three scopes".to_owned(),
        ));
    }
    let active_authority_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM portable_devices
         WHERE library_id = ?1 AND device_id = ?2 AND device_kind = 'macos'
           AND role = 'authority' AND enrollment_state = 'active'",
        params![descriptor.library_id, descriptor.authority_device_id],
        |row| row.get(0),
    )?;
    if active_authority_count != 1 {
        return Err(FixtureAuthorityError::Integrity(
            "fixture Mac authority device is missing or inactive".to_owned(),
        ));
    }
    let _seed_writer_id = fixture_seed_writer_id(connection, &descriptor)?;
    #[cfg(feature = "sanitized-development-fixtures")]
    {
        let stored_key: Vec<u8> = connection.query_row(
            "SELECT public_signing_key FROM portable_devices WHERE device_id = ?1",
            [&_seed_writer_id],
            |row| row.get(0),
        )?;
        let expected = fixture_record_crypto(
            &descriptor.library_id,
            &_seed_writer_id,
            &descriptor.default_scope_id,
            descriptor.authority_generation,
            descriptor.purge_generation,
            descriptor.key_epoch,
        )?
        .signing_public_key();
        if stored_key != expected {
            return Err(FixtureAuthorityError::Integrity(
                "fixture seed writer signing key diverged".to_owned(),
            ));
        }
    }

    let profile: Option<(i64, String, String, String, i64)> = connection
        .query_row(
            "SELECT protocol_version, environment, library_data_class,
                    capabilities_json, high_water_cursor
             FROM direct_authority_profiles
             WHERE library_id = ?1 AND readiness_state = 'fixture_ready'",
            [&descriptor.library_id],
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
        .optional()?;
    let Some((protocol_version, environment, data_class, capabilities_json, high_water)) = profile
    else {
        return Err(FixtureAuthorityError::Integrity(
            "fixture direct authority profile is missing".to_owned(),
        ));
    };
    let stored_capabilities: ProtocolCapabilities = serde_json::from_str(&capabilities_json)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    if protocol_version != i64::from(SYNC_PROTOCOL_VERSION)
        || environment != "development"
        || data_class != "sanitized_fixture"
        || stored_capabilities != descriptor.capabilities
        || high_water < 1
    {
        return Err(FixtureAuthorityError::Integrity(
            "fixture direct authority profile diverged".to_owned(),
        ));
    }
    let change_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM direct_authority_changes WHERE library_id = ?1",
        [&descriptor.library_id],
        |row| row.get(0),
    )?;
    let sequence_bounds: (Option<i64>, Option<i64>) = connection.query_row(
        "SELECT MIN(sequence), MAX(sequence) FROM direct_authority_changes
         WHERE library_id = ?1",
        [&descriptor.library_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if change_count != high_water || sequence_bounds != (Some(1), Some(high_water)) {
        return Err(FixtureAuthorityError::Integrity(
            "direct authority change sequence is incomplete".to_owned(),
        ));
    }
    let (head_count, linked_head_count): (i64, i64) = connection.query_row(
        "SELECT
           (SELECT COUNT(*) FROM record_heads h
            JOIN portable_records p ON p.record_id = h.record_id
            WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')),
           (SELECT COUNT(*) FROM record_heads h
            JOIN portable_records p ON p.record_id = h.record_id
            JOIN direct_authority_mutations m
              ON m.record_id = h.record_id AND m.version_id = h.accepted_version_id
            JOIN direct_authority_changes c ON c.transaction_id = m.transaction_id
            WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder'))",
        [&descriptor.library_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if head_count == 0 || linked_head_count != head_count {
        return Err(FixtureAuthorityError::Integrity(
            "a portable Notes head lacks committed direct ciphertext".to_owned(),
        ));
    }
    validate_direct_bootstrap_and_log(connection, &descriptor, high_water, head_count)?;
    for kind in ["note", "category", "folder"] {
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM portable_records
             WHERE library_id = ?1 AND kind = ?2",
            params![descriptor.library_id, kind],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Err(FixtureAuthorityError::Integrity(format!(
                "fixture has no '{kind}' portable record"
            )));
        }
    }
    let checkpoint_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM direct_sync_checkpoints
         WHERE library_id = ?1 AND authority_generation = ?2
           AND key_epoch = ?3",
        params![
            descriptor.library_id,
            to_i64(descriptor.authority_generation, "authority generation")?,
            to_i64(descriptor.key_epoch, "key epoch")?,
        ],
        |row| row.get(0),
    )?;
    if checkpoint_count == 0 {
        return Err(FixtureAuthorityError::Integrity(
            "fixture bootstrap checkpoint is missing".to_owned(),
        ));
    }
    Ok(descriptor)
}

fn validate_direct_bootstrap_and_log(
    connection: &Connection,
    descriptor: &SanitizedFixtureAuthorityDescriptor,
    high_water: i64,
    head_count: i64,
) -> Result<(), FixtureAuthorityError> {
    let mut statement = connection.prepare(
        "SELECT p.record_id, h.accepted_revision, h.accepted_version_id,
                m.envelope_json, c.sequence
         FROM record_heads h
         JOIN portable_records p ON p.record_id = h.record_id
         JOIN direct_authority_mutations m
           ON m.record_id = h.record_id AND m.version_id = h.accepted_version_id
         JOIN direct_authority_changes c ON c.transaction_id = m.transaction_id
         WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')
         ORDER BY p.record_id",
    )?;
    let rows = statement.query_map([&descriptor.library_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (record_id, revision, version_id, envelope_json, sequence) = row?;
        let mutation: MutationEnvelope = serde_json::from_str(&envelope_json).map_err(|_| {
            FixtureAuthorityError::Integrity("stored head mutation is invalid".to_owned())
        })?;
        if mutation.record_id != record_id || mutation.version_id != version_id {
            return Err(FixtureAuthorityError::Integrity(
                "portable head and direct mutation diverged".to_owned(),
            ));
        }
        records.push(BootstrapRecord {
            record_id,
            accepted_head: AcceptedHead {
                revision: to_u64(revision, "accepted revision")?,
                version_id,
                ciphertext_hash: mutation.ciphertext_hash.clone(),
                authority_generation: mutation.authority_generation,
                acceptance_checkpoint: to_u64(sequence, "acceptance checkpoint")?,
            },
            mutation,
        });
    }
    drop(statement);
    if i64::try_from(records.len()).ok() != Some(head_count) {
        return Err(FixtureAuthorityError::Integrity(
            "bootstrap record count diverged".to_owned(),
        ));
    }
    let mut snapshot = BootstrapSnapshot {
        contract_version: BOOTSTRAP_SNAPSHOT_VERSION.to_owned(),
        library_id: descriptor.library_id.clone(),
        authority_generation: descriptor.authority_generation,
        purge_generation: descriptor.purge_generation,
        key_epoch: descriptor.key_epoch,
        high_water_cursor: to_u64(high_water, "high-water cursor")?,
        records,
        checkpoint_digest: String::new(),
    };
    snapshot.checkpoint_digest = snapshot.computed_checkpoint_digest();
    snapshot
        .validate()
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;

    let mut changes = connection.prepare(
        "SELECT c.sequence, c.transaction_digest, t.transaction_id,
                t.transaction_json, t.receipt_json
         FROM direct_authority_changes c
         JOIN direct_authority_transactions t ON t.transaction_id = c.transaction_id
         WHERE c.library_id = ?1 ORDER BY c.sequence",
    )?;
    let negotiated = negotiate_capabilities(&descriptor.capabilities, &descriptor.capabilities)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    let rows = changes.query_map([&descriptor.library_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut expected_sequence = 1_i64;
    for row in rows {
        let (sequence, digest, transaction_id, transaction_json, receipt_json) = row?;
        let signed: SignedTransaction = serde_json::from_str(&transaction_json).map_err(|_| {
            FixtureAuthorityError::Integrity("stored direct transaction is invalid".to_owned())
        })?;
        let receipt: TransactionReceipt = serde_json::from_str(&receipt_json).map_err(|_| {
            FixtureAuthorityError::Integrity("stored direct receipt is invalid".to_owned())
        })?;
        signed
            .validate(0, &negotiated)
            .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
        let mutation_ids = {
            let mut members = signed.members.iter().collect::<Vec<_>>();
            members.sort_by_key(|member| member.transaction_member_index);
            members
                .into_iter()
                .map(|member| member.mutation_id.clone())
                .collect::<Vec<_>>()
        };
        if sequence != expected_sequence
            || signed.manifest.transaction_id != transaction_id
            || signed.signed_digest() != digest
            || receipt.transaction_id != transaction_id
            || receipt.transaction_digest != digest
            || receipt.library_id != descriptor.library_id
            || receipt.device_id != signed.manifest.device_id
            || receipt.device_transaction_counter != signed.manifest.device_transaction_counter
            || receipt.authority_generation != signed.manifest.authority_generation
            || receipt.purge_generation != signed.manifest.purge_generation
            || receipt.mutation_ids != mutation_ids
            || receipt.high_water_cursor != to_u64(sequence, "change sequence")?
            || !matches!(receipt.disposition, ReceiptDisposition::Accepted { .. })
        {
            return Err(FixtureAuthorityError::Integrity(
                "accepted direct transaction binding is invalid".to_owned(),
            ));
        }
        let mut stored_members = connection.prepare(
            "SELECT envelope_json FROM direct_authority_mutations
             WHERE transaction_id = ?1 ORDER BY member_index",
        )?;
        let stored_members = stored_members
            .query_map([&transaction_id], |row| row.get::<_, String>(0))?
            .map(|value| {
                value.and_then(|json| {
                    serde_json::from_str::<MutationEnvelope>(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut signed_members = signed.members;
        signed_members.sort_by_key(|member| member.transaction_member_index);
        if stored_members != signed_members {
            return Err(FixtureAuthorityError::Integrity(
                "direct mutation rows diverged from their signed transaction".to_owned(),
            ));
        }
        #[cfg(feature = "sanitized-development-fixtures")]
        for member in &signed_members {
            open_generated_fixture_seed_mutation(member, &descriptor.default_scope_id)?;
        }
        expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
            FixtureAuthorityError::Integrity("direct change sequence overflowed".to_owned())
        })?;
    }
    if expected_sequence - 1 != high_water {
        return Err(FixtureAuthorityError::Integrity(
            "direct transaction log ended before its high-water cursor".to_owned(),
        ));
    }
    Ok(())
}

fn fixture_seed_writer_id(
    connection: &Connection,
    descriptor: &SanitizedFixtureAuthorityDescriptor,
) -> Result<String, FixtureAuthorityError> {
    let mut statement = connection.prepare(
        "SELECT device_id FROM portable_devices
         WHERE library_id = ?1 AND device_kind = 'fixture_seed'
           AND role = 'replica' AND enrollment_state = 'revoked'",
    )?;
    let ids = statement
        .query_map([&descriptor.library_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match ids.as_slice() {
        [device_id] if crate::portable::is_uuid_v7(device_id) => Ok(device_id.clone()),
        _ => Err(FixtureAuthorityError::Integrity(
            "fixture must retain exactly one revoked seed writer".to_owned(),
        )),
    }
}

fn exact_notes_capabilities() -> ProtocolCapabilities {
    let mut capabilities = ProtocolCapabilities::new(
        SYNC_PROTOCOL_VERSION,
        SYNC_PROTOCOL_VERSION,
        BTreeMap::from([
            ("category".to_owned(), RecordKindCapability::new(1, 1)),
            ("folder".to_owned(), RecordKindCapability::new(1, 1)),
            ("note".to_owned(), RecordKindCapability::new(1, 1)),
        ]),
    );
    capabilities.max_transaction_members = MAX_DIRECT_TRANSACTION_MEMBERS;
    capabilities.max_transaction_bytes = MAX_DIRECT_TRANSACTION_BYTES;
    capabilities
}

fn pairing_policy(descriptor: &SanitizedFixtureAuthorityDescriptor) -> PairingPolicy {
    PairingPolicy {
        library_id: descriptor.library_id.clone(),
        environment: descriptor.environment,
        library_data_class: descriptor.library_data_class,
        authority_generation: descriptor.authority_generation,
        grantable_scopes: BTreeSet::from([
            RecordKind::Note,
            RecordKind::Category,
            RecordKind::Folder,
        ]),
        capabilities: BTreeMap::from([
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
        ]),
    }
}

fn capabilities_digest(
    capabilities: &ProtocolCapabilities,
) -> Result<String, FixtureAuthorityError> {
    let value = serde_json::to_value(capabilities)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    Ok(canonical_sha256(&value))
}

fn descriptor_digest(
    descriptor: &SanitizedFixtureAuthorityDescriptor,
) -> Result<String, FixtureAuthorityError> {
    let capabilities = serde_json::to_value(&descriptor.capabilities)
        .map_err(|error| FixtureAuthorityError::Integrity(error.to_string()))?;
    Ok(canonical_sha256(&json!({
        "fixtureContract": FIXTURE_CONTRACT,
        "fixtureClass": FIXTURE_CLASS,
        "contractVersion": FIXTURE_CONTRACT_VERSION,
        "libraryId": descriptor.library_id,
        "authorityDeviceId": descriptor.authority_device_id,
        "defaultScopeId": descriptor.default_scope_id,
        "authorityGeneration": descriptor.authority_generation,
        "purgeGeneration": descriptor.purge_generation,
        "keyEpoch": descriptor.key_epoch,
        "environment": descriptor.environment,
        "libraryDataClass": descriptor.library_data_class,
        "capabilities": capabilities,
    })))
}

fn to_u64(value: i64, field: &str) -> Result<u64, FixtureAuthorityError> {
    u64::try_from(value)
        .map_err(|_| FixtureAuthorityError::Integrity(format!("{field} is negative")))
}

fn to_i64(value: u64, field: &str) -> Result<i64, FixtureAuthorityError> {
    i64::try_from(value)
        .map_err(|_| FixtureAuthorityError::Integrity(format!("{field} exceeds SQLite range")))
}

struct ProvisioningClock;

impl FixtureAuthorityClock for ProvisioningClock {
    fn now_ms(&self) -> Result<i64, ()> {
        Ok(FIXTURE_CREATED_AT_MS)
    }
}
