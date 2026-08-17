//! Fixture-only state machine for the direct device-enrollment protocol.
//!
//! This module deliberately contains no production cryptography and no network
//! transport. Callers must supply a [`PairingCrypto`] implementation and TLS
//! evidence from a transport adapter. The current safety gate accepts sanitized
//! fixture libraries only; personal data must remain blocked until the external
//! cryptographic review and cross-language vectors required by Decision 008 are
//! complete.

use serde::de::{
    DeserializeOwned, DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

pub const PAIRING_PROTOCOL: &str = "noted.direct-pairing.v1";
pub const PAIRING_SUITE: &str = "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm";
pub const MAX_INVITATION_LIFETIME_MS: i64 = 5 * 60 * 1_000;
pub const MAX_CLOCK_SKEW_MS: i64 = 30 * 1_000;
pub const MAX_FAILED_ATTEMPTS: u8 = 5;
pub const MAX_PAIRING_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_JSON_DEPTH: usize = 8;
pub const MAX_JSON_MEMBERS: usize = 1_024;
pub const MAX_JSON_OBJECT_MEMBERS: usize = 64;
pub const MAX_JSON_ARRAY_ELEMENTS: usize = 128;
pub const MAX_JSON_STRING_BYTES: usize = 1_024;
pub const MAX_JSON_TOTAL_STRING_BYTES: usize = 8 * 1024;
pub const MAX_REPLAY_ENTRIES: usize = 128;
pub const MAX_QUARANTINE_ENTRIES: usize = 128;

const NONCE_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const P256_PUBLIC_KEY_BYTES: usize = 65;
const X25519_PUBLIC_KEY_BYTES: usize = 32;
const P1363_SIGNATURE_BYTES: usize = 64;
const MAX_SEALED_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Development,
    Production,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingRole {
    MacAuthority,
    IphoneCompanion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Note,
    Category,
    Folder,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryDataClass {
    SanitizedFixture,
    Personal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindCapability {
    pub reader_version: u32,
    pub writer_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub protocol: String,
    pub suite: String,
    pub invitation_id: String,
    pub invitation_nonce: Vec<u8>,
    pub authority_signing_public_key: Vec<u8>,
    pub mac_pairing_signing_public_key: Vec<u8>,
    pub mac_pairing_hpke_public_key: Vec<u8>,
    pub tls_spki_sha256: Vec<u8>,
    pub library_id: String,
    pub authority_generation: u64,
    pub scope_ceiling: BTreeSet<RecordKind>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub environment: Environment,
    pub authority_role: PairingRole,
    pub intended_client_role: PairingRole,
    pub library_data_class: LibraryDataClass,
    pub authority_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    pub protocol: String,
    pub suite: String,
    pub message_id: String,
    pub invitation_id: String,
    pub nonce_proof: Vec<u8>,
    pub client_nonce: Vec<u8>,
    pub proposed_device_id: String,
    pub display_name: String,
    pub client_signing_public_key: Vec<u8>,
    pub client_hpke_public_key: Vec<u8>,
    pub requested_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
    pub app_version: String,
    pub build_version: String,
    pub library_id: String,
    pub authority_generation: u64,
    pub environment: Environment,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub observed_tls_spki_sha256: Vec<u8>,
    pub proof_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentReceipt {
    pub protocol: String,
    pub suite: String,
    pub receipt_id: String,
    pub invitation_id: String,
    pub library_id: String,
    pub device_id: String,
    pub client_signing_key_fingerprint: Vec<u8>,
    pub client_hpke_key_fingerprint: Vec<u8>,
    pub mac_signing_key_fingerprint: Vec<u8>,
    pub mac_hpke_key_fingerprint: Vec<u8>,
    pub granted_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
    pub authority_generation: u64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub transcript_digest: Vec<u8>,
    pub environment: Environment,
    pub mac_role: PairingRole,
    pub client_role: PairingRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerHello {
    pub protocol: String,
    pub suite: String,
    pub server_nonce: Vec<u8>,
    pub receipt: EnrollmentReceipt,
    pub challenge_ciphertext: Vec<u8>,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub proof_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginEnrollment {
    pub receipt_id: String,
    pub server_hello_bytes: Vec<u8>,
    pub verification_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapEnvelope {
    pub protocol: String,
    pub receipt_id: String,
    pub ciphertext: Vec<u8>,
    pub ciphertext_digest: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientFinish {
    pub protocol: String,
    pub suite: String,
    pub message_id: String,
    pub receipt_id: String,
    pub invitation_id: String,
    pub library_id: String,
    pub device_id: String,
    pub authority_generation: u64,
    pub environment: Environment,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub transcript_digest: Vec<u8>,
    pub ciphertext_digest: Vec<u8>,
    pub proof_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerFinish {
    pub protocol: String,
    pub suite: String,
    pub receipt: EnrollmentReceipt,
    pub activated_at_ms: i64,
    pub sender_role: PairingRole,
    pub recipient_role: PairingRole,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationState {
    Pending,
    Consumed,
    Active,
    Cancelled,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptState {
    PendingUserConfirmation,
    PendingFinish,
    Active,
    Cancelled,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSigningKey {
    MacPairing,
    MacAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalHpkeKey {
    MacPairing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshValuePurpose {
    ReceiptId,
    ServerNonce,
}

/// Cryptographic and entropy operations are intentionally abstract. This
/// state machine never treats a fixture implementation as production-ready.
pub trait PairingCrypto: Send + Sync + 'static {
    fn verify_signature(
        &self,
        signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()>;

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()>;

    fn seal_authenticated(
        &self,
        sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        associated_data: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ()>;

    fn exporter_secret(
        &self,
        sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        transcript_digest: &[u8],
    ) -> Result<Vec<u8>, ()>;

    fn fresh_bytes(&self, purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()>;

    fn fresh_uuid_v7(&self, purpose: FreshValuePurpose) -> Result<String, ()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPolicy {
    pub library_id: String,
    pub environment: Environment,
    pub library_data_class: LibraryDataClass,
    pub authority_generation: u64,
    pub grantable_scopes: BTreeSet<RecordKind>,
    pub capabilities: BTreeMap<RecordKind, KindCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingError {
    FixtureOnly,
    PayloadTooLarge,
    UnsupportedEncoding,
    ParseRejected(String),
    UnsupportedProtocol,
    UnsupportedSuite,
    DowngradeRejected,
    InvalidIdentifier,
    InvalidField(&'static str),
    InvalidSignature,
    CryptoUnavailable,
    InsecureTransport,
    PinMismatch,
    InvitationNotFound,
    InvitationExpired,
    InvitationCancelled,
    InvitationConsumed,
    AttemptLimitReached,
    AuthorityChanged,
    BindingMismatch(&'static str),
    ScopeCeilingExceeded,
    CapabilityMismatch,
    ScopeNotGranted,
    ReceiptNotFound,
    ReceiptExpired,
    UserConfirmationRequired,
    VerificationMismatch,
    EnrollmentCancelled,
    EnrollmentAlreadyActive,
    DeviceRevoked,
    IdReuseQuarantined,
    ResourceLimit,
    StateUnavailable,
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FixtureOnly => write!(
                formatter,
                "personal-data pairing is disabled pending external cryptographic review"
            ),
            Self::PayloadTooLarge => write!(formatter, "pairing payload exceeds the byte limit"),
            Self::UnsupportedEncoding => {
                write!(formatter, "compressed pairing payloads are not accepted")
            }
            Self::ParseRejected(reason) => {
                write!(formatter, "pairing payload was rejected: {reason}")
            }
            Self::UnsupportedProtocol => write!(formatter, "unsupported pairing protocol"),
            Self::UnsupportedSuite => write!(formatter, "unsupported pairing suite"),
            Self::DowngradeRejected => write!(formatter, "pairing suite downgrade rejected"),
            Self::InvalidIdentifier => write!(formatter, "invalid pairing identifier"),
            Self::InvalidField(field) => write!(formatter, "invalid pairing field: {field}"),
            Self::InvalidSignature => write!(formatter, "pairing signature rejected"),
            Self::CryptoUnavailable => write!(formatter, "pairing cryptography is unavailable"),
            Self::InsecureTransport => write!(formatter, "pairing requires TLS 1.3 without 0-RTT"),
            Self::PinMismatch => write!(formatter, "pairing TLS pin mismatch"),
            Self::InvitationNotFound => write!(formatter, "pairing invitation not found"),
            Self::InvitationExpired => write!(formatter, "pairing invitation expired"),
            Self::InvitationCancelled => write!(formatter, "pairing invitation cancelled"),
            Self::InvitationConsumed => write!(formatter, "pairing invitation already consumed"),
            Self::AttemptLimitReached => {
                write!(formatter, "pairing invitation attempt limit reached")
            }
            Self::AuthorityChanged => write!(formatter, "library authority generation changed"),
            Self::BindingMismatch(field) => write!(formatter, "pairing binding mismatch: {field}"),
            Self::ScopeCeilingExceeded => {
                write!(formatter, "requested scopes exceed the invitation ceiling")
            }
            Self::CapabilityMismatch => {
                write!(formatter, "record-kind capability negotiation failed")
            }
            Self::ScopeNotGranted => write!(formatter, "record-kind scope was not granted"),
            Self::ReceiptNotFound => write!(formatter, "pairing receipt not found"),
            Self::ReceiptExpired => write!(formatter, "pairing receipt expired"),
            Self::UserConfirmationRequired => write!(formatter, "user confirmation is required"),
            Self::VerificationMismatch => write!(
                formatter,
                "verification code or displayed scopes do not match"
            ),
            Self::EnrollmentCancelled => write!(formatter, "device enrollment cancelled"),
            Self::EnrollmentAlreadyActive => {
                write!(formatter, "device enrollment is already active")
            }
            Self::DeviceRevoked => write!(formatter, "device enrollment is revoked"),
            Self::IdReuseQuarantined => {
                write!(formatter, "byte-different identifier reuse was quarantined")
            }
            Self::ResourceLimit => write!(formatter, "pairing state resource limit reached"),
            Self::StateUnavailable => write!(formatter, "pairing state is unavailable"),
        }
    }
}

impl std::error::Error for PairingError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportEvidence {
    pub tls_version: String,
    pub used_zero_rtt: bool,
    pub peer_spki_sha256: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineRecord {
    pub identifier: String,
    pub reason: String,
    pub accepted_digest: Vec<u8>,
    pub observed_digest: Vec<u8>,
}

#[derive(Clone)]
pub struct PairingCheckpoint {
    policy: PairingPolicy,
    ledger: Ledger,
}

#[derive(Debug, Clone)]
struct Ledger {
    authority_generation: u64,
    invitations: BTreeMap<String, StoredInvitation>,
    receipts: BTreeMap<String, StoredReceipt>,
    devices: BTreeMap<String, StoredDevice>,
    hello_replays: BTreeMap<String, HelloReplay>,
    finish_replays: BTreeMap<String, FinishReplay>,
    quarantines: Vec<QuarantineRecord>,
}

#[derive(Debug, Clone)]
struct StoredInvitation {
    invitation_digest: Vec<u8>,
    nonce_hash: Vec<u8>,
    mac_pairing_signing_public_key: Vec<u8>,
    mac_pairing_hpke_public_key: Vec<u8>,
    tls_spki_sha256: Vec<u8>,
    library_id: String,
    authority_generation: u64,
    scope_ceiling: BTreeSet<RecordKind>,
    expires_at_ms: i64,
    environment: Environment,
    failed_attempts: u8,
    state: InvitationState,
    receipt_id: Option<String>,
}

#[derive(Debug, Clone)]
struct StoredReceipt {
    receipt: EnrollmentReceipt,
    client_signing_public_key: Vec<u8>,
    client_hpke_public_key: Vec<u8>,
    server_hello: BeginEnrollment,
    verification_code: String,
    bootstrap: Option<BootstrapEnvelope>,
    failed_finish_attempts: u8,
    state: ReceiptState,
    server_finish_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct StoredDevice {
    receipt_id: String,
    state: DeviceState,
    activated_at_ms: i64,
    revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct HelloReplay {
    digest: Vec<u8>,
    tls_spki_sha256: Vec<u8>,
    result: BeginEnrollment,
}

#[derive(Debug, Clone)]
struct FinishReplay {
    digest: Vec<u8>,
    tls_spki_sha256: Vec<u8>,
    result: Vec<u8>,
}

pub struct PairingMachine<C: PairingCrypto> {
    crypto: Arc<C>,
    policy: PairingPolicy,
    ledger: Arc<Mutex<Ledger>>,
}

impl<C: PairingCrypto> PairingMachine<C> {
    pub fn new_fixture_only(crypto: C, policy: PairingPolicy) -> Result<Self, PairingError> {
        validate_policy(&policy)?;
        Ok(Self {
            crypto: Arc::new(crypto),
            ledger: Arc::new(Mutex::new(Ledger {
                authority_generation: policy.authority_generation,
                invitations: BTreeMap::new(),
                receipts: BTreeMap::new(),
                devices: BTreeMap::new(),
                hello_replays: BTreeMap::new(),
                finish_replays: BTreeMap::new(),
                quarantines: Vec::new(),
            })),
            policy,
        })
    }

    pub fn restore_fixture_only(
        crypto: C,
        policy: PairingPolicy,
        checkpoint: PairingCheckpoint,
    ) -> Result<Self, PairingError> {
        validate_policy(&policy)?;
        if checkpoint.ledger.authority_generation != policy.authority_generation {
            return Err(PairingError::AuthorityChanged);
        }
        if checkpoint.policy != policy {
            return Err(PairingError::BindingMismatch("checkpoint policy"));
        }
        Ok(Self {
            crypto: Arc::new(crypto),
            policy,
            ledger: Arc::new(Mutex::new(checkpoint.ledger)),
        })
    }

    pub fn checkpoint(&self) -> Result<PairingCheckpoint, PairingError> {
        Ok(PairingCheckpoint {
            policy: self.policy.clone(),
            ledger: self.lock_ledger()?.clone(),
        })
    }

    pub fn register_invitation(
        &self,
        invitation: Invitation,
        now_ms: i64,
    ) -> Result<(), PairingError> {
        validate_invitation_shape(&invitation, &self.policy, now_ms)?;
        if invitation.library_data_class != LibraryDataClass::SanitizedFixture {
            return Err(PairingError::FixtureOnly);
        }
        let unsigned = canonical_invitation_unsigned(&invitation);
        self.crypto
            .verify_signature(
                PairingRole::MacAuthority,
                &invitation.authority_signing_public_key,
                &unsigned,
                &invitation.authority_signature,
            )
            .map_err(|_| PairingError::InvalidSignature)?;
        let signed_digest = sha256(&canonical_invitation_signed(&invitation));

        let mut ledger = self.lock_ledger()?;
        if ledger.authority_generation != invitation.authority_generation {
            return Err(PairingError::AuthorityChanged);
        }
        if let Some(existing) = ledger.invitations.get(&invitation.invitation_id) {
            if existing.invitation_digest == signed_digest {
                return Ok(());
            }
            let accepted = existing.invitation_digest.clone();
            quarantine(
                &mut ledger,
                &invitation.invitation_id,
                "byte-different invitation id reuse",
                &accepted,
                &signed_digest,
            );
            return Err(PairingError::IdReuseQuarantined);
        }
        if ledger.invitations.len() >= MAX_REPLAY_ENTRIES {
            return Err(PairingError::ResourceLimit);
        }

        ledger.invitations.insert(
            invitation.invitation_id,
            StoredInvitation {
                invitation_digest: signed_digest,
                nonce_hash: invitation_nonce_proof(&invitation.invitation_nonce),
                mac_pairing_signing_public_key: invitation.mac_pairing_signing_public_key,
                mac_pairing_hpke_public_key: invitation.mac_pairing_hpke_public_key,
                tls_spki_sha256: invitation.tls_spki_sha256,
                library_id: invitation.library_id,
                authority_generation: invitation.authority_generation,
                scope_ceiling: invitation.scope_ceiling,
                expires_at_ms: invitation.expires_at_ms,
                environment: invitation.environment,
                failed_attempts: 0,
                state: InvitationState::Pending,
                receipt_id: None,
            },
        );
        Ok(())
    }

    pub fn process_client_hello(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<BeginEnrollment, PairingError> {
        let hello: ClientHello = parse_bounded_json(bytes, content_encoding)?;
        validate_client_hello_shape(&hello)?;
        let signed_bytes = canonical_client_hello_signed(&hello);
        let message_digest = sha256(&signed_bytes);
        let mut ledger = self.lock_ledger()?;

        if let Some(replay) = ledger.hello_replays.get(&hello.message_id) {
            validate_transport_evidence(transport, &replay.tls_spki_sha256)?;
            if replay.digest == message_digest {
                return Ok(replay.result.clone());
            }
            let accepted = replay.digest.clone();
            quarantine(
                &mut ledger,
                &hello.message_id,
                "byte-different ClientHello message id reuse",
                &accepted,
                &message_digest,
            );
            return Err(PairingError::IdReuseQuarantined);
        }
        let tls_spki_sha256 = ledger
            .invitations
            .get(&hello.invitation_id)
            .ok_or(PairingError::InvitationNotFound)?
            .tls_spki_sha256
            .clone();
        if ledger.hello_replays.len() >= MAX_REPLAY_ENTRIES {
            return Err(PairingError::ResourceLimit);
        }

        let result = self.process_client_hello_locked(&mut ledger, &hello, transport, now_ms);
        if let Ok(begin) = &result {
            ledger.hello_replays.insert(
                hello.message_id,
                HelloReplay {
                    digest: message_digest,
                    tls_spki_sha256,
                    result: begin.clone(),
                },
            );
        }
        result
    }

    fn process_client_hello_locked(
        &self,
        ledger: &mut Ledger,
        hello: &ClientHello,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<BeginEnrollment, PairingError> {
        let Some(existing) = ledger.invitations.get(&hello.invitation_id) else {
            return Err(PairingError::InvitationNotFound);
        };
        if existing.state == InvitationState::Pending && is_expired(now_ms, existing.expires_at_ms)
        {
            ledger
                .invitations
                .get_mut(&hello.invitation_id)
                .expect("invitation exists")
                .state = InvitationState::Expired;
        }

        let snapshot = ledger
            .invitations
            .get(&hello.invitation_id)
            .expect("invitation exists")
            .clone();
        match snapshot.state {
            InvitationState::Pending => {}
            InvitationState::Expired => return Err(PairingError::InvitationExpired),
            InvitationState::Cancelled => return Err(PairingError::InvitationCancelled),
            InvitationState::Consumed | InvitationState::Active | InvitationState::Revoked => {
                return Err(PairingError::InvitationConsumed)
            }
        }

        let validation = self.validate_client_hello(&snapshot, hello, transport, ledger);
        if let Err(error) = validation {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(
                if ledger
                    .invitations
                    .get(&hello.invitation_id)
                    .is_some_and(|invitation| invitation.failed_attempts >= MAX_FAILED_ATTEMPTS)
                {
                    PairingError::AttemptLimitReached
                } else {
                    error
                },
            );
        }

        let server_nonce = self
            .crypto
            .fresh_bytes(FreshValuePurpose::ServerNonce, NONCE_BYTES)
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if server_nonce.len() != NONCE_BYTES {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::CryptoUnavailable);
        }
        let receipt_id = self
            .crypto
            .fresh_uuid_v7(FreshValuePurpose::ReceiptId)
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if !is_uuid_v7(&receipt_id) || ledger.receipts.contains_key(&receipt_id) {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::CryptoUnavailable);
        }

        let granted_scopes: BTreeSet<_> = hello
            .requested_scopes
            .intersection(&self.policy.grantable_scopes)
            .copied()
            .collect();
        if granted_scopes.is_empty() {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::ScopeCeilingExceeded);
        }
        let capabilities = negotiate_capabilities(
            &granted_scopes,
            &hello.capabilities,
            &self.policy.capabilities,
        )?;

        let mut receipt = EnrollmentReceipt {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            receipt_id: receipt_id.clone(),
            invitation_id: hello.invitation_id.clone(),
            library_id: hello.library_id.clone(),
            device_id: hello.proposed_device_id.clone(),
            client_signing_key_fingerprint: sha256(&hello.client_signing_public_key),
            client_hpke_key_fingerprint: sha256(&hello.client_hpke_public_key),
            mac_signing_key_fingerprint: sha256(&snapshot.mac_pairing_signing_public_key),
            mac_hpke_key_fingerprint: sha256(&snapshot.mac_pairing_hpke_public_key),
            granted_scopes,
            capabilities,
            authority_generation: hello.authority_generation,
            created_at_ms: now_ms,
            expires_at_ms: snapshot.expires_at_ms,
            transcript_digest: Vec::new(),
            environment: hello.environment,
            mac_role: PairingRole::MacAuthority,
            client_role: PairingRole::IphoneCompanion,
        };
        let client_digest = sha256(&canonical_client_hello_signed(hello));
        let transcript_digest = pairing_transcript_digest(
            &snapshot.invitation_digest,
            &client_digest,
            &server_nonce,
            &receipt,
        );
        receipt.transcript_digest = transcript_digest.clone();

        let challenge_plaintext = canonical_components(
            "noted.direct-pairing.v1/challenge",
            &[
                ("receipt_id", receipt_id.as_bytes()),
                ("transcript_digest", &transcript_digest),
                ("library_id", receipt.library_id.as_bytes()),
                ("device_id", receipt.device_id.as_bytes()),
            ],
        );
        let challenge_ciphertext = self
            .crypto
            .seal_authenticated(
                LocalHpkeKey::MacPairing,
                &hello.client_hpke_public_key,
                &transcript_digest,
                &challenge_plaintext,
            )
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if challenge_ciphertext.is_empty() || challenge_ciphertext.len() > MAX_SEALED_BYTES {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::CryptoUnavailable);
        }

        let mut server_hello = ServerHello {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            server_nonce,
            receipt: receipt.clone(),
            challenge_ciphertext,
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
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if server_hello.proof_signature.len() != P1363_SIGNATURE_BYTES {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::CryptoUnavailable);
        }
        let server_hello_bytes =
            serde_json::to_vec(&server_hello).map_err(|_| PairingError::StateUnavailable)?;
        let exporter_secret = self
            .crypto
            .exporter_secret(
                LocalHpkeKey::MacPairing,
                &hello.client_hpke_public_key,
                &transcript_digest,
            )
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if exporter_secret.len() < DIGEST_BYTES {
            record_failed_attempt(ledger, &hello.invitation_id);
            return Err(PairingError::CryptoUnavailable);
        }
        let verification_code = derive_verification_code(&exporter_secret, &transcript_digest);
        let begin = BeginEnrollment {
            receipt_id: receipt_id.clone(),
            server_hello_bytes,
            verification_code: verification_code.clone(),
        };

        let invitation = ledger
            .invitations
            .get_mut(&hello.invitation_id)
            .expect("invitation exists");
        invitation.state = InvitationState::Consumed;
        invitation.receipt_id = Some(receipt_id.clone());
        ledger.receipts.insert(
            receipt_id,
            StoredReceipt {
                receipt,
                client_signing_public_key: hello.client_signing_public_key.clone(),
                client_hpke_public_key: hello.client_hpke_public_key.clone(),
                server_hello: begin.clone(),
                verification_code,
                bootstrap: None,
                failed_finish_attempts: 0,
                state: ReceiptState::PendingUserConfirmation,
                server_finish_bytes: None,
            },
        );
        Ok(begin)
    }

    fn validate_client_hello(
        &self,
        invitation: &StoredInvitation,
        hello: &ClientHello,
        transport: &TransportEvidence,
        ledger: &Ledger,
    ) -> Result<(), PairingError> {
        validate_transport_evidence(transport, &invitation.tls_spki_sha256)?;
        if hello.observed_tls_spki_sha256 != invitation.tls_spki_sha256 {
            return Err(PairingError::PinMismatch);
        }
        if hello.protocol != PAIRING_PROTOCOL {
            return Err(PairingError::UnsupportedProtocol);
        }
        if hello.suite != PAIRING_SUITE {
            return Err(if hello.suite.starts_with("noted.direct-pairing") {
                PairingError::DowngradeRejected
            } else {
                PairingError::UnsupportedSuite
            });
        }
        if hello.sender_role != PairingRole::IphoneCompanion
            || hello.recipient_role != PairingRole::MacAuthority
        {
            return Err(PairingError::BindingMismatch("roles"));
        }
        if hello.environment != invitation.environment
            || hello.environment != self.policy.environment
        {
            return Err(PairingError::BindingMismatch("environment"));
        }
        if hello.library_id != invitation.library_id || hello.library_id != self.policy.library_id {
            return Err(PairingError::BindingMismatch("library_id"));
        }
        if hello.authority_generation != invitation.authority_generation
            || hello.authority_generation != ledger.authority_generation
        {
            return Err(PairingError::AuthorityChanged);
        }
        if hello.nonce_proof != invitation.nonce_hash {
            return Err(PairingError::BindingMismatch("invitation_nonce"));
        }
        if hello.requested_scopes.is_empty()
            || !hello.requested_scopes.is_subset(&invitation.scope_ceiling)
        {
            return Err(PairingError::ScopeCeilingExceeded);
        }
        if ledger.devices.contains_key(&hello.proposed_device_id) {
            return Err(PairingError::BindingMismatch("device_id"));
        }
        validate_requested_capabilities(&hello.requested_scopes, &hello.capabilities)?;
        self.crypto
            .verify_signature(
                PairingRole::IphoneCompanion,
                &hello.client_signing_public_key,
                &canonical_client_hello_unsigned(hello),
                &hello.proof_signature,
            )
            .map_err(|_| PairingError::InvalidSignature)
    }

    pub fn confirm_user(
        &self,
        receipt_id: &str,
        displayed_verification_code: &str,
        displayed_scopes: &BTreeSet<RecordKind>,
        approved: bool,
        now_ms: i64,
    ) -> Result<BootstrapEnvelope, PairingError> {
        let mut ledger = self.lock_ledger()?;
        let Some(receipt_snapshot) = ledger.receipts.get(receipt_id).cloned() else {
            return Err(PairingError::ReceiptNotFound);
        };
        if is_expired(now_ms, receipt_snapshot.receipt.expires_at_ms) {
            expire_receipt(&mut ledger, receipt_id);
            return Err(PairingError::ReceiptExpired);
        }
        match receipt_snapshot.state {
            ReceiptState::Cancelled => return Err(PairingError::EnrollmentCancelled),
            ReceiptState::Expired => return Err(PairingError::ReceiptExpired),
            ReceiptState::Revoked => return Err(PairingError::DeviceRevoked),
            ReceiptState::Active => return Err(PairingError::EnrollmentAlreadyActive),
            ReceiptState::PendingFinish | ReceiptState::PendingUserConfirmation => {}
        }

        if !approved {
            cancel_receipt(&mut ledger, receipt_id);
            return Err(PairingError::EnrollmentCancelled);
        }
        if displayed_verification_code != receipt_snapshot.verification_code
            || displayed_scopes != &receipt_snapshot.receipt.granted_scopes
        {
            cancel_receipt(&mut ledger, receipt_id);
            return Err(PairingError::VerificationMismatch);
        }
        if receipt_snapshot.state == ReceiptState::PendingFinish {
            return receipt_snapshot
                .bootstrap
                .ok_or(PairingError::StateUnavailable);
        }

        #[derive(Serialize)]
        struct FixtureBootstrap<'a> {
            fixture_marker: &'static str,
            receipt_id: &'a str,
            library_id: &'a str,
            device_id: &'a str,
            authority_generation: u64,
            scopes: &'a BTreeSet<RecordKind>,
        }
        let plaintext = serde_json::to_vec(&FixtureBootstrap {
            fixture_marker: "sanitized-fixtures-only",
            receipt_id,
            library_id: &receipt_snapshot.receipt.library_id,
            device_id: &receipt_snapshot.receipt.device_id,
            authority_generation: receipt_snapshot.receipt.authority_generation,
            scopes: &receipt_snapshot.receipt.granted_scopes,
        })
        .map_err(|_| PairingError::StateUnavailable)?;
        let associated_data = canonical_receipt(&receipt_snapshot.receipt);
        let ciphertext = self
            .crypto
            .seal_authenticated(
                LocalHpkeKey::MacPairing,
                &receipt_snapshot.client_hpke_public_key,
                &associated_data,
                &plaintext,
            )
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if ciphertext.is_empty() || ciphertext.len() > MAX_SEALED_BYTES {
            return Err(PairingError::CryptoUnavailable);
        }
        let bootstrap = BootstrapEnvelope {
            protocol: PAIRING_PROTOCOL.to_owned(),
            receipt_id: receipt_id.to_owned(),
            ciphertext_digest: sha256(&ciphertext),
            ciphertext,
        };
        let receipt = ledger.receipts.get_mut(receipt_id).expect("receipt exists");
        receipt.bootstrap = Some(bootstrap.clone());
        receipt.state = ReceiptState::PendingFinish;
        Ok(bootstrap)
    }

    pub fn process_client_finish(
        &self,
        bytes: &[u8],
        content_encoding: Option<&str>,
        transport: &TransportEvidence,
        now_ms: i64,
    ) -> Result<Vec<u8>, PairingError> {
        let finish: ClientFinish = parse_bounded_json(bytes, content_encoding)?;
        validate_client_finish_shape(&finish)?;
        let message_digest = sha256(&canonical_client_finish_signed(&finish));
        let mut ledger = self.lock_ledger()?;
        if let Some(replay) = ledger.finish_replays.get(&finish.message_id) {
            validate_transport_evidence(transport, &replay.tls_spki_sha256)?;
            if replay.digest == message_digest {
                return Ok(replay.result.clone());
            }
            let accepted = replay.digest.clone();
            quarantine(
                &mut ledger,
                &finish.message_id,
                "byte-different ClientFinish message id reuse",
                &accepted,
                &message_digest,
            );
            return Err(PairingError::IdReuseQuarantined);
        }
        let tls_spki_sha256 = ledger
            .receipts
            .get(&finish.receipt_id)
            .and_then(|receipt| ledger.invitations.get(&receipt.receipt.invitation_id))
            .ok_or(PairingError::ReceiptNotFound)?
            .tls_spki_sha256
            .clone();
        validate_transport_evidence(transport, &tls_spki_sha256)?;
        if ledger.finish_replays.len() >= MAX_REPLAY_ENTRIES {
            return Err(PairingError::ResourceLimit);
        }
        let result = self.process_client_finish_locked(&mut ledger, &finish, now_ms);
        if let Ok(response) = &result {
            ledger.finish_replays.insert(
                finish.message_id,
                FinishReplay {
                    digest: message_digest,
                    tls_spki_sha256,
                    result: response.clone(),
                },
            );
        }
        result
    }

    fn process_client_finish_locked(
        &self,
        ledger: &mut Ledger,
        finish: &ClientFinish,
        now_ms: i64,
    ) -> Result<Vec<u8>, PairingError> {
        let Some(snapshot) = ledger.receipts.get(&finish.receipt_id).cloned() else {
            return Err(PairingError::ReceiptNotFound);
        };
        if is_expired(now_ms, snapshot.receipt.expires_at_ms) {
            expire_receipt(ledger, &finish.receipt_id);
            return Err(PairingError::ReceiptExpired);
        }
        match snapshot.state {
            ReceiptState::PendingUserConfirmation => {
                return Err(PairingError::UserConfirmationRequired)
            }
            ReceiptState::PendingFinish => {}
            ReceiptState::Active => return Err(PairingError::EnrollmentAlreadyActive),
            ReceiptState::Cancelled => return Err(PairingError::EnrollmentCancelled),
            ReceiptState::Expired => return Err(PairingError::ReceiptExpired),
            ReceiptState::Revoked => return Err(PairingError::DeviceRevoked),
        }
        let bootstrap = snapshot.bootstrap.ok_or(PairingError::StateUnavailable)?;
        let validation =
            validate_finish_bindings(&snapshot.receipt, &bootstrap, finish).and_then(|_| {
                self.crypto
                    .verify_signature(
                        PairingRole::IphoneCompanion,
                        &snapshot.client_signing_public_key,
                        &canonical_client_finish_unsigned(finish),
                        &finish.proof_signature,
                    )
                    .map_err(|_| PairingError::InvalidSignature)
            });
        if let Err(error) = validation {
            let receipt = ledger
                .receipts
                .get_mut(&finish.receipt_id)
                .expect("receipt exists");
            receipt.failed_finish_attempts = receipt.failed_finish_attempts.saturating_add(1);
            if receipt.failed_finish_attempts >= MAX_FAILED_ATTEMPTS {
                cancel_receipt(ledger, &finish.receipt_id);
                return Err(PairingError::AttemptLimitReached);
            }
            return Err(error);
        }

        let mut server_finish = ServerFinish {
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            receipt: snapshot.receipt.clone(),
            activated_at_ms: now_ms,
            sender_role: PairingRole::MacAuthority,
            recipient_role: PairingRole::IphoneCompanion,
            signature: Vec::new(),
        };
        server_finish.signature = self
            .crypto
            .sign(
                LocalSigningKey::MacAuthority,
                &canonical_server_finish_unsigned(&server_finish),
            )
            .map_err(|_| PairingError::CryptoUnavailable)?;
        if server_finish.signature.len() != P1363_SIGNATURE_BYTES {
            return Err(PairingError::CryptoUnavailable);
        }
        let response =
            serde_json::to_vec(&server_finish).map_err(|_| PairingError::StateUnavailable)?;

        let receipt = ledger
            .receipts
            .get_mut(&finish.receipt_id)
            .expect("receipt exists");
        receipt.state = ReceiptState::Active;
        receipt.server_finish_bytes = Some(response.clone());
        let invitation = ledger
            .invitations
            .get_mut(&snapshot.receipt.invitation_id)
            .ok_or(PairingError::StateUnavailable)?;
        invitation.state = InvitationState::Active;
        ledger.devices.insert(
            snapshot.receipt.device_id.clone(),
            StoredDevice {
                receipt_id: finish.receipt_id.clone(),
                state: DeviceState::Active,
                activated_at_ms: now_ms,
                revoked_at_ms: None,
            },
        );
        Ok(response)
    }

    pub fn revoke_device(&self, device_id: &str, now_ms: i64) -> Result<(), PairingError> {
        let mut ledger = self.lock_ledger()?;
        let Some(device_snapshot) = ledger.devices.get(device_id).cloned() else {
            return Err(PairingError::ReceiptNotFound);
        };
        if device_snapshot.state == DeviceState::Revoked {
            return Ok(());
        }
        let invitation_id = {
            let receipt = ledger
                .receipts
                .get_mut(&device_snapshot.receipt_id)
                .ok_or(PairingError::StateUnavailable)?;
            receipt.state = ReceiptState::Revoked;
            receipt.receipt.invitation_id.clone()
        };
        let invitation = ledger
            .invitations
            .get_mut(&invitation_id)
            .ok_or(PairingError::StateUnavailable)?;
        invitation.state = InvitationState::Revoked;
        let device = ledger.devices.get_mut(device_id).expect("device exists");
        device.state = DeviceState::Revoked;
        device.revoked_at_ms = Some(now_ms);
        Ok(())
    }

    pub fn rotate_authority_generation(&self, new_generation: u64) -> Result<(), PairingError> {
        let mut ledger = self.lock_ledger()?;
        if new_generation <= ledger.authority_generation {
            return Err(PairingError::AuthorityChanged);
        }
        ledger.authority_generation = new_generation;
        for invitation in ledger.invitations.values_mut() {
            if matches!(
                invitation.state,
                InvitationState::Pending | InvitationState::Consumed
            ) {
                invitation.state = InvitationState::Cancelled;
            }
        }
        for receipt in ledger.receipts.values_mut() {
            if matches!(
                receipt.state,
                ReceiptState::PendingUserConfirmation | ReceiptState::PendingFinish
            ) {
                receipt.state = ReceiptState::Cancelled;
            }
        }
        Ok(())
    }

    /// Models an app restart. Pending invitations survive only while the exact
    /// pairing sheet is explicitly kept open; already-consumed receipts remain
    /// resumable from their durable checkpoint.
    pub fn handle_restart(
        &self,
        kept_open_invitation_id: Option<&str>,
    ) -> Result<(), PairingError> {
        let mut ledger = self.lock_ledger()?;
        for (id, invitation) in &mut ledger.invitations {
            if invitation.state == InvitationState::Pending
                && kept_open_invitation_id != Some(id.as_str())
            {
                invitation.state = InvitationState::Cancelled;
            }
        }
        Ok(())
    }

    pub fn cancel_invitation(&self, invitation_id: &str) -> Result<(), PairingError> {
        let mut ledger = self.lock_ledger()?;
        let invitation = ledger
            .invitations
            .get_mut(invitation_id)
            .ok_or(PairingError::InvitationNotFound)?;
        if invitation.state == InvitationState::Active {
            return Err(PairingError::EnrollmentAlreadyActive);
        }
        invitation.state = InvitationState::Cancelled;
        if let Some(receipt_id) = invitation.receipt_id.clone() {
            if let Some(receipt) = ledger.receipts.get_mut(&receipt_id) {
                receipt.state = ReceiptState::Cancelled;
                receipt.bootstrap = None;
            }
        }
        Ok(())
    }

    pub fn invitation_state(&self, id: &str) -> Result<Option<InvitationState>, PairingError> {
        Ok(self.lock_ledger()?.invitations.get(id).map(|row| row.state))
    }

    pub fn invitation_failed_attempts(&self, id: &str) -> Result<Option<u8>, PairingError> {
        Ok(self
            .lock_ledger()?
            .invitations
            .get(id)
            .map(|row| row.failed_attempts))
    }

    pub fn receipt_state(&self, id: &str) -> Result<Option<ReceiptState>, PairingError> {
        Ok(self.lock_ledger()?.receipts.get(id).map(|row| row.state))
    }

    pub fn device_state(&self, id: &str) -> Result<Option<DeviceState>, PairingError> {
        Ok(self.lock_ledger()?.devices.get(id).map(|row| row.state))
    }

    /// Narrow authorization hook for the future `/sync/v1` adapter. It binds
    /// the device to this library, environment, and authority generation and
    /// fails immediately once the registry row is revoked.
    pub fn require_active_device(
        &self,
        device_id: &str,
        library_id: &str,
        environment: Environment,
        authority_generation: u64,
    ) -> Result<(), PairingError> {
        let ledger = self.lock_ledger()?;
        let device = ledger
            .devices
            .get(device_id)
            .ok_or(PairingError::ReceiptNotFound)?;
        if device.state == DeviceState::Revoked {
            return Err(PairingError::DeviceRevoked);
        }
        let receipt = ledger
            .receipts
            .get(&device.receipt_id)
            .ok_or(PairingError::StateUnavailable)?;
        if receipt.receipt.library_id != library_id {
            return Err(PairingError::BindingMismatch("library_id"));
        }
        if receipt.receipt.environment != environment {
            return Err(PairingError::BindingMismatch("environment"));
        }
        if receipt.receipt.authority_generation != authority_generation
            || ledger.authority_generation != authority_generation
        {
            return Err(PairingError::AuthorityChanged);
        }
        Ok(())
    }

    /// Authorizes one scoped read or write without exposing the full enrollment
    /// receipt to a transport adapter.
    pub fn require_active_device_scope(
        &self,
        device_id: &str,
        library_id: &str,
        environment: Environment,
        authority_generation: u64,
        scope: RecordKind,
        require_write: bool,
    ) -> Result<KindCapability, PairingError> {
        let ledger = self.lock_ledger()?;
        let device = ledger
            .devices
            .get(device_id)
            .ok_or(PairingError::ReceiptNotFound)?;
        if device.state == DeviceState::Revoked {
            return Err(PairingError::DeviceRevoked);
        }
        let receipt = ledger
            .receipts
            .get(&device.receipt_id)
            .ok_or(PairingError::StateUnavailable)?;
        if receipt.receipt.library_id != library_id {
            return Err(PairingError::BindingMismatch("library_id"));
        }
        if receipt.receipt.environment != environment {
            return Err(PairingError::BindingMismatch("environment"));
        }
        if receipt.receipt.authority_generation != authority_generation
            || ledger.authority_generation != authority_generation
        {
            return Err(PairingError::AuthorityChanged);
        }
        if !receipt.receipt.granted_scopes.contains(&scope) {
            return Err(PairingError::ScopeNotGranted);
        }
        let capability = receipt
            .receipt
            .capabilities
            .get(&scope)
            .copied()
            .ok_or(PairingError::CapabilityMismatch)?;
        if require_write && capability.writer_version.is_none() {
            return Err(PairingError::ScopeNotGranted);
        }
        Ok(capability)
    }

    pub fn quarantine_records(&self) -> Result<Vec<QuarantineRecord>, PairingError> {
        Ok(self.lock_ledger()?.quarantines.clone())
    }

    pub fn active_device_timestamps(
        &self,
        id: &str,
    ) -> Result<Option<(i64, Option<i64>)>, PairingError> {
        Ok(self
            .lock_ledger()?
            .devices
            .get(id)
            .map(|row| (row.activated_at_ms, row.revoked_at_ms)))
    }

    pub fn pending_server_hello(
        &self,
        receipt_id: &str,
    ) -> Result<Option<BeginEnrollment>, PairingError> {
        Ok(self
            .lock_ledger()?
            .receipts
            .get(receipt_id)
            .map(|row| row.server_hello.clone()))
    }

    fn lock_ledger(&self) -> Result<std::sync::MutexGuard<'_, Ledger>, PairingError> {
        self.ledger
            .lock()
            .map_err(|_| PairingError::StateUnavailable)
    }
}

fn validate_policy(policy: &PairingPolicy) -> Result<(), PairingError> {
    if !is_uuid_v7(&policy.library_id) {
        return Err(PairingError::InvalidIdentifier);
    }
    if policy.authority_generation == 0 || policy.grantable_scopes.is_empty() {
        return Err(PairingError::InvalidField("policy"));
    }
    if policy.environment != Environment::Development
        || policy.library_data_class != LibraryDataClass::SanitizedFixture
    {
        return Err(PairingError::FixtureOnly);
    }
    validate_requested_capabilities(&policy.grantable_scopes, &policy.capabilities)
}

fn validate_invitation_shape(
    invitation: &Invitation,
    policy: &PairingPolicy,
    now_ms: i64,
) -> Result<(), PairingError> {
    if invitation.protocol != PAIRING_PROTOCOL {
        return Err(PairingError::UnsupportedProtocol);
    }
    if invitation.library_data_class != policy.library_data_class {
        return Err(PairingError::FixtureOnly);
    }
    if invitation.suite != PAIRING_SUITE {
        return Err(PairingError::UnsupportedSuite);
    }
    if !is_uuid_v7(&invitation.invitation_id) {
        return Err(PairingError::InvalidIdentifier);
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
    if !is_uuid_v7(&invitation.library_id) {
        return Err(PairingError::InvalidIdentifier);
    }
    if invitation.authority_role != PairingRole::MacAuthority
        || invitation.intended_client_role != PairingRole::IphoneCompanion
    {
        return Err(PairingError::BindingMismatch("roles"));
    }
    if invitation.library_id != policy.library_id {
        return Err(PairingError::BindingMismatch("library_id"));
    }
    if invitation.environment != policy.environment {
        return Err(PairingError::BindingMismatch("environment"));
    }
    if invitation.authority_generation != policy.authority_generation {
        return Err(PairingError::AuthorityChanged);
    }
    if invitation.scope_ceiling.is_empty()
        || !invitation.scope_ceiling.is_subset(&policy.grantable_scopes)
    {
        return Err(PairingError::ScopeCeilingExceeded);
    }
    let lifetime_ms = invitation
        .expires_at_ms
        .checked_sub(invitation.created_at_ms)
        .ok_or(PairingError::InvitationExpired)?;
    if lifetime_ms <= 0
        || lifetime_ms > MAX_INVITATION_LIFETIME_MS
        || invitation.created_at_ms > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
        || is_expired(now_ms, invitation.expires_at_ms)
    {
        return Err(PairingError::InvitationExpired);
    }
    Ok(())
}

fn validate_transport_evidence(
    transport: &TransportEvidence,
    expected_spki_sha256: &[u8],
) -> Result<(), PairingError> {
    if transport.tls_version != "1.3" || transport.used_zero_rtt {
        return Err(PairingError::InsecureTransport);
    }
    if transport.peer_spki_sha256 != expected_spki_sha256 {
        return Err(PairingError::PinMismatch);
    }
    Ok(())
}

fn validate_client_hello_shape(hello: &ClientHello) -> Result<(), PairingError> {
    if !is_uuid_v7(&hello.message_id)
        || !is_uuid_v7(&hello.invitation_id)
        || !is_uuid_v7(&hello.proposed_device_id)
    {
        return Err(PairingError::InvalidIdentifier);
    }
    exact_len(&hello.nonce_proof, DIGEST_BYTES, "nonce_proof")?;
    exact_len(&hello.client_nonce, NONCE_BYTES, "client_nonce")?;
    exact_len(
        &hello.client_signing_public_key,
        P256_PUBLIC_KEY_BYTES,
        "client_signing_public_key",
    )?;
    exact_len(
        &hello.client_hpke_public_key,
        X25519_PUBLIC_KEY_BYTES,
        "client_hpke_public_key",
    )?;
    exact_len(
        &hello.observed_tls_spki_sha256,
        DIGEST_BYTES,
        "observed_tls_spki_sha256",
    )?;
    exact_len(
        &hello.proof_signature,
        P1363_SIGNATURE_BYTES,
        "proof_signature",
    )?;
    if !is_uuid_v7(&hello.library_id) {
        return Err(PairingError::InvalidIdentifier);
    }
    validate_text(&hello.display_name, 80, "display_name")?;
    validate_text(&hello.app_version, 64, "app_version")?;
    validate_text(&hello.build_version, 64, "build_version")?;
    Ok(())
}

fn validate_client_finish_shape(finish: &ClientFinish) -> Result<(), PairingError> {
    if !is_uuid_v7(&finish.message_id)
        || !is_uuid_v7(&finish.receipt_id)
        || !is_uuid_v7(&finish.invitation_id)
        || !is_uuid_v7(&finish.device_id)
    {
        return Err(PairingError::InvalidIdentifier);
    }
    exact_len(&finish.transcript_digest, DIGEST_BYTES, "transcript_digest")?;
    exact_len(&finish.ciphertext_digest, DIGEST_BYTES, "ciphertext_digest")?;
    exact_len(
        &finish.proof_signature,
        P1363_SIGNATURE_BYTES,
        "proof_signature",
    )?;
    if !is_uuid_v7(&finish.library_id) {
        return Err(PairingError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_finish_bindings(
    receipt: &EnrollmentReceipt,
    bootstrap: &BootstrapEnvelope,
    finish: &ClientFinish,
) -> Result<(), PairingError> {
    if finish.protocol != PAIRING_PROTOCOL {
        return Err(PairingError::UnsupportedProtocol);
    }
    if finish.suite != PAIRING_SUITE {
        return Err(PairingError::DowngradeRejected);
    }
    if finish.sender_role != PairingRole::IphoneCompanion
        || finish.recipient_role != PairingRole::MacAuthority
    {
        return Err(PairingError::BindingMismatch("roles"));
    }
    if finish.receipt_id != receipt.receipt_id
        || finish.invitation_id != receipt.invitation_id
        || finish.library_id != receipt.library_id
        || finish.device_id != receipt.device_id
        || finish.authority_generation != receipt.authority_generation
        || finish.environment != receipt.environment
        || finish.transcript_digest != receipt.transcript_digest
        || finish.ciphertext_digest != bootstrap.ciphertext_digest
    {
        return Err(PairingError::BindingMismatch("finish transcript"));
    }
    Ok(())
}

fn validate_requested_capabilities(
    scopes: &BTreeSet<RecordKind>,
    capabilities: &BTreeMap<RecordKind, KindCapability>,
) -> Result<(), PairingError> {
    if capabilities.len() != scopes.len()
        || capabilities.keys().any(|kind| !scopes.contains(kind))
        || scopes.iter().any(|kind| !capabilities.contains_key(kind))
    {
        return Err(PairingError::CapabilityMismatch);
    }
    for capability in capabilities.values() {
        if capability.reader_version == 0
            || capability.writer_version == Some(0)
            || capability
                .writer_version
                .is_some_and(|writer| writer > capability.reader_version)
        {
            return Err(PairingError::CapabilityMismatch);
        }
    }
    Ok(())
}

fn negotiate_capabilities(
    scopes: &BTreeSet<RecordKind>,
    client: &BTreeMap<RecordKind, KindCapability>,
    server: &BTreeMap<RecordKind, KindCapability>,
) -> Result<BTreeMap<RecordKind, KindCapability>, PairingError> {
    let mut selected = BTreeMap::new();
    for kind in scopes {
        let client_capability = client.get(kind).ok_or(PairingError::CapabilityMismatch)?;
        let server_capability = server.get(kind).ok_or(PairingError::CapabilityMismatch)?;
        let reader_version = client_capability
            .reader_version
            .min(server_capability.reader_version);
        if reader_version == 0 {
            return Err(PairingError::CapabilityMismatch);
        }
        let writer_version = match (
            client_capability.writer_version,
            server_capability.writer_version,
        ) {
            (Some(client), Some(server)) => Some(client.min(server).min(reader_version)),
            _ => None,
        };
        selected.insert(
            *kind,
            KindCapability {
                reader_version,
                writer_version,
            },
        );
    }
    Ok(selected)
}

fn record_failed_attempt(ledger: &mut Ledger, invitation_id: &str) {
    if let Some(invitation) = ledger.invitations.get_mut(invitation_id) {
        invitation.failed_attempts = invitation.failed_attempts.saturating_add(1);
        if invitation.failed_attempts >= MAX_FAILED_ATTEMPTS {
            invitation.state = InvitationState::Cancelled;
        }
    }
}

fn expire_receipt(ledger: &mut Ledger, receipt_id: &str) {
    let invitation_id = ledger
        .receipts
        .get(receipt_id)
        .map(|row| row.receipt.invitation_id.clone());
    if let Some(receipt) = ledger.receipts.get_mut(receipt_id) {
        receipt.state = ReceiptState::Expired;
        receipt.bootstrap = None;
    }
    if let Some(invitation_id) = invitation_id {
        if let Some(invitation) = ledger.invitations.get_mut(&invitation_id) {
            invitation.state = InvitationState::Expired;
        }
    }
}

fn cancel_receipt(ledger: &mut Ledger, receipt_id: &str) {
    let invitation_id = ledger
        .receipts
        .get(receipt_id)
        .map(|row| row.receipt.invitation_id.clone());
    if let Some(receipt) = ledger.receipts.get_mut(receipt_id) {
        receipt.state = ReceiptState::Cancelled;
        receipt.bootstrap = None;
    }
    if let Some(invitation_id) = invitation_id {
        if let Some(invitation) = ledger.invitations.get_mut(&invitation_id) {
            invitation.state = InvitationState::Cancelled;
        }
    }
}

fn quarantine(
    ledger: &mut Ledger,
    identifier: &str,
    reason: &str,
    accepted_digest: &[u8],
    observed_digest: &[u8],
) {
    if ledger.quarantines.len() >= MAX_QUARANTINE_ENTRIES {
        return;
    }
    ledger.quarantines.push(QuarantineRecord {
        identifier: identifier.to_owned(),
        reason: reason.to_owned(),
        accepted_digest: accepted_digest.to_vec(),
        observed_digest: observed_digest.to_vec(),
    });
}

fn is_expired(now_ms: i64, expires_at_ms: i64) -> bool {
    now_ms > expires_at_ms.saturating_add(MAX_CLOCK_SKEW_MS)
}

fn exact_len(value: &[u8], expected: usize, field: &'static str) -> Result<(), PairingError> {
    if value.len() == expected {
        Ok(())
    } else {
        Err(PairingError::InvalidField(field))
    }
}

fn validate_text(value: &str, max: usize, field: &'static str) -> Result<(), PairingError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(PairingError::InvalidField(field))
    } else {
        Ok(())
    }
}

fn is_uuid_v7(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return false;
    }
    if bytes[14] != b'7' || !matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b') {
        return false;
    }
    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

pub fn invitation_nonce_proof(invitation_nonce: &[u8]) -> Vec<u8> {
    sha256(&canonical_components(
        "noted.direct-pairing.v1/invitation-nonce-proof",
        &[("nonce", invitation_nonce)],
    ))
}

pub fn canonical_invitation_unsigned(invitation: &Invitation) -> Vec<u8> {
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/invitation");
    builder.text("protocol", &invitation.protocol);
    builder.text("suite", &invitation.suite);
    builder.text("invitation_id", &invitation.invitation_id);
    builder.bytes("invitation_nonce", &invitation.invitation_nonce);
    builder.bytes(
        "authority_signing_public_key",
        &invitation.authority_signing_public_key,
    );
    builder.bytes(
        "mac_pairing_signing_public_key",
        &invitation.mac_pairing_signing_public_key,
    );
    builder.bytes(
        "mac_pairing_hpke_public_key",
        &invitation.mac_pairing_hpke_public_key,
    );
    builder.bytes("tls_spki_sha256", &invitation.tls_spki_sha256);
    builder.text("library_id", &invitation.library_id);
    builder.u64("authority_generation", invitation.authority_generation);
    builder.record_kinds("scope_ceiling", &invitation.scope_ceiling);
    builder.i64("created_at_ms", invitation.created_at_ms);
    builder.i64("expires_at_ms", invitation.expires_at_ms);
    builder.text("environment", environment_name(invitation.environment));
    builder.text("authority_role", role_name(invitation.authority_role));
    builder.text(
        "intended_client_role",
        role_name(invitation.intended_client_role),
    );
    builder.text(
        "library_data_class",
        data_class_name(invitation.library_data_class),
    );
    builder.finish()
}

fn canonical_invitation_signed(invitation: &Invitation) -> Vec<u8> {
    let unsigned = canonical_invitation_unsigned(invitation);
    canonical_components(
        "noted.direct-pairing.v1/signed-invitation",
        &[
            ("unsigned", &unsigned),
            ("signature", &invitation.authority_signature),
        ],
    )
}

pub fn canonical_client_hello_unsigned(hello: &ClientHello) -> Vec<u8> {
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/client-hello");
    builder.text("protocol", &hello.protocol);
    builder.text("suite", &hello.suite);
    builder.text("message_id", &hello.message_id);
    builder.text("invitation_id", &hello.invitation_id);
    builder.bytes("nonce_proof", &hello.nonce_proof);
    builder.bytes("client_nonce", &hello.client_nonce);
    builder.text("proposed_device_id", &hello.proposed_device_id);
    builder.text("display_name", &hello.display_name);
    builder.bytes(
        "client_signing_public_key",
        &hello.client_signing_public_key,
    );
    builder.bytes("client_hpke_public_key", &hello.client_hpke_public_key);
    builder.record_kinds("requested_scopes", &hello.requested_scopes);
    builder.capabilities("capabilities", &hello.capabilities);
    builder.text("app_version", &hello.app_version);
    builder.text("build_version", &hello.build_version);
    builder.text("library_id", &hello.library_id);
    builder.u64("authority_generation", hello.authority_generation);
    builder.text("environment", environment_name(hello.environment));
    builder.text("sender_role", role_name(hello.sender_role));
    builder.text("recipient_role", role_name(hello.recipient_role));
    builder.bytes("observed_tls_spki_sha256", &hello.observed_tls_spki_sha256);
    builder.finish()
}

fn canonical_client_hello_signed(hello: &ClientHello) -> Vec<u8> {
    let unsigned = canonical_client_hello_unsigned(hello);
    canonical_components(
        "noted.direct-pairing.v1/signed-client-hello",
        &[
            ("unsigned", &unsigned),
            ("signature", &hello.proof_signature),
        ],
    )
}

pub fn canonical_client_finish_unsigned(finish: &ClientFinish) -> Vec<u8> {
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/client-finish");
    builder.text("protocol", &finish.protocol);
    builder.text("suite", &finish.suite);
    builder.text("message_id", &finish.message_id);
    builder.text("receipt_id", &finish.receipt_id);
    builder.text("invitation_id", &finish.invitation_id);
    builder.text("library_id", &finish.library_id);
    builder.text("device_id", &finish.device_id);
    builder.u64("authority_generation", finish.authority_generation);
    builder.text("environment", environment_name(finish.environment));
    builder.text("sender_role", role_name(finish.sender_role));
    builder.text("recipient_role", role_name(finish.recipient_role));
    builder.bytes("transcript_digest", &finish.transcript_digest);
    builder.bytes("ciphertext_digest", &finish.ciphertext_digest);
    builder.finish()
}

fn canonical_client_finish_signed(finish: &ClientFinish) -> Vec<u8> {
    let unsigned = canonical_client_finish_unsigned(finish);
    canonical_components(
        "noted.direct-pairing.v1/signed-client-finish",
        &[
            ("unsigned", &unsigned),
            ("signature", &finish.proof_signature),
        ],
    )
}

fn canonical_server_hello_unsigned(server: &ServerHello) -> Vec<u8> {
    let receipt = canonical_receipt(&server.receipt);
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/server-hello");
    builder.text("protocol", &server.protocol);
    builder.text("suite", &server.suite);
    builder.bytes("server_nonce", &server.server_nonce);
    builder.bytes("receipt", &receipt);
    builder.bytes("challenge_ciphertext", &server.challenge_ciphertext);
    builder.text("sender_role", role_name(server.sender_role));
    builder.text("recipient_role", role_name(server.recipient_role));
    builder.finish()
}

fn canonical_server_finish_unsigned(finish: &ServerFinish) -> Vec<u8> {
    let receipt = canonical_receipt(&finish.receipt);
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/server-finish");
    builder.text("protocol", &finish.protocol);
    builder.text("suite", &finish.suite);
    builder.bytes("receipt", &receipt);
    builder.i64("activated_at_ms", finish.activated_at_ms);
    builder.text("sender_role", role_name(finish.sender_role));
    builder.text("recipient_role", role_name(finish.recipient_role));
    builder.finish()
}

pub fn canonical_receipt(receipt: &EnrollmentReceipt) -> Vec<u8> {
    let mut builder = CanonicalBuilder::new("noted.direct-pairing.v1/receipt");
    builder.text("protocol", &receipt.protocol);
    builder.text("suite", &receipt.suite);
    builder.text("receipt_id", &receipt.receipt_id);
    builder.text("invitation_id", &receipt.invitation_id);
    builder.text("library_id", &receipt.library_id);
    builder.text("device_id", &receipt.device_id);
    builder.bytes(
        "client_signing_key_fingerprint",
        &receipt.client_signing_key_fingerprint,
    );
    builder.bytes(
        "client_hpke_key_fingerprint",
        &receipt.client_hpke_key_fingerprint,
    );
    builder.bytes(
        "mac_signing_key_fingerprint",
        &receipt.mac_signing_key_fingerprint,
    );
    builder.bytes(
        "mac_hpke_key_fingerprint",
        &receipt.mac_hpke_key_fingerprint,
    );
    builder.record_kinds("granted_scopes", &receipt.granted_scopes);
    builder.capabilities("capabilities", &receipt.capabilities);
    builder.u64("authority_generation", receipt.authority_generation);
    builder.i64("created_at_ms", receipt.created_at_ms);
    builder.i64("expires_at_ms", receipt.expires_at_ms);
    builder.bytes("transcript_digest", &receipt.transcript_digest);
    builder.text("environment", environment_name(receipt.environment));
    builder.text("mac_role", role_name(receipt.mac_role));
    builder.text("client_role", role_name(receipt.client_role));
    builder.finish()
}

fn pairing_transcript_digest(
    invitation_digest: &[u8],
    client_hello_digest: &[u8],
    server_nonce: &[u8],
    receipt_without_digest: &EnrollmentReceipt,
) -> Vec<u8> {
    let receipt = canonical_receipt(receipt_without_digest);
    sha256(&canonical_components(
        "noted.direct-pairing.v1/transcript",
        &[
            ("invitation_digest", invitation_digest),
            ("client_hello_digest", client_hello_digest),
            ("server_nonce", server_nonce),
            ("receipt_proposal", &receipt),
        ],
    ))
}

/// Deterministic, domain-separated fixture derivation. A production provider
/// must replace this with the reviewed HKDF-SHA256 exporter construction and
/// match cross-language golden vectors before the fixture-only gate is removed.
pub fn derive_verification_code(exporter_secret: &[u8], transcript_digest: &[u8]) -> String {
    let digest = sha256(&canonical_components(
        "noted.direct-pairing.v1/sas",
        &[
            ("exporter_secret", exporter_secret),
            ("transcript_digest", transcript_digest),
        ],
    ));
    let value =
        u64::from_be_bytes(digest[..8].try_into().expect("eight-byte digest prefix")) % 100_000_000;
    let digits = format!("{value:08}");
    format!("{} {}", &digits[..4], &digits[4..])
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn canonical_components(domain: &str, fields: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = CanonicalBuilder::new(domain);
    for (label, value) in fields {
        builder.bytes(label, value);
    }
    builder.finish()
}

struct CanonicalBuilder {
    bytes: Vec<u8>,
}

impl CanonicalBuilder {
    fn new(domain: &str) -> Self {
        let mut builder = Self { bytes: Vec::new() };
        builder.bytes("domain", domain.as_bytes());
        builder
    }

    fn bytes(&mut self, label: &str, value: &[u8]) {
        self.bytes
            .extend_from_slice(&(label.len() as u32).to_be_bytes());
        self.bytes.extend_from_slice(label.as_bytes());
        self.bytes
            .extend_from_slice(&(value.len() as u64).to_be_bytes());
        self.bytes.extend_from_slice(value);
    }

    fn text(&mut self, label: &str, value: &str) {
        self.bytes(label, value.as_bytes());
    }

    fn u64(&mut self, label: &str, value: u64) {
        self.bytes(label, &value.to_be_bytes());
    }

    fn i64(&mut self, label: &str, value: i64) {
        self.bytes(label, &value.to_be_bytes());
    }

    fn record_kinds(&mut self, label: &str, values: &BTreeSet<RecordKind>) {
        let mut nested = CanonicalBuilder::new("noted.direct-pairing.v1/record-kind-list");
        nested.u64("count", values.len() as u64);
        for value in values {
            nested.text("kind", record_kind_name(*value));
        }
        self.bytes(label, &nested.finish());
    }

    fn capabilities(&mut self, label: &str, values: &BTreeMap<RecordKind, KindCapability>) {
        let mut nested = CanonicalBuilder::new("noted.direct-pairing.v1/capabilities");
        nested.u64("count", values.len() as u64);
        for (kind, capability) in values {
            nested.text("kind", record_kind_name(*kind));
            nested.u64("reader_version", capability.reader_version as u64);
            match capability.writer_version {
                Some(writer) => {
                    nested.text("writer_present", "true");
                    nested.u64("writer_version", writer as u64);
                }
                None => nested.text("writer_present", "false"),
            }
        }
        self.bytes(label, &nested.finish());
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn environment_name(value: Environment) -> &'static str {
    match value {
        Environment::Development => "development",
        Environment::Production => "production",
    }
}

fn role_name(value: PairingRole) -> &'static str {
    match value {
        PairingRole::MacAuthority => "mac_authority",
        PairingRole::IphoneCompanion => "iphone_companion",
    }
}

fn data_class_name(value: LibraryDataClass) -> &'static str {
    match value {
        LibraryDataClass::SanitizedFixture => "sanitized_fixture",
        LibraryDataClass::Personal => "personal",
    }
}

fn record_kind_name(value: RecordKind) -> &'static str {
    match value {
        RecordKind::Note => "note",
        RecordKind::Category => "category",
        RecordKind::Folder => "folder",
        RecordKind::Media => "media",
    }
}

pub fn parse_bounded_json<T: DeserializeOwned>(
    bytes: &[u8],
    content_encoding: Option<&str>,
) -> Result<T, PairingError> {
    if !matches!(content_encoding, None | Some("identity")) {
        return Err(PairingError::UnsupportedEncoding);
    }
    if bytes.len() > MAX_PAIRING_MESSAGE_BYTES {
        return Err(PairingError::PayloadTooLarge);
    }
    let budget = Arc::new(Mutex::new(ParseBudget::default()));
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = BoundedValueSeed {
        depth: 0,
        budget: Arc::clone(&budget),
    }
    .deserialize(&mut deserializer)
    .map_err(|error| PairingError::ParseRejected(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| PairingError::ParseRejected(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| PairingError::ParseRejected(error.to_string()))
}

#[derive(Default)]
struct ParseBudget {
    members: usize,
    total_string_bytes: usize,
}

struct BoundedValueSeed {
    depth: usize,
    budget: Arc<Mutex<ParseBudget>>,
}

impl<'de> DeserializeSeed<'de> for BoundedValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > MAX_JSON_DEPTH {
            return Err(D::Error::custom("JSON nesting limit exceeded"));
        }
        deserializer.deserialize_any(BoundedValueVisitor {
            depth: self.depth,
            budget: self.budget,
        })
    }
}

struct BoundedValueVisitor {
    depth: usize,
    budget: Arc<Mutex<ParseBudget>>,
}

impl<'de> Visitor<'de> for BoundedValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
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
        charge_string::<E>(&self.budget, value)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: DeError>(self, value: String) -> Result<Self::Value, E> {
        charge_string::<E>(&self.budget, &value)?;
        Ok(Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(BoundedValueSeed {
            depth: self.depth + 1,
            budget: Arc::clone(&self.budget),
        })? {
            if values.len() >= MAX_JSON_ARRAY_ELEMENTS {
                return Err(A::Error::custom("JSON array element limit exceeded"));
            }
            charge_member::<A::Error>(&self.budget)?;
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
            if values.len() >= MAX_JSON_OBJECT_MEMBERS {
                return Err(A::Error::custom("JSON object member limit exceeded"));
            }
            charge_string::<A::Error>(&self.budget, &key)?;
            charge_member::<A::Error>(&self.budget)?;
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON object member"));
            }
            let value = map.next_value_seed(BoundedValueSeed {
                depth: self.depth + 1,
                budget: Arc::clone(&self.budget),
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

fn charge_member<E: DeError>(budget: &Mutex<ParseBudget>) -> Result<(), E> {
    let mut budget = budget
        .lock()
        .map_err(|_| E::custom("JSON budget unavailable"))?;
    budget.members += 1;
    if budget.members > MAX_JSON_MEMBERS {
        return Err(E::custom("JSON total member limit exceeded"));
    }
    Ok(())
}

fn charge_string<E: DeError>(budget: &Mutex<ParseBudget>, value: &str) -> Result<(), E> {
    if value.len() > MAX_JSON_STRING_BYTES {
        return Err(E::custom("JSON string limit exceeded"));
    }
    let mut budget = budget
        .lock()
        .map_err(|_| E::custom("JSON budget unavailable"))?;
    budget.total_string_bytes += value.len();
    if budget.total_string_bytes > MAX_JSON_TOTAL_STRING_BYTES {
        return Err(E::custom("JSON total string limit exceeded"));
    }
    Ok(())
}
