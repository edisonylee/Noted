#[cfg(not(target_os = "ios"))]
include!("desktop.rs");

#[cfg(target_os = "ios")]
mod mobile;

// The portable iPhone repository is public as a narrow integration-test seam.
// It is wired into the command surface only by `mobile.rs` on iOS.
pub mod mobile_store;

pub mod direct_sync;
pub mod mobile_deep_link;
pub mod pairing_protocol;
pub mod portable;
pub mod sync_protocol;

#[cfg(target_os = "ios")]
pub use mobile::run;
