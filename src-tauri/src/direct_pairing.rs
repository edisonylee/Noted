//! Restart-safe, fixture-only Mac coordinator for direct iPhone enrollment.
//!
//! This module is intentionally not a listener or a general command router.
//! Its public surface is limited to the three typed pairing transitions plus
//! invitation registration and revocation.  Cryptography is supplied through
//! [`PairingCrypto`], trusted time through [`AuthorityClock`], and every wire
//! response is committed in the same SQLite transaction as its state change
//! before it can be returned to a transport adapter.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::{
    direct_authority_store::{
        ActivateEnrollment, ActivateOutcome, ConfirmEnrollment, ConfirmOutcome, ConsumeInvitation,
        ConsumeOutcome, DirectAuthorityStore, InvitationRegistration, NewInvitation, StoreError,
    },
    direct_pairing_delivery::{
        BootstrapDeliveryBinding, BootstrapDeliveryCoordinator, BootstrapDeliveryError,
        BootstrapDeliveryReplay, BootstrapDeliveryResolution, BootstrapDeliverySnapshot,
        BootstrapDeliveryStore, BootstrapDeliveryTerminal, BootstrapDeliveryTransport,
        BootstrapDeliveryVerifier, BootstrapReplayCommit, MacBootstrapDeliverySigner,
    },
    pairing_protocol::{
        bootstrap_associated_data, bootstrap_envelope_digest, bootstrap_hpke_exporter_context,
        bootstrap_hpke_info, canonical_challenge_plaintext, canonical_client_finish_signed,
        canonical_client_finish_unsigned, canonical_client_hello_signed,
        canonical_client_hello_unsigned, canonical_invitation_signed,
        canonical_invitation_unsigned, canonical_server_finish_unsigned,
        canonical_server_hello_unsigned, challenge_hpke_exporter_context, challenge_hpke_info,
        derive_verification_code, enrollment_confirmation_digest, fixture_bootstrap_metadata,
        invitation_nonce_proof, is_uuid_v7, negotiate_capabilities, pairing_transcript_digest,
        parse_bounded_json, validate_bootstrap, validate_bootstrap_key_package_envelope,
        validate_client_finish_shape, validate_client_hello_shape, validate_finish_bindings,
        validate_hpke_envelope, validate_invitation_shape, validate_policy,
        validate_requested_capabilities, validate_transport_evidence, AuthenticatedHpkeEnvelope,
        BootstrapEnvelope, ClientFinish, ClientHello, EnrollmentReceipt, Environment,
        FreshValuePurpose, KindCapability, LocalHpkeKey, LocalSigningKey, PairingCrypto,
        PairingError, PairingPolicy, PairingRole, RecordKind, ServerFinish, ServerHello,
        TransportEvidence, BOOTSTRAP_SYNC_PROTOCOL_VERSION, PAIRING_PROTOCOL, PAIRING_SUITE,
    },
};

const _: [(); crate::sync_protocol::SYNC_PROTOCOL_VERSION as usize] =
    [(); BOOTSTRAP_SYNC_PROTOCOL_VERSION as usize];

const P256_PUBLIC_KEY_BYTES: usize = 65;
const X25519_PUBLIC_KEY_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const NONCE_BYTES: usize = 32;
const P1363_SIGNATURE_BYTES: usize = 64;
const BOOTSTRAP_DELIVERY_REPLAY_RATE_WINDOW_MS: i64 = 5 * 60 * 1_000;
const MAX_BOOTSTRAP_DELIVERY_REPLAYS_PER_WINDOW: i64 = 128;

/// Trusted wall-clock seam. Implementations must read authority-local time;
/// request bodies never provide timestamps used for expiry or abuse windows.
pub trait AuthorityClock: Send + Sync + 'static {
    fn now_ms(&self) -> Result<i64, AuthorityClockError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityClockError;

impl fmt::Display for AuthorityClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("trusted authority clock is unavailable")
    }
}

impl std::error::Error for AuthorityClockError {}

