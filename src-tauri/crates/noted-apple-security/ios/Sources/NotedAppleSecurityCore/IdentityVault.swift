import Foundation
import LocalAuthentication
import Security

private let identityService = "com.noted.app.apple-security.identity.v1"

@available(iOS 17.0, macOS 14.0, *)
public final class IdentityVault: @unchecked Sendable {
  private let queue = DispatchQueue(label: "com.noted.app.apple-security.identity-vault")
  private let store: any IdentityRecordStore
  private let now: @Sendable () -> Int64
  private let keyPairValidator: @Sendable (IdentityRecord) throws -> Void
  private let recordNonceProvider: RecordCryptoContractV1.NonceProvider
  private let recordSigner: @Sendable (IdentityRecord, Data) throws -> Data

  public init(
    accessGroup: String? = nil,
    now: @escaping @Sendable () -> Int64 = {
      Int64((Date().timeIntervalSince1970 * 1_000).rounded())
    }
  ) {
    self.store = KeychainIdentityStore(accessGroup: accessGroup)
    self.now = now
    self.keyPairValidator = { try AppleCrypto.validateKeyPair(record: $0) }
    self.recordNonceProvider = { try AppleCrypto.secureRandomBytes(count: 12) }
    self.recordSigner = { try AppleCrypto.sign(record: $0, message: $1) }
  }

  /// Internal dependency seam for host Keychain-store tests. The public
  /// initializer always uses Data Protection Keychain + native cryptography.
  init(
    store: any IdentityRecordStore,
    now: @escaping @Sendable () -> Int64,
    keyPairValidator: @escaping @Sendable (IdentityRecord) throws -> Void,
    recordNonceProvider: @escaping RecordCryptoContractV1.NonceProvider,
    recordSigner: @escaping @Sendable (IdentityRecord, Data) throws -> Data
  ) {
    self.store = store
    self.now = now
    self.keyPairValidator = keyPairValidator
    self.recordNonceProvider = recordNonceProvider
    self.recordSigner = recordSigner
  }

  public func prepareIdentity(
    deviceId: String,
    fixtureGate: String?
  ) throws -> PublicIdentityDescriptor {
    try queue.sync {
      try UUIDv7Generator.validate(deviceId)
      let material = try AppleCrypto.generateIdentity(fixtureGate: fixtureGate)
      let handle = UUID().uuidString.lowercased()
      let record = IdentityRecord(
        version: 1,
        handle: handle,
        deviceId: deviceId,
        lifecycle: .pending,
        signingKeyBacking: material.backing,
        signingKeyRepresentation: material.signingKeyRepresentation,
        agreementPrivateKey: material.agreementPrivateKey,
        signingPublicKey: material.signingPublicKey,
        agreementPublicKey: material.agreementPublicKey,
        pendingBootstrap: nil,
        activeBootstrap: nil,
        createdAtMs: now(),
        activatedAtMs: nil)
      try validate(record)
      try store.add(record)
      return record.publicDescriptor()
    }
  }

  public func identity(handle: String) throws -> PublicIdentityDescriptor {
    try queue.sync {
      let record = try store.load(handle: try validatedHandle(handle))
      try validate(record)
      return record.publicDescriptor()
    }
  }

  /// Enumerates public descriptors for crash/reinstall reconciliation without
  /// releasing any private or bootstrap material from Keychain custody.
  public func inventory() throws -> IdentityInventory {
    try queue.sync {
      let records = try store.loadAll()
      // Inventory is the recovery entry point for pre-contract pending
      // bootstraps. Expose only their identity tombstone candidate; the public
      // descriptor deliberately omits a metadata-less bootstrap binding.
      for record in records {
        try validate(record, allowLegacyPendingBootstrap: true)
      }
      let descriptors = records.map { $0.publicDescriptor() }
      return IdentityInventory(
        pending: descriptors.filter { $0.lifecycle == .pending }.sorted { $0.handle < $1.handle },
        active: descriptors.filter { $0.lifecycle == .active }.sorted { $0.handle < $1.handle },
        discarded: descriptors.filter { $0.lifecycle == .discarded }.sorted {
          $0.handle < $1.handle
        })
    }
  }

  public func sign(handle: String, message: Data) throws -> Data {
    try queue.sync {
      let record = try store.load(handle: try validatedHandle(handle))
      try validate(record)
      return try AppleCrypto.sign(record: record, message: message)
    }
  }

  public func sealRecord(
    identityHandle: String,
    context: RecordCryptoContextV1,
    plaintext: Data
  ) throws -> RecordCiphertextDescriptorV1 {
    try queue.sync {
      let record = try store.load(handle: try validatedHandle(identityHandle))
      try validate(record)
      return try RecordCryptoContractV1.seal(
        record: record,
        context: context,
        plaintext: plaintext,
        nonceProvider: recordNonceProvider,
        signer: { try self.recordSigner(record, $0) })
    }
  }

