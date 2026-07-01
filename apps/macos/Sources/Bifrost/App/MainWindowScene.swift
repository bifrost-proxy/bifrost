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
                if appModel.selectedSidebarItem != .settings {
                    TopToolbar {
                        createRuleSheetVisible = true
                    }

                    Divider()
                }

                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                Divider()

                StatusBar()
            }
            .background(Color(nsColor: .windowBackgroundColor))
        }
        .navigationSplitViewStyle(.balanced)
        .frame(minWidth: 1180, minHeight: 760)
        .ignoresSafeArea(.container, edges: .top)
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
                await appModel.refreshData()
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch appModel.selectedSidebarItem {
        case .network:
            TrafficView()
        case .rules:
            RulesView()
        case .settings:
            SettingsView()
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
            window.title = "Bifrost"
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.styleMask.insert(.fullSizeContentView)
            if #available(macOS 11.0, *) {
                window.titlebarSeparatorStyle = .none
            }
            window.isReleasedWhenClosed = false
        }
    }
}

private struct TopToolbar: View {
    @EnvironmentObject private var appModel: AppModel
    let createRule: () -> Void

    private let ruleFilters = ["Hit Rule"]
    private let protocolFilters = ["HTTP", "HTTPS", "WS", "WSS", "H3"]
    private let typeFilters = ["JSON", "Form", "XML", "JS", "CSS", "Font", "Doc", "Media", "SSE"]
    private let statusFilters = ["1xx", "2xx", "3xx", "4xx", "5xx", "error"]

    var body: some View {
        HStack(spacing: 10) {
            switch appModel.selectedSidebarItem {
            case .network:
                networkToolbar
            case .rules:
                rulesToolbar
            case .settings:
                EmptyView()
            }
        }
        .frame(height: 42)
        .padding(.horizontal, 12)
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

            Spacer(minLength: 8)

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

    private var rulesToolbar: some View {
        HStack(spacing: 8) {
            Image(systemName: SidebarItem.rules.systemImage)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            Text("Rules")
                .font(.system(size: 13, weight: .semibold))
            Text("\(appModel.rules.filter(\.enabled).count)/\(appModel.rules.count) enabled")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(.quaternary, in: Capsule())

            Spacer()

            Button(action: createRule) {
                Label("New Rule", systemImage: "plus")
            }
            .buttonStyle(.borderless)

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

            MiniToolbarSwitch(
                isOn: isOn,
                isEnabled: !isDisabled,
                action: action
            )
            .frame(width: 28, height: 16)
        }
        .frame(height: 22)
        .opacity(isDisabled ? 0.48 : 1)
        .help(help)
        .animation(.easeInOut(duration: 0.12), value: isOn)
    }
}

private struct MiniToolbarSwitch: NSViewRepresentable {
    let isOn: Bool
    let isEnabled: Bool
    let action: (Bool) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(action: action)
    }

    func makeNSView(context: Context) -> NSView {
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 28, height: 16))
        let control = NSSwitch(frame: NSRect(x: 0, y: 0, width: 46, height: 26))
        control.controlSize = .mini
        control.target = context.coordinator
        control.action = #selector(Coordinator.changed(_:))
        control.scaleUnitSquare(to: NSSize(width: 0.58, height: 0.58))
        control.frame.origin = NSPoint(x: 0, y: 1)
        container.addSubview(control)
        context.coordinator.control = control
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        context.coordinator.action = action
        guard let control = context.coordinator.control else {
            return
        }
        control.state = isOn ? .on : .off
        control.isEnabled = isEnabled
    }

    final class Coordinator: NSObject {
        weak var control: NSSwitch?
        var action: (Bool) -> Void

        init(action: @escaping (Bool) -> Void) {
            self.action = action
        }

        @objc func changed(_ sender: NSSwitch) {
            action(sender.state == .on)
        }
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
