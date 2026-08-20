import CryptoKit
import Foundation
import XCTest

@testable import NotedAppleSecurityCore

@available(iOS 17.0, macOS 14.0, *)
final class RecordCryptoTests: XCTestCase {
  func testSwiftMatchesSharedVectorAndOpensItsP1363Signature() throws {
    let vector = try loadVector()
    let context = try vectorContext(vector)
    let libraryKey = try hex(vector, "libraryKeyHex")
    let nonce = try hex(vector, "nonceHex")
    let plaintext = try base64(vector, "plaintextBase64")
    let signingKey = try fixtureSigningKey()
    XCTAssertEqual(
      signingKey.publicKey.x963Representation,
      try hex(vector, "signerPublicKeyX963Hex"))
    let record = activeRecord(
      context: context,
      libraryKey: libraryKey,
      signingPublicKey: signingKey.publicKey.x963Representation)

    XCTAssertEqual(
      Data(SHA256.hash(data: try RecordCryptoContractV1.canonicalContext(context))),
      try hex(vector, "canonicalContextSha256Hex"))
    XCTAssertEqual(RecordCryptoContractV1.hkdfSalt(), try hex(vector, "hkdfSaltHex"))
    XCTAssertEqual(
      Data(SHA256.hash(data: try RecordCryptoContractV1.hkdfInfo(context))),
      try hex(vector, "hkdfInfoSha256Hex"))
    XCTAssertEqual(
      Data(SHA256.hash(data: try RecordCryptoContractV1.associatedData(context))),
      try hex(vector, "aadSha256Hex"))

    let derived = HKDF<SHA256>.deriveKey(
      inputKeyMaterial: SymmetricKey(data: libraryKey),
      salt: RecordCryptoContractV1.hkdfSalt(),
      info: try RecordCryptoContractV1.hkdfInfo(context),
      outputByteCount: 32
    ).withUnsafeBytes { Data($0) }
    XCTAssertEqual(derived, try hex(vector, "derivedKeyHex"))

    let deterministic = try RecordCryptoContractV1.seal(
      record: record,
      context: context,
      plaintext: plaintext,
      nonceProvider: { nonce },
      signer: { try signingKey.signature(for: $0).rawRepresentation })
    XCTAssertEqual(
      try XCTUnwrap(Data(base64Encoded: deterministic.ciphertextBase64)),
      try base64(vector, "ciphertextBase64"))
    XCTAssertEqual(
      try XCTUnwrap(Data(base64Encoded: deterministic.contextDigestBase64)),
      try hex(vector, "canonicalContextSha256Hex"))
    XCTAssertEqual(
      try XCTUnwrap(Data(base64Encoded: deterministic.envelopeDigestBase64)),
      try hex(vector, "envelopeDigestHex"))

    let vectorSealed = RecordCiphertextDescriptorV1(
      version: 1,
      cipherSuite: BootstrapContractV1.recordCipherSuite,
      nonceBase64: nonce.base64EncodedString(),
      ciphertextBase64: try string(vector, "ciphertextBase64"),
      contextDigestBase64: try hex(vector, "canonicalContextSha256Hex").base64EncodedString(),
      envelopeDigestBase64: try hex(vector, "envelopeDigestHex").base64EncodedString(),
      recordSignatureBase64: try hex(vector, "recordSignatureP1363Hex").base64EncodedString())
    XCTAssertEqual(
      try RecordCryptoContractV1.signatureMessage(
        envelopeDigest: try hex(vector, "envelopeDigestHex")),
      try hex(vector, "signatureMessageHex"))
    let opened = try RecordCryptoContractV1.open(
      record: record,
      context: context,
      sealed: vectorSealed,
      expectedSignerPublicKey: try hex(vector, "signerPublicKeyX963Hex"))
    XCTAssertEqual(try XCTUnwrap(Data(base64Encoded: opened.plaintextBase64)), plaintext)
  }

