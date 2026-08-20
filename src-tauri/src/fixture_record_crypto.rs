//! Strict record-crypto adapter for the sanitized direct-sync fixture.
//!
//! This module is intentionally available only with the explicit
//! `sanitized-development-fixtures` feature.  It connects the pure sync
//! envelope contract to the shared Apple-security record format while keeping
//! library and signing key bytes inside the fixture custody provider.

#![cfg(feature = "sanitized-development-fixtures")]

use crate::mobile_sync_runtime::{ActiveSyncProfile, MobileSyncCrypto, MobileSyncRuntimeError};
use crate::pairing_protocol::{Environment, LibraryDataClass};
use crate::portable::{canonical_json, ContextRecordV1, LifecycleState};
use crate::sync_protocol::{
    MutationDraft, MutationEnvelope, MutationOperation, PreparedTransaction, ProtocolError,
    SignedTransaction, SYNC_PROTOCOL_VERSION,
};
use noted_apple_security::{
    decode_record_ciphertext_v1, encode_record_ciphertext_v1, RecordCryptoContextV1,
    RecordCryptoOperationV1, RecordKindV1, SanitizedFixtureRecordCrypto,
    MAX_RECORD_PLAINTEXT_BYTES, RECORD_CIPHER_SUITE, RECORD_CRYPTO_CONTEXT_VERSION,
};
use sha2::{Digest, Sha256};
use std::fmt;

const P256_X963_PUBLIC_KEY_BYTES: usize = 65;
const P256_P1363_SIGNATURE_BYTES: usize = 64;
const CUSTODY_HANDLE_DOMAIN: &[u8] = b"noted.fixture-record-custody.v1";

/// Secret-free, deterministic reference to one in-memory fixture custody
/// instance.  It is derived only from authenticated public profile facts and
/// the public signing key.  It is deliberately not serializable: restoring a
/// fixture still requires the authenticated bootstrap and its native-owned
/// key package, never this handle alone.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FixtureCustodyHandle(String);

impl FixtureCustodyHandle {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn derive(profile: &ActiveSyncProfile, signing_public_key: &[u8]) -> Self {
        let mut digest = Sha256::new();
        append_component(&mut digest, b"domain", CUSTODY_HANDLE_DOMAIN);
        append_component(
            &mut digest,
            b"identity_handle",
            profile.identity_handle.as_bytes(),
        );
        append_component(&mut digest, b"library_id", profile.library_id.as_bytes());
        append_component(&mut digest, b"device_id", profile.device_id.as_bytes());
        append_component(
            &mut digest,
            b"authority_generation",
            &profile.authority_generation.to_be_bytes(),
        );
        append_component(
            &mut digest,
            b"purge_generation",
            &profile.purge_generation.to_be_bytes(),
        );
        append_component(&mut digest, b"key_epoch", &profile.key_epoch.to_be_bytes());
        append_component(&mut digest, b"signing_public_key", signing_public_key);
        Self(format!(
            "fixture-record-v1:{}",
            hex_lower(&digest.finalize())
        ))
    }
}

impl fmt::Debug for FixtureCustodyHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("FixtureCustodyHandle")
            .field(&self.0)
            .finish()
    }
}

/// Public inventory for diagnostics and restart reconciliation.  It contains
/// no library key or signing private key and intentionally has no Serde
/// implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixturePublicCustody {
    pub handle: FixtureCustodyHandle,
    pub signing_public_key: [u8; P256_X963_PUBLIC_KEY_BYTES],
}

/// A writer key that may be used by record open only after the containing
/// bootstrap/pull response and its exact writer directory were authenticated
/// by the authority.  Construction remains crate-private so transport-facing
/// code cannot deserialize an untrusted key directly into this capability.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorityAuthenticatedWriterKey {
    writer_device_id: String,
    signing_public_key: [u8; P256_X963_PUBLIC_KEY_BYTES],
    fixture_binding_sha256: [u8; 32],
}

impl AuthorityAuthenticatedWriterKey {
    /// Call only after `mobile_sync_runtime` has authenticated the response and
    /// validated the exact historical writer-key directory.
    pub(crate) fn from_validated_directory(
        profile: &ActiveSyncProfile,
        writer_device_id: &str,
        signing_public_key: &[u8],
    ) -> Result<Self, FixtureRecordCryptoError> {
        validate_fixture_profile(profile)?;
        let signing_public_key: [u8; P256_X963_PUBLIC_KEY_BYTES] = signing_public_key
            .try_into()
            .map_err(|_| FixtureRecordCryptoError::WriterKeyRejected)?;
        if signing_public_key[0] != 0x04 || !crate::portable::is_uuid_v7(writer_device_id) {
            return Err(FixtureRecordCryptoError::WriterKeyRejected);
        }
        Ok(Self {
            writer_device_id: writer_device_id.to_owned(),
            signing_public_key,
            fixture_binding_sha256: fixture_binding_sha256(profile),
        })
    }

