import AppKit
import BifrostNativeCore
import SwiftUI
import WebKit

struct ActivityView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var contentWidth: CGFloat = 0

    private var metrics: SystemOverview.Metrics? {
        appModel.overview?.metrics
    }

    var body: some View {
        NativePageScaffold(title: "活动") {
            activityMetricGrid

            ActiveRulesSummaryCard(summary: appModel.activeRulesSummary)
                .equatable()

            NativeCard {
                VStack(alignment: .leading, spacing: 14) {
                    HStack {
                        Text("流量分布")
                            .font(.system(size: 15, weight: .semibold))
                        Spacer()
                        Text("按应用统计")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.secondary)
                    }
                    ActivityBars(rows: appModel.activityClientAppCounts.map { ($0.name, $0.count) })
                        .equatable()
                }
            }
        }
        .background(ActivityWidthReader(width: $contentWidth))
    }

    private var activityMetricGrid: some View {
        activityMetricGrid(columnCount: activityMetricColumnCount)
    }

    private var activityMetricColumnCount: Int {
        guard contentWidth > 0 else {
            return 6
        }
        let minimumCardWidth: CGFloat = 150
        let spacing: CGFloat = 18
        let fit = Int((contentWidth + spacing) / (minimumCardWidth + spacing))
        return min(6, max(1, fit))
    }

    private func activityMetricGrid(columnCount: Int) -> some View {
        LazyVGrid(
            columns: Array(
                repeating: GridItem(.flexible(minimum: 150), spacing: 18, alignment: .topLeading),
                count: columnCount
            ),
            alignment: .leading,
            spacing: 18
        ) {
            activityMetricCards
        }
    }

    @ViewBuilder
    private var activityMetricCards: some View {
        NativeMetricCard(
            title: "活动连接",
            value: "\(metrics?.activeConnections ?? 0)",
            caption: "\(appModel.activityClientAppCounts.count) 个应用",
            tint: .orange
        )
        NativeMetricCard(
            title: "上传",
            value: formatRate(metrics?.bytesSentRate),
            caption: formatBytes(metrics?.bytesSent),
            tint: .indigo
        )
        NativeMetricCard(
            title: "下载",
            value: formatRate(metrics?.bytesReceivedRate),
            caption: formatBytes(metrics?.bytesReceived),
            tint: .cyan
        )
        NativeMetricCard(
            title: "请求",
            value: "\(metrics?.totalRequests ?? 0)",
            caption: qpsText,
            tint: .green
        )
        NativeMetricCard(
            title: "规则",
            value: rulesSummary,
            caption: "当前规则集",
            tint: .purple
        )
        NativeMetricCard(
            title: "服务",
            value: sidecarStatusText,
            caption: appModel.adminHostPortLabel,
            tint: .blue
        )
    }

    private var qpsText: String {
        guard let qps = metrics?.qps else {
            return "实时速率"
        }
        return String(format: "%.2f QPS", qps)
    }

    private var rulesSummary: String {
        let enabled = appModel.overview?.rules?.enabled ?? appModel.rules.filter(\.enabled).count
        let total = appModel.overview?.rules?.total ?? appModel.rules.count
        return "\(enabled)/\(total)"
    }

    private var sidecarStatusText: String {
        switch appModel.sidecarState {
        case .running(_, let origin):
            switch origin {
            case .existingDefaultDataDirectory:
                return "运行中 · CLI"
            case .launchedBundledSidecar:
                return "运行中 · App"
            }
        case .starting:
            return "启动中"
        case .recovering:
            return "恢复中"
        case .failed:
            return "异常"
        case .stopped:
            return "未启动"
        }
    }
}

private struct ActivityWidthReader: View {
    @Binding var width: CGFloat

    var body: some View {
        GeometryReader { proxy in
            Color.clear
                .preference(key: ActivityWidthPreferenceKey.self, value: proxy.size.width)
        }
        .onPreferenceChange(ActivityWidthPreferenceKey.self) { nextWidth in
            guard abs(width - nextWidth) > 1 else {
                return
            }
            width = nextWidth
        }
    }
}

