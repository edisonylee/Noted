//! Fixture-only iPhone side of the direct device-enrollment protocol.
//!
//! This module complements [`crate::pairing_protocol::PairingMachine`]. It
//! owns protocol state and public transcript material, while private signing,
//! HPKE, and library keys remain behind [`PairingClientCrypto`]. Production
//! and personal-data enrollment stay rejected until the native Apple adapter,
//! cross-language vectors, and external cryptographic review are complete.

use crate::pairing_protocol::{
    self, canonical_challenge_plaintext, canonical_client_finish_unsigned,
    canonical_client_hello_signed, canonical_client_hello_unsigned, canonical_invitation_signed,
    canonical_invitation_unsigned, canonical_server_finish_unsigned,
    canonical_server_hello_unsigned, derive_verification_code, invitation_nonce_proof,
    pairing_transcript_digest, parse_bounded_json, validate_bootstrap, validate_hpke_envelope,
    AuthenticatedHpkeEnvelope, BootstrapEnvelope, BootstrapMetadataV1, ClientFinish, ClientHello,
    EnrollmentReceipt, Environment, Invitation, KindCapability, LibraryDataClass, PairingError,
    PairingRole, RecordKind, ServerFinish, ServerHello, TransportEvidence,
    HPKE_EXPORTER_SECRET_BYTES, MAX_CLOCK_SKEW_MS, MAX_INVITATION_LIFETIME_MS,
    MAX_PAIRING_MESSAGE_BYTES, PAIRING_PROTOCOL, PAIRING_SUITE,
};
use crate::portable::is_uuid_v7;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use zeroize::Zeroizing;

