import AppKit
import BifrostNativeCore
import SwiftUI

struct ActivityView: View {
    @EnvironmentObject private var appModel: AppModel

    private var metrics: SystemOverview.Metrics? {
        appModel.overview?.metrics
    }

    var body: some View {
        NativePageScaffold(title: "活动") {
            LazyVGrid(columns: [
                GridItem(.flexible(), spacing: 18),
                GridItem(.flexible(), spacing: 18),
                GridItem(.flexible(), spacing: 18),
            ], spacing: 18) {
                NativeMetricCard(
                    title: "活动连接",
                    value: "\(metrics?.activeConnections ?? 0)",
                    caption: "\(appModel.activityClientAppCounts.count) 个应用 · \(appModel.activityClientIpCounts.count) 个 IP",
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
                    value: "\(metrics?.totalRequests ?? appModel.trafficRecords.count)",
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
                    ActivityBars(rows: appModel.activityClientAppCounts.prefix(6).map { ($0.name, $0.count) })
                }
            }
        }
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
        case .running:
            return "运行中"
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

struct DashboardView: View {
    @EnvironmentObject private var appModel: AppModel
    @StateObject private var model = OverviewControlModel()

    var body: some View {
        NativePageScaffold(title: "概览") {
            LazyVGrid(columns: [
                GridItem(.flexible(), spacing: 18),
                GridItem(.flexible(), spacing: 18),
            ], spacing: 18) {
                OverviewToggleCard(
                    title: "系统代理",
                    subtitle: systemProxySubtitle,
                    status: appModel.systemProxyStatus?.enabled == true ? "已接管" : "未接管",
                    tint: appModel.systemProxyStatus?.enabled == true ? .green : .orange,
                    isOn: appModel.systemProxyStatus?.enabled ?? false,
                    isDisabled: !(appModel.systemProxyStatus?.supported ?? false) || appModel.isTogglingSystemProxy
                ) { enabled in
                    Task { await appModel.setSystemProxyEnabled(enabled) }
                }

                TlsInterceptionCard()

                RemoteInvokeCard(model: model)
                    .gridCellColumns(2)
                SyncControlCard(model: model)
                CertificateControlCard(model: model)
                    .gridCellColumns(2)

                NativeCard {
                    VStack(alignment: .leading, spacing: 16) {
                        HStack {
                            NativeCardHeader(title: "本机服务", subtitle: appModel.adminHostPortLabel)
                            Spacer()
                            Button("刷新") {
                                Task {
                                    await appModel.refreshData()
                                    await model.refresh()
                                }
                            }
                            .buttonStyle(.borderless)
                        }
                        HStack(spacing: 12) {
                            CompactFact(title: "版本", value: appModel.overview?.system?.version ?? "-")
                            CompactFact(title: "PID", value: appModel.overview?.system?.pid.map(String.init) ?? "-")
                            CompactFact(title: "记录", value: "\(appModel.overview?.traffic?.recorded ?? appModel.trafficRecords.count)")
                        }
                    }
                }
            }
        }
        .task(id: appModel.adminURL) {
            await model.configure(baseURL: appModel.adminURL)
        }
    }

    private var systemProxySubtitle: String {
        guard let status = appModel.systemProxyStatus else {
            return "读取中"
        }
        guard status.supported else {
            return "当前平台不支持"
        }
        if let host = status.host, let port = status.port {
            return "\(host):\(port)"
        }
        return appModel.adminHostPortLabel
    }
}

struct NetworkWebView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        NativePageScaffold(title: "网络") {
            NativeCard {
                VStack(alignment: .leading, spacing: 18) {
                    HStack(spacing: 14) {
                        Image(systemName: "globe")
                            .font(.system(size: 28, weight: .medium))
                            .foregroundStyle(.blue)
                            .frame(width: 42, height: 42)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("网络详情")
                                .font(.system(size: 18, weight: .semibold))
                            Text(appModel.adminHostPortLabel)
                                .font(.system(size: 12))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button {
                            appModel.openWebUI()
                        } label: {
                            Label("在浏览器打开", systemImage: "arrow.up.right.square")
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    Divider()
                    HStack(spacing: 12) {
                        CompactFact(title: "当前记录", value: "\(appModel.overview?.traffic?.recorded ?? appModel.trafficRecords.count)")
                        CompactFact(title: "活动连接", value: "\(appModel.overview?.metrics?.activeConnections ?? 0)")
                        CompactFact(title: "规则命中", value: "\(appModel.activityRuleHitCount)")
                    }
                }
            }
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
    @Published var copiedProbeURLAt: Date?
    @Published var isLoading = false
    @Published var isMutating = false
    @Published var errorMessage: String?

    private var baseURL = URL(string: "http://127.0.0.1:9900")!

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
            syncStatus = try await sync
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
            self.syncStatus = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(enabled: enabled))
        }
    }

    func setAutoSyncEnabled(_ enabled: Bool) async {
        await mutate {
            self.syncStatus = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(autoSync: enabled))
        }
    }

