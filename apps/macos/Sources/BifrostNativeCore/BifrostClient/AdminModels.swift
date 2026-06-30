import Foundation

public struct SystemOverview: Decodable, Equatable, Sendable {
    public struct Metrics: Decodable, Equatable, Sendable {
        public var activeConnections: Int?
        public var qps: Double?
        public var totalRequests: Int?
        public var bytesSent: Int?
        public var bytesReceived: Int?
        public var bytesSentRate: Double?
        public var bytesReceivedRate: Double?
        public var memoryUsed: Int?
        public var cpuUsage: Double?

        enum CodingKeys: String, CodingKey {
            case activeConnections = "active_connections"
            case qps
            case totalRequests = "total_requests"
            case bytesSent = "bytes_sent"
            case bytesReceived = "bytes_received"
            case bytesSentRate = "bytes_sent_rate"
            case bytesReceivedRate = "bytes_received_rate"
            case memoryUsed = "memory_used"
            case cpuUsage = "cpu_usage"
        }
    }

    public struct Server: Decodable, Equatable, Sendable {
        public var adminURL: String?
        public var port: Int?

        enum CodingKeys: String, CodingKey {
            case adminURL = "admin_url"
            case port
        }
    }

    public struct System: Decodable, Equatable, Sendable {
        public var pid: Int?
        public var version: String?
        public var uptimeSecs: Int?

        enum CodingKeys: String, CodingKey {
            case pid
            case version
            case uptimeSecs = "uptime_secs"
        }
    }

    public struct RulesSummary: Decodable, Equatable, Sendable {
        public var enabled: Int?
        public var total: Int?
    }

    public struct TrafficSummary: Decodable, Equatable, Sendable {
        public var recorded: Int?
    }

    public var metrics: Metrics?
    public var server: Server?
    public var system: System?
    public var rules: RulesSummary?
    public var traffic: TrafficSummary?
}

public struct TrafficListResponse: Decodable, Equatable, Sendable {
    public var records: [TrafficRecordSummary]
    public var nextCursor: Int?
    public var total: Int?

    enum CodingKeys: String, CodingKey {
        case records
        case nextCursor = "next_cursor"
        case total
    }
}

public struct TrafficRecordSummary: Decodable, Equatable, Identifiable, Sendable {
    public var id: String
    public var seq: Int?
    public var method: String?
    public var host: String?
    public var path: String?
    public var status: Int?
    public var contentType: String?
    public var responseSize: Int?
    public var durationMs: Int?
    public var listenerPort: Int?
    public var protocolName: String?
    public var clientApp: String?
    public var clientIp: String?
    public var startTime: String?
    public var endTime: String?
    public var flags: Int?
    public var matchedRuleCount: Int?
    public var matchedProtocols: [String]

    public init(
        id: String,
        seq: Int? = nil,
        method: String? = nil,
        host: String? = nil,
        path: String? = nil,
        status: Int? = nil,
        contentType: String? = nil,
        responseSize: Int? = nil,
        durationMs: Int? = nil,
        listenerPort: Int? = nil,
        protocolName: String? = nil,
        clientApp: String? = nil,
        clientIp: String? = nil,
        startTime: String? = nil,
        endTime: String? = nil,
        flags: Int? = nil,
        matchedRuleCount: Int? = nil,
        matchedProtocols: [String] = []
    ) {
        self.id = id
        self.seq = seq
        self.method = method
        self.host = host
        self.path = path
        self.status = status
        self.contentType = contentType
        self.responseSize = responseSize
        self.durationMs = durationMs
        self.listenerPort = listenerPort
        self.protocolName = protocolName
        self.clientApp = clientApp
        self.clientIp = clientIp
        self.startTime = startTime
        self.endTime = endTime
        self.flags = flags
        self.matchedRuleCount = matchedRuleCount
        self.matchedProtocols = matchedProtocols
    }

    enum CodingKeys: String, CodingKey {
        case id
        case seq
        case method = "m"
        case host = "h"
        case path = "p"
        case status = "s"
        case contentType = "ct"
        case responseSize = "res_sz"
        case durationMs = "dur"
        case listenerPort = "lp"
        case protocolName = "proto"
        case clientApp = "capp"
        case clientIp = "cip"
        case startTime = "st"
        case endTime = "et"
        case flags
        case matchedRuleCount = "rc"
        case matchedProtocols = "rp"
    }

    public var hasRuleHit: Bool {
        if (matchedRuleCount ?? 0) > 0 {
            return true
        }
        return ((flags ?? 0) & (1 << 4)) != 0
    }
}

public struct TrafficDeltaData: Decodable, Equatable, Sendable {
    public var inserts: [TrafficRecordSummary]
    public var updates: [TrafficRecordSummary]
    public var hasMore: Bool
    public var serverTotal: Int
    public var serverSequence: Int?

