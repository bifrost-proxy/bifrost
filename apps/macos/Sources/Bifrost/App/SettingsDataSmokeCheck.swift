import BifrostNativeCore
import Foundation

enum SettingsDataSmokeCheck {
    static func run() {
        Task {
            do {
                let binaryPath = SidecarResolver.resolveBundledBinary()
                    ?? SidecarResolver.resolveDevelopmentBinary(packageDirectory: packageDirectory())
                guard let binaryPath else {
                    throw SmokeCheckError.missingSidecar
                }

                let manager = SidecarManager(
                    configuration: SidecarConfiguration(binaryPath: binaryPath)
                )
                try await manager.ensureRunning()
                let state = await manager.currentState()
                guard case .running(let port) = state else {
                    throw SmokeCheckError.serviceNotRunning
                }

                let client = try BifrostClient(baseURL: URL(string: "http://127.0.0.1:\(port)")!)

                async let systemProxy = client.fetchSystemProxy()
                async let launchd = client.fetchSystemProxyLaunchd()
                async let cliProxy = client.fetchCliProxy()
                async let proxyAddress = client.fetchProxyAddress()
                async let certInfo = client.fetchCertInfo()
                async let mobileDevices = client.fetchMobileDevices()
                async let syncStatus = client.fetchSyncStatus()
                async let remoteStatus = client.fetchRemoteInvokeStatus()
                async let identity = client.fetchClientIdentity()
                async let pairings = client.fetchPendingPairings()
                async let grants = client.fetchRemoteInvokeGrants()
                async let calls = client.fetchRemoteInvokeCalls(limit: 5)

                let loadedSystemProxy = try await systemProxy
                let loadedLaunchd = try await launchd
                let loadedCliProxy = try await cliProxy
                let loadedProxyAddress = try await proxyAddress
                let loadedCertInfo = try await certInfo
                let loadedMobileDevices = try await mobileDevices
                let loadedSyncStatus = try await syncStatus
                let loadedRemoteStatus = try await remoteStatus
                let loadedIdentity = try await identity
                let loadedPairings = try await pairings
                let loadedGrants = try await grants
                let loadedCalls = try await calls
                let sshKeyAvailable = (try? await client.fetchRemoteInvokeSshKey()) != nil

                print(
                    "Bifrost settings data check passed: port=\(port) " +
                    "proxy_supported=\(loadedSystemProxy.supported) " +
                    "proxy_addresses=\(loadedProxyAddress.addresses.count) " +
                    "launchd_supported=\(loadedLaunchd.supported) " +
                    "cli_proxy=\(loadedCliProxy.enabled) " +
                    "cert_status=\(loadedCertInfo.status) " +
                    "mobile_android=\(loadedMobileDevices.android?.devices.count ?? 0) " +
                    "mobile_ios=\(loadedMobileDevices.ios?.devices.count ?? 0) " +
                    "sync_reason=\(loadedSyncStatus.reason) " +
                    "remote_state=\(loadedRemoteStatus.state) " +
                    "remote_identity=\(loadedIdentity.instanceID) " +
                    "pending_pairings=\(loadedPairings.pairings.count) " +
                    "grants=\(loadedGrants.grants.count) " +
                    "calls=\(loadedCalls.calls.count) " +
                    "ssh_key=\(sshKeyAvailable)"
                )
                Foundation.exit(0)
            } catch {
                fputs("Bifrost settings data check failed: \(error)\n", stderr)
                Foundation.exit(1)
            }
        }
        dispatchMain()
    }

    private static func packageDirectory() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }
}
