#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use crate::{Error, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const MAX_SIGNING_MESSAGE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_HPKE_FIELD_BYTES: usize = 256 * 1024;
pub(crate) const P256_PUBLIC_KEY_BYTES: usize = 65;
pub(crate) const P256_SIGNATURE_BYTES: usize = 64;
pub(crate) const X25519_PUBLIC_KEY_BYTES: usize = 32;
pub(crate) const SHA256_BYTES: usize = 32;
#[cfg(feature = "sanitized-development-fixtures")]
pub(crate) const FIXTURE_GATE: &str = "sanitized-development-fixture-v1";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityHandle(String);

impl IdentityHandle {
    pub(crate) fn parse(value: String) -> Result<Self> {
        validate_opaque_handle(&value)?;
        Ok(Self(value))
    }

    pub fn expose_opaque(&self) -> &str {
        &self.0
    }

    pub fn from_opaque(value: &str) -> Result<Self> {
        Self::parse(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PendingBootstrapHandle(String);

impl PendingBootstrapHandle {
    pub(crate) fn parse(value: String) -> Result<Self> {
        validate_opaque_handle(&value)?;
        Ok(Self(value))
    }

    pub fn expose_opaque(&self) -> &str {
        &self.0
    }

    pub fn from_opaque(value: &str) -> Result<Self> {
        Self::parse(value.to_owned())
    }
}

fn validate_opaque_handle(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let canonical = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        });
    if canonical {
        Ok(())
    } else {
        Err(Error::InvalidNativeResponse("opaque handle"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLifecycle {
    Pending,
    Active,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningKeyBacking {
    SecureEnclave,
    SoftwareFixture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicIdentity {
    pub handle: IdentityHandle,
    pub device_id: String,
    pub signing_public_key: Vec<u8>,
    pub hpke_public_key: Vec<u8>,
    pub lifecycle: IdentityLifecycle,
    pub signing_key_backing: SigningKeyBacking,
    pub bootstrap_recovery: Option<BootstrapRecovery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapRecovery {
    pub pending_bootstrap_handle: PendingBootstrapHandle,
    pub receipt_id: String,
    pub envelope_digest: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityInventory {
    pub pending: Vec<PublicIdentity>,
    pub active: Vec<PublicIdentity>,
    pub discarded: Vec<PublicIdentity>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IdentityInventoryWire {
    pub pending: Vec<PublicIdentityWire>,
    pub active: Vec<PublicIdentityWire>,
    pub discarded: Vec<PublicIdentityWire>,
}

impl TryFrom<IdentityInventoryWire> for IdentityInventory {
    type Error = Error;

    fn try_from(wire: IdentityInventoryWire) -> Result<Self> {
        Ok(Self {
            pending: wire
                .pending
                .into_iter()
                .map(PublicIdentity::try_from)
                .collect::<Result<Vec<_>>>()?,
            active: wire
                .active
                .into_iter()
                .map(PublicIdentity::try_from)
                .collect::<Result<Vec<_>>>()?,
            discarded: wire
                .discarded
                .into_iter()
                .map(PublicIdentity::try_from)
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicIdentityWire {
    pub handle: String,
    pub device_id: String,
    pub signing_public_key_base64: String,
    pub hpke_public_key_base64: String,
    pub lifecycle: IdentityLifecycle,
    pub signing_key_backing: SigningKeyBacking,
    pub bootstrap_recovery: Option<BootstrapRecoveryWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapRecoveryWire {
    pub pending_bootstrap_handle: String,
    pub receipt_id: String,
    pub envelope_digest_base64: String,
}

impl TryFrom<PublicIdentityWire> for PublicIdentity {
    type Error = Error;

    fn try_from(wire: PublicIdentityWire) -> Result<Self> {
        validate_uuid_v7(&wire.device_id)?;
        let signing_public_key = decode_exact(
            &wire.signing_public_key_base64,
            P256_PUBLIC_KEY_BYTES,
            "P-256 public key",
        )?;
        if signing_public_key.first() != Some(&0x04) {
            return Err(Error::InvalidNativeResponse("P-256 public key encoding"));
        }
        Ok(Self {
            handle: IdentityHandle::parse(wire.handle)?,
            device_id: wire.device_id,
            signing_public_key,
            hpke_public_key: decode_exact(
                &wire.hpke_public_key_base64,
                X25519_PUBLIC_KEY_BYTES,
                "X25519 public key",
            )?,
            lifecycle: wire.lifecycle,
            signing_key_backing: wire.signing_key_backing,
            bootstrap_recovery: wire
                .bootstrap_recovery
                .map(|recovery| -> Result<BootstrapRecovery> {
                    validate_uuid_v7(&recovery.receipt_id)?;
                    Ok(BootstrapRecovery {
                        pending_bootstrap_handle: PendingBootstrapHandle::parse(
                            recovery.pending_bootstrap_handle,
                        )?,
                        receipt_id: recovery.receipt_id,
                        envelope_digest: decode_exact(
                            &recovery.envelope_digest_base64,
                            SHA256_BYTES,
                            "bootstrap envelope digest",
                        )?,
                    })
                })
                .transpose()?,
        })
    }
}

pub(crate) fn validate_uuid_v7(value: &str) -> Result<()> {
    validate_opaque_handle(value)?;
    let bytes = value.as_bytes();
    if bytes[14] != b'7' || !matches!(bytes[19], b'8' | b'9' | b'a' | b'b') {
        return Err(Error::InvalidNativeResponse("UUIDv7 device ID"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrepareIdentityArgs<'a> {
    pub device_id: &'a str,
    pub fixture_gate: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityArgs<'a> {
    pub handle: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignArgs<'a> {
    pub handle: &'a str,
    pub message_base64: String,
}

impl<'a> SignArgs<'a> {
    pub(crate) fn new(handle: &'a IdentityHandle, message: &[u8]) -> Result<Self> {
        if message.len() > MAX_SIGNING_MESSAGE_BYTES {
            return Err(Error::InvalidNativeResponse("oversize signing request"));
        }
        Ok(Self {
            handle: handle.expose_opaque(),
            message_base64: BASE64.encode(message),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignatureWire {
    pub signature_base64: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifySignatureArgs {
    pub public_key_base64: String,
    pub message_base64: String,
    pub signature_base64: String,
}

impl VerifySignatureArgs {
    pub(crate) fn new(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<Self> {
        if public_key.len() != P256_PUBLIC_KEY_BYTES
            || public_key.first() != Some(&0x04)
            || signature.len() != P256_SIGNATURE_BYTES
            || message.len() > MAX_SIGNING_MESSAGE_BYTES
        {
            return Err(Error::InvalidNativeResponse(
                "invalid P-256 verification request",
            ));
        }
        Ok(Self {
            public_key_base64: BASE64.encode(public_key),
            message_base64: BASE64.encode(message),
            signature_base64: BASE64.encode(signature),
        })
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct VerificationWire {
    pub valid: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct FreshBytesArgs {
    pub length: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FreshBytesWire {
    pub bytes_base64: String,
}

impl FreshBytesWire {
    pub(crate) fn decode(self, expected: usize) -> Result<Vec<u8>> {
        decode_exact(&self.bytes_base64, expected, "secure random bytes")
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct FreshUuidV7Wire {
    pub value: String,
}

impl SignatureWire {
    pub(crate) fn decode(self) -> Result<Vec<u8>> {
        decode_exact(
            &self.signature_base64,
            P256_SIGNATURE_BYTES,
            "P-256 P1363 signature",
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenHpkeArgs<'a> {
    pub handle: &'a str,
    pub sender_public_key_base64: String,
    pub info_base64: String,
    pub associated_data_base64: String,
    pub encapsulated_key_base64: String,
    pub ciphertext_base64: String,
    pub exporter_context_base64: String,
}

impl<'a> OpenHpkeArgs<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        handle: &'a IdentityHandle,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        encapsulated_key: &[u8],
        ciphertext: &[u8],
        exporter_context: &[u8],
    ) -> Result<Self> {
        if sender_public_key.len() != X25519_PUBLIC_KEY_BYTES
            || encapsulated_key.len() != X25519_PUBLIC_KEY_BYTES
            || info.len() > MAX_HPKE_FIELD_BYTES
            || associated_data.len() > MAX_HPKE_FIELD_BYTES
            || ciphertext.len() > MAX_HPKE_FIELD_BYTES
            || ciphertext.len() < 16
            || exporter_context.len() > MAX_HPKE_FIELD_BYTES
        {
            return Err(Error::InvalidNativeResponse("invalid HPKE request"));
        }
        Ok(Self {
            handle: handle.expose_opaque(),
            sender_public_key_base64: BASE64.encode(sender_public_key),
            info_base64: BASE64.encode(info),
            associated_data_base64: BASE64.encode(associated_data),
            encapsulated_key_base64: BASE64.encode(encapsulated_key),
            ciphertext_base64: BASE64.encode(ciphertext),
            exporter_context_base64: BASE64.encode(exporter_context),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenHpkeWire {
    pub plaintext_base64: String,
    pub exporter_secret_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedHpke {
    pub plaintext: Vec<u8>,
    pub exporter_secret: [u8; SHA256_BYTES],
}

impl TryFrom<OpenHpkeWire> for OpenedHpke {
    type Error = Error;

    fn try_from(wire: OpenHpkeWire) -> Result<Self> {
        let plaintext = decode_bounded(
            &wire.plaintext_base64,
            MAX_HPKE_FIELD_BYTES,
            "HPKE plaintext",
        )?;
        let exporter = decode_exact(
            &wire.exporter_secret_base64,
            SHA256_BYTES,
            "HPKE exporter secret",
        )?;
        let mut exporter_secret = [0_u8; SHA256_BYTES];
        exporter_secret.copy_from_slice(&exporter);
        Ok(Self {
            plaintext,
            exporter_secret,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageBootstrapArgs<'a> {
    pub handle: &'a str,
    pub sender_public_key_base64: String,
    pub info_base64: String,
    pub associated_data_base64: String,
    pub encapsulated_key_base64: String,
    pub ciphertext_base64: String,
    pub receipt_id: &'a str,
    pub envelope_digest_base64: String,
}

impl<'a> StageBootstrapArgs<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        handle: &'a IdentityHandle,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        encapsulated_key: &[u8],
        ciphertext: &[u8],
        receipt_id: &'a str,
        envelope_digest: &[u8],
    ) -> Result<Self> {
        if sender_public_key.len() != X25519_PUBLIC_KEY_BYTES
            || encapsulated_key.len() != X25519_PUBLIC_KEY_BYTES
            || envelope_digest.len() != SHA256_BYTES
            || receipt_id.is_empty()
            || receipt_id.len() > 128
            || info.len() > MAX_HPKE_FIELD_BYTES
            || associated_data.len() > MAX_HPKE_FIELD_BYTES
            || ciphertext.len() > MAX_HPKE_FIELD_BYTES
            || ciphertext.len() < 16
        {
            return Err(Error::InvalidNativeResponse(
                "invalid staged-bootstrap request",
            ));
        }
        Ok(Self {
            handle: handle.expose_opaque(),
            sender_public_key_base64: BASE64.encode(sender_public_key),
            info_base64: BASE64.encode(info),
            associated_data_base64: BASE64.encode(associated_data),
            encapsulated_key_base64: BASE64.encode(encapsulated_key),
            ciphertext_base64: BASE64.encode(ciphertext),
            receipt_id,
            envelope_digest_base64: BASE64.encode(envelope_digest),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageBootstrapWire {
    pub pending_bootstrap_handle: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapTransitionArgs<'a> {
    pub identity_handle: &'a str,
    pub pending_bootstrap_handle: &'a str,
    pub receipt_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedDataState {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedDataEvent {
    pub state: ProtectedDataState,
    pub observed_at_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HardenStoreArgs {
    pub database_path: String,
    pub recovery_paths: Vec<String>,
}

impl HardenStoreArgs {
    pub(crate) fn new(database_path: &Path, recovery_paths: &[PathBuf]) -> Result<Self> {
        let database_path = absolute_utf8(database_path)?;
        let recovery_paths = recovery_paths
            .iter()
            .map(|path| absolute_utf8(path))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            database_path,
            recovery_paths,
        })
    }
}

fn absolute_utf8(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        return Err(Error::InvalidNativeResponse("non-absolute store path"));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or(Error::InvalidNativeResponse("non-UTF-8 store path"))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreProtectionReport {
    pub protection_class: String,
    pub hardened_paths: Vec<String>,
    pub inherited_pending_paths: Vec<String>,
    pub violations: Vec<String>,
}

impl StoreProtectionReport {
    pub fn is_compliant(&self) -> bool {
        self.protection_class == "NSFileProtectionComplete"
            && self.violations.is_empty()
            && self.inherited_pending_paths.is_empty()
    }
}

#[cfg(feature = "sanitized-development-fixtures")]
pub(crate) fn fixture_gate() -> &'static str {
    FIXTURE_GATE
}

fn decode_exact(value: &str, expected: usize, field: &'static str) -> Result<Vec<u8>> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| Error::InvalidNativeResponse(field))?;
    if decoded.len() != expected {
        return Err(Error::InvalidNativeResponse(field));
    }
    Ok(decoded)
}

fn decode_bounded(value: &str, maximum: usize, field: &'static str) -> Result<Vec<u8>> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| Error::InvalidNativeResponse(field))?;
    if decoded.len() > maximum {
        return Err(Error::InvalidNativeResponse(field));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_handles_require_canonical_lowercase_uuid_shape() {
        assert!(IdentityHandle::parse("018f47a0-7b80-7000-8000-000000000001".into()).is_ok());
        assert!(IdentityHandle::parse("018F47A0-7B80-7000-8000-000000000001".into()).is_err());
        assert!(IdentityHandle::parse("../../identity".into()).is_err());
    }

    #[test]
    fn p256_wire_shape_is_checked_before_use() {
        let wire = PublicIdentityWire {
            handle: "018f47a0-7b80-7000-8000-000000000001".into(),
            device_id: "018f47a0-7b80-7000-8000-000000000001".into(),
            signing_public_key_base64: BASE64.encode([0x04; 65]),
            hpke_public_key_base64: BASE64.encode([0x22; 32]),
            lifecycle: IdentityLifecycle::Pending,
            signing_key_backing: SigningKeyBacking::SecureEnclave,
            bootstrap_recovery: None,
        };
        assert!(PublicIdentity::try_from(wire).is_ok());
    }

    #[test]
    fn public_identity_rejects_non_v7_device_ids() {
        let wire = PublicIdentityWire {
            handle: "018f47a0-7b80-7000-8000-000000000001".into(),
            device_id: "018f47a0-7b80-4000-8000-000000000001".into(),
            signing_public_key_base64: BASE64.encode([0x04; 65]),
            hpke_public_key_base64: BASE64.encode([0x22; 32]),
            lifecycle: IdentityLifecycle::Pending,
            signing_key_backing: SigningKeyBacking::SecureEnclave,
            bootstrap_recovery: None,
        };
        assert!(PublicIdentity::try_from(wire).is_err());
    }

    #[test]
    fn public_inventory_recovery_exposes_only_opaque_bootstrap_binding() {
        let wire = PublicIdentityWire {
            handle: "018f47a0-7b80-4000-8000-000000000001".into(),
            device_id: "018f47a0-7b80-7000-8000-000000000002".into(),
            signing_public_key_base64: BASE64.encode([0x04; 65]),
            hpke_public_key_base64: BASE64.encode([0x22; 32]),
            lifecycle: IdentityLifecycle::Pending,
            signing_key_backing: SigningKeyBacking::SecureEnclave,
            bootstrap_recovery: Some(BootstrapRecoveryWire {
                pending_bootstrap_handle: "018f47a0-7b80-4000-8000-000000000003".into(),
                receipt_id: "018f47a0-7b80-7000-8000-000000000004".into(),
                envelope_digest_base64: BASE64.encode([0x55; 32]),
            }),
        };
        let identity = PublicIdentity::try_from(wire).expect("parse public recovery binding");
        let recovery = identity
            .bootstrap_recovery
            .expect("public recovery binding is present");
        assert_eq!(
            recovery.pending_bootstrap_handle.expose_opaque(),
            "018f47a0-7b80-4000-8000-000000000003"
        );
        assert_eq!(recovery.envelope_digest, vec![0x55; 32]);
    }

    #[test]
    fn identity_creation_binds_the_canonical_replica_device_id() {
        let args = PrepareIdentityArgs {
            device_id: "018f47a0-7b80-7000-8000-000000000006",
            fixture_gate: None,
        };
        let json = serde_json::to_value(args).expect("serialize native identity request");
        assert_eq!(
            json,
            serde_json::json!({
                "deviceId": "018f47a0-7b80-7000-8000-000000000006",
                "fixtureGate": null
            })
        );
        assert!(validate_uuid_v7("018f47a0-7b80-7000-8000-000000000006").is_ok());
        assert!(validate_uuid_v7("018f47a0-7b80-4000-8000-000000000006").is_err());
    }

    #[test]
    fn store_paths_must_be_absolute() {
        assert!(HardenStoreArgs::new(Path::new("noted.sqlite3"), &[]).is_err());
        assert!(HardenStoreArgs::new(Path::new("/tmp/noted.sqlite3"), &[]).is_ok());
    }
}
