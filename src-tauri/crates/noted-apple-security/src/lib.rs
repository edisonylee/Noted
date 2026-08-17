//! Native iOS key-custody and Data Protection boundary for Noted.
//!
//! The crate is intentionally isolated until the direct-sync security gate is
//! approved. Rust receives opaque Keychain handles and public keys only. The
//! Secure Enclave signing reference, X25519 private key, and decrypted library
//! bootstrap stay inside the native boundary.

mod error;
mod models;

pub use error::{Error, NativeErrorCode, Result};
pub use models::{
    BootstrapRecovery, IdentityHandle, IdentityInventory, IdentityLifecycle, OpenedHpke,
    PendingBootstrapHandle, ProtectedDataEvent, ProtectedDataState, PublicIdentity,
    SigningKeyBacking, StoreProtectionReport,
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