const NONCE_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const P256_PUBLIC_KEY_BYTES: usize = 65;
const X25519_PUBLIC_KEY_BYTES: usize = 32;
const P1363_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingClientState {
    Ready,
    AwaitingServerHello,
    AwaitingUserConfirmation,
    AwaitingBootstrap,
    BootstrapPrepared,
    AwaitingServerFinish,
    PendingActivation,
    Active,
    CancellationPending,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFreshValuePurpose {
    ClientNonce,
    ClientHelloMessageId,
    ClientFinishMessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPublicIdentity {
    pub device_id: String,
    pub signing_public_key: Vec<u8>,
    pub hpke_public_key: Vec<u8>,
}

pub struct OpenedPairingChallenge {
    pub plaintext: Zeroizing<Vec<u8>>,
    pub exporter_secret: Zeroizing<[u8; HPKE_EXPORTER_SECRET_BYTES]>,
}

/// Native cryptography and key custody required by the iPhone pairing role.
///
/// Implementations expose public identity material but never private keys. A
/// bootstrap is decrypted directly into native pending storage and represented
/// in Rust only by an opaque associated type. Staging, activation, and discard
/// must be idempotent for the same receipt and envelope digest.
#[allow(clippy::result_unit_err)]
pub trait PairingClientCrypto: Send + Sync + 'static {
    type PendingKeyReference: Send;

    fn public_identity(&self) -> Result<ClientPublicIdentity, ()>;

    fn verify_signature(
        &self,
        signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()>;

    fn sign_device(&self, message: &[u8]) -> Result<Vec<u8>, ()>;

    fn open_challenge_authenticated(
        &self,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        envelope: &AuthenticatedHpkeEnvelope,
        exporter_context: &[u8],
    ) -> Result<OpenedPairingChallenge, ()>;

    fn stage_bootstrap_authenticated(
        &self,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        envelope: &AuthenticatedHpkeEnvelope,
        metadata: &BootstrapMetadataV1,
        receipt: &EnrollmentReceipt,
        envelope_digest: &[u8],
    ) -> Result<Self::PendingKeyReference, ()>;

    fn activate_pending_bootstrap(
        &self,
        pending: &Self::PendingKeyReference,
        receipt: &EnrollmentReceipt,
    ) -> Result<(), ()>;

    fn discard_pending_bootstrap(&self, pending: &Self::PendingKeyReference) -> Result<(), ()>;

    fn fresh_bytes(&self, purpose: ClientFreshValuePurpose, length: usize) -> Result<Vec<u8>, ()>;

    fn fresh_uuid_v7(&self, purpose: ClientFreshValuePurpose) -> Result<String, ()>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingClientConfig {
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub requested_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
    pub display_name: String,
    pub app_version: String,
    pub build_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingConfirmation {
    pub receipt_id: String,
    pub verification_code: String,
    pub granted_scopes: BTreeSet<RecordKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingActivation {
    pub receipt: EnrollmentReceipt,
    pub activated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingClientError {
    Protocol(PairingError),
    InvalidState {
        expected: &'static str,
        actual: PairingClientState,
    },
    ReplayMismatch(&'static str),
    CryptoUnavailable,
    KeyCustodyUnavailable,
    ActivationUnavailable,
}

impl From<PairingError> for PairingClientError {
    fn from(error: PairingError) -> Self {
        Self::Protocol(error)
    }
}

impl fmt::Display for PairingClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => error.fmt(formatter),
            Self::InvalidState { expected, actual } => {
                write!(
                    formatter,
                    "pairing client expected {expected}, found {actual:?}"
                )
            }
            Self::ReplayMismatch(message) => {
                write!(
                    formatter,
                    "byte-different pairing response replay: {message}"
                )
            }
            Self::CryptoUnavailable => {
                write!(formatter, "client pairing cryptography is unavailable")
            }
            Self::KeyCustodyUnavailable => {
                write!(formatter, "client pending-key custody is unavailable")
            }
            Self::ActivationUnavailable => {
                write!(formatter, "client enrollment activation is unavailable")
            }
        }
    }
}

impl std::error::Error for PairingClientError {}

pub struct PairingClient<C: PairingClientCrypto> {
    crypto: C,
    config: PairingClientConfig,
    invitation: Invitation,
    invitation_bytes: Vec<u8>,
    identity: ClientPublicIdentity,
    state: PairingClientState,
    client_hello: Option<ClientHello>,
    client_hello_bytes: Option<Vec<u8>>,
    server_hello: Option<ServerHello>,
    server_hello_bytes: Option<Vec<u8>>,
    confirmation: Option<PairingConfirmation>,
    user_decision: Option<bool>,
    bootstrap_bytes: Option<Vec<u8>>,
    client_finish_bytes: Option<Vec<u8>>,
    pending_key: Option<C::PendingKeyReference>,
    server_finish: Option<ServerFinish>,
    server_finish_bytes: Option<Vec<u8>>,
    activation: Option<PairingActivation>,
}

/// Secret-free durable representation of the iPhone pairing role. Every wire
/// message is retained byte-for-byte so restart recovery never regenerates a
/// message ID or randomized Secure Enclave signature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingClientCheckpoint {
    pub version: u32,
    pub config: PairingClientConfig,
    pub state: PairingClientState,
    pub invitation_bytes: Vec<u8>,
    pub identity: ClientPublicIdentity,
    pub client_hello_bytes: Option<Vec<u8>>,
    pub server_hello_bytes: Option<Vec<u8>>,
    pub confirmation: Option<PairingConfirmation>,
    pub user_decision: Option<bool>,
    pub bootstrap_bytes: Option<Vec<u8>>,
    pub client_finish_bytes: Option<Vec<u8>>,
    pub server_finish_bytes: Option<Vec<u8>>,
    pub activation: Option<PairingActivation>,
}

impl<C: PairingClientCrypto> PairingClient<C> {
    pub fn new_fixture_only(
        crypto: C,
        config: PairingClientConfig,
        invitation_bytes: &[u8],
        content_encoding: Option<&str>,
        now_ms: i64,
    ) -> Result<Self, PairingClientError> {
        validate_config(&config)?;
        let invitation: Invitation = parse_bounded_json(invitation_bytes, content_encoding)?;
        validate_invitation(&invitation, &config, now_ms)?;
        crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &invitation.authority_signing_public_key,
                &canonical_invitation_unsigned(&invitation),
                &invitation.authority_signature,
            )
            .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;

        let identity = crypto
            .public_identity()
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        validate_identity(&identity)?;

        Ok(Self {
            crypto,
            config,
            invitation,
            invitation_bytes: invitation_bytes.to_vec(),
            identity,
            state: PairingClientState::Ready,
            client_hello: None,
            client_hello_bytes: None,
            server_hello: None,
            server_hello_bytes: None,
            confirmation: None,
            user_decision: None,
            bootstrap_bytes: None,
            client_finish_bytes: None,
            pending_key: None,
            server_finish: None,
            server_finish_bytes: None,
            activation: None,
        })
    }

    pub fn state(&self) -> PairingClientState {
        self.state
    }

    pub fn invitation(&self) -> &Invitation {
        &self.invitation
    }

    pub fn confirmation(&self) -> Option<&PairingConfirmation> {
        self.confirmation.as_ref()
    }

    pub fn activation(&self) -> Option<&PairingActivation> {
        self.activation.as_ref()
    }

    pub fn identity(&self) -> &ClientPublicIdentity {
        &self.identity
    }

    pub fn checkpoint(&self) -> PairingClientCheckpoint {
        PairingClientCheckpoint {
            version: 1,
            config: self.config.clone(),
            state: self.state,
            invitation_bytes: self.invitation_bytes.clone(),
            identity: self.identity.clone(),
            client_hello_bytes: self.client_hello_bytes.clone(),
            server_hello_bytes: self.server_hello_bytes.clone(),
            confirmation: self.confirmation.clone(),
            user_decision: self.user_decision,
            bootstrap_bytes: self.bootstrap_bytes.clone(),
            client_finish_bytes: self.client_finish_bytes.clone(),
            server_finish_bytes: self.server_finish_bytes.clone(),
            activation: self.activation.clone(),
        }
    }

    pub fn restore_fixture_only(
        crypto: C,
        checkpoint: PairingClientCheckpoint,
        pending_key: Option<C::PendingKeyReference>,
        now_ms: i64,
    ) -> Result<Self, PairingClientError> {
        if checkpoint.version != 1 {
            return Err(PairingError::StateUnavailable.into());
        }
        validate_config(&checkpoint.config)?;
        ensure_checkpoint_wire_bounds(&checkpoint)?;
        let invitation: Invitation = parse_bounded_json(&checkpoint.invitation_bytes, None)?;
        // A durable transcript may be resumed after its invitation expires
        // only for cleanup or a previously verified activation transition.
        let validation_time = now_ms.min(invitation.expires_at_ms);
        validate_invitation(&invitation, &checkpoint.config, validation_time)?;
        crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &invitation.authority_signing_public_key,
                &canonical_invitation_unsigned(&invitation),
                &invitation.authority_signature,
            )
            .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;
        validate_identity(&checkpoint.identity)?;
        let native_identity = crypto
            .public_identity()
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if native_identity != checkpoint.identity {
            return Err(PairingError::BindingMismatch("native identity").into());
        }

        let client_hello = checkpoint
            .client_hello_bytes
            .as_deref()
            .map(|bytes| parse_bounded_json::<ClientHello>(bytes, None))
            .transpose()?;
        if let Some(hello) = &client_hello {
            validate_checkpoint_client_hello(
                &crypto,
                hello,
                &invitation,
                &checkpoint.config,
                &checkpoint.identity,
            )?;
        }
        let server_hello = checkpoint
            .server_hello_bytes
            .as_deref()
            .map(|bytes| parse_server_message::<ServerHello>(bytes, None))
            .transpose()?;
        let server_finish = checkpoint
            .server_finish_bytes
            .as_deref()
            .map(|bytes| parse_server_message::<ServerFinish>(bytes, None))
            .transpose()?;

        let mut client = Self {
            crypto,
            config: checkpoint.config,
            invitation,
            invitation_bytes: checkpoint.invitation_bytes,
            identity: checkpoint.identity,
            state: checkpoint.state,
            client_hello,
            client_hello_bytes: checkpoint.client_hello_bytes,
            server_hello,
            server_hello_bytes: checkpoint.server_hello_bytes,
            confirmation: checkpoint.confirmation,
            user_decision: checkpoint.user_decision,
            bootstrap_bytes: checkpoint.bootstrap_bytes,
            client_finish_bytes: checkpoint.client_finish_bytes,
            pending_key,
            server_finish,
            server_finish_bytes: checkpoint.server_finish_bytes,
            activation: checkpoint.activation,
        };
        client.validate_restored_checkpoint(now_ms)?;
        Ok(client)
    }

    pub fn expected_bootstrap_binding(&self) -> Option<(String, Vec<u8>)> {
        let receipt = &self.server_hello.as_ref()?.receipt;
        let bootstrap: BootstrapEnvelope =
            parse_server_message(self.bootstrap_bytes.as_deref()?, None).ok()?;
        Some((receipt.receipt_id.clone(), bootstrap.envelope_digest))
    }

    pub fn expected_bootstrap_metadata(&self) -> Option<BootstrapMetadataV1> {
        let bootstrap: BootstrapEnvelope =
            parse_server_message(self.bootstrap_bytes.as_deref()?, None).ok()?;
        Some(bootstrap.metadata)
    }

    pub fn pending_key_reference(&self) -> Option<&C::PendingKeyReference> {
        self.pending_key.as_ref()
    }

    fn validate_restored_checkpoint(&mut self, now_ms: i64) -> Result<(), PairingClientError> {
        if let Some(server) = self.server_hello.clone() {
            self.validate_server_hello(&server, now_ms.min(server.receipt.expires_at_ms))?;
            self.crypto
                .verify_signature(
                    PairingRole::MacAuthority,
                    &self.invitation.mac_pairing_signing_public_key,
                    &canonical_server_hello_unsigned(&server),
                    &server.proof_signature,
                )
                .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;
            let opened = self
                .crypto
                .open_challenge_authenticated(
                    &self.invitation.mac_pairing_hpke_public_key,
                    &pairing_protocol::challenge_hpke_info(&server.receipt),
                    &server.receipt.transcript_digest,
                    &server.challenge,
                    &pairing_protocol::challenge_hpke_exporter_context(&server.receipt),
                )
                .map_err(|_| PairingClientError::CryptoUnavailable)?;
            if opened.plaintext.as_slice() != canonical_challenge_plaintext(&server.receipt) {
                return Err(PairingError::BindingMismatch("challenge plaintext").into());
            }
            let expected = PairingConfirmation {
                receipt_id: server.receipt.receipt_id.clone(),
                verification_code: derive_verification_code(
                    opened.exporter_secret.as_ref(),
                    &server.receipt.transcript_digest,
                ),
                granted_scopes: server.receipt.granted_scopes.clone(),
            };
            if self.confirmation.as_ref() != Some(&expected) {
                return Err(PairingError::BindingMismatch("pairing confirmation").into());
            }
        }
        if let Some(bootstrap_bytes) = self.bootstrap_bytes.as_deref() {
            let bootstrap: BootstrapEnvelope = parse_server_message(bootstrap_bytes, None)?;
            let receipt = &self
                .server_hello
                .as_ref()
                .ok_or(PairingError::StateUnavailable)?
                .receipt;
            validate_bootstrap(&bootstrap, receipt)?;
            let finish: ClientFinish = parse_bounded_json(
                self.client_finish_bytes
                    .as_deref()
                    .ok_or(PairingError::StateUnavailable)?,
                None,
            )?;
            validate_checkpoint_client_finish(&finish, receipt, &bootstrap)?;
            self.crypto
                .verify_signature(
                    PairingRole::IphoneCompanion,
                    &self.identity.signing_public_key,
                    &canonical_client_finish_unsigned(&finish),
                    &finish.proof_signature,
                )
                .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;
        }
        if let Some(finish) = &self.server_finish {
            let receipt = &self
                .server_hello
                .as_ref()
                .ok_or(PairingError::StateUnavailable)?
                .receipt;
            validate_server_finish(finish, receipt, now_ms.max(finish.activated_at_ms))?;
            self.crypto
                .verify_signature(
                    PairingRole::MacAuthority,
                    &self.invitation.authority_signing_public_key,
                    &canonical_server_finish_unsigned(finish),
                    &finish.signature,
                )
                .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;
        }
        validate_checkpoint_state_shape(self)
    }

    pub fn create_client_hello(
        &mut self,
        transport: &TransportEvidence,
    ) -> Result<Vec<u8>, PairingClientError> {
        self.require_state(PairingClientState::Ready, "a validated invitation")?;
        validate_transport(transport, &self.invitation.tls_spki_sha256)?;

        let client_nonce = self
            .crypto
            .fresh_bytes(ClientFreshValuePurpose::ClientNonce, NONCE_BYTES)
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if client_nonce.len() != NONCE_BYTES {
            return Err(PairingClientError::CryptoUnavailable);
        }
        let message_id = self
            .crypto
            .fresh_uuid_v7(ClientFreshValuePurpose::ClientHelloMessageId)
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if !is_uuid_v7(&message_id) {
            return Err(PairingClientError::CryptoUnavailable);
        }

        let mut hello = ClientHello {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            message_id,
            invitation_id: self.invitation.invitation_id.clone(),
            nonce_proof: invitation_nonce_proof(&self.invitation.invitation_nonce),
            client_nonce,
            proposed_device_id: self.identity.device_id.clone(),
            display_name: self.config.display_name.clone(),
            client_signing_public_key: self.identity.signing_public_key.clone(),
            client_hpke_public_key: self.identity.hpke_public_key.clone(),
            requested_scopes: self.config.requested_scopes.clone(),
            capabilities: self.config.capabilities.clone(),
            app_version: self.config.app_version.clone(),
            build_version: self.config.build_version.clone(),
            library_id: self.invitation.library_id.clone(),
            authority_generation: self.invitation.authority_generation,
            environment: self.invitation.environment,
            sender_role: PairingRole::IphoneCompanion,
            recipient_role: PairingRole::MacAuthority,
            observed_tls_spki_sha256: transport.peer_spki_sha256.clone(),
            proof_signature: Vec::new(),
        };
        hello.proof_signature = self
            .crypto
            .sign_device(&canonical_client_hello_unsigned(&hello))
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if hello.proof_signature.len() != P1363_SIGNATURE_BYTES {
            return Err(PairingClientError::CryptoUnavailable);
        }
        let bytes = encode_message(&hello)?;
        self.client_hello = Some(hello);
        self.client_hello_bytes = Some(bytes.clone());
        self.state = PairingClientState::AwaitingServerHello;
        Ok(bytes)
    }

    pub fn retry_client_hello(&self) -> Result<Vec<u8>, PairingClientError> {
        self.require_state(
            PairingClientState::AwaitingServerHello,
            "an unanswered ClientHello",
        )?;
        self.client_hello_bytes
            .clone()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))
    }

    pub fn process_server_hello(
        &mut self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<PairingConfirmation, PairingClientError> {
        validate_transport(transport, &self.invitation.tls_spki_sha256)?;
        if let Some(accepted) = &self.server_hello_bytes {
            if accepted != bytes {
                return Err(PairingClientError::ReplayMismatch("ServerHello"));
            }
            return self
                .confirmation
                .clone()
                .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable));
        }
        self.require_state(
            PairingClientState::AwaitingServerHello,
            "a sent ClientHello",
        )?;
        ensure_not_expired(now_ms, self.invitation.expires_at_ms)?;

        let server: ServerHello = parse_server_message(bytes, content_encoding)?;
        self.validate_server_hello(&server, now_ms)?;
        self.crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &self.invitation.mac_pairing_signing_public_key,
                &canonical_server_hello_unsigned(&server),
                &server.proof_signature,
            )
            .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;

        let opened = self
            .crypto
            .open_challenge_authenticated(
                &self.invitation.mac_pairing_hpke_public_key,
                &pairing_protocol::challenge_hpke_info(&server.receipt),
                &server.receipt.transcript_digest,
                &server.challenge,
                &pairing_protocol::challenge_hpke_exporter_context(&server.receipt),
            )
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if opened.plaintext.as_slice() != canonical_challenge_plaintext(&server.receipt) {
            return Err(PairingClientError::Protocol(PairingError::BindingMismatch(
                "challenge plaintext",
            )));
        }
        let confirmation = PairingConfirmation {
            receipt_id: server.receipt.receipt_id.clone(),
            verification_code: derive_verification_code(
                opened.exporter_secret.as_ref(),
                &server.receipt.transcript_digest,
            ),
            granted_scopes: server.receipt.granted_scopes.clone(),
        };

        self.server_hello = Some(server);
        self.server_hello_bytes = Some(bytes.to_vec());
        self.confirmation = Some(confirmation.clone());
        self.state = PairingClientState::AwaitingUserConfirmation;
        Ok(confirmation)
    }

    pub fn confirm_on_device(
        &mut self,
        displayed_verification_code: &str,
        displayed_scopes: &BTreeSet<RecordKind>,
        approved: bool,
    ) -> Result<(), PairingClientError> {
        if self.state == PairingClientState::AwaitingBootstrap && approved {
            let confirmation = self
                .confirmation
                .as_ref()
                .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
            if displayed_verification_code == confirmation.verification_code
                && displayed_scopes == &confirmation.granted_scopes
            {
                return Ok(());
            }
        }
        self.require_state(
            PairingClientState::AwaitingUserConfirmation,
            "the local pairing confirmation",
        )?;
        let confirmation = self
            .confirmation
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        if !approved {
            self.user_decision = Some(false);
            self.state = PairingClientState::CancellationPending;
            return Err(PairingClientError::Protocol(
                PairingError::EnrollmentCancelled,
            ));
        }
        if displayed_verification_code != confirmation.verification_code
            || displayed_scopes != &confirmation.granted_scopes
        {
            self.user_decision = Some(false);
            self.state = PairingClientState::CancellationPending;
            return Err(PairingClientError::Protocol(
                PairingError::VerificationMismatch,
            ));
        }
        self.user_decision = Some(true);
        self.state = PairingClientState::AwaitingBootstrap;
        Ok(())
    }

    pub fn process_bootstrap(
        &mut self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<Vec<u8>, PairingClientError> {
        let finish = self.prepare_bootstrap(bytes, content_encoding, transport, now_ms)?;
        self.stage_prepared_bootstrap()?;
        Ok(finish)
    }

    /// Validates and signs ClientFinish without touching native bootstrap
    /// custody. The runtime checkpoints these bytes before the native stage.
    pub fn prepare_bootstrap(
        &mut self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<Vec<u8>, PairingClientError> {
        validate_transport(transport, &self.invitation.tls_spki_sha256)?;
        if let Some(accepted) = &self.bootstrap_bytes {
            if accepted != bytes {
                return Err(PairingClientError::ReplayMismatch("BootstrapEnvelope"));
            }
            return self
                .client_finish_bytes
                .clone()
                .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable));
        }
        self.require_state(
            PairingClientState::AwaitingBootstrap,
            "matching user confirmation",
        )?;
        let server = self
            .server_hello
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        ensure_not_expired(now_ms, server.receipt.expires_at_ms)?;

        let bootstrap: BootstrapEnvelope = parse_server_message(bytes, content_encoding)?;
        validate_bootstrap(&bootstrap, &server.receipt)?;

        let message_id = self
            .crypto
            .fresh_uuid_v7(ClientFreshValuePurpose::ClientFinishMessageId)
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if !is_uuid_v7(&message_id) {
            return Err(PairingClientError::CryptoUnavailable);
        }
        let mut finish = ClientFinish {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            message_id,
            receipt_id: server.receipt.receipt_id.clone(),
            invitation_id: server.receipt.invitation_id.clone(),
            library_id: server.receipt.library_id.clone(),
            device_id: server.receipt.device_id.clone(),
            authority_generation: server.receipt.authority_generation,
            environment: server.receipt.environment,
            sender_role: PairingRole::IphoneCompanion,
            recipient_role: PairingRole::MacAuthority,
            transcript_digest: server.receipt.transcript_digest.clone(),
            bootstrap_envelope_digest: bootstrap.envelope_digest.clone(),
            proof_signature: Vec::new(),
        };
        finish.proof_signature = self
            .crypto
            .sign_device(&canonical_client_finish_unsigned(&finish))
            .map_err(|_| PairingClientError::CryptoUnavailable)?;
        if finish.proof_signature.len() != P1363_SIGNATURE_BYTES {
            return Err(PairingClientError::CryptoUnavailable);
        }
        let finish_bytes = encode_message(&finish)?;

        self.bootstrap_bytes = Some(bytes.to_vec());
        self.client_finish_bytes = Some(finish_bytes.clone());
        self.state = PairingClientState::BootstrapPrepared;
        Ok(finish_bytes)
    }

    /// Stages the already-checkpointed envelope into native custody. Repeating
    /// this call is safe because the native layer binds its opaque handle to
    /// the receipt and envelope digest.
    pub fn stage_prepared_bootstrap(&mut self) -> Result<Vec<u8>, PairingClientError> {
        if self.state == PairingClientState::AwaitingServerFinish {
            return self.retry_client_finish();
        }
        self.require_state(
            PairingClientState::BootstrapPrepared,
            "a durably checkpointed BootstrapEnvelope and ClientFinish",
        )?;
        let server = self
            .server_hello
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        let bootstrap: BootstrapEnvelope = parse_server_message(
            self.bootstrap_bytes
                .as_deref()
                .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?,
            None,
        )?;
        let pending = self
            .crypto
            .stage_bootstrap_authenticated(
                &self.invitation.mac_pairing_hpke_public_key,
                &pairing_protocol::bootstrap_hpke_info(&bootstrap.metadata),
                &pairing_protocol::bootstrap_associated_data(&bootstrap.metadata),
                &bootstrap.sealed_key_package,
                &bootstrap.metadata,
                &server.receipt,
                &bootstrap.envelope_digest,
            )
            .map_err(|_| PairingClientError::KeyCustodyUnavailable)?;

        self.pending_key = Some(pending);
        self.state = PairingClientState::AwaitingServerFinish;
        self.retry_client_finish()
    }

    /// Used only after the runtime matches native public recovery metadata to
    /// the prepared receipt and envelope digest.
    pub fn resume_staged_bootstrap(
        &mut self,
        pending: C::PendingKeyReference,
    ) -> Result<Vec<u8>, PairingClientError> {
        self.require_state(
            PairingClientState::BootstrapPrepared,
            "a prepared bootstrap awaiting native reconciliation",
        )?;
        self.pending_key = Some(pending);
        self.state = PairingClientState::AwaitingServerFinish;
        self.retry_client_finish()
    }

    pub fn retry_client_finish(&self) -> Result<Vec<u8>, PairingClientError> {
        self.require_state(
            PairingClientState::AwaitingServerFinish,
            "an unanswered ClientFinish",
        )?;
        self.client_finish_bytes
            .clone()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))
    }

    pub fn process_server_finish(
        &mut self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<PairingActivation, PairingClientError> {
        self.prepare_server_finish(bytes, content_encoding, transport, now_ms)?;
        self.retry_activation()
    }

    /// Verifies ServerFinish without activating the Keychain bootstrap. The
    /// runtime persists this state first and then performs the idempotent
    /// native transition.
    pub fn prepare_server_finish(
        &mut self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<(), PairingClientError> {
        validate_transport(transport, &self.invitation.tls_spki_sha256)?;
        if let Some(accepted) = &self.server_finish_bytes {
            if accepted != bytes {
                return Err(PairingClientError::ReplayMismatch("ServerFinish"));
            }
            return match self.state {
                PairingClientState::PendingActivation | PairingClientState::Active => Ok(()),
                actual => Err(PairingClientError::InvalidState {
                    expected: "pending or completed activation",
                    actual,
                }),
            };
        }
        self.require_state(
            PairingClientState::AwaitingServerFinish,
            "a sent ClientFinish",
        )?;
        let expected = self
            .server_hello
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        ensure_not_expired(now_ms, expected.receipt.expires_at_ms)?;

        let finish: ServerFinish = parse_server_message(bytes, content_encoding)?;
        validate_server_finish(&finish, &expected.receipt, now_ms)?;
        self.crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &self.invitation.authority_signing_public_key,
                &canonical_server_finish_unsigned(&finish),
                &finish.signature,
            )
            .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))?;

        self.server_finish = Some(finish);
        self.server_finish_bytes = Some(bytes.to_vec());
        self.state = PairingClientState::PendingActivation;
        Ok(())
    }

    pub fn retry_activation(&mut self) -> Result<PairingActivation, PairingClientError> {
        if self.state == PairingClientState::Active {
            return self
                .activation
                .clone()
                .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable));
        }
        self.require_state(
            PairingClientState::PendingActivation,
            "a verified ServerFinish awaiting native activation",
        )?;
        let pending = self
            .pending_key
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        let finish = self
            .server_finish
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        self.crypto
            .activate_pending_bootstrap(pending, &finish.receipt)
            .map_err(|_| PairingClientError::ActivationUnavailable)?;

        let activation = PairingActivation {
            receipt: finish.receipt.clone(),
            activated_at_ms: finish.activated_at_ms,
        };
        self.pending_key.take();
        self.activation = Some(activation.clone());
        self.state = PairingClientState::Active;
        Ok(activation)
    }

    /// Cancels only before a verified ServerFinish. Once the Mac has activated
    /// the device, a separate authenticated revocation flow is required.
    pub fn cancel(&mut self) -> Result<(), PairingClientError> {
        match self.state {
            PairingClientState::Active | PairingClientState::PendingActivation => {
                return Err(PairingClientError::InvalidState {
                    expected: "pairing before server activation",
                    actual: self.state,
                })
            }
            PairingClientState::Cancelled => return Ok(()),
            PairingClientState::CancellationPending => return self.retry_cancellation(),
            _ => {}
        }
        self.state = PairingClientState::CancellationPending;
        self.finish_cancellation()
    }

    pub fn retry_cancellation(&mut self) -> Result<(), PairingClientError> {
        self.require_state(
            PairingClientState::CancellationPending,
            "a pending native key discard",
        )?;
        self.finish_cancellation()
    }

    fn finish_cancellation(&mut self) -> Result<(), PairingClientError> {
        if let Some(pending) = self.pending_key.as_ref() {
            self.crypto
                .discard_pending_bootstrap(pending)
                .map_err(|_| PairingClientError::KeyCustodyUnavailable)?;
        }
        self.pending_key.take();
        self.state = PairingClientState::Cancelled;
        Ok(())
    }

    fn validate_server_hello(
        &self,
        server: &ServerHello,
        now_ms: i64,
    ) -> Result<(), PairingClientError> {
        if server.protocol != PAIRING_PROTOCOL {
            return Err(PairingError::UnsupportedProtocol.into());
        }
        if server.suite != PAIRING_SUITE {
            return Err(PairingError::DowngradeRejected.into());
        }
        if server.sender_role != PairingRole::MacAuthority
            || server.recipient_role != PairingRole::IphoneCompanion
        {
            return Err(PairingError::BindingMismatch("roles").into());
        }
        exact_len(&server.server_nonce, NONCE_BYTES, "server_nonce")?;
        exact_len(
            &server.proof_signature,
            P1363_SIGNATURE_BYTES,
            "proof_signature",
        )?;
        validate_hpke_envelope(&server.challenge)?;

        let hello = self
            .client_hello
            .as_ref()
            .ok_or(PairingClientError::Protocol(PairingError::StateUnavailable))?;
        let receipt = &server.receipt;
        validate_receipt_shape(receipt)?;
        ensure_not_expired(now_ms, receipt.expires_at_ms)?;
        if receipt.protocol != PAIRING_PROTOCOL || receipt.suite != PAIRING_SUITE {
            return Err(PairingError::DowngradeRejected.into());
        }
        if receipt.receipt_id.is_empty()
            || receipt.invitation_id != self.invitation.invitation_id
            || receipt.library_id != self.invitation.library_id
            || receipt.device_id != self.identity.device_id
            || receipt.authority_generation != self.invitation.authority_generation
            || receipt.environment != self.invitation.environment
            || receipt.mac_role != PairingRole::MacAuthority
            || receipt.client_role != PairingRole::IphoneCompanion
            || receipt.expires_at_ms != self.invitation.expires_at_ms
        {
            return Err(PairingError::BindingMismatch("receipt").into());
        }
        if receipt.created_at_ms < self.invitation.created_at_ms
            || receipt.created_at_ms > receipt.expires_at_ms
            || receipt.created_at_ms > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
        {
            return Err(PairingError::InvitationExpired.into());
        }
        if receipt.client_signing_key_fingerprint != sha256(&self.identity.signing_public_key)
            || receipt.client_hpke_key_fingerprint != sha256(&self.identity.hpke_public_key)
            || receipt.mac_signing_key_fingerprint
                != sha256(&self.invitation.mac_pairing_signing_public_key)
            || receipt.mac_hpke_key_fingerprint
                != sha256(&self.invitation.mac_pairing_hpke_public_key)
        {
            return Err(PairingError::BindingMismatch("key fingerprints").into());
        }
        validate_selected_capabilities(
            &receipt.granted_scopes,
            &receipt.capabilities,
            &hello.requested_scopes,
            &hello.capabilities,
        )?;

        let mut receipt_proposal = receipt.clone();
        receipt_proposal.transcript_digest.clear();
        let expected_transcript = pairing_transcript_digest(
            &sha256(&canonical_invitation_signed(&self.invitation)),
            &sha256(&canonical_client_hello_signed(hello)),
            &server.server_nonce,
            &receipt_proposal,
        );
        if receipt.transcript_digest != expected_transcript {
            return Err(PairingError::BindingMismatch("transcript digest").into());
        }
        Ok(())
    }

    fn require_state(
        &self,
        required: PairingClientState,
        expected: &'static str,
    ) -> Result<(), PairingClientError> {
        if self.state == required {
            Ok(())
        } else {
            Err(PairingClientError::InvalidState {
                expected,
                actual: self.state,
            })
        }
    }
}

