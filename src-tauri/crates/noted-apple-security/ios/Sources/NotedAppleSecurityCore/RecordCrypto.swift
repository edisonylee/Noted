import CryptoKit
import Foundation

public enum RecordKindV1: String, Codable, CaseIterable, Equatable, Sendable {
  case note
  case category
  case folder
}

public enum RecordCryptoOperationV1: String, Codable, Equatable, Sendable {
  case create
  case update
  case delete
}

public enum RecordCryptoLimitsV1 {
  public static let nonceByteCount = 12
  public static let tagByteCount = 16
  public static let maximumContainerByteCount = 512 * 1024
  public static let fixedContainerOverhead =
    4 + 4 + 4 + nonceByteCount + 32 + 32 + 64
  public static let maximumCiphertextByteCount =
    maximumContainerByteCount - fixedContainerOverhead
  public static let maximumPlaintextByteCount = maximumCiphertextByteCount - tagByteCount
}

/// Public, target-independent facts that bind one encrypted record version.
/// No secret or native key handle is part of this value.
public struct RecordCryptoContextV1: Codable, Equatable, Sendable {
  public let version: UInt32
  public let cipherSuite: String
  public let libraryId: String
  public let recordId: String
  public let recordKind: RecordKindV1
  public let schemaVersion: UInt32
  public let baseRevision: UInt64
  public let baseVersionId: String?
  public let proposedRevision: UInt64
  public let versionId: String
  public let mutationId: String
  public let authorityGeneration: UInt64
  public let purgeGeneration: UInt64
  public let keyEpoch: UInt64
  public let operation: RecordCryptoOperationV1

  public init(
    version: UInt32,
    cipherSuite: String,
    libraryId: String,
    recordId: String,
    recordKind: RecordKindV1,
    schemaVersion: UInt32,
    baseRevision: UInt64,
    baseVersionId: String?,
    proposedRevision: UInt64,
    versionId: String,
    mutationId: String,
    authorityGeneration: UInt64,
    purgeGeneration: UInt64,
    keyEpoch: UInt64,
    operation: RecordCryptoOperationV1
  ) {
    self.version = version
    self.cipherSuite = cipherSuite
    self.libraryId = libraryId
    self.recordId = recordId
    self.recordKind = recordKind
    self.schemaVersion = schemaVersion
    self.baseRevision = baseRevision
    self.baseVersionId = baseVersionId
    self.proposedRevision = proposedRevision
    self.versionId = versionId
    self.mutationId = mutationId
    self.authorityGeneration = authorityGeneration
    self.purgeGeneration = purgeGeneration
    self.keyEpoch = keyEpoch
    self.operation = operation
  }

  public init(from decoder: Decoder) throws {
    try rejectUnknownKeys(decoder, allowed: Set(CodingKeys.allCases.map(\.stringValue)))
    let values = try decoder.container(keyedBy: CodingKeys.self)
    self.init(
      version: try values.decode(UInt32.self, forKey: .version),
      cipherSuite: try values.decode(String.self, forKey: .cipherSuite),
      libraryId: try values.decode(String.self, forKey: .libraryId),
      recordId: try values.decode(String.self, forKey: .recordId),
      recordKind: try values.decode(RecordKindV1.self, forKey: .recordKind),
      schemaVersion: try values.decode(UInt32.self, forKey: .schemaVersion),
      baseRevision: try values.decode(UInt64.self, forKey: .baseRevision),
      baseVersionId: try values.decodeIfPresent(String.self, forKey: .baseVersionId),
      proposedRevision: try values.decode(UInt64.self, forKey: .proposedRevision),
      versionId: try values.decode(String.self, forKey: .versionId),
      mutationId: try values.decode(String.self, forKey: .mutationId),
      authorityGeneration: try values.decode(UInt64.self, forKey: .authorityGeneration),
      purgeGeneration: try values.decode(UInt64.self, forKey: .purgeGeneration),
      keyEpoch: try values.decode(UInt64.self, forKey: .keyEpoch),
      operation: try values.decode(RecordCryptoOperationV1.self, forKey: .operation))
  }

  private enum CodingKeys: String, CodingKey, CaseIterable {
    case version
    case cipherSuite
    case libraryId
    case recordId
    case recordKind
    case schemaVersion
    case baseRevision
    case baseVersionId
    case proposedRevision
    case versionId
    case mutationId
    case authorityGeneration
    case purgeGeneration
    case keyEpoch
    case operation
  }
}

