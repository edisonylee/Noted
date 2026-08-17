import Foundation
import Security

public enum NotedSecurityError: Error, Equatable, Sendable {
  case invalidArguments(String)
  case pathRejected(String)
  case keychainFailure(OSStatus)
  case passcodeRequired
  case secureEnclaveUnavailable
  case entropyUnavailable
  case fixtureGateRejected
  case identityNotFound
  case invalidIdentityState(expected: String, actual: String)
  case identityCorrupted(String)
  case bootstrapReplayMismatch
  case legacyBootstrapRequiresDiscard
  case signingFailed
  case hpkeOpenFailed
  case protectedDataUnavailable
  case fileProtectionFailed(String)
  case backupExclusionFailed(String)
  case unsupportedPlatform

  public var code: String {
    switch self {
    case .invalidArguments: "invalid_arguments"
    case .pathRejected: "path_rejected"
    case .keychainFailure: "keychain_failure"
    case .passcodeRequired: "passcode_required"
    case .secureEnclaveUnavailable: "secure_enclave_unavailable"
    case .entropyUnavailable: "entropy_unavailable"
    case .fixtureGateRejected: "fixture_gate_rejected"
    case .identityNotFound: "identity_not_found"
    case .invalidIdentityState: "invalid_identity_state"
    case .identityCorrupted: "identity_corrupted"
    case .bootstrapReplayMismatch: "bootstrap_replay_mismatch"
    case .legacyBootstrapRequiresDiscard: "legacy_bootstrap_requires_discard"
    case .signingFailed: "signing_failed"
    case .hpkeOpenFailed: "hpke_open_failed"
    case .protectedDataUnavailable: "protected_data_unavailable"
    case .fileProtectionFailed: "file_protection_failed"
    case .backupExclusionFailed: "backup_exclusion_failed"
    case .unsupportedPlatform: "unsupported_platform"
    }
  }

  public var message: String {
    switch self {
    case .invalidArguments(let detail): "Invalid Apple security arguments: \(detail)"
    case .pathRejected(let path): "Path is outside the app container: \(path)"
    case .keychainFailure(let status): "Keychain operation failed (OSStatus \(status))"
    case .passcodeRequired: "A device passcode is required for this key custody policy"
    case .secureEnclaveUnavailable: "Secure Enclave P-256 signing is unavailable"
    case .entropyUnavailable: "Cryptographic random generation failed"
    case .fixtureGateRejected:
      "Software signing is restricted to an explicitly gated DEBUG simulator fixture"
    case .identityNotFound: "The opaque identity handle was not found"
    case .invalidIdentityState(let expected, let actual):
      "Identity state must be \(expected), found \(actual)"
    case .identityCorrupted(let detail): "The Keychain identity record is invalid: \(detail)"
    case .bootstrapReplayMismatch: "The pending bootstrap does not match the accepted replay"
    case .legacyBootstrapRequiresDiscard:
      "The legacy sanitized bootstrap must be discarded and paired again"
    case .signingFailed: "The native signing operation failed"
    case .hpkeOpenFailed: "Authenticated HPKE open failed"
    case .protectedDataUnavailable: "Protected data is unavailable while the device is locked"
    case .fileProtectionFailed(let path):
      "NSFileProtectionComplete could not be enforced for \(path)"
    case .backupExclusionFailed(let path): "Backup exclusion could not be enforced for \(path)"
    case .unsupportedPlatform: "This operation is available only on iOS"
    }
  }

  static func fromKeychainStatus(_ status: OSStatus) -> NotedSecurityError {
    switch status {
    case errSecInteractionNotAllowed:
      return .protectedDataUnavailable
    case errSecAuthFailed:
      return .passcodeRequired
    case -25295:  // errSecPasscodeRequired is not exported by every host SDK.
      return .passcodeRequired
    default:
      return .keychainFailure(status)
    }
  }
}
