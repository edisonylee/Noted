//! Fixture-only SQLite authority adapter for direct sync.
//!
//! [`DirectSyncAuthority`] exposes deterministic data operations, while the
//! route-facing exact-wire trait reserves push/ack work, lets the service sign
//! the candidate, and then atomically commits semantic state with those exact
//! response bytes. Reopening this adapter therefore preserves authorization,
//! pending work, and byte-identical replay without a volatile pairing ledger.
//!
//! There is deliberately no listener, personal-data constructor, key access,
//! or production-mode switch here.  The only constructor verifies an existing
//! sanitized-development v3 authority database and the portable Notes schema.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::direct_authority_store::{
    AckOutcome, AcknowledgeCheckpoint, CheckpointOutcome, DirectAuthorityStore, IssueCheckpoint,
    RevokeOutcome, StoreError, EXACT_RESPONSE_REPLAY_RETENTION_MS,
};
use crate::direct_sync::{
    AckReceipt, AuthorityIdentity, AuthorityStoreError, DirectSyncAuthority, DirectSyncEnrollment,
    EnrollmentAuthorizationError, ExactWireDirectSyncAuthority, ExactWireResponse,
    PrepareAckResponseOutcome, PreparePushResponseOutcome, PreparedAckResponse,
    PreparedPushResponse, SyncCheckpoint,
};
use crate::pairing_protocol::{Environment, KindCapability, LibraryDataClass, RecordKind};
use crate::portable::{
    canonical_json, canonical_sha256, is_uuid_v7, AuthorityKind, ContextRecordV1, LifecycleState,
    ScopeClass,
};
use crate::sync_protocol::{
    negotiate_capabilities, AcceptedChange, AcceptedHead, BootstrapRecord, BootstrapSnapshot,
    ChangePage, HeadAdvance, HeadConflict, MutationEnvelope, ProtocolCapabilities, ProtocolError,
    ReceiptDisposition, SignedTransaction, SubmitOutcome, TerminalRejection, TransactionReceipt,
    BOOTSTRAP_SNAPSHOT_VERSION, MAX_PULL_PAGE_CHANGES,
};

const FIXTURE_CIPHERTEXT_PREFIX: &[u8] = b"fixture-json:";
const DIRECT_PUSH_ENDPOINT: &str = "/sync/v1/push";
const DIRECT_ACK_ENDPOINT: &str = "/sync/v1/ack";
const MAX_EXACT_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_REQUEST_REPLAYS_PER_DEVICE: i64 = 128;
const DIRECT_FIXTURE_SOURCE: &str = "direct_fixture_sync";

