import CryptoKit
import Foundation
import Security

struct GeneratedIdentityMaterial: Sendable {
  let backing: SigningKeyBacking
  let signingKeyRepresentation: Data
  let agreementPrivateKey: Data
  let signingPublicKey: Data
  let agreementPublicKey: Data
}

public struct AuthenticatedHpkeOpen: Sendable {
  public let plaintext: Data
  public let exporterSecret: Data?
}

@available(iOS 17.0, macOS 14.0, *)
public enum AppleCrypto {
  static let hpkeSuite = HPKE.Ciphersuite(
    kem: .Curve25519_HKDF_SHA256,
    kdf: .HKDF_SHA256,
    aead: .AES_GCM_256)

  static func generateIdentity(fixtureGate: String?) throws -> GeneratedIdentityMaterial {
    let agreement = Curve25519.KeyAgreement.PrivateKey()

    #if targetEnvironment(simulator)
      guard
        FixturePolicy.softwareSigningAllowed(
          isDebug: _isDebugAssertConfiguration(),
          isSimulator: true,
          gate: fixtureGate)
      else {
        throw NotedSecurityError.fixtureGateRejected
      }
      let signing = P256.Signing.PrivateKey()
      return GeneratedIdentityMaterial(
        backing: .softwareFixture,
        signingKeyRepresentation: signing.rawRepresentation,
        agreementPrivateKey: agreement.rawRepresentation,
        signingPublicKey: signing.publicKey.x963Representation,
        agreementPublicKey: agreement.publicKey.rawRepresentation)
    #else
      guard fixtureGate == nil else {
        throw NotedSecurityError.fixtureGateRejected
      }
      guard SecureEnclave.isAvailable else {
        throw NotedSecurityError.secureEnclaveUnavailable
      }
      var accessError: Unmanaged<CFError>?
      guard
        let accessControl = SecAccessControlCreateWithFlags(
          nil,
          kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
          [.privateKeyUsage],
          &accessError)
      else {
        _ = accessError?.takeRetainedValue()
        throw NotedSecurityError.passcodeRequired
      }
      do {
        let signing = try SecureEnclave.P256.Signing.PrivateKey(
          compactRepresentable: true,
          accessControl: accessControl)
        return GeneratedIdentityMaterial(
          backing: .secureEnclave,
          signingKeyRepresentation: signing.dataRepresentation,
          agreementPrivateKey: agreement.rawRepresentation,
          signingPublicKey: signing.publicKey.x963Representation,
          agreementPublicKey: agreement.publicKey.rawRepresentation)
      } catch {
        throw mapSecureEnclaveError(error)
      }
    #endif
  }

  static func sign(record: IdentityRecord, message: Data) throws -> Data {
    guard record.lifecycle != .discarded,
      let representation = record.signingKeyRepresentation
    else {
      throw NotedSecurityError.invalidIdentityState(
        expected: "pending_or_active",
        actual: record.lifecycle.rawValue)
    }
    guard message.count <= 256 * 1024 else {
      throw NotedSecurityError.invalidArguments("signing message exceeds 256 KiB")
    }
    do {
      switch record.signingKeyBacking {
      case .secureEnclave:
        let key = try SecureEnclave.P256.Signing.PrivateKey(
          dataRepresentation: representation)
        return try key.signature(for: message).rawRepresentation
      case .softwareFixture:
        guard FixturePolicy.softwareSigningAllowedInCurrentProcess else {
          throw NotedSecurityError.fixtureGateRejected
        }
        let key = try P256.Signing.PrivateKey(rawRepresentation: representation)
        return try key.signature(for: message).rawRepresentation
      }
    } catch let error as NotedSecurityError {
      throw error
    } catch {
      throw NotedSecurityError.signingFailed
    }
  }

  /// Verification stays in CryptoKit so Rust never becomes a second parser
  /// for Apple's P-256 wire formats.
  public static func verifyP256Signature(
    publicKey: Data,
    message: Data,
    signature: Data
  ) throws -> Bool {
    guard publicKey.count == 65, publicKey.first == 0x04, signature.count == 64,
      message.count <= 256 * 1024
    else {
      throw NotedSecurityError.invalidArguments("invalid P-256 verification request")
    }
    do {
      let key = try P256.Signing.PublicKey(x963Representation: publicKey)
      let value = try P256.Signing.ECDSASignature(rawRepresentation: signature)
      return key.isValidSignature(value, for: message)
    } catch {
      throw NotedSecurityError.invalidArguments("invalid P-256 verification encoding")
    }
  }

