//! Phone-side direct-sync request boundary.
//!
//! This module deliberately does not know about Tauri commands, URLs, Bonjour
//! metadata, or JavaScript.  A caller supplies an already authenticated,
//! endpoint-scoped transport session, an active pairing profile restored from
//! the authenticated activation record, native signing/verification, and a
//! durable exact-wire journal.  The actor keeps one request in flight and
//! commits its exact signed bytes before the first socket write.

use crate::direct_sync::{
    parse_bounded_direct_json, request_signing_bytes, response_signing_bytes, AckRequest,
    BootstrapRequest, BootstrapResponse, CheckpointRequest, DirectEndpoint, DirectResponse,
    DirectSyncLimits, NegotiateRequest, PullRequest, PullResponse, PushRequest, SignedSyncRequest,
    SignedSyncResponse, DIRECT_SYNC_CONTENT_TYPE, MAX_DIRECT_SIGNATURE_BYTES,
};
use crate::pairing_protocol::{Environment, KindCapability, LibraryDataClass, RecordKind};
use crate::portable::{canonical_json, is_uuid_v7};
use crate::sync_protocol::SYNC_PROTOCOL_VERSION;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

const P256_PUBLIC_KEY_BYTES: usize = 65;
const P256_P1363_SIGNATURE_BYTES: usize = 64;
const SHA256_BYTES: usize = 32;
const MAX_SEMANTIC_REFERENCE_BYTES: usize = 16 * 1024;

/// Public, authenticated material needed by the direct-sync actor. Private
/// identity and library keys remain behind `MobileSyncCrypto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSyncProfile {
    pub identity_handle: String,
    pub receipt_id: String,
    pub activation_sha256: String,
    pub library_id: String,
    pub device_id: String,
    pub default_scope_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub durable_sync_spki_sha256: [u8; SHA256_BYTES],
    pub device_signing_public_key: Vec<u8>,
    pub authority_signing_public_key: Vec<u8>,
    pub granted_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
    pub revoked: bool,
}

impl ActiveSyncProfile {
    pub fn validate_fixture(&self) -> Result<(), MobileSyncRuntimeError> {
        if !is_canonical_uuid(&self.identity_handle)
            || !is_uuid_v7(&self.receipt_id)
            || !is_sha256_hex(&self.activation_sha256)
            || !is_uuid_v7(&self.library_id)
            || !is_uuid_v7(&self.device_id)
            || !is_uuid_v7(&self.default_scope_id)
            || self.authority_generation == 0
            || self.authority_generation > i64::MAX as u64
            || self.purge_generation > i64::MAX as u64
            || self.key_epoch == 0
            || self.key_epoch > i64::MAX as u64
            || self.environment != Environment::Development
            || self.library_data_class != LibraryDataClass::SanitizedFixture
            || self.durable_sync_spki_sha256.iter().all(|byte| *byte == 0)
            || self.device_signing_public_key.len() != P256_PUBLIC_KEY_BYTES
            || self.device_signing_public_key.first() != Some(&0x04)
            || self.authority_signing_public_key.len() != P256_PUBLIC_KEY_BYTES
            || self.authority_signing_public_key.first() != Some(&0x04)
            || self.granted_scopes != fixture_scopes()
            || self.capabilities != fixture_capabilities()
            || self.revoked
        {
            return Err(MobileSyncRuntimeError::InvalidActiveProfile);
        }
        Ok(())
    }
}

/// Non-secret semantic binding retained beside exact wire bytes. It lets
/// recovery prove that a journal row still means the same operation without
/// persisting decrypted record content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExactRequestPurpose {
    Negotiate {
        capabilities_sha256: String,
    },
    Bootstrap {
        requested_record_kinds: BTreeSet<String>,
        checkpoint_digest: Option<String>,
        after_record_id: Option<String>,
        limit: u32,
    },
    Push {
        transaction_id: String,
        transaction_digest: String,
        device_transaction_counter: u64,
    },
    Pull {
        requested_cursor: u64,
        limit: u32,
        requested_record_kinds: BTreeSet<String>,
    },
    Checkpoint {
        known_cursor: Option<u64>,
    },
    Ack {
        high_water_cursor: u64,
        checkpoint_digest: String,
    },
}

impl ExactRequestPurpose {
    pub fn endpoint(&self) -> DirectEndpoint {
        match self {
            Self::Negotiate { .. } => DirectEndpoint::Negotiate,
            Self::Bootstrap { .. } => DirectEndpoint::Bootstrap,
            Self::Push { .. } => DirectEndpoint::Push,
            Self::Pull { .. } => DirectEndpoint::Pull,
            Self::Checkpoint { .. } => DirectEndpoint::Checkpoint,
            Self::Ack { .. } => DirectEndpoint::Ack,
        }
    }

    fn validate(&self) -> Result<(), MobileSyncRuntimeError> {
        let encoded = serde_json::to_vec(self)
            .map_err(|_| MobileSyncRuntimeError::InvalidSemanticReference)?;
        if encoded.is_empty() || encoded.len() > MAX_SEMANTIC_REFERENCE_BYTES {
            return Err(MobileSyncRuntimeError::InvalidSemanticReference);
        }
        match self {
            Self::Negotiate {
                capabilities_sha256,
            } if !is_sha256_hex(capabilities_sha256) => {
                return Err(MobileSyncRuntimeError::InvalidSemanticReference);
            }
            Self::Negotiate { .. } => {}
            Self::Pull {
                requested_cursor, ..
            } if *requested_cursor > i64::MAX as u64 => {
                return Err(MobileSyncRuntimeError::InvalidSemanticReference);
            }
            Self::Pull {
                limit,
                requested_record_kinds,
                ..
            } if *limit == 0 || requested_record_kinds.is_empty() => {
                return Err(MobileSyncRuntimeError::InvalidSemanticReference);
            }
            Self::Pull { .. } => {}
            Self::Checkpoint {
                known_cursor: Some(cursor),
            } if *cursor > i64::MAX as u64 => {
                return Err(MobileSyncRuntimeError::InvalidSemanticReference);
            }
            Self::Checkpoint { .. } => {}
            Self::Bootstrap {
                requested_record_kinds,
                checkpoint_digest,
                after_record_id,
                limit,
            } => {
                if requested_record_kinds.is_empty()
                    || *limit == 0
                    || checkpoint_digest
                        .as_deref()
                        .is_some_and(|digest| !is_sha256_hex(digest))
                    || after_record_id
                        .as_deref()
                        .is_some_and(|record_id| !is_uuid_v7(record_id))
                    || (after_record_id.is_some() && checkpoint_digest.is_none())
                {
                    return Err(MobileSyncRuntimeError::InvalidSemanticReference);
                }
            }
            Self::Push {
                transaction_id,
                transaction_digest,
                device_transaction_counter,
            } => {
                if !is_uuid_v7(transaction_id)
                    || !is_sha256_hex(transaction_digest)
                    || *device_transaction_counter == 0
                    || *device_transaction_counter > i64::MAX as u64
                {
                    return Err(MobileSyncRuntimeError::InvalidSemanticReference);
                }
            }
            Self::Ack {
                high_water_cursor,
                checkpoint_digest,
            } if *high_water_cursor > i64::MAX as u64 || !is_sha256_hex(checkpoint_digest) => {
                return Err(MobileSyncRuntimeError::InvalidSemanticReference);
            }
            Self::Ack { .. } => {}
        }
        Ok(())
    }

