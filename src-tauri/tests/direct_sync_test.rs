use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use tauri_app_lib::direct_sync::*;
use tauri_app_lib::pairing_protocol::{
    canonical_client_finish_unsigned, canonical_client_hello_unsigned,
    canonical_invitation_unsigned, invitation_nonce_proof, ClientFinish, ClientHello, Environment,
    FreshValuePurpose, Invitation, KindCapability, LibraryDataClass, LocalHpkeKey, LocalSigningKey,
    PairingCrypto, PairingMachine, PairingPolicy, PairingRole, RecordKind as PairingRecordKind,
    ServerHello, TransportEvidence, PAIRING_PROTOCOL, PAIRING_SUITE,
};
use tauri_app_lib::sync_protocol::{
    AuthorityState, MutationDraft, ProtocolCapabilities, ReceiptDisposition, RecordKindCapability,
    SignedTransaction, TransactionHeader, SYNC_PROTOCOL_VERSION,
};

const NOW: u64 = 10_000;
const NOW_MS: i64 = 1_725_000_000_000;
const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000001";
const DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000002";
const OTHER_DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000003";
const INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000004";
const HELLO_ID: &str = "018f47a0-7b80-7000-8000-000000000005";
const FINISH_ID: &str = "018f47a0-7b80-7000-8000-000000000006";

fn id(number: u64) -> String {
    format!("018f47a0-7b80-7000-8000-{number:012x}")
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn fixture_signature(public_key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"noted.direct-sync.fixture/signature");
    hasher.update(public_key);
    hasher.update(message);
    let digest = hasher.finalize();
    [digest.as_slice(), digest.as_slice()].concat()
}

struct FixturePairingCrypto {
    counter: AtomicU64,
}

impl Default for FixturePairingCrypto {
    fn default() -> Self {
        Self {
            counter: AtomicU64::new(5_000),
        }
    }
}

impl FixturePairingCrypto {
    fn mac_pairing_key() -> Vec<u8> {
        let mut key = vec![0x21; 65];
        key[0] = 4;
        key
    }

    fn mac_authority_key() -> Vec<u8> {
        let mut key = vec![0x22; 65];
        key[0] = 4;
        key
    }

    fn client_key() -> Vec<u8> {
        let mut key = vec![0x23; 65];
        key[0] = 4;
        key
    }
}

impl PairingCrypto for FixturePairingCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        (signature == fixture_signature(public_key, message))
            .then_some(())
            .ok_or(())
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        let public_key = match key {
            LocalSigningKey::MacPairing => Self::mac_pairing_key(),
            LocalSigningKey::MacAuthority => Self::mac_authority_key(),
        };
        Ok(fixture_signature(&public_key, message))
    }

    fn seal_authenticated(
        &self,
        _sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        associated_data: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ()> {
        let mut bytes = b"fixture-seal".to_vec();
        bytes.extend_from_slice(recipient_public_key);
        bytes.extend_from_slice(associated_data);
        bytes.extend_from_slice(plaintext);
        Ok(sha256(&bytes))
    }

    fn exporter_secret(
        &self,
        _sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        transcript_digest: &[u8],
    ) -> Result<Vec<u8>, ()> {
        let mut bytes = b"fixture-exporter".to_vec();
        bytes.extend_from_slice(recipient_public_key);
        bytes.extend_from_slice(transcript_digest);
        Ok(sha256(&bytes))
    }

    fn fresh_bytes(&self, purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) as u8;
        let tag = match purpose {
            FreshValuePurpose::ReceiptId => 0x31,
            FreshValuePurpose::ServerNonce => 0x32,
        };
        Ok((0..length)
            .map(|offset| tag ^ counter ^ offset as u8)
            .collect())
    }

    fn fresh_uuid_v7(&self, _purpose: FreshValuePurpose) -> Result<String, ()> {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(id(counter))
    }
}

fn notes_pairing_scopes() -> BTreeSet<PairingRecordKind> {
    [
        PairingRecordKind::Note,
        PairingRecordKind::Category,
        PairingRecordKind::Folder,
    ]
    .into_iter()
    .collect()
}

fn notes_pairing_capabilities() -> BTreeMap<PairingRecordKind, KindCapability> {
    notes_pairing_scopes()
        .into_iter()
        .map(|kind| {
            (
                kind,
                KindCapability {
                    reader_version: 1,
                    writer_version: Some(1),
                },
            )
        })
        .collect()
}

fn active_pairing() -> PairingMachine<FixturePairingCrypto> {
    let policy = PairingPolicy {
        library_id: LIBRARY_ID.to_owned(),
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_generation: 7,
        grantable_scopes: notes_pairing_scopes(),
        capabilities: notes_pairing_capabilities(),
    };
    let machine =
        PairingMachine::new_fixture_only(FixturePairingCrypto::default(), policy).unwrap();
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: INVITATION_ID.to_owned(),
        invitation_nonce: vec![0x41; 32],
        authority_signing_public_key: FixturePairingCrypto::mac_authority_key(),
        mac_pairing_signing_public_key: FixturePairingCrypto::mac_pairing_key(),
        mac_pairing_hpke_public_key: vec![0x42; 32],
        tls_spki_sha256: vec![0x43; 32],
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: 7,
        scope_ceiling: notes_pairing_scopes(),
        created_at_ms: NOW_MS,
        expires_at_ms: NOW_MS + 300_000,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = fixture_signature(
        &invitation.authority_signing_public_key,
        &canonical_invitation_unsigned(&invitation),
    );
    machine
        .register_invitation(invitation.clone(), NOW_MS)
        .unwrap();

    let mut hello = ClientHello {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id: HELLO_ID.to_owned(),
        invitation_id: INVITATION_ID.to_owned(),
        nonce_proof: invitation_nonce_proof(&invitation.invitation_nonce),
        client_nonce: vec![0x44; 32],
        proposed_device_id: DEVICE_ID.to_owned(),
        display_name: "Fixture iPhone".to_owned(),
        client_signing_public_key: FixturePairingCrypto::client_key(),
        client_hpke_public_key: vec![0x45; 32],
        requested_scopes: notes_pairing_scopes(),
        capabilities: notes_pairing_capabilities(),
        app_version: "fixture".to_owned(),
        build_version: "1".to_owned(),
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: 7,
        environment: Environment::Development,
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        observed_tls_spki_sha256: invitation.tls_spki_sha256.clone(),
        proof_signature: Vec::new(),
    };
    hello.proof_signature = fixture_signature(
        &hello.client_signing_public_key,
        &canonical_client_hello_unsigned(&hello),
    );
    let begin = machine
        .process_client_hello(
            &serde_json::to_vec(&hello).unwrap(),
            None,
            &tauri_app_lib::pairing_protocol::TransportEvidence {
                tls_version: "1.3".to_owned(),
                used_zero_rtt: false,
                peer_spki_sha256: invitation.tls_spki_sha256,
            },
            NOW_MS + 1_000,
        )
        .unwrap();
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    let bootstrap = machine
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW_MS + 2_000,
        )
        .unwrap();
    let receipt = server_hello.receipt;
    let mut finish = ClientFinish {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id: FINISH_ID.to_owned(),
        receipt_id: receipt.receipt_id,
        invitation_id: receipt.invitation_id,
        library_id: receipt.library_id,
        device_id: receipt.device_id,
        authority_generation: receipt.authority_generation,
        environment: receipt.environment,
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        transcript_digest: receipt.transcript_digest,
        ciphertext_digest: bootstrap.ciphertext_digest,
        proof_signature: Vec::new(),
    };
    finish.proof_signature = fixture_signature(
        &FixturePairingCrypto::client_key(),
        &canonical_client_finish_unsigned(&finish),
    );
    machine
        .process_client_finish(
            &serde_json::to_vec(&finish).unwrap(),
            None,
            &TransportEvidence {
                tls_version: "1.3".to_owned(),
                used_zero_rtt: false,
                peer_spki_sha256: vec![0x43; 32],
            },
            NOW_MS + 3_000,
        )
        .unwrap();
    machine
}

