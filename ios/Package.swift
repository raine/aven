// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "AvenUniFFI",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "AvenUniFFI", targets: ["AvenUniFFI"]),
        .executable(
            name: "AvenUniFFIPackageProbe",
            targets: ["AvenUniFFIPackageProbe"]
        ),
    ],
    targets: [
        .binaryTarget(
            name: "aven_uniffiFFI",
            path: "Generated/AvenUniFFI.xcframework"
        ),
        .target(
            name: "AvenUniFFI",
            dependencies: ["aven_uniffiFFI"],
            path: "Generated/Sources/AvenUniFFI",
            linkerSettings: [
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("iconv"),
            ]
        ),
        .executableTarget(
            name: "AvenUniFFIPackageProbe",
            dependencies: ["AvenUniFFI"],
            path: "Sources/AvenUniFFIPackageProbe"
        ),
    ]
)