/// Native bridge descriptor. `recordSignatureBase64` is the inner encrypted
/// record signature serialized inside a mutation's ciphertext. It must never
/// be reused as the outer mutation-envelope signature.
public struct RecordCiphertextDescriptorV1: Codable, Equatable, Sendable {
  public let version: UInt32
  public let cipherSuite: String
  public let nonceBase64: String
  public let ciphertextBase64: String
  public let contextDigestBase64: String
  public let envelopeDigestBase64: String
  public let recordSignatureBase64: String

  public init(
    version: UInt32,
    cipherSuite: String,
    nonceBase64: String,
    ciphertextBase64: String,
    contextDigestBase64: String,
    envelopeDigestBase64: String,
    recordSignatureBase64: String
  ) {
    self.version = version
    self.cipherSuite = cipherSuite
    self.nonceBase64 = nonceBase64
    self.ciphertextBase64 = ciphertextBase64
    self.contextDigestBase64 = contextDigestBase64
    self.envelopeDigestBase64 = envelopeDigestBase64
    self.recordSignatureBase64 = recordSignatureBase64
  }

  public init(from decoder: Decoder) throws {
    try rejectUnknownKeys(decoder, allowed: Set(CodingKeys.allCases.map(\.stringValue)))
    let values = try decoder.container(keyedBy: CodingKeys.self)
    self.init(
      version: try values.decode(UInt32.self, forKey: .version),
      cipherSuite: try values.decode(String.self, forKey: .cipherSuite),
      nonceBase64: try values.decode(String.self, forKey: .nonceBase64),
      ciphertextBase64: try values.decode(String.self, forKey: .ciphertextBase64),
      contextDigestBase64: try values.decode(String.self, forKey: .contextDigestBase64),
      envelopeDigestBase64: try values.decode(String.self, forKey: .envelopeDigestBase64),
      recordSignatureBase64: try values.decode(String.self, forKey: .recordSignatureBase64))
  }

  private enum CodingKeys: String, CodingKey, CaseIterable {
    case version
    case cipherSuite
    case nonceBase64
    case ciphertextBase64
    case contextDigestBase64
    case envelopeDigestBase64
    case recordSignatureBase64
  }
}

public struct OpenedRecordDescriptorV1: Codable, Equatable, Sendable {
  public let plaintextBase64: String
  public let contextDigestBase64: String
  public let envelopeDigestBase64: String
}

@available(iOS 17.0, macOS 14.0, *)
enum RecordCryptoContractV1 {
  static let contextVersion: UInt32 = 1
  static let nonceByteCount = RecordCryptoLimitsV1.nonceByteCount
  static let tagByteCount = RecordCryptoLimitsV1.tagByteCount
  static let maximumContainerByteCount = RecordCryptoLimitsV1.maximumContainerByteCount
  static let fixedContainerOverhead = RecordCryptoLimitsV1.fixedContainerOverhead
  static let maximumCiphertextByteCount = RecordCryptoLimitsV1.maximumCiphertextByteCount
  static let maximumPlaintextByteCount = RecordCryptoLimitsV1.maximumPlaintextByteCount
  static let hkdfSaltDomain = "noted.record-aead.v1/hkdf-salt"

  typealias NonceProvider = @Sendable () throws -> Data
  typealias RecordSigner = @Sendable (Data) throws -> Data

  static func validate(context: RecordCryptoContextV1) throws {
    guard context.version == contextVersion,
      context.cipherSuite == BootstrapContractV1.recordCipherSuite,
      context.schemaVersion == 1,
      context.authorityGeneration > 0,
      context.keyEpoch > 0,
      context.authorityGeneration <= UInt64(Int64.max),
      context.purgeGeneration <= UInt64(Int64.max),
      context.keyEpoch <= UInt64(Int64.max),
      context.baseRevision <= UInt64(Int64.max),
      context.proposedRevision <= UInt64(Int64.max),
      context.baseRevision < UInt64.max,
      context.proposedRevision == context.baseRevision + 1
    else {
      throw NotedSecurityError.invalidArguments("invalid record crypto context")
    }
    try UUIDv7Generator.validate(context.libraryId)
    try UUIDv7Generator.validate(context.recordId)
    try UUIDv7Generator.validate(context.versionId)
    try UUIDv7Generator.validate(context.mutationId)
    if let baseVersionId = context.baseVersionId {
      try UUIDv7Generator.validate(baseVersionId)
    }
    let initial = context.baseRevision == 0 && context.baseVersionId == nil
    let continuation = context.baseRevision > 0 && context.baseVersionId != nil
    guard
      (context.operation == .create && initial)
        || ((context.operation == .update || context.operation == .delete) && continuation)
    else {
      throw NotedSecurityError.invalidArguments("invalid record revision operation")
    }
    var identifiers = [context.libraryId, context.recordId, context.versionId, context.mutationId]
    if let baseVersionId = context.baseVersionId { identifiers.append(baseVersionId) }
    guard Set(identifiers).count == identifiers.count else {
      throw NotedSecurityError.invalidArguments("record identifiers must be distinct")
    }
  }

