#[path = "../src/pairing_protocol.rs"]
mod pairing_protocol;

use pairing_protocol::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

const NOW: i64 = 1_725_000_000_000;
const INVITATION_ID: &str = "018f47a0-7b80-7000-8000-000000000001";
const HELLO_ID: &str = "018f47a0-7b80-7000-8000-000000000002";
const DEVICE_ID: &str = "018f47a0-7b80-7000-8000-000000000003";
const FINISH_ID: &str = "018f47a0-7b80-7000-8000-000000000004";
const LIBRARY_ID: &str = "018f47a0-7b80-7000-8000-000000000005";

struct FixtureCrypto {
    counter: AtomicU64,
    fail_next_server_nonce: AtomicBool,
}

impl Default for FixtureCrypto {
    fn default() -> Self {
        Self {
            counter: AtomicU64::new(1_000),
            fail_next_server_nonce: AtomicBool::new(false),
        }
    }
}

impl FixtureCrypto {
    fn signature(public_key: &[u8], message: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"noted.fixture/signature");
        hasher.update((public_key.len() as u64).to_be_bytes());
        hasher.update(public_key);
        hasher.update((message.len() as u64).to_be_bytes());
        hasher.update(message);
        let digest = hasher.finalize();
        [digest.as_slice(), digest.as_slice()].concat()
    }

    fn mac_pairing_public_key() -> Vec<u8> {
        let mut key = vec![2; 65];
        key[0] = 4;
        key
    }

    fn mac_authority_public_key() -> Vec<u8> {
        let mut key = vec![1; 65];
        key[0] = 4;
        key
    }

    fn failing_server_nonce_once() -> Self {
        Self {
            counter: AtomicU64::new(1_000),
            fail_next_server_nonce: AtomicBool::new(true),
        }
    }
}

