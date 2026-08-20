//! Native iOS key-custody and Data Protection boundary for Noted.
//!
//! The crate is intentionally isolated until the direct-sync security gate is
//! approved. Rust receives opaque Keychain handles and public keys only. The
//! Secure Enclave signing reference, X25519 private key, and decrypted library
//! bootstrap stay inside the native boundary.

mod error;
#[cfg(feature = "sanitized-development-fixtures")]
mod fixture_record_crypto;
mod models;
mod record_crypto;

pub use error::{Error, NativeErrorCode, Result};
#[cfg(feature = "sanitized-development-fixtures")]
pub use fixture_record_crypto::SanitizedFixtureRecordCrypto;
pub use models::{
    BootstrapCapabilityV1, BootstrapMetadataV1, BootstrapRecovery, IdentityHandle,
    IdentityInventory, IdentityLifecycle, OpenedHpke, PendingBootstrapHandle, ProtectedDataEvent,
    ProtectedDataState, PublicIdentity, SigningKeyBacking, StagedBootstrapDescriptor,
    StoreProtectionReport,
};
pub use record_crypto::{
    canonical_record_crypto_context, decode_record_ciphertext_v1, encode_record_ciphertext_v1,
    max_plaintext_for_transaction, record_associated_data, record_context_digest,
    record_envelope_digest, record_hkdf_info, record_hkdf_salt, record_signature_message,
    OpenedRecordV1, RecordCiphertextV1, RecordCryptoContextV1, RecordCryptoOperationV1,
    RecordKindV1, MAX_RECORD_CIPHERTEXT_BYTES, MAX_RECORD_CIPHERTEXT_CONTAINER_BYTES,
    MAX_RECORD_PLAINTEXT_BYTES, RECORD_CIPHERTEXT_V1_FIXED_OVERHEAD, RECORD_CIPHER_SUITE,
    RECORD_CRYPTO_CONTEXT_VERSION, RECORD_HKDF_SALT_DOMAIN, RECORD_NONCE_BYTES, RECORD_TAG_BYTES,
};

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

#[cfg(target_os = "ios")]
mod mobile;

#[cfg(target_os = "ios")]
pub use mobile::AppleSecurity;

#[cfg(not(target_os = "ios"))]
pub struct AppleSecurity<R: Runtime>(std::marker::PhantomData<fn() -> R>);

#[cfg(not(target_os = "ios"))]
impl<R: Runtime> AppleSecurity<R> {
    pub fn prepare_identity(&self, _device_id: &str) -> Result<PublicIdentity> {
        Err(Error::UnsupportedPlatform)
    }
}

/// Access the native boundary from an app or app handle.
pub trait AppleSecurityExt<R: Runtime> {
    fn apple_security(&self) -> &AppleSecurity<R>;
}

impl<R: Runtime, T: Manager<R>> AppleSecurityExt<R> for T {
    fn apple_security(&self) -> &AppleSecurity<R> {
        self.state::<AppleSecurity<R>>().inner()
    }
}

/// Construct the Tauri plugin. No commands are exposed to JavaScript; the app's
/// Rust pairing adapter is the only intended caller.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("noted-apple-security")
        .setup(|app, api| {
            #[cfg(target_os = "ios")]
            let security = mobile::init(app, api)?;
            #[cfg(not(target_os = "ios"))]
            let security = {
                let _ = api;
                AppleSecurity::<R>(std::marker::PhantomData)
            };
            app.manage(security);
            Ok(())
        })
        .build()
}
