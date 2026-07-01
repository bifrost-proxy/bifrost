import Foundation

public struct TrafficQuery: Equatable, Sendable {
    public static let defaultServerRetainedLimit = 5_000

    public var limit: Int
    public var cursor: Int?
    public var direction: String?
    public var method: String?
    public var host: String?
    public var status: Int?
    public var protocolName: String?
    public var listenerPort: Int?
    public var clientApp: String?

    public init(
        limit: Int = TrafficQuery.defaultServerRetainedLimit,
        cursor: Int? = nil,
        direction: String? = nil,
        method: String? = nil,
        host: String? = nil,
        status: Int? = nil,
        protocolName: String? = nil,
        listenerPort: Int? = nil,
        clientApp: String? = nil
    ) {
        self.limit = max(1, limit)
        self.cursor = cursor
        self.direction = direction
        self.method = method
        self.host = host
        self.status = status
        self.protocolName = protocolName
        self.listenerPort = listenerPort
        self.clientApp = clientApp
    }

    public var queryItems: [URLQueryItem] {
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        if let cursor {
            items.append(URLQueryItem(name: "cursor", value: String(cursor)))
        }
        if let direction, !direction.isEmpty {
            items.append(URLQueryItem(name: "direction", value: direction))
        }
        if let method, !method.isEmpty {
            items.append(URLQueryItem(name: "method", value: method))
        }
        if let host, !host.isEmpty {
            items.append(URLQueryItem(name: "host", value: host))
        }
        if let status {
            items.append(URLQueryItem(name: "status", value: String(status)))
        }
        if let protocolName, !protocolName.isEmpty {
            items.append(URLQueryItem(name: "protocol", value: protocolName))
        }
        if let listenerPort {
            items.append(URLQueryItem(name: "listener_port", value: String(listenerPort)))
        }
        if let clientApp, !clientApp.isEmpty {
            items.append(URLQueryItem(name: "client_app", value: clientApp))
        }
        return items
    }
}

public struct TrafficUpdatesQuery: Equatable, Sendable {
    public var afterId: String?
    public var afterSequence: Int?
    public var pendingIds: [String]
    public var limit: Int

    public init(
        afterId: String? = nil,
        afterSequence: Int? = nil,
        pendingIds: [String] = [],
        limit: Int
    ) {
        self.afterId = afterId
        self.afterSequence = afterSequence
        self.pendingIds = pendingIds
        self.limit = max(1, limit)
    }

    public var queryItems: [URLQueryItem] {
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        if let afterId, !afterId.isEmpty {
            items.append(URLQueryItem(name: "after_id", value: afterId))
        }
        if let afterSequence {
            items.append(URLQueryItem(name: "after_seq", value: String(afterSequence)))
        }
        if !pendingIds.isEmpty {
            items.append(URLQueryItem(name: "pending_ids", value: pendingIds.joined(separator: ",")))
        }
        return items
    }
}
