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
    try checkEqual(
        TrafficQuery().queryItems.first,
        URLQueryItem(name: "limit", value: "5000"),
        "default native traffic query must request the server-retained default instead of the Admin API 100-row fallback"
    )
    try checkEqual(
        TrafficQuery(limit: 500, cursor: 42, direction: "backward").queryItems,
        [
            URLQueryItem(name: "limit", value: "500"),
            URLQueryItem(name: "cursor", value: "42"),
            URLQueryItem(name: "direction", value: "backward"),
        ],
        "native traffic history query must match the WebUI backward cursor strategy"
    )
    try checkEqual(
        TrafficUpdatesQuery(afterId: "req-1", afterSequence: 7, pendingIds: ["req-open"], limit: 1_000).queryItems,
        [
            URLQueryItem(name: "limit", value: "1000"),
            URLQueryItem(name: "after_id", value: "req-1"),
            URLQueryItem(name: "after_seq", value: "7"),
            URLQueryItem(name: "pending_ids", value: "req-open"),
        ],
        "native traffic updates query must match the WebUI incremental update strategy"
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

    let matchingStatusJSON = Data("""
    {"schema_version":1,"running":true,"listener":{"host":"0.0.0.0","port":9900},"data_dir":"/tmp/bifrost-data"}
    """.utf8)
    let matchingSnapshot = try await manager.decodeStatusSnapshot(from: matchingStatusJSON)
    let acceptsMatchingDataDirectory = await manager.statusSnapshotMatchesConfiguredDataDirectory(matchingSnapshot)
    let rejectsDifferentDataDirectory = await manager.statusSnapshotMatchesConfiguredDataDirectory(snapshot)
    try checkEqual(
        acceptsMatchingDataDirectory,
        true,
        "native app should be allowed to mount a running service for the configured default data directory"
    )
    try checkEqual(
        rejectsDifferentDataDirectory,
        false,
        "native app must not mount a running service from a different data directory"
    )
}

func runAdminModelDecodingChecks() throws {
    let missingMatchedProtocolsJSON = Data("""
    {"records":[{"id":"1","seq":7,"m":"CONNECT","h":"chatgpt.com","s":200}],"total":1}
    """.utf8)
    let response = try JSONDecoder().decode(TrafficListResponse.self, from: missingMatchedProtocolsJSON)
    try checkEqual(response.records.count, 1, "traffic response should decode one record")
    try checkEqual(response.records[0].matchedProtocols, [], "missing traffic rp should default to empty matched protocol list")
    try checkEqual(response.records[0].host, "chatgpt.com", "traffic compact host key should decode")

    let trafficUpdatesJSON = Data("""
    {"new_records":[{"id":"2","seq":8,"m":"GET","h":"example.com","s":200}],"updated_records":[{"id":"1","seq":7,"m":"CONNECT","h":"chatgpt.com","s":200}],"has_more":true,"server_total":777,"server_sequence":888}
    """.utf8)
    let updates = try JSONDecoder().decode(TrafficUpdatesResponse.self, from: trafficUpdatesJSON)
    try checkEqual(updates.newRecords.count, 1, "traffic updates new_records should decode")
    try checkEqual(updates.updatedRecords.count, 1, "traffic updates updated_records should decode")
    try checkEqual(updates.hasMore, true, "traffic updates has_more should decode")
    try checkEqual(updates.serverTotal, 777, "traffic updates server_total should decode")

    let performanceConfigJSON = Data("""
    {"traffic":{"max_records":12000,"max_db_size_bytes":2147483648,"file_retention_days":7,"binary_traffic_performance_mode":true},"breakpoint":{"timeout_ms":30000},"resource_alerts":{}}
    """.utf8)
    let performanceConfig = try JSONDecoder().decode(PerformanceConfigResponse.self, from: performanceConfigJSON)
    try checkEqual(performanceConfig.traffic.maxRecords, 12_000, "performance traffic retention limit should decode")
    try checkEqual(performanceConfig.traffic.fileRetentionDays, 7, "performance retention days should decode")

    let userListJSON = Data("""
    {"code":0,"message":"ok","data":{"list":[{"id":"u1","user_id":"user-1","nickname":"Eden","create_time":"2026-07-04T00:00:00Z","update_time":"2026-07-04T00:00:00Z"}],"total":1}}
    """.utf8)
    let userEnvelope = try JSONDecoder().decode(RemoteEnvelope<RemoteListPayload<GroupUser>>.self, from: userListJSON)
    guard let user = userEnvelope.data.list?.first else {
        throw CheckFailure.failed("group user search list should decode")
    }
    try checkEqual(user.userID, "user-1", "group user user_id should decode")
    try checkEqual(user.email, "", "missing group user email should default to empty string")
}

