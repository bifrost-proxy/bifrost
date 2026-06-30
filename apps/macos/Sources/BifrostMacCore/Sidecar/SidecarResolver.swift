import Foundation

public enum SidecarResolver {
    public static func resolveBundledBinary(
        bundle: Bundle = .main,
        fileManager: FileManager = .default
    ) -> URL? {
        let binaryName = "bifrost"
        let candidates = [
            bundle.resourceURL?.appendingPathComponent("bin/\(binaryName)"),
            bundle.resourceURL?.appendingPathComponent("resources/bin/\(binaryName)")
        ].compactMap { $0 }
        return candidates.first { fileManager.isExecutableFile(atPath: $0.path) }
    }

    public static func resolveDevelopmentBinary(
        packageDirectory: URL,
        configuration: String = "debug",
        fileManager: FileManager = .default
    ) -> URL? {
        let repoRoot = packageDirectory
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let candidates = [
            repoRoot.appendingPathComponent("target/\(configuration)/bifrost"),
            packageDirectory.appendingPathComponent(".build/sidecar/bin/bifrost")
        ]
        return candidates.first { fileManager.isExecutableFile(atPath: $0.path) }
    }
}