impl PairingCrypto for FixtureCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        (signature == Self::signature(public_key, message))
            .then_some(())
            .ok_or(())
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        let public_key = match key {
            LocalSigningKey::MacPairing => Self::mac_pairing_public_key(),
            LocalSigningKey::MacAuthority => Self::mac_authority_public_key(),
        };
        Ok(Self::signature(&public_key, message))
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
        let mut encapsulated_hasher = Sha256::new();
        encapsulated_hasher.update(b"noted.fixture/auth-seal/encapsulated-key");
        encapsulated_hasher.update(recipient_public_key);
        encapsulated_hasher.update(info);
        let encapsulated_key: [u8; HPKE_ENCAPSULATED_KEY_BYTES] =
            encapsulated_hasher.finalize().into();

        let mut ciphertext_hasher = Sha256::new();
        ciphertext_hasher.update(b"noted.fixture/auth-seal/ciphertext");
        ciphertext_hasher.update(encapsulated_key);
        ciphertext_hasher.update(recipient_public_key);
        ciphertext_hasher.update(info);
        ciphertext_hasher.update(associated_data);
        ciphertext_hasher.update(plaintext);
        let ciphertext = ciphertext_hasher.finalize().to_vec();

        let mut exporter_hasher = Sha256::new();
        exporter_hasher.update(b"noted.fixture/auth-seal/exporter");
        exporter_hasher.update(encapsulated_key);
        exporter_hasher.update(&ciphertext);
        exporter_hasher.update(exporter_context);
        let exporter_secret: [u8; HPKE_EXPORTER_SECRET_BYTES] = exporter_hasher.finalize().into();

        Ok(AuthenticatedHpkeSeal {
            envelope: AuthenticatedHpkeEnvelope {
                encapsulated_key: encapsulated_key.to_vec(),
                ciphertext,
            },
            exporter_secret: zeroize::Zeroizing::new(exporter_secret),
        })
    }

    fn fresh_bytes(&self, purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        if purpose == FreshValuePurpose::ServerNonce
            && self.fail_next_server_nonce.swap(false, Ordering::SeqCst)
        {
            return Err(());
        }
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let tag = match purpose {
            FreshValuePurpose::ReceiptId => 0x71,
            FreshValuePurpose::ServerNonce => 0x72,
        };
        Ok((0..length)
            .map(|offset| tag ^ (counter as u8) ^ (offset as u8))
            .collect())
    }

    fn fresh_uuid_v7(&self, _purpose: FreshValuePurpose) -> Result<String, ()> {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(format!("018f47a0-7b80-7000-8000-{counter:012x}"))
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

fn policy() -> PairingPolicy {
    PairingPolicy {
        library_id: LIBRARY_ID.to_owned(),
        environment: Environment::Development,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_generation: 7,
        grantable_scopes: scopes(),
        capabilities: capabilities(),
    }
}

fn invitation(data_class: LibraryDataClass) -> Invitation {
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: INVITATION_ID.to_owned(),
        invitation_nonce: vec![0x11; 32],
        authority_signing_public_key: FixtureCrypto::mac_authority_public_key(),
        mac_pairing_signing_public_key: FixtureCrypto::mac_pairing_public_key(),
        mac_pairing_hpke_public_key: vec![0x22; 32],
        tls_spki_sha256: vec![0x33; 32],
        library_id: LIBRARY_ID.to_owned(),
        authority_generation: 7,
        scope_ceiling: scopes(),
        created_at_ms: NOW,
        expires_at_ms: NOW + MAX_INVITATION_LIFETIME_MS,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: data_class,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = FixtureCrypto::signature(
        &invitation.authority_signing_public_key,
        &canonical_invitation_unsigned(&invitation),
    );
    invitation
}

fn hello(invitation: &Invitation, message_id: &str, device_id: &str) -> ClientHello {
    let mut hello = ClientHello {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        message_id: message_id.to_owned(),
        invitation_id: invitation.invitation_id.clone(),
        nonce_proof: invitation_nonce_proof(&invitation.invitation_nonce),
        client_nonce: vec![0x44; 32],
        proposed_device_id: device_id.to_owned(),
        display_name: "Sanitized iPhone".to_owned(),
        client_signing_public_key: {
            let mut key = vec![0x55; 65];
            key[0] = 4;
            key
        },
        client_hpke_public_key: vec![0x66; 32],
        requested_scopes: scopes(),
        capabilities: capabilities(),
        app_version: "0.1.0-fixture".to_owned(),
        build_version: "1".to_owned(),
        library_id: invitation.library_id.clone(),
        authority_generation: invitation.authority_generation,
        environment: invitation.environment,
        sender_role: PairingRole::IphoneCompanion,
        recipient_role: PairingRole::MacAuthority,
        observed_tls_spki_sha256: invitation.tls_spki_sha256.clone(),
        proof_signature: Vec::new(),
    };
    sign_hello(&mut hello);
    hello
}

fn sign_hello(hello: &mut ClientHello) {
    hello.proof_signature = FixtureCrypto::signature(
        &hello.client_signing_public_key,
        &canonical_client_hello_unsigned(hello),
    );
}

fn transport(invitation: &Invitation) -> TransportEvidence {
    TransportEvidence {
        tls_version: "1.3".to_owned(),
        used_zero_rtt: false,
        peer_spki_sha256: invitation.tls_spki_sha256.clone(),
    }
}

fn register_and_begin(
    machine: &PairingMachine<FixtureCrypto>,
) -> (Invitation, ClientHello, Vec<u8>, BeginEnrollment) {
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();
    let hello = hello(&invitation, HELLO_ID, DEVICE_ID);
    let bytes = serde_json::to_vec(&hello).unwrap();
    let begin = machine
        .process_client_hello(&bytes, None, &transport(&invitation), NOW + 1_000)
        .unwrap();
    (invitation, hello, bytes, begin)
}

fn finish_for(begin: &BeginEnrollment, bootstrap: &BootstrapEnvelope) -> ClientFinish {
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
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
        bootstrap_envelope_digest: bootstrap.envelope_digest.clone(),
        proof_signature: Vec::new(),
    };
    finish.proof_signature = FixtureCrypto::signature(
        &{
            let mut key = vec![0x55; 65];
            key[0] = 4;
            key
        },
        &canonical_client_finish_unsigned(&finish),
    );
    finish
}

