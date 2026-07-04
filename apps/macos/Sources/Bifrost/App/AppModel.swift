import BifrostNativeCore
import AppKit
import Foundation
import SwiftUI

enum RuleMovePlacement {
    case before
    case after
}

enum NativeAppUpdateState: Equatable {
    case idle
    case available(latestVersion: String)
    case installing(latestVersion: String)
    case restarting(latestVersion: String)
    case failed(message: String, latestVersion: String?)
}

private enum NativeAppUpdateError: LocalizedError {
    case installTimedOut(String)

    var errorDescription: String? {
        switch self {
        case .installTimedOut(let message):
            "Native app installation timed out: \(message)"
        }
    }
}

@MainActor
final class AppModel: ObservableObject {
    private enum TrafficSyncPolicy {
        static let initialWindowLimit = 160
        static let historyBatchLimit = 500
        static let maxNativeRecords = 240
        static let maxPendingIds = 500
        static let trafficDeltaFlushDelayNanoseconds: UInt64 = 500_000_000
        static let metricsPublishInterval: TimeInterval = 1.0
        static let activityAppMetricsRefreshInterval: TimeInterval = 10.0
        static let fallbackActivityAppMetricsRefreshInterval: TimeInterval = 30.0
        static let fallbackPollingIntervalNanoseconds: UInt64 = 8_000_000_000
        static let realtimeEventPublishInterval: TimeInterval = 30.0
        static let subscriptionDebounceNanoseconds: UInt64 = 200_000_000
        static let realtimeMetricsIntervalMs = 1_000
    }

    @Published var sidecarState: SidecarState = .stopped
    @Published var selectedSidebarItem: SidebarItem = .activity
    @Published var colorSchemeMode: ColorSchemeMode = .system
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
    @Published var ruleGroups: [RuleGroup] = []
    @Published var selectedRuleGroupID: String?
    @Published var activeRuleGroupName: String?
    @Published var activeRuleGroupWritable = false
    @Published var isLoadingRuleGroups = false
    @Published var activeRulesSummary: ActiveRulesSummary?
    @Published var values: [ValueItem] = []
    @Published var selectedRuleName: String?
    @Published var selectedRuleDetail: RuleDetail?
    @Published var ruleDraftContent = ""
    @Published var isSavingRule = false
    @Published var isAutoSavingRule = false
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
    @Published var systemProxyLaunchdStatus: SystemProxyLaunchdStatus?
    @Published var cliProxyStatus: CliProxyStatus?
    @Published var proxyAddressInfo: ProxyAddressInfo?
    @Published var syncStatus: SyncStatus?
    @Published var performanceConfig: PerformanceConfigResponse?
    @Published var tlsConfig: TlsConfig?
    @Published var breakpointSettings: BreakpointSettings?
    @Published var dataError: String?
    @Published var isLoadingData = false
    @Published var isTogglingSystemProxy = false
    @Published var isTogglingSystemProxyLaunchd = false
    @Published var isTogglingInjectBifrostBadge = false
    @Published var isTogglingTls = false
    @Published var isTogglingBreakpoint = false
    @Published var realtimeState: RealtimeConnectionState = .disconnected
    @Published var realtimeClientId: Int?
    @Published var realtimeFallbackActive = false
    @Published var lastRealtimeEventAt: Date?
    @Published var nativeAppUpdateState: NativeAppUpdateState = .idle
    @Published private(set) var activityClientAppCounts: [(name: String, count: Int)] = []
    @Published private(set) var activityClientIpCounts: [(name: String, count: Int)] = []
    @Published private(set) var activityRuleHitCount = 0

