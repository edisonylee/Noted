import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class IdentityLifecycleTests: XCTestCase {
  private func metadata(keyEpoch: UInt64 = 3) -> BootstrapMetadataV1 {
    let capability = BootstrapCapabilityV1(readerVersion: 1, writerVersion: 1)
    return BootstrapMetadataV1(
      version: 1,
      protocolName: "noted.direct-pairing.v1",
      suite: "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm",
      syncProtocolVersion: 1,
      environment: "development",
      libraryDataClass: "sanitized_fixture",
      receiptId: "018f47a0-7b80-7000-8000-000000000005",
      libraryId: "018f47a0-7b80-7000-8000-000000000006",
      deviceId: "018f47a0-7b80-7000-8000-000000000002",
      authorityGeneration: 7,
      purgeGeneration: 2,
      keyEpoch: keyEpoch,
      defaultScopeId: "018f47a0-7b80-7000-8000-000000000007",
      defaultScopeClass: "unknown",
      grantedScopes: ["note", "category", "folder"],
      capabilities: ["note": capability, "category": capability, "folder": capability],
      recordCipherSuite: "noted.record-aead.v1+aes256gcm+hkdfsha256",
      durableSyncSpkiSha256: [UInt8](repeating: 0x77, count: 32),
      transcriptDigest: [UInt8](repeating: 0x88, count: 32))
  }

  private func keyPackage(keyEpoch: UInt64 = 3) -> Data {
    var value = Data("NBK1".utf8)
    var version = UInt32(1).bigEndian
    withUnsafeBytes(of: &version) { value.append(contentsOf: $0) }
    var epoch = keyEpoch.bigEndian
    withUnsafeBytes(of: &epoch) { value.append(contentsOf: $0) }
    value.append(Data(repeating: 0x99, count: 32))
    return value
  }

  private func record() -> IdentityRecord {
    IdentityRecord(
      version: 1,
      handle: "018f47a0-7b80-7000-8000-000000000001",
      deviceId: "018f47a0-7b80-7000-8000-000000000002",
      lifecycle: .pending,
      signingKeyBacking: .softwareFixture,
      signingKeyRepresentation: Data(repeating: 0x11, count: 32),
      agreementPrivateKey: Data(repeating: 0x22, count: 32),
      signingPublicKey: Data([0x04] + [UInt8](repeating: 0x33, count: 64)),
      agreementPublicKey: Data(repeating: 0x44, count: 32),
      pendingBootstrap: nil,
      activeBootstrap: nil,
      createdAtMs: 1_725_000_000_000,
      activatedAtMs: nil)
  }

  func testStageAndActivateAreIdempotentForTheExactReceipt() throws {
    var value = record()
    let digest = Data(repeating: 0x55, count: 32)
    let first = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: digest,
      material: keyPackage(),
      metadata: metadata())
    let replay = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000004",
      receiptId: "receipt-1",
      envelopeDigest: digest,
      material: keyPackage(),
      metadata: metadata())
    XCTAssertEqual(first, replay)

    try IdentityLifecycleMachine.activate(
      record: &value,
      bootstrapHandle: first.handle,
      receiptId: first.receiptId,
      activatedAtMs: 1_725_000_000_100)
    try IdentityLifecycleMachine.activate(
      record: &value,
      bootstrapHandle: first.handle,
      receiptId: first.receiptId,
      activatedAtMs: 1_725_000_000_200)
    XCTAssertEqual(value.lifecycle, .active)
    XCTAssertNil(value.pendingBootstrap)
    XCTAssertEqual(value.activeBootstrap, first)
    XCTAssertEqual(value.activatedAtMs, 1_725_000_000_100)
    let recovery = value.publicDescriptor().bootstrapRecovery
    XCTAssertEqual(recovery?.pendingBootstrapHandle, first.handle)
    XCTAssertEqual(recovery?.receiptId, first.receiptId)
    XCTAssertEqual(recovery?.envelopeDigestBase64, digest.base64EncodedString())
    XCTAssertEqual(recovery?.metadata, metadata())
  }

  func testPublicRecoveryDescriptorNeverContainsBootstrapPlaintext() throws {
    var value = record()
    let material = keyPackage()
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "018f47a0-7b80-7000-8000-000000000005",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: material,
      metadata: metadata())

    let encoded = try JSONEncoder().encode(value.publicDescriptor())
    XCTAssertFalse(encoded.contains(material))
    XCTAssertFalse(encoded.contains(material))
    XCTAssertEqual(value.publicDescriptor().bootstrapRecovery?.pendingBootstrapHandle, staged.handle)
    XCTAssertEqual(value.publicDescriptor().bootstrapRecovery?.metadata, metadata())
  }

  func testByteDifferentReplayIsRejected() throws {
    var value = record()
    _ = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(),
      metadata: metadata())
    XCTAssertThrowsError(
      try IdentityLifecycleMachine.stage(
        record: &value,
        bootstrapHandle: "018f47a0-7b80-7000-8000-000000000004",
        receiptId: "receipt-1",
        envelopeDigest: Data(repeating: 0x56, count: 32),
        material: keyPackage(),
        metadata: metadata())
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .bootstrapReplayMismatch)
    }
  }

  func testBootstrapMetadataForAnotherDeviceIsRejectedBeforeMutation() throws {
    var value = record()
    let expected = value
    let capability = BootstrapCapabilityV1(readerVersion: 1, writerVersion: 1)
    let wrongDevice = BootstrapMetadataV1(
      version: 1,
      protocolName: "noted.direct-pairing.v1",
      suite: "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm",
      syncProtocolVersion: 1,
      environment: "development",
      libraryDataClass: "sanitized_fixture",
      receiptId: "018f47a0-7b80-7000-8000-000000000005",
      libraryId: "018f47a0-7b80-7000-8000-000000000006",
      deviceId: "018f47a0-7b80-7000-8000-000000000099",
      authorityGeneration: 7,
      purgeGeneration: 2,
      keyEpoch: 3,
      defaultScopeId: "018f47a0-7b80-7000-8000-000000000007",
      defaultScopeClass: "unknown",
      grantedScopes: ["note", "category", "folder"],
      capabilities: ["note": capability, "category": capability, "folder": capability],
      recordCipherSuite: "noted.record-aead.v1+aes256gcm+hkdfsha256",
      durableSyncSpkiSha256: [UInt8](repeating: 0x77, count: 32),
      transcriptDigest: [UInt8](repeating: 0x88, count: 32))

    XCTAssertThrowsError(
      try IdentityLifecycleMachine.stage(
        record: &value,
        bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
        receiptId: wrongDevice.receiptId,
        envelopeDigest: Data(repeating: 0x55, count: 32),
        material: keyPackage(),
        metadata: wrongDevice))
    XCTAssertEqual(value, expected)
  }

  func testDiscardIsOneWayAndWipesEverySecretField() throws {
    var value = record()
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(),
      metadata: metadata())
    try IdentityLifecycleMachine.discardPending(
      record: &value,
      bootstrapHandle: staged.handle,
      receiptId: staged.receiptId)
    try IdentityLifecycleMachine.discardPending(
      record: &value,
      bootstrapHandle: nil,
      receiptId: nil)
    XCTAssertEqual(value.lifecycle, .discarded)
    XCTAssertNil(value.signingKeyRepresentation)
    XCTAssertNil(value.agreementPrivateKey)
    XCTAssertNil(value.pendingBootstrap)
    XCTAssertNil(value.activeBootstrap)
  }

  func testActiveCannotBeDiscardedAsPending() throws {
    var value = record()
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(),
      metadata: metadata())
    try IdentityLifecycleMachine.activate(
      record: &value,
      bootstrapHandle: staged.handle,
      receiptId: staged.receiptId,
      activatedAtMs: 2)
    XCTAssertThrowsError(
      try IdentityLifecycleMachine.discardPending(
        record: &value,
        bootstrapHandle: staged.handle,
        receiptId: staged.receiptId))
  }

  func testLegacyBootstrapDecodesButRequiresExplicitDiscard() throws {
    var value = record()
    _ = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: metadata().receiptId,
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(),
      metadata: metadata())
    let encoded = try JSONEncoder().encode(value)
    var object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    var pending = try XCTUnwrap(object["pendingBootstrap"] as? [String: Any])
    pending.removeValue(forKey: "metadata")
    object["pendingBootstrap"] = pending
    let legacy = try JSONDecoder().decode(
      IdentityRecord.self,
      from: JSONSerialization.data(withJSONObject: object))
    XCTAssertNil(legacy.pendingBootstrap?.metadata)
    XCTAssertNil(legacy.publicDescriptor().bootstrapRecovery)
    XCTAssertNoThrow(
      try IdentityBootstrapValidator.validate(
        legacy, allowLegacyPendingBootstrap: true))
    XCTAssertThrowsError(try IdentityBootstrapValidator.validate(legacy)) { error in
      XCTAssertEqual(error as? NotedSecurityError, .legacyBootstrapRequiresDiscard)
    }

    var stagedLegacy = legacy
    XCTAssertThrowsError(
      try IdentityLifecycleMachine.stage(
        record: &stagedLegacy,
        bootstrapHandle: "018f47a0-7b80-7000-8000-000000000004",
        receiptId: metadata().receiptId,
        envelopeDigest: Data(repeating: 0x55, count: 32),
        material: keyPackage(),
        metadata: metadata())
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .legacyBootstrapRequiresDiscard)
    }
    try IdentityLifecycleMachine.discardPending(
      record: &stagedLegacy,
      bootstrapHandle: stagedLegacy.pendingBootstrap?.handle,
      receiptId: stagedLegacy.pendingBootstrap?.receiptId)
    XCTAssertEqual(stagedLegacy.lifecycle, .discarded)
  }

  func testLegacyActiveBootstrapIsNeverInventoryReadable() throws {
    var value = record()
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: metadata().receiptId,
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(),
      metadata: metadata())
    let encoded = try JSONEncoder().encode(value)
    var object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    var pending = try XCTUnwrap(object["pendingBootstrap"] as? [String: Any])
    pending.removeValue(forKey: "metadata")
    object["pendingBootstrap"] = NSNull()
    object["activeBootstrap"] = pending
    object["lifecycle"] = IdentityLifecycle.active.rawValue
    object["activatedAtMs"] = 1_725_000_000_100
    let legacyActive = try JSONDecoder().decode(
      IdentityRecord.self,
      from: JSONSerialization.data(withJSONObject: object))

    XCTAssertEqual(legacyActive.activeBootstrap?.handle, staged.handle)
    XCTAssertThrowsError(
      try IdentityBootstrapValidator.validate(
        legacyActive, allowLegacyPendingBootstrap: true)
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .legacyBootstrapRequiresDiscard)
    }
  }

  func testInventoryExceptionDoesNotWeakenAuthenticatedBootstrapValidation() throws {
    var value = record()
    value.pendingBootstrap = StagedBootstrap(
      handle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: metadata().receiptId,
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: keyPackage(keyEpoch: 3),
      metadata: metadata(keyEpoch: 4))

    XCTAssertThrowsError(
      try IdentityBootstrapValidator.validate(
        value, allowLegacyPendingBootstrap: true))
  }
}