  func testRoundTripUsesFreshNoncesAndNeverReturnsTheLibraryKey() throws {
    let context = context()
    let signingKey = P256.Signing.PrivateKey()
    let libraryKey = Data((1...32).map(UInt8.init))
    let record = activeRecord(
      context: context,
      libraryKey: libraryKey,
      signingPublicKey: signingKey.publicKey.x963Representation)
    let plaintext = Data("meeting notes stay local".utf8)
    let sign: RecordCryptoContractV1.RecordSigner = {
      try signingKey.signature(for: $0).rawRepresentation
    }
    let first = try RecordCryptoContractV1.seal(
      record: record, context: context, plaintext: plaintext, signer: sign)
    let second = try RecordCryptoContractV1.seal(
      record: record, context: context, plaintext: plaintext, signer: sign)
    XCTAssertNotEqual(first.nonceBase64, second.nonceBase64)
    XCTAssertNotEqual(first.ciphertextBase64, second.ciphertextBase64)

    for sealed in [first, second] {
      let opened = try RecordCryptoContractV1.open(
        record: record,
        context: context,
        sealed: sealed,
        expectedSignerPublicKey: signingKey.publicKey.x963Representation)
      XCTAssertEqual(try XCTUnwrap(Data(base64Encoded: opened.plaintextBase64)), plaintext)
      let encoded = try JSONEncoder().encode(sealed)
      XCTAssertFalse(encoded.contains(libraryKey))
    }
  }

  func testEveryContextBindingRejectsRebinding() throws {
    let original = context()
    let signingKey = P256.Signing.PrivateKey()
    let record = activeRecord(
      context: original,
      libraryKey: Data(repeating: 0x91, count: 32),
      signingPublicKey: signingKey.publicKey.x963Representation)
    let sealed = try RecordCryptoContractV1.seal(
      record: record,
      context: original,
      plaintext: Data("bound".utf8),
      nonceProvider: { Data(repeating: 0x12, count: 12) },
      signer: { try signingKey.signature(for: $0).rawRepresentation })

    let mutations: [(String, Any)] = [
      ("version", 2),
      ("cipherSuite", "noted.record-aead.v2+aes256gcm+hkdfsha256"),
      ("libraryId", "018f47a0-7b80-7000-8000-000000000201"),
      ("recordId", "018f47a0-7b80-7000-8000-000000000202"),
      ("recordKind", "category"),
      ("schemaVersion", 2),
      ("baseRevision", 2),
      ("baseVersionId", "018f47a0-7b80-7000-8000-000000000203"),
      ("proposedRevision", 3),
      ("versionId", "018f47a0-7b80-7000-8000-000000000204"),
      ("mutationId", "018f47a0-7b80-7000-8000-000000000205"),
      ("authorityGeneration", 8),
      ("purgeGeneration", 3),
      ("keyEpoch", 4),
      ("operation", "delete"),
    ]
    for (field, value) in mutations {
      let rebound = try replacingContextField(original, field: field, value: value)
      XCTAssertThrowsError(
        try RecordCryptoContractV1.open(
          record: record,
          context: rebound,
          sealed: sealed,
          expectedSignerPublicKey: signingKey.publicKey.x963Representation),
        "field \(field) was not authenticated")
    }
  }