    pub fn writer_device_id(&self) -> &str {
        &self.writer_device_id
    }

    pub fn signing_public_key(&self) -> &[u8; P256_X963_PUBLIC_KEY_BYTES] {
        &self.signing_public_key
    }
}

impl fmt::Debug for AuthorityAuthenticatedWriterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorityAuthenticatedWriterKey")
            .field("writer_device_id", &self.writer_device_id)
            .field(
                "signing_public_key_sha256",
                &hex_lower(&Sha256::digest(self.signing_public_key)),
            )
            .finish_non_exhaustive()
    }
}

/// In-memory fixture-only custody plus exact sync-envelope binding.
///
/// `SanitizedFixtureRecordCrypto` owns and zeroizes the library/signing secret
/// material.  This adapter stores only the provider and authenticated public
/// profile facts; neither type implements Serialize or exposes secret bytes.
pub struct SanitizedFixtureRecordCryptoAdapter {
    profile: ActiveSyncProfile,
    custody: SanitizedFixtureRecordCrypto,
    public_custody: FixturePublicCustody,
    fixture_binding_sha256: [u8; 32],
}

impl SanitizedFixtureRecordCryptoAdapter {
    pub fn new(
        profile: ActiveSyncProfile,
        custody: SanitizedFixtureRecordCrypto,
    ) -> Result<Self, FixtureRecordCryptoError> {
        validate_fixture_profile(&profile)?;
        let signing_public_key = custody.signing_public_key();
        let metadata = custody.bootstrap_metadata();
        if metadata.environment != "development"
            || metadata.library_data_class != "sanitized_fixture"
            || metadata.receipt_id != profile.receipt_id
            || metadata.library_id != profile.library_id
            || metadata.device_id != profile.device_id
            || metadata.authority_generation != profile.authority_generation
            || metadata.purge_generation != profile.purge_generation
            || metadata.key_epoch != profile.key_epoch
            || metadata.default_scope_id != profile.default_scope_id
            || metadata.durable_sync_spki_sha256 != profile.durable_sync_spki_sha256
            || signing_public_key.as_slice() != profile.device_signing_public_key.as_slice()
        {
            return Err(FixtureRecordCryptoError::CustodyProfileMismatch);
        }
        let public_custody = FixturePublicCustody {
            handle: FixtureCustodyHandle::derive(&profile, &signing_public_key),
            signing_public_key,
        };
        let fixture_binding_sha256 = fixture_binding_sha256(&profile);
        Ok(Self {
            profile,
            custody,
            public_custody,
            fixture_binding_sha256,
        })
    }

    pub fn public_custody(&self) -> &FixturePublicCustody {
        &self.public_custody
    }

    /// Seal one canonical portable record and return a sync draft whose
    /// ciphertext is the exact NRC1 container.  The caller must supply an
    /// empty ciphertext so ciphertext from a prior mutation can never be
    /// silently replaced or rebound.
    pub fn seal_draft(
        &self,
        mut draft: MutationDraft,
        record: &ContextRecordV1,
    ) -> Result<MutationDraft, FixtureRecordCryptoError> {
        validate_fixture_profile(&self.profile)?;
        if !draft.ciphertext.is_empty() {
            return Err(FixtureRecordCryptoError::DraftAlreadySealed);
        }
        let context = context_from_draft(&self.profile, &draft)?;
        validate_plaintext_binding(record, &context)?;
        let plaintext = canonical_record_bytes(record)?;
        if plaintext.len() > MAX_RECORD_PLAINTEXT_BYTES {
            return Err(FixtureRecordCryptoError::PlaintextTooLarge);
        }
        let sealed = self
            .custody
            .seal_record(&context, &plaintext)
            .map_err(FixtureRecordCryptoError::NativeCrypto)?;
        draft.ciphertext = encode_record_ciphertext_v1(&sealed, &context)
            .map_err(FixtureRecordCryptoError::NativeCrypto)?;
        Ok(draft)
    }

