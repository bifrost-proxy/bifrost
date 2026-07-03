import AppKit
import SwiftUI

struct MainWindowScene: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var createRuleSheetVisible = false

    var body: some View {
        NavigationSplitView {
            PrimarySidebar(selection: $appModel.selectedSidebarItem)
                .navigationSplitViewColumnWidth(min: 176, ideal: 204, max: 232)
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
            .frame(minWidth: 0, maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationSplitViewStyle(.balanced)
        .frame(minWidth: 960, minHeight: 720)
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

            configure(window)
            self.window = window
        }

        private func configure(_ window: NSWindow) {
            window.title = ""
            if #available(macOS 11.0, *) {
                window.subtitle = ""
            }
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
    static let content = adaptiveColor(
        light: NSColor(calibratedRed: 0.955, green: 0.972, blue: 0.992, alpha: 1),
        dark: NSColor(calibratedRed: 0.070, green: 0.086, blue: 0.105, alpha: 1)
    )
    static let sidebar = adaptiveColor(
        light: NSColor(calibratedRed: 0.925, green: 0.950, blue: 0.972, alpha: 1),
        dark: NSColor(calibratedRed: 0.090, green: 0.108, blue: 0.132, alpha: 1)
    )
    static let sidebarSelection = adaptiveColor(
        light: NSColor(calibratedRed: 0.820, green: 0.860, blue: 0.900, alpha: 0.58),
        dark: NSColor(calibratedRed: 0.300, green: 0.380, blue: 0.470, alpha: 0.50)
    )
    static let card = adaptiveColor(
        light: NSColor.white,
        dark: NSColor(calibratedRed: 0.118, green: 0.137, blue: 0.165, alpha: 1)
    )
    static let cardBorder = adaptiveColor(
        light: NSColor(calibratedRed: 0.780, green: 0.840, blue: 0.900, alpha: 0.28),
        dark: NSColor(calibratedRed: 0.430, green: 0.500, blue: 0.600, alpha: 0.32)
    )
    static let cardHighlight = adaptiveColor(
        light: NSColor(calibratedWhite: 1.0, alpha: 0.95),
        dark: NSColor(calibratedRed: 0.320, green: 0.390, blue: 0.480, alpha: 0.34)
    )
    static let cardGlow = adaptiveColor(
        light: NSColor(calibratedRed: 0.620, green: 0.720, blue: 0.880, alpha: 0.20),
        dark: NSColor(calibratedRed: 0.180, green: 0.390, blue: 0.640, alpha: 0.24)
    )
    static let cardShadow = adaptiveColor(
        light: NSColor(calibratedWhite: 0.0, alpha: 0.040),
        dark: NSColor(calibratedWhite: 0.0, alpha: 0.30)
    )
    static let subtleFill = adaptiveColor(
        light: NSColor(calibratedRed: 0.720, green: 0.770, blue: 0.830, alpha: 0.18),
        dark: NSColor(calibratedRed: 0.500, green: 0.580, blue: 0.680, alpha: 0.16)
    )
    static let hoverShadow = adaptiveColor(
        light: NSColor(calibratedWhite: 0.0, alpha: 0.065),
        dark: NSColor(calibratedWhite: 0.0, alpha: 0.42)
    )

    static func resolvedContentColor(for appearance: NSAppearance.Name) -> NSColor {
        resolvedColor(light: NSColor(calibratedRed: 0.955, green: 0.972, blue: 0.992, alpha: 1), dark: NSColor(calibratedRed: 0.070, green: 0.086, blue: 0.105, alpha: 1), appearance: appearance)
    }

    private static func adaptiveColor(light: NSColor, dark: NSColor) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            resolvedColor(light: light, dark: dark, appearance: appearance.name)
        })
    }

    private static func resolvedColor(light: NSColor, dark: NSColor, appearance: NSAppearance.Name) -> NSColor {
        let resolved = NSAppearance(named: appearance)?
            .bestMatch(from: [.darkAqua, .aqua, .vibrantDark, .vibrantLight])
        return (resolved == .darkAqua || resolved == .vibrantDark) ? dark : light
    }
}

private struct StatusBar: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        HStack(spacing: 8) {
            StatusBarItem(color: statusColor, text: proxyText, width: 116)
            StatusBarItem(color: syncColor, text: syncText, width: 112)
            StatusBarItem(color: tlsColor, text: tlsText, width: 68)

            Divider()
                .frame(height: 14)

            StatusBarMetric(text: "↑ \(formatRate(appModel.overview?.metrics?.bytesSentRate))", width: 78)
            StatusBarMetric(text: "↓ \(formatRate(appModel.overview?.metrics?.bytesReceivedRate))", width: 78)
            StatusBarMetric(text: "Total: \(formatBytes(totalBytes))", width: 94)
            StatusBarMetric(text: "Conn: \(appModel.overview?.metrics?.activeConnections ?? 0)", width: 58)
            StatusBarMetric(text: "Req: \(appModel.overview?.metrics?.totalRequests ?? 0)", width: 74)
            StatusBarMetric(text: "Mem: \(formatBytes(appModel.overview?.metrics?.memoryUsed))", width: 96)
            StatusBarMetric(text: "CPU: \(cpuText)", width: 70)
            StatusBarMetric(text: "Uptime: \(uptimeText)", width: 92)

            Spacer()

            StatusBarMetric(text: "v\(appModel.overview?.system?.version ?? "-")", width: 62, alignment: .trailing)
            StatusBarMetric(text: "Skill", width: 28, alignment: .trailing)
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
    let width: CGFloat

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
            Text(text)
                .lineLimit(1)
                .truncationMode(.tail)
                .monospacedDigit()
        }
        .frame(width: width, alignment: .leading)
    }
}

private struct StatusBarMetric: View {
    let text: String
    let width: CGFloat
    var alignment: Alignment = .leading

    var body: some View {
        Text(text)
            .lineLimit(1)
            .truncationMode(.tail)
            .monospacedDigit()
            .frame(width: width, alignment: alignment)
    }
}