private struct ActivityWidthPreferenceKey: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private struct ActiveRulesSummaryCard: View, Equatable {
    let summary: ActiveRulesSummary?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("生效规则解析")
                            .font(.system(size: 15, weight: .semibold))
                        Text("当前代理端口正在使用的规则集合")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusPill(title: summaryStatusText, color: summaryStatusColor)
                }

                if let summary {
                    if summary.rules.isEmpty && summary.variableConflicts.isEmpty {
                        EmptyNativeState(title: "暂无生效规则")
                            .frame(maxWidth: .infinity, alignment: .center)
                            .padding(.vertical, 18)
                    } else {
                        VStack(alignment: .leading, spacing: 14) {
                            if !summary.variableConflicts.isEmpty {
                                variableConflicts(summary.variableConflicts)
                            }
                            activeRules(summary.rules)
                            if !summary.mergedContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                mergedRules(summary.mergedContent)
                            }
                        }
                    }
                } else {
                    Text("正在读取生效规则解析信息...")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 12)
                }
            }
        }
    }

    private var summaryStatusText: String {
        guard let summary else {
            return "读取中"
        }
        return "\(summary.total) active"
    }

    private var summaryStatusColor: Color {
        guard let summary else {
            return .secondary
        }
        return summary.total > 0 ? .green : .secondary
    }

    private func activeRules(_ rules: [ActiveRuleItem]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Active Rules")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)

            if localRules(in: rules).isEmpty && groupedRules(in: rules).isEmpty {
                Text("No active rules resolved")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    if !localRules(in: rules).isEmpty {
                        RuleTokenFlow {
                            ForEach(localRules(in: rules)) { rule in
                                ActiveRuleToken(rule: rule)
                            }
                        }
                    }

                    ForEach(groupedRules(in: rules), id: \.id) { group in
                        VStack(alignment: .leading, spacing: 7) {
                            HStack(spacing: 5) {
                                Image(systemName: "person.2")
                                    .font(.system(size: 11, weight: .semibold))
                                Text(group.name)
                                    .font(.system(size: 11, weight: .semibold))
                                    .lineLimit(1)
                            }
                            .foregroundStyle(.secondary)

                            RuleTokenFlow {
                                ForEach(group.rules) { rule in
                                    ActiveRuleToken(rule: rule)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func variableConflicts(_ conflicts: [ActiveRuleVariableConflict]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(Color.orange)
                Text("Variable Conflicts")
                    .font(.system(size: 12, weight: .semibold))
            }

            ForEach(conflicts) { conflict in
                VStack(alignment: .leading, spacing: 5) {
                    Text("{\(conflict.variableName)}")
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    ForEach(conflict.definitions) { definition in
                        Text("\(definition.ruleName): \(definition.valuePreview)")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
            }
        }
        .padding(12)
        .background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .stroke(Color.orange.opacity(0.20))
        )
    }

    private func mergedRules(_ content: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Merged Rules")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .textCase(.uppercase)
                Spacer()
                Text("\(content.split(whereSeparator: \.isNewline).count) lines")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                Text(content.trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.primary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
    }

    private func localRules(in rules: [ActiveRuleItem]) -> [ActiveRuleItem] {
        rules.filter { $0.groupID == nil }
    }

    private func groupedRules(in rules: [ActiveRuleItem]) -> [ActiveRuleGroup] {
        let grouped = Dictionary(grouping: rules.filter { $0.groupID != nil }) { rule in
            rule.groupID ?? ""
        }
        return grouped
            .map { groupID, rules in
                ActiveRuleGroup(
                    id: groupID,
                    name: rules.first?.groupName ?? groupID,
                    rules: rules.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
                )
            }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    private struct ActiveRuleGroup {
        let id: String
        let name: String
        let rules: [ActiveRuleItem]
    }
}

private struct RuleTokenFlow<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        LazyVGrid(
            columns: [
                GridItem(.adaptive(minimum: 150, maximum: 260), spacing: 8, alignment: .topLeading)
            ],
            alignment: .leading,
            spacing: 8
        ) {
            content
        }
    }
}

private struct ActiveRuleToken: View {
    let rule: ActiveRuleItem

    var body: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(rule.groupID == nil ? Color.blue : Color.purple)
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(rule.name)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text("\(rule.ruleCount) entries")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 4)
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 10)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}

struct DashboardView: View {
    @EnvironmentObject private var appModel: AppModel
    @StateObject private var model = OverviewControlModel()

    var body: some View {
        NativePageScaffold(title: "概览") {
            overviewControlGrid

            RemoteInvokeCard(model: model)

            MobileConnectionCheckCard(model: model)
        }
        .task(id: appModel.adminURL) {
            await model.configure(baseURL: appModel.adminURL)
        }
    }

    private var overviewControlGrid: some View {
        ViewThatFits(in: .horizontal) {
            overviewControlGrid(columnCount: 4)
            overviewControlGrid(columnCount: 3)
            overviewControlGrid(columnCount: 2)
            overviewControlGrid(columnCount: 1)
        }
    }

    private func overviewControlGrid(columnCount: Int) -> some View {
        LazyVGrid(
            columns: Array(
                repeating: GridItem(.flexible(minimum: 220), spacing: 18, alignment: .topLeading),
                count: columnCount
            ),
            alignment: .leading,
            spacing: 18
        ) {
            overviewControlCards
        }
    }

    @ViewBuilder
    private var overviewControlCards: some View {
        SystemProxyCard()
        TlsInterceptionCard()
        SyncControlCard(model: model)
        CertificateManagementCard(model: model)
    }
}

struct NetworkWebView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        NativePageScaffold(title: "抓包") {
            NativeCard {
                VStack(alignment: .leading, spacing: 18) {
                    ViewThatFits(in: .horizontal) {
                        networkHeader
                        VStack(alignment: .leading, spacing: 12) {
                            networkHeader
                        }
                    }
                    Divider()
                    AdaptiveFactGrid(minimum: 118) {
                        CompactFact(title: "当前记录", value: "\(appModel.overview?.traffic?.recorded ?? appModel.trafficRecords.count)")
                        CompactFact(title: "活动连接", value: "\(appModel.overview?.metrics?.activeConnections ?? 0)")
                        CompactFact(title: "规则命中", value: "\(appModel.activityRuleHitCount)")
                    }
                }
            }
        }
    }

    private var networkHeader: some View {
        HStack(spacing: 14) {
            Image(systemName: "globe")
                .font(.system(size: 28, weight: .medium))
                .foregroundStyle(.blue)
                .frame(width: 42, height: 42)
            VStack(alignment: .leading, spacing: 4) {
                Text("抓包详情")
                    .font(.system(size: 18, weight: .semibold))
                Text(appModel.adminHostPortLabel)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Button {
                appModel.openWebUI()
            } label: {
                Label("在浏览器打开", systemImage: "arrow.up.right.square")
            }
            .buttonStyle(.borderedProminent)
        }
    }
}

struct GroupsWebView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        NativePageScaffold(title: "小组管理") {
            NativeCard {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(spacing: 14) {
                        Image(systemName: "person.2")
                            .font(.system(size: 26, weight: .medium))
                            .foregroundStyle(.blue)
                            .frame(width: 42, height: 42)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("小组管理")
                                .font(.system(size: 18, weight: .semibold))
                            Text("复用 Web UI 的 Groups 工作台")
                                .font(.system(size: 12))
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 8)
                        Button {
                            appModel.openGroupsWebUI()
                        } label: {
                            Label("在浏览器打开", systemImage: "arrow.up.right.square")
                        }
                        .buttonStyle(.borderedProminent)
                    }

                    EmbeddedWebUIPage(url: appModel.webUIURL(path: "groups"))
                        .frame(minHeight: 560)
                        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .stroke(AppSurface.cardBorder)
                        )
                }
            }
        }
    }
}

private struct EmbeddedWebUIPage: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.allowsBackForwardNavigationGestures = true
        webView.setValue(false, forKey: "drawsBackground")
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {
        if webView.url != url {
            webView.load(URLRequest(url: url))
        }
    }
}

@MainActor
private final class OverviewControlModel: ObservableObject {
    @Published var certInfo: CertInfo?
    @Published var mobileDevices: MobileDevicesResponse?
    @Published var proxyAddressInfo: ProxyAddressInfo?
    @Published var trustProbeSession: TrustProbeSession?
    @Published var syncStatus: SyncStatus?
    @Published var remoteInvokeStatus: RemoteInvokeStatus?
    @Published var remoteInvokeGrants: GrantsListResponse?
    @Published var remoteInvokeCalls: CallsListResponse?
    @Published var remoteInvokeSshKey: RemoteInvokeSshKeyRecord?
    @Published var copiedSshKeyAt: Date?
    @Published var copiedPairCodeAt: Date?
    @Published var copiedProbeURLAt: Date?
    @Published var syncRemoteBaseURLDraft = ""
    @Published var isEditingSyncRemoteBaseURL = false
    @Published var isLoading = false
    @Published var isMutating = false
    @Published var errorMessage: String?

    private var baseURL = URL(string: "http://127.0.0.1:9900")!
    private var trustProbePollingTask: Task<Void, Never>?
    private var pollingTrustProbeSessionID: String?

    deinit {
        trustProbePollingTask?.cancel()
    }

