//! Desktop ownership for the sanitized iPhone companion authority.
//!
//! This is deliberately feature-gated and cannot open the personal Noted
//! database. It owns the authority runtime, private-LAN TLS listener, Bonjour
//! advertisement, invitation, and pending owner confirmation as one unit.

#![cfg(all(target_os = "macos", feature = "sanitized-development-fixtures"))]

use crate::{
    direct_pairing::{AuthorityBindings, AuthorityClock, AuthorityClockError},
    direct_sync::{DirectEndpoint, DirectRequest, DirectResponse, DirectSyncCrypto, DirectSyncLimits},
    direct_sync_transport::{
        DirectSyncRequestHandler, FixtureAuthorityRequestHandler, FixtureTlsIdentity,
        FixtureTransportPolicy, PairingEndpoint, PairingTransportRequest,
        PairingTransportResponse, SanitizedBonjourAdvertisement, SanitizedPrivateLanServer,
    },
    durable_direct_sync::FixtureAuthorityClock,
    fixture_authority_runtime::{
        provision_sanitized_fixture_authority, FixtureAuthorityError,
        SanitizedFixtureAuthorityRuntime,
    },
    pairing_protocol::{
        canonical_invitation_unsigned, AuthenticatedHpkeEnvelope, AuthenticatedHpkeSeal,
        BootstrapMetadataV1, Environment, FreshValuePurpose, Invitation, LibraryDataClass,
        LocalHpkeKey, LocalSigningKey, PairingCrypto, PairingRole, RecordKind,
        BOOTSTRAP_KEY_PACKAGE_BYTES, MAX_INVITATION_LIFETIME_MS, PAIRING_PROTOCOL,
        PAIRING_SUITE,
    },
    portable::new_uuid_v7,
};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, setup_sender,
    Deserializable, Kem as KemTrait, OpModeS, Serializable,
};
use p256::ecdsa::{
    signature::{Signer, Verifier},
    Signature, SigningKey, VerifyingKey,
};
use rand::{rngs::OsRng as RandOsRng, RngCore};
use rand_core::{OsRng as HpkeOsRng, TryRngCore};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex as AsyncMutex;
use zeroize::Zeroizing;

const FIXTURE_LIBRARY_KEY: [u8; 32] = [0x31; 32];
const FIXTURE_INSTANCE_NAME: &str = "Noted Fixture";

#[derive(Clone, Copy)]
struct SystemAuthorityClock;

impl AuthorityClock for SystemAuthorityClock {
    fn now_ms(&self) -> Result<i64, AuthorityClockError> {
        system_now_ms().map_err(|_| AuthorityClockError)
    }
}

impl FixtureAuthorityClock for SystemAuthorityClock {
    fn now_ms(&self) -> Result<i64, ()> {
        system_now_ms()
    }
}

#[derive(Clone)]
struct MacPairingCrypto {
    authority_signing_key: Arc<SigningKey>,
    pairing_signing_key: Arc<SigningKey>,
    hpke_private_key: Arc<Vec<u8>>,
    hpke_public_key: [u8; 32],
}

impl MacPairingCrypto {
    fn generate() -> Result<Self, String> {
        type Kem = X25519HkdfSha256;
        let mut signing_rng = RandOsRng;
        let authority_signing_key = Arc::new(SigningKey::random(&mut signing_rng));
        let pairing_signing_key = Arc::new(SigningKey::random(&mut signing_rng));
        let mut hpke_rng = HpkeOsRng.unwrap_err();
        let (private_key, public_key) = Kem::gen_keypair(&mut hpke_rng);
        let hpke_public_key: [u8; 32] = public_key
            .to_bytes()
            .as_slice()
            .try_into()
            .map_err(|_| "generated HPKE public key has an invalid length".to_string())?;
        Ok(Self {
            authority_signing_key,
            pairing_signing_key,
            hpke_private_key: Arc::new(private_key.to_bytes().to_vec()),
            hpke_public_key,
        })
    }