fn ensure_checkpoint_wire_bounds(
    checkpoint: &PairingClientCheckpoint,
) -> Result<(), PairingClientError> {
    if checkpoint.invitation_bytes.is_empty()
        || [
            Some(checkpoint.invitation_bytes.as_slice()),
            checkpoint.client_hello_bytes.as_deref(),
            checkpoint.server_hello_bytes.as_deref(),
            checkpoint.bootstrap_bytes.as_deref(),
            checkpoint.client_finish_bytes.as_deref(),
            checkpoint.server_finish_bytes.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|bytes| bytes.len() > MAX_PAIRING_MESSAGE_BYTES)
    {
        return Err(PairingError::PayloadTooLarge.into());
    }
    Ok(())
}

fn validate_checkpoint_client_hello<C: PairingClientCrypto>(
    crypto: &C,
    hello: &ClientHello,
    invitation: &Invitation,
    config: &PairingClientConfig,
    identity: &ClientPublicIdentity,
) -> Result<(), PairingClientError> {
    if hello.protocol != PAIRING_PROTOCOL
        || hello.suite != PAIRING_SUITE
        || !is_uuid_v7(&hello.message_id)
        || hello.invitation_id != invitation.invitation_id
        || hello.library_id != invitation.library_id
        || hello.authority_generation != invitation.authority_generation
        || hello.environment != invitation.environment
        || hello.sender_role != PairingRole::IphoneCompanion
        || hello.recipient_role != PairingRole::MacAuthority
        || hello.nonce_proof != invitation_nonce_proof(&invitation.invitation_nonce)
        || hello.proposed_device_id != identity.device_id
        || hello.client_signing_public_key != identity.signing_public_key
        || hello.client_hpke_public_key != identity.hpke_public_key
        || hello.requested_scopes != config.requested_scopes
        || hello.capabilities != config.capabilities
        || hello.display_name != config.display_name
        || hello.app_version != config.app_version
        || hello.build_version != config.build_version
        || hello.observed_tls_spki_sha256 != invitation.tls_spki_sha256
    {
        return Err(PairingError::BindingMismatch("restored ClientHello").into());
    }
    exact_len(&hello.client_nonce, NONCE_BYTES, "client_nonce")?;
    exact_len(
        &hello.proof_signature,
        P1363_SIGNATURE_BYTES,
        "proof_signature",
    )?;
    crypto
        .verify_signature(
            PairingRole::IphoneCompanion,
            &identity.signing_public_key,
            &canonical_client_hello_unsigned(hello),
            &hello.proof_signature,
        )
        .map_err(|_| PairingClientError::Protocol(PairingError::InvalidSignature))
}

