// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "BifrostMac",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "BifrostMac", targets: ["BifrostMac"]),
        .executable(name: "BifrostMacCoreChecks", targets: ["BifrostMacCoreChecks"]),
        .library(name: "BifrostMacCore", targets: ["BifrostMacCore"])
    ],
    targets: [
        .target(
            name: "BifrostMacCore",
            path: "Sources/BifrostMacCore"
        ),
        .executableTarget(
            name: "BifrostMac",
            dependencies: ["BifrostMacCore"],
            path: "Sources/BifrostMac"
        ),
        .executableTarget(
            name: "BifrostMacCoreChecks",
            dependencies: ["BifrostMacCore"],
            path: "Sources/BifrostMacCoreChecks"
        )
    ]
)
