import CryptoKit
import Foundation

public struct BootstrapCapabilityV1: Codable, Equatable, Sendable {
  public let readerVersion: UInt32
  public let writerVersion: UInt32?

  public init(readerVersion: UInt32, writerVersion: UInt32?) {
    self.readerVersion = readerVersion
    self.writerVersion = writerVersion
  }
}

/// Public pairing metadata. This value contains no key material and is safe to
/// persist in the replica database or return through the Rust bridge.
public struct BootstrapMetadataV1: Codable, Equatable, Sendable {
  public let version: UInt32
  public let protocolName: String
  public let suite: String
  public let syncProtocolVersion: UInt32
  public let environment: String
  public let libraryDataClass: String
  public let receiptId: String
  public let libraryId: String
  public let deviceId: String
  public let authorityGeneration: UInt64
  public let purgeGeneration: UInt64
  public let keyEpoch: UInt64
  public let defaultScopeId: String
  public let defaultScopeClass: String
  public let grantedScopes: [String]
  public let capabilities: [String: BootstrapCapabilityV1]
  public let recordCipherSuite: String
  public let durableSyncSpkiSha256: [UInt8]
  public let transcriptDigest: [UInt8]

  enum CodingKeys: String, CodingKey {
    case version
    case protocolName = "protocol"
    case suite
    case syncProtocolVersion
    case environment
    case libraryDataClass
    case receiptId
    case libraryId
    case deviceId
    case authorityGeneration
    case purgeGeneration
    case keyEpoch
    case defaultScopeId
    case defaultScopeClass
    case grantedScopes
    case capabilities
    case recordCipherSuite
    case durableSyncSpkiSha256
    case transcriptDigest
  }
}

public struct StagedBootstrapDescriptor: Codable, Equatable, Sendable {
  public let pendingBootstrapHandle: String
  public let metadata: BootstrapMetadataV1
}

enum BootstrapContractV1 {
  static let metadataVersion: UInt32 = 1
  static let syncProtocolVersion: UInt32 = 1
  static let keyPackageVersion: UInt32 = 1
  static let keyPackageByteCount = 48
  static let keyPackageCiphertextByteCount = keyPackageByteCount + 16
  static let pairingProtocol = "noted.direct-pairing.v1"
  static let pairingSuite =
    "tls13+p256-p1363+auth-hpke-x25519-hkdfsha256-aes256gcm"
  static let recordCipherSuite = "noted.record-aead.v1+aes256gcm+hkdfsha256"
  static let exactScopes = ["note", "category", "folder"]

  static func validate(metadata: BootstrapMetadataV1) throws {
    let exactCapability = BootstrapCapabilityV1(readerVersion: 1, writerVersion: 1)
    guard metadata.version == metadataVersion,
      metadata.protocolName == pairingProtocol,
      metadata.suite == pairingSuite,
      metadata.syncProtocolVersion == syncProtocolVersion,
      metadata.environment == "development",
      metadata.libraryDataClass == "sanitized_fixture",
      metadata.authorityGeneration > 0,
      metadata.keyEpoch > 0,
      metadata.authorityGeneration <= UInt64(Int64.max),
      metadata.purgeGeneration <= UInt64(Int64.max),
      metadata.keyEpoch <= UInt64(Int64.max),
      metadata.defaultScopeClass == "unknown",
      metadata.grantedScopes == exactScopes,
      metadata.capabilities.count == exactScopes.count,
      exactScopes.allSatisfy({ metadata.capabilities[$0] == exactCapability }),
      metadata.recordCipherSuite == recordCipherSuite,
      metadata.durableSyncSpkiSha256.count == 32,
      metadata.durableSyncSpkiSha256.contains(where: { $0 != 0 }),
      metadata.transcriptDigest.count == 32
    else {
      throw NotedSecurityError.invalidArguments("invalid bootstrap contract")
    }
    try UUIDv7Generator.validate(metadata.receiptId)
    try UUIDv7Generator.validate(metadata.libraryId)
    try UUIDv7Generator.validate(metadata.deviceId)
    try UUIDv7Generator.validate(metadata.defaultScopeId)
  }