fn validate_checkpoint_client_finish(
    finish: &ClientFinish,
    receipt: &EnrollmentReceipt,
    bootstrap: &BootstrapEnvelope,
) -> Result<(), PairingClientError> {
    if finish.protocol != PAIRING_PROTOCOL
        || finish.suite != PAIRING_SUITE
        || !is_uuid_v7(&finish.message_id)
        || finish.receipt_id != receipt.receipt_id
        || finish.invitation_id != receipt.invitation_id
        || finish.library_id != receipt.library_id
        || finish.device_id != receipt.device_id
        || finish.authority_generation != receipt.authority_generation
        || finish.environment != receipt.environment
        || finish.sender_role != PairingRole::IphoneCompanion
        || finish.recipient_role != PairingRole::MacAuthority
        || finish.transcript_digest != receipt.transcript_digest
        || finish.bootstrap_envelope_digest != bootstrap.envelope_digest
    {
        return Err(PairingError::BindingMismatch("restored ClientFinish").into());
    }
    exact_len(
        &finish.proof_signature,
        P1363_SIGNATURE_BYTES,
        "proof_signature",
    )
}

fn validate_checkpoint_state_shape<C: PairingClientCrypto>(
    client: &PairingClient<C>,
) -> Result<(), PairingClientError> {
    let hello = client.client_hello.is_some() && client.client_hello_bytes.is_some();
    let server = client.server_hello.is_some()
        && client.server_hello_bytes.is_some()
        && client.confirmation.is_some();
    let bootstrap = client.bootstrap_bytes.is_some() && client.client_finish_bytes.is_some();
    let finish = client.server_finish.is_some() && client.server_finish_bytes.is_some();
    let pending = client.pending_key.is_some();
    let active = client.activation.is_some();
    let valid = match client.state {
        PairingClientState::Ready => {
            !hello && !server && !bootstrap && !finish && !pending && !active
        }
        PairingClientState::AwaitingServerHello => {
            hello && !server && !bootstrap && !finish && !pending && !active
        }
        PairingClientState::AwaitingUserConfirmation => {
            hello
                && server
                && client.user_decision.is_none()
                && !bootstrap
                && !finish
                && !pending
                && !active
        }
        PairingClientState::AwaitingBootstrap => {
            hello
                && server
                && client.user_decision == Some(true)
                && !bootstrap
                && !finish
                && !pending
                && !active
        }
        PairingClientState::BootstrapPrepared => {
            hello
                && server
                && client.user_decision == Some(true)
                && bootstrap
                && !finish
                && !pending
                && !active
        }
        PairingClientState::AwaitingServerFinish => {
            hello
                && server
                && client.user_decision == Some(true)
                && bootstrap
                && !finish
                && pending
                && !active
        }
        PairingClientState::PendingActivation => {
            hello && server && bootstrap && finish && pending && !active
        }
        PairingClientState::Active => hello && server && bootstrap && finish && !pending && active,
        PairingClientState::CancellationPending => !active,
        PairingClientState::Cancelled => !pending && !active,
    };
    if !valid {
        return Err(PairingError::StateUnavailable.into());
    }
    if let (Some(activation), Some(finish)) = (&client.activation, &client.server_finish) {
        if activation.receipt != finish.receipt
            || activation.activated_at_ms != finish.activated_at_ms
        {
            return Err(PairingError::BindingMismatch("restored activation").into());
        }
    }
    Ok(())
}