    func openSyncLogin() async {
        await mutate {
            self.syncStatus = try await BifrostClient(baseURL: self.baseURL).openSyncLogin()
        }
    }

    func logoutSync() async {
        await mutate {
            self.syncStatus = try await BifrostClient(baseURL: self.baseURL).logoutSyncSession()
        }
    }

    func runSyncNow() async {
        await mutate {
            self.syncStatus = try await BifrostClient(baseURL: self.baseURL).runSyncNow()
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
            return
        }
        if !forceCreate,
           let session = trustProbeSession,
           session.host == host {
            trustProbeSession = try? await client.fetchTrustProbeSession(sessionID: session.sessionID)
            if trustProbeSession != nil {
                return
            }
        }
        trustProbeSession = try await client.createTrustProbeSession(host: host, ttlSeconds: 600)
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

                HStack(spacing: 12) {
                    CompactFact(title: "已授权客户端", value: model.remoteInvokeClientCountText)
                    CompactFact(title: "活动调用", value: "\(model.remoteInvokeStatus?.activeCallIDs.count ?? 0)")
                    CompactFact(title: "最近调用", value: model.remoteInvokeCallCountText)
                    CompactFact(title: "最近活跃", value: model.remoteInvokeRecentActivity)
                }

                HStack(alignment: .top, spacing: 14) {
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
                            Spacer()
                        }
                        HStack(spacing: 8) {
                            Button(model.remoteInvokeSshKey == nil ? "生成 SSH Key" : "重新生成") {
                                Task { await model.createRemoteInvokeSshKey() }
                            }
                            .buttonStyle(.bordered)
                            .disabled(model.isMutating)
                            Button(copiedTitle) {
                                Task { await model.copyRemoteInvokeSshKey() }
                            }
                            .buttonStyle(.borderless)
                            .disabled(model.remoteInvokeSshKey == nil || model.isMutating)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)

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
                    .frame(maxWidth: .infinity, alignment: .leading)
                }