    /// Attach real P-256 P1363 outer signatures only after the manifest is
    /// frozen.  Before release, every member is opened again with the same
    /// authenticated public key, proving that the inner and outer writers are
    /// identical and that every canonical record is bound to its mutation.
    pub fn sign_prepared_transaction(
        &self,
        prepared: PreparedTransaction,
    ) -> Result<SignedTransaction, FixtureRecordCryptoError> {
        validate_fixture_profile(&self.profile)?;
        let signatures = prepared
            .signing_inputs()
            .into_iter()
            .map(|input| {
                self.custody
                    .sign_p256_p1363(&input.canonical_bytes)
                    .map(|signature| signature.to_vec())
                    .map_err(FixtureRecordCryptoError::NativeCrypto)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signed = prepared
            .attach_signatures(signatures)
            .map_err(FixtureRecordCryptoError::Protocol)?;
        self.validate_outbound_transaction(&signed)?;
        Ok(signed)
    }

    /// Verify both the outer mutation signature and the inner NRC1 record
    /// signature with the exact authority-authenticated historical writer key,
    /// then decrypt and parse one lossless canonical ContextRecordV1.
    pub fn open_envelope(
        &self,
        envelope: &MutationEnvelope,
        writer: &AuthorityAuthenticatedWriterKey,
    ) -> Result<ContextRecordV1, FixtureRecordCryptoError> {
        validate_fixture_profile(&self.profile)?;
        if writer.fixture_binding_sha256 != self.fixture_binding_sha256
            || writer.writer_device_id != envelope.device_id
        {
            return Err(FixtureRecordCryptoError::WriterKeyRejected);
        }
        if envelope.signature.len() != P256_P1363_SIGNATURE_BYTES
            || !SanitizedFixtureRecordCrypto::verify_p256_p1363(
                &writer.signing_public_key,
                &envelope.signing_bytes(),
                &envelope.signature,
            )
            .map_err(FixtureRecordCryptoError::NativeCrypto)?
        {
            return Err(FixtureRecordCryptoError::OuterSignatureRejected);
        }
        let context = context_from_envelope(&self.profile, envelope)?;
        let sealed = decode_record_ciphertext_v1(&envelope.ciphertext, &context)
            .map_err(FixtureRecordCryptoError::NativeCrypto)?;
        let opened = self
            .custody
            .open_record(&context, &sealed, &writer.signing_public_key)
            .map_err(FixtureRecordCryptoError::NativeCrypto)?;
        if opened.plaintext.len() > MAX_RECORD_PLAINTEXT_BYTES {
            return Err(FixtureRecordCryptoError::PlaintextTooLarge);
        }
        let record: ContextRecordV1 = serde_json::from_slice(&opened.plaintext)
            .map_err(|_| FixtureRecordCryptoError::InvalidCanonicalRecord)?;
        validate_plaintext_binding(&record, &context)?;
        if canonical_record_bytes(&record)? != opened.plaintext {
            return Err(FixtureRecordCryptoError::NonCanonicalRecord);
        }
        Ok(record)
    }

    fn validate_outbound_transaction(
        &self,
        transaction: &SignedTransaction,
    ) -> Result<(), FixtureRecordCryptoError> {
        let manifest = &transaction.manifest;
        if manifest.protocol_version != SYNC_PROTOCOL_VERSION
            || manifest.library_id != self.profile.library_id
            || manifest.device_id != self.profile.device_id
            || manifest.authority_generation != self.profile.authority_generation
            || manifest.purge_generation != self.profile.purge_generation
            || manifest.key_epoch != self.profile.key_epoch
        {
            return Err(FixtureRecordCryptoError::TransactionProfileMismatch);
        }
        let writer = AuthorityAuthenticatedWriterKey::from_validated_directory(
            &self.profile,
            &self.profile.device_id,
            &self.profile.device_signing_public_key,
        )?;
        for envelope in &transaction.members {
            self.open_envelope(envelope, &writer)?;
        }
        Ok(())
    }
}

impl fmt::Debug for SanitizedFixtureRecordCryptoAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SanitizedFixtureRecordCryptoAdapter")
            .field("library_id", &self.profile.library_id)
            .field("device_id", &self.profile.device_id)
            .field("custody_handle", &self.public_custody.handle)
            .finish_non_exhaustive()
    }
}

impl MobileSyncCrypto for SanitizedFixtureRecordCryptoAdapter {
    fn fresh_uuid_v7(&self) -> Result<String, MobileSyncRuntimeError> {
        self.validate_live_custody()?;
        let value = crate::portable::new_uuid_v7();
        if !crate::portable::is_uuid_v7(&value) {
            return Err(MobileSyncRuntimeError::NativeCryptoRejected);
        }
        Ok(value)
    }

    fn sign(
        &self,
        identity_handle: &str,
        message: &[u8],
    ) -> Result<Vec<u8>, MobileSyncRuntimeError> {
        self.validate_live_custody()?;
        if identity_handle != self.profile.identity_handle {
            return Err(MobileSyncRuntimeError::NativeCryptoRejected);
        }
        self.custody
            .sign_p256_p1363(message)
            .map(|signature| signature.to_vec())
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)
    }

    fn verify_p256_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<bool, MobileSyncRuntimeError> {
        self.validate_live_custody()?;
        if public_key.len() != P256_X963_PUBLIC_KEY_BYTES
            || public_key.first() != Some(&0x04)
            || signature.len() != P256_P1363_SIGNATURE_BYTES
        {
            return Err(MobileSyncRuntimeError::NativeCryptoRejected);
        }
        SanitizedFixtureRecordCrypto::verify_p256_p1363(public_key, message, signature)
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)
    }
}