fn validate_config(config: &PairingClientConfig) -> Result<(), PairingClientError> {
    if config.environment != Environment::Development
        || config.library_data_class != LibraryDataClass::SanitizedFixture
    {
        return Err(PairingError::FixtureOnly.into());
    }
    validate_text(&config.display_name, 80, "display_name")?;
    validate_text(&config.app_version, 64, "app_version")?;
    validate_text(&config.build_version, 64, "build_version")?;
    validate_capability_map(&config.requested_scopes, &config.capabilities)?;
    pairing_protocol::validate_fixture_scopes_and_capabilities(
        &config.requested_scopes,
        &config.capabilities,
    )?;
    Ok(())
}

fn validate_invitation(
    invitation: &Invitation,
    config: &PairingClientConfig,
    now_ms: i64,
) -> Result<(), PairingClientError> {
    if invitation.environment != Environment::Development
        || invitation.library_data_class != LibraryDataClass::SanitizedFixture
        || invitation.environment != config.environment
        || invitation.library_data_class != config.library_data_class
    {
        return Err(PairingError::FixtureOnly.into());
    }
    if invitation.protocol != PAIRING_PROTOCOL {
        return Err(PairingError::UnsupportedProtocol.into());
    }
    if invitation.suite != PAIRING_SUITE {
        return Err(PairingError::UnsupportedSuite.into());
    }
    if !is_uuid_v7(&invitation.invitation_id) || !is_uuid_v7(&invitation.library_id) {
        return Err(PairingError::InvalidIdentifier.into());
    }
    exact_len(
        &invitation.invitation_nonce,
        NONCE_BYTES,
        "invitation_nonce",
    )?;
    exact_len(
        &invitation.authority_signing_public_key,
        P256_PUBLIC_KEY_BYTES,
        "authority_signing_public_key",
    )?;
    exact_len(
        &invitation.mac_pairing_signing_public_key,
        P256_PUBLIC_KEY_BYTES,
        "mac_pairing_signing_public_key",
    )?;
    exact_len(
        &invitation.mac_pairing_hpke_public_key,
        X25519_PUBLIC_KEY_BYTES,
        "mac_pairing_hpke_public_key",
    )?;
    exact_len(&invitation.tls_spki_sha256, DIGEST_BYTES, "tls_spki_sha256")?;
    exact_len(
        &invitation.authority_signature,
        P1363_SIGNATURE_BYTES,
        "authority_signature",
    )?;
    if invitation.authority_generation == 0 {
        return Err(PairingError::InvalidField("authority_generation").into());
    }
    if invitation.authority_role != PairingRole::MacAuthority
        || invitation.intended_client_role != PairingRole::IphoneCompanion
    {
        return Err(PairingError::BindingMismatch("roles").into());
    }
    if invitation.scope_ceiling.is_empty()
        || config.requested_scopes.is_empty()
        || !config.requested_scopes.is_subset(&invitation.scope_ceiling)
    {
        return Err(PairingError::ScopeCeilingExceeded.into());
    }
    let lifetime = invitation
        .expires_at_ms
        .checked_sub(invitation.created_at_ms)
        .ok_or(PairingError::InvitationExpired)?;
    if lifetime <= 0
        || lifetime > MAX_INVITATION_LIFETIME_MS
        || invitation.created_at_ms > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
    {
        return Err(PairingError::InvitationExpired.into());
    }
    ensure_not_expired(now_ms, invitation.expires_at_ms)
}

