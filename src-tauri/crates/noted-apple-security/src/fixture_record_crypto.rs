use crate::models::{
    BootstrapMetadataV1, MAX_SIGNING_MESSAGE_BYTES, P256_PUBLIC_KEY_BYTES, P256_SIGNATURE_BYTES,
};
use crate::record_crypto::{
    record_associated_data, record_context_digest, record_envelope_digest, record_hkdf_info,
    record_hkdf_salt, record_signature_message, OpenedRecordV1, RecordCiphertextV1,
    RecordCryptoContextV1, MAX_RECORD_PLAINTEXT_BYTES, RECORD_CRYPTO_CONTEXT_VERSION,
    RECORD_NONCE_BYTES, RECORD_TAG_BYTES,
};
use crate::{Error, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

/// Rust-only custody for sanitized development fixtures.
///
/// This type deliberately implements neither `Clone`, `Debug`, nor Serde. It
/// owns both secret inputs in zeroizing containers and exposes only its public
/// signing key plus ciphertext/plaintext operations. Callers must pass
/// `Zeroizing` values so ownership transfer cannot silently retain the
/// provider's particular copies through the constructor.
pub struct SanitizedFixtureRecordCrypto {
    metadata: BootstrapMetadataV1,
    signing_public_key: [u8; P256_PUBLIC_KEY_BYTES],
    library_key: Zeroizing<[u8; 32]>,
    signing_key: Zeroizing<[u8; 32]>,
}

impl SanitizedFixtureRecordCrypto {
    pub fn new(
        metadata: BootstrapMetadataV1,
        library_key: Zeroizing<[u8; 32]>,
        signing_key: Zeroizing<[u8; 32]>,
    ) -> Result<Self> {
        metadata.validate()?;
        if library_key.iter().all(|byte| *byte == 0) {
            return Err(fixture_error("invalid library key"));
        }
        let signing = SigningKey::from_slice(&signing_key[..])
            .map_err(|_| fixture_error("invalid signing key"))?;
        let encoded = signing.verifying_key().to_encoded_point(false);
        let signing_public_key = encoded
            .as_bytes()
            .try_into()
            .map_err(|_| fixture_error("invalid signing public key"))?;
        Ok(Self {
            metadata,
            signing_public_key,
            library_key,
            signing_key,
        })
    }

    /// Returns the validated, non-secret bootstrap facts bound to this custody.
    pub fn bootstrap_metadata(&self) -> &BootstrapMetadataV1 {
        &self.metadata
    }

    pub fn signing_public_key(&self) -> [u8; P256_PUBLIC_KEY_BYTES] {
        self.signing_public_key
    }

    pub fn seal_record(
        &self,
        context: &RecordCryptoContextV1,
        plaintext: &[u8],
    ) -> Result<RecordCiphertextV1> {
        let mut nonce = [0_u8; RECORD_NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| fixture_error("OS entropy unavailable"))?;
        self.seal_record_with_nonce(context, plaintext, nonce)
    }

    pub fn open_record(
        &self,
        context: &RecordCryptoContextV1,
        sealed: &RecordCiphertextV1,
        authority_authenticated_writer_public_key: &[u8],
    ) -> Result<OpenedRecordV1> {
        self.validate_key_use(context)?;
        sealed.validate_for(context)?;
        let signature_message = record_signature_message(&sealed.envelope_digest);
        let signature_valid = Self::verify_p256_p1363(
            authority_authenticated_writer_public_key,
            &signature_message,
            &sealed.record_signature,
        )
        .map_err(|_| fixture_error("record signature invalid"))?;
        if !signature_valid {
            return Err(fixture_error("record signature invalid"));
        }

        let key = self.derive_record_key(context)?;
        let cipher = Aes256Gcm::new_from_slice(&key[..])
            .map_err(|_| fixture_error("record key rejected"))?;
        let aad = record_associated_data(context)?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| fixture_error("record authentication failed"))?;
        if plaintext.len() > MAX_RECORD_PLAINTEXT_BYTES {
            return Err(fixture_error("record plaintext exceeds limit"));
        }
        Ok(OpenedRecordV1 {
            plaintext,
            context_digest: sealed.context_digest,
            envelope_digest: sealed.envelope_digest,
        })
    }

    pub fn sign_p256_p1363(&self, message: &[u8]) -> Result<[u8; P256_SIGNATURE_BYTES]> {
        self.validate_metadata()?;
        if message.len() > MAX_SIGNING_MESSAGE_BYTES {
            return Err(fixture_error("signing message exceeds limit"));
        }
        let signing = SigningKey::from_slice(&self.signing_key[..])
            .map_err(|_| fixture_error("invalid signing key"))?;
        let signature: Signature = signing.sign(message);
        let mut raw = [0_u8; P256_SIGNATURE_BYTES];
        raw.copy_from_slice(signature.to_bytes().as_slice());
        Ok(raw)
    }

    pub fn verify_p256_p1363(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<bool> {
        if public_key.len() != P256_PUBLIC_KEY_BYTES
            || public_key.first() != Some(&0x04)
            || signature.len() != P256_SIGNATURE_BYTES
            || message.len() > MAX_SIGNING_MESSAGE_BYTES
        {
            return Err(fixture_error("invalid P-256 verification request"));
        }
        let verifying = VerifyingKey::from_sec1_bytes(public_key)
            .map_err(|_| fixture_error("invalid P-256 public key"))?;
        let signature = Signature::from_slice(signature)
            .map_err(|_| fixture_error("invalid P-256 signature encoding"))?;
        Ok(verifying.verify(message, &signature).is_ok())
    }

    fn seal_record_with_nonce(
        &self,
        context: &RecordCryptoContextV1,
        plaintext: &[u8],
        nonce: [u8; RECORD_NONCE_BYTES],
    ) -> Result<RecordCiphertextV1> {
        self.validate_key_use(context)?;
        if plaintext.len() > MAX_RECORD_PLAINTEXT_BYTES {
            return Err(fixture_error("record plaintext exceeds limit"));
        }
        let key = self.derive_record_key(context)?;
        let cipher = Aes256Gcm::new_from_slice(&key[..])
            .map_err(|_| fixture_error("record key rejected"))?;
        let aad = record_associated_data(context)?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| fixture_error("record encryption failed"))?;
        if ciphertext.len() != plaintext.len() + RECORD_TAG_BYTES {
            return Err(fixture_error("record ciphertext length"));
        }
        let context_digest = record_context_digest(context)?;
        let envelope_digest = record_envelope_digest(context, &nonce, &ciphertext)?;
        let record_signature = self
            .sign_p256_p1363(&record_signature_message(&envelope_digest))?
            .to_vec();
        let sealed = RecordCiphertextV1 {
            version: RECORD_CRYPTO_CONTEXT_VERSION,
            cipher_suite: self.metadata.record_cipher_suite.clone(),
            nonce: nonce.to_vec(),
            ciphertext,
            context_digest,
            envelope_digest,
            record_signature,
        };
        sealed.validate_for(context)?;
        Ok(sealed)
    }

    fn validate_key_use(&self, context: &RecordCryptoContextV1) -> Result<()> {
        self.validate_metadata()?;
        context.validate_against_bootstrap(&self.metadata)
    }

    fn validate_metadata(&self) -> Result<()> {
        // Re-check on every secret operation so future interior-mutability or
        // persistence changes cannot accidentally weaken the constructor gate.
        self.metadata.validate()
    }

    fn derive_record_key(&self, context: &RecordCryptoContextV1) -> Result<Zeroizing<[u8; 32]>> {
        let mut key = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(Some(&record_hkdf_salt()), &self.library_key[..])
            .expand(&record_hkdf_info(context)?, &mut key[..])
            .map_err(|_| fixture_error("record key derivation failed"))?;
        Ok(key)
    }
}