    fn matches_payload(&self, payload: &serde_json::Value) -> bool {
        match self {
            Self::Negotiate {
                capabilities_sha256,
            } => serde_json::from_value::<NegotiateRequest>(payload.clone())
                .ok()
                .is_some_and(|request| {
                    canonical_value_sha256(&request.capabilities) == *capabilities_sha256
                }),
            Self::Bootstrap {
                requested_record_kinds,
                checkpoint_digest,
                after_record_id,
                limit,
            } => serde_json::from_value::<BootstrapRequest>(payload.clone())
                .ok()
                .is_some_and(|request| {
                    request.requested_record_kinds == *requested_record_kinds
                        && request.checkpoint_digest == *checkpoint_digest
                        && request.after_record_id == *after_record_id
                        && request.limit == *limit
                }),
            Self::Push {
                transaction_id,
                transaction_digest,
                device_transaction_counter,
            } => serde_json::from_value::<PushRequest>(payload.clone())
                .ok()
                .is_some_and(|request| {
                    request.transaction.manifest.transaction_id == *transaction_id
                        && request.transaction.signed_digest() == *transaction_digest
                        && request.transaction.manifest.device_transaction_counter
                            == *device_transaction_counter
                }),
            Self::Pull {
                requested_cursor,
                limit,
                requested_record_kinds,
            } => serde_json::from_value::<PullRequest>(payload.clone())
                .ok()
                .is_some_and(|request| {
                    request.cursor == *requested_cursor
                        && request.limit == *limit
                        && request.requested_record_kinds == *requested_record_kinds
                }),
            Self::Checkpoint { known_cursor } => {
                serde_json::from_value::<CheckpointRequest>(payload.clone())
                    .ok()
                    .is_some_and(|request| request.known_cursor == *known_cursor)
            }
            Self::Ack {
                high_water_cursor,
                checkpoint_digest,
            } => serde_json::from_value::<AckRequest>(payload.clone())
                .ok()
                .is_some_and(|request| {
                    request.high_water_cursor == *high_water_cursor
                        && request.checkpoint_digest == *checkpoint_digest
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactRequestState {
    AwaitingResponse,
    ResponseStored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedResponseWire {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub body_sha256: [u8; SHA256_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournaledExactRequest {
    pub endpoint: DirectEndpoint,
    pub request_id: String,
    pub purpose: ExactRequestPurpose,
    pub request_body: Vec<u8>,
    pub request_body_sha256: [u8; SHA256_BYTES],
    pub state: ExactRequestState,
    pub attempt_count: u32,
    pub response: Option<AuthenticatedResponseWire>,
}

impl JournaledExactRequest {
    fn validate_shape(&self) -> Result<(), MobileSyncRuntimeError> {
        self.purpose.validate()?;
        if self.endpoint != self.purpose.endpoint()
            || !is_uuid_v7(&self.request_id)
            || self.request_body.is_empty()
            || sha256(&self.request_body) != self.request_body_sha256
            || (self.state == ExactRequestState::AwaitingResponse && self.response.is_some())
            || (self.state == ExactRequestState::ResponseStored && self.response.is_none())
        {
            return Err(MobileSyncRuntimeError::JournalCorrupt);
        }
        if let Some(response) = &self.response {
            if !(100..=599).contains(&response.status)
                || response.content_type != DIRECT_SYNC_CONTENT_TYPE
                || response.body.is_empty()
                || sha256(&response.body) != response.body_sha256
            {
                return Err(MobileSyncRuntimeError::JournalCorrupt);
            }
        }
        Ok(())
    }
}

/// The SQL implementation must reject a second in-flight row and exact-ID
/// reuse with different bytes. `store_authenticated_response` commits the response
/// before any semantic apply; `complete_exact_request` is called only after
/// that apply is durable.
pub trait ExactRequestJournal {
    fn active_sync_profile(&self) -> Result<ActiveSyncProfile, MobileSyncRuntimeError>;

    fn unresolved_exact_request(
        &self,
    ) -> Result<Option<JournaledExactRequest>, MobileSyncRuntimeError>;

    fn prepare_exact_request(
        &mut self,
        request: JournaledExactRequest,
    ) -> Result<(), MobileSyncRuntimeError>;

    fn record_transport_attempt(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
    ) -> Result<(), MobileSyncRuntimeError>;

    fn store_authenticated_response(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
        response: AuthenticatedResponseWire,
    ) -> Result<(), MobileSyncRuntimeError>;

    fn complete_exact_request(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
    ) -> Result<(), MobileSyncRuntimeError>;
}

/// Native-only cryptography. Implementations bind the identity handle to
/// Keychain/Secure Enclave custody and never return private material.
pub trait MobileSyncCrypto {
    fn fresh_uuid_v7(&self) -> Result<String, MobileSyncRuntimeError>;

    fn sign(
        &self,
        identity_handle: &str,
        message: &[u8],
    ) -> Result<Vec<u8>, MobileSyncRuntimeError>;

    fn verify_p256_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<bool, MobileSyncRuntimeError>;
}

impl<T: MobileSyncCrypto + ?Sized> MobileSyncCrypto for &T {
    fn fresh_uuid_v7(&self) -> Result<String, MobileSyncRuntimeError> {
        (**self).fresh_uuid_v7()
    }

    fn sign(
        &self,
        identity_handle: &str,
        message: &[u8],
    ) -> Result<Vec<u8>, MobileSyncRuntimeError> {
        (**self).sign(identity_handle, message)
    }

    fn verify_p256_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<bool, MobileSyncRuntimeError> {
        (**self).verify_p256_signature(public_key, message, signature)
    }
}

pub type DirectSyncPostFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DirectResponse, MobileSyncRuntimeError>> + Send + 'a>>;

/// Opaque verified session: construction must already have enforced numeric
/// private-LAN addressing, the authenticated SPKI pin, TLS 1.3, no 0-RTT or
/// resumption, and the fixed endpoint set. The actor supplies no URL.
pub trait VerifiedDirectSyncSession {
    fn post<'a>(
        &'a self,
        endpoint: DirectEndpoint,
        exact_body: Vec<u8>,
    ) -> DirectSyncPostFuture<'a>;
}

impl<T: VerifiedDirectSyncSession + ?Sized> VerifiedDirectSyncSession for &T {
    fn post<'a>(
        &'a self,
        endpoint: DirectEndpoint,
        exact_body: Vec<u8>,
    ) -> DirectSyncPostFuture<'a> {
        (**self).post(endpoint, exact_body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedExactRequest {
    pub endpoint: DirectEndpoint,
    pub request_id: String,
    pub exact_body: Vec<u8>,
    pub exact_body_sha256: [u8; SHA256_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSyncResponse<T> {
    pub completion: ExactRequestCompletion,
    pub exact_body: Vec<u8>,
    pub exact_body_sha256: [u8; SHA256_BYTES],
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRequestCompletion {
    pub endpoint: DirectEndpoint,
    pub request_id: String,
    pub request_body_sha256: [u8; SHA256_BYTES],
}

/// Build and sign a request without sending it. The returned body is the only
/// body the journal and transport may use for this request ID.
pub fn prepare_signed_request<T: Serialize>(
    profile: &ActiveSyncProfile,
    crypto: &impl MobileSyncCrypto,
    limits: &DirectSyncLimits,
    purpose: &ExactRequestPurpose,
    payload: T,
) -> Result<PreparedExactRequest, MobileSyncRuntimeError> {
    profile.validate_fixture()?;
    purpose.validate()?;
    let endpoint = purpose.endpoint();
    let payload_value =
        serde_json::to_value(&payload).map_err(|_| MobileSyncRuntimeError::InvalidRequest)?;
    if !purpose.matches_payload(&payload_value) {
        return Err(MobileSyncRuntimeError::PurposePayloadMismatch);
    }
    let request_id = crypto.fresh_uuid_v7()?;
    if !is_uuid_v7(&request_id) {
        return Err(MobileSyncRuntimeError::NativeCryptoRejected);
    }
    let mut request = SignedSyncRequest {
        protocol_version: SYNC_PROTOCOL_VERSION,
        request_id: request_id.clone(),
        library_id: profile.library_id.clone(),
        device_id: profile.device_id.clone(),
        authority_generation: profile.authority_generation,
        environment: profile.environment,
        library_data_class: profile.library_data_class,
        payload,
        signature: Vec::new(),
    };
    let signing_bytes = request_signing_bytes(endpoint, &request)
        .map_err(|_| MobileSyncRuntimeError::InvalidRequest)?;
    request.signature = crypto.sign(&profile.identity_handle, &signing_bytes)?;
    if request.signature.len() != P256_P1363_SIGNATURE_BYTES
        || request.signature.len() > MAX_DIRECT_SIGNATURE_BYTES
    {
        return Err(MobileSyncRuntimeError::NativeCryptoRejected);
    }
    let exact_body =
        serde_json::to_vec(&request).map_err(|_| MobileSyncRuntimeError::InvalidRequest)?;
    if exact_body.is_empty() || exact_body.len() > limits.for_endpoint(endpoint).request_bytes {
        return Err(MobileSyncRuntimeError::RequestTooLarge);
    }
    Ok(PreparedExactRequest {
        endpoint,
        request_id,
        exact_body_sha256: sha256(&exact_body),
        exact_body,
    })
}

/// Revalidate a journaled request before every transmission. This detects a
/// corrupted or byte-rebound row without asking native custody to sign again.
pub fn validate_journaled_request(
    profile: &ActiveSyncProfile,
    crypto: &impl MobileSyncCrypto,
    limits: &DirectSyncLimits,
    journaled: &JournaledExactRequest,
) -> Result<(), MobileSyncRuntimeError> {
    profile.validate_fixture()?;
    journaled.validate_shape()?;
    let endpoint_limit = limits.for_endpoint(journaled.endpoint).request_bytes;
    let request: SignedSyncRequest<serde_json::Value> =
        parse_bounded_direct_json(&journaled.request_body, endpoint_limit)
            .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
    if request.protocol_version != SYNC_PROTOCOL_VERSION
        || request.request_id != journaled.request_id
        || request.library_id != profile.library_id
        || request.device_id != profile.device_id
        || request.authority_generation != profile.authority_generation
        || request.environment != profile.environment
        || request.library_data_class != profile.library_data_class
        || request.signature.len() != P256_P1363_SIGNATURE_BYTES
        || !journaled.purpose.matches_payload(&request.payload)
    {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    let signing_bytes = request_signing_bytes(journaled.endpoint, &request)
        .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
    if !crypto.verify_p256_signature(
        &profile.device_signing_public_key,
        &signing_bytes,
        &request.signature,
    )? {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    Ok(())
}

/// Verify the complete signed response and its exact request binding before it
/// can enter the durable response journal.
pub fn verify_signed_response<T: DeserializeOwned + Serialize>(
    profile: &ActiveSyncProfile,
    crypto: &impl MobileSyncCrypto,
    limits: &DirectSyncLimits,
    expected_endpoint: DirectEndpoint,
    expected_request_id: &str,
    expected_request_body_sha256: [u8; SHA256_BYTES],
    response: &DirectResponse,
) -> Result<VerifiedSyncResponse<T>, MobileSyncRuntimeError> {
    profile.validate_fixture()?;
    if response.status != 200 {
        return Err(classify_server_error(response.status, &response.body));
    }
    if response.content_type != DIRECT_SYNC_CONTENT_TYPE {
        return Err(MobileSyncRuntimeError::InvalidContentType);
    }
    let signed: SignedSyncResponse<T> = parse_bounded_direct_json(
        &response.body,
        limits.for_endpoint(expected_endpoint).response_bytes,
    )
    .map_err(|_| MobileSyncRuntimeError::InvalidResponse)?;
    if signed.protocol_version != SYNC_PROTOCOL_VERSION
        || signed.request_id != expected_request_id
        || signed.library_id != profile.library_id
        || signed.device_id != profile.device_id
        || signed.authority_generation != profile.authority_generation
        || signed.signature.len() != P256_P1363_SIGNATURE_BYTES
    {
        return Err(MobileSyncRuntimeError::ResponseBindingMismatch);
    }
    let signing_bytes = response_signing_bytes(expected_endpoint, &signed)
        .map_err(|_| MobileSyncRuntimeError::InvalidResponse)?;
    if !crypto.verify_p256_signature(
        &profile.authority_signing_public_key,
        &signing_bytes,
        &signed.signature,
    )? {
        return Err(MobileSyncRuntimeError::AuthoritySignatureRejected);
    }
    Ok(VerifiedSyncResponse {
        completion: ExactRequestCompletion {
            endpoint: expected_endpoint,
            request_id: signed.request_id,
            request_body_sha256: expected_request_body_sha256,
        },
        exact_body_sha256: sha256(&response.body),
        exact_body: response.body.clone(),
        payload: signed.payload,
    })
}

/// Validate the authority-authenticated historical writer directory and every
/// outer mutation signature before native record decryption is attempted.
pub fn validate_bootstrap_writer_signatures(
    crypto: &(impl MobileSyncCrypto + ?Sized),
    response: &BootstrapResponse,
) -> Result<(), MobileSyncRuntimeError> {
    validate_writer_signatures(
        crypto,
        &response.writer_signing_keys,
        response.page.records.iter().map(|record| &record.mutation),
    )
}

/// Pull pages use the same exact directory contract as bootstrap pages.
pub fn validate_pull_writer_signatures(
    crypto: &(impl MobileSyncCrypto + ?Sized),
    response: &PullResponse,
) -> Result<(), MobileSyncRuntimeError> {
    validate_writer_signatures(
        crypto,
        &response.writer_signing_keys,
        response
            .page
            .changes
            .iter()
            .flat_map(|change| change.transaction.members.iter()),
    )
}

fn validate_writer_signatures<'a>(
    crypto: &(impl MobileSyncCrypto + ?Sized),
    keys: &BTreeMap<String, Vec<u8>>,
    mutations: impl IntoIterator<Item = &'a crate::sync_protocol::MutationEnvelope>,
) -> Result<(), MobileSyncRuntimeError> {
    let mutations = mutations.into_iter().collect::<Vec<_>>();
    let required = mutations
        .iter()
        .map(|mutation| mutation.device_id.as_str())
        .collect::<BTreeSet<_>>();
    if required.len() != keys.len()
        || !required
            .iter()
            .all(|writer_id| keys.contains_key(*writer_id))
        || keys.iter().any(|(writer_id, key)| {
            !is_uuid_v7(writer_id)
                || key.len() != P256_PUBLIC_KEY_BYTES
                || key.first() != Some(&0x04)
        })
    {
        return Err(MobileSyncRuntimeError::WriterDirectoryRejected);
    }
    for mutation in mutations {
        let key = keys
            .get(&mutation.device_id)
            .ok_or(MobileSyncRuntimeError::WriterDirectoryRejected)?;
        if mutation.signature.len() != P256_P1363_SIGNATURE_BYTES
            || !crypto.verify_p256_signature(key, &mutation.signing_bytes(), &mutation.signature)?
        {
            return Err(MobileSyncRuntimeError::WriterSignatureRejected);
        }
    }
    Ok(())
}

/// Serial, one-in-flight request actor. Higher-level semantic code owns the
/// bootstrap/pull/push state machine and calls `complete_verified` only after
/// the decoded application transaction has committed.
pub struct MobileSyncRequestActor<S, C, N> {
    journal: S,
    crypto: C,
    session: N,
    limits: DirectSyncLimits,
}

impl<S, C, N> MobileSyncRequestActor<S, C, N>
where
    S: ExactRequestJournal,
    C: MobileSyncCrypto,
    N: VerifiedDirectSyncSession,
{
    pub fn new(
        journal: S,
        crypto: C,
        session: N,
        limits: DirectSyncLimits,
    ) -> Result<Self, MobileSyncRuntimeError> {
        journal.active_sync_profile()?.validate_fixture()?;
        Ok(Self {
            journal,
            crypto,
            session,
            limits,
        })
    }

    pub fn journal(&self) -> &S {
        &self.journal
    }

    pub fn journal_mut(&mut self) -> &mut S {
        &mut self.journal
    }

    pub async fn begin<TRequest, TResponse>(
        &mut self,
        purpose: ExactRequestPurpose,
        payload: TRequest,
    ) -> Result<VerifiedSyncResponse<TResponse>, MobileSyncRuntimeError>
    where
        TRequest: Serialize,
        TResponse: DeserializeOwned + Serialize,
    {
        if self.journal.unresolved_exact_request()?.is_some() {
            return Err(MobileSyncRuntimeError::RequestAlreadyInFlight);
        }
        let profile = self.journal.active_sync_profile()?;
        let prepared =
            prepare_signed_request(&profile, &self.crypto, &self.limits, &purpose, payload)?;
        let journaled = JournaledExactRequest {
            endpoint: prepared.endpoint,
            request_id: prepared.request_id.clone(),
            purpose,
            request_body: prepared.exact_body.clone(),
            request_body_sha256: prepared.exact_body_sha256,
            state: ExactRequestState::AwaitingResponse,
            attempt_count: 0,
            response: None,
        };
        self.journal.prepare_exact_request(journaled.clone())?;
        self.send_and_store(&profile, &journaled).await
    }

    pub async fn recover<TResponse>(
        &mut self,
        expected_endpoint: DirectEndpoint,
    ) -> Result<Option<VerifiedSyncResponse<TResponse>>, MobileSyncRuntimeError>
    where
        TResponse: DeserializeOwned + Serialize,
    {
        let Some(journaled) = self.journal.unresolved_exact_request()? else {
            return Ok(None);
        };
        if journaled.endpoint != expected_endpoint {
            return Err(MobileSyncRuntimeError::RecoveryEndpointMismatch);
        }
        let profile = self.journal.active_sync_profile()?;
        validate_journaled_request(&profile, &self.crypto, &self.limits, &journaled)?;
        if let Some(stored) = &journaled.response {
            let direct = DirectResponse {
                status: stored.status,
                content_type: DIRECT_SYNC_CONTENT_TYPE,
                body: stored.body.clone(),
            };
            return verify_signed_response(
                &profile,
                &self.crypto,
                &self.limits,
                journaled.endpoint,
                &journaled.request_id,
                journaled.request_body_sha256,
                &direct,
            )
            .map(Some)
            .or_else(|error| {
                if stored.status == 200 {
                    Err(error)
                } else {
                    Err(classify_server_error(stored.status, &stored.body))
                }
            });
        }
        self.send_and_store(&profile, &journaled).await.map(Some)
    }

    pub fn complete_verified(
        &mut self,
        completion: &ExactRequestCompletion,
    ) -> Result<(), MobileSyncRuntimeError> {
        self.journal
            .complete_exact_request(&completion.request_id, completion.request_body_sha256)
    }

    async fn send_and_store<TResponse>(
        &mut self,
        profile: &ActiveSyncProfile,
        journaled: &JournaledExactRequest,
    ) -> Result<VerifiedSyncResponse<TResponse>, MobileSyncRuntimeError>
    where
        TResponse: DeserializeOwned + Serialize,
    {
        validate_journaled_request(profile, &self.crypto, &self.limits, journaled)?;
        self.journal
            .record_transport_attempt(&journaled.request_id, journaled.request_body_sha256)?;
        let response = self
            .session
            .post(journaled.endpoint, journaled.request_body.clone())
            .await?;
        validate_authenticated_response_shape(&self.limits, journaled.endpoint, &response)?;
        let verified = if response.status == 200 {
            Some(verify_signed_response(
                profile,
                &self.crypto,
                &self.limits,
                journaled.endpoint,
                &journaled.request_id,
                journaled.request_body_sha256,
                &response,
            )?)
        } else {
            None
        };
        self.journal.store_authenticated_response(
            &journaled.request_id,
            journaled.request_body_sha256,
            AuthenticatedResponseWire {
                status: response.status,
                content_type: response.content_type.to_owned(),
                body: response.body.clone(),
                body_sha256: sha256(&response.body),
            },
        )?;
        match verified {
            Some(verified) => Ok(verified),
            None => Err(classify_server_error(response.status, &response.body)),
        }
    }
}

fn validate_authenticated_response_shape(
    limits: &DirectSyncLimits,
    endpoint: DirectEndpoint,
    response: &DirectResponse,
) -> Result<(), MobileSyncRuntimeError> {
    if !(100..=599).contains(&response.status)
        || response.content_type != DIRECT_SYNC_CONTENT_TYPE
        || response.body.is_empty()
        || response.body.len() > limits.for_endpoint(endpoint).response_bytes
    {
        return Err(MobileSyncRuntimeError::InvalidResponse);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectSyncErrorBody {
    error: DirectSyncErrorCode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectSyncErrorCode {
    code: String,
}

fn classify_server_error(status: u16, body: &[u8]) -> MobileSyncRuntimeError {
    let parsed = parse_bounded_direct_json::<DirectSyncErrorBody>(body, 8 * 1024).ok();
    let code = parsed
        .as_ref()
        .map(|response| response.error.code.as_str())
        .unwrap_or("malformed_error");
    match code {
        "device_revoked" => MobileSyncRuntimeError::DeviceRevoked,
        "bootstrap_changed_restart_required" => MobileSyncRuntimeError::BootstrapChanged,
        _ => MobileSyncRuntimeError::ServerRejected {
            status,
            code: code.to_owned(),
        },
    }
}

fn fixture_scopes() -> BTreeSet<RecordKind> {
    [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
        .into_iter()
        .collect()
}

fn fixture_capabilities() -> BTreeMap<RecordKind, KindCapability> {
    fixture_scopes()
        .into_iter()
        .map(|kind| {
            (
                kind,
                KindCapability {
                    reader_version: 1,
                    writer_version: Some(1),
                },
            )
        })
        .collect()
}

fn canonical_value_sha256(value: &impl Serialize) -> String {
    let value = serde_json::to_value(value).expect("serializable protocol value");
    let digest = Sha256::digest(canonical_json(&value).as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_canonical_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
        })
}

fn sha256(bytes: &[u8]) -> [u8; SHA256_BYTES] {
    Sha256::digest(bytes).into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileSyncRuntimeError {
    InvalidActiveProfile,
    InvalidSemanticReference,
    InvalidRequest,
    RequestTooLarge,
    PurposePayloadMismatch,
    RequestAlreadyInFlight,
    JournalCorrupt,
    RecoveryEndpointMismatch,
    NativeCryptoRejected,
    TransportUnavailable,
    DeviceRevoked,
    BootstrapChanged,
    ServerRejected { status: u16, code: String },
    InvalidContentType,
    InvalidResponse,
    ResponseBindingMismatch,
    AuthoritySignatureRejected,
    WriterDirectoryRejected,
    WriterSignatureRejected,
    StateUnavailable,
}

impl fmt::Display for MobileSyncRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerRejected { status, code } => {
                write!(formatter, "direct sync returned HTTP {status} ({code})")
            }
            other => write!(
                formatter,
                "{}",
                match other {
                    Self::InvalidActiveProfile => "active sync profile is invalid",
                    Self::InvalidSemanticReference => "exact request semantic reference is invalid",
                    Self::InvalidRequest => "direct-sync request is invalid",
                    Self::RequestTooLarge => "direct-sync request exceeds its endpoint limit",
                    Self::PurposePayloadMismatch =>
                        "direct-sync request purpose does not match its payload",
                    Self::RequestAlreadyInFlight =>
                        "an exact direct-sync request is already in flight",
                    Self::JournalCorrupt => "the exact direct-sync request journal is corrupt",
                    Self::RecoveryEndpointMismatch =>
                        "the recovered direct-sync request has a different endpoint",
                    Self::NativeCryptoRejected => "native sync cryptography rejected the operation",
                    Self::TransportUnavailable =>
                        "the authenticated direct-sync session is unavailable",
                    Self::DeviceRevoked => "the direct-sync device has been revoked",
                    Self::BootstrapChanged => "the direct-sync bootstrap changed and must restart",
                    Self::InvalidContentType => "direct-sync response has an invalid content type",
                    Self::InvalidResponse =>
                        "direct-sync response is malformed or exceeds its limit",
                    Self::ResponseBindingMismatch =>
                        "direct-sync response does not match its exact request",
                    Self::AuthoritySignatureRejected =>
                        "direct-sync authority signature is invalid",
                    Self::WriterDirectoryRejected => "direct-sync writer key directory is invalid",
                    Self::WriterSignatureRejected =>
                        "direct-sync mutation writer signature is invalid",
                    Self::StateUnavailable => "direct-sync state is unavailable",
                    Self::ServerRejected { .. } => unreachable!(),
                }
            ),
        }
    }
}

impl std::error::Error for MobileSyncRuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_sync::{CheckpointRequest, CheckpointResponse, SyncCheckpoint};
    use crate::sync_protocol::{
        MutationDraft, MutationOperation, SignedTransaction, TransactionHeader,
    };
    use p256::ecdsa::signature::{Signer, Verifier};
    use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    const IDENTITY_HANDLE: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e070";
    const LIBRARY_ID: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e071";
    const DEVICE_ID: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e072";
    const REQUEST_ID: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e073";

    struct TestCrypto {
        device_key: SigningKey,
        request_ids: Mutex<VecDeque<String>>,
    }

    impl TestCrypto {
        fn new(
            device_key: SigningKey,
            request_ids: impl IntoIterator<Item = &'static str>,
        ) -> Self {
            Self {
                device_key,
                request_ids: Mutex::new(request_ids.into_iter().map(str::to_owned).collect()),
            }
        }
    }

    impl MobileSyncCrypto for TestCrypto {
        fn fresh_uuid_v7(&self) -> Result<String, MobileSyncRuntimeError> {
            self.request_ids
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?
                .pop_front()
                .ok_or(MobileSyncRuntimeError::StateUnavailable)
        }

        fn sign(
            &self,
            identity_handle: &str,
            message: &[u8],
        ) -> Result<Vec<u8>, MobileSyncRuntimeError> {
            if identity_handle != IDENTITY_HANDLE {
                return Err(MobileSyncRuntimeError::NativeCryptoRejected);
            }
            let signature: Signature = self.device_key.sign(message);
            Ok(signature.to_bytes().to_vec())
        }

        fn verify_p256_signature(
            &self,
            public_key: &[u8],
            message: &[u8],
            signature: &[u8],
        ) -> Result<bool, MobileSyncRuntimeError> {
            let key = VerifyingKey::from_sec1_bytes(public_key)
                .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)?;
            let signature = Signature::from_slice(signature)
                .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)?;
            Ok(key.verify(message, &signature).is_ok())
        }
    }

    fn signing_key(byte: u8) -> SigningKey {
        SigningKey::from_bytes((&[byte; 32]).into()).expect("valid fixture signing key")
    }

    fn public_key(key: &SigningKey) -> Vec<u8> {
        key.verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec()
    }

    fn profile(device_key: &SigningKey, authority_key: &SigningKey) -> ActiveSyncProfile {
        ActiveSyncProfile {
            identity_handle: IDENTITY_HANDLE.to_owned(),
            receipt_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e075".to_owned(),
            activation_sha256: "b".repeat(64),
            library_id: LIBRARY_ID.to_owned(),
            device_id: DEVICE_ID.to_owned(),
            default_scope_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e076".to_owned(),
            authority_generation: 3,
            purge_generation: 2,
            key_epoch: 4,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            durable_sync_spki_sha256: [9; SHA256_BYTES],
            device_signing_public_key: public_key(device_key),
            authority_signing_public_key: public_key(authority_key),
            granted_scopes: fixture_scopes(),
            capabilities: fixture_capabilities(),
            revoked: false,
        }
    }

    fn checkpoint_payload() -> CheckpointResponse {
        CheckpointResponse {
            checkpoint: SyncCheckpoint {
                contract_version: "noted.sync-bootstrap.v1".to_owned(),
                library_id: LIBRARY_ID.to_owned(),
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 4,
                high_water_cursor: 7,
                checkpoint_digest: "a".repeat(64),
            },
            changed_since_known_cursor: true,
        }
    }

    fn authority_response<T: Serialize>(
        key: &SigningKey,
        endpoint: DirectEndpoint,
        request_id: &str,
        payload: T,
    ) -> DirectResponse {
        let mut signed = SignedSyncResponse {
            protocol_version: SYNC_PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            library_id: LIBRARY_ID.to_owned(),
            device_id: DEVICE_ID.to_owned(),
            authority_generation: 3,
            payload,
            signature: Vec::new(),
        };
        let message = response_signing_bytes(endpoint, &signed).expect("response signing bytes");
        let signature: Signature = key.sign(&message);
        signed.signature = signature.to_bytes().to_vec();
        DirectResponse {
            status: 200,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body: serde_json::to_vec(&signed).expect("serialize signed response"),
        }
    }

    fn signed_fixture_mutation(writer_key: &SigningKey) -> crate::sync_protocol::MutationEnvelope {
        let prepared = SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: SYNC_PROTOCOL_VERSION,
                library_id: LIBRARY_ID.to_owned(),
                transaction_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e077".to_owned(),
                device_id: DEVICE_ID.to_owned(),
                device_transaction_counter: 1,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 4,
            },
            vec![MutationDraft {
                mutation_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e078".to_owned(),
                operation: MutationOperation::Create,
                record_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e079".to_owned(),
                record_kind: "note".to_owned(),
                record_schema_version: 1,
                base_head_revision: 0,
                base_head_version_id: None,
                proposed_revision: 1,
                version_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e07a".to_owned(),
                ciphertext: b"sealed-record".to_vec(),
            }],
            100,
        )
        .expect("prepare fixture transaction");
        let signature: Signature = writer_key.sign(&prepared.signing_inputs()[0].canonical_bytes);
        let mut signed = prepared
            .attach_signatures(vec![signature.to_bytes().to_vec()])
            .expect("attach fixture signature");
        signed.members.remove(0)
    }

    #[test]
    fn request_is_signed_for_its_exact_endpoint_and_revalidates() {
        let device_key = signing_key(7);
        let authority_key = signing_key(11);
        let profile = profile(&device_key, &authority_key);
        let crypto = TestCrypto::new(device_key, [REQUEST_ID]);
        let purpose = ExactRequestPurpose::Checkpoint {
            known_cursor: Some(6),
        };
        let prepared = prepare_signed_request(
            &profile,
            &crypto,
            &DirectSyncLimits::default(),
            &purpose,
            CheckpointRequest {
                known_cursor: Some(6),
            },
        )
        .expect("prepare exact request");
        let journaled = JournaledExactRequest {
            endpoint: prepared.endpoint,
            request_id: prepared.request_id,
            purpose,
            request_body: prepared.exact_body,
            request_body_sha256: prepared.exact_body_sha256,
            state: ExactRequestState::AwaitingResponse,
            attempt_count: 1,
            response: None,
        };

        validate_journaled_request(&profile, &crypto, &DirectSyncLimits::default(), &journaled)
            .expect("validate exact journal row");

        let mut rebound = journaled;
        rebound.endpoint = DirectEndpoint::Ack;
        assert_eq!(
            validate_journaled_request(&profile, &crypto, &DirectSyncLimits::default(), &rebound,),
            Err(MobileSyncRuntimeError::JournalCorrupt)
        );
    }

    #[test]
    fn authority_response_binds_endpoint_request_and_profile() {
        let device_key = signing_key(7);
        let authority_key = signing_key(11);
        let profile = profile(&device_key, &authority_key);
        let crypto = TestCrypto::new(device_key, []);
        let response = authority_response(
            &authority_key,
            DirectEndpoint::Checkpoint,
            REQUEST_ID,
            checkpoint_payload(),
        );

        let verified: VerifiedSyncResponse<CheckpointResponse> = verify_signed_response(
            &profile,
            &crypto,
            &DirectSyncLimits::default(),
            DirectEndpoint::Checkpoint,
            REQUEST_ID,
            [8; SHA256_BYTES],
            &response,
        )
        .expect("verify authority response");
        assert_eq!(verified.payload.checkpoint.high_water_cursor, 7);

        assert_eq!(
            verify_signed_response::<CheckpointResponse>(
                &profile,
                &crypto,
                &DirectSyncLimits::default(),
                DirectEndpoint::Ack,
                REQUEST_ID,
                [8; SHA256_BYTES],
                &response,
            ),
            Err(MobileSyncRuntimeError::AuthoritySignatureRejected)
        );
        assert_eq!(
            verify_signed_response::<CheckpointResponse>(
                &profile,
                &crypto,
                &DirectSyncLimits::default(),
                DirectEndpoint::Checkpoint,
                "018f47f2-8ee8-7a28-91eb-9b3f2619e074",
                [8; SHA256_BYTES],
                &response,
            ),
            Err(MobileSyncRuntimeError::ResponseBindingMismatch)
        );
    }

    #[test]
    fn response_parser_rejects_duplicate_keys_and_non_success_status() {
        let device_key = signing_key(7);
        let authority_key = signing_key(11);
        let profile = profile(&device_key, &authority_key);
        let crypto = TestCrypto::new(device_key, []);
        let duplicate = DirectResponse {
            status: 200,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body: format!(
                "{{\"protocol_version\":1,\"protocol_version\":1,\"request_id\":\"{REQUEST_ID}\"}}"
            )
            .into_bytes(),
        };
        assert_eq!(
            verify_signed_response::<CheckpointResponse>(
                &profile,
                &crypto,
                &DirectSyncLimits::default(),
                DirectEndpoint::Checkpoint,
                REQUEST_ID,
                [8; SHA256_BYTES],
                &duplicate,
            ),
            Err(MobileSyncRuntimeError::InvalidResponse)
        );

        let rejected = DirectResponse {
            status: 403,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body: br#"{"error":{"code":"device_revoked"}}"#.to_vec(),
        };
        assert_eq!(
            verify_signed_response::<CheckpointResponse>(
                &profile,
                &crypto,
                &DirectSyncLimits::default(),
                DirectEndpoint::Checkpoint,
                REQUEST_ID,
                [8; SHA256_BYTES],
                &rejected,
            ),
            Err(MobileSyncRuntimeError::DeviceRevoked)
        );
    }

    #[test]
    fn writer_directory_is_exact_and_mutation_signatures_are_verified() {
        let writer_key = signing_key(17);
        let crypto = TestCrypto::new(signing_key(7), []);
        let mutation = signed_fixture_mutation(&writer_key);
        let keys = BTreeMap::from([(DEVICE_ID.to_owned(), public_key(&writer_key))]);

        validate_writer_signatures(&crypto, &keys, [&mutation])
            .expect("verify exact historical writer directory");

        let mut unused = keys.clone();
        unused.insert(
            "018f47f2-8ee8-7a28-91eb-9b3f2619e07b".to_owned(),
            public_key(&signing_key(19)),
        );
        assert_eq!(
            validate_writer_signatures(&crypto, &unused, [&mutation]),
            Err(MobileSyncRuntimeError::WriterDirectoryRejected)
        );
        assert_eq!(
            validate_writer_signatures(&crypto, &BTreeMap::new(), [&mutation]),
            Err(MobileSyncRuntimeError::WriterDirectoryRejected)
        );

        let mut tampered = mutation;
        tampered.signature[0] ^= 1;
        assert_eq!(
            validate_writer_signatures(&crypto, &keys, [&tampered]),
            Err(MobileSyncRuntimeError::WriterSignatureRejected)
        );
    }

    #[derive(Default)]
    struct JournalState {
        request: Option<JournaledExactRequest>,
        completed: Vec<String>,
    }

    #[derive(Clone)]
    struct TestJournal {
        profile: ActiveSyncProfile,
        state: Arc<Mutex<JournalState>>,
    }

    impl ExactRequestJournal for TestJournal {
        fn active_sync_profile(&self) -> Result<ActiveSyncProfile, MobileSyncRuntimeError> {
            Ok(self.profile.clone())
        }

        fn unresolved_exact_request(
            &self,
        ) -> Result<Option<JournaledExactRequest>, MobileSyncRuntimeError> {
            Ok(self
                .state
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?
                .request
                .clone())
        }

        fn prepare_exact_request(
            &mut self,
            request: JournaledExactRequest,
        ) -> Result<(), MobileSyncRuntimeError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?;
            if state.request.is_some() {
                return Err(MobileSyncRuntimeError::RequestAlreadyInFlight);
            }
            state.request = Some(request);
            Ok(())
        }

        fn record_transport_attempt(
            &mut self,
            request_id: &str,
            exact_request_sha256: [u8; SHA256_BYTES],
        ) -> Result<(), MobileSyncRuntimeError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?;
            let request = state
                .request
                .as_mut()
                .ok_or(MobileSyncRuntimeError::JournalCorrupt)?;
            if request.request_id != request_id
                || request.request_body_sha256 != exact_request_sha256
                || request.response.is_some()
            {
                return Err(MobileSyncRuntimeError::JournalCorrupt);
            }
            request.attempt_count = request
                .attempt_count
                .checked_add(1)
                .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
            Ok(())
        }

        fn store_authenticated_response(
            &mut self,
            request_id: &str,
            exact_request_sha256: [u8; SHA256_BYTES],
            response: AuthenticatedResponseWire,
        ) -> Result<(), MobileSyncRuntimeError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?;
            let request = state
                .request
                .as_mut()
                .ok_or(MobileSyncRuntimeError::JournalCorrupt)?;
            if request.request_id != request_id
                || request.request_body_sha256 != exact_request_sha256
            {
                return Err(MobileSyncRuntimeError::JournalCorrupt);
            }
            match &request.response {
                Some(existing) if existing == &response => return Ok(()),
                Some(_) => return Err(MobileSyncRuntimeError::JournalCorrupt),
                None => {}
            }
            request.response = Some(response);
            request.state = ExactRequestState::ResponseStored;
            Ok(())
        }

        fn complete_exact_request(
            &mut self,
            request_id: &str,
            exact_request_sha256: [u8; SHA256_BYTES],
        ) -> Result<(), MobileSyncRuntimeError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?;
            let request = state
                .request
                .as_ref()
                .ok_or(MobileSyncRuntimeError::JournalCorrupt)?;
            if request.request_id != request_id
                || request.request_body_sha256 != exact_request_sha256
                || request.state != ExactRequestState::ResponseStored
            {
                return Err(MobileSyncRuntimeError::JournalCorrupt);
            }
            state.completed.push(request_id.to_owned());
            state.request = None;
            Ok(())
        }
    }

    #[derive(Default)]
    struct SessionState {
        requests: Vec<(DirectEndpoint, Vec<u8>)>,
        responses: VecDeque<Result<DirectResponse, MobileSyncRuntimeError>>,
        require_prepared_journal: Option<Arc<Mutex<JournalState>>>,
    }

    #[derive(Clone)]
    struct TestSession(Arc<Mutex<SessionState>>);

    impl VerifiedDirectSyncSession for TestSession {
        fn post<'a>(
            &'a self,
            endpoint: DirectEndpoint,
            exact_body: Vec<u8>,
        ) -> DirectSyncPostFuture<'a> {
            let result = self
                .0
                .lock()
                .map_err(|_| MobileSyncRuntimeError::StateUnavailable)
                .and_then(|mut state| {
                    if let Some(journal) = &state.require_prepared_journal {
                        let prepared = journal
                            .lock()
                            .map_err(|_| MobileSyncRuntimeError::StateUnavailable)?
                            .request
                            .is_some();
                        if !prepared {
                            return Err(MobileSyncRuntimeError::JournalCorrupt);
                        }
                    }
                    state.requests.push((endpoint, exact_body));
                    state
                        .responses
                        .pop_front()
                        .unwrap_or(Err(MobileSyncRuntimeError::TransportUnavailable))
                });
            Box::pin(async move { result })
        }
    }

    fn actor_fixture(
        session_responses: Vec<Result<DirectResponse, MobileSyncRuntimeError>>,
    ) -> (
        MobileSyncRequestActor<TestJournal, TestCrypto, TestSession>,
        Arc<Mutex<JournalState>>,
        Arc<Mutex<SessionState>>,
    ) {
        let device_key = signing_key(7);
        let authority_key = signing_key(11);
        let profile = profile(&device_key, &authority_key);
        let journal_state = Arc::new(Mutex::new(JournalState::default()));
        let journal = TestJournal {
            profile,
            state: Arc::clone(&journal_state),
        };
        let session_state = Arc::new(Mutex::new(SessionState {
            requests: Vec::new(),
            responses: session_responses.into(),
            require_prepared_journal: Some(Arc::clone(&journal_state)),
        }));
        let session = TestSession(Arc::clone(&session_state));
        let actor = MobileSyncRequestActor::new(
            journal,
            TestCrypto::new(device_key, [REQUEST_ID]),
            session,
            DirectSyncLimits::default(),
        )
        .expect("create request actor");
        (actor, journal_state, session_state)
    }

    #[tokio::test]
    async fn actor_journals_before_transport_and_response_before_apply() {
        let authority_key = signing_key(11);
        let response = authority_response(
            &authority_key,
            DirectEndpoint::Checkpoint,
            REQUEST_ID,
            checkpoint_payload(),
        );
        let (mut actor, journal_state, session_state) = actor_fixture(vec![Ok(response)]);

        let verified: VerifiedSyncResponse<CheckpointResponse> = actor
            .begin(
                ExactRequestPurpose::Checkpoint {
                    known_cursor: Some(6),
                },
                CheckpointRequest {
                    known_cursor: Some(6),
                },
            )
            .await
            .expect("send checkpoint request");
        assert_eq!(verified.payload.checkpoint.high_water_cursor, 7);
        let request_digest = {
            let state = journal_state.lock().expect("journal state");
            let request = state.request.as_ref().expect("request remains until apply");
            assert_eq!(request.state, ExactRequestState::ResponseStored);
            assert!(request.response.is_some());
            request.request_body_sha256
        };
        assert_eq!(
            session_state.lock().expect("session state").requests.len(),
            1
        );

        actor
            .complete_verified(&ExactRequestCompletion {
                endpoint: DirectEndpoint::Checkpoint,
                request_id: REQUEST_ID.to_owned(),
                request_body_sha256: request_digest,
            })
            .expect("complete after semantic apply");
        let state = journal_state.lock().expect("journal state");
        assert!(state.request.is_none());
        assert_eq!(state.completed, [REQUEST_ID]);
    }

    #[tokio::test]
    async fn transport_failure_preserves_and_replays_identical_request_bytes() {
        let authority_key = signing_key(11);
        let response = authority_response(
            &authority_key,
            DirectEndpoint::Checkpoint,
            REQUEST_ID,
            checkpoint_payload(),
        );
        let (mut actor, journal_state, session_state) = actor_fixture(vec![
            Err(MobileSyncRuntimeError::TransportUnavailable),
            Ok(response),
        ]);

        let first = actor
            .begin::<_, CheckpointResponse>(
                ExactRequestPurpose::Checkpoint {
                    known_cursor: Some(6),
                },
                CheckpointRequest {
                    known_cursor: Some(6),
                },
            )
            .await;
        assert_eq!(first, Err(MobileSyncRuntimeError::TransportUnavailable));
        let original = journal_state
            .lock()
            .expect("journal state")
            .request
            .as_ref()
            .expect("request is recoverable")
            .request_body
            .clone();

        let recovered = actor
            .recover::<CheckpointResponse>(DirectEndpoint::Checkpoint)
            .await
            .expect("recover exact request")
            .expect("response available");
        assert_eq!(recovered.payload.checkpoint.high_water_cursor, 7);
        let state = session_state.lock().expect("session state");
        assert_eq!(state.requests.len(), 2);
        assert_eq!(state.requests[0].1, original);
        assert_eq!(state.requests[1].1, original);
        assert_eq!(
            journal_state
                .lock()
                .expect("journal state")
                .request
                .as_ref()
                .expect("request remains")
                .attempt_count,
            2
        );
    }

    #[tokio::test]
    async fn recovery_uses_committed_response_without_network_replay() {
        let authority_key = signing_key(11);
        let response = authority_response(
            &authority_key,
            DirectEndpoint::Checkpoint,
            REQUEST_ID,
            checkpoint_payload(),
        );
        let (mut actor, _journal_state, session_state) = actor_fixture(vec![Ok(response)]);
        actor
            .begin::<_, CheckpointResponse>(
                ExactRequestPurpose::Checkpoint {
                    known_cursor: Some(6),
                },
                CheckpointRequest {
                    known_cursor: Some(6),
                },
            )
            .await
            .expect("store verified response");
        assert_eq!(
            session_state.lock().expect("session state").requests.len(),
            1
        );

        let recovered = actor
            .recover::<CheckpointResponse>(DirectEndpoint::Checkpoint)
            .await
            .expect("recover committed response")
            .expect("response remains pending apply");
        assert_eq!(recovered.payload.checkpoint.high_water_cursor, 7);
        assert_eq!(
            session_state.lock().expect("session state").requests.len(),
            1
        );
    }

    #[tokio::test]
    async fn authenticated_revocation_is_stored_before_interpretation() {
        let revoked = DirectResponse {
            status: 403,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body: br#"{"error":{"code":"device_revoked"}}"#.to_vec(),
        };
        let (mut actor, journal_state, session_state) = actor_fixture(vec![Ok(revoked)]);

        let result = actor
            .begin::<_, CheckpointResponse>(
                ExactRequestPurpose::Checkpoint {
                    known_cursor: Some(6),
                },
                CheckpointRequest {
                    known_cursor: Some(6),
                },
            )
            .await;
        assert_eq!(result, Err(MobileSyncRuntimeError::DeviceRevoked));
        {
            let state = journal_state.lock().expect("journal state");
            let request = state
                .request
                .as_ref()
                .expect("revocation response retained");
            assert_eq!(request.state, ExactRequestState::ResponseStored);
            assert_eq!(request.response.as_ref().expect("response").status, 403);
        }
        assert_eq!(
            actor
                .recover::<CheckpointResponse>(DirectEndpoint::Checkpoint)
                .await,
            Err(MobileSyncRuntimeError::DeviceRevoked)
        );
        assert_eq!(
            session_state.lock().expect("session state").requests.len(),
            1
        );
    }

    #[test]
    fn revoked_or_overflowed_profiles_and_semantics_fail_closed() {
        let device_key = signing_key(7);
        let authority_key = signing_key(11);
        let mut profile = profile(&device_key, &authority_key);
        profile.revoked = true;
        assert_eq!(
            profile.validate_fixture(),
            Err(MobileSyncRuntimeError::InvalidActiveProfile)
        );
        assert_eq!(
            ExactRequestPurpose::Pull {
                requested_cursor: i64::MAX as u64 + 1,
                limit: 1,
                requested_record_kinds: ["note".to_owned()].into_iter().collect(),
            }
            .validate(),
            Err(MobileSyncRuntimeError::InvalidSemanticReference)
        );
    }
}