                if let calls = model.remoteInvokeCalls?.calls, !calls.isEmpty {
                    Divider()
                    HStack(spacing: 12) {
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

    private var copiedTitle: String {
        guard let copiedAt = model.copiedSshKeyAt,
              Date().timeIntervalSince(copiedAt) < 3 else {
            return "复制 SSH Key"
        }
        return "已复制"
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
                        subtitle: "按应用、域名和 IP 控制解包范围"
                    )
                    Spacer()
                    StatusPill(
                        title: appModel.tlsConfig?.enableTlsInterception == true ? "已开启" : "已关闭",
                        color: appModel.tlsConfig?.enableTlsInterception == true ? .green : .secondary
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
                    GridItem(.flexible(), spacing: 10),
                    GridItem(.flexible(), spacing: 10),
                    GridItem(.flexible(), spacing: 10),
                ], spacing: 10) {
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
                    .background(.white, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
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
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    NativeCardHeader(
                        title: "同步",
                        subtitle: model.syncStatus?.user?.email ?? model.syncStatus?.remoteBaseURL ?? "读取中"
                    )
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { model.syncStatus?.enabled ?? false },
                        set: { enabled in Task { await model.setSyncEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.syncStatus == nil || model.isMutating)
                }
                HStack(spacing: 10) {
                    StatusPill(
                        title: model.syncStatus?.authorized == true ? "已授权" : "未授权",
                        color: model.syncStatus?.authorized == true ? .green : .orange
                    )
                    Toggle("自动同步", isOn: Binding(
                        get: { model.syncStatus?.autoSync ?? false },
                        set: { enabled in Task { await model.setAutoSyncEnabled(enabled) } }
                    ))
                    .toggleStyle(.switch)
                    .font(.system(size: 12))
                    .disabled(model.syncStatus == nil || model.isMutating)
                    Spacer()
                    Button(model.syncStatus?.hasSession == true ? "退出" : "登录") {
                        Task {
                            if model.syncStatus?.hasSession == true {
                                await model.logoutSync()
                            } else {
                                await model.openSyncLogin()
                            }
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
    }
}

private struct CertificateControlCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "证书与移动端",
                        subtitle: model.certInfo?.statusMessage ?? "CA 状态、移动设备与连接可用性"
                    )
                    Spacer()
                    StatusPill(
                        title: model.certInfo?.trusted == true ? "已信任" : "未信任",
                        color: model.certInfo?.trusted == true ? .green : .orange
                    )
                }

                HStack(alignment: .top, spacing: 18) {
                    VStack(alignment: .leading, spacing: 12) {
                        HStack(spacing: 12) {
                            CompactFact(title: "本机 CA", value: model.certInfo?.statusLabel ?? "读取中")
                            CompactFact(title: "代理地址", value: model.proxyAddressInfo?.addresses.first(where: \.isPreferred)?.address ?? "-")
                            CompactFact(title: "移动设备", value: "\(model.detectedMobileDevices.count)")
                        }
                        Text(fingerprintText)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                        HStack(spacing: 8) {
                            Button("安装本机 CA") {
                                Task { await model.installCertificate() }
                            }
                            .buttonStyle(.bordered)
                            .disabled(model.certInfo == nil || model.certInfo?.available == false || model.isMutating)
                            Button("刷新设备") {
                                Task { await model.refreshMobileDevices() }
                            }
                            .buttonStyle(.borderless)
                            .disabled(model.isMutating)
                            Button("重新生成 QR") {
                                Task { await model.regenerateTrustProbe() }
                            }
                            .buttonStyle(.borderless)
                            .disabled(model.preferredTrustProbeHost == nil || model.isMutating)
                        }

                        VStack(alignment: .leading, spacing: 8) {
                            Text("已连接设备")
                                .font(.system(size: 13, weight: .semibold))
                            if model.detectedMobileDevices.isEmpty {
                                Text("暂无 USB 设备。可让手机扫描右侧二维码检查同网、证书与代理连接。")
                                    .font(.system(size: 12))
                                    .foregroundStyle(.secondary)
                            } else {
                                ForEach(model.detectedMobileDevices.prefix(3)) { device in
                                    MobileDeviceRow(device: device)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .topLeading)

                    AvailabilityProbePanel(model: model)
                        .frame(width: 260, alignment: .topLeading)
                }
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

private struct AvailabilityProbePanel: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("可用性检查")
                    .font(.system(size: 13, weight: .semibold))
                Spacer()
                StatusPill(title: probeStatusTitle, color: probeStatusColor)
            }
            QRPreview(urlString: model.trustProbeSession?.qrCodeURL)
            Text(model.trustProbeSession?.landingURL ?? "等待生成检查链接")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(2)
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
            }
            if let devices = model.trustProbeSession?.devices, !devices.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(devices.prefix(3)) { device in
                        TrustProbeDeviceRow(device: device)
                    }
                }
            } else {
                Text("扫码后会在这里显示正在连接的设备。")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(14)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
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

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(.white)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(AppSurface.cardBorder)
                )
            if let value = urlString, let url = URL(string: value) {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image
                            .resizable()
                            .interpolation(.none)
                            .scaledToFit()
                            .padding(10)
                    case .failure:
                        Image(systemName: "qrcode")
                            .font(.system(size: 42, weight: .regular))
                            .foregroundStyle(.tertiary)
                    default:
                        ProgressView()
                    }
                }
            } else {
                Image(systemName: "qrcode")
                    .font(.system(size: 42, weight: .regular))
                    .foregroundStyle(.tertiary)
            }
        }
        .frame(width: 136, height: 136)
    }
}

private struct TrustProbeDeviceRow: View {
    let device: TrustProbeDevice

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(device.tlsTrusted ? Color.green : (device.networkReachable ? Color.blue : Color.orange))
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(device.platformHint ?? device.clientIP ?? device.deviceID)
                    .font(.system(size: 11, weight: .medium))
                    .lineLimit(1)
                Text(device.proxyConfigurationMessage ?? device.lastError ?? device.status)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }
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

private struct ActivityBars: View {
    let rows: [(String, Int)]

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
    let formatter = ByteCountFormatter()
    formatter.allowedUnits = [.useBytes, .useKB, .useMB, .useGB]
    formatter.countStyle = .file
    return formatter.string(fromByteCount: Int64(max(value, 0)))
}