fn notes_protocol_capabilities() -> ProtocolCapabilities {
    let mut capabilities = ProtocolCapabilities::new(
        1,
        1,
        BTreeMap::from([
            ("note".to_owned(), RecordKindCapability::new(1, 1)),
            ("category".to_owned(), RecordKindCapability::new(1, 1)),
            ("folder".to_owned(), RecordKindCapability::new(1, 1)),
        ]),
    );
    capabilities.max_transaction_members = MAX_DIRECT_TRANSACTION_MEMBERS;
    capabilities.max_transaction_bytes = MAX_DIRECT_TRANSACTION_BYTES;
    capabilities
}

#[derive(Clone, Default)]
struct FixtureDirectCrypto {
    verified_sources: Arc<Mutex<Vec<String>>>,
}

impl FixtureDirectCrypto {
    fn request_signature(
        endpoint: DirectEndpoint,
        device_id: &str,
        signing_digest: &str,
    ) -> Vec<u8> {
        let mut bytes = b"noted.direct-sync.fixture/request".to_vec();
        bytes.extend_from_slice(endpoint.path().as_bytes());
        bytes.extend_from_slice(device_id.as_bytes());
        bytes.extend_from_slice(signing_digest.as_bytes());
        let digest = sha256(&bytes);
        [digest.as_slice(), digest.as_slice()].concat()
    }

    fn response_signature(endpoint: DirectEndpoint, signing_digest: &str) -> Vec<u8> {
        let mut bytes = b"noted.direct-sync.fixture/response".to_vec();
        bytes.extend_from_slice(endpoint.path().as_bytes());
        bytes.extend_from_slice(signing_digest.as_bytes());
        let digest = sha256(&bytes);
        [digest.as_slice(), digest.as_slice()].concat()
    }
}

impl DirectSyncCrypto for FixtureDirectCrypto {
    fn verify_request_signature(
        &self,
        endpoint: DirectEndpoint,
        device_id: &str,
        signing_digest: &str,
        signature: &[u8],
    ) -> Result<(), ()> {
        (signature == Self::request_signature(endpoint, device_id, signing_digest))
            .then_some(())
            .ok_or(())
    }

    fn verify_mutation_ciphertext(
        &self,
        device_id: &str,
        mutation: &tauri_app_lib::sync_protocol::MutationEnvelope,
    ) -> Result<(), ()> {
        if device_id != mutation.device_id
            || !mutation.ciphertext.starts_with(b"fixture:")
            || mutation.signature != [0xaa]
        {
            return Err(());
        }
        self.verified_sources
            .lock()
            .map_err(|_| ())?
            .push(device_id.to_owned());
        Ok(())
    }

    fn authenticate_response(
        &self,
        endpoint: DirectEndpoint,
        signing_digest: &str,
    ) -> Result<Vec<u8>, ()> {
        Ok(Self::response_signature(endpoint, signing_digest))
    }
}

type TestService =
    DirectSyncService<FixturePairingCrypto, AuthorityStateStore, FixtureDirectCrypto>;

fn authority_with_devices(devices: &[&str]) -> AuthorityState {
    let capabilities = notes_protocol_capabilities();
    let mut authority =
        AuthorityState::new(LIBRARY_ID.to_owned(), 7, 0, 1, capabilities.clone()).unwrap();
    for device in devices {
        authority
            .register_device((*device).to_owned(), capabilities.clone())
            .unwrap();
    }
    authority
}

struct CoordinatedAuthority {
    state: AuthorityState,
    push_entered: Option<Arc<Barrier>>,
    release_push: Option<Arc<Barrier>>,
    fail_revoke: bool,
}

impl CoordinatedAuthority {
    fn new(state: AuthorityState) -> Self {
        Self {
            state,
            push_entered: None,
            release_push: None,
            fail_revoke: false,
        }
    }
}

impl DirectSyncAuthority for CoordinatedAuthority {
    fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError> {
        Ok(AuthorityIdentity {
            library_id: self.state.library_id().to_owned(),
            authority_generation: self.state.authority_generation(),
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
        })
    }

    fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError> {
        Ok(self.state.capabilities().clone())
    }

    fn bootstrap(
        &self,
    ) -> Result<tauri_app_lib::sync_protocol::BootstrapSnapshot, AuthorityStoreError> {
        Ok(self.state.bootstrap_snapshot()?)
    }

    fn pull(
        &self,
        cursor: u64,
        limit: u32,
    ) -> Result<tauri_app_lib::sync_protocol::ChangePage, AuthorityStoreError> {
        Ok(self.state.changes_after(cursor, limit)?)
    }

