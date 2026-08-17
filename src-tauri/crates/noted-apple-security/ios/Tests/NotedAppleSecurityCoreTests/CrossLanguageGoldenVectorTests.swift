import CryptoKit
import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class CrossLanguageGoldenVectorTests: XCTestCase {
  @available(macOS 14.0, iOS 17.0, *)
  func testCanonicalTranscriptReceiptSignatureAndSAS() throws {
    let root = try loadFixture()
    XCTAssertEqual(try string(root, "fixture_class"), "sanitized_cross_language_golden_vectors")
    XCTAssertEqual(try string(root, "protocol"), "noted.direct-pairing.v1")
    XCTAssertEqual(
      try string(root, "suite"),
      "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm")

    let crossLanguage = try dictionary(root, "cross_language")
    XCTAssertEqual(try integer(crossLanguage, "artifact_version"), 1)
    let canonical = try dictionary(crossLanguage, "canonical")
    let proposal = try canonicalReceipt(root: root, transcriptDigest: Data())
    let transcript = canonicalComponents(
      domain: "noted.direct-pairing.v1/transcript",
      fields: [
        ("invitation_digest", try hex(canonical, "invitation_digest_hex")),
        ("client_hello_digest", try hex(canonical, "client_hello_digest_hex")),
        ("server_nonce", try hex(canonical, "server_nonce_hex")),
        ("receipt_proposal", proposal),
      ])
    let expectedTranscript = try base64(canonical, "transcript_canonical_base64")
    XCTAssertEqual(transcript, expectedTranscript)

    let transcriptDigest = Data(SHA256.hash(data: transcript))
    XCTAssertEqual(transcriptDigest, try hex(canonical, "transcript_digest_hex"))

    let receipt = try canonicalReceipt(root: root, transcriptDigest: transcriptDigest)
    XCTAssertEqual(receipt, try base64(canonical, "receipt_canonical_base64"))
    XCTAssertEqual(
      Data(SHA256.hash(data: receipt)),
      try hex(canonical, "receipt_digest_hex"))

    let signatureVector = try dictionary(crossLanguage, "signature")
    XCTAssertEqual(
      try string(signatureVector, "algorithm"),
      "ecdsa-p256-sha256-p1363")
    XCTAssertEqual(try string(signatureVector, "message"), "canonical_transcript")
    XCTAssertNil(signatureVector["private_key_hex"])
    let publicKey = try P256.Signing.PublicKey(
      x963Representation: try hex(signatureVector, "public_key_x963_hex"))
    let signature = try P256.Signing.ECDSASignature(
      rawRepresentation: try hex(signatureVector, "signature_p1363_hex"))
    XCTAssertTrue(publicKey.isValidSignature(signature, for: transcript))
    var tamperedTranscript = transcript
    tamperedTranscript[0] ^= 1
    XCTAssertFalse(publicKey.isValidSignature(signature, for: tamperedTranscript))

    let notedHPKE = try dictionary(crossLanguage, "noted_hpke")
    let challenge = try dictionary(notedHPKE, "challenge")
    let sas = try dictionary(crossLanguage, "sas")
    XCTAssertEqual(
      deriveVerificationCode(
        exporterSecret: try hex(challenge, "exported_value_hex"),
        transcriptDigest: transcriptDigest),
      try string(sas, "verification_code"))
  }

  @available(macOS 14.0, iOS 17.0, *)
  func testRFC9180AuthenticatedHpkeOpenAndExporter() throws {
    let root = try loadFixture()
    let crossLanguage = try dictionary(root, "cross_language")
    let hpke = try dictionary(crossLanguage, "hpke")
    XCTAssertEqual(try integer(hpke, "mode"), 2)
    XCTAssertEqual(try integer(hpke, "kem_id"), 32)
    XCTAssertEqual(try integer(hpke, "kdf_id"), 1)
    XCTAssertEqual(try integer(hpke, "aead_id"), 2)

    let recipientPrivate = try Curve25519.KeyAgreement.PrivateKey(
      rawRepresentation: try hex(hpke, "recipient_private_key_hex"))
    XCTAssertEqual(
      recipientPrivate.publicKey.rawRepresentation,
      try hex(hpke, "recipient_public_key_hex"))
    let senderAuthentication = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: try hex(hpke, "sender_auth_public_key_hex"))
    var recipient = try HPKE.Recipient(
      privateKey: recipientPrivate,
      ciphersuite: AppleCrypto.hpkeSuite,
      info: try hex(hpke, "info_hex"),
      encapsulatedKey: try hex(hpke, "encapsulated_key_hex"),
      authenticatedBy: senderAuthentication)
    let plaintext = try recipient.open(
      try hex(hpke, "ciphertext_hex"),
      authenticating: try hex(hpke, "aad_hex"))
    XCTAssertEqual(plaintext, try hex(hpke, "plaintext_hex"))
    let exporter = try recipient.exportSecret(
      context: try hex(hpke, "exporter_context_hex"),
      outputByteCount: 32
    ).withUnsafeBytes { Data($0) }
    XCTAssertEqual(exporter, try hex(hpke, "exported_value_hex"))

    var tamperedCiphertext = try hex(hpke, "ciphertext_hex")
    tamperedCiphertext[0] ^= 1
    var tamperedRecipient = try HPKE.Recipient(
      privateKey: recipientPrivate,
      ciphersuite: AppleCrypto.hpkeSuite,
      info: try hex(hpke, "info_hex"),
      encapsulatedKey: try hex(hpke, "encapsulated_key_hex"),
      authenticatedBy: senderAuthentication)
    XCTAssertThrowsError(
      try tamperedRecipient.open(
        tamperedCiphertext,
        authenticating: try hex(hpke, "aad_hex")))
  }

  @available(macOS 14.0, iOS 17.0, *)
  func testNotedChallengeAndBootstrapHpkeApplicationBindings() throws {
    let root = try loadFixture()
    let crossLanguage = try dictionary(root, "cross_language")
    let canonical = try dictionary(crossLanguage, "canonical")
    let receiptFields = try dictionary(canonical, "receipt")
    let notedHPKE = try dictionary(crossLanguage, "noted_hpke")
    let challenge = try dictionary(notedHPKE, "challenge")
    let bootstrap = try dictionary(notedHPKE, "bootstrap")
    let transcriptDigest = try hex(canonical, "transcript_digest_hex")
    let receipt = try canonicalReceipt(root: root, transcriptDigest: transcriptDigest)

    XCTAssertEqual(try string(challenge, "info_source"), "challenge_hpke_info(receipt)")
    XCTAssertEqual(
      try string(challenge, "associated_data_source"),
      "canonical.transcript_digest_hex")
    XCTAssertEqual(
      try string(challenge, "plaintext_source"),
      "canonical_challenge_plaintext(receipt)")
    XCTAssertEqual(
      try string(challenge, "exporter_context_source"),
      "challenge_hpke_exporter_context(receipt)")

    let challengePlaintext = canonicalComponents(
      domain: "noted.direct-pairing.v1/challenge",
      fields: [
        ("receipt_id", Data(try string(receiptFields, "receipt_id").utf8)),
        ("transcript_digest", transcriptDigest),
        ("library_id", Data(try string(receiptFields, "library_id").utf8)),
        ("device_id", Data(try string(receiptFields, "device_id").utf8)),
      ])
    let challengeResult = try openNotedAuthenticatedHPKE(
      root: root,
      keyMaterial: notedHPKE,
      vector: challenge,
      info: try pairingHPKEContext(
        domain: "noted.direct-pairing.v1/hpke/challenge/info",
        root: root,
        transcriptDigest: transcriptDigest),
      associatedData: transcriptDigest,
      exporterContext: try pairingHPKEContext(
        domain: "noted.direct-pairing.v1/hpke/challenge/sas-exporter",
        root: root,
        transcriptDigest: transcriptDigest))
    XCTAssertEqual(challengeResult.plaintext, challengePlaintext)
    XCTAssertEqual(challengeResult.exporter, try hex(challenge, "exported_value_hex"))
    XCTAssertEqual(
      try authenticatedHPKEEnvelopeDigest(vector: challenge),
      try hex(challenge, "envelope_digest_hex"))

    XCTAssertEqual(try string(bootstrap, "info_source"), "bootstrap_hpke_info(receipt)")
    XCTAssertEqual(
      try string(bootstrap, "associated_data_source"),
      "canonical_receipt(receipt)")
    XCTAssertEqual(
      try string(bootstrap, "exporter_context_source"),
      "bootstrap_hpke_exporter_context(receipt)")
    let bootstrapResult = try openNotedAuthenticatedHPKE(
      root: root,
      keyMaterial: notedHPKE,
      vector: bootstrap,
      info: try pairingHPKEContext(
        domain: "noted.direct-pairing.v1/hpke/bootstrap/info",
        root: root,
        transcriptDigest: transcriptDigest),
      associatedData: receipt,
      exporterContext: try pairingHPKEContext(
        domain: "noted.direct-pairing.v1/hpke/bootstrap/exporter",
        root: root,
        transcriptDigest: transcriptDigest))
    XCTAssertEqual(bootstrapResult.plaintext, try base64(bootstrap, "plaintext_base64"))
    XCTAssertEqual(bootstrapResult.exporter, try hex(bootstrap, "exported_value_hex"))
    XCTAssertEqual(
      try authenticatedHPKEEnvelopeDigest(vector: bootstrap),
      try hex(bootstrap, "envelope_digest_hex"))
  }

  private func loadFixture() throws -> [String: Any] {
    var sourceRoot = URL(fileURLWithPath: #filePath)
    for _ in 0..<6 {
      sourceRoot.deleteLastPathComponent()
    }
    let fixtureURL =
      sourceRoot
      .appendingPathComponent("tests", isDirectory: true)
      .appendingPathComponent("fixtures", isDirectory: true)
      .appendingPathComponent("pairing_v1_canonical.json", isDirectory: false)
    let object = try JSONSerialization.jsonObject(with: Data(contentsOf: fixtureURL))
    guard let root = object as? [String: Any] else {
      throw VectorError.invalid("fixture root")
    }
    return root
  }

  private func canonicalReceipt(
    root: [String: Any],
    transcriptDigest: Data
  ) throws -> Data {
    let crossLanguage = try dictionary(root, "cross_language")
    let canonical = try dictionary(crossLanguage, "canonical")
    let receipt = try dictionary(canonical, "receipt")
    var builder = CanonicalBuilder(domain: "noted.direct-pairing.v1/receipt")
    builder.text("protocol", try string(root, "protocol"))
    builder.text("suite", try string(root, "suite"))
    builder.text("receipt_id", try string(receipt, "receipt_id"))
    builder.text("invitation_id", try string(receipt, "invitation_id"))
    builder.text("library_id", try string(receipt, "library_id"))
    builder.text("device_id", try string(receipt, "device_id"))
    builder.bytes(
      "client_signing_key_fingerprint",
      try hex(receipt, "client_signing_key_fingerprint_hex"))
    builder.bytes(
      "client_hpke_key_fingerprint",
      try hex(receipt, "client_hpke_key_fingerprint_hex"))
    builder.bytes(
      "mac_signing_key_fingerprint",
      try hex(receipt, "mac_signing_key_fingerprint_hex"))
    builder.bytes(
      "mac_hpke_key_fingerprint",
      try hex(receipt, "mac_hpke_key_fingerprint_hex"))

    guard let scopeValues = receipt["granted_scopes"] as? [String] else {
      throw VectorError.invalid("granted_scopes")
    }
    var scopes = CanonicalBuilder(domain: "noted.direct-pairing.v1/record-kind-list")
    scopes.unsigned64("count", UInt64(scopeValues.count))
    for scope in scopeValues {
      scopes.text("kind", scope)
    }
    builder.bytes("granted_scopes", scopes.finish())

    guard let capabilityValues = receipt["capabilities"] as? [[String: Any]] else {
      throw VectorError.invalid("capabilities")
    }
    var capabilities = CanonicalBuilder(domain: "noted.direct-pairing.v1/capabilities")
    capabilities.unsigned64("count", UInt64(capabilityValues.count))
    for capability in capabilityValues {
      capabilities.text("kind", try string(capability, "kind"))
      capabilities.unsigned64(
        "reader_version",
        UInt64(try integer(capability, "reader_version")))
      if let writer = capability["writer_version"] as? NSNumber {
        capabilities.text("writer_present", "true")
        capabilities.unsigned64("writer_version", writer.uint64Value)
      } else {
        capabilities.text("writer_present", "false")
      }
    }
    builder.bytes("capabilities", capabilities.finish())
    builder.unsigned64(
      "authority_generation",
      UInt64(try integer(receipt, "authority_generation")))
    builder.signed64("created_at_ms", Int64(try integer(receipt, "created_at_ms")))
    builder.signed64("expires_at_ms", Int64(try integer(receipt, "expires_at_ms")))
    builder.bytes("transcript_digest", transcriptDigest)
    builder.text("environment", try string(receipt, "environment"))
    builder.text("mac_role", try string(receipt, "mac_role"))
    builder.text("client_role", try string(receipt, "client_role"))
    return builder.finish()
  }

  private func pairingHPKEContext(
    domain: String,
    root: [String: Any],
    transcriptDigest: Data
  ) throws -> Data {
    let crossLanguage = try dictionary(root, "cross_language")
    let canonical = try dictionary(crossLanguage, "canonical")
    let receipt = try dictionary(canonical, "receipt")
    return canonicalComponents(
      domain: domain,
      fields: [
        ("protocol", Data(try string(root, "protocol").utf8)),
        ("suite", Data(try string(root, "suite").utf8)),
        ("receipt_id", Data(try string(receipt, "receipt_id").utf8)),
        ("library_id", Data(try string(receipt, "library_id").utf8)),
        ("device_id", Data(try string(receipt, "device_id").utf8)),
        ("transcript_digest", transcriptDigest),
      ])
  }

  @available(macOS 14.0, iOS 17.0, *)
  private func openNotedAuthenticatedHPKE(
    root: [String: Any],
    keyMaterial: [String: Any],
    vector: [String: Any],
    info: Data,
    associatedData: Data,
    exporterContext: Data
  ) throws -> (plaintext: Data, exporter: Data) {
    XCTAssertEqual(try string(root, "protocol"), "noted.direct-pairing.v1")
    let recipientPrivate = try Curve25519.KeyAgreement.PrivateKey(
      rawRepresentation: try hex(keyMaterial, "test_only_recipient_private_key_hex"))
    XCTAssertEqual(
      recipientPrivate.publicKey.rawRepresentation,
      try hex(keyMaterial, "recipient_public_key_hex"))
    let senderAuthentication = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: try hex(keyMaterial, "sender_auth_public_key_hex"))
    var recipient = try HPKE.Recipient(
      privateKey: recipientPrivate,
      ciphersuite: AppleCrypto.hpkeSuite,
      info: info,
      encapsulatedKey: try hex(vector, "encapsulated_key_hex"),
      authenticatedBy: senderAuthentication)
    let plaintext = try recipient.open(
      try hex(vector, "ciphertext_hex"),
      authenticating: associatedData)
    let exporter = try recipient.exportSecret(
      context: exporterContext,
      outputByteCount: 32
    ).withUnsafeBytes { Data($0) }
    return (plaintext, exporter)
  }

  private func authenticatedHPKEEnvelopeDigest(vector: [String: Any]) throws -> Data {
    let envelope = canonicalComponents(
      domain: "noted.direct-pairing.v1/authenticated-hpke-envelope",
      fields: [
        ("encapsulated_key", try hex(vector, "encapsulated_key_hex")),
        ("ciphertext", try hex(vector, "ciphertext_hex")),
      ])
    return Data(SHA256.hash(data: envelope))
  }

  private func deriveVerificationCode(
    exporterSecret: Data,
    transcriptDigest: Data
  ) -> String {
    let modulus: UInt64 = 100_000_000
    let acceptBelow = UInt64.max - (UInt64.max % modulus)
    for attempt in UInt32.min...UInt32.max {
      var attemptBigEndian = attempt.bigEndian
      let attemptBytes = withUnsafeBytes(of: &attemptBigEndian) { Data($0) }
      let info = canonicalComponents(
        domain: "noted.direct-pairing.v1/sas-hkdf-info",
        fields: [("attempt", attemptBytes)])
      let key = HKDF<SHA256>.deriveKey(
        inputKeyMaterial: SymmetricKey(data: exporterSecret),
        salt: transcriptDigest,
        info: info,
        outputByteCount: 8)
      let candidate = key.withUnsafeBytes { bytes in
        bytes.reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
      }
      if candidate < acceptBelow {
        let digits = String(format: "%08llu", candidate % modulus)
        let split = digits.index(digits.startIndex, offsetBy: 4)
        return "\(digits[..<split]) \(digits[split...])"
      }
    }
    fatalError("64-bit rejection sampling exhausted every HKDF counter")
  }

  private func canonicalComponents(
    domain: String,
    fields: [(String, Data)]
  ) -> Data {
    var builder = CanonicalBuilder(domain: domain)
    for (label, value) in fields {
      builder.bytes(label, value)
    }
    return builder.finish()
  }

  private func dictionary(_ parent: [String: Any], _ key: String) throws -> [String: Any] {
    guard let value = parent[key] as? [String: Any] else {
      throw VectorError.invalid(key)
    }
    return value
  }

  private func string(_ parent: [String: Any], _ key: String) throws -> String {
    guard let value = parent[key] as? String else {
      throw VectorError.invalid(key)
    }
    return value
  }

  private func integer(_ parent: [String: Any], _ key: String) throws -> Int {
    guard let value = parent[key] as? NSNumber else {
      throw VectorError.invalid(key)
    }
    return value.intValue
  }

  private func hex(_ parent: [String: Any], _ key: String) throws -> Data {
    let value = try string(parent, key)
    guard value.count.isMultiple(of: 2) else {
      throw VectorError.invalid(key)
    }
    var bytes = Data(capacity: value.count / 2)
    var index = value.startIndex
    while index < value.endIndex {
      let end = value.index(index, offsetBy: 2)
      guard let byte = UInt8(value[index..<end], radix: 16) else {
        throw VectorError.invalid(key)
      }
      bytes.append(byte)
      index = end
    }
    return bytes
  }

  private func base64(_ parent: [String: Any], _ key: String) throws -> Data {
    guard let value = Data(base64Encoded: try string(parent, key)) else {
      throw VectorError.invalid(key)
    }
    return value
  }
}

private enum VectorError: Error {
  case invalid(String)
}

private struct CanonicalBuilder {
  private var data = Data()

  init(domain: String) {
    bytes("domain", Data(domain.utf8))
  }

  mutating func bytes(_ label: String, _ value: Data) {
    var labelLength = UInt32(label.utf8.count).bigEndian
    withUnsafeBytes(of: &labelLength) { data.append(contentsOf: $0) }
    data.append(contentsOf: label.utf8)
    var valueLength = UInt64(value.count).bigEndian
    withUnsafeBytes(of: &valueLength) { data.append(contentsOf: $0) }
    data.append(value)
  }

  mutating func text(_ label: String, _ value: String) {
    bytes(label, Data(value.utf8))
  }

  mutating func unsigned64(_ label: String, _ value: UInt64) {
    var value = value.bigEndian
    bytes(label, withUnsafeBytes(of: &value) { Data($0) })
  }

  mutating func signed64(_ label: String, _ value: Int64) {
    var value = value.bigEndian
    bytes(label, withUnsafeBytes(of: &value) { Data($0) })
  }

  func finish() -> Data {
    data
  }
}
