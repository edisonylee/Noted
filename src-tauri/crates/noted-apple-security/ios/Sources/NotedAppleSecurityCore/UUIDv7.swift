import Foundation
import Security

enum UUIDv7Generator {
  static func validate(_ value: String) throws {
    let characters = Array(value.utf8)
    guard characters.count == 36,
      value == value.lowercased(),
      UUID(uuidString: value)?.uuidString.lowercased() == value,
      characters[14] == 0x37,
      [UInt8(0x38), 0x39, 0x61, 0x62].contains(characters[19])
    else {
      throw NotedSecurityError.invalidArguments("device ID must be a canonical UUIDv7")
    }
  }

  static func generate(unixMilliseconds: Int64) throws -> String {
    guard unixMilliseconds >= 0, UInt64(unixMilliseconds) < (1 << 48) else {
      throw NotedSecurityError.invalidArguments("UUIDv7 timestamp is out of range")
    }
    var random = [UInt8](repeating: 0, count: 10)
    guard SecRandomCopyBytes(kSecRandomDefault, random.count, &random) == errSecSuccess else {
      throw NotedSecurityError.entropyUnavailable
    }
    return try generate(unixMilliseconds: unixMilliseconds, randomBytes: random)
  }

  static func generate(
    unixMilliseconds: Int64,
    randomBytes: [UInt8]
  ) throws -> String {
    guard unixMilliseconds >= 0, UInt64(unixMilliseconds) < (1 << 48),
      randomBytes.count == 10
    else {
      throw NotedSecurityError.invalidArguments("invalid UUIDv7 input")
    }
    let timestamp = UInt64(unixMilliseconds)
    var bytes = [UInt8](repeating: 0, count: 16)
    for index in 0..<6 {
      bytes[index] = UInt8((timestamp >> UInt64((5 - index) * 8)) & 0xff)
    }
    for index in 0..<10 {
      bytes[index + 6] = randomBytes[index]
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x70
    bytes[8] = (bytes[8] & 0x3f) | 0x80
    let hex = bytes.map { String(format: "%02x", $0) }
    return
      hex[0...3].joined() + "-" + hex[4...5].joined() + "-" + hex[6...7].joined() + "-"
      + hex[8...9].joined() + "-" + hex[10...15].joined()
  }
}
