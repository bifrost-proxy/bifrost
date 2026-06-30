import BifrostNativeCore
import Foundation

enum AdminDataSmokeCheck {
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
                async let overview = client.fetchSystemOverview()
                async let traffic = client.fetchTraffic(query: TrafficQuery(limit: 5))
                async let rules = client.fetchRules()
                async let values = client.fetchValues()
                async let systemProxy = client.fetchSystemProxy()
                async let tlsConfig = client.fetchTlsConfig()
                async let breakpointSettings = client.fetchBreakpointSettings()

                let loadedOverview = try await overview
                let loadedTraffic = try await traffic
                let loadedRules = try await rules
                let loadedValues = try await values
                let loadedSystemProxy = try await systemProxy
                let loadedTlsConfig = try await tlsConfig
                let loadedBreakpointSettings = try await breakpointSettings
                let pushClientId = try await checkPushConnection(port: port)
                let pendingAuthSSE = try checkSSEStreamHeader(port: port, path: "/whitelist/pending/stream")
                let pendingIpTlsSSE = try checkSSEStreamHeader(port: port, path: "/config/ip-tls/pending/stream")
                let sseStreams = [pendingAuthSSE, pendingIpTlsSSE].joined(separator: ",")
                let crudSummary = try await checkCoreCRUD(client: client)
                let firstRuleDetail: RuleDetail?
                if let firstRule = loadedRules.first {
                    firstRuleDetail = try await client.fetchRule(name: firstRule.name)
                } else {
                    firstRuleDetail = nil
                }

