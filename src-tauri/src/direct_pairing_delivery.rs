//! Authenticated, HTTP-independent delivery of an approved pairing bootstrap.
//!
//! `ClientHello` may complete before the Mac owner approves an enrollment.  A
//! phone therefore needs one deliberately narrow follow-up operation: poll the
//! same receipt until its exact [`BootstrapEnvelope`] is ready.  This module
//! defines that operation without introducing a bearer credential, URL, or
//! general command surface.
//!
//! The transport adapter is responsible for accepting only the fixed pairing
//! route and for supplying peer-certificate evidence.  A durable coordinator
//! implements [`BootstrapDeliveryStore`]; the coordinator commits the exact
//! signed response before returning it, so a crash or concurrent retry cannot
//! produce two observable answers for one message id.

use crate::{
    pairing_protocol::{
        bootstrap_envelope_digest, validate_bootstrap_key_package_envelope,
        validate_fixture_scopes_and_capabilities, BootstrapEnvelope, Environment, LibraryDataClass,
        PairingRole, ScopeClass, BOOTSTRAP_METADATA_VERSION, BOOTSTRAP_SYNC_PROTOCOL_VERSION,
        PAIRING_PROTOCOL, PAIRING_SUITE, RECORD_CIPHER_SUITE,
    },
    portable::{canonical_json, is_uuid_v7},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt;

pub const BOOTSTRAP_DELIVERY_ROUTE: &str = "/pairing/v1/bootstrap";
pub const MAX_BOOTSTRAP_POLL_REQUEST_BYTES: usize = 8 * 1024;
pub const MAX_BOOTSTRAP_ENVELOPE_BYTES: usize = 16 * 1024;
pub const MAX_BOOTSTRAP_POLL_RESPONSE_BYTES: usize = 96 * 1024;
pub const MIN_BOOTSTRAP_RETRY_AFTER_MS: u32 = 250;
pub const MAX_BOOTSTRAP_RETRY_AFTER_MS: u32 = 5_000;

const SHA256_BYTES: usize = 32;
const P256_PUBLIC_KEY_BYTES: usize = 65;
const P1363_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapPollRequest {
    pub protocol: String,
    pub suite: String,
    pub message_id: String,
    pub receipt_id: String,
    pub device_id: String,
    pub transcript_digest: Vec<u8>,
    pub tls_spki_sha256: Vec<u8>,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub proof_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BootstrapPollDisposition {
    Pending {
        retry_after_ms: u32,
    },
    Ready {
        exact_bootstrap_envelope: Vec<u8>,
        exact_bootstrap_envelope_sha256: Vec<u8>,
    },
    Rejected {
        reason: BootstrapDeliveryTerminal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapDeliveryTerminal {
    Cancelled,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapPollResponse {
    pub protocol: String,
    pub suite: String,
    pub request_message_id: String,
    pub exact_request_sha256: Vec<u8>,
    pub receipt_id: String,
    pub device_id: String,
    pub transcript_digest: Vec<u8>,
    pub tls_spki_sha256: Vec<u8>,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub disposition: BootstrapPollDisposition,
    pub proof_signature: Vec<u8>,
}

impl BootstrapPollResponse {
    /// Status for a thin HTTPS adapter; the signed body remains authoritative.
    pub const fn http_status(&self) -> u16 {
        match self.disposition {
            BootstrapPollDisposition::Pending { .. } => 202,
            BootstrapPollDisposition::Ready { .. } => 200,
            BootstrapPollDisposition::Rejected {
                reason: BootstrapDeliveryTerminal::Expired,
            } => 410,
            BootstrapPollDisposition::Rejected { .. } => 403,
        }
    }
}

/// Authority-owned receipt facts. None of these values may be sourced from a
/// poll request when a durable store constructs this binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeliveryBinding {
    pub receipt_id: String,
    pub device_id: String,
    pub transcript_digest: [u8; SHA256_BYTES],
    pub tls_spki_sha256: [u8; SHA256_BYTES],
    pub iphone_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    pub mac_pairing_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapDeliveryResolution {
    Pending { retry_after_ms: u32 },
    Ready { exact_bootstrap_envelope: Vec<u8> },
    Rejected { reason: BootstrapDeliveryTerminal },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeliverySnapshot {
    pub binding: BootstrapDeliveryBinding,
    pub resolution: BootstrapDeliveryResolution,
}

/// Exact request/response pair committed by the durable coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeliveryReplay {
    pub message_id: String,
    pub receipt_id: String,
    pub device_id: String,
    pub tls_spki_sha256: [u8; SHA256_BYTES],
    pub exact_request_sha256: [u8; SHA256_BYTES],
    pub exact_response_sha256: [u8; SHA256_BYTES],
    pub exact_response_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapReplayCommit {
    Inserted,
    Existing(BootstrapDeliveryReplay),
    Conflict,
}

/// Persistence boundary for an atomic, restart-safe pairing delivery adapter.
///
/// `commit_replay` must use a uniqueness constraint on `message_id`. On a
/// race, it returns the winning row as `Existing`; it must never overwrite it.
/// The coordinator returns bytes only after this method succeeds.
pub trait BootstrapDeliveryStore: Send + Sync {
    type Error: fmt::Display;

    fn load_delivery(
        &self,
        receipt_id: &str,
    ) -> Result<Option<BootstrapDeliverySnapshot>, Self::Error>;

    fn load_replay(&self, message_id: &str)
        -> Result<Option<BootstrapDeliveryReplay>, Self::Error>;

    fn commit_replay(
        &self,
        replay: &BootstrapDeliveryReplay,
    ) -> Result<BootstrapReplayCommit, Self::Error>;
}

#[allow(clippy::result_unit_err)]
pub trait BootstrapDeliveryVerifier: Send + Sync {
    fn verify_p256_p1363(
        &self,
        signer_role: PairingRole,
        public_key: &[u8; P256_PUBLIC_KEY_BYTES],
        message: &[u8],
        signature: &[u8; P1363_SIGNATURE_BYTES],
    ) -> Result<(), ()>;
}

#[allow(clippy::result_unit_err)]
pub trait IphoneBootstrapPollSigner: Send + Sync {
    fn sign_iphone_poll(&self, message: &[u8]) -> Result<[u8; P1363_SIGNATURE_BYTES], ()>;
}

#[allow(clippy::result_unit_err)]
pub trait MacBootstrapDeliverySigner: Send + Sync {
    fn sign_mac_delivery(&self, message: &[u8]) -> Result<[u8; P1363_SIGNATURE_BYTES], ()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeliveryTransport {
    pub tls_version: String,
    pub used_zero_rtt: bool,
    pub peer_spki_sha256: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapDeliveryError {
    PayloadTooLarge,
    ParseRejected,
    UnsupportedProtocol,
    UnsupportedSuite,
    InvalidIdentifier,
    InvalidField(&'static str),
    InvalidSignature,
    CryptoUnavailable,
    InsecureTransport,
    PinMismatch,
    ReceiptNotFound,
    BindingMismatch(&'static str),
    ReplayConflict,
    Store(String),
}

impl fmt::Display for BootstrapDeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("bootstrap delivery payload too large"),
            Self::ParseRejected => formatter.write_str("bootstrap delivery payload rejected"),
            Self::UnsupportedProtocol => formatter.write_str("unsupported pairing protocol"),
            Self::UnsupportedSuite => formatter.write_str("unsupported pairing suite"),
            Self::InvalidIdentifier => formatter.write_str("invalid bootstrap delivery identifier"),
            Self::InvalidField(field) => {
                write!(formatter, "invalid bootstrap delivery field: {field}")
            }
            Self::InvalidSignature => formatter.write_str("bootstrap delivery signature rejected"),
            Self::CryptoUnavailable => {
                formatter.write_str("bootstrap delivery cryptography unavailable")
            }
            Self::InsecureTransport => {
                formatter.write_str("bootstrap delivery requires TLS 1.3 without 0-RTT")
            }
            Self::PinMismatch => formatter.write_str("bootstrap delivery TLS pin mismatch"),
            Self::ReceiptNotFound => formatter.write_str("bootstrap delivery receipt not found"),
            Self::BindingMismatch(field) => {
                write!(formatter, "bootstrap delivery binding mismatch: {field}")
            }
            Self::ReplayConflict => {
                formatter.write_str("byte-different bootstrap poll message id reuse")
            }
            Self::Store(reason) => write!(formatter, "bootstrap delivery store failed: {reason}"),
        }
    }
}

impl std::error::Error for BootstrapDeliveryError {}

pub struct BootstrapDeliveryCoordinator<S, C> {
    store: S,
    crypto: C,
}

impl<S, C> BootstrapDeliveryCoordinator<S, C>
where
    S: BootstrapDeliveryStore,
    C: BootstrapDeliveryVerifier + MacBootstrapDeliverySigner,
{
    pub fn new(store: S, crypto: C) -> Self {
        Self { store, crypto }
    }

    /// Authenticate, resolve, durably commit, then return an exact response.
    pub fn handle_poll(
        &self,
        exact_request_bytes: &[u8],
        transport: &BootstrapDeliveryTransport,
    ) -> Result<Vec<u8>, BootstrapDeliveryError> {
        let request = parse_poll_request(exact_request_bytes)?;
        let snapshot = self
            .store
            .load_delivery(&request.receipt_id)
            .map_err(|error| BootstrapDeliveryError::Store(error.to_string()))?
            .ok_or(BootstrapDeliveryError::ReceiptNotFound)?;
        validate_request(&request, &snapshot.binding, transport, &self.crypto)?;

        let exact_request_sha256 = sha256_array(exact_request_bytes);
        if let Some(replay) = self
            .store
            .load_replay(&request.message_id)
            .map_err(|error| BootstrapDeliveryError::Store(error.to_string()))?
        {
            validate_replay(&replay, &request, &snapshot.binding, &exact_request_sha256)?;
            let response: BootstrapPollResponse = parse_bounded(
                &replay.exact_response_bytes,
                MAX_BOOTSTRAP_POLL_RESPONSE_BYTES,
            )?;
            validate_response(
                &response,
                &request,
                &snapshot.binding,
                &exact_request_sha256,
                &self.crypto,
            )?;
            return Ok(replay.exact_response_bytes);
        }

        let disposition = match snapshot.resolution {
            BootstrapDeliveryResolution::Pending { retry_after_ms } => {
                validate_retry_after(retry_after_ms)?;
                BootstrapPollDisposition::Pending { retry_after_ms }
            }
            BootstrapDeliveryResolution::Ready {
                exact_bootstrap_envelope,
            } => {
                validate_exact_bootstrap_envelope(&exact_bootstrap_envelope, &snapshot.binding)?;
                BootstrapPollDisposition::Ready {
                    exact_bootstrap_envelope_sha256: sha256_array(&exact_bootstrap_envelope)
                        .to_vec(),
                    exact_bootstrap_envelope,
                }
            }
            BootstrapDeliveryResolution::Rejected { reason } => {
                BootstrapPollDisposition::Rejected { reason }
            }
        };
        let response = signed_response(
            &request,
            &snapshot.binding,
            exact_request_sha256,
            disposition,
            &self.crypto,
        )?;
        let exact_response_bytes =
            serde_json::to_vec(&response).map_err(|_| BootstrapDeliveryError::ParseRejected)?;
        if exact_response_bytes.len() > MAX_BOOTSTRAP_POLL_RESPONSE_BYTES {
            return Err(BootstrapDeliveryError::PayloadTooLarge);
        }
        let replay = BootstrapDeliveryReplay {
            message_id: request.message_id.clone(),
            receipt_id: request.receipt_id.clone(),
            device_id: request.device_id.clone(),
            tls_spki_sha256: snapshot.binding.tls_spki_sha256,
            exact_request_sha256,
            exact_response_sha256: sha256_array(&exact_response_bytes),
            exact_response_bytes,
        };
        match self
            .store
            .commit_replay(&replay)
            .map_err(|error| BootstrapDeliveryError::Store(error.to_string()))?
        {
            BootstrapReplayCommit::Inserted => Ok(replay.exact_response_bytes),
            BootstrapReplayCommit::Existing(existing) => {
                validate_replay(
                    &existing,
                    &request,
                    &snapshot.binding,
                    &exact_request_sha256,
                )?;
                let response: BootstrapPollResponse = parse_bounded(
                    &existing.exact_response_bytes,
                    MAX_BOOTSTRAP_POLL_RESPONSE_BYTES,
                )?;
                validate_response(
                    &response,
                    &request,
                    &snapshot.binding,
                    &exact_request_sha256,
                    &self.crypto,
                )?;
                Ok(existing.exact_response_bytes)
            }
            BootstrapReplayCommit::Conflict => Err(BootstrapDeliveryError::ReplayConflict),
        }
    }
}

pub fn sign_poll_request<S: IphoneBootstrapPollSigner>(
    binding: &BootstrapDeliveryBinding,
    message_id: String,
    signer: &S,
) -> Result<Vec<u8>, BootstrapDeliveryError> {
    validate_binding(binding)?;
    if !is_uuid_v7(&message_id) {
        return Err(BootstrapDeliveryError::InvalidIdentifier);
    }
    let mut request = BootstrapPollRequest {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id,
        receipt_id: binding.receipt_id.clone(),
        device_id: binding.device_id.clone(),
        transcript_digest: binding.transcript_digest.to_vec(),
        tls_spki_sha256: binding.tls_spki_sha256.to_vec(),
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        proof_signature: Vec::new(),
    };
    request.proof_signature = signer
        .sign_iphone_poll(&request_signing_bytes(&request)?)
        .map_err(|_| BootstrapDeliveryError::CryptoUnavailable)?
        .to_vec();
    let bytes = serde_json::to_vec(&request).map_err(|_| BootstrapDeliveryError::ParseRejected)?;
    if bytes.len() > MAX_BOOTSTRAP_POLL_REQUEST_BYTES {
        return Err(BootstrapDeliveryError::PayloadTooLarge);
    }
    Ok(bytes)
}

pub fn parse_poll_request(bytes: &[u8]) -> Result<BootstrapPollRequest, BootstrapDeliveryError> {
    parse_bounded(bytes, MAX_BOOTSTRAP_POLL_REQUEST_BYTES)
}

pub fn parse_and_verify_poll_response<V: BootstrapDeliveryVerifier>(
    exact_response_bytes: &[u8],
    exact_request_bytes: &[u8],
    binding: &BootstrapDeliveryBinding,
    verifier: &V,
) -> Result<BootstrapPollResponse, BootstrapDeliveryError> {
    validate_binding(binding)?;
    let request = parse_poll_request(exact_request_bytes)?;
    let response: BootstrapPollResponse =
        parse_bounded(exact_response_bytes, MAX_BOOTSTRAP_POLL_RESPONSE_BYTES)?;
    validate_response(
        &response,
        &request,
        binding,
        &sha256_array(exact_request_bytes),
        verifier,
    )?;
    Ok(response)
}

pub fn request_signing_bytes(
    request: &BootstrapPollRequest,
) -> Result<Vec<u8>, BootstrapDeliveryError> {
    let mut value =
        serde_json::to_value(request).map_err(|_| BootstrapDeliveryError::ParseRejected)?;
    empty_signature(&mut value)?;
    Ok(canonical_json(&json!({
        "domain": "noted.direct-pairing.v1/bootstrap-poll/request",
        "route": BOOTSTRAP_DELIVERY_ROUTE,
        "request": value,
    }))
    .into_bytes())
}

pub fn response_signing_bytes(
    response: &BootstrapPollResponse,
) -> Result<Vec<u8>, BootstrapDeliveryError> {
    let mut value =
        serde_json::to_value(response).map_err(|_| BootstrapDeliveryError::ParseRejected)?;
    empty_signature(&mut value)?;
    Ok(canonical_json(&json!({
        "domain": "noted.direct-pairing.v1/bootstrap-poll/response",
        "route": BOOTSTRAP_DELIVERY_ROUTE,
        "response": value,
    }))
    .into_bytes())
}

fn signed_response<S: MacBootstrapDeliverySigner>(
    request: &BootstrapPollRequest,
    binding: &BootstrapDeliveryBinding,
    exact_request_sha256: [u8; SHA256_BYTES],
    disposition: BootstrapPollDisposition,
    signer: &S,
) -> Result<BootstrapPollResponse, BootstrapDeliveryError> {
    let mut response = BootstrapPollResponse {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        request_message_id: request.message_id.clone(),
        exact_request_sha256: exact_request_sha256.to_vec(),
        receipt_id: binding.receipt_id.clone(),
        device_id: binding.device_id.clone(),
        transcript_digest: binding.transcript_digest.to_vec(),
        tls_spki_sha256: binding.tls_spki_sha256.to_vec(),
        sender_role: PairingRole::MacAuthority,
        recipient_role: PairingRole::IphoneCompanion,
        disposition,
        proof_signature: Vec::new(),
    };
    response.proof_signature = signer
        .sign_mac_delivery(&response_signing_bytes(&response)?)
        .map_err(|_| BootstrapDeliveryError::CryptoUnavailable)?
        .to_vec();
    Ok(response)
}

fn validate_request<V: BootstrapDeliveryVerifier>(
    request: &BootstrapPollRequest,
    binding: &BootstrapDeliveryBinding,
    transport: &BootstrapDeliveryTransport,
    verifier: &V,
) -> Result<(), BootstrapDeliveryError> {
    validate_binding(binding)?;
    validate_request_claims(request, binding)?;
    validate_transport(transport, binding)?;
    let signature = to_array::<P1363_SIGNATURE_BYTES>(&request.proof_signature, "proof_signature")?;
    verifier
        .verify_p256_p1363(
            PairingRole::IphoneCompanion,
            &binding.iphone_signing_public_key,
            &request_signing_bytes(request)?,
            &signature,
        )
        .map_err(|_| BootstrapDeliveryError::InvalidSignature)
}

fn validate_response<V: BootstrapDeliveryVerifier>(
    response: &BootstrapPollResponse,
    request: &BootstrapPollRequest,
    binding: &BootstrapDeliveryBinding,
    exact_request_sha256: &[u8; SHA256_BYTES],
    verifier: &V,
) -> Result<(), BootstrapDeliveryError> {
    validate_request_claims(request, binding)?;
    validate_protocol_and_ids(
        &response.protocol,
        &response.suite,
        &response.request_message_id,
        &response.receipt_id,
        &response.device_id,
    )?;
    if response.sender_role != PairingRole::MacAuthority
        || response.recipient_role != PairingRole::IphoneCompanion
    {
        return Err(BootstrapDeliveryError::BindingMismatch("roles"));
    }
    if response.request_message_id != request.message_id
        || response.receipt_id != binding.receipt_id
        || response.device_id != binding.device_id
        || response.transcript_digest.as_slice() != binding.transcript_digest
        || response.exact_request_sha256.as_slice() != exact_request_sha256
    {
        return Err(BootstrapDeliveryError::BindingMismatch("poll request"));
    }
    if response.tls_spki_sha256.as_slice() != binding.tls_spki_sha256 {
        return Err(BootstrapDeliveryError::PinMismatch);
    }
    match &response.disposition {
        BootstrapPollDisposition::Pending { retry_after_ms } => {
            validate_retry_after(*retry_after_ms)?;
        }
        BootstrapPollDisposition::Ready {
            exact_bootstrap_envelope,
            exact_bootstrap_envelope_sha256,
        } => {
            if exact_bootstrap_envelope_sha256.as_slice() != sha256_array(exact_bootstrap_envelope)
            {
                return Err(BootstrapDeliveryError::BindingMismatch(
                    "exact bootstrap envelope digest",
                ));
            }
            validate_exact_bootstrap_envelope(exact_bootstrap_envelope, binding)?;
        }
        BootstrapPollDisposition::Rejected { .. } => {}
    }
    let signature =
        to_array::<P1363_SIGNATURE_BYTES>(&response.proof_signature, "proof_signature")?;
    verifier
        .verify_p256_p1363(
            PairingRole::MacAuthority,
            &binding.mac_pairing_signing_public_key,
            &response_signing_bytes(response)?,
            &signature,
        )
        .map_err(|_| BootstrapDeliveryError::InvalidSignature)
}

fn validate_exact_bootstrap_envelope(
    exact_bytes: &[u8],
    binding: &BootstrapDeliveryBinding,
) -> Result<(), BootstrapDeliveryError> {
    let envelope: BootstrapEnvelope = parse_bounded(exact_bytes, MAX_BOOTSTRAP_ENVELOPE_BYTES)?;
    if envelope.protocol != PAIRING_PROTOCOL || envelope.metadata.protocol != PAIRING_PROTOCOL {
        return Err(BootstrapDeliveryError::UnsupportedProtocol);
    }
    if envelope.metadata.suite != PAIRING_SUITE {
        return Err(BootstrapDeliveryError::UnsupportedSuite);
    }
    if envelope.receipt_id != binding.receipt_id
        || envelope.metadata.receipt_id != binding.receipt_id
        || envelope.metadata.device_id != binding.device_id
        || envelope.metadata.transcript_digest.as_slice() != binding.transcript_digest
    {
        return Err(BootstrapDeliveryError::BindingMismatch(
            "bootstrap envelope",
        ));
    }
    if envelope.metadata.version != BOOTSTRAP_METADATA_VERSION
        || envelope.metadata.sync_protocol_version != BOOTSTRAP_SYNC_PROTOCOL_VERSION
        || envelope.metadata.environment != Environment::Development
        || envelope.metadata.library_data_class != LibraryDataClass::SanitizedFixture
        || envelope.metadata.authority_generation == 0
        || envelope.metadata.key_epoch == 0
        || envelope.metadata.authority_generation > i64::MAX as u64
        || envelope.metadata.purge_generation > i64::MAX as u64
        || envelope.metadata.key_epoch > i64::MAX as u64
        || envelope.metadata.default_scope_class != ScopeClass::Unknown
        || envelope.metadata.record_cipher_suite != RECORD_CIPHER_SUITE
        || !is_uuid_v7(&envelope.metadata.library_id)
        || !is_uuid_v7(&envelope.metadata.default_scope_id)
        || envelope.metadata.durable_sync_spki_sha256.len() != SHA256_BYTES
    {
        return Err(BootstrapDeliveryError::InvalidField(
            "bootstrap envelope metadata",
        ));
    }
    validate_bootstrap_key_package_envelope(&envelope.sealed_key_package)
        .map_err(|_| BootstrapDeliveryError::InvalidField("sealed_key_package"))?;
    validate_fixture_scopes_and_capabilities(
        &envelope.metadata.granted_scopes,
        &envelope.metadata.capabilities,
    )
    .map_err(|_| BootstrapDeliveryError::InvalidField("bootstrap capabilities"))?;
    if envelope.envelope_digest.len() != SHA256_BYTES
        || envelope.envelope_digest != bootstrap_envelope_digest(&envelope)
    {
        return Err(BootstrapDeliveryError::BindingMismatch(
            "bootstrap envelope digest",
        ));
    }
    Ok(())
}

fn validate_binding(binding: &BootstrapDeliveryBinding) -> Result<(), BootstrapDeliveryError> {
    if !is_uuid_v7(&binding.receipt_id) || !is_uuid_v7(&binding.device_id) {
        return Err(BootstrapDeliveryError::InvalidIdentifier);
    }
    if binding.tls_spki_sha256.iter().all(|byte| *byte == 0) {
        return Err(BootstrapDeliveryError::InvalidField("tls_spki_sha256"));
    }
    validate_public_key(&binding.iphone_signing_public_key)?;
    validate_public_key(&binding.mac_pairing_signing_public_key)?;
    Ok(())
}

fn validate_request_claims(
    request: &BootstrapPollRequest,
    binding: &BootstrapDeliveryBinding,
) -> Result<(), BootstrapDeliveryError> {
    validate_protocol_and_ids(
        &request.protocol,
        &request.suite,
        &request.message_id,
        &request.receipt_id,
        &request.device_id,
    )?;
    if request.sender_role != PairingRole::IphoneCompanion
        || request.recipient_role != PairingRole::MacAuthority
    {
        return Err(BootstrapDeliveryError::BindingMismatch("roles"));
    }
    if request.receipt_id != binding.receipt_id
        || request.device_id != binding.device_id
        || request.transcript_digest.as_slice() != binding.transcript_digest
    {
        return Err(BootstrapDeliveryError::BindingMismatch(
            "receipt transcript",
        ));
    }
    if request.tls_spki_sha256.as_slice() != binding.tls_spki_sha256 {
        return Err(BootstrapDeliveryError::PinMismatch);
    }
    to_array::<P1363_SIGNATURE_BYTES>(&request.proof_signature, "proof_signature")?;
    Ok(())
}

fn validate_public_key(
    public_key: &[u8; P256_PUBLIC_KEY_BYTES],
) -> Result<(), BootstrapDeliveryError> {
    // The verifier must additionally decode and validate the SEC1 point.
    if public_key[0] != 0x04 || public_key[1..].iter().all(|byte| *byte == 0) {
        return Err(BootstrapDeliveryError::InvalidField("p256_public_key"));
    }
    Ok(())
}

fn validate_protocol_and_ids(
    protocol: &str,
    suite: &str,
    message_id: &str,
    receipt_id: &str,
    device_id: &str,
) -> Result<(), BootstrapDeliveryError> {
    if protocol != PAIRING_PROTOCOL {
        return Err(BootstrapDeliveryError::UnsupportedProtocol);
    }
    if suite != PAIRING_SUITE {
        return Err(BootstrapDeliveryError::UnsupportedSuite);
    }
    if !is_uuid_v7(message_id) || !is_uuid_v7(receipt_id) || !is_uuid_v7(device_id) {
        return Err(BootstrapDeliveryError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_transport(
    transport: &BootstrapDeliveryTransport,
    binding: &BootstrapDeliveryBinding,
) -> Result<(), BootstrapDeliveryError> {
    if transport.tls_version != "1.3" || transport.used_zero_rtt {
        return Err(BootstrapDeliveryError::InsecureTransport);
    }
    if transport.peer_spki_sha256.as_slice() != binding.tls_spki_sha256 {
        return Err(BootstrapDeliveryError::PinMismatch);
    }
    Ok(())
}

fn validate_retry_after(retry_after_ms: u32) -> Result<(), BootstrapDeliveryError> {
    if !(MIN_BOOTSTRAP_RETRY_AFTER_MS..=MAX_BOOTSTRAP_RETRY_AFTER_MS).contains(&retry_after_ms) {
        return Err(BootstrapDeliveryError::InvalidField("retry_after_ms"));
    }
    Ok(())
}

fn validate_replay(
    replay: &BootstrapDeliveryReplay,
    request: &BootstrapPollRequest,
    binding: &BootstrapDeliveryBinding,
    exact_request_sha256: &[u8; SHA256_BYTES],
) -> Result<(), BootstrapDeliveryError> {
    if replay.message_id != request.message_id
        || replay.receipt_id != binding.receipt_id
        || replay.device_id != binding.device_id
        || replay.tls_spki_sha256 != binding.tls_spki_sha256
        || &replay.exact_request_sha256 != exact_request_sha256
        || replay.exact_response_sha256 != sha256_array(&replay.exact_response_bytes)
        || replay.exact_response_bytes.len() > MAX_BOOTSTRAP_POLL_RESPONSE_BYTES
    {
        return Err(BootstrapDeliveryError::ReplayConflict);
    }
    Ok(())
}

fn empty_signature(value: &mut Value) -> Result<(), BootstrapDeliveryError> {
    value
        .as_object_mut()
        .ok_or(BootstrapDeliveryError::ParseRejected)?
        .insert("proof_signature".to_owned(), json!([]));
    Ok(())
}

fn parse_bounded<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, BootstrapDeliveryError> {
    if bytes.len() > limit {
        return Err(BootstrapDeliveryError::PayloadTooLarge);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value =
        T::deserialize(&mut deserializer).map_err(|_| BootstrapDeliveryError::ParseRejected)?;
    deserializer
        .end()
        .map_err(|_| BootstrapDeliveryError::ParseRejected)?;
    Ok(value)
}

fn to_array<const N: usize>(
    bytes: &[u8],
    field: &'static str,
) -> Result<[u8; N], BootstrapDeliveryError> {
    bytes
        .try_into()
        .map_err(|_| BootstrapDeliveryError::InvalidField(field))
}

fn sha256_array(bytes: &[u8]) -> [u8; SHA256_BYTES] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing_protocol::{
        AuthenticatedHpkeEnvelope, BootstrapMetadataV1, KindCapability, RecordKind,
        BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES, HPKE_ENCAPSULATED_KEY_BYTES,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Mutex,
    };

    const RECEIPT: &str = "018f0000-0000-7000-8000-000000000001";
    const DEVICE: &str = "018f0000-0000-7000-8000-000000000002";
    const MESSAGE_ONE: &str = "018f0000-0000-7000-8000-000000000003";
    const MESSAGE_TWO: &str = "018f0000-0000-7000-8000-000000000004";
    const LIBRARY: &str = "018f0000-0000-7000-8000-000000000005";
    const SCOPE: &str = "018f0000-0000-7000-8000-000000000006";

    #[derive(Clone, Copy)]
    struct TestCrypto;

    impl TestCrypto {
        fn signature(role: PairingRole, key: &[u8], message: &[u8]) -> [u8; 64] {
            let role = match role {
                PairingRole::MacAuthority => b"mac".as_slice(),
                PairingRole::IphoneCompanion => b"iphone".as_slice(),
            };
            let mut first = Sha256::new();
            first.update(role);
            first.update(key);
            first.update(message);
            let first = first.finalize();
            let mut second = Sha256::new();
            second.update(b"p1363-test-half");
            second.update(first);
            let second = second.finalize();
            let mut signature = [0; 64];
            signature[..32].copy_from_slice(&first);
            signature[32..].copy_from_slice(&second);
            signature
        }
    }

    impl BootstrapDeliveryVerifier for TestCrypto {
        fn verify_p256_p1363(
            &self,
            signer_role: PairingRole,
            public_key: &[u8; 65],
            message: &[u8],
            signature: &[u8; 64],
        ) -> Result<(), ()> {
            (Self::signature(signer_role, public_key, message) == *signature)
                .then_some(())
                .ok_or(())
        }
    }

    impl IphoneBootstrapPollSigner for TestCrypto {
        fn sign_iphone_poll(&self, message: &[u8]) -> Result<[u8; 64], ()> {
            Ok(Self::signature(
                PairingRole::IphoneCompanion,
                &binding().iphone_signing_public_key,
                message,
            ))
        }
    }

    impl MacBootstrapDeliverySigner for TestCrypto {
        fn sign_mac_delivery(&self, message: &[u8]) -> Result<[u8; 64], ()> {
            Ok(Self::signature(
                PairingRole::MacAuthority,
                &binding().mac_pairing_signing_public_key,
                message,
            ))
        }
    }

    struct TestStore {
        snapshot: Mutex<BootstrapDeliverySnapshot>,
        replays: Mutex<BTreeMap<String, BootstrapDeliveryReplay>>,
    }

    impl TestStore {
        fn pending() -> Self {
            Self {
                snapshot: Mutex::new(BootstrapDeliverySnapshot {
                    binding: binding(),
                    resolution: BootstrapDeliveryResolution::Pending {
                        retry_after_ms: 500,
                    },
                }),
                replays: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl BootstrapDeliveryStore for TestStore {
        type Error = &'static str;

        fn load_delivery(
            &self,
            receipt_id: &str,
        ) -> Result<Option<BootstrapDeliverySnapshot>, Self::Error> {
            let snapshot = self.snapshot.lock().map_err(|_| "poison")?.clone();
            Ok((snapshot.binding.receipt_id == receipt_id).then_some(snapshot))
        }

        fn load_replay(
            &self,
            message_id: &str,
        ) -> Result<Option<BootstrapDeliveryReplay>, Self::Error> {
            Ok(self
                .replays
                .lock()
                .map_err(|_| "poison")?
                .get(message_id)
                .cloned())
        }

        fn commit_replay(
            &self,
            replay: &BootstrapDeliveryReplay,
        ) -> Result<BootstrapReplayCommit, Self::Error> {
            let mut replays = self.replays.lock().map_err(|_| "poison")?;
            if let Some(existing) = replays.get(&replay.message_id) {
                return Ok(
                    if existing.exact_request_sha256 == replay.exact_request_sha256 {
                        BootstrapReplayCommit::Existing(existing.clone())
                    } else {
                        BootstrapReplayCommit::Conflict
                    },
                );
            }
            replays.insert(replay.message_id.clone(), replay.clone());
            Ok(BootstrapReplayCommit::Inserted)
        }
    }

    fn key(fill: u8) -> [u8; 65] {
        let mut key = [fill; 65];
        key[0] = 0x04;
        key
    }

    fn binding() -> BootstrapDeliveryBinding {
        BootstrapDeliveryBinding {
            receipt_id: RECEIPT.to_owned(),
            device_id: DEVICE.to_owned(),
            transcript_digest: [7; 32],
            tls_spki_sha256: [8; 32],
            iphone_signing_public_key: key(9),
            mac_pairing_signing_public_key: key(10),
        }
    }

    fn transport() -> BootstrapDeliveryTransport {
        BootstrapDeliveryTransport {
            tls_version: "1.3".to_owned(),
            used_zero_rtt: false,
            peer_spki_sha256: binding().tls_spki_sha256.to_vec(),
        }
    }

    fn envelope_bytes() -> Vec<u8> {
        let scopes = BTreeSet::from([RecordKind::Note, RecordKind::Category, RecordKind::Folder]);
        let capabilities = scopes
            .iter()
            .copied()
            .map(|kind| {
                (
                    kind,
                    KindCapability {
                        reader_version: 1,
                        writer_version: Some(1),
                    },
                )
            })
            .collect();
        let mut envelope = BootstrapEnvelope {
            protocol: PAIRING_PROTOCOL.to_owned(),
            receipt_id: RECEIPT.to_owned(),
            metadata: BootstrapMetadataV1 {
                version: BOOTSTRAP_METADATA_VERSION,
                protocol: PAIRING_PROTOCOL.to_owned(),
                suite: PAIRING_SUITE.to_owned(),
                sync_protocol_version: BOOTSTRAP_SYNC_PROTOCOL_VERSION,
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
                receipt_id: RECEIPT.to_owned(),
                library_id: LIBRARY.to_owned(),
                device_id: DEVICE.to_owned(),
                authority_generation: 1,
                purge_generation: 0,
                key_epoch: 1,
                default_scope_id: SCOPE.to_owned(),
                default_scope_class: ScopeClass::Unknown,
                granted_scopes: scopes,
                capabilities,
                record_cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
                durable_sync_spki_sha256: vec![11; 32],
                transcript_digest: vec![7; 32],
            },
            sealed_key_package: AuthenticatedHpkeEnvelope {
                encapsulated_key: vec![12; HPKE_ENCAPSULATED_KEY_BYTES],
                ciphertext: vec![13; BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES],
            },
            envelope_digest: Vec::new(),
        };
        envelope.envelope_digest = bootstrap_envelope_digest(&envelope);
        serde_json::to_vec(&envelope).unwrap()
    }

    fn request(message_id: &str) -> Vec<u8> {
        sign_poll_request(&binding(), message_id.to_owned(), &TestCrypto).unwrap()
    }

    #[test]
    fn pending_response_is_signed_request_bound_and_replayed_exactly() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let request = request(MESSAGE_ONE);
        let first = coordinator.handle_poll(&request, &transport()).unwrap();
        let second = coordinator.handle_poll(&request, &transport()).unwrap();
        assert_eq!(first, second);
        let response =
            parse_and_verify_poll_response(&first, &request, &binding(), &TestCrypto).unwrap();
        assert_eq!(response.http_status(), 202);
        assert_eq!(
            response.disposition,
            BootstrapPollDisposition::Pending {
                retry_after_ms: 500
            }
        );
    }

    #[test]
    fn a_new_message_observes_pending_to_ready_transition() {
        let store = TestStore::pending();
        let first = request(MESSAGE_ONE);
        let pending = BootstrapDeliveryCoordinator::new(&store, TestCrypto)
            .handle_poll(&first, &transport())
            .unwrap();
        assert_eq!(
            parse_and_verify_poll_response(&pending, &first, &binding(), &TestCrypto)
                .unwrap()
                .http_status(),
            202
        );
        store.snapshot.lock().unwrap().resolution = BootstrapDeliveryResolution::Ready {
            exact_bootstrap_envelope: envelope_bytes(),
        };
        let second = request(MESSAGE_TWO);
        let ready = BootstrapDeliveryCoordinator::new(&store, TestCrypto)
            .handle_poll(&second, &transport())
            .unwrap();
        let response =
            parse_and_verify_poll_response(&ready, &second, &binding(), &TestCrypto).unwrap();
        assert_eq!(response.http_status(), 200);
        let BootstrapPollDisposition::Ready {
            exact_bootstrap_envelope,
            exact_bootstrap_envelope_sha256,
        } = response.disposition
        else {
            panic!("expected ready")
        };
        assert_eq!(exact_bootstrap_envelope, envelope_bytes());
        assert_eq!(
            exact_bootstrap_envelope_sha256,
            sha256_array(&exact_bootstrap_envelope)
        );
    }

    #[test]
    fn terminal_pairing_states_are_signed_and_never_masquerade_as_pending() {
        for (reason, expected_status) in [
            (BootstrapDeliveryTerminal::Cancelled, 403),
            (BootstrapDeliveryTerminal::Expired, 410),
            (BootstrapDeliveryTerminal::Revoked, 403),
        ] {
            let store = TestStore::pending();
            store.snapshot.lock().unwrap().resolution =
                BootstrapDeliveryResolution::Rejected { reason };
            let exact_request = request(MESSAGE_ONE);
            let exact_response = BootstrapDeliveryCoordinator::new(store, TestCrypto)
                .handle_poll(&exact_request, &transport())
                .unwrap();
            let response = parse_and_verify_poll_response(
                &exact_response,
                &exact_request,
                &binding(),
                &TestCrypto,
            )
            .unwrap();
            assert_eq!(response.http_status(), expected_status);
            assert_eq!(
                response.disposition,
                BootstrapPollDisposition::Rejected { reason }
            );
        }
    }

    impl<T: BootstrapDeliveryStore + ?Sized> BootstrapDeliveryStore for &T {
        type Error = T::Error;

        fn load_delivery(
            &self,
            receipt_id: &str,
        ) -> Result<Option<BootstrapDeliverySnapshot>, Self::Error> {
            (**self).load_delivery(receipt_id)
        }

        fn load_replay(
            &self,
            message_id: &str,
        ) -> Result<Option<BootstrapDeliveryReplay>, Self::Error> {
            (**self).load_replay(message_id)
        }

        fn commit_replay(
            &self,
            replay: &BootstrapDeliveryReplay,
        ) -> Result<BootstrapReplayCommit, Self::Error> {
            (**self).commit_replay(replay)
        }
    }

    #[test]
    fn tampered_request_signature_is_rejected() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let mut value: Value = serde_json::from_slice(&request(MESSAGE_ONE)).unwrap();
        value["transcript_digest"][0] = json!(99);
        let error = coordinator
            .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
            .unwrap_err();
        assert!(matches!(
            error,
            BootstrapDeliveryError::BindingMismatch("receipt transcript")
                | BootstrapDeliveryError::InvalidSignature
        ));
    }

    #[test]
    fn role_swap_is_rejected() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let mut value: Value = serde_json::from_slice(&request(MESSAGE_ONE)).unwrap();
        value["sender_role"] = json!("mac_authority");
        assert_eq!(
            coordinator
                .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::BindingMismatch("roles")
        );
    }

    #[test]
    fn cross_receipt_request_is_rejected() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let mut value: Value = serde_json::from_slice(&request(MESSAGE_ONE)).unwrap();
        value["receipt_id"] = json!("018f0000-0000-7000-8000-000000000099");
        assert_eq!(
            coordinator
                .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::ReceiptNotFound
        );
    }

    #[test]
    fn cross_pin_transport_and_claim_are_rejected() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let mut wrong_transport = transport();
        wrong_transport.peer_spki_sha256[0] ^= 1;
        assert_eq!(
            coordinator
                .handle_poll(&request(MESSAGE_ONE), &wrong_transport)
                .unwrap_err(),
            BootstrapDeliveryError::PinMismatch
        );

        let mut value: Value = serde_json::from_slice(&request(MESSAGE_TWO)).unwrap();
        value["tls_spki_sha256"][0] = json!(99);
        assert_eq!(
            coordinator
                .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::PinMismatch
        );
    }

    #[test]
    fn byte_different_message_id_reuse_is_rejected() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let exact = request(MESSAGE_ONE);
        coordinator.handle_poll(&exact, &transport()).unwrap();
        let mut value: Value = serde_json::from_slice(&exact).unwrap();
        value["proof_signature"][0] = json!(99);
        assert!(matches!(
            coordinator
                .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::InvalidSignature | BootstrapDeliveryError::ReplayConflict
        ));
    }

    #[test]
    fn unknown_request_and_response_fields_are_rejected() {
        let mut request_value: Value = serde_json::from_slice(&request(MESSAGE_ONE)).unwrap();
        request_value["bearer_token"] = json!("forbidden");
        assert_eq!(
            parse_poll_request(&serde_json::to_vec(&request_value).unwrap()).unwrap_err(),
            BootstrapDeliveryError::ParseRejected
        );

        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let exact_request = request(MESSAGE_TWO);
        let response = coordinator
            .handle_poll(&exact_request, &transport())
            .unwrap();
        let mut response_value: Value = serde_json::from_slice(&response).unwrap();
        response_value["url"] = json!("https://example.invalid");
        assert_eq!(
            parse_and_verify_poll_response(
                &serde_json::to_vec(&response_value).unwrap(),
                &exact_request,
                &binding(),
                &TestCrypto,
            )
            .unwrap_err(),
            BootstrapDeliveryError::ParseRejected
        );
    }

    #[test]
    fn request_response_and_retry_limits_are_enforced() {
        assert_eq!(
            parse_poll_request(&vec![b' '; MAX_BOOTSTRAP_POLL_REQUEST_BYTES + 1]).unwrap_err(),
            BootstrapDeliveryError::PayloadTooLarge
        );
        assert_eq!(
            parse_and_verify_poll_response(
                &vec![b' '; MAX_BOOTSTRAP_POLL_RESPONSE_BYTES + 1],
                &request(MESSAGE_ONE),
                &binding(),
                &TestCrypto,
            )
            .unwrap_err(),
            BootstrapDeliveryError::PayloadTooLarge
        );
        let store = TestStore::pending();
        store.snapshot.lock().unwrap().resolution = BootstrapDeliveryResolution::Pending {
            retry_after_ms: MAX_BOOTSTRAP_RETRY_AFTER_MS + 1,
        };
        assert_eq!(
            BootstrapDeliveryCoordinator::new(store, TestCrypto)
                .handle_poll(&request(MESSAGE_ONE), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::InvalidField("retry_after_ms")
        );
    }

    #[test]
    fn ready_payload_digest_tampering_is_rejected_even_if_outer_shape_is_valid() {
        let store = TestStore::pending();
        store.snapshot.lock().unwrap().resolution = BootstrapDeliveryResolution::Ready {
            exact_bootstrap_envelope: envelope_bytes(),
        };
        let exact_request = request(MESSAGE_ONE);
        let response = BootstrapDeliveryCoordinator::new(store, TestCrypto)
            .handle_poll(&exact_request, &transport())
            .unwrap();
        let mut value: Value = serde_json::from_slice(&response).unwrap();
        value["disposition"]["exact_bootstrap_envelope_sha256"][0] = json!(99);
        assert_eq!(
            parse_and_verify_poll_response(
                &serde_json::to_vec(&value).unwrap(),
                &exact_request,
                &binding(),
                &TestCrypto,
            )
            .unwrap_err(),
            BootstrapDeliveryError::BindingMismatch("exact bootstrap envelope digest")
        );
    }

    #[test]
    fn response_cannot_be_rebound_to_another_request_or_receipt() {
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let first = request(MESSAGE_ONE);
        let response = coordinator.handle_poll(&first, &transport()).unwrap();
        let second = request(MESSAGE_TWO);
        assert_eq!(
            parse_and_verify_poll_response(&response, &second, &binding(), &TestCrypto)
                .unwrap_err(),
            BootstrapDeliveryError::BindingMismatch("poll request")
        );

        let mut other = binding();
        other.receipt_id = "018f0000-0000-7000-8000-000000000099".to_owned();
        assert_eq!(
            parse_and_verify_poll_response(&response, &first, &other, &TestCrypto).unwrap_err(),
            BootstrapDeliveryError::BindingMismatch("receipt transcript")
        );
    }

    #[test]
    fn invalid_p256_and_p1363_shapes_are_rejected() {
        let mut invalid = binding();
        invalid.iphone_signing_public_key = [0; 65];
        assert_eq!(
            sign_poll_request(&invalid, MESSAGE_ONE.to_owned(), &TestCrypto).unwrap_err(),
            BootstrapDeliveryError::InvalidField("p256_public_key")
        );
        let coordinator = BootstrapDeliveryCoordinator::new(TestStore::pending(), TestCrypto);
        let mut value: Value = serde_json::from_slice(&request(MESSAGE_ONE)).unwrap();
        value["proof_signature"] = json!([1, 2, 3]);
        assert_eq!(
            coordinator
                .handle_poll(&serde_json::to_vec(&value).unwrap(), &transport())
                .unwrap_err(),
            BootstrapDeliveryError::InvalidField("proof_signature")
        );
    }
}
