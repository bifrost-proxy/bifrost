import Foundation
import NetworkExtension
import Network

final class TransparentProxyProvider: NETransparentProxyProvider {
    private let queue = DispatchQueue(label: "com.bifrost.proxy.enhanced.flow")
    private var proxyHost = "127.0.0.1"
    private var proxyPort: UInt16 = 9900
    private var relays: [UUID: TCPFlowRelay] = [:]

    override func startProxy(options: [String: Any]? = nil, completionHandler: @escaping (Error?) -> Void) {
        if
            let proto = protocolConfiguration as? NETunnelProviderProtocol,
            let config = proto.providerConfiguration,
            let host = config["proxyHost"] as? String,
            let port = config["proxyPort"] as? Int,
            let validPort = UInt16(exactly: port)
        {
            proxyHost = host
            proxyPort = validPort
        }
        completionHandler(nil)
    }

    override func stopProxy(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        queue.sync {
            relays.values.forEach { $0.cancel() }
            relays.removeAll()
        }
        completionHandler()
    }

    override func handleNewFlow(_ flow: NEAppProxyFlow) -> Bool {
        guard let tcpFlow = flow as? NEAppProxyTCPFlow else {
            return false
        }
        guard let target = Self.targetEndpoint(for: tcpFlow) else {
            return false
        }

        let relayID = UUID()
        let relay = TCPFlowRelay(
            flow: tcpFlow,
            targetHost: target.host,
            targetPort: target.port,
            proxyHost: proxyHost,
            proxyPort: proxyPort,
            queue: queue
        ) { [weak self] in
            self?.queue.async {
                self?.relays.removeValue(forKey: relayID)
            }
        }

        queue.async {
            self.relays[relayID] = relay
            relay.start()
        }
        return true
    }

    private static func targetEndpoint(for flow: NEAppProxyTCPFlow) -> (host: String, port: UInt16)? {
        if #available(macOS 15.0, *) {
            switch flow.remoteFlowEndpoint {
            case .hostPort(let host, let port):
                return (hostString(host), port.rawValue)
            default:
                return nil
            }
        }
        return nil
    }

    private static func hostString(_ host: Network.NWEndpoint.Host) -> String {
        switch host {
        case .name(let name, _):
            return name
        case .ipv4(let address):
            return "\(address)"
        case .ipv6(let address):
            return "\(address)"
        @unknown default:
            return "\(host)"
        }
    }
}

private final class TCPFlowRelay {
    private let flow: NEAppProxyTCPFlow
    private let targetHost: String
    private let targetPort: UInt16
    private let proxyHost: String
    private let proxyPort: UInt16
    private let queue: DispatchQueue
    private let connection: NWConnection
    private let onClose: () -> Void
    private var proxyReadBuffer = Data()
    private var closed = false

    init(
        flow: NEAppProxyTCPFlow,
        targetHost: String,
        targetPort: UInt16,
        proxyHost: String,
        proxyPort: UInt16,
        queue: DispatchQueue,
        onClose: @escaping () -> Void
    ) {
        self.flow = flow
        self.targetHost = targetHost
        self.targetPort = targetPort
        self.proxyHost = proxyHost
        self.proxyPort = proxyPort
        self.queue = queue
        self.onClose = onClose
        self.connection = NWConnection(
            host: NWEndpoint.Host(proxyHost),
            port: NWEndpoint.Port(rawValue: proxyPort)!,
            using: .tcp
        )
    }