  /// `expectedSignerPublicKey` must come from an independently authenticated
  /// device-key directory. No key embedded beside the ciphertext is trusted.
  public func openRecord(
    identityHandle: String,
    context: RecordCryptoContextV1,
    sealed: RecordCiphertextDescriptorV1,
    expectedSignerPublicKey: Data
  ) throws -> OpenedRecordDescriptorV1 {
    try queue.sync {
      let record = try store.load(handle: try validatedHandle(identityHandle))
      try validate(record)
      return try RecordCryptoContractV1.open(
        record: record,
        context: context,
        sealed: sealed,
        expectedSignerPublicKey: expectedSignerPublicKey)
    }
  }

  public func openAuthenticatedHpke(
    handle: String,
    senderPublicKey: Data,
    info: Data,
    associatedData: Data,
    encapsulatedKey: Data,
    ciphertext: Data,
    exporterContext: Data
  ) throws -> AuthenticatedHpkeOpen {
    try queue.sync {
      let record = try store.load(handle: try validatedHandle(handle))
      try validate(record)
      return try AppleCrypto.openAuthenticatedHpke(
        record: record,
        senderPublicKey: senderPublicKey,
        info: info,
        associatedData: associatedData,
        encapsulatedKey: encapsulatedKey,
        ciphertext: ciphertext,
        exporterContext: exporterContext)
    }
  }

  public func stageBootstrapAuthenticated(
    handle: String,
    senderPublicKey: Data,
    info: Data,
    associatedData: Data,
    encapsulatedKey: Data,
    ciphertext: Data,
    receiptId: String,
    envelopeDigest: Data,
    metadata: BootstrapMetadataV1
  ) throws -> StagedBootstrapDescriptor {
    try queue.sync {
      let canonicalHandle = try validatedHandle(handle)
      var record = try store.load(handle: canonicalHandle)
      try validate(record)
      guard envelopeDigest.count == 32,
        !receiptId.isEmpty,
        receiptId.utf8.count <= 128
      else {
        throw NotedSecurityError.invalidArguments("invalid receipt binding")
      }
      try UUIDv7Generator.validate(receiptId)
      try BootstrapContractV1.validate(metadata: metadata)
      let expectedInfo = try BootstrapContractV1.info(metadata: metadata)
      let expectedAssociatedData = try BootstrapContractV1.associatedData(metadata: metadata)
      let expectedEnvelopeDigest = try BootstrapContractV1.envelopeDigest(
        protocolName: metadata.protocolName,
        receiptId: receiptId,
        metadata: metadata,
        encapsulatedKey: encapsulatedKey,
        ciphertext: ciphertext)
      guard metadata.receiptId == receiptId,
        info == expectedInfo,
        associatedData == expectedAssociatedData,
        ciphertext.count == BootstrapContractV1.keyPackageCiphertextByteCount,
        envelopeDigest == expectedEnvelopeDigest
      else {
        throw NotedSecurityError.invalidArguments("invalid bootstrap contract")
      }

      if let existing = record.pendingBootstrap {
        guard existing.receiptId == receiptId,
          existing.envelopeDigest == envelopeDigest,
          existing.metadata == Optional(metadata)
        else {
          throw NotedSecurityError.bootstrapReplayMismatch
        }
        return StagedBootstrapDescriptor(
          pendingBootstrapHandle: existing.handle,
          metadata: metadata)
      }

      let opened = try AppleCrypto.openAuthenticatedHpke(
        record: record,
        senderPublicKey: senderPublicKey,
        info: info,
        associatedData: associatedData,
        encapsulatedKey: encapsulatedKey,
        ciphertext: ciphertext,
        exporterContext: nil)
      try BootstrapContractV1.validateKeyPackage(opened.plaintext, metadata: metadata)
      let pending = try IdentityLifecycleMachine.stage(
        record: &record,
        bootstrapHandle: UUID().uuidString.lowercased(),
        receiptId: receiptId,
        envelopeDigest: envelopeDigest,
        material: opened.plaintext,
        metadata: metadata)
      try validate(record)
      try store.replace(record)
      guard let stagedMetadata = pending.metadata else {
        throw NotedSecurityError.legacyBootstrapRequiresDiscard
      }
      return StagedBootstrapDescriptor(
        pendingBootstrapHandle: pending.handle,
        metadata: stagedMetadata)
    }
  }

