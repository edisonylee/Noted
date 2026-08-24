use std::convert::TryFrom;

use sha2::{Digest, Sha256};
use tauri_app_lib::{
    mobile_store::{MobilePairingActivation, MobilePairingCheckpoint, MobileStore},
    pairing_client::{
        ClientPublicIdentity, PairingActivation, PairingClientCheckpoint, PairingClientConfig,
        PairingClientState, PairingConfirmation,
    },
    pairing_protocol::{
        bootstrap_envelope_digest, fixture_record_capabilities, fixture_record_scopes,
        AuthenticatedHpkeEnvelope, BootstrapEnvelope, BootstrapMetadataV1, EnrollmentReceipt,
        Environment, Invitation, LibraryDataClass, PairingRole, ScopeClass, ServerFinish,
        ServerHello, BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES, BOOTSTRAP_METADATA_VERSION,
        BOOTSTRAP_SYNC_PROTOCOL_VERSION, HPKE_ENCAPSULATED_KEY_BYTES, PAIRING_PROTOCOL,
        PAIRING_SUITE, RECORD_CIPHER_SUITE,
    },
    portable::new_uuid_v7,
};

const IDENTITY_HANDLE: &str = "018f47a0-7b80-4000-8000-000000000001";
const PENDING_HANDLE: &str = "018f47a0-7b80-4000-8000-000000000002";
const FIXTURE_TIME_MS: i64 = 1_725_000_000_000;

