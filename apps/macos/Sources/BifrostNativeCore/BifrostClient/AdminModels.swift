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

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        seq = try container.decodeIfPresent(Int.self, forKey: .seq)
        method = try container.decodeIfPresent(String.self, forKey: .method)
        host = try container.decodeIfPresent(String.self, forKey: .host)
        path = try container.decodeIfPresent(String.self, forKey: .path)
        status = try container.decodeIfPresent(Int.self, forKey: .status)
        contentType = try container.decodeIfPresent(String.self, forKey: .contentType)
        responseSize = try container.decodeIfPresent(Int.self, forKey: .responseSize)
        durationMs = try container.decodeIfPresent(Int.self, forKey: .durationMs)
        listenerPort = try container.decodeIfPresent(Int.self, forKey: .listenerPort)
        protocolName = try container.decodeIfPresent(String.self, forKey: .protocolName)
        clientApp = try container.decodeIfPresent(String.self, forKey: .clientApp)
        clientIp = try container.decodeIfPresent(String.self, forKey: .clientIp)
        startTime = try container.decodeIfPresent(String.self, forKey: .startTime)
        endTime = try container.decodeIfPresent(String.self, forKey: .endTime)
        flags = try container.decodeIfPresent(Int.self, forKey: .flags)
        matchedRuleCount = try container.decodeIfPresent(Int.self, forKey: .matchedRuleCount)
        matchedProtocols = try container.decodeIfPresent([String].self, forKey: .matchedProtocols) ?? []
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

public struct SystemProxyLaunchdStatus: Decodable, Equatable, Sendable {
    public var supported: Bool
    public var installed: Bool
    public var loaded: Bool
    public var label: String?
    public var plistPath: String?
    public var program: String?
    public var dataDir: String?
    public var installedVersion: String?
    public var installedMode: String?
    public var currentVersion: String?
    public var needsUpgrade: Bool?
    public var needsUpgradeReason: String?
    public var message: String?

    enum CodingKeys: String, CodingKey {
        case supported
        case installed
        case loaded
        case label
        case plistPath = "plist_path"
        case program
        case dataDir = "data_dir"
        case installedVersion = "installed_version"
        case installedMode = "installed_mode"
        case currentVersion = "current_version"
        case needsUpgrade = "needs_upgrade"
        case needsUpgradeReason = "needs_upgrade_reason"
        case message
    }
}

public struct CliProxyStatus: Decodable, Equatable, Sendable {
    public var enabled: Bool
    public var shell: String?
    public var configFiles: [String]
    public var proxyURL: String?

    enum CodingKeys: String, CodingKey {
        case enabled
        case shell
        case configFiles = "config_files"
        case proxyURL = "proxy_url"
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

public struct SetSystemProxyLaunchdRequest: Encodable, Equatable, Sendable {
    public var enabled: Bool

    public init(enabled: Bool) {
        self.enabled = enabled
    }
}

public struct ProxyAddress: Decodable, Equatable, Identifiable, Sendable {
    public var ip: String
    public var address: String
    public var qrcodeURL: String?
    public var isPreferred: Bool

    public var id: String { address }

    enum CodingKeys: String, CodingKey {
        case ip
        case address
        case qrcodeURL = "qrcode_url"
        case isPreferred = "is_preferred"
    }
}

public struct ProxyAddressInfo: Decodable, Equatable, Sendable {
    public var port: Int
    public var localIPs: [String]
    public var addresses: [ProxyAddress]

    enum CodingKeys: String, CodingKey {
        case port
        case localIPs = "local_ips"
        case addresses
    }
}

public struct CertInfo: Decodable, Equatable, Sendable {
    public var available: Bool
    public var status: String
    public var statusLabel: String
    public var installed: Bool
    public var trusted: Bool
    public var statusMessage: String
    public var sha256Fingerprint: String?
    public var localIPs: [String]
    public var downloadURLs: [String]
    public var qrcodeURLs: [String]