  static func validate(
    context: RecordCryptoContextV1,
    against metadata: BootstrapMetadataV1
  ) throws {
    try validate(context: context)
    try BootstrapContractV1.validate(metadata: metadata)
    guard context.libraryId == metadata.libraryId,
      context.authorityGeneration == metadata.authorityGeneration,
      context.purgeGeneration == metadata.purgeGeneration,
      context.keyEpoch == metadata.keyEpoch,
      context.cipherSuite == metadata.recordCipherSuite,
      metadata.grantedScopes.contains(context.recordKind.rawValue),
      let capability = metadata.capabilities[context.recordKind.rawValue],
      capability.readerVersion >= context.schemaVersion,
      capability.writerVersion == context.schemaVersion
    else {
      throw NotedSecurityError.recordContextMismatch
    }
  }

  static func canonicalContext(_ context: RecordCryptoContextV1) throws -> Data {
    try validate(context: context)
    var builder = RecordCanonicalBuilder(domain: "noted.record-aead.v1/context")
    builder.unsigned64("version", UInt64(context.version))
    builder.text("cipher_suite", context.cipherSuite)
    builder.text("library_id", context.libraryId)
    builder.text("record_id", context.recordId)
    builder.text("record_kind", context.recordKind.rawValue)
    builder.unsigned64("schema_version", UInt64(context.schemaVersion))
    builder.unsigned64("base_revision", context.baseRevision)
    if let baseVersionId = context.baseVersionId {
      builder.text("base_version_present", "true")
      builder.text("base_version_id", baseVersionId)
    } else {
      builder.text("base_version_present", "false")
    }
    builder.unsigned64("proposed_revision", context.proposedRevision)
    builder.text("version_id", context.versionId)
    builder.text("mutation_id", context.mutationId)
    builder.unsigned64("authority_generation", context.authorityGeneration)
    builder.unsigned64("purge_generation", context.purgeGeneration)
    builder.unsigned64("key_epoch", context.keyEpoch)
    builder.text("operation", context.operation.rawValue)
    return builder.finish()
  }

  static func hkdfSalt() -> Data {
    Data(SHA256.hash(data: Data(hkdfSaltDomain.utf8)))
  }

  static func hkdfInfo(_ context: RecordCryptoContextV1) throws -> Data {
    try contextWrapper(domain: "noted.record-aead.v1/hkdf-info", context: context)
  }

  static func associatedData(_ context: RecordCryptoContextV1) throws -> Data {
    try contextWrapper(domain: "noted.record-aead.v1/aad", context: context)
  }

  static func contextDigest(_ context: RecordCryptoContextV1) throws -> Data {
    Data(SHA256.hash(data: try canonicalContext(context)))
  }

  static func envelopeDigest(
    context: RecordCryptoContextV1,
    nonce: Data,
    ciphertext: Data
  ) throws -> Data {
    guard nonce.count == nonceByteCount,
      (tagByteCount...maximumCiphertextByteCount).contains(ciphertext.count)
    else {
      throw NotedSecurityError.invalidArguments("invalid record ciphertext bounds")
    }
    var builder = RecordCanonicalBuilder(domain: "noted.record-aead.v1/envelope")
    builder.bytes("context", try canonicalContext(context))
    builder.bytes("nonce", nonce)
    builder.bytes("ciphertext", ciphertext)
    return Data(SHA256.hash(data: builder.finish()))
  }

  static func signatureMessage(envelopeDigest: Data) throws -> Data {
    guard envelopeDigest.count == 32 else {
      throw NotedSecurityError.invalidArguments("invalid record envelope digest")
    }
    var builder = RecordCanonicalBuilder(domain: "noted.record-aead.v1/signature")
    builder.bytes("envelope_digest", envelopeDigest)
    return Data(SHA256.hash(data: builder.finish()))
  }