    fn push(
        &mut self,
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<tauri_app_lib::sync_protocol::SubmitOutcome, AuthorityStoreError> {
        if let Some(entered) = &self.push_entered {
            entered.wait();
        }
        if let Some(release) = &self.release_push {
            release.wait();
        }
        Ok(self.state.submit_transaction(transaction, now)?)
    }

    fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError> {
        let snapshot = self.state.bootstrap_snapshot()?;
        Ok(SyncCheckpoint {
            contract_version: snapshot.contract_version,
            library_id: snapshot.library_id,
            authority_generation: snapshot.authority_generation,
            purge_generation: snapshot.purge_generation,
            key_epoch: snapshot.key_epoch,
            high_water_cursor: snapshot.high_water_cursor,
            checkpoint_digest: snapshot.checkpoint_digest,
        })
    }

    fn acknowledge(
        &mut self,
        _device_id: &str,
        _cursor: u64,
        _checkpoint_digest: &str,
    ) -> Result<AckReceipt, AuthorityStoreError> {
        Err(AuthorityStoreError::StateUnavailable)
    }

    fn revoke_device(&mut self, device_id: &str) -> Result<(), AuthorityStoreError> {
        if self.fail_revoke {
            return Err(AuthorityStoreError::StateUnavailable);
        }
        self.state.revoke_device(device_id)?;
        Ok(())
    }
}

fn service_with_authority(
    authority: AuthorityState,
    limits: DirectSyncLimits,
) -> (TestService, (), Arc<Mutex<Vec<String>>>) {
    let pairing = active_pairing();
    let crypto = FixtureDirectCrypto::default();
    let verified_sources = Arc::clone(&crypto.verified_sources);
    let store = AuthorityStateStore::new_fixture_only(authority);
    let service = DirectSyncService::new(
        pairing,
        store,
        crypto,
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits,
        },
    )
    .unwrap();
    (service, (), verified_sources)
}

fn service() -> (TestService, (), Arc<Mutex<Vec<String>>>) {
    service_with_authority(
        authority_with_devices(&[DEVICE_ID]),
        DirectSyncLimits::default(),
    )
}

fn sign_request<T: Serialize>(
    endpoint: DirectEndpoint,
    mut request: SignedSyncRequest<T>,
) -> SignedSyncRequest<T> {
    let digest = request_signing_digest(endpoint, &request).unwrap();
    request.signature =
        FixtureDirectCrypto::request_signature(endpoint, &request.device_id, &digest);
    request
}

fn signed_request<T: Serialize>(
    endpoint: DirectEndpoint,
    request_number: u64,
    payload: T,
) -> SignedSyncRequest<T> {
    sign_request(
        endpoint,
        SignedSyncRequest {
            protocol_version: SYNC_PROTOCOL_VERSION,
            request_id: id(10_000 + request_number),
            library_id: LIBRARY_ID.to_owned(),
            device_id: DEVICE_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            payload,
            signature: Vec::new(),
        },
    )
}

fn wire_request<T: Serialize>(
    endpoint: DirectEndpoint,
    signed: &SignedSyncRequest<T>,
) -> DirectRequest {
    DirectRequest {
        method: "POST".to_owned(),
        target: endpoint.path().to_owned(),
        content_type: Some(DIRECT_SYNC_CONTENT_TYPE.to_owned()),
        content_encoding: None,
        body: serde_json::to_vec(signed).unwrap(),
        authority_now: NOW,
        transport: SecureTransportEvidence {
            tls_version: "1.3".to_owned(),
            used_zero_rtt: false,
            server_spki_sha256: vec![0x43; 32],
        },
    }
}

fn response_payload<T: DeserializeOwned + Serialize>(
    endpoint: DirectEndpoint,
    response: &DirectResponse,
) -> T {
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let signed: SignedSyncResponse<T> = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(signed.protocol_version, SYNC_PROTOCOL_VERSION);
    assert!(tauri_app_lib::portable::is_uuid_v7(&signed.request_id));
    assert_eq!(signed.library_id, LIBRARY_ID);
    assert_eq!(signed.device_id, DEVICE_ID);
    assert_eq!(signed.authority_generation, 7);
    let digest = response_signing_digest(endpoint, &signed).unwrap();
    assert_eq!(
        signed.signature,
        FixtureDirectCrypto::response_signature(endpoint, &digest)
    );
    signed.payload
}

