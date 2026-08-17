use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri_app_lib::pairing_client::{
    ClientFreshValuePurpose, ClientPublicIdentity, OpenedPairingChallenge, PairingActivation,
    PairingClient, PairingClientConfig, PairingClientCrypto, PairingClientError,
    PairingClientState,
};
use tauri_app_lib::pairing_protocol::{
    canonical_client_hello_unsigned, canonical_invitation_unsigned, AuthenticatedHpkeEnvelope,
    AuthenticatedHpkeSeal, BootstrapEnvelope, Environment, FreshValuePurpose, Invitation,
    KindCapability, LibraryDataClass, LocalHpkeKey, LocalSigningKey, PairingCrypto, PairingError,
    PairingMachine, PairingPolicy, PairingRole, RecordKind, TransportEvidence,
    HPKE_EXPORTER_SECRET_BYTES, MAX_INVITATION_LIFETIME_MS, MAX_PAIRING_MESSAGE_BYTES,
    PAIRING_PROTOCOL, PAIRING_SUITE,
};
use zeroize::Zeroizing;

const NOW: i64 = 1_725_000_000_000;
const INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000101";
const HELLO_ID: &str = "018f47a0-7b80-7000-8000-000000000102";
const DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000103";
const FINISH_ID: &str = "018f47a0-7b80-7000-8000-000000000104";
const RECEIPT_ID: &str = "018f47a0-7b80-7000-8000-000000000105";
const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000106";

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn signature(public_key: &[u8], message: &[u8]) -> Vec<u8> {
    let digest = sha256(&[b"noted.fixture/signature", public_key, message]);
    [digest.as_slice(), digest.as_slice()].concat()
}

fn mac_authority_signing_key() -> Vec<u8> {
    let mut key = vec![0x11; 65];
    key[0] = 4;
    key
}

fn mac_pairing_signing_key() -> Vec<u8> {
    let mut key = vec![0x22; 65];
    key[0] = 4;
    key
}

fn mac_pairing_hpke_key() -> Vec<u8> {
    vec![0x33; 32]
}

fn client_signing_key() -> Vec<u8> {
    let mut key = vec![0x44; 65];
    key[0] = 4;
    key
}

fn client_hpke_key() -> Vec<u8> {
    vec![0x55; 32]
}

fn fixture_seal(
    sender_public_key: &[u8],
    recipient_public_key: &[u8],
    info: &[u8],
    associated_data: &[u8],
    plaintext: &[u8],
    exporter_context: &[u8],
) -> AuthenticatedHpkeSeal {
    let encapsulated_key = sha256(&[
        b"noted.fixture/auth-hpke/encapsulated-key",
        recipient_public_key,
        info,
    ]);
    let tag = sha256(&[
        b"noted.fixture/auth-hpke/tag",
        sender_public_key,
        recipient_public_key,
        &encapsulated_key,
        info,
        associated_data,
        plaintext,
    ]);
    let mut ciphertext = plaintext.to_vec();
    ciphertext.extend_from_slice(&tag);
    let exporter_secret = sha256(&[
        b"noted.fixture/auth-hpke/exporter",
        &encapsulated_key,
        &ciphertext,
        exporter_context,
    ]);
    AuthenticatedHpkeSeal {
        envelope: AuthenticatedHpkeEnvelope {
            encapsulated_key: encapsulated_key.to_vec(),
            ciphertext,
        },
        exporter_secret: Zeroizing::new(exporter_secret),
    }
}

fn fixture_open(
    sender_public_key: &[u8],
    recipient_public_key: &[u8],
    info: &[u8],
    associated_data: &[u8],
    envelope: &AuthenticatedHpkeEnvelope,
) -> Result<Vec<u8>, ()> {
    let expected_encapsulated = sha256(&[
        b"noted.fixture/auth-hpke/encapsulated-key",
        recipient_public_key,
        info,
    ]);
    if envelope.encapsulated_key != expected_encapsulated || envelope.ciphertext.len() < 32 {
        return Err(());
    }
    let split = envelope.ciphertext.len() - 32;
    let (plaintext, tag) = envelope.ciphertext.split_at(split);
    let expected_tag = sha256(&[
        b"noted.fixture/auth-hpke/tag",
        sender_public_key,
        recipient_public_key,
        &expected_encapsulated,
        info,
        associated_data,
        plaintext,
    ]);
    if tag != expected_tag {
        return Err(());
    }
    Ok(plaintext.to_vec())
}

