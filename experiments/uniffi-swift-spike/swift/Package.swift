// swift-tools-version: 6.0

import Foundation
import PackageDescription

let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let rustDirectory = packageDirectory
    .deletingLastPathComponent()
    .appendingPathComponent("rust/target/release")
let linkMode = Context.environment["AVEN_INTEROP_LINK_MODE"] ?? "static"
let libraryExtension = linkMode == "dynamic" ? "dylib" : "a"
let rustLibrary = rustDirectory
    .appendingPathComponent("libaven_interop_spike.\(libraryExtension)")
    .path

let package = Package(
    name: "AvenInteropProof",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "aven-interop-proof", targets: ["AvenInteropProof"]),
    ],
    targets: [
        .systemLibrary(
            name: "aven_interop_spikeFFI",
            path: "Sources/aven_interop_spikeFFI"
        ),
        .target(
            name: "AvenInterop",
            dependencies: ["aven_interop_spikeFFI"],
            path: "Sources/AvenInterop"
        ),
        .executableTarget(
            name: "AvenInteropProof",
            dependencies: ["AvenInterop"],
            path: "Sources/AvenInteropProof",
            linkerSettings: [.unsafeFlags([rustLibrary])]
        ),
    ]
)