fn notes_slice() -> BTreeSet<String> {
    ["note", "category", "folder"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn transaction(
    device_id: &str,
    counter: u64,
    transaction_number: u64,
    drafts: Vec<(&str, u64, Option<String>, u64, Vec<u8>)>,
) -> SignedTransaction {
    SignedTransaction::assemble(
        TransactionHeader {
            protocol_version: 1,
            library_id: LIBRARY_ID.to_owned(),
            transaction_id: id(20_000 + transaction_number),
            device_id: device_id.to_owned(),
            device_transaction_counter: counter,
            authority_generation: 7,
            purge_generation: 0,
            key_epoch: 1,
        },
        drafts
            .into_iter()
            .enumerate()
            .map(
                |(index, (kind, base, base_version, proposed, ciphertext))| MutationDraft {
                    mutation_id: id(30_000 + transaction_number * 10 + index as u64),
                    record_id: id(40_000 + transaction_number * 10 + index as u64),
                    record_kind: kind.to_owned(),
                    record_schema_version: 1,
                    base_head_revision: base,
                    base_head_version_id: base_version,
                    proposed_revision: proposed,
                    version_id: id(50_000 + transaction_number * 10 + index as u64),
                    ciphertext,
                    signature: vec![0xaa],
                },
            )
            .collect(),
        NOW + 1_000,
    )
    .unwrap()
}

fn single_record_transaction(
    counter: u64,
    transaction_number: u64,
    kind: &str,
    ciphertext: Vec<u8>,
) -> SignedTransaction {
    transaction(
        DEVICE_ID,
        counter,
        transaction_number,
        vec![(kind, 0, None, 1, ciphertext)],
    )
}

#[test]
fn negotiate_succeeds_and_authority_identity_is_bound_at_construction() {
    let (service, _, _) = service();
    let negotiate = signed_request(
        DirectEndpoint::Negotiate,
        0,
        NegotiateRequest {
            capabilities: notes_protocol_capabilities(),
        },
    );
    let negotiate_response = service.handle(wire_request(DirectEndpoint::Negotiate, &negotiate));
    let negotiate_envelope: SignedSyncResponse<NegotiateResponse> =
        serde_json::from_slice(&negotiate_response.body).unwrap();
    assert_eq!(negotiate_envelope.request_id, negotiate.request_id);
    let response: NegotiateResponse =
        response_payload(DirectEndpoint::Negotiate, &negotiate_response);
    assert_eq!(response.negotiated.protocol_version, SYNC_PROTOCOL_VERSION);
    assert_eq!(
        response.negotiated.max_transaction_members,
        MAX_DIRECT_TRANSACTION_MEMBERS
    );
    assert_eq!(
        response.negotiated.max_transaction_bytes,
        MAX_DIRECT_TRANSACTION_BYTES
    );
    assert_eq!(
        response
            .negotiated
            .record_kinds
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        notes_slice()
    );

    let other_library = id(999);
    let mut wrong_authority =
        AuthorityState::new(other_library, 7, 0, 1, notes_protocol_capabilities()).unwrap();
    wrong_authority
        .register_device(DEVICE_ID.to_owned(), notes_protocol_capabilities())
        .unwrap();
    let result = DirectSyncService::new(
        active_pairing(),
        AuthorityStateStore::new_fixture_only(wrong_authority),
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    );
    assert!(matches!(result, Err(DirectSyncError::InvalidConfiguration)));
}

#[test]
fn service_construction_rejects_production_and_personal_data_even_with_valid_ids() {
    let production = DirectSyncService::new(
        active_pairing(),
        AuthorityStateStore::new_fixture_only(authority_with_devices(&[DEVICE_ID])),
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Production,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    );
    assert!(matches!(
        production,
        Err(DirectSyncError::InvalidConfiguration)
    ));

    let personal = DirectSyncService::new(
        active_pairing(),
        AuthorityStateStore::new_fixture_only(authority_with_devices(&[DEVICE_ID])),
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::Personal,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    );
    assert!(matches!(
        personal,
        Err(DirectSyncError::InvalidConfiguration)
    ));

    let mut too_small_for_advertised_transactions = DirectSyncLimits::default();
    too_small_for_advertised_transactions.pull.response_bytes -= 1;
    let undersized_wire = DirectSyncService::new(
        active_pairing(),
        AuthorityStateStore::new_fixture_only(authority_with_devices(&[DEVICE_ID])),
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: too_small_for_advertised_transactions,
        },
    );
    assert!(matches!(
        undersized_wire,
        Err(DirectSyncError::InvalidConfiguration)
    ));
}

#[test]
fn router_exposes_only_six_post_json_routes_without_query_or_fragment() {
    let (service, _, _) = service();
    let signed = signed_request(
        DirectEndpoint::Checkpoint,
        1,
        CheckpointRequest { known_cursor: None },
    );
    let valid = wire_request(DirectEndpoint::Checkpoint, &signed);

    let mut wrong_method = valid.clone();
    wrong_method.method = "GET".to_owned();
    assert_eq!(service.handle(wrong_method).status, 405);

    let mut wrong_content_type = valid.clone();
    wrong_content_type.content_type = Some("text/plain".to_owned());
    assert_eq!(service.handle(wrong_content_type).status, 415);

    let mut compressed = valid.clone();
    compressed.content_encoding = Some("gzip".to_owned());
    assert_eq!(service.handle(compressed).status, 415);

    for target in [
        "/sync/v1/checkpoint?token=secret",
        "/sync/v1/checkpoint#secret",
    ] {
        let mut request = valid.clone();
        request.target = target.to_owned();
        assert_eq!(service.handle(request).status, 400);
    }

    for legacy in [
        "/api/export_db",
        "/api/read_inbox_image",
        "/api/note_delete_forever",
        "/api/set_provider_settings",
        "/api/gcal_set_client",
        "/api/brain_add_vault",
        "/sync/v1/ask",
        "/sync/v1/provider",
        "/sync/v1/filesystem",
    ] {
        let mut request = valid.clone();
        request.target = legacy.to_owned();
        assert_eq!(service.handle(request).status, 404, "{legacy}");
    }
    assert_eq!(DirectEndpoint::ALL.len(), 6);
}

#[test]
fn tls13_no_zero_rtt_and_the_exact_server_pin_are_mandatory() {
    let (service, _, _) = service();
    let signed = signed_request(
        DirectEndpoint::Checkpoint,
        2,
        CheckpointRequest { known_cursor: None },
    );
    let base = wire_request(DirectEndpoint::Checkpoint, &signed);

    let mut wrong_pin = base.clone();
    wrong_pin.transport.server_spki_sha256[0] ^= 1;
    assert_eq!(service.handle(wrong_pin).status, 401);

    let mut tls12 = base.clone();
    tls12.transport.tls_version = "1.2".to_owned();
    assert_eq!(service.handle(tls12).status, 401);

    let mut early_data = base;
    early_data.transport.used_zero_rtt = true;
    assert_eq!(service.handle(early_data).status, 401);
}

#[test]
fn malformed_unknown_deep_and_oversized_requests_fail_before_dispatch() {
    let (service, _, _) = service();
    let signed = signed_request(
        DirectEndpoint::Checkpoint,
        3,
        CheckpointRequest { known_cursor: None },
    );
    let mut malformed = wire_request(DirectEndpoint::Checkpoint, &signed);
    malformed.body = b"not-json".to_vec();
    assert_eq!(service.handle(malformed).status, 400);

    let mut value = serde_json::to_value(&signed).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), serde_json::Value::Bool(true));
    let mut unknown = wire_request(DirectEndpoint::Checkpoint, &signed);
    unknown.body = serde_json::to_vec(&value).unwrap();
    assert_eq!(service.handle(unknown).status, 400);

    let mut duplicate = wire_request(DirectEndpoint::Checkpoint, &signed);
    let duplicate_json = String::from_utf8(duplicate.body).unwrap().replacen(
        "\"protocol_version\":1",
        "\"protocol_version\":1,\"protocol_version\":1",
        1,
    );
    duplicate.body = duplicate_json.into_bytes();
    assert_eq!(service.handle(duplicate).status, 400);

    let mut deep = wire_request(DirectEndpoint::Checkpoint, &signed);
    deep.body = format!(
        "{}0{}",
        "[".repeat(MAX_DIRECT_JSON_DEPTH + 2),
        "]".repeat(MAX_DIRECT_JSON_DEPTH + 2)
    )
    .into_bytes();
    assert_eq!(service.handle(deep).status, 400);

    let push = signed_request(
        DirectEndpoint::Push,
        4,
        PushRequest {
            transaction: single_record_transaction(1, 1, "note", b"fixture:small".to_vec()),
        },
    );
    let mut oversized = wire_request(DirectEndpoint::Push, &push);
    oversized.body = vec![b' '; DirectSyncLimits::default().push.request_bytes + 1];
    assert_eq!(service.handle(oversized).status, 413);
}