                print(
                    "Bifrost admin data check passed: port=\(port) " +
                    "pid=\(loadedOverview.system?.pid ?? -1) " +
                    "traffic_records=\(loadedTraffic.records.count) " +
                    "rules=\(loadedRules.count) " +
                    "first_rule_detail=\(firstRuleDetail?.name ?? "-") " +
                    "values=\(loadedValues.total) " +
                    "system_proxy=\(loadedSystemProxy.enabled) " +
                    "tls_decode=\(loadedTlsConfig.enableTlsInterception) " +
                    "breakpoint=\(loadedBreakpointSettings.enabled) " +
                    "push_client_id=\(pushClientId) " +
                    "sse_streams=\(sseStreams) " +
                    "crud=\(crudSummary)"
                )
                Foundation.exit(0)
            } catch {
                fputs("Bifrost admin data check failed: \(error)\n", stderr)
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

    private static func checkPushConnection(port: UInt16) async throws -> Int {
        let pushClient = PushClient(baseURL: URL(string: "http://127.0.0.1:\(port)")!)
        let stream = try await pushClient.connect(subscription: PushSubscription())
        let message = try await firstPushMessage(from: stream, timeoutNanoseconds: 3_000_000_000)
        await pushClient.disconnect()
        guard case .connected(let clientId) = message else {
            throw SmokeCheckError.pushUnexpectedMessage
        }
        return clientId
    }

    private static func checkSSEStreamHeader(port: UInt16, path: String) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
        process.arguments = [
            "-sS",
            "--max-time",
            "2",
            "-D",
            "-",
            "-o",
            "/dev/null",
            "http://127.0.0.1:\(port)/_bifrost/api\(path)",
        ]

        let outputPipe = Pipe()
        let errorPipe = Pipe()
        process.standardOutput = outputPipe
        process.standardError = errorPipe
        try process.run()
        process.waitUntilExit()

        let output = String(data: outputPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let errorOutput = String(data: errorPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let headerLines = output
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        guard let contentType = headerLines.first(where: { $0.localizedCaseInsensitiveContains("content-type:") }),
              contentType.localizedCaseInsensitiveContains("text/event-stream") else {
            throw SmokeCheckError.sseHeaderMissing(path: path, output: output + errorOutput)
        }
        return "\(path)=text/event-stream"
    }

    private static func checkCoreCRUD(client: BifrostClient) async throws -> String {
        let suffix = "\(Int(Date().timeIntervalSince1970 * 1000))"
        let ruleName = "codex_native_smoke_rule_\(suffix)"
        let renamedRuleName = "\(ruleName)_renamed"
        let valueName = "codex_native_smoke_value_\(suffix)"
        let renamedValueName = "\(valueName)_renamed"
        let scriptName = "codex_native_smoke_script_\(suffix)"
        let renamedScriptName = "\(scriptName)_renamed"

        func cleanup() async {
            try? await client.deleteRule(name: ruleName)
            try? await client.deleteRule(name: renamedRuleName)
            try? await client.deleteValue(name: valueName)
            try? await client.deleteValue(name: renamedValueName)
            try? await client.deleteScript(type: .request, name: scriptName)
            try? await client.deleteScript(type: .request, name: renamedScriptName)
        }

        try? await client.deleteRule(name: ruleName)
        try? await client.deleteRule(name: renamedRuleName)
        try? await client.deleteValue(name: valueName)
        try? await client.deleteValue(name: renamedValueName)
        try? await client.deleteScript(type: .request, name: scriptName)
        try? await client.deleteScript(type: .request, name: renamedScriptName)

        do {
            try await client.createRule(name: ruleName, content: "# Native smoke rule\n")
            var rule = try await client.fetchRule(name: ruleName)
            try smokeCheck(rule.content.contains("Native smoke rule"), "created rule content was not persisted")
            try await client.updateRule(name: ruleName, content: "# Native smoke rule updated\n")
            rule = try await client.fetchRule(name: ruleName)
            try smokeCheck(rule.content.contains("updated"), "updated rule content was not persisted")
            try await client.setRuleEnabled(name: ruleName, enabled: false)
            rule = try await client.fetchRule(name: ruleName)
            try smokeCheck(rule.enabled == false, "disabled rule state was not persisted")
            try await client.setRuleEnabled(name: ruleName, enabled: true)
            try await client.renameRule(oldName: ruleName, newName: renamedRuleName)
            rule = try await client.fetchRule(name: renamedRuleName)
            try smokeCheck(rule.name == renamedRuleName, "renamed rule was not readable")
            try await client.deleteRule(name: renamedRuleName)

            try await client.createValue(name: valueName, value: "{\"native\":true}")
            var value = try await client.fetchValue(name: valueName)
            try smokeCheck(value.value.contains("native"), "created value was not persisted")
            try await client.updateValue(name: valueName, value: "{\"native\":\"updated\"}")
            value = try await client.fetchValue(name: valueName)
            try smokeCheck(value.value.contains("updated"), "updated value was not persisted")
            try await client.renameValue(oldName: valueName, newName: renamedValueName)
            value = try await client.fetchValue(name: renamedValueName)
            try smokeCheck(value.name == renamedValueName, "renamed value was not readable")
            try await client.deleteValue(name: renamedValueName)

            _ = try await client.saveScript(
                type: .request,
                name: scriptName,
                content: "function onRequest(request) { return request; }"
            )
            var script = try await client.fetchScript(type: .request, name: scriptName)
            try smokeCheck(script.content.contains("onRequest"), "created request script was not persisted")
            script = try await client.saveScript(
                type: .request,
                name: scriptName,
                content: "function onRequest(request) { request.headers = request.headers || {}; return request; }"
            )
            try smokeCheck(script.content.contains("headers"), "updated request script was not persisted")
            try await client.renameScript(type: .request, oldName: scriptName, newName: renamedScriptName)
            script = try await client.fetchScript(type: .request, name: renamedScriptName)
            try smokeCheck(script.name == renamedScriptName, "renamed request script was not readable")
            try await client.deleteScript(type: .request, name: renamedScriptName)

            await cleanup()
            return "rules,values,scripts"
        } catch {
            await cleanup()
            throw error
        }
    }

    private static func smokeCheck(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() {
            throw SmokeCheckError.crudFailed(message)
        }
    }

    private static func firstPushMessage(
        from stream: AsyncThrowingStream<PushMessage, Error>,
        timeoutNanoseconds: UInt64
    ) async throws -> PushMessage {
        try await withThrowingTaskGroup(of: PushMessage.self) { group in
            group.addTask {
                for try await message in stream {
                    return message
                }
                throw SmokeCheckError.pushDisconnected
            }
            group.addTask {
                try await Task.sleep(nanoseconds: timeoutNanoseconds)
                throw SmokeCheckError.pushTimeout
            }
            guard let message = try await group.next() else {
                throw SmokeCheckError.pushDisconnected
            }
            group.cancelAll()
            return message
        }
    }
}

enum SmokeCheckError: Error {
    case missingSidecar
    case serviceNotRunning
    case pushDisconnected
    case pushTimeout
    case pushUnexpectedMessage
    case sseHeaderMissing(path: String, output: String)
    case crudFailed(String)
}
