// swift-tools-version: 5.9

import Foundation
import PackageDescription

// Tauri's generated Swift API imports UIKit and therefore cannot be compiled
// by a host `swift test`. This explicit task-specific switch removes only the
// adapter target; ordinary Cargo/iOS builds always take the full package path.
let coreTestsOnly =
  ProcessInfo.processInfo.environment[
    "NOTED_APPLE_SECURITY_CORE_TESTS_ONLY"
  ] == "1"

var products: [Product] = [
  .library(
    name: "NotedAppleSecurityCore",
    targets: ["NotedAppleSecurityCore"])
]
var dependencies: [Package.Dependency] = []
var targets: [Target] = [
  .target(
    name: "NotedAppleSecurityCore",
    path: "Sources/NotedAppleSecurityCore"),
  .testTarget(
    name: "NotedAppleSecurityCoreTests",
    dependencies: ["NotedAppleSecurityCore"],
    path: "Tests/NotedAppleSecurityCoreTests"),
]

if !coreTestsOnly {
  products.append(
    .library(
      name: "noted-apple-security",
      type: .static,
      targets: ["NotedAppleSecurityPlugin"]))
  dependencies.append(.package(name: "Tauri", path: "../.tauri/tauri-api"))
  targets.append(
    .target(
      name: "NotedAppleSecurityPlugin",
      dependencies: [
        "NotedAppleSecurityCore",
        .byName(name: "Tauri"),
      ],
      path: "Sources/NotedAppleSecurityPlugin"))
}

let package = Package(
  name: "noted-apple-security",
  platforms: [
    .iOS(.v17),
    .macOS(.v14),
  ],
  products: products,
  dependencies: dependencies,
  targets: targets)
