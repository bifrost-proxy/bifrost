import Foundation

public struct SidecarConfiguration: Equatable, Sendable {
    public static let defaultPort: UInt16 = 9900
    public static let maxPortIncrementAttempts: UInt16 = 64

    public var binaryPath: URL
    public var dataDirectory: URL
    public var preferredPort: UInt16
    public var bindHost: String
    public var adminHost: String
    public var skipCertCheck: Bool
    public var noSystemProxy: Bool
    public var disableTray: Bool
    public var disableSyncAutoLoginPrompt: Bool
    public var daemonize: Bool

    public init(
        binaryPath: URL,
        dataDirectory: URL = SidecarConfiguration.defaultDataDirectory(),
        preferredPort: UInt16 = SidecarConfiguration.defaultPort,
        bindHost: String = "0.0.0.0",
        adminHost: String = "127.0.0.1",
        skipCertCheck: Bool = true,
        noSystemProxy: Bool = true,
        disableTray: Bool = true,
        disableSyncAutoLoginPrompt: Bool = true,
        daemonize: Bool = true
    ) {
        self.binaryPath = binaryPath
        self.dataDirectory = dataDirectory
        self.preferredPort = preferredPort
        self.bindHost = bindHost
        self.adminHost = adminHost
        self.skipCertCheck = skipCertCheck
        self.noSystemProxy = noSystemProxy
        self.disableTray = disableTray
        self.disableSyncAutoLoginPrompt = disableSyncAutoLoginPrompt
        self.daemonize = daemonize
    }

    public static func defaultDataDirectory(
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> URL {
        homeDirectory.appendingPathComponent(".bifrost", isDirectory: true)
    }
}

public struct SidecarCommandPlan: Equatable, Sendable {
    public var executableURL: URL
    public var arguments: [String]
    public var environment: [String: String]
    public var stdoutLogURL: URL
    public var stderrLogURL: URL
}
