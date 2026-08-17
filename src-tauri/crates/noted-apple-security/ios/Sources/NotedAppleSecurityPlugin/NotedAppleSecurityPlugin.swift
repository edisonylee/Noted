#if os(iOS)
  import Foundation
  import NotedAppleSecurityCore
  import Tauri
  import UIKit
  import WebKit

  private let maximumFieldBytes = 256 * 1024
  private let maximumBootstrapBindingBytes = 16 * 1024
  private let bootstrapKeyPackageCiphertextBytes = 64

  private struct PrepareIdentityArgs: Decodable {
    let deviceId: String
    let fixtureGate: String?
  }

  private struct IdentityArgs: Decodable {
    let handle: String
  }

  private struct SignArgs: Decodable {
    let handle: String
    let messageBase64: String
  }

  private struct SignatureResponse: Encodable {
    let signatureBase64: String
  }

  private struct VerifySignatureArgs: Decodable {
    let publicKeyBase64: String
    let messageBase64: String
    let signatureBase64: String
  }

  private struct VerificationResponse: Encodable {
    let valid: Bool
  }

  private struct FreshBytesArgs: Decodable {
    let length: Int
  }

  private struct FreshBytesResponse: Encodable {
    let bytesBase64: String
  }

  private struct FreshUUIDv7Response: Encodable {
    let value: String
  }

  private struct OpenHpkeArgs: Decodable {
    let handle: String
    let senderPublicKeyBase64: String
    let infoBase64: String
    let associatedDataBase64: String
    let encapsulatedKeyBase64: String
    let ciphertextBase64: String
    let exporterContextBase64: String
  }

  private struct OpenHpkeResponse: Encodable {
    let plaintextBase64: String
    let exporterSecretBase64: String
  }

  private struct StageBootstrapArgs: Decodable {
    let handle: String
    let senderPublicKeyBase64: String
    let infoBase64: String
    let associatedDataBase64: String
    let encapsulatedKeyBase64: String
    let ciphertextBase64: String
    let receiptId: String
    let envelopeDigestBase64: String
    let metadata: BootstrapMetadataV1
  }

  private struct BootstrapTransitionArgs: Decodable {
    let identityHandle: String
    let pendingBootstrapHandle: String
    let receiptId: String
  }

  private struct DiscardPendingArgs: Decodable {
    let identityHandle: String
    let pendingBootstrapHandle: String?
    let receiptId: String?
  }

  private struct SubscribeProtectedDataArgs: Decodable {
    let handler: Channel
  }

  private struct ProtectedDataStateResponse: Encodable {
    let state: ProtectedDataState
  }

  private struct HardenStoreArgs: Decodable {
    let databasePath: String
    let recoveryPaths: [String]
  }

  @available(iOS 17.0, *)
  final class NotedAppleSecurityPlugin: Plugin {
    private let vault = IdentityVault()
    private let protectedData = ProtectedDataMonitor()
    private var protectedDataChannel: Channel?

    @objc func prepareIdentity(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(PrepareIdentityArgs.self)
        return try self.vault.prepareIdentity(
          deviceId: args.deviceId,
          fixtureGate: args.fixtureGate)
      }
    }

    @objc func getIdentity(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(IdentityArgs.self)
        return try self.vault.identity(handle: args.handle)
      }
    }

    @objc func listIdentities(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        try self.vault.inventory()
      }
    }

    @objc func sign(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(SignArgs.self)
        let message = try decodeBase64(args.messageBase64, maximum: maximumFieldBytes)
        let signature = try self.vault.sign(handle: args.handle, message: message)
        guard signature.count == 64 else {
          throw NotedSecurityError.signingFailed
        }
        return SignatureResponse(signatureBase64: signature.base64EncodedString())
      }
    }

    @objc func verifyP256Signature(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(VerifySignatureArgs.self)
        return VerificationResponse(
          valid: try AppleCrypto.verifyP256Signature(
            publicKey: try decodeBase64(args.publicKeyBase64, exact: 65),
            message: try decodeBase64(args.messageBase64, maximum: maximumFieldBytes),
            signature: try decodeBase64(args.signatureBase64, exact: 64)))
      }
    }

    @objc func freshBytes(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(FreshBytesArgs.self)
        return FreshBytesResponse(
          bytesBase64: try AppleCrypto.secureRandomBytes(count: args.length).base64EncodedString())
      }
    }

    @objc func freshUUIDv7(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        FreshUUIDv7Response(
          value: try AppleCrypto.freshUUIDv7(
            unixMilliseconds: Int64((Date().timeIntervalSince1970 * 1_000).rounded())))
      }
    }

    @objc func openAuthenticatedHpke(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(OpenHpkeArgs.self)
        let opened = try self.vault.openAuthenticatedHpke(
          handle: args.handle,
          senderPublicKey: try decodeBase64(args.senderPublicKeyBase64, exact: 32),
          info: try decodeBase64(args.infoBase64, maximum: maximumFieldBytes),
          associatedData: try decodeBase64(args.associatedDataBase64, maximum: maximumFieldBytes),
          encapsulatedKey: try decodeBase64(args.encapsulatedKeyBase64, exact: 32),
          ciphertext: try decodeBase64(args.ciphertextBase64, maximum: maximumFieldBytes),
          exporterContext: try decodeBase64(args.exporterContextBase64, maximum: maximumFieldBytes))
        guard let exporter = opened.exporterSecret, exporter.count == 32 else {
          throw NotedSecurityError.hpkeOpenFailed
        }
        return OpenHpkeResponse(
          plaintextBase64: opened.plaintext.base64EncodedString(),
          exporterSecretBase64: exporter.base64EncodedString())
      }
    }

    @objc func stageBootstrapAuthenticated(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(StageBootstrapArgs.self)
        return try self.vault.stageBootstrapAuthenticated(
          handle: args.handle,
          senderPublicKey: try decodeBase64(args.senderPublicKeyBase64, exact: 32),
          info: try decodeBase64(
            args.infoBase64, maximum: maximumBootstrapBindingBytes),
          associatedData: try decodeBase64(
            args.associatedDataBase64, maximum: maximumBootstrapBindingBytes),
          encapsulatedKey: try decodeBase64(args.encapsulatedKeyBase64, exact: 32),
          ciphertext: try decodeBase64(
            args.ciphertextBase64, exact: bootstrapKeyPackageCiphertextBytes),
          receiptId: args.receiptId,
          envelopeDigest: try decodeBase64(args.envelopeDigestBase64, exact: 32),
          metadata: args.metadata)
      }
    }

    @objc func activateBootstrap(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(BootstrapTransitionArgs.self)
        return try self.vault.activateBootstrap(
          identityHandle: args.identityHandle,
          pendingBootstrapHandle: args.pendingBootstrapHandle,
          receiptId: args.receiptId)
      }
    }

    @objc func discardPending(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(DiscardPendingArgs.self)
        return try self.vault.discardPending(
          identityHandle: args.identityHandle,
          pendingBootstrapHandle: args.pendingBootstrapHandle,
          receiptId: args.receiptId)
      }
    }

    @objc func protectedDataState(_ invoke: Invoke) {
      invoke.resolve(ProtectedDataStateResponse(state: protectedData.currentState))
    }

    @objc func subscribeProtectedData(_ invoke: Invoke) {
      do {
        let args = try invoke.parseArgs(SubscribeProtectedDataArgs.self)
        protectedDataChannel = args.handler
        protectedData.subscribe { [weak self] event in
          guard let channel = self?.protectedDataChannel else { return }
          try? channel.send(event)
        }
        invoke.resolve()
      } catch {
        reject(invoke, error)
      }
    }

    @objc func hardenStoreFiles(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(HardenStoreArgs.self)
        let plan = try StoreProtectionPlan.make(
          databasePath: args.databasePath,
          recoveryPaths: args.recoveryPaths)
        return try StoreFileProtection.harden(plan: plan)
      }
    }

    @objc func prepareStoreDirectory(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(HardenStoreArgs.self)
        let plan = try StoreProtectionPlan.make(
          databasePath: args.databasePath,
          recoveryPaths: args.recoveryPaths)
        return try StoreFileProtection.prepareDirectory(plan: plan)
      }
    }

    @objc func verifyStoreFiles(_ invoke: Invoke) {
      resolveInvocation(invoke) {
        let args = try invoke.parseArgs(HardenStoreArgs.self)
        let plan = try StoreProtectionPlan.make(
          databasePath: args.databasePath,
          recoveryPaths: args.recoveryPaths)
        return try StoreFileProtection.verify(plan: plan)
      }
    }
  }

  private func resolveInvocation<T: Encodable>(_ invoke: Invoke, _ body: () throws -> T) {
    do {
      invoke.resolve(try body())
    } catch {
      reject(invoke, error)
    }
  }

  private func reject(_ invoke: Invoke, _ error: Error) {
    if let securityError = error as? NotedSecurityError {
      invoke.reject(securityError.message, code: securityError.code)
    } else if error is DecodingError {
      invoke.reject("Invalid native command arguments", code: "invalid_arguments")
    } else {
      invoke.reject("Native Apple security operation failed", code: "keychain_failure")
    }
  }

  private func decodeBase64(_ value: String, exact: Int) throws -> Data {
    let data = try decodeBase64(value, maximum: exact)
    guard data.count == exact else {
      throw NotedSecurityError.invalidArguments("invalid binary field length")
    }
    return data
  }

  private func decodeBase64(_ value: String, maximum: Int) throws -> Data {
    guard value.utf8.count <= ((maximum + 2) / 3) * 4 + 4,
      let data = Data(base64Encoded: value),
      data.count <= maximum
    else {
      throw NotedSecurityError.invalidArguments("invalid base64 field")
    }
    return data
  }

  @_cdecl("init_plugin_noted_apple_security")
  func initPlugin() -> Plugin {
    if #available(iOS 17.0, *) {
      return NotedAppleSecurityPlugin()
    }
    fatalError("Noted Apple security requires iOS 17 or newer")
  }
#endif