#[test]
fn pending_receipts_survive_restart_and_finish_replays_are_byte_identical() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, _hello, hello_bytes, begin) = register_and_begin(&machine);

    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Consumed)
    );
    assert_eq!(
        machine.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::PendingUserConfirmation)
    );
    let mut wrong_replay_pin = transport(&invitation);
    wrong_replay_pin.peer_spki_sha256[0] ^= 1;
    assert_eq!(
        machine
            .process_client_hello(
                &hello_bytes,
                Some("identity"),
                &wrong_replay_pin,
                NOW + 2_000,
            )
            .unwrap_err(),
        PairingError::PinMismatch
    );
    let replay = machine
        .process_client_hello(
            &hello_bytes,
            Some("identity"),
            &transport(&invitation),
            NOW + 2_000,
        )
        .unwrap();
    assert_eq!(replay, begin);

    let checkpoint = machine.checkpoint().unwrap();
    let restored =
        PairingMachine::restore_fixture_only(FixtureCrypto::default(), policy(), checkpoint)
            .unwrap();
    assert_eq!(
        restored.pending_server_hello(&begin.receipt_id).unwrap(),
        Some(begin.clone())
    );

    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    let bootstrap = restored
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW + 3_000,
        )
        .unwrap();
    let finish = finish_for(&begin, &bootstrap);
    let finish_bytes = serde_json::to_vec(&finish).unwrap();

    let pending_finish_checkpoint = restored.checkpoint().unwrap();
    let restored_again = PairingMachine::restore_fixture_only(
        FixtureCrypto::default(),
        policy(),
        pending_finish_checkpoint,
    )
    .unwrap();
    let first = restored_again
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 4_000)
        .unwrap();
    let activated_checkpoint = restored_again.checkpoint().unwrap();
    let activated_restore = PairingMachine::restore_fixture_only(
        FixtureCrypto::default(),
        policy(),
        activated_checkpoint,
    )
    .unwrap();
    let retry = activated_restore
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 9_000)
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(
        activated_restore.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::Active)
    );
    assert_eq!(
        activated_restore.device_state(DEVICE_ID).unwrap(),
        Some(DeviceState::Active)
    );
    activated_restore
        .require_active_device(DEVICE_ID, LIBRARY_ID, Environment::Development, 7)
        .unwrap();

    activated_restore
        .revoke_device(DEVICE_ID, NOW + 10_000)
        .unwrap();
    assert_eq!(
        activated_restore.device_state(DEVICE_ID).unwrap(),
        Some(DeviceState::Revoked)
    );
    assert_eq!(
        activated_restore
            .require_active_device(DEVICE_ID, LIBRARY_ID, Environment::Development, 7)
            .unwrap_err(),
        PairingError::DeviceRevoked
    );
    assert_eq!(
        activated_restore.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::Revoked)
    );
    assert_eq!(
        activated_restore
            .active_device_timestamps(DEVICE_ID)
            .unwrap(),
        Some((NOW + 4_000, Some(NOW + 10_000)))
    );
}

#[test]
fn personal_libraries_and_overlong_or_expired_invitations_fail_closed() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    assert_eq!(
        machine
            .register_invitation(invitation(LibraryDataClass::Personal), NOW)
            .unwrap_err(),
        PairingError::FixtureOnly
    );

    let mut too_long = invitation(LibraryDataClass::SanitizedFixture);
    too_long.expires_at_ms += 1;
    too_long.authority_signature = FixtureCrypto::signature(
        &too_long.authority_signing_public_key,
        &canonical_invitation_unsigned(&too_long),
    );
    assert_eq!(
        machine.register_invitation(too_long, NOW).unwrap_err(),
        PairingError::InvitationExpired
    );

    let expired = invitation(LibraryDataClass::SanitizedFixture);
    assert_eq!(
        machine
            .register_invitation(
                expired,
                NOW + MAX_INVITATION_LIFETIME_MS + MAX_CLOCK_SKEW_MS + 1
            )
            .unwrap_err(),
        PairingError::InvitationExpired
    );

    let mut overflowing = invitation(LibraryDataClass::SanitizedFixture);
    overflowing.created_at_ms = i64::MIN;
    overflowing.expires_at_ms = i64::MAX;
    overflowing.authority_signature = FixtureCrypto::signature(
        &overflowing.authority_signing_public_key,
        &canonical_invitation_unsigned(&overflowing),
    );
    assert_eq!(
        machine.register_invitation(overflowing, NOW).unwrap_err(),
        PairingError::InvitationExpired
    );

    let mut production_policy = policy();
    production_policy.environment = Environment::Production;
    assert!(matches!(
        PairingMachine::new_fixture_only(FixtureCrypto::default(), production_policy),
        Err(PairingError::FixtureOnly)
    ));
}

