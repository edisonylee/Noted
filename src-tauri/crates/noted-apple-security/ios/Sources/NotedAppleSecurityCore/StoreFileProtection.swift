import Foundation

public struct StoreProtectionReport: Codable, Equatable, Sendable {
  public let protectionClass: String
  public let hardenedPaths: [String]
  public let inheritedPendingPaths: [String]
  public let violations: [String]
}

public struct StoreProtectionPlan: Equatable, Sendable {
  public let directories: [URL]
  public let requiredFiles: [URL]
  public let optionalSidecars: [URL]

  public static func make(
    databasePath: String,
    recoveryPaths: [String],
    containerRoot: String = NSHomeDirectory()
  ) throws -> StoreProtectionPlan {
    let root = URL(fileURLWithPath: containerRoot, isDirectory: true).standardizedFileURL
    let database = try acceptedFile(databasePath, under: root)
    let recoveries = try recoveryPaths.map { try acceptedFile($0, under: root) }
    let required = unique([database] + recoveries)
    let sidecars = [
      URL(fileURLWithPath: database.path + "-wal"),
      URL(fileURLWithPath: database.path + "-shm"),
    ]
    let directories = unique(required.map { $0.deletingLastPathComponent() })
    return StoreProtectionPlan(
      directories: directories,
      requiredFiles: required,
      optionalSidecars: sidecars)
  }

  private static func acceptedFile(_ path: String, under root: URL) throws -> URL {
    guard path.hasPrefix("/") else {
      throw NotedSecurityError.invalidArguments("store paths must be absolute")
    }
    let file = URL(fileURLWithPath: path).standardizedFileURL
    let rootPath = root.path.hasSuffix("/") ? root.path : root.path + "/"
    guard file.path.hasPrefix(rootPath), file.path != root.path else {
      throw NotedSecurityError.pathRejected(path)
    }
    return file
  }

  private static func unique(_ urls: [URL]) -> [URL] {
    var seen = Set<String>()
    return urls.filter { seen.insert($0.path).inserted }
  }
}

public enum StoreFileProtection {
  public static func prepareDirectory(plan: StoreProtectionPlan) throws -> StoreProtectionReport {
    let manager = FileManager.default
    var hardened: [String] = []
    for directory in plan.directories {
      var isDirectory: ObjCBool = false
      guard manager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
        isDirectory.boolValue
      else {
        throw NotedSecurityError.fileProtectionFailed(directory.path)
      }
      try apply(to: directory)
      hardened.append(directory.path)
    }
    let absent = (plan.requiredFiles + plan.optionalSidecars)
      .filter { !manager.fileExists(atPath: $0.path) }
      .map(\.path)
      .sorted()
    return StoreProtectionReport(
      protectionClass: "NSFileProtectionComplete",
      hardenedPaths: hardened.sorted(),
      inheritedPendingPaths: absent,
      violations: [])
  }

  public static func harden(plan: StoreProtectionPlan) throws -> StoreProtectionReport {
    let manager = FileManager.default
    var hardened: [String] = []
    var pending: [String] = []

    for directory in plan.directories {
      var isDirectory: ObjCBool = false
      guard manager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
        isDirectory.boolValue
      else {
        throw NotedSecurityError.fileProtectionFailed(directory.path)
      }
      try apply(to: directory)
      hardened.append(directory.path)
    }
    for file in plan.requiredFiles {
      guard manager.fileExists(atPath: file.path) else {
        throw NotedSecurityError.fileProtectionFailed(file.path)
      }
      try apply(to: file)
      hardened.append(file.path)
    }
    for sidecar in plan.optionalSidecars {
      if manager.fileExists(atPath: sidecar.path) {
        try apply(to: sidecar)
        hardened.append(sidecar.path)
      } else {
        // The pre-open directory step reduces the creation-time window, but is
        // not accepted as final proof. Callers must re-run this method after
        // SQLite opens so every live WAL/SHM sidecar is explicitly inspected.
        pending.append(sidecar.path)
      }
    }
    let verification = try verify(plan: plan)
    return StoreProtectionReport(
      protectionClass: verification.protectionClass,
      hardenedPaths: hardened.sorted(),
      inheritedPendingPaths: pending.sorted(),
      violations: verification.violations)
  }

  public static func verify(plan: StoreProtectionPlan) throws -> StoreProtectionReport {
    let manager = FileManager.default
    var checked: [String] = []
    var pending: [String] = []
    var violations: [String] = []
    let required = plan.directories + plan.requiredFiles
    for url in required {
      guard manager.fileExists(atPath: url.path) else {
        violations.append("missing:\(url.path)")
        continue
      }
      checked.append(url.path)
      try inspect(url: url, violations: &violations)
    }
    for url in plan.optionalSidecars {
      if manager.fileExists(atPath: url.path) {
        checked.append(url.path)
        try inspect(url: url, violations: &violations)
      } else {
        pending.append(url.path)
      }
    }
    return StoreProtectionReport(
      protectionClass: "NSFileProtectionComplete",
      hardenedPaths: checked.sorted(),
      inheritedPendingPaths: pending.sorted(),
      violations: violations.sorted())
  }

  private static func apply(to url: URL) throws {
    do {
      try FileManager.default.setAttributes(
        [.protectionKey: FileProtectionType.complete],
        ofItemAtPath: url.path)
    } catch {
      throw NotedSecurityError.fileProtectionFailed(url.path)
    }
    do {
      var mutableURL = url
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try mutableURL.setResourceValues(values)
    } catch {
      throw NotedSecurityError.backupExclusionFailed(url.path)
    }
  }

  private static func inspect(url: URL, violations: inout [String]) throws {
    let attributes: [FileAttributeKey: Any]
    do {
      attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    } catch {
      throw NotedSecurityError.fileProtectionFailed(url.path)
    }
    #if !targetEnvironment(simulator)
      guard attributes[.protectionKey] as? FileProtectionType == .complete else {
        violations.append("protection:\(url.path)")
        return
      }
    #endif
    do {
      let values = try url.resourceValues(forKeys: [.isExcludedFromBackupKey])
      if values.isExcludedFromBackup != true {
        violations.append("backup:\(url.path)")
      }
    } catch {
      throw NotedSecurityError.backupExclusionFailed(url.path)
    }
  }
}
