import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class IdentityLifecycleTests: XCTestCase {
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
      material: Data("sanitized".utf8))
    let replay = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000004",
      receiptId: "receipt-1",
      envelopeDigest: digest,
      material: Data("sanitized".utf8))
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
  }

  func testPublicRecoveryDescriptorNeverContainsBootstrapPlaintext() throws {
    var value = record()
    let material = Data("never-cross-the-boundary".utf8)
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "018f47a0-7b80-7000-8000-000000000005",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: material)

    let encoded = try JSONEncoder().encode(value.publicDescriptor())
    XCTAssertFalse(encoded.contains(material))
    XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains("never-cross-the-boundary"))
    XCTAssertEqual(value.publicDescriptor().bootstrapRecovery?.pendingBootstrapHandle, staged.handle)
  }

  func testByteDifferentReplayIsRejected() throws {
    var value = record()
    _ = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: Data("sanitized".utf8))
    XCTAssertThrowsError(
      try IdentityLifecycleMachine.stage(
        record: &value,
        bootstrapHandle: "018f47a0-7b80-7000-8000-000000000004",
        receiptId: "receipt-1",
        envelopeDigest: Data(repeating: 0x56, count: 32),
        material: Data("sanitized".utf8))
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .bootstrapReplayMismatch)
    }
  }

  func testDiscardIsOneWayAndWipesEverySecretField() throws {
    var value = record()
    let staged = try IdentityLifecycleMachine.stage(
      record: &value,
      bootstrapHandle: "018f47a0-7b80-7000-8000-000000000003",
      receiptId: "receipt-1",
      envelopeDigest: Data(repeating: 0x55, count: 32),
      material: Data("sanitized".utf8))
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
      material: Data("sanitized".utf8))
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
}
