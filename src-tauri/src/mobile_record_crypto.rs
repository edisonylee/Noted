//! Phone-side canonical record cryptography boundary.
//!
//! The sync orchestrator deals only in complete canonical `ContextRecordV1`
//! bytes and opaque mutation ciphertext. Implementations keep library and
//! signing keys in native custody. The shared validation in this module makes
//! the outer signature, NRC1 context, and decrypted portable record describe
//! exactly the same mutation before any caller may apply it.

use crate::mobile_sync_runtime::{ActiveSyncProfile, MobileSyncCrypto};
#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
use crate::portable::{canonical_json, ContextRecordV1, LifecycleState};
#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
use crate::sync_protocol::MutationOperation;
#[cfg(target_os = "ios")]
use crate::sync_protocol::SYNC_PROTOCOL_VERSION;
use crate::sync_protocol::{
    MutationDraft, MutationEnvelope, PreparedTransaction, ProtocolError, SignedTransaction,
};
#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
use noted_apple_security::{
    RecordCryptoContextV1, RecordCryptoOperationV1, RecordKindV1, MAX_RECORD_PLAINTEXT_BYTES,
    RECORD_CIPHER_SUITE, RECORD_CRYPTO_CONTEXT_VERSION,
};
#[cfg(target_os = "ios")]
use sha2::{Digest, Sha256};
use std::fmt;

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures"))]
const P256_X963_PUBLIC_KEY_BYTES: usize = 65;
#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures"))]
const P256_P1363_SIGNATURE_BYTES: usize = 64;

/// Native-only operations required above the exact signed-request journal.
///
/// Implementations must never return a library key or signing private key. A
/// caller may persist only the returned NRC1 ciphertext, public writer key,
/// and the canonical plaintext already destined for the protected local
/// replica.
pub trait MobileRecordCrypto: MobileSyncCrypto {
    /// Retire native private material only after authenticated revocation is
    /// durable. Fixture implementations may keep the default no-op; production
    /// iPhone custody overrides it with an idempotent Keychain tombstone.
    fn retire_active_identity(
        &self,
        _profile: &ActiveSyncProfile,
    ) -> Result<(), MobileRecordCryptoError> {
        Ok(())
    }

    fn seal_canonical_record(
        &self,
        profile: &ActiveSyncProfile,
        draft: MutationDraft,
        canonical_record_bytes: &[u8],
    ) -> Result<MutationDraft, MobileRecordCryptoError>;

    fn sign_prepared_transaction(
        &self,
        profile: &ActiveSyncProfile,
        prepared: PreparedTransaction,
    ) -> Result<SignedTransaction, MobileRecordCryptoError>;

    /// `writer_public_key` must come from the exact authority-authenticated
    /// bootstrap/pull response containing this envelope. Implementations
    /// independently verify the outer and inner signatures before returning.
    fn open_canonical_record(
        &self,
        profile: &ActiveSyncProfile,
        envelope: &MutationEnvelope,
        writer_public_key: &[u8],
    ) -> Result<Vec<u8>, MobileRecordCryptoError>;
}

#[derive(Debug)]
pub enum MobileRecordCryptoError {
    InvalidProfile,
    InvalidCanonicalRecord,
    NonCanonicalRecord,
    InvalidMutationBinding,
    UnsupportedRecordKind,
    PlaintextTooLarge,
    OuterSignatureRejected,
    NativeCryptoRejected,
    Protocol(ProtocolError),
}

impl fmt::Display for MobileRecordCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidProfile => "record crypto rejected the active sync profile",
            Self::InvalidCanonicalRecord => "record crypto rejected the canonical record",
            Self::NonCanonicalRecord => "record bytes are not canonical JSON",
            Self::InvalidMutationBinding => "record and mutation bindings do not match",
            Self::UnsupportedRecordKind => "record kind is unsupported by record crypto v1",
            Self::PlaintextTooLarge => "canonical record exceeds the record-crypto limit",
            Self::OuterSignatureRejected => "mutation writer signature was rejected",
            Self::NativeCryptoRejected => "native record cryptography rejected the operation",
            Self::Protocol(_) => "sync transaction construction failed",
        })
    }
}