  static func canonicalMetadata(_ metadata: BootstrapMetadataV1) throws -> Data {
    try validate(metadata: metadata)
    var builder = BootstrapCanonicalBuilder(
      domain: "noted.direct-pairing.v1/bootstrap-metadata-v1")
    builder.unsigned64("version", UInt64(metadata.version))
    builder.text("protocol", metadata.protocolName)
    builder.text("suite", metadata.suite)
    builder.unsigned64("sync_protocol_version", UInt64(metadata.syncProtocolVersion))
    builder.text("environment", metadata.environment)
    builder.text("library_data_class", metadata.libraryDataClass)
    builder.text("receipt_id", metadata.receiptId)
    builder.text("library_id", metadata.libraryId)
    builder.text("device_id", metadata.deviceId)
    builder.unsigned64("authority_generation", metadata.authorityGeneration)
    builder.unsigned64("purge_generation", metadata.purgeGeneration)
    builder.unsigned64("key_epoch", metadata.keyEpoch)
    builder.text("default_scope_id", metadata.defaultScopeId)
    builder.text("default_scope_class", metadata.defaultScopeClass)

    var scopes = BootstrapCanonicalBuilder(
      domain: "noted.direct-pairing.v1/record-kind-list")
    scopes.unsigned64("count", UInt64(metadata.grantedScopes.count))
    for scope in metadata.grantedScopes {
      scopes.text("kind", scope)
    }
    builder.bytes("granted_scopes", scopes.finish())

    var capabilities = BootstrapCanonicalBuilder(
      domain: "noted.direct-pairing.v1/capabilities")
    capabilities.unsigned64("count", UInt64(metadata.capabilities.count))
    for scope in exactScopes {
      guard let capability = metadata.capabilities[scope] else {
        throw NotedSecurityError.invalidArguments("invalid bootstrap contract")
      }
      capabilities.text("kind", scope)
      capabilities.unsigned64("reader_version", UInt64(capability.readerVersion))
      if let writer = capability.writerVersion {
        capabilities.text("writer_present", "true")
        capabilities.unsigned64("writer_version", UInt64(writer))
      } else {
        capabilities.text("writer_present", "false")
      }
    }
    builder.bytes("capabilities", capabilities.finish())
    builder.text("record_cipher_suite", metadata.recordCipherSuite)
    builder.bytes(
      "durable_sync_spki_sha256", Data(metadata.durableSyncSpkiSha256))
    builder.bytes("transcript_digest", Data(metadata.transcriptDigest))
    return builder.finish()
  }

  static func info(metadata: BootstrapMetadataV1) throws -> Data {
    try metadataContext(
      domain: "noted.direct-pairing.v1/hpke/bootstrap/info", metadata: metadata)
  }

  static func associatedData(metadata: BootstrapMetadataV1) throws -> Data {
    try metadataContext(
      domain: "noted.direct-pairing.v1/hpke/bootstrap/aad", metadata: metadata)
  }

  static func exporterContext(metadata: BootstrapMetadataV1) throws -> Data {
    try metadataContext(
      domain: "noted.direct-pairing.v1/hpke/bootstrap/exporter", metadata: metadata)
  }

  static func envelopeDigest(
    protocolName: String,
    receiptId: String,
    metadata: BootstrapMetadataV1,
    encapsulatedKey: Data,
    ciphertext: Data
  ) throws -> Data {
    guard protocolName == pairingProtocol,
      receiptId == metadata.receiptId,
      encapsulatedKey.count == 32,
      ciphertext.count == keyPackageCiphertextByteCount
    else {
      throw NotedSecurityError.invalidArguments("invalid bootstrap contract")
    }
    var sealed = BootstrapCanonicalBuilder(
      domain: "noted.direct-pairing.v1/authenticated-hpke-envelope")
    sealed.bytes("encapsulated_key", encapsulatedKey)
    sealed.bytes("ciphertext", ciphertext)
    var envelope = BootstrapCanonicalBuilder(
      domain: "noted.direct-pairing.v1/bootstrap-envelope-v1")
    envelope.text("protocol", protocolName)
    envelope.text("receipt_id", receiptId)
    envelope.bytes("metadata", try canonicalMetadata(metadata))
    envelope.bytes("sealed_key_package", sealed.finish())
    return Data(SHA256.hash(data: envelope.finish()))
  }

  static func validateKeyPackage(_ package: Data, metadata: BootstrapMetadataV1) throws {
    guard package.count == keyPackageByteCount,
      package.prefix(4) == Data("NBK1".utf8),
      readUInt32(package, offset: 4) == keyPackageVersion,
      readUInt64(package, offset: 8) == metadata.keyEpoch,
      package.suffix(32).contains(where: { $0 != 0 })
    else {
      throw NotedSecurityError.invalidArguments("invalid bootstrap contract")
    }
  }

  private static func metadataContext(
    domain: String,
    metadata: BootstrapMetadataV1
  ) throws -> Data {
    var builder = BootstrapCanonicalBuilder(domain: domain)
    builder.bytes("metadata", try canonicalMetadata(metadata))
    return builder.finish()
  }

  private static func readUInt32(_ data: Data, offset: Int) -> UInt32 {
    data[offset..<(offset + 4)].reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
  }

  private static func readUInt64(_ data: Data, offset: Int) -> UInt64 {
    data[offset..<(offset + 8)].reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
  }
}

private struct BootstrapCanonicalBuilder {
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