fn validate_identity(identity: &ClientPublicIdentity) -> Result<(), PairingClientError> {
    if !is_uuid_v7(&identity.device_id) {
        return Err(PairingError::InvalidIdentifier.into());
    }
    exact_len(
        &identity.signing_public_key,
        P256_PUBLIC_KEY_BYTES,
        "client_signing_public_key",
    )?;
    exact_len(
        &identity.hpke_public_key,
        X25519_PUBLIC_KEY_BYTES,
        "client_hpke_public_key",
    )?;
    Ok(())
}

fn validate_receipt_shape(receipt: &EnrollmentReceipt) -> Result<(), PairingClientError> {
    if !is_uuid_v7(&receipt.receipt_id)
        || !is_uuid_v7(&receipt.invitation_id)
        || !is_uuid_v7(&receipt.library_id)
        || !is_uuid_v7(&receipt.device_id)
    {
        return Err(PairingError::InvalidIdentifier.into());
    }
    exact_len(
        &receipt.client_signing_key_fingerprint,
        DIGEST_BYTES,
        "client_signing_key_fingerprint",
    )?;
    exact_len(
        &receipt.client_hpke_key_fingerprint,
        DIGEST_BYTES,
        "client_hpke_key_fingerprint",
    )?;
    exact_len(
        &receipt.mac_signing_key_fingerprint,
        DIGEST_BYTES,
        "mac_signing_key_fingerprint",
    )?;
    exact_len(
        &receipt.mac_hpke_key_fingerprint,
        DIGEST_BYTES,
        "mac_hpke_key_fingerprint",
    )?;
    exact_len(
        &receipt.transcript_digest,
        DIGEST_BYTES,
        "transcript_digest",
    )?;
    if receipt.authority_generation == 0 {
        return Err(PairingError::InvalidField("authority_generation").into());
    }
    Ok(())
}