#[allow(clippy::result_unit_err)]
pub trait FixtureAuthorityClock: Send + Sync + 'static {
    fn now_ms(&self) -> Result<i64, ()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableAuthorityError {
    Protocol(ProtocolError),
    Database(String),
    FixtureOnly,
    InvalidInput(&'static str),
    StateUnavailable(&'static str),
    StateChanged,
    RequestReplayMismatch,
    ResponseTooLarge,
    AckMismatch,
}

impl From<rusqlite::Error> for DurableAuthorityError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<ProtocolError> for DurableAuthorityError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

impl From<StoreError> for DurableAuthorityError {
    fn from(value: StoreError) -> Self {
        match value {
            StoreError::FixtureOnly => Self::FixtureOnly,
            StoreError::DeviceNotFound => Self::Protocol(ProtocolError::DeviceUnknown),
            StoreError::DeviceRevoked => Self::Protocol(ProtocolError::DeviceRevoked),
            StoreError::AckMismatch | StoreError::CheckpointMismatch => Self::AckMismatch,
            StoreError::InvalidInput(field) => Self::InvalidInput(field),
            StoreError::Database(error) => Self::Database(error),
            _ => Self::StateUnavailable("direct authority store rejected the transition"),
        }
    }
}

impl From<DurableAuthorityError> for AuthorityStoreError {
    fn from(value: DurableAuthorityError) -> Self {
        match value {
            DurableAuthorityError::Protocol(error) => Self::Protocol(error),
            DurableAuthorityError::AckMismatch => Self::AckMismatch,
            _ => Self::StateUnavailable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedPush {
    request_id: String,
    request_digest: [u8; 32],
    transaction_id: String,
    transaction_digest: String,
    receipt: TransactionReceipt,
    basis_digest: String,
    terminal_transaction: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DurablePreparedAck {
    request_id: String,
    request_digest: [u8; 32],
    device_id: String,
    high_water_cursor: u64,
    checkpoint_digest: String,
    receipt: AckReceipt,
}

impl PreparedPush {
    pub fn receipt(&self) -> &TransactionReceipt {
        &self.receipt
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedPush {
    pub receipt: TransactionReceipt,
    pub status_code: u16,
    pub exact_response_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparePushOutcome {
    NeedsFinalization(PreparedPush),
    ExactReplay(FinalizedPush),
}

#[derive(Debug, Clone)]
struct Profile {
    library_id: String,
    authority_generation: u64,
    purge_generation: u64,
    key_epoch: u64,
    high_water_cursor: u64,
    state_revision: u64,
    capabilities: ProtocolCapabilities,
}

#[derive(Debug, Clone)]
struct StoredTransaction {
    transaction: SignedTransaction,
    signed_digest: String,
    state: String,
    receipt: Option<TransactionReceipt>,
}

#[derive(Clone)]
pub struct SqliteDirectSyncAuthority {
    database_path: PathBuf,
    library_id: String,
    clock: Arc<dyn FixtureAuthorityClock>,
}

impl SqliteDirectSyncAuthority {
    pub fn open_sanitized_fixture(
        database_path: impl AsRef<Path>,
        library_id: &str,
        clock: Arc<dyn FixtureAuthorityClock>,
    ) -> Result<Self, DurableAuthorityError> {
        if !is_uuid_v7(library_id) {
            return Err(DurableAuthorityError::InvalidInput("library_id"));
        }
        let adapter = Self {
            database_path: database_path.as_ref().to_path_buf(),
            library_id: library_id.to_owned(),
            clock,
        };
        let connection = adapter.open_connection()?;
        DirectAuthorityStore::verify_schema(&connection)?;
        require_portable_notes_schema(&connection)?;
        let profile = load_profile(&connection, &adapter.library_id)?;
        require_notes_capabilities(&profile.capabilities)?;
        Ok(adapter)
    }

    /// Reserve and validate a push in one immediate transaction.  Prepared
    /// rows are invisible to pull/bootstrap and are safe to rediscover after a
    /// process restart.  `authority_now` must come from the trusted service
    /// clock, never from the signed transaction.
    pub fn prepare_push(
        &self,
        request_id: &str,
        request_digest: [u8; 32],
        transaction: SignedTransaction,
        authority_now: u64,
    ) -> Result<PreparePushOutcome, DurableAuthorityError> {
        if !is_uuid_v7(request_id) {
            return Err(DurableAuthorityError::InvalidInput("request_id"));
        }
        let now_ms = i64_from_u64(authority_now, "authority_now")?;
        self.write(|database| {
            require_active_device(database, &self.library_id, &transaction.manifest.device_id)?;
            if let Some(replay) =
                load_request_replay(database, &transaction.manifest.device_id, request_id)?
            {
                if replay.request_digest != request_digest
                    || replay.endpoint != DIRECT_PUSH_ENDPOINT
                {
                    return Err(DurableAuthorityError::RequestReplayMismatch);
                }
                let stored = load_transaction(database, &transaction.manifest.transaction_id)?
                    .ok_or(DurableAuthorityError::StateUnavailable(
                        "request replay has no transaction",
                    ))?;
                if stored.signed_digest != transaction.signed_digest() || stored.state == "prepared"
                {
                    return Err(DurableAuthorityError::RequestReplayMismatch);
                }
                let receipt = stored
                    .receipt
                    .ok_or(DurableAuthorityError::StateUnavailable(
                        "terminal transaction has no receipt",
                    ))?;
                return Ok(PreparePushOutcome::ExactReplay(FinalizedPush {
                    receipt,
                    status_code: replay.status_code,
                    exact_response_bytes: replay.exact_response_bytes,
                }));
            }

            let (stored, terminal_transaction) = ensure_prepared_transaction(
                database,
                &self.library_id,
                transaction,
                authority_now,
                now_ms,
            )?;
            if terminal_transaction {
                let receipt = stored
                    .receipt
                    .ok_or(DurableAuthorityError::StateUnavailable(
                        "terminal transaction has no receipt",
                    ))?;
                return Ok(PreparePushOutcome::NeedsFinalization(PreparedPush {
                    request_id: request_id.to_owned(),
                    request_digest,
                    transaction_id: stored.transaction.manifest.transaction_id.clone(),
                    transaction_digest: stored.signed_digest,
                    basis_digest: terminal_basis_digest(&receipt),
                    receipt,
                    terminal_transaction: true,
                }));
            }

            let profile = load_profile(database, &self.library_id)?;
            let receipt =
                candidate_receipt(database, &profile, &stored.transaction, authority_now)?;
            let basis_digest = head_basis_digest(database, &profile)?;
            Ok(PreparePushOutcome::NeedsFinalization(PreparedPush {
                request_id: request_id.to_owned(),
                request_digest,
                transaction_id: stored.transaction.manifest.transaction_id.clone(),
                transaction_digest: stored.signed_digest,
                receipt,
                basis_digest,
                terminal_transaction: false,
            }))
        })
    }

    /// Atomically commits a prepared terminal outcome and the exact signed
    /// response bytes.  The caller may emit the returned bytes only after this
    /// method succeeds.  A changed head/generation basis returns `StateChanged`
    /// so the caller can prepare and sign a fresh candidate without ever having
    /// exposed the stale one.
    pub fn finalize_push(
        &self,
        prepared: &PreparedPush,
        status_code: u16,
        exact_response_bytes: &[u8],
        authority_now: u64,
    ) -> Result<FinalizedPush, DurableAuthorityError> {
        if !(100..=599).contains(&status_code) {
            return Err(DurableAuthorityError::InvalidInput("status_code"));
        }
        if exact_response_bytes.is_empty() || exact_response_bytes.len() > MAX_EXACT_RESPONSE_BYTES
        {
            return Err(DurableAuthorityError::ResponseTooLarge);
        }
        let now_ms = i64_from_u64(authority_now, "authority_now")?;
        self.write(|database| {
            let stored = load_transaction(database, &prepared.transaction_id)?.ok_or(
                DurableAuthorityError::StateUnavailable("prepared transaction disappeared"),
            )?;
            require_active_device(
                database,
                &self.library_id,
                &stored.transaction.manifest.device_id,
            )?;
            if stored.signed_digest != prepared.transaction_digest {
                return Err(DurableAuthorityError::RequestReplayMismatch);
            }
            if let Some(replay) = load_request_replay(
                database,
                &stored.transaction.manifest.device_id,
                &prepared.request_id,
            )? {
                if replay.request_digest != prepared.request_digest
                    || replay.endpoint != DIRECT_PUSH_ENDPOINT
                {
                    return Err(DurableAuthorityError::RequestReplayMismatch);
                }
                let receipt = stored
                    .receipt
                    .ok_or(DurableAuthorityError::StateUnavailable(
                        "finalized replay transaction has no receipt",
                    ))?;
                return Ok(FinalizedPush {
                    receipt,
                    status_code: replay.status_code,
                    exact_response_bytes: replay.exact_response_bytes,
                });
            }

            if prepared.terminal_transaction {
                let receipt = stored
                    .receipt
                    .ok_or(DurableAuthorityError::StateUnavailable(
                        "terminal transaction has no receipt",
                    ))?;
                if stored.state == "prepared"
                    || receipt != prepared.receipt
                    || prepared.basis_digest != terminal_basis_digest(&receipt)
                {
                    return Err(DurableAuthorityError::StateChanged);
                }
                insert_request_replay(
                    database,
                    &stored.transaction.manifest.device_id,
                    &prepared.request_id,
                    NewRequestReplay {
                        endpoint: DIRECT_PUSH_ENDPOINT,
                        request_digest: &prepared.request_digest,
                        status_code,
                        exact_response_bytes,
                        authority_now_ms: now_ms,
                    },
                )?;
                return Ok(FinalizedPush {
                    receipt,
                    status_code,
                    exact_response_bytes: exact_response_bytes.to_vec(),
                });
            }

            if stored.state != "prepared" {
                return Err(DurableAuthorityError::StateChanged);
            }
            let profile = load_profile(database, &self.library_id)?;
            if head_basis_digest(database, &profile)? != prepared.basis_digest {
                return Err(DurableAuthorityError::StateChanged);
            }
            let receipt =
                candidate_receipt(database, &profile, &stored.transaction, authority_now)?;
            if receipt != prepared.receipt {
                return Err(DurableAuthorityError::StateChanged);
            }
            finalize_transaction(database, &profile, &stored.transaction, &receipt, now_ms)?;
            insert_request_replay(
                database,
                &stored.transaction.manifest.device_id,
                &prepared.request_id,
                NewRequestReplay {
                    endpoint: DIRECT_PUSH_ENDPOINT,
                    request_digest: &prepared.request_digest,
                    status_code,
                    exact_response_bytes,
                    authority_now_ms: now_ms,
                },
            )?;
            Ok(FinalizedPush {
                receipt,
                status_code,
                exact_response_bytes: exact_response_bytes.to_vec(),
            })
        })
    }

    fn prepare_ack(
        &self,
        request_id: &str,
        request_digest: [u8; 32],
        device_id: &str,
        high_water_cursor: u64,
        checkpoint_digest: &str,
        authority_now: u64,
    ) -> Result<PrepareAckResponseOutcome, DurableAuthorityError> {
        if !is_uuid_v7(request_id) || !is_uuid_v7(device_id) {
            return Err(DurableAuthorityError::InvalidInput(
                "request_id_or_device_id",
            ));
        }
        i64_from_u64(authority_now, "authority_now")?;
        self.write(|database| {
            require_active_device(database, &self.library_id, device_id)?;
            if let Some(replay) = load_request_replay(database, device_id, request_id)? {
                if replay.request_digest != request_digest || replay.endpoint != DIRECT_ACK_ENDPOINT
                {
                    return Err(DurableAuthorityError::RequestReplayMismatch);
                }
                return Ok(PrepareAckResponseOutcome::ExactReplay(ExactWireResponse {
                    status_code: replay.status_code,
                    body: replay.exact_response_bytes,
                }));
            }
            let profile = load_profile(database, &self.library_id)?;
            validate_ack_candidate(
                database,
                &profile,
                device_id,
                high_water_cursor,
                checkpoint_digest,
            )?;
            let receipt = AckReceipt {
                device_id: device_id.to_owned(),
                high_water_cursor,
                checkpoint_digest: checkpoint_digest.to_owned(),
            };
            let durable = DurablePreparedAck {
                request_id: request_id.to_owned(),
                request_digest,
                device_id: device_id.to_owned(),
                high_water_cursor,
                checkpoint_digest: checkpoint_digest.to_owned(),
                receipt: receipt.clone(),
            };
            let authority_token = serde_json::to_vec(&durable).map_err(|_| {
                DurableAuthorityError::StateUnavailable("ack preparation serialization failed")
            })?;
            Ok(PrepareAckResponseOutcome::Candidate(PreparedAckResponse {
                receipt,
                authority_token,
            }))
        })
    }

    fn finalize_ack(
        &self,
        prepared: &PreparedAckResponse,
        status_code: u16,
        exact_response_bytes: &[u8],
        authority_now: u64,
    ) -> Result<ExactWireResponse, DurableAuthorityError> {
        if !(100..=599).contains(&status_code) {
            return Err(DurableAuthorityError::InvalidInput("status_code"));
        }
        if exact_response_bytes.is_empty() || exact_response_bytes.len() > MAX_EXACT_RESPONSE_BYTES
        {
            return Err(DurableAuthorityError::ResponseTooLarge);
        }
        let durable: DurablePreparedAck = serde_json::from_slice(&prepared.authority_token)
            .map_err(|_| {
                DurableAuthorityError::StateUnavailable("ack preparation token is invalid")
            })?;
        if durable.receipt != prepared.receipt {
            return Err(DurableAuthorityError::StateChanged);
        }
        let now_ms = i64_from_u64(authority_now, "authority_now")?;
        self.write(|database| {
            require_active_device(database, &self.library_id, &durable.device_id)?;
            if let Some(replay) =
                load_request_replay(database, &durable.device_id, &durable.request_id)?
            {
                if replay.request_digest != durable.request_digest
                    || replay.endpoint != DIRECT_ACK_ENDPOINT
                {
                    return Err(DurableAuthorityError::RequestReplayMismatch);
                }
                return Ok(ExactWireResponse {
                    status_code: replay.status_code,
                    body: replay.exact_response_bytes,
                });
            }
            let profile = load_profile(database, &self.library_id)?;
            validate_ack_candidate(
                database,
                &profile,
                &durable.device_id,
                durable.high_water_cursor,
                &durable.checkpoint_digest,
            )?;
            let acknowledgement = AcknowledgeCheckpoint {
                library_id: self.library_id.clone(),
                device_id: durable.device_id.clone(),
                authority_generation: profile.authority_generation,
                high_water_cursor: durable.high_water_cursor,
                checkpoint_digest: durable.checkpoint_digest.clone(),
                exact_response_bytes: exact_response_bytes.to_vec(),
                authority_now_ms: now_ms,
            };
            match DirectAuthorityStore::acknowledge_checkpoint(database, &acknowledgement)? {
                AckOutcome::Recorded(_) | AckOutcome::ExactReplay(_) => {}
            }
            insert_request_replay(
                database,
                &durable.device_id,
                &durable.request_id,
                NewRequestReplay {
                    endpoint: DIRECT_ACK_ENDPOINT,
                    request_digest: &durable.request_digest,
                    status_code,
                    exact_response_bytes,
                    authority_now_ms: now_ms,
                },
            )?;
            Ok(ExactWireResponse {
                status_code,
                body: exact_response_bytes.to_vec(),
            })
        })
    }

    fn open_connection(&self) -> Result<Connection, DurableAuthorityError> {
        let connection = Connection::open(&self.database_path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY;")?;
        connection.busy_timeout(Duration::from_secs(10))?;
        Ok(connection)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, DurableAuthorityError>,
    ) -> Result<T, DurableAuthorityError> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = operation(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    fn trusted_now_ms(&self) -> Result<i64, DurableAuthorityError> {
        self.clock
            .now_ms()
            .map_err(|_| DurableAuthorityError::StateUnavailable("authority clock unavailable"))
    }
}

impl DirectSyncAuthority for SqliteDirectSyncAuthority {
    fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError> {
        self.write(|database| {
            let profile = load_profile(database, &self.library_id)?;
            Ok(AuthorityIdentity {
                library_id: profile.library_id,
                authority_generation: profile.authority_generation,
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
            })
        })
        .map_err(Into::into)
    }

    fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError> {
        self.write(|database| Ok(load_profile(database, &self.library_id)?.capabilities))
            .map_err(Into::into)
    }

    fn bootstrap(&self) -> Result<BootstrapSnapshot, AuthorityStoreError> {
        let now_ms = self.trusted_now_ms().map_err(AuthorityStoreError::from)?;
        self.write(|database| {
            let profile = load_profile(database, &self.library_id)?;
            let snapshot = bootstrap_snapshot(database, &profile)?;
            persist_checkpoint(database, &snapshot, now_ms)?;
            Ok(snapshot)
        })
        .map_err(Into::into)
    }

    fn pull(&self, cursor: u64, limit: u32) -> Result<ChangePage, AuthorityStoreError> {
        self.write(|database| pull_page(database, &self.library_id, cursor, limit))
            .map_err(Into::into)
    }

    fn push(
        &mut self,
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<SubmitOutcome, AuthorityStoreError> {
        let now_ms = i64_from_u64(now, "authority_now").map_err(AuthorityStoreError::from)?;
        self.write(|database| {
            require_active_device(database, &self.library_id, &transaction.manifest.device_id)?;
            let (stored, terminal) =
                ensure_prepared_transaction(database, &self.library_id, transaction, now, now_ms)?;
            if terminal {
                return stored.receipt.map(SubmitOutcome::Replay).ok_or(
                    DurableAuthorityError::StateUnavailable("terminal transaction has no receipt"),
                );
            }
            let profile = load_profile(database, &self.library_id)?;
            let receipt = candidate_receipt(database, &profile, &stored.transaction, now)?;
            finalize_transaction(database, &profile, &stored.transaction, &receipt, now_ms)?;
            Ok(SubmitOutcome::Terminal(receipt))
        })
        .map_err(Into::into)
    }

    fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError> {
        let now_ms = self.trusted_now_ms().map_err(AuthorityStoreError::from)?;
        self.write(|database| {
            let profile = load_profile(database, &self.library_id)?;
            let snapshot = bootstrap_snapshot(database, &profile)?;
            persist_checkpoint(database, &snapshot, now_ms)?;
            Ok(checkpoint_from_snapshot(&snapshot))
        })
        .map_err(Into::into)
    }

    fn acknowledge(
        &mut self,
        device_id: &str,
        cursor: u64,
        checkpoint_digest: &str,
    ) -> Result<AckReceipt, AuthorityStoreError> {
        let now_ms = self.trusted_now_ms().map_err(AuthorityStoreError::from)?;
        self.write(|database| {
            let profile = load_profile(database, &self.library_id)?;
            require_active_device(database, &self.library_id, device_id)?;
            let receipt = AckReceipt {
                device_id: device_id.to_owned(),
                high_water_cursor: cursor,
                checkpoint_digest: checkpoint_digest.to_owned(),
            };
            let exact_response_bytes = serde_json::to_vec(&receipt)
                .map_err(|_| DurableAuthorityError::StateUnavailable("ack serialization failed"))?;
            match DirectAuthorityStore::acknowledge_checkpoint(
                database,
                &AcknowledgeCheckpoint {
                    library_id: self.library_id.clone(),
                    device_id: device_id.to_owned(),
                    authority_generation: profile.authority_generation,
                    high_water_cursor: cursor,
                    checkpoint_digest: checkpoint_digest.to_owned(),
                    exact_response_bytes,
                    authority_now_ms: now_ms,
                },
            )? {
                AckOutcome::Recorded(_) | AckOutcome::ExactReplay(_) => Ok(receipt),
            }
        })
        .map_err(Into::into)
    }

    fn revoke_device(&mut self, device_id: &str) -> Result<(), AuthorityStoreError> {
        let now_ms = self.trusted_now_ms().map_err(AuthorityStoreError::from)?;
        self.write(|database| {
            match DirectAuthorityStore::revoke_device(database, device_id, now_ms)? {
                RevokeOutcome::Revoked | RevokeOutcome::AlreadyRevoked => Ok(()),
            }
        })
        .map_err(Into::into)
    }
}

impl ExactWireDirectSyncAuthority for SqliteDirectSyncAuthority {
    fn prepare_push_response(
        &mut self,
        request_id: &str,
        request_digest: [u8; 32],
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<PreparePushResponseOutcome, AuthorityStoreError> {
        match SqliteDirectSyncAuthority::prepare_push(
            self,
            request_id,
            request_digest,
            transaction,
            now,
        )
        .map_err(AuthorityStoreError::from)?
        {
            PreparePushOutcome::ExactReplay(replay) => {
                Ok(PreparePushResponseOutcome::ExactReplay(ExactWireResponse {
                    status_code: replay.status_code,
                    body: replay.exact_response_bytes,
                }))
            }
            PreparePushOutcome::NeedsFinalization(prepared) => {
                let receipt = prepared.receipt.clone();
                let authority_token = serde_json::to_vec(&prepared)
                    .map_err(|_| AuthorityStoreError::StateUnavailable)?;
                Ok(PreparePushResponseOutcome::Candidate(
                    PreparedPushResponse {
                        receipt,
                        authority_token,
                    },
                ))
            }
        }
    }

    fn finalize_push_response(
        &mut self,
        prepared: &PreparedPushResponse,
        status_code: u16,
        exact_response_bytes: &[u8],
        now: u64,
    ) -> Result<ExactWireResponse, AuthorityStoreError> {
        let durable: PreparedPush = serde_json::from_slice(&prepared.authority_token)
            .map_err(|_| AuthorityStoreError::StateUnavailable)?;
        if durable.receipt != prepared.receipt {
            return Err(AuthorityStoreError::StateUnavailable);
        }
        let finalized = SqliteDirectSyncAuthority::finalize_push(
            self,
            &durable,
            status_code,
            exact_response_bytes,
            now,
        )
        .map_err(AuthorityStoreError::from)?;
        Ok(ExactWireResponse {
            status_code: finalized.status_code,
            body: finalized.exact_response_bytes,
        })
    }

    fn prepare_ack_response(
        &mut self,
        request_id: &str,
        request_digest: [u8; 32],
        device_id: &str,
        cursor: u64,
        checkpoint_digest: &str,
        now: u64,
    ) -> Result<PrepareAckResponseOutcome, AuthorityStoreError> {
        self.prepare_ack(
            request_id,
            request_digest,
            device_id,
            cursor,
            checkpoint_digest,
            now,
        )
        .map_err(AuthorityStoreError::from)
    }

    fn finalize_ack_response(
        &mut self,
        prepared: &PreparedAckResponse,
        status_code: u16,
        exact_response_bytes: &[u8],
        now: u64,
    ) -> Result<ExactWireResponse, AuthorityStoreError> {
        self.finalize_ack(prepared, status_code, exact_response_bytes, now)
            .map_err(AuthorityStoreError::from)
    }
}

impl DirectSyncEnrollment for SqliteDirectSyncAuthority {
    fn require_active_device(
        &self,
        device_id: &str,
        library_id: &str,
        environment: Environment,
        authority_generation: u64,
    ) -> Result<(), EnrollmentAuthorizationError> {
        self.write(|database| {
            durable_enrollment_capabilities(
                database,
                &self.library_id,
                device_id,
                library_id,
                environment,
                authority_generation,
            )?;
            Ok(())
        })
        .map_err(map_durable_enrollment_error)
    }

    fn require_active_device_scope(
        &self,
        device_id: &str,
        library_id: &str,
        environment: Environment,
        authority_generation: u64,
        scope: RecordKind,
        require_write: bool,
    ) -> Result<KindCapability, EnrollmentAuthorizationError> {
        self.write(|database| {
            let (granted_scopes, capabilities) = durable_enrollment_capabilities(
                database,
                &self.library_id,
                device_id,
                library_id,
                environment,
                authority_generation,
            )?;
            let kind = pairing_record_kind_name(scope);
            if !granted_scopes.contains(kind) {
                return Err(DurableAuthorityError::InvalidInput("scope_not_granted"));
            }
            let capability = capabilities.record_kinds.get(kind).ok_or(
                DurableAuthorityError::StateUnavailable("active enrollment capability is missing"),
            )?;
            if require_write && capability.max_write_schema_version == 0 {
                return Err(DurableAuthorityError::InvalidInput("scope_not_granted"));
            }
            Ok(KindCapability {
                reader_version: capability.max_read_schema_version,
                writer_version: (capability.max_write_schema_version > 0)
                    .then_some(capability.max_write_schema_version),
            })
        })
        .map_err(|error| {
            if matches!(
                error,
                DurableAuthorityError::InvalidInput("scope_not_granted")
            ) {
                EnrollmentAuthorizationError::ScopeViolation
            } else {
                map_durable_enrollment_error(error)
            }
        })
    }

    fn revoke_device(
        &self,
        device_id: &str,
        now_ms: i64,
    ) -> Result<(), EnrollmentAuthorizationError> {
        self.write(|database| {
            match DirectAuthorityStore::revoke_device(database, device_id, now_ms)? {
                RevokeOutcome::Revoked | RevokeOutcome::AlreadyRevoked => Ok(()),
            }
        })
        .map_err(map_durable_enrollment_error)
    }
}

fn map_durable_enrollment_error(error: DurableAuthorityError) -> EnrollmentAuthorizationError {
    match error {
        DurableAuthorityError::Protocol(ProtocolError::DeviceRevoked) => {
            EnrollmentAuthorizationError::Revoked
        }
        DurableAuthorityError::Protocol(ProtocolError::DeviceUnknown) => {
            EnrollmentAuthorizationError::NotAuthorized
        }
        _ => EnrollmentAuthorizationError::StateUnavailable,
    }
}

fn pairing_record_kind_name(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Note => "note",
        RecordKind::Category => "category",
        RecordKind::Folder => "folder",
        RecordKind::Media => "media",
    }
}

fn durable_enrollment_capabilities(
    database: &Transaction<'_>,
    configured_library_id: &str,
    device_id: &str,
    requested_library_id: &str,
    environment: Environment,
    authority_generation: u64,
) -> Result<(BTreeSet<String>, ProtocolCapabilities), DurableAuthorityError> {
    if requested_library_id != configured_library_id || environment != Environment::Development {
        return Err(ProtocolError::DeviceUnknown.into());
    }
    let profile = load_profile(database, configured_library_id)?;
    if profile.authority_generation != authority_generation {
        return Err(ProtocolError::DeviceUnknown.into());
    }
    let device: Option<(String, String, String)> = database
        .query_row(
            "SELECT role, enrollment_state, capabilities_json
             FROM portable_devices WHERE device_id = ?1 AND library_id = ?2",
            params![device_id, configured_library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((role, state, device_capabilities_json)) = device else {
        return Err(ProtocolError::DeviceUnknown.into());
    };
    if state == "revoked" {
        return Err(ProtocolError::DeviceRevoked.into());
    }
    if role != "replica" || state != "active" {
        return Err(DurableAuthorityError::StateUnavailable(
            "direct device role or state is invalid",
        ));
    }
    let receipt: Option<(i64, String, String, String, String, i64)> = database
        .query_row(
            "SELECT r.authority_generation, r.granted_scopes_json, r.capabilities_json,
                    i.environment, i.library_id, i.authority_generation
             FROM direct_enrollment_receipts r
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE r.device_id = ?1 AND r.library_id = ?2 AND r.state = 'active'",
            params![device_id, configured_library_id],
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
        .optional()?;
    let Some((
        receipt_generation,
        scopes_json,
        receipt_capabilities_json,
        invitation_environment,
        invitation_library_id,
        invitation_generation,
    )) = receipt
    else {
        return Err(DurableAuthorityError::StateUnavailable(
            "active direct device has no active enrollment receipt",
        ));
    };
    if u64_from_db(receipt_generation, "authority_generation")? != authority_generation
        || invitation_environment != "development"
        || invitation_library_id != configured_library_id
        || u64_from_db(invitation_generation, "authority_generation")? != authority_generation
    {
        return Err(ProtocolError::DeviceUnknown.into());
    }
    let granted_scopes: BTreeSet<String> = serde_json::from_str(&scopes_json)
        .map_err(|_| DurableAuthorityError::StateUnavailable("granted scopes are invalid"))?;
    let receipt_capabilities: ProtocolCapabilities =
        serde_json::from_str(&receipt_capabilities_json).map_err(|_| {
            DurableAuthorityError::StateUnavailable("enrollment capabilities are invalid")
        })?;
    let device_capabilities: ProtocolCapabilities = serde_json::from_str(&device_capabilities_json)
        .map_err(|_| DurableAuthorityError::StateUnavailable("device capabilities are invalid"))?;
    receipt_capabilities.validate()?;
    device_capabilities.validate()?;
    if receipt_capabilities != device_capabilities {
        return Err(DurableAuthorityError::StateUnavailable(
            "device and enrollment capabilities disagree",
        ));
    }
    Ok((granted_scopes, receipt_capabilities))
}

#[derive(Debug)]
struct RequestReplay {
    request_digest: [u8; 32],
    endpoint: String,
    status_code: u16,
    exact_response_bytes: Vec<u8>,
}

struct NewRequestReplay<'a> {
    endpoint: &'a str,
    request_digest: &'a [u8; 32],
    status_code: u16,
    exact_response_bytes: &'a [u8],
    authority_now_ms: i64,
}

fn load_request_replay(
    database: &Transaction<'_>,
    device_id: &str,
    request_id: &str,
) -> Result<Option<RequestReplay>, DurableAuthorityError> {
    let row = database
        .query_row(
            "SELECT request_digest, endpoint, status_code, exact_response_bytes
             FROM direct_request_replays WHERE device_id = ?1 AND request_id = ?2",
            params![device_id, request_id],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(digest, endpoint, status, bytes)| {
        let request_digest: [u8; 32] = digest
            .try_into()
            .map_err(|_| DurableAuthorityError::StateUnavailable("invalid replay digest"))?;
        let status_code = u16::try_from(status)
            .map_err(|_| DurableAuthorityError::StateUnavailable("invalid replay status"))?;
        Ok(RequestReplay {
            request_digest,
            endpoint,
            status_code,
            exact_response_bytes: bytes,
        })
    })
    .transpose()
}

fn insert_request_replay(
    database: &Transaction<'_>,
    device_id: &str,
    request_id: &str,
    replay: NewRequestReplay<'_>,
) -> Result<(), DurableAuthorityError> {
    let count: i64 = database.query_row(
        "SELECT COUNT(*) FROM direct_request_replays
         WHERE device_id = ?1 AND created_at_ms > ?2",
        params![
            device_id,
            replay
                .authority_now_ms
                .saturating_sub(EXACT_RESPONSE_REPLAY_RETENTION_MS),
        ],
        |row| row.get(0),
    )?;
    if count >= MAX_REQUEST_REPLAYS_PER_DEVICE {
        return Err(DurableAuthorityError::StateUnavailable(
            "request replay rate window is full",
        ));
    }
    database.execute(
        "INSERT INTO direct_request_replays
         (device_id, request_id, endpoint, request_digest, status_code,
          exact_response_bytes, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            device_id,
            request_id,
            replay.endpoint,
            replay.request_digest.as_slice(),
            i64::from(replay.status_code),
            replay.exact_response_bytes,
            replay.authority_now_ms,
        ],
    )?;
    Ok(())
}

fn ensure_prepared_transaction(
    database: &Transaction<'_>,
    library_id: &str,
    transaction: SignedTransaction,
    authority_now: u64,
    authority_now_ms: i64,
) -> Result<(StoredTransaction, bool), DurableAuthorityError> {
    let digest = transaction.signed_digest();
    if let Some(stored) = load_transaction(database, &transaction.manifest.transaction_id)? {
        if stored.signed_digest != digest || stored.transaction != transaction {
            return Err(ProtocolError::TransactionIdReuse.into());
        }
        let terminal = stored.state != "prepared";
        return Ok((stored, terminal));
    }

    let profile = load_profile(database, library_id)?;
    if transaction.manifest.library_id != library_id {
        return Err(ProtocolError::WrongLibrary.into());
    }
    require_active_device(database, library_id, &transaction.manifest.device_id)?;
    let negotiated = negotiate_capabilities(&profile.capabilities, &profile.capabilities)?;
    transaction.validate(authority_now, &negotiated)?;
    validate_generation_floors(&profile, &transaction)?;

    let pending: bool = database.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM direct_authority_transactions
           WHERE device_id = ?1 AND state = 'prepared'
         )",
        [&transaction.manifest.device_id],
        |row| row.get(0),
    )?;
    if pending {
        return Err(ProtocolError::PriorTransactionPending.into());
    }
    for member in &transaction.members {
        let existing: Option<String> = database
            .query_row(
                "SELECT signed_digest FROM direct_authority_mutations WHERE mutation_id = ?1",
                [&member.mutation_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(ProtocolError::MutationIdReuse.into());
        }
    }

    let last_counter: i64 = database
        .query_row(
            "SELECT last_transaction_counter FROM portable_devices
             WHERE device_id = ?1 AND library_id = ?2 AND role = 'replica'
               AND enrollment_state = 'active'",
            params![transaction.manifest.device_id, library_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(ProtocolError::DeviceUnknown)?;
    let expected = u64_from_db(last_counter, "last_transaction_counter")?
        .checked_add(1)
        .ok_or(ProtocolError::CounterGap {
            expected: u64::MAX,
            provided: transaction.manifest.device_transaction_counter,
        })?;
    if transaction.manifest.device_transaction_counter != expected {
        let bound: Option<String> = database
            .query_row(
                "SELECT signed_digest FROM direct_authority_transactions
                 WHERE device_id = ?1 AND device_transaction_counter = ?2",
                params![
                    transaction.manifest.device_id,
                    i64_from_u64(
                        transaction.manifest.device_transaction_counter,
                        "device_transaction_counter"
                    )?,
                ],
                |row| row.get(0),
            )
            .optional()?;
        return Err(if bound.is_some() {
            ProtocolError::CounterReuse
        } else {
            ProtocolError::CounterGap {
                expected,
                provided: transaction.manifest.device_transaction_counter,
            }
        }
        .into());
    }

    let transaction_json = canonical_json(&serde_json::to_value(&transaction).map_err(|_| {
        DurableAuthorityError::StateUnavailable("transaction serialization failed")
    })?);
    database.execute(
        "INSERT INTO direct_authority_transactions
         (transaction_id, library_id, device_id, authority_generation,
          device_transaction_counter, signed_digest, transaction_json,
          state, receipt_json, accepted_cursor, created_at_ms, expires_at_ms,
          terminal_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'prepared', NULL, NULL, ?8, ?9, NULL)",
        params![
            transaction.manifest.transaction_id,
            library_id,
            transaction.manifest.device_id,
            i64_from_u64(
                transaction.manifest.authority_generation,
                "authority_generation"
            )?,
            i64_from_u64(
                transaction.manifest.device_transaction_counter,
                "device_transaction_counter"
            )?,
            digest,
            transaction_json,
            authority_now_ms,
            i64_from_u64(transaction.manifest.expires_at, "expires_at")?,
        ],
    )?;
    for member in &transaction.members {
        let envelope_json = canonical_json(&serde_json::to_value(member).map_err(|_| {
            DurableAuthorityError::StateUnavailable("mutation serialization failed")
        })?);
        database.execute(
            "INSERT INTO direct_authority_mutations
             (mutation_id, transaction_id, member_index, signed_digest,
              record_id, version_id, envelope_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                member.mutation_id,
                transaction.manifest.transaction_id,
                i64::from(member.transaction_member_index),
                member.signed_digest(),
                member.record_id,
                member.version_id,
                envelope_json,
            ],
        )?;
    }
    let changed = database.execute(
        "UPDATE portable_devices SET last_transaction_counter = ?2
         WHERE device_id = ?1 AND enrollment_state = 'active'
           AND last_transaction_counter = ?3",
        params![
            transaction.manifest.device_id,
            i64_from_u64(
                transaction.manifest.device_transaction_counter,
                "device_transaction_counter"
            )?,
            last_counter,
        ],
    )?;
    if changed != 1 {
        return Err(DurableAuthorityError::StateChanged);
    }
    Ok((
        StoredTransaction {
            transaction,
            signed_digest: digest,
            state: "prepared".to_owned(),
            receipt: None,
        },
        false,
    ))
}

fn load_transaction(
    database: &Transaction<'_>,
    transaction_id: &str,
) -> Result<Option<StoredTransaction>, DurableAuthorityError> {
    let row = database
        .query_row(
            "SELECT transaction_json, signed_digest, state, receipt_json
             FROM direct_authority_transactions WHERE transaction_id = ?1",
            [transaction_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(transaction_json, signed_digest, state, receipt_json)| {
        let transaction: SignedTransaction =
            serde_json::from_str(&transaction_json).map_err(|_| {
                DurableAuthorityError::StateUnavailable("stored transaction is invalid")
            })?;
        if transaction.signed_digest() != signed_digest {
            return Err(DurableAuthorityError::StateUnavailable(
                "stored transaction digest changed",
            ));
        }
        let receipt = receipt_json
            .map(|value| {
                serde_json::from_str(&value).map_err(|_| {
                    DurableAuthorityError::StateUnavailable("stored receipt is invalid")
                })
            })
            .transpose()?;
        Ok(StoredTransaction {
            transaction,
            signed_digest,
            state,
            receipt,
        })
    })
    .transpose()
}

fn candidate_receipt(
    database: &Transaction<'_>,
    profile: &Profile,
    transaction: &SignedTransaction,
    authority_now: u64,
) -> Result<TransactionReceipt, DurableAuthorityError> {
    let disposition = if transaction.manifest.expires_at < authority_now {
        ReceiptDisposition::Rejected {
            code: TerminalRejection::Expired,
        }
    } else if transaction.manifest.authority_generation != profile.authority_generation {
        ReceiptDisposition::Rejected {
            code: TerminalRejection::AuthorityGenerationChanged,
        }
    } else if transaction.manifest.purge_generation != profile.purge_generation {
        ReceiptDisposition::Rejected {
            code: TerminalRejection::PurgeGenerationChanged,
        }
    } else {
        let mut conflicts = Vec::new();
        for member in &transaction.members {
            let current = load_direct_head(database, &profile.library_id, &member.record_id)?;
            let matches = match &current {
                None => member.base_head_revision == 0 && member.base_head_version_id.is_none(),
                Some(head) => {
                    member.base_head_revision == head.revision
                        && member.base_head_version_id.as_deref() == Some(head.version_id.as_str())
                }
            };
            if !matches {
                conflicts.push(HeadConflict {
                    record_id: member.record_id.clone(),
                    proposed_version_id: member.version_id.clone(),
                    accepted_head: current,
                });
            }
        }
        if conflicts.is_empty() {
            ReceiptDisposition::Accepted {
                advances: transaction
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
                    .collect(),
            }
        } else {
            ReceiptDisposition::Conflict { conflicts }
        }
    };
    let accepted = matches!(disposition, ReceiptDisposition::Accepted { .. });
    let high_water_cursor = if accepted {
        profile
            .high_water_cursor
            .checked_add(1)
            .ok_or(ProtocolError::CursorOverflow)?
    } else {
        profile.high_water_cursor
    };
    let mut ordered_members = transaction.members.iter().collect::<Vec<_>>();
    ordered_members.sort_by_key(|member| member.transaction_member_index);
    Ok(TransactionReceipt {
        library_id: profile.library_id.clone(),
        transaction_id: transaction.manifest.transaction_id.clone(),
        transaction_digest: transaction.signed_digest(),
        mutation_ids: ordered_members
            .iter()
            .map(|member| member.mutation_id.clone())
            .collect(),
        device_id: transaction.manifest.device_id.clone(),
        device_transaction_counter: transaction.manifest.device_transaction_counter,
        authority_generation: profile.authority_generation,
        purge_generation: profile.purge_generation,
        high_water_cursor,
        disposition,
    })
}

fn finalize_transaction(
    database: &Transaction<'_>,
    profile: &Profile,
    transaction: &SignedTransaction,
    receipt: &TransactionReceipt,
    authority_now_ms: i64,
) -> Result<(), DurableAuthorityError> {
    let (state, accepted_cursor) = match &receipt.disposition {
        ReceiptDisposition::Accepted { .. } => ("accepted", Some(receipt.high_water_cursor)),
        ReceiptDisposition::Conflict { .. } => ("conflict", None),
        ReceiptDisposition::Rejected { .. } => ("rejected", None),
    };
    if state == "accepted" {
        if receipt.high_water_cursor != profile.high_water_cursor + 1 {
            return Err(DurableAuthorityError::StateChanged);
        }
        materialize_fixture_transaction(database, transaction, receipt, authority_now_ms)?;
        let changed = database.execute(
            "UPDATE direct_authority_profiles
             SET high_water_cursor = ?2, state_revision = state_revision + 1,
                 updated_at_ms = ?3
             WHERE library_id = ?1 AND high_water_cursor = ?4
               AND state_revision = ?5 AND environment = 'development'
               AND library_data_class = 'sanitized_fixture'",
            params![
                profile.library_id,
                i64_from_u64(receipt.high_water_cursor, "high_water_cursor")?,
                authority_now_ms,
                i64_from_u64(profile.high_water_cursor, "high_water_cursor")?,
                i64_from_u64(profile.state_revision, "state_revision")?,
            ],
        )?;
        if changed != 1 {
            return Err(DurableAuthorityError::StateChanged);
        }
    }
    let receipt_json =
        canonical_json(&serde_json::to_value(receipt).map_err(|_| {
            DurableAuthorityError::StateUnavailable("receipt serialization failed")
        })?);
    let changed = database.execute(
        "UPDATE direct_authority_transactions
         SET state = ?2, receipt_json = ?3, accepted_cursor = ?4,
             terminal_at_ms = ?5
         WHERE transaction_id = ?1 AND state = 'prepared'",
        params![
            transaction.manifest.transaction_id,
            state,
            receipt_json,
            accepted_cursor
                .map(|cursor| i64_from_u64(cursor, "accepted_cursor"))
                .transpose()?,
            authority_now_ms,
        ],
    )?;
    if changed != 1 {
        return Err(DurableAuthorityError::StateChanged);
    }
    if state == "accepted" {
        database.execute(
            "INSERT INTO direct_authority_changes
             (library_id, sequence, transaction_id, transaction_digest, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                profile.library_id,
                i64_from_u64(receipt.high_water_cursor, "high_water_cursor")?,
                transaction.manifest.transaction_id,
                receipt.transaction_digest,
                authority_now_ms,
            ],
        )?;
    }
    Ok(())
}

fn materialize_fixture_transaction(
    database: &Transaction<'_>,
    transaction: &SignedTransaction,
    receipt: &TransactionReceipt,
    authority_now_ms: i64,
) -> Result<(), DurableAuthorityError> {
    let accepted_at = portable_timestamp(authority_now_ms)?;
    database.execute(
        "INSERT INTO change_transactions
         (transaction_id, library_id, device_id, device_transaction_counter,
          member_count, manifest_digest, commit_marker, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
        params![
            transaction.manifest.transaction_id,
            transaction.manifest.library_id,
            transaction.manifest.device_id,
            i64_from_u64(
                transaction.manifest.device_transaction_counter,
                "device_transaction_counter"
            )?,
            i64::from(transaction.manifest.member_count),
            transaction.manifest.digest(),
            accepted_at,
        ],
    )?;
    let mut members = transaction.members.iter().collect::<Vec<_>>();
    members.sort_by_key(|member| member.transaction_member_index);
    for member in members {
        let record = decode_fixture_record(member)?;
        require_record_scope(database, &record)?;
        let existing: Option<(String, String)> = database
            .query_row(
                "SELECT library_id, source_table FROM portable_records WHERE record_id = ?1",
                [&record.record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((library_id, source_table)) = existing {
            if library_id != record.library_id || source_table != DIRECT_FIXTURE_SOURCE {
                return Err(DurableAuthorityError::FixtureOnly);
            }
            database.execute(
                "UPDATE portable_records SET lifecycle_state = ?2, trashed_at = ?3,
                 tombstoned_at = ?4, updated_at = ?5
                 WHERE record_id = ?1",
                params![
                    record.record_id,
                    lifecycle_name(&record.lifecycle.state),
                    record.lifecycle.trashed_at,
                    record.lifecycle.tombstoned_at,
                    record.updated_at,
                ],
            )?;
        } else {
            let source_row_id: i64 = database.query_row(
                "SELECT COALESCE(MAX(source_row_id), 0) + 1 FROM portable_records
                 WHERE source_table = ?1",
                [DIRECT_FIXTURE_SOURCE],
                |row| row.get(0),
            )?;
            database.execute(
                "INSERT INTO portable_records
                 (record_id, library_id, kind, record_schema_version, source_table,
                  source_row_id, scope_id, sensitivity, authority_kind,
                  authority_origin, write_policy, lifecycle_state, trashed_at,
                  tombstoned_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         'read_write', ?11, ?12, ?13, ?14, ?15)",
                params![
                    record.record_id,
                    record.library_id,
                    record.kind,
                    i64::from(record.record_schema_version),
                    DIRECT_FIXTURE_SOURCE,
                    source_row_id,
                    record.scope.scope_id,
                    record.sensitivity,
                    authority_name(&record.authority.kind),
                    record.authority.origin,
                    lifecycle_name(&record.lifecycle.state),
                    record.lifecycle.trashed_at,
                    record.lifecycle.tombstoned_at,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
        }
        let snapshot_json =
            canonical_json(&serde_json::to_value(&record).map_err(|_| {
                DurableAuthorityError::StateUnavailable("record serialization failed")
            })?);
        database.execute(
            "INSERT INTO record_versions
             (version_id, record_id, revision, content_hash, snapshot_json,
              source_device_id, transaction_id, created_at, accepted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.version_id,
                record.record_id,
                i64_from_u64(record.revision, "record revision")?,
                record.content_hash,
                snapshot_json,
                transaction.manifest.device_id,
                transaction.manifest.transaction_id,
                record.created_at,
                accepted_at,
            ],
        )?;
        database.execute(
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
                record.record_id,
                i64_from_u64(record.revision, "record revision")?,
                record.version_id,
                record.content_hash,
                i64_from_u64(receipt.authority_generation, "authority_generation")?,
                accepted_at,
            ],
        )?;
        database.execute(
            "INSERT INTO change_log
             (mutation_id, transaction_id, transaction_member_index, record_id,
              record_kind, base_revision, base_version_id, proposed_revision,
              version_id, mutation_digest, authority_generation, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     'accepted_remote', ?12)",
            params![
                member.mutation_id,
                transaction.manifest.transaction_id,
                i64::from(member.transaction_member_index),
                member.record_id,
                member.record_kind,
                i64_from_u64(member.base_head_revision, "base_head_revision")?,
                member.base_head_version_id,
                i64_from_u64(member.proposed_revision, "proposed_revision")?,
                member.version_id,
                member.signed_digest(),
                i64_from_u64(member.authority_generation, "authority_generation")?,
                accepted_at,
            ],
        )?;
    }
    Ok(())
}

fn decode_fixture_record(
    member: &MutationEnvelope,
) -> Result<ContextRecordV1, DurableAuthorityError> {
    let bytes = member
        .ciphertext
        .strip_prefix(FIXTURE_CIPHERTEXT_PREFIX)
        .ok_or(DurableAuthorityError::FixtureOnly)?;
    let record: ContextRecordV1 = serde_json::from_slice(bytes)
        .map_err(|_| DurableAuthorityError::StateUnavailable("fixture record is not valid JSON"))?;
    record.validate().map_err(|_| {
        DurableAuthorityError::StateUnavailable("fixture record contract is invalid")
    })?;
    if record.library_id != member.library_id
        || record.record_id != member.record_id
        || record.kind != member.record_kind
        || record.record_schema_version != member.record_schema_version
        || record.revision != member.proposed_revision
        || record.version_id != member.version_id
    {
        return Err(DurableAuthorityError::StateUnavailable(
            "fixture record does not match mutation envelope",
        ));
    }
    Ok(record)
}

fn require_record_scope(
    database: &Transaction<'_>,
    record: &ContextRecordV1,
) -> Result<(), DurableAuthorityError> {
    let expected_class = scope_name(&record.scope.class);
    let scope: Option<(String, String)> = database
        .query_row(
            "SELECT library_id, scope_class FROM library_scopes WHERE scope_id = ?1",
            [&record.scope.scope_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if scope.as_ref().is_none_or(|(library_id, class)| {
        library_id != &record.library_id || class != expected_class
    }) {
        return Err(DurableAuthorityError::StateUnavailable(
            "fixture record scope is not registered",
        ));
    }
    Ok(())
}

fn bootstrap_snapshot(
    database: &Transaction<'_>,
    profile: &Profile,
) -> Result<BootstrapSnapshot, DurableAuthorityError> {
    let total_heads: i64 = database.query_row(
        "SELECT COUNT(*) FROM record_heads h
         JOIN portable_records p ON p.record_id = h.record_id
         WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')",
        [&profile.library_id],
        |row| row.get(0),
    )?;
    let mut statement = database.prepare(
        "SELECT p.record_id, h.accepted_revision, h.accepted_version_id,
                m.envelope_json, c.sequence
         FROM record_heads h
         JOIN portable_records p ON p.record_id = h.record_id
         JOIN direct_authority_mutations m
           ON m.record_id = p.record_id AND m.version_id = h.accepted_version_id
         JOIN direct_authority_changes c ON c.transaction_id = m.transaction_id
         WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')
         ORDER BY p.record_id",
    )?;
    let rows = statement.query_map([&profile.library_id], |row| {
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
            DurableAuthorityError::StateUnavailable("stored head mutation is invalid")
        })?;
        if mutation.record_id != record_id || mutation.version_id != version_id {
            return Err(DurableAuthorityError::StateUnavailable(
                "portable head and direct mutation diverged",
            ));
        }
        records.push(BootstrapRecord {
            record_id,
            accepted_head: AcceptedHead {
                revision: u64_from_db(revision, "accepted_revision")?,
                version_id,
                ciphertext_hash: mutation.ciphertext_hash.clone(),
                authority_generation: mutation.authority_generation,
                acceptance_checkpoint: u64_from_db(sequence, "acceptance_checkpoint")?,
            },
            mutation,
        });
    }
    if records.len() as i64 != total_heads {
        return Err(DurableAuthorityError::StateUnavailable(
            "a portable Notes head has no committed direct ciphertext",
        ));
    }
    let mut snapshot = BootstrapSnapshot {
        contract_version: BOOTSTRAP_SNAPSHOT_VERSION.to_owned(),
        library_id: profile.library_id.clone(),
        authority_generation: profile.authority_generation,
        purge_generation: profile.purge_generation,
        key_epoch: profile.key_epoch,
        high_water_cursor: profile.high_water_cursor,
        records,
        checkpoint_digest: String::new(),
    };
    snapshot.checkpoint_digest = snapshot.computed_checkpoint_digest();
    snapshot.validate()?;
    Ok(snapshot)
}

fn pull_page(
    database: &Transaction<'_>,
    library_id: &str,
    cursor: u64,
    limit: u32,
) -> Result<ChangePage, DurableAuthorityError> {
    if limit == 0 || limit > MAX_PULL_PAGE_CHANGES {
        return Err(ProtocolError::InvalidPullLimit {
            maximum: MAX_PULL_PAGE_CHANGES,
            provided: limit,
        }
        .into());
    }
    let profile = load_profile(database, library_id)?;
    if cursor > profile.high_water_cursor {
        return Err(ProtocolError::CursorAhead {
            high_water: profile.high_water_cursor,
            provided: cursor,
        }
        .into());
    }
    let mut statement = database.prepare(
        "SELECT c.sequence, c.transaction_digest, t.transaction_json, t.receipt_json
         FROM direct_authority_changes c
         JOIN direct_authority_transactions t ON t.transaction_id = c.transaction_id
         WHERE c.library_id = ?1 AND c.sequence > ?2
         ORDER BY c.sequence LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            library_id,
            i64_from_u64(cursor, "cursor")?,
            i64::from(limit)
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut changes = Vec::new();
    let mut expected = cursor.checked_add(1).ok_or(ProtocolError::CursorOverflow)?;
    for row in rows {
        let (sequence, transaction_digest, transaction_json, receipt_json) = row?;
        let sequence = u64_from_db(sequence, "sequence")?;
        if sequence != expected {
            return Err(DurableAuthorityError::StateUnavailable(
                "direct change sequence is not contiguous",
            ));
        }
        let transaction: SignedTransaction =
            serde_json::from_str(&transaction_json).map_err(|_| {
                DurableAuthorityError::StateUnavailable("stored transaction is invalid")
            })?;
        let receipt: TransactionReceipt = serde_json::from_str(&receipt_json)
            .map_err(|_| DurableAuthorityError::StateUnavailable("stored receipt is invalid"))?;
        if transaction.signed_digest() != transaction_digest
            || receipt.transaction_digest != transaction_digest
            || receipt.high_water_cursor != sequence
            || !matches!(receipt.disposition, ReceiptDisposition::Accepted { .. })
        {
            return Err(DurableAuthorityError::StateUnavailable(
                "accepted change binding is invalid",
            ));
        }
        changes.push(AcceptedChange {
            sequence,
            transaction_digest,
            transaction,
            receipt,
        });
        expected = expected
            .checked_add(1)
            .ok_or(ProtocolError::CursorOverflow)?;
    }
    let next_cursor = changes.last().map_or(cursor, |change| change.sequence);
    Ok(ChangePage {
        requested_cursor: cursor,
        next_cursor,
        high_water_cursor: profile.high_water_cursor,
        has_more: next_cursor < profile.high_water_cursor,
        changes,
    })
}

fn persist_checkpoint(
    database: &Transaction<'_>,
    snapshot: &BootstrapSnapshot,
    authority_now_ms: i64,
) -> Result<(), DurableAuthorityError> {
    let checkpoint = checkpoint_from_snapshot(snapshot);
    let exact_response_bytes = serde_json::to_vec(&checkpoint)
        .map_err(|_| DurableAuthorityError::StateUnavailable("checkpoint serialization failed"))?;
    match DirectAuthorityStore::issue_checkpoint(
        database,
        &IssueCheckpoint {
            library_id: snapshot.library_id.clone(),
            authority_generation: snapshot.authority_generation,
            high_water_cursor: snapshot.high_water_cursor,
            purge_generation: snapshot.purge_generation,
            key_epoch: snapshot.key_epoch,
            checkpoint_digest: snapshot.checkpoint_digest.clone(),
            exact_response_bytes,
            created_at_ms: authority_now_ms,
        },
    )? {
        CheckpointOutcome::Issued(_) | CheckpointOutcome::ExactReplay(_) => Ok(()),
    }
}

fn checkpoint_from_snapshot(snapshot: &BootstrapSnapshot) -> SyncCheckpoint {
    SyncCheckpoint {
        contract_version: snapshot.contract_version.clone(),
        library_id: snapshot.library_id.clone(),
        authority_generation: snapshot.authority_generation,
        purge_generation: snapshot.purge_generation,
        key_epoch: snapshot.key_epoch,
        high_water_cursor: snapshot.high_water_cursor,
        checkpoint_digest: snapshot.checkpoint_digest.clone(),
    }
}

fn load_direct_head(
    database: &Transaction<'_>,
    library_id: &str,
    record_id: &str,
) -> Result<Option<AcceptedHead>, DurableAuthorityError> {
    let row = database
        .query_row(
            "SELECT h.accepted_revision, h.accepted_version_id, m.envelope_json,
                    c.sequence
             FROM record_heads h
             JOIN portable_records p ON p.record_id = h.record_id
             LEFT JOIN direct_authority_mutations m
               ON m.record_id = h.record_id AND m.version_id = h.accepted_version_id
             LEFT JOIN direct_authority_changes c ON c.transaction_id = m.transaction_id
             WHERE h.record_id = ?1 AND p.library_id = ?2",
            params![record_id, library_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(revision, version_id, envelope_json, sequence)| {
        let envelope_json = envelope_json.ok_or(DurableAuthorityError::StateUnavailable(
            "portable head has no direct ciphertext",
        ))?;
        let sequence = sequence.ok_or(DurableAuthorityError::StateUnavailable(
            "portable head has no direct acceptance sequence",
        ))?;
        let mutation: MutationEnvelope = serde_json::from_str(&envelope_json)
            .map_err(|_| DurableAuthorityError::StateUnavailable("stored mutation is invalid"))?;
        if mutation.record_id != record_id || mutation.version_id != version_id {
            return Err(DurableAuthorityError::StateUnavailable(
                "portable head and direct mutation diverged",
            ));
        }
        Ok(AcceptedHead {
            revision: u64_from_db(revision, "accepted_revision")?,
            version_id,
            ciphertext_hash: mutation.ciphertext_hash,
            authority_generation: mutation.authority_generation,
            acceptance_checkpoint: u64_from_db(sequence, "acceptance_checkpoint")?,
        })
    })
    .transpose()
}

fn head_basis_digest(
    database: &Transaction<'_>,
    profile: &Profile,
) -> Result<String, DurableAuthorityError> {
    let mut statement = database.prepare(
        "SELECT p.record_id, h.accepted_revision, h.accepted_version_id,
                h.content_hash, h.authority_generation
         FROM record_heads h
         JOIN portable_records p ON p.record_id = h.record_id
         WHERE p.library_id = ?1 AND p.kind IN ('note', 'category', 'folder')
         ORDER BY p.record_id",
    )?;
    let heads = statement
        .query_map([&profile.library_id], |row| {
            Ok(json!({
                "record_id": row.get::<_, String>(0)?,
                "revision": row.get::<_, i64>(1)?,
                "version_id": row.get::<_, String>(2)?,
                "content_hash": row.get::<_, String>(3)?,
                "authority_generation": row.get::<_, i64>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(canonical_sha256(&json!({
        "library_id": profile.library_id,
        "authority_generation": profile.authority_generation,
        "purge_generation": profile.purge_generation,
        "key_epoch": profile.key_epoch,
        "high_water_cursor": profile.high_water_cursor,
        "state_revision": profile.state_revision,
        "heads": heads,
    })))
}

fn terminal_basis_digest(receipt: &TransactionReceipt) -> String {
    canonical_sha256(&json!({ "terminal_receipt": receipt }))
}

fn load_profile(database: &Connection, library_id: &str) -> Result<Profile, DurableAuthorityError> {
    let row = database
        .query_row(
            "SELECT p.environment, p.library_data_class, p.readiness_state,
                    p.capabilities_json, p.high_water_cursor, p.state_revision,
                    l.authority_generation, l.purge_generation, l.current_key_epoch
             FROM direct_authority_profiles p
             JOIN libraries l ON l.library_id = p.library_id
             WHERE p.library_id = ?1",
            [library_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((
        environment,
        data_class,
        readiness,
        capabilities,
        cursor,
        revision,
        generation,
        purge,
        key_epoch,
    )) = row
    else {
        return Err(DurableAuthorityError::StateUnavailable(
            "fixture authority profile is missing",
        ));
    };
    if environment != "development" || data_class != "sanitized_fixture" {
        return Err(DurableAuthorityError::FixtureOnly);
    }
    if readiness != "fixture_ready" {
        return Err(DurableAuthorityError::StateUnavailable(
            "fixture authority profile is not ready",
        ));
    }
    let capabilities: ProtocolCapabilities = serde_json::from_str(&capabilities)
        .map_err(|_| DurableAuthorityError::StateUnavailable("capabilities are invalid"))?;
    capabilities.validate()?;
    let key_epoch = u64_from_db(key_epoch, "current_key_epoch")?;
    if key_epoch == 0 {
        return Err(DurableAuthorityError::StateUnavailable("key epoch is zero"));
    }
    Ok(Profile {
        library_id: library_id.to_owned(),
        authority_generation: u64_from_db(generation, "authority_generation")?,
        purge_generation: u64_from_db(purge, "purge_generation")?,
        key_epoch,
        high_water_cursor: u64_from_db(cursor, "high_water_cursor")?,
        state_revision: u64_from_db(revision, "state_revision")?,
        capabilities,
    })
}

fn require_active_device(
    database: &Transaction<'_>,
    library_id: &str,
    device_id: &str,
) -> Result<(), DurableAuthorityError> {
    let state: Option<(String, String)> = database
        .query_row(
            "SELECT role, enrollment_state FROM portable_devices
             WHERE device_id = ?1 AND library_id = ?2",
            params![device_id, library_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match state
        .as_ref()
        .map(|(role, state)| (role.as_str(), state.as_str()))
    {
        Some(("replica", "active")) => Ok(()),
        Some(("replica", "revoked")) => Err(ProtocolError::DeviceRevoked.into()),
        Some(_) => Err(DurableAuthorityError::StateUnavailable(
            "direct device role or state is invalid",
        )),
        None => Err(ProtocolError::DeviceUnknown.into()),
    }
}

fn validate_ack_candidate(
    database: &Transaction<'_>,
    profile: &Profile,
    device_id: &str,
    high_water_cursor: u64,
    checkpoint_digest: &str,
) -> Result<(), DurableAuthorityError> {
    if checkpoint_digest.len() != 64
        || !checkpoint_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(DurableAuthorityError::InvalidInput("checkpoint_digest"));
    }
    let issued: bool = database.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM direct_sync_checkpoints
           WHERE library_id = ?1 AND authority_generation = ?2
             AND high_water_cursor = ?3 AND checkpoint_digest = ?4
         )",
        params![
            profile.library_id,
            i64_from_u64(profile.authority_generation, "authority_generation")?,
            i64_from_u64(high_water_cursor, "high_water_cursor")?,
            checkpoint_digest,
        ],
        |row| row.get(0),
    )?;
    if !issued {
        return Err(DurableAuthorityError::AckMismatch);
    }
    let existing: Option<(Option<i64>, Option<String>)> = database
        .query_row(
            "SELECT acknowledged_cursor, checkpoint_digest
             FROM direct_device_sync_state
             WHERE device_id = ?1 AND library_id = ?2",
            params![device_id, profile.library_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((cursor, digest)) = existing else {
        return Err(DurableAuthorityError::StateUnavailable(
            "active direct device is missing sync state",
        ));
    };
    if let Some(cursor) = cursor {
        let stored_cursor = u64_from_db(cursor, "acknowledged_cursor")?;
        if stored_cursor == high_water_cursor && digest.as_deref() == Some(checkpoint_digest) {
            return Ok(());
        }
        if high_water_cursor <= stored_cursor {
            return Err(DurableAuthorityError::AckMismatch);
        }
    }
    Ok(())
}

fn validate_generation_floors(
    profile: &Profile,
    transaction: &SignedTransaction,
) -> Result<(), DurableAuthorityError> {
    let manifest = &transaction.manifest;
    if manifest.authority_generation < profile.authority_generation {
        return Err(ProtocolError::AuthorityGenerationStale {
            minimum: profile.authority_generation,
            provided: manifest.authority_generation,
        }
        .into());
    }
    if manifest.authority_generation > profile.authority_generation {
        return Err(ProtocolError::AuthorityGenerationAhead {
            current: profile.authority_generation,
            provided: manifest.authority_generation,
        }
        .into());
    }
    if manifest.purge_generation < profile.purge_generation {
        return Err(ProtocolError::PurgeGenerationStale {
            minimum: profile.purge_generation,
            provided: manifest.purge_generation,
        }
        .into());
    }
    if manifest.purge_generation > profile.purge_generation {
        return Err(ProtocolError::PurgeGenerationAhead {
            current: profile.purge_generation,
            provided: manifest.purge_generation,
        }
        .into());
    }
    if manifest.key_epoch < profile.key_epoch {
        return Err(ProtocolError::KeyEpochStale {
            minimum: profile.key_epoch,
            provided: manifest.key_epoch,
        }
        .into());
    }
    if manifest.key_epoch > profile.key_epoch {
        return Err(ProtocolError::KeyEpochAhead {
            current: profile.key_epoch,
            provided: manifest.key_epoch,
        }
        .into());
    }
    Ok(())
}

fn require_notes_capabilities(
    capabilities: &ProtocolCapabilities,
) -> Result<(), DurableAuthorityError> {
    let kinds: BTreeSet<_> = capabilities
        .record_kinds
        .keys()
        .map(String::as_str)
        .collect();
    if kinds != BTreeSet::from(["category", "folder", "note"]) {
        return Err(DurableAuthorityError::StateUnavailable(
            "fixture authority is not the exact Notes slice",
        ));
    }
    Ok(())
}

fn require_portable_notes_schema(connection: &Connection) -> Result<(), DurableAuthorityError> {
    for table in [
        "libraries",
        "library_scopes",
        "portable_devices",
        "portable_records",
        "change_transactions",
        "record_versions",
        "record_heads",
        "change_log",
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(DurableAuthorityError::StateUnavailable(
                "portable Notes schema is incomplete",
            ));
        }
    }
    Ok(())
}

fn portable_timestamp(timestamp_ms: i64) -> Result<String, DurableAuthorityError> {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(DurableAuthorityError::InvalidInput("authority_now"))
}

fn lifecycle_name(state: &LifecycleState) -> &'static str {
    match state {
        LifecycleState::Active => "active",
        LifecycleState::Trash => "trash",
        LifecycleState::Tombstone => "tombstone",
    }
}

fn authority_name(kind: &AuthorityKind) -> &'static str {
    match kind {
        AuthorityKind::Noted => "noted",
        AuthorityKind::External => "external",
        AuthorityKind::Derived => "derived",
    }
}

fn scope_name(class: &ScopeClass) -> &'static str {
    match class {
        ScopeClass::Work => "work",
        ScopeClass::Personal => "personal",
        ScopeClass::Unknown => "unknown",
    }
}

fn u64_from_db(value: i64, field: &'static str) -> Result<u64, DurableAuthorityError> {
    u64::try_from(value).map_err(|_| DurableAuthorityError::StateUnavailable(field))
}

fn i64_from_u64(value: u64, field: &'static str) -> Result<i64, DurableAuthorityError> {
    i64::try_from(value).map_err(|_| DurableAuthorityError::InvalidInput(field))
}