    enum CodingKeys: String, CodingKey {
        case inserts
        case updates
        case hasMore = "has_more"
        case serverTotal = "server_total"
        case serverSequence = "server_sequence"
    }
}

public struct TrafficDeletedData: Decodable, Equatable, Sendable {
    public var ids: [String]
}

public struct ValuesPushData: Decodable, Equatable, Sendable {
    public var values: [ValueItem]
    public var total: Int
}

public struct SettingsUpdateData: Decodable, Equatable, Sendable {
    public var scope: String
    public var data: Data

    enum CodingKeys: String, CodingKey {
        case scope
        case data
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        scope = try container.decode(String.self, forKey: .scope)
        let rawData = try container.decode(JSONValue.self, forKey: .data)
        data = try JSONEncoder().encode(rawData)
    }
}

public struct BreakpointSettingsPushData: Decodable, Equatable, Sendable {
    public var enabled: Bool
    public var maxBodyBytes: Int

    enum CodingKeys: String, CodingKey {
        case enabled
        case maxBodyBytes = "max_body_bytes"
    }
}

public enum PushMessage: Decodable, Equatable, Sendable {
    case connected(clientId: Int)
    case trafficDelta(TrafficDeltaData)
    case trafficDeleted(TrafficDeletedData)
    case valuesUpdate(ValuesPushData)
    case settingsUpdate(SettingsUpdateData)
    case breakpointSettingsUpdated(BreakpointSettingsPushData)
    case disconnect(String?)
    case ignored(String)

    private enum CodingKeys: String, CodingKey {
        case type
        case data
    }

    private enum ConnectedKeys: String, CodingKey {
        case clientId = "client_id"
    }

    private enum DisconnectKeys: String, CodingKey {
        case reason
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "connected":
            let data = try container.nestedContainer(keyedBy: ConnectedKeys.self, forKey: .data)
            self = .connected(clientId: try data.decode(Int.self, forKey: .clientId))
        case "traffic_delta":
            self = .trafficDelta(try container.decode(TrafficDeltaData.self, forKey: .data))
        case "traffic_deleted":
            self = .trafficDeleted(try container.decode(TrafficDeletedData.self, forKey: .data))
        case "values_update":
            self = .valuesUpdate(try container.decode(ValuesPushData.self, forKey: .data))
        case "settings_update":
            self = .settingsUpdate(try container.decode(SettingsUpdateData.self, forKey: .data))
        case "breakpoint_settings_updated":
            self = .breakpointSettingsUpdated(try container.decode(BreakpointSettingsPushData.self, forKey: .data))
        case "disconnect":
            let data = try? container.nestedContainer(keyedBy: DisconnectKeys.self, forKey: .data)
            self = .disconnect(try data?.decodeIfPresent(String.self, forKey: .reason))
        default:
            self = .ignored(type)
        }
    }
}

private enum JSONValue: Codable, Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value):
            try container.encode(value)
        case .number(let value):
            try container.encode(value)
        case .bool(let value):
            try container.encode(value)
        case .object(let value):
            try container.encode(value)
        case .array(let value):
            try container.encode(value)
        case .null:
            try container.encodeNil()
        }
    }
}

public struct RuleSummary: Decodable, Equatable, Identifiable, Sendable {
    public var name: String
    public var enabled: Bool
    public var sortOrder: Int?
    public var ruleCount: Int?
    public var updatedAt: String?

    public var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case enabled
        case sortOrder = "sort_order"
        case ruleCount = "rule_count"
        case updatedAt = "updated_at"
    }
}

public struct RuleDetail: Decodable, Equatable, Identifiable, Sendable {
    public var name: String
    public var content: String
    public var enabled: Bool
    public var sortOrder: Int?
    public var createdAt: String?
    public var updatedAt: String?

    public var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case content
        case enabled
        case sortOrder = "sort_order"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

public struct CreateRuleRequest: Encodable, Equatable, Sendable {
    public var name: String
    public var content: String
    public var enabled: Bool

    public init(name: String, content: String, enabled: Bool = true) {
        self.name = name
        self.content = content
        self.enabled = enabled
    }
}

public struct UpdateRuleRequest: Encodable, Equatable, Sendable {
    public var content: String?
    public var enabled: Bool?

    public init(content: String? = nil, enabled: Bool? = nil) {
        self.content = content
        self.enabled = enabled
    }
}

public struct RenameRequest: Encodable, Equatable, Sendable {
    public var newName: String

    public init(newName: String) {
        self.newName = newName
    }

    enum CodingKeys: String, CodingKey {
        case newName = "new_name"
    }
}

public struct ReorderRulesRequest: Encodable, Equatable, Sendable {
    public var order: [String]