    func configure(baseURL: URL) async {
        self.baseURL = baseURL
        await refresh()
    }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let client = try BifrostClient(baseURL: baseURL)
            async let cert = client.fetchCertInfo()
            async let mobile = client.fetchMobileDevices()
            async let proxyAddress = client.fetchProxyAddress()
            async let sync = client.fetchSyncStatus()
            async let remote = client.fetchRemoteInvokeStatus()
            async let grants = client.fetchRemoteInvokeGrants()
            async let calls = client.fetchRemoteInvokeCalls(limit: 12)
            async let sshKey = client.fetchRemoteInvokeSshKey()
            certInfo = try await cert
            mobileDevices = try await mobile
            proxyAddressInfo = try await proxyAddress
            applySyncStatus(try await sync)
            remoteInvokeStatus = try await remote
            remoteInvokeGrants = try await grants
            remoteInvokeCalls = try await calls
            remoteInvokeSshKey = try await sshKey
            try await refreshTrustProbeSession(client: client, forceCreate: false)
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func setRemoteDiscoveryEnabled(_ enabled: Bool) async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            if enabled {
                _ = try await client.enterDiscoveryMode()
            } else {
                try await client.exitDiscoveryMode()
            }
            self.remoteInvokeStatus = try await client.fetchRemoteInvokeStatus()
            self.remoteInvokeGrants = try await client.fetchRemoteInvokeGrants()
            self.remoteInvokeCalls = try await client.fetchRemoteInvokeCalls(limit: 12)
        }
    }

    func refreshRemotePairCode() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            _ = try await client.refreshPairCode()
            self.remoteInvokeStatus = try await client.fetchRemoteInvokeStatus()
        }
    }

    func copyRemotePairCode() {
        guard let pairCode = remoteInvokeStatus?.discoverySession?.pairCode else {
            return
        }
        copyToPasteboard(pairCode)
        copiedPairCodeAt = Date()
    }

    func createRemoteInvokeSshKey() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            let label = "Bifrost Mac Native"
            let secret = if self.remoteInvokeSshKey == nil {
                try await client.createRemoteInvokeSshKey(label: label)
            } else {
                try await client.resetRemoteInvokeSshKey()
            }
            self.copyToPasteboard(secret.bifrostKeyFile)
            self.copiedSshKeyAt = Date()
            self.remoteInvokeSshKey = try await client.fetchRemoteInvokeSshKey()
            self.remoteInvokeGrants = try await client.fetchRemoteInvokeGrants()
        }
    }

    func copyRemoteInvokeSshKey() async {
        await mutate {
            let secret = try await BifrostClient(baseURL: self.baseURL).fetchRemoteInvokeSshPrivateKey()
            self.copyToPasteboard(secret.bifrostKeyFile)
            self.copiedSshKeyAt = Date()
        }
    }

    func setSyncEnabled(_ enabled: Bool) async {
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(enabled: enabled))
            self.applySyncStatus(status)
        }
    }

    func setAutoSyncEnabled(_ enabled: Bool) async {
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(autoSync: enabled))
            self.applySyncStatus(status)
        }
    }

    func openSyncLogin() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).openSyncLogin())
        }
    }

    func logoutSync() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).logoutSyncSession())
        }
    }

    func runSyncNow() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).runSyncNow())
        }
    }

    func handleSyncRemoteBaseURLClick() async {
        guard syncStatus != nil else {
            return
        }
        if syncStatus?.hasSession == true {
            beginSyncRemoteBaseURLEdit()
        } else {
            await openSyncLogin()
        }
    }

    func beginSyncRemoteBaseURLEdit() {
        syncRemoteBaseURLDraft = syncStatus?.remoteBaseURL ?? syncRemoteBaseURLDraft
        isEditingSyncRemoteBaseURL = true
    }

    func cancelSyncRemoteBaseURLEdit() {
        syncRemoteBaseURLDraft = syncStatus?.remoteBaseURL ?? ""
        isEditingSyncRemoteBaseURL = false
    }

    func saveSyncRemoteBaseURL() async {
        let trimmed = syncRemoteBaseURLDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(remoteBaseURL: trimmed))
            self.applySyncStatus(status, forceDraft: true)
            self.isEditingSyncRemoteBaseURL = false
        }
    }

    func installCertificate() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            self.certInfo = try await client.installLocalCA()
            try await self.refreshTrustProbeSession(client: client, forceCreate: false)
        }
    }

    func refreshMobileDevices() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            self.mobileDevices = try await client.refreshMobileDevices()
            try await self.refreshTrustProbeSession(client: client, forceCreate: false)
        }
    }

    func regenerateTrustProbe() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            try await self.refreshTrustProbeSession(client: client, forceCreate: true)
        }
    }

    func copyTrustProbeURL() {
        guard let url = trustProbeSession?.landingURL else {
            return
        }
        copyToPasteboard(url)
        copiedProbeURLAt = Date()
    }

    func openTrustProbeURL() {
        guard let value = trustProbeSession?.landingURL,
              let url = URL(string: value) else {
            return
        }
        NSWorkspace.shared.open(url)
    }

    var preferredTrustProbeHost: String? {
        proxyAddressInfo?.addresses.first(where: \.isPreferred)?.ip
            ?? proxyAddressInfo?.localIPs.first
            ?? certInfo?.localIPs.first
    }

    var detectedMobileDevices: [MobileDevice] {
        (mobileDevices?.ios?.devices ?? []) + (mobileDevices?.android?.devices ?? [])
    }

    private func refreshTrustProbeSession(client: BifrostClient, forceCreate: Bool) async throws {
        guard let host = preferredTrustProbeHost else {
            trustProbeSession = nil
            stopTrustProbePolling()
            return
        }
        if !forceCreate,
           let session = trustProbeSession,
           session.host == host {
            if let refreshed = try? await client.fetchTrustProbeSession(sessionID: session.sessionID) {
                applyTrustProbeSession(refreshed)
                return
            }
        }
        applyTrustProbeSession(try await client.createTrustProbeSession(host: host, ttlSeconds: 600))
    }

    private func applyTrustProbeSession(_ session: TrustProbeSession) {
        trustProbeSession = session
        if session.status == "expired" {
            stopTrustProbePolling()
        } else {
            startTrustProbePolling(sessionID: session.sessionID)
        }
    }

    private func startTrustProbePolling(sessionID: String) {
        if pollingTrustProbeSessionID == sessionID,
           trustProbePollingTask?.isCancelled == false {
            return
        }
        stopTrustProbePolling()
        pollingTrustProbeSessionID = sessionID
        let baseURL = baseURL
        trustProbePollingTask = Task { [weak self] in
            let client = try? BifrostClient(baseURL: baseURL)
            while !Task.isCancelled {
                do {
                    guard let client else {
                        return
                    }
                    let session = try await client.fetchTrustProbeSession(sessionID: sessionID)
                    await MainActor.run {
                        guard self?.trustProbeSession?.sessionID == sessionID else {
                            return
                        }
                        self?.trustProbeSession = session
                        if session.status == "expired" {
                            self?.stopTrustProbePolling()
                        }
                    }
                    try await Task.sleep(nanoseconds: 2_000_000_000)
                } catch is CancellationError {
                    return
                } catch {
                    await MainActor.run {
                        guard self?.trustProbeSession?.sessionID == sessionID else {
                            return
                        }
                        self?.errorMessage = error.localizedDescription
                    }
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                }
            }
        }
    }

    private func stopTrustProbePolling() {
        trustProbePollingTask?.cancel()
        trustProbePollingTask = nil
        pollingTrustProbeSessionID = nil
    }

    private func applySyncStatus(_ status: SyncStatus, forceDraft: Bool = false) {
        syncStatus = status
        if forceDraft || !isEditingSyncRemoteBaseURL {
            syncRemoteBaseURLDraft = status.remoteBaseURL
        }
    }

    private func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }

    func formatRemoteTime(_ flexible: FlexibleString?) -> String {
        guard let value = flexible?.value else {
            return "-"
        }
        return value
    }

    func formatRemoteTime(_ value: Double?) -> String {
        guard let value else {
            return "-"
        }
        return Date(timeIntervalSince1970: value).formatted(date: .omitted, time: .shortened)
    }

    var remoteInvokeCallCountText: String {
        "\(remoteInvokeCalls?.calls.count ?? 0)"
    }

    var remoteInvokeClientCountText: String {
        "\(remoteInvokeGrants?.grants.count ?? 0)"
    }

    var remoteInvokeRecentActivity: String {
        let grantTimes = remoteInvokeGrants?.grants.compactMap(\.lastUsedAt) ?? []
        let callTimes = remoteInvokeCalls?.calls.compactMap { $0.finishedAt ?? $0.createdAt } ?? []
        guard let latest = (grantTimes + callTimes).max() else {
            return "暂无"
        }
        return Date(timeIntervalSince1970: latest).formatted(date: .abbreviated, time: .shortened)
    }

    private func mutate(_ operation: @escaping () async throws -> Void) async {
        isMutating = true
        defer { isMutating = false }
        do {
            try await operation()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

private struct RemoteInvokeCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "Remote Invoke",
                        subtitle: "SSH Key 授权、客户端与调用观测"
                    )
                    Spacer()
                    StatusPill(title: remoteStatusTitle, color: remoteStatusColor)
                    Toggle("", isOn: Binding(
                        get: { model.remoteInvokeStatus?.discoverySession != nil },
                        set: { enabled in Task { await model.setRemoteDiscoveryEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.remoteInvokeStatus == nil || model.isMutating)
                }

                if let session = model.remoteInvokeStatus?.discoverySession {
                    RemoteDiscoveryCodeStrip(session: session, model: model)
                }

                AdaptiveFactGrid(minimum: 126) {
                    CompactFact(title: "已授权客户端", value: model.remoteInvokeClientCountText)
                    CompactFact(title: "活动调用", value: "\(model.remoteInvokeStatus?.activeCallIDs.count ?? 0)")
                    CompactFact(title: "最近调用", value: model.remoteInvokeCallCountText)
                    CompactFact(title: "最近活跃", value: model.remoteInvokeRecentActivity)
                }

                ViewThatFits(in: .horizontal) {
                    remoteInvokeBody
                    VStack(alignment: .leading, spacing: 14) {
                        sshKeySection
                        clientSection
                    }
                }

                if let calls = model.remoteInvokeCalls?.calls, !calls.isEmpty {
                    Divider()
                    AdaptiveFactGrid(minimum: 126) {
                        ForEach(calls.prefix(3)) { call in
                            CompactFact(
                                title: call.callerDisplayName ?? call.commandKind ?? "调用",
                                value: call.status
                            )
                        }
                    }
                }
            }
        }
    }

    private var remoteInvokeBody: some View {
        HStack(alignment: .top, spacing: 14) {
            sshKeySection
                .frame(maxWidth: .infinity, alignment: .leading)
            clientSection
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var sshKeySection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("SSH Key")
                .font(.system(size: 13, weight: .semibold))
            HStack(spacing: 8) {
                StatusPill(
                    title: model.remoteInvokeSshKey?.status ?? "未生成",
                    color: model.remoteInvokeSshKey == nil ? .orange : .green
                )
                Text(shortFingerprint)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
            }
            ViewThatFits(in: .horizontal) {
                sshKeyButtons
                VStack(alignment: .leading, spacing: 8) {
                    sshKeyButtons
                }
            }
        }
    }

    private var sshKeyButtons: some View {
        HStack(spacing: 8) {
            Button(model.remoteInvokeSshKey == nil ? "生成 SSH Key" : "重新生成") {
                Task { await model.createRemoteInvokeSshKey() }
            }
            .buttonStyle(.bordered)
            .disabled(model.isMutating)
            Button {
                Task { await model.copyRemoteInvokeSshKey() }
            } label: {
                Label("复制 SSH Key", systemImage: "doc.on.doc")
                    .labelStyle(.titleAndIcon)
            }
            .buttonStyle(.bordered)
            .disabled(model.remoteInvokeSshKey == nil || model.isMutating)
            if sshKeyRecentlyCopied {
                Text("已复制")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var clientSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("客户端")
                .font(.system(size: 13, weight: .semibold))
            if let grants = model.remoteInvokeGrants?.grants, !grants.isEmpty {
                ForEach(grants.prefix(3)) { grant in
                    RemoteInvokeGrantRow(grant: grant)
                }
            } else {
                Text("暂无已授权客户端")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .frame(height: 38, alignment: .center)
            }
        }
    }

    private var remoteStatusTitle: String {
        guard let status = model.remoteInvokeStatus else {
            return "读取中"
        }
        if status.discoverySession != nil {
            return "发现中"
        }
        return status.state
    }

    private var remoteStatusColor: Color {
        guard let status = model.remoteInvokeStatus else {
            return .secondary
        }
        if status.discoverySession != nil || status.state.lowercased().contains("connected") {
            return .green
        }
        return .secondary
    }

    private var shortFingerprint: String {
        guard let value = model.remoteInvokeSshKey?.sshKeyFingerprint, !value.isEmpty else {
            return "尚未生成"
        }
        return String(value.prefix(22))
    }

    private var sshKeyRecentlyCopied: Bool {
        guard let copiedAt = model.copiedSshKeyAt,
              Date().timeIntervalSince(copiedAt) < 3 else {
            return false
        }
        return true
    }
}

