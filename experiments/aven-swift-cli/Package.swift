// swift-tools-version: 6.0

import Foundation
import PackageDescription

let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustLibrary = packageDirectory
    .appendingPathComponent("Generated/lib/libaven_uniffi.a")
    .path
let rustLinkerSettings: [LinkerSetting] = [.unsafeFlags([rustLibrary])]

let package = Package(
    name: "AvenLocalProof",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "aven-local-proof", targets: ["AvenLocalProof"]),
    ],
    targets: [
        .systemLibrary(
            name: "aven_uniffiFFI",
            path: "Generated/Sources/aven_uniffiFFI"
        ),
        .target(
            name: "AvenUniFFI",
            dependencies: ["aven_uniffiFFI"],
            path: "Generated/Sources/AvenUniFFI"
        ),
        .target(
            name: "AvenLocalProofCore",
            dependencies: ["AvenUniFFI"],
            path: "Sources/AvenLocalProofCore"
        ),
        .executableTarget(
            name: "AvenLocalProof",
            dependencies: ["AvenLocalProofCore"],
            path: "Sources/AvenLocalProof",
            linkerSettings: rustLinkerSettings
        ),
        .testTarget(
            name: "AvenLocalProofCoreTests",
            dependencies: ["AvenLocalProofCore", "AvenUniFFI"],
            path: "Tests/AvenLocalProofCoreTests",
            linkerSettings: rustLinkerSettings
        ),
    ]
)