#[test]
fn request_signature_fixture_class_enrollment_and_revocation_are_enforced() {
    let (service, _, _) = service();
    let mut bad_signature = signed_request(
        DirectEndpoint::Checkpoint,
        5,
        CheckpointRequest { known_cursor: None },
    );
    bad_signature.signature[0] ^= 1;
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &bad_signature))
            .status,
        401
    );

    let mut personal = signed_request(
        DirectEndpoint::Checkpoint,
        6,
        CheckpointRequest { known_cursor: None },
    );
    personal.library_data_class = LibraryDataClass::Personal;
    personal = sign_request(DirectEndpoint::Checkpoint, personal);
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &personal))
            .status,
        403
    );

    let mut unenrolled = signed_request(
        DirectEndpoint::Checkpoint,
        7,
        CheckpointRequest { known_cursor: None },
    );
    unenrolled.device_id = OTHER_DEVICE_ID.to_owned();
    unenrolled = sign_request(DirectEndpoint::Checkpoint, unenrolled);
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &unenrolled))
            .status,
        401
    );

    service.revoke_device(DEVICE_ID, NOW_MS + 5_000).unwrap();
    let revoked = signed_request(
        DirectEndpoint::Checkpoint,
        8,
        CheckpointRequest { known_cursor: None },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &revoked))
            .status,
        403
    );
}

#[test]
fn revocation_linearizes_after_an_inflight_commit_and_blocks_every_later_request() {
    let push_entered = Arc::new(Barrier::new(2));
    let release_push = Arc::new(Barrier::new(2));
    let mut authority = CoordinatedAuthority::new(authority_with_devices(&[DEVICE_ID]));
    authority.push_entered = Some(Arc::clone(&push_entered));
    authority.release_push = Some(Arc::clone(&release_push));
    let service = Arc::new(
        DirectSyncService::new(
            active_pairing(),
            authority,
            FixtureDirectCrypto::default(),
            DirectSyncConfig {
                library_id: LIBRARY_ID.to_owned(),
                authority_generation: 7,
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
                server_spki_sha256: vec![0x43; 32],
                limits: DirectSyncLimits::default(),
            },
        )
        .unwrap(),
    );
    let push = signed_request(
        DirectEndpoint::Push,
        200,
        PushRequest {
            transaction: single_record_transaction(1, 200, "note", b"fixture:race".to_vec()),
        },
    );
    let push_service = Arc::clone(&service);
    let push_thread =
        std::thread::spawn(move || push_service.handle(wire_request(DirectEndpoint::Push, &push)));
    push_entered.wait();

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let revoke_service = Arc::clone(&service);
    let revoke_thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = revoke_service.revoke_device(DEVICE_ID, NOW_MS + 6_000);
        result_tx.send(result).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        result_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    release_push.wait();
    assert_eq!(push_thread.join().unwrap().status, 200);
    assert!(result_rx.recv().unwrap().is_ok());
    revoke_thread.join().unwrap();

    let after_revoke = signed_request(
        DirectEndpoint::Checkpoint,
        201,
        CheckpointRequest { known_cursor: None },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &after_revoke))
            .status,
        403
    );
}

#[test]
fn authority_revoke_failure_still_leaves_pairing_fail_closed() {
    let mut authority = CoordinatedAuthority::new(authority_with_devices(&[DEVICE_ID]));
    authority.fail_revoke = true;
    let service = DirectSyncService::new(
        active_pairing(),
        authority,
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    )
    .unwrap();
    assert!(matches!(
        service.revoke_device(DEVICE_ID, NOW_MS + 7_000),
        Err(DirectSyncError::StateUnavailable)
    ));

    let request = signed_request(
        DirectEndpoint::Checkpoint,
        202,
        CheckpointRequest { known_cursor: None },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Checkpoint, &request))
            .status,
        403
    );
}

#[test]
fn exact_notes_slice_and_each_write_scope_are_required() {
    let (service, _, _) = service();
    let wrong_slice = signed_request(
        DirectEndpoint::Bootstrap,
        9,
        BootstrapRequest {
            requested_record_kinds: BTreeSet::from(["note".to_owned()]),
            checkpoint_digest: None,
            after_record_id: None,
            limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Bootstrap, &wrong_slice))
            .status,
        403
    );

    let media_push = signed_request(
        DirectEndpoint::Push,
        10,
        PushRequest {
            transaction: single_record_transaction(1, 10, "media", b"fixture:media".to_vec()),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &media_push))
            .status,
        403
    );

    let mut future_schema =
        single_record_transaction(1, 11, "note", b"fixture:future-schema".to_vec());
    future_schema.members[0].record_schema_version = 2;
    future_schema.manifest.ordered_member_digests = future_schema
        .members
        .iter()
        .map(|member| member.member_digest())
        .collect();
    let manifest_digest = future_schema.manifest.digest();
    for member in &mut future_schema.members {
        member.transaction_manifest_digest = manifest_digest.clone();
    }
    let future_schema = signed_request(
        DirectEndpoint::Push,
        11,
        PushRequest {
            transaction: future_schema,
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &future_schema))
            .status,
        403
    );
}

#[test]
fn duplicate_push_is_byte_identical_reordered_members_accept_and_stale_head_conflicts() {
    let (service, _, _) = service();
    let first_transaction = single_record_transaction(1, 20, "note", b"fixture:first".to_vec());
    let first_request = signed_request(
        DirectEndpoint::Push,
        11,
        PushRequest {
            transaction: first_transaction.clone(),
        },
    );
    let first_wire = wire_request(DirectEndpoint::Push, &first_request);
    let first_response = service.handle(first_wire.clone());
    let replay_response = service.handle(first_wire);
    assert_eq!(first_response.status, 200);
    assert_eq!(first_response.body, replay_response.body);
    let first: PushResponse = response_payload(DirectEndpoint::Push, &first_response);
    assert!(matches!(
        first.receipt.disposition,
        ReceiptDisposition::Accepted { .. }
    ));

    let first_member = &first_transaction.members[0];
    let stale = transaction(
        DEVICE_ID,
        2,
        21,
        vec![("note", 0, None, 1, b"fixture:stale".to_vec())],
    );
    // Target the already-accepted record while retaining a stale zero base.
    let mut stale = stale;
    stale.members[0].record_id = first_member.record_id.clone();
    stale.manifest.ordered_member_digests = stale
        .members
        .iter()
        .map(|member| member.member_digest())
        .collect();
    let manifest_digest = stale.manifest.digest();
    for member in &mut stale.members {
        member.transaction_manifest_digest = manifest_digest.clone();
    }
    let stale_request =
        signed_request(DirectEndpoint::Push, 12, PushRequest { transaction: stale });
    let stale_response = service.handle(wire_request(DirectEndpoint::Push, &stale_request));
    let stale: PushResponse = response_payload(DirectEndpoint::Push, &stale_response);
    assert!(matches!(
        stale.receipt.disposition,
        ReceiptDisposition::Conflict { .. }
    ));

    let mut reordered = transaction(
        DEVICE_ID,
        3,
        22,
        vec![
            ("category", 0, None, 1, b"fixture:category".to_vec()),
            ("folder", 0, None, 1, b"fixture:folder".to_vec()),
        ],
    );
    reordered.members.reverse();
    let reordered_request = signed_request(
        DirectEndpoint::Push,
        13,
        PushRequest {
            transaction: reordered,
        },
    );
    let reordered: PushResponse = response_payload(
        DirectEndpoint::Push,
        &service.handle(wire_request(DirectEndpoint::Push, &reordered_request)),
    );
    assert!(matches!(
        reordered.receipt.disposition,
        ReceiptDisposition::Accepted { .. }
    ));
}