private struct RemoteDiscoveryCodeStrip: View {
    let session: DiscoverySession
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("授权码")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                    Button {
                        model.copyRemotePairCode()
                    } label: {
                        HStack(spacing: 8) {
                            Text(session.pairCode)
                                .font(.system(size: 28, weight: .semibold, design: .monospaced))
                            Image(systemName: "doc.on.doc")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("复制授权码")
                }

                if pairCodeRecentlyCopied {
                    Text("已复制")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                }

                Spacer(minLength: 12)

                VStack(alignment: .trailing, spacing: 6) {
                    Text(remainingText(now: context.date))
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(remainingSeconds(now: context.date) > 0 ? Color.secondary : Color.orange)
                    Button {
                        Task { await model.refreshRemotePairCode() }
                    } label: {
                        Label("重置", systemImage: "arrow.triangle.2.circlepath")
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.isMutating)
                }
            }
            .padding(.vertical, 12)
            .padding(.horizontal, 14)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
        }
    }

    private var pairCodeRecentlyCopied: Bool {
        guard let copiedAt = model.copiedPairCodeAt else {
            return false
        }
        return Date().timeIntervalSince(copiedAt) < 3
    }

    private func remainingText(now: Date) -> String {
        let seconds = remainingSeconds(now: now)
        guard seconds > 0 else {
            return "已过期"
        }
        return String(format: "剩余 %02d:%02d", seconds / 60, seconds % 60)
    }

    private func remainingSeconds(now: Date) -> Int {
        let expiresAt = normalizedDate(from: session.expiresAt)
        return max(0, Int(ceil(expiresAt.timeIntervalSince(now))))
    }

    private func normalizedDate(from epoch: Double) -> Date {
        if epoch > 1_000_000_000_000 {
            return Date(timeIntervalSince1970: epoch / 1000)
        }
        return Date(timeIntervalSince1970: epoch)
    }
}