    fn authority_public_key(&self) -> [u8; 65] {
        p256_public_key(&self.authority_signing_key)
    }

    fn pairing_public_key(&self) -> [u8; 65] {
        p256_public_key(&self.pairing_signing_key)
    }

    fn seal(
        &self,
        recipient_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        plaintext: &[u8],
        exporter_context: &[u8],
    ) -> Result<AuthenticatedHpkeSeal, ()> {
        type Kem = X25519HkdfSha256;
        let sender_private = <Kem as KemTrait>::PrivateKey::from_bytes(&self.hpke_private_key)
            .map_err(|_| ())?;
        let sender_public = <Kem as KemTrait>::sk_to_pk(&sender_private);
        let recipient_public = <Kem as KemTrait>::PublicKey::from_bytes(recipient_public_key)
            .map_err(|_| ())?;
        let mut rng = HpkeOsRng.unwrap_err();
        let (encapsulated_key, mut sender) = setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
            &OpModeS::Auth((sender_private, sender_public)),
            &recipient_public,
            info,
            &mut rng,
        )
        .map_err(|_| ())?;
        let ciphertext = sender.seal(plaintext, associated_data).map_err(|_| ())?;
        let mut exporter_secret = [0_u8; 32];
        sender
            .export(exporter_context, &mut exporter_secret)
            .map_err(|_| ())?;
        Ok(AuthenticatedHpkeSeal {
            envelope: AuthenticatedHpkeEnvelope {
                encapsulated_key: encapsulated_key.to_bytes().to_vec(),
                ciphertext,
            },
            exporter_secret: Zeroizing::new(exporter_secret),
        })
    }
}

impl PairingCrypto for MacPairingCrypto {
    fn verify_signature(
        &self,
        _signer_role: PairingRole,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        verify_p256(public_key, message, signature)
    }

    fn sign(&self, key: LocalSigningKey, message: &[u8]) -> Result<Vec<u8>, ()> {
        let key = match key {
            LocalSigningKey::MacPairing => &self.pairing_signing_key,
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
        self.seal(
            recipient_public_key,
            info,
            associated_data,
            plaintext,
            exporter_context,
        )
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
        let mut package = Zeroizing::new(Vec::with_capacity(BOOTSTRAP_KEY_PACKAGE_BYTES));
        package.extend_from_slice(b"NBK1");
        package.extend_from_slice(&1_u32.to_be_bytes());
        package.extend_from_slice(&metadata.key_epoch.to_be_bytes());
        package.extend_from_slice(&FIXTURE_LIBRARY_KEY);
        self.seal(
            recipient_public_key,
            info,
            associated_data,
            package.as_slice(),
            exporter_context,
        )
    }

    fn fresh_bytes(&self, _purpose: FreshValuePurpose, length: usize) -> Result<Vec<u8>, ()> {
        let mut bytes = vec![0_u8; length];
        RandOsRng.fill_bytes(&mut bytes);
        Ok(bytes)
    }

    fn fresh_uuid_v7(&self, _purpose: FreshValuePurpose) -> Result<String, ()> {
        Ok(new_uuid_v7())
    }
}

#[derive(Clone)]
struct EnrolledDeviceSyncCrypto {
    database_path: Arc<PathBuf>,
    authority_signing_key: Arc<SigningKey>,
}

impl EnrolledDeviceSyncCrypto {
    fn device_signing_key(&self, device_id: &str) -> Result<Vec<u8>, ()> {
        let connection = Connection::open_with_flags(
            self.database_path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ())?;
        connection
            .query_row(
                "SELECT public_signing_key FROM portable_devices
                 WHERE device_id = ?1 AND enrollment_state = 'active' AND role = 'replica'",
                [device_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ())?
            .ok_or(())
    }
}

impl DirectSyncCrypto for EnrolledDeviceSyncCrypto {
    fn verify_request_signature(
        &self,
        _endpoint: DirectEndpoint,
        device_id: &str,
        signing_bytes: &[u8],
        signature: &[u8],
    ) -> Result<(), ()> {
        verify_p256(
            &self.device_signing_key(device_id)?,
            signing_bytes,
            signature,
        )
    }