fn validate_selected_capabilities(
    granted_scopes: &BTreeSet<RecordKind>,
    selected: &BTreeMap<RecordKind, KindCapability>,
    requested_scopes: &BTreeSet<RecordKind>,
    supported: &BTreeMap<RecordKind, KindCapability>,
) -> Result<(), PairingClientError> {
    if granted_scopes.is_empty() || !granted_scopes.is_subset(requested_scopes) {
        return Err(PairingError::ScopeCeilingExceeded.into());
    }
    validate_capability_map(granted_scopes, selected)?;
    for scope in granted_scopes {
        let chosen = selected
            .get(scope)
            .ok_or(PairingError::CapabilityMismatch)?;
        let local = supported
            .get(scope)
            .ok_or(PairingError::CapabilityMismatch)?;
        if chosen.reader_version > local.reader_version
            || match (chosen.writer_version, local.writer_version) {
                (Some(chosen), Some(local)) => chosen > local,
                (Some(_), None) => true,
                _ => false,
            }
        {
            return Err(PairingError::CapabilityMismatch.into());
        }
    }
    Ok(())
}

fn validate_capability_map(
    scopes: &BTreeSet<RecordKind>,
    capabilities: &BTreeMap<RecordKind, KindCapability>,
) -> Result<(), PairingClientError> {
    if scopes.is_empty()
        || capabilities.len() != scopes.len()
        || capabilities.keys().any(|kind| !scopes.contains(kind))
        || scopes.iter().any(|kind| !capabilities.contains_key(kind))
    {
        return Err(PairingError::CapabilityMismatch.into());
    }
    if capabilities.values().any(|capability| {
        capability.reader_version == 0
            || capability.writer_version == Some(0)
            || capability
                .writer_version
                .is_some_and(|writer| writer > capability.reader_version)
    }) {
        return Err(PairingError::CapabilityMismatch.into());
    }
    Ok(())
}