  public func activateBootstrap(
    identityHandle: String,
    pendingBootstrapHandle: String,
    receiptId: String
  ) throws -> PublicIdentityDescriptor {
    try queue.sync {
      var record = try store.load(handle: try validatedHandle(identityHandle))
      try validate(record)
      try UUIDv7Generator.validate(receiptId)
      try IdentityLifecycleMachine.activate(
        record: &record,
        bootstrapHandle: try validatedHandle(pendingBootstrapHandle),
        receiptId: receiptId,
        activatedAtMs: now())
      try store.replace(record)
      return record.publicDescriptor()
    }
  }

  public func discardPending(
    identityHandle: String,
    pendingBootstrapHandle: String?,
    receiptId: String?
  ) throws -> PublicIdentityDescriptor {
    try queue.sync {
      var record = try store.load(handle: try validatedHandle(identityHandle))
      try validate(record, allowLegacyPendingBootstrap: true)
      let pendingHandle = try pendingBootstrapHandle.map(validatedHandle)
      try IdentityLifecycleMachine.discardPending(
        record: &record,
        bootstrapHandle: pendingHandle,
        receiptId: receiptId)
      try store.replace(record)
      return record.publicDescriptor()
    }
  }

  /// Applies only authority-authenticated revocation evidence already committed
  /// by the Rust store. The exact active bootstrap binding must match before all
  /// native secret material is removed in one Keychain item update.
  public func revokeActive(
    identityHandle: String,
    receiptId: String,
    authorityGeneration: UInt64,
    purgeGeneration: UInt64,
    keyEpoch: UInt64
  ) throws -> PublicIdentityDescriptor {
    try queue.sync {
      var record = try store.load(handle: try validatedHandle(identityHandle))
      try validate(record)
      try UUIDv7Generator.validate(receiptId)
      try IdentityLifecycleMachine.revokeActive(
        record: &record,
        receiptId: receiptId,
        authorityGeneration: authorityGeneration,
        purgeGeneration: purgeGeneration,
        keyEpoch: keyEpoch)
      try validate(record)
      try store.replace(record)
      return record.publicDescriptor()
    }
  }

  private func validatedHandle(_ handle: String) throws -> String {
    guard handle.count == 36,
      handle == handle.lowercased(),
      let uuid = UUID(uuidString: handle),
      uuid.uuidString.lowercased() == handle
    else {
      throw NotedSecurityError.invalidArguments("invalid opaque handle")
    }
    return handle
  }

  private func validate(
    _ record: IdentityRecord,
    allowLegacyPendingBootstrap: Bool = false
  ) throws {
    guard record.version == 1,
      record.handle == record.handle.lowercased(),
      UUID(uuidString: record.handle)?.uuidString.lowercased() == record.handle,
      record.signingPublicKey.count == 65,
      record.signingPublicKey.first == 0x04,
      record.agreementPublicKey.count == 32
    else {
      throw NotedSecurityError.identityCorrupted("record shape")
    }
    try IdentityBootstrapValidator.validate(
      record, allowLegacyPendingBootstrap: allowLegacyPendingBootstrap)
    switch record.lifecycle {
    case .pending:
      guard record.signingKeyRepresentation != nil,
        record.agreementPrivateKey?.count == 32,
        record.activeBootstrap == nil,
        record.activatedAtMs == nil
      else {
        throw NotedSecurityError.identityCorrupted("invalid pending lifecycle fields")
      }
      try keyPairValidator(record)
    case .active:
      guard record.signingKeyRepresentation != nil,
        record.agreementPrivateKey?.count == 32,
        record.pendingBootstrap == nil,
        record.activeBootstrap != nil,
        record.activatedAtMs != nil
      else {
        throw NotedSecurityError.identityCorrupted("invalid active lifecycle fields")
      }
      try keyPairValidator(record)
    case .discarded:
      guard record.signingKeyRepresentation == nil,
        record.agreementPrivateKey == nil,
        record.pendingBootstrap == nil,
        record.activeBootstrap == nil
      else {
        throw NotedSecurityError.identityCorrupted("discard tombstone contains secret material")
      }
    }
  }
}

protocol IdentityRecordStore: Sendable {
  func add(_ record: IdentityRecord) throws
  func load(handle: String) throws -> IdentityRecord
  func loadAll() throws -> [IdentityRecord]
  func replace(_ record: IdentityRecord) throws
}

/// Keeps the one legacy recovery exception narrow and independently testable:
/// only a metadata-less bootstrap on a still-pending identity may be listed so
/// the caller can discard the entire identity. Active custody and every
/// metadata-bearing bootstrap always use the complete authenticated contract.
enum IdentityBootstrapValidator {
  static func validate(
    _ record: IdentityRecord,
    allowLegacyPendingBootstrap: Bool = false
  ) throws {
    if let pending = record.pendingBootstrap {
      try validate(
        pending,
        deviceId: record.deviceId,
        allowLegacyMetadata: allowLegacyPendingBootstrap && record.lifecycle == .pending)
    }
    if let active = record.activeBootstrap {
      try validate(active, deviceId: record.deviceId, allowLegacyMetadata: false)
    }
  }

