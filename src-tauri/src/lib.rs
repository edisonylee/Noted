#[cfg(not(target_os = "ios"))]
include!("desktop.rs");

#[cfg(target_os = "ios")]
mod mobile;

#[cfg(any(target_os = "ios", test))]
mod mobile_store;

pub mod portable;

#[cfg(target_os = "ios")]
pub use mobile::run;