private struct SystemProxyCard: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var copiedAddress: String?
    @State private var copiedAt: Date?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack(alignment: .top) {
                    NativeCardHeader(title: "系统代理", subtitle: systemProxySubtitle)
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { systemProxyEnabledByBifrost },
                        set: { enabled in Task { await appModel.setSystemProxyEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(systemProxyToggleDisabled)
                }

                StatusPill(title: systemProxyStatusTitle, color: systemProxyStatusColor)

                VStack(alignment: .leading, spacing: 8) {
                    Text("代理地址")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .textCase(.uppercase)
                    if proxyAddresses.isEmpty {
                        ProxyAddressCopyRow(
                            title: "本机",
                            address: appModel.adminHostPortLabel,
                            isPreferred: true,
                            isCopied: copiedAddress == appModel.adminHostPortLabel && recentlyCopied
                        ) {
                            copyProxyAddress(appModel.adminHostPortLabel)
                        }
                    } else {
                        ForEach(proxyAddresses) { address in
                            ProxyAddressCopyRow(
                                title: address.isPreferred ? "推荐" : address.ip,
                                address: address.address,
                                isPreferred: address.isPreferred,
                                isCopied: copiedAddress == address.address && recentlyCopied
                            ) {
                                copyProxyAddress(address.address)
                            }
                        }
                    }
                }

                Divider()
                    .opacity(0.55)

                VStack(spacing: 9) {
                    SystemProxyOptionToggleRow(
                        title: "Boot/Shutdown Cleanup",
                        detail: launchdDetail,
                        isOn: launchdEnabled,
                        isDisabled: launchdToggleDisabled
                    ) { enabled in
                        Task { await appModel.setSystemProxyLaunchdEnabled(enabled) }
                    }

                    SystemProxyOptionToggleRow(
                        title: "Inject Bifrost Badge",
                        detail: "只应用于 HTML 页面，用来标记流量正在经过 Bifrost。",
                        isOn: appModel.performanceConfig?.traffic.injectBifrostBadge ?? false,
                        isDisabled: appModel.performanceConfig == nil || appModel.isTogglingInjectBifrostBadge
                    ) { enabled in
                        Task { await appModel.setInjectBifrostBadgeEnabled(enabled) }
                    }

                    SystemProxyOptionStatusRow(
                        title: "CLI Proxy (ENV)",
                        detail: cliProxyDetail,
                        status: appModel.cliProxyStatus?.enabled == true ? "Enabled" : "Disabled",
                        color: appModel.cliProxyStatus?.enabled == true ? .green : .secondary
                    )
                }
            }
        }
    }

    private var proxyAddresses: [ProxyAddress] {
        appModel.proxyAddressInfo?.addresses ?? []
    }

    private var systemProxyToggleDisabled: Bool {
        !(appModel.systemProxyStatus?.supported ?? false) || appModel.isTogglingSystemProxy
    }

    private var systemProxyEnabledByBifrost: Bool {
        guard let status = appModel.systemProxyStatus else {
            return false
        }
        return status.enabled && status.managedByBifrost != false
    }

    private var systemProxyStatusTitle: String {
        guard let status = appModel.systemProxyStatus else {
            return "读取中"
        }
        guard status.supported else {
            return "不支持"
        }
        if status.enabled && status.managedByBifrost == false {
            return "被其他代理占用"
        }
        if status.configuredEnabled == true && !systemProxyEnabledByBifrost {
            return "已配置待接管"
        }
        return systemProxyEnabledByBifrost ? "已接管" : "未接管"
    }

    private var systemProxyStatusColor: Color {
        guard let status = appModel.systemProxyStatus else {
            return .secondary
        }
        if status.enabled && status.managedByBifrost == false {
            return .orange
        }
        if status.configuredEnabled == true && !systemProxyEnabledByBifrost {
            return .orange
        }
        return systemProxyEnabledByBifrost ? .green : .secondary
    }

    private var systemProxySubtitle: String {
        if let info = appModel.proxyAddressInfo,
           let preferred = info.addresses.first(where: \.isPreferred) ?? info.addresses.first {
            return preferred.address
        }
        if let status = appModel.systemProxyStatus,
           let host = status.host,
           let port = status.port {
            return "\(host):\(port)"
        }
        return appModel.adminHostPortLabel
    }

    private var launchdEnabled: Bool {
        guard let launchd = appModel.systemProxyLaunchdStatus else {
            return false
        }
        return launchd.installed && launchd.loaded && launchd.needsUpgrade != true
    }

    private var launchdToggleDisabled: Bool {
        !(appModel.systemProxyLaunchdStatus?.supported ?? false) || appModel.isTogglingSystemProxyLaunchd
    }

    private var launchdDetail: String {
        guard let launchd = appModel.systemProxyLaunchdStatus else {
            return "读取中"
        }
        if launchd.needsUpgrade == true {
            return launchd.needsUpgradeReason ?? launchd.message ?? "清理组件需要升级。"
        }
        return launchd.message ?? "崩溃恢复和重启后恢复 Bifrost 管理的系统代理设置。"
    }

    private var cliProxyDetail: String {
        guard let cli = appModel.cliProxyStatus else {
            return "读取中"
        }
        let shell = cli.shell ?? "-"
        let files = cli.configFiles.prefix(2).map { URL(fileURLWithPath: $0).lastPathComponent }.joined(separator: ", ")
        return "Shell: \(shell) · Files: \(files.isEmpty ? "-" : files)"
    }

    private var recentlyCopied: Bool {
        guard let copiedAt else {
            return false
        }
        return Date().timeIntervalSince(copiedAt) < 3
    }

    private func copyProxyAddress(_ address: String) {
        appModel.copyToPasteboard(address)
        copiedAddress = address
        copiedAt = Date()
    }
}

private struct ProxyAddressCopyRow: View {
    let title: String
    let address: String
    let isPreferred: Bool
    let isCopied: Bool
    let onCopy: () -> Void

    var body: some View {
        Button(action: onCopy) {
            HStack(spacing: 8) {
                Image(systemName: isPreferred ? "star.fill" : "network")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(isPreferred ? Color.yellow : Color.secondary)
                    .frame(width: 14)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Text(address)
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Label(isCopied ? "已复制" : "复制", systemImage: isCopied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(isCopied ? Color.green : Color.secondary)
                    .labelStyle(.titleAndIcon)
            }
            .padding(.vertical, 7)
            .padding(.horizontal, 9)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
    }
}

private struct SystemProxyOptionToggleRow: View {
    let title: String
    let detail: String
    let isOn: Bool
    let isDisabled: Bool
    let onToggle: (Bool) -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Toggle("", isOn: Binding(get: { isOn }, set: onToggle))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(isDisabled)
        }
    }
}

private struct SystemProxyOptionStatusRow: View {
    let title: String
    let detail: String
    let status: String
    let color: Color

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            StatusPill(title: status, color: color)
        }
    }
}

private struct TlsInterceptionCard: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var editingKind: TlsListKind?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "TLS 解密",
                        subtitle: "打开为默认解包，关闭为白名单解包"
                    )
                    Spacer()
                    StatusPill(
                        title: tlsModeTitle,
                        color: tlsModeColor
                    )
                    Toggle("", isOn: Binding(
                        get: { appModel.tlsConfig?.enableTlsInterception ?? false },
                        set: { enabled in Task { await appModel.setTlsInterceptionEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(appModel.tlsConfig == nil || appModel.isTogglingTls)
                }

                LazyVGrid(columns: [
                    GridItem(.adaptive(minimum: 112, maximum: 180), spacing: 10, alignment: .topLeading)
                ], alignment: .leading, spacing: 10) {
                    ForEach(TlsListKind.allCases) { kind in
                        Button {
                            editingKind = kind
                        } label: {
                            TlsListCountTile(
                                title: kind.title,
                                value: "\(kind.values(in: appModel.tlsConfig).count)",
                                tint: kind.isInclude ? .green : .orange
                            )
                        }
                        .buttonStyle(.plain)
                        .disabled(appModel.tlsConfig == nil || appModel.isTogglingTls)
                    }
                }
            }
        }
        .sheet(item: $editingKind) { kind in
            TlsListEditorSheet(
                kind: kind,
                values: kind.values(in: appModel.tlsConfig),
                isSaving: appModel.isTogglingTls
            ) { values in
                guard var config = appModel.tlsConfig else {
                    return
                }
                kind.update(&config, values: values)
                Task {
                    await appModel.updateTlsConfig(config)
                }
            }
        }
    }

    private var tlsModeTitle: String {
        guard let config = appModel.tlsConfig else {
            return "读取中"
        }
        return config.enableTlsInterception ? "默认解包" : "白名单解包"
    }

    private var tlsModeColor: Color {
        guard let config = appModel.tlsConfig else {
            return .secondary
        }
        return config.enableTlsInterception ? .green : .blue
    }
}

