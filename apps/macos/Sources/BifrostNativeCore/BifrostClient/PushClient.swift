import Foundation

public struct PushSubscription: Equatable, Sendable {
    public var lastTrafficId: String?
    public var lastSequence: Int?
    public var pendingIds: [String]
    public var needTraffic: Bool
    public var needOverview: Bool
    public var needMetrics: Bool
    public var needValues: Bool
    public var needScripts: Bool
    public var settingsScopes: [String]
    public var metricsIntervalMs: Int?

    public init(
        lastTrafficId: String? = nil,
        lastSequence: Int? = nil,
        pendingIds: [String] = [],
        needTraffic: Bool = true,
        needOverview: Bool = true,
        needMetrics: Bool = true,
        needValues: Bool = true,
        needScripts: Bool = true,
        settingsScopes: [String] = [
            "system_proxy",
            "tls_config",
            "proxy_address",
            "notifications",
        ],
        metricsIntervalMs: Int? = nil
    ) {
        self.lastTrafficId = lastTrafficId
        self.lastSequence = lastSequence
        self.pendingIds = pendingIds
        self.needTraffic = needTraffic
        self.needOverview = needOverview
        self.needMetrics = needMetrics
        self.needValues = needValues
        self.needScripts = needScripts
        self.settingsScopes = settingsScopes
        self.metricsIntervalMs = metricsIntervalMs
    }
}