#[test]
fn unknown_hello_ids_and_transient_crypto_failures_cannot_poison_replay_capacity() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let registered_invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(registered_invitation.clone(), NOW)
        .unwrap();

    for offset in 0..(MAX_REPLAY_ENTRIES + 8) {
        let mut unknown = hello(
            &registered_invitation,
            &format!("018f47a0-7b80-7000-8000-{:012x}", 10_000 + offset),
            DEVICE_ID,
        );
        unknown.invitation_id = format!("018f47a0-7b80-7000-8000-{:012x}", 20_000 + offset);
        sign_hello(&mut unknown);
        assert_eq!(
            machine
                .process_client_hello(
                    &serde_json::to_vec(&unknown).unwrap(),
                    None,
                    &transport(&registered_invitation),
                    NOW + 1_000,
                )
                .unwrap_err(),
            PairingError::InvitationNotFound
        );
    }

    let valid = hello(&registered_invitation, HELLO_ID, DEVICE_ID);
    machine
        .process_client_hello(
            &serde_json::to_vec(&valid).unwrap(),
            None,
            &transport(&registered_invitation),
            NOW + 1_000,
        )
        .unwrap();

    let retry_machine =
        PairingMachine::new_fixture_only(FixtureCrypto::failing_server_nonce_once(), policy())
            .unwrap();
    let retry_invitation = invitation(LibraryDataClass::SanitizedFixture);
    retry_machine
        .register_invitation(retry_invitation.clone(), NOW)
        .unwrap();
    let retry_hello = hello(&retry_invitation, HELLO_ID, DEVICE_ID);
    let retry_bytes = serde_json::to_vec(&retry_hello).unwrap();
    assert_eq!(
        retry_machine
            .process_client_hello(
                &retry_bytes,
                None,
                &transport(&retry_invitation),
                NOW + 1_000,
            )
            .unwrap_err(),
        PairingError::CryptoUnavailable
    );
    let recovered = retry_machine
        .process_client_hello(
            &retry_bytes,
            None,
            &transport(&retry_invitation),
            NOW + 1_001,
        )
        .unwrap();
    let replay = retry_machine
        .process_client_hello(
            &retry_bytes,
            None,
            &transport(&retry_invitation),
            NOW + 1_002,
        )
        .unwrap();
    assert_eq!(recovered, replay);
}

#[test]
fn five_failed_attempts_cancel_an_invitation_durably() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();

    for attempt in 0..MAX_FAILED_ATTEMPTS {
        let message_id = format!("018f47a0-7b80-7000-8000-{:012x}", 100 + attempt as u64);
        let mut hello = hello(&invitation, &message_id, DEVICE_ID);
        hello.proof_signature[0] ^= 0xff;
        let error = machine
            .process_client_hello(
                &serde_json::to_vec(&hello).unwrap(),
                None,
                &transport(&invitation),
                NOW + 1_000,
            )
            .unwrap_err();
        if attempt + 1 == MAX_FAILED_ATTEMPTS {
            assert_eq!(error, PairingError::AttemptLimitReached);
        } else {
            assert_eq!(error, PairingError::InvalidSignature);
        }
    }
    assert_eq!(
        machine.invitation_failed_attempts(INVITATION_ID).unwrap(),
        Some(MAX_FAILED_ATTEMPTS)
    );
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );

    let checkpoint = machine.checkpoint().unwrap();
    let restored =
        PairingMachine::restore_fixture_only(FixtureCrypto::default(), policy(), checkpoint)
            .unwrap();
    assert_eq!(
        restored.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );
}

