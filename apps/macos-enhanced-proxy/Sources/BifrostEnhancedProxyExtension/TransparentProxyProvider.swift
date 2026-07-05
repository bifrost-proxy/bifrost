import Foundation
import NetworkExtension
import Network

final class TransparentProxyProvider: NETransparentProxyProvider {
    private var proxyEndpoint: Network.NWEndpoint?

    override func startProxy(options: [String: Any]? = nil, completionHandler: @escaping (Error?) -> Void) {
        if
            let proto = protocolConfiguration as? NETunnelProviderProtocol,
            let config = proto.providerConfiguration,
            let host = config["proxyHost"] as? String,
            let port = config["proxyPort"] as? Int,
            let nwPort = Network.NWEndpoint.Port(rawValue: UInt16(port))
        {
            proxyEndpoint = .hostPort(host: Network.NWEndpoint.Host(host), port: nwPort)
        }
        completionHandler(nil)
    }

    override func stopProxy(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        proxyEndpoint = nil
        completionHandler()
    }

    override func handleNewFlow(_ flow: NEAppProxyFlow) -> Bool {
        // The signed implementation should bridge eligible TCP flows to the
        // local Bifrost proxy target from providerConfiguration. Until that is
        // wired and approved by macOS, accepting the flow would blackhole it.
        return false
    }
}
