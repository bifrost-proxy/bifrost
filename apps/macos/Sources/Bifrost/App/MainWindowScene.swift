import AppKit
import SwiftUI

struct MainWindowScene: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        HStack(spacing: 0) {
            Sidebar(selection: $appModel.selectedSidebarItem)

            Divider()

            VStack(spacing: 0) {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                Divider()

                StatusBar()
            }
            .padding(.top, 36)
        }
        .frame(minWidth: 1180, minHeight: 760)
        .ignoresSafeArea(.container, edges: .top)
        .preferredColorScheme(appModel.colorSchemeMode.colorScheme)
        .background(WindowTitlebarConfigurator())
        .task {
            await appModel.ensureService()
        }
    }

    @ViewBuilder
    private var content: some View {
        switch appModel.selectedSidebarItem {
        case .network:
            TrafficView()
        case .replay:
            APIBackedFeatureView(
                title: "Replay",
                systemImage: "bolt",
                endpoints: [
                    FeatureEndpoint(title: "Stats", path: "/replay/stats"),
                    FeatureEndpoint(title: "Groups", path: "/replay/groups"),
                    FeatureEndpoint(title: "Saved Requests", path: "/replay/requests", queryItems: [URLQueryItem(name: "limit", value: "20")]),
                    FeatureEndpoint(title: "History", path: "/replay/history", queryItems: [URLQueryItem(name: "limit", value: "20")]),
                ]
            )
        case .rules:
            RulesView()
        case .values:
            ValuesView()
        case .scripts:
            ScriptsView()
        case .ai:
            APIBackedFeatureView(
                title: "AI",
                systemImage: "face.smiling",
                endpoints: [
                    FeatureEndpoint(title: "IM Providers", path: "/im-gateway/providers"),
                    FeatureEndpoint(title: "External CLI Config", path: "/im-gateway/external-cli/config"),
                    FeatureEndpoint(title: "ASR Capabilities", path: "/asr/capabilities"),
                ]
            )
        case .devTools:
            APIBackedFeatureView(
                title: "DevTools",
                systemImage: "ladybug",
                endpoints: [
                    FeatureEndpoint(title: "Pages", path: "/devtools/pages", queryItems: [URLQueryItem(name: "online", value: "true")]),
                ]
            )
        case .groups:
            APIBackedFeatureView(
                title: "Groups",
                systemImage: "person.2.badge.gearshape",
                endpoints: [
                    FeatureEndpoint(title: "Groups", path: "/group", queryItems: [URLQueryItem(name: "offset", value: "0"), URLQueryItem(name: "limit", value: "50")]),
                ]
            )
        case .notify:
            APIBackedFeatureView(
                title: "Notify",
                systemImage: "bell",
                endpoints: [
                    FeatureEndpoint(title: "Notifications", path: "/notifications", queryItems: [URLQueryItem(name: "limit", value: "20")]),
                    FeatureEndpoint(title: "Unread Count", path: "/notifications/unread-count"),
                    FeatureEndpoint(title: "Client Trust", path: "/notifications/client-trust"),
                ]
            )
        case .settings:
            SettingsView()
        }
    }
}