    func start() {
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.openFlow()
            case .failed(let error):
                self.close(error)
            case .cancelled:
                self.close(nil)
            default:
                break
            }
        }
        connection.start(queue: queue)
    }

    func cancel() {
        close(nil)
    }

    private func openFlow() {
        flow.open(withLocalEndpoint: nil) { [weak self] error in
            guard let self else { return }
            if let error {
                self.close(error)
                return
            }
            self.performSocksHandshake()
        }
    }

    private func performSocksHandshake() {
        sendToProxy(Data([0x05, 0x01, 0x00])) { [weak self] error in
            guard let self else { return }
            if let error {
                self.close(error)
                return
            }
            self.readFromProxy(count: 2) { greeting in
                guard greeting.count == 2, greeting[0] == 0x05, greeting[1] == 0x00 else {
                    self.close(nil)
                    return
                }
                guard let request = self.makeSocksConnectRequest() else {
                    self.close(nil)
                    return
                }
                self.sendToProxy(request) { error in
                    if let error {
                        self.close(error)
                        return
                    }
                    self.readSocksConnectResponse()
                }
            }
        }
    }

    private func readSocksConnectResponse() {
        readFromProxy(count: 4) { [weak self] header in
            guard let self else { return }
            guard header.count == 4, header[0] == 0x05, header[1] == 0x00 else {
                self.close(nil)
                return
            }
            let addressLength: Int
            switch header[3] {
            case 0x01:
                addressLength = 4
            case 0x03:
                self.readFromProxy(count: 1) { domainLength in
                    guard domainLength.count == 1 else {
                        self.close(nil)
                        return
                    }
                    self.readFromProxy(count: Int(domainLength[0]) + 2) { _ in
                        self.startPumps()
                    }
                }
                return
            case 0x04:
                addressLength = 16
            default:
                self.close(nil)
                return
            }
            readFromProxy(count: addressLength + 2) { [weak self] _ in
                self?.startPumps()
            }
        }
    }

    private func startPumps() {
        pumpFlowToProxy()
        pumpProxyToFlow()
    }

    private func pumpFlowToProxy() {
        guard !closed else { return }
        flow.readData { [weak self] data, error in
            guard let self else { return }
            if let error {
                self.close(error)
                return
            }
            guard let data, !data.isEmpty else {
                self.close(nil)
                return
            }
            self.sendToProxy(data) { error in
                if let error {
                    self.close(error)
                    return
                }
                self.pumpFlowToProxy()
            }
        }
    }

    private func pumpProxyToFlow() {
        guard !closed else { return }
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.close(error)
                return
            }
            if let data, !data.isEmpty {
                self.flow.write(data) { error in
                    if let error {
                        self.close(error)
                        return
                    }
                    if isComplete {
                        self.close(nil)
                    } else {
                        self.pumpProxyToFlow()
                    }
                }
                return
            }
            if isComplete {
                self.close(nil)
            } else {
                self.pumpProxyToFlow()
            }
        }
    }

    private func sendToProxy(_ data: Data, completion: @escaping (NWError?) -> Void) {
        connection.send(content: data, completion: .contentProcessed(completion))
    }

    private func readFromProxy(count: Int, completion: @escaping (Data) -> Void) {
        if proxyReadBuffer.count >= count {
            let prefix = proxyReadBuffer.prefix(count)
            proxyReadBuffer.removeFirst(count)
            completion(Data(prefix))
            return
        }

        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let error {
                self.close(error)
                return
            }
            if let data, !data.isEmpty {
                self.proxyReadBuffer.append(data)
                self.readFromProxy(count: count, completion: completion)
                return
            }
            if isComplete {
                self.close(nil)
            } else {
                self.readFromProxy(count: count, completion: completion)
            }
        }
    }

    private func makeSocksConnectRequest() -> Data? {
        guard let hostBytes = targetHost.data(using: .utf8), hostBytes.count <= 255 else {
            return nil
        }
        var request = Data([0x05, 0x01, 0x00, 0x03, UInt8(hostBytes.count)])
        request.append(hostBytes)
        request.append(UInt8((targetPort >> 8) & 0xff))
        request.append(UInt8(targetPort & 0xff))
        return request
    }

    private func close(_ error: Error?) {
        guard !closed else { return }
        closed = true
        connection.cancel()
        flow.closeReadWithError(error)
        flow.closeWriteWithError(error)
        onClose()
    }
}