    enum CodingKeys: String, CodingKey {
        case available
        case status
        case statusLabel = "status_label"
        case installed
        case trusted
        case statusMessage = "status_message"
        case sha256Fingerprint = "sha256_fingerprint"
        case localIPs = "local_ips"
        case downloadURLs = "download_urls"
        case qrcodeURLs = "qrcode_urls"
    }
}

public struct LocalCAInstallRequest: Encodable, Equatable, Sendable {
    public var confirmation: String

    public init(confirmation: String = "install_local_ca_certificate") {
        self.confirmation = confirmation
    }
}

public struct MobileDevicesResponse: Decodable, Equatable, Sendable {
    public var android: MobileDiscovery?
    public var ios: MobileDiscovery?
    public var iosProfileURL: String?
    public var iosProfileQRCodeURL: String?
    public var iosWifiProxyProfileURL: String?
    public var iosWifiProxyProfileQRCodeURL: String?
    public var suggestedWifiSSID: String?
    public var suggestedWifiSSIDMessage: String?
    public var ordinaryDeviceNotice: String?
    public var managedDeviceNotice: String?

    enum CodingKeys: String, CodingKey {
        case android
        case ios
        case iosProfileURL = "ios_profile_url"
        case iosProfileQRCodeURL = "ios_profile_qrcode_url"
        case iosWifiProxyProfileURL = "ios_wifi_proxy_profile_url"
        case iosWifiProxyProfileQRCodeURL = "ios_wifi_proxy_profile_qrcode_url"
        case suggestedWifiSSID = "suggested_wifi_ssid"
        case suggestedWifiSSIDMessage = "suggested_wifi_ssid_message"
        case ordinaryDeviceNotice = "ordinary_device_notice"
        case managedDeviceNotice = "managed_device_notice"
    }
}

public struct MobileDiscovery: Decodable, Equatable, Sendable {
    public var supported: Bool?
    public var adbAvailable: Bool?
    public var devices: [MobileDevice]
    public var message: String

    enum CodingKeys: String, CodingKey {
        case supported
        case adbAvailable = "adb_available"
        case devices
        case message
    }
}

public struct MobileDevice: Decodable, Equatable, Identifiable, Sendable {
    public var id: String
    public var name: String?
    public var managedInstallTarget: String?
    public var platform: String
    public var status: String
    public var capability: String
    public var statusMessage: String

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case managedInstallTarget = "managed_install_target"
        case platform
        case status
        case capability
        case statusMessage = "status_message"
    }
}

public struct SyncUser: Decodable, Equatable, Sendable {
    public var userID: String
    public var nickname: String?
    public var avatar: String?
    public var email: String?

    enum CodingKeys: String, CodingKey {
        case userID = "user_id"
        case nickname
        case avatar
        case email
    }
}

public struct SyncStatus: Decodable, Equatable, Sendable {
    public var enabled: Bool
    public var autoSync: Bool
    public var remoteBaseURL: String
    public var hasSession: Bool
    public var reachable: Bool
    public var authorized: Bool
    public var syncing: Bool
    public var reason: String
    public var lastSyncAt: String?
    public var lastSyncAction: String?
    public var lastError: String?
    public var user: SyncUser?

    enum CodingKeys: String, CodingKey {
        case enabled
        case autoSync = "auto_sync"
        case remoteBaseURL = "remote_base_url"
        case hasSession = "has_session"
        case reachable
        case authorized
        case syncing
        case reason
        case lastSyncAt = "last_sync_at"
        case lastSyncAction = "last_sync_action"
        case lastError = "last_error"
        case user
    }
}

public struct UpdateSyncConfigRequest: Encodable, Equatable, Sendable {
    public var enabled: Bool?
    public var autoSync: Bool?
    public var remoteBaseURL: String?

    public init(enabled: Bool? = nil, autoSync: Bool? = nil, remoteBaseURL: String? = nil) {
        self.enabled = enabled
        self.autoSync = autoSync
        self.remoteBaseURL = remoteBaseURL
    }