fn validate_server_finish(
    finish: &ServerFinish,
    expected_receipt: &EnrollmentReceipt,
    now_ms: i64,
) -> Result<(), PairingClientError> {
    if finish.protocol != PAIRING_PROTOCOL {
        return Err(PairingError::UnsupportedProtocol.into());
    }
    if finish.suite != PAIRING_SUITE {
        return Err(PairingError::DowngradeRejected.into());
    }
    if finish.sender_role != PairingRole::MacAuthority
        || finish.recipient_role != PairingRole::IphoneCompanion
    {
        return Err(PairingError::BindingMismatch("roles").into());
    }
    if &finish.receipt != expected_receipt {
        return Err(PairingError::BindingMismatch("server finish receipt").into());
    }
    exact_len(
        &finish.signature,
        P1363_SIGNATURE_BYTES,
        "server_finish_signature",
    )?;
    if finish.activated_at_ms < finish.receipt.created_at_ms
        || finish.activated_at_ms > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
    {
        return Err(PairingError::BindingMismatch("activation time").into());
    }
    Ok(())
}

fn validate_transport(
    transport: &TransportEvidence,
    expected_spki_sha256: &[u8],
) -> Result<(), PairingClientError> {
    if transport.tls_version != "1.3" || transport.used_zero_rtt {
        return Err(PairingError::InsecureTransport.into());
    }
    if transport.peer_spki_sha256 != expected_spki_sha256 {
        return Err(PairingError::PinMismatch.into());
    }
    Ok(())
}

fn ensure_not_expired(now_ms: i64, expires_at_ms: i64) -> Result<(), PairingClientError> {
    if now_ms > expires_at_ms.saturating_add(MAX_CLOCK_SKEW_MS) {
        Err(PairingError::InvitationExpired.into())
    } else {
        Ok(())
    }
}

fn exact_len(value: &[u8], expected: usize, field: &'static str) -> Result<(), PairingClientError> {
    if value.len() == expected {
        Ok(())
    } else {
        Err(PairingError::InvalidField(field).into())
    }
}

fn validate_text(value: &str, max: usize, field: &'static str) -> Result<(), PairingClientError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(PairingError::InvalidField(field).into())
    } else {
        Ok(())
    }
}

fn encode_message<T: serde::Serialize>(message: &T) -> Result<Vec<u8>, PairingClientError> {
    let bytes = serde_json::to_vec(message)
        .map_err(|_| PairingClientError::Protocol(PairingError::StateUnavailable))?;
    if bytes.len() > MAX_PAIRING_MESSAGE_BYTES {
        Err(PairingError::PayloadTooLarge.into())
    } else {
        Ok(bytes)
    }
}

/// Server messages can contain an HPKE ciphertext up to the protocol's sealed
/// payload limit. Serde's JSON representation of `Vec<u8>` is an integer array,
/// so the generic 128-element JSON-array guard used for client messages would
/// reject a valid envelope. The typed structures deny unknown fields, the JSON
/// parser keeps its recursion limit, and the wire-size and encoding limits are
/// still enforced before exact field and envelope validation.
fn parse_server_message<T: DeserializeOwned>(
    bytes: &[u8],
    content_encoding: Option<&str>,
) -> Result<T, PairingClientError> {
    if !matches!(content_encoding, None | Some("identity")) {
        return Err(PairingError::UnsupportedEncoding.into());
    }
    if bytes.len() > MAX_PAIRING_MESSAGE_BYTES {
        return Err(PairingError::PayloadTooLarge.into());
    }
    serde_json::from_slice(bytes)
        .map_err(|error| PairingError::ParseRejected(error.to_string()).into())
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