  func testCiphertextNonceDigestSignatureAndExpectedSignerTamperingFailClosed() throws {
    let context = context()
    let signer = P256.Signing.PrivateKey()
    let record = activeRecord(
      context: context,
      libraryKey: Data(repeating: 0x77, count: 32),
      signingPublicKey: signer.publicKey.x963Representation)
    let sealed = try RecordCryptoContractV1.seal(
      record: record,
      context: context,
      plaintext: Data("tamper target".utf8),
      nonceProvider: { Data(repeating: 0x42, count: 12) },
      signer: { try signer.signature(for: $0).rawRepresentation })
    let nonce = try XCTUnwrap(Data(base64Encoded: sealed.nonceBase64))
    let ciphertext = try XCTUnwrap(Data(base64Encoded: sealed.ciphertextBase64))
    let contextDigest = try XCTUnwrap(Data(base64Encoded: sealed.contextDigestBase64))
    let envelopeDigest = try XCTUnwrap(Data(base64Encoded: sealed.envelopeDigestBase64))
    let signature = try XCTUnwrap(Data(base64Encoded: sealed.recordSignatureBase64))

    for changed in [
      descriptor(sealed, nonce: toggled(nonce)),
      descriptor(sealed, ciphertext: toggled(ciphertext)),
      descriptor(sealed, contextDigest: toggled(contextDigest)),
      descriptor(sealed, envelopeDigest: toggled(envelopeDigest)),
      descriptor(sealed, recordSignature: toggled(signature)),
    ] {
      XCTAssertThrowsError(
        try RecordCryptoContractV1.open(
          record: record,
          context: context,
          sealed: changed,
          expectedSignerPublicKey: signer.publicKey.x963Representation))
    }
    XCTAssertThrowsError(
      try RecordCryptoContractV1.open(
        record: record,
        context: context,
        sealed: sealed,
        expectedSignerPublicKey: P256.Signing.PrivateKey().publicKey.x963Representation)
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .recordSignatureInvalid)
    }
  }

  func testMalformedUnknownAndOversizeInputsAreRejectedBeforeCrypto() throws {
    let context = context()
    let signer = P256.Signing.PrivateKey()
    let record = activeRecord(
      context: context,
      libraryKey: Data(repeating: 0x66, count: 32),
      signingPublicKey: signer.publicKey.x963Representation)
    let sealed = try RecordCryptoContractV1.seal(
      record: record,
      context: context,
      plaintext: Data(),
      nonceProvider: { Data(repeating: 0x31, count: 12) },
      signer: { try signer.signature(for: $0).rawRepresentation })

    let malformed = RecordCiphertextDescriptorV1(
      version: sealed.version,
      cipherSuite: sealed.cipherSuite,
      nonceBase64: "%%%",
      ciphertextBase64: sealed.ciphertextBase64,
      contextDigestBase64: sealed.contextDigestBase64,
      envelopeDigestBase64: sealed.envelopeDigestBase64,
      recordSignatureBase64: sealed.recordSignatureBase64)
    XCTAssertThrowsError(
      try RecordCryptoContractV1.open(
        record: record,
        context: context,
        sealed: malformed,
        expectedSignerPublicKey: signer.publicKey.x963Representation))

    XCTAssertThrowsError(
      try RecordCryptoContractV1.seal(
        record: record,
        context: context,
        plaintext: Data(repeating: 0, count: RecordCryptoContractV1.maximumPlaintextByteCount + 1),
        signer: { try signer.signature(for: $0).rawRepresentation }))

    let oversized = descriptor(
      sealed,
      ciphertext: Data(
        repeating: 0,
        count: RecordCryptoContractV1.maximumCiphertextByteCount + 1))
    XCTAssertThrowsError(
      try RecordCryptoContractV1.open(
        record: record,
        context: context,
        sealed: oversized,
        expectedSignerPublicKey: signer.publicKey.x963Representation))

    var contextObject = try XCTUnwrap(
      JSONSerialization.jsonObject(with: JSONEncoder().encode(context)) as? [String: Any])
    contextObject["unexpected"] = true
    XCTAssertThrowsError(
      try JSONDecoder().decode(
        RecordCryptoContextV1.self,
        from: JSONSerialization.data(withJSONObject: contextObject)))
  }

  func testOnlyExactActiveSanitizedBootstrapCanUseTheKey() throws {
    let context = context()
    let signingKey = P256.Signing.PrivateKey()
    let key = Data(repeating: 0x54, count: 32)
    let seal: (IdentityRecord) throws -> Void = { record in
      _ = try RecordCryptoContractV1.seal(
        record: record,
        context: context,
        plaintext: Data(),
        nonceProvider: { Data(repeating: 0, count: 12) },
        signer: { try signingKey.signature(for: $0).rawRepresentation })
    }

    XCTAssertThrowsError(try seal(pendingRecord(context: context, libraryKey: key)))
    XCTAssertThrowsError(try seal(discardedRecord(context: context)))
    XCTAssertThrowsError(try seal(legacyActiveRecord(context: context, libraryKey: key))) { error in
      XCTAssertEqual(error as? NotedSecurityError, .legacyBootstrapRequiresDiscard)
    }
    XCTAssertThrowsError(
      try seal(
        activeRecord(
          context: context,
          libraryKey: key,
          signingPublicKey: signingKey.publicKey.x963Representation,
          environment: "production")))
    XCTAssertThrowsError(
      try seal(
        activeRecord(
          context: context,
          libraryKey: key,
          signingPublicKey: signingKey.publicKey.x963Representation,
          libraryDataClass: "personal")))
    XCTAssertThrowsError(
      try seal(
        activeRecord(
          context: context,
          libraryKey: key,
          signingPublicKey: signingKey.publicKey.x963Representation,
          receiptOverride: "018f47a0-7b80-7000-8000-000000000299")))
    XCTAssertThrowsError(
      try seal(
        activeRecord(
          context: context,
          libraryKey: key,
          signingPublicKey: signingKey.publicKey.x963Representation,
          packageEpoch: 4)))
  }

  func testRestartThroughDataProtectedKeychainTestStoreAndLockedFailure() throws {
    let context = context()
    let signer = P256.Signing.PrivateKey()
    let record = activeRecord(
      context: context,
      libraryKey: Data(repeating: 0x43, count: 32),
      signingPublicKey: signer.publicKey.x963Representation)
    let store = KeychainRecordTestStore()
    try store.add(record)
    let firstVault = fixtureVault(store: store, signer: signer, nonce: 0x22)
    let sealed = try firstVault.sealRecord(
      identityHandle: record.handle,
      context: context,
      plaintext: Data("survives process restart".utf8))

    // A new vault instance reads a freshly decoded copy of the same protected
    // record, mirroring process restart without touching the developer's real Keychain.
    let restartedVault = fixtureVault(store: store, signer: signer, nonce: 0x23)
    let opened = try restartedVault.openRecord(
      identityHandle: record.handle,
      context: context,
      sealed: sealed,
      expectedSignerPublicKey: signer.publicKey.x963Representation)
    XCTAssertEqual(
      try XCTUnwrap(Data(base64Encoded: opened.plaintextBase64)),
      Data("survives process restart".utf8))

    store.isProtectedDataLocked = true
    XCTAssertThrowsError(
      try restartedVault.openRecord(
        identityHandle: record.handle,
        context: context,
        sealed: sealed,
        expectedSignerPublicKey: signer.publicKey.x963Representation)
    ) { error in
      XCTAssertEqual(error as? NotedSecurityError, .protectedDataUnavailable)
    }
  }

  private func fixtureVault(
    store: KeychainRecordTestStore,
    signer: P256.Signing.PrivateKey,
    nonce: UInt8
  ) -> IdentityVault {
    IdentityVault(
      store: store,
      now: { 1_725_000_000_000 },
      keyPairValidator: { _ in },
      recordNonceProvider: { Data(repeating: nonce, count: 12) },
      recordSigner: { _, message in
        try signer.signature(for: message).rawRepresentation
      })
  }

  private func context() -> RecordCryptoContextV1 {
    RecordCryptoContextV1(
      version: 1,
      cipherSuite: BootstrapContractV1.recordCipherSuite,
      libraryId: "018f47a0-7b80-7000-8000-000000000101",
      recordId: "018f47a0-7b80-7000-8000-000000000102",
      recordKind: .note,
      schemaVersion: 1,
      baseRevision: 1,
      baseVersionId: "018f47a0-7b80-7000-8000-000000000103",
      proposedRevision: 2,
      versionId: "018f47a0-7b80-7000-8000-000000000104",
      mutationId: "018f47a0-7b80-7000-8000-000000000105",
      authorityGeneration: 7,
      purgeGeneration: 2,
      keyEpoch: 3,
      operation: .update)
  }

  private func metadata(
    context: RecordCryptoContextV1,
    environment: String = "development",
    libraryDataClass: String = "sanitized_fixture"
  ) -> BootstrapMetadataV1 {
    let capability = BootstrapCapabilityV1(readerVersion: 1, writerVersion: 1)
    return BootstrapMetadataV1(
      version: 1,
      protocolName: BootstrapContractV1.pairingProtocol,
      suite: BootstrapContractV1.pairingSuite,
      syncProtocolVersion: 1,
      environment: environment,
      libraryDataClass: libraryDataClass,
      receiptId: "018f47a0-7b80-7000-8000-000000000106",
      libraryId: context.libraryId,
      deviceId: "018f47a0-7b80-7000-8000-000000000107",
      authorityGeneration: context.authorityGeneration,
      purgeGeneration: context.purgeGeneration,
      keyEpoch: context.keyEpoch,
      defaultScopeId: "018f47a0-7b80-7000-8000-000000000108",
      defaultScopeClass: "unknown",
      grantedScopes: ["note", "category", "folder"],
      capabilities: ["note": capability, "category": capability, "folder": capability],
      recordCipherSuite: context.cipherSuite,
      durableSyncSpkiSha256: [UInt8](repeating: 0x77, count: 32),
      transcriptDigest: [UInt8](repeating: 0x88, count: 32))
  }

  private func activeRecord(
    context: RecordCryptoContextV1,
    libraryKey: Data,
    signingPublicKey: Data,
    environment: String = "development",
    libraryDataClass: String = "sanitized_fixture",
    receiptOverride: String? = nil,
    packageEpoch: UInt64? = nil
  ) -> IdentityRecord {
    let metadata = metadata(
      context: context,
      environment: environment,
      libraryDataClass: libraryDataClass)
    let bootstrap = StagedBootstrap(
      handle: "018f47a0-7b80-7000-8000-000000000109",
      receiptId: receiptOverride ?? metadata.receiptId,
      envelopeDigest: Data(repeating: 0x99, count: 32),
      material: keyPackage(key: libraryKey, epoch: packageEpoch ?? context.keyEpoch),
      metadata: metadata)
    return IdentityRecord(
      version: 1,
      handle: "018f47a0-7b80-7000-8000-000000000110",
      deviceId: metadata.deviceId,
      lifecycle: .active,
      signingKeyBacking: .softwareFixture,
      signingKeyRepresentation: Data(repeating: 1, count: 32),
      agreementPrivateKey: Data(repeating: 2, count: 32),
      signingPublicKey: signingPublicKey,
      agreementPublicKey: Data(repeating: 3, count: 32),
      pendingBootstrap: nil,
      activeBootstrap: bootstrap,
      createdAtMs: 1,
      activatedAtMs: 2)
  }

  private func pendingRecord(
    context: RecordCryptoContextV1,
    libraryKey: Data
  ) -> IdentityRecord {
    let active = activeRecord(
      context: context,
      libraryKey: libraryKey,
      signingPublicKey: Data([0x04] + [UInt8](repeating: 4, count: 64)))
    return IdentityRecord(
      version: active.version,
      handle: active.handle,
      deviceId: active.deviceId,
      lifecycle: .pending,
      signingKeyBacking: active.signingKeyBacking,
      signingKeyRepresentation: active.signingKeyRepresentation,
      agreementPrivateKey: active.agreementPrivateKey,
      signingPublicKey: active.signingPublicKey,
      agreementPublicKey: active.agreementPublicKey,
      pendingBootstrap: active.activeBootstrap,
      activeBootstrap: nil,
      createdAtMs: active.createdAtMs,
      activatedAtMs: nil)
  }

  private func discardedRecord(context: RecordCryptoContextV1) -> IdentityRecord {
    let active = activeRecord(
      context: context,
      libraryKey: Data(repeating: 5, count: 32),
      signingPublicKey: Data([0x04] + [UInt8](repeating: 6, count: 64)))
    return IdentityRecord(
      version: active.version,
      handle: active.handle,
      deviceId: active.deviceId,
      lifecycle: .discarded,
      signingKeyBacking: active.signingKeyBacking,
      signingKeyRepresentation: nil,
      agreementPrivateKey: nil,
      signingPublicKey: active.signingPublicKey,
      agreementPublicKey: active.agreementPublicKey,
      pendingBootstrap: nil,
      activeBootstrap: nil,
      createdAtMs: active.createdAtMs,
      activatedAtMs: nil)
  }

  private func legacyActiveRecord(
    context: RecordCryptoContextV1,
    libraryKey: Data
  ) -> IdentityRecord {
    let active = activeRecord(
      context: context,
      libraryKey: libraryKey,
      signingPublicKey: Data([0x04] + [UInt8](repeating: 7, count: 64)))
    let bootstrap = active.activeBootstrap.map {
      StagedBootstrap(
        handle: $0.handle,
        receiptId: $0.receiptId,
        envelopeDigest: $0.envelopeDigest,
        material: $0.material,
        metadata: nil)
    }
    return IdentityRecord(
      version: active.version,
      handle: active.handle,
      deviceId: active.deviceId,
      lifecycle: .active,
      signingKeyBacking: active.signingKeyBacking,
      signingKeyRepresentation: active.signingKeyRepresentation,
      agreementPrivateKey: active.agreementPrivateKey,
      signingPublicKey: active.signingPublicKey,
      agreementPublicKey: active.agreementPublicKey,
      pendingBootstrap: nil,
      activeBootstrap: bootstrap,
      createdAtMs: active.createdAtMs,
      activatedAtMs: active.activatedAtMs)
  }

  private func keyPackage(key: Data, epoch: UInt64) -> Data {
    precondition(key.count == 32)
    var package = Data("NBK1".utf8)
    var version = UInt32(1).bigEndian
    withUnsafeBytes(of: &version) { package.append(contentsOf: $0) }
    var epoch = epoch.bigEndian
    withUnsafeBytes(of: &epoch) { package.append(contentsOf: $0) }
    package.append(key)
    return package
  }

  private func replacingContextField(
    _ context: RecordCryptoContextV1,
    field: String,
    value: Any
  ) throws -> RecordCryptoContextV1 {
    var object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: JSONEncoder().encode(context)) as? [String: Any])
    object[field] = value
    return try JSONDecoder().decode(
      RecordCryptoContextV1.self,
      from: JSONSerialization.data(withJSONObject: object))
  }

  private func descriptor(
    _ value: RecordCiphertextDescriptorV1,
    nonce: Data? = nil,
    ciphertext: Data? = nil,
    contextDigest: Data? = nil,
    envelopeDigest: Data? = nil,
    recordSignature: Data? = nil
  ) -> RecordCiphertextDescriptorV1 {
    RecordCiphertextDescriptorV1(
      version: value.version,
      cipherSuite: value.cipherSuite,
      nonceBase64: nonce?.base64EncodedString() ?? value.nonceBase64,
      ciphertextBase64: ciphertext?.base64EncodedString() ?? value.ciphertextBase64,
      contextDigestBase64: contextDigest?.base64EncodedString() ?? value.contextDigestBase64,
      envelopeDigestBase64: envelopeDigest?.base64EncodedString() ?? value.envelopeDigestBase64,
      recordSignatureBase64: recordSignature?.base64EncodedString()
        ?? value.recordSignatureBase64)
  }

  private func toggled(_ data: Data) -> Data {
    var changed = data
    changed[changed.startIndex] ^= 1
    return changed
  }

  private func fixtureSigningKey() throws -> P256.Signing.PrivateKey {
    try P256.Signing.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([1]))
  }

  private func loadVector() throws -> [String: Any] {
    var root = URL(fileURLWithPath: #filePath)
    for _ in 0..<4 { root.deleteLastPathComponent() }
    let data = try Data(
      contentsOf:
        root
        .appendingPathComponent("fixtures", isDirectory: true)
        .appendingPathComponent("record_crypto_v1.json"))
    return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
  }

  private func vectorContext(_ root: [String: Any]) throws -> RecordCryptoContextV1 {
    let object = try XCTUnwrap(root["context"] as? [String: Any])
    return try JSONDecoder().decode(
      RecordCryptoContextV1.self,
      from: JSONSerialization.data(withJSONObject: object))
  }

  private func string(_ root: [String: Any], _ key: String) throws -> String {
    try XCTUnwrap(root[key] as? String)
  }

  private func base64(_ root: [String: Any], _ key: String) throws -> Data {
    try XCTUnwrap(Data(base64Encoded: try string(root, key)))
  }

  private func hex(_ root: [String: Any], _ key: String) throws -> Data {
    let value = try string(root, key)
    guard value.count.isMultiple(of: 2) else { throw RecordVectorError.invalidHex }
    var bytes = Data(capacity: value.count / 2)
    var index = value.startIndex
    while index < value.endIndex {
      let end = value.index(index, offsetBy: 2)
      guard let byte = UInt8(value[index..<end], radix: 16) else {
        throw RecordVectorError.invalidHex
      }
      bytes.append(byte)
      index = end
    }
    return bytes
  }
}