private enum TlsListKind: String, CaseIterable, Identifiable {
    case appInclude
    case appExclude
    case domainInclude
    case domainExclude
    case ipInclude
    case ipExclude

    var id: String { rawValue }

    var title: String {
        switch self {
        case .appInclude:
            return "应用白名单"
        case .appExclude:
            return "应用黑名单"
        case .domainInclude:
            return "域名白名单"
        case .domainExclude:
            return "域名黑名单"
        case .ipInclude:
            return "IP 白名单"
        case .ipExclude:
            return "IP 黑名单"
        }
    }

    var editorTitle: String {
        "\(title)编辑"
    }

    var placeholder: String {
        switch self {
        case .appInclude, .appExclude:
            return "Safari\nGoogle Chrome\ncom.apple.Safari"
        case .domainInclude, .domainExclude:
            return "*.example.com\napi.example.com"
        case .ipInclude, .ipExclude:
            return "10.0.0.0/8\n192.168.1.20"
        }
    }

    var isInclude: Bool {
        switch self {
        case .appInclude, .domainInclude, .ipInclude:
            return true
        case .appExclude, .domainExclude, .ipExclude:
            return false
        }
    }

    func values(in config: TlsConfig?) -> [String] {
        guard let config else {
            return []
        }
        switch self {
        case .appInclude:
            return config.appInterceptInclude
        case .appExclude:
            return config.appInterceptExclude
        case .domainInclude:
            return config.interceptInclude
        case .domainExclude:
            return config.interceptExclude
        case .ipInclude:
            return config.ipInterceptInclude
        case .ipExclude:
            return config.ipInterceptExclude
        }
    }

    func update(_ config: inout TlsConfig, values: [String]) {
        switch self {
        case .appInclude:
            config.appInterceptInclude = values
        case .appExclude:
            config.appInterceptExclude = values
        case .domainInclude:
            config.interceptInclude = values
        case .domainExclude:
            config.interceptExclude = values
        case .ipInclude:
            config.ipInterceptInclude = values
        case .ipExclude:
            config.ipInterceptExclude = values
        }
    }
}

private struct TlsListCountTile: View {
    let title: String
    let value: String
    let tint: Color

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(value)
                    .font(.system(size: 17, weight: .semibold))
            }
            Spacer()
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 11)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}

private struct TlsListEditorSheet: View {
    let kind: TlsListKind
    let isSaving: Bool
    let onSave: ([String]) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var draftText: String

    init(kind: TlsListKind, values: [String], isSaving: Bool, onSave: @escaping ([String]) -> Void) {
        self.kind = kind
        self.isSaving = isSaving
        self.onSave = onSave
        _draftText = State(initialValue: values.joined(separator: "\n"))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                NativeCardHeader(title: kind.editorTitle, subtitle: "每行一个匹配项，保存后立即影响 TLS 解包范围")
                Spacer()
                Button("取消") {
                    dismiss()
                }
                .buttonStyle(.borderless)
                Button("保存") {
                    onSave(normalizedValues)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .disabled(isSaving)
            }

            ZStack(alignment: .topLeading) {
                TextEditor(text: $draftText)
                    .font(.system(size: 13, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .padding(8)
                    .background(AppSurface.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .stroke(AppSurface.cardBorder)
                    )
                if draftText.isEmpty {
                    Text(kind.placeholder)
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .padding(.top, 16)
                        .padding(.leading, 14)
                        .allowsHitTesting(false)
                }
            }
            .frame(minWidth: 520, minHeight: 260)
        }
        .padding(22)
        .background(AppSurface.content)
    }

    private var normalizedValues: [String] {
        var seen = Set<String>()
        return draftText
            .split(whereSeparator: \.isNewline)
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .filter { seen.insert($0).inserted }
    }
}

private struct SyncControlCard: View {
    @EnvironmentObject private var appModel: AppModel
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    NativeCardHeader(
                        title: "同步",
                        subtitle: model.syncStatus?.user?.email ?? "规则与设置远端同步"
                    )
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { model.syncStatus?.enabled ?? false },
                        set: { enabled in Task { await model.setSyncEnabled(enabled); await appModel.refreshSyncStatus() } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.syncStatus == nil || model.isMutating)
                }
                SyncRemoteServiceRow(model: model)
                ViewThatFits(in: .horizontal) {
                    syncControls
                    VStack(alignment: .leading, spacing: 10) {
                        syncControls
                    }
                }
            }
        }
    }

    private var syncControls: some View {
        HStack(spacing: 10) {
            StatusPill(
                title: model.syncStatus?.authorized == true ? "已授权" : "未授权",
                color: model.syncStatus?.authorized == true ? .green : .orange
            )
            Toggle("自动同步", isOn: Binding(
                get: { model.syncStatus?.autoSync ?? false },
                set: { enabled in Task { await model.setAutoSyncEnabled(enabled); await appModel.refreshSyncStatus() } }
            ))
            .toggleStyle(.switch)
            .font(.system(size: 12))
            .disabled(model.syncStatus == nil || model.isMutating)
            Spacer(minLength: 8)
            Button(model.syncStatus?.hasSession == true ? "退出" : "登录") {
                Task {
                    if model.syncStatus?.hasSession == true {
                        await model.logoutSync()
                    } else {
                        await model.openSyncLogin()
                    }
                    await appModel.refreshSyncStatus()
                }
            }
            .buttonStyle(.borderless)
            Button("同步") {
                Task { await model.runSyncNow() }
            }
            .buttonStyle(.borderless)
            .disabled(model.syncStatus?.enabled != true || model.isMutating)
        }
    }
}

private struct SyncRemoteServiceRow: View {
    @ObservedObject var model: OverviewControlModel
    @FocusState private var isFocused: Bool

    var body: some View {
        Group {
            if model.isEditingSyncRemoteBaseURL {
                editingRow
            } else {
                readRow
            }
        }
        .animation(.snappy(duration: 0.18), value: model.isEditingSyncRemoteBaseURL)
    }

    private var readRow: some View {
        Button {
            Task { await model.handleSyncRemoteBaseURLClick() }
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("远端服务")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Text(remoteBaseURLText)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Label(isSignedIn ? "编辑" : "登录授权", systemImage: isSignedIn ? "pencil" : "person.crop.circle.badge.checkmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .labelStyle(.titleAndIcon)
            }
            .padding(.vertical, 9)
            .padding(.horizontal, 11)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(model.syncStatus == nil || model.isMutating)
        .help(isSignedIn ? "编辑远端服务地址" : "登录并授权远端同步")
    }

    private var editingRow: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 5) {
                Text("远端服务")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                TextField("https://bifrost.example.com", text: $model.syncRemoteBaseURLDraft)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12, design: .monospaced))
                    .focused($isFocused)
                    .onSubmit {
                        Task { await model.saveSyncRemoteBaseURL() }
                    }
            }
            Button {
                Task { await model.saveSyncRemoteBaseURL() }
            } label: {
                Image(systemName: "checkmark")
            }
            .buttonStyle(.borderless)
            .disabled(model.isMutating)
            .help("保存远端服务地址")
            Button {
                model.cancelSyncRemoteBaseURLEdit()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .disabled(model.isMutating)
            .help("取消编辑")
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 11)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onAppear {
            isFocused = true
        }
    }

    private var remoteBaseURLText: String {
        guard let value = model.syncStatus?.remoteBaseURL, !value.isEmpty else {
            return "未配置"
        }
        return value
    }

    private var isSignedIn: Bool {
        model.syncStatus?.hasSession == true
    }
}

