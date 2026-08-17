//! Durable, fixture-only storage seam for the direct Mac authority.
//!
//! This module deliberately contains no listener, Tauri command, private key,
//! or production-mode constructor. Callers perform cryptography outside the
//! database writer, start one immediate SQLite transaction, invoke one of the
//! transition primitives below, and emit a response only after committing.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

pub const DIRECT_AUTHORITY_SCHEMA_VERSION: u32 = 3;

const DEVELOPMENT: &str = "development";
const SANITIZED_FIXTURE: &str = "sanitized_fixture";
const MAX_INVITATION_LIFETIME_MS: i64 = 5 * 60 * 1_000;
const PAIRING_LEDGER_RATE_WINDOW_MS: i64 = 5 * 60 * 1_000;
const MAX_FAILED_ATTEMPTS: i64 = 5;
const MAX_REPLAY_ROWS: i64 = 128;
const MAX_QUARANTINE_ROWS: i64 = 128;
/// Exact response rows are immutable audit evidence. Admission limits count
/// only this trusted-authority retry/rate window, so retained history can never
/// permanently lock a device out.
pub const EXACT_RESPONSE_REPLAY_RETENTION_MS: i64 = PAIRING_LEDGER_RATE_WINDOW_MS;
const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

const DIRECT_AUTHORITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS direct_authority_profiles (
  library_id           TEXT PRIMARY KEY REFERENCES libraries(library_id) ON DELETE RESTRICT,
  protocol_version     INTEGER NOT NULL CHECK(protocol_version > 0),
  environment          TEXT NOT NULL CHECK(environment IN ('development', 'production')),
  library_data_class   TEXT NOT NULL CHECK(library_data_class IN ('sanitized_fixture', 'personal')),
  capabilities_json    TEXT NOT NULL,
  high_water_cursor    INTEGER NOT NULL DEFAULT 0 CHECK(high_water_cursor >= 0),
  state_revision       INTEGER NOT NULL DEFAULT 0 CHECK(state_revision >= 0),
  readiness_state      TEXT NOT NULL CHECK(readiness_state IN ('fixture_ready', 'initializing', 'disabled')),
  created_at_ms        INTEGER NOT NULL,
  updated_at_ms        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS direct_pairing_invitations (
  invitation_id                    TEXT PRIMARY KEY,
  library_id                       TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  authority_generation             INTEGER NOT NULL CHECK(authority_generation > 0),
  invitation_digest                BLOB NOT NULL CHECK(length(invitation_digest) = 32),
  nonce_hash                       BLOB NOT NULL CHECK(length(nonce_hash) = 32),
  mac_pairing_signing_public_key   BLOB NOT NULL CHECK(length(mac_pairing_signing_public_key) = 65),
  mac_pairing_hpke_public_key      BLOB NOT NULL CHECK(length(mac_pairing_hpke_public_key) = 32),
  tls_spki_sha256                  BLOB NOT NULL CHECK(length(tls_spki_sha256) = 32),
  scope_ceiling_json               TEXT NOT NULL,
  environment                      TEXT NOT NULL CHECK(environment IN ('development', 'production')),
  created_at_ms                    INTEGER NOT NULL,
  expires_at_ms                    INTEGER NOT NULL,
  failed_attempts                  INTEGER NOT NULL DEFAULT 0 CHECK(failed_attempts BETWEEN 0 AND 5),
  state                            TEXT NOT NULL CHECK(state IN ('pending', 'consumed', 'active', 'cancelled', 'expired', 'revoked')),
  state_revision                   INTEGER NOT NULL DEFAULT 0 CHECK(state_revision >= 0),
  CHECK(expires_at_ms > created_at_ms),
  CHECK(expires_at_ms - created_at_ms <= 300000)
);
CREATE INDEX IF NOT EXISTS direct_pairing_invitations_library_state
  ON direct_pairing_invitations(library_id, state, expires_at_ms);

CREATE TABLE IF NOT EXISTS direct_enrollment_receipts (
  receipt_id                       TEXT PRIMARY KEY,
  invitation_id                    TEXT NOT NULL UNIQUE REFERENCES direct_pairing_invitations(invitation_id) ON DELETE RESTRICT,
  library_id                       TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  device_id                        TEXT NOT NULL,
  display_name                     TEXT NOT NULL,
  app_version                      TEXT NOT NULL,
  build_version                    TEXT NOT NULL,
  authority_generation             INTEGER NOT NULL CHECK(authority_generation > 0),
  receipt_json                     TEXT NOT NULL,
  granted_scopes_json              TEXT NOT NULL,
  capabilities_json                TEXT NOT NULL,
  client_signing_public_key        BLOB NOT NULL CHECK(length(client_signing_public_key) = 65),
  client_hpke_public_key           BLOB NOT NULL CHECK(length(client_hpke_public_key) = 32),
  begin_response_bytes             BLOB NOT NULL CHECK(length(begin_response_bytes) > 0 AND length(begin_response_bytes) <= 2097152),
  verification_code               TEXT,
  confirmation_digest             BLOB CHECK(confirmation_digest IS NULL OR length(confirmation_digest) = 32),
  bootstrap_envelope_bytes         BLOB,
  bootstrap_envelope_digest        BLOB CHECK(bootstrap_envelope_digest IS NULL OR length(bootstrap_envelope_digest) = 32),
  bootstrap_response_bytes         BLOB,
  failed_finish_attempts           INTEGER NOT NULL DEFAULT 0 CHECK(failed_finish_attempts BETWEEN 0 AND 5),
  state                            TEXT NOT NULL CHECK(state IN ('pending_user_confirmation', 'pending_finish', 'active', 'cancelled', 'expired', 'revoked')),
  server_finish_bytes              BLOB,
  created_at_ms                    INTEGER NOT NULL,
  expires_at_ms                    INTEGER NOT NULL,
  activated_at_ms                  INTEGER,
  revoked_at_ms                    INTEGER,
  state_revision                   INTEGER NOT NULL DEFAULT 0 CHECK(state_revision >= 0),
  CHECK((bootstrap_envelope_bytes IS NULL) = (bootstrap_envelope_digest IS NULL)),
  CHECK((bootstrap_envelope_bytes IS NULL) = (bootstrap_response_bytes IS NULL)),
  CHECK((confirmation_digest IS NULL) = (bootstrap_response_bytes IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS direct_enrollment_receipts_live_device
  ON direct_enrollment_receipts(library_id, device_id)
  WHERE state IN ('pending_user_confirmation', 'pending_finish', 'active');
CREATE INDEX IF NOT EXISTS direct_enrollment_receipts_library_state
  ON direct_enrollment_receipts(library_id, state, expires_at_ms);

CREATE TABLE IF NOT EXISTS direct_pairing_replays (
  message_kind         TEXT NOT NULL CHECK(message_kind IN ('client_hello', 'client_finish')),
  message_id           TEXT NOT NULL,
  subject_id           TEXT NOT NULL,
  request_digest       BLOB NOT NULL CHECK(length(request_digest) = 32),
  tls_spki_sha256      BLOB NOT NULL CHECK(length(tls_spki_sha256) = 32),
  exact_response_bytes BLOB NOT NULL CHECK(length(exact_response_bytes) > 0 AND length(exact_response_bytes) <= 2097152),
  created_at_ms        INTEGER NOT NULL,
  PRIMARY KEY(message_kind, message_id)
);
CREATE INDEX IF NOT EXISTS direct_pairing_replays_created
  ON direct_pairing_replays(message_kind, created_at_ms);

CREATE TABLE IF NOT EXISTS direct_pairing_quarantine (
  quarantine_id        INTEGER PRIMARY KEY AUTOINCREMENT,
  identifier_kind      TEXT NOT NULL,
  identifier           TEXT NOT NULL,
  accepted_digest      BLOB NOT NULL CHECK(length(accepted_digest) = 32),
  observed_digest      BLOB NOT NULL CHECK(length(observed_digest) = 32),
  reason               TEXT NOT NULL,
  quarantined_at_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS direct_pairing_quarantine_created
  ON direct_pairing_quarantine(quarantined_at_ms);

CREATE TABLE IF NOT EXISTS direct_authority_transactions (
  transaction_id             TEXT PRIMARY KEY,
  library_id                 TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  device_id                  TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  authority_generation       INTEGER NOT NULL CHECK(authority_generation > 0),
  device_transaction_counter INTEGER NOT NULL CHECK(device_transaction_counter > 0),
  signed_digest              TEXT NOT NULL CHECK(length(signed_digest) = 64),
  transaction_json           TEXT NOT NULL,
  state                      TEXT NOT NULL CHECK(state IN ('prepared', 'accepted', 'conflict', 'rejected')),
  receipt_json               TEXT,
  accepted_cursor            INTEGER CHECK(accepted_cursor IS NULL OR accepted_cursor > 0),
  created_at_ms              INTEGER NOT NULL,
  expires_at_ms              INTEGER NOT NULL,
  terminal_at_ms             INTEGER,
  UNIQUE(device_id, device_transaction_counter),
  UNIQUE(library_id, accepted_cursor),
  CHECK((state = 'prepared') = (receipt_json IS NULL)),
  CHECK((state = 'accepted') = (accepted_cursor IS NOT NULL)),
  CHECK((state = 'prepared') = (terminal_at_ms IS NULL))
);

CREATE TABLE IF NOT EXISTS direct_authority_mutations (
  mutation_id          TEXT PRIMARY KEY,
  transaction_id       TEXT NOT NULL REFERENCES direct_authority_transactions(transaction_id) ON DELETE RESTRICT,
  member_index         INTEGER NOT NULL CHECK(member_index >= 0),
  signed_digest        TEXT NOT NULL CHECK(length(signed_digest) = 64),
  record_id            TEXT NOT NULL,
  version_id           TEXT NOT NULL,
  envelope_json        TEXT NOT NULL,
  UNIQUE(transaction_id, member_index)
);

CREATE TABLE IF NOT EXISTS direct_authority_changes (
  library_id           TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  sequence             INTEGER NOT NULL CHECK(sequence > 0),
  transaction_id       TEXT NOT NULL UNIQUE REFERENCES direct_authority_transactions(transaction_id) ON DELETE RESTRICT,
  transaction_digest   TEXT NOT NULL CHECK(length(transaction_digest) = 64),
  created_at_ms        INTEGER NOT NULL,
  PRIMARY KEY(library_id, sequence)
);

CREATE TABLE IF NOT EXISTS direct_sync_checkpoints (
  library_id             TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  authority_generation   INTEGER NOT NULL CHECK(authority_generation > 0),
  high_water_cursor      INTEGER NOT NULL CHECK(high_water_cursor >= 0),
  purge_generation       INTEGER NOT NULL CHECK(purge_generation >= 0),
  key_epoch              INTEGER NOT NULL CHECK(key_epoch > 0),
  checkpoint_digest      TEXT NOT NULL CHECK(length(checkpoint_digest) = 64),
  exact_response_bytes   BLOB NOT NULL CHECK(length(exact_response_bytes) > 0 AND length(exact_response_bytes) <= 2097152),
  created_at_ms          INTEGER NOT NULL,
  PRIMARY KEY(library_id, authority_generation, high_water_cursor),
  UNIQUE(library_id, checkpoint_digest),
  UNIQUE(library_id, authority_generation, high_water_cursor, checkpoint_digest)
);

CREATE TABLE IF NOT EXISTS direct_device_sync_state (
  device_id                TEXT PRIMARY KEY REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  library_id               TEXT NOT NULL REFERENCES direct_authority_profiles(library_id) ON DELETE RESTRICT,
  authority_generation     INTEGER NOT NULL CHECK(authority_generation > 0),
  acknowledged_cursor      INTEGER CHECK(acknowledged_cursor IS NULL OR acknowledged_cursor >= 0),
  checkpoint_digest        TEXT CHECK(checkpoint_digest IS NULL OR length(checkpoint_digest) = 64),
  last_ack_response_bytes  BLOB,
  acknowledged_at_ms       INTEGER,
  last_seen_at_ms           INTEGER,
  CHECK((acknowledged_cursor IS NULL) = (checkpoint_digest IS NULL)),
  CHECK((acknowledged_cursor IS NULL) = (last_ack_response_bytes IS NULL)),
  FOREIGN KEY(library_id, authority_generation, acknowledged_cursor, checkpoint_digest)
    REFERENCES direct_sync_checkpoints(library_id, authority_generation, high_water_cursor, checkpoint_digest)
    ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS direct_request_replays (
  device_id             TEXT NOT NULL REFERENCES portable_devices(device_id) ON DELETE RESTRICT,
  request_id            TEXT NOT NULL,
  endpoint              TEXT NOT NULL,
  request_digest        BLOB NOT NULL CHECK(length(request_digest) = 32),
  status_code           INTEGER NOT NULL CHECK(status_code BETWEEN 100 AND 599),
  exact_response_bytes  BLOB NOT NULL CHECK(length(exact_response_bytes) > 0 AND length(exact_response_bytes) <= 2097152),
  created_at_ms         INTEGER NOT NULL,
  PRIMARY KEY(device_id, request_id)
);
CREATE INDEX IF NOT EXISTS direct_request_replays_created
  ON direct_request_replays(device_id, created_at_ms);

CREATE TRIGGER IF NOT EXISTS direct_invitation_state_guard
BEFORE UPDATE OF state ON direct_pairing_invitations
WHEN NOT (
  OLD.state = NEW.state OR
  (OLD.state = 'pending' AND NEW.state IN ('consumed', 'cancelled', 'expired')) OR
  (OLD.state = 'consumed' AND NEW.state IN ('active', 'cancelled', 'expired', 'revoked')) OR
  (OLD.state = 'active' AND NEW.state = 'revoked')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid direct invitation state transition');
END;

CREATE TRIGGER IF NOT EXISTS direct_receipt_state_guard
BEFORE UPDATE OF state ON direct_enrollment_receipts
WHEN NOT (
  OLD.state = NEW.state OR
  (OLD.state = 'pending_user_confirmation' AND NEW.state IN ('pending_finish', 'cancelled', 'expired')) OR
  (OLD.state = 'pending_finish' AND NEW.state IN ('active', 'cancelled', 'expired', 'revoked')) OR
  (OLD.state = 'active' AND NEW.state = 'revoked')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid direct receipt state transition');
END;

CREATE TRIGGER IF NOT EXISTS direct_terminal_transaction_guard
BEFORE UPDATE ON direct_authority_transactions
WHEN OLD.state != 'prepared'
BEGIN
  SELECT RAISE(ABORT, 'terminal direct transaction is immutable');
END;

CREATE TRIGGER IF NOT EXISTS direct_pairing_replays_no_update
BEFORE UPDATE ON direct_pairing_replays BEGIN
  SELECT RAISE(ABORT, 'direct pairing replay is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_pairing_replays_no_delete
BEFORE DELETE ON direct_pairing_replays BEGIN
  SELECT RAISE(ABORT, 'direct pairing replay is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_pairing_quarantine_no_update
BEFORE UPDATE ON direct_pairing_quarantine BEGIN
  SELECT RAISE(ABORT, 'direct pairing quarantine is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_pairing_quarantine_no_delete
BEFORE DELETE ON direct_pairing_quarantine BEGIN
  SELECT RAISE(ABORT, 'direct pairing quarantine is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_authority_mutations_no_update
BEFORE UPDATE ON direct_authority_mutations BEGIN
  SELECT RAISE(ABORT, 'direct authority mutation is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_authority_mutations_no_delete
BEFORE DELETE ON direct_authority_mutations BEGIN
  SELECT RAISE(ABORT, 'direct authority mutation is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_authority_changes_no_update
BEFORE UPDATE ON direct_authority_changes BEGIN
  SELECT RAISE(ABORT, 'direct authority change is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_authority_changes_no_delete
BEFORE DELETE ON direct_authority_changes BEGIN
  SELECT RAISE(ABORT, 'direct authority change is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_sync_checkpoints_no_update
BEFORE UPDATE ON direct_sync_checkpoints BEGIN
  SELECT RAISE(ABORT, 'direct sync checkpoint is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_sync_checkpoints_no_delete
BEFORE DELETE ON direct_sync_checkpoints BEGIN
  SELECT RAISE(ABORT, 'direct sync checkpoint is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_request_replays_no_update
BEFORE UPDATE ON direct_request_replays BEGIN
  SELECT RAISE(ABORT, 'direct request replay is immutable');
END;
CREATE TRIGGER IF NOT EXISTS direct_request_replays_no_delete
BEFORE DELETE ON direct_request_replays BEGIN
  SELECT RAISE(ABORT, 'direct request replay is immutable');
END;
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Database(String),
    InvalidInput(&'static str),
    FixtureOnly,
    StateUnavailable(&'static str),
    InvitationNotFound,
    InvitationConsumed,
    InvitationExpired,
    ReceiptNotFound,
    UserConfirmationRequired,
    EnrollmentCancelled,
    EnrollmentAlreadyActive,
    DeviceNotFound,
    DeviceRevoked,
    PinMismatch,
    ReplayLimit,
    QuarantineLimit,
    CheckpointMismatch,
    AckMismatch,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "direct authority database error: {error}"),
            Self::InvalidInput(field) => {
                write!(formatter, "invalid direct authority field: {field}")
            }
            Self::FixtureOnly => {
                formatter.write_str("direct authority is limited to sanitized development fixtures")
            }
            Self::StateUnavailable(reason) => {
                write!(formatter, "direct authority state unavailable: {reason}")
            }
            Self::InvitationNotFound => formatter.write_str("direct invitation not found"),
            Self::InvitationConsumed => formatter.write_str("direct invitation already consumed"),
            Self::InvitationExpired => formatter.write_str("direct invitation expired"),
            Self::ReceiptNotFound => formatter.write_str("direct enrollment receipt not found"),
            Self::UserConfirmationRequired => {
                formatter.write_str("direct enrollment requires user confirmation")
            }
            Self::EnrollmentCancelled => formatter.write_str("direct enrollment is cancelled"),
            Self::EnrollmentAlreadyActive => {
                formatter.write_str("direct enrollment is already active")
            }
            Self::DeviceNotFound => formatter.write_str("direct device not found"),
            Self::DeviceRevoked => formatter.write_str("direct device is revoked"),
            Self::PinMismatch => formatter.write_str("direct transport pin mismatch"),
            Self::ReplayLimit => formatter.write_str("direct replay limit reached"),
            Self::QuarantineLimit => formatter.write_str("direct quarantine limit reached"),
            Self::CheckpointMismatch => formatter.write_str("direct checkpoint mismatch"),
            Self::AckMismatch => formatter.write_str("direct acknowledgement mismatch"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone)]
pub struct NewInvitation {
    pub invitation_id: String,
    pub library_id: String,
    pub authority_generation: u64,
    pub invitation_digest: [u8; 32],
    pub nonce_hash: [u8; 32],
    pub mac_pairing_signing_public_key: [u8; 65],
    pub mac_pairing_hpke_public_key: [u8; 32],
    pub tls_spki_sha256: [u8; 32],
    pub scope_ceiling_json: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvitationRegistration {
    Registered,
    ExactReplay,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    Recorded {
        failed_attempts: u8,
        attempts_remaining: u8,
    },
    Cancelled,
    Expired,
}

#[derive(Debug, Clone)]
pub struct ConsumeInvitation {
    pub message_id: String,
    pub invitation_id: String,
    pub request_digest: [u8; 32],
    pub observed_tls_spki_sha256: [u8; 32],
    pub receipt_id: String,
    pub device_id: String,
    pub display_name: String,
    pub app_version: String,
    pub build_version: String,
    pub receipt_json: String,
    pub granted_scopes_json: String,
    pub capabilities_json: String,
    pub client_signing_public_key: [u8; 65],
    pub client_hpke_public_key: [u8; 32],
    pub exact_begin_response_bytes: Vec<u8>,
    pub verification_code: String,
    /// Trusted Mac-authority time; this type is constructed by the coordinator
    /// and is never deserialized directly from ClientHello.
    pub authority_now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsumeOutcome {
    Consumed(Vec<u8>),
    ExactReplay(Vec<u8>),
    Quarantined,
    Expired,
}

#[derive(Debug, Clone)]
pub struct ConfirmEnrollment {
    pub receipt_id: String,
    pub confirmation_digest: [u8; 32],
    pub displayed_verification_code: String,
    pub displayed_scopes_json: String,
    pub approved: bool,
    pub bootstrap_envelope_bytes: Vec<u8>,
    pub bootstrap_envelope_digest: [u8; 32],
    pub exact_bootstrap_response_bytes: Vec<u8>,
    /// Trusted Mac-authority time, not a value supplied by the phone.
    pub authority_now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Confirmed(Vec<u8>),
    ExactReplay(Vec<u8>),
    Quarantined,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone)]
pub struct ActivateEnrollment {
    pub message_id: String,
    pub receipt_id: String,
    pub device_id: String,
    pub authority_generation: u64,
    pub request_digest: [u8; 32],
    pub observed_tls_spki_sha256: [u8; 32],
    pub exact_server_finish_bytes: Vec<u8>,
    /// Trusted Mac-authority time; ClientFinish cannot select replay windows.
    pub authority_now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivateOutcome {
    Activated(Vec<u8>),
    ExactReplay(Vec<u8>),
    Quarantined,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    Revoked,
    AlreadyRevoked,
}

#[derive(Debug, Clone)]
pub struct FixtureAcceptedChange {
    pub request_id: String,
    pub request_digest: [u8; 32],
    pub exact_response_bytes: Vec<u8>,
    pub library_id: String,
    pub authority_generation: u64,
    pub transaction_id: String,
    pub device_id: String,
    pub device_transaction_counter: u64,
    pub transaction_digest: String,
    pub transaction_json: String,
    pub receipt_json: String,
    /// Trusted wall-clock time supplied by the Mac authority, never copied
    /// from the signed device transaction. Abuse windows and durable replay
    /// timestamps use this value so a device cannot move its own rate window.
    pub authority_now_ms: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixtureChangeOutcome {
    Accepted {
        cursor: u64,
        exact_response_bytes: Vec<u8>,
    },
    ExactReplay(Vec<u8>),
    Quarantined,
}

#[derive(Debug, Clone)]
pub struct IssueCheckpoint {
    pub library_id: String,
    pub authority_generation: u64,
    pub high_water_cursor: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub checkpoint_digest: String,
    pub exact_response_bytes: Vec<u8>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointOutcome {
    Issued(Vec<u8>),
    ExactReplay(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct AcknowledgeCheckpoint {
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: u64,
    pub high_water_cursor: u64,
    pub checkpoint_digest: String,
    pub exact_response_bytes: Vec<u8>,
    /// Trusted Mac-authority time, kept outside the signed acknowledgement.
    pub authority_now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckOutcome {
    Recorded(Vec<u8>),
    ExactReplay(Vec<u8>),
}

#[derive(Debug, Clone)]
struct ReplayRow {
    subject_id: String,
    digest: Vec<u8>,
    pin: Vec<u8>,
    response: Vec<u8>,
}

#[derive(Debug, Clone)]
struct InvitationRow {
    library_id: String,
    authority_generation: u64,
    pin: Vec<u8>,
    expires_at_ms: i64,
    state: String,
}

#[derive(Debug, Clone)]
struct ReceiptRow {
    invitation_id: String,
    library_id: String,
    device_id: String,
    display_name: String,
    authority_generation: u64,
    granted_scopes_json: String,
    capabilities_json: String,
    signing_key: Vec<u8>,
    hpke_key: Vec<u8>,
    verification_code: Option<String>,
    confirmation_digest: Option<Vec<u8>>,
    bootstrap_response: Option<Vec<u8>>,
    failed_finish_attempts: i64,
    expires_at_ms: i64,
    state: String,
}

#[derive(Debug)]
struct AcknowledgementStateRow {
    acknowledged_cursor: Option<i64>,
    checkpoint_digest: Option<String>,
    exact_response_bytes: Option<Vec<u8>>,
}

pub struct DirectAuthorityStore;

impl DirectAuthorityStore {
    /// Install only the v3 expansion. The caller owns migration ordering,
    /// recovery snapshots, schema stamping, and the surrounding transaction.
    pub fn install_schema(transaction: &Transaction<'_>) -> StoreResult<()> {
        transaction.execute_batch(DIRECT_AUTHORITY_SCHEMA)?;
        Ok(())
    }

    /// Create the sole supported authority profile. There is intentionally no
    /// API accepting caller-selected environment or data-class labels.
    pub fn initialize_fixture_profile(
        transaction: &Transaction<'_>,
        library_id: &str,
        authority_generation: u64,
        capabilities_json: &str,
        now_ms: i64,
    ) -> StoreResult<()> {
        validate_authority_time(now_ms)?;
        validate_uuid_v7(library_id, "library_id")?;
        validate_json(capabilities_json, "capabilities_json")?;
        let generation: i64 = transaction
            .query_row(
                "SELECT authority_generation FROM libraries WHERE library_id = ?1",
                [library_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::StateUnavailable("portable library is missing"))?;
        if u64_from_db(generation, "authority_generation")? != authority_generation
            || authority_generation == 0
        {
            return Err(StoreError::StateUnavailable(
                "portable authority generation does not match",
            ));
        }

        let existing: Option<(String, String, i64, String)> = transaction
            .query_row(
                "SELECT environment, library_data_class, protocol_version, capabilities_json
                 FROM direct_authority_profiles WHERE library_id = ?1",
                [library_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((environment, data_class, protocol_version, capabilities)) = existing {
            if environment == DEVELOPMENT
                && data_class == SANITIZED_FIXTURE
                && protocol_version == 1
                && json_equal(&capabilities, capabilities_json)?
            {
                return Ok(());
            }
            return Err(StoreError::FixtureOnly);
        }

        transaction.execute(
            "INSERT INTO direct_authority_profiles
             (library_id, protocol_version, environment, library_data_class,
              capabilities_json, high_water_cursor, state_revision,
              readiness_state, created_at_ms, updated_at_ms)
             VALUES (?1, 1, 'development', 'sanitized_fixture', ?2, 0, 0,
                     'fixture_ready', ?3, ?3)",
            params![library_id, capabilities_json, now_ms],
        )?;
        Ok(())
    }

    pub fn register_invitation(
        transaction: &Transaction<'_>,
        invitation: &NewInvitation,
        now_ms: i64,
    ) -> StoreResult<InvitationRegistration> {
        validate_authority_time(now_ms)?;
        validate_invitation(invitation, now_ms)?;
        require_fixture_profile(
            transaction,
            &invitation.library_id,
            invitation.authority_generation,
        )?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT invitation_digest FROM direct_pairing_invitations
                 WHERE invitation_id = ?1",
                [&invitation.invitation_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
        {
            if existing == invitation.invitation_digest {
                return Ok(InvitationRegistration::ExactReplay);
            }
            quarantine(
                transaction,
                "invitation",
                &invitation.invitation_id,
                &existing,
                &invitation.invitation_digest,
                "byte-different invitation id reuse",
                now_ms,
            )?;
            return Ok(InvitationRegistration::Quarantined);
        }
        let invitation_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM direct_pairing_invitations
             WHERE state IN ('pending', 'consumed') AND expires_at_ms > ?1",
            [now_ms],
            |row| row.get(0),
        )?;
        if invitation_count >= MAX_REPLAY_ROWS {
            return Err(StoreError::ReplayLimit);
        }
        transaction.execute(
            "INSERT INTO direct_pairing_invitations
             (invitation_id, library_id, authority_generation, invitation_digest,
              nonce_hash, mac_pairing_signing_public_key,
              mac_pairing_hpke_public_key, tls_spki_sha256,
              scope_ceiling_json, environment, created_at_ms, expires_at_ms,
              failed_attempts, state, state_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'development',
                     ?10, ?11, 0, 'pending', 0)",
            params![
                invitation.invitation_id,
                invitation.library_id,
                i64_from_u64(invitation.authority_generation, "authority_generation")?,
                invitation.invitation_digest.as_slice(),
                invitation.nonce_hash.as_slice(),
                invitation.mac_pairing_signing_public_key.as_slice(),
                invitation.mac_pairing_hpke_public_key.as_slice(),
                invitation.tls_spki_sha256.as_slice(),
                invitation.scope_ceiling_json,
                invitation.created_at_ms,
                invitation.expires_at_ms,
            ],
        )?;
        Ok(InvitationRegistration::Registered)
    }

    /// Invalidate unconsumed invitations when a Mac authority session starts.
    /// Consumed receipts are deliberately untouched so their transcript-bound
    /// confirmation and finish retries remain restart-safe.
    pub fn invalidate_pending_invitations_on_restart(
        transaction: &Transaction<'_>,
        library_id: &str,
        authority_generation: u64,
    ) -> StoreResult<usize> {
        validate_uuid_v7(library_id, "library_id")?;
        require_fixture_profile(transaction, library_id, authority_generation)?;
        Ok(transaction.execute(
            "UPDATE direct_pairing_invitations
             SET state = 'cancelled', state_revision = state_revision + 1
             WHERE library_id = ?1 AND authority_generation = ?2
               AND state = 'pending'",
            params![
                library_id,
                i64_from_u64(authority_generation, "authority_generation")?
            ],
        )?)
    }

    /// Persist one validated ClientHello failure. The cryptographic coordinator
    /// decides which failures consume an attempt; this primitive only makes the
    /// monotonic count and terminal cancellation crash-safe.
    pub fn record_invitation_failure(
        transaction: &Transaction<'_>,
        invitation_id: &str,
        now_ms: i64,
    ) -> StoreResult<AttemptOutcome> {
        validate_authority_time(now_ms)?;
        validate_uuid_v7(invitation_id, "invitation_id")?;
        let row: Option<(String, i64, i64, i64, String)> = transaction
            .query_row(
                "SELECT library_id, authority_generation, expires_at_ms,
                        failed_attempts, state
                 FROM direct_pairing_invitations WHERE invitation_id = ?1",
                [invitation_id],
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
        let Some((library_id, generation, expires_at_ms, failed_attempts, state)) = row else {
            return Err(StoreError::InvitationNotFound);
        };
        require_fixture_profile(
            transaction,
            &library_id,
            u64_from_db(generation, "authority_generation")?,
        )?;
        match state.as_str() {
            "cancelled" => return Ok(AttemptOutcome::Cancelled),
            "expired" => return Ok(AttemptOutcome::Expired),
            "revoked" => return Err(StoreError::DeviceRevoked),
            "consumed" | "active" => return Err(StoreError::InvitationConsumed),
            "pending" => {}
            _ => return Err(StoreError::StateUnavailable("unknown invitation state")),
        }
        if now_ms >= expires_at_ms {
            transaction.execute(
                "UPDATE direct_pairing_invitations
                 SET state = 'expired', state_revision = state_revision + 1
                 WHERE invitation_id = ?1 AND state = 'pending'",
                [invitation_id],
            )?;
            return Ok(AttemptOutcome::Expired);
        }
        let next = failed_attempts
            .checked_add(1)
            .ok_or(StoreError::StateUnavailable("invitation attempt overflow"))?;
        let cancelled = next >= MAX_FAILED_ATTEMPTS;
        transaction.execute(
            "UPDATE direct_pairing_invitations
             SET failed_attempts = ?2,
                 state = CASE WHEN ?3 THEN 'cancelled' ELSE state END,
                 state_revision = state_revision + 1
             WHERE invitation_id = ?1 AND state = 'pending'",
            params![invitation_id, next.min(MAX_FAILED_ATTEMPTS), cancelled],
        )?;
        if cancelled {
            Ok(AttemptOutcome::Cancelled)
        } else {
            Ok(AttemptOutcome::Recorded {
                failed_attempts: next as u8,
                attempts_remaining: (MAX_FAILED_ATTEMPTS - next) as u8,
            })
        }
    }

    pub fn consume_invitation(
        transaction: &Transaction<'_>,
        request: &ConsumeInvitation,
    ) -> StoreResult<ConsumeOutcome> {
        validate_consume(request)?;
        if let Some(replay) = replay_row(transaction, "client_hello", &request.message_id)? {
            if replay.pin != request.observed_tls_spki_sha256 {
                return Err(StoreError::PinMismatch);
            }
            if replay.digest == request.request_digest {
                if let Err(error) = require_hello_replay_authorized(
                    transaction,
                    &replay.subject_id,
                    request.authority_now_ms,
                ) {
                    if error == StoreError::InvitationExpired {
                        let receipt = receipt_row(transaction, &replay.subject_id)?
                            .ok_or(StoreError::ReceiptNotFound)?;
                        expire_receipt(transaction, &replay.subject_id, &receipt.invitation_id)?;
                        return Ok(ConsumeOutcome::Expired);
                    }
                    return Err(error);
                }
                return Ok(ConsumeOutcome::ExactReplay(replay.response));
            }
            quarantine(
                transaction,
                "client_hello",
                &request.message_id,
                &replay.digest,
                &request.request_digest,
                "byte-different ClientHello message id reuse",
                request.authority_now_ms,
            )?;
            return Ok(ConsumeOutcome::Quarantined);
        }
        enforce_replay_capacity(transaction, "client_hello", request.authority_now_ms)?;

        let invitation = invitation_row(transaction, &request.invitation_id)?
            .ok_or(StoreError::InvitationNotFound)?;
        if invitation.pin != request.observed_tls_spki_sha256 {
            return Err(StoreError::PinMismatch);
        }
        require_fixture_profile(
            transaction,
            &invitation.library_id,
            invitation.authority_generation,
        )?;
        if invitation.state == "pending" && request.authority_now_ms >= invitation.expires_at_ms {
            transaction.execute(
                "UPDATE direct_pairing_invitations
                 SET state = 'expired', state_revision = state_revision + 1
                 WHERE invitation_id = ?1 AND state = 'pending'",
                [&request.invitation_id],
            )?;
            return Ok(ConsumeOutcome::Expired);
        }
        if invitation.state != "pending" {
            return Err(if invitation.state == "expired" {
                StoreError::InvitationExpired
            } else {
                StoreError::InvitationConsumed
            });
        }

        // A phone may abandon enrollment after ClientHello and return with a
        // fresh invitation after the old receipt expires. Retire that stale
        // live row before the unique live-device index is evaluated so an
        // unknown old receipt id cannot permanently prevent re-enrollment.
        expire_stale_device_receipt(
            transaction,
            &invitation.library_id,
            &request.device_id,
            request.authority_now_ms,
        )?;

        transaction.execute(
            "INSERT INTO direct_enrollment_receipts
             (receipt_id, invitation_id, library_id, device_id, display_name,
              app_version, build_version, authority_generation, receipt_json,
              granted_scopes_json, capabilities_json, client_signing_public_key,
              client_hpke_public_key, begin_response_bytes, verification_code,
              failed_finish_attempts, state, created_at_ms, expires_at_ms,
              state_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15, 0, 'pending_user_confirmation',
                     ?16, ?17, 0)",
            params![
                request.receipt_id,
                request.invitation_id,
                invitation.library_id,
                request.device_id,
                request.display_name,
                request.app_version,
                request.build_version,
                i64_from_u64(invitation.authority_generation, "authority_generation")?,
                request.receipt_json,
                request.granted_scopes_json,
                request.capabilities_json,
                request.client_signing_public_key.as_slice(),
                request.client_hpke_public_key.as_slice(),
                request.exact_begin_response_bytes,
                request.verification_code,
                request.authority_now_ms,
                invitation.expires_at_ms,
            ],
        )?;
        let changed = transaction.execute(
            "UPDATE direct_pairing_invitations
             SET state = 'consumed', state_revision = state_revision + 1
             WHERE invitation_id = ?1 AND state = 'pending'",
            [&request.invitation_id],
        )?;
        if changed != 1 {
            return Err(StoreError::InvitationConsumed);
        }
        insert_pairing_replay(
            transaction,
            "client_hello",
            &request.message_id,
            &request.receipt_id,
            &request.request_digest,
            &request.observed_tls_spki_sha256,
            &request.exact_begin_response_bytes,
            request.authority_now_ms,
        )?;
        Ok(ConsumeOutcome::Consumed(
            request.exact_begin_response_bytes.clone(),
        ))
    }

    pub fn confirm_enrollment(
        transaction: &Transaction<'_>,
        confirmation: &ConfirmEnrollment,
    ) -> StoreResult<ConfirmOutcome> {
        validate_authority_time(confirmation.authority_now_ms)?;
        validate_uuid_v7(&confirmation.receipt_id, "receipt_id")?;
        validate_json(&confirmation.displayed_scopes_json, "displayed_scopes_json")?;
        let receipt = receipt_row(transaction, &confirmation.receipt_id)?
            .ok_or(StoreError::ReceiptNotFound)?;
        require_fixture_profile(
            transaction,
            &receipt.library_id,
            receipt.authority_generation,
        )?;
        if confirmation.authority_now_ms >= receipt.expires_at_ms
            && matches!(
                receipt.state.as_str(),
                "pending_user_confirmation" | "pending_finish"
            )
        {
            expire_receipt(
                transaction,
                &confirmation.receipt_id,
                &receipt.invitation_id,
            )?;
            return Ok(ConfirmOutcome::Expired);
        }
        match receipt.state.as_str() {
            "pending_finish" => {
                let digest_matches = receipt.confirmation_digest.as_deref()
                    == Some(confirmation.confirmation_digest.as_slice());
                let code_matches = receipt.verification_code.as_deref()
                    == Some(confirmation.displayed_verification_code.as_str());
                let scopes_match = json_equal(
                    &receipt.granted_scopes_json,
                    &confirmation.displayed_scopes_json,
                )?;
                if !confirmation.approved || !digest_matches || !code_matches || !scopes_match {
                    let accepted = receipt.confirmation_digest.as_deref().ok_or(
                        StoreError::StateUnavailable(
                            "pending finish receipt is missing confirmation digest",
                        ),
                    )?;
                    quarantine(
                        transaction,
                        "enrollment_confirmation",
                        &confirmation.receipt_id,
                        accepted,
                        &confirmation.confirmation_digest,
                        "byte-different enrollment confirmation replay",
                        confirmation.authority_now_ms,
                    )?;
                    return Ok(ConfirmOutcome::Quarantined);
                }
                return receipt
                    .bootstrap_response
                    .map(ConfirmOutcome::ExactReplay)
                    .ok_or(StoreError::StateUnavailable(
                        "pending finish receipt is missing bootstrap response",
                    ));
            }
            "pending_user_confirmation" => {}
            "active" => return Err(StoreError::EnrollmentAlreadyActive),
            "cancelled" | "revoked" => return Err(StoreError::EnrollmentCancelled),
            "expired" => return Err(StoreError::InvitationExpired),
            _ => return Err(StoreError::StateUnavailable("unknown receipt state")),
        }

        let code_matches = receipt.verification_code.as_deref()
            == Some(confirmation.displayed_verification_code.as_str());
        let scopes_match = json_equal(
            &receipt.granted_scopes_json,
            &confirmation.displayed_scopes_json,
        )?;
        if !confirmation.approved || !code_matches || !scopes_match {
            transaction.execute(
                "UPDATE direct_enrollment_receipts
                 SET state = 'cancelled', verification_code = NULL,
                     confirmation_digest = NULL,
                     bootstrap_envelope_bytes = NULL,
                     bootstrap_envelope_digest = NULL,
                     bootstrap_response_bytes = NULL,
                     state_revision = state_revision + 1
                 WHERE receipt_id = ?1 AND state = 'pending_user_confirmation'",
                [&confirmation.receipt_id],
            )?;
            transaction.execute(
                "UPDATE direct_pairing_invitations
                 SET state = 'cancelled', state_revision = state_revision + 1
                 WHERE invitation_id = ?1 AND state = 'consumed'",
                [&receipt.invitation_id],
            )?;
            return Ok(ConfirmOutcome::Cancelled);
        }
        validate_nonempty_bytes(
            &confirmation.bootstrap_envelope_bytes,
            MAX_RESPONSE_BYTES,
            "bootstrap_envelope_bytes",
        )?;
        validate_nonempty_bytes(
            &confirmation.exact_bootstrap_response_bytes,
            MAX_RESPONSE_BYTES,
            "exact_bootstrap_response_bytes",
        )?;
        transaction.execute(
            "UPDATE direct_enrollment_receipts
             SET state = 'pending_finish', bootstrap_envelope_bytes = ?2,
                 bootstrap_envelope_digest = ?3, bootstrap_response_bytes = ?4,
                 confirmation_digest = ?5, state_revision = state_revision + 1
             WHERE receipt_id = ?1 AND state = 'pending_user_confirmation'",
            params![
                confirmation.receipt_id,
                confirmation.bootstrap_envelope_bytes,
                confirmation.bootstrap_envelope_digest.as_slice(),
                confirmation.exact_bootstrap_response_bytes,
                confirmation.confirmation_digest.as_slice(),
            ],
        )?;
        Ok(ConfirmOutcome::Confirmed(
            confirmation.exact_bootstrap_response_bytes.clone(),
        ))
    }

    /// Persist one validated ClientFinish failure and destroy the pending
    /// bootstrap envelope when the durable attempt limit is reached.
    pub fn record_finish_failure(
        transaction: &Transaction<'_>,
        receipt_id: &str,
        now_ms: i64,
    ) -> StoreResult<AttemptOutcome> {
        validate_authority_time(now_ms)?;
        validate_uuid_v7(receipt_id, "receipt_id")?;
        let receipt = receipt_row(transaction, receipt_id)?.ok_or(StoreError::ReceiptNotFound)?;
        require_fixture_profile(
            transaction,
            &receipt.library_id,
            receipt.authority_generation,
        )?;
        match receipt.state.as_str() {
            "cancelled" => return Ok(AttemptOutcome::Cancelled),
            "expired" => return Ok(AttemptOutcome::Expired),
            "revoked" => return Err(StoreError::DeviceRevoked),
            "active" => return Err(StoreError::EnrollmentAlreadyActive),
            "pending_user_confirmation" => return Err(StoreError::UserConfirmationRequired),
            "pending_finish" => {}
            _ => return Err(StoreError::StateUnavailable("unknown receipt state")),
        }
        if now_ms >= receipt.expires_at_ms {
            expire_receipt(transaction, receipt_id, &receipt.invitation_id)?;
            return Ok(AttemptOutcome::Expired);
        }
        let next = receipt
            .failed_finish_attempts
            .checked_add(1)
            .ok_or(StoreError::StateUnavailable("finish attempt overflow"))?;
        let cancelled = next >= MAX_FAILED_ATTEMPTS;
        transaction.execute(
            "UPDATE direct_enrollment_receipts
             SET failed_finish_attempts = ?2,
                 state = CASE WHEN ?3 THEN 'cancelled' ELSE state END,
                 verification_code = CASE WHEN ?3 THEN NULL ELSE verification_code END,
                 bootstrap_envelope_bytes = CASE WHEN ?3 THEN NULL ELSE bootstrap_envelope_bytes END,
                 bootstrap_envelope_digest = CASE WHEN ?3 THEN NULL ELSE bootstrap_envelope_digest END,
                 bootstrap_response_bytes = CASE WHEN ?3 THEN NULL ELSE bootstrap_response_bytes END,
                 confirmation_digest = CASE WHEN ?3 THEN NULL ELSE confirmation_digest END,
                 state_revision = state_revision + 1
             WHERE receipt_id = ?1 AND state = 'pending_finish'",
            params![receipt_id, next.min(MAX_FAILED_ATTEMPTS), cancelled],
        )?;
        if cancelled {
            transaction.execute(
                "UPDATE direct_pairing_invitations
                 SET state = 'cancelled', state_revision = state_revision + 1
                 WHERE invitation_id = ?1 AND state = 'consumed'",
                [&receipt.invitation_id],
            )?;
            Ok(AttemptOutcome::Cancelled)
        } else {
            Ok(AttemptOutcome::Recorded {
                failed_attempts: next as u8,
                attempts_remaining: (MAX_FAILED_ATTEMPTS - next) as u8,
            })
        }
    }

    pub fn activate_enrollment(
        transaction: &Transaction<'_>,
        activation: &ActivateEnrollment,
    ) -> StoreResult<ActivateOutcome> {
        validate_activation(activation)?;
        if let Some(replay) = replay_row(transaction, "client_finish", &activation.message_id)? {
            if replay.pin != activation.observed_tls_spki_sha256 {
                return Err(StoreError::PinMismatch);
            }
            if replay.digest == activation.request_digest {
                if replay.subject_id != activation.receipt_id {
                    return Err(StoreError::StateUnavailable(
                        "ClientFinish replay subject does not match",
                    ));
                }
                require_finish_replay_authorized(
                    transaction,
                    &replay.subject_id,
                    &activation.device_id,
                )?;
                return Ok(ActivateOutcome::ExactReplay(replay.response));
            }
            quarantine(
                transaction,
                "client_finish",
                &activation.message_id,
                &replay.digest,
                &activation.request_digest,
                "byte-different ClientFinish message id reuse",
                activation.authority_now_ms,
            )?;
            return Ok(ActivateOutcome::Quarantined);
        }
        enforce_replay_capacity(transaction, "client_finish", activation.authority_now_ms)?;
        let receipt =
            receipt_row(transaction, &activation.receipt_id)?.ok_or(StoreError::ReceiptNotFound)?;
        if receipt.device_id != activation.device_id
            || receipt.authority_generation != activation.authority_generation
        {
            return Err(StoreError::StateUnavailable(
                "finish binding does not match receipt",
            ));
        }
        require_fixture_profile(
            transaction,
            &receipt.library_id,
            receipt.authority_generation,
        )?;
        let invitation = invitation_row(transaction, &receipt.invitation_id)?.ok_or(
            StoreError::StateUnavailable("receipt invitation is missing"),
        )?;
        if invitation.pin != activation.observed_tls_spki_sha256 {
            return Err(StoreError::PinMismatch);
        }
        if activation.authority_now_ms >= receipt.expires_at_ms
            && matches!(
                receipt.state.as_str(),
                "pending_user_confirmation" | "pending_finish"
            )
        {
            expire_receipt(transaction, &activation.receipt_id, &receipt.invitation_id)?;
            return Ok(ActivateOutcome::Expired);
        }
        match receipt.state.as_str() {
            "pending_user_confirmation" => return Err(StoreError::UserConfirmationRequired),
            "pending_finish" => {}
            "active" => return Err(StoreError::EnrollmentAlreadyActive),
            "cancelled" | "revoked" => return Err(StoreError::EnrollmentCancelled),
            "expired" => return Err(StoreError::InvitationExpired),
            _ => return Err(StoreError::StateUnavailable("unknown receipt state")),
        }
        if invitation.state != "consumed" {
            return Err(StoreError::StateUnavailable(
                "pending receipt invitation is not consumed",
            ));
        }
        if receipt.bootstrap_response.is_none() {
            return Err(StoreError::StateUnavailable(
                "pending finish receipt is missing bootstrap response",
            ));
        }
        if transaction
            .query_row(
                "SELECT enrollment_state FROM portable_devices WHERE device_id = ?1",
                [&activation.device_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some()
        {
            return Err(StoreError::StateUnavailable(
                "device id is already registered",
            ));
        }
        let activated_at = portable_timestamp(activation.authority_now_ms)?;

        transaction.execute(
            "INSERT INTO portable_devices
             (device_id, library_id, device_kind, display_name, role,
              enrollment_state, capabilities_json, public_signing_key,
              public_encryption_key, last_transaction_counter, created_at,
              enrolled_at, revoked_at)
             VALUES (?1, ?2, 'ios', ?3, 'replica', 'active', ?4, ?5, ?6,
                     0, ?7, ?7, NULL)",
            params![
                activation.device_id,
                receipt.library_id,
                receipt.display_name,
                receipt.capabilities_json,
                receipt.signing_key,
                receipt.hpke_key,
                activated_at,
            ],
        )?;
        transaction.execute(
            "INSERT INTO direct_device_sync_state
             (device_id, library_id, authority_generation, last_seen_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                activation.device_id,
                receipt.library_id,
                i64_from_u64(receipt.authority_generation, "authority_generation")?,
                activation.authority_now_ms,
            ],
        )?;
        transaction.execute(
            "UPDATE direct_enrollment_receipts
             SET state = 'active', server_finish_bytes = ?2,
                 verification_code = NULL, activated_at_ms = ?3,
                 state_revision = state_revision + 1
             WHERE receipt_id = ?1 AND state = 'pending_finish'",
            params![
                activation.receipt_id,
                activation.exact_server_finish_bytes,
                activation.authority_now_ms,
            ],
        )?;
        transaction.execute(
            "UPDATE direct_pairing_invitations
             SET state = 'active', state_revision = state_revision + 1
             WHERE invitation_id = ?1 AND state = 'consumed'",
            [&receipt.invitation_id],
        )?;
        insert_pairing_replay(
            transaction,
            "client_finish",
            &activation.message_id,
            &activation.receipt_id,
            &activation.request_digest,
            &activation.observed_tls_spki_sha256,
            &activation.exact_server_finish_bytes,
            activation.authority_now_ms,
        )?;
        Ok(ActivateOutcome::Activated(
            activation.exact_server_finish_bytes.clone(),
        ))
    }

    pub fn revoke_device(
        transaction: &Transaction<'_>,
        device_id: &str,
        now_ms: i64,
    ) -> StoreResult<RevokeOutcome> {
        validate_authority_time(now_ms)?;
        validate_uuid_v7(device_id, "device_id")?;
        let device: Option<(String, String)> = transaction
            .query_row(
                "SELECT library_id, enrollment_state FROM portable_devices
                 WHERE device_id = ?1 AND role = 'replica'",
                [device_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((library_id, state)) = device else {
            return Err(StoreError::DeviceNotFound);
        };
        let generation = library_generation(transaction, &library_id)?;
        require_fixture_profile(transaction, &library_id, generation)?;
        if state == "revoked" {
            return Ok(RevokeOutcome::AlreadyRevoked);
        }
        if state != "active" {
            return Err(StoreError::StateUnavailable("unknown device state"));
        }
        let revoked_at = portable_timestamp(now_ms)?;
        let receipt: Option<(String, String)> = transaction
            .query_row(
                "SELECT receipt_id, invitation_id FROM direct_enrollment_receipts
                 WHERE library_id = ?1 AND device_id = ?2 AND state = 'active'",
                params![library_id, device_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((receipt_id, invitation_id)) = receipt else {
            return Err(StoreError::StateUnavailable(
                "active registry device has no active receipt",
            ));
        };
        transaction.execute(
            "UPDATE portable_devices
             SET enrollment_state = 'revoked', revoked_at = ?2
             WHERE device_id = ?1 AND enrollment_state = 'active'",
            params![device_id, revoked_at],
        )?;
        transaction.execute(
            "UPDATE direct_enrollment_receipts
             SET state = 'revoked', revoked_at_ms = ?2,
                 verification_code = NULL, state_revision = state_revision + 1
             WHERE receipt_id = ?1 AND state = 'active'",
            params![receipt_id, now_ms],
        )?;
        transaction.execute(
            "UPDATE direct_pairing_invitations
             SET state = 'revoked', state_revision = state_revision + 1
             WHERE invitation_id = ?1 AND state = 'active'",
            [invitation_id],
        )?;
        Ok(RevokeOutcome::Revoked)
    }

    /// Append a metadata-only accepted change for generated/sanitized fixtures.
    /// It intentionally cannot materialize a real record and is unsuitable for
    /// personal Notes; the later decrypt/domain adapter owns that transaction.
    pub fn append_fixture_accepted_change(
        transaction: &Transaction<'_>,
        change: &FixtureAcceptedChange,
    ) -> StoreResult<FixtureChangeOutcome> {
        validate_uuid_v7(&change.request_id, "request_id")?;
        validate_uuid_v7(&change.library_id, "library_id")?;
        validate_uuid_v7(&change.transaction_id, "transaction_id")?;
        validate_uuid_v7(&change.device_id, "device_id")?;
        validate_sha256_hex(&change.transaction_digest, "transaction_digest")?;
        validate_json(&change.transaction_json, "transaction_json")?;
        validate_json(&change.receipt_json, "receipt_json")?;
        validate_nonempty_bytes(
            &change.exact_response_bytes,
            MAX_RESPONSE_BYTES,
            "exact_response_bytes",
        )?;
        if change.authority_now_ms < 0
            || change.created_at_ms < 0
            || change.expires_at_ms <= change.created_at_ms
        {
            return Err(StoreError::InvalidInput("transaction lifetime"));
        }
        if change.authority_generation == 0 {
            return Err(StoreError::InvalidInput("authority_generation"));
        }
        require_fixture_profile(transaction, &change.library_id, change.authority_generation)?;
        let state: Option<(String, String, i64)> = transaction
            .query_row(
                "SELECT role, enrollment_state, last_transaction_counter
                 FROM portable_devices WHERE device_id = ?1 AND library_id = ?2",
                params![change.device_id, change.library_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((role, state, last_counter)) = state else {
            return Err(StoreError::DeviceNotFound);
        };
        if state == "revoked" {
            return Err(StoreError::DeviceRevoked);
        }
        if role != "replica" || state != "active" {
            return Err(StoreError::StateUnavailable(
                "direct change device is not an active replica",
            ));
        }
        let existing_replay: Option<(Vec<u8>, String, Vec<u8>)> = transaction
            .query_row(
                "SELECT request_digest, endpoint, exact_response_bytes
                 FROM direct_request_replays
                 WHERE device_id = ?1 AND request_id = ?2",
                params![change.device_id, change.request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((digest, endpoint, response)) = existing_replay {
            if digest == change.request_digest && endpoint == "/sync/v1/push" {
                return Ok(FixtureChangeOutcome::ExactReplay(response));
            }
            quarantine(
                transaction,
                "sync_request",
                &change.request_id,
                &digest,
                &change.request_digest,
                "byte-different direct sync request id reuse",
                change.authority_now_ms,
            )?;
            return Ok(FixtureChangeOutcome::Quarantined);
        }
        let recent_replays: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM direct_request_replays
             WHERE device_id = ?1 AND created_at_ms > ?2",
            params![
                change.device_id,
                change
                    .authority_now_ms
                    .saturating_sub(PAIRING_LEDGER_RATE_WINDOW_MS),
            ],
            |row| row.get(0),
        )?;
        if recent_replays >= MAX_REPLAY_ROWS {
            return Err(StoreError::ReplayLimit);
        }
        let expected = u64_from_db(last_counter, "last_transaction_counter")?
            .checked_add(1)
            .ok_or(StoreError::StateUnavailable("device counter overflow"))?;
        if expected != change.device_transaction_counter {
            return Err(StoreError::StateUnavailable("device counter is not next"));
        }
        let next_cursor: i64 = transaction.query_row(
            "UPDATE direct_authority_profiles
             SET high_water_cursor = high_water_cursor + 1,
                 state_revision = state_revision + 1, updated_at_ms = ?2
             WHERE library_id = ?1 AND environment = 'development'
               AND library_data_class = 'sanitized_fixture'
             RETURNING high_water_cursor",
            params![change.library_id, change.authority_now_ms],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE portable_devices SET last_transaction_counter = ?2
             WHERE device_id = ?1 AND enrollment_state = 'active'",
            params![
                change.device_id,
                i64_from_u64(
                    change.device_transaction_counter,
                    "device_transaction_counter"
                )?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO direct_authority_transactions
             (transaction_id, library_id, device_id, authority_generation,
              device_transaction_counter, signed_digest, transaction_json,
              state, receipt_json, accepted_cursor, created_at_ms,
              expires_at_ms, terminal_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'accepted', ?8, ?9, ?10, ?11, ?12)",
            params![
                change.transaction_id,
                change.library_id,
                change.device_id,
                i64_from_u64(change.authority_generation, "authority_generation")?,
                i64_from_u64(
                    change.device_transaction_counter,
                    "device_transaction_counter"
                )?,
                change.transaction_digest,
                change.transaction_json,
                change.receipt_json,
                next_cursor,
                change.created_at_ms,
                change.expires_at_ms,
                change.authority_now_ms,
            ],
        )?;
        transaction.execute(
            "INSERT INTO direct_authority_changes
             (library_id, sequence, transaction_id, transaction_digest, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                change.library_id,
                next_cursor,
                change.transaction_id,
                change.transaction_digest,
                change.authority_now_ms,
            ],
        )?;
        transaction.execute(
            "INSERT INTO direct_request_replays
             (device_id, request_id, endpoint, request_digest, status_code,
              exact_response_bytes, created_at_ms)
             VALUES (?1, ?2, '/sync/v1/push', ?3, 200, ?4, ?5)",
            params![
                change.device_id,
                change.request_id,
                change.request_digest.as_slice(),
                change.exact_response_bytes,
                change.authority_now_ms,
            ],
        )?;
        Ok(FixtureChangeOutcome::Accepted {
            cursor: u64_from_db(next_cursor, "high_water_cursor")?,
            exact_response_bytes: change.exact_response_bytes.clone(),
        })
    }

    pub fn issue_checkpoint(
        transaction: &Transaction<'_>,
        checkpoint: &IssueCheckpoint,
    ) -> StoreResult<CheckpointOutcome> {
        validate_checkpoint(checkpoint)?;
        require_fixture_profile(
            transaction,
            &checkpoint.library_id,
            checkpoint.authority_generation,
        )?;
        let existing: Option<(String, Vec<u8>)> = transaction
            .query_row(
                "SELECT checkpoint_digest, exact_response_bytes
                 FROM direct_sync_checkpoints
                 WHERE library_id = ?1 AND authority_generation = ?2
                   AND high_water_cursor = ?3",
                params![
                    checkpoint.library_id,
                    i64_from_u64(checkpoint.authority_generation, "authority_generation")?,
                    i64_from_u64(checkpoint.high_water_cursor, "high_water_cursor")?,
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((digest, response)) = existing {
            if digest == checkpoint.checkpoint_digest {
                return Ok(CheckpointOutcome::ExactReplay(response));
            }
            return Err(StoreError::CheckpointMismatch);
        }
        let (high_water, purge_generation, key_epoch): (i64, i64, i64) = transaction.query_row(
            "SELECT p.high_water_cursor, l.purge_generation, l.current_key_epoch
                 FROM direct_authority_profiles p
                 JOIN libraries l ON l.library_id = p.library_id
                 WHERE p.library_id = ?1",
            [&checkpoint.library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if u64_from_db(high_water, "high_water_cursor")? != checkpoint.high_water_cursor
            || u64_from_db(purge_generation, "purge_generation")? != checkpoint.purge_generation
            || u64_from_db(key_epoch, "key_epoch")? != checkpoint.key_epoch
        {
            return Err(StoreError::CheckpointMismatch);
        }
        transaction.execute(
            "INSERT INTO direct_sync_checkpoints
             (library_id, authority_generation, high_water_cursor,
              purge_generation, key_epoch, checkpoint_digest,
              exact_response_bytes, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                checkpoint.library_id,
                i64_from_u64(checkpoint.authority_generation, "authority_generation")?,
                i64_from_u64(checkpoint.high_water_cursor, "high_water_cursor")?,
                i64_from_u64(checkpoint.purge_generation, "purge_generation")?,
                i64_from_u64(checkpoint.key_epoch, "key_epoch")?,
                checkpoint.checkpoint_digest,
                checkpoint.exact_response_bytes,
                checkpoint.created_at_ms,
            ],
        )?;
        Ok(CheckpointOutcome::Issued(
            checkpoint.exact_response_bytes.clone(),
        ))
    }

    pub fn acknowledge_checkpoint(
        transaction: &Transaction<'_>,
        acknowledgement: &AcknowledgeCheckpoint,
    ) -> StoreResult<AckOutcome> {
        validate_ack(acknowledgement)?;
        require_fixture_profile(
            transaction,
            &acknowledgement.library_id,
            acknowledgement.authority_generation,
        )?;
        let state: Option<String> = transaction
            .query_row(
                "SELECT enrollment_state FROM portable_devices
                 WHERE device_id = ?1 AND library_id = ?2 AND role = 'replica'",
                params![acknowledgement.device_id, acknowledgement.library_id],
                |row| row.get(0),
            )
            .optional()?;
        match state.as_deref() {
            Some("active") => {}
            Some("revoked") => return Err(StoreError::DeviceRevoked),
            Some(_) => return Err(StoreError::StateUnavailable("unknown device state")),
            None => return Err(StoreError::DeviceNotFound),
        }
        let issued: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM direct_sync_checkpoints
               WHERE library_id = ?1 AND authority_generation = ?2
                 AND high_water_cursor = ?3 AND checkpoint_digest = ?4
             )",
            params![
                acknowledgement.library_id,
                i64_from_u64(acknowledgement.authority_generation, "authority_generation")?,
                i64_from_u64(acknowledgement.high_water_cursor, "high_water_cursor")?,
                acknowledgement.checkpoint_digest,
            ],
            |row| row.get(0),
        )?;
        if !issued {
            return Err(StoreError::AckMismatch);
        }
        let existing: Option<AcknowledgementStateRow> = transaction
            .query_row(
                "SELECT acknowledged_cursor, checkpoint_digest, last_ack_response_bytes
                 FROM direct_device_sync_state WHERE device_id = ?1",
                [&acknowledgement.device_id],
                |row| {
                    Ok(AcknowledgementStateRow {
                        acknowledged_cursor: row.get(0)?,
                        checkpoint_digest: row.get(1)?,
                        exact_response_bytes: row.get(2)?,
                    })
                },
            )
            .optional()?;
        let Some(AcknowledgementStateRow {
            acknowledged_cursor: cursor,
            checkpoint_digest: digest,
            exact_response_bytes: response,
        }) = existing
        else {
            return Err(StoreError::StateUnavailable(
                "active direct device is missing sync state",
            ));
        };
        if let Some(cursor) = cursor {
            let stored_cursor = u64_from_db(cursor, "acknowledged_cursor")?;
            if stored_cursor == acknowledgement.high_water_cursor
                && digest.as_deref() == Some(acknowledgement.checkpoint_digest.as_str())
            {
                return response
                    .map(AckOutcome::ExactReplay)
                    .ok_or(StoreError::StateUnavailable(
                        "acknowledgement replay response is missing",
                    ));
            }
            if acknowledgement.high_water_cursor <= stored_cursor {
                return Err(StoreError::AckMismatch);
            }
        }
        transaction.execute(
            "UPDATE direct_device_sync_state
             SET authority_generation = ?2, acknowledged_cursor = ?3,
                 checkpoint_digest = ?4, last_ack_response_bytes = ?5,
                 acknowledged_at_ms = ?6, last_seen_at_ms = ?6
             WHERE device_id = ?1",
            params![
                acknowledgement.device_id,
                i64_from_u64(acknowledgement.authority_generation, "authority_generation")?,
                i64_from_u64(acknowledgement.high_water_cursor, "high_water_cursor")?,
                acknowledgement.checkpoint_digest,
                acknowledgement.exact_response_bytes,
                acknowledgement.authority_now_ms,
            ],
        )?;
        Ok(AckOutcome::Recorded(
            acknowledgement.exact_response_bytes.clone(),
        ))
    }

    pub fn verify_schema(connection: &Connection) -> StoreResult<()> {
        for table in [
            "direct_authority_profiles",
            "direct_pairing_invitations",
            "direct_enrollment_receipts",
            "direct_pairing_replays",
            "direct_pairing_quarantine",
            "direct_authority_transactions",
            "direct_authority_mutations",
            "direct_authority_changes",
            "direct_sync_checkpoints",
            "direct_device_sync_state",
            "direct_request_replays",
        ] {
            require_schema_object(connection, "table", table)?;
        }
        for trigger in [
            "direct_invitation_state_guard",
            "direct_receipt_state_guard",
            "direct_terminal_transaction_guard",
            "direct_pairing_replays_no_update",
            "direct_pairing_replays_no_delete",
            "direct_pairing_quarantine_no_update",
            "direct_pairing_quarantine_no_delete",
            "direct_authority_mutations_no_update",
            "direct_authority_mutations_no_delete",
            "direct_authority_changes_no_update",
            "direct_authority_changes_no_delete",
            "direct_sync_checkpoints_no_update",
            "direct_sync_checkpoints_no_delete",
            "direct_request_replays_no_update",
            "direct_request_replays_no_delete",
        ] {
            require_schema_object(connection, "trigger", trigger)?;
        }
        for index in [
            "direct_pairing_invitations_library_state",
            "direct_pairing_replays_created",
            "direct_pairing_quarantine_created",
            "direct_enrollment_receipts_live_device",
            "direct_enrollment_receipts_library_state",
            "direct_request_replays_created",
        ] {
            require_schema_object(connection, "index", index)?;
        }
        if connection
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some()
        {
            return Err(StoreError::StateUnavailable("foreign key check failed"));
        }
        let non_fixture_profiles: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_authority_profiles
             WHERE environment != 'development'
                OR library_data_class != 'sanitized_fixture'",
            [],
            |row| row.get(0),
        )?;
        if non_fixture_profiles != 0 {
            return Err(StoreError::FixtureOnly);
        }
        let non_development_invitations: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_pairing_invitations
             WHERE environment != 'development'",
            [],
            |row| row.get(0),
        )?;
        if non_development_invitations != 0 {
            return Err(StoreError::FixtureOnly);
        }
        verify_json_column(connection, "direct_authority_profiles", "capabilities_json")?;
        verify_json_column(
            connection,
            "direct_pairing_invitations",
            "scope_ceiling_json",
        )?;
        for column in ["receipt_json", "granted_scopes_json", "capabilities_json"] {
            verify_json_column(connection, "direct_enrollment_receipts", column)?;
        }
        verify_json_column(
            connection,
            "direct_authority_transactions",
            "transaction_json",
        )?;
        verify_nullable_json_column(connection, "direct_authority_transactions", "receipt_json")?;
        verify_json_column(connection, "direct_authority_mutations", "envelope_json")?;

        let inconsistent_enrollments: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_enrollment_receipts r
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             LEFT JOIN portable_devices d ON d.device_id = r.device_id
             WHERE r.library_id != i.library_id
                OR r.authority_generation != i.authority_generation
                OR (r.state = 'pending_user_confirmation' AND i.state != 'consumed')
                OR (r.state = 'pending_finish' AND (i.state != 'consumed'
                    OR r.bootstrap_response_bytes IS NULL))
                OR (r.state = 'active' AND (i.state != 'active'
                    OR d.device_id IS NULL OR d.enrollment_state != 'active'
                    OR d.role != 'replica'
                    OR r.server_finish_bytes IS NULL))
                OR (r.state = 'revoked' AND (i.state != 'revoked'
                    OR d.device_id IS NULL OR d.enrollment_state != 'revoked'))",
            [],
            |row| row.get(0),
        )?;
        if inconsistent_enrollments != 0 {
            return Err(StoreError::StateUnavailable(
                "enrollment cross-links are inconsistent",
            ));
        }
        let orphan_consumed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_pairing_invitations i
             LEFT JOIN direct_enrollment_receipts r ON r.invitation_id = i.invitation_id
             WHERE i.state IN ('consumed', 'active', 'revoked') AND r.receipt_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if orphan_consumed != 0 {
            return Err(StoreError::StateUnavailable(
                "consumed invitation is missing its receipt",
            ));
        }
        let bad_cursor: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_authority_profiles p
             WHERE p.high_water_cursor != (
               SELECT COUNT(*) FROM direct_authority_changes c
               WHERE c.library_id = p.library_id
             ) OR p.high_water_cursor != COALESCE((
               SELECT MAX(c.sequence) FROM direct_authority_changes c
               WHERE c.library_id = p.library_id
             ), 0)",
            [],
            |row| row.get(0),
        )?;
        if bad_cursor != 0 {
            return Err(StoreError::StateUnavailable(
                "accepted change cursor is not contiguous",
            ));
        }
        let bad_changes: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_authority_changes c
             JOIN direct_authority_transactions t ON t.transaction_id = c.transaction_id
             WHERE t.state != 'accepted' OR t.accepted_cursor != c.sequence
                OR t.library_id != c.library_id OR t.signed_digest != c.transaction_digest",
            [],
            |row| row.get(0),
        )?;
        if bad_changes != 0 {
            return Err(StoreError::StateUnavailable(
                "accepted change binding is inconsistent",
            ));
        }
        let bad_transactions: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_authority_transactions t
             JOIN portable_devices d ON d.device_id = t.device_id
             JOIN libraries l ON l.library_id = t.library_id
             WHERE d.library_id != t.library_id OR d.role != 'replica'
                OR t.authority_generation > l.authority_generation",
            [],
            |row| row.get(0),
        )?;
        if bad_transactions != 0 {
            return Err(StoreError::StateUnavailable(
                "direct transaction binding is inconsistent",
            ));
        }
        let bad_checkpoints: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_sync_checkpoints c
             JOIN direct_authority_profiles p ON p.library_id = c.library_id
             JOIN libraries l ON l.library_id = c.library_id
             WHERE c.high_water_cursor > p.high_water_cursor
                OR c.authority_generation > l.authority_generation
                OR c.purge_generation > l.purge_generation
                OR c.key_epoch > l.current_key_epoch",
            [],
            |row| row.get(0),
        )?;
        if bad_checkpoints != 0 {
            return Err(StoreError::StateUnavailable(
                "issued checkpoint binding is inconsistent",
            ));
        }
        let bad_device_sync_state: i64 = connection.query_row(
            "SELECT COUNT(*) FROM direct_device_sync_state s
             JOIN portable_devices d ON d.device_id = s.device_id
             JOIN libraries l ON l.library_id = s.library_id
             WHERE s.library_id != d.library_id OR d.role != 'replica'
                OR s.authority_generation > l.authority_generation",
            [],
            |row| row.get(0),
        )?;
        if bad_device_sync_state != 0 {
            return Err(StoreError::StateUnavailable(
                "device sync state binding is inconsistent",
            ));
        }
        Ok(())
    }
}

fn validate_invitation(invitation: &NewInvitation, now_ms: i64) -> StoreResult<()> {
    validate_authority_time(now_ms)?;
    validate_uuid_v7(&invitation.invitation_id, "invitation_id")?;
    validate_uuid_v7(&invitation.library_id, "library_id")?;
    validate_json(&invitation.scope_ceiling_json, "scope_ceiling_json")?;
    if invitation.authority_generation == 0
        || invitation.created_at_ms > now_ms
        || invitation.expires_at_ms <= now_ms
        || invitation.expires_at_ms <= invitation.created_at_ms
        || invitation.expires_at_ms - invitation.created_at_ms > MAX_INVITATION_LIFETIME_MS
    {
        return Err(StoreError::InvalidInput("invitation lifetime"));
    }
    Ok(())
}

fn validate_consume(request: &ConsumeInvitation) -> StoreResult<()> {
    validate_authority_time(request.authority_now_ms)?;
    for (value, field) in [
        (&request.message_id, "message_id"),
        (&request.invitation_id, "invitation_id"),
        (&request.receipt_id, "receipt_id"),
        (&request.device_id, "device_id"),
    ] {
        validate_uuid_v7(value, field)?;
    }
    validate_text(&request.display_name, 128, "display_name")?;
    validate_text(&request.app_version, 64, "app_version")?;
    validate_text(&request.build_version, 64, "build_version")?;
    validate_text(&request.verification_code, 16, "verification_code")?;
    for (json, field) in [
        (&request.receipt_json, "receipt_json"),
        (&request.granted_scopes_json, "granted_scopes_json"),
        (&request.capabilities_json, "capabilities_json"),
    ] {
        validate_json(json, field)?;
    }
    validate_nonempty_bytes(
        &request.exact_begin_response_bytes,
        MAX_RESPONSE_BYTES,
        "exact_begin_response_bytes",
    )
}

fn validate_activation(activation: &ActivateEnrollment) -> StoreResult<()> {
    validate_authority_time(activation.authority_now_ms)?;
    for (value, field) in [
        (&activation.message_id, "message_id"),
        (&activation.receipt_id, "receipt_id"),
        (&activation.device_id, "device_id"),
    ] {
        validate_uuid_v7(value, field)?;
    }
    if activation.authority_generation == 0 {
        return Err(StoreError::InvalidInput("authority_generation"));
    }
    validate_nonempty_bytes(
        &activation.exact_server_finish_bytes,
        MAX_RESPONSE_BYTES,
        "exact_server_finish_bytes",
    )
}

fn validate_checkpoint(checkpoint: &IssueCheckpoint) -> StoreResult<()> {
    validate_authority_time(checkpoint.created_at_ms)?;
    validate_uuid_v7(&checkpoint.library_id, "library_id")?;
    validate_sha256_hex(&checkpoint.checkpoint_digest, "checkpoint_digest")?;
    if checkpoint.authority_generation == 0 {
        return Err(StoreError::InvalidInput("authority_generation"));
    }
    if checkpoint.key_epoch == 0 {
        return Err(StoreError::InvalidInput("key_epoch"));
    }
    validate_nonempty_bytes(
        &checkpoint.exact_response_bytes,
        MAX_RESPONSE_BYTES,
        "exact_response_bytes",
    )
}

fn validate_ack(acknowledgement: &AcknowledgeCheckpoint) -> StoreResult<()> {
    validate_authority_time(acknowledgement.authority_now_ms)?;
    validate_uuid_v7(&acknowledgement.library_id, "library_id")?;
    validate_uuid_v7(&acknowledgement.device_id, "device_id")?;
    validate_sha256_hex(&acknowledgement.checkpoint_digest, "checkpoint_digest")?;
    if acknowledgement.authority_generation == 0 {
        return Err(StoreError::InvalidInput("authority_generation"));
    }
    validate_nonempty_bytes(
        &acknowledgement.exact_response_bytes,
        MAX_RESPONSE_BYTES,
        "exact_response_bytes",
    )
}

fn validate_authority_time(authority_now_ms: i64) -> StoreResult<()> {
    if authority_now_ms < 0 {
        Err(StoreError::InvalidInput("authority time"))
    } else {
        Ok(())
    }
}

fn require_fixture_profile(
    transaction: &Transaction<'_>,
    library_id: &str,
    authority_generation: u64,
) -> StoreResult<()> {
    let row: Option<(String, String, String, i64)> = transaction
        .query_row(
            "SELECT p.environment, p.library_data_class, p.readiness_state,
                    l.authority_generation
             FROM direct_authority_profiles p
             JOIN libraries l ON l.library_id = p.library_id
             WHERE p.library_id = ?1",
            [library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((environment, data_class, readiness, generation)) = row else {
        return Err(StoreError::StateUnavailable(
            "fixture authority is not initialized",
        ));
    };
    if environment != DEVELOPMENT || data_class != SANITIZED_FIXTURE {
        return Err(StoreError::FixtureOnly);
    }
    if readiness != "fixture_ready" {
        return Err(StoreError::StateUnavailable(
            "fixture authority is not ready",
        ));
    }
    if u64_from_db(generation, "authority_generation")? != authority_generation {
        return Err(StoreError::StateUnavailable("authority generation changed"));
    }
    Ok(())
}

fn library_generation(transaction: &Transaction<'_>, library_id: &str) -> StoreResult<u64> {
    let generation: i64 = transaction
        .query_row(
            "SELECT authority_generation FROM libraries WHERE library_id = ?1",
            [library_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StoreError::StateUnavailable("portable library is missing"))?;
    u64_from_db(generation, "authority_generation")
}

fn require_hello_replay_authorized(
    transaction: &Transaction<'_>,
    receipt_id: &str,
    now_ms: i64,
) -> StoreResult<()> {
    let receipt = receipt_row(transaction, receipt_id)?.ok_or(StoreError::ReceiptNotFound)?;
    require_fixture_profile(
        transaction,
        &receipt.library_id,
        receipt.authority_generation,
    )?;
    let invitation = invitation_row(transaction, &receipt.invitation_id)?.ok_or(
        StoreError::StateUnavailable("receipt invitation is missing"),
    )?;
    if now_ms >= receipt.expires_at_ms
        && matches!(
            receipt.state.as_str(),
            "pending_user_confirmation" | "pending_finish"
        )
    {
        return Err(StoreError::InvitationExpired);
    }
    match (receipt.state.as_str(), invitation.state.as_str()) {
        ("pending_user_confirmation" | "pending_finish", "consumed") | ("active", "active") => {
            Ok(())
        }
        ("revoked", _) | (_, "revoked") => Err(StoreError::DeviceRevoked),
        ("cancelled", _) | (_, "cancelled") => Err(StoreError::EnrollmentCancelled),
        ("expired", _) | (_, "expired") => Err(StoreError::InvitationExpired),
        _ => Err(StoreError::StateUnavailable(
            "ClientHello replay state is inconsistent",
        )),
    }
}

fn require_finish_replay_authorized(
    transaction: &Transaction<'_>,
    receipt_id: &str,
    device_id: &str,
) -> StoreResult<()> {
    let receipt = receipt_row(transaction, receipt_id)?.ok_or(StoreError::ReceiptNotFound)?;
    if receipt.device_id != device_id {
        return Err(StoreError::StateUnavailable(
            "ClientFinish replay device does not match",
        ));
    }
    require_fixture_profile(
        transaction,
        &receipt.library_id,
        receipt.authority_generation,
    )?;
    let invitation = invitation_row(transaction, &receipt.invitation_id)?.ok_or(
        StoreError::StateUnavailable("receipt invitation is missing"),
    )?;
    match (receipt.state.as_str(), invitation.state.as_str()) {
        ("revoked", _) | (_, "revoked") => return Err(StoreError::DeviceRevoked),
        ("cancelled", _) | (_, "cancelled") => return Err(StoreError::EnrollmentCancelled),
        ("expired", _) | (_, "expired") => return Err(StoreError::InvitationExpired),
        ("active", "active") => {}
        _ => {
            return Err(StoreError::StateUnavailable(
                "ClientFinish replay state is inconsistent",
            ))
        }
    }
    let device_state: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT library_id, role, enrollment_state
             FROM portable_devices WHERE device_id = ?1",
            [device_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match device_state {
        Some((library_id, role, state))
            if library_id == receipt.library_id && role == "replica" && state == "active" =>
        {
            Ok(())
        }
        Some((_, _, state)) if state == "revoked" => Err(StoreError::DeviceRevoked),
        Some(_) => Err(StoreError::StateUnavailable(
            "ClientFinish replay device state is inconsistent",
        )),
        None => Err(StoreError::DeviceNotFound),
    }
}

fn replay_row(
    transaction: &Transaction<'_>,
    kind: &str,
    message_id: &str,
) -> StoreResult<Option<ReplayRow>> {
    Ok(transaction
        .query_row(
            "SELECT subject_id, request_digest, tls_spki_sha256, exact_response_bytes
             FROM direct_pairing_replays
             WHERE message_kind = ?1 AND message_id = ?2",
            params![kind, message_id],
            |row| {
                Ok(ReplayRow {
                    subject_id: row.get(0)?,
                    digest: row.get(1)?,
                    pin: row.get(2)?,
                    response: row.get(3)?,
                })
            },
        )
        .optional()?)
}

fn invitation_row(
    transaction: &Transaction<'_>,
    invitation_id: &str,
) -> StoreResult<Option<InvitationRow>> {
    transaction
        .query_row(
            "SELECT library_id, authority_generation, tls_spki_sha256,
                    expires_at_ms, state
             FROM direct_pairing_invitations WHERE invitation_id = ?1",
            [invitation_id],
            |row| {
                let generation: i64 = row.get(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    generation,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .map(|(library_id, generation, pin, expires_at_ms, state)| {
            Ok::<InvitationRow, StoreError>(InvitationRow {
                library_id,
                authority_generation: u64_from_db(generation, "authority_generation")?,
                pin,
                expires_at_ms,
                state,
            })
        })
        .transpose()
}

fn receipt_row(transaction: &Transaction<'_>, receipt_id: &str) -> StoreResult<Option<ReceiptRow>> {
    transaction
        .query_row(
            "SELECT invitation_id, library_id, device_id, display_name,
                    authority_generation,
                    granted_scopes_json, capabilities_json,
                    client_signing_public_key, client_hpke_public_key,
                    verification_code, confirmation_digest,
                    bootstrap_response_bytes, failed_finish_attempts,
                    expires_at_ms, state
             FROM direct_enrollment_receipts WHERE receipt_id = ?1",
            [receipt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<Vec<u8>>>(10)?,
                    row.get::<_, Option<Vec<u8>>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, String>(14)?,
                ))
            },
        )
        .optional()?
        .map(
            |(
                invitation_id,
                library_id,
                device_id,
                display_name,
                generation,
                granted_scopes_json,
                capabilities_json,
                signing_key,
                hpke_key,
                verification_code,
                confirmation_digest,
                bootstrap_response,
                failed_finish_attempts,
                expires_at_ms,
                state,
            )| {
                Ok::<ReceiptRow, StoreError>(ReceiptRow {
                    invitation_id,
                    library_id,
                    device_id,
                    display_name,
                    authority_generation: u64_from_db(generation, "authority_generation")?,
                    granted_scopes_json,
                    capabilities_json,
                    signing_key,
                    hpke_key,
                    verification_code,
                    confirmation_digest,
                    bootstrap_response,
                    failed_finish_attempts,
                    expires_at_ms,
                    state,
                })
            },
        )
        .transpose()
}

fn enforce_replay_capacity(
    transaction: &Transaction<'_>,
    kind: &str,
    now_ms: i64,
) -> StoreResult<()> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM direct_pairing_replays
         WHERE message_kind = ?1 AND created_at_ms > ?2",
        params![kind, now_ms.saturating_sub(PAIRING_LEDGER_RATE_WINDOW_MS)],
        |row| row.get(0),
    )?;
    if count >= MAX_REPLAY_ROWS {
        Err(StoreError::ReplayLimit)
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_pairing_replay(
    transaction: &Transaction<'_>,
    kind: &str,
    message_id: &str,
    subject_id: &str,
    request_digest: &[u8; 32],
    pin: &[u8; 32],
    response: &[u8],
    now_ms: i64,
) -> StoreResult<()> {
    transaction.execute(
        "INSERT INTO direct_pairing_replays
         (message_kind, message_id, subject_id, request_digest,
          tls_spki_sha256, exact_response_bytes, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            kind,
            message_id,
            subject_id,
            request_digest.as_slice(),
            pin.as_slice(),
            response,
            now_ms,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn quarantine(
    transaction: &Transaction<'_>,
    kind: &str,
    identifier: &str,
    accepted: &[u8],
    observed: &[u8],
    reason: &str,
    now_ms: i64,
) -> StoreResult<()> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM direct_pairing_quarantine
         WHERE quarantined_at_ms > ?1",
        [now_ms.saturating_sub(PAIRING_LEDGER_RATE_WINDOW_MS)],
        |row| row.get(0),
    )?;
    if count >= MAX_QUARANTINE_ROWS {
        return Err(StoreError::QuarantineLimit);
    }
    transaction.execute(
        "INSERT INTO direct_pairing_quarantine
         (identifier_kind, identifier, accepted_digest, observed_digest,
          reason, quarantined_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind, identifier, accepted, observed, reason, now_ms],
    )?;
    Ok(())
}

fn expire_receipt(
    transaction: &Transaction<'_>,
    receipt_id: &str,
    invitation_id: &str,
) -> StoreResult<()> {
    transaction.execute(
        "UPDATE direct_enrollment_receipts
         SET state = 'expired', verification_code = NULL,
             confirmation_digest = NULL,
             bootstrap_envelope_bytes = NULL,
             bootstrap_envelope_digest = NULL,
             bootstrap_response_bytes = NULL,
             state_revision = state_revision + 1
         WHERE receipt_id = ?1 AND state IN ('pending_user_confirmation', 'pending_finish')",
        [receipt_id],
    )?;
    transaction.execute(
        "UPDATE direct_pairing_invitations
         SET state = 'expired', state_revision = state_revision + 1
         WHERE invitation_id = ?1 AND state IN ('pending', 'consumed')",
        [invitation_id],
    )?;
    Ok(())
}

fn expire_stale_device_receipt(
    transaction: &Transaction<'_>,
    library_id: &str,
    device_id: &str,
    authority_now_ms: i64,
) -> StoreResult<()> {
    let stale: Option<(String, String)> = transaction
        .query_row(
            "SELECT receipt_id, invitation_id
             FROM direct_enrollment_receipts
             WHERE library_id = ?1 AND device_id = ?2
               AND state IN ('pending_user_confirmation', 'pending_finish')
               AND expires_at_ms <= ?3",
            params![library_id, device_id, authority_now_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((receipt_id, invitation_id)) = stale {
        expire_receipt(transaction, &receipt_id, &invitation_id)?;
    }
    Ok(())
}

fn require_schema_object(connection: &Connection, kind: &str, name: &str) -> StoreResult<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_schema WHERE type = ?1 AND name = ?2
         )",
        params![kind, name],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StoreError::StateUnavailable(
            "direct authority schema is incomplete",
        ))
    }
}

fn verify_json_column(connection: &Connection, table: &str, column: &str) -> StoreResult<()> {
    let sql = format!("SELECT {column} FROM {table}");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        validate_json(&row?, "stored_json")?;
    }
    Ok(())
}

fn verify_nullable_json_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> StoreResult<()> {
    let sql = format!("SELECT {column} FROM {table} WHERE {column} IS NOT NULL");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        validate_json(&row?, "stored_json")?;
    }
    Ok(())
}

fn validate_json(raw: &str, field: &'static str) -> StoreResult<()> {
    if raw.is_empty() || raw.len() > MAX_JSON_BYTES {
        return Err(StoreError::InvalidInput(field));
    }
    serde_json::from_str::<Value>(raw)
        .map(|_| ())
        .map_err(|_| StoreError::InvalidInput(field))
}

fn json_equal(left: &str, right: &str) -> StoreResult<bool> {
    let left: Value = serde_json::from_str(left).map_err(|_| StoreError::InvalidInput("json"))?;
    let right: Value = serde_json::from_str(right).map_err(|_| StoreError::InvalidInput("json"))?;
    Ok(left == right)
}

fn validate_text(value: &str, maximum: usize, field: &'static str) -> StoreResult<()> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        Err(StoreError::InvalidInput(field))
    } else {
        Ok(())
    }
}

fn validate_nonempty_bytes(value: &[u8], maximum: usize, field: &'static str) -> StoreResult<()> {
    if value.is_empty() || value.len() > maximum {
        Err(StoreError::InvalidInput(field))
    } else {
        Ok(())
    }
}

fn validate_sha256_hex(value: &str, field: &'static str) -> StoreResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(StoreError::InvalidInput(field))
    }
}

fn portable_timestamp(timestamp_ms: i64) -> StoreResult<String> {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(StoreError::InvalidInput("timestamp_ms"))
}

fn validate_uuid_v7(value: &str, field: &'static str) -> StoreResult<()> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[14] == b'7'
        && bytes[18] == b'-'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit());
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidInput(field))
    }
}

fn i64_from_u64(value: u64, field: &'static str) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| StoreError::InvalidInput(field))
}

fn u64_from_db(value: i64, field: &'static str) -> StoreResult<u64> {
    u64::try_from(value).map_err(|_| StoreError::StateUnavailable(field))
}
