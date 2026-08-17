import Foundation
import XCTest

@testable import NotedAppleSecurityCore

final class UUIDv7Tests: XCTestCase {
  func testDeterministicCanonicalUUIDv7Layout() throws {
    let value = try UUIDv7Generator.generate(
      unixMilliseconds: 1_725_000_000_000,
      randomBytes: Array(0x10...0x19))
    XCTAssertEqual(value, "0191a203-2200-7011-9213-141516171819")
    XCTAssertEqual(value[value.index(value.startIndex, offsetBy: 14)], "7")
    XCTAssertTrue("89ab".contains(value[value.index(value.startIndex, offsetBy: 19)]))
    XCTAssertEqual(UUID(uuidString: value)?.uuidString.lowercased(), value)
  }

  func testRejectsTimestampOutsideFortyEightBits() {
    XCTAssertThrowsError(
      try UUIDv7Generator.generate(
        unixMilliseconds: 1 << 48,
        randomBytes: [UInt8](repeating: 0, count: 10)))
  }

  func testDeviceIdentityBindingRequiresCanonicalUUIDv7() throws {
    try UUIDv7Generator.validate("018f47a0-7b80-7000-8000-000000000006")
    XCTAssertThrowsError(
      try UUIDv7Generator.validate("018f47a0-7b80-4000-8000-000000000006"))
    XCTAssertThrowsError(
      try UUIDv7Generator.validate("018F47A0-7B80-7000-8000-000000000006"))
  }
}
