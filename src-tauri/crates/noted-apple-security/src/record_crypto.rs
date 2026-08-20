#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use crate::models::{
    validate_uuid_v7, BootstrapMetadataV1, IdentityHandle, P256_PUBLIC_KEY_BYTES,
    P256_SIGNATURE_BYTES, SHA256_BYTES,
};
use crate::{Error, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crate::models::RECORD_CIPHER_SUITE;

pub const RECORD_CRYPTO_CONTEXT_VERSION: u32 = 1;
pub const RECORD_NONCE_BYTES: usize = 12;
pub const RECORD_TAG_BYTES: usize = 16;
pub const MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES: usize = 512 * 1024;
/// `NRC1` magic + u32 version + u32 ciphertext length + nonce + two SHA-256
/// digests + the fixed-width inner P-256 signature.
pub const RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD: usize =
    4 + 4 + 4 + RECORD_NONCE_BYTES + SHA256_BYTES + SHA256_BYTES + P256_SIGNATURE_BYTES;
pub const MAX_RECORD_CIPHERTEXT_BYTES: usize =
    MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES - RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD;
pub const MAX_RECORD_PLAINTEXT_BYTES: usize = MAX_RECORD_CIPHERTEXT_BYTES - RECORD_TAG_BYTES;
pub const RECORD_HKDF_SALT_DOMAIN: &str = "noted.record-aead.v1/hkdf-salt";

const RECORD_CIPHERTEXT_MAGIC: &[u8; 4] = b"NRC1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKindV1 {
    Note,
    Category,
    Folder,
}

impl RecordKindV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Category => "category",
            Self::Folder => "folder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordCryptoOperationV1 {
    Create,
    Update,
    Delete,
}

impl RecordCryptoOperationV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// Public v1 record-crypto facts shared by the Mac authority and iPhone.
///
/// No key handle or key byte is present. Every field is included in both the
/// HKDF info and AEAD associated data so a ciphertext cannot be rebound to a
/// different record, revision, mutation, generation, epoch, or operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordCryptoContextV1 {
    pub version: u32,
    pub cipher_suite: String,
    pub library_id: String,
    pub record_id: String,
    pub record_kind: RecordKindV1,
    pub schema_version: u32,
    pub base_revision: u64,
    pub base_version_id: Option<String>,
    pub proposed_revision: u64,
    pub version_id: String,
    pub mutation_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub operation: RecordCryptoOperationV1,
}

impl RecordCryptoContextV1 {
    pub fn validate(&self) -> Result<()> {
        if self.version != RECORD_CRYPTO_CONTEXT_VERSION
            || self.cipher_suite != RECORD_CIPHER_SUITE
            || self.schema_version != 1
            || self.authority_generation == 0
            || self.key_epoch == 0
            || self.authority_generation > i64::MAX as u64
            || self.purge_generation > i64::MAX as u64
            || self.key_epoch > i64::MAX as u64
            || self.base_revision > i64::MAX as u64
            || self.proposed_revision > i64::MAX as u64
            || self
                .base_revision
                .checked_add(1)
                .is_none_or(|revision| revision != self.proposed_revision)
        {
            return Err(Error::InvalidNativeResponse("record crypto context"));
        }
        for identifier in [
            &self.library_id,
            &self.record_id,
            &self.version_id,
            &self.mutation_id,
        ] {
            validate_uuid_v7(identifier)?;
        }
        if let Some(base_version_id) = &self.base_version_id {
            validate_uuid_v7(base_version_id)?;
        }
        let initial = self.base_revision == 0 && self.base_version_id.is_none();
        let continuation = self.base_revision > 0 && self.base_version_id.is_some();
        if !match self.operation {
            RecordCryptoOperationV1::Create => initial,
            RecordCryptoOperationV1::Update | RecordCryptoOperationV1::Delete => continuation,
        } {
            return Err(Error::InvalidNativeResponse(
                "record crypto revision operation",
            ));
        }
        if self.library_id == self.record_id
            || self.library_id == self.version_id
            || self.library_id == self.mutation_id
            || self.record_id == self.version_id
            || self.record_id == self.mutation_id
            || self.version_id == self.mutation_id
            || self.base_version_id.as_ref().is_some_and(|base| {
                base == &self.library_id
                    || base == &self.record_id
                    || base == &self.version_id
                    || base == &self.mutation_id
            })
        {
            return Err(Error::InvalidNativeResponse("record crypto identifiers"));
        }
        Ok(())
    }