fn assert_hello_rejected(
    mutate_hello: impl FnOnce(&mut ClientHello),
    mutate_transport: impl FnOnce(&mut TransportEvidence),
    expected: PairingError,
) {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();
    let mut hello = hello(&invitation, HELLO_ID, DEVICE_ID);
    mutate_hello(&mut hello);
    sign_hello(&mut hello);
    let mut transport = transport(&invitation);
    mutate_transport(&mut transport);
    assert_eq!(
        machine
            .process_client_hello(
                &serde_json::to_vec(&hello).unwrap(),
                None,
                &transport,
                NOW + 1_000,
            )
            .unwrap_err(),
        expected
    );
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Pending)
    );
}

#[test]
fn transcript_and_transport_bind_scope_roles_environment_library_authority_and_pin() {
    assert_hello_rejected(
        |hello| {
            hello.requested_scopes.insert(RecordKind::Media);
            hello.capabilities.insert(
                RecordKind::Media,
                KindCapability {
                    reader_version: 1,
                    writer_version: Some(1),
                },
            );
        },
        |_| {},
        PairingError::ScopeCeilingExceeded,
    );
    assert_hello_rejected(
        |hello| hello.sender_role = PairingRole::MacAuthority,
        |_| {},
        PairingError::BindingMismatch("roles"),
    );
    assert_hello_rejected(
        |hello| hello.environment = Environment::Production,
        |_| {},
        PairingError::BindingMismatch("environment"),
    );
    assert_hello_rejected(
        |hello| hello.library_id = "018f47a0-7b80-7000-8000-000000000006".to_owned(),
        |_| {},
        PairingError::BindingMismatch("library_id"),
    );
    assert_hello_rejected(
        |hello| hello.authority_generation += 1,
        |_| {},
        PairingError::AuthorityChanged,
    );
    assert_hello_rejected(
        |_| {},
        |transport| transport.peer_spki_sha256[0] ^= 1,
        PairingError::PinMismatch,
    );
    assert_hello_rejected(
        |_| {},
        |transport| transport.tls_version = "1.2".to_owned(),
        PairingError::InsecureTransport,
    );
    assert_hello_rejected(
        |hello| {
            hello.capabilities.remove(&RecordKind::Folder);
        },
        |_| {},
        PairingError::CapabilityMismatch,
    );
    assert_hello_rejected(
        |hello| hello.suite = "noted.direct-pairing.v0".to_owned(),
        |_| {},
        PairingError::DowngradeRejected,
    );
}

#[test]
fn identical_hello_is_idempotent_but_changed_message_id_content_is_quarantined() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, mut hello, bytes, accepted) = register_and_begin(&machine);
    assert_eq!(
        machine
            .process_client_hello(&bytes, None, &transport(&invitation), NOW + 2_000)
            .unwrap(),
        accepted
    );

    hello.display_name = "Different iPhone".to_owned();
    sign_hello(&mut hello);
    assert_eq!(
        machine
            .process_client_hello(
                &serde_json::to_vec(&hello).unwrap(),
                None,
                &transport(&invitation),
                NOW + 3_000,
            )
            .unwrap_err(),
        PairingError::IdReuseQuarantined
    );
    let quarantines = machine.quarantine_records().unwrap();
    assert_eq!(quarantines.len(), 1);
    assert_eq!(quarantines[0].identifier, HELLO_ID);
    assert_ne!(
        quarantines[0].accepted_digest,
        quarantines[0].observed_digest
    );
}

#[test]
fn invitation_registration_is_idempotent_and_changed_content_is_quarantined() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();

    let mut changed = invitation;
    changed.invitation_nonce[0] ^= 1;
    changed.authority_signature = FixtureCrypto::signature(
        &changed.authority_signing_public_key,
        &canonical_invitation_unsigned(&changed),
    );
    assert_eq!(
        machine.register_invitation(changed, NOW).unwrap_err(),
        PairingError::IdReuseQuarantined
    );
    assert_eq!(machine.quarantine_records().unwrap().len(), 1);
}