public actor PushClient {
    private let baseURL: URL
    private let clientId: String
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private let tracePush = ProcessInfo.processInfo.environment["BIFROST_NATIVE_TRACE_PUSH"] == "1"

    public init(
        baseURL: URL,
        clientId: String = "bifrost-mac-native",
        session: URLSession = .shared
    ) {
        self.baseURL = baseURL
        self.clientId = clientId
        self.session = session
    }

    public func connect(subscription: PushSubscription = PushSubscription()) throws -> AsyncThrowingStream<PushMessage, Error> {
        disconnect()
        let request = URLRequest(url: try makePushURL(subscription: subscription))
        let task = session.webSocketTask(with: request)
        self.task = task
        task.resume()
        try send(subscription: subscription)
        trace("connected to \(request.url?.absoluteString ?? "<invalid-url>")")

        return AsyncThrowingStream { continuation in
            let receiveTask = Task {
                await receiveLoop(task: task, continuation: continuation)
            }
            continuation.onTermination = { _ in
                receiveTask.cancel()
                task.cancel(with: .goingAway, reason: nil)
            }
        }
    }

    public func send(subscription: PushSubscription) throws {
        guard let task else {
            return
        }
        let payload = try JSONSerialization.data(withJSONObject: subscriptionPayload(subscription))
        guard let text = String(data: payload, encoding: .utf8) else {
            return
        }
        task.send(.string(text)) { _ in }
    }

    public func disconnect() {
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    private func receiveLoop(
        task: URLSessionWebSocketTask,
        continuation: AsyncThrowingStream<PushMessage, Error>.Continuation
    ) async {
        do {
            while !Task.isCancelled {
                let message = try await task.receive()
                switch message {
                case .string(let text):
                    if let data = text.data(using: .utf8),
                       let message = decodePushMessage(data) {
                        trace("message \(message.traceType)")
                        continuation.yield(message)
                    }
                case .data(let data):
                    if let message = decodePushMessage(data) {
                        trace("message \(message.traceType)")
                        continuation.yield(message)
                    }
                @unknown default:
                    break
                }
            }
            continuation.finish()
        } catch {
            trace("receive failed: \(error.localizedDescription)")
            continuation.finish(throwing: error)
        }
    }

    private func decodePushMessage(_ data: Data) -> PushMessage? {
        try? JSONDecoder().decode(PushMessage.self, from: data)
    }

    private func trace(_ message: String) {
        guard tracePush else {
            return
        }
        NSLog("[bifrost-native-push] %@", message)
    }

    private func makePushURL(subscription: PushSubscription) throws -> URL {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
        components?.scheme = baseURL.scheme == "https" ? "wss" : "ws"
        let basePath = components?.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? ""
        components?.path = "/" + [basePath, "_bifrost/api/push"]
            .filter { !$0.isEmpty }
            .joined(separator: "/")

        var queryItems = [
            URLQueryItem(name: "x_client_id", value: clientId),
        ]
        queryItems.append(contentsOf: subscriptionQueryItems(subscription))
        components?.queryItems = queryItems

        guard let url = components?.url else {
            throw AdminAPIError.invalidURL("\(baseURL)/_bifrost/api/push")
        }
        return url
    }

    private func subscriptionQueryItems(_ subscription: PushSubscription) -> [URLQueryItem] {
        var items: [URLQueryItem] = []
        if let lastTrafficId = subscription.lastTrafficId {
            items.append(URLQueryItem(name: "last_traffic_id", value: lastTrafficId))
        }
        if let lastSequence = subscription.lastSequence {
            items.append(URLQueryItem(name: "last_sequence", value: "\(lastSequence)"))
        }
        if !subscription.pendingIds.isEmpty {
            items.append(URLQueryItem(name: "pending_ids", value: subscription.pendingIds.joined(separator: ",")))
        }
        if subscription.needTraffic {
            items.append(URLQueryItem(name: "need_traffic", value: "true"))
        }
        if subscription.needOverview {
            items.append(URLQueryItem(name: "need_overview", value: "true"))
        }
        if subscription.needMetrics {
            items.append(URLQueryItem(name: "need_metrics", value: "true"))
        }
        if subscription.needValues {
            items.append(URLQueryItem(name: "need_values", value: "true"))
        }
        if subscription.needScripts {
            items.append(URLQueryItem(name: "need_scripts", value: "true"))
        }
        if !subscription.settingsScopes.isEmpty {
            items.append(URLQueryItem(name: "settings_scopes", value: subscription.settingsScopes.joined(separator: ",")))
        }
        if let metricsIntervalMs = subscription.metricsIntervalMs {
            items.append(URLQueryItem(name: "metrics_interval_ms", value: "\(metricsIntervalMs)"))
        }
        return items
    }

    private func subscriptionPayload(_ subscription: PushSubscription) -> [String: Any] {
        var payload: [String: Any] = [
            "need_traffic": subscription.needTraffic,
            "need_overview": subscription.needOverview,
            "need_metrics": subscription.needMetrics,
            "need_values": subscription.needValues,
            "need_scripts": subscription.needScripts,
        ]
        if let lastTrafficId = subscription.lastTrafficId {
            payload["last_traffic_id"] = lastTrafficId
        }
        if let lastSequence = subscription.lastSequence {
            payload["last_sequence"] = lastSequence
        }
        if !subscription.pendingIds.isEmpty {
            payload["pending_ids"] = subscription.pendingIds
        }
        if !subscription.settingsScopes.isEmpty {
            payload["settings_scopes"] = subscription.settingsScopes
        }
        if let metricsIntervalMs = subscription.metricsIntervalMs {
            payload["metrics_interval_ms"] = metricsIntervalMs
        }
        return payload
    }
}

private extension PushMessage {
    var traceType: String {
        switch self {
        case .connected:
            return "connected"
        case .trafficDelta:
            return "traffic_delta"
        case .trafficDeleted:
            return "traffic_deleted"
        case .overviewUpdate:
            return "overview_update"
        case .metricsUpdate:
            return "metrics_update"
        case .valuesUpdate:
            return "values_update"
        case .settingsUpdate:
            return "settings_update"
        case .breakpointSettingsUpdated:
            return "breakpoint_settings_updated"
        case .disconnect:
            return "disconnect"
        case .ignored(let type):
            return "ignored:\(type)"
        }
    }
}