    fn verify_mutation_ciphertext(
        &self,
        device_id: &str,
        mutation: &crate::sync_protocol::MutationEnvelope,
    ) -> Result<(), ()> {
        if mutation.device_id != device_id || !mutation.ciphertext.starts_with(b"NRC1") {
            return Err(());
        }
        verify_p256(
            &self.device_signing_key(device_id)?,
            &mutation.signing_bytes(),
            &mutation.signature,
        )
    }

    fn authenticate_response(
        &self,
        _endpoint: DirectEndpoint,
        signing_bytes: &[u8],
    ) -> Result<Vec<u8>, ()> {
        Ok(sign_p256(&self.authority_signing_key, signing_bytes))
    }
}

type AuthorityRuntime = SanitizedFixtureAuthorityRuntime<
    MacPairingCrypto,
    SystemAuthorityClock,
    EnrolledDeviceSyncCrypto,
>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingConfirmation {
    receipt_id: String,
    verification_code: String,
    scopes: BTreeSet<RecordKind>,
}

struct ManagedAuthorityHandler {
    runtime: AuthorityRuntime,
    pending_confirmation: Mutex<Option<PendingConfirmation>>,
}

impl DirectSyncRequestHandler for ManagedAuthorityHandler {
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.runtime.handle_direct_sync(request)
    }
}

impl FixtureAuthorityRequestHandler for ManagedAuthorityHandler {
    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse {
        if request.endpoint != PairingEndpoint::ClientHello {
            return self.runtime.handle_pairing(request);
        }
        match self
            .runtime
            .process_client_hello(&request.body, None, &request.transport)
        {
            Ok(result) => {
                if let Some(verification_code) = result.verification_code {
                    if let Ok(hello) = serde_json::from_slice::<crate::pairing_protocol::ClientHello>(
                        &request.body,
                    ) {
                        if let Ok(mut pending) = self.pending_confirmation.lock() {
                            *pending = Some(PendingConfirmation {
                                receipt_id: result.receipt_id,
                                verification_code,
                                scopes: hello.requested_scopes,
                            });
                        }
                    }
                }
                PairingTransportResponse {
                    status: 200,
                    body: result.exact_response_bytes,
                }
            }
            Err(_) => PairingTransportResponse {
                status: 400,
                body: br#"{"error":{"code":"pairing_rejected"}}"#.to_vec(),
            },
        }
    }
}

struct ActiveAuthority {
    _server: SanitizedPrivateLanServer,
    _advertisement: SanitizedBonjourAdvertisement,
    handler: Arc<ManagedAuthorityHandler>,
    info: MobileAuthorityInfo,
}

#[derive(Default)]
pub struct MobileAuthorityState(AsyncMutex<Option<ActiveAuthority>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAuthorityInfo {
    active: bool,
    address: String,
    port: u16,
    invitation_json: String,
    invitation_expires_at_ms: i64,
    pending_confirmation: Option<PendingConfirmation>,
}

#[tauri::command]
pub async fn mobile_authority_start(
    app: AppHandle,
    state: State<'_, MobileAuthorityState>,
) -> Result<MobileAuthorityInfo, String> {
    let mut active = state.0.lock().await;
    let now = system_now_ms().map_err(|_| "system clock is unavailable".to_string())?;
    let address = private_lan_ipv4()?;
    if let Some(authority) = active
        .as_ref()
        .filter(|authority| {
            authority.info.invitation_expires_at_ms > now
                && authority.info.address == address.to_string()
        })
    {
        return Ok(authority_info(authority));
    }
    // Dropping the expired listener also invalidates the old Bonjour endpoint.
    // A fresh authority below gets new keys, TLS identity, and invitation.
    *active = None;

    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("mobile-fixture");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let database_path = directory.join("authority.sqlite3");
    let descriptor = provision_sanitized_fixture_authority(&database_path)
        .map_err(display_authority_error)?;
    let tls_identity = FixtureTlsIdentity::generate_for_private_lan(address)
        .map_err(|error| error.to_string())?;
    let tls_pin = tls_identity.spki_sha256();
    let pairing_crypto = MacPairingCrypto::generate()?;
    let bindings = AuthorityBindings {
        authority_signing_public_key: pairing_crypto.authority_public_key(),
        mac_pairing_signing_public_key: pairing_crypto.pairing_public_key(),
        mac_pairing_hpke_public_key: pairing_crypto.hpke_public_key,
        tls_spki_sha256: tls_pin,
    };
    let runtime = AuthorityRuntime::open(
        &database_path,
        pairing_crypto.clone(),
        SystemAuthorityClock,
        EnrolledDeviceSyncCrypto {
            database_path: Arc::new(database_path.clone()),
            authority_signing_key: Arc::clone(&pairing_crypto.authority_signing_key),
        },
        Arc::new(SystemAuthorityClock),
        bindings.clone(),
    )
    .map_err(display_authority_error)?;
    let mut nonce = vec![0_u8; 32];
    RandOsRng.fill_bytes(&mut nonce);
    let mut invitation = Invitation {
        protocol: PAIRING_PROTOCOL.to_owned(),
        suite: PAIRING_SUITE.to_owned(),
        invitation_id: new_uuid_v7(),
        invitation_nonce: nonce,
        authority_signing_public_key: bindings.authority_signing_public_key.to_vec(),
        mac_pairing_signing_public_key: bindings.mac_pairing_signing_public_key.to_vec(),
        mac_pairing_hpke_public_key: bindings.mac_pairing_hpke_public_key.to_vec(),
        tls_spki_sha256: bindings.tls_spki_sha256.to_vec(),
        library_id: descriptor.library_id,
        authority_generation: descriptor.authority_generation,
        scope_ceiling: fixture_scopes(),
        created_at_ms: now,
        expires_at_ms: now + MAX_INVITATION_LIFETIME_MS,
        environment: Environment::Development,
        authority_role: PairingRole::MacAuthority,
        intended_client_role: PairingRole::IphoneCompanion,
        library_data_class: LibraryDataClass::SanitizedFixture,
        authority_signature: Vec::new(),
    };
    invitation.authority_signature = pairing_crypto
        .sign(
            LocalSigningKey::MacAuthority,
            &canonical_invitation_unsigned(&invitation),
        )
        .map_err(|_| "invitation signing failed".to_string())?;
    runtime
        .register_invitation(&invitation)
        .map_err(display_authority_error)?;
    let invitation_json = serde_json::to_string(&invitation).map_err(|error| error.to_string())?;
    let handler = Arc::new(ManagedAuthorityHandler {
        runtime,
        pending_confirmation: Mutex::new(None),
    });
    let policy = FixtureTransportPolicy::new_fixture_only(tls_pin, DirectSyncLimits::default())
        .map_err(|error| error.to_string())?;
    let server = SanitizedPrivateLanServer::spawn_authority_fixture_only(
        address,
        Arc::clone(&handler),
        tls_identity,
        policy,
    )
    .await
    .map_err(|error| error.to_string())?;
    let port = server.local_addr().port();
    let advertisement =
        SanitizedBonjourAdvertisement::start_fixture_only(FIXTURE_INSTANCE_NAME, address, port)
            .map_err(|error| error.to_string())?;
    let info = MobileAuthorityInfo {
        active: true,
        address: address.to_string(),
        port,
        invitation_json,
        invitation_expires_at_ms: invitation.expires_at_ms,
        pending_confirmation: None,
    };
    let authority = ActiveAuthority {
        _server: server,
        _advertisement: advertisement,
        handler,
        info,
    };
    let response = authority_info(&authority);
    *active = Some(authority);
    Ok(response)
}

#[tauri::command]
pub async fn mobile_authority_status(
    state: State<'_, MobileAuthorityState>,
) -> Result<Option<MobileAuthorityInfo>, String> {
    Ok(state.0.lock().await.as_ref().map(authority_info))
}

#[tauri::command]
pub async fn mobile_authority_confirm(
    state: State<'_, MobileAuthorityState>,
    receipt_id: String,
    verification_code: String,
    approved: bool,
) -> Result<MobileAuthorityInfo, String> {
    let active = state.0.lock().await;
    let authority = active
        .as_ref()
        .ok_or_else(|| "mobile authority is not running".to_string())?;
    let pending = authority
        .handler
        .pending_confirmation
        .lock()
        .map_err(|_| "pairing confirmation state is unavailable".to_string())?
        .clone()
        .ok_or_else(|| "no phone is waiting for confirmation".to_string())?;
    if pending.receipt_id != receipt_id || pending.verification_code != verification_code {
        return Err("pairing confirmation does not match the waiting phone".to_string());
    }
    authority
        .handler
        .runtime
        .confirm_owner(
            &pending.receipt_id,
            &pending.verification_code,
            &pending.scopes,
            approved,
        )
        .map_err(display_authority_error)?;
    *authority
        .handler
        .pending_confirmation
        .lock()
        .map_err(|_| "pairing confirmation state is unavailable".to_string())? = None;
    Ok(authority_info(authority))
}

fn authority_info(authority: &ActiveAuthority) -> MobileAuthorityInfo {
    let mut info = authority.info.clone();
    info.pending_confirmation = authority
        .handler
        .pending_confirmation
        .lock()
        .ok()
        .and_then(|pending| pending.clone());
    info
}

fn fixture_scopes() -> BTreeSet<RecordKind> {
    [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
        .into_iter()
        .collect()
}

fn private_lan_ipv4() -> Result<Ipv4Addr, String> {
    // Ask the kernel which source address it would use for the default route.
    // Enumerating interfaces on macOS commonly returns a VM bridge before Wi-Fi.
    // UDP connect selects a route without sending a packet.
    let route_probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| error.to_string())?;
    route_probe
        .connect((Ipv4Addr::new(1, 1, 1, 1), 80))
        .map_err(|error| error.to_string())?;
    match route_probe.local_addr().map_err(|error| error.to_string())? {
        SocketAddr::V4(address) if is_private_address(*address.ip()) => Ok(*address.ip()),
        SocketAddr::V6(_) | SocketAddr::V4(_) => {
            Err("connect the Mac to a private Wi-Fi or Ethernet network".to_string())
        }
    }
}

fn is_private_address(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    address.is_private() || (octets[0] == 169 && octets[1] == 254)
}

fn p256_public_key(key: &SigningKey) -> [u8; 65] {
    key.verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .try_into()
        .expect("P-256 uncompressed public key has a fixed length")
}

fn sign_p256(key: &SigningKey, message: &[u8]) -> Vec<u8> {
    let signature: Signature = key.sign(message);
    signature.to_bytes().to_vec()
}

fn verify_p256(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), ()> {
    let key = VerifyingKey::from_sec1_bytes(public_key).map_err(|_| ())?;
    let signature = Signature::from_slice(signature).map_err(|_| ())?;
    key.verify(message, &signature).map_err(|_| ())
}

fn system_now_ms() -> Result<i64, ()> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())?;
    i64::try_from(duration.as_millis()).map_err(|_| ())
}

fn display_authority_error(error: FixtureAuthorityError) -> String {
    error.to_string()
}
