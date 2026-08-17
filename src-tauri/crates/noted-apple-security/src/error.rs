use std::fmt;
#[cfg(target_os = "ios")]
use tauri::plugin::mobile::PluginInvokeError;

pub type Result<T> = std::result::Result<T, Error>;

/// Stable native error codes returned by the Apple security boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeErrorCode {
    InvalidArguments,
    PathRejected,
    KeychainFailure,
    PasscodeRequired,
    SecureEnclaveUnavailable,
    EntropyUnavailable,
    FixtureGateRejected,
    IdentityNotFound,
    InvalidIdentityState,
    IdentityCorrupted,
    BootstrapReplayMismatch,
    LegacyBootstrapRequiresDiscard,
    SigningFailed,
    HpkeOpenFailed,
    ProtectedDataUnavailable,
    FileProtectionFailed,
    BackupExclusionFailed,
    UnsupportedPlatform,
    Unknown,
}

impl NativeErrorCode {
    pub fn from_code(code: &str) -> Self {
        match code {
            "invalid_arguments" => Self::InvalidArguments,
            "path_rejected" => Self::PathRejected,
            "keychain_failure" => Self::KeychainFailure,
            "passcode_required" => Self::PasscodeRequired,
            "secure_enclave_unavailable" => Self::SecureEnclaveUnavailable,
            "entropy_unavailable" => Self::EntropyUnavailable,
            "fixture_gate_rejected" => Self::FixtureGateRejected,
            "identity_not_found" => Self::IdentityNotFound,
            "invalid_identity_state" => Self::InvalidIdentityState,
            "identity_corrupted" => Self::IdentityCorrupted,
            "bootstrap_replay_mismatch" => Self::BootstrapReplayMismatch,
            "legacy_bootstrap_requires_discard" => Self::LegacyBootstrapRequiresDiscard,
            "signing_failed" => Self::SigningFailed,
            "hpke_open_failed" => Self::HpkeOpenFailed,
            "protected_data_unavailable" => Self::ProtectedDataUnavailable,
            "file_protection_failed" => Self::FileProtectionFailed,
            "backup_exclusion_failed" => Self::BackupExclusionFailed,
            "unsupported_platform" => Self::UnsupportedPlatform,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    UnsupportedPlatform,
    InvalidNativeResponse(&'static str),
    Native {
        code: NativeErrorCode,
        raw_code: Option<String>,
        message: String,
    },
    #[cfg(target_os = "ios")]
    Bridge(PluginInvokeError),
}

#[cfg(target_os = "ios")]
impl From<PluginInvokeError> for Error {
    fn from(error: PluginInvokeError) -> Self {
        match error {
            PluginInvokeError::InvokeRejected(response) => {
                let raw_code = response.code;
                let code = raw_code
                    .as_deref()
                    .map(NativeErrorCode::from_code)
                    .unwrap_or(NativeErrorCode::Unknown);
                Self::Native {
                    code,
                    raw_code,
                    message: response
                        .message
                        .unwrap_or_else(|| "native Apple security operation failed".to_owned()),
                }
            }
            other => Self::Bridge(other),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "Apple security is available only in the iOS runtime"
                )
            }
            Self::InvalidNativeResponse(field) => {
                write!(formatter, "native Apple security returned invalid {field}")
            }
            Self::Native { code, message, .. } => write!(formatter, "{code:?}: {message}"),
            #[cfg(target_os = "ios")]
            Self::Bridge(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_error_codes_are_stable_and_fail_unknown_closed() {
        assert_eq!(
            NativeErrorCode::from_code("passcode_required"),
            NativeErrorCode::PasscodeRequired
        );
        assert_eq!(
            NativeErrorCode::from_code("protected_data_unavailable"),
            NativeErrorCode::ProtectedDataUnavailable
        );
        assert_eq!(
            NativeErrorCode::from_code("future_native_error"),
            NativeErrorCode::Unknown
        );
    }
}
