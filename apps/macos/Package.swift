// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "Bifrost",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "Bifrost", targets: ["Bifrost"]),
        .executable(name: "BifrostNativeCoreChecks", targets: ["BifrostNativeCoreChecks"]),
        .library(name: "BifrostNativeCore", targets: ["BifrostNativeCore"])
    ],
    targets: [
        .target(
            name: "BifrostNativeCore",
            path: "Sources/BifrostNativeCore"
        ),
        .executableTarget(
            name: "Bifrost",
            dependencies: ["BifrostNativeCore"],
            path: "Sources/Bifrost",
            resources: [
                .copy("Resources")
            ]
        ),
        .executableTarget(
            name: "BifrostNativeCoreChecks",
            dependencies: ["BifrostNativeCore"],
            path: "Sources/BifrostNativeCoreChecks"
        )
    ]
)
