import AppKit
import SwiftUI

struct MainWindowScene: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var createRuleSheetVisible = false

    var body: some View {
        NavigationSplitView {
            PrimarySidebar(selection: $appModel.selectedSidebarItem)
                .navigationSplitViewColumnWidth(min: 188, ideal: 218, max: 252)
        } detail: {
            VStack(spacing: 0) {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                Divider()

                StatusBar()
            }
            .background {
                AppSurface.content
            }
        }
        .navigationSplitViewStyle(.balanced)
        .frame(minWidth: 1180, minHeight: 760)
        .preferredColorScheme(appModel.colorSchemeMode.colorScheme)
        .background(WindowChromeConfigurator())
        .sheet(isPresented: $createRuleSheetVisible) {
            NameEntrySheet(
                title: "New Rule",
                prompt: "Rule name",
                initialValue: "",
                confirmTitle: "Create"
            ) { name in
                Task { await appModel.createRule(name: name) }
            }
        }
        .task {
            await appModel.ensureService()
        }
        .onChange(of: appModel.selectedSidebarItem) { _ in
            Task {
                await appModel.handleSidebarSelectionChanged()
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch appModel.selectedSidebarItem {
        case .activity:
            ActivityView()
        case .overview:
            DashboardView()
        case .rules:
            RulesView()
        case .network:
            NetworkWebView()
        }
    }
}

private struct WindowChromeConfigurator: NSViewRepresentable {
    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async {
            context.coordinator.attach(to: view)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            context.coordinator.attach(to: nsView)
        }
    }

    @MainActor
    final class Coordinator {
        private weak var window: NSWindow?

        func attach(to markerView: NSView) {
            guard let window = markerView.window else {
                return
            }

            if self.window !== window {
                configure(window)
                self.window = window
            }
        }

        private func configure(_ window: NSWindow) {
            window.title = ""
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.isOpaque = false
            window.backgroundColor = .clear
            window.hasShadow = true
            window.isMovableByWindowBackground = true
            window.styleMask.insert(.fullSizeContentView)
            if #available(macOS 11.0, *) {
                window.titlebarSeparatorStyle = .none
            }
            window.isReleasedWhenClosed = false
        }
    }
}

enum AppSurface {
    static let content = Color(red: 0.955, green: 0.972, blue: 0.992)
    static let sidebar = Color(red: 0.925, green: 0.95, blue: 0.972)
    static let sidebarSelection = Color(red: 0.82, green: 0.86, blue: 0.90).opacity(0.58)
    static let card = Color.white
    static let cardBorder = Color(red: 0.78, green: 0.84, blue: 0.90).opacity(0.28)
    static let cardHighlight = Color.white.opacity(0.95)
    static let cardGlow = Color(red: 0.62, green: 0.72, blue: 0.88).opacity(0.20)
    static let cardShadow = Color.black.opacity(0.040)
    static let subtleFill = Color(red: 0.72, green: 0.77, blue: 0.83).opacity(0.18)
    static let hoverShadow = Color.black.opacity(0.065)
}

private struct StatusBar: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        HStack(spacing: 14) {
            StatusBarItem(color: statusColor, text: proxyText)
            StatusBarItem(color: syncColor, text: syncText)
            StatusBarItem(color: tlsColor, text: tlsText)

            Divider()
                .frame(height: 14)

            Text("↑ \(formatRate(appModel.overview?.metrics?.bytesSentRate))")
            Text("↓ \(formatRate(appModel.overview?.metrics?.bytesReceivedRate))")
            Text("Total: \(formatBytes(totalBytes))")
            Text("Conn: \(appModel.overview?.metrics?.activeConnections ?? 0)")
            Text("Req: \(appModel.overview?.metrics?.totalRequests ?? 0)")
            Text("Mem: \(formatBytes(appModel.overview?.metrics?.memoryUsed))")
            Text("CPU: \(cpuText)")
            Text("Uptime: \(uptimeText)")

            Spacer()

            Text("v\(appModel.overview?.system?.version ?? "-")")
            Text("Skill")
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .frame(height: 22)
        .padding(.horizontal, 8)
        .background(.bar)
    }

    private var statusColor: Color {
        if case .running = appModel.sidecarState {
            return .green
        }
        if case .failed = appModel.sidecarState {
            return .red
        }
        return .orange
    }

    private var syncColor: Color {
        if appModel.realtimeState.isConnected {
            return .green
        }
        if case .failed = appModel.realtimeState {
            return .orange
        }
        return .secondary
    }

    private var tlsColor: Color {
        (appModel.tlsConfig?.enableTlsInterception ?? false) ? .green : .secondary
    }

    private var proxyText: String {
        switch appModel.sidecarState {
        case .running:
            return "Proxy: Running"
        case .starting:
            return "Proxy: Starting"
        case .stopped:
            return "Proxy: Stopped"
        case .failed:
            return "Proxy: Failed"
        case .recovering:
            return "Proxy: Recovering"
        }
    }

    private var tlsText: String {
        if appModel.tlsConfig?.enableTlsInterception == true {
            return "TLS: Scoped"
        }
        return "TLS: Off"
    }

    private var syncText: String {
        if appModel.realtimeFallbackActive {
            return "\(appModel.realtimeState.label) + Poll"
        }
        if let clientId = appModel.realtimeClientId,
           appModel.realtimeState.isConnected {
            return "\(appModel.realtimeState.label) #\(clientId)"
        }
        return appModel.realtimeState.label
    }

    private var qpsText: String {
        guard let qps = appModel.overview?.metrics?.qps else {
            return "0.00"
        }
        return String(format: "%.2f", qps)
    }

    private var totalBytes: Int? {
        guard let sent = appModel.overview?.metrics?.bytesSent,
              let received = appModel.overview?.metrics?.bytesReceived else {
            return nil
        }
        return sent + received
    }

    private var cpuText: String {
        guard let cpu = appModel.overview?.metrics?.cpuUsage else {
            return "-"
        }
        return String(format: "%.1f%%", cpu)
    }

    private var uptimeText: String {
        guard let seconds = appModel.overview?.system?.uptimeSecs else {
            return "-"
        }
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        let hours = seconds / 3600
        let minutes = (seconds % 3600) / 60
        return minutes > 0 ? "\(hours)h \(minutes)m" : "\(hours)h"
    }

    private func formatRate(_ bytesPerSecond: Double?) -> String {
        guard let bytesPerSecond else {
            return "0 B/s"
        }
        return "\(formatBytes(Int(bytesPerSecond)))/s"
    }

    private func formatBytes(_ bytes: Int?) -> String {
        guard let bytes else {
            return "-"
        }
        if bytes < 1024 {
            return "\(bytes) B"
        }
        let units = ["KB", "MB", "GB", "TB"]
        var value = Double(bytes) / 1024
        var unitIndex = 0
        while value >= 1024, unitIndex < units.count - 1 {
            value /= 1024
            unitIndex += 1
        }
        return String(format: "%.1f %@", value, units[unitIndex])
    }
}

private struct StatusBarItem: View {
    let color: Color
    let text: String

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
            Text(text)
        }
        .fixedSize(horizontal: true, vertical: false)
    }
}
