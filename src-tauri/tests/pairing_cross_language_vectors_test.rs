#![allow(dead_code)]

#[path = "../src/pairing_protocol.rs"]
mod pairing_protocol;
#[path = "../src/portable.rs"]
mod portable;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, setup_receiver, setup_sender,
    Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable,
};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use pairing_protocol::{
    bootstrap_associated_data, bootstrap_envelope_digest, bootstrap_hpke_exporter_context,
    bootstrap_hpke_info, canonical_authenticated_hpke_envelope, canonical_bootstrap_metadata,
    canonical_challenge_plaintext, canonical_receipt, challenge_hpke_exporter_context,
    challenge_hpke_info, derive_verification_code, pairing_transcript_digest,
    sanitized_fixture_key_package, AuthenticatedHpkeEnvelope, BootstrapEnvelope,
    BootstrapMetadataV1, EnrollmentReceipt, Environment, KindCapability, LibraryDataClass,
    PairingRole, RecordKind, ScopeClass, BOOTSTRAP_KEY_PACKAGE_BYTES, PAIRING_PROTOCOL,
    PAIRING_SUITE, RECORD_CIPHER_SUITE,
};
use rand_core::{CryptoRng, RngCore};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const FIXTURE: &str = include_str!("fixtures/pairing_v1_canonical.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("cross-language vector JSON must parse")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing vector field {key}"))
}

fn text(value: &Value, key: &str) -> String {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("vector field {key} must be text"))
        .to_owned()
}

fn hex_bytes(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "hex input must have an even length");
    (0..value.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&value[offset..offset + 2], 16)
                .unwrap_or_else(|_| panic!("invalid hex at byte {offset}"))
        })
        .collect()
}