  private static func validate(
    _ bootstrap: StagedBootstrap,
    deviceId: String,
    allowLegacyMetadata: Bool
  ) throws {
    guard let metadata = bootstrap.metadata else {
      if allowLegacyMetadata { return }
      throw NotedSecurityError.legacyBootstrapRequiresDiscard
    }
    guard bootstrap.envelopeDigest.count == 32,
      bootstrap.receiptId == metadata.receiptId,
      metadata.deviceId == deviceId
    else {
      throw NotedSecurityError.identityCorrupted("bootstrap binding")
    }
    try BootstrapContractV1.validate(metadata: metadata)
    try BootstrapContractV1.validateKeyPackage(
      bootstrap.material, metadata: metadata)
  }
}

struct KeychainIdentityStore: IdentityRecordStore, Sendable {
  let accessGroup: String?

  func add(_ record: IdentityRecord) throws {
    let data = try encoded(record)
    var query = baseQuery(handle: record.handle)
    query[kSecValueData as String] = data
    query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly
    let status = SecItemAdd(query as CFDictionary, nil)
    guard status == errSecSuccess else {
      throw NotedSecurityError.fromKeychainStatus(status)
    }
  }

  func load(handle: String) throws -> IdentityRecord {
    var query = baseQuery(handle: handle)
    query[kSecReturnData as String] = true
    query[kSecReturnAttributes as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    query[kSecUseAuthenticationContext as String] = nonInteractiveContext()
    var output: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &output)
    if status == errSecItemNotFound {
      throw NotedSecurityError.identityNotFound
    }
    guard status == errSecSuccess else {
      throw NotedSecurityError.fromKeychainStatus(status)
    }
    guard let attributes = output as? [String: Any] else {
      throw NotedSecurityError.identityCorrupted("missing Keychain attributes")
    }
    return try decode(attributes)
  }

  func loadAll() throws -> [IdentityRecord] {
    var query = baseQuery(handle: nil)
    query[kSecReturnData as String] = true
    query[kSecReturnAttributes as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitAll
    query[kSecUseAuthenticationContext as String] = nonInteractiveContext()
    var output: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &output)
    if status == errSecItemNotFound {
      return []
    }
    guard status == errSecSuccess else {
      throw NotedSecurityError.fromKeychainStatus(status)
    }
    guard let items = output as? [[String: Any]] else {
      throw NotedSecurityError.identityCorrupted("invalid Keychain inventory")
    }
    return try items.map(decode)
  }

  func replace(_ record: IdentityRecord) throws {
    let data = try encoded(record)
    let status = SecItemUpdate(
      baseQuery(handle: record.handle) as CFDictionary,
      [kSecValueData as String: data] as CFDictionary)
    if status == errSecItemNotFound {
      throw NotedSecurityError.identityNotFound
    }
    guard status == errSecSuccess else {
      throw NotedSecurityError.fromKeychainStatus(status)
    }
  }

  private func encoded(_ record: IdentityRecord) throws -> Data {
    do {
      return try JSONEncoder().encode(record)
    } catch {
      throw NotedSecurityError.identityCorrupted("record encoding")
    }
  }

  private func decode(_ attributes: [String: Any]) throws -> IdentityRecord {
    guard let data = attributes[kSecValueData as String] as? Data else {
      throw NotedSecurityError.identityCorrupted("missing Keychain data")
    }
    guard
      attributes[kSecAttrAccessible as String] as? String
        == kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly as String
    else {
      throw NotedSecurityError.identityCorrupted("unexpected Keychain accessibility")
    }
    if let synchronizable = attributes[kSecAttrSynchronizable as String] as? Bool,
      synchronizable
    {
      throw NotedSecurityError.identityCorrupted("synchronizable Keychain item")
    }
    do {
      return try JSONDecoder().decode(IdentityRecord.self, from: data)
    } catch {
      throw NotedSecurityError.identityCorrupted("record decoding")
    }
  }

  private func baseQuery(handle: String?) -> [String: Any] {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: identityService,
      kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
      kSecUseDataProtectionKeychain as String: true,
    ]
    if let handle {
      query[kSecAttrAccount as String] = handle
    }
    if let accessGroup {
      query[kSecAttrAccessGroup as String] = accessGroup
    }
    return query
  }

  private func nonInteractiveContext() -> LAContext {
    let context = LAContext()
    context.interactionNotAllowed = true
    return context
  }
}