    private let sidecarManager: SidecarManager?
    private var didEnsureService = false
    private var trafficRecordIndexById: [String: Int] = [:]
    private var pushClient: PushClient?
    private var realtimeTask: Task<Void, Never>?
    private var pollingTask: Task<Void, Never>?
    private var trafficDeltaFlushTask: Task<Void, Never>?
    private var trafficHistoryTask: Task<Void, Never>?
    private var nativeUpdateTask: Task<Void, Never>?
    private var nativeUpdateInstallTask: Task<Void, Never>?
    private var ruleOrderSaveTask: Task<Void, Never>?
    private var realtimeSubscriptionTask: Task<Void, Never>?
    private var activityAppMetricsTask: Task<Void, Never>?
    private var promptedNativeUpdateVersions = Set<String>()
    private var pendingTrafficInserts: [TrafficRecordSummary] = []
    private var pendingTrafficUpdates: [TrafficRecordSummary] = []
    private var pendingMetricsUpdate: SystemOverview.Metrics?
    private var metricsPublishTask: Task<Void, Never>?
    private var lastMetricsPublishAt = Date.distantPast
    private var lastActivityAppMetricsRefreshAt = Date.distantPast
    private var lastRealtimeEventPublishAt = Date.distantPast
    private var lastRealtimeSubscription: PushSubscription?
    private var pendingRealtimeSubscription: PushSubscription?
    private var trafficServerTotal = 0
    private var trafficServerSequence: Int?
    private var trafficHasMore = false
    private var trafficOldestSequence: Int?
    private var pendingTrafficIds = Set<String>()
    private var interfaceActive = true

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
        NSWorkspace.shared.open(webUIURL())
    }

    func openGroupsWebUI() {
        NSWorkspace.shared.open(webUIURL(path: "groups"))
    }

    func webUIURL(path: String? = nil) -> URL {
        let root = adminURL.appendingPathComponent("_bifrost")
        guard let path, !path.isEmpty else {
            return root
        }
        return root.appendingPathComponent(path)
    }

    var canShowGroupManagement: Bool {
        syncStatus?.enabled == true
            && syncStatus?.hasSession == true
            && syncStatus?.authorized == true
    }

    var canShowRuleGroupSwitcher: Bool {
        canShowGroupManagement
    }

    var isGroupRulesMode: Bool {
        selectedRuleGroupID != nil
    }

    var ruleScopeTitle: String {
        if let selectedRuleGroupID {
            return activeRuleGroupName
                ?? ruleGroups.first { $0.id == selectedRuleGroupID }?.name
                ?? "Group Rules"
        }
        return "My Rules"
    }

    var sortedRuleGroups: [RuleGroup] {
        ruleGroups.sorted {
            ($0.permissionRank, $0.name.localizedLowercase) < ($1.permissionRank, $1.name.localizedLowercase)
        }
    }

    var canEditCurrentRuleScope: Bool {
        !isGroupRulesMode || activeRuleGroupWritable
    }

    var canCreateRuleInCurrentScope: Bool {
        canEditCurrentRuleScope
    }

    var canEditSelectedRuleContent: Bool {
        guard selectedRuleDetail != nil else {
            return false
        }
        return canEditCurrentRuleScope && selectedRuleDetail?.canEditContent != false
    }

    var canToggleSelectedRule: Bool {
        guard let detail = selectedRuleDetail else {
            return false
        }
        if isDefaultRule(detail.name) {
            return false
        }
        return canEditCurrentRuleScope && detail.canDisable != false
    }

    var canRenameSelectedRule: Bool {
        guard let detail = selectedRuleDetail else {
            return false
        }
        if isDefaultRule(detail.name) || isGroupRulesMode {
            return false
        }
        return canEditCurrentRuleScope && detail.canRename != false
    }

    var canDeleteSelectedRule: Bool {
        guard let detail = selectedRuleDetail else {
            return false
        }
        if isDefaultRule(detail.name) {
            return false
        }
        return canEditCurrentRuleScope && detail.canDelete != false
    }

    var visibleSidebarItems: [SidebarItem] {
        SidebarItem.visibleItems(canShowGroups: canShowGroupManagement)
    }

    func ensureSelectedSidebarItemVisible() {
        if !visibleSidebarItems.contains(selectedSidebarItem) {
            selectedSidebarItem = .overview
        }
    }

    func setInterfaceActive(_ active: Bool) {
        guard interfaceActive != active else {
            return
        }
        interfaceActive = active
        if !active {
            activityAppMetricsTask?.cancel()
            activityAppMetricsTask = nil
            metricsPublishTask?.cancel()
            metricsPublishTask = nil
            pendingMetricsUpdate = nil
        } else {
            flushPendingMetricsUpdate()
            scheduleActivityAppMetricsRefresh(force: true)
        }
    }

    func refreshSyncStatus() async {
        do {
            assignIfChanged(&syncStatus, try await BifrostClient(baseURL: adminURL).fetchSyncStatus())
            ensureSelectedSidebarItemVisible()
            assignIfChanged(&dataError, nil)
        } catch {
            assignIfChanged(&dataError, error.localizedDescription)
        }
    }

    func ensureService() async {
        if case .running(_, _) = sidecarState {
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
            startNativeAppUpdateChecks()
        } catch {
            didEnsureService = false
            sidecarState = .failed(error.localizedDescription)
        }
    }

    private func startNativeAppUpdateChecks() {
        if nativeUpdateTask != nil {
            return
        }
        if ProcessInfo.processInfo.environment["BIFROST_NATIVE_UPDATE_CHECK_DISABLED"] == "1" {
            return
        }
        nativeUpdateTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await self.checkNativeAppUpdate(forceRefresh: false)
                let seconds = Self.nativeUpdateIntervalSeconds()
                try? await Task.sleep(nanoseconds: UInt64(seconds) * 1_000_000_000)
            }
        }
    }

    private static func nativeUpdateIntervalSeconds() -> UInt64 {
        if let raw = ProcessInfo.processInfo.environment["BIFROST_NATIVE_UPDATE_INTERVAL_SECONDS"],
           let value = UInt64(raw),
           value >= 60 {
            return value
        }
        return 6 * 60 * 60
    }

    private func checkNativeAppUpdate(forceRefresh: Bool) async {
        do {
            let client = try BifrostClient(baseURL: adminURL)
            let version = try await client.fetchVersionCheck(forceRefresh: forceRefresh)
            guard version.hasUpdate, let latest = version.latestVersion else {
                if case .available = nativeAppUpdateState {
                    nativeAppUpdateState = .idle
                }
                return
            }
            guard !promptedNativeUpdateVersions.contains(latest) || forceRefresh else {
                return
            }
            promptedNativeUpdateVersions.insert(latest)
            nativeAppUpdateState = .available(latestVersion: latest)
        } catch {
            #if DEBUG
            print("Native app update check failed: \(error.localizedDescription)")
            #endif
        }
    }

    func installNativeAppUpdate() {
        guard nativeUpdateInstallTask == nil else {
            return
        }

        let targetVersion: String?
        switch nativeAppUpdateState {
        case .available(let latest), .failed(_, let latest?):
            targetVersion = latest
        default:
            targetVersion = nil
        }
        guard let targetVersion else {
            return
        }

        nativeUpdateInstallTask = Task { [weak self] in
            guard let self else { return }
            defer { self.nativeUpdateInstallTask = nil }
            await self.runNativeAppUpdateInstall(latestVersion: targetVersion)
        }
    }

    private func runNativeAppUpdateInstall(latestVersion: String) async {
        nativeAppUpdateState = .installing(latestVersion: latestVersion)

        do {
            let client = try BifrostClient(baseURL: adminURL)
            let install = try await client.installNativeApp()
            let installedPath = install.status.installPath
            if install.accepted {
                let status = try await waitForNativeAppInstall(
                    latestVersion: latestVersion,
                    client: client
                )
                restartNativeApp(latestVersion: latestVersion, installedAppPath: status.installPath)
            } else if install.status.installed && !install.status.needsInstall {
                restartNativeApp(latestVersion: latestVersion, installedAppPath: installedPath)
            } else {
                nativeAppUpdateState = .failed(
                    message: install.status.message,
                    latestVersion: latestVersion
                )
            }
        } catch {
            nativeAppUpdateState = .failed(
                message: error.localizedDescription,
                latestVersion: latestVersion
            )
        }
    }

    private func waitForNativeAppInstall(
        latestVersion: String,
        client: BifrostClient
    ) async throws -> NativeAppStatus {
        var lastStatus: NativeAppStatus?
        var lastError: Error?
        for _ in 0..<90 {
            try await Task.sleep(nanoseconds: 1_000_000_000)
            do {
                let status = try await client.fetchNativeAppStatus()
                lastStatus = status
                if status.installed,
                   !status.needsInstall,
                   status.installedVersion == nil || status.installedVersion == latestVersion
                {
                    return status
                }
            } catch {
                lastError = error
            }
        }

        if let lastStatus {
            throw NativeAppUpdateError.installTimedOut(lastStatus.message)
        }
        throw NativeAppUpdateError.installTimedOut(lastError?.localizedDescription ?? "Installation did not finish in time")
    }

    private func restartNativeApp(latestVersion: String, installedAppPath: String) {
        nativeAppUpdateState = .restarting(latestVersion: latestVersion)
        let appURL = URL(fileURLWithPath: installedAppPath)
        NSWorkspace.shared.open(appURL)
        NSApp.terminate(nil)
    }

    func refreshData(
        includeTraffic: Bool? = nil,
        includeOverview: Bool = true,
        includeRules: Bool = true,
        includeActiveRulesSummary: Bool = false,
        includeSystemControls: Bool = true,
        includeActivityAppMetrics: Bool? = nil
    ) async {
        isLoadingData = true
        defer { isLoadingData = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)

            var errors: [String] = []
            let shouldLoadTraffic = includeTraffic ?? selectedSidebarItem.needsTrafficRecords
            let shouldLoadActiveRulesSummary = includeActiveRulesSummary || selectedSidebarItem == .activity
            let shouldLoadActivityAppMetrics = includeActivityAppMetrics ?? (selectedSidebarItem == .activity)
            async let overviewResult: Result<SystemOverview, Error>? = includeOverview
                ? Self.captureResult { try await client.fetchSystemOverview() }
                : nil
            async let activityAppMetricsResult: Result<[AppMetrics], Error>? = shouldLoadActivityAppMetrics
                ? Self.captureResult { try await client.fetchAppMetrics() }
                : nil
            async let activeRulesSummaryResult: Result<ActiveRulesSummary, Error>? = shouldLoadActiveRulesSummary
                ? Self.captureResult { try await client.fetchActiveRulesSummary() }
                : nil
            async let systemProxyResult: Result<SystemProxyStatus, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchSystemProxy() }
                : nil
            async let systemProxyLaunchdResult: Result<SystemProxyLaunchdStatus, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchSystemProxyLaunchd() }
                : nil
            async let cliProxyResult: Result<CliProxyStatus, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchCliProxy() }
                : nil
            async let proxyAddressResult: Result<ProxyAddressInfo, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchProxyAddress() }
                : nil
            async let syncStatusResult: Result<SyncStatus, Error> = Self.captureResult { try await client.fetchSyncStatus() }
            async let performanceConfigResult: Result<PerformanceConfigResponse, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchPerformanceConfig() }
                : nil
            async let tlsConfigResult: Result<TlsConfig, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchTlsConfig() }
                : nil
            async let breakpointResult: Result<BreakpointSettings, Error>? = includeSystemControls
                ? Self.captureResult { try await client.fetchBreakpointSettings() }
                : nil

            if let result = await overviewResult {
                switch result {
                case .success(let overview):
                    assignIfChanged(&self.overview, overview)
                case .failure(let error):
                    errors.append("Overview: \(error.localizedDescription)")
                }
            }

            if shouldLoadTraffic {
                do {
                    try await reloadTrafficFromServer(client: client)
                } catch {
                    errors.append("Traffic: \(error.localizedDescription)")
                }
            }

            if let result = await activityAppMetricsResult {
                switch result {
                case .success(let metrics):
                    lastActivityAppMetricsRefreshAt = Date()
                    assignCountsIfChanged(&self.activityClientAppCounts, Self.appMetricsToCounts(metrics))
                case .failure(let error):
                    errors.append("App Metrics: \(error.localizedDescription)")
                }
            }

            if includeRules {
                do {
                    try await loadRulesForCurrentScope(client: client)
                } catch {
                    errors.append("Rules: \(error.localizedDescription)")
                }
            }

            if let result = await activeRulesSummaryResult {
                switch result {
                case .success(let summary):
                    assignIfChanged(&self.activeRulesSummary, summary)
                case .failure(let error):
                    errors.append("Active Rules: \(error.localizedDescription)")
                }
            }

            if let result = await systemProxyResult {
                switch result {
                case .success(let status):
                    assignIfChanged(&self.systemProxyStatus, status)
                case .failure(let error):
                    errors.append("System Proxy: \(error.localizedDescription)")
                }
            }

            if let result = await systemProxyLaunchdResult {
                switch result {
                case .success(let status):
                    assignIfChanged(&self.systemProxyLaunchdStatus, status)
                case .failure(let error):
                    errors.append("System Proxy Cleanup: \(error.localizedDescription)")
                }
            }

            if let result = await cliProxyResult {
                switch result {
                case .success(let status):
                    assignIfChanged(&self.cliProxyStatus, status)
                case .failure(let error):
                    errors.append("CLI Proxy: \(error.localizedDescription)")
                }
            }

            if let result = await proxyAddressResult {
                switch result {
                case .success(let info):
                    assignIfChanged(&self.proxyAddressInfo, info)
                case .failure(let error):
                    errors.append("Proxy Address: \(error.localizedDescription)")
                }
            }

            switch await syncStatusResult {
            case .success(let status):
                assignIfChanged(&self.syncStatus, status)
                ensureSelectedSidebarItemVisible()
                if canShowRuleGroupSwitcher {
                    do {
                        try await loadRuleGroups(client: client)
                    } catch {
                        errors.append("Groups: \(error.localizedDescription)")
                    }
                } else {
                    clearRuleGroupScope()
                }
            case .failure(let error):
                errors.append("Sync: \(error.localizedDescription)")
            }

            if let result = await performanceConfigResult {
                switch result {
                case .success(let config):
                    assignIfChanged(&self.performanceConfig, config)
                case .failure(let error):
                    errors.append("Performance: \(error.localizedDescription)")
                }
            }

            if let result = await tlsConfigResult {
                switch result {
                case .success(let config):
                    assignIfChanged(&self.tlsConfig, config)
                case .failure(let error):
                    errors.append("TLS: \(error.localizedDescription)")
                }
            }

            if let result = await breakpointResult {
                switch result {
                case .success(let settings):
                    assignIfChanged(&self.breakpointSettings, settings)
                case .failure(let error):
                    errors.append("Breakpoint: \(error.localizedDescription)")
                }
            }

            if includeRules && selectedRuleName == nil {
                selectedRuleName = self.rules.first?.name
            }
            assignIfChanged(&self.dataError, errors.isEmpty ? nil : errors.joined(separator: " · "))
            updateRealtimeSubscription()
        } catch {
            assignIfChanged(&self.dataError, error.localizedDescription)
        }
    }

    private nonisolated static func captureResult<T: Sendable>(
        _ operation: @Sendable () async throws -> T
    ) async -> Result<T, Error> {
        do {
            return .success(try await operation())
        } catch {
            return .failure(error)
        }
    }

    private func assignIfChanged<T: Equatable>(_ storage: inout T, _ value: T) {
        if storage != value {
            storage = value
        }
    }

    private func assignCountsIfChanged(_ storage: inout [(name: String, count: Int)], _ value: [(name: String, count: Int)]) {
        guard storage.count == value.count else {
            storage = value
            return
        }
        for index in storage.indices where storage[index].name != value[index].name || storage[index].count != value[index].count {
            storage = value
            return
        }
    }

    func refreshRuleGroups() async {
        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await loadRuleGroups(client: client)
        } catch {
            dataError = error.localizedDescription
        }
    }

    func searchRuleGroups(keyword: String) async -> [RuleGroup] {
        guard canShowRuleGroupSwitcher else {
            return []
        }
        do {
            let client = try BifrostClient(baseURL: adminURL)
            let response = try await client.fetchRuleGroups(keyword: keyword, limit: 80)
            return response.list.sorted {
                ($0.permissionRank, $0.name.localizedLowercase) < ($1.permissionRank, $1.name.localizedLowercase)
            }
        } catch {
            dataError = error.localizedDescription
            return []
        }
    }

    private func loadRuleGroups(client: BifrostClient) async throws {
        guard canShowRuleGroupSwitcher else {
            clearRuleGroupScope()
            return
        }
        isLoadingRuleGroups = true
        defer { isLoadingRuleGroups = false }
        let response = try await client.fetchRuleGroups(limit: 80)
        ruleGroups = response.list
        if let selectedRuleGroupID,
           !ruleGroups.contains(where: { $0.id == selectedRuleGroupID }) {
            clearRuleGroupScope()
            try await loadRulesForCurrentScope(client: client)
        }
        dataError = nil
    }

    private func clearRuleGroupScope() {
        ruleGroups = []
        selectedRuleGroupID = nil
        activeRuleGroupName = nil
        activeRuleGroupWritable = false
    }

    private func loadRulesForCurrentScope(client: BifrostClient) async throws {
        if let groupID = selectedRuleGroupID {
            let response = try await client.fetchGroupRules(groupID: groupID)
            activeRuleGroupName = response.groupName
            activeRuleGroupWritable = response.writable
            rules = response.rules.map(\.ruleSummary)
        } else {
            activeRuleGroupName = nil
            activeRuleGroupWritable = false
            rules = try await client.fetchRules()
        }
    }

    func selectRuleScope(groupID: String?) async {
        guard selectedRuleGroupID != groupID else {
            return
        }
        selectedRuleGroupID = groupID
        activeRuleGroupName = groupID.flatMap { id in ruleGroups.first { $0.id == id }?.name }
        activeRuleGroupWritable = false
        selectedRuleName = nil
        selectedRuleDetail = nil
        ruleDraftContent = ""

        do {
            let client = try BifrostClient(baseURL: adminURL)
            try await loadRulesForCurrentScope(client: client)
            if let firstName = sortedRules.first?.name {
                await selectRule(firstName)
            }
            dataError = nil
        } catch {
            rules = []
            dataError = error.localizedDescription
        }
    }

    func handleSidebarSelectionChanged() async {
        clearPendingTrafficDelta()
        if !selectedSidebarItem.needsTrafficRecords {
            trafficHistoryTask?.cancel()
            trafficHistoryTask = nil
        }
        updateRealtimeSubscription()
        await refreshSelectedSidebarData()
    }

    private func refreshSelectedSidebarData() async {
        switch selectedSidebarItem {
        case .activity:
            await refreshData(includeTraffic: true, includeRules: false, includeActiveRulesSummary: true, includeSystemControls: false)
        case .overview:
            await refreshData(includeTraffic: false, includeRules: false, includeSystemControls: true)
        case .rules:
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
            await refreshRuleEditorDynamicData()
            if let name = selectedRuleName,
               selectedRuleDetail?.name != name {
                await selectRule(name)
            }
        case .network:
            await refreshData(includeTraffic: true, includeRules: false, includeSystemControls: false)
        case .groups:
            await refreshSyncStatus()
        }
    }

    var ruleEditorContext: BifrostRuleEditorContext {
        let languageService = BifrostRuleLanguageService()
        return BifrostRuleEditorContext(
            currentRuleName: selectedRuleName,
            currentGroupName: nil,
            ruleNames: rules.map(\.name),
            values: values.map(\.name),
            requestScripts: scriptsByType[.request, default: []].map(\.name),
            responseScripts: scriptsByType[.response, default: []].map(\.name),
            parserScripts: scriptsByType[.parser, default: []].map(\.name),
            localVariables: languageService.localVariables(in: ruleDraftContent)
        )
    }

    func refreshRuleEditorDynamicData() async {
        do {
            let client = try BifrostClient(baseURL: adminURL)
            async let valuesResult = client.fetchValues()
            async let scriptsResult = client.fetchScripts()
            values = try await valuesResult.values
            let scripts = try await scriptsResult
            scriptsByType = Dictionary(uniqueKeysWithValues: ScriptType.allCases.map { type in
                (type, scripts.scripts(for: type))
            })
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func navigateFromRuleEditor(_ target: BifrostNavigationTarget) {
        switch target {
        case .editorLine:
            break
        case .rule(_, let name):
            selectedSidebarItem = .rules
            Task { await selectRule(name) }
        case .value, .script:
            openWebUI()
        }
    }

    func isDefaultRule(_ name: String?) -> Bool {
        name == "Default"
    }

    func isRuleProtected(_ rule: RuleSummary) -> Bool {
        isDefaultRule(rule.name) || !canEditCurrentRuleScope
    }

    func canReorderRule(_ rule: RuleSummary) -> Bool {
        !isGroupRulesMode && !isDefaultRule(rule.name) && rule.canReorder != false
    }

    var sortedRules: [RuleSummary] {
        rules.sorted {
            ($0.sortOrder ?? Int.max, $0.name) < ($1.sortOrder ?? Int.max, $1.name)
        }
    }

    func selectRule(_ name: String) async {
        selectedRuleName = name
        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                selectedRuleDetail = try await client.fetchGroupRule(groupID: groupID, name: name).ruleDetail
            } else {
                selectedRuleDetail = try await client.fetchRule(name: name)
            }
            ruleDraftContent = selectedRuleDetail?.content ?? ""
            dataError = nil
        } catch {
            selectedRuleDetail = nil
            ruleDraftContent = ""
            dataError = error.localizedDescription
        }
    }

    func autosaveSelectedRule(name: String, content: String) async {
        guard selectedRuleName == name else {
            return
        }
        guard canEditSelectedRuleContent else {
            dataError = "Current rule list is read-only."
            return
        }
        guard selectedRuleDetail?.content != content else {
            return
        }
        isAutoSavingRule = true
        defer { isAutoSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                _ = try await client.updateGroupRule(groupID: groupID, name: name, content: content)
            } else {
                try await client.updateRule(name: name, content: content)
            }
            if selectedRuleName == name {
                selectedRuleDetail?.content = content
            }
            dataError = nil
        } catch {
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
        guard canCreateRuleInCurrentScope else {
            dataError = "Current rule list is read-only."
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                _ = try await client.createGroupRule(groupID: groupID, name: trimmed, content: "# New rule\n")
            } else {
                try await client.createRule(name: trimmed, content: "# New rule\n")
            }
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
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
        guard canEditSelectedRuleContent else {
            dataError = "Current rule list is read-only."
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                _ = try await client.updateGroupRule(groupID: groupID, name: name, content: content)
            } else {
                try await client.updateRule(name: name, content: content)
            }
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
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
        guard !isDefaultRule(name) || enabled else {
            dataError = "Default rule must stay enabled."
            return
        }
        guard canToggleSelectedRule else {
            dataError = "Current rule list is read-only."
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                try await client.setGroupRuleEnabled(groupID: groupID, name: name, enabled: enabled)
            } else {
                try await client.setRuleEnabled(name: name, enabled: enabled)
            }
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
            await selectRule(name)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
        }
    }

    func renameSelectedRule(to newName: String) async {
        guard let oldName = selectedRuleName else {
            return
        }
        guard !isDefaultRule(oldName) else {
            dataError = "Default rule cannot be renamed."
            return
        }
        guard canRenameSelectedRule else {
            dataError = "Current rule cannot be renamed."
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
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
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
        guard !isDefaultRule(name) else {
            dataError = "Default rule cannot be deleted."
            return
        }
        guard canDeleteSelectedRule else {
            dataError = "Current rule cannot be deleted."
            return
        }
        isSavingRule = true
        defer { isSavingRule = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            if let groupID = selectedRuleGroupID {
                try await client.deleteGroupRule(groupID: groupID, name: name)
            } else {
                try await client.deleteRule(name: name)
            }
            selectedRuleName = nil
            selectedRuleDetail = nil
            ruleDraftContent = ""
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
            if let nextName = sortedRules.first?.name {
                await selectRule(nextName)
            }
            dataError = nil
        } catch {
            dataError = error.localizedDescription
        }
    }

    func moveRules(from source: IndexSet, to destination: Int) {
        var ordered = sortedRules
        guard source.allSatisfy({ index in
            ordered.indices.contains(index) && canReorderRule(ordered[index])
        }) else {
            return
        }
        ordered.move(fromOffsets: source, toOffset: destination)
        if let defaultIndex = ordered.firstIndex(where: { isDefaultRule($0.name) }), defaultIndex != 0 {
            let defaultRule = ordered.remove(at: defaultIndex)
            ordered.insert(defaultRule, at: 0)
        }
        rules = ordered.enumerated().map { offset, rule in
            var updated = rule
            updated.sortOrder = offset
            return updated
        }
        scheduleRuleOrderSave(order: ordered.map(\.name))
    }

    func moveRule(named name: String, relativeTo targetName: String, placement: RuleMovePlacement) {
        var ordered = sortedRules
        guard let sourceIndex = ordered.firstIndex(where: { $0.name == name }),
              let targetIndex = ordered.firstIndex(where: { $0.name == targetName }),
              sourceIndex != targetIndex,
              canReorderRule(ordered[sourceIndex]),
              canReorderRule(ordered[targetIndex]) else {
            return
        }

        let originalOrder = ordered.map(\.name)
        let movedRule = ordered.remove(at: sourceIndex)
        var insertionIndex = targetIndex + (placement == .after ? 1 : 0)
        if sourceIndex < insertionIndex {
            insertionIndex -= 1
        }
        insertionIndex = min(max(insertionIndex, 1), ordered.count)
        ordered.insert(movedRule, at: insertionIndex)

        guard ordered.map(\.name) != originalOrder else {
            return
        }
        applyRuleOrder(ordered)
    }

    private func applyRuleOrder(_ ordered: [RuleSummary]) {
        rules = ordered.enumerated().map { offset, rule in
            var updated = rule
            updated.sortOrder = offset
            return updated
        }
        scheduleRuleOrderSave(order: ordered.map(\.name))
    }

    private func scheduleRuleOrderSave(order: [String]) {
        ruleOrderSaveTask?.cancel()
        ruleOrderSaveTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 350_000_000)
            guard !Task.isCancelled else {
                return
            }
            await self?.saveRuleOrder(order)
        }
    }

    private func saveRuleOrder(_ order: [String]) async {
        guard !isGroupRulesMode else {
            return
        }
        do {
            try await BifrostClient(baseURL: adminURL).reorderRules(order)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false)
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

    func setSystemProxyLaunchdEnabled(_ enabled: Bool) async {
        guard !isTogglingSystemProxyLaunchd else {
            return
        }
        isTogglingSystemProxyLaunchd = true
        defer { isTogglingSystemProxyLaunchd = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            systemProxyLaunchdStatus = try await client.setSystemProxyLaunchd(enabled: enabled)
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func setInjectBifrostBadgeEnabled(_ enabled: Bool) async {
        guard !isTogglingInjectBifrostBadge else {
            return
        }
        isTogglingInjectBifrostBadge = true
        defer { isTogglingInjectBifrostBadge = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            performanceConfig = try await client.updatePerformanceConfig(
                UpdatePerformanceConfigRequest(injectBifrostBadge: enabled)
            )
            dataError = nil
        } catch {
            dataError = error.localizedDescription
            await refreshData()
        }
    }

    func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
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

    func updateTlsConfig(_ newConfig: TlsConfig) async {
        guard !isTogglingTls else {
            return
        }
        isTogglingTls = true
        defer { isTogglingTls = false }

        do {
            let client = try BifrostClient(baseURL: adminURL)
            tlsConfig = try await client.updateTlsConfig(newConfig)
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
            activityClientAppCounts = []
            refreshActivityTrafficSummaries()
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
        if case .running(let port, _) = sidecarState {
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
        trimNativeTrafficRecordsIfNeeded()
        trafficServerTotal = response.serverTotal
        trafficServerSequence = response.serverSequence
        trafficHasMore = response.hasMore
        refreshTrafficBoundaryState()
        refreshActivityTrafficSummaries()
        rebuildPendingTrafficIds()
        updateRealtimeSubscription()

        // The native shell shows lightweight activity/device summaries only.
        // Full Network history remains in the Web UI, so avoid expensive backfill here.
    }

    private func startTrafficHistoryBackfill() {
        trafficHistoryTask?.cancel()
        trafficHistoryTask = Task { [weak self] in
            await self?.backfillTrafficHistory()
        }
    }

    private func backfillTrafficHistory() async {
        while !Task.isCancelled {
            guard selectedSidebarItem.needsTrafficRecords,
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
        realtimeSubscriptionTask?.cancel()
        metricsPublishTask?.cancel()
        activityAppMetricsTask?.cancel()
        trafficDeltaFlushTask = nil
        trafficHistoryTask = nil
        realtimeSubscriptionTask = nil
        metricsPublishTask = nil
        activityAppMetricsTask = nil
        pendingRealtimeSubscription = nil
        lastRealtimeSubscription = nil
        pendingMetricsUpdate = nil
        lastMetricsPublishAt = .distantPast
        lastActivityAppMetricsRefreshAt = .distantPast
        lastRealtimeEventPublishAt = .distantPast
        pendingTrafficInserts.removeAll(keepingCapacity: true)
        pendingTrafficUpdates.removeAll(keepingCapacity: true)
        setRealtimeState(.connecting)

        pollingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: TrafficSyncPolicy.fallbackPollingIntervalNanoseconds)
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
        guard case .running(_, _) = sidecarState else {
            return
        }
        guard realtimeState != .connected else {
            assignIfChanged(&realtimeFallbackActive, false)
            return
        }
        assignIfChanged(&realtimeFallbackActive, true)
        await refreshSelectedSidebarDataFromFallback()
    }

    private func refreshSelectedSidebarDataFromFallback() async {
        switch selectedSidebarItem {
        case .activity:
            let shouldRefreshAppMetrics = Date().timeIntervalSince(lastActivityAppMetricsRefreshAt)
                >= TrafficSyncPolicy.fallbackActivityAppMetricsRefreshInterval
            await refreshData(
                includeTraffic: true,
                includeRules: false,
                includeActiveRulesSummary: true,
                includeSystemControls: false,
                includeActivityAppMetrics: shouldRefreshAppMetrics
            )
        case .overview:
            await refreshData(includeTraffic: false, includeRules: false, includeSystemControls: true, includeActivityAppMetrics: false)
        case .rules:
            await refreshData(includeTraffic: false, includeRules: true, includeSystemControls: false, includeActivityAppMetrics: false)
            await refreshRuleEditorDynamicData()
            if let name = selectedRuleName,
               selectedRuleDetail?.name != name {
                await selectRule(name)
            }
        case .network:
            await refreshData(includeTraffic: true, includeRules: false, includeSystemControls: false, includeActivityAppMetrics: false)
        case .groups:
            await refreshSyncStatus()
        }
    }

    private func setPushClient(_ client: PushClient) {
        pushClient = client
    }

    private func setRealtimeState(_ state: RealtimeConnectionState) {
        assignIfChanged(&realtimeState, state)
        assignIfChanged(&realtimeFallbackActive, state != .connected)
    }

    private func setRealtimeError(_ message: String) {
        assignIfChanged(&realtimeState, .failed(message))
        assignIfChanged(&realtimeFallbackActive, true)
    }

    private func handlePushMessage(_ message: PushMessage) async {
        noteRealtimeEvent()
        switch message {
        case .connected(let clientId):
            assignIfChanged(&realtimeClientId, clientId)
            setRealtimeState(.connected)
            updateRealtimeSubscription(force: true)
            scheduleActivityAppMetricsRefresh()
        case .trafficDelta(let data):
            guard selectedSidebarItem.needsTrafficRecords else {
                return
            }
            enqueueTrafficDelta(data)
        case .trafficDeleted(let data):
            guard selectedSidebarItem.needsTrafficRecords else {
                return
            }
            flushPendingTrafficDelta()
            removeTraffic(ids: data.ids)
            updateRealtimeSubscription()
        case .overviewUpdate(let data):
            assignIfChanged(&overview, data)
        case .metricsUpdate(let data):
            applyMetricsUpdate(data.metrics)
        case .valuesUpdate:
            break
        case .settingsUpdate(let data):
            applySettingsUpdate(data)
        case .breakpointSettingsUpdated(let data):
            assignIfChanged(&breakpointSettings, BreakpointSettings(enabled: data.enabled, maxBodyBytes: data.maxBodyBytes))
        case .disconnect(let reason):
            setRealtimeError(reason ?? "Server requested refresh")
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
            try? await Task.sleep(nanoseconds: TrafficSyncPolicy.trafficDeltaFlushDelayNanoseconds)
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
            assignIfChanged(&trafficRecords, deduplicatedTrafficRecords(trafficRecords))
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
        trimNativeTrafficRecordsIfNeeded()
        refreshTrafficBoundaryState()
        refreshActivityTrafficSummaries()
        rebuildPendingTrafficIds()
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
        refreshActivityTrafficSummaries()
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
            if let value = try? decoder.decode(SystemProxyStatus.self, from: update.data) {
                assignIfChanged(&systemProxyStatus, value)
            }
        case "cli_proxy":
            if let value = try? decoder.decode(CliProxyStatus.self, from: update.data) {
                assignIfChanged(&cliProxyStatus, value)
            }
        case "proxy_address":
            if let value = try? decoder.decode(ProxyAddressInfo.self, from: update.data) {
                assignIfChanged(&proxyAddressInfo, value)
            }
        case "tls_config":
            if let value = try? decoder.decode(TlsConfig.self, from: update.data) {
                assignIfChanged(&tlsConfig, value)
            }
        default:
            break
        }
    }

    private func applyMetricsUpdate(_ metrics: SystemOverview.Metrics) {
        guard overview?.metrics != metrics else {
            return
        }
        scheduleActivityAppMetricsRefresh()
        let now = Date()
        guard now.timeIntervalSince(lastMetricsPublishAt) >= TrafficSyncPolicy.metricsPublishInterval else {
            pendingMetricsUpdate = metrics
            schedulePendingMetricsPublish()
            return
        }
        publishMetrics(metrics, at: now)
    }

    private func schedulePendingMetricsPublish() {
        guard metricsPublishTask == nil else {
            return
        }
        let elapsed = Date().timeIntervalSince(lastMetricsPublishAt)
        let delay = max(TrafficSyncPolicy.metricsPublishInterval - elapsed, 0)
        metricsPublishTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            if Task.isCancelled {
                return
            }
            self?.flushPendingMetricsUpdate()
        }
    }

    private func flushPendingMetricsUpdate() {
        metricsPublishTask = nil
        guard let metrics = pendingMetricsUpdate else {
            return
        }
        pendingMetricsUpdate = nil
        guard overview?.metrics != metrics else {
            return
        }
        publishMetrics(metrics, at: Date())
    }

    private func publishMetrics(_ metrics: SystemOverview.Metrics, at date: Date) {
        var nextOverview = overview ?? SystemOverview()
        nextOverview.metrics = metrics
        assignIfChanged(&overview, nextOverview)
        lastMetricsPublishAt = date
    }

    private func scheduleActivityAppMetricsRefresh(force: Bool = false) {
        guard interfaceActive else {
            return
        }
        guard selectedSidebarItem == .activity else {
            return
        }
        guard activityAppMetricsTask == nil else {
            return
        }
        let now = Date()
        guard force || now.timeIntervalSince(lastActivityAppMetricsRefreshAt) >= TrafficSyncPolicy.activityAppMetricsRefreshInterval else {
            return
        }
        lastActivityAppMetricsRefreshAt = now
        activityAppMetricsTask = Task { [weak self] in
            guard let self else {
                return
            }
            defer {
                self.activityAppMetricsTask = nil
            }
            do {
                let client = try BifrostClient(baseURL: self.adminURL)
                let metrics = try await client.fetchAppMetrics()
                if Task.isCancelled {
                    return
                }
                self.assignCountsIfChanged(&self.activityClientAppCounts, Self.appMetricsToCounts(metrics))
            } catch {
                // The overview metrics stream remains authoritative; app distribution refresh is best effort.
            }
        }
    }

    private func makePushSubscription() -> PushSubscription {
        let lastRecord = trafficRecords.last
        let needsTraffic = selectedSidebarItem.needsTrafficRecords
        return PushSubscription(
            lastTrafficId: needsTraffic ? lastRecord?.id : nil,
            lastSequence: needsTraffic ? lastRecord?.seq : nil,
            pendingIds: needsTraffic ? Array(pendingTrafficIds) : [],
            needTraffic: needsTraffic,
            needOverview: true,
            needMetrics: true,
            needValues: false,
            needScripts: false,
            settingsScopes: [
                "system_proxy",
                "cli_proxy",
                "proxy_address",
                "tls_config",
            ],
            metricsIntervalMs: TrafficSyncPolicy.realtimeMetricsIntervalMs
        )
    }

    private func updateRealtimeSubscription(force: Bool = false) {
        guard realtimeState == .connected,
              let pushClient else {
            return
        }
        let subscription = makePushSubscription()
        guard force || subscription != lastRealtimeSubscription else {
            return
        }
        pendingRealtimeSubscription = subscription
        realtimeSubscriptionTask?.cancel()
        if force {
            flushPendingRealtimeSubscription(pushClient: pushClient)
            return
        }
        realtimeSubscriptionTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: TrafficSyncPolicy.subscriptionDebounceNanoseconds)
            if Task.isCancelled {
                return
            }
            self?.flushPendingRealtimeSubscription()
        }
    }

    private func flushPendingRealtimeSubscription() {
        guard let pushClient else {
            return
        }
        flushPendingRealtimeSubscription(pushClient: pushClient)
    }

    private func flushPendingRealtimeSubscription(pushClient: PushClient) {
        realtimeSubscriptionTask = nil
        guard let subscription = pendingRealtimeSubscription else {
            return
        }
        pendingRealtimeSubscription = nil
        guard subscription != lastRealtimeSubscription else {
            return
        }
        lastRealtimeSubscription = subscription
        Task {
            try? await pushClient.send(subscription: subscription)
        }
    }

    private func noteRealtimeEvent() {
        let now = Date()
        guard now.timeIntervalSince(lastRealtimeEventPublishAt) >= TrafficSyncPolicy.realtimeEventPublishInterval else {
            return
        }
        lastRealtimeEventPublishAt = now
        assignIfChanged(&lastRealtimeEventAt, now)
    }

    private func rebuildTrafficRecordIndex() {
        var indexById: [String: Int] = [:]
        indexById.reserveCapacity(trafficRecords.count)
        for (offset, record) in trafficRecords.enumerated() {
            indexById[record.id] = offset
        }
        trafficRecordIndexById = indexById
    }

    private func trimNativeTrafficRecordsIfNeeded() {
        let overflow = trafficRecords.count - TrafficSyncPolicy.maxNativeRecords
        guard overflow > 0 else {
            return
        }
        let removedIds = Set(trafficRecords.prefix(overflow).map(\.id))
        trafficRecords.removeFirst(overflow)
        rebuildTrafficRecordIndex()
        if let selectedTrafficId, removedIds.contains(selectedTrafficId) {
            self.selectedTrafficId = nil
            selectedTrafficDetailText = ""
            selectedTrafficRequestBodyText = ""
            selectedTrafficResponseBodyText = ""
        }
    }

    private func refreshActivityTrafficSummaries() {
        var ips: [String: Int] = [:]
        var ruleHits = 0
        for record in trafficRecords {
            if let clientIp = record.clientIp, !clientIp.isEmpty {
                ips[clientIp, default: 0] += 1
            }
            if record.hasRuleHit {
                ruleHits += 1
            }
        }
        assignCountsIfChanged(&activityClientIpCounts, sortedCounts(ips))
        assignIfChanged(&activityRuleHitCount, ruleHits)
    }

    private static func appMetricsToCounts(_ metrics: [AppMetrics]) -> [(name: String, count: Int)] {
        metrics
            .filter { !$0.appName.isEmpty && $0.requests > 0 }
            .map { metric in
                (
                    name: metric.appName,
                    count: metric.requests > UInt64(Int.max) ? Int.max : Int(metric.requests)
                )
            }
            .sorted { lhs, rhs in
                if lhs.count != rhs.count {
                    return lhs.count > rhs.count
                }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
    }

    private func sortedCounts(_ values: [String: Int]) -> [(name: String, count: Int)] {
        values
            .map { (name: $0.key, count: $0.value) }
            .sorted { lhs, rhs in
                if lhs.count != rhs.count {
                    return lhs.count > rhs.count
                }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
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
    case system = "System"
    case light = "Light"
    case dark = "Dark"

    var id: String { rawValue }

    var colorScheme: ColorScheme? {
        switch self {
        case .system:
            return nil
        case .light:
            return .light
        case .dark:
            return .dark
        }
    }

    var next: ColorSchemeMode {
        switch self {
        case .system:
            return .light
        case .light:
            return .dark
        case .dark:
            return .system
        }
    }

    var systemImage: String {
        switch self {
        case .system:
            return "circle.lefthalf.filled"
        case .light:
            return "sun.max"
        case .dark:
            return "moon"
        }
    }
}