    public init(order: [String]) {
        self.order = order
    }
}

public struct ValuesListResponse: Decodable, Equatable, Sendable {
    public var values: [ValueItem]
    public var total: Int
}

public struct ValueItem: Decodable, Equatable, Identifiable, Sendable {
    public var name: String
    public var value: String
    public var createdAt: String?
    public var updatedAt: String?

    public var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case value
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

public struct CreateValueRequest: Encodable, Equatable, Sendable {
    public var name: String
    public var value: String

    public init(name: String, value: String) {
        self.name = name
        self.value = value
    }
}

public struct UpdateValueRequest: Encodable, Equatable, Sendable {
    public var value: String

    public init(value: String) {
        self.value = value
    }
}

public enum ScriptType: String, Codable, CaseIterable, Identifiable, Sendable {
    case request
    case response
    case decode
    case parser

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .request:
            return "Request"
        case .response:
            return "Response"
        case .decode:
            return "Decode"
        case .parser:
            return "Parser"
        }
    }
}

public struct ScriptInfo: Decodable, Equatable, Identifiable, Sendable {
    public var name: String
    public var scriptType: ScriptType
    public var description: String?
    public var createdAt: Double
    public var updatedAt: Double

    public var id: String { "\(scriptType.rawValue):\(name)" }

    enum CodingKeys: String, CodingKey {
        case name
        case scriptType = "script_type"
        case description
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

public struct ScriptDetail: Decodable, Equatable, Identifiable, Sendable {
    public var name: String
    public var scriptType: ScriptType
    public var description: String?
    public var createdAt: Double
    public var updatedAt: Double
    public var content: String

    public var id: String { "\(scriptType.rawValue):\(name)" }

    enum CodingKeys: String, CodingKey {
        case name
        case scriptType = "script_type"
        case description
        case createdAt = "created_at"
        case updatedAt = "updated_at"
        case content
    }
}

public struct ScriptsListResponse: Decodable, Equatable, Sendable {
    public var request: [ScriptInfo]
    public var response: [ScriptInfo]
    public var decode: [ScriptInfo]
    public var parser: [ScriptInfo]

    public func scripts(for type: ScriptType) -> [ScriptInfo] {
        switch type {
        case .request:
            return request
        case .response:
            return response
        case .decode:
            return decode
        case .parser:
            return parser
        }
    }
}

public struct SaveScriptRequest: Encodable, Equatable, Sendable {
    public var content: String
    public var description: String?

    public init(content: String, description: String? = nil) {
        self.content = content
        self.description = description
    }
}

public struct SystemProxyStatus: Decodable, Equatable, Sendable {
    public var supported: Bool
    public var enabled: Bool
    public var host: String?
    public var port: Int?
    public var bypass: String?
    public var managedByBifrost: Bool?
    public var configuredEnabled: Bool?
    public var configuredBypass: String?

    enum CodingKeys: String, CodingKey {
        case supported
        case enabled
        case host
        case port
        case bypass
        case managedByBifrost = "managed_by_bifrost"
        case configuredEnabled = "configured_enabled"
        case configuredBypass = "configured_bypass"
    }
}

public struct SetSystemProxyRequest: Encodable, Equatable, Sendable {
    public var enabled: Bool
    public var bypass: String?

    public init(enabled: Bool, bypass: String? = nil) {
        self.enabled = enabled
        self.bypass = bypass
    }
}

public struct TlsConfig: Codable, Equatable, Sendable {
    public var enableTlsInterception: Bool
    public var interceptExclude: [String]
    public var interceptInclude: [String]
    public var appInterceptExclude: [String]
    public var appInterceptInclude: [String]
    public var ipInterceptExclude: [String]
    public var ipInterceptInclude: [String]
    public var unsafeSsl: Bool
    public var disconnectOnConfigChange: Bool

    enum CodingKeys: String, CodingKey {
        case enableTlsInterception = "enable_tls_interception"
        case interceptExclude = "intercept_exclude"
        case interceptInclude = "intercept_include"
        case appInterceptExclude = "app_intercept_exclude"
        case appInterceptInclude = "app_intercept_include"
        case ipInterceptExclude = "ip_intercept_exclude"
        case ipInterceptInclude = "ip_intercept_include"
        case unsafeSsl = "unsafe_ssl"
        case disconnectOnConfigChange = "disconnect_on_config_change"
    }
}

public struct BreakpointSettings: Codable, Equatable, Sendable {
    public var enabled: Bool
    public var maxBodyBytes: Int

    public init(enabled: Bool, maxBodyBytes: Int) {
        self.enabled = enabled
        self.maxBodyBytes = maxBodyBytes
    }

    enum CodingKeys: String, CodingKey {
        case enabled
        case maxBodyBytes = "max_body_bytes"
    }
}