impl std::error::Error for MobileRecordCryptoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
pub(crate) fn validate_record_profile(
    profile: &ActiveSyncProfile,
) -> Result<(), MobileRecordCryptoError> {
    // M4 deliberately supports only authenticated sanitized fixtures. The
    // production personal-library policy remains closed until external review.
    profile
        .validate_fixture()
        .map_err(|_| MobileRecordCryptoError::InvalidProfile)
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
pub(crate) fn parse_canonical_record(
    bytes: &[u8],
) -> Result<ContextRecordV1, MobileRecordCryptoError> {
    if bytes.is_empty() || bytes.len() > MAX_RECORD_PLAINTEXT_BYTES {
        return Err(MobileRecordCryptoError::PlaintextTooLarge);
    }
    let record: ContextRecordV1 = serde_json::from_slice(bytes)
        .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
    record
        .validate()
        .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
    let value = serde_json::to_value(&record)
        .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
    if canonical_json(&value).as_bytes() != bytes {
        return Err(MobileRecordCryptoError::NonCanonicalRecord);
    }
    Ok(record)
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
pub(crate) fn context_from_draft(
    profile: &ActiveSyncProfile,
    draft: &MutationDraft,
) -> Result<RecordCryptoContextV1, MobileRecordCryptoError> {
    validate_record_profile(profile)?;
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
        .map_err(|_| MobileRecordCryptoError::InvalidMutationBinding)?;
    Ok(context)
}

#[cfg(target_os = "ios")]
pub(crate) fn context_from_envelope(
    profile: &ActiveSyncProfile,
    envelope: &MutationEnvelope,
) -> Result<RecordCryptoContextV1, MobileRecordCryptoError> {
    validate_record_profile(profile)?;
    if envelope.protocol_version != SYNC_PROTOCOL_VERSION
        || envelope.library_id != profile.library_id
        || envelope.authority_generation != profile.authority_generation
        || envelope.purge_generation != profile.purge_generation
        || envelope.key_epoch != profile.key_epoch
        || envelope.ciphertext_hash != sha256_hex(&envelope.ciphertext)
    {
        return Err(MobileRecordCryptoError::InvalidMutationBinding);
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
        .map_err(|_| MobileRecordCryptoError::InvalidMutationBinding)?;
    Ok(context)
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
pub(crate) fn validate_record_binding(
    record: &ContextRecordV1,
    context: &RecordCryptoContextV1,
) -> Result<(), MobileRecordCryptoError> {
    record
        .validate()
        .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
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
        return Err(MobileRecordCryptoError::InvalidMutationBinding);
    }
    Ok(())
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures"))]
pub(crate) fn validate_writer_and_outer_signature(
    crypto: &impl MobileSyncCrypto,
    envelope: &MutationEnvelope,
    writer_public_key: &[u8],
) -> Result<(), MobileRecordCryptoError> {
    if writer_public_key.len() != P256_X963_PUBLIC_KEY_BYTES
        || writer_public_key.first() != Some(&0x04)
        || envelope.signature.len() != P256_P1363_SIGNATURE_BYTES
        || !crypto
            .verify_p256_signature(
                writer_public_key,
                &envelope.signing_bytes(),
                &envelope.signature,
            )
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?
    {
        return Err(MobileRecordCryptoError::OuterSignatureRejected);
    }
    Ok(())
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
fn map_record_kind(kind: &str) -> Result<RecordKindV1, MobileRecordCryptoError> {
    match kind {
        "note" => Ok(RecordKindV1::Note),
        "category" => Ok(RecordKindV1::Category),
        "folder" => Ok(RecordKindV1::Folder),
        _ => Err(MobileRecordCryptoError::UnsupportedRecordKind),
    }
}

#[cfg(any(target_os = "ios", feature = "sanitized-development-fixtures", test))]
fn map_operation(operation: MutationOperation) -> RecordCryptoOperationV1 {
    match operation {
        MutationOperation::Create => RecordCryptoOperationV1::Create,
        MutationOperation::Update => RecordCryptoOperationV1::Update,
        MutationOperation::Delete => RecordCryptoOperationV1::Delete,
    }
}

#[cfg(target_os = "ios")]
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(feature = "sanitized-development-fixtures")]
impl MobileRecordCrypto for crate::fixture_record_crypto::SanitizedFixtureRecordCryptoAdapter {
    fn seal_canonical_record(
        &self,
        profile: &ActiveSyncProfile,
        draft: MutationDraft,
        canonical_record_bytes: &[u8],
    ) -> Result<MutationDraft, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        let record = parse_canonical_record(canonical_record_bytes)?;
        let context = context_from_draft(profile, &draft)?;
        validate_record_binding(&record, &context)?;
        self.seal_draft(draft, &record)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)
    }

    fn sign_prepared_transaction(
        &self,
        profile: &ActiveSyncProfile,
        prepared: PreparedTransaction,
    ) -> Result<SignedTransaction, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        self.sign_prepared_transaction(prepared)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)
    }

    fn open_canonical_record(
        &self,
        profile: &ActiveSyncProfile,
        envelope: &MutationEnvelope,
        writer_public_key: &[u8],
    ) -> Result<Vec<u8>, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        validate_writer_and_outer_signature(self, envelope, writer_public_key)?;
        let writer = crate::fixture_record_crypto::AuthorityAuthenticatedWriterKey::from_validated_directory(
            profile,
            &envelope.device_id,
            writer_public_key,
        )
        .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let record = self
            .open_envelope(envelope, &writer)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let value = serde_json::to_value(record)
            .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
        Ok(canonical_json(&value).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing_protocol::{
        fixture_record_capabilities, fixture_record_scopes, Environment, LibraryDataClass,
    };
    use crate::portable::{
        AuthorityKind, RecordAuthority, RecordLifecycle, RecordScope, ScopeClass,
    };
    use serde_json::json;

    fn profile() -> ActiveSyncProfile {
        ActiveSyncProfile {
            identity_handle: "10000000-0000-4000-8000-000000000001".to_owned(),
            receipt_id: "00000000-0000-7000-8000-000000000009".to_owned(),
            activation_sha256: "11".repeat(32),
            library_id: "00000000-0000-7000-8000-000000000001".to_owned(),
            device_id: "00000000-0000-7000-8000-000000000002".to_owned(),
            default_scope_id: "00000000-0000-7000-8000-000000000004".to_owned(),
            authority_generation: 3,
            purge_generation: 2,
            key_epoch: 4,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            durable_sync_spki_sha256: [0x22; 32],
            device_signing_public_key: [vec![0x04], vec![0x11; 64]].concat(),
            authority_signing_public_key: [vec![0x04], vec![0x22; 64]].concat(),
            granted_scopes: fixture_record_scopes(),
            capabilities: fixture_record_capabilities(),
            revoked: false,
        }
    }

    fn record() -> ContextRecordV1 {
        ContextRecordV1::new(
            profile().library_id,
            "00000000-0000-7000-8000-000000000005".to_owned(),
            "note".to_owned(),
            1,
            1,
            "00000000-0000-7000-8000-000000000006".to_owned(),
            "2026-08-17T12:00:00Z".to_owned(),
            "2026-08-17T12:00:00Z".to_owned(),
            None,
            RecordScope {
                scope_id: "00000000-0000-7000-8000-000000000004".to_owned(),
                class: ScopeClass::Unknown,
            },
            "standard".to_owned(),
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: None,
            },
            json!({"title": "Exact", "body": "bytes"}),
            json!({"fixture": true}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )
        .expect("record")
    }

    #[test]
    fn canonical_parser_rejects_reformatted_or_extended_bytes() {
        let value = serde_json::to_value(record()).expect("value");
        let canonical = canonical_json(&value).into_bytes();
        assert_eq!(
            parse_canonical_record(&canonical).expect("canonical"),
            record()
        );

        let pretty = serde_json::to_vec_pretty(&value).expect("pretty");
        assert_eq!(
            parse_canonical_record(&pretty).unwrap_err().to_string(),
            "record bytes are not canonical JSON"
        );

        let mut trailing = canonical;
        trailing.extend_from_slice(b"\n");
        assert_eq!(
            parse_canonical_record(&trailing).unwrap_err().to_string(),
            "record bytes are not canonical JSON"
        );
    }

    #[test]
    fn mutation_context_requires_exact_record_binding() {
        let profile = profile();
        let draft = MutationDraft {
            mutation_id: "00000000-0000-7000-8000-000000000007".to_owned(),
            operation: MutationOperation::Create,
            record_id: "00000000-0000-7000-8000-000000000005".to_owned(),
            record_kind: "note".to_owned(),
            record_schema_version: 1,
            base_head_revision: 0,
            base_head_version_id: None,
            proposed_revision: 1,
            version_id: "00000000-0000-7000-8000-000000000006".to_owned(),
            ciphertext: Vec::new(),
        };
        let context = context_from_draft(&profile, &draft).expect("context");
        validate_record_binding(&record(), &context).expect("binding");

        let mut wrong = record();
        wrong.revision = 2;
        assert!(matches!(
            validate_record_binding(&wrong, &context),
            Err(MobileRecordCryptoError::InvalidMutationBinding)
        ));
    }
}