private enum RecordVectorError: Error {
  case invalidHex
}

/// Encodes on every write and decodes on every read to model the persistence
/// and protected-data behavior of the production Keychain store across vaults.
private final class KeychainRecordTestStore: IdentityRecordStore, @unchecked Sendable {
  private let lock = NSLock()
  private var records: [String: Data] = [:]
  var isProtectedDataLocked = false

  func add(_ record: IdentityRecord) throws {
    lock.lock()
    defer { lock.unlock() }
    records[record.handle] = try JSONEncoder().encode(record)
  }

  func load(handle: String) throws -> IdentityRecord {
    lock.lock()
    defer { lock.unlock() }
    guard !isProtectedDataLocked else { throw NotedSecurityError.protectedDataUnavailable }
    guard let data = records[handle] else { throw NotedSecurityError.identityNotFound }
    return try JSONDecoder().decode(IdentityRecord.self, from: data)
  }

  func loadAll() throws -> [IdentityRecord] {
    lock.lock()
    defer { lock.unlock() }
    guard !isProtectedDataLocked else { throw NotedSecurityError.protectedDataUnavailable }
    return try records.values.map { try JSONDecoder().decode(IdentityRecord.self, from: $0) }
  }

  func replace(_ record: IdentityRecord) throws {
    lock.lock()
    defer { lock.unlock() }
    guard !isProtectedDataLocked else { throw NotedSecurityError.protectedDataUnavailable }
    guard records[record.handle] != nil else { throw NotedSecurityError.identityNotFound }
    records[record.handle] = try JSONEncoder().encode(record)
  }
}