func runRuleLanguageChecks() throws {
    let service = BifrostRuleLanguageService()
    let text = """
    # comment
    token = abc
    ```headers
    x-tt-env: boe
    ```
    @Default
    reqScript://auth
    resScript://rewrite
    bp://parser
    proxy://example.com?token={token}&headers={headers}&global=${global_token}
    /api\\/v1/.*
    """
    let context = BifrostRuleEditorContext(
        currentRuleName: "Current",
        ruleNames: ["Default", "DirectOnly"],
        values: ["global_token", "region"],
        requestScripts: ["auth"],
        responseScripts: ["rewrite"],
        parserScripts: ["parser"],
        localVariables: service.localVariables(in: text)
    )

    let tokens = service.tokenize(text, context: context)
    try check(tokens.contains { $0.kind == .comment }, "rule tokenizer should detect comments")
    try check(tokens.contains { $0.kind == .ruleReference }, "rule tokenizer should detect @rule references")
    try check(tokens.contains { $0.kind == .requestScript }, "rule tokenizer should detect request script references")
    try check(tokens.contains { $0.kind == .responseScript }, "rule tokenizer should detect response script references")
    try check(tokens.contains { $0.kind == .parserScript }, "rule tokenizer should detect parser script references")
    try check(tokens.contains { $0.kind == .variable }, "rule tokenizer should detect value variables")
    try check(tokens.contains { $0.kind == .localVariable }, "rule tokenizer should highlight block variable names")
    try check(tokens.contains { $0.kind == .regexp }, "rule tokenizer should detect regex literals")

    let localVariables = service.localVariables(in: text)
    try checkEqual(localVariables.first?.name, "token", "rule language should extract local key=value variables")
    try checkEqual(localVariables.first?.line, 2, "rule language local variable line should be 1-based")
    try check(
        localVariables.contains { $0.name == "headers" && $0.line == 3 },
        "rule language should extract fenced block variables"
    )

    let ruleCursor = (text as NSString).range(of: "@Def").location + "@Def".utf16.count
    let ruleCompletions = service.completions(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: ruleCursor),
        context: context
    )
    try checkEqual(ruleCompletions.first?.label, "@Default", "rule completion should fuzzy match @ references")

    let valueCursor = (text as NSString).range(of: "{tok").location + "{tok".utf16.count
    let valueCompletions = service.completions(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: valueCursor),
        context: context
    )
    try check(valueCompletions.contains { $0.label == "{token}" && $0.kind == .localVariable }, "value completion should include local variables")
    try check(valueCompletions.contains { $0.label == "{global_token}" }, "value completion should include global values")

    let blockValueCursor = (text as NSString).range(of: "{hea").location + "{hea".utf16.count
    let blockValueCompletions = service.completions(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: blockValueCursor),
        context: context
    )
    try check(
        blockValueCompletions.contains { $0.label == "{headers}" && $0.kind == .localVariable },
        "value completion should include fenced block variables"
    )

    let scriptCursor = (text as NSString).range(of: "reqScript://au").location + "reqScript://au".utf16.count
    let scriptCompletions = service.completions(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: scriptCursor),
        context: context
    )
    try checkEqual(scriptCompletions.first?.insertText, "auth", "request script completion should replace only typed suffix")

    let ruleReferenceOffset = (text as NSString).range(of: "@Default").location + 2
    let ruleReference = service.reference(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: ruleReferenceOffset),
        context: context
    )
    try checkEqual(ruleReference?.name, "Default", "referenceAt should detect @rule under cursor")
    try checkEqual(
        service.navigationTarget(for: ruleReference!, context: context),
        .rule(group: nil, name: "Default"),
        "rule reference should navigate to rule"
    )

    let valueReferenceOffset = (text as NSString).range(of: "{token}").location + 2
    let valueReference = service.reference(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: valueReferenceOffset),
        context: context
    )
    try checkEqual(valueReference?.type, .localVariable, "local variable references should win over global values")
    try checkEqual(
        service.navigationTarget(for: valueReference!, context: context),
        .editorLine(2),
        "local variable navigation should target defining line"
    )

    let blockReferenceOffset = (text as NSString).range(of: "{headers}").location + 2
    let blockReference = service.reference(
        in: text,
        cursor: BifrostTextPosition(utf16Offset: blockReferenceOffset),
        context: context
    )
    try checkEqual(blockReference?.type, .localVariable, "block variable references should be local variables")
    try checkEqual(
        service.navigationTarget(for: blockReference!, context: context),
        .editorLine(3),
        "block variable navigation should target fenced definition line"
    )
}

@main
struct BifrostNativeCoreChecks {
    static func main() async {
        do {
            try runAdminAPIRequestFactoryChecks()
            try runAdminModelDecodingChecks()
            try runRuleLanguageChecks()
            try await runSidecarConfigurationChecks()
            print("BifrostNativeCoreChecks passed")
        } catch {
            fputs("BifrostNativeCoreChecks failed: \(error)\n", stderr)
            Foundation.exit(1)
        }
    }
}