#[test]
fn the_largest_advertised_transaction_can_be_pushed_and_pulled() {
    let (service, _, _) = service();
    let mut ciphertext = b"fixture:".to_vec();
    ciphertext.extend(std::iter::repeat_n(
        u8::MAX,
        MAX_DIRECT_TRANSACTION_BYTES as usize - ciphertext.len(),
    ));
    let request = signed_request(
        DirectEndpoint::Push,
        14,
        PushRequest {
            transaction: single_record_transaction(1, 30, "note", ciphertext),
        },
    );
    let wire = wire_request(DirectEndpoint::Push, &request);
    assert!(wire.body.len() > 16 * 1024);
    assert!(
        wire.body.len() < DirectSyncLimits::default().push.request_bytes,
        "largest advertised transaction serialized to {} bytes",
        wire.body.len()
    );
    let response = service.handle(wire);
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );

    let pull = signed_request(
        DirectEndpoint::Pull,
        26,
        PullRequest {
            cursor: 0,
            limit: 1,
            requested_record_kinds: notes_slice(),
        },
    );
    let pulled = service.handle(wire_request(DirectEndpoint::Pull, &pull));
    assert!(pulled.body.len() <= DirectSyncLimits::default().pull.response_bytes);
    let page: PullResponse = response_payload(DirectEndpoint::Pull, &pulled);
    assert_eq!(page.page.next_cursor, 1);
    assert_eq!(page.page.changes.len(), 1);
}

#[test]
fn transactions_that_exceed_the_advertised_limit_never_commit() {
    let (service, _, _) = service();
    let mut oversized_ciphertext = b"fixture:".to_vec();
    oversized_ciphertext.extend(std::iter::repeat_n(
        0x61,
        MAX_DIRECT_TRANSACTION_BYTES as usize + 1 - oversized_ciphertext.len(),
    ));
    let oversized = signed_request(
        DirectEndpoint::Push,
        27,
        PushRequest {
            transaction: single_record_transaction(1, 32, "note", oversized_ciphertext),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &oversized))
            .status,
        413
    );

    let checkpoint = signed_request(
        DirectEndpoint::Checkpoint,
        29,
        CheckpointRequest { known_cursor: None },
    );
    let response: CheckpointResponse = response_payload(
        DirectEndpoint::Checkpoint,
        &service.handle(wire_request(DirectEndpoint::Checkpoint, &checkpoint)),
    );
    assert_eq!(response.checkpoint.high_water_cursor, 0);
}

#[test]
fn oversized_aggregate_bootstrap_is_paged_and_checkpoint_bound() {
    let (service, _, _) = service();
    for counter in 1..=3 {
        let mut ciphertext = b"fixture:".to_vec();
        ciphertext.extend(std::iter::repeat_n(
            u8::MAX,
            MAX_DIRECT_TRANSACTION_BYTES as usize - ciphertext.len(),
        ));
        let push = signed_request(
            DirectEndpoint::Push,
            100 + counter,
            PushRequest {
                transaction: single_record_transaction(counter, 100 + counter, "note", ciphertext),
            },
        );
        assert_eq!(
            service
                .handle(wire_request(DirectEndpoint::Push, &push))
                .status,
            200
        );
    }

    let first_request = signed_request(
        DirectEndpoint::Bootstrap,
        110,
        BootstrapRequest {
            requested_record_kinds: notes_slice(),
            checkpoint_digest: None,
            after_record_id: None,
            limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
        },
    );
    let first_response = service.handle(wire_request(DirectEndpoint::Bootstrap, &first_request));
    assert!(first_response.body.len() <= DirectSyncLimits::default().bootstrap.response_bytes);
    let first: BootstrapResponse = response_payload(DirectEndpoint::Bootstrap, &first_response);
    assert_eq!(first.page.records.len(), 1);
    assert!(first.page.has_more);
    let digest = first.page.checkpoint_digest.clone();
    let mut after = first.page.next_after_record_id.clone();
    let mut record_ids = first
        .page
        .records
        .iter()
        .map(|record| record.record_id.clone())
        .collect::<Vec<_>>();

    while after.is_some() && record_ids.len() < 3 {
        let request = signed_request(
            DirectEndpoint::Bootstrap,
            111 + record_ids.len() as u64,
            BootstrapRequest {
                requested_record_kinds: notes_slice(),
                checkpoint_digest: Some(digest.clone()),
                after_record_id: after.clone(),
                limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
            },
        );
        let page: BootstrapResponse = response_payload(
            DirectEndpoint::Bootstrap,
            &service.handle(wire_request(DirectEndpoint::Bootstrap, &request)),
        );
        assert_eq!(page.page.checkpoint_digest, digest);
        record_ids.extend(
            page.page
                .records
                .iter()
                .map(|record| record.record_id.clone()),
        );
        after = if page.page.has_more {
            page.page.next_after_record_id
        } else {
            None
        };
    }
    record_ids.sort();
    record_ids.dedup();
    assert_eq!(record_ids.len(), 3);

    let changed = signed_request(
        DirectEndpoint::Push,
        120,
        PushRequest {
            transaction: single_record_transaction(4, 120, "note", b"fixture:new-head".to_vec()),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &changed))
            .status,
        200
    );
    let stale_continuation = signed_request(
        DirectEndpoint::Bootstrap,
        121,
        BootstrapRequest {
            requested_record_kinds: notes_slice(),
            checkpoint_digest: Some(digest),
            after_record_id: first.page.next_after_record_id,
            limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
        },
    );
    let response = service.handle(wire_request(DirectEndpoint::Bootstrap, &stale_continuation));
    assert_eq!(response.status, 409);
    let error: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(error["error"]["code"], "bootstrap_changed_restart_required");
}

