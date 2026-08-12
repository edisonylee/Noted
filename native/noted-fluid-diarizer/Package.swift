// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "NotedFluidDiarizer",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "noted-fluid-diarizer", targets: ["NotedFluidDiarizer"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/FluidInference/FluidAudio.git",
            exact: "0.15.5"
        ),
    ],
    targets: [
        .executableTarget(
            name: "NotedFluidDiarizer",
            dependencies: [
                .product(name: "FluidAudio", package: "FluidAudio"),
            ]
        ),
    ],
    cxxLanguageStandard: .cxx17
)
