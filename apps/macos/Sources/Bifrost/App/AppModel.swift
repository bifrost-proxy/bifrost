import BifrostNativeCore
import Foundation
import SwiftUI

@MainActor
final class AppModel: ObservableObject {
    private enum TrafficSyncPolicy {
        static let initialWindowLimit = 500
        static let historyBatchLimit = 500
        static let maxPendingIds = 500
    }

    @Published var sidecarState: SidecarState = .stopped
    @Published var selectedSidebarItem: SidebarItem = .network
    @Published var colorSchemeMode: ColorSchemeMode = .light
    @Published var isFilterPanelCollapsed = false
    @Published var isDetailPanelCollapsed = false
    @Published var networkToolbarFilters = NetworkToolbarFilters()
    @Published var isNetworkSearchVisible = false
    @Published var networkSearchText = ""
    @Published var selectedTrafficId: String?
    @Published var selectedTrafficDetailText = ""
    @Published var selectedTrafficRequestBodyText = ""
    @Published var selectedTrafficResponseBodyText = ""
    @Published var isLoadingTrafficDetail = false
    @Published var overview: SystemOverview?
    @Published var trafficRecords: [TrafficRecordSummary] = []
    @Published var rules: [RuleSummary] = []
    @Published var values: [ValueItem] = []
    @Published var selectedRuleName: String?
    @Published var selectedRuleDetail: RuleDetail?
    @Published var ruleDraftContent = ""
    @Published var isSavingRule = false
    @Published var selectedValueName: String?
    @Published var selectedValueDraft = ""
    @Published var isSavingValue = false
    @Published var scriptsByType: [ScriptType: [ScriptInfo]] = [:]
    @Published var selectedScriptType: ScriptType = .request
    @Published var selectedScriptName: String?
    @Published var selectedScriptDetail: ScriptDetail?
    @Published var selectedScriptDraft = ""
    @Published var isSavingScript = false
    @Published var systemProxyStatus: SystemProxyStatus?
    @Published var tlsConfig: TlsConfig?
    @Published var breakpointSettings: BreakpointSettings?
    @Published var dataError: String?
    @Published var isLoadingData = false
    @Published var isTogglingSystemProxy = false
    @Published var isTogglingTls = false
    @Published var isTogglingBreakpoint = false
    @Published var realtimeState: RealtimeConnectionState = .disconnected
    @Published var realtimeClientId: Int?
    @Published var realtimeFallbackActive = false
    @Published var lastRealtimeEventAt: Date?

    private let sidecarManager: SidecarManager?
    private var didEnsureService = false
    private var trafficRecordIndexById: [String: Int] = [:]
    private var pushClient: PushClient?
    private var realtimeTask: Task<Void, Never>?
    private var pollingTask: Task<Void, Never>?
    private var trafficDeltaFlushTask: Task<Void, Never>?
    private var trafficHistoryTask: Task<Void, Never>?
    private var pendingTrafficInserts: [TrafficRecordSummary] = []
    private var pendingTrafficUpdates: [TrafficRecordSummary] = []
    private var trafficServerTotal = 0
    private var trafficServerSequence: Int?
    private var trafficHasMore = false
    private var trafficOldestSequence: Int?
    private var pendingTrafficIds = Set<String>()

    init() {
        if let binaryPath = SidecarResolver.resolveBundledBinary()
            ?? SidecarResolver.resolveDevelopmentBinary(packageDirectory: AppModel.packageDirectory())
        {
            sidecarManager = SidecarManager(
                configuration: SidecarConfiguration(binaryPath: binaryPath)
            )
        } else {
            sidecarManager = nil
            sidecarState = .failed("Missing bundled Bifrost CLI sidecar.")
        }
    }

    func openWebUI() {
        NSWorkspace.shared.open(adminURL.appendingPathComponent("_bifrost"))
    }

    func ensureService() async {
        if case .running = sidecarState {
            return
        }
        if case .starting = sidecarState {
            return
        }
        if didEnsureService {
            didEnsureService = false
        }
        guard !didEnsureService else {
            return
        }
        didEnsureService = true

        guard let sidecarManager else {
            sidecarState = .failed("Missing bundled Bifrost CLI sidecar.")
            return
        }

        do {
            try await sidecarManager.ensureRunning()
            sidecarState = await sidecarManager.currentState()
            await refreshData()
            startRealtimeSync()
        } catch {
            didEnsureService = false
            sidecarState = .failed(error.localizedDescription)
        }
    }