impl SanitizedFixtureRecordCryptoAdapter {
    fn validate_live_custody(&self) -> Result<(), MobileSyncRuntimeError> {
        validate_fixture_profile(&self.profile)
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)?;
        let signing_public_key = self.custody.signing_public_key();
        let expected_handle = FixtureCustodyHandle::derive(&self.profile, &signing_public_key);
        if signing_public_key != self.public_custody.signing_public_key
            || expected_handle != self.public_custody.handle
            || self.fixture_binding_sha256 != fixture_binding_sha256(&self.profile)
        {
            return Err(MobileSyncRuntimeError::NativeCryptoRejected);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum FixtureRecordCryptoError {
    FixtureProfileRejected,
    CustodyProfileMismatch,
    WriterKeyRejected,
    OuterSignatureRejected,
    TransactionProfileMismatch,
    DraftAlreadySealed,
    UnsupportedRecordKind,
    InvalidMutationBinding,
    InvalidCanonicalRecord,
    NonCanonicalRecord,
    PlaintextTooLarge,
    Protocol(ProtocolError),
    NativeCrypto(noted_apple_security::Error),
}

impl fmt::Display for FixtureRecordCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::FixtureProfileRejected => "fixture record crypto rejected the active profile",
            Self::CustodyProfileMismatch => {
                "fixture record custody does not match the enrolled device"
            }
            Self::WriterKeyRejected => "writer key was not authenticated for this fixture",
            Self::OuterSignatureRejected => "outer mutation signature was rejected",
            Self::TransactionProfileMismatch => {
                "transaction does not match the active fixture profile"
            }
            Self::DraftAlreadySealed => "mutation draft already contains ciphertext",
            Self::UnsupportedRecordKind => "record kind is not supported by record crypto v1",
            Self::InvalidMutationBinding => "mutation fields do not match the record binding",
            Self::InvalidCanonicalRecord => "decrypted record is invalid",
            Self::NonCanonicalRecord => "decrypted record is not canonically encoded",
            Self::PlaintextTooLarge => "canonical record exceeds the record-crypto limit",
            Self::Protocol(_) => "sync transaction construction failed",
            Self::NativeCrypto(_) => "native fixture record cryptography failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for FixtureRecordCryptoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::NativeCrypto(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_fixture_profile(profile: &ActiveSyncProfile) -> Result<(), FixtureRecordCryptoError> {
    if profile.environment != Environment::Development
        || profile.library_data_class != LibraryDataClass::SanitizedFixture
        || profile.validate_fixture().is_err()
    {
        return Err(FixtureRecordCryptoError::FixtureProfileRejected);
    }
    Ok(())
}

fn context_from_draft(
    profile: &ActiveSyncProfile,
    draft: &MutationDraft,
) -> Result<RecordCryptoContextV1, FixtureRecordCryptoError> {
    let context = RecordCryptoContextV1 {
        version: RECORD_CRYPTO_CONTEXT_VERSION,
        cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
        library_id: profile.library_id.clone(),
        record_id: draft.record_id.clone(),
        record_kind: map_record_kind(&draft.record_kind)?,
        schema_version: draft.record_schema_version,
        base_revision: draft.base_head_revision,
        base_version_id: draft.base_head_version_id.clone(),
        proposed_revision: draft.proposed_revision,
        version_id: draft.version_id.clone(),
        mutation_id: draft.mutation_id.clone(),
        authority_generation: profile.authority_generation,
        purge_generation: profile.purge_generation,
        key_epoch: profile.key_epoch,
        operation: map_operation(draft.operation),
    };
    context
        .validate()
        .map_err(FixtureRecordCryptoError::NativeCrypto)?;
    Ok(context)
}

fn context_from_envelope(
    profile: &ActiveSyncProfile,
    envelope: &MutationEnvelope,
) -> Result<RecordCryptoContextV1, FixtureRecordCryptoError> {
    if envelope.protocol_version != SYNC_PROTOCOL_VERSION
        || envelope.library_id != profile.library_id
        || envelope.authority_generation != profile.authority_generation
        || envelope.purge_generation != profile.purge_generation
        || envelope.key_epoch != profile.key_epoch
        || envelope.ciphertext_hash != hex_lower(&Sha256::digest(&envelope.ciphertext))
    {
        return Err(FixtureRecordCryptoError::InvalidMutationBinding);
    }
    let context = RecordCryptoContextV1 {
        version: RECORD_CRYPTO_CONTEXT_VERSION,
        cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
        library_id: envelope.library_id.clone(),
        record_id: envelope.record_id.clone(),
        record_kind: map_record_kind(&envelope.record_kind)?,
        schema_version: envelope.record_schema_version,
        base_revision: envelope.base_head_revision,
        base_version_id: envelope.base_head_version_id.clone(),
        proposed_revision: envelope.proposed_revision,
        version_id: envelope.version_id.clone(),
        mutation_id: envelope.mutation_id.clone(),
        authority_generation: envelope.authority_generation,
        purge_generation: envelope.purge_generation,
        key_epoch: envelope.key_epoch,
        operation: map_operation(envelope.operation),
    };
    context
        .validate()
        .map_err(FixtureRecordCryptoError::NativeCrypto)?;
    Ok(context)
}

fn validate_plaintext_binding(
    record: &ContextRecordV1,
    context: &RecordCryptoContextV1,
) -> Result<(), FixtureRecordCryptoError> {
    record
        .validate()
        .map_err(|_| FixtureRecordCryptoError::InvalidCanonicalRecord)?;
    if record.library_id != context.library_id
        || record.record_id != context.record_id
        || record.kind != context.record_kind.as_str()
        || record.record_schema_version != context.schema_version
        || record.revision != context.proposed_revision
        || record.version_id != context.version_id
        || match context.operation {
            RecordCryptoOperationV1::Delete => record.lifecycle.state != LifecycleState::Tombstone,
            RecordCryptoOperationV1::Create | RecordCryptoOperationV1::Update => {
                record.lifecycle.state == LifecycleState::Tombstone
            }
        }
    {
        return Err(FixtureRecordCryptoError::InvalidMutationBinding);
    }
    Ok(())
}

fn canonical_record_bytes(record: &ContextRecordV1) -> Result<Vec<u8>, FixtureRecordCryptoError> {
    let value = serde_json::to_value(record)
        .map_err(|_| FixtureRecordCryptoError::InvalidCanonicalRecord)?;
    Ok(canonical_json(&value).into_bytes())
}

fn map_record_kind(kind: &str) -> Result<RecordKindV1, FixtureRecordCryptoError> {
    match kind {
        "note" => Ok(RecordKindV1::Note),
        "category" => Ok(RecordKindV1::Category),
        "folder" => Ok(RecordKindV1::Folder),
        _ => Err(FixtureRecordCryptoError::UnsupportedRecordKind),
    }
}

fn map_operation(operation: MutationOperation) -> RecordCryptoOperationV1 {
    match operation {
        MutationOperation::Create => RecordCryptoOperationV1::Create,
        MutationOperation::Update => RecordCryptoOperationV1::Update,
        MutationOperation::Delete => RecordCryptoOperationV1::Delete,
    }
}

fn fixture_binding_sha256(profile: &ActiveSyncProfile) -> [u8; 32] {
    let mut digest = Sha256::new();
    append_component(&mut digest, b"domain", b"noted.fixture-record-binding.v1");
    append_component(&mut digest, b"library_id", profile.library_id.as_bytes());
    append_component(
        &mut digest,
        b"authority_generation",
        &profile.authority_generation.to_be_bytes(),
    );
    append_component(
        &mut digest,
        b"purge_generation",
        &profile.purge_generation.to_be_bytes(),
    );
    append_component(&mut digest, b"key_epoch", &profile.key_epoch.to_be_bytes());
    append_component(
        &mut digest,
        b"authority_signing_public_key",
        &profile.authority_signing_public_key,
    );
    digest.finalize().into()
}

fn append_component(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u32).to_be_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_sync::{
        request_signing_bytes, CheckpointRequest, DirectEndpoint, DirectSyncLimits,
        SignedSyncRequest,
    };
    use crate::mobile_sync_runtime::{
        prepare_signed_request, ActiveSyncProfile, ExactRequestPurpose, MobileSyncCrypto,
    };
    use crate::pairing_protocol::{
        fixture_record_capabilities, fixture_record_scopes, Environment, LibraryDataClass,
    };
    use crate::portable::{
        AuthorityKind, RecordAuthority, RecordLifecycle, RecordScope, ScopeClass,
    };
    use crate::sync_protocol::{MutationDraft, TransactionHeader};
    use noted_apple_security::{
        BootstrapCapabilityV1, BootstrapMetadataV1, SanitizedFixtureRecordCrypto,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use zeroize::Zeroizing;

    const LIBRARY_ID: &str = "00000000-0000-7000-8000-000000000001";
    const DEVICE_ID: &str = "00000000-0000-7000-8000-000000000002";
    const OTHER_DEVICE_ID: &str = "00000000-0000-7000-8000-000000000003";
    const SCOPE_ID: &str = "00000000-0000-7000-8000-000000000004";
    const RECORD_ID: &str = "00000000-0000-7000-8000-000000000005";
    const VERSION_ID: &str = "00000000-0000-7000-8000-000000000006";
    const MUTATION_ID: &str = "00000000-0000-7000-8000-000000000007";
    const TRANSACTION_ID: &str = "00000000-0000-7000-8000-000000000008";
    const RECEIPT_ID: &str = "00000000-0000-7000-8000-000000000009";

    fn profile(signing_public_key: Vec<u8>) -> ActiveSyncProfile {
        ActiveSyncProfile {
            identity_handle: "10000000-0000-4000-8000-000000000001".to_owned(),
            receipt_id: RECEIPT_ID.to_owned(),
            activation_sha256: "11".repeat(32),
            library_id: LIBRARY_ID.to_owned(),
            device_id: DEVICE_ID.to_owned(),
            default_scope_id: SCOPE_ID.to_owned(),
            authority_generation: 3,
            purge_generation: 2,
            key_epoch: 4,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            durable_sync_spki_sha256: [0x22; 32],
            device_signing_public_key: signing_public_key,
            authority_signing_public_key: fixture_crypto([0x41; 32]).signing_public_key().to_vec(),
            granted_scopes: fixture_record_scopes(),
            capabilities: fixture_record_capabilities(),
            revoked: false,
        }
    }

    fn native_metadata(device_id: &str) -> BootstrapMetadataV1 {
        BootstrapMetadataV1 {
            version: 1,
            protocol: "noted.direct-pairing.v1".to_owned(),
            suite: "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm".to_owned(),
            sync_protocol_version: 1,
            environment: "development".to_owned(),
            library_data_class: "sanitized_fixture".to_owned(),
            receipt_id: RECEIPT_ID.to_owned(),
            library_id: LIBRARY_ID.to_owned(),
            device_id: device_id.to_owned(),
            authority_generation: 3,
            purge_generation: 2,
            key_epoch: 4,
            default_scope_id: SCOPE_ID.to_owned(),
            default_scope_class: "unknown".to_owned(),
            granted_scopes: vec![
                "note".to_owned(),
                "category".to_owned(),
                "folder".to_owned(),
            ],
            capabilities: BTreeMap::from([
                (
                    "note".to_owned(),
                    BootstrapCapabilityV1 {
                        reader_version: 1,
                        writer_version: Some(1),
                    },
                ),
                (
                    "category".to_owned(),
                    BootstrapCapabilityV1 {
                        reader_version: 1,
                        writer_version: Some(1),
                    },
                ),
                (
                    "folder".to_owned(),
                    BootstrapCapabilityV1 {
                        reader_version: 1,
                        writer_version: Some(1),
                    },
                ),
            ]),
            record_cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            durable_sync_spki_sha256: [0x22; 32],
            transcript_digest: [0x33; 32],
        }
    }

    fn fixture_crypto(signing_key: [u8; 32]) -> SanitizedFixtureRecordCrypto {
        SanitizedFixtureRecordCrypto::new(
            native_metadata(DEVICE_ID),
            Zeroizing::new([0x31; 32]),
            Zeroizing::new(signing_key),
        )
        .expect("valid fixture crypto")
    }

    fn adapter() -> SanitizedFixtureRecordCryptoAdapter {
        let crypto = fixture_crypto([0x51; 32]);
        let profile = profile(crypto.signing_public_key().to_vec());
        SanitizedFixtureRecordCryptoAdapter::new(profile, crypto).expect("fixture adapter")
    }

    fn record(content: serde_json::Value) -> ContextRecordV1 {
        ContextRecordV1::new(
            LIBRARY_ID.to_owned(),
            RECORD_ID.to_owned(),
            "note".to_owned(),
            1,
            1,
            VERSION_ID.to_owned(),
            "2026-08-17T12:00:00Z".to_owned(),
            "2026-08-17T12:00:00Z".to_owned(),
            None,
            RecordScope {
                scope_id: SCOPE_ID.to_owned(),
                class: ScopeClass::Unknown,
            },
            "standard".to_owned(),
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: None,
            },
            content,
            json!({"fixture": true}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )
        .expect("valid fixture record")
    }

    fn draft() -> MutationDraft {
        MutationDraft {
            mutation_id: MUTATION_ID.to_owned(),
            operation: MutationOperation::Create,
            record_id: RECORD_ID.to_owned(),
            record_kind: "note".to_owned(),
            record_schema_version: 1,
            base_head_revision: 0,
            base_head_version_id: None,
            proposed_revision: 1,
            version_id: VERSION_ID.to_owned(),
            ciphertext: Vec::new(),
        }
    }

    fn signed_envelope(
        adapter: &SanitizedFixtureRecordCryptoAdapter,
        draft: MutationDraft,
    ) -> MutationEnvelope {
        adapter
            .sign_prepared_transaction(prepared_transaction(draft))
            .expect("signed transaction")
            .members
            .into_iter()
            .next()
            .expect("member")
    }

    fn prepared_transaction(draft: MutationDraft) -> PreparedTransaction {
        PreparedTransaction::prepare(
            TransactionHeader {
                protocol_version: 1,
                library_id: LIBRARY_ID.to_owned(),
                transaction_id: TRANSACTION_ID.to_owned(),
                device_id: DEVICE_ID.to_owned(),
                device_transaction_counter: 1,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 4,
            },
            vec![draft],
            10_000,
        )
        .expect("prepared transaction")
    }

    fn signed_envelope_without_outbound_reopen(
        adapter: &SanitizedFixtureRecordCryptoAdapter,
        prepared: PreparedTransaction,
    ) -> MutationEnvelope {
        let signatures = prepared
            .signing_inputs()
            .into_iter()
            .map(|input| {
                adapter
                    .custody
                    .sign_p256_p1363(&input.canonical_bytes)
                    .expect("outer fixture signature")
                    .to_vec()
            })
            .collect();
        prepared
            .attach_signatures(signatures)
            .expect("attach signatures")
            .members
            .into_iter()
            .next()
            .expect("member")
    }

    fn writer_key(
        adapter: &SanitizedFixtureRecordCryptoAdapter,
    ) -> AuthorityAuthenticatedWriterKey {
        AuthorityAuthenticatedWriterKey::from_validated_directory(
            &adapter.profile,
            DEVICE_ID,
            &adapter.public_custody.signing_public_key,
        )
        .expect("authenticated writer key")
    }

    #[test]
    fn canonical_record_round_trips_through_inner_and_outer_signatures() {
        let adapter = adapter();
        let expected = record(json!({"body": "fixture", "title": "Hello"}));
        let sealed = adapter.seal_draft(draft(), &expected).expect("seal");
        assert!(sealed.ciphertext.starts_with(b"NRC1"));
        let envelope = signed_envelope(&adapter, sealed);
        assert_eq!(envelope.signature.len(), P256_P1363_SIGNATURE_BYTES);
        let opened = adapter
            .open_envelope(&envelope, &writer_key(&adapter))
            .expect("open");
        assert_eq!(opened, expected);
    }

    #[test]
    fn repeated_seal_uses_a_fresh_nonce() {
        let adapter = adapter();
        let value = record(json!({"title": "same plaintext"}));
        let first = adapter
            .seal_draft(draft(), &value)
            .expect("first seal")
            .ciphertext;
        let second = adapter
            .seal_draft(draft(), &value)
            .expect("second seal")
            .ciphertext;
        assert_ne!(first, second);
        assert_ne!(&first[12..24], &second[12..24]);
    }

    #[test]
    fn tampered_outer_or_inner_envelope_fails_closed() {
        let adapter = adapter();
        let value = record(json!({"title": "tamper target"}));
        let sealed = adapter.seal_draft(draft(), &value).expect("seal");
        let envelope = signed_envelope(&adapter, sealed.clone());

        let mut outer_tamper = envelope.clone();
        let last = outer_tamper.ciphertext.len() - 1;
        outer_tamper.ciphertext[last] ^= 1;
        assert!(matches!(
            adapter.open_envelope(&outer_tamper, &writer_key(&adapter)),
            Err(FixtureRecordCryptoError::OuterSignatureRejected)
        ));

        let mut inner_tamper = sealed;
        let last = inner_tamper.ciphertext.len() - 1;
        inner_tamper.ciphertext[last] ^= 1;
        let prepared = prepared_transaction(inner_tamper);
        assert!(matches!(
            adapter.sign_prepared_transaction(prepared.clone()),
            Err(FixtureRecordCryptoError::NativeCrypto(_))
        ));
        let tampered_envelope = signed_envelope_without_outbound_reopen(&adapter, prepared);
        assert!(matches!(
            adapter.open_envelope(&tampered_envelope, &writer_key(&adapter)),
            Err(FixtureRecordCryptoError::NativeCrypto(_))
        ));
    }

    #[test]
    fn wrong_or_cross_fixture_writer_key_is_rejected() {
        let adapter = adapter();
        let value = record(json!({"title": "writer binding"}));
        let envelope =
            signed_envelope(&adapter, adapter.seal_draft(draft(), &value).expect("seal"));

        let wrong_crypto = fixture_crypto([0x61; 32]);
        let wrong = AuthorityAuthenticatedWriterKey::from_validated_directory(
            &adapter.profile,
            DEVICE_ID,
            &wrong_crypto.signing_public_key(),
        )
        .expect("well-shaped wrong key");
        assert!(matches!(
            adapter.open_envelope(&envelope, &wrong),
            Err(FixtureRecordCryptoError::OuterSignatureRejected)
        ));

        let different_writer = AuthorityAuthenticatedWriterKey::from_validated_directory(
            &adapter.profile,
            OTHER_DEVICE_ID,
            &adapter.public_custody.signing_public_key,
        )
        .expect("other writer key");
        assert!(matches!(
            adapter.open_envelope(&envelope, &different_writer),
            Err(FixtureRecordCryptoError::WriterKeyRejected)
        ));
    }

    #[test]
    fn size_operation_and_record_bindings_fail_closed() {
        let adapter = adapter();
        let oversized = record(json!({"body": "x".repeat(MAX_RECORD_PLAINTEXT_BYTES)}));
        assert!(matches!(
            adapter.seal_draft(draft(), &oversized),
            Err(FixtureRecordCryptoError::PlaintextTooLarge)
        ));

        let mut mismatched = record(json!({"title": "wrong revision"}));
        mismatched.revision = 2;
        assert!(matches!(
            adapter.seal_draft(draft(), &mismatched),
            Err(FixtureRecordCryptoError::InvalidMutationBinding)
        ));

        let mut delete = draft();
        delete.operation = MutationOperation::Delete;
        delete.base_head_revision = 1;
        delete.base_head_version_id = Some("00000000-0000-7000-8000-000000000010".to_owned());
        delete.proposed_revision = 2;
        delete.version_id = "00000000-0000-7000-8000-000000000011".to_owned();
        delete.mutation_id = "00000000-0000-7000-8000-000000000012".to_owned();
        assert!(matches!(
            adapter.seal_draft(delete, &record(json!({"title": "not tombstoned"}))),
            Err(FixtureRecordCryptoError::InvalidMutationBinding)
        ));
    }

    #[test]
    fn production_personal_and_mismatched_custody_are_rejected() {
        let crypto = fixture_crypto([0x71; 32]);
        let mut rejected = profile(crypto.signing_public_key().to_vec());
        rejected.environment = Environment::Production;
        rejected.library_data_class = LibraryDataClass::Personal;
        assert!(matches!(
            SanitizedFixtureRecordCryptoAdapter::new(rejected, crypto),
            Err(FixtureRecordCryptoError::FixtureProfileRejected)
        ));

        let crypto = fixture_crypto([0x72; 32]);
        let rejected = profile(vec![0x04; P256_X963_PUBLIC_KEY_BYTES]);
        assert!(matches!(
            SanitizedFixtureRecordCryptoAdapter::new(rejected, crypto),
            Err(FixtureRecordCryptoError::CustodyProfileMismatch)
        ));

        let crypto = SanitizedFixtureRecordCrypto::new(
            native_metadata(OTHER_DEVICE_ID),
            Zeroizing::new([0x31; 32]),
            Zeroizing::new([0x73; 32]),
        )
        .expect("valid but differently bound fixture crypto");
        let rejected = profile(crypto.signing_public_key().to_vec());
        assert!(matches!(
            SanitizedFixtureRecordCryptoAdapter::new(rejected, crypto),
            Err(FixtureRecordCryptoError::CustodyProfileMismatch)
        ));
    }

    #[test]
    fn custody_handle_is_deterministic_and_secret_free() {
        let first = adapter();
        let second = adapter();
        assert_eq!(first.public_custody.handle, second.public_custody.handle);
        assert!(first
            .public_custody
            .handle
            .as_str()
            .starts_with("fixture-record-v1:"));
        let diagnostic = format!("{first:?}");
        assert!(!diagnostic.contains(&hex_lower(&[0x31; 32])));
        assert!(!diagnostic.contains(&hex_lower(&[0x51; 32])));
    }

    #[test]
    fn mobile_sync_crypto_rejects_the_wrong_identity_handle() {
        let adapter = adapter();
        assert!(matches!(
            MobileSyncCrypto::sign(&adapter, "10000000-0000-4000-8000-000000000099", b"request"),
            Err(MobileSyncRuntimeError::NativeCryptoRejected)
        ));
        let signature =
            MobileSyncCrypto::sign(&adapter, &adapter.profile.identity_handle, b"request")
                .expect("bound identity signs");
        assert_eq!(signature.len(), P256_P1363_SIGNATURE_BYTES);
        assert!(MobileSyncCrypto::verify_p256_signature(
            &adapter,
            &adapter.profile.device_signing_public_key,
            b"request",
            &signature,
        )
        .expect("valid signature shape"));
    }

    #[test]
    fn mobile_sync_request_uses_real_custody_signature() {
        let adapter = adapter();
        let purpose = ExactRequestPurpose::Checkpoint {
            known_cursor: Some(9),
        };
        let prepared = prepare_signed_request(
            &adapter.profile,
            &adapter,
            &DirectSyncLimits::default(),
            &purpose,
            CheckpointRequest {
                known_cursor: Some(9),
            },
        )
        .expect("signed checkpoint request");
        assert_eq!(prepared.endpoint, DirectEndpoint::Checkpoint);
        assert!(crate::portable::is_uuid_v7(&prepared.request_id));

        let request: SignedSyncRequest<CheckpointRequest> =
            serde_json::from_slice(&prepared.exact_body).expect("exact request");
        assert_eq!(request.payload.known_cursor, Some(9));
        let signing_bytes = request_signing_bytes(DirectEndpoint::Checkpoint, &request)
            .expect("request signing bytes");
        assert!(SanitizedFixtureRecordCrypto::verify_p256_p1363(
            &adapter.profile.device_signing_public_key,
            &signing_bytes,
            &request.signature,
        )
        .expect("real P-256 request signature"));
    }
}
