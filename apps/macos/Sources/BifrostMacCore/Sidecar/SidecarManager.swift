import Foundation
import Network

public enum SidecarState: Equatable, Sendable {
    case stopped
    case starting
    case running(port: UInt16)
    case failed(String)
    case recovering
}

public actor SidecarManager {
    private let configuration: SidecarConfiguration
    private var process: Process?
    private var state: SidecarState = .stopped

    public init(configuration: SidecarConfiguration) {
        self.configuration = configuration
    }

    public func currentState() -> SidecarState {
        state
    }

    public func makeStartPlan(port: UInt16) -> SidecarCommandPlan {
        var arguments = [
            "start",
            "--host",
            configuration.bindHost,
            "--port",
            String(port)
        ]
        if configuration.skipCertCheck {
            arguments.append("--skip-cert-check")
        }
        if configuration.noSystemProxy {
            arguments.append("--no-system-proxy")
        }

        var environment = ProcessInfo.processInfo.environment
        environment["BIFROST_DATA_DIR"] = configuration.dataDirectory.path
        if configuration.disableTray {
            environment["BIFROST_DISABLE_TRAY"] = "1"
        }
        if configuration.disableSyncAutoLoginPrompt {
            environment["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"] = "1"
        }

        let logDirectory = configuration.dataDirectory
            .appendingPathComponent("logs", isDirectory: true)
        return SidecarCommandPlan(
            executableURL: configuration.binaryPath,
            arguments: arguments,
            environment: environment,
            stdoutLogURL: logDirectory.appendingPathComponent("macos-native-sidecar.out.log"),
            stderrLogURL: logDirectory.appendingPathComponent("macos-native-sidecar.err.log")
        )
    }

    public func adminURL(port: UInt16) -> URL {
        URL(string: "http://\(configuration.adminHost):\(port)")!
    }

    public func candidatePorts() -> [UInt16] {
        PortSelection.candidatePorts(
            preferredPort: configuration.preferredPort,
            maxIncrementAttempts: SidecarConfiguration.maxPortIncrementAttempts
        )
    }

    public func start() async throws {
        state = .starting
        let ports = candidatePorts()
        guard let port = ports.first else {
            state = .failed("No candidate ports available.")
            return
        }

        let plan = makeStartPlan(port: port)
        try FileManager.default.createDirectory(
            at: plan.stdoutLogURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )

        let process = Process()
        process.executableURL = plan.executableURL
        process.arguments = plan.arguments
        process.environment = plan.environment
        process.standardOutput = try FileHandle(forWritingTo: plan.stdoutLogURL)
        process.standardError = try FileHandle(forWritingTo: plan.stderrLogURL)
        try process.run()
        self.process = process
        state = .running(port: port)
    }

    public func stop() {
        process?.terminate()
        process = nil
        state = .stopped
    }
}

public enum PortSelection {
    public static func candidatePorts(
        preferredPort: UInt16,
        maxIncrementAttempts: UInt16
    ) -> [UInt16] {
        (0...maxIncrementAttempts).compactMap { offset in
            let (port, overflow) = preferredPort.addingReportingOverflow(offset)
            return overflow || port == 0 ? nil : port
        }
    }
}
