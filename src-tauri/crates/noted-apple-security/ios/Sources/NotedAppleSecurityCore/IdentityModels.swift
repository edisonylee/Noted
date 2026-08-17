import Foundation

public enum IdentityLifecycle: String, Codable, Equatable, Sendable {
  case pending
  case active
  case discarded
}

public enum SigningKeyBacking: String, Codable, Equatable, Sendable {
  case secureEnclave = "secure_enclave"
  case softwareFixture = "software_fixture"
}

public struct PublicIdentityDescriptor: Codable, Equatable, Sendable {
  public let handle: String
  public let deviceId: String
  public let signingPublicKeyBase64: String
  public let hpkePublicKeyBase64: String
  public let lifecycle: IdentityLifecycle
  public let signingKeyBacking: SigningKeyBacking
  /// Public crash-recovery binding only. The decrypted bootstrap and every
  /// private key remain in the Data Protection Keychain record.
  public let bootstrapRecovery: BootstrapRecoveryDescriptor?
}

public struct BootstrapRecoveryDescriptor: Codable, Equatable, Sendable {
  public let pendingBootstrapHandle: String
  public let receiptId: String
  public let envelopeDigestBase64: String
  public let metadata: BootstrapMetadataV1
}

public struct IdentityInventory: Codable, Equatable, Sendable {
  public let pending: [PublicIdentityDescriptor]
  public let active: [PublicIdentityDescriptor]
  public let discarded: [PublicIdentityDescriptor]
}

struct StagedBootstrap: Codable, Equatable, Sendable {
  let handle: String
  let receiptId: String
  let envelopeDigest: Data
  let material: Data
  /// Optional only so the pre-contract fixture record can be decoded and
  /// classified for explicit discard. New staging never writes nil.
  let metadata: BootstrapMetadataV1?
}

struct IdentityRecord: Codable, Equatable, Sendable {
  let version: Int
  let handle: String
  let deviceId: String
  var lifecycle: IdentityLifecycle
  let signingKeyBacking: SigningKeyBacking
  var signingKeyRepresentation: Data?
  var agreementPrivateKey: Data?
  let signingPublicKey: Data
  let agreementPublicKey: Data
  var pendingBootstrap: StagedBootstrap?
  var activeBootstrap: StagedBootstrap?
  let createdAtMs: Int64
  var activatedAtMs: Int64?
}

enum IdentityLifecycleMachine {
  static func stage(
    record: inout IdentityRecord,
    bootstrapHandle: String,
    receiptId: String,
    envelopeDigest: Data,
    material: Data,
    metadata: BootstrapMetadataV1
  ) throws -> StagedBootstrap {
    guard record.lifecycle == .pending else {
      throw NotedSecurityError.invalidIdentityState(
        expected: IdentityLifecycle.pending.rawValue,
        actual: record.lifecycle.rawValue)
    }
    guard metadata.deviceId == record.deviceId else {
      throw NotedSecurityError.invalidArguments("bootstrap device binding")
    }
    if let existing = record.pendingBootstrap {
      guard existing.metadata != nil else {
        throw NotedSecurityError.legacyBootstrapRequiresDiscard
      }
      guard existing.receiptId == receiptId,
        existing.envelopeDigest == envelopeDigest,
        existing.material == material,
        existing.metadata == metadata
      else {
        throw NotedSecurityError.bootstrapReplayMismatch
      }
      return existing
    }
    let pending = StagedBootstrap(
      handle: bootstrapHandle,
      receiptId: receiptId,
      envelopeDigest: envelopeDigest,
      material: material,
      metadata: metadata)
    record.pendingBootstrap = pending
    return pending
  }

  static func activate(
    record: inout IdentityRecord,
    bootstrapHandle: String,
    receiptId: String,
    activatedAtMs: Int64
  ) throws {
    if record.lifecycle == .active {
      guard let active = record.activeBootstrap,
        active.handle == bootstrapHandle,
        active.receiptId == receiptId
      else {
        throw NotedSecurityError.bootstrapReplayMismatch
      }
      return
    }
    guard record.lifecycle == .pending else {
      throw NotedSecurityError.invalidIdentityState(
        expected: IdentityLifecycle.pending.rawValue,
        actual: record.lifecycle.rawValue)
    }
    guard let pending = record.pendingBootstrap,
      pending.handle == bootstrapHandle,
      pending.receiptId == receiptId
    else {
      throw NotedSecurityError.bootstrapReplayMismatch
    }
    record.lifecycle = .active
    record.activeBootstrap = pending
    record.pendingBootstrap = nil
    record.activatedAtMs = activatedAtMs
  }

  static func discardPending(
    record: inout IdentityRecord,
    bootstrapHandle: String?,
    receiptId: String?
  ) throws {
    if record.lifecycle == .discarded {
      return
    }
    guard record.lifecycle == .pending else {
      throw NotedSecurityError.invalidIdentityState(
        expected: IdentityLifecycle.pending.rawValue,
        actual: record.lifecycle.rawValue)
    }
    if bootstrapHandle != nil || receiptId != nil {
      guard let pending = record.pendingBootstrap,
        pending.handle == bootstrapHandle,
        pending.receiptId == receiptId
      else {
        throw NotedSecurityError.bootstrapReplayMismatch
      }
    }

    // This single-record transition is the logical commit point. All secret
    // representations are removed in the same SecItemUpdate as the tombstone.
    record.lifecycle = .discarded
    record.signingKeyRepresentation = nil
    record.agreementPrivateKey = nil
    record.pendingBootstrap = nil
    record.activeBootstrap = nil
    record.activatedAtMs = nil
  }
}

extension IdentityRecord {
  func publicDescriptor() -> PublicIdentityDescriptor {
    let bootstrap = (pendingBootstrap ?? activeBootstrap).flatMap { staged in
      staged.metadata.map { (staged, $0) }
    }
    return PublicIdentityDescriptor(
      handle: handle,
      deviceId: deviceId,
      signingPublicKeyBase64: signingPublicKey.base64EncodedString(),
      hpkePublicKeyBase64: agreementPublicKey.base64EncodedString(),
      lifecycle: lifecycle,
      signingKeyBacking: signingKeyBacking,
      bootstrapRecovery: bootstrap.map { staged, metadata in
        BootstrapRecoveryDescriptor(
          pendingBootstrapHandle: staged.handle,
          receiptId: staged.receiptId,
          envelopeDigestBase64: staged.envelopeDigest.base64EncodedString(),
          metadata: metadata)
      })
  }
}
