import CryptoKit
import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class AuthenticatedHpkeTests: XCTestCase {
  func testNativeP256VerificationAcceptsOnlyTheSignedMessage() throws {
    let key = P256.Signing.PrivateKey()
    let message = Data("sanitized pairing transcript".utf8)
    let signature = try key.signature(for: message).rawRepresentation

    XCTAssertTrue(
      try AppleCrypto.verifyP256Signature(
        publicKey: key.publicKey.x963Representation,
        message: message,
        signature: signature))
    XCTAssertFalse(
      try AppleCrypto.verifyP256Signature(
        publicKey: key.publicKey.x963Representation,
        message: Data("different".utf8),
        signature: signature))
  }

  func testNativeEntropyAndUuidV7HaveBoundedCanonicalShapes() throws {
    XCTAssertEqual(try AppleCrypto.secureRandomBytes(count: 32).count, 32)
    XCTAssertThrowsError(try AppleCrypto.secureRandomBytes(count: 0))
    let uuid = try AppleCrypto.freshUUIDv7(unixMilliseconds: 1_725_000_000_000)
    try UUIDv7Generator.validate(uuid)
  }

  @available(macOS 14.0, iOS 17.0, *)
  func testAuthenticatedX25519Aes256GcmRoundTrip() throws {
    let recipientPrivate = try Curve25519.KeyAgreement.PrivateKey(
      rawRepresentation: Data([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
        0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
        0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
      ]))
    let senderAuthentication = try Curve25519.KeyAgreement.PrivateKey(
      rawRepresentation: Data([
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37,
        0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
        0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
      ]))
    let info = Data("noted.fixture/info".utf8)
    let aad = Data("noted.fixture/aad".utf8)
    let plaintext = Data("sanitized-bootstrap".utf8)
    let exporterContext = Data("noted.fixture/exporter".utf8)
    var sender = try HPKE.Sender(
      recipientKey: recipientPrivate.publicKey,
      ciphersuite: AppleCrypto.hpkeSuite,
      info: info,
      authenticatedBy: senderAuthentication)
    let ciphertext = try sender.seal(plaintext, authenticating: aad)
    let senderExporter = try sender.exportSecret(context: exporterContext, outputByteCount: 32)
      .withUnsafeBytes { Data($0) }

    let record = IdentityRecord(
      version: 1,
      handle: "018f47a0-7b80-7000-8000-000000000001",
      deviceId: "018f47a0-7b80-7000-8000-000000000002",
      lifecycle: .pending,
      signingKeyBacking: .softwareFixture,
      signingKeyRepresentation: Data(repeating: 0, count: 32),
      agreementPrivateKey: recipientPrivate.rawRepresentation,
      signingPublicKey: Data([0x04] + [UInt8](repeating: 0, count: 64)),
      agreementPublicKey: recipientPrivate.publicKey.rawRepresentation,
      pendingBootstrap: nil,
      activeBootstrap: nil,
      createdAtMs: 1,
      activatedAtMs: nil)
    let opened = try AppleCrypto.openAuthenticatedHpke(
      record: record,
      senderPublicKey: senderAuthentication.publicKey.rawRepresentation,
      info: info,
      associatedData: aad,
      encapsulatedKey: sender.encapsulatedKey,
      ciphertext: ciphertext,
      exporterContext: exporterContext)
    XCTAssertEqual(opened.plaintext, plaintext)
    XCTAssertEqual(opened.exporterSecret, senderExporter)
  }
}
