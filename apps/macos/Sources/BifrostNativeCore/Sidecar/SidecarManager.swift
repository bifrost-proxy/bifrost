import Foundation

public enum SidecarState: Equatable, Sendable {
    case stopped
    case starting
    case running(port: UInt16)
    case failed(String)
    case recovering
}

public struct SidecarStatusSnapshot: Decodable, Equatable, Sendable {
    public struct Listener: Decodable, Equatable, Sendable {
        public var host: String?
        public var port: UInt16
    }

    public var running: Bool
    public var listener: Listener?
    public var dataDir: String?

    enum CodingKeys: String, CodingKey {
        case running
        case listener
        case dataDir = "data_dir"
    }
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
        if configuration.daemonize {
            arguments.append("--daemon")
        }
        if configuration.skipCertCheck {
            arguments.append("--skip-cert-check")
        }
        if configuration.noSystemProxy {
            arguments.append("--no-system-proxy")
        }

        let environment = makeEnvironment()

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

    public func ensureRunning() async throws {
        state = .starting
        if let port = try? probeRunningService() {
            state = .running(port: port)
            return
        }

        try await start()

        for _ in 0..<40 {
            if let port = try? probeRunningService() {
                state = .running(port: port)
                return
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }

        state = .failed("Timed out waiting for Bifrost daemon to become ready.")
    }

    public func decodeStatusSnapshot(from data: Data) throws -> SidecarStatusSnapshot {
        try JSONDecoder().decode(SidecarStatusSnapshot.self, from: data)
    }

    public func probeRunningService() throws -> UInt16? {
        let output = try runBifrostCommand(arguments: ["status", "--format", "json"])
        guard !output.isEmpty else {
            return nil
        }
        let snapshot = try decodeStatusSnapshot(from: output)
        guard snapshot.running, let port = snapshot.listener?.port else {
            return nil
        }
        return port
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
        if configuration.daemonize {
            process.waitUntilExit()
            self.process = nil
        } else {
            self.process = process
            state = .running(port: port)
        }
    }

    public func stop() {
        process?.terminate()
        process = nil
        state = .stopped
    }

    private func runBifrostCommand(arguments: [String]) throws -> Data {
        let process = Process()
        process.executableURL = configuration.binaryPath
        process.arguments = arguments
        process.environment = makeEnvironment()

        let output = Pipe()
        process.standardOutput = output
        process.standardError = Pipe()
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        guard process.terminationStatus == 0 else {
            return Data()
        }
        return data
    }

    private func makeEnvironment() -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        environment["BIFROST_DATA_DIR"] = configuration.dataDirectory.path
        if configuration.disableTray {
            environment["BIFROST_DISABLE_TRAY"] = "1"
        }
        if configuration.disableSyncAutoLoginPrompt {
            environment["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"] = "1"
        }
        return environment
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
