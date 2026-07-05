// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "BifrostEnhancedProxy",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "BifrostEnhancedProxyHost", targets: ["BifrostEnhancedProxyHost"]),
        .library(name: "BifrostEnhancedProxyExtension", targets: ["BifrostEnhancedProxyExtension"])
    ],
    targets: [
        .executableTarget(
            name: "BifrostEnhancedProxyHost",
            path: "Sources/BifrostEnhancedProxyHost"
        ),
        .target(
            name: "BifrostEnhancedProxyExtension",
            path: "Sources/BifrostEnhancedProxyExtension"
        )
    ]
)
