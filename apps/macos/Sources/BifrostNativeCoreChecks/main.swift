import BifrostNativeCore
import Foundation

enum CheckFailure: Error, CustomStringConvertible {
    case failed(String)

    var description: String {
        switch self {
        case .failed(let message):
            return message
        }
    }
}

func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw CheckFailure.failed(message)
    }
}

func checkEqual<T: Equatable>(_ actual: T, _ expected: T, _ message: String) throws {
    if actual != expected {
        throw CheckFailure.failed("\(message). actual=\(actual) expected=\(expected)")
    }
}

func runAdminAPIRequestFactoryChecks() throws {
    let factory = try AdminAPIRequestFactory(
        baseURL: URL(string: "http://127.0.0.1:9900")!,
        clientId: "test-client",
        authToken: "secret",
        csrfToken: "csrf"
    )

    let url = try factory.makeURL(
        path: "/traffic",
        queryItems: [
            URLQueryItem(name: "limit", value: "50"),
            URLQueryItem(name: "host", value: "example.com")
        ]
    )
    try checkEqual(
        url.absoluteString,
        "http://127.0.0.1:9900/_bifrost/api/traffic?limit=50&host=example.com",
        "Admin API URL prefix or query construction changed"
    )

    let request = try factory.makeRequest(
        method: .post,
        path: "/rules",
        body: Data("{}".utf8)
    )
    try checkEqual(request.value(forHTTPHeaderField: "X-Client-Id"), "test-client", "missing client id header")
    try checkEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer secret", "missing auth header")
    try checkEqual(request.value(forHTTPHeaderField: AdminAPIRequestFactory.csrfHeaderName), "csrf", "missing csrf header")
    try checkEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/json", "missing json content type")

    do {
        _ = try factory.makeURL(path: "traffic")
        throw CheckFailure.failed("relative Admin API path should fail")
    } catch AdminAPIError.invalidPath {
    }
}

func runSidecarConfigurationChecks() async throws {
    let home = URL(fileURLWithPath: "/Users/tester", isDirectory: true)
    try checkEqual(
        SidecarConfiguration.defaultDataDirectory(homeDirectory: home).path,
        "/Users/tester/.bifrost",
        "default data directory must stay compatible with the existing CLI and Tauri desktop"
    )

    let configuration = SidecarConfiguration(
        binaryPath: URL(fileURLWithPath: "/tmp/bifrost"),
        dataDirectory: URL(fileURLWithPath: "/tmp/bifrost-data", isDirectory: true),
        preferredPort: 9900
    )
    let manager = SidecarManager(configuration: configuration)
    let plan = await manager.makeStartPlan(port: 9900)
    try checkEqual(plan.executableURL.path, "/tmp/bifrost", "wrong sidecar executable path")
    try checkEqual(plan.arguments, [
        "start",
        "--host",
        "0.0.0.0",
        "--port",
        "9900",
        "--daemon",
        "--skip-cert-check",
        "--no-system-proxy"
    ], "sidecar start arguments changed")
    try checkEqual(plan.environment["BIFROST_DATA_DIR"], "/tmp/bifrost-data", "missing sidecar data dir")
    try checkEqual(plan.environment["BIFROST_DISABLE_TRAY"], "1", "tray must be disabled in development smoke plans")
    try checkEqual(plan.environment["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"], "1", "sync auto-login prompt must be disabled in development smoke plans")

    try checkEqual(
        PortSelection.candidatePorts(preferredPort: 9900, maxIncrementAttempts: 3),
        [9900, 9901, 9902, 9903],
        "candidate port strategy must match Tauri sidecar increment behavior"
    )

    let statusJSON = Data("""
    {"schema_version":1,"running":true,"listener":{"host":"0.0.0.0","port":9900},"data_dir":"/Users/tester/.bifrost"}
    """.utf8)
    let snapshot = try await manager.decodeStatusSnapshot(from: statusJSON)
    try checkEqual(snapshot.running, true, "status snapshot should preserve running=true")
    try checkEqual(snapshot.listener?.port, 9900, "status snapshot should decode listener port")
    try checkEqual(snapshot.dataDir, "/Users/tester/.bifrost", "status snapshot should decode shared default data dir")
}

@main
struct BifrostNativeCoreChecks {
    static func main() async {
        do {
            try runAdminAPIRequestFactoryChecks()
            try await runSidecarConfigurationChecks()
            print("BifrostNativeCoreChecks passed")
        } catch {
            fputs("BifrostNativeCoreChecks failed: \(error)\n", stderr)
            Foundation.exit(1)
        }
    }
}