#[test]
fn bad_authenticated_ciphertext_is_rejected_before_authority_state_changes() {
    let (service, _, _) = service();
    let request = signed_request(
        DirectEndpoint::Push,
        15,
        PushRequest {
            transaction: single_record_transaction(1, 31, "note", b"not-a-fixture".to_vec()),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &request))
            .status,
        422
    );

    let checkpoint = signed_request(
        DirectEndpoint::Checkpoint,
        16,
        CheckpointRequest { known_cursor: None },
    );
    let response: CheckpointResponse = response_payload(
        DirectEndpoint::Checkpoint,
        &service.handle(wire_request(DirectEndpoint::Checkpoint, &checkpoint)),
    );
    assert_eq!(response.checkpoint.high_water_cursor, 0);
}

#[test]
fn pull_pages_are_bounded_and_verify_the_original_author_device() {
    let mut authority = authority_with_devices(&[DEVICE_ID, OTHER_DEVICE_ID]);
    for counter in 1..=3 {
        authority
            .submit_transaction(
                transaction(
                    OTHER_DEVICE_ID,
                    counter,
                    40 + counter,
                    vec![(
                        "note",
                        0,
                        None,
                        1,
                        format!("fixture:other-{counter}").into_bytes(),
                    )],
                ),
                NOW,
            )
            .unwrap();
    }
    let (service, _, verified_sources) =
        service_with_authority(authority, DirectSyncLimits::default());
    let pull = signed_request(
        DirectEndpoint::Pull,
        17,
        PullRequest {
            cursor: 0,
            limit: 2,
            requested_record_kinds: notes_slice(),
        },
    );
    let response = service.handle(wire_request(DirectEndpoint::Pull, &pull));
    assert!(response.body.len() <= DirectSyncLimits::default().pull.response_bytes);
    let page: PullResponse = response_payload(DirectEndpoint::Pull, &response);
    assert_eq!(page.page.changes.len(), 2);
    assert!(page.page.has_more);
    assert!(verified_sources
        .lock()
        .unwrap()
        .iter()
        .all(|source| source == OTHER_DEVICE_ID));
}

#[test]
fn a_nonprogressing_authority_page_fails_closed_instead_of_stalling_the_cursor() {
    struct NonprogressingStore;

    impl DirectSyncAuthority for NonprogressingStore {
        fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError> {
            Ok(AuthorityIdentity {
                library_id: LIBRARY_ID.to_owned(),
                authority_generation: 7,
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
            })
        }

        fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError> {
            Ok(notes_protocol_capabilities())
        }

        fn bootstrap(
            &self,
        ) -> Result<tauri_app_lib::sync_protocol::BootstrapSnapshot, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }

        fn pull(
            &self,
            cursor: u64,
            _limit: u32,
        ) -> Result<tauri_app_lib::sync_protocol::ChangePage, AuthorityStoreError> {
            Ok(tauri_app_lib::sync_protocol::ChangePage {
                requested_cursor: cursor,
                next_cursor: cursor,
                high_water_cursor: cursor + 1,
                has_more: true,
                changes: Vec::new(),
            })
        }

        fn push(
            &mut self,
            _transaction: SignedTransaction,
            _now: u64,
        ) -> Result<tauri_app_lib::sync_protocol::SubmitOutcome, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }

        fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }

        fn acknowledge(
            &mut self,
            _device_id: &str,
            _cursor: u64,
            _checkpoint_digest: &str,
        ) -> Result<AckReceipt, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }

        fn revoke_device(&mut self, _device_id: &str) -> Result<(), AuthorityStoreError> {
            Ok(())
        }
    }

    let service = DirectSyncService::new(
        active_pairing(),
        NonprogressingStore,
        FixtureDirectCrypto::default(),
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    )
    .unwrap();
    let pull = signed_request(
        DirectEndpoint::Pull,
        25,
        PullRequest {
            cursor: 0,
            limit: 1,
            requested_record_kinds: notes_slice(),
        },
    );
    let response = service.handle(wire_request(DirectEndpoint::Pull, &pull));
    assert_eq!(response.status, 503);
    let error: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(error["error"]["code"], "state_unavailable");
}

#[test]
fn bootstrap_checkpoint_and_ack_share_one_dependency_complete_digest() {
    let mut authority = authority_with_devices(&[DEVICE_ID]);
    authority
        .submit_transaction(
            transaction(
                DEVICE_ID,
                1,
                50,
                vec![
                    ("note", 0, None, 1, b"fixture:note".to_vec()),
                    ("category", 0, None, 1, b"fixture:category".to_vec()),
                    ("folder", 0, None, 1, b"fixture:folder".to_vec()),
                ],
            ),
            NOW,
        )
        .unwrap();
    let (service, _, _) = service_with_authority(authority, DirectSyncLimits::default());

    let bootstrap_request = signed_request(
        DirectEndpoint::Bootstrap,
        18,
        BootstrapRequest {
            requested_record_kinds: notes_slice(),
            checkpoint_digest: None,
            after_record_id: None,
            limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
        },
    );
    let bootstrap: BootstrapResponse = response_payload(
        DirectEndpoint::Bootstrap,
        &service.handle(wire_request(DirectEndpoint::Bootstrap, &bootstrap_request)),
    );
    assert_eq!(bootstrap.page.records.len(), 3);
    assert!(!bootstrap.page.has_more);

    let checkpoint_request = signed_request(
        DirectEndpoint::Checkpoint,
        19,
        CheckpointRequest { known_cursor: None },
    );
    let checkpoint: CheckpointResponse = response_payload(
        DirectEndpoint::Checkpoint,
        &service.handle(wire_request(
            DirectEndpoint::Checkpoint,
            &checkpoint_request,
        )),
    );
    assert_eq!(
        bootstrap.page.checkpoint_digest,
        checkpoint.checkpoint.checkpoint_digest
    );

    let ack_request = signed_request(
        DirectEndpoint::Ack,
        20,
        AckRequest {
            high_water_cursor: bootstrap.page.high_water_cursor,
            checkpoint_digest: bootstrap.page.checkpoint_digest.clone(),
        },
    );
    let first_ack = service.handle(wire_request(DirectEndpoint::Ack, &ack_request));
    let second_ack = service.handle(wire_request(DirectEndpoint::Ack, &ack_request));
    assert_eq!(first_ack.status, 200);
    assert_eq!(first_ack.body, second_ack.body);
    let authenticated_ack: AckResponse = response_payload(DirectEndpoint::Ack, &first_ack);
    assert_eq!(authenticated_ack.receipt.device_id, DEVICE_ID);
    assert_eq!(
        authenticated_ack.receipt.high_water_cursor,
        bootstrap.page.high_water_cursor
    );

    let later_push = signed_request(
        DirectEndpoint::Push,
        24,
        PushRequest {
            transaction: single_record_transaction(2, 51, "note", b"fixture:later".to_vec()),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Push, &later_push))
            .status,
        200
    );
    let delayed_ack = service.handle(wire_request(DirectEndpoint::Ack, &ack_request));
    assert_eq!(first_ack.body, delayed_ack.body);

    let omit_dependency = signed_request(
        DirectEndpoint::Bootstrap,
        21,
        BootstrapRequest {
            requested_record_kinds: BTreeSet::from(["note".to_owned(), "folder".to_owned()]),
            checkpoint_digest: None,
            after_record_id: None,
            limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Bootstrap, &omit_dependency,))
            .status,
        403
    );
}