/// Public-key and transport bindings owned by the Mac authority.
///
/// Private material remains behind [`PairingCrypto`]; this value contains only
/// the exact public identities that an invitation is permitted to advertise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityBindings {
    pub authority_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    pub mac_pairing_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    pub mac_pairing_hpke_public_key: [u8; X25519_PUBLIC_KEY_BYTES],
    pub tls_spki_sha256: [u8; DIGEST_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHelloResult {
    pub receipt_id: String,
    pub exact_response_bytes: Vec<u8>,
    pub verification_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerConfirmationResult {
    Bootstrap(Vec<u8>),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorError {
    Protocol(PairingError),
    Store(StoreError),
    Database(String),
    Serialization,
    ClockUnavailable,
    BootstrapDelivery(BootstrapDeliveryError),
    StateUnavailable(&'static str),
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "direct pairing protocol error: {error}"),
            Self::Store(error) => write!(formatter, "direct pairing store error: {error}"),
            Self::Database(error) => write!(formatter, "direct pairing database error: {error}"),
            Self::Serialization => formatter.write_str("direct pairing serialization failed"),
            Self::ClockUnavailable => formatter.write_str("direct pairing authority clock failed"),
            Self::BootstrapDelivery(error) => write!(formatter, "{error}"),
            Self::StateUnavailable(reason) => {
                write!(formatter, "direct pairing state unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for CoordinatorError {}

impl From<PairingError> for CoordinatorError {
    fn from(value: PairingError) -> Self {
        Self::Protocol(value)
    }
}

impl From<StoreError> for CoordinatorError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<rusqlite::Error> for CoordinatorError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<BootstrapDeliveryError> for CoordinatorError {
    fn from(value: BootstrapDeliveryError) -> Self {
        Self::BootstrapDelivery(value)
    }
}

struct InvitationSnapshot {
    invitation_digest: [u8; DIGEST_BYTES],
    nonce_hash: [u8; DIGEST_BYTES],
    mac_pairing_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    mac_pairing_hpke_public_key: [u8; X25519_PUBLIC_KEY_BYTES],
    tls_spki_sha256: [u8; DIGEST_BYTES],
    library_id: String,
    authority_generation: u64,
    scope_ceiling: BTreeSet<RecordKind>,
    environment: Environment,
    expires_at_ms: i64,
    state: String,
}

struct ReceiptSnapshot {
    receipt: EnrollmentReceipt,
    row_receipt_id: String,
    row_invitation_id: String,
    row_library_id: String,
    row_device_id: String,
    row_authority_generation: u64,
    row_granted_scopes: BTreeSet<RecordKind>,
    row_capabilities: BTreeMap<RecordKind, KindCapability>,
    client_signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    client_hpke_public_key: [u8; X25519_PUBLIC_KEY_BYTES],
    verification_code: Option<String>,
    confirmation_digest: Option<[u8; DIGEST_BYTES]>,
    bootstrap_envelope_bytes: Option<Vec<u8>>,
    bootstrap_envelope_digest: Option<[u8; DIGEST_BYTES]>,
    bootstrap_response_bytes: Option<Vec<u8>>,
    state: String,
    tls_spki_sha256: [u8; DIGEST_BYTES],
}

struct BootstrapMaterial {
    envelope_bytes: Vec<u8>,
    envelope_digest: [u8; DIGEST_BYTES],
    response_bytes: Vec<u8>,
}

pub struct DirectPairingCoordinator<C: PairingCrypto, T: AuthorityClock> {
    connection: Arc<Mutex<Connection>>,
    crypto: Arc<C>,
    clock: Arc<T>,
    policy: PairingPolicy,
    bindings: AuthorityBindings,
}

impl<C: PairingCrypto, T: AuthorityClock> DirectPairingCoordinator<C, T> {
    pub fn new_fixture_only(
        mut connection: Connection,
        crypto: C,
        clock: T,
        policy: PairingPolicy,
        bindings: AuthorityBindings,
    ) -> Result<Self, CoordinatorError> {
        validate_policy(&policy)?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        connection.busy_timeout(Duration::from_secs(10))?;
        let foreign_keys_enabled: i64 =
            connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if foreign_keys_enabled != 1 {
            return Err(CoordinatorError::StateUnavailable(
                "SQLite foreign-key enforcement is disabled",
            ));
        }
        DirectAuthorityStore::verify_schema(&connection)?;
        let profile: Option<(String, String, i64)> = connection
            .query_row(
                "SELECT p.environment, p.library_data_class, l.authority_generation
                 FROM direct_authority_profiles p
                 JOIN libraries l ON l.library_id = p.library_id
                 WHERE p.library_id = ?1 AND p.readiness_state = 'fixture_ready'",
                [&policy.library_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if profile
            != Some((
                "development".to_owned(),
                "sanitized_fixture".to_owned(),
                i64::try_from(policy.authority_generation)
                    .map_err(|_| CoordinatorError::StateUnavailable("authority generation"))?,
            ))
        {
            return Err(CoordinatorError::Protocol(PairingError::FixtureOnly));
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        DirectAuthorityStore::invalidate_pending_invitations_on_restart(
            &transaction,
            &policy.library_id,
            policy.authority_generation,
        )?;
        transaction.commit()?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            crypto: Arc::new(crypto),
            clock: Arc::new(clock),
            policy,
            bindings,
        })
    }

    /// Register the already signed, fixture-only invitation shown by the Mac.
    pub fn register_invitation(
        &self,
        invitation: &crate::pairing_protocol::Invitation,
    ) -> Result<InvitationRegistration, CoordinatorError> {
        let now_ms = self.now_ms()?;
        validate_invitation_shape(invitation, &self.policy, now_ms)?;
        if invitation.authority_signing_public_key != self.bindings.authority_signing_public_key
            || invitation.mac_pairing_signing_public_key
                != self.bindings.mac_pairing_signing_public_key
            || invitation.mac_pairing_hpke_public_key != self.bindings.mac_pairing_hpke_public_key
            || invitation.tls_spki_sha256 != self.bindings.tls_spki_sha256
        {
            return Err(PairingError::BindingMismatch("authority public keys or TLS pin").into());
        }
        self.crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &self.bindings.authority_signing_public_key,
                &canonical_invitation_unsigned(invitation),
                &invitation.authority_signature,
            )
            .map_err(|_| CoordinatorError::Protocol(PairingError::InvalidSignature))?;

        let request = NewInvitation {
            invitation_id: invitation.invitation_id.clone(),
            library_id: invitation.library_id.clone(),
            authority_generation: invitation.authority_generation,
            invitation_digest: sha256_array(&canonical_invitation_signed(invitation)),
            nonce_hash: to_array(&invitation_nonce_proof(&invitation.invitation_nonce))?,
            mac_pairing_signing_public_key: self.bindings.mac_pairing_signing_public_key,
            mac_pairing_hpke_public_key: self.bindings.mac_pairing_hpke_public_key,
            tls_spki_sha256: self.bindings.tls_spki_sha256,
            scope_ceiling_json: serde_json::to_string(&invitation.scope_ceiling)
                .map_err(|_| CoordinatorError::Serialization)?,
            created_at_ms: invitation.created_at_ms,
            expires_at_ms: invitation.expires_at_ms,
        };
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = DirectAuthorityStore::register_invitation(&transaction, &request, now_ms)?;
        transaction.commit()?;
        Ok(outcome)
    }

    /// Handle exactly one `ClientHello` body. The returned bytes are safe to
    /// emit only because the durable transition has already committed.
    pub fn process_client_hello(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
    ) -> Result<ClientHelloResult, CoordinatorError> {
        let hello: ClientHello = parse_bounded_json(bytes, content_encoding)?;
        validate_client_hello_shape(&hello)?;
        let now_ms = self.now_ms()?;
        let request_digest = sha256_array(&canonical_client_hello_signed(&hello));
        let observed_pin = to_array(&hello.observed_tls_spki_sha256)?;
        let mut connection = self.lock_connection()?;

        if let Some((subject_id, replay_pin)) =
            pairing_replay_subject(&connection, "client_hello", &hello.message_id)?
        {
            validate_transport_evidence(transport, &replay_pin)?;
            let probe = consume_request(
                &hello,
                subject_id,
                request_digest,
                observed_pin,
                vec![1],
                "replay".to_owned(),
                now_ms,
            )?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = DirectAuthorityStore::consume_invitation(&transaction, &probe)?;
            transaction.commit()?;
            return match outcome {
                ConsumeOutcome::ExactReplay(response) => hello_result(&connection, response),
                ConsumeOutcome::Quarantined => Err(PairingError::IdReuseQuarantined.into()),
                ConsumeOutcome::Expired => Err(PairingError::InvitationExpired.into()),
                ConsumeOutcome::Consumed(_) => Err(CoordinatorError::StateUnavailable(
                    "replay probe unexpectedly consumed an invitation",
                )),
            };
        }

        let invitation = load_invitation(&connection, &hello.invitation_id)?
            .ok_or(PairingError::InvitationNotFound)?;
        if let Err(error) = self.validate_client_hello(&connection, &invitation, &hello, transport)
        {
            persist_invitation_failure(&mut connection, &hello.invitation_id, now_ms)?;
            return Err(error.into());
        }
        match invitation.state.as_str() {
            "pending" => {}
            "cancelled" => return Err(PairingError::InvitationCancelled.into()),
            "expired" => return Err(PairingError::InvitationExpired.into()),
            "consumed" | "active" | "revoked" => {
                return Err(PairingError::InvitationConsumed.into())
            }
            _ => return Err(PairingError::StateUnavailable.into()),
        }
        if now_ms >= invitation.expires_at_ms {
            persist_invitation_failure(&mut connection, &hello.invitation_id, now_ms)?;
            return Err(PairingError::InvitationExpired.into());
        }

        let generated = match self.generate_server_hello(&invitation, &hello, now_ms) {
            Ok(generated) => generated,
            Err(error) => {
                persist_invitation_failure(&mut connection, &hello.invitation_id, now_ms)?;
                return Err(error);
            }
        };
        let mut request = consume_request(
            &hello,
            generated.receipt.receipt_id.clone(),
            request_digest,
            observed_pin,
            generated.exact_response_bytes.clone(),
            generated.verification_code.clone(),
            now_ms,
        )?;
        request.receipt_json = serde_json::to_string(&generated.receipt)
            .map_err(|_| CoordinatorError::Serialization)?;
        request.granted_scopes_json = serde_json::to_string(&generated.receipt.granted_scopes)
            .map_err(|_| CoordinatorError::Serialization)?;
        request.capabilities_json = serde_json::to_string(&generated.receipt.capabilities)
            .map_err(|_| CoordinatorError::Serialization)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = DirectAuthorityStore::consume_invitation(&transaction, &request)?;
        transaction.commit()?;
        match outcome {
            ConsumeOutcome::Consumed(response) | ConsumeOutcome::ExactReplay(response) => {
                hello_result(&connection, response)
            }
            ConsumeOutcome::Quarantined => Err(PairingError::IdReuseQuarantined.into()),
            ConsumeOutcome::Expired => Err(PairingError::InvitationExpired.into()),
        }
    }

    /// Persist the local owner's decision and, when approved, the exact HPKE
    /// bootstrap response. No caller-supplied confirmation digest is accepted.
    pub fn confirm_owner(
        &self,
        receipt_id: &str,
        displayed_verification_code: &str,
        displayed_scopes: &BTreeSet<RecordKind>,
        approved: bool,
    ) -> Result<OwnerConfirmationResult, CoordinatorError> {
        if !is_uuid_v7(receipt_id) {
            return Err(PairingError::InvalidIdentifier.into());
        }
        let now_ms = self.now_ms()?;
        let mut connection = self.lock_connection()?;
        let snapshot =
            load_receipt(&connection, receipt_id)?.ok_or(PairingError::ReceiptNotFound)?;
        self.validate_receipt_snapshot(&snapshot)?;

        if snapshot.state == "active" {
            if !approved {
                return Err(PairingError::EnrollmentAlreadyActive.into());
            }
            let bootstrap = committed_bootstrap(&snapshot)?;
            let calculated = confirmation_digest_array(
                &snapshot.receipt,
                true,
                displayed_verification_code,
                displayed_scopes,
                &bootstrap.envelope_bytes,
                &bootstrap.envelope_digest,
                &bootstrap.response_bytes,
            );
            if snapshot.confirmation_digest != Some(calculated) {
                return Err(PairingError::IdReuseQuarantined.into());
            }
            return Ok(OwnerConfirmationResult::Bootstrap(bootstrap.response_bytes));
        }
        if snapshot.state == "revoked" {
            return Err(PairingError::DeviceRevoked.into());
        }

        let scopes_json =
            serde_json::to_string(displayed_scopes).map_err(|_| CoordinatorError::Serialization)?;
        if snapshot.state == "pending_finish" {
            let bootstrap = committed_bootstrap(&snapshot)?;
            let calculated = confirmation_digest_array(
                &snapshot.receipt,
                approved,
                displayed_verification_code,
                displayed_scopes,
                &bootstrap.envelope_bytes,
                &bootstrap.envelope_digest,
                &bootstrap.response_bytes,
            );
            let request = ConfirmEnrollment {
                receipt_id: receipt_id.to_owned(),
                confirmation_digest: calculated,
                displayed_verification_code: displayed_verification_code.to_owned(),
                displayed_scopes_json: scopes_json,
                approved,
                bootstrap_envelope_bytes: bootstrap.envelope_bytes,
                bootstrap_envelope_digest: bootstrap.envelope_digest,
                exact_bootstrap_response_bytes: bootstrap.response_bytes,
                authority_now_ms: now_ms,
            };
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = DirectAuthorityStore::confirm_enrollment(&transaction, &request)?;
            transaction.commit()?;
            return map_confirmation_outcome(outcome);
        }
        if snapshot.state != "pending_user_confirmation" {
            return Err(PairingError::EnrollmentCancelled.into());
        }

        if !approved
            || snapshot.verification_code.as_deref() != Some(displayed_verification_code)
            || snapshot.receipt.granted_scopes != *displayed_scopes
        {
            let request = ConfirmEnrollment {
                receipt_id: receipt_id.to_owned(),
                confirmation_digest: confirmation_digest_array(
                    &snapshot.receipt,
                    approved,
                    displayed_verification_code,
                    displayed_scopes,
                    &[],
                    &[0; DIGEST_BYTES],
                    &[],
                ),
                displayed_verification_code: displayed_verification_code.to_owned(),
                displayed_scopes_json: scopes_json,
                approved,
                bootstrap_envelope_bytes: Vec::new(),
                bootstrap_envelope_digest: [0; DIGEST_BYTES],
                exact_bootstrap_response_bytes: Vec::new(),
                authority_now_ms: now_ms,
            };
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = DirectAuthorityStore::confirm_enrollment(&transaction, &request)?;
            transaction.commit()?;
            return map_confirmation_outcome(outcome);
        }

        let bootstrap = self.generate_bootstrap(&connection, &snapshot)?;
        let confirmation_digest = confirmation_digest_array(
            &snapshot.receipt,
            true,
            displayed_verification_code,
            displayed_scopes,
            &bootstrap.envelope_bytes,
            &bootstrap.envelope_digest,
            &bootstrap.response_bytes,
        );
        let request = ConfirmEnrollment {
            receipt_id: receipt_id.to_owned(),
            confirmation_digest,
            displayed_verification_code: displayed_verification_code.to_owned(),
            displayed_scopes_json: scopes_json,
            approved: true,
            bootstrap_envelope_bytes: bootstrap.envelope_bytes,
            bootstrap_envelope_digest: bootstrap.envelope_digest,
            exact_bootstrap_response_bytes: bootstrap.response_bytes,
            authority_now_ms: now_ms,
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = DirectAuthorityStore::confirm_enrollment(&transaction, &request)?;
        transaction.commit()?;
        map_confirmation_outcome(outcome)
    }

    /// Handle exactly one `ClientFinish` body and durably activate the replica.
    pub fn process_client_finish(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
    ) -> Result<Vec<u8>, CoordinatorError> {
        let finish: ClientFinish = parse_bounded_json(bytes, content_encoding)?;
        validate_client_finish_shape(&finish)?;
        let now_ms = self.now_ms()?;
        let request_digest = sha256_array(&canonical_client_finish_signed(&finish));
        let mut connection = self.lock_connection()?;

        if let Some((_subject_id, replay_pin)) =
            pairing_replay_subject(&connection, "client_finish", &finish.message_id)?
        {
            validate_transport_evidence(transport, &replay_pin)?;
            let probe = ActivateEnrollment {
                message_id: finish.message_id.clone(),
                receipt_id: finish.receipt_id.clone(),
                device_id: finish.device_id.clone(),
                authority_generation: finish.authority_generation,
                request_digest,
                observed_tls_spki_sha256: replay_pin,
                exact_server_finish_bytes: vec![1],
                authority_now_ms: now_ms,
            };
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = DirectAuthorityStore::activate_enrollment(&transaction, &probe)?;
            transaction.commit()?;
            return match outcome {
                ActivateOutcome::ExactReplay(response) => Ok(response),
                ActivateOutcome::Quarantined => Err(PairingError::IdReuseQuarantined.into()),
                ActivateOutcome::Expired => Err(PairingError::ReceiptExpired.into()),
                ActivateOutcome::Activated(_) => Err(CoordinatorError::StateUnavailable(
                    "replay probe unexpectedly activated a device",
                )),
            };
        }

        let snapshot =
            load_receipt(&connection, &finish.receipt_id)?.ok_or(PairingError::ReceiptNotFound)?;
        self.validate_receipt_snapshot(&snapshot)?;
        if let Err(error) = self.validate_client_finish(&snapshot, &finish, transport) {
            persist_finish_failure(&mut connection, &finish.receipt_id, now_ms)?;
            return Err(error.into());
        }
        let response = match self.generate_server_finish(&snapshot.receipt, now_ms) {
            Ok(response) => response,
            Err(error) => {
                persist_finish_failure(&mut connection, &finish.receipt_id, now_ms)?;
                return Err(error);
            }
        };
        let request = ActivateEnrollment {
            message_id: finish.message_id,
            receipt_id: finish.receipt_id,
            device_id: finish.device_id,
            authority_generation: finish.authority_generation,
            request_digest,
            observed_tls_spki_sha256: snapshot.tls_spki_sha256,
            exact_server_finish_bytes: response,
            authority_now_ms: now_ms,
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = DirectAuthorityStore::activate_enrollment(&transaction, &request)?;
        transaction.commit()?;
        match outcome {
            ActivateOutcome::Activated(response) | ActivateOutcome::ExactReplay(response) => {
                Ok(response)
            }
            ActivateOutcome::Quarantined => Err(PairingError::IdReuseQuarantined.into()),
            ActivateOutcome::Expired => Err(PairingError::ReceiptExpired.into()),
        }
    }

    /// Poll the exact approved bootstrap through the same durable pairing
    /// authority. The request is authenticated by the enrolled phone signing
    /// key, the response by the Mac pairing key, and every returned byte is
    /// committed before it can leave this method.
    pub fn process_bootstrap_poll(
        &self,
        bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<Vec<u8>, CoordinatorError> {
        let delivery_transport = BootstrapDeliveryTransport {
            tls_version: transport.tls_version.clone(),
            used_zero_rtt: transport.used_zero_rtt,
            peer_spki_sha256: transport.peer_spki_sha256.clone(),
        };
        let crypto = PairingBootstrapDeliveryCrypto(self.crypto.as_ref());
        BootstrapDeliveryCoordinator::new(self, crypto)
            .handle_poll(bytes, &delivery_transport)
            .map_err(Into::into)
    }

    fn now_ms(&self) -> Result<i64, CoordinatorError> {
        let value = self
            .clock
            .now_ms()
            .map_err(|_| CoordinatorError::ClockUnavailable)?;
        if value < 0 {
            return Err(CoordinatorError::ClockUnavailable);
        }
        Ok(value)
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, CoordinatorError> {
        self.connection
            .lock()
            .map_err(|_| CoordinatorError::StateUnavailable("database writer lock poisoned"))
    }

    fn validate_client_hello(
        &self,
        connection: &Connection,
        invitation: &InvitationSnapshot,
        hello: &ClientHello,
        transport: &TransportEvidence,
    ) -> Result<(), PairingError> {
        validate_transport_evidence(transport, &invitation.tls_spki_sha256)?;
        if hello.protocol != PAIRING_PROTOCOL {
            return Err(PairingError::UnsupportedProtocol);
        }
        if hello.suite != PAIRING_SUITE {
            return Err(PairingError::DowngradeRejected);
        }
        if hello.sender_role != PairingRole::IphoneCompanion
            || hello.recipient_role != PairingRole::MacAuthority
        {
            return Err(PairingError::BindingMismatch("roles"));
        }
        if hello.environment != Environment::Development
            || hello.environment != invitation.environment
            || hello.environment != self.policy.environment
        {
            return Err(PairingError::BindingMismatch("environment"));
        }
        if hello.library_id != invitation.library_id || hello.library_id != self.policy.library_id {
            return Err(PairingError::BindingMismatch("library_id"));
        }
        if hello.authority_generation != invitation.authority_generation
            || hello.authority_generation != self.policy.authority_generation
        {
            return Err(PairingError::AuthorityChanged);
        }
        if hello.observed_tls_spki_sha256 != invitation.tls_spki_sha256
            || invitation.tls_spki_sha256 != self.bindings.tls_spki_sha256
        {
            return Err(PairingError::PinMismatch);
        }
        if hello.nonce_proof != invitation.nonce_hash {
            return Err(PairingError::BindingMismatch("invitation nonce"));
        }
        if invitation.mac_pairing_signing_public_key != self.bindings.mac_pairing_signing_public_key
            || invitation.mac_pairing_hpke_public_key != self.bindings.mac_pairing_hpke_public_key
        {
            return Err(PairingError::BindingMismatch("Mac pairing keys"));
        }
        if hello.requested_scopes.is_empty()
            || !hello.requested_scopes.is_subset(&invitation.scope_ceiling)
            || !hello
                .requested_scopes
                .is_subset(&self.policy.grantable_scopes)
        {
            return Err(PairingError::ScopeCeilingExceeded);
        }
        validate_requested_capabilities(&hello.requested_scopes, &hello.capabilities)?;
        negotiate_capabilities(
            &hello.requested_scopes,
            &hello.capabilities,
            &self.policy.capabilities,
        )?;
        let device_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM portable_devices WHERE device_id = ?1)
                     OR EXISTS(
                       SELECT 1 FROM direct_enrollment_receipts
                       WHERE library_id = ?2 AND device_id = ?1
                         AND state IN ('pending_user_confirmation', 'pending_finish', 'active')
                     )",
                params![hello.proposed_device_id, hello.library_id],
                |row| row.get(0),
            )
            .map_err(|_| PairingError::StateUnavailable)?;
        if device_exists {
            return Err(PairingError::BindingMismatch("device_id"));
        }
        self.crypto
            .verify_signature(
                PairingRole::IphoneCompanion,
                &hello.client_signing_public_key,
                &canonical_client_hello_unsigned(hello),
                &hello.proof_signature,
            )
            .map_err(|_| PairingError::InvalidSignature)
    }

    fn generate_server_hello(
        &self,
        invitation: &InvitationSnapshot,
        hello: &ClientHello,
        now_ms: i64,
    ) -> Result<GeneratedServerHello, CoordinatorError> {
        let server_nonce = self
            .crypto
            .fresh_bytes(FreshValuePurpose::ServerNonce, NONCE_BYTES)
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        if server_nonce.len() != NONCE_BYTES {
            return Err(PairingError::CryptoUnavailable.into());
        }
        let receipt_id = self
            .crypto
            .fresh_uuid_v7(FreshValuePurpose::ReceiptId)
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        if !is_uuid_v7(&receipt_id) {
            return Err(PairingError::CryptoUnavailable.into());
        }
        let granted_scopes = hello.requested_scopes.clone();
        let capabilities = negotiate_capabilities(
            &granted_scopes,
            &hello.capabilities,
            &self.policy.capabilities,
        )?;
        let mut receipt = EnrollmentReceipt {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            receipt_id,
            invitation_id: hello.invitation_id.clone(),
            library_id: hello.library_id.clone(),
            device_id: hello.proposed_device_id.clone(),
            client_signing_key_fingerprint: sha256_vec(&hello.client_signing_public_key),
            client_hpke_key_fingerprint: sha256_vec(&hello.client_hpke_public_key),
            mac_signing_key_fingerprint: sha256_vec(&invitation.mac_pairing_signing_public_key),
            mac_hpke_key_fingerprint: sha256_vec(&invitation.mac_pairing_hpke_public_key),
            granted_scopes,
            capabilities,
            authority_generation: hello.authority_generation,
            created_at_ms: now_ms,
            expires_at_ms: invitation.expires_at_ms,
            transcript_digest: Vec::new(),
            environment: Environment::Development,
            mac_role: PairingRole::MacAuthority,
            client_role: PairingRole::IphoneCompanion,
        };
        let client_digest = sha256_vec(&canonical_client_hello_signed(hello));
        receipt.transcript_digest = pairing_transcript_digest(
            &invitation.invitation_digest,
            &client_digest,
            &server_nonce,
            &receipt,
        );
        let seal = self
            .crypto
            .seal_authenticated(
                LocalHpkeKey::MacPairing,
                &hello.client_hpke_public_key,
                &challenge_hpke_info(&receipt),
                &receipt.transcript_digest,
                &canonical_challenge_plaintext(&receipt),
                &challenge_hpke_exporter_context(&receipt),
            )
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        validate_hpke_envelope(&seal.envelope)?;
        let verification_code =
            derive_verification_code(seal.exporter_secret.as_ref(), &receipt.transcript_digest);
        let mut server_hello = ServerHello {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            server_nonce,
            receipt: receipt.clone(),
            challenge: seal.envelope,
            sender_role: PairingRole::MacAuthority,
            recipient_role: PairingRole::IphoneCompanion,
            proof_signature: Vec::new(),
        };
        server_hello.proof_signature = self
            .crypto
            .sign(
                LocalSigningKey::MacPairing,
                &canonical_server_hello_unsigned(&server_hello),
            )
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        if server_hello.proof_signature.len() != P1363_SIGNATURE_BYTES {
            return Err(PairingError::CryptoUnavailable.into());
        }
        Ok(GeneratedServerHello {
            receipt,
            exact_response_bytes: serde_json::to_vec(&server_hello)
                .map_err(|_| CoordinatorError::Serialization)?,
            verification_code,
        })
    }

    fn validate_receipt_snapshot(
        &self,
        snapshot: &ReceiptSnapshot,
    ) -> Result<(), CoordinatorError> {
        let receipt = &snapshot.receipt;
        if receipt.protocol != PAIRING_PROTOCOL
            || receipt.suite != PAIRING_SUITE
            || receipt.receipt_id != snapshot.row_receipt_id
            || receipt.invitation_id != snapshot.row_invitation_id
            || receipt.library_id != snapshot.row_library_id
            || receipt.device_id != snapshot.row_device_id
            || receipt.authority_generation != snapshot.row_authority_generation
            || receipt.granted_scopes != snapshot.row_granted_scopes
            || receipt.capabilities != snapshot.row_capabilities
            || receipt.library_id != self.policy.library_id
            || receipt.authority_generation != self.policy.authority_generation
            || receipt.environment != Environment::Development
            || receipt.environment != self.policy.environment
            || receipt.mac_role != PairingRole::MacAuthority
            || receipt.client_role != PairingRole::IphoneCompanion
        {
            return Err(PairingError::BindingMismatch("durable receipt policy").into());
        }
        if !receipt
            .granted_scopes
            .is_subset(&self.policy.grantable_scopes)
        {
            return Err(PairingError::ScopeCeilingExceeded.into());
        }
        validate_requested_capabilities(&receipt.granted_scopes, &receipt.capabilities)?;
        if receipt.client_signing_key_fingerprint != sha256_vec(&snapshot.client_signing_public_key)
            || receipt.client_hpke_key_fingerprint != sha256_vec(&snapshot.client_hpke_public_key)
            || receipt.mac_signing_key_fingerprint
                != sha256_vec(&self.bindings.mac_pairing_signing_public_key)
            || receipt.mac_hpke_key_fingerprint
                != sha256_vec(&self.bindings.mac_pairing_hpke_public_key)
            || snapshot.tls_spki_sha256 != self.bindings.tls_spki_sha256
        {
            return Err(PairingError::BindingMismatch("durable receipt keys").into());
        }
        Ok(())
    }

    fn generate_bootstrap(
        &self,
        connection: &Connection,
        snapshot: &ReceiptSnapshot,
    ) -> Result<BootstrapMaterial, CoordinatorError> {
        let receipt = &snapshot.receipt;
        let (purge_generation, key_epoch, default_scope_id) =
            load_bootstrap_authority_state(connection, &receipt.library_id)?;
        let metadata = fixture_bootstrap_metadata(
            receipt,
            purge_generation,
            key_epoch,
            &default_scope_id,
            &snapshot.tls_spki_sha256,
        )?;
        let seal = self
            .crypto
            .seal_bootstrap_key_package(
                LocalHpkeKey::MacPairing,
                &snapshot.client_hpke_public_key,
                &bootstrap_hpke_info(&metadata),
                &bootstrap_associated_data(&metadata),
                &metadata,
                &bootstrap_hpke_exporter_context(&metadata),
            )
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        validate_bootstrap_key_package_envelope(&seal.envelope)?;
        let envelope_bytes =
            serde_json::to_vec(&seal.envelope).map_err(|_| CoordinatorError::Serialization)?;
        let mut response = BootstrapEnvelope {
            protocol: PAIRING_PROTOCOL.to_owned(),
            receipt_id: receipt.receipt_id.clone(),
            metadata,
            sealed_key_package: seal.envelope,
            envelope_digest: Vec::new(),
        };
        let envelope_digest = to_array(&bootstrap_envelope_digest(&response))?;
        response.envelope_digest = envelope_digest.to_vec();
        let response_bytes =
            serde_json::to_vec(&response).map_err(|_| CoordinatorError::Serialization)?;
        Ok(BootstrapMaterial {
            envelope_bytes,
            envelope_digest,
            response_bytes,
        })
    }

    fn validate_client_finish(
        &self,
        snapshot: &ReceiptSnapshot,
        finish: &ClientFinish,
        transport: &TransportEvidence,
    ) -> Result<(), PairingError> {
        validate_transport_evidence(transport, &snapshot.tls_spki_sha256)?;
        let material = committed_bootstrap(snapshot).map_err(|_| PairingError::StateUnavailable)?;
        let bootstrap: BootstrapEnvelope = serde_json::from_slice(&material.response_bytes)
            .map_err(|_| PairingError::StateUnavailable)?;
        validate_finish_bindings(&snapshot.receipt, &bootstrap, finish)?;
        self.crypto
            .verify_signature(
                PairingRole::IphoneCompanion,
                &snapshot.client_signing_public_key,
                &canonical_client_finish_unsigned(finish),
                &finish.proof_signature,
            )
            .map_err(|_| PairingError::InvalidSignature)
    }

    fn generate_server_finish(
        &self,
        receipt: &EnrollmentReceipt,
        now_ms: i64,
    ) -> Result<Vec<u8>, CoordinatorError> {
        let mut finish = ServerFinish {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            receipt: receipt.clone(),
            activated_at_ms: now_ms,
            sender_role: PairingRole::MacAuthority,
            recipient_role: PairingRole::IphoneCompanion,
            signature: Vec::new(),
        };
        finish.signature = self
            .crypto
            .sign(
                LocalSigningKey::MacAuthority,
                &canonical_server_finish_unsigned(&finish),
            )
            .map_err(|_| CoordinatorError::Protocol(PairingError::CryptoUnavailable))?;
        if finish.signature.len() != P1363_SIGNATURE_BYTES {
            return Err(PairingError::CryptoUnavailable.into());
        }
        serde_json::to_vec(&finish).map_err(|_| CoordinatorError::Serialization)
    }
}

struct PairingBootstrapDeliveryCrypto<'a, C: PairingCrypto>(&'a C);

impl<C: PairingCrypto> BootstrapDeliveryVerifier for PairingBootstrapDeliveryCrypto<'_, C> {
    fn verify_p256_p1363(
        &self,
        signer_role: PairingRole,
        public_key: &[u8; P256_PUBLIC_KEY_BYTES],
        message: &[u8],
        signature: &[u8; P1363_SIGNATURE_BYTES],
    ) -> Result<(), ()> {
        self.0
            .verify_signature(signer_role, public_key, message, signature)
    }
}

impl<C: PairingCrypto> MacBootstrapDeliverySigner for PairingBootstrapDeliveryCrypto<'_, C> {
    fn sign_mac_delivery(&self, message: &[u8]) -> Result<[u8; P1363_SIGNATURE_BYTES], ()> {
        self.0
            .sign(LocalSigningKey::MacPairing, message)?
            .try_into()
            .map_err(|_| ())
    }
}

impl<C: PairingCrypto, T: AuthorityClock> BootstrapDeliveryStore
    for &DirectPairingCoordinator<C, T>
{
    type Error = CoordinatorError;

    fn load_delivery(
        &self,
        receipt_id: &str,
    ) -> Result<Option<BootstrapDeliverySnapshot>, Self::Error> {
        let connection = self.lock_connection()?;
        let Some(snapshot) = load_receipt(&connection, receipt_id)? else {
            return Ok(None);
        };
        self.validate_receipt_snapshot(&snapshot)?;
        let binding = BootstrapDeliveryBinding {
            receipt_id: snapshot.receipt.receipt_id.clone(),
            device_id: snapshot.receipt.device_id.clone(),
            transcript_digest: to_array(&snapshot.receipt.transcript_digest)?,
            tls_spki_sha256: snapshot.tls_spki_sha256,
            iphone_signing_public_key: snapshot.client_signing_public_key,
            mac_pairing_signing_public_key: self.bindings.mac_pairing_signing_public_key,
        };
        let expired =
            snapshot.state != "active" && self.now_ms()? >= snapshot.receipt.expires_at_ms;
        let resolution = if expired {
            BootstrapDeliveryResolution::Rejected {
                reason: BootstrapDeliveryTerminal::Expired,
            }
        } else {
            match snapshot.state.as_str() {
                "pending_user_confirmation" => BootstrapDeliveryResolution::Pending {
                    retry_after_ms: 500,
                },
                "pending_finish" | "active" => BootstrapDeliveryResolution::Ready {
                    exact_bootstrap_envelope: committed_bootstrap(&snapshot)?.response_bytes,
                },
                "cancelled" => BootstrapDeliveryResolution::Rejected {
                    reason: BootstrapDeliveryTerminal::Cancelled,
                },
                "expired" => BootstrapDeliveryResolution::Rejected {
                    reason: BootstrapDeliveryTerminal::Expired,
                },
                "revoked" => BootstrapDeliveryResolution::Rejected {
                    reason: BootstrapDeliveryTerminal::Revoked,
                },
                _ => {
                    return Err(CoordinatorError::StateUnavailable(
                        "invalid bootstrap delivery receipt state",
                    ))
                }
            }
        };
        Ok(Some(BootstrapDeliverySnapshot {
            binding,
            resolution,
        }))
    }

    fn load_replay(
        &self,
        message_id: &str,
    ) -> Result<Option<BootstrapDeliveryReplay>, Self::Error> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT message_id, receipt_id, device_id, tls_spki_sha256,
                        exact_request_sha256, exact_response_sha256,
                        exact_response_bytes
                 FROM direct_bootstrap_delivery_replays WHERE message_id = ?1",
                [message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(message_id, receipt_id, device_id, pin, request, response, bytes)| {
                    Ok(BootstrapDeliveryReplay {
                        message_id,
                        receipt_id,
                        device_id,
                        tls_spki_sha256: to_array(&pin)?,
                        exact_request_sha256: to_array(&request)?,
                        exact_response_sha256: to_array(&response)?,
                        exact_response_bytes: bytes,
                    })
                },
            )
            .transpose()
    }

    fn commit_replay(
        &self,
        replay: &BootstrapDeliveryReplay,
    ) -> Result<BootstrapReplayCommit, Self::Error> {
        let now_ms = self.now_ms()?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT message_id, receipt_id, device_id, tls_spki_sha256,
                        exact_request_sha256, exact_response_sha256,
                        exact_response_bytes
                 FROM direct_bootstrap_delivery_replays WHERE message_id = ?1",
                [&replay.message_id],
                |row| {
                    Ok(BootstrapDeliveryReplay {
                        message_id: row.get(0)?,
                        receipt_id: row.get(1)?,
                        device_id: row.get(2)?,
                        tls_spki_sha256: row
                            .get::<_, Vec<u8>>(3)?
                            .try_into()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        exact_request_sha256: row
                            .get::<_, Vec<u8>>(4)?
                            .try_into()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        exact_response_sha256: row
                            .get::<_, Vec<u8>>(5)?
                            .try_into()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        exact_response_bytes: row.get(6)?,
                    })
                },
            )
            .optional()?;
        if let Some(existing) = existing {
            transaction.commit()?;
            return Ok(
                if existing.exact_request_sha256 == replay.exact_request_sha256 {
                    BootstrapReplayCommit::Existing(existing)
                } else {
                    BootstrapReplayCommit::Conflict
                },
            );
        }
        let recent: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM direct_bootstrap_delivery_replays
             WHERE receipt_id = ?1 AND created_at_ms >= ?2",
            params![
                replay.receipt_id,
                now_ms.saturating_sub(BOOTSTRAP_DELIVERY_REPLAY_RATE_WINDOW_MS)
            ],
            |row| row.get(0),
        )?;
        if recent >= MAX_BOOTSTRAP_DELIVERY_REPLAYS_PER_WINDOW {
            return Err(CoordinatorError::Protocol(PairingError::ResourceLimit));
        }
        transaction.execute(
            "INSERT INTO direct_bootstrap_delivery_replays (
               message_id, receipt_id, device_id, tls_spki_sha256,
               exact_request_sha256, exact_response_sha256,
               exact_response_bytes, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                replay.message_id,
                replay.receipt_id,
                replay.device_id,
                replay.tls_spki_sha256.as_slice(),
                replay.exact_request_sha256.as_slice(),
                replay.exact_response_sha256.as_slice(),
                replay.exact_response_bytes,
                now_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(BootstrapReplayCommit::Inserted)
    }
}

struct GeneratedServerHello {
    receipt: EnrollmentReceipt,
    exact_response_bytes: Vec<u8>,
    verification_code: String,
}

fn load_bootstrap_authority_state(
    connection: &Connection,
    library_id: &str,
) -> Result<(u64, u64, String), CoordinatorError> {
    let row: Option<(i64, i64, String)> = connection
        .query_row(
            "SELECT l.purge_generation, l.current_key_epoch, s.scope_id
             FROM libraries l
             JOIN library_scopes s ON s.library_id = l.library_id
             WHERE l.library_id = ?1 AND s.scope_class = 'unknown'",
            [library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((purge_generation, key_epoch, default_scope_id)) = row else {
        return Err(CoordinatorError::StateUnavailable(
            "durable unknown bootstrap scope is missing",
        ));
    };
    let purge_generation = u64::try_from(purge_generation)
        .map_err(|_| CoordinatorError::StateUnavailable("negative purge generation"))?;
    let key_epoch = u64::try_from(key_epoch)
        .map_err(|_| CoordinatorError::StateUnavailable("negative key epoch"))?;
    if key_epoch == 0 || !is_uuid_v7(&default_scope_id) {
        return Err(CoordinatorError::StateUnavailable(
            "durable bootstrap key or scope is invalid",
        ));
    }
    Ok((purge_generation, key_epoch, default_scope_id))
}

fn load_invitation(
    connection: &Connection,
    invitation_id: &str,
) -> Result<Option<InvitationSnapshot>, CoordinatorError> {
    connection
        .query_row(
            "SELECT invitation_digest, nonce_hash,
                    mac_pairing_signing_public_key, mac_pairing_hpke_public_key,
                    tls_spki_sha256, library_id, authority_generation,
                    scope_ceiling_json, environment, expires_at_ms, state
             FROM direct_pairing_invitations WHERE invitation_id = ?1",
            [invitation_id],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .map(
            |(
                digest,
                nonce,
                signing,
                hpke,
                pin,
                library,
                generation,
                scopes,
                environment,
                expires,
                state,
            )| {
                Ok(InvitationSnapshot {
                    invitation_digest: to_array(&digest)?,
                    nonce_hash: to_array(&nonce)?,
                    mac_pairing_signing_public_key: to_array(&signing)?,
                    mac_pairing_hpke_public_key: to_array(&hpke)?,
                    tls_spki_sha256: to_array(&pin)?,
                    library_id: library,
                    authority_generation: u64::try_from(generation).map_err(|_| {
                        CoordinatorError::StateUnavailable("negative authority generation")
                    })?,
                    scope_ceiling: serde_json::from_str(&scopes)
                        .map_err(|_| CoordinatorError::StateUnavailable("invalid stored scopes"))?,
                    environment: match environment.as_str() {
                        "development" => Environment::Development,
                        _ => return Err(CoordinatorError::Protocol(PairingError::FixtureOnly)),
                    },
                    expires_at_ms: expires,
                    state,
                })
            },
        )
        .transpose()
}

fn load_receipt(
    connection: &Connection,
    receipt_id: &str,
) -> Result<Option<ReceiptSnapshot>, CoordinatorError> {
    connection
        .query_row(
            "SELECT r.receipt_json, r.receipt_id, r.invitation_id, r.library_id,
                    r.device_id, r.authority_generation, r.granted_scopes_json,
                    r.capabilities_json, r.client_signing_public_key,
                    r.client_hpke_public_key, r.verification_code,
                    r.confirmation_digest, r.bootstrap_envelope_bytes,
                    r.bootstrap_envelope_digest, r.bootstrap_response_bytes,
                    r.state, i.tls_spki_sha256
             FROM direct_enrollment_receipts r
             JOIN direct_pairing_invitations i ON i.invitation_id = r.invitation_id
             WHERE r.receipt_id = ?1",
            [receipt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<Vec<u8>>>(11)?,
                    row.get::<_, Option<Vec<u8>>>(12)?,
                    row.get::<_, Option<Vec<u8>>>(13)?,
                    row.get::<_, Option<Vec<u8>>>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, Vec<u8>>(16)?,
                ))
            },
        )
        .optional()?
        .map(
            |(
                receipt,
                row_receipt_id,
                row_invitation_id,
                row_library_id,
                row_device_id,
                row_generation,
                row_scopes,
                row_capabilities,
                signing,
                hpke,
                code,
                confirmation,
                envelope,
                envelope_digest,
                response,
                state,
                pin,
            )| {
                Ok(ReceiptSnapshot {
                    receipt: serde_json::from_str(&receipt).map_err(|_| {
                        CoordinatorError::StateUnavailable("invalid stored receipt")
                    })?,
                    row_receipt_id,
                    row_invitation_id,
                    row_library_id,
                    row_device_id,
                    row_authority_generation: u64::try_from(row_generation).map_err(|_| {
                        CoordinatorError::StateUnavailable("negative receipt generation")
                    })?,
                    row_granted_scopes: serde_json::from_str(&row_scopes).map_err(|_| {
                        CoordinatorError::StateUnavailable("invalid stored granted scopes")
                    })?,
                    row_capabilities: serde_json::from_str(&row_capabilities).map_err(|_| {
                        CoordinatorError::StateUnavailable("invalid stored capabilities")
                    })?,
                    client_signing_public_key: to_array(&signing)?,
                    client_hpke_public_key: to_array(&hpke)?,
                    verification_code: code,
                    confirmation_digest: confirmation.map(|value| to_array(&value)).transpose()?,
                    bootstrap_envelope_bytes: envelope,
                    bootstrap_envelope_digest: envelope_digest
                        .map(|value| to_array(&value))
                        .transpose()?,
                    bootstrap_response_bytes: response,
                    state,
                    tls_spki_sha256: to_array(&pin)?,
                })
            },
        )
        .transpose()
}

fn pairing_replay_subject(
    connection: &Connection,
    kind: &str,
    message_id: &str,
) -> Result<Option<(String, [u8; DIGEST_BYTES])>, CoordinatorError> {
    connection
        .query_row(
            "SELECT subject_id, tls_spki_sha256 FROM direct_pairing_replays
             WHERE message_kind = ?1 AND message_id = ?2",
            params![kind, message_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?
        .map(|(subject, pin)| Ok((subject, to_array(&pin)?)))
        .transpose()
}

fn consume_request(
    hello: &ClientHello,
    receipt_id: String,
    request_digest: [u8; DIGEST_BYTES],
    observed_pin: [u8; DIGEST_BYTES],
    response: Vec<u8>,
    verification_code: String,
    now_ms: i64,
) -> Result<ConsumeInvitation, CoordinatorError> {
    Ok(ConsumeInvitation {
        message_id: hello.message_id.clone(),
        invitation_id: hello.invitation_id.clone(),
        request_digest,
        observed_tls_spki_sha256: observed_pin,
        receipt_id,
        device_id: hello.proposed_device_id.clone(),
        display_name: hello.display_name.clone(),
        app_version: hello.app_version.clone(),
        build_version: hello.build_version.clone(),
        receipt_json: "{}".to_owned(),
        granted_scopes_json: serde_json::to_string(&hello.requested_scopes)
            .map_err(|_| CoordinatorError::Serialization)?,
        capabilities_json: serde_json::to_string(&hello.capabilities)
            .map_err(|_| CoordinatorError::Serialization)?,
        client_signing_public_key: to_array(&hello.client_signing_public_key)?,
        client_hpke_public_key: to_array(&hello.client_hpke_public_key)?,
        exact_begin_response_bytes: response,
        verification_code,
        authority_now_ms: now_ms,
    })
}

fn hello_result(
    connection: &Connection,
    exact_response_bytes: Vec<u8>,
) -> Result<ClientHelloResult, CoordinatorError> {
    let server: ServerHello = serde_json::from_slice(&exact_response_bytes)
        .map_err(|_| CoordinatorError::StateUnavailable("invalid stored ServerHello"))?;
    let verification_code = connection
        .query_row(
            "SELECT verification_code FROM direct_enrollment_receipts WHERE receipt_id = ?1",
            [&server.receipt.receipt_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(ClientHelloResult {
        receipt_id: server.receipt.receipt_id,
        exact_response_bytes,
        verification_code,
    })
}

fn persist_invitation_failure(
    connection: &mut Connection,
    invitation_id: &str,
    now_ms: i64,
) -> Result<(), CoordinatorError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    DirectAuthorityStore::record_invitation_failure(&transaction, invitation_id, now_ms)?;
    transaction.commit()?;
    Ok(())
}

fn persist_finish_failure(
    connection: &mut Connection,
    receipt_id: &str,
    now_ms: i64,
) -> Result<(), CoordinatorError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    DirectAuthorityStore::record_finish_failure(&transaction, receipt_id, now_ms)?;
    transaction.commit()?;
    Ok(())
}

fn committed_bootstrap(snapshot: &ReceiptSnapshot) -> Result<BootstrapMaterial, CoordinatorError> {
    let envelope_bytes =
        snapshot
            .bootstrap_envelope_bytes
            .clone()
            .ok_or(CoordinatorError::StateUnavailable(
                "bootstrap envelope missing",
            ))?;
    let envelope_digest =
        snapshot
            .bootstrap_envelope_digest
            .ok_or(CoordinatorError::StateUnavailable(
                "bootstrap digest missing",
            ))?;
    let response_bytes =
        snapshot
            .bootstrap_response_bytes
            .clone()
            .ok_or(CoordinatorError::StateUnavailable(
                "bootstrap response missing",
            ))?;
    let envelope: AuthenticatedHpkeEnvelope = serde_json::from_slice(&envelope_bytes)
        .map_err(|_| CoordinatorError::StateUnavailable("invalid stored bootstrap envelope"))?;
    let response: BootstrapEnvelope = serde_json::from_slice(&response_bytes)
        .map_err(|_| CoordinatorError::StateUnavailable("invalid stored bootstrap response"))?;
    if validate_bootstrap(&response, &snapshot.receipt).is_err()
        || response.sealed_key_package != envelope
        || response.envelope_digest != envelope_digest
    {
        return Err(CoordinatorError::StateUnavailable(
            "stored bootstrap binding is inconsistent",
        ));
    }
    Ok(BootstrapMaterial {
        envelope_bytes,
        envelope_digest,
        response_bytes,
    })
}

fn confirmation_digest_array(
    receipt: &EnrollmentReceipt,
    approved: bool,
    code: &str,
    scopes: &BTreeSet<RecordKind>,
    envelope: &[u8],
    envelope_digest: &[u8],
    response: &[u8],
) -> [u8; DIGEST_BYTES] {
    enrollment_confirmation_digest(
        receipt,
        approved,
        code,
        scopes,
        envelope,
        envelope_digest,
        response,
    )
    .try_into()
    .expect("SHA-256 confirmation digests are always 32 bytes")
}

fn map_confirmation_outcome(
    outcome: ConfirmOutcome,
) -> Result<OwnerConfirmationResult, CoordinatorError> {
    match outcome {
        ConfirmOutcome::Confirmed(response) | ConfirmOutcome::ExactReplay(response) => {
            Ok(OwnerConfirmationResult::Bootstrap(response))
        }
        ConfirmOutcome::Cancelled => Ok(OwnerConfirmationResult::Cancelled),
        ConfirmOutcome::Quarantined => Err(PairingError::IdReuseQuarantined.into()),
        ConfirmOutcome::Expired => Err(PairingError::ReceiptExpired.into()),
    }
}

fn sha256_vec(value: &[u8]) -> Vec<u8> {
    Sha256::digest(value).to_vec()
}

fn sha256_array(value: &[u8]) -> [u8; DIGEST_BYTES] {
    Sha256::digest(value).into()
}

fn to_array<const N: usize>(value: &[u8]) -> Result<[u8; N], CoordinatorError> {
    value
        .try_into()
        .map_err(|_| CoordinatorError::StateUnavailable("stored byte length mismatch"))
}
