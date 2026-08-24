#![cfg(all(not(target_os = "ios"), feature = "sanitized-development-fixtures"))]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use noted_apple_security::{
    BootstrapCapabilityV1 as NativeBootstrapCapabilityV1,
    BootstrapMetadataV1 as NativeBootstrapMetadataV1, SanitizedFixtureRecordCrypto,
    RECORD_CIPHER_SUITE,
};
use p256::ecdsa::{
    signature::{Signer, Verifier},
    Signature, SigningKey, VerifyingKey,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri_app_lib::{
    direct_authority_store::InvitationRegistration,
    direct_pairing::{
        AuthorityBindings, AuthorityClock, AuthorityClockError, OwnerConfirmationResult,
    },
    direct_sync::{
        DirectEndpoint, DirectRequest, DirectResponse, DirectSyncCrypto, DirectSyncLimits,
        NegotiateRequest, NegotiateResponse,
    },
    direct_sync_transport::{
        DirectSyncRequestHandler, FixtureLoopbackServer, FixtureTlsIdentity,
        FixtureTransportPolicy, PrivateLanDirectSyncSession,
    },
    durable_direct_sync::FixtureAuthorityClock,
    fixture_authority_runtime::{
        provision_sanitized_fixture_authority, SanitizedFixtureAuthorityDescriptor,
        SanitizedFixtureAuthorityRuntime,
    },
    fixture_record_crypto::SanitizedFixtureRecordCryptoAdapter,
    mobile_notes_sync::{MobileNotesSyncError, MobileNotesSyncOrchestrator},
    mobile_store::{MobilePairingActivation, MobilePairingCheckpoint, MobileStore},
    mobile_sync_runtime::{
        ExactRequestJournal, ExactRequestPurpose, MobileSyncRequestActor, MobileSyncRuntimeError,
    },
    mobile_sync_store_adapter::MobileStoreExactRequestJournal,
    pairing_client::{
        ClientFreshValuePurpose, ClientPublicIdentity, OpenedPairingChallenge, PairingClient,
        PairingClientConfig, PairingClientCrypto,
    },
    pairing_protocol::{
        canonical_invitation_unsigned, AuthenticatedHpkeEnvelope, AuthenticatedHpkeSeal,
        BootstrapEnvelope, BootstrapMetadataV1, Environment, FreshValuePurpose, Invitation,
        KindCapability, LibraryDataClass, LocalHpkeKey, LocalSigningKey, PairingCrypto,
        PairingRole, RecordKind, TransportEvidence, BOOTSTRAP_KEY_PACKAGE_BYTES, PAIRING_PROTOCOL,
        PAIRING_SUITE,
    },
    portable::{canonical_sha256, new_uuid_v7},
    sync_protocol::{
        MutationEnvelope, ProtocolCapabilities, RecordKindCapability, SYNC_PROTOCOL_VERSION,
    },
};
use zeroize::Zeroizing;

const FIXTURE_LIBRARY_KEY: [u8; 32] = [0x31; 32];
const PHONE_SIGNING_KEY: [u8; 32] = [0x61; 32];
const FIXTURE_SEED_SIGNING_KEY: [u8; 32] = [0x51; 32];
const AUTHORITY_SIGNING_KEY: [u8; 32] = [0x62; 32];
const MAC_PAIRING_SIGNING_KEY: [u8; 32] = [0x63; 32];
const MAC_PAIRING_HPKE_PUBLIC_KEY: [u8; 32] = [0x64; 32];
const PHONE_HPKE_PUBLIC_KEY: [u8; 32] = [0x65; 32];

const INVITATION_ID: &str = "00000000-0000-7000-8000-000000000101";
const RECEIPT_ID: &str = "00000000-0000-7000-8000-000000000102";
const CLIENT_HELLO_ID: &str = "00000000-0000-7000-8000-000000000103";
const CLIENT_FINISH_ID: &str = "00000000-0000-7000-8000-000000000104";
const IDENTITY_HANDLE: &str = "00000000-0000-4000-8000-000000000105";
const PENDING_BOOTSTRAP_HANDLE: &str = "00000000-0000-4000-8000-000000000106";

type AuthorityRuntime = SanitizedFixtureAuthorityRuntime<
    AuthorityPairingCrypto,
    FixedPairingClock,
    AuthoritySyncCrypto,
>;

struct TestPaths {
    directory: PathBuf,
    authority_database: PathBuf,
    mobile_database: PathBuf,
}

impl TestPaths {
    fn new() -> Self {
        let temp_root = fs::canonicalize(std::env::temp_dir()).expect("canonical temporary root");
        let directory = temp_root.join(format!(
            "noted-mobile-notes-encrypted-e2e-{}-{}",
            std::process::id(),
            new_uuid_v7()
        ));
        fs::create_dir(&directory).expect("create isolated E2E directory");
        Self {
            authority_database: directory.join("authority.sqlite3"),
            mobile_database: directory.join("mobile.sqlite3"),
            directory,
        }
    }
}

impl Drop for TestPaths {
    fn drop(&mut self) {
        for database in [&self.authority_database, &self.mobile_database] {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut path = database.as_os_str().to_owned();
                path.push(suffix);
                let _ = fs::remove_file(PathBuf::from(path));
            }
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[derive(Clone, Copy)]
struct FixedPairingClock(i64);

impl AuthorityClock for FixedPairingClock {
    fn now_ms(&self) -> Result<i64, AuthorityClockError> {
        Ok(self.0)
    }
}

#[derive(Clone, Copy)]
struct FixedAuthorityClock(i64);

impl FixtureAuthorityClock for FixedAuthorityClock {
    fn now_ms(&self) -> Result<i64, ()> {
        Ok(self.0)
    }
}

#[derive(Clone)]
struct AuthorityPairingCrypto {
    authority_signing_key: Arc<SigningKey>,
    mac_pairing_signing_key: Arc<SigningKey>,
}

impl AuthorityPairingCrypto {
    fn new() -> Self {
        Self {
            authority_signing_key: Arc::new(signing_key(AUTHORITY_SIGNING_KEY)),
            mac_pairing_signing_key: Arc::new(signing_key(MAC_PAIRING_SIGNING_KEY)),
        }
    }
}

impl PairingCrypto for AuthorityPairingCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        verify_p256(public_key, message, signature)
            .then_some(())
            .ok_or(())
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        let key = match key {
            LocalSigningKey::MacPairing => &self.mac_pairing_signing_key,
            LocalSigningKey::MacAuthority => &self.authority_signing_key,
        };
        Ok(sign_p256(key, message))
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
            &MAC_PAIRING_HPKE_PUBLIC_KEY,
            recipient_public_key,
            info,
            associated_data,
            plaintext,
            exporter_context,
        ))
    }

    fn seal_bootstrap_key_package(
        &self,
        _sender_key: LocalHpkeKey,
        recipient_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        metadata: &BootstrapMetadataV1,
        exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        let package = fixture_bootstrap_key_package(metadata.key_epoch);
        Ok(fixture_seal(
            &MAC_PAIRING_HPKE_PUBLIC_KEY,
            recipient_public_key,
            info,
            associated_data,
            package.as_slice(),
            exporter_context,
        ))
    }

    fn fresh_bytes(&self, purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        let byte = match purpose {
            FreshValuePurpose::ReceiptId => 0x71,
            FreshValuePurpose::ServerNonce => 0x72,
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

#[derive(Clone)]
struct AuthoritySyncCrypto {
    phone_device_id: String,
    phone_signing_public_key: Vec<u8>,
    authority_signing_key: Arc<SigningKey>,
}

impl DirectSyncCrypto for AuthoritySyncCrypto {
    fn verify_request_signature(
        &self,
        _endpoint: DirectEndpoint,
        device_id: &str,
        signing_bytes: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        (device_id == self.phone_device_id
            && verify_p256(&self.phone_signing_public_key, signing_bytes, signature))
        .then_some(())
        .ok_or(())
    }

    fn verify_mutation_ciphertext(
        &self,
        device_id: &str,
        mutation: &MutationEnvelope,
    ) -> Result<(), ()> {
        let signing_public_key = if device_id == self.phone_device_id {
            self.phone_signing_public_key.clone()
        } else {
            public_key(&signing_key(FIXTURE_SEED_SIGNING_KEY)).to_vec()
        };
        (mutation.device_id == device_id
            && mutation.ciphertext.starts_with(b"NRC1")
            && verify_p256(
                &signing_public_key,
                &mutation.signing_bytes(),
                &mutation.signature,
            ))
        .then_some(())
        .ok_or(())
    }

    fn authenticate_response(
        &self,
        _endpoint: DirectEndpoint,
        signing_bytes: &[u8],
    ) -> Result<Vec<u8>, ()> {
        Ok(sign_p256(&self.authority_signing_key, signing_bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingKeyReference(String);

#[derive(Default)]
struct ClientCustodyState {
    staged: BTreeMap<String, Vec<u8>>,
    active: BTreeSet<String>,
}

#[derive(Clone)]
struct ClientPairingCrypto {
    device_id: String,
    signing_key: Arc<SigningKey>,
    custody: Arc<Mutex<ClientCustodyState>>,
}

impl ClientPairingCrypto {
    fn new(device_id: String) -> Self {
        Self {
            device_id,
            signing_key: Arc::new(signing_key(PHONE_SIGNING_KEY)),
            custody: Arc::new(Mutex::new(ClientCustodyState::default())),
        }
    }
}

impl PairingClientCrypto for ClientPairingCrypto {
    type PendingKeyReference = PendingKeyReference;

    fn public_identity(&self) -> Result<ClientPublicIdentity, ()> {
        Ok(ClientPublicIdentity {
            device_id: self.device_id.clone(),
            signing_public_key: public_key(&self.signing_key).to_vec(),
            hpke_public_key: PHONE_HPKE_PUBLIC_KEY.to_vec(),
        })
    }

    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        verify_p256(public_key, message, signature)
            .then_some(())
            .ok_or(())
    }

    fn sign_device(&self, message: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(sign_p256(&self.signing_key, message))
    }

    fn open_challenge_authenticated(
        &self,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        envelope: &AuthenticatedHpkeEnvelope,
        exporter_context: &[u8],
    ) -> Result<OpenedPairingChallenge, ()> {
        let plaintext = fixture_open(
            sender_public_key,
            &PHONE_HPKE_PUBLIC_KEY,
            info,
            associated_data,
            envelope,
        )?;
        let exporter_secret = fixture_hash(&[
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
        metadata: &BootstrapMetadataV1,
        receipt: &tauri_app_lib::pairing_protocol::EnrollmentReceipt,
        envelope_digest: &[u8],
    ) -> Result<Self::PendingKeyReference, ()> {
        let plaintext = fixture_open(
            sender_public_key,
            &PHONE_HPKE_PUBLIC_KEY,
            info,
            associated_data,
            envelope,
        )?;
        if plaintext.len() != BOOTSTRAP_KEY_PACKAGE_BYTES
            || &plaintext[..4] != b"NBK1"
            || u64::from_be_bytes(plaintext[8..16].try_into().map_err(|_| ())?)
                != metadata.key_epoch
            || plaintext[16..] != FIXTURE_LIBRARY_KEY
            || metadata.receipt_id != receipt.receipt_id
        {
            return Err(());
        }
        let reference = PendingKeyReference(receipt.receipt_id.clone());
        let mut bound = envelope_digest.to_vec();
        bound.extend_from_slice(&plaintext);
        let mut custody = self.custody.lock().map_err(|_| ())?;
        match custody.staged.get(&reference.0) {
            Some(existing) if existing != &bound => return Err(()),
            Some(_) => {}
            None => {
                custody.staged.insert(reference.0.clone(), bound);
            }
        }
        Ok(reference)
    }

    fn activate_pending_bootstrap(
        &self,
        pending: &Self::PendingKeyReference,
        receipt: &tauri_app_lib::pairing_protocol::EnrollmentReceipt,
    ) -> Result<(), ()> {
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
        self.custody
            .lock()
            .map_err(|_| ())?
            .staged
            .remove(&pending.0);
        Ok(())
    }

    fn fresh_bytes(&self, purpose: ClientFreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        match purpose {
            ClientFreshValuePurpose::ClientNonce => Ok(vec![0x73; length]),
            ClientFreshValuePurpose::ClientHelloMessageId
            | ClientFreshValuePurpose::ClientFinishMessageId => Err(()),
        }
    }

    fn fresh_uuid_v7(&self, purpose: ClientFreshValuePurpose) -> Result<String, ()> {
        match purpose {
            ClientFreshValuePurpose::ClientHelloMessageId => Ok(CLIENT_HELLO_ID.to_owned()),
            ClientFreshValuePurpose::ClientFinishMessageId => Ok(CLIENT_FINISH_ID.to_owned()),
            ClientFreshValuePurpose::ClientNonce => Err(()),
        }
    }
}

struct RecordingAuthorityHandler {
    runtime: AuthorityRuntime,
    requests: Mutex<Vec<(DirectEndpoint, Vec<u8>)>>,
    responses: Mutex<Vec<(DirectEndpoint, Vec<u8>)>>,
    tamper_next_response: Mutex<Option<DirectEndpoint>>,
}

impl RecordingAuthorityHandler {
    fn new(runtime: AuthorityRuntime) -> Self {
        Self {
            runtime,
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
            tamper_next_response: Mutex::new(None),
        }
    }

    fn tamper_next(&self, endpoint: DirectEndpoint) {
        *self
            .tamper_next_response
            .lock()
            .expect("lock tamper selector") = Some(endpoint);
    }

    fn request_bodies(&self, endpoint: DirectEndpoint) -> Vec<Vec<u8>> {
        self.requests
            .lock()
            .expect("lock request recording")
            .iter()
            .filter(|(observed, _)| *observed == endpoint)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }

    fn response_bodies(&self, endpoint: DirectEndpoint) -> Vec<Vec<u8>> {
        self.responses
            .lock()
            .expect("lock response recording")
            .iter()
            .filter(|(observed, _)| *observed == endpoint)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

impl DirectSyncRequestHandler for RecordingAuthorityHandler {
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        let endpoint = DirectEndpoint::ALL
            .into_iter()
            .find(|endpoint| endpoint.path() == request.target)
            .expect("fixture server supplied one typed direct-sync endpoint");
        self.requests
            .lock()
            .expect("lock request recording")
            .push((endpoint, request.body.clone()));
        let mut response = self
            .runtime
            .handle_sync(request)
            .expect("fixture authority operation gate");
        self.responses
            .lock()
            .expect("lock response recording")
            .push((endpoint, response.body.clone()));
        let should_tamper = {
            let mut selected = self
                .tamper_next_response
                .lock()
                .expect("lock tamper selector");
            if *selected == Some(endpoint) {
                *selected = None;
                true
            } else {
                false
            }
        };
        if should_tamper && response.status == 200 {
            response.body = tamper_signed_response(&response.body);
        }
        response
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_mobile_notes_sync_survives_restart_tamper_and_revocation() {
    let paths = TestPaths::new();
    let pairing_now = now_ms();
    let limits = DirectSyncLimits::default();

    let mobile_store = MobileStore::open(&paths.mobile_database).expect("open mobile store");
    let phone_device_id = mobile_store
        .replica_device_id()
        .expect("read generated mobile replica identity");

    let tls_identity = FixtureTlsIdentity::generate().expect("generate fixture TLS identity");
    let tls_pin = tls_identity.spki_sha256();
    let authority_pairing_crypto = AuthorityPairingCrypto::new();
    let authority_signing_public_key = public_key(&authority_pairing_crypto.authority_signing_key);
    let mac_pairing_signing_public_key =
        public_key(&authority_pairing_crypto.mac_pairing_signing_key);
    let phone_signing_public_key = public_key(&signing_key(PHONE_SIGNING_KEY));

    let descriptor = provision_sanitized_fixture_authority(&paths.authority_database)
        .expect("provision sanitized fixture authority");
    let bindings = AuthorityBindings {
        authority_signing_public_key,
        mac_pairing_signing_public_key,
        mac_pairing_hpke_public_key: MAC_PAIRING_HPKE_PUBLIC_KEY,
        tls_spki_sha256: tls_pin,
    };
    let runtime = SanitizedFixtureAuthorityRuntime::open(
        &paths.authority_database,
        authority_pairing_crypto.clone(),
        FixedPairingClock(pairing_now),
        AuthoritySyncCrypto {
            phone_device_id: phone_device_id.clone(),
            phone_signing_public_key: phone_signing_public_key.to_vec(),
            authority_signing_key: Arc::clone(&authority_pairing_crypto.authority_signing_key),
        },
        Arc::new(FixedAuthorityClock(pairing_now)),
        bindings.clone(),
    )
    .expect("open fixture pairing and sync authority");
    let handler = Arc::new(RecordingAuthorityHandler::new(runtime));

    let bootstrap = pair_and_activate(
        &mobile_store,
        &handler.runtime,
        &descriptor,
        &bindings,
        &authority_pairing_crypto,
        pairing_now,
    );
    let profile = MobileStoreExactRequestJournal::new(&mobile_store)
        .active_sync_profile()
        .expect("derive active profile from exact activation");
    assert_eq!(profile.device_signing_public_key, phone_signing_public_key);
    assert_eq!(
        profile.authority_signing_public_key,
        authority_signing_public_key
    );
    assert_eq!(profile.durable_sync_spki_sha256, tls_pin);

    let record_custody = SanitizedFixtureRecordCrypto::new(
        native_bootstrap_metadata(&bootstrap.metadata),
        Zeroizing::new(FIXTURE_LIBRARY_KEY),
        Zeroizing::new(PHONE_SIGNING_KEY),
    )
    .expect("construct fixture/native-equivalent record custody");
    let record_crypto = SanitizedFixtureRecordCryptoAdapter::new(profile.clone(), record_custody)
        .expect("bind record custody to authenticated activation");

    let transport_policy =
        FixtureTransportPolicy::new_fixture_only(tls_pin, limits.clone()).expect("TLS policy");
    let server = FixtureLoopbackServer::spawn_fixture_only(
        Arc::clone(&handler),
        tls_identity,
        transport_policy,
    )
    .await
    .expect("start loopback fixture authority");
    let session = PrivateLanDirectSyncSession::from_loopback_fixture_for_test(
        &profile,
        server.local_addr(),
        limits.clone(),
    )
    .expect("construct actual private-LAN session");

    // Store an authenticated response, then simulate process death before the
    // semantic completion marker. The restarted orchestrator must consume the
    // durable response without replaying this exact request on the network.
    let capabilities = protocol_capabilities(&profile);
    let capabilities_sha256 =
        canonical_sha256(&serde_json::to_value(&capabilities).expect("capability value"));
    let mut crash_actor = MobileSyncRequestActor::new(
        MobileStoreExactRequestJournal::new(&mobile_store),
        &record_crypto,
        &session,
        limits.clone(),
    )
    .expect("construct exact-request actor");
    crash_actor
        .begin::<_, NegotiateResponse>(
            ExactRequestPurpose::Negotiate {
                capabilities_sha256,
            },
            NegotiateRequest { capabilities },
        )
        .await
        .expect("receive and durably store negotiation response");
    let stored_request = mobile_store
        .recover_direct_sync_requests()
        .expect("recover stored response")
        .into_iter()
        .next()
        .expect("one response-stored request");
    assert_eq!(stored_request.state, "response_received");
    assert_eq!(stored_request.attempts, 1);
    let stored_request_bytes = stored_request.request_bytes;
    drop(crash_actor);
    drop(mobile_store);

    let mobile_store = MobileStore::open(&paths.mobile_database).expect("restart mobile store");
    let mut orchestrator =
        MobileNotesSyncOrchestrator::new(&mobile_store, &record_crypto, &session, limits.clone())
            .expect("construct Notes orchestrator after restart");
    let bootstrap_report = orchestrator
        .sync_once()
        .await
        .expect("recover response and apply encrypted bootstrap");
    assert!(bootstrap_report.recovered_request);
    assert!(bootstrap_report.bootstrapped);
    assert!(bootstrap_report.bootstrap_records >= 3);
    assert!(bootstrap_report.acknowledged);
    assert_eq!(
        handler
            .request_bodies(DirectEndpoint::Negotiate)
            .iter()
            .filter(|body| body.as_slice() == stored_request_bytes.as_slice())
            .count(),
        1,
        "a response_received request must not be transmitted after restart"
    );

    let workspace = mobile_store
        .workspace(None, Some("all"), None)
        .expect("load bootstrapped workspace");
    let fixture_note = workspace
        .notes
        .iter()
        .find(|note| note.title == "Generated phone sync fixture")
        .expect("decrypted fixture note is available offline")
        .clone();
    assert!(fixture_note
        .body
        .contains("Generated development-only content"));
    assert_encrypted_bootstrap(&handler);

    // Make two independent offline writes. The first advances an accepted
    // authority head; the second creates a new canonical phone-owned record.
    let edited = mobile_store
        .update(
            &fixture_note.record_id,
            "Generated fixture edited offline",
            "Edited locally on the iPhone before reconnecting.",
        )
        .expect("edit the bootstrapped note offline");
    let created = mobile_store
        .create(
            "Created offline on iPhone",
            "This canonical note existed locally before the next TLS session.",
        )
        .expect("create a new note offline");
    assert_eq!(
        mobile_store
            .eligible_canonical_outbox_transaction_groups(16)
            .expect("inspect encrypted outbox candidates")
            .len(),
        2
    );

    let convergence_report = orchestrator
        .sync_once()
        .await
        .expect("push, pull echo, and acknowledge offline writes");
    assert_eq!(convergence_report.pushed_transactions, 2);
    assert_eq!(convergence_report.accepted_pushes, 2);
    assert!(convergence_report.pulled_transactions >= 2);
    assert!(convergence_report.pulled_records >= 2);
    assert!(convergence_report.acknowledged);
    assert!(mobile_store
        .eligible_canonical_outbox_transaction_groups(16)
        .expect("inspect settled canonical outbox")
        .is_empty());

    let converged = mobile_store
        .workspace(None, Some("all"), None)
        .expect("load converged workspace");
    assert!(converged.notes.iter().any(|note| {
        note.record_id == edited.record_id
            && note.title == edited.title
            && note.body == edited.body
            && note.sync_state == "synced"
    }));
    assert!(converged.notes.iter().any(|note| {
        note.record_id == created.record_id
            && note.title == created.title
            && note.body == created.body
            && note.sync_state == "synced"
    }));
    assert_phone_mutations_are_encrypted(
        &paths.authority_database,
        &phone_device_id,
        &[
            edited.title.as_str(),
            edited.body.as_str(),
            created.title.as_str(),
            created.body.as_str(),
        ],
    );

    // Corrupt one otherwise valid signed response. No response or semantic
    // state may be applied. Restart then proves that the unresolved journal
    // resends the byte-identical request before normal sync continues.
    let notes_before_tamper = converged.notes.clone();
    handler.tamper_next(DirectEndpoint::Negotiate);
    let negotiate_before = handler.request_bodies(DirectEndpoint::Negotiate).len();
    let tamper_error = orchestrator
        .sync_once()
        .await
        .expect_err("tampered authority signature must fail closed");
    assert!(matches!(
        tamper_error,
        MobileNotesSyncError::Runtime(MobileSyncRuntimeError::AuthoritySignatureRejected)
    ));
    assert_eq!(
        mobile_store
            .workspace(None, Some("all"), None)
            .expect("workspace after rejected response")
            .notes,
        notes_before_tamper
    );
    let tampered_request =
        handler.request_bodies(DirectEndpoint::Negotiate)[negotiate_before].clone();
    let unresolved = mobile_store
        .recover_direct_sync_requests()
        .expect("recover request after tamper rejection");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].state, "pending");
    assert_eq!(unresolved[0].request_bytes, tampered_request);
    drop(orchestrator);
    drop(mobile_store);

    let mobile_store = MobileStore::open(&paths.mobile_database).expect("restart after tamper");
    let mut orchestrator =
        MobileNotesSyncOrchestrator::new(&mobile_store, &record_crypto, &session, limits.clone())
            .expect("restore orchestrator after tamper");
    let replay_report = orchestrator
        .sync_once()
        .await
        .expect("replay exact request and converge after untampered response");
    assert!(replay_report.recovered_request);
    assert!(replay_report.acknowledged);
    assert_eq!(
        handler
            .request_bodies(DirectEndpoint::Negotiate)
            .iter()
            .filter(|body| body.as_slice() == tampered_request.as_slice())
            .count(),
        2,
        "an awaiting_response request must replay byte-identically after restart"
    );

    for endpoint in DirectEndpoint::ALL {
        assert!(
            !handler.request_bodies(endpoint).is_empty(),
            "{} was not exercised over pinned TLS",
            endpoint.path()
        );
    }

    handler
        .runtime
        .revoke_device(&phone_device_id, pairing_now + 1)
        .expect("revoke paired phone at the authority");
    let revoked_error = orchestrator
        .sync_once()
        .await
        .expect_err("revoked device cannot negotiate direct sync");
    assert!(matches!(
        revoked_error,
        MobileNotesSyncError::Runtime(MobileSyncRuntimeError::DeviceRevoked)
    ));
    let requests_after_revocation = handler.request_bodies(DirectEndpoint::Negotiate).len();
    drop(orchestrator);
    drop(mobile_store);

    // The authenticated HTTP revocation error is itself durable. A restart
    // consumes it locally rather than retrying the revoked request.
    let mobile_store = MobileStore::open(&paths.mobile_database).expect("restart revoked store");
    let mut orchestrator =
        MobileNotesSyncOrchestrator::new(&mobile_store, &record_crypto, &session, limits)
            .expect("restore revoked orchestrator");
    let recovered_revocation = orchestrator
        .sync_once()
        .await
        .expect_err("stored revocation remains terminal after restart");
    assert!(matches!(
        recovered_revocation,
        MobileNotesSyncError::Runtime(MobileSyncRuntimeError::DeviceRevoked)
    ));
    assert_eq!(
        handler.request_bodies(DirectEndpoint::Negotiate).len(),
        requests_after_revocation,
        "stored device_revoked response must not be replayed on the network"
    );

    drop(orchestrator);
    drop(mobile_store);
    server.shutdown().await.expect("stop fixture TLS server");
    drop(handler);
}

fn pair_and_activate(
    store: &MobileStore,
    runtime: &AuthorityRuntime,
    descriptor: &SanitizedFixtureAuthorityDescriptor,
    bindings: &AuthorityBindings,
    authority_crypto: &AuthorityPairingCrypto,
    now: i64,
) -> BootstrapEnvelope {
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: INVITATION_ID.to_owned(),
        invitation_nonce: vec![0x81; 32],
        authority_signing_public_key: bindings.authority_signing_public_key.to_vec(),
        mac_pairing_signing_public_key: bindings.mac_pairing_signing_public_key.to_vec(),
        mac_pairing_hpke_public_key: bindings.mac_pairing_hpke_public_key.to_vec(),
        tls_spki_sha256: bindings.tls_spki_sha256.to_vec(),
        library_id: descriptor.library_id.clone(),
        authority_generation: descriptor.authority_generation,
        scope_ceiling: fixture_scopes(),
        created_at_ms: now - 1_000,
        expires_at_ms: now + 299_000,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = sign_p256(
        &authority_crypto.authority_signing_key,
        &canonical_invitation_unsigned(&invitation),
    );
    assert_eq!(
        runtime
            .register_invitation(&invitation)
            .expect("register signed invitation"),
        InvitationRegistration::Registered
    );

    let client_crypto = ClientPairingCrypto::new(
        store
            .replica_device_id()
            .expect("read mobile identity for pairing"),
    );
    let mut client = PairingClient::new_fixture_only(
        client_crypto,
        PairingClientConfig {
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            requested_scopes: fixture_scopes(),
            capabilities: fixture_capabilities(),
            display_name: "Encrypted Notes E2E iPhone".to_owned(),
            app_version: "0.1.0-fixture".to_owned(),
            build_version: "1".to_owned(),
        },
        &serde_json::to_vec(&invitation).expect("encode invitation"),
        None,
        now,
    )
    .expect("construct actual pairing client");
    let transport = TransportEvidence {
        tls_version: "1.3".to_owned(),
        used_zero_rtt: false,
        peer_spki_sha256: bindings.tls_spki_sha256.to_vec(),
    };
    let client_hello = client
        .create_client_hello(&transport)
        .expect("create signed ClientHello");
    let begin = runtime
        .process_client_hello(&client_hello, None, &transport)
        .expect("process signed ClientHello");
    let confirmation = client
        .process_server_hello(&begin.exact_response_bytes, None, &transport, now)
        .expect("verify ServerHello and authenticated challenge");
    assert_eq!(
        begin.verification_code.as_deref(),
        Some(confirmation.verification_code.as_str())
    );
    client
        .confirm_on_device(
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .expect("confirm matching code on phone");
    let bootstrap_bytes = match runtime
        .confirm_owner(
            &confirmation.receipt_id,
            &confirmation.verification_code,
            &confirmation.granted_scopes,
            true,
        )
        .expect("confirm matching code on Mac")
    {
        OwnerConfirmationResult::Bootstrap(bytes) => bytes,
        OwnerConfirmationResult::Cancelled => panic!("approved pairing was cancelled"),
    };
    let bootstrap: BootstrapEnvelope =
        serde_json::from_slice(&bootstrap_bytes).expect("decode exact bootstrap envelope");
    let client_finish = client
        .process_bootstrap(&bootstrap_bytes, None, &transport, now)
        .expect("stage authenticated key package and create ClientFinish");
    let server_finish = runtime
        .process_client_finish(&client_finish, None, &transport)
        .expect("activate phone enrollment at authority");
    client
        .prepare_server_finish(&server_finish, None, &transport, now)
        .expect("verify exact ServerFinish before native activation");

    let pending = MobilePairingCheckpoint {
        identity_handle: IDENTITY_HANDLE.to_owned(),
        pending_bootstrap_handle: Some(PENDING_BOOTSTRAP_HANDLE.to_owned()),
        client: client.checkpoint(),
        updated_at: now,
    };
    store
        .save_pairing_checkpoint(&pending)
        .expect("persist exact PendingActivation predecessor");
    let pairing_activation = client
        .retry_activation()
        .expect("activate staged fixture custody");
    let active = MobilePairingCheckpoint {
        identity_handle: IDENTITY_HANDLE.to_owned(),
        pending_bootstrap_handle: None,
        client: client.checkpoint(),
        updated_at: now + 1,
    };
    let durable = MobilePairingActivation {
        receipt_id: pairing_activation.receipt.receipt_id,
        library_id: bootstrap.metadata.library_id.clone(),
        device_id: bootstrap.metadata.device_id.clone(),
        default_scope_id: bootstrap.metadata.default_scope_id.clone(),
        authority_generation: i64::try_from(bootstrap.metadata.authority_generation)
            .expect("authority generation fits SQLite"),
        purge_generation: i64::try_from(bootstrap.metadata.purge_generation)
            .expect("purge generation fits SQLite"),
        key_epoch: i64::try_from(bootstrap.metadata.key_epoch).expect("key epoch fits SQLite"),
        sync_spki_sha256: bootstrap.metadata.durable_sync_spki_sha256.clone(),
        record_cipher_suite: bootstrap.metadata.record_cipher_suite.clone(),
        granted_scopes: bootstrap.metadata.granted_scopes.clone(),
        capabilities: bootstrap.metadata.capabilities.clone(),
        checkpoint: active,
    };
    let result = store
        .finalize_pairing_activation(&durable)
        .expect("atomically finalize authenticated pairing activation");
    assert_eq!(result.adopted_note_count, 0);
    assert!(!result.replayed);
    bootstrap
}

fn fixture_scopes() -> BTreeSet<RecordKind> {
    [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
        .into_iter()
        .collect()
}

fn fixture_capabilities() -> BTreeMap<RecordKind, KindCapability> {
    fixture_scopes()
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

fn protocol_capabilities(
    profile: &tauri_app_lib::mobile_sync_runtime::ActiveSyncProfile,
) -> ProtocolCapabilities {
    ProtocolCapabilities::new(
        SYNC_PROTOCOL_VERSION,
        SYNC_PROTOCOL_VERSION,
        profile
            .capabilities
            .iter()
            .map(|(kind, capability)| {
                let name = match kind {
                    RecordKind::Note => "note",
                    RecordKind::Category => "category",
                    RecordKind::Folder => "folder",
                    RecordKind::Media => "media",
                };
                (
                    name.to_owned(),
                    RecordKindCapability::new(
                        capability.reader_version,
                        capability.writer_version.unwrap_or(0),
                    ),
                )
            })
            .collect(),
    )
}

fn native_bootstrap_metadata(metadata: &BootstrapMetadataV1) -> NativeBootstrapMetadataV1 {
    let capability = NativeBootstrapCapabilityV1 {
        reader_version: 1,
        writer_version: Some(1),
    };
    NativeBootstrapMetadataV1 {
        version: metadata.version,
        protocol: metadata.protocol.clone(),
        suite: metadata.suite.clone(),
        sync_protocol_version: metadata.sync_protocol_version,
        environment: "development".to_owned(),
        library_data_class: "sanitized_fixture".to_owned(),
        receipt_id: metadata.receipt_id.clone(),
        library_id: metadata.library_id.clone(),
        device_id: metadata.device_id.clone(),
        authority_generation: metadata.authority_generation,
        purge_generation: metadata.purge_generation,
        key_epoch: metadata.key_epoch,
        default_scope_id: metadata.default_scope_id.clone(),
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
        durable_sync_spki_sha256: metadata
            .durable_sync_spki_sha256
            .as_slice()
            .try_into()
            .expect("pairing bootstrap TLS pin"),
        transcript_digest: metadata
            .transcript_digest
            .as_slice()
            .try_into()
            .expect("pairing bootstrap transcript digest"),
    }
}

fn assert_encrypted_bootstrap(handler: &RecordingAuthorityHandler) {
    let responses = handler.response_bodies(DirectEndpoint::Bootstrap);
    assert!(!responses.is_empty());
    let mut encrypted_records = 0;
    for body in responses {
        assert!(!body
            .windows(b"Generated development-only content".len())
            .any(|window| window == b"Generated development-only content"));
        let response: tauri_app_lib::direct_sync::SignedSyncResponse<
            tauri_app_lib::direct_sync::BootstrapResponse,
        > = serde_json::from_slice(&body).expect("decode recorded bootstrap response");
        for record in response.payload.page.records {
            encrypted_records += 1;
            assert!(record.mutation.ciphertext.starts_with(b"NRC1"));
        }
    }
    assert!(encrypted_records >= 3);
}

fn assert_phone_mutations_are_encrypted(
    database: &Path,
    phone_device_id: &str,
    forbidden_plaintext: &[&str],
) {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open authority database for ciphertext assertion");
    let envelopes = connection
        .prepare(
            "SELECT mutations.envelope_json
             FROM direct_authority_mutations AS mutations
             JOIN direct_authority_transactions AS transactions USING (transaction_id)
             WHERE transactions.device_id = ?1 AND transactions.state = 'accepted'
             ORDER BY transactions.device_transaction_counter, mutations.member_index",
        )
        .and_then(|mut statement| {
            statement
                .query_map([phone_device_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("load accepted phone mutation envelopes");
    assert_eq!(envelopes.len(), 2);
    for envelope_json in envelopes {
        for plaintext in forbidden_plaintext {
            assert!(
                !envelope_json.contains(plaintext),
                "authority journal leaked canonical plaintext"
            );
        }
        let envelope: MutationEnvelope =
            serde_json::from_str(&envelope_json).expect("decode accepted mutation envelope");
        assert!(envelope.ciphertext.starts_with(b"NRC1"));
        assert!(verify_p256(
            &public_key(&signing_key(PHONE_SIGNING_KEY)),
            &envelope.signing_bytes(),
            &envelope.signature,
        ));
    }
}

fn signing_key(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes((&seed).into()).expect("valid fixed P-256 fixture key")
}

fn public_key(signing_key: &SigningKey) -> [u8; 65] {
    signing_key
        .verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .try_into()
        .expect("uncompressed P-256 public key")
}

fn sign_p256(signing_key: &SigningKey, message: &[u8]) -> Vec<u8> {
    let signature: Signature = signing_key.sign(message);
    signature.to_bytes().to_vec()
}

fn verify_p256(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(verifying_key) = VerifyingKey::from_sec1_bytes(public_key) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    verifying_key.verify(message, &signature).is_ok()
}

fn fixture_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn fixture_seal(
    sender_public_key: &[u8],
    recipient_public_key: &[u8],
    info: &[u8],
    associated_data: &[u8],
    plaintext: &[u8],
    exporter_context: &[u8],
) -> AuthenticatedHpkeSeal {
    let encapsulated_key = fixture_hash(&[
        b"noted.fixture/auth-hpke/encapsulated-key",
        recipient_public_key,
        info,
    ]);
    let tag = fixture_hash(&[
        b"noted.fixture/auth-hpke/tag",
        sender_public_key,
        recipient_public_key,
        &encapsulated_key,
        info,
        associated_data,
        plaintext,
    ]);
    let mut ciphertext = plaintext.to_vec();
    ciphertext.extend_from_slice(&tag[..16]);
    let exporter_secret = fixture_hash(&[
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
    let encapsulated_key = fixture_hash(&[
        b"noted.fixture/auth-hpke/encapsulated-key",
        recipient_public_key,
        info,
    ]);
    if envelope.encapsulated_key != encapsulated_key || envelope.ciphertext.len() < 16 {
        return Err(());
    }
    let split = envelope.ciphertext.len() - 16;
    let (plaintext, observed_tag) = envelope.ciphertext.split_at(split);
    let expected_tag = fixture_hash(&[
        b"noted.fixture/auth-hpke/tag",
        sender_public_key,
        recipient_public_key,
        &encapsulated_key,
        info,
        associated_data,
        plaintext,
    ]);
    if observed_tag != &expected_tag[..16] {
        return Err(());
    }
    Ok(plaintext.to_vec())
}

fn fixture_bootstrap_key_package(key_epoch: u64) -> Zeroizing<Vec<u8>> {
    let mut package = Zeroizing::new(Vec::with_capacity(BOOTSTRAP_KEY_PACKAGE_BYTES));
    package.extend_from_slice(b"NBK1");
    package.extend_from_slice(&1_u32.to_be_bytes());
    package.extend_from_slice(&key_epoch.to_be_bytes());
    package.extend_from_slice(&FIXTURE_LIBRARY_KEY);
    package
}

fn tamper_signed_response(body: &[u8]) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(body).expect("decode signed response to tamper");
    let signature = value
        .get_mut("signature")
        .and_then(Value::as_array_mut)
        .expect("signed response signature array");
    let first = signature
        .first()
        .and_then(Value::as_u64)
        .expect("P1363 signature byte");
    signature[0] = Value::from((first ^ 1) as u8);
    serde_json::to_vec(&value).expect("encode tampered signed response")
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_millis(),
    )
    .expect("current time fits i64")
}