private struct CertificateManagementCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "证书管理",
                        subtitle: model.certInfo?.statusMessage ?? "安装并验证本机 CA"
                    )
                    Spacer()
                    StatusPill(
                        title: model.certInfo?.trusted == true ? "已信任" : "未信任",
                        color: model.certInfo?.trusted == true ? .green : .orange
                    )
                }

                CertificateSummarySection(model: model, fingerprintText: fingerprintText)
            }
        }
    }

    private var fingerprintText: String {
        guard let value = model.certInfo?.sha256Fingerprint, !value.isEmpty else {
            return "SHA256: -"
        }
        return "SHA256: \(value)"
    }
}

private struct CertificateSummarySection: View {
    @ObservedObject var model: OverviewControlModel
    let fingerprintText: String

    private var factColumns: [GridItem] {
        [
            GridItem(.adaptive(minimum: 118, maximum: 180), spacing: 10, alignment: .topLeading)
        ]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            LazyVGrid(columns: factColumns, alignment: .leading, spacing: 10) {
                CompactFact(title: "本机 CA", value: model.certInfo?.statusLabel ?? "读取中")
                CompactFact(title: "代理地址", value: model.proxyAddressInfo?.addresses.first(where: \.isPreferred)?.address ?? "-")
            }

            Text(fingerprintText)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)

            if shouldShowInstallButton {
                Button("安装本机 CA") {
                    Task { await model.installCertificate() }
                }
                .buttonStyle(.bordered)
                .disabled(model.certInfo == nil || model.certInfo?.available == false || model.isMutating)
            }
        }
    }

    private var shouldShowInstallButton: Bool {
        model.certInfo?.trusted != true
    }
}

private struct RemoteInvokeGrantRow: View {
    let grant: Grant

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(grant.status == "active" ? Color.green : Color.secondary.opacity(0.45))
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(grant.callerDisplayName ?? String(grant.callerFingerprint.prefix(10)))
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text("调用 \(grant.useCount ?? 0) 次")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text(grant.grantScope)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }
}

private struct MobileDeviceRow: View {
    let device: MobileDevice

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: device.platform.lowercased().contains("ios") ? "iphone" : "smartphone")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(device.name ?? device.id)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(device.certificateStatus?.message ?? device.statusMessage)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            StatusPill(
                title: device.certificateStatus?.trusted == true ? "已信任" : device.status,
                color: device.certificateStatus?.trusted == true ? .green : .orange
            )
        }
    }
}

private struct MobileConnectionCheckCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "移动端连接检查",
                        subtitle: "扫码检查同网、证书与代理连接"
                    )
                    Spacer()
                    StatusPill(title: probeStatusTitle, color: probeStatusColor)
                }

                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .top, spacing: 18) {
                        QRPreview(urlString: model.trustProbeSession?.qrCodeURL)
                        mobileProbeDetails
                    }
                    VStack(alignment: .leading, spacing: 14) {
                        QRPreview(urlString: model.trustProbeSession?.qrCodeURL)
                        mobileProbeDetails
                    }
                }
            }
        }
    }

    private var mobileProbeDetails: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(model.trustProbeSession?.landingURL ?? "等待生成检查链接")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .truncationMode(.middle)

            HStack(spacing: 8) {
                Button("打开") {
                    model.openTrustProbeURL()
                }
                .buttonStyle(.borderless)
                .disabled(model.trustProbeSession == nil)
                Button(model.copiedProbeURLAt.map { Date().timeIntervalSince($0) < 3 } == true ? "已复制" : "复制链接") {
                    model.copyTrustProbeURL()
                }
                .buttonStyle(.borderless)
                .disabled(model.trustProbeSession == nil)
                Spacer()
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("已连接设备")
                    .font(.system(size: 13, weight: .semibold))
                if model.detectedMobileDevices.isEmpty {
                    Text("暂无 USB 设备。可让手机扫描二维码检查同网、证书与代理连接。")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    ForEach(model.detectedMobileDevices.prefix(3)) { device in
                        MobileDeviceRow(device: device)
                    }
                }
            }

            if let devices = model.trustProbeSession?.devices, !devices.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 6) {
                        Text("扫码设备")
                            .font(.system(size: 13, weight: .semibold))
                        Text("\(devices.count)")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.quaternary.opacity(0.7), in: Capsule())
                    }
                    ForEach(devices.prefix(4)) { device in
                        TrustProbeDeviceRow(device: device)
                    }
                }
            } else {
                Text("扫码后会在这里显示正在连接的设备。")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }

            if let session = model.trustProbeSession,
               !session.proxyConfigured,
               let message = session.proxyConfigurationMessage,
               !message.isEmpty {
                TrustProbeMessageRow(symbol: "exclamationmark.triangle.fill", tint: .orange, message: message)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    private var probeStatusTitle: String {
        guard let session = model.trustProbeSession else {
            return "未生成"
        }
        if session.tlsTrusted {
            return "证书可信"
        }
        if session.networkReachable {
            return "网络可达"
        }
        if session.opened {
            return "已打开"
        }
        return session.status
    }

    private var probeStatusColor: Color {
        guard let session = model.trustProbeSession else {
            return .orange
        }
        if session.tlsTrusted {
            return .green
        }
        if session.networkReachable {
            return .blue
        }
        return .orange
    }
}

private struct QRPreview: View {
    let urlString: String?
    @State private var svgText: String?
    @State private var isLoading = false
    @State private var didFail = false

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(.white)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(AppSurface.cardBorder)
                )
            if let svgText {
                InlineSVGView(svgText: svgText)
                    .padding(10)
            } else if isLoading {
                ProgressView()
            } else {
                Image(systemName: "qrcode")
                    .font(.system(size: 42, weight: .regular))
                    .foregroundStyle(didFail ? AnyShapeStyle(.orange.opacity(0.7)) : AnyShapeStyle(.tertiary))
            }
        }
        .frame(width: 136, height: 136)
        .task(id: urlString) {
            await loadQRCode()
        }
    }

    private func loadQRCode() async {
        await MainActor.run {
            svgText = nil
            didFail = false
            isLoading = urlString != nil
        }
        guard let value = urlString,
              let originalURL = URL(string: value),
              let loadURL = localQRCodeLoadURL(from: originalURL) else {
            await MainActor.run {
                isLoading = false
                didFail = urlString != nil
            }
            return
        }

        do {
            var request = URLRequest(url: loadURL, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 5)
            request.setValue("image/svg+xml", forHTTPHeaderField: "Accept")
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse,
                  (200 ..< 300).contains(http.statusCode),
                  let text = String(data: data, encoding: .utf8),
                  let svg = normalizedSVG(from: text) else {
                throw URLError(.cannotDecodeContentData)
            }
            await MainActor.run {
                svgText = svg
                isLoading = false
                didFail = false
            }
        } catch {
            await MainActor.run {
                svgText = nil
                isLoading = false
                didFail = true
            }
        }
    }

    private func localQRCodeLoadURL(from url: URL) -> URL? {
        guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return nil
        }
        components.scheme = "http"
        components.host = "127.0.0.1"
        return components.url
    }

    private func normalizedSVG(from text: String) -> String? {
        guard let range = text.range(of: "<svg", options: [.caseInsensitive]) else {
            return nil
        }
        return String(text[range.lowerBound...])
    }
}