    enum CodingKeys: String, CodingKey {
        case enabled
        case autoSync = "auto_sync"
        case remoteBaseURL = "remote_base_url"
    }
}

public struct RemoteInvokeStatus: Decodable, Equatable, Sendable {
    public var state: String
    public var discoverySession: DiscoverySession?
    public var pendingPairingsCount: Int
    public var activeCallIDs: [String]

    enum CodingKeys: String, CodingKey {
        case state
        case discoverySession = "discovery_session"
        case pendingPairingsCount = "pending_pairings_count"
        case activeCallIDs = "active_call_ids"
    }
}

public struct DiscoverySession: Decodable, Equatable, Sendable {
    public var sessionID: String
    public var pairCode: String
    public var expiresAt: Double
    public var createdAt: Double

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case pairCode = "pair_code"
        case expiresAt = "expires_at"
        case createdAt = "created_at"
    }
}

public struct ClientIdentity: Decodable, Equatable, Sendable {
    public var instanceID: String
    public var deviceName: String
    public var platform: String

    enum CodingKeys: String, CodingKey {
        case instanceID = "instance_id"
        case deviceName = "device_name"
        case platform
    }
}

public struct DiscoveryResponse: Decodable, Equatable, Sendable {
    public var success: Bool
    public var session: DiscoverySession
}

public struct PendingPairingsResponse: Decodable, Equatable, Sendable {
    public var pairings: [PairingRequest]
}

public struct PairingRequest: Decodable, Equatable, Identifiable, Sendable {
    public var pairingID: String
    public var callerInfo: CallerInfo
    public var commandSummary: CommandSummary
    public var callerPubkey: String?

    public var id: String { pairingID }

    enum CodingKeys: String, CodingKey {
        case pairingID = "pairing_id"
        case callerInfo = "caller_info"
        case commandSummary = "command_summary"
        case callerPubkey = "caller_pubkey"
    }
}

public struct CallerInfo: Decodable, Equatable, Sendable {
    public var fingerprint: String
    public var displayName: String?
    public var userAgent: String?
    public var sourceIP: String?
    public var platform: String?

    enum CodingKeys: String, CodingKey {
        case fingerprint
        case displayName = "display_name"
        case userAgent = "user_agent"
        case sourceIP = "source_ip"
        case platform
    }
}

public struct CommandSummary: Decodable, Equatable, Sendable {
    public var commandPreview: String
    public var maskedArgsJSON: String?
    public var payloadDigest: String?
    public var payloadSize: Int?

    enum CodingKeys: String, CodingKey {
        case commandPreview = "command_preview"
        case maskedArgsJSON = "masked_args_json"
        case payloadDigest = "payload_digest"
        case payloadSize = "payload_size"
    }
}

public struct PairingApprovalInput: Encodable, Equatable, Sendable {
    public var grantMode: String
    public var grantScope: String?
    public var interactiveAllowed: Bool?
    public var stdinAllowed: Bool?
    public var fileAccess: String?

    public init(
        grantMode: String = "1h",
        grantScope: String? = "remote_query",
        interactiveAllowed: Bool? = false,
        stdinAllowed: Bool? = false,
        fileAccess: String? = "none"
    ) {
        self.grantMode = grantMode
        self.grantScope = grantScope
        self.interactiveAllowed = interactiveAllowed
        self.stdinAllowed = stdinAllowed
        self.fileAccess = fileAccess
    }

    enum CodingKeys: String, CodingKey {
        case grantMode = "grant_mode"
        case grantScope = "grant_scope"
        case interactiveAllowed = "interactive_allowed"
        case stdinAllowed = "stdin_allowed"
        case fileAccess = "file_access"
    }
}

public struct Grant: Decodable, Equatable, Identifiable, Sendable {
    public var grantID: String
    public var callerFingerprint: String
    public var callerDisplayName: String?
    public var authMethod: String?
    public var grantMode: String
    public var grantScope: String
    public var status: String
    public var createdAt: Double?
    public var expiresAt: Double?
    public var lastUsedAt: Double?
    public var useCount: Int?

    public var id: String { grantID }