#[test]
fn simultaneous_scans_have_one_atomic_winner() {
    let machine =
        Arc::new(PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap());
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine
        .register_invitation(invitation.clone(), NOW)
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));

    let mut handles = Vec::new();
    for offset in 0..2_u64 {
        let machine = Arc::clone(&machine);
        let invitation = invitation.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let message_id = format!("018f47a0-7b80-7000-8000-{:012x}", 200 + offset);
            let device_id = format!("018f47a0-7b80-7000-8000-{:012x}", 300 + offset);
            let hello = hello(&invitation, &message_id, &device_id);
            let bytes = serde_json::to_vec(&hello).unwrap();
            barrier.wait();
            machine.process_client_hello(&bytes, None, &transport(&invitation), NOW + 1_000)
        }));
    }
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(PairingError::InvitationConsumed)))
            .count(),
        1
    );
}

#[test]
fn confirmation_mismatch_cancels_and_finish_requires_confirmation() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, _hello, _bytes, begin) = register_and_begin(&machine);
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();

    let placeholder_bootstrap = BootstrapEnvelope {
        protocol: PAIRING_PROTOCOL.to_owned(),
        receipt_id: begin.receipt_id.clone(),
        sealed_bootstrap: AuthenticatedHpkeEnvelope {
            encapsulated_key: vec![1; HPKE_ENCAPSULATED_KEY_BYTES],
            ciphertext: vec![1],
        },
        envelope_digest: vec![0; 32],
    };
    let finish = finish_for(&begin, &placeholder_bootstrap);
    assert_eq!(
        machine
            .process_client_finish(
                &serde_json::to_vec(&finish).unwrap(),
                None,
                &transport(&invitation),
                NOW + 2_000,
            )
            .unwrap_err(),
        PairingError::UserConfirmationRequired
    );

    assert_eq!(
        machine
            .confirm_user(
                &begin.receipt_id,
                "0000 0000",
                &server_hello.receipt.granted_scopes,
                true,
                NOW + 3_000,
            )
            .unwrap_err(),
        PairingError::VerificationMismatch
    );
    assert_eq!(
        machine.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::Cancelled)
    );
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );
}

#[test]
fn client_finish_requires_tls13_without_early_data_and_the_original_pin() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, _hello, _bytes, begin) = register_and_begin(&machine);
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    let bootstrap = machine
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let finish = finish_for(&begin, &bootstrap);
    let finish_bytes = serde_json::to_vec(&finish).unwrap();

    let mut tls12 = transport(&invitation);
    tls12.tls_version = "1.2".to_owned();
    assert_eq!(
        machine
            .process_client_finish(&finish_bytes, None, &tls12, NOW + 3_000)
            .unwrap_err(),
        PairingError::InsecureTransport
    );

    let mut early_data = transport(&invitation);
    early_data.used_zero_rtt = true;
    assert_eq!(
        machine
            .process_client_finish(&finish_bytes, None, &early_data, NOW + 3_000)
            .unwrap_err(),
        PairingError::InsecureTransport
    );

    let mut wrong_pin = transport(&invitation);
    wrong_pin.peer_spki_sha256[0] ^= 1;
    assert_eq!(
        machine
            .process_client_finish(&finish_bytes, None, &wrong_pin, NOW + 3_000)
            .unwrap_err(),
        PairingError::PinMismatch
    );
    assert_eq!(
        machine.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::PendingFinish)
    );

    machine
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 3_000)
        .unwrap();
    assert_eq!(
        machine.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::Active)
    );

    let mut replay_wrong_pin = transport(&invitation);
    replay_wrong_pin.peer_spki_sha256[0] ^= 1;
    assert_eq!(
        machine
            .process_client_finish(&finish_bytes, None, &replay_wrong_pin, NOW + 3_001)
            .unwrap_err(),
        PairingError::PinMismatch
    );
}