private struct InlineSVGView: NSViewRepresentable {
    let svgText: String

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.setValue(false, forKey: "drawsBackground")
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {
        guard context.coordinator.lastSVGText != svgText else {
            return
        }
        context.coordinator.lastSVGText = svgText
        webView.loadHTMLString(html(for: svgText), baseURL: nil)
    }

    private func html(for svgText: String) -> String {
        """
        <!doctype html>
        <html>
        <head>
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <style>
            html, body {
              margin: 0;
              width: 100%;
              height: 100%;
              overflow: hidden;
              background: transparent;
            }
            body {
              display: flex;
              align-items: center;
              justify-content: center;
            }
            svg {
              width: 100%;
              height: 100%;
              display: block;
            }
          </style>
        </head>
        <body>\(svgText)</body>
        </html>
        """
    }

    final class Coordinator {
        var lastSVGText: String?
    }
}

private struct TrustProbeDeviceRow: View {
    let device: TrustProbeDevice

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(device.tlsTrusted ? Color.green : (device.networkReachable ? Color.blue : Color.orange))
                .frame(width: 7, height: 7)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(shortDeviceID)
                        .font(.system(size: 11, weight: .semibold, design: .monospaced))
                        .lineLimit(1)
                    if let platform = device.platformHint, !platform.isEmpty {
                        Text(platform)
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.quaternary.opacity(0.65), in: Capsule())
                    }
                    if let ip = device.clientIP, !ip.isEmpty {
                        Text(ip)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    Text(formatProbeLastSeen(device.lastSeen))
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }

                HStack(spacing: 5) {
                    TrustProbeDeviceStatusTag(title: device.opened ? "已打开" : "等待打开", color: device.opened ? .green : .gray)
                    TrustProbeDeviceStatusTag(title: device.networkReachable ? "网络可达" : "网络待检", color: device.networkReachable ? .green : .gray)
                    TrustProbeDeviceStatusTag(title: device.tlsTrusted ? "证书可信" : tlsPendingTitle, color: tlsStatusColor)
                    TrustProbeDeviceStatusTag(title: proxyAccessTitle, color: proxyAccessColor)
                    TrustProbeDeviceStatusTag(title: device.proxyConfigured ? "代理已配置" : proxyConfiguredTitle, color: device.proxyConfigured ? .green : proxyConfiguredColor)
                }

                if let message = device.lastError ?? device.proxyConfigurationMessage ?? device.proxyAccessMessage,
                   !message.isEmpty {
                    Text(message)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private var shortDeviceID: String {
        guard device.deviceID.count > 10 else {
            return device.deviceID
        }
        return String(device.deviceID.prefix(8))
    }

    private var tlsPendingTitle: String {
        device.status == "tls_failed" ? "证书失败" : "证书待检"
    }

    private var tlsStatusColor: Color {
        if device.tlsTrusted {
            return .green
        }
        if device.status == "tls_failed" {
            return .red
        }
        return .gray
    }

    private var proxyAccessTitle: String {
        guard let status = device.proxyAccessStatus, !status.isEmpty else {
            return "授权待检"
        }
        return status == "allowed" ? "授权通过" : "授权 \(status)"
    }

    private var proxyAccessColor: Color {
        if device.proxyAccessAllowed == true {
            return .green
        }
        return device.proxyAccessStatus == "pending" ? .orange : .gray
    }

    private var proxyConfiguredTitle: String {
        device.proxyConfigurationMessage?.isEmpty == false ? "代理缺失" : "代理待检"
    }

    private var proxyConfiguredColor: Color {
        device.proxyConfigurationMessage?.isEmpty == false ? .red : .gray
    }
}

private struct TrustProbeDeviceStatusTag: View {
    let title: String
    let color: Color

    var body: some View {
        Text(title)
            .font(.system(size: 9, weight: .medium))
            .foregroundStyle(color)
            .lineLimit(1)
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(color.opacity(0.10), in: Capsule())
    }
}

private struct TrustProbeMessageRow: View {
    let symbol: String
    let tint: Color
    let message: String

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(tint)
                .padding(.top, 1)
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private func formatProbeLastSeen(_ value: String) -> String {
    if let date = ISO8601DateFormatter().date(from: value) {
        return date.formatted(date: .omitted, time: .standard)
    }
    return value
}

private struct OverviewToggleCard: View {
    let title: String
    let subtitle: String
    let status: String
    let tint: Color
    let isOn: Bool
    let isDisabled: Bool
    let onToggle: (Bool) -> Void

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(title: title, subtitle: subtitle)
                    Spacer()
                    Toggle("", isOn: Binding(get: { isOn }, set: onToggle))
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(isDisabled)
                }
                StatusPill(title: status, color: tint)
            }
        }
    }
}

private struct ActivityBars: View, Equatable {
    let rows: [(String, Int)]

    static func == (lhs: ActivityBars, rhs: ActivityBars) -> Bool {
        guard lhs.rows.count == rhs.rows.count else {
            return false
        }
        for index in lhs.rows.indices where lhs.rows[index].0 != rhs.rows[index].0 || lhs.rows[index].1 != rhs.rows[index].1 {
            return false
        }
        return true
    }

    var body: some View {
        let maxValue = max(rows.map(\.1).max() ?? 1, 1)
        VStack(spacing: 10) {
            if rows.isEmpty {
                EmptyNativeState(title: "暂无流量")
                    .frame(height: 180)
            } else {
                ForEach(rows, id: \.0) { row in
                    HStack(spacing: 10) {
                        Text(row.0)
                            .font(.system(size: 12))
                            .lineLimit(1)
                            .frame(width: 150, alignment: .leading)
                        GeometryReader { proxy in
                            RoundedRectangle(cornerRadius: 3)
                                .fill(Color.accentColor.opacity(0.78))
                                .frame(width: max(4, proxy.size.width * CGFloat(row.1) / CGFloat(maxValue)))
                        }
                        .frame(height: 8)
                        Text("\(row.1)")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(.secondary)
                            .frame(width: 44, alignment: .trailing)
                    }
                }
            }
        }
    }
}

private struct NativeMetricCard: View {
    let title: String
    let value: String
    let caption: String
    let tint: Color

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    Text(title)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Circle()
                        .fill(tint)
                        .frame(width: 8, height: 8)
                }
                Text(value)
                    .font(.system(size: 31, weight: .bold, design: .rounded))
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                Text(caption)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(minHeight: 126, alignment: .topLeading)
        }
    }
}

private func formatRate(_ value: Double?) -> String {
    guard let value else {
        return "0 B/s"
    }
    return "\(formatBytes(Int(value)))/s"
}

private func formatBytes(_ value: Int?) -> String {
    formatBytes(value ?? 0)
}

private func formatBytes(_ value: Int) -> String {
    let bytes = max(value, 0)
    if bytes < 1024 {
        return "\(bytes) B"
    }
    let units = ["KB", "MB", "GB", "TB"]
    var scaled = Double(bytes) / 1024
    var unitIndex = 0
    while scaled >= 1024, unitIndex < units.count - 1 {
        scaled /= 1024
        unitIndex += 1
    }
    if scaled >= 100 {
        return String(format: "%.0f %@", scaled, units[unitIndex])
    }
    return String(format: "%.1f %@", scaled, units[unitIndex])
}