    enum CodingKeys: String, CodingKey {
        case grantID = "grant_id"
        case callerFingerprint = "caller_fingerprint"
        case callerDisplayName = "caller_display_name"
        case authMethod = "auth_method"
        case grantMode = "grant_mode"
        case grantScope = "grant_scope"
        case status
        case createdAt = "created_at"
        case expiresAt = "expires_at"
        case lastUsedAt = "last_used_at"
        case useCount = "use_count"
    }
}

public struct GrantsListResponse: Decodable, Equatable, Sendable {
    public var grants: [Grant]
}

public struct RemoteCall: Decodable, Equatable, Identifiable, Sendable {
    public var callID: String
    public var grantID: String?
    public var callerFingerprint: String?
    public var callerDisplayName: String?
    public var status: String
    public var commandSummary: CommandSummary?
    public var commandKind: String?
    public var createdAt: Double?
    public var finishedAt: Double?
    public var exitCode: Int?
    public var durationMs: Int?

    public var id: String { callID }

    enum CodingKeys: String, CodingKey {
        case callID = "call_id"
        case grantID = "grant_id"
        case callerFingerprint = "caller_fingerprint"
        case callerDisplayName = "caller_display_name"
        case status
        case commandSummary = "command_summary"
        case commandKind = "command_kind"
        case createdAt = "created_at"
        case finishedAt = "finished_at"
        case exitCode = "exit_code"
        case durationMs = "duration_ms"
    }
}

public struct CallsListResponse: Decodable, Equatable, Sendable {
    public var calls: [RemoteCall]
    public var nextCursor: Double?
    public var limit: Int?

    enum CodingKeys: String, CodingKey {
        case calls
        case nextCursor = "next_cursor"
        case limit
    }
}

public struct RemoteInvokeSshCallerInfo: Decodable, Equatable, Sendable {
    public var hostname: String?
    public var username: String?
    public var platform: String?
    public var userAgent: String?
    public var sourceIP: String?
    public var ip: String?

    enum CodingKeys: String, CodingKey {
        case hostname
        case username
        case platform
        case userAgent = "user_agent"
        case sourceIP = "source_ip"
        case ip
    }
}

public struct RemoteInvokeSshKeyRecord: Decodable, Equatable, Sendable {
    public var id: String?
    public var label: String?
    public var deviceCode: String
    public var sshKeyFingerprint: String?
    public var status: String?
    public var grantMode: String?
    public var createdAt: FlexibleString?
    public var lastUsedAt: FlexibleString?
    public var lastCallerInfo: RemoteInvokeSshCallerInfo?

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case deviceCode = "device_code"
        case sshKeyFingerprint = "ssh_key_fingerprint"
        case status
        case grantMode = "grant_mode"
        case createdAt = "created_at"
        case lastUsedAt = "last_used_at"
        case lastCallerInfo = "last_caller_info"
    }
}

public struct RemoteInvokeSshKeySecretPayload: Decodable, Equatable, Sendable {
    public var id: String?
    public var label: String?
    public var deviceCode: String
    public var sshKeyFingerprint: String
    public var bifrostKeyFile: String
    public var publicKeyPEM: String?
    public var grantMode: String?

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case deviceCode = "device_code"
        case sshKeyFingerprint = "ssh_key_fingerprint"
        case bifrostKeyFile = "bifrost_key_file"
        case publicKeyPEM = "public_key_pem"
        case grantMode = "grant_mode"
    }
}

public struct CreateRemoteInvokeSshKeyInput: Encodable, Equatable, Sendable {
    public var label: String
    public var grantMode: String

    public init(label: String, grantMode: String = "permanent") {
        self.label = label
        self.grantMode = grantMode
    }

    enum CodingKeys: String, CodingKey {
        case label
        case grantMode = "grant_mode"
    }
}

public struct FlexibleString: Decodable, Equatable, Sendable, CustomStringConvertible {
    public var value: String

    public var description: String { value }

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let string = try? container.decode(String.self) {
            value = string
        } else if let double = try? container.decode(Double.self) {
            value = String(double)
        } else if let int = try? container.decode(Int.self) {
            value = String(int)
        } else {
            value = ""
        }
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