impl Drop for SanitizedFixtureRecordCrypto {
    fn drop(&mut self) {
        self.library_key.zeroize();
        self.signing_key.zeroize();
    }
}

fn fixture_error(detail: &'static str) -> Error {
    Error::SanitizedFixtureCrypto(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BootstrapCapabilityV1, PAIRING_PROTOCOL, PAIRING_SUITE};
    use crate::record_crypto::{RecordCryptoOperationV1, RecordKindV1, RECORD_CIPHER_SUITE};
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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

    fn metadata(environment: &str, library_data_class: &str) -> BootstrapMetadataV1 {
        let capability = BootstrapCapabilityV1 {
            reader_version: 1,
            writer_version: Some(1),
        };
        BootstrapMetadataV1 {
            version: 1,
            protocol: PAIRING_PROTOCOL.to_owned(),
            suite: PAIRING_SUITE.to_owned(),
            sync_protocol_version: 1,
            environment: environment.to_owned(),
            library_data_class: library_data_class.to_owned(),
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

    fn provider() -> SanitizedFixtureRecordCrypto {
        SanitizedFixtureRecordCrypto::new(
            metadata("development", "sanitized_fixture"),
            Zeroizing::new(std::array::from_fn(|index| index as u8 + 1)),
            Zeroizing::new({
                let mut signing = [0_u8; 32];
                signing[31] = 1;
                signing
            }),
        )
        .unwrap()
    }

    #[test]
    fn provider_is_send_sync_and_round_trips_with_fresh_nonces() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SanitizedFixtureRecordCrypto>();

        let provider = provider();
        assert_eq!(
            provider.bootstrap_metadata(),
            &metadata("development", "sanitized_fixture")
        );
        let plaintext = b"sanitized fixture note";
        let first = provider.seal_record(&context(), plaintext).unwrap();
        let second = provider.seal_record(&context(), plaintext).unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_eq!(
            provider
                .open_record(&context(), &first, &provider.signing_public_key(),)
                .unwrap()
                .plaintext,
            plaintext
        );
    }

    #[test]
    fn provider_matches_and_opens_the_swift_golden_vector() {
        let vector: Value =
            serde_json::from_str(include_str!("../fixtures/record_crypto_v1.json")).unwrap();
        let context: RecordCryptoContextV1 =
            serde_json::from_value(vector["context"].clone()).unwrap();
        let provider = provider();
        assert_eq!(
            provider.signing_public_key().as_slice(),
            hex(&vector, "signerPublicKeyX963Hex")
        );
        let nonce: [u8; RECORD_NONCE_BYTES] = hex(&vector, "nonceHex").try_into().unwrap();
        let plaintext = BASE64
            .decode(vector["plaintextBase64"].as_str().unwrap())
            .unwrap();
        let sealed = provider
            .seal_record_with_nonce(&context, &plaintext, nonce)
            .unwrap();
        assert_eq!(
            sealed.ciphertext,
            BASE64
                .decode(vector["ciphertextBase64"].as_str().unwrap())
                .unwrap()
        );
        assert_eq!(
            sealed.context_digest.as_slice(),
            hex(&vector, "canonicalContextSha256Hex")
        );
        assert_eq!(
            sealed.envelope_digest.as_slice(),
            hex(&vector, "envelopeDigestHex")
        );
        assert!(SanitizedFixtureRecordCrypto::verify_p256_p1363(
            &provider.signing_public_key(),
            &record_signature_message(&sealed.envelope_digest),
            &sealed.record_signature,
        )
        .unwrap());

        let vector_sealed = RecordCiphertextV1 {
            version: 1,
            cipher_suite: RECORD_CIPHER_SUITE.to_owned(),
            nonce: nonce.to_vec(),
            ciphertext: BASE64
                .decode(vector["ciphertextBase64"].as_str().unwrap())
                .unwrap(),
            context_digest: hex(&vector, "canonicalContextSha256Hex")
                .try_into()
                .unwrap(),
            envelope_digest: hex(&vector, "envelopeDigestHex").try_into().unwrap(),
            record_signature: hex(&vector, "recordSignatureP1363Hex"),
        };
        assert_eq!(
            provider
                .open_record(&context, &vector_sealed, &provider.signing_public_key(),)
                .unwrap()
                .plaintext,
            plaintext
        );
    }

    #[test]
    fn outer_signatures_are_p1363_and_require_the_supplied_writer_key() {
        let provider = provider();
        let message = b"outer mutation signing bytes";
        let signature = provider.sign_p256_p1363(message).unwrap();
        assert!(SanitizedFixtureRecordCrypto::verify_p256_p1363(
            &provider.signing_public_key(),
            message,
            &signature,
        )
        .unwrap());
        assert!(!SanitizedFixtureRecordCrypto::verify_p256_p1363(
            &provider.signing_public_key(),
            b"other mutation signing bytes",
            &signature,
        )
        .unwrap());

        let sealed = provider.seal_record(&context(), b"signed").unwrap();
        let other = SigningKey::from_slice(&[9_u8; 32]).unwrap();
        assert!(provider
            .open_record(
                &context(),
                &sealed,
                other.verifying_key().to_encoded_point(false).as_bytes(),
            )
            .is_err());
    }

    #[test]
    fn fixture_policy_and_secret_inputs_fail_closed() {
        let library = || Zeroizing::new([7_u8; 32]);
        let signing = || Zeroizing::new([8_u8; 32]);
        assert!(SanitizedFixtureRecordCrypto::new(
            metadata("production", "sanitized_fixture"),
            library(),
            signing(),
        )
        .is_err());
        assert!(SanitizedFixtureRecordCrypto::new(
            metadata("development", "personal"),
            library(),
            signing(),
        )
        .is_err());
        assert!(SanitizedFixtureRecordCrypto::new(
            metadata("development", "sanitized_fixture"),
            Zeroizing::new([0_u8; 32]),
            signing(),
        )
        .is_err());
        assert!(SanitizedFixtureRecordCrypto::new(
            metadata("development", "sanitized_fixture"),
            library(),
            Zeroizing::new([0_u8; 32]),
        )
        .is_err());
    }

    #[test]
    fn wrong_context_tamper_and_oversize_are_rejected() {
        let provider = provider();
        let context = context();
        let sealed = provider.seal_record(&context, b"bound").unwrap();

        let mut wrong_context = context.clone();
        wrong_context.key_epoch += 1;
        assert!(provider
            .open_record(&wrong_context, &sealed, &provider.signing_public_key(),)
            .is_err());
        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 1;
        assert!(provider
            .open_record(&context, &tampered, &provider.signing_public_key())
            .is_err());
        let mut signature_tampered = sealed;
        signature_tampered.record_signature[0] ^= 1;
        assert!(provider
            .open_record(
                &context,
                &signature_tampered,
                &provider.signing_public_key(),
            )
            .is_err());
        assert!(provider
            .seal_record(&context, &vec![0_u8; MAX_RECORD_PLAINTEXT_BYTES + 1])
            .is_err());
        assert!(provider
            .sign_p256_p1363(&vec![0_u8; MAX_SIGNING_MESSAGE_BYTES + 1])
            .is_err());
    }

    fn hex(value: &Value, key: &str) -> Vec<u8> {
        let value = value[key].as_str().unwrap();
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}