#[derive(Default)]
struct ServerFixtureCrypto;

impl PairingCrypto for ServerFixtureCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        observed: &[u8],
    ) -> Result<(), ()> {
        (observed == signature(public_key, message))
            .then_some(())
            .ok_or(())
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        let public_key = match key {
            LocalSigningKey::MacPairing => mac_pairing_signing_key(),
            LocalSigningKey::MacAuthority => mac_authority_signing_key(),
        };
        Ok(signature(&public_key, message))
    }

    fn seal_authenticated(
        &self,
        _sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        plaintext: &[u8],
        exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        Ok(fixture_seal(
            &mac_pairing_hpke_key(),
            recipient_public_key,
            info,
            associated_data,
            plaintext,
            exporter_context,
        ))
    }

    fn fresh_bytes(&self, purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        let byte = match purpose {
            FreshValuePurpose::ReceiptId => 0x61,
            FreshValuePurpose::ServerNonce => 0x62,
        };
        Ok(vec![byte; length])
    }

    fn fresh_uuid_v7(&self, purpose: FreshValuePurpose) -> Result<String, ()> {
        match purpose {
            FreshValuePurpose::ReceiptId => Ok(RECEIPT_ID.to_owned()),
            FreshValuePurpose::ServerNonce => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingKeyReference(String);

#[derive(Default)]
struct CustodyState {
    staged: BTreeMap<String, Vec<u8>>,
    active: BTreeSet<String>,
    discarded: BTreeSet<String>,
}

#[derive(Clone)]
struct ClientFixtureCrypto {
    custody: Arc<Mutex<CustodyState>>,
    open_calls: Arc<AtomicUsize>,
    stage_calls: Arc<AtomicUsize>,
    activation_calls: Arc<AtomicUsize>,
    discard_calls: Arc<AtomicUsize>,
    fail_activation_once: Arc<AtomicBool>,
    fail_discard_once: Arc<AtomicBool>,
}

impl ClientFixtureCrypto {
    fn new(fail_activation_once: bool, fail_discard_once: bool) -> Self {
        Self {
            custody: Arc::new(Mutex::new(CustodyState::default())),
            open_calls: Arc::new(AtomicUsize::new(0)),
            stage_calls: Arc::new(AtomicUsize::new(0)),
            activation_calls: Arc::new(AtomicUsize::new(0)),
            discard_calls: Arc::new(AtomicUsize::new(0)),
            fail_activation_once: Arc::new(AtomicBool::new(fail_activation_once)),
            fail_discard_once: Arc::new(AtomicBool::new(fail_discard_once)),
        }
    }
}

impl PairingClientCrypto for ClientFixtureCrypto {
    type PendingKeyReference = PendingKeyReference;

    fn public_identity(&self) -> Result<ClientPublicIdentity, ()> {
        Ok(ClientPublicIdentity {
            device_id: DEVICE_ID.to_owned(),
            signing_public_key: client_signing_key(),
            hpke_public_key: client_hpke_key(),
        })
    }

    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        observed: &[u8],
    ) -> Result<(), ()> {
        (observed == signature(public_key, message))
            .then_some(())
            .ok_or(())
    }

    fn sign_device(&self, message: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(signature(&client_signing_key(), message))
    }

    fn open_challenge_authenticated(
        &self,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        envelope: &AuthenticatedHpkeEnvelope,
        exporter_context: &[u8],
    ) -> Result<OpenedPairingChallenge, ()> {
        self.open_calls.fetch_add(1, Ordering::SeqCst);
        let plaintext = fixture_open(
            sender_public_key,
            &client_hpke_key(),
            info,
            associated_data,
            envelope,
        )?;
        let exporter_secret: [u8; HPKE_EXPORTER_SECRET_BYTES] = sha256(&[
            b"noted.fixture/auth-hpke/exporter",
            &envelope.encapsulated_key,
            &envelope.ciphertext,
            exporter_context,
        ]);
        Ok(OpenedPairingChallenge {
            plaintext: Zeroizing::new(plaintext),
            exporter_secret: Zeroizing::new(exporter_secret),
        })
    }

    fn stage_bootstrap_authenticated(
        &self,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        envelope: &AuthenticatedHpkeEnvelope,
        receipt: &tauri_app_lib::pairing_protocol::EnrollmentReceipt,
        envelope_digest: &[u8],
    ) -> Result<Self::PendingKeyReference, ()> {
        self.stage_calls.fetch_add(1, Ordering::SeqCst);
        let plaintext = fixture_open(
            sender_public_key,
            &client_hpke_key(),
            info,
            associated_data,
            envelope,
        )?;
        let reference = PendingKeyReference(receipt.receipt_id.clone());
        let mut custody = self.custody.lock().map_err(|_| ())?;
        if let Some(existing) = custody.staged.get(&reference.0) {
            if existing != &plaintext {
                return Err(());
            }
        } else {
            let mut bound = envelope_digest.to_vec();
            bound.extend_from_slice(&plaintext);
            custody.staged.insert(reference.0.clone(), bound);
        }
        Ok(reference)
    }

    fn activate_pending_bootstrap(
        &self,
        pending: &Self::PendingKeyReference,
        receipt: &tauri_app_lib::pairing_protocol::EnrollmentReceipt,
    ) -> Result<(), ()> {
        self.activation_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_activation_once.swap(false, Ordering::SeqCst) {
            return Err(());
        }
        if pending.0 != receipt.receipt_id {
            return Err(());
        }
        let mut custody = self.custody.lock().map_err(|_| ())?;
        if custody.active.contains(&pending.0) {
            return Ok(());
        }
        custody.staged.remove(&pending.0).ok_or(())?;
        custody.active.insert(pending.0.clone());
        Ok(())
    }

    fn discard_pending_bootstrap(&self, pending: &Self::PendingKeyReference) -> Result<(), ()> {
        self.discard_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_discard_once.swap(false, Ordering::SeqCst) {
            return Err(());
        }
        let mut custody = self.custody.lock().map_err(|_| ())?;
        custody.staged.remove(&pending.0);
        custody.discarded.insert(pending.0.clone());
        Ok(())
    }

    fn fresh_bytes(&self, purpose: ClientFreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        match purpose {
            ClientFreshValuePurpose::ClientNonce => Ok(vec![0x71; length]),
            _ => Err(()),
        }
    }

    fn fresh_uuid_v7(&self, purpose: ClientFreshValuePurpose) -> Result<String, ()> {
        match purpose {
            ClientFreshValuePurpose::ClientHelloMessageId => Ok(HELLO_ID.to_owned()),
            ClientFreshValuePurpose::ClientFinishMessageId => Ok(FINISH_ID.to_owned()),
            ClientFreshValuePurpose::ClientNonce => Err(()),
        }
    }
}

fn scopes() -> BTreeSet<RecordKind> {
    [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
        .into_iter()
        .collect()
}

fn capabilities() -> BTreeMap<RecordKind, KindCapability> {
    scopes()
        .into_iter()
        .map(|scope| {
            (
                scope,
                KindCapability {
                    reader_version: 1,
                    writer_version: Some(1),
                },
            )
        })
        .collect()
}

fn client_config() -> PairingClientConfig {
    PairingClientConfig {
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        requested_scopes: scopes(),
        capabilities: capabilities(),
        display_name: "Fixture iPhone".to_owned(),
        app_version: "0.1.0-fixture".to_owned(),
        build_version: "1".to_owned(),
    }
}

fn server_policy() -> PairingPolicy {
    PairingPolicy {
        library_id: LIBRARY_ID.to_owned(),
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_generation: 9,
        grantable_scopes: scopes(),
        capabilities: capabilities(),
    }
}

fn invitation() -> Invitation {
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: INVITATION_ID.to_owned(),
        invitation_nonce: vec![0x81; 32],
        authority_signing_public_key: mac_authority_signing_key(),
        mac_pairing_signing_public_key: mac_pairing_signing_key(),
        mac_pairing_hpke_public_key: mac_pairing_hpke_key(),
        tls_spki_sha256: vec![0x82; 32],
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: 9,
        scope_ceiling: scopes(),
        created_at_ms: NOW,
        expires_at_ms: NOW + MAX_INVITATION_LIFETIME_MS,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = signature(
        &invitation.authority_signing_public_key,
        &canonical_invitation_unsigned(&invitation),
    );
    invitation
}

fn transport(invitation: &Invitation) -> TransportEvidence {
    TransportEvidence {
        tls_version: "1.3".to_owned(),
        used_zero_rtt: false,
        peer_spki_sha256: invitation.tls_spki_sha256.clone(),
    }
}

fn pair(
    client_crypto: ClientFixtureCrypto,
) -> (
    PairingClient<ClientFixtureCrypto>,
    PairingMachine<ServerFixtureCrypto>,
    Invitation,
    TransportEvidence,
) {
    let invitation = invitation();
    let bytes = serde_json::to_vec(&invitation).unwrap();
    let client =
        PairingClient::new_fixture_only(client_crypto, client_config(), &bytes, None, NOW).unwrap();
    let server = PairingMachine::new_fixture_only(ServerFixtureCrypto, server_policy()).unwrap();
    server.register_invitation(invitation.clone(), NOW).unwrap();
    let transport = transport(&invitation);
    (client, server, invitation, transport)
}

fn drive_to_server_finish(
    client_crypto: ClientFixtureCrypto,
) -> (
    PairingClient<ClientFixtureCrypto>,
    ClientFixtureCrypto,
    Vec<u8>,
    Vec<u8>,
    TransportEvidence,
) {
    let handle = client_crypto.clone();
    let (mut client, server, _invitation, transport) = pair(client_crypto);
    let hello = client.create_client_hello(&transport).unwrap();
    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();
    let confirmation = client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
    assert_eq!(confirmation.verification_code, begin.verification_code);
    client
        .confirm_on_device(
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .unwrap();
    let bootstrap = server
        .confirm_user(
            &confirmation.receipt_id,
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let bootstrap_bytes = serde_json::to_vec(&bootstrap).unwrap();
    let finish = client
        .process_bootstrap(&bootstrap_bytes, None, &transport, NOW + 2_000)
        .unwrap();
    let server_finish = server
        .process_client_finish(&finish, None, &transport, NOW + 3_000)
        .unwrap();
    (client, handle, bootstrap_bytes, server_finish, transport)
}

#[test]
fn client_completes_fixture_pairing_with_byte_identical_retries() {
    let crypto = ClientFixtureCrypto::new(false, false);
    let handle = crypto.clone();
    let (mut client, server, _invitation, transport) = pair(crypto);

    let hello = client.create_client_hello(&transport).unwrap();
    assert_eq!(client.retry_client_hello().unwrap(), hello);
    let parsed_hello: tauri_app_lib::pairing_protocol::ClientHello =
        serde_json::from_slice(&hello).unwrap();
    assert_eq!(
        parsed_hello.proof_signature,
        signature(
            &client_signing_key(),
            &canonical_client_hello_unsigned(&parsed_hello)
        )
    );

    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();
    let confirmation = client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
    assert_eq!(confirmation.verification_code, begin.verification_code);
    assert_eq!(
        client
            .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
            .unwrap(),
        confirmation
    );

    client
        .confirm_on_device(
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .unwrap();
    let bootstrap = server
        .confirm_user(
            &confirmation.receipt_id,
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let bootstrap_bytes = serde_json::to_vec(&bootstrap).unwrap();
    let finish = client
        .process_bootstrap(&bootstrap_bytes, None, &transport, NOW + 2_000)
        .unwrap();
    assert_eq!(client.retry_client_finish().unwrap(), finish);
    assert_eq!(
        client
            .process_bootstrap(&bootstrap_bytes, None, &transport, NOW + 2_000)
            .unwrap(),
        finish
    );
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 1);

    let server_finish = server
        .process_client_finish(&finish, None, &transport, NOW + 3_000)
        .unwrap();
    let activation = client
        .process_server_finish(&server_finish, None, &transport, NOW + 3_000)
        .unwrap();
    assert_eq!(activation.receipt.device_id, DEVICE_ID);
    assert_eq!(client.state(), PairingClientState::Active);
    assert_eq!(
        client
            .process_server_finish(&server_finish, None, &transport, NOW + 3_000)
            .unwrap(),
        activation
    );
    assert_eq!(handle.activation_calls.load(Ordering::SeqCst), 1);
    assert!(handle.custody.lock().unwrap().active.contains(RECEIPT_ID));
}

#[test]
fn production_or_personal_pairing_is_rejected_before_identity_access() {
    let mut production = client_config();
    production.environment = Environment::Production;
    let bytes = serde_json::to_vec(&invitation()).unwrap();
    assert_eq!(
        PairingClient::new_fixture_only(
            ClientFixtureCrypto::new(false, false),
            production,
            &bytes,
            None,
            NOW
        )
        .err()
        .unwrap(),
        PairingClientError::Protocol(PairingError::FixtureOnly)
    );

    let mut personal = client_config();
    personal.library_data_class = LibraryDataClass::Personal;
    assert_eq!(
        PairingClient::new_fixture_only(
            ClientFixtureCrypto::new(false, false),
            personal,
            &bytes,
            None,
            NOW
        )
        .err()
        .unwrap(),
        PairingClientError::Protocol(PairingError::FixtureOnly)
    );
}

#[test]
fn invalid_invitation_and_server_signatures_fail_closed_and_allow_safe_retry() {
    let mut bad_invitation = invitation();
    bad_invitation.authority_signature[0] ^= 1;
    assert_eq!(
        PairingClient::new_fixture_only(
            ClientFixtureCrypto::new(false, false),
            client_config(),
            &serde_json::to_vec(&bad_invitation).unwrap(),
            None,
            NOW
        )
        .err()
        .unwrap(),
        PairingClientError::Protocol(PairingError::InvalidSignature)
    );

    let crypto = ClientFixtureCrypto::new(false, false);
    let handle = crypto.clone();
    let (mut client, server, _invitation, transport) = pair(crypto);
    let hello = client.create_client_hello(&transport).unwrap();
    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();
    let mut tampered: tauri_app_lib::pairing_protocol::ServerHello =
        serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    tampered.proof_signature[0] ^= 1;
    assert_eq!(
        client
            .process_server_hello(
                &serde_json::to_vec(&tampered).unwrap(),
                None,
                &transport,
                NOW + 1_000
            )
            .unwrap_err(),
        PairingClientError::Protocol(PairingError::InvalidSignature)
    );
    assert_eq!(handle.open_calls.load(Ordering::SeqCst), 0);
    client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
    assert_eq!(handle.open_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_bootstrap_is_rejected_before_native_staging_and_valid_retry_succeeds() {
    let crypto = ClientFixtureCrypto::new(false, false);
    let handle = crypto.clone();
    let (mut client, server, _invitation, transport) = pair(crypto);
    let hello = client.create_client_hello(&transport).unwrap();
    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();
    let confirmation = client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
    client
        .confirm_on_device(
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .unwrap();
    let bootstrap = server
        .confirm_user(
            &confirmation.receipt_id,
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let valid_bytes = serde_json::to_vec(&bootstrap).unwrap();
    let mut tampered = bootstrap;
    tampered.envelope_digest[0] ^= 1;

    assert_eq!(
        client
            .process_bootstrap(
                &serde_json::to_vec(&tampered).unwrap(),
                None,
                &transport,
                NOW + 2_000,
            )
            .unwrap_err(),
        PairingClientError::Protocol(PairingError::BindingMismatch("bootstrap envelope digest"))
    );
    assert_eq!(client.state(), PairingClientState::AwaitingBootstrap);
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 0);

    client
        .process_bootstrap(&valid_bytes, None, &transport, NOW + 2_000)
        .unwrap();
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_server_finish_never_activates_pending_keys_and_valid_retry_succeeds() {
    let (mut client, handle, _bootstrap, server_finish, transport) =
        drive_to_server_finish(ClientFixtureCrypto::new(false, false));
    let mut tampered: tauri_app_lib::pairing_protocol::ServerFinish =
        serde_json::from_slice(&server_finish).unwrap();
    tampered.signature[0] ^= 1;

    assert_eq!(
        client
            .process_server_finish(
                &serde_json::to_vec(&tampered).unwrap(),
                None,
                &transport,
                NOW + 3_000,
            )
            .unwrap_err(),
        PairingClientError::Protocol(PairingError::InvalidSignature)
    );
    assert_eq!(client.state(), PairingClientState::AwaitingServerFinish);
    assert_eq!(handle.activation_calls.load(Ordering::SeqCst), 0);

    client
        .process_server_finish(&server_finish, None, &transport, NOW + 3_000)
        .unwrap();
    assert_eq!(handle.activation_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn server_message_encoding_and_wire_limits_fail_before_state_changes() {
    let (mut client, server, _invitation, transport) = pair(ClientFixtureCrypto::new(false, false));
    let hello = client.create_client_hello(&transport).unwrap();
    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();

    assert_eq!(
        client
            .process_server_hello(
                &begin.server_hello_bytes,
                Some("gzip"),
                &transport,
                NOW + 1_000,
            )
            .unwrap_err(),
        PairingClientError::Protocol(PairingError::UnsupportedEncoding)
    );
    assert_eq!(
        client
            .process_server_hello(
                &vec![b' '; MAX_PAIRING_MESSAGE_BYTES + 1],
                None,
                &transport,
                NOW + 1_000,
            )
            .unwrap_err(),
        PairingClientError::Protocol(PairingError::PayloadTooLarge)
    );
    assert_eq!(client.state(), PairingClientState::AwaitingServerHello);

    client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
}

#[test]
fn verified_server_finish_keeps_pending_key_for_activation_retry() {
    let (mut client, handle, _bootstrap, server_finish, transport) =
        drive_to_server_finish(ClientFixtureCrypto::new(true, false));

    assert_eq!(
        client
            .process_server_finish(&server_finish, None, &transport, NOW + 3_000)
            .unwrap_err(),
        PairingClientError::ActivationUnavailable
    );
    assert_eq!(client.state(), PairingClientState::PendingActivation);
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 1);
    assert_eq!(handle.activation_calls.load(Ordering::SeqCst), 1);

    let PairingActivation { receipt, .. } = client.retry_activation().unwrap();
    assert_eq!(receipt.receipt_id, RECEIPT_ID);
    assert_eq!(client.state(), PairingClientState::Active);
    assert_eq!(handle.activation_calls.load(Ordering::SeqCst), 2);
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_retries_native_discard_without_restaging_or_exporting_key() {
    let crypto = ClientFixtureCrypto::new(false, true);
    let handle = crypto.clone();
    let (mut client, server, _invitation, transport) = pair(crypto);
    let hello = client.create_client_hello(&transport).unwrap();
    let begin = server
        .process_client_hello(&hello, None, &transport, NOW + 1_000)
        .unwrap();
    let confirmation = client
        .process_server_hello(&begin.server_hello_bytes, None, &transport, NOW + 1_000)
        .unwrap();
    client
        .confirm_on_device(
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .unwrap();
    let bootstrap: BootstrapEnvelope = server
        .confirm_user(
            &confirmation.receipt_id,
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    client
        .process_bootstrap(
            &serde_json::to_vec(&bootstrap).unwrap(),
            None,
            &transport,
            NOW + 2_000,
        )
        .unwrap();

    assert_eq!(
        client.cancel().unwrap_err(),
        PairingClientError::KeyCustodyUnavailable
    );
    assert_eq!(client.state(), PairingClientState::CancellationPending);
    client.retry_cancellation().unwrap();
    assert_eq!(client.state(), PairingClientState::Cancelled);
    assert_eq!(handle.stage_calls.load(Ordering::SeqCst), 1);
    assert_eq!(handle.discard_calls.load(Ordering::SeqCst), 2);
    let custody = handle.custody.lock().unwrap();
    assert!(custody.staged.is_empty());
    assert!(custody.discarded.contains(RECEIPT_ID));
}

#[test]
fn tls_pin_and_zero_rtt_are_checked_on_the_client_side() {
    let (mut client, _server, invitation, mut evidence) =
        pair(ClientFixtureCrypto::new(false, false));
    evidence.used_zero_rtt = true;
    assert_eq!(
        client.create_client_hello(&evidence).unwrap_err(),
        PairingClientError::Protocol(PairingError::InsecureTransport)
    );
    evidence.used_zero_rtt = false;
    evidence.peer_spki_sha256[0] ^= 1;
    assert_eq!(
        client.create_client_hello(&evidence).unwrap_err(),
        PairingClientError::Protocol(PairingError::PinMismatch)
    );
    assert_eq!(client.state(), PairingClientState::Ready);
    client.create_client_hello(&transport(&invitation)).unwrap();
}
