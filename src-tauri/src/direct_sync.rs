//! HTTP-independent boundary for the six direct-sync operations.
//!
//! A network adapter may translate an HTTPS request into [`DirectRequest`], but
//! it cannot select a Tauri command or an arbitrary backend function. This
//! module accepts only the fixed `/sync/v1` routes below, authenticates their
//! complete typed envelopes, and delegates sequencing through the narrow
//! [`DirectSyncAuthority`] trait.
//!
//! The implementation remains restricted to sanitized fixture libraries until
//! the production cryptography provider and external review in Decision 008 are
//! complete.

use crate::pairing_protocol::{
    Environment, LibraryDataClass, PairingCrypto, PairingError, PairingMachine, RecordKind,
};
use crate::portable::{canonical_sha256, is_uuid_v7};
use crate::sync_protocol::{
    negotiate_capabilities, AuthorityState, BootstrapRecord, BootstrapSnapshot, ChangePage,
    MutationEnvelope, NegotiatedCapabilities, ProtocolCapabilities, ProtocolError,
    ReceiptDisposition, SignedTransaction, SubmitOutcome, TransactionReceipt,
    BOOTSTRAP_SNAPSHOT_VERSION, MAX_PULL_PAGE_CHANGES, SYNC_PROTOCOL_VERSION,
};
use serde::de::{
    DeserializeOwned, DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

pub const DIRECT_SYNC_CONTENT_TYPE: &str = "application/json";
pub const MAX_DIRECT_PULL_CHANGES: u32 = 64;
pub const MAX_DIRECT_BOOTSTRAP_RECORDS: u32 = 64;
pub const MAX_DIRECT_SIGNATURE_BYTES: usize = 512;
pub const MAX_DIRECT_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_DIRECT_TRANSACTION_MEMBERS: u32 = 128;
pub const MAX_DIRECT_TRANSACTION_BYTES: u64 = 512 * 1024;
pub const DIRECT_TRANSACTION_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const DIRECT_TRANSACTION_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_DIRECT_JSON_DEPTH: usize = 12;
pub const MAX_DIRECT_JSON_OBJECT_MEMBERS: usize = 128;
pub const MAX_DIRECT_JSON_TOTAL_MEMBERS: usize = 1_100_000;
pub const MAX_DIRECT_JSON_ARRAY_ELEMENTS: usize = 1_048_576;
pub const MAX_DIRECT_JSON_STRING_BYTES: usize = 256 * 1024;
pub const MAX_DIRECT_JSON_TOTAL_STRING_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DirectEndpoint {
    Negotiate,
    Bootstrap,
    Push,
    Pull,
    Checkpoint,
    Ack,
}

impl DirectEndpoint {
    pub const ALL: [Self; 6] = [
        Self::Negotiate,
        Self::Bootstrap,
        Self::Push,
        Self::Pull,
        Self::Checkpoint,
        Self::Ack,
    ];

    pub const fn path(self) -> &'static str {
        match self {
            Self::Negotiate => "/sync/v1/negotiate",
            Self::Bootstrap => "/sync/v1/bootstrap",
            Self::Push => "/sync/v1/push",
            Self::Pull => "/sync/v1/pull",
            Self::Checkpoint => "/sync/v1/checkpoint",
            Self::Ack => "/sync/v1/ack",
        }
    }

    fn parse(target: &str) -> Result<Self, DirectSyncError> {
        if target.contains('?') || target.contains('#') {
            return Err(DirectSyncError::CredentialsInTarget);
        }
        Self::ALL
            .into_iter()
            .find(|endpoint| endpoint.path() == target)
            .ok_or(DirectSyncError::RouteNotFound)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointLimits {
    pub request_bytes: usize,
    pub response_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSyncLimits {
    pub negotiate: EndpointLimits,
    pub bootstrap: EndpointLimits,
    pub push: EndpointLimits,
    pub pull: EndpointLimits,
    pub checkpoint: EndpointLimits,
    pub ack: EndpointLimits,
}

impl Default for DirectSyncLimits {
    fn default() -> Self {
        Self {
            negotiate: EndpointLimits {
                request_bytes: 8 * 1024,
                response_bytes: 64 * 1024,
            },
            bootstrap: EndpointLimits {
                request_bytes: 4 * 1024,
                response_bytes: 4 * 1024 * 1024,
            },
            push: EndpointLimits {
                request_bytes: DIRECT_TRANSACTION_REQUEST_BYTES,
                response_bytes: 256 * 1024,
            },
            pull: EndpointLimits {
                request_bytes: 4 * 1024,
                response_bytes: DIRECT_TRANSACTION_RESPONSE_BYTES,
            },
            checkpoint: EndpointLimits {
                request_bytes: 4 * 1024,
                response_bytes: 32 * 1024,
            },
            ack: EndpointLimits {
                request_bytes: 4 * 1024,
                response_bytes: 16 * 1024,
            },
        }
    }
}

impl DirectSyncLimits {
    fn for_endpoint(&self, endpoint: DirectEndpoint) -> EndpointLimits {
        match endpoint {
            DirectEndpoint::Negotiate => self.negotiate,
            DirectEndpoint::Bootstrap => self.bootstrap,
            DirectEndpoint::Push => self.push,
            DirectEndpoint::Pull => self.pull,
            DirectEndpoint::Checkpoint => self.checkpoint,
            DirectEndpoint::Ack => self.ack,
        }
    }

    fn validate(&self) -> Result<(), DirectSyncError> {
        for endpoint in DirectEndpoint::ALL {
            let limit = self.for_endpoint(endpoint);
            if limit.request_bytes == 0
                || limit.request_bytes > MAX_DIRECT_REQUEST_BYTES
                || limit.response_bytes == 0
                || limit.response_bytes > 4 * 1024 * 1024
            {
                return Err(DirectSyncError::InvalidConfiguration);
            }
        }
        // The v1 capability advertises a 512 KiB opaque transaction. JSON
        // currently represents byte vectors as integer arrays, so the push and
        // pull wire ceilings must reserve the full 4 MiB envelope budget. A
        // smaller adapter would promise transactions it cannot round-trip.
        if self.push.request_bytes < DIRECT_TRANSACTION_REQUEST_BYTES
            || self.pull.response_bytes < DIRECT_TRANSACTION_RESPONSE_BYTES
        {
            return Err(DirectSyncError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectSyncConfig {
    pub library_id: String,
    pub authority_generation: u64,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub server_spki_sha256: Vec<u8>,
    pub limits: DirectSyncLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecureTransportEvidence {
    /// Supplied by the TLS adapter, never deserialized from the request body.
    pub tls_version: String,
    pub used_zero_rtt: bool,
    pub server_spki_sha256: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectRequest {
    pub method: String,
    pub target: String,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub body: Vec<u8>,
    pub authority_now: u64,
    pub transport: SecureTransportEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSyncRequest<T> {
    pub protocol_version: u32,
    pub request_id: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: u64,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub payload: T,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedSyncResponse<T> {
    pub protocol_version: u32,
    pub request_id: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: u64,
    pub payload: T,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NegotiateRequest {
    pub capabilities: ProtocolCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NegotiateResponse {
    pub negotiated: NegotiatedCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapRequest {
    pub requested_record_kinds: BTreeSet<String>,
    pub checkpoint_digest: Option<String>,
    pub after_record_id: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapResponse {
    pub page: BootstrapPage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapPage {
    pub contract_version: String,
    pub library_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub high_water_cursor: u64,
    pub checkpoint_digest: String,
    pub requested_after_record_id: Option<String>,
    pub next_after_record_id: Option<String>,
    pub has_more: bool,
    pub records: Vec<BootstrapRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushRequest {
    pub transaction: SignedTransaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushResponse {
    pub receipt: TransactionReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    pub cursor: u64,
    pub limit: u32,
    pub requested_record_kinds: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullResponse {
    pub page: ChangePage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    pub known_cursor: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncCheckpoint {
    pub contract_version: String,
    pub library_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub high_water_cursor: u64,
    pub checkpoint_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointResponse {
    pub checkpoint: SyncCheckpoint,
    pub changed_since_known_cursor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckRequest {
    pub high_water_cursor: u64,
    pub checkpoint_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckReceipt {
    pub device_id: String,
    pub high_water_cursor: u64,
    pub checkpoint_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckResponse {
    pub receipt: AckReceipt,
}

/// Production implementations verify the device signature, every mutation's
/// authenticated ciphertext/signature, and authenticate responses. The core
/// provides no default implementation and therefore cannot silently fall back
/// to fixture cryptography.
pub trait DirectSyncCrypto: Send + Sync + 'static {
    fn verify_request_signature(
        &self,
        endpoint: DirectEndpoint,
        device_id: &str,
        signing_digest: &str,
        signature: &[u8],
    ) -> Result<(), ()>;

    fn verify_mutation_ciphertext(
        &self,
        device_id: &str,
        mutation: &MutationEnvelope,
    ) -> Result<(), ()>;

    fn authenticate_response(
        &self,
        endpoint: DirectEndpoint,
        signing_digest: &str,
    ) -> Result<Vec<u8>, ()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityStoreError {
    Protocol(ProtocolError),
    AckMismatch,
    StateUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityIdentity {
    pub library_id: String,
    pub authority_generation: u64,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
}

impl From<ProtocolError> for AuthorityStoreError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

/// The service depends only on these six data operations. A durable SQLite
/// adapter can implement this trait without exposing database paths or desktop
/// command dispatch.
pub trait DirectSyncAuthority: Send {
    fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError>;
    fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError>;
    fn bootstrap(&self) -> Result<BootstrapSnapshot, AuthorityStoreError>;
    fn pull(&self, cursor: u64, limit: u32) -> Result<ChangePage, AuthorityStoreError>;
    fn push(
        &mut self,
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<SubmitOutcome, AuthorityStoreError>;
    fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError>;
    fn acknowledge(
        &mut self,
        device_id: &str,
        cursor: u64,
        checkpoint_digest: &str,
    ) -> Result<AckReceipt, AuthorityStoreError>;
    fn revoke_device(&mut self, device_id: &str) -> Result<(), AuthorityStoreError>;
}

/// In-memory M4 adapter around the deterministic convergence authority. It is
/// intentionally replaceable by the durable Notes/SQLite adapter.
pub struct AuthorityStateStore {
    state: AuthorityState,
    acknowledgements: BTreeMap<String, AckReceipt>,
    issued_checkpoints: Mutex<BTreeMap<u64, String>>,
}

impl AuthorityStateStore {
    pub fn new_fixture_only(state: AuthorityState) -> Self {
        Self {
            state,
            acknowledgements: BTreeMap::new(),
            issued_checkpoints: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn into_inner(self) -> AuthorityState {
        self.state
    }
}

impl DirectSyncAuthority for AuthorityStateStore {
    fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError> {
        // This adapter has no production metadata store by design. Its
        // constructor name and the service constructor's fixture gate prevent
        // these labels from being mistaken for caller-controlled identity.
        Ok(AuthorityIdentity {
            library_id: self.state.library_id().to_owned(),
            authority_generation: self.state.authority_generation(),
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
        })
    }

    fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError> {
        Ok(self.state.capabilities().clone())
    }

    fn bootstrap(&self) -> Result<BootstrapSnapshot, AuthorityStoreError> {
        let snapshot = self.state.bootstrap_snapshot()?;
        self.remember_checkpoint(snapshot.high_water_cursor, &snapshot.checkpoint_digest)?;
        Ok(snapshot)
    }

    fn pull(&self, cursor: u64, limit: u32) -> Result<ChangePage, AuthorityStoreError> {
        Ok(self.state.changes_after(cursor, limit)?)
    }

    fn push(
        &mut self,
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<SubmitOutcome, AuthorityStoreError> {
        Ok(self.state.submit_transaction(transaction, now)?)
    }

    fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError> {
        let checkpoint = checkpoint_from_snapshot(self.state.bootstrap_snapshot()?)?;
        self.remember_checkpoint(checkpoint.high_water_cursor, &checkpoint.checkpoint_digest)?;
        Ok(checkpoint)
    }

    fn acknowledge(
        &mut self,
        device_id: &str,
        cursor: u64,
        checkpoint_digest: &str,
    ) -> Result<AckReceipt, AuthorityStoreError> {
        let checkpoint = self.checkpoint()?;
        if cursor > checkpoint.high_water_cursor {
            return Err(AuthorityStoreError::AckMismatch);
        }
        let issued_matches = self
            .issued_checkpoints
            .lock()
            .map_err(|_| AuthorityStoreError::StateUnavailable)?
            .get(&cursor)
            .is_some_and(|digest| digest == checkpoint_digest);
        if !issued_matches {
            return Err(AuthorityStoreError::AckMismatch);
        }
        let proposed = AckReceipt {
            device_id: device_id.to_owned(),
            high_water_cursor: cursor,
            checkpoint_digest: checkpoint_digest.to_owned(),
        };
        match self.acknowledgements.get(device_id) {
            Some(existing) if existing == &proposed => Ok(existing.clone()),
            Some(existing) if proposed.high_water_cursor < existing.high_water_cursor => {
                Err(AuthorityStoreError::AckMismatch)
            }
            Some(existing) if proposed.high_water_cursor == existing.high_water_cursor => {
                Err(AuthorityStoreError::AckMismatch)
            }
            Some(_) | None => {
                self.acknowledgements
                    .insert(device_id.to_owned(), proposed.clone());
                Ok(proposed)
            }
        }
    }

    fn revoke_device(&mut self, device_id: &str) -> Result<(), AuthorityStoreError> {
        self.state.revoke_device(device_id)?;
        Ok(())
    }
}

impl AuthorityStateStore {
    fn remember_checkpoint(
        &self,
        cursor: u64,
        checkpoint_digest: &str,
    ) -> Result<(), AuthorityStoreError> {
        let mut issued = self
            .issued_checkpoints
            .lock()
            .map_err(|_| AuthorityStoreError::StateUnavailable)?;
        match issued.get(&cursor) {
            Some(existing) if existing != checkpoint_digest => {
                Err(AuthorityStoreError::StateUnavailable)
            }
            Some(_) => Ok(()),
            None => {
                issued.insert(cursor, checkpoint_digest.to_owned());
                Ok(())
            }
        }
    }
}

fn checkpoint_from_snapshot(
    snapshot: BootstrapSnapshot,
) -> Result<SyncCheckpoint, AuthorityStoreError> {
    snapshot.validate()?;
    Ok(SyncCheckpoint {
        contract_version: snapshot.contract_version,
        library_id: snapshot.library_id,
        authority_generation: snapshot.authority_generation,
        purge_generation: snapshot.purge_generation,
        key_epoch: snapshot.key_epoch,
        high_water_cursor: snapshot.high_water_cursor,
        checkpoint_digest: snapshot.checkpoint_digest,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectSyncError {
    MethodNotAllowed,
    RouteNotFound,
    CredentialsInTarget,
    UnsupportedContentType,
    UnsupportedContentEncoding,
    InsecureTransport,
    PinMismatch,
    RequestTooLarge,
    ResponseTooLarge,
    MalformedJson,
    InvalidEnvelope,
    FixtureOnly,
    RequestSignatureRejected,
    CiphertextRejected,
    DeviceNotAuthorized,
    DeviceRevoked,
    ScopeViolation,
    Protocol(ProtocolError),
    AckMismatch,
    BootstrapChanged,
    InvalidConfiguration,
    StateUnavailable,
}

impl DirectSyncError {
    pub const fn status(&self) -> u16 {
        match self {
            Self::MethodNotAllowed => 405,
            Self::RouteNotFound => 404,
            Self::CredentialsInTarget
            | Self::MalformedJson
            | Self::InvalidEnvelope
            | Self::InvalidConfiguration => 400,
            Self::UnsupportedContentType | Self::UnsupportedContentEncoding => 415,
            Self::InsecureTransport
            | Self::PinMismatch
            | Self::RequestSignatureRejected
            | Self::DeviceNotAuthorized => 401,
            Self::FixtureOnly | Self::DeviceRevoked | Self::ScopeViolation => 403,
            Self::RequestTooLarge | Self::ResponseTooLarge => 413,
            Self::CiphertextRejected => 422,
            Self::Protocol(error) => match error {
                ProtocolError::TransactionIdReuse
                | ProtocolError::MutationIdReuse
                | ProtocolError::CounterReuse
                | ProtocolError::CounterGap { .. }
                | ProtocolError::PriorTransactionPending
                | ProtocolError::ReplicaBaseMismatch => 409,
                ProtocolError::DeviceUnknown => 401,
                ProtocolError::DeviceRevoked => 403,
                _ => 422,
            },
            Self::AckMismatch | Self::BootstrapChanged => 409,
            Self::StateUnavailable => 503,
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::MethodNotAllowed => "method_not_allowed",
            Self::RouteNotFound => "route_not_found",
            Self::CredentialsInTarget => "query_or_fragment_rejected",
            Self::UnsupportedContentType => "unsupported_content_type",
            Self::UnsupportedContentEncoding => "unsupported_content_encoding",
            Self::InsecureTransport => "secure_transport_required",
            Self::PinMismatch => "tls_pin_mismatch",
            Self::RequestTooLarge => "request_too_large",
            Self::ResponseTooLarge => "response_too_large",
            Self::MalformedJson => "malformed_json",
            Self::InvalidEnvelope => "invalid_envelope",
            Self::FixtureOnly => "fixture_only",
            Self::RequestSignatureRejected => "request_signature_rejected",
            Self::CiphertextRejected => "ciphertext_rejected",
            Self::DeviceNotAuthorized => "device_not_authorized",
            Self::DeviceRevoked => "device_revoked",
            Self::ScopeViolation => "scope_violation",
            Self::Protocol(_) => "sync_protocol_rejected",
            Self::AckMismatch => "ack_mismatch",
            Self::BootstrapChanged => "bootstrap_changed_restart_required",
            Self::InvalidConfiguration => "invalid_configuration",
            Self::StateUnavailable => "state_unavailable",
        }
    }
}

impl fmt::Display for DirectSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for DirectSyncError {}

impl From<AuthorityStoreError> for DirectSyncError {
    fn from(value: AuthorityStoreError) -> Self {
        match value {
            AuthorityStoreError::Protocol(error) => Self::Protocol(error),
            AuthorityStoreError::AckMismatch => Self::AckMismatch,
            AuthorityStoreError::StateUnavailable => Self::StateUnavailable,
        }
    }
}

pub struct DirectSyncService<C, A, V>
where
    C: PairingCrypto,
    A: DirectSyncAuthority,
    V: DirectSyncCrypto,
{
    pairing: PairingMachine<C>,
    authority: Mutex<A>,
    /// Linearizes authorization, mutation commit, and device revocation. A
    /// revocation therefore takes effect either wholly before or wholly after
    /// an in-flight request; it cannot split authorization from commit.
    operation_gate: Mutex<()>,
    crypto: V,
    config: DirectSyncConfig,
}

impl<C, A, V> DirectSyncService<C, A, V>
where
    C: PairingCrypto,
    A: DirectSyncAuthority,
    V: DirectSyncCrypto,
{
    pub fn new(
        pairing: PairingMachine<C>,
        authority: A,
        crypto: V,
        config: DirectSyncConfig,
    ) -> Result<Self, DirectSyncError> {
        if !is_uuid_v7(&config.library_id)
            || config.authority_generation == 0
            || config.server_spki_sha256.len() != 32
            || config.environment != Environment::Development
            || config.library_data_class != LibraryDataClass::SanitizedFixture
        {
            return Err(DirectSyncError::InvalidConfiguration);
        }
        config.limits.validate()?;
        let authority_identity = authority.identity()?;
        if authority_identity.library_id != config.library_id
            || authority_identity.authority_generation != config.authority_generation
            || authority_identity.environment != config.environment
            || authority_identity.library_data_class != config.library_data_class
        {
            return Err(DirectSyncError::InvalidConfiguration);
        }
        let authority_capabilities = authority.capabilities()?;
        authority_capabilities
            .validate()
            .map_err(DirectSyncError::Protocol)?;
        // This is an authority-store invariant, not only a transport hint. It
        // prevents a pre-populated log from containing a transaction that the
        // direct adapter can never return to a replica.
        if authority_capabilities.max_transaction_members > MAX_DIRECT_TRANSACTION_MEMBERS
            || authority_capabilities.max_transaction_bytes > MAX_DIRECT_TRANSACTION_BYTES
        {
            return Err(DirectSyncError::InvalidConfiguration);
        }
        let authority_kinds: BTreeSet<_> = authority_capabilities
            .record_kinds
            .keys()
            .cloned()
            .collect();
        if authority_kinds != notes_slice_record_kinds() {
            return Err(DirectSyncError::InvalidConfiguration);
        }
        Ok(Self {
            pairing,
            authority: Mutex::new(authority),
            operation_gate: Mutex::new(()),
            crypto,
            config,
        })
    }

    pub fn handle(&self, request: DirectRequest) -> DirectResponse {
        let result = self
            .operation_gate
            .lock()
            .map_err(|_| DirectSyncError::StateUnavailable)
            .and_then(|_operation| self.handle_inner(request));
        match result {
            Ok(response) => response,
            Err(error) => error_response(error),
        }
    }

    /// The only supported service-level revocation path. It holds the same
    /// gate as request handling and updates both the enrollment ledger and the
    /// convergence authority before another request can begin.
    pub fn revoke_device(&self, device_id: &str, now_ms: i64) -> Result<(), DirectSyncError> {
        let _operation = self
            .operation_gate
            .lock()
            .map_err(|_| DirectSyncError::StateUnavailable)?;
        self.pairing
            .revoke_device(device_id, now_ms)
            .map_err(map_pairing_error)?;
        self.lock_authority()?.revoke_device(device_id)?;
        Ok(())
    }

    fn handle_inner(&self, request: DirectRequest) -> Result<DirectResponse, DirectSyncError> {
        if request.method != "POST" {
            return Err(DirectSyncError::MethodNotAllowed);
        }
        let endpoint = DirectEndpoint::parse(&request.target)?;
        if !is_json_content_type(request.content_type.as_deref()) {
            return Err(DirectSyncError::UnsupportedContentType);
        }
        if !matches!(request.content_encoding.as_deref(), None | Some("identity")) {
            return Err(DirectSyncError::UnsupportedContentEncoding);
        }
        self.validate_transport(&request.transport)?;
        let limits = self.config.limits.for_endpoint(endpoint);
        if request.body.len() > limits.request_bytes {
            return Err(DirectSyncError::RequestTooLarge);
        }

        match endpoint {
            DirectEndpoint::Negotiate => {
                let signed: SignedSyncRequest<NegotiateRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                let requested_kinds: BTreeSet<_> = signed
                    .payload
                    .capabilities
                    .record_kinds
                    .keys()
                    .cloned()
                    .collect();
                self.require_notes_slice(&requested_kinds)?;
                for (kind, capability) in &signed.payload.capabilities.record_kinds {
                    self.authorize_scope(&signed, kind, capability.max_read_schema_version, false)?;
                    if capability.max_write_schema_version > 0 {
                        self.authorize_scope(
                            &signed,
                            kind,
                            capability.max_write_schema_version,
                            true,
                        )?;
                    }
                }
                let authority = self.lock_authority()?;
                let direct_capabilities = authority.capabilities()?;
                let negotiated =
                    negotiate_capabilities(&direct_capabilities, &signed.payload.capabilities)
                        .map_err(DirectSyncError::Protocol)?;
                self.respond(
                    endpoint,
                    &signed,
                    NegotiateResponse { negotiated },
                    limits.response_bytes,
                )
            }
            DirectEndpoint::Bootstrap => {
                let signed: SignedSyncRequest<BootstrapRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                if signed.payload.limit == 0
                    || signed.payload.limit > MAX_DIRECT_BOOTSTRAP_RECORDS
                    || signed
                        .payload
                        .checkpoint_digest
                        .as_deref()
                        .is_some_and(|digest| !is_sha256(digest))
                    || signed
                        .payload
                        .after_record_id
                        .as_deref()
                        .is_some_and(|record_id| !is_uuid_v7(record_id))
                    || (signed.payload.after_record_id.is_some()
                        && signed.payload.checkpoint_digest.is_none())
                {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                self.require_notes_slice(&signed.payload.requested_record_kinds)?;
                self.authorize_requested_kinds(
                    &signed,
                    &signed.payload.requested_record_kinds,
                    false,
                )?;
                let authority = self.lock_authority()?;
                let snapshot = authority.bootstrap()?;
                self.validate_bootstrap_binding(&snapshot)?;
                if signed
                    .payload
                    .checkpoint_digest
                    .as_ref()
                    .is_some_and(|digest| digest != &snapshot.checkpoint_digest)
                {
                    return Err(DirectSyncError::BootstrapChanged);
                }
                let page = self.paginate_bootstrap(&signed, snapshot, limits.response_bytes)?;
                for record in &page.records {
                    if !signed
                        .payload
                        .requested_record_kinds
                        .contains(&record.mutation.record_kind)
                    {
                        return Err(DirectSyncError::ScopeViolation);
                    }
                    self.authorize_scope(
                        &signed,
                        &record.mutation.record_kind,
                        record.mutation.record_schema_version,
                        false,
                    )?;
                    self.crypto
                        .verify_mutation_ciphertext(&record.mutation.device_id, &record.mutation)
                        .map_err(|_| DirectSyncError::CiphertextRejected)?;
                }
                self.respond(
                    endpoint,
                    &signed,
                    BootstrapResponse { page },
                    limits.response_bytes,
                )
            }
            DirectEndpoint::Push => {
                let signed: SignedSyncRequest<PushRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                if signed.payload.transaction.members.is_empty()
                    || signed.payload.transaction.manifest.member_count
                        > MAX_DIRECT_TRANSACTION_MEMBERS
                    || signed.payload.transaction.manifest.byte_total > MAX_DIRECT_TRANSACTION_BYTES
                {
                    if signed.payload.transaction.members.is_empty() {
                        return Err(DirectSyncError::InvalidEnvelope);
                    }
                    return Err(DirectSyncError::RequestTooLarge);
                }
                if signed.payload.transaction.manifest.member_count
                    != signed.payload.transaction.members.len() as u32
                {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                for member in &signed.payload.transaction.members {
                    self.authorize_scope(
                        &signed,
                        &member.record_kind,
                        member.record_schema_version,
                        true,
                    )?;
                    if member.device_id != signed.device_id
                        || member.library_id != signed.library_id
                        || member.authority_generation != signed.authority_generation
                    {
                        return Err(DirectSyncError::InvalidEnvelope);
                    }
                }
                if signed.payload.transaction.manifest.device_id != signed.device_id
                    || signed.payload.transaction.manifest.library_id != signed.library_id
                    || signed.payload.transaction.manifest.authority_generation
                        != signed.authority_generation
                {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                for member in &signed.payload.transaction.members {
                    self.crypto
                        .verify_mutation_ciphertext(&signed.device_id, member)
                        .map_err(|_| DirectSyncError::CiphertextRejected)?;
                }
                self.ensure_transaction_is_pullable(&signed, &signed.payload.transaction)?;
                let mut authority = self.lock_authority()?;
                let outcome =
                    authority.push(signed.payload.transaction.clone(), request.authority_now)?;
                let receipt = match outcome {
                    SubmitOutcome::Terminal(receipt) | SubmitOutcome::Replay(receipt) => receipt,
                };
                self.validate_receipt_binding(&receipt, &signed.payload.transaction, false)?;
                self.respond(
                    endpoint,
                    &signed,
                    PushResponse { receipt },
                    limits.response_bytes,
                )
            }
            DirectEndpoint::Pull => {
                let signed: SignedSyncRequest<PullRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                if signed.payload.limit == 0 || signed.payload.limit > MAX_DIRECT_PULL_CHANGES {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                self.require_notes_slice(&signed.payload.requested_record_kinds)?;
                self.authorize_requested_kinds(
                    &signed,
                    &signed.payload.requested_record_kinds,
                    false,
                )?;
                let authority = self.lock_authority()?;
                let capabilities = authority.capabilities()?;
                let raw = authority.pull(
                    signed.payload.cursor,
                    signed.payload.limit.min(MAX_PULL_PAGE_CHANGES),
                )?;
                self.validate_pull_page_binding(&signed, &raw)?;
                let page =
                    self.filter_and_bound_pull(&signed, raw, &capabilities, limits.response_bytes)?;
                self.respond(
                    endpoint,
                    &signed,
                    PullResponse { page },
                    limits.response_bytes,
                )
            }
            DirectEndpoint::Checkpoint => {
                let signed: SignedSyncRequest<CheckpointRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                self.authorize_requested_kinds(&signed, &notes_slice_record_kinds(), false)?;
                let authority = self.lock_authority()?;
                let checkpoint = authority.checkpoint()?;
                self.validate_checkpoint_binding(&checkpoint)?;
                if signed
                    .payload
                    .known_cursor
                    .is_some_and(|cursor| cursor > checkpoint.high_water_cursor)
                {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                let changed_since_known_cursor = signed
                    .payload
                    .known_cursor
                    .is_none_or(|cursor| cursor != checkpoint.high_water_cursor);
                self.respond(
                    endpoint,
                    &signed,
                    CheckpointResponse {
                        checkpoint,
                        changed_since_known_cursor,
                    },
                    limits.response_bytes,
                )
            }
            DirectEndpoint::Ack => {
                let signed: SignedSyncRequest<AckRequest> = parse_request(&request.body)?;
                self.validate_request(endpoint, &signed)?;
                self.authorize_requested_kinds(&signed, &notes_slice_record_kinds(), false)?;
                if !is_sha256(&signed.payload.checkpoint_digest) {
                    return Err(DirectSyncError::InvalidEnvelope);
                }
                let mut authority = self.lock_authority()?;
                let receipt = authority.acknowledge(
                    &signed.device_id,
                    signed.payload.high_water_cursor,
                    &signed.payload.checkpoint_digest,
                )?;
                if receipt.device_id != signed.device_id
                    || receipt.high_water_cursor != signed.payload.high_water_cursor
                    || receipt.checkpoint_digest != signed.payload.checkpoint_digest
                {
                    return Err(DirectSyncError::StateUnavailable);
                }
                self.respond(
                    endpoint,
                    &signed,
                    AckResponse { receipt },
                    limits.response_bytes,
                )
            }
        }
    }

    fn validate_transport(
        &self,
        transport: &SecureTransportEvidence,
    ) -> Result<(), DirectSyncError> {
        if transport.tls_version != "1.3" || transport.used_zero_rtt {
            return Err(DirectSyncError::InsecureTransport);
        }
        if transport.server_spki_sha256 != self.config.server_spki_sha256 {
            return Err(DirectSyncError::PinMismatch);
        }
        Ok(())
    }

    fn validate_request<T: Serialize>(
        &self,
        endpoint: DirectEndpoint,
        request: &SignedSyncRequest<T>,
    ) -> Result<(), DirectSyncError> {
        if request.protocol_version != SYNC_PROTOCOL_VERSION
            || !is_uuid_v7(&request.request_id)
            || !is_uuid_v7(&request.library_id)
            || !is_uuid_v7(&request.device_id)
            || request.authority_generation == 0
            || request.signature.is_empty()
            || request.signature.len() > MAX_DIRECT_SIGNATURE_BYTES
        {
            return Err(DirectSyncError::InvalidEnvelope);
        }
        if request.library_data_class != self.config.library_data_class {
            return Err(DirectSyncError::FixtureOnly);
        }
        if request.library_id != self.config.library_id
            || request.authority_generation != self.config.authority_generation
            || request.environment != self.config.environment
        {
            return Err(DirectSyncError::DeviceNotAuthorized);
        }
        let digest = request_signing_digest(endpoint, request)?;
        self.crypto
            .verify_request_signature(endpoint, &request.device_id, &digest, &request.signature)
            .map_err(|_| DirectSyncError::RequestSignatureRejected)?;
        self.pairing
            .require_active_device(
                &request.device_id,
                &request.library_id,
                request.environment,
                request.authority_generation,
            )
            .map_err(map_pairing_error)
    }

    fn authorize_requested_kinds<T>(
        &self,
        request: &SignedSyncRequest<T>,
        kinds: &BTreeSet<String>,
        require_write: bool,
    ) -> Result<(), DirectSyncError> {
        if kinds.is_empty() || kinds.len() > 16 {
            return Err(DirectSyncError::InvalidEnvelope);
        }
        for kind in kinds {
            self.authorize_scope(request, kind, 1, require_write)?;
        }
        Ok(())
    }

    fn require_notes_slice(&self, kinds: &BTreeSet<String>) -> Result<(), DirectSyncError> {
        if kinds == &notes_slice_record_kinds() {
            Ok(())
        } else {
            Err(DirectSyncError::ScopeViolation)
        }
    }

    fn authorize_scope<T>(
        &self,
        request: &SignedSyncRequest<T>,
        kind: &str,
        schema_version: u32,
        require_write: bool,
    ) -> Result<(), DirectSyncError> {
        let scope = pairing_record_kind(kind).ok_or(DirectSyncError::ScopeViolation)?;
        let capability = self
            .pairing
            .require_active_device_scope(
                &request.device_id,
                &request.library_id,
                request.environment,
                request.authority_generation,
                scope,
                require_write,
            )
            .map_err(map_pairing_error)?;
        let supported_version = if require_write {
            capability.writer_version.unwrap_or(0)
        } else {
            capability.reader_version
        };
        if schema_version == 0 || schema_version > supported_version {
            return Err(DirectSyncError::ScopeViolation);
        }
        Ok(())
    }

    fn validate_bootstrap_binding(
        &self,
        snapshot: &BootstrapSnapshot,
    ) -> Result<(), DirectSyncError> {
        snapshot
            .validate()
            .map_err(|_| DirectSyncError::StateUnavailable)?;
        if snapshot.library_id != self.config.library_id
            || snapshot.authority_generation != self.config.authority_generation
        {
            return Err(DirectSyncError::StateUnavailable);
        }
        Ok(())
    }

    fn paginate_bootstrap(
        &self,
        request: &SignedSyncRequest<BootstrapRequest>,
        snapshot: BootstrapSnapshot,
        response_limit: usize,
    ) -> Result<BootstrapPage, DirectSyncError> {
        let BootstrapSnapshot {
            contract_version,
            library_id,
            authority_generation,
            purge_generation,
            key_epoch,
            high_water_cursor,
            mut records,
            checkpoint_digest,
        } = snapshot;
        records.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        let start = match request.payload.after_record_id.as_deref() {
            Some(after) => records
                .iter()
                .position(|record| record.record_id == after)
                .and_then(|index| index.checked_add(1))
                .ok_or(DirectSyncError::InvalidEnvelope)?,
            None => 0,
        };
        let requested_after_record_id = request.payload.after_record_id.clone();
        let mut page = BootstrapPage {
            contract_version,
            library_id,
            authority_generation,
            purge_generation,
            key_epoch,
            high_water_cursor,
            checkpoint_digest,
            requested_after_record_id,
            next_after_record_id: None,
            has_more: start < records.len(),
            records: Vec::new(),
        };

        let end = start
            .checked_add(request.payload.limit as usize)
            .unwrap_or(usize::MAX)
            .min(records.len());
        for record in records[start..end].iter().cloned() {
            page.next_after_record_id = Some(record.record_id.clone());
            page.records.push(record);
            page.has_more = start + page.records.len() < records.len();
            let projected = BootstrapResponse { page: page.clone() };
            if unsigned_response_size(request, &projected)? > response_limit {
                page.records.pop();
                page.next_after_record_id =
                    page.records.last().map(|record| record.record_id.clone());
                page.has_more = true;
                if page.records.is_empty() {
                    return Err(DirectSyncError::ResponseTooLarge);
                }
                break;
            }
        }
        if page.records.is_empty() && start == records.len() {
            page.has_more = false;
        }
        Ok(page)
    }

    fn validate_checkpoint_binding(
        &self,
        checkpoint: &SyncCheckpoint,
    ) -> Result<(), DirectSyncError> {
        if checkpoint.contract_version != BOOTSTRAP_SNAPSHOT_VERSION
            || checkpoint.library_id != self.config.library_id
            || checkpoint.authority_generation != self.config.authority_generation
            || checkpoint.key_epoch == 0
            || !is_sha256(&checkpoint.checkpoint_digest)
        {
            return Err(DirectSyncError::StateUnavailable);
        }
        Ok(())
    }

    fn validate_pull_page_binding(
        &self,
        request: &SignedSyncRequest<PullRequest>,
        page: &ChangePage,
    ) -> Result<(), DirectSyncError> {
        let remaining = page
            .high_water_cursor
            .checked_sub(page.requested_cursor)
            .ok_or(DirectSyncError::StateUnavailable)?;
        let expected_change_count = remaining.min(u64::from(request.payload.limit)) as usize;
        if page.requested_cursor != request.payload.cursor
            || page.next_cursor < page.requested_cursor
            || page.next_cursor > page.high_water_cursor
            || page.has_more != (page.next_cursor < page.high_water_cursor)
            || page.changes.len() != expected_change_count
        {
            return Err(DirectSyncError::StateUnavailable);
        }
        let mut expected_sequence = page
            .requested_cursor
            .checked_add(1)
            .ok_or(DirectSyncError::StateUnavailable)?;
        for change in &page.changes {
            if change.sequence != expected_sequence
                || change.sequence > page.next_cursor
                || change.transaction.manifest.library_id != self.config.library_id
                || change.transaction.manifest.authority_generation
                    != self.config.authority_generation
            {
                return Err(DirectSyncError::StateUnavailable);
            }
            self.validate_receipt_binding(&change.receipt, &change.transaction, true)?;
            if change.transaction_digest != change.transaction.signed_digest()
                || change.receipt.high_water_cursor != change.sequence
            {
                return Err(DirectSyncError::StateUnavailable);
            }
            expected_sequence = expected_sequence
                .checked_add(1)
                .ok_or(DirectSyncError::StateUnavailable)?;
        }
        let expected_next = page
            .changes
            .last()
            .map_or(page.requested_cursor, |change| change.sequence);
        if page.next_cursor != expected_next {
            return Err(DirectSyncError::StateUnavailable);
        }
        Ok(())
    }

    fn validate_receipt_binding(
        &self,
        receipt: &TransactionReceipt,
        transaction: &SignedTransaction,
        require_accepted: bool,
    ) -> Result<(), DirectSyncError> {
        let manifest = &transaction.manifest;
        let mut ordered_members = transaction.members.iter().collect::<Vec<_>>();
        ordered_members.sort_by_key(|member| member.transaction_member_index);
        let mutation_ids = ordered_members
            .iter()
            .map(|member| member.mutation_id.clone())
            .collect::<Vec<_>>();
        if receipt.library_id != manifest.library_id
            || receipt.transaction_id != manifest.transaction_id
            || receipt.transaction_digest != transaction.signed_digest()
            || receipt.mutation_ids != mutation_ids
            || receipt.device_id != manifest.device_id
            || receipt.device_transaction_counter != manifest.device_transaction_counter
            || receipt.authority_generation != manifest.authority_generation
            || receipt.purge_generation != manifest.purge_generation
        {
            return Err(DirectSyncError::StateUnavailable);
        }
        match &receipt.disposition {
            ReceiptDisposition::Accepted { advances } => {
                if advances.len() != ordered_members.len()
                    || ordered_members.iter().any(|member| {
                        !advances.iter().any(|advance| {
                            advance.record_id == member.record_id
                                && advance.record_kind == member.record_kind
                                && advance.record_schema_version == member.record_schema_version
                                && advance.base_revision == member.base_head_revision
                                && advance.base_version_id == member.base_head_version_id
                                && advance.revision == member.proposed_revision
                                && advance.version_id == member.version_id
                                && advance.ciphertext_hash == member.ciphertext_hash
                        })
                    })
                {
                    return Err(DirectSyncError::StateUnavailable);
                }
            }
            ReceiptDisposition::Conflict { conflicts } => {
                let mut record_ids = BTreeSet::new();
                if conflicts.is_empty()
                    || conflicts.iter().any(|conflict| {
                        !record_ids.insert(conflict.record_id.as_str())
                            || !ordered_members.iter().any(|member| {
                                member.record_id == conflict.record_id
                                    && member.version_id == conflict.proposed_version_id
                            })
                    })
                {
                    return Err(DirectSyncError::StateUnavailable);
                }
            }
            ReceiptDisposition::Rejected { .. } => {}
        }
        if require_accepted && !matches!(receipt.disposition, ReceiptDisposition::Accepted { .. }) {
            return Err(DirectSyncError::StateUnavailable);
        }
        Ok(())
    }

    fn ensure_transaction_is_pullable(
        &self,
        request: &SignedSyncRequest<PushRequest>,
        transaction: &SignedTransaction,
    ) -> Result<(), DirectSyncError> {
        let mut ordered_members = transaction.members.iter().collect::<Vec<_>>();
        ordered_members.sort_by_key(|member| member.transaction_member_index);
        let advances = ordered_members
            .iter()
            .map(|member| crate::sync_protocol::HeadAdvance {
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
            library_id: transaction.manifest.library_id.clone(),
            transaction_id: transaction.manifest.transaction_id.clone(),
            transaction_digest: transaction.signed_digest(),
            mutation_ids: ordered_members
                .iter()
                .map(|member| member.mutation_id.clone())
                .collect(),
            device_id: transaction.manifest.device_id.clone(),
            device_transaction_counter: transaction.manifest.device_transaction_counter,
            authority_generation: transaction.manifest.authority_generation,
            purge_generation: transaction.manifest.purge_generation,
            high_water_cursor: u64::MAX,
            disposition: ReceiptDisposition::Accepted { advances },
        };
        let projected = PullResponse {
            page: ChangePage {
                requested_cursor: u64::MAX - 1,
                next_cursor: u64::MAX,
                high_water_cursor: u64::MAX,
                has_more: false,
                changes: vec![crate::sync_protocol::AcceptedChange {
                    sequence: u64::MAX,
                    transaction_digest: transaction.signed_digest(),
                    transaction: transaction.clone(),
                    receipt,
                }],
            },
        };
        let max_response_bytes = self
            .config
            .limits
            .for_endpoint(DirectEndpoint::Pull)
            .response_bytes;
        if unsigned_response_size(request, &projected)? > max_response_bytes {
            return Err(DirectSyncError::ResponseTooLarge);
        }
        Ok(())
    }

    fn filter_and_bound_pull(
        &self,
        request: &SignedSyncRequest<PullRequest>,
        raw: ChangePage,
        capabilities: &ProtocolCapabilities,
        response_limit: usize,
    ) -> Result<ChangePage, DirectSyncError> {
        let requested = &request.payload.requested_record_kinds;

        let mut page = ChangePage {
            requested_cursor: raw.requested_cursor,
            next_cursor: raw.requested_cursor,
            high_water_cursor: raw.high_water_cursor,
            has_more: raw.requested_cursor < raw.high_water_cursor,
            changes: Vec::new(),
        };
        for change in raw.changes {
            let requested_members = change
                .transaction
                .members
                .iter()
                .filter(|member| requested.contains(&member.record_kind))
                .count();
            if requested_members > 0 && requested_members != change.transaction.members.len() {
                return Err(DirectSyncError::ScopeViolation);
            }
            if requested_members == change.transaction.members.len() {
                let negotiated = negotiate_capabilities(capabilities, capabilities)
                    .map_err(|_| DirectSyncError::StateUnavailable)?;
                change
                    .transaction
                    .validate(0, &negotiated)
                    .map_err(|_| DirectSyncError::StateUnavailable)?;
                for member in &change.transaction.members {
                    self.authorize_scope(
                        request,
                        &member.record_kind,
                        member.record_schema_version,
                        false,
                    )?;
                    self.crypto
                        .verify_mutation_ciphertext(&member.device_id, member)
                        .map_err(|_| DirectSyncError::CiphertextRejected)?;
                }
                page.changes.push(change.clone());
                page.next_cursor = change.sequence;
                page.has_more = page.next_cursor < page.high_water_cursor;
                let projected = PullResponse { page: page.clone() };
                if unsigned_response_size(request, &projected)? > response_limit {
                    page.changes.pop();
                    page.next_cursor = change.sequence.saturating_sub(1);
                    page.has_more = true;
                    if page.next_cursor == page.requested_cursor && page.changes.is_empty() {
                        return Err(DirectSyncError::ResponseTooLarge);
                    }
                    break;
                }
            } else {
                // Transactions are atomic: never expose a subset. Advancing the
                // cursor over an unsubscribed transaction is safe because the
                // device explicitly omitted every member's record kind.
                page.next_cursor = change.sequence;
                page.has_more = page.next_cursor < page.high_water_cursor;
            }
        }
        Ok(page)
    }

    fn respond<T: Serialize>(
        &self,
        endpoint: DirectEndpoint,
        request: &SignedSyncRequest<impl Serialize>,
        payload: T,
        max_bytes: usize,
    ) -> Result<DirectResponse, DirectSyncError> {
        let mut response = SignedSyncResponse {
            protocol_version: SYNC_PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            library_id: request.library_id.clone(),
            device_id: request.device_id.clone(),
            authority_generation: request.authority_generation,
            payload,
            signature: Vec::new(),
        };
        let digest = response_signing_digest(endpoint, &response)?;
        response.signature = self
            .crypto
            .authenticate_response(endpoint, &digest)
            .map_err(|_| DirectSyncError::StateUnavailable)?;
        if response.signature.is_empty() || response.signature.len() > MAX_DIRECT_SIGNATURE_BYTES {
            return Err(DirectSyncError::StateUnavailable);
        }
        let body = serde_json::to_vec(&response).map_err(|_| DirectSyncError::StateUnavailable)?;
        if body.len() > max_bytes {
            return Err(DirectSyncError::ResponseTooLarge);
        }
        Ok(DirectResponse {
            status: 200,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body,
        })
    }

    fn lock_authority(&self) -> Result<std::sync::MutexGuard<'_, A>, DirectSyncError> {
        self.authority
            .lock()
            .map_err(|_| DirectSyncError::StateUnavailable)
    }
}

fn pairing_record_kind(kind: &str) -> Option<RecordKind> {
    match kind {
        "note" => Some(RecordKind::Note),
        "category" => Some(RecordKind::Category),
        "folder" => Some(RecordKind::Folder),
        "media" => Some(RecordKind::Media),
        _ => None,
    }
}

fn notes_slice_record_kinds() -> BTreeSet<String> {
    ["note", "category", "folder"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn map_pairing_error(error: PairingError) -> DirectSyncError {
    match error {
        PairingError::DeviceRevoked => DirectSyncError::DeviceRevoked,
        PairingError::ScopeNotGranted | PairingError::CapabilityMismatch => {
            DirectSyncError::ScopeViolation
        }
        _ => DirectSyncError::DeviceNotAuthorized,
    }
}

fn parse_request<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DirectSyncError> {
    if bytes.len() > MAX_DIRECT_REQUEST_BYTES {
        return Err(DirectSyncError::RequestTooLarge);
    }
    let budget = Arc::new(Mutex::new(DirectJsonBudget::default()));
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = DirectValueSeed {
        depth: 0,
        budget: Arc::clone(&budget),
    }
    .deserialize(&mut deserializer)
    .map_err(|_| DirectSyncError::MalformedJson)?;
    deserializer
        .end()
        .map_err(|_| DirectSyncError::MalformedJson)?;
    serde_json::from_value(value).map_err(|_| DirectSyncError::MalformedJson)
}

#[derive(Default)]
struct DirectJsonBudget {
    members: usize,
    total_string_bytes: usize,
}

struct DirectValueSeed {
    depth: usize,
    budget: Arc<Mutex<DirectJsonBudget>>,
}

impl<'de> DeserializeSeed<'de> for DirectValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > MAX_DIRECT_JSON_DEPTH {
            return Err(D::Error::custom("JSON nesting limit exceeded"));
        }
        deserializer.deserialize_any(DirectValueVisitor {
            depth: self.depth,
            budget: self.budget,
        })
    }
}

struct DirectValueVisitor {
    depth: usize,
    budget: Arc<Mutex<DirectJsonBudget>>,
}

impl<'de> Visitor<'de> for DirectValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded direct-sync JSON")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: DeError>(self, _value: f64) -> Result<Self::Value, E> {
        Err(E::custom("floating-point values are not permitted"))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E: DeError>(self, value: &str) -> Result<Self::Value, E> {
        charge_direct_string::<E>(&self.budget, value)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: DeError>(self, value: String) -> Result<Self::Value, E> {
        charge_direct_string::<E>(&self.budget, &value)?;
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(DirectValueSeed {
            depth: self.depth + 1,
            budget: Arc::clone(&self.budget),
        })? {
            if values.len() >= MAX_DIRECT_JSON_ARRAY_ELEMENTS {
                return Err(A::Error::custom("JSON array element limit exceeded"));
            }
            charge_direct_member::<A::Error>(&self.budget)?;
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.len() >= MAX_DIRECT_JSON_OBJECT_MEMBERS {
                return Err(A::Error::custom("JSON object member limit exceeded"));
            }
            charge_direct_string::<A::Error>(&self.budget, &key)?;
            charge_direct_member::<A::Error>(&self.budget)?;
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON object member"));
            }
            let value = map.next_value_seed(DirectValueSeed {
                depth: self.depth + 1,
                budget: Arc::clone(&self.budget),
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

fn charge_direct_member<E: DeError>(budget: &Mutex<DirectJsonBudget>) -> Result<(), E> {
    let mut budget = budget
        .lock()
        .map_err(|_| E::custom("JSON budget unavailable"))?;
    budget.members += 1;
    if budget.members > MAX_DIRECT_JSON_TOTAL_MEMBERS {
        return Err(E::custom("JSON total member limit exceeded"));
    }
    Ok(())
}

fn charge_direct_string<E: DeError>(
    budget: &Mutex<DirectJsonBudget>,
    value: &str,
) -> Result<(), E> {
    if value.len() > MAX_DIRECT_JSON_STRING_BYTES {
        return Err(E::custom("JSON string limit exceeded"));
    }
    let mut budget = budget
        .lock()
        .map_err(|_| E::custom("JSON budget unavailable"))?;
    budget.total_string_bytes += value.len();
    if budget.total_string_bytes > MAX_DIRECT_JSON_TOTAL_STRING_BYTES {
        return Err(E::custom("JSON total string limit exceeded"));
    }
    Ok(())
}

fn is_json_content_type(content_type: Option<&str>) -> bool {
    matches!(
        content_type,
        Some("application/json") | Some("application/json; charset=utf-8")
    )
}

pub fn request_signing_digest<T: Serialize>(
    endpoint: DirectEndpoint,
    request: &SignedSyncRequest<T>,
) -> Result<String, DirectSyncError> {
    let mut value = serde_json::to_value(request).map_err(|_| DirectSyncError::InvalidEnvelope)?;
    value
        .as_object_mut()
        .ok_or(DirectSyncError::InvalidEnvelope)?
        .insert("signature".to_owned(), json!([]));
    Ok(canonical_sha256(&json!({
        "domain": "noted.direct-sync.v1/request",
        "endpoint": endpoint.path(),
        "request": value,
    })))
}

pub fn response_signing_digest<T: Serialize>(
    endpoint: DirectEndpoint,
    response: &SignedSyncResponse<T>,
) -> Result<String, DirectSyncError> {
    let mut value =
        serde_json::to_value(response).map_err(|_| DirectSyncError::StateUnavailable)?;
    value
        .as_object_mut()
        .ok_or(DirectSyncError::StateUnavailable)?
        .insert("signature".to_owned(), json!([]));
    Ok(canonical_sha256(&json!({
        "domain": "noted.direct-sync.v1/response",
        "endpoint": endpoint.path(),
        "response": value,
    })))
}

fn unsigned_response_size<RequestPayload: Serialize, ResponsePayload: Serialize>(
    request: &SignedSyncRequest<RequestPayload>,
    payload: &ResponsePayload,
) -> Result<usize, DirectSyncError> {
    let response = SignedSyncResponse {
        protocol_version: SYNC_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        library_id: request.library_id.clone(),
        device_id: request.device_id.clone(),
        authority_generation: request.authority_generation,
        payload,
        signature: vec![u8::MAX; MAX_DIRECT_SIGNATURE_BYTES],
    };
    serde_json::to_vec(&response)
        .map(|bytes| bytes.len())
        .map_err(|_| DirectSyncError::StateUnavailable)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn error_response(error: DirectSyncError) -> DirectResponse {
    let body = serde_json::to_vec(&json!({
        "error": {
            "code": error.code(),
        }
    }))
    .unwrap_or_else(|_| b"{\"error\":{\"code\":\"state_unavailable\"}}".to_vec());
    DirectResponse {
        status: error.status(),
        content_type: DIRECT_SYNC_CONTENT_TYPE,
        body,
    }
}