#[test]
fn unknown_finish_ids_cannot_exhaust_replay_capacity() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, _hello, _bytes, begin) = register_and_begin(&machine);
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    let bootstrap = machine
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let finish = finish_for(&begin, &bootstrap);

    for offset in 0..(MAX_REPLAY_ENTRIES + 8) {
        let mut unknown = finish.clone();
        unknown.message_id = format!("018f47a0-7b80-7000-8000-{:012x}", 30_000 + offset);
        unknown.receipt_id = format!("018f47a0-7b80-7000-8000-{:012x}", 40_000 + offset);
        unknown.proof_signature = FixtureCrypto::signature(
            &{
                let mut key = vec![0x55; 65];
                key[0] = 4;
                key
            },
            &canonical_client_finish_unsigned(&unknown),
        );
        assert_eq!(
            machine
                .process_client_finish(
                    &serde_json::to_vec(&unknown).unwrap(),
                    None,
                    &transport(&invitation),
                    NOW + 3_000,
                )
                .unwrap_err(),
            PairingError::ReceiptNotFound
        );
    }

    let finish_bytes = serde_json::to_vec(&finish).unwrap();
    let activated = machine
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 3_000)
        .unwrap();
    let replay = machine
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 3_001)
        .unwrap();
    assert_eq!(activated, replay);
}

#[test]
fn changed_finish_content_reusing_an_id_is_quarantined() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (invitation, _hello, _bytes, begin) = register_and_begin(&machine);
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();
    let bootstrap = machine
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    let finish = finish_for(&begin, &bootstrap);
    let finish_bytes = serde_json::to_vec(&finish).unwrap();
    machine
        .process_client_finish(&finish_bytes, None, &transport(&invitation), NOW + 3_000)
        .unwrap();

    let mut changed = finish;
    changed.environment = Environment::Production;
    changed.proof_signature = FixtureCrypto::signature(
        &{
            let mut key = vec![0x55; 65];
            key[0] = 4;
            key
        },
        &canonical_client_finish_unsigned(&changed),
    );
    assert_eq!(
        machine
            .process_client_finish(
                &serde_json::to_vec(&changed).unwrap(),
                None,
                &transport(&invitation),
                NOW + 4_000,
            )
            .unwrap_err(),
        PairingError::IdReuseQuarantined
    );
}

#[test]
fn restart_and_authority_rotation_invalidate_only_unfinished_work() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invite = invitation(LibraryDataClass::SanitizedFixture);
    machine.register_invitation(invite, NOW).unwrap();
    machine.handle_restart(Some(INVITATION_ID)).unwrap();
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Pending)
    );
    machine.cancel_invitation(INVITATION_ID).unwrap();
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );

    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invite = invitation(LibraryDataClass::SanitizedFixture);
    machine.register_invitation(invite, NOW).unwrap();
    machine.handle_restart(None).unwrap();
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );

    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (_invitation, _hello, _bytes, begin) = register_and_begin(&machine);
    machine.rotate_authority_generation(8).unwrap();
    assert_eq!(
        machine.invitation_state(INVITATION_ID).unwrap(),
        Some(InvitationState::Cancelled)
    );
    assert_eq!(
        machine.receipt_state(&begin.receipt_id).unwrap(),
        Some(ReceiptState::Cancelled)
    );
}

#[test]
fn checkpoint_restore_rejects_a_narrowed_or_other_policy() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let checkpoint = machine.checkpoint().unwrap();
    let mut narrowed = policy();
    narrowed.grantable_scopes.remove(&RecordKind::Folder);
    narrowed.capabilities.remove(&RecordKind::Folder);

    assert!(matches!(
        PairingMachine::restore_fixture_only(FixtureCrypto::default(), narrowed, checkpoint),
        Err(PairingError::BindingMismatch("checkpoint policy"))
    ));
}

