import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class PolicyAndProtectionPlanTests: XCTestCase {
  func testSoftwareSigningNeedsAllThreeFixtureGates() {
    XCTAssertTrue(
      FixturePolicy.softwareSigningAllowed(
        isDebug: true,
        isSimulator: true,
        gate: FixturePolicy.exactGate))
    XCTAssertFalse(
      FixturePolicy.softwareSigningAllowed(
        isDebug: false,
        isSimulator: true,
        gate: FixturePolicy.exactGate))
    XCTAssertFalse(
      FixturePolicy.softwareSigningAllowed(
        isDebug: true,
        isSimulator: false,
        gate: FixturePolicy.exactGate))
    XCTAssertFalse(
      FixturePolicy.softwareSigningAllowed(
        isDebug: true,
        isSimulator: true,
        gate: "almost"))
  }

  func testProtectionPlanIncludesDatabaseWalShmAndRecoveryFile() throws {
    let plan = try StoreProtectionPlan.make(
      databasePath: "/fixture/Library/Application Support/noted-mobile.sqlite3",
      recoveryPaths: ["/fixture/Library/Application Support/noted-mobile.recovery.sqlite3"],
      containerRoot: "/fixture")
    XCTAssertEqual(
      plan.requiredFiles.map(\.path),
      [
        "/fixture/Library/Application Support/noted-mobile.sqlite3",
        "/fixture/Library/Application Support/noted-mobile.recovery.sqlite3",
      ])
    XCTAssertEqual(
      Set(plan.optionalSidecars.map(\.path)),
      Set([
        "/fixture/Library/Application Support/noted-mobile.sqlite3-wal",
        "/fixture/Library/Application Support/noted-mobile.sqlite3-shm",
      ]))
  }

  func testProtectionPlanRejectsTraversalOutsideContainer() {
    XCTAssertThrowsError(
      try StoreProtectionPlan.make(
        databasePath: "/fixture/../outside/noted.sqlite3",
        recoveryPaths: [],
        containerRoot: "/fixture")
    ) { error in
      guard case .pathRejected = error as? NotedSecurityError else {
        return XCTFail("expected path rejection, got \(error)")
      }
    }
  }
}
