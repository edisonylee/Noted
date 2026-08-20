//! iPhone native-key adapters for signed direct-sync requests and canonical
//! NRC1 records. Private signing and library key material never crosses the
//! native plugin boundary.

use crate::mobile_record_crypto::{
    context_from_draft, context_from_envelope, parse_canonical_record, validate_record_binding,
    validate_record_profile, validate_writer_and_outer_signature, MobileRecordCrypto,
    MobileRecordCryptoError,
};
use crate::mobile_sync_runtime::{MobileSyncCrypto, MobileSyncRuntimeError};
use crate::portable::canonical_json;
use crate::sync_protocol::{
    MutationDraft, MutationEnvelope, PreparedTransaction, SignedTransaction,
};
use noted_apple_security::{
    decode_record_ciphertext_v1, encode_record_ciphertext_v1, AppleSecurity, IdentityHandle,
};
use tauri::Runtime;

pub struct AppleMobileSyncCrypto<'a, R: Runtime> {
    security: &'a AppleSecurity<R>,
}

impl<'a, R: Runtime> AppleMobileSyncCrypto<'a, R> {
    pub const fn new(security: &'a AppleSecurity<R>) -> Self {
        Self { security }
    }
}

impl<R: Runtime> MobileSyncCrypto for AppleMobileSyncCrypto<'_, R> {
    fn fresh_uuid_v7(&self) -> Result<String, MobileSyncRuntimeError> {
        self.security
            .fresh_uuid_v7()
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)
    }

    fn sign(
        &self,
        identity_handle: &str,
        message: &[u8],
    ) -> Result<Vec<u8>, MobileSyncRuntimeError> {
        let handle = IdentityHandle::from_opaque(identity_handle)
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)?;
        self.security
            .sign(&handle, message)
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)
    }

    fn verify_p256_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<bool, MobileSyncRuntimeError> {
        self.security
            .verify_p256_signature(public_key, message, signature)
            .map_err(|_| MobileSyncRuntimeError::NativeCryptoRejected)
    }
}

impl<R: Runtime> MobileRecordCrypto for AppleMobileSyncCrypto<'_, R> {
    fn seal_canonical_record(
        &self,
        profile: &crate::mobile_sync_runtime::ActiveSyncProfile,
        mut draft: MutationDraft,
        canonical_record_bytes: &[u8],
    ) -> Result<MutationDraft, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        if !draft.ciphertext.is_empty() {
            return Err(MobileRecordCryptoError::InvalidMutationBinding);
        }
        let record = parse_canonical_record(canonical_record_bytes)?;
        let context = context_from_draft(profile, &draft)?;
        validate_record_binding(&record, &context)?;
        let identity = IdentityHandle::from_opaque(&profile.identity_handle)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let sealed = self
            .security
            .seal_record(&identity, &context, canonical_record_bytes)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        draft.ciphertext = encode_record_ciphertext_v1(&sealed, &context)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        Ok(draft)
    }

    fn sign_prepared_transaction(
        &self,
        profile: &crate::mobile_sync_runtime::ActiveSyncProfile,
        prepared: PreparedTransaction,
    ) -> Result<SignedTransaction, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        let identity = IdentityHandle::from_opaque(&profile.identity_handle)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let signatures = prepared
            .signing_inputs()
            .into_iter()
            .map(|input| {
                self.security
                    .sign(&identity, &input.canonical_bytes)
                    .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signed = prepared
            .attach_signatures(signatures)
            .map_err(MobileRecordCryptoError::Protocol)?;
        if signed.manifest.library_id != profile.library_id
            || signed.manifest.device_id != profile.device_id
            || signed.manifest.authority_generation != profile.authority_generation
            || signed.manifest.purge_generation != profile.purge_generation
            || signed.manifest.key_epoch != profile.key_epoch
        {
            return Err(MobileRecordCryptoError::InvalidMutationBinding);
        }
        // Reopen every outbound record through the same native custody before
        // releasing the transaction. This proves the outer and inner writers
        // match and catches any bridge/context divergence immediately.
        for envelope in &signed.members {
            self.open_canonical_record(profile, envelope, &profile.device_signing_public_key)?;
        }
        Ok(signed)
    }

    fn open_canonical_record(
        &self,
        profile: &crate::mobile_sync_runtime::ActiveSyncProfile,
        envelope: &MutationEnvelope,
        writer_public_key: &[u8],
    ) -> Result<Vec<u8>, MobileRecordCryptoError> {
        validate_record_profile(profile)?;
        validate_writer_and_outer_signature(self, envelope, writer_public_key)?;
        let context = context_from_envelope(profile, envelope)?;
        let sealed = decode_record_ciphertext_v1(&envelope.ciphertext, &context)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let identity = IdentityHandle::from_opaque(&profile.identity_handle)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let opened = self
            .security
            .open_record(&identity, &context, &sealed, writer_public_key)
            .map_err(|_| MobileRecordCryptoError::NativeCryptoRejected)?;
        let record = parse_canonical_record(&opened.plaintext)?;
        validate_record_binding(&record, &context)?;
        let value = serde_json::to_value(record)
            .map_err(|_| MobileRecordCryptoError::InvalidCanonicalRecord)?;
        let canonical = canonical_json(&value).into_bytes();
        if canonical != opened.plaintext {
            return Err(MobileRecordCryptoError::NonCanonicalRecord);
        }
        Ok(canonical)
    }
}
