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
                    caption: "\(appModel.clientAppCounts.count) 个应用 · \(appModel.clientIpCounts.count) 个 IP",
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
                    ActivityBars(rows: appModel.clientAppCounts.prefix(6).map { ($0.name, $0.count) })
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

                OverviewToggleCard(
                    title: "TLS 解密",
                    subtitle: "证书信任后可解密 HTTPS 流量",
                    status: appModel.tlsConfig?.enableTlsInterception == true ? "已开启" : "已关闭",
                    tint: appModel.tlsConfig?.enableTlsInterception == true ? .green : .secondary,
                    isOn: appModel.tlsConfig?.enableTlsInterception ?? false,
                    isDisabled: appModel.tlsConfig == nil || appModel.isTogglingTls
                ) { enabled in
                    Task { await appModel.setTlsInterceptionEnabled(enabled) }
                }

                RemoteInvokeCard(model: model)
                SyncControlCard(model: model)
                CertificateControlCard(model: model)

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
                        CompactFact(title: "规则命中", value: "\(appModel.trafficRecords.filter(\.hasRuleHit).count)")
                    }
                }
            }
        }
    }
}

@MainActor
private final class OverviewControlModel: ObservableObject {
    @Published var certInfo: CertInfo?
    @Published var syncStatus: SyncStatus?
    @Published var remoteInvokeStatus: RemoteInvokeStatus?
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
            async let sync = client.fetchSyncStatus()
            async let remote = client.fetchRemoteInvokeStatus()
            certInfo = try await cert
            syncStatus = try await sync
            remoteInvokeStatus = try await remote
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
            self.certInfo = try await BifrostClient(baseURL: self.baseURL).installLocalCA()
        }
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
        OverviewToggleCard(
            title: "远程调用",
            subtitle: "配对 \(model.remoteInvokeStatus?.pendingPairingsCount ?? 0) · 调用 \(model.remoteInvokeStatus?.activeCallIDs.count ?? 0)",
            status: model.remoteInvokeStatus?.discoverySession == nil ? (model.remoteInvokeStatus?.state ?? "unknown") : "发现中",
            tint: model.remoteInvokeStatus?.discoverySession == nil ? .secondary : .green,
            isOn: model.remoteInvokeStatus?.discoverySession != nil,
            isDisabled: model.isMutating || model.remoteInvokeStatus == nil
        ) { enabled in
            Task { await model.setRemoteDiscoveryEnabled(enabled) }
        }
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
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    NativeCardHeader(
                        title: "证书",
                        subtitle: model.certInfo?.statusMessage ?? "读取中"
                    )
                    Spacer()
                    StatusPill(
                        title: model.certInfo?.trusted == true ? "已信任" : "未信任",
                        color: model.certInfo?.trusted == true ? .green : .orange
                    )
                }
                HStack {
                    Text(model.certInfo?.sha256Fingerprint?.prefix(18).description ?? "-")
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("安装本机 CA") {
                        Task { await model.installCertificate() }
                    }
                    .buttonStyle(.borderless)
                    .disabled(model.certInfo == nil || model.certInfo?.available == false || model.isMutating)
                }
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

private struct NativePageScaffold<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                Text(title)
                    .font(.system(size: 30, weight: .bold))
                    .padding(.top, 20)
                content
            }
            .padding(.horizontal, 36)
            .padding(.bottom, 36)
            .frame(maxWidth: 1180, alignment: .leading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(AppSurface.content)
    }
}

private struct NativeCard<Content: View>: View {
    @ViewBuilder var content: Content
    @State private var isHovering = false

    var body: some View {
        content
            .padding(18)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(AppSurface.card)
            }
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(AppSurface.cardBorder)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(
                        LinearGradient(
                            colors: [
                                AppSurface.cardHighlight,
                                AppSurface.cardHighlight.opacity(0.45),
                                AppSurface.cardBorder,
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        ),
                        lineWidth: 1
                    )
            )
            .shadow(color: AppSurface.cardGlow, radius: isHovering ? 22 : 12, x: 0, y: 0)
            .shadow(color: isHovering ? AppSurface.hoverShadow : AppSurface.cardShadow, radius: isHovering ? 18 : 10, x: 0, y: isHovering ? 10 : 5)
            .scaleEffect(isHovering ? 1.004 : 1)
            .animation(.easeOut(duration: 0.16), value: isHovering)
            .onHover { isHovering = $0 }
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

private struct NativeCardHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .font(.system(size: 15, weight: .semibold))
            Text(subtitle)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
    }
}

private struct CompactFact: View {
    let title: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 15, weight: .semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.7)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
    }
}

private struct StatusPill: View {
    let title: String
    let color: Color

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            Text(title)
                .font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(.secondary)
    }
}

private struct EmptyNativeState: View {
    let title: String

    var body: some View {
        VStack(spacing: 8) {
            Text(title)
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
