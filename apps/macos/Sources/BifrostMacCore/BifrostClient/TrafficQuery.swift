import Foundation

public struct TrafficQuery: Equatable, Sendable {
    public var limit: Int
    public var cursor: Int?
    public var method: String?
    public var host: String?
    public var status: Int?
    public var protocolName: String?
    public var listenerPort: Int?
    public var clientApp: String?

    public init(
        limit: Int = 100,
        cursor: Int? = nil,
        method: String? = nil,
        host: String? = nil,
        status: Int? = nil,
        protocolName: String? = nil,
        listenerPort: Int? = nil,
        clientApp: String? = nil
    ) {
        self.limit = limit
        self.cursor = cursor
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