  static func seal(
    record: IdentityRecord,
    context: RecordCryptoContextV1,
    plaintext: Data,
    nonceProvider: NonceProvider = { try AppleCrypto.secureRandomBytes(count: nonceByteCount) },
    signer: RecordSigner? = nil
  ) throws -> RecordCiphertextDescriptorV1 {
    guard plaintext.count <= maximumPlaintextByteCount else {
      throw NotedSecurityError.invalidArguments("record plaintext exceeds transaction limit")
    }
    let bootstrap = try activeBootstrap(record: record, context: context)
    let nonce = try nonceProvider()
    guard nonce.count == nonceByteCount else {
      throw NotedSecurityError.entropyUnavailable
    }
    let sealed: AES.GCM.SealedBox
    do {
      sealed = try AES.GCM.seal(
        plaintext,
        using: try recordKey(bootstrap: bootstrap, context: context),
        nonce: try AES.GCM.Nonce(data: nonce),
        authenticating: try associatedData(context))
    } catch let error as NotedSecurityError {
      throw error
    } catch {
      throw NotedSecurityError.recordCryptoFailed
    }
    var ciphertext = sealed.ciphertext
    ciphertext.append(sealed.tag)
    let contextDigest = try contextDigest(context)
    let envelopeDigest = try envelopeDigest(
      context: context, nonce: nonce, ciphertext: ciphertext)
    let message = try signatureMessage(envelopeDigest: envelopeDigest)
    let recordSignature = try signer?(message) ?? AppleCrypto.sign(record: record, message: message)
    guard recordSignature.count == 64 else {
      throw NotedSecurityError.signingFailed
    }
    return RecordCiphertextDescriptorV1(
      version: contextVersion,
      cipherSuite: BootstrapContractV1.recordCipherSuite,
      nonceBase64: nonce.base64EncodedString(),
      ciphertextBase64: ciphertext.base64EncodedString(),
      contextDigestBase64: contextDigest.base64EncodedString(),
      envelopeDigestBase64: envelopeDigest.base64EncodedString(),
      recordSignatureBase64: recordSignature.base64EncodedString())
  }

  static func open(
    record: IdentityRecord,
    context: RecordCryptoContextV1,
    sealed: RecordCiphertextDescriptorV1,
    expectedSignerPublicKey: Data
  ) throws -> OpenedRecordDescriptorV1 {
    let bootstrap = try activeBootstrap(record: record, context: context)
    let components = try validate(sealed: sealed, context: context)
    guard expectedSignerPublicKey.count == 65, expectedSignerPublicKey.first == 0x04 else {
      throw NotedSecurityError.invalidArguments("invalid record signer public key")
    }
    let message = try signatureMessage(envelopeDigest: components.envelopeDigest)
    let signatureValid: Bool
    do {
      signatureValid = try AppleCrypto.verifyP256Signature(
        publicKey: expectedSignerPublicKey,
        message: message,
        signature: components.recordSignature)
    } catch {
      throw NotedSecurityError.recordSignatureInvalid
    }
    guard signatureValid else {
      throw NotedSecurityError.recordSignatureInvalid
    }

    let encrypted = components.ciphertext.dropLast(tagByteCount)
    let tag = components.ciphertext.suffix(tagByteCount)
    let plaintext: Data
    do {
      let box = try AES.GCM.SealedBox(
        nonce: AES.GCM.Nonce(data: components.nonce),
        ciphertext: Data(encrypted),
        tag: Data(tag))
      plaintext = try AES.GCM.open(
        box,
        using: try recordKey(bootstrap: bootstrap, context: context),
        authenticating: try associatedData(context))
    } catch let error as NotedSecurityError {
      throw error
    } catch {
      throw NotedSecurityError.recordCryptoFailed
    }
    guard plaintext.count <= maximumPlaintextByteCount else {
      throw NotedSecurityError.recordCryptoFailed
    }
    return OpenedRecordDescriptorV1(
      plaintextBase64: plaintext.base64EncodedString(),
      contextDigestBase64: components.contextDigest.base64EncodedString(),
      envelopeDigestBase64: components.envelopeDigest.base64EncodedString())
  }

  private struct SealedComponents {
    let nonce: Data
    let ciphertext: Data
    let contextDigest: Data
    let envelopeDigest: Data
    let recordSignature: Data
  }