    func refreshData(includeTraffic: Bool? = nil) async {
        isLoadingData = true
        defer { isLoadingData = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            async let overview = client.fetchSystemOverview()
            async let rules = client.fetchRules()
            async let systemProxy = client.fetchSystemProxy()
            async let tlsConfig = client.fetchTlsConfig()
            async let breakpointSettings = client.fetchBreakpointSettings()

            var errors: [String] = []
            let shouldLoadTraffic = includeTraffic ?? (selectedSidebarItem == .network)

            do {
                self.overview = try await overview
            } catch {
                errors.append("Overview: \(error.localizedDescription)")
            }

            if shouldLoadTraffic {
                do {
                    try await reloadTrafficFromServer(client: client)
                } catch {
                    errors.append("Traffic: \(error.localizedDescription)")
                }
            }

            do {
                self.rules = try await rules
            } catch {
                errors.append("Rules: \(error.localizedDescription)")
            }

            do {
                self.systemProxyStatus = try await systemProxy
            } catch {
                errors.append("System Proxy: \(error.localizedDescription)")
            }

            do {
                self.tlsConfig = try await tlsConfig
            } catch {
                errors.append("TLS: \(error.localizedDescription)")
            }

            do {
                self.breakpointSettings = try await breakpointSettings
            } catch {
                errors.append("Breakpoint: \(error.localizedDescription)")
            }

            if selectedRuleName == nil {
                selectedRuleName = self.rules.first?.name
            }
            self.dataError = errors.isEmpty ? nil : errors.joined(separator: " · ")
            if shouldLoadTraffic {
                await selectInitialTrafficRecordIfNeeded()
            }
            updateRealtimeSubscription()
        } catch {
            self.dataError = error.localizedDescription
        }
    }

    func handleSidebarSelectionChanged() async {
        clearPendingTrafficDelta()
        if selectedSidebarItem != .network {
            trafficHistoryTask?.cancel()
            trafficHistoryTask = nil
        }
        updateRealtimeSubscription()
        await refreshData(includeTraffic: selectedSidebarItem == .network)
    }

    func selectRule(_ name: String) async {
        selectedRuleName = name
        do {
            let client = try BifrostClient(baseURL: adminURL)
            selectedRuleDetail = try await client.fetchRule(name: name)
            ruleDraftContent = selectedRuleDetail?.content ?? ""
            dataError = nil
        } catch {
            selectedRuleDetail = nil
            ruleDraftContent = ""
            dataError = error.localizedDescription
        }
    }

    func selectValue(_ name: String) {
        selectedValueName = name
        selectedValueDraft = values.first { $0.name == name }?.value ?? ""
    }

