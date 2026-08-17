import Foundation

enum FixturePolicy {
  static let exactGate = "sanitized-development-fixture-v1"

  static func softwareSigningAllowed(
    isDebug: Bool,
    isSimulator: Bool,
    gate: String?
  ) -> Bool {
    isDebug && isSimulator && gate == exactGate
  }

  static var softwareSigningAllowedInCurrentProcess: Bool {
    #if DEBUG && targetEnvironment(simulator)
      true
    #else
      false
    #endif
  }
}