    pub fn validate_against_bootstrap(&self, metadata: &BootstrapMetadataV1) -> Result<()> {
        self.validate()?;
        metadata.validate()?;
        let capability = metadata.capabilities.get(self.record_kind.as_str());
        if self.library_id != metadata.library_id
            || self.authority_generation != metadata.authority_generation
            || self.purge_generation != metadata.purge_generation
            || self.key_epoch != metadata.key_epoch
            || self.cipher_suite != metadata.record_cipher_suite
            || !metadata
                .granted_scopes
                .iter()
                .any(|scope| scope == self.record_kind.as_str())
            || capability.is_none_or(|capability| {
                capability.reader_version < self.schema_version
                    || capability.writer_version != Some(self.schema_version)
            })
        {
            return Err(Error::InvalidNativeResponse(
                "record crypto bootstrap binding",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordCiphertextV1 {
    pub version: u32,
    pub cipher_suite: String,
    pub nonce: Vec<u8>,
    /// AES-GCM ciphertext followed by its 16-byte authentication tag.
    pub ciphertext: Vec<u8>,
    pub context_digest: [u8; SHA256_BYTES],
    pub envelope_digest: [u8; SHA256_BYTES],
    /// Inner P-256 ECDSA signature in fixed-width IEEE P1363 form over
    /// [`record_signature_message`]. This is serialized inside a mutation's
    /// ciphertext and is never the outer `MutationEnvelope` signature.
    pub record_signature: Vec<u8>,
}

impl RecordCiphertextV1 {
    pub fn validate_for(&self, context: &RecordCryptoContextV1) -> Result<()> {
        context.validate()?;
        if self.version != RECORD_CRYPTO_CONTEXT_VERSION
            || self.cipher_suite != RECORD_CIPHER_SUITE
            || self.nonce.len() != RECORD_NONCE_BYTES
            || !(RECORD_TAG_BYTES..=MAX_RECORD_CIPHERTEXT_BYTES).contains(&self.ciphertext.len())
            || self.record_signature.len() != P256_SIGNATURE_BYTES
            || self.context_digest != record_context_digest(context)?
            || self.envelope_digest
                != record_envelope_digest(context, &self.nonce, &self.ciphertext)?
        {
            return Err(Error::InvalidNativeResponse("record ciphertext"));
        }
        Ok(())
    }
}

/// Maximum plaintext that fits in `remaining_transaction_bytes` after the
/// public record container and AES-GCM tag. Callers with multiple mutations
/// must decrement their remaining aggregate budget after each encoded blob.
pub const fn max_plaintext_for_transaction(remaining_transaction_bytes: usize) -> Option<usize> {
    let minimum = RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + RECORD_TAG_BYTES;
    if remaining_transaction_bytes < minimum {
        return None;
    }
    let available = remaining_transaction_bytes - minimum;
    if available < MAX_RECORD_PLAINTEXT_BYTES {
        Some(available)
    } else {
        Some(MAX_RECORD_PLAINTEXT_BYTES)
    }
}

/// Deterministic public wire form stored as `MutationEnvelope.ciphertext`.
/// The cipher suite is implied by v1 and reconstituted only after all lengths
/// and record/context bindings validate.
pub fn encode_record_ciphertext_v1(
    value: &RecordCiphertextV1,
    context: &RecordCryptoContextV1,
) -> Result<Vec<u8>> {
    value.validate_for(context)?;
    let ciphertext_len: u32 = value
        .ciphertext
        .len()
        .try_into()
        .map_err(|_| Error::InvalidNativeResponse("record ciphertext length"))?;
    let mut encoded =
        Vec::with_capacity(RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + value.ciphertext.len());
    encoded.extend_from_slice(RECORD_CIPHERTEXT_MAGIC);
    encoded.extend_from_slice(&value.version.to_be_bytes());
    encoded.extend_from_slice(&ciphertext_len.to_be_bytes());
    encoded.extend_from_slice(&value.nonce);
    encoded.extend_from_slice(&value.context_digest);
    encoded.extend_from_slice(&value.envelope_digest);
    encoded.extend_from_slice(&value.record_signature);
    encoded.extend_from_slice(&value.ciphertext);
    debug_assert_eq!(
        encoded.len(),
        RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + value.ciphertext.len()
    );
    Ok(encoded)
}

/// Strict inverse of [`encode_record_ciphertext_v1`]. Declared length must
/// consume the input exactly; truncated values and trailing bytes fail closed.
pub fn decode_record_ciphertext_v1(
    encoded: &[u8],
    context: &RecordCryptoContextV1,
) -> Result<RecordCiphertextV1> {
    context.validate()?;
    if !(RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + RECORD_TAG_BYTES
        ..=MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES)
        .contains(&encoded.len())
        || encoded.get(..4) != Some(RECORD_CIPHERTEXT_MAGIC.as_slice())
    {
        return Err(Error::InvalidNativeResponse("record ciphertext encoding"));
    }
    let version = u32::from_be_bytes(
        encoded[4..8]
            .try_into()
            .map_err(|_| Error::InvalidNativeResponse("record ciphertext encoding"))?,
    );
    let ciphertext_len = u32::from_be_bytes(
        encoded[8..12]
            .try_into()
            .map_err(|_| Error::InvalidNativeResponse("record ciphertext encoding"))?,
    ) as usize;
    if !(RECORD_TAG_BYTES..=MAX_RECORD_CIPHERTEXT_BYTES).contains(&ciphertext_len)
        || encoded.len() != RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + ciphertext_len
    {
        return Err(Error::InvalidNativeResponse("record ciphertext length"));
    }
    let mut cursor = 12;
    let nonce = encoded[cursor..cursor + RECORD_NONCE_BYTES].to_vec();
    cursor += RECORD_NONCE_BYTES;
    let context_digest = encoded[cursor..cursor + SHA256_BYTES]
        .try_into()
        .map_err(|_| Error::InvalidNativeResponse("record context digest"))?;
    cursor += SHA256_BYTES;
    let envelope_digest = encoded[cursor..cursor + SHA256_BYTES]
        .try_into()
        .map_err(|_| Error::InvalidNativeResponse("record envelope digest"))?;
    cursor += SHA256_BYTES;
    let record_signature = encoded[cursor..cursor + P256_SIGNATURE_BYTES].to_vec();
    cursor += P256_SIGNATURE_BYTES;
    let ciphertext = encoded[cursor..].to_vec();
    let value = RecordCiphertextV1 {
        version,
        cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
        nonce,
        ciphertext,
        context_digest,
        envelope_digest,
        record_signature,
    };
    value.validate_for(context)?;
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedRecordV1 {
    pub plaintext: Vec<u8>,
    pub context_digest: [u8; SHA256_BYTES],
    pub envelope_digest: [u8; SHA256_BYTES],
}

pub fn canonical_record_crypto_context(context: &RecordCryptoContextV1) -> Result<Vec<u8>> {
    context.validate()?;
    let mut builder = CanonicalBuilder::new("noted.record-aead.v1/context");
    builder.u64("version", u64::from(context.version));
    builder.text("cipher_suite", &context.cipher_suite);
    builder.text("library_id", &context.library_id);
    builder.text("record_id", &context.record_id);
    builder.text("record_kind", context.record_kind.as_str());
    builder.u64("schema_version", u64::from(context.schema_version));
    builder.u64("base_revision", context.base_revision);
    match &context.base_version_id {
        Some(base_version_id) => {
            builder.text("base_version_present", "true");
            builder.text("base_version_id", base_version_id);
        }
        None => builder.text("base_version_present", "false"),
    }
    builder.u64("proposed_revision", context.proposed_revision);
    builder.text("version_id", &context.version_id);
    builder.text("mutation_id", &context.mutation_id);
    builder.u64("authority_generation", context.authority_generation);
    builder.u64("purge_generation", context.purge_generation);
    builder.u64("key_epoch", context.key_epoch);
    builder.text("operation", context.operation.as_str());
    Ok(builder.finish())
}

pub fn record_hkdf_info(context: &RecordCryptoContextV1) -> Result<Vec<u8>> {
    let context = canonical_record_crypto_context(context)?;
    Ok(canonical_components(
        "noted.record-aead.v1/hkdf-info",
        &[("context", &context)],
    ))
}

/// Fixed public salt for the v1 per-record HKDF. Domain separation and all
/// mutable record facts are carried independently in [`record_hkdf_info`].
pub fn record_hkdf_salt() -> [u8; SHA256_BYTES] {
    Sha256::digest(RECORD_HKDF_SALT_DOMAIN.as_bytes()).into()
}

pub fn record_associated_data(context: &RecordCryptoContextV1) -> Result<Vec<u8>> {
    let context = canonical_record_crypto_context(context)?;
    Ok(canonical_components(
        "noted.record-aead.v1/aad",
        &[("context", &context)],
    ))
}

pub fn record_context_digest(context: &RecordCryptoContextV1) -> Result<[u8; SHA256_BYTES]> {
    Ok(Sha256::digest(canonical_record_crypto_context(context)?).into())
}

pub fn record_envelope_digest(
    context: &RecordCryptoContextV1,
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<[u8; SHA256_BYTES]> {
    if nonce.len() != RECORD_NONCE_BYTES
        || !(RECORD_TAG_BYTES..=MAX_RECORD_CIPHERTEXT_BYTES).contains(&ciphertext.len())
    {
        return Err(Error::InvalidNativeResponse("record ciphertext bounds"));
    }
    let context = canonical_record_crypto_context(context)?;
    Ok(Sha256::digest(canonical_components(
        "noted.record-aead.v1/envelope",
        &[
            ("context", &context),
            ("nonce", nonce),
            ("ciphertext", ciphertext),
        ],
    ))
    .into())
}

pub fn record_signature_message(envelope_digest: &[u8; SHA256_BYTES]) -> [u8; SHA256_BYTES] {
    Sha256::digest(canonical_components(
        "noted.record-aead.v1/signature",
        &[("envelope_digest", envelope_digest)],
    ))
    .into()
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
        let mut value = Self { bytes: Vec::new() };
        value.bytes("domain", domain.as_bytes());
        value
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

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SealRecordArgs<'a> {
    pub identity_handle: &'a str,
    pub context: &'a RecordCryptoContextV1,
    pub plaintext_base64: String,
}

impl<'a> SealRecordArgs<'a> {
    pub(crate) fn new(
        identity: &'a IdentityHandle,
        context: &'a RecordCryptoContextV1,
        plaintext: &[u8],
    ) -> Result<Self> {
        context.validate()?;
        if plaintext.len() > MAX_RECORD_PLAINTEXT_BYTES {
            return Err(Error::InvalidNativeResponse("record plaintext bounds"));
        }
        Ok(Self {
            identity_handle: identity.expose_opaque(),
            context,
            plaintext_base64: BASE64.encode(plaintext),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenRecordArgs<'a> {
    pub identity_handle: &'a str,
    pub context: &'a RecordCryptoContextV1,
    pub sealed: RecordCiphertextBridge,
    pub signer_public_key_base64: String,
}

impl<'a> OpenRecordArgs<'a> {
    pub(crate) fn new(
        identity: &'a IdentityHandle,
        context: &'a RecordCryptoContextV1,
        sealed: &RecordCiphertextV1,
        signer_public_key: &[u8],
    ) -> Result<Self> {
        sealed.validate_for(context)?;
        if signer_public_key.len() != P256_PUBLIC_KEY_BYTES
            || signer_public_key.first() != Some(&0x04)
        {
            return Err(Error::InvalidNativeResponse("record signer public key"));
        }
        Ok(Self {
            identity_handle: identity.expose_opaque(),
            context,
            sealed: RecordCiphertextBridge::from_public(sealed),
            signer_public_key_base64: BASE64.encode(signer_public_key),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordCiphertextBridge {
    pub version: u32,
    pub cipher_suite: String,
    pub nonce_base64: String,
    pub ciphertext_base64: String,
    pub context_digest_base64: String,
    pub envelope_digest_base64: String,
    pub record_signature_base64: String,
}

impl RecordCiphertextBridge {
    fn from_public(value: &RecordCiphertextV1) -> Self {
        Self {
            version: value.version,
            cipher_suite: value.cipher_suite.clone(),
            nonce_base64: BASE64.encode(&value.nonce),
            ciphertext_base64: BASE64.encode(&value.ciphertext),
            context_digest_base64: BASE64.encode(value.context_digest),
            envelope_digest_base64: BASE64.encode(value.envelope_digest),
            record_signature_base64: BASE64.encode(&value.record_signature),
        }
    }

    pub(crate) fn into_public(self, context: &RecordCryptoContextV1) -> Result<RecordCiphertextV1> {
        let value = RecordCiphertextV1 {
            version: self.version,
            cipher_suite: self.cipher_suite,
            nonce: decode_bounded_exact(
                &self.nonce_base64,
                RECORD_NONCE_BYTES,
                RECORD_NONCE_BYTES,
                "record nonce",
            )?,
            ciphertext: decode_bounded_exact(
                &self.ciphertext_base64,
                RECORD_TAG_BYTES,
                MAX_RECORD_CIPHERTEXT_BYTES,
                "record ciphertext",
            )?,
            context_digest: decode_array(&self.context_digest_base64, "record context digest")?,
            envelope_digest: decode_array(&self.envelope_digest_base64, "record envelope digest")?,
            record_signature: decode_bounded_exact(
                &self.record_signature_base64,
                P256_SIGNATURE_BYTES,
                P256_SIGNATURE_BYTES,
                "record signature",
            )?,
        };
        value.validate_for(context)?;
        Ok(value)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenedRecordBridge {
    pub plaintext_base64: String,
    pub context_digest_base64: String,
    pub envelope_digest_base64: String,
}

impl OpenedRecordBridge {
    pub(crate) fn into_public(
        self,
        context: &RecordCryptoContextV1,
        sealed: &RecordCiphertextV1,
    ) -> Result<OpenedRecordV1> {
        sealed.validate_for(context)?;
        let value = OpenedRecordV1 {
            plaintext: decode_bounded_exact(
                &self.plaintext_base64,
                0,
                MAX_RECORD_PLAINTEXT_BYTES,
                "record plaintext",
            )?,
            context_digest: decode_array(&self.context_digest_base64, "record context digest")?,
            envelope_digest: decode_array(&self.envelope_digest_base64, "record envelope digest")?,
        };
        if value.context_digest != sealed.context_digest
            || value.envelope_digest != sealed.envelope_digest
        {
            return Err(Error::InvalidNativeResponse("opened record binding"));
        }
        Ok(value)
    }
}

fn decode_array(value: &str, field: &'static str) -> Result<[u8; SHA256_BYTES]> {
    let decoded = decode_bounded_exact(value, SHA256_BYTES, SHA256_BYTES, field)?;
    decoded
        .try_into()
        .map_err(|_| Error::InvalidNativeResponse(field))
}

fn decode_bounded_exact(
    value: &str,
    minimum: usize,
    maximum: usize,
    field: &'static str,
) -> Result<Vec<u8>> {
    if value.len() > maximum.div_ceil(3) * 4 + 4 {
        return Err(Error::InvalidNativeResponse(field));
    }
    let decoded = BASE64
        .decode(value)
        .map_err(|_| Error::InvalidNativeResponse(field))?;
    if !(minimum..=maximum).contains(&decoded.len()) {
        return Err(Error::InvalidNativeResponse(field));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BootstrapCapabilityV1, PAIRING_PROTOCOL, PAIRING_SUITE};
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn context() -> RecordCryptoContextV1 {
        RecordCryptoContextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            library_id: "018f47a0-7b80-7000-8000-000000000101".to_owned(),
            record_id: "018f47a0-7b80-7000-8000-000000000102".to_owned(),
            record_kind: RecordKindV1::Note,
            schema_version: 1,
            base_revision: 1,
            base_version_id: Some("018f47a0-7b80-7000-8000-000000000103".to_owned()),
            proposed_revision: 2,
            version_id: "018f47a0-7b80-7000-8000-000000000104".to_owned(),
            mutation_id: "018f47a0-7b80-7000-8000-000000000105".to_owned(),
            authority_generation: 7,
            purge_generation: 2,
            key_epoch: 3,
            operation: RecordCryptoOperationV1::Update,
        }
    }

    fn metadata() -> BootstrapMetadataV1 {
        let capability = BootstrapCapabilityV1 {
            reader_version: 1,
            writer_version: Some(1),
        };
        BootstrapMetadataV1 {
            version: 1,
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            sync_protocol_version: 1,
            environment: "development".to_owned(),
            library_data_class: "sanitized_fixture".to_owned(),
            receipt_id: "018f47a0-7b80-7000-8000-000000000106".to_owned(),
            library_id: context().library_id,
            device_id: "018f47a0-7b80-7000-8000-000000000107".to_owned(),
            authority_generation: 7,
            purge_generation: 2,
            key_epoch: 3,
            default_scope_id: "018f47a0-7b80-7000-8000-000000000108".to_owned(),
            default_scope_class: "unknown".to_owned(),
            granted_scopes: vec![
                "note".to_owned(),
                "category".to_owned(),
                "folder".to_owned(),
            ],
            capabilities: BTreeMap::from([
                ("note".to_owned(), capability),
                ("category".to_owned(), capability),
                ("folder".to_owned(), capability),
            ]),
            record_cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            durable_sync_spki_sha256: [0x77; 32],
            transcript_digest: [0x88; 32],
        }
    }

    #[test]
    fn context_is_strict_and_binds_active_bootstrap() {
        let value = context();
        assert!(value.validate().is_ok());
        assert!(value.validate_against_bootstrap(&metadata()).is_ok());

        let mut changed = value.clone();
        changed.proposed_revision = 3;
        assert!(changed.validate().is_err());
        changed = value.clone();
        changed.operation = RecordCryptoOperationV1::Create;
        assert!(changed.validate().is_err());
        changed = value.clone();
        changed.key_epoch += 1;
        assert!(changed.validate_against_bootstrap(&metadata()).is_err());
        changed = value;
        changed.record_kind = RecordKindV1::Category;
        changed.schema_version = 2;
        assert!(changed.validate_against_bootstrap(&metadata()).is_err());
    }

    #[test]
    fn record_descriptor_recomputes_every_public_binding() {
        let context = context();
        let nonce = vec![0x11; RECORD_NONCE_BYTES];
        let ciphertext = vec![0x22; 48];
        let mut sealed = RecordCiphertextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            context_digest: record_context_digest(&context).unwrap(),
            envelope_digest: record_envelope_digest(&context, &nonce, &ciphertext).unwrap(),
            nonce,
            ciphertext,
            record_signature: vec![0x33; P256_SIGNATURE_BYTES],
        };
        assert!(sealed.validate_for(&context).is_ok());
        sealed.nonce[0] ^= 1;
        assert!(sealed.validate_for(&context).is_err());
    }

    #[test]
    fn bridge_rejects_oversize_or_rebound_native_results() {
        let context = context();
        let nonce = vec![0x11; RECORD_NONCE_BYTES];
        let ciphertext = vec![0x22; 48];
        let sealed = RecordCiphertextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            context_digest: record_context_digest(&context).unwrap(),
            envelope_digest: record_envelope_digest(&context, &nonce, &ciphertext).unwrap(),
            nonce,
            ciphertext,
            record_signature: vec![0x33; P256_SIGNATURE_BYTES],
        };
        let wire = RecordCiphertextBridge::from_public(&sealed);
        assert_eq!(wire.into_public(&context).unwrap(), sealed);

        let mut oversize = RecordCiphertextBridge::from_public(&sealed);
        oversize.ciphertext_base64 = BASE64.encode(vec![0_u8; MAX_RECORD_CIPHERTEXT_BYTES + 1]);
        assert!(oversize.into_public(&context).is_err());
        let mut rebound = RecordCiphertextBridge::from_public(&sealed);
        rebound.context_digest_base64 = BASE64.encode([0x44; SHA256_BYTES]);
        assert!(rebound.into_public(&context).is_err());
    }

    #[test]
    fn fixed_binary_codec_is_exact_bounded_and_rejects_trailing_bytes() {
        let context = context();
        let nonce = vec![0x11; RECORD_NONCE_BYTES];
        let ciphertext = vec![0x22; 48];
        let sealed = RecordCiphertextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            context_digest: record_context_digest(&context).unwrap(),
            envelope_digest: record_envelope_digest(&context, &nonce, &ciphertext).unwrap(),
            nonce,
            ciphertext,
            record_signature: vec![0x33; P256_SIGNATURE_BYTES],
        };
        let encoded = encode_record_ciphertext_v1(&sealed, &context).unwrap();
        assert_eq!(
            encoded.len(),
            RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + sealed.ciphertext.len()
        );
        assert_eq!(
            decode_record_ciphertext_v1(&encoded, &context).unwrap(),
            sealed
        );

        let mut wrong_magic = encoded.clone();
        wrong_magic[0] ^= 1;
        assert!(decode_record_ciphertext_v1(&wrong_magic, &context).is_err());
        let mut wrong_version = encoded.clone();
        wrong_version[7] = 2;
        assert!(decode_record_ciphertext_v1(&wrong_version, &context).is_err());
        let mut wrong_length = encoded.clone();
        wrong_length[11] = wrong_length[11].wrapping_add(1);
        assert!(decode_record_ciphertext_v1(&wrong_length, &context).is_err());
        assert!(decode_record_ciphertext_v1(&encoded[..encoded.len() - 1], &context).is_err());
        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode_record_ciphertext_v1(&trailing, &context).is_err());
        let mut tampered_nonce = encode_record_ciphertext_v1(&sealed, &context).unwrap();
        tampered_nonce[12] ^= 1;
        assert!(decode_record_ciphertext_v1(&tampered_nonce, &context).is_err());

        assert_eq!(RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD, 152);
        assert_eq!(
            max_plaintext_for_transaction(MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES),
            Some(MAX_RECORD_PLAINTEXT_BYTES)
        );
        assert_eq!(
            max_plaintext_for_transaction(RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD + RECORD_TAG_BYTES),
            Some(0)
        );
        assert_eq!(max_plaintext_for_transaction(1), None);

        let maximum_ciphertext = vec![0x5a; MAX_RECORD_CIPHERTEXT_BYTES];
        let maximum = RecordCiphertextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            context_digest: record_context_digest(&context).unwrap(),
            envelope_digest: record_envelope_digest(
                &context,
                &[0x6b; RECORD_NONCE_BYTES],
                &maximum_ciphertext,
            )
            .unwrap(),
            nonce: vec![0x6b; RECORD_NONCE_BYTES],
            ciphertext: maximum_ciphertext,
            record_signature: vec![0x7c; P256_SIGNATURE_BYTES],
        };
        let maximum_encoded = encode_record_ciphertext_v1(&maximum, &context).unwrap();
        assert_eq!(maximum_encoded.len(), MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES);
        assert_eq!(
            decode_record_ciphertext_v1(&maximum_encoded, &context).unwrap(),
            maximum
        );
    }

    #[test]
    fn rust_matches_the_sanitized_swift_record_contract_vector() {
        let vector: Value =
            serde_json::from_str(include_str!("../fixtures/record_crypto_v1.json")).unwrap();
        assert_eq!(
            vector["fixtureClass"].as_str(),
            Some("sanitized_record_crypto_v1")
        );
        let context: RecordCryptoContextV1 =
            serde_json::from_value(vector["context"].clone()).unwrap();
        let expected_context_digest = hex(&vector, "canonicalContextSha256Hex");
        let expected_info_digest = hex(&vector, "hkdfInfoSha256Hex");
        let expected_aad_digest = hex(&vector, "aadSha256Hex");
        assert_eq!(
            Sha256::digest(canonical_record_crypto_context(&context).unwrap()).as_slice(),
            expected_context_digest
        );
        assert_eq!(
            record_context_digest(&context).unwrap().as_slice(),
            expected_context_digest
        );
        assert_eq!(record_hkdf_salt().as_slice(), hex(&vector, "hkdfSaltHex"));
        assert_eq!(
            Sha256::digest(record_hkdf_info(&context).unwrap()).as_slice(),
            expected_info_digest
        );
        assert_eq!(
            Sha256::digest(record_associated_data(&context).unwrap()).as_slice(),
            expected_aad_digest
        );

        let nonce = hex(&vector, "nonceHex");
        let ciphertext = BASE64
            .decode(vector["ciphertextBase64"].as_str().unwrap())
            .unwrap();
        let envelope_digest = record_envelope_digest(&context, &nonce, &ciphertext).unwrap();
        assert_eq!(
            envelope_digest.as_slice(),
            hex(&vector, "envelopeDigestHex")
        );
        assert_eq!(
            record_signature_message(&envelope_digest).as_slice(),
            hex(&vector, "signatureMessageHex")
        );

        let mut unknown = vector["context"].clone();
        unknown["unknownField"] = Value::Bool(true);
        assert!(serde_json::from_value::<RecordCryptoContextV1>(unknown).is_err());
    }

    fn hex(value: &Value, key: &str) -> Vec<u8> {
        let value = value[key].as_str().unwrap();
        assert!(value.len().is_multiple_of(2));
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).unwrap();
                u8::from_str_radix(pair, 16).unwrap()
            })
            .collect()
    }
}