    func createRule(name: String) async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Rule name is required."
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.createRule(name: trimmed, content: "# New rule\n")
            await refreshData()
            await selectRule(trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func saveSelectedRule(content: String) async {
        guard let name = selectedRuleName else {
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.updateRule(name: name, content: content)
            await refreshData()
            await selectRule(name)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func setSelectedRuleEnabled(_ enabled: Bool) async {
        guard let name = selectedRuleName else {
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.setRuleEnabled(name: name, enabled: enabled)
            await refreshData()
            await selectRule(name)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func renameSelectedRule(to newName: String) async {
        guard let oldName = selectedRuleName else {
            return
        }
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Rule name is required."
            return
        }
        guard trimmed != oldName else {
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.renameRule(oldName: oldName, newName: trimmed)
            selectedRuleName = trimmed
            await refreshData()
            await selectRule(trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func deleteSelectedRule() async {
        guard let name = selectedRuleName else {
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.deleteRule(name: name)
            selectedRuleName = nil
            selectedRuleDetail = nil
            ruleDraftContent = ""
            await refreshData()
            if let nextName = rules.first?.name {
                await selectRule(nextName)
            }
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func createValue(name: String) async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Value name is required."
            return
        }
        isSavingValue = true
        defer { isSavingValue = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.createValue(name: trimmed, value: "")
            await refreshData()
            selectValue(trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func saveSelectedValue(content: String) async {
        guard let name = selectedValueName else {
            return
        }
        isSavingValue = true
        defer { isSavingValue = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.updateValue(name: name, value: content)
            await refreshData()
            selectValue(name)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func renameSelectedValue(to newName: String) async {
        guard let oldName = selectedValueName else {
            return
        }
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Value name is required."
            return
        }
        guard trimmed != oldName else {
            return
        }
        isSavingValue = true
        defer { isSavingValue = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.renameValue(oldName: oldName, newName: trimmed)
            selectedValueName = trimmed
            await refreshData()
            selectValue(trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func deleteSelectedValue() async {
        guard let name = selectedValueName else {
            return
        }
        isSavingValue = true
        defer { isSavingValue = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.deleteValue(name: name)
            selectedValueName = nil
            selectedValueDraft = ""
            await refreshData()
            if let nextName = values.first?.name {
                selectValue(nextName)
            }
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func selectScriptType(_ type: ScriptType) async {
        selectedScriptType = type
        let nextName = scriptsByType[type]?.first?.name
        selectedScriptName = nextName
        selectedScriptDetail = nil
        selectedScriptDraft = ""
        if let nextName {
            await selectScript(type: type, name: nextName)
        }
    }

    func selectScript(type: ScriptType, name: String) async {
        selectedScriptType = type
        selectedScriptName = name
        do {
            let client = try BifrostClient(baseURL: adminURL)
            selectedScriptDetail = try await client.fetchScript(type: type, name: name)
            selectedScriptDraft = selectedScriptDetail?.content ?? ""
            dataError = nil
        } catch {
            selectedScriptDetail = nil
            selectedScriptDraft = ""
            dataError = error.localizedDescription
        }
    }

    func createScript(type: ScriptType, name: String) async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Script name is required."
            return
        }
        isSavingScript = true
        defer { isSavingScript = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            _ = try await client.saveScript(type: type, name: trimmed, content: defaultScriptContent(for: type))
            await refreshData()
            await selectScript(type: type, name: trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func saveSelectedScript(content: String) async {
        guard let name = selectedScriptName else {
            return
        }
        let type = selectedScriptType
        isSavingScript = true
        defer { isSavingScript = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            selectedScriptDetail = try await client.saveScript(type: type, name: name, content: content)
            selectedScriptDraft = selectedScriptDetail?.content ?? content
            await refreshData()
            selectedScriptType = type
            selectedScriptName = name
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func renameSelectedScript(to newName: String) async {
        guard let oldName = selectedScriptName else {
            return
        }
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            dataError = "Script name is required."
            return
        }
        guard trimmed != oldName else {
            return
        }
        let type = selectedScriptType
        isSavingScript = true
        defer { isSavingScript = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.renameScript(type: type, oldName: oldName, newName: trimmed)
            await refreshData()
            await selectScript(type: type, name: trimmed)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func deleteSelectedScript() async {
        guard let name = selectedScriptName else {
            return
        }
        let type = selectedScriptType
        isSavingScript = true
        defer { isSavingScript = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.deleteScript(type: type, name: name)
            selectedScriptName = nil
            selectedScriptDetail = nil
            selectedScriptDraft = ""
            await refreshData()
            if let nextName = scriptsByType[type]?.first?.name {
                await selectScript(type: type, name: nextName)
            }
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func setSystemProxyEnabled(_ enabled: Bool) async {
        guard !isTogglingSystemProxy else {
            return
        }
        isTogglingSystemProxy = true
        defer { isTogglingSystemProxy = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            systemProxyStatus = try await client.setSystemProxy(enabled: enabled)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func setTlsInterceptionEnabled(_ enabled: Bool) async {
        guard !isTogglingTls else {
            return
        }
        guard var tlsConfig else {
            await refreshData()
            return
        }
        isTogglingTls = true
        defer { isTogglingTls = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            tlsConfig.enableTlsInterception = enabled
            self.tlsConfig = try await client.updateTlsConfig(tlsConfig)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func setBreakpointEnabled(_ enabled: Bool) async {
        guard !isTogglingBreakpoint else {
            return
        }
        guard var breakpointSettings else {
            await refreshData()
            return
        }
        isTogglingBreakpoint = true
        defer { isTogglingBreakpoint = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            breakpointSettings.enabled = enabled
            self.breakpointSettings = try await client.updateBreakpointSettings(breakpointSettings)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func clearTraffic() async {
        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await client.clearTraffic()
            clearPendingTrafficDelta()
            trafficRecords = []
            trafficRecordIndexById = [:]
            dataError = nil
            await refreshData()
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func selectTrafficRecord(_ record: TrafficRecordSummary) async {
        selectedTrafficId = record.id
        isLoadingTrafficDetail = true
        defer { isLoadingTrafficDetail = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            async let detail = client.getTraffic(id: record.id)
            async let requestBody = client.getRequestBody(id: record.id)
            async let responseBody = client.getResponseBody(id: record.id)

            let loadedDetail = try await detail
            let loadedRequestBody = try await requestBody
            let loadedResponseBody = try await responseBody
            selectedTrafficDetailText = prettyPayload(loadedDetail)
            selectedTrafficRequestBodyText = bodyPreview(loadedRequestBody)
            selectedTrafficResponseBodyText = bodyPreview(loadedResponseBody)
            dataError = nil
        } catch {
            selectedTrafficDetailText = ""
            selectedTrafficRequestBodyText = ""
            selectedTrafficResponseBodyText = ""
            dataError = error.localizedDescription
        }
    }

    func toggleNetworkToolbarFilter(group: NetworkToolbarFilterGroup, tag: String) {
        networkToolbarFilters.toggle(group: group, tag: tag)
    }

    var displayedTrafficRecords: [TrafficRecordSummary] {
        if networkToolbarFilters.isEmpty && networkSearchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return trafficRecords
        }
        return trafficRecords.filter { record in
            networkToolbarFilters.matches(record)
                && matchesSearchText(record)
        }
    }

    var adminURL: URL {
        if case .running(let port) = sidecarState {
            return URL(string: "http://127.0.0.1:\(port)")!
        }
        return URL(string: "http://127.0.0.1:9900")!
    }

    var adminHostPortLabel: String {
        guard let host = adminURL.host else {
            return adminURL.absoluteString
        }
        if let port = adminURL.port {
            return "\(host):\(port)"
        }
        return host
    }

    var clientIpCounts: [(name: String, count: Int)] {
        countedValues(trafficRecords.compactMap { record in
            guard let clientIp = record.clientIp, !clientIp.isEmpty else {
                return nil
            }
            return clientIp
        })
    }

    var clientAppCounts: [(name: String, count: Int)] {
        countedValues(trafficRecords.compactMap { record in
            guard let clientApp = record.clientApp, !clientApp.isEmpty else {
                return nil
            }
            return clientApp
        })
    }

    var domainCounts: [(name: String, count: Int)] {
        countedValues(trafficRecords.compactMap { record in
            guard let host = record.host, !host.isEmpty else {
                return nil
            }
            return host
        })
    }

    private static func packageDirectory() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private func defaultScriptContent(for type: ScriptType) -> String {
        switch type {
        case .request:
            return """
            // Modify outbound requests here.
            function onRequest(request) {
              return request;
            }
            """
        case .response:
            return """
            // Modify inbound responses here.
            function onResponse(response) {
              return response;
            }
            """
        case .decode:
            return """
            // Decode custom payloads here.
            function onDecode(payload) {
              return payload;
            }
            """
        case .parser:
            return """
            // Parse captured traffic here.
            function onParse(record) {
              return record;
            }
            """
        }
    }

    private func countedValues(_ values: [String]) -> [(name: String, count: Int)] {
        Dictionary(grouping: values, by: { $0 })
            .map { (name: $0.key, count: $0.value.count) }
            .sorted { lhs, rhs in
                if lhs.count != rhs.count {
                    return lhs.count > rhs.count
                }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
    }

    private func matchesSearchText(_ record: TrafficRecordSummary) -> Bool {
        let keyword = networkSearchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !keyword.isEmpty else {
            return true
        }
        let haystack = [
            record.seq.map(String.init),
            record.method,
            record.host,
            record.path,
            record.protocolName,
            record.clientApp,
            record.clientIp,
            record.status.map(String.init),
            record.listenerPort.map(String.init),
        ]
        .compactMap { $0 }
        .joined(separator: " ")
        .localizedLowercase
        return haystack.contains(keyword.localizedLowercase)
    }

    private func prettyPayload(_ data: Data) -> String {
        if let object = try? JSONSerialization.jsonObject(with: data),
           let prettyData = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]),
           let text = String(data: prettyData, encoding: .utf8) {
            return text
        }
        return String(data: data, encoding: .utf8) ?? "\(data.count) bytes"
    }

    private func bodyPreview(_ data: Data) -> String {
        let text = prettyPayload(data)
        guard text.count > 2_400 else {
            return text
        }
        return String(text.prefix(2_400)) + "\n..."
    }

    private func reloadTrafficFromServer(client: BifrostClient) async throws {
        trafficHistoryTask?.cancel()
        trafficHistoryTask = nil
        clearPendingTrafficDelta()

        let response = try await client.fetchTrafficUpdates(
            query: TrafficUpdatesQuery(limit: TrafficSyncPolicy.initialWindowLimit)
        )
        let initialRecords = deduplicatedTrafficRecords(response.newRecords + response.updatedRecords)
            .sorted(by: trafficRecordSortOrder)

        trafficRecords = initialRecords
        rebuildTrafficRecordIndex()
        trafficServerTotal = response.serverTotal
        trafficServerSequence = response.serverSequence
        trafficHasMore = response.hasMore
        refreshTrafficBoundaryState()
        rebuildPendingTrafficIds()
        updateRealtimeSubscription()

        if response.hasMore {
            startTrafficHistoryBackfill()
        }
    }

    private func startTrafficHistoryBackfill() {
        trafficHistoryTask?.cancel()
        trafficHistoryTask = Task { [weak self] in
            await self?.backfillTrafficHistory()
        }
    }

    private func backfillTrafficHistory() async {
        while !Task.isCancelled {
            guard selectedSidebarItem == .network,
                  trafficHasMore,
                  let cursor = trafficOldestSequence else {
                return
            }

            do {
                let client = try BifrostClient(baseURL: adminURL)
                let response = try await client.fetchTraffic(
                    query: TrafficQuery(
                        limit: TrafficSyncPolicy.historyBatchLimit,
                        cursor: cursor,
                        direction: "backward"
                    )
                )

                if Task.isCancelled {
                    return
                }

                mergeTrafficRecords(updates: [], inserts: response.records)
                trafficServerTotal = response.total ?? trafficServerTotal
                trafficServerSequence = response.serverSequence ?? trafficServerSequence
                trafficHasMore = response.hasMore ?? false
                refreshTrafficBoundaryState()
                updateRealtimeSubscription()

                if response.records.isEmpty || !trafficHasMore {
                    return
                }
                await Task.yield()
            } catch {
                dataError = "Traffic history: \(error.localizedDescription)"
                return
            }
        }
    }

    private func startRealtimeSync() {
        realtimeTask?.cancel()
        pollingTask?.cancel()
        trafficDeltaFlushTask?.cancel()
        trafficHistoryTask?.cancel()
        trafficDeltaFlushTask = nil
        trafficHistoryTask = nil
        pendingTrafficInserts.removeAll(keepingCapacity: true)
        pendingTrafficUpdates.removeAll(keepingCapacity: true)
        realtimeState = .connecting
        realtimeFallbackActive = false

        pollingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                guard let self else { return }
                await self.refreshDataFromFallback()
            }
        }

        realtimeTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                do {
                    self.setRealtimeState(.connecting)
                    let client = PushClient(baseURL: self.adminURL)
                    self.setPushClient(client)
                    let stream = try await client.connect(subscription: self.makePushSubscription())

                    for try await message in stream {
                        if Task.isCancelled {
                            break
                        }
                        await self.handlePushMessage(message)
                    }
                    self.setRealtimeState(.reconnecting)
                } catch {
                    self.setRealtimeError(error.localizedDescription)
                }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }

    private func refreshDataFromFallback() async {
        guard case .running = sidecarState else {
            return
        }
        realtimeFallbackActive = realtimeState != .connected
        if realtimeFallbackActive {
            await refreshData(includeTraffic: selectedSidebarItem == .network)
        }
    }

    private func setPushClient(_ client: PushClient) {
        pushClient = client
    }

    private func setRealtimeState(_ state: RealtimeConnectionState) {
        realtimeState = state
        realtimeFallbackActive = state != .connected
    }

    private func setRealtimeError(_ message: String) {
        realtimeState = .failed(message)
        realtimeFallbackActive = true
    }

    private func handlePushMessage(_ message: PushMessage) async {
        lastRealtimeEventAt = Date()
        switch message {
        case .connected(let clientId):
            realtimeClientId = clientId
            realtimeState = .connected
            realtimeFallbackActive = false
            updateRealtimeSubscription()
        case .trafficDelta(let data):
            if selectedSidebarItem == .network {
                enqueueTrafficDelta(data)
            }
        case .trafficDeleted(let data):
            flushPendingTrafficDelta()
            removeTraffic(ids: data.ids)
            updateRealtimeSubscription()
        case .valuesUpdate:
            break
        case .settingsUpdate(let data):
            applySettingsUpdate(data)
        case .breakpointSettingsUpdated(let data):
            breakpointSettings = BreakpointSettings(enabled: data.enabled, maxBodyBytes: data.maxBodyBytes)
        case .disconnect(let reason):
            realtimeState = .failed(reason ?? "Server requested refresh")
            realtimeFallbackActive = true
        case .ignored:
            break
        }
    }

    private func enqueueTrafficDelta(_ data: TrafficDeltaData) {
        trafficServerTotal = data.serverTotal
        trafficServerSequence = data.serverSequence ?? trafficServerSequence
        pendingTrafficUpdates.append(contentsOf: data.updates)
        pendingTrafficInserts.append(contentsOf: data.inserts)
        guard trafficDeltaFlushTask == nil else {
            return
        }
        trafficDeltaFlushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 16_000_000)
            if Task.isCancelled {
                return
            }
            self?.flushPendingTrafficDelta()
        }
    }

    private func flushPendingTrafficDelta() {
        trafficDeltaFlushTask?.cancel()
        trafficDeltaFlushTask = nil
        guard !pendingTrafficUpdates.isEmpty || !pendingTrafficInserts.isEmpty else {
            return
        }
        let updates = pendingTrafficUpdates
        let inserts = pendingTrafficInserts
        pendingTrafficUpdates.removeAll(keepingCapacity: true)
        pendingTrafficInserts.removeAll(keepingCapacity: true)
        mergeTrafficRecords(updates: updates, inserts: inserts)
        updateRealtimeSubscription()
    }

    private func clearPendingTrafficDelta() {
        trafficDeltaFlushTask?.cancel()
        trafficDeltaFlushTask = nil
        pendingTrafficUpdates.removeAll(keepingCapacity: true)
        pendingTrafficInserts.removeAll(keepingCapacity: true)
    }

    private func mergeTrafficRecords(updates: [TrafficRecordSummary], inserts: [TrafficRecordSummary]) {
        if trafficRecordIndexById.count != trafficRecords.count {
            trafficRecords = deduplicatedTrafficRecords(trafficRecords)
            rebuildTrafficRecordIndex()
        }
        var needsSort = false
        var lastSequence = trafficRecords.last?.seq ?? Int.min

        for record in updates {
            if let index = trafficRecordIndexById[record.id] {
                trafficRecords[index] = mergeTrafficRecord(existing: trafficRecords[index], incoming: record)
            } else {
                trafficRecordIndexById[record.id] = trafficRecords.count
                trafficRecords.append(record)
                let sequence = record.seq ?? Int.max
                if sequence < lastSequence {
                    needsSort = true
                }
                lastSequence = max(lastSequence, sequence)
            }
        }
        for record in inserts {
            if let index = trafficRecordIndexById[record.id] {
                trafficRecords[index] = mergeTrafficRecord(existing: trafficRecords[index], incoming: record)
            } else {
                trafficRecordIndexById[record.id] = trafficRecords.count
                trafficRecords.append(record)
                let sequence = record.seq ?? Int.max
                if sequence < lastSequence {
                    needsSort = true
                }
                lastSequence = max(lastSequence, sequence)
            }
        }

        if needsSort {
            trafficRecords.sort(by: trafficRecordSortOrder)
            rebuildTrafficRecordIndex()
        }
        refreshTrafficBoundaryState()
        rebuildPendingTrafficIds()

        Task {
            await selectInitialTrafficRecordIfNeeded()
        }
    }

    private func mergeTrafficRecord(
        existing: TrafficRecordSummary?,
        incoming: TrafficRecordSummary
    ) -> TrafficRecordSummary {
        guard let existing else {
            return incoming
        }
        return TrafficRecordSummary(
            id: incoming.id,
            seq: incoming.seq ?? existing.seq,
            method: incoming.method ?? existing.method,
            host: incoming.host ?? existing.host,
            path: incoming.path ?? existing.path,
            status: incoming.status ?? existing.status,
            contentType: incoming.contentType ?? existing.contentType,
            responseSize: incoming.responseSize ?? existing.responseSize,
            durationMs: incoming.durationMs ?? existing.durationMs,
            listenerPort: incoming.listenerPort ?? existing.listenerPort,
            protocolName: incoming.protocolName ?? existing.protocolName,
            clientApp: incoming.clientApp ?? existing.clientApp,
            clientIp: incoming.clientIp ?? existing.clientIp,
            startTime: incoming.startTime ?? existing.startTime,
            endTime: incoming.endTime ?? existing.endTime,
            flags: incoming.flags ?? existing.flags,
            matchedRuleCount: incoming.matchedRuleCount ?? existing.matchedRuleCount,
            matchedProtocols: incoming.matchedProtocols.isEmpty ? existing.matchedProtocols : incoming.matchedProtocols
        )
    }

    private func removeTraffic(ids: [String]) {
        guard !ids.isEmpty else {
            return
        }
        let deleted = Set(ids)
        trafficRecords.removeAll { deleted.contains($0.id) }
        rebuildTrafficRecordIndex()
        refreshTrafficBoundaryState()
        rebuildPendingTrafficIds()
        trafficServerTotal = max(trafficServerTotal - ids.count, 0)
        if let selectedTrafficId, deleted.contains(selectedTrafficId) {
            self.selectedTrafficId = nil
            selectedTrafficDetailText = ""
            selectedTrafficRequestBodyText = ""
            selectedTrafficResponseBodyText = ""
        }
    }

    private func applySettingsUpdate(_ update: SettingsUpdateData) {
        let decoder = JSONDecoder()
        switch update.scope {
        case "system_proxy":
            systemProxyStatus = try? decoder.decode(SystemProxyStatus.self, from: update.data)
        case "tls_config":
            tlsConfig = try? decoder.decode(TlsConfig.self, from: update.data)
        default:
            break
        }
    }

    private func makePushSubscription() -> PushSubscription {
        let trafficEnabled = selectedSidebarItem == .network
        let lastRecord = trafficEnabled ? trafficRecords.last : nil
        return PushSubscription(
            lastTrafficId: lastRecord?.id,
            lastSequence: lastRecord?.seq,
            pendingIds: trafficEnabled ? Array(pendingTrafficIds) : [],
            needTraffic: trafficEnabled,
            needValues: false,
            needScripts: false,
            settingsScopes: [
                "system_proxy",
                "tls_config",
            ]
        )
    }

    private func updateRealtimeSubscription() {
        guard realtimeState == .connected,
              let pushClient else {
            return
        }
        let subscription = makePushSubscription()
        Task {
            try? await pushClient.send(subscription: subscription)
        }
    }

    private func selectInitialTrafficRecordIfNeeded() async {
        guard selectedTrafficId == nil,
              let firstRecord = displayedTrafficRecords.first ?? trafficRecords.first else {
            return
        }
        await selectTrafficRecord(firstRecord)
    }

    private func rebuildTrafficRecordIndex() {
        var indexById: [String: Int] = [:]
        indexById.reserveCapacity(trafficRecords.count)
        for (offset, record) in trafficRecords.enumerated() {
            indexById[record.id] = offset
        }
        trafficRecordIndexById = indexById
    }

    private func refreshTrafficBoundaryState() {
        trafficOldestSequence = trafficRecords.first?.seq
    }

    private func rebuildPendingTrafficIds() {
        var ids = Set<String>()
        ids.reserveCapacity(min(trafficRecords.count, TrafficSyncPolicy.maxPendingIds))
        for record in trafficRecords where isPendingTrafficRecord(record) {
            ids.insert(record.id)
            if ids.count >= TrafficSyncPolicy.maxPendingIds {
                break
            }
        }
        pendingTrafficIds = ids
    }

    private func isPendingTrafficRecord(_ record: TrafficRecordSummary) -> Bool {
        guard let status = record.status else {
            return true
        }
        return status == 0
    }

    private func deduplicatedTrafficRecords(_ records: [TrafficRecordSummary]) -> [TrafficRecordSummary] {
        var indexById: [String: Int] = [:]
        var result: [TrafficRecordSummary] = []
        result.reserveCapacity(records.count)
        for record in records {
            if let index = indexById[record.id] {
                result[index] = mergeTrafficRecord(existing: result[index], incoming: record)
            } else {
                indexById[record.id] = result.count
                result.append(record)
            }
        }
        return result
    }

    private func trafficRecordSortOrder(_ lhs: TrafficRecordSummary, _ rhs: TrafficRecordSummary) -> Bool {
        switch (lhs.seq, rhs.seq) {
        case let (left?, right?) where left != right:
            return left < right
        case (_?, nil):
            return true
        case (nil, _?):
            return false
        default:
            return lhs.id.localizedStandardCompare(rhs.id) == .orderedAscending
        }
    }
}

private extension ScriptsListResponse {
    func asDictionary() -> [ScriptType: [ScriptInfo]] {
        [
            .request: request.sortedByName(),
            .response: response.sortedByName(),
            .decode: decode.sortedByName(),
            .parser: parser.sortedByName(),
        ]
    }
}

private extension Array where Element == ScriptInfo {
    func sortedByName() -> [ScriptInfo] {
        sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }
}

enum RealtimeConnectionState: Equatable, Sendable {
    case disconnected
    case connecting
    case connected
    case reconnecting
    case failed(String)

    var label: String {
        switch self {
        case .disconnected:
            return "Sync: Off"
        case .connecting:
            return "Sync: Connecting"
        case .connected:
            return "Sync: Live"
        case .reconnecting:
            return "Sync: Reconnecting"
        case .failed:
            return "Sync: Fallback"
        }
    }

    var isConnected: Bool {
        if case .connected = self {
            return true
        }
        return false
    }
}

enum NetworkToolbarFilterGroup: String, CaseIterable, Sendable {
    case rule
    case networkProtocol
    case type
    case status
    case imported
}

struct NetworkToolbarFilters: Equatable, Sendable {
    var rule: String?
    var networkProtocol: String?
    var type: String?
    var status: String?
    var imported: String?

    var isEmpty: Bool {
        rule == nil
            && networkProtocol == nil
            && type == nil
            && status == nil
            && imported == nil
    }

    func selectedTag(for group: NetworkToolbarFilterGroup) -> String? {
        switch group {
        case .rule:
            return rule
        case .networkProtocol:
            return networkProtocol
        case .type:
            return type
        case .status:
            return status
        case .imported:
            return imported
        }
    }

    mutating func toggle(group: NetworkToolbarFilterGroup, tag: String) {
        let nextValue = selectedTag(for: group) == tag ? nil : tag
        switch group {
        case .rule:
            rule = nextValue
        case .networkProtocol:
            networkProtocol = nextValue
        case .type:
            type = nextValue
        case .status:
            status = nextValue
        case .imported:
            imported = nextValue
        }
    }

    func matches(_ record: TrafficRecordSummary) -> Bool {
        if rule != nil {
            return false
        }
        if let networkProtocol,
           record.protocolName?.localizedUppercase != networkProtocol {
            return false
        }
        if let type,
           inferredType(for: record).localizedUppercase != type.localizedUppercase {
            return false
        }
        if let status,
           !matchesStatus(status, recordStatus: record.status) {
            return false
        }
        if imported != nil,
           !record.id.hasPrefix("OUT-") {
            return false
        }
        return true
    }

    private func matchesStatus(_ statusFilter: String, recordStatus: Int?) -> Bool {
        guard let recordStatus else {
            return statusFilter == "error"
        }
        switch statusFilter {
        case "1xx":
            return (100..<200).contains(recordStatus)
        case "2xx":
            return (200..<300).contains(recordStatus)
        case "3xx":
            return (300..<400).contains(recordStatus)
        case "4xx":
            return (400..<500).contains(recordStatus)
        case "5xx":
            return (500..<600).contains(recordStatus)
        case "error":
            return recordStatus == 0 || recordStatus >= 600
        default:
            return true
        }
    }

    private func inferredType(for record: TrafficRecordSummary) -> String {
        guard let path = record.path?.localizedLowercase else {
            return "-"
        }
        if path.contains(".js") { return "JS" }
        if path.contains(".css") { return "CSS" }
        if path.contains(".png") || path.contains(".jpg") || path.contains(".jpeg") || path.contains(".webp") || path.contains(".gif") {
            return "Media"
        }
        if path.contains(".woff") || path.contains(".ttf") || path.contains(".otf") {
            return "Font"
        }
        if path.contains(".html") || path.contains(".htm") || path.contains(".pdf") {
            return "Doc"
        }
        if record.method == "CONNECT" {
            return "-"
        }
        return "JSON"
    }
}

enum ColorSchemeMode: String, CaseIterable, Identifiable {
    case light = "Light"
    case dark = "Dark"

    var id: String { rawValue }

    var colorScheme: ColorScheme {
        switch self {
        case .light:
            return .light
        case .dark:
            return .dark
        }
    }

    var next: ColorSchemeMode {
        switch self {
        case .light:
            return .dark
        case .dark:
            return .light
        }
    }

    var systemImage: String {
        switch self {
        case .light:
            return "sun.max"
        case .dark:
            return "moon"
        }
    }
}