  public static func secureRandomBytes(count: Int) throws -> Data {
    guard (1...64).contains(count) else {
      throw NotedSecurityError.invalidArguments("random byte count must be between 1 and 64")
    }
    var bytes = [UInt8](repeating: 0, count: count)
    guard SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess else {
      throw NotedSecurityError.entropyUnavailable
    }
    return Data(bytes)
  }

  public static func freshUUIDv7(unixMilliseconds: Int64) throws -> String {
    try UUIDv7Generator.generate(unixMilliseconds: unixMilliseconds)
  }

  static func validateKeyPair(record: IdentityRecord) throws {
    guard record.lifecycle != .discarded,
      let signingRepresentation = record.signingKeyRepresentation,
      let agreementRepresentation = record.agreementPrivateKey
    else {
      throw NotedSecurityError.identityCorrupted("missing private key representation")
    }
    do {
      let agreement = try Curve25519.KeyAgreement.PrivateKey(
        rawRepresentation: agreementRepresentation)
      guard agreement.publicKey.rawRepresentation == record.agreementPublicKey else {
        throw NotedSecurityError.identityCorrupted("X25519 public key mismatch")
      }
      let signingPublicKey: Data
      switch record.signingKeyBacking {
      case .secureEnclave:
        let signing = try SecureEnclave.P256.Signing.PrivateKey(
          dataRepresentation: signingRepresentation)
        signingPublicKey = signing.publicKey.x963Representation
      case .softwareFixture:
        guard FixturePolicy.softwareSigningAllowedInCurrentProcess else {
          throw NotedSecurityError.fixtureGateRejected
        }
        let signing = try P256.Signing.PrivateKey(rawRepresentation: signingRepresentation)
        signingPublicKey = signing.publicKey.x963Representation
      }
      guard signingPublicKey == record.signingPublicKey else {
        throw NotedSecurityError.identityCorrupted("P-256 public key mismatch")
      }
    } catch let error as NotedSecurityError {
      throw error
    } catch {
      throw NotedSecurityError.identityCorrupted("private/public key validation failed")
    }
  }

  static func openAuthenticatedHpke(
    record: IdentityRecord,
    senderPublicKey: Data,
    info: Data,
    associatedData: Data,
    encapsulatedKey: Data,
    ciphertext: Data,
    exporterContext: Data?
  ) throws -> AuthenticatedHpkeOpen {
    guard record.lifecycle != .discarded,
      let agreementPrivateKey = record.agreementPrivateKey
    else {
      throw NotedSecurityError.invalidIdentityState(
        expected: "pending_or_active",
        actual: record.lifecycle.rawValue)
    }
    guard senderPublicKey.count == 32,
      encapsulatedKey.count == 32,
      ciphertext.count >= 16,
      ciphertext.count <= 256 * 1024,
      info.count <= 256 * 1024,
      associatedData.count <= 256 * 1024,
      (exporterContext?.count ?? 0) <= 256 * 1024
    else {
      throw NotedSecurityError.invalidArguments("invalid authenticated HPKE field size")
    }
    do {
      let privateKey = try Curve25519.KeyAgreement.PrivateKey(
        rawRepresentation: agreementPrivateKey)
      let authenticationKey = try Curve25519.KeyAgreement.PublicKey(
        rawRepresentation: senderPublicKey)
      var recipient = try HPKE.Recipient(
        privateKey: privateKey,
        ciphersuite: hpkeSuite,
        info: info,
        encapsulatedKey: encapsulatedKey,
        authenticatedBy: authenticationKey)
      let plaintext = try recipient.open(ciphertext, authenticating: associatedData)
      let exporterSecret: Data?
      if let exporterContext {
        let secret = try recipient.exportSecret(
          context: exporterContext,
          outputByteCount: 32)
        exporterSecret = secret.withUnsafeBytes { Data($0) }
      } else {
        exporterSecret = nil
      }
      return AuthenticatedHpkeOpen(
        plaintext: plaintext,
        exporterSecret: exporterSecret)
    } catch let error as NotedSecurityError {
      throw error
    } catch {
      throw NotedSecurityError.hpkeOpenFailed
    }
  }

  private static func mapSecureEnclaveError(_ error: Error) -> NotedSecurityError {
    let nsError = error as NSError
    if nsError.domain == NSOSStatusErrorDomain {
      return .fromKeychainStatus(OSStatus(nsError.code))
    }
    return .secureEnclaveUnavailable
  }
}