private struct WindowTitlebarConfigurator: NSViewRepresentable {
    @EnvironmentObject private var appModel: AppModel

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async {
            context.coordinator.attach(to: view, appModel: appModel)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            context.coordinator.attach(to: nsView, appModel: appModel)
            context.coordinator.update(appModel: appModel)
        }
    }

    @MainActor
    final class Coordinator {
        private let sidebarWidth: CGFloat = 72
        private let titlebarRailBackgroundIdentifier = NSUserInterfaceItemIdentifier("BifrostTitlebarRailBackground")
        private weak var window: NSWindow?
        private var accessory: NSTitlebarAccessoryViewController?
        private var hostingView: NSHostingView<AnyView>?

        func attach(to markerView: NSView, appModel: AppModel) {
            guard let window = markerView.window else {
                return
            }

            if self.window !== window {
                if let existing = accessory,
                   let index = window.titlebarAccessoryViewControllers.firstIndex(of: existing) {
                    window.removeTitlebarAccessoryViewController(at: index)
                }
                configure(window)
                let hostingView = NSHostingView(rootView: AnyView(TopToolbar().environmentObject(appModel)))
                hostingView.frame = NSRect(x: 0, y: 0, width: max(window.frame.width - sidebarWidth, 820), height: 36)
                hostingView.autoresizingMask = [.width]

                let accessory = NSTitlebarAccessoryViewController()
                accessory.layoutAttribute = .right
                accessory.view = hostingView
                window.addTitlebarAccessoryViewController(accessory)

                self.window = window
                self.accessory = accessory
                self.hostingView = hostingView
            }

            update(appModel: appModel)
        }

        func update(appModel: AppModel) {
            guard let window else {
                return
            }
            hostingView?.frame.size = NSSize(width: max(window.frame.width - sidebarWidth, 820), height: 36)
            hostingView?.rootView = AnyView(TopToolbar().environmentObject(appModel))
            hideSystemWindowButtons(in: window)
        }

        private func configure(_ window: NSWindow) {
            window.title = "Bifrost"
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.styleMask.insert(.fullSizeContentView)
            if #available(macOS 11.0, *) {
                window.titlebarSeparatorStyle = .none
            }
            window.isReleasedWhenClosed = false
            hideSystemWindowButtons(in: window)
        }

        private func hideSystemWindowButtons(in window: NSWindow) {
            hideSystemWindowButtonsOnce(in: window)
            DispatchQueue.main.async { [weak window] in
                guard let window else {
                    return
                }
                self.hideSystemWindowButtonsOnce(in: window)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak window] in
                guard let window else {
                    return
                }
                self.hideSystemWindowButtonsOnce(in: window)
            }
        }

        private func hideSystemWindowButtonsOnce(in window: NSWindow) {
            let buttons: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
            for type in buttons {
                guard let button = window.standardWindowButton(type) else {
                    continue
                }
                button.isHidden = true
                button.alphaValue = 0
                button.isEnabled = false
            }
        }
    }
}

private struct TopToolbar: View {
    @EnvironmentObject private var appModel: AppModel

    private let ruleFilters = ["Hit Rule"]
    private let protocolFilters = ["HTTP", "HTTPS", "WS", "WSS", "H3"]
    private let typeFilters = ["JSON", "Form", "XML", "JS", "CSS", "Font", "Doc", "Media", "SSE"]
    private let statusFilters = ["1xx", "2xx", "3xx", "4xx", "5xx", "error"]

    var body: some View {
        HStack(spacing: 10) {
            if appModel.selectedSidebarItem == .network {
                networkToolbar
            } else {
                sectionToolbar
            }
        }
        .frame(height: 32)
        .padding(.leading, 4)
        .padding(.trailing, 10)
        .background(.bar)
    }

    private var networkToolbar: some View {
        HStack(spacing: 8) {
            ToolbarIconButton(
                systemImage: "line.3.horizontal.decrease",
                isActive: !appModel.isFilterPanelCollapsed,
                help: appModel.isFilterPanelCollapsed ? "Show filter panel" : "Hide filter panel"
            ) {
                withAnimation(.easeInOut(duration: 0.16)) {
                    appModel.isFilterPanelCollapsed.toggle()
                }
            }

            Divider()
                .frame(height: 16)

            ToolbarIconButton(systemImage: "trash", help: "Clear all traffic") {
                Task {
                    await appModel.clearTraffic()
                }
            }

            Divider()
                .frame(height: 16)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 5) {
                    filterTags(ruleFilters, group: .rule)
                    verticalSeparator
                    filterTags(protocolFilters, group: .networkProtocol)
                    verticalSeparator
                    filterTags(typeFilters, group: .type)
                    verticalSeparator
                    filterTags(statusFilters, group: .status)
                    verticalSeparator
                    filterTags(["Imported"], group: .imported)
                }
            }
            .frame(maxWidth: 470)

            Button {
                appModel.isFilterPanelCollapsed = false
            } label: {
                Label("Add Filter", systemImage: "plus")
            }
            .buttonStyle(.borderless)
            .font(.system(size: 12))

            Button {
                appModel.isNetworkSearchVisible.toggle()
            } label: {
                Label("Fuzzy Search", systemImage: "magnifyingglass")
            }
            .buttonStyle(.borderless)
            .font(.system(size: 12))