#[test]
fn partial_atomic_transaction_intersection_fails_closed() {
    struct MixedStore {
        capabilities: ProtocolCapabilities,
        page: tauri_app_lib::sync_protocol::ChangePage,
    }

    impl DirectSyncAuthority for MixedStore {
        fn identity(&self) -> Result<AuthorityIdentity, AuthorityStoreError> {
            Ok(AuthorityIdentity {
                library_id: LIBRARY_ID.to_owned(),
                authority_generation: 7,
                environment: Environment::Development,
                library_data_class: LibraryDataClass::SanitizedFixture,
            })
        }
        fn capabilities(&self) -> Result<ProtocolCapabilities, AuthorityStoreError> {
            Ok(self.capabilities.clone())
        }
        fn bootstrap(
            &self,
        ) -> Result<tauri_app_lib::sync_protocol::BootstrapSnapshot, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }
        fn pull(
            &self,
            _cursor: u64,
            _limit: u32,
        ) -> Result<tauri_app_lib::sync_protocol::ChangePage, AuthorityStoreError> {
            Ok(self.page.clone())
        }
        fn push(
            &mut self,
            _transaction: SignedTransaction,
            _now: u64,
        ) -> Result<tauri_app_lib::sync_protocol::SubmitOutcome, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }
        fn checkpoint(&self) -> Result<SyncCheckpoint, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }
        fn acknowledge(
            &mut self,
            _device_id: &str,
            _cursor: u64,
            _checkpoint_digest: &str,
        ) -> Result<AckReceipt, AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }
        fn revoke_device(&mut self, _device_id: &str) -> Result<(), AuthorityStoreError> {
            Err(AuthorityStoreError::StateUnavailable)
        }
    }

    // A hostile/corrupt store returns a transaction whose Note dependency is
    // grouped with a Media member outside the M4 subscription. The service must
    // not emit the Note and advance the cursor.
    let mixed = transaction(
        DEVICE_ID,
        1,
        60,
        vec![
            ("note", 0, None, 1, b"fixture:note".to_vec()),
            ("media", 0, None, 1, b"fixture:media".to_vec()),
        ],
    );
    let advances = mixed
        .members
        .iter()
        .map(|member| tauri_app_lib::sync_protocol::HeadAdvance {
            record_id: member.record_id.clone(),
            record_kind: member.record_kind.clone(),
            record_schema_version: member.record_schema_version,
            base_revision: member.base_head_revision,
            base_version_id: member.base_head_version_id.clone(),
            revision: member.proposed_revision,
            version_id: member.version_id.clone(),
            ciphertext_hash: member.ciphertext_hash.clone(),
        })
        .collect();
    let receipt = tauri_app_lib::sync_protocol::TransactionReceipt {
        library_id: LIBRARY_ID.to_owned(),
        transaction_id: mixed.manifest.transaction_id.clone(),
        transaction_digest: mixed.signed_digest(),
        mutation_ids: mixed
            .members
            .iter()
            .map(|member| member.mutation_id.clone())
            .collect(),
        device_id: DEVICE_ID.to_owned(),
        device_transaction_counter: 1,
        authority_generation: 7,
        purge_generation: 0,
        high_water_cursor: 1,
        disposition: ReceiptDisposition::Accepted { advances },
    };
    let page = tauri_app_lib::sync_protocol::ChangePage {
        requested_cursor: 0,
        next_cursor: 1,
        high_water_cursor: 1,
        has_more: false,
        changes: vec![tauri_app_lib::sync_protocol::AcceptedChange {
            sequence: 1,
            transaction_digest: mixed.signed_digest(),
            transaction: mixed,
            receipt,
        }],
    };
    let pairing = active_pairing();
    let crypto = FixtureDirectCrypto::default();
    let service = DirectSyncService::new(
        pairing,
        MixedStore {
            capabilities: notes_protocol_capabilities(),
            page,
        },
        crypto,
        DirectSyncConfig {
            library_id: LIBRARY_ID.to_owned(),
            authority_generation: 7,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            server_spki_sha256: vec![0x43; 32],
            limits: DirectSyncLimits::default(),
        },
    )
    .unwrap();
    let request = signed_request(
        DirectEndpoint::Pull,
        22,
        PullRequest {
            cursor: 0,
            limit: 10,
            requested_record_kinds: notes_slice(),
        },
    );
    assert_eq!(
        service
            .handle(wire_request(DirectEndpoint::Pull, &request))
            .status,
        403
    );
}

#[test]
fn response_limits_fail_closed_without_emitting_partial_json() {
    let mut limits = DirectSyncLimits::default();
    limits.checkpoint.response_bytes = 128;
    let (service, _, _) = service_with_authority(authority_with_devices(&[DEVICE_ID]), limits);
    let request = signed_request(
        DirectEndpoint::Checkpoint,
        23,
        CheckpointRequest { known_cursor: None },
    );
    let response = service.handle(wire_request(DirectEndpoint::Checkpoint, &request));
    assert_eq!(response.status, 413);
    let error: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(error["error"]["code"], "response_too_large");
}
