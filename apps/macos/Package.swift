// swift-tools-version: 5.9

import Foundation
import PackageDescription

let enableCodeEditRuleEditor = ProcessInfo.processInfo.environment["BIFROST_BUILD_CODEEDIT_RULE_EDITOR"] == "1"
let codeEditRuleEditorDependencies: [Package.Dependency] = enableCodeEditRuleEditor
    ? [
        .package(
            url: "https://github.com/CodeEditApp/CodeEditSourceEditor.git",
            exact: "0.15.2"
        ),
        .package(
            url: "https://github.com/CodeEditApp/CodeEditTextView.git",
            from: "0.12.1"
        ),
        .package(
            url: "https://github.com/CodeEditApp/CodeEditLanguages.git",
            exact: "0.1.20"
        ),
    ]
    : []
let bifrostTargetDependencies: [Target.Dependency] = [
    "BifrostNativeCore",
] + (
    enableCodeEditRuleEditor
        ? [
            .product(name: "CodeEditSourceEditor", package: "CodeEditSourceEditor"),
            .product(name: "CodeEditTextView", package: "CodeEditTextView"),
            .product(name: "CodeEditLanguages", package: "CodeEditLanguages"),
        ]
        : []
)

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
    dependencies: codeEditRuleEditorDependencies,
    targets: [
        .target(
            name: "BifrostNativeCore",
            path: "Sources/BifrostNativeCore"
        ),
        .executableTarget(
            name: "Bifrost",
            dependencies: bifrostTargetDependencies,
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