            if appModel.isNetworkSearchVisible {
                TextField("Search host, path, method...", text: $appModel.networkSearchText)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 11))
                    .frame(width: 190)
            }

            Spacer(minLength: 8)

            CompactToolbarToggle(
                title: "Breakpoint",
                isOn: appModel.breakpointSettings?.enabled ?? false,
                isDisabled: appModel.breakpointSettings == nil || appModel.isTogglingBreakpoint,
                help: "Toggle the global breakpoint gate"
            ) { enabled in
                Task {
                    await appModel.setBreakpointEnabled(enabled)
                }
            }

            CompactToolbarToggle(
                title: "TLS Decode",
                isOn: appModel.tlsConfig?.enableTlsInterception ?? false,
                isDisabled: appModel.tlsConfig == nil || appModel.isTogglingTls,
                help: "Toggle TLS interception"
            ) { enabled in
                Task {
                    await appModel.setTlsInterceptionEnabled(enabled)
                }
            }

            CompactToolbarToggle(
                title: "System Proxy",
                isOn: appModel.systemProxyStatus?.enabled ?? false,
                isDisabled: !(appModel.systemProxyStatus?.supported ?? false) || appModel.isTogglingSystemProxy,
                help: systemProxyHelp
            ) { enabled in
                Task {
                    await appModel.setSystemProxyEnabled(enabled)
                }
            }

            ToolbarIconButton(
                systemImage: appModel.isDetailPanelCollapsed ? "sidebar.right" : "sidebar.leading",
                help: appModel.isDetailPanelCollapsed ? "Show detail panel" : "Hide detail panel"
            ) {
                withAnimation(.easeInOut(duration: 0.16)) {
                    appModel.isDetailPanelCollapsed.toggle()
                }
            }
        }
    }

    private var sectionToolbar: some View {
        HStack(spacing: 8) {
            Image(systemName: appModel.selectedSidebarItem.systemImage)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            Text(appModel.selectedSidebarItem.rawValue)
                .font(.system(size: 13, weight: .semibold))
            Text(sectionSubtitle)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Spacer()
            Button {
                Task {
                    await appModel.refreshData()
                }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
        }
    }

    private var sectionSubtitle: String {
        switch appModel.selectedSidebarItem {
        case .rules:
            return "\(appModel.rules.filter(\.enabled).count)/\(appModel.rules.count) enabled"
        case .values:
            return "\(appModel.values.count) values"
        case .scripts:
            let count = appModel.scriptsByType[appModel.selectedScriptType]?.count ?? 0
            return "\(appModel.selectedScriptType.label) · \(count) scripts"
        case .settings:
            return appModel.adminHostPortLabel
        case .network:
            return "\(appModel.displayedTrafficRecords.count) records"
        default:
            return "API status"
        }
    }

    private func filterTags(_ tags: [String], group: NetworkToolbarFilterGroup) -> some View {
        ForEach(tags, id: \.self) { tag in
            let isSelected = appModel.networkToolbarFilters.selectedTag(for: group) == tag
            Button {
                appModel.toggleNetworkToolbarFilter(group: group, tag: tag)
            } label: {
                Text(tag)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                    .lineLimit(1)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(isSelected ? Color.accentColor.opacity(0.14) : Color.clear, in: RoundedRectangle(cornerRadius: 4))
            }
            .buttonStyle(.plain)
            .help("Filter \(tag)")
        }
    }

    private var verticalSeparator: some View {
        Rectangle()
            .fill(Color.secondary.opacity(0.24))
            .frame(width: 1, height: 14)
    }

    private var systemProxyHelp: String {
        guard let status = appModel.systemProxyStatus else {
            return "System proxy status is loading"
        }
        guard status.supported else {
            return "System proxy is not supported on this platform"
        }
        return "Toggle macOS system proxy for \(appModel.adminHostPortLabel)"
    }
}

private struct ToolbarIconButton: View {
    let systemImage: String
    var isActive = false
    let help: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 13, weight: .medium))
                .frame(width: 24, height: 24)
                .foregroundStyle(isActive ? Color.accentColor : Color.secondary)
                .background(isActive ? Color.accentColor.opacity(0.12) : Color.clear)
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

private struct CompactToolbarToggle: View {
    let title: String
    let isOn: Bool
    let isDisabled: Bool
    let help: String
    let action: (Bool) -> Void

    var body: some View {
        HStack(spacing: 5) {
            Text(title)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)

            Toggle("", isOn: Binding(
                get: { isOn },
                set: action
            ))
            .labelsHidden()
            .toggleStyle(.switch)
            .controlSize(.mini)
            .disabled(isDisabled)
        }
        .frame(height: 22)
        .opacity(isDisabled ? 0.48 : 1)
        .help(help)
        .animation(.easeInOut(duration: 0.12), value: isOn)
    }
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
