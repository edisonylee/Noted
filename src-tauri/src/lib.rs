#[cfg(not(target_os = "ios"))]
include!("desktop.rs");

#[cfg(target_os = "ios")]
mod mobile;

// The portable iPhone repository is public as a narrow integration-test seam.
// It is wired into the command surface only by `mobile.rs` on iOS.
pub mod mobile_store;

pub mod direct_pairing_delivery;
pub mod direct_sync;
pub mod direct_sync_transport;
pub mod fixture_record_crypto;
pub mod mobile_deep_link;
pub mod mobile_notes_sync;
pub mod mobile_pairing_runtime;
pub mod mobile_record_crypto;
#[cfg(target_os = "ios")]
pub mod mobile_sync_native;
pub mod mobile_sync_runtime;
pub mod mobile_sync_store_adapter;
pub mod pairing_client;
pub mod pairing_protocol;
pub mod portable;
pub mod sync_protocol;

#[cfg(target_os = "ios")]
pub use mobile::run;
