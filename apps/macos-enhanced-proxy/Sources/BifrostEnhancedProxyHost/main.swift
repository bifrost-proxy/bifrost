import Foundation
import NetworkExtension

struct EnhancedProxyConfig: Codable {
    var proxyHost: String
    var proxyPort: Int
    var includedPorts: [Int]
    var excludedApps: [String]
}

let managerDescription = "Bifrost Enhanced Proxy"
let providerBundleIdentifier = "com.bifrost.proxy.enhanced.network-extension"

func loadManagers() async throws -> [NETransparentProxyManager] {
    try await withCheckedThrowingContinuation { continuation in
        NETransparentProxyManager.loadAllFromPreferences { managers, error in
            if let error {
                continuation.resume(throwing: error)
            } else {
                continuation.resume(returning: managers ?? [])
            }
        }
    }
}

func save(_ manager: NETransparentProxyManager) async throws {
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
        manager.saveToPreferences { error in
            if let error {
                continuation.resume(throwing: error)
            } else {
                continuation.resume(returning: ())
            }
        }
    }
}

func remove(_ manager: NETransparentProxyManager) async throws {
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
        manager.removeFromPreferences { error in
            if let error {
                continuation.resume(throwing: error)
            } else {
                continuation.resume(returning: ())
            }
        }
    }
}

func makeProtocolConfiguration(config: EnhancedProxyConfig) -> NETunnelProviderProtocol {
    let proto = NETunnelProviderProtocol()
    proto.providerBundleIdentifier = providerBundleIdentifier
    proto.serverAddress = "\(config.proxyHost):\(config.proxyPort)"
    proto.providerConfiguration = [
        "proxyHost": config.proxyHost,
        "proxyPort": config.proxyPort,
        "includedPorts": config.includedPorts,
        "excludedApps": config.excludedApps
    ]
    return proto
}

func enable(config: EnhancedProxyConfig) async throws {
    let existing = try await loadManagers()
    let manager = existing.first { $0.localizedDescription == managerDescription } ?? NETransparentProxyManager()
    manager.localizedDescription = managerDescription
    manager.protocolConfiguration = makeProtocolConfiguration(config: config)
    manager.isEnabled = true
    try await save(manager)
    print("configured")
}

func disable() async throws {
    for manager in try await loadManagers() where manager.localizedDescription == managerDescription {
        manager.isEnabled = false
        try await save(manager)
        print("disabled")
        return
    }
    print("not_configured")
}

func status() async throws {
    let managers = try await loadManagers()
    if let manager = managers.first(where: { $0.localizedDescription == managerDescription }) {
        print(manager.isEnabled ? "enabled" : "disabled")
    } else {
        print("not_configured")
    }
}

@main
enum Main {
    static func main() async {
        let args = CommandLine.arguments.dropFirst()
        do {
            switch args.first {
            case "enable":
                let config = EnhancedProxyConfig(
                    proxyHost: ProcessInfo.processInfo.environment["BIFROST_PROXY_HOST"] ?? "127.0.0.1",
                    proxyPort: Int(ProcessInfo.processInfo.environment["BIFROST_PROXY_PORT"] ?? "9900") ?? 9900,
                    includedPorts: [80, 443],
                    excludedApps: ["bifrost", "Bifrost", "Bifrost Enhanced Proxy"]
                )
                try await enable(config: config)
            case "disable":
                try await disable()
            case "status":
                try await status()
            default:
                print("usage: BifrostEnhancedProxyHost enable|disable|status")
                Foundation.exit(2)
            }
        } catch {
            fputs("error: \(error)\n", stderr)
            Foundation.exit(1)
        }
    }
}