#[test]
fn parser_rejects_compression_size_depth_duplicates_unknown_fields_and_floats() {
    assert_eq!(
        parse_bounded_json::<ClientHello>(b"{}", Some("gzip")).unwrap_err(),
        PairingError::UnsupportedEncoding
    );
    assert_eq!(
        parse_bounded_json::<ClientHello>(&vec![b' '; MAX_PAIRING_MESSAGE_BYTES + 1], None)
            .unwrap_err(),
        PairingError::PayloadTooLarge
    );

    let deeply_nested = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH + 2),
        "]".repeat(MAX_JSON_DEPTH + 2)
    );
    assert!(matches!(
        parse_bounded_json::<serde_json::Value>(deeply_nested.as_bytes(), None),
        Err(PairingError::ParseRejected(_))
    ));
    assert!(matches!(
        parse_bounded_json::<serde_json::Value>(br#"{"a":1,"a":1}"#, None),
        Err(PairingError::ParseRejected(_))
    ));
    assert!(matches!(
        parse_bounded_json::<serde_json::Value>(br#"{"a":1.5}"#, None),
        Err(PairingError::ParseRejected(_))
    ));

    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    let hello = hello(&invitation, HELLO_ID, DEVICE_ID);
    let mut value = serde_json::to_value(hello).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_owned(), Value::Bool(true));
    assert!(matches!(
        parse_bounded_json::<ClientHello>(&serde_json::to_vec(&value).unwrap(), None),
        Err(PairingError::ParseRejected(_))
    ));
}

#[test]
fn malformed_payloads_do_not_consume_attempts() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    machine.register_invitation(invitation, NOW).unwrap();
    assert!(matches!(
        machine.process_client_hello(
            b"not-json",
            None,
            &TransportEvidence {
                tls_version: "1.3".to_owned(),
                used_zero_rtt: false,
                peer_spki_sha256: vec![0x33; 32],
            },
            NOW + 1_000
        ),
        Err(PairingError::ParseRejected(_))
    ));
    assert_eq!(
        machine.invitation_failed_attempts(INVITATION_ID).unwrap(),
        Some(0)
    );
}

#[test]
fn hpke_wire_envelopes_bind_the_encapsulated_key_and_ciphertext_atomically() {
    let machine = PairingMachine::new_fixture_only(FixtureCrypto::default(), policy()).unwrap();
    let (_invitation, _hello, _hello_bytes, begin) = register_and_begin(&machine);
    let server_hello: ServerHello = serde_json::from_slice(&begin.server_hello_bytes).unwrap();

    assert_eq!(
        server_hello.challenge.encapsulated_key.len(),
        HPKE_ENCAPSULATED_KEY_BYTES
    );
    assert!(!server_hello.challenge.ciphertext.is_empty());

    let bootstrap = machine
        .confirm_user(
            &begin.receipt_id,
            &begin.verification_code,
            &server_hello.receipt.granted_scopes,
            true,
            NOW + 2_000,
        )
        .unwrap();
    assert_eq!(
        bootstrap.sealed_bootstrap.encapsulated_key.len(),
        HPKE_ENCAPSULATED_KEY_BYTES
    );
    assert_eq!(
        bootstrap.envelope_digest,
        Sha256::digest(canonical_authenticated_hpke_envelope(
            &bootstrap.sealed_bootstrap
        ))
        .to_vec()
    );
    assert_ne!(server_hello.challenge, bootstrap.sealed_bootstrap);

    let mut substituted = bootstrap.sealed_bootstrap.clone();
    substituted.encapsulated_key[0] ^= 1;
    assert_ne!(
        Sha256::digest(canonical_authenticated_hpke_envelope(&substituted)).to_vec(),
        bootstrap.envelope_digest
    );
}

#[test]
fn canonical_transcript_and_sas_fixture_are_stable() {
    let invitation = invitation(LibraryDataClass::SanitizedFixture);
    let hello = hello(&invitation, HELLO_ID, DEVICE_ID);
    let invitation_digest = Sha256::digest(canonical_invitation_unsigned(&invitation));
    let hello_digest = Sha256::digest(canonical_client_hello_unsigned(&hello));
    let code = derive_verification_code(&invitation_digest, &hello_digest);

    assert_eq!(
        format!("{invitation_digest:x}"),
        "b162694b79bdf9a1e1888882aad1b85a1770f3a8db67697e29f8b441ae024280"
    );
    assert_eq!(
        format!("{hello_digest:x}"),
        "1846adbad4ad800cfd286ca0ddecf0d8fae0d8e8b6354b7dec4f1cee9a3de11d"
    );
    assert_eq!(code, "9250 3210");
}
