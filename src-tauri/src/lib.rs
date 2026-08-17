#[cfg(not(target_os = "ios"))]
include!("desktop.rs");

#[cfg(target_os = "ios")]
mod mobile;

#[cfg(target_os = "ios")]
pub use mobile::run;