pub fn finalize_fixture_pairing(
    store: &MobileStore,
    library_id: &str,
    default_scope_id: &str,
    authority_generation: i64,
    purge_generation: i64,
) -> usize {
    let authority_generation_u64 =
        u64::try_from(authority_generation).expect("positive fixture authority generation");
    let purge_generation_u64 =
        u64::try_from(purge_generation).expect("nonnegative fixture purge generation");
    let device_id = store
        .replica_device_id()
        .expect("fixture replica device ID");
    let receipt_id = new_uuid_v7();
    let invitation_id = new_uuid_v7();
    let scopes = fixture_record_scopes();
    let capabilities = fixture_record_capabilities();
    let transcript_digest = vec![5; 32];
    let receipt = EnrollmentReceipt {
        protocol: PAIRING_PROTOCOL.to_string(),
        suite: PAIRING_SUITE.to_string(),
        receipt_id: receipt_id.clone(),
        invitation_id: invitation_id.clone(),
        library_id: library_id.to_string(),
        device_id: device_id.clone(),
        client_signing_key_fingerprint: sha256(&p256_fixture_key(4)),
        client_hpke_key_fingerprint: sha256(&[7; HPKE_ENCAPSULATED_KEY_BYTES]),
        mac_signing_key_fingerprint: sha256(&p256_fixture_key(5)),
        mac_hpke_key_fingerprint: sha256(&[6; HPKE_ENCAPSULATED_KEY_BYTES]),
        granted_scopes: scopes.clone(),
        capabilities: capabilities.clone(),
        authority_generation: authority_generation_u64,
        created_at_ms: FIXTURE_TIME_MS,
        expires_at_ms: FIXTURE_TIME_MS + 60_000,
        transcript_digest: transcript_digest.clone(),
        environment: Environment::Development,
        mac_role: PairingRole::MacAuthority,
        client_role: PairingRole::IphoneCompanion,
    };
    let invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_string(),
        suite: PAIRING_SUITE.to_string(),
        invitation_id,
        invitation_nonce: vec![6; 32],
        authority_signing_public_key: p256_fixture_key(4),
        mac_pairing_signing_public_key: p256_fixture_key(5),
        mac_pairing_hpke_public_key: vec![6; HPKE_ENCAPSULATED_KEY_BYTES],
        tls_spki_sha256: vec![7; 32],
        library_id: library_id.to_string(),
        authority_generation: authority_generation_u64,
        scope_ceiling: scopes.clone(),
        created_at_ms: FIXTURE_TIME_MS,
        expires_at_ms: FIXTURE_TIME_MS + 60_000,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: vec![8; 64],
    };
    let server_hello = ServerHello {
        protocol: PAIRING_PROTOCOL.to_string(),
        suite: PAIRING_SUITE.to_string(),
        server_nonce: vec![9; 32],
        receipt: receipt.clone(),
        challenge: AuthenticatedHpkeEnvelope {
            encapsulated_key: vec![10; HPKE_ENCAPSULATED_KEY_BYTES],
            ciphertext: vec![11; 32],
        },
        sender_role: PairingRole::MacAuthority,
        recipient_role: PairingRole::IphoneCompanion,
        proof_signature: vec![12; 64],
    };
    let sync_spki_sha256 = vec![13; 32];
    let metadata = BootstrapMetadataV1 {
        version: BOOTSTRAP_METADATA_VERSION,
        protocol: PAIRING_PROTOCOL.to_string(),
        suite: PAIRING_SUITE.to_string(),
        sync_protocol_version: BOOTSTRAP_SYNC_PROTOCOL_VERSION,
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        receipt_id: receipt_id.clone(),
        library_id: library_id.to_string(),
        device_id: device_id.clone(),
        authority_generation: authority_generation_u64,
        purge_generation: purge_generation_u64,
        key_epoch: 1,
        default_scope_id: default_scope_id.to_string(),
        default_scope_class: ScopeClass::Unknown,
        granted_scopes: scopes.clone(),
        capabilities: capabilities.clone(),
        record_cipher_suite: RECORD_CIPHER_SUITE.to_string(),
        durable_sync_spki_sha256: sync_spki_sha256.clone(),
        transcript_digest,
    };
    let mut bootstrap = BootstrapEnvelope {
        protocol: PAIRING_PROTOCOL.to_string(),
        receipt_id: receipt_id.clone(),
        metadata,
        sealed_key_package: AuthenticatedHpkeEnvelope {
            encapsulated_key: vec![14; HPKE_ENCAPSULATED_KEY_BYTES],
            ciphertext: vec![15; BOOTSTRAP_KEY_PACKAGE_CIPHERTEXT_BYTES],
        },
        envelope_digest: Vec::new(),
    };
    bootstrap.envelope_digest = bootstrap_envelope_digest(&bootstrap);
    let activated_at_ms = FIXTURE_TIME_MS + 20_000;
    let server_finish = ServerFinish {
        protocol: PAIRING_PROTOCOL.to_string(),
        suite: PAIRING_SUITE.to_string(),
        receipt: receipt.clone(),
        activated_at_ms,
        sender_role: PairingRole::MacAuthority,
        recipient_role: PairingRole::IphoneCompanion,
        signature: vec![16; 64],
    };
    let active_checkpoint = MobilePairingCheckpoint {
        identity_handle: IDENTITY_HANDLE.to_string(),
        pending_bootstrap_handle: None,
        client: PairingClientCheckpoint {
            version: 1,
            config: PairingClientConfig {
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
                requested_scopes: scopes.clone(),
                capabilities: capabilities.clone(),
                display_name: "Fixture iPhone".to_string(),
                app_version: "0.1.0".to_string(),
                build_version: "1".to_string(),
            },
            state: PairingClientState::Active,
            invitation_bytes: serde_json::to_vec(&invitation).expect("encode invitation"),
            identity: ClientPublicIdentity {
                device_id: device_id.clone(),
                signing_public_key: p256_fixture_key(4),
                hpke_public_key: vec![7; HPKE_ENCAPSULATED_KEY_BYTES],
            },
            client_hello_bytes: Some(br#"{"fixture":"client-hello"}"#.to_vec()),
            server_hello_bytes: Some(
                serde_json::to_vec(&server_hello).expect("encode server hello"),
            ),
            confirmation: Some(PairingConfirmation {
                receipt_id: receipt_id.clone(),
                verification_code: "12345678".to_string(),
                granted_scopes: scopes.clone(),
            }),
            user_decision: Some(true),
            bootstrap_bytes: Some(serde_json::to_vec(&bootstrap).expect("encode bootstrap")),
            client_finish_bytes: Some(br#"{"fixture":"client-finish"}"#.to_vec()),
            server_finish_bytes: Some(
                serde_json::to_vec(&server_finish).expect("encode server finish"),
            ),
            activation: Some(PairingActivation {
                receipt,
                activated_at_ms,
            }),
        },
        updated_at: FIXTURE_TIME_MS + 30_000,
    };
    let activation = MobilePairingActivation {
        receipt_id,
        library_id: library_id.to_string(),
        device_id,
        default_scope_id: default_scope_id.to_string(),
        authority_generation,
        purge_generation,
        key_epoch: 1,
        sync_spki_sha256,
        record_cipher_suite: RECORD_CIPHER_SUITE.to_string(),
        granted_scopes: scopes,
        capabilities,
        checkpoint: active_checkpoint,
    };
    let mut pending = activation.checkpoint.clone();
    pending.client.state = PairingClientState::PendingActivation;
    pending.client.activation = None;
    pending.pending_bootstrap_handle = Some(PENDING_HANDLE.to_string());
    pending.updated_at -= 1;
    store
        .save_pairing_checkpoint(&pending)
        .expect("save exact PendingActivation predecessor");
    store
        .finalize_pairing_activation(&activation)
        .expect("atomically finalize fixture pairing")
        .adopted_note_count
}

fn p256_fixture_key(fill: u8) -> Vec<u8> {
    let mut key = vec![fill; 65];
    key[0] = 4;
    key
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