fn hex_field(value: &Value, key: &str) -> Vec<u8> {
    hex_bytes(&text(value, key))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encode_base64(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn canonical_components(domain: &str, fields: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_field(&mut bytes, "domain", domain.as_bytes());
    for (label, value) in fields {
        append_field(&mut bytes, label, value);
    }
    bytes
}

fn append_field(output: &mut Vec<u8>, label: &str, value: &[u8]) {
    output.extend_from_slice(&(label.len() as u32).to_be_bytes());
    output.extend_from_slice(label.as_bytes());
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn record_kind(name: &str) -> RecordKind {
    match name {
        "note" => RecordKind::Note,
        "category" => RecordKind::Category,
        "folder" => RecordKind::Folder,
        "media" => RecordKind::Media,
        _ => panic!("unknown record kind {name}"),
    }
}

fn role(name: &str) -> PairingRole {
    match name {
        "mac_authority" => PairingRole::MacAuthority,
        "iphone_companion" => PairingRole::IphoneCompanion,
        _ => panic!("unknown pairing role {name}"),
    }
}

fn receipt_from_vector(root: &Value, transcript_digest: Vec<u8>) -> EnrollmentReceipt {
    let canonical = field(field(root, "cross_language"), "canonical");
    let receipt = field(canonical, "receipt");
    let granted_scopes = field(receipt, "granted_scopes")
        .as_array()
        .expect("granted_scopes must be an array")
        .iter()
        .map(|value| record_kind(value.as_str().expect("scope must be text")))
        .collect::<BTreeSet<_>>();
    let capabilities = field(receipt, "capabilities")
        .as_array()
        .expect("capabilities must be an array")
        .iter()
        .map(|value| {
            let kind = record_kind(
                field(value, "kind")
                    .as_str()
                    .expect("capability kind must be text"),
            );
            let reader_version = field(value, "reader_version")
                .as_u64()
                .expect("reader version must be unsigned") as u32;
            let writer_version = field(value, "writer_version").as_u64().map(|v| v as u32);
            (
                kind,
                KindCapability {
                    reader_version,
                    writer_version,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    EnrollmentReceipt {
        protocol: text(root, "protocol"),
        suite: text(root, "suite"),
        receipt_id: text(receipt, "receipt_id"),
        invitation_id: text(receipt, "invitation_id"),
        library_id: text(receipt, "library_id"),
        device_id: text(receipt, "device_id"),
        client_signing_key_fingerprint: hex_field(receipt, "client_signing_key_fingerprint_hex"),
        client_hpke_key_fingerprint: hex_field(receipt, "client_hpke_key_fingerprint_hex"),
        mac_signing_key_fingerprint: hex_field(receipt, "mac_signing_key_fingerprint_hex"),
        mac_hpke_key_fingerprint: hex_field(receipt, "mac_hpke_key_fingerprint_hex"),
        granted_scopes,
        capabilities,
        authority_generation: field(receipt, "authority_generation")
            .as_u64()
            .expect("authority generation must be unsigned"),
        created_at_ms: field(receipt, "created_at_ms")
            .as_i64()
            .expect("created_at_ms must be signed"),
        expires_at_ms: field(receipt, "expires_at_ms")
            .as_i64()
            .expect("expires_at_ms must be signed"),
        transcript_digest,
        environment: match text(receipt, "environment").as_str() {
            "development" => Environment::Development,
            "production" => Environment::Production,
            value => panic!("unknown environment {value}"),
        },
        mac_role: role(&text(receipt, "mac_role")),
        client_role: role(&text(receipt, "client_role")),
    }
}

fn canonical_material(root: &Value) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let canonical = field(field(root, "cross_language"), "canonical");
    let invitation_digest = hex_field(canonical, "invitation_digest_hex");
    let client_hello_digest = hex_field(canonical, "client_hello_digest_hex");
    let server_nonce = hex_field(canonical, "server_nonce_hex");
    let proposal = receipt_from_vector(root, Vec::new());
    let receipt_proposal_bytes = canonical_receipt(&proposal);
    let transcript_bytes = canonical_components(
        "noted.direct-pairing.v1/transcript",
        &[
            ("invitation_digest", &invitation_digest),
            ("client_hello_digest", &client_hello_digest),
            ("server_nonce", &server_nonce),
            ("receipt_proposal", &receipt_proposal_bytes),
        ],
    );
    let transcript_digest = pairing_transcript_digest(
        &invitation_digest,
        &client_hello_digest,
        &server_nonce,
        &proposal,
    );
    assert_eq!(transcript_digest, sha256(&transcript_bytes));
    (receipt_proposal_bytes, transcript_bytes, transcript_digest)
}

fn bootstrap_metadata_from_vector(
    root: &Value,
    receipt: &EnrollmentReceipt,
) -> BootstrapMetadataV1 {
    let contract = field(field(root, "cross_language"), "bootstrap_contract");
    let metadata = field(contract, "metadata");
    let granted_scopes = field(metadata, "granted_scopes")
        .as_array()
        .expect("bootstrap granted_scopes must be an array")
        .iter()
        .map(|value| record_kind(value.as_str().expect("bootstrap scope must be text")))
        .collect::<BTreeSet<_>>();
    let capabilities = field(metadata, "capabilities")
        .as_object()
        .expect("bootstrap capabilities must be an object")
        .iter()
        .map(|(kind, value)| {
            (
                record_kind(kind),
                KindCapability {
                    reader_version: field(value, "reader_version")
                        .as_u64()
                        .expect("bootstrap reader version must be unsigned")
                        as u32,
                    writer_version: field(value, "writer_version").as_u64().map(|v| v as u32),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let parsed = BootstrapMetadataV1 {
        version: field(metadata, "version")
            .as_u64()
            .expect("metadata version") as u32,
        protocol: text(metadata, "protocol"),
        suite: text(metadata, "suite"),
        sync_protocol_version: field(metadata, "sync_protocol_version")
            .as_u64()
            .expect("sync protocol version") as u32,
        environment: match text(metadata, "environment").as_str() {
            "development" => Environment::Development,
            "production" => Environment::Production,
            value => panic!("unknown bootstrap environment {value}"),
        },
        library_data_class: match text(metadata, "library_data_class").as_str() {
            "sanitized_fixture" => LibraryDataClass::SanitizedFixture,
            "personal" => LibraryDataClass::Personal,
            value => panic!("unknown bootstrap library data class {value}"),
        },
        receipt_id: text(metadata, "receipt_id"),
        library_id: text(metadata, "library_id"),
        device_id: text(metadata, "device_id"),
        authority_generation: field(metadata, "authority_generation")
            .as_u64()
            .expect("authority generation"),
        purge_generation: field(metadata, "purge_generation")
            .as_u64()
            .expect("purge generation"),
        key_epoch: field(metadata, "key_epoch").as_u64().expect("key epoch"),
        default_scope_id: text(metadata, "default_scope_id"),
        default_scope_class: match text(metadata, "default_scope_class").as_str() {
            "unknown" => ScopeClass::Unknown,
            "work" => ScopeClass::Work,
            "personal" => ScopeClass::Personal,
            value => panic!("unknown default scope class {value}"),
        },
        granted_scopes,
        capabilities,
        record_cipher_suite: text(metadata, "record_cipher_suite"),
        durable_sync_spki_sha256: hex_field(metadata, "durable_sync_spki_sha256_hex"),
        transcript_digest: hex_field(metadata, "transcript_digest_hex"),
    };
    assert_eq!(parsed.protocol, text(root, "protocol"));
    assert_eq!(parsed.suite, text(root, "suite"));
    assert_eq!(parsed.receipt_id, receipt.receipt_id);
    assert_eq!(parsed.library_id, receipt.library_id);
    assert_eq!(parsed.device_id, receipt.device_id);
    assert_eq!(parsed.authority_generation, receipt.authority_generation);
    assert_eq!(parsed.granted_scopes, receipt.granted_scopes);
    assert_eq!(parsed.capabilities, receipt.capabilities);
    assert_eq!(parsed.transcript_digest, receipt.transcript_digest);
    assert_eq!(parsed.record_cipher_suite, RECORD_CIPHER_SUITE);
    parsed
}

/// Known-answer-test input only. Implementing `CryptoRng` is deliberately
/// confined to this integration-test binary so `setup_sender` can reproduce a
/// published artifact byte for byte. Production code must always use fresh OS
/// randomness and cannot import this type.
struct DeterministicKatRng {
    bytes: Vec<u8>,
    offset: usize,
}

impl DeterministicKatRng {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, offset: 0 }
    }
}

impl RngCore for DeterministicKatRng {
    fn next_u32(&mut self) -> u32 {
        let mut output = [0; 4];
        self.fill_bytes(&mut output);
        u32::from_le_bytes(output)
    }

    fn next_u64(&mut self) -> u64 {
        let mut output = [0; 8];
        self.fill_bytes(&mut output);
        u64::from_le_bytes(output)
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        let end = self
            .offset
            .checked_add(destination.len())
            .expect("KAT byte offset must not overflow");
        destination.copy_from_slice(
            self.bytes
                .get(self.offset..end)
                .expect("HPKE KAT requested more bytes than its fixed ephemeral IKM"),
        );
        self.offset = end;
    }
}

impl CryptoRng for DeterministicKatRng {}

struct ProtocolSeal {
    envelope: AuthenticatedHpkeEnvelope,
    exported_value: Vec<u8>,
}

fn reproduce_protocol_seal(
    noted_hpke: &Value,
    vector: &Value,
    info: &[u8],
    associated_data: &[u8],
    plaintext: &[u8],
    exporter_context: &[u8],
) -> ProtocolSeal {
    type Kem = X25519HkdfSha256;

    let sender_private = <Kem as KemTrait>::PrivateKey::from_bytes(&hex_field(
        noted_hpke,
        "test_only_sender_auth_private_key_hex",
    ))
    .expect("test-only sender authentication private key must decode");
    let sender_public = <Kem as KemTrait>::sk_to_pk(&sender_private);
    assert_eq!(
        sender_public.to_bytes().as_slice(),
        hex_field(noted_hpke, "sender_auth_public_key_hex")
    );
    let recipient_private = <Kem as KemTrait>::PrivateKey::from_bytes(&hex_field(
        noted_hpke,
        "test_only_recipient_private_key_hex",
    ))
    .expect("test-only recipient private key must decode");
    let recipient_public = <Kem as KemTrait>::sk_to_pk(&recipient_private);
    assert_eq!(
        recipient_public.to_bytes().as_slice(),
        hex_field(noted_hpke, "recipient_public_key_hex")
    );

    let mut rng = DeterministicKatRng::new(hex_field(vector, "test_only_ephemeral_ikm_hex"));
    let (encapsulated_key, mut sender) = setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
        &OpModeS::Auth((sender_private, sender_public.clone())),
        &recipient_public,
        info,
        &mut rng,
    )
    .expect("test-only authenticated HPKE sender context must initialize");
    let ciphertext = sender
        .seal(plaintext, associated_data)
        .expect("test-only authenticated HPKE seal must succeed");
    let mut sender_export = vec![0; 32];
    sender
        .export(exporter_context, &mut sender_export)
        .expect("test-only authenticated HPKE export must succeed");
    let envelope = AuthenticatedHpkeEnvelope {
        encapsulated_key: encapsulated_key.to_bytes().to_vec(),
        ciphertext,
    };

    let mut recipient = setup_receiver::<AesGcm256, HkdfSha256, Kem>(
        &OpModeR::Auth(sender_public),
        &recipient_private,
        &<Kem as KemTrait>::EncappedKey::from_bytes(&envelope.encapsulated_key)
            .expect("generated encapsulated key must decode"),
        info,
    )
    .expect("test-only authenticated HPKE recipient context must initialize");
    assert_eq!(
        recipient
            .open(&envelope.ciphertext, associated_data)
            .expect("generated authenticated HPKE ciphertext must open"),
        plaintext
    );
    let mut recipient_export = vec![0; sender_export.len()];
    recipient
        .export(exporter_context, &mut recipient_export)
        .expect("test-only authenticated HPKE recipient export must succeed");
    assert_eq!(recipient_export, sender_export);

    ProtocolSeal {
        envelope,
        exported_value: sender_export,
    }
}

#[test]
fn canonical_transcript_receipt_and_sas_match_the_shared_vector() {
    let root = fixture();
    assert_eq!(
        text(&root, "fixture_class"),
        "sanitized_cross_language_golden_vectors"
    );
    assert_eq!(text(&root, "protocol"), PAIRING_PROTOCOL);
    assert_eq!(text(&root, "suite"), PAIRING_SUITE);
    let cross_language = field(&root, "cross_language");
    assert_eq!(field(cross_language, "artifact_version").as_u64(), Some(1));
    let canonical = field(cross_language, "canonical");

    let (proposal_bytes, transcript_bytes, transcript_digest) = canonical_material(&root);
    let receipt = receipt_from_vector(&root, transcript_digest.clone());
    let receipt_bytes = canonical_receipt(&receipt);
    let receipt_digest = sha256(&receipt_bytes);

    assert!(!proposal_bytes.is_empty());
    assert_eq!(
        text(canonical, "transcript_canonical_base64"),
        encode_base64(&transcript_bytes)
    );
    assert_eq!(
        text(canonical, "transcript_digest_hex"),
        encode_hex(&transcript_digest)
    );
    assert_eq!(
        text(canonical, "receipt_canonical_base64"),
        encode_base64(&receipt_bytes)
    );
    assert_eq!(
        text(canonical, "receipt_digest_hex"),
        encode_hex(&receipt_digest)
    );

    let sas = field(cross_language, "sas");
    let hpke = field(field(cross_language, "noted_hpke"), "challenge");
    let verification_code =
        derive_verification_code(&hex_field(hpke, "exported_value_hex"), &transcript_digest);
    assert_eq!(text(sas, "verification_code"), verification_code);
}

#[test]
fn noted_challenge_and_bootstrap_hpke_boundaries_match_the_shared_vector() {
    let root = fixture();
    let cross_language = field(&root, "cross_language");
    let noted_hpke = field(cross_language, "noted_hpke");
    let challenge_vector = field(noted_hpke, "challenge");
    let bootstrap_vector = field(noted_hpke, "bootstrap");
    let (_, _, transcript_digest) = canonical_material(&root);
    let receipt = receipt_from_vector(&root, transcript_digest.clone());

    let challenge = reproduce_protocol_seal(
        noted_hpke,
        challenge_vector,
        &challenge_hpke_info(&receipt),
        &transcript_digest,
        &canonical_challenge_plaintext(&receipt),
        &challenge_hpke_exporter_context(&receipt),
    );

    let metadata = bootstrap_metadata_from_vector(&root, &receipt);
    let metadata_canonical = canonical_bootstrap_metadata(&metadata);
    let bootstrap_plaintext = sanitized_fixture_key_package(metadata.key_epoch);
    assert_eq!(bootstrap_plaintext.len(), BOOTSTRAP_KEY_PACKAGE_BYTES);
    let bootstrap = reproduce_protocol_seal(
        noted_hpke,
        bootstrap_vector,
        &bootstrap_hpke_info(&metadata),
        &bootstrap_associated_data(&metadata),
        bootstrap_plaintext.as_slice(),
        &bootstrap_hpke_exporter_context(&metadata),
    );

    for (vector, seal) in [
        (challenge_vector, &challenge),
        (bootstrap_vector, &bootstrap),
    ] {
        let envelope_digest = sha256(&canonical_authenticated_hpke_envelope(&seal.envelope));
        assert_eq!(
            text(vector, "encapsulated_key_hex"),
            encode_hex(&seal.envelope.encapsulated_key)
        );
        assert_eq!(
            text(vector, "ciphertext_hex"),
            encode_hex(&seal.envelope.ciphertext)
        );
        assert_eq!(
            text(vector, "exported_value_hex"),
            encode_hex(&seal.exported_value)
        );
        assert_eq!(
            text(vector, "envelope_digest_hex"),
            encode_hex(&envelope_digest)
        );
    }
    let contract = field(cross_language, "bootstrap_contract");
    assert_eq!(
        text(contract, "metadata_canonical_base64"),
        encode_base64(&metadata_canonical)
    );
    assert_eq!(
        text(contract, "metadata_digest_hex"),
        encode_hex(&sha256(&metadata_canonical))
    );
    let mut envelope = BootstrapEnvelope {
        protocol: PAIRING_PROTOCOL.to_owned(),
        receipt_id: receipt.receipt_id.clone(),
        metadata,
        sealed_key_package: bootstrap.envelope.clone(),
        envelope_digest: Vec::new(),
    };
    envelope.envelope_digest = bootstrap_envelope_digest(&envelope);
    assert_eq!(
        text(bootstrap_vector, "key_package_sha256_hex"),
        encode_hex(&sha256(bootstrap_plaintext.as_slice()))
    );
    assert_eq!(
        text(bootstrap_vector, "bootstrap_envelope_digest_hex"),
        encode_hex(&envelope.envelope_digest)
    );
}

#[test]
fn p256_sha256_p1363_signature_verifies_the_canonical_transcript() {
    let root = fixture();
    let cross_language = field(&root, "cross_language");
    let signature_vector = field(cross_language, "signature");
    assert_eq!(
        text(signature_vector, "algorithm"),
        "ecdsa-p256-sha256-p1363"
    );
    assert_eq!(text(signature_vector, "message"), "canonical_transcript");
    assert!(signature_vector.get("private_key_hex").is_none());

    let (_, transcript_bytes, _) = canonical_material(&root);
    let verifying_key =
        VerifyingKey::from_sec1_bytes(&hex_field(signature_vector, "public_key_x963_hex"))
            .expect("P-256 public key must be SEC1/X9.63 uncompressed form");
    let signature = Signature::from_slice(&hex_field(signature_vector, "signature_p1363_hex"))
        .expect("signature must be a 64-byte IEEE P1363 r||s value");
    verifying_key
        .verify(&transcript_bytes, &signature)
        .expect("valid cross-language signature must verify");

    let mut tampered = transcript_bytes;
    tampered[0] ^= 1;
    assert!(verifying_key.verify(&tampered, &signature).is_err());
}

#[test]
fn rfc9180_authenticated_hpke_vector_opens_and_exports() {
    type Kem = X25519HkdfSha256;

    let root = fixture();
    let hpke = field(field(&root, "cross_language"), "hpke");
    assert_eq!(field(hpke, "mode").as_u64(), Some(2));
    assert_eq!(field(hpke, "kem_id").as_u64(), Some(32));
    assert_eq!(field(hpke, "kdf_id").as_u64(), Some(1));
    assert_eq!(field(hpke, "aead_id").as_u64(), Some(2));

    let recipient_private =
        <Kem as KemTrait>::PrivateKey::from_bytes(&hex_field(hpke, "recipient_private_key_hex"))
            .expect("RFC recipient private key must decode");
    assert_eq!(
        <Kem as KemTrait>::sk_to_pk(&recipient_private)
            .to_bytes()
            .as_slice(),
        hex_field(hpke, "recipient_public_key_hex")
    );
    let sender_auth_public =
        <Kem as KemTrait>::PublicKey::from_bytes(&hex_field(hpke, "sender_auth_public_key_hex"))
            .expect("RFC sender authentication public key must decode");
    let encapsulated_key =
        <Kem as KemTrait>::EncappedKey::from_bytes(&hex_field(hpke, "encapsulated_key_hex"))
            .expect("RFC encapsulated key must decode");

    let mut recipient = setup_receiver::<AesGcm256, HkdfSha256, Kem>(
        &OpModeR::Auth(sender_auth_public),
        &recipient_private,
        &encapsulated_key,
        &hex_field(hpke, "info_hex"),
    )
    .expect("RFC authenticated recipient context must initialize");
    let plaintext = recipient
        .open(
            &hex_field(hpke, "ciphertext_hex"),
            &hex_field(hpke, "aad_hex"),
        )
        .expect("RFC ciphertext must authenticate and open");
    assert_eq!(plaintext, hex_field(hpke, "plaintext_hex"));

    let expected_export = hex_field(hpke, "exported_value_hex");
    let mut exported = vec![0; expected_export.len()];
    recipient
        .export(&hex_field(hpke, "exporter_context_hex"), &mut exported)
        .expect("RFC exporter must succeed");
    assert_eq!(exported, expected_export);

    let mut tampered_ciphertext = hex_field(hpke, "ciphertext_hex");
    tampered_ciphertext[0] ^= 1;
    let mut tampered_recipient = setup_receiver::<AesGcm256, HkdfSha256, Kem>(
        &OpModeR::Auth(
            <Kem as KemTrait>::PublicKey::from_bytes(&hex_field(
                hpke,
                "sender_auth_public_key_hex",
            ))
            .unwrap(),
        ),
        &recipient_private,
        &encapsulated_key,
        &hex_field(hpke, "info_hex"),
    )
    .unwrap();
    assert!(tampered_recipient
        .open(&tampered_ciphertext, &hex_field(hpke, "aad_hex"))
        .is_err());
}