  private static func validate(
    sealed: RecordCiphertextDescriptorV1,
    context: RecordCryptoContextV1
  ) throws -> SealedComponents {
    try validate(context: context)
    guard sealed.version == contextVersion,
      sealed.cipherSuite == BootstrapContractV1.recordCipherSuite
    else {
      throw NotedSecurityError.invalidArguments("invalid record ciphertext version")
    }
    let nonce = try decodeCanonicalBase64(
      sealed.nonceBase64, minimum: nonceByteCount, maximum: nonceByteCount)
    let ciphertext = try decodeCanonicalBase64(
      sealed.ciphertextBase64,
      minimum: tagByteCount,
      maximum: maximumCiphertextByteCount)
    let suppliedContextDigest = try decodeCanonicalBase64(
      sealed.contextDigestBase64, minimum: 32, maximum: 32)
    let suppliedEnvelopeDigest = try decodeCanonicalBase64(
      sealed.envelopeDigestBase64, minimum: 32, maximum: 32)
    let recordSignature = try decodeCanonicalBase64(
      sealed.recordSignatureBase64, minimum: 64, maximum: 64)
    let expectedContextDigest = try contextDigest(context)
    let expectedEnvelopeDigest = try envelopeDigest(
      context: context, nonce: nonce, ciphertext: ciphertext)
    guard suppliedContextDigest == expectedContextDigest,
      suppliedEnvelopeDigest == expectedEnvelopeDigest
    else {
      throw NotedSecurityError.recordContextMismatch
    }
    return SealedComponents(
      nonce: nonce,
      ciphertext: ciphertext,
      contextDigest: suppliedContextDigest,
      envelopeDigest: suppliedEnvelopeDigest,
      recordSignature: recordSignature)
  }

  private static func activeBootstrap(
    record: IdentityRecord,
    context: RecordCryptoContextV1
  ) throws -> StagedBootstrap {
    guard record.lifecycle == .active else {
      throw NotedSecurityError.invalidIdentityState(
        expected: IdentityLifecycle.active.rawValue,
        actual: record.lifecycle.rawValue)
    }
    guard record.pendingBootstrap == nil, let bootstrap = record.activeBootstrap else {
      throw NotedSecurityError.identityCorrupted("missing active bootstrap")
    }
    guard let metadata = bootstrap.metadata else {
      throw NotedSecurityError.legacyBootstrapRequiresDiscard
    }
    try IdentityBootstrapValidator.validate(record)
    guard bootstrap.receiptId == metadata.receiptId,
      metadata.deviceId == record.deviceId
    else {
      throw NotedSecurityError.recordContextMismatch
    }
    try validate(context: context, against: metadata)
    return bootstrap
  }

  private static func recordKey(
    bootstrap: StagedBootstrap,
    context: RecordCryptoContextV1
  ) throws -> SymmetricKey {
    guard let metadata = bootstrap.metadata else {
      throw NotedSecurityError.legacyBootstrapRequiresDiscard
    }
    try BootstrapContractV1.validateKeyPackage(bootstrap.material, metadata: metadata)
    let libraryKey = SymmetricKey(data: bootstrap.material.suffix(32))
    return HKDF<SHA256>.deriveKey(
      inputKeyMaterial: libraryKey,
      salt: hkdfSalt(),
      info: try hkdfInfo(context),
      outputByteCount: 32)
  }

  private static func contextWrapper(
    domain: String,
    context: RecordCryptoContextV1
  ) throws -> Data {
    var builder = RecordCanonicalBuilder(domain: domain)
    builder.bytes("context", try canonicalContext(context))
    return builder.finish()
  }
}

private func decodeCanonicalBase64(
  _ value: String,
  minimum: Int,
  maximum: Int
) throws -> Data {
  guard minimum >= 0, maximum >= minimum,
    value.utf8.count <= ((maximum + 2) / 3) * 4 + 4,
    let decoded = Data(base64Encoded: value),
    (minimum...maximum).contains(decoded.count),
    decoded.base64EncodedString() == value
  else {
    throw NotedSecurityError.invalidArguments("invalid record binary field")
  }
  return decoded
}

private func rejectUnknownKeys(_ decoder: Decoder, allowed: Set<String>) throws {
  let container = try decoder.container(keyedBy: AnyRecordCodingKey.self)
  guard container.allKeys.allSatisfy({ allowed.contains($0.stringValue) }) else {
    throw DecodingError.dataCorrupted(
      .init(codingPath: decoder.codingPath, debugDescription: "unknown record crypto field"))
  }
}

private struct AnyRecordCodingKey: CodingKey {
  let stringValue: String
  let intValue: Int?

  init?(stringValue: String) {
    self.stringValue = stringValue
    self.intValue = nil
  }

  init?(intValue: Int) {
    self.stringValue = String(intValue)
    self.intValue = intValue
  }
}

private struct RecordCanonicalBuilder {
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

  func finish() -> Data { data }
}
