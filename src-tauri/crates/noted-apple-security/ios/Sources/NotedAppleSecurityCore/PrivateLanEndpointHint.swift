import Foundation

public struct PrivateLanEndpointHint: Codable, Equatable, Sendable {
  public let address: String

  public init(address: String) {
    self.address = address
  }
}

/// Reduces untrusted Bonjour TXT metadata to a bounded numeric IPv4 socket
/// hint. Authentication never depends on this value: Rust validates it again
/// and uses the SPKI pin from the durable pairing activation.
public enum PrivateLanEndpointHintParser {
  public static let protocolVersion = "noted.direct-sync.v1"
  public static let maximumCandidates = 16

  public static func parse(txt: [String: String]) -> PrivateLanEndpointHint? {
    guard txt.count <= 8,
      txt["protocol"] == protocolVersion,
      let host = txt["address"],
      let portText = txt["port"],
      host.utf8.count <= 15,
      portText.utf8.count <= 5,
      let port = UInt16(portText),
      port != 0,
      isPrivateIPv4(host)
    else {
      return nil
    }
    return PrivateLanEndpointHint(address: "\(host):\(port)")
  }

  public static func uniqueBounded(
    _ hints: some Sequence<PrivateLanEndpointHint>
  ) -> [PrivateLanEndpointHint] {
    var seen = Set<String>()
    var result: [PrivateLanEndpointHint] = []
    for hint in hints where seen.insert(hint.address).inserted {
      guard result.count < maximumCandidates else { break }
      result.append(hint)
    }
    return result.sorted { $0.address < $1.address }
  }

  private static func isPrivateIPv4(_ value: String) -> Bool {
    let parts = value.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count == 4 else { return false }
    var octets: [UInt8] = []
    for part in parts {
      guard !part.isEmpty,
        part.count <= 3,
        part == "0" || part.first != "0",
        let octet = UInt8(part)
      else {
        return false
      }
      octets.append(octet)
    }
    return octets[0] == 10
      || (octets[0] == 172 && (16...31).contains(octets[1]))
      || (octets[0] == 192 && octets[1] == 168)
      || (octets[0] == 169 && octets[1] == 254)
  }
}
