import Foundation

public actor BifrostClient {
    private let factory: AdminAPIRequestFactory
    private let session: URLSession

    public init(factory: AdminAPIRequestFactory, session: URLSession = .shared) {
        self.factory = factory
        self.session = session
    }

    public init(baseURL: URL) throws {
        self.factory = try AdminAPIRequestFactory(baseURL: baseURL)
        self.session = .shared
    }

    public func getSystemOverview() async throws -> Data {
        try await request(.get, path: "/system/overview")
    }

    public func fetchSystemOverview() async throws -> SystemOverview {
        try await decode(SystemOverview.self, from: getSystemOverview())
    }

    public func listTraffic(query: TrafficQuery = TrafficQuery()) async throws -> Data {
        try await request(.get, path: "/traffic", queryItems: query.queryItems)
    }

    public func fetchTraffic(query: TrafficQuery = TrafficQuery()) async throws -> TrafficListResponse {
        try await decode(TrafficListResponse.self, from: listTraffic(query: query))
    }

    public func fetchAppIcon(appName: String) async throws -> Data {
        try await request(.get, path: "/app-icon/\(appName)")
    }

    public func getTraffic(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)")
    }

    public func getRequestBody(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)/request-body")
    }

    public func getResponseBody(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)/response-body")
    }

    public func clearTraffic() async throws {
        _ = try await request(.delete, path: "/traffic")
    }

    public func listRules() async throws -> Data {
        try await request(.get, path: "/rules")
    }

    public func fetchRules() async throws -> [RuleSummary] {
        try await decode([RuleSummary].self, from: listRules())
    }

    public func fetchRule(name: String) async throws -> RuleDetail {
        let data = try await request(.get, path: "/rules/\(encodePathSegment(name))")
        return try await decode(RuleDetail.self, from: data)
    }

    public func createRule(name: String, content: String, enabled: Bool = true) async throws {
        let body = try JSONEncoder().encode(CreateRuleRequest(name: name, content: content, enabled: enabled))
        _ = try await request(.post, path: "/rules", body: body)
    }

    public func updateRule(name: String, content: String? = nil, enabled: Bool? = nil) async throws {
        let body = try JSONEncoder().encode(UpdateRuleRequest(content: content, enabled: enabled))
        _ = try await request(.put, path: "/rules/\(encodePathSegment(name))", body: body)
    }

    public func setRuleEnabled(name: String, enabled: Bool) async throws {
        let action = enabled ? "enable" : "disable"
        _ = try await request(.put, path: "/rules/\(encodePathSegment(name))/\(action)")
    }

    public func renameRule(oldName: String, newName: String) async throws {
        let body = try JSONEncoder().encode(RenameRequest(newName: newName))
        _ = try await request(.put, path: "/rules/\(encodePathSegment(oldName))/rename", body: body)
    }

    public func reorderRules(_ order: [String]) async throws {
        let body = try JSONEncoder().encode(ReorderRulesRequest(order: order))
        _ = try await request(.put, path: "/rules/reorder", body: body)
    }

    public func deleteRule(name: String) async throws {
        _ = try await request(.delete, path: "/rules/\(encodePathSegment(name))")
    }

    public func listValues() async throws -> Data {
        try await request(.get, path: "/values")
    }

    public func fetchValues() async throws -> ValuesListResponse {
        try await decode(ValuesListResponse.self, from: listValues())
    }

    public func fetchValue(name: String) async throws -> ValueItem {
        let data = try await request(.get, path: "/values/\(encodePathSegment(name))")
        return try await decode(ValueItem.self, from: data)
    }

    public func createValue(name: String, value: String) async throws {
        let body = try JSONEncoder().encode(CreateValueRequest(name: name, value: value))
        _ = try await request(.post, path: "/values", body: body)
    }

    public func updateValue(name: String, value: String) async throws {
        let body = try JSONEncoder().encode(UpdateValueRequest(value: value))
        _ = try await request(.put, path: "/values/\(encodePathSegment(name))", body: body)
    }

    public func renameValue(oldName: String, newName: String) async throws {
        let value = try await fetchValue(name: oldName)
        try await createValue(name: newName, value: value.value)
        try await deleteValue(name: oldName)
    }

    public func deleteValue(name: String) async throws {
        _ = try await request(.delete, path: "/values/\(encodePathSegment(name))")
    }

    public func listScripts() async throws -> Data {
        try await request(.get, path: "/scripts")
    }

    public func fetchScripts() async throws -> ScriptsListResponse {
        try await decode(ScriptsListResponse.self, from: listScripts())
    }

    public func fetchScript(type: ScriptType, name: String) async throws -> ScriptDetail {
        let data = try await request(
            .get,
            path: "/scripts/\(encodePathSegment(type.rawValue))/\(encodePathSegment(name))"
        )
        return try await decode(ScriptDetail.self, from: data)
    }

    public func saveScript(
        type: ScriptType,
        name: String,
        content: String,
        description: String? = nil
    ) async throws -> ScriptDetail {
        let body = try JSONEncoder().encode(SaveScriptRequest(content: content, description: description))
        let data = try await request(
            .put,
            path: "/scripts/\(encodePathSegment(type.rawValue))/\(encodePathSegment(name))",
            body: body
        )
        return try await decode(ScriptDetail.self, from: data)
    }

    public func renameScript(type: ScriptType, oldName: String, newName: String) async throws {
        let body = try JSONEncoder().encode(RenameRequest(newName: newName))
        _ = try await request(
            .post,
            path: "/scripts/rename/\(encodePathSegment(type.rawValue))/\(encodePathSegment(oldName))",
            body: body
        )
    }

    public func deleteScript(type: ScriptType, name: String) async throws {
        _ = try await request(
            .delete,
            path: "/scripts/\(encodePathSegment(type.rawValue))/\(encodePathSegment(name))"
        )
    }

    public func getCertInfo() async throws -> Data {
        try await request(.get, path: "/cert")
    }

    public func getProxyAddress() async throws -> Data {
        try await request(.get, path: "/proxy/address")
    }

    public func getSystemProxy() async throws -> Data {
        try await request(.get, path: "/proxy/system")
    }

    public func fetchSystemProxy() async throws -> SystemProxyStatus {
        try await decode(SystemProxyStatus.self, from: getSystemProxy())
    }

    public func setSystemProxy(enabled: Bool, bypass: String? = nil) async throws -> SystemProxyStatus {
        let body = try JSONEncoder().encode(SetSystemProxyRequest(enabled: enabled, bypass: bypass))
        let data = try await request(.put, path: "/proxy/system", body: body)
        return try await decode(SystemProxyStatus.self, from: data)
    }

    public func getTlsConfig() async throws -> Data {
        try await request(.get, path: "/config/tls")
    }

    public func fetchTlsConfig() async throws -> TlsConfig {
        try await decode(TlsConfig.self, from: getTlsConfig())
    }

    public func updateTlsConfig(_ config: TlsConfig) async throws -> TlsConfig {
        let body = try JSONEncoder().encode(config)
        let data = try await request(.put, path: "/config/tls", body: body)
        return try await decode(TlsConfig.self, from: data)
    }

    public func getBreakpointSettings() async throws -> Data {
        try await request(.get, path: "/breakpoint/settings")
    }

    public func fetchBreakpointSettings() async throws -> BreakpointSettings {
        try await decode(BreakpointSettings.self, from: getBreakpointSettings())
    }

    public func updateBreakpointSettings(_ settings: BreakpointSettings) async throws -> BreakpointSettings {
        let body = try JSONEncoder().encode(settings)
        let data = try await request(.post, path: "/breakpoint/settings", body: body)
        return try await decode(BreakpointSettings.self, from: data)
    }

    public func request(
        _ method: HTTPMethod,
        path: String,
        queryItems: [URLQueryItem] = [],
        body: Data? = nil
    ) async throws -> Data {
        let request = try factory.makeRequest(
            method: method,
            path: path,
            queryItems: queryItems,
            body: body
        )
        let (data, response) = try await session.data(for: request)
        if let httpResponse = response as? HTTPURLResponse,
           !(200..<300).contains(httpResponse.statusCode) {
            throw BifrostClientError.httpStatus(httpResponse.statusCode, data)
        }
        return data
    }

    private func decode<T: Decodable>(_ type: T.Type, from data: Data) async throws -> T {
        try JSONDecoder().decode(type, from: data)
    }

    private func encodePathSegment(_ segment: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#[]@!$&'()*+,;=")
        return segment.addingPercentEncoding(withAllowedCharacters: allowed) ?? segment
    }
}

public enum BifrostClientError: Error, Equatable {
    case httpStatus(Int, Data)
}
