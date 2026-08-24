import XCTest
@testable import NotedAppleSecurityCore

final class PrivateLanEndpointHintTests: XCTestCase {
  func testAcceptsOnlyVersionedPrivateNumericIPv4Hints() {
    XCTAssertEqual(
      PrivateLanEndpointHintParser.parse(txt: [
        "protocol": "noted.direct-sync.v1",
        "address": "192.168.1.8",
        "port": "43123",
      ]),
      PrivateLanEndpointHint(address: "192.168.1.8:43123"))

    for address in ["example.com", "127.0.0.1", "8.8.8.8", "192.168.001.8"] {
      XCTAssertNil(
        PrivateLanEndpointHintParser.parse(txt: [
          "protocol": "noted.direct-sync.v1",
          "address": address,
          "port": "43123",
        ]))
    }
    XCTAssertNil(
      PrivateLanEndpointHintParser.parse(txt: [
        "protocol": "noted.direct-sync.v0",
        "address": "10.0.0.8",
        "port": "43123",
      ]))
    XCTAssertNil(
      PrivateLanEndpointHintParser.parse(txt: [
        "protocol": "noted.direct-sync.v1",
        "address": "10.0.0.8",
        "port": "0",
      ]))
  }

  func testDeduplicatesSortsAndBoundsCandidates() {
    let inputs = (1...20).reversed().map {
      PrivateLanEndpointHint(address: "10.0.0.\($0):43123")
    } + [PrivateLanEndpointHint(address: "10.0.0.1:43123")]
    let result = PrivateLanEndpointHintParser.uniqueBounded(inputs)

    XCTAssertEqual(result.count, PrivateLanEndpointHintParser.maximumCandidates)
    XCTAssertEqual(Set(result.map(\.address)).count, result.count)
    XCTAssertEqual(result, result.sorted { $0.address < $1.address })
  }
}
