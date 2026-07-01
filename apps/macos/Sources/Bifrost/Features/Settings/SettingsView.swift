import AppKit
import BifrostNativeCore
import SwiftUI

private enum SettingsSection: String, CaseIterable, Identifiable {
    case proxy = "Proxy"
    case certificate = "Certificate"
    case sync = "Sync"
    case remoteInvoke = "Remote Invoke"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .proxy:
            return "network"
        case .certificate:
            return "checkmark.shield"
        case .sync:
            return "arrow.triangle.2.circlepath"
        case .remoteInvoke:
            return "terminal"
        }
    }
}

struct SettingsView: View {
    @EnvironmentObject private var appModel: AppModel
    @StateObject private var model = SettingsViewModel()
    @State private var selectedSection: SettingsSection = .proxy

    var body: some View {
        HStack(spacing: 0) {
            settingsSidebar
            Divider()
            ScrollView {
                selectedContent
                    .frame(maxWidth: 980, alignment: .leading)
                    .padding(24)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(nsColor: .textBackgroundColor))
        }
        .task(id: appModel.adminURL) {
            await model.configure(baseURL: appModel.adminURL)
        }
        .task(id: selectedSection) {
            await model.refresh(section: selectedSection)
        }
        .alert("Settings Error", isPresented: Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )) {
            Button("OK") {
                model.errorMessage = nil
            }
        } message: {
            Text(model.errorMessage ?? "")
        }
        .sheet(item: $model.sshSecret) { secret in
            SshSecretSheet(secret: secret)
        }
    }

    private var settingsSidebar: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Settings")
                .font(.title2.weight(.semibold))
                .padding(.horizontal, 16)
                .padding(.top, 18)

            VStack(spacing: 2) {
                ForEach(SettingsSection.allCases) { section in
                    Button {
                        selectedSection = section
                    } label: {
                        Label(section.rawValue, systemImage: section.systemImage)
                            .font(.system(size: 13, weight: .medium))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(selectedSection == section ? Color.primary : Color.secondary)
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .fill(selectedSection == section ? Color.primary.opacity(0.08) : Color.clear)
                    )
                    .help(section.rawValue)
                }
            }
            .padding(.horizontal, 10)

            Spacer()
        }
        .frame(width: 220)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    @ViewBuilder
    private var selectedContent: some View {
        switch selectedSection {
        case .proxy:
            ProxySettingsPage(model: model)
        case .certificate:
            CertificateSettingsPage(model: model)
        case .sync:
            SyncSettingsPage(model: model)
        case .remoteInvoke:
            RemoteInvokeSettingsPage(model: model)
        }
    }
}

@MainActor
private final class SettingsViewModel: ObservableObject {
    @Published var systemProxy: SystemProxyStatus?
    @Published var systemProxyLaunchd: SystemProxyLaunchdStatus?
    @Published var cliProxy: CliProxyStatus?
    @Published var proxyAddress: ProxyAddressInfo?
    @Published var certInfo: CertInfo?
    @Published var mobileDevices: MobileDevicesResponse?
    @Published var syncStatus: SyncStatus?
    @Published var remoteInvokeStatus: RemoteInvokeStatus?
    @Published var clientIdentity: ClientIdentity?
    @Published var pendingPairings: [PairingRequest] = []
    @Published var grants: [Grant] = []
    @Published var calls: [RemoteCall] = []
    @Published var sshKey: RemoteInvokeSshKeyRecord?
    @Published var remoteBaseURLDraft = ""
    @Published var isLoading = false
    @Published var isMutating = false
    @Published var errorMessage: String?
    @Published var sshSecret: SshSecret?

    private var baseURL = URL(string: "http://127.0.0.1:9900")!

    func configure(baseURL: URL) async {
        guard self.baseURL != baseURL || systemProxy == nil else {
            return
        }
        self.baseURL = baseURL
        await refreshAll()
    }

    func refreshAll() async {
        isLoading = true
        defer { isLoading = false }
        await refreshProxy()
        await refreshCertificate()
        await refreshSync()
        await refreshRemoteInvoke()
    }

    func refresh(section: SettingsSection) async {
        switch section {
        case .proxy:
            await refreshProxy()
        case .certificate:
            await refreshCertificate()
        case .sync:
            await refreshSync()
        case .remoteInvoke:
            await refreshRemoteInvoke()
        }
    }

    func refreshProxy() async {
        await run {
            let client = try self.client()
            async let systemProxy = client.fetchSystemProxy()
            async let launchd = client.fetchSystemProxyLaunchd()
            async let cli = client.fetchCliProxy()
            async let address = client.fetchProxyAddress()
            self.systemProxy = try await systemProxy
            self.systemProxyLaunchd = try await launchd
            self.cliProxy = try await cli
            self.proxyAddress = try await address
        }
    }

    func setSystemProxy(enabled: Bool) async {
        await mutate {
            self.systemProxy = try await self.client().setSystemProxy(enabled: enabled)
            await self.refreshProxy()
        }
    }

    func setSystemProxyLaunchd(enabled: Bool) async {
        await mutate {
            self.systemProxyLaunchd = try await self.client().setSystemProxyLaunchd(enabled: enabled)
            await self.refreshProxy()
        }
    }

    func refreshCertificate() async {
        await run {
            let client = try self.client()
            async let cert = client.fetchCertInfo()
            async let devices = client.fetchMobileDevices()
            self.certInfo = try await cert
            self.mobileDevices = try await devices
        }
    }

    func installLocalCA() async {
        await mutate {
            self.certInfo = try await self.client().installLocalCA()
        }
    }

    func refreshMobileDevices() async {
        await mutate {
            self.mobileDevices = try await self.client().refreshMobileDevices()
        }
    }

    func refreshSync() async {
        await run {
            let status = try await self.client().fetchSyncStatus()
            self.syncStatus = status
            self.remoteBaseURLDraft = status.remoteBaseURL
        }
    }

    func setSyncEnabled(_ enabled: Bool) async {
        await mutate {
            self.syncStatus = try await self.client().updateSyncConfig(UpdateSyncConfigRequest(enabled: enabled))
            self.remoteBaseURLDraft = self.syncStatus?.remoteBaseURL ?? self.remoteBaseURLDraft
        }
    }

    func setAutoSync(_ enabled: Bool) async {
        await mutate {
            self.syncStatus = try await self.client().updateSyncConfig(UpdateSyncConfigRequest(autoSync: enabled))
        }
    }

    func saveRemoteBaseURL() async {
        let trimmed = remoteBaseURLDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        await mutate {
            self.syncStatus = try await self.client().updateSyncConfig(UpdateSyncConfigRequest(remoteBaseURL: trimmed))
            self.remoteBaseURLDraft = self.syncStatus?.remoteBaseURL ?? trimmed
        }
    }

    func openSyncLogin() async {
        await mutate {
            self.syncStatus = try await self.client().openSyncLogin()
        }
    }

    func logoutSync() async {
        await mutate {
            self.syncStatus = try await self.client().logoutSyncSession()
        }
    }

    func runSyncNow() async {
        await mutate {
            self.syncStatus = try await self.client().runSyncNow()
        }
    }

    func refreshRemoteInvoke() async {
        await run {
            let client = try self.client()
            async let status = client.fetchRemoteInvokeStatus()
            async let identity = client.fetchClientIdentity()
            async let pairings = client.fetchPendingPairings()
            async let grants = client.fetchRemoteInvokeGrants()
            async let calls = client.fetchRemoteInvokeCalls(limit: 50)
            self.remoteInvokeStatus = try await status
            self.clientIdentity = try await identity
            self.pendingPairings = try await pairings.pairings
            self.grants = try await grants.grants
            self.calls = try await calls.calls
            self.sshKey = try? await client.fetchRemoteInvokeSshKey()
        }
    }

    func enterDiscovery() async {
        await mutate {
            _ = try await self.client().enterDiscoveryMode()
            await self.refreshRemoteInvoke()
        }
    }

    func exitDiscovery() async {
        await mutate {
            try await self.client().exitDiscoveryMode()
            await self.refreshRemoteInvoke()
        }
    }

    func refreshPairCode() async {
        await mutate {
            _ = try await self.client().refreshPairCode()
            await self.refreshRemoteInvoke()
        }
    }

    func approvePairing(_ pairing: PairingRequest) async {
        await mutate {
            try await self.client().approvePairing(pairing.pairingID, input: PairingApprovalInput())
            await self.refreshRemoteInvoke()
        }
    }

    func rejectPairing(_ pairing: PairingRequest) async {
        await mutate {
            try await self.client().rejectPairing(pairing.pairingID)
            await self.refreshRemoteInvoke()
        }
    }

    func revokeGrant(_ grant: Grant) async {
        await mutate {
            try await self.client().revokeRemoteInvokeGrant(grant.grantID)
            await self.refreshRemoteInvoke()
        }
    }

    func clearCalls() async {
        await mutate {
            try await self.client().clearRemoteInvokeCalls()
            await self.refreshRemoteInvoke()
        }
    }

    func createSSHKey() async {
        await mutate {
            let payload = try await self.client().createRemoteInvokeSshKey(label: "Bifrost Mac")
            self.sshSecret = SshSecret(payload: payload)
            await self.refreshRemoteInvoke()
        }
    }

    func copySSHPrivateKey() async {
        await mutate {
            let payload = try await self.client().fetchRemoteInvokeSshPrivateKey()
            self.sshSecret = SshSecret(payload: payload)
        }
    }

    func resetSSHKey() async {
        await mutate {
            let payload = try await self.client().resetRemoteInvokeSshKey()
            self.sshSecret = SshSecret(payload: payload)
            await self.refreshRemoteInvoke()
        }
    }

    func revokeSSHKey() async {
        await mutate {
            try await self.client().revokeRemoteInvokeSshKey()
            await self.refreshRemoteInvoke()
        }
    }

    func publicURL(path: String, queryItems: [URLQueryItem] = []) -> URL? {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
        components?.path = path
        if !queryItems.isEmpty {
            components?.queryItems = queryItems
        }
        return components?.url
    }

    private func client() throws -> BifrostClient {
        try BifrostClient(baseURL: baseURL)
    }

    private func run(_ operation: @escaping () async throws -> Void) async {
        do {
            try await operation()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func mutate(_ operation: @escaping () async throws -> Void) async {
        isMutating = true
        defer { isMutating = false }
        await run(operation)
    }
}

private struct ProxySettingsPage: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsPageHeader(
                title: "Proxy",
                subtitle: "System proxy, shell proxy and LAN proxy addresses use the same Bifrost service."
            )

            SettingsCard("System Proxy", systemImage: "network") {
                if let status = model.systemProxy {
                    SettingsToggleRow(
                        title: "Enable System Proxy",
                        detail: systemProxyDetail(status),
                        isOn: status.enabled && status.managedByBifrost != false,
                        isDisabled: !status.supported || model.isMutating
                    ) { enabled in
                        await model.setSystemProxy(enabled: enabled)
                    }

                    Divider()
                    SettingsValueGrid(rows: [
                        ("Host", status.host ?? "-"),
                        ("Port", status.port.map(String.init) ?? "-"),
                        ("Bypass", status.bypass?.isEmpty == false ? status.bypass! : "-"),
                        ("Managed by Bifrost", status.managedByBifrost.map { $0 ? "Yes" : "No" } ?? "Unknown"),
                    ])
                } else {
                    LoadingRow()
                }
            }

            SettingsCard("Crash Recovery LaunchAgent", systemImage: "lifepreserver") {
                if let launchd = model.systemProxyLaunchd {
                    SettingsToggleRow(
                        title: "Restore Bifrost proxy after crash",
                        detail: launchd.message ?? launchd.needsUpgradeReason ?? "Install a LaunchAgent that restores managed proxy state.",
                        isOn: launchd.installed && launchd.loaded && launchd.needsUpgrade != true,
                        isDisabled: !launchd.supported || model.isMutating
                    ) { enabled in
                        await model.setSystemProxyLaunchd(enabled: enabled)
                    }
                    Divider()
                    SettingsValueGrid(rows: [
                        ("Label", launchd.label ?? "-"),
                        ("Installed Version", launchd.installedVersion ?? "-"),
                        ("Current Version", launchd.currentVersion ?? "-"),
                        ("Needs Upgrade", launchd.needsUpgrade == true ? "Yes" : "No"),
                    ])
                } else {
                    LoadingRow()
                }
            }

            SettingsCard("CLI Proxy", systemImage: "terminal") {
                if let cli = model.cliProxy {
                    StatusLine(
                        title: cli.enabled ? "Enabled" : "Disabled",
                        subtitle: cli.proxyURL ?? "-",
                        color: cli.enabled ? .green : .secondary
                    )
                    SettingsValueGrid(rows: [
                        ("Shell", cli.shell ?? "-"),
                        ("Config Files", cli.configFiles.isEmpty ? "-" : cli.configFiles.joined(separator: "\n")),
                    ])
                } else {
                    LoadingRow()
                }
            }

            SettingsCard("Proxy Addresses", systemImage: "qrcode") {
                if let address = model.proxyAddress {
                    SettingsValueGrid(rows: [
                        ("Port", String(address.port)),
                        ("Local IPs", address.localIPs.joined(separator: ", ")),
                    ])
                    Divider()
                    ForEach(address.addresses) { item in
                        HStack(spacing: 10) {
                            Image(systemName: item.isPreferred ? "star.fill" : "circle")
                                .foregroundStyle(item.isPreferred ? .yellow : .secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(item.address)
                                    .textSelection(.enabled)
                                Text(item.ip)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            CopyButton(text: item.address)
                        }
                    }
                } else {
                    LoadingRow()
                }
            }
        }
    }

    private func systemProxyDetail(_ status: SystemProxyStatus) -> String {
        if !status.supported {
            return "System proxy is not supported on this platform."
        }
        if status.enabled && status.managedByBifrost == false {
            return "The current system proxy is owned by another process. Enabling lets Bifrost take over."
        }
        return "Routes macOS HTTP and HTTPS traffic through Bifrost."
    }
}

private struct CertificateSettingsPage: View {
    @ObservedObject var model: SettingsViewModel
    @State private var confirmLocalCAInstall = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsPageHeader(
                title: "Certificate",
                subtitle: "Install and verify the Bifrost CA used for HTTPS inspection."
            )

            SettingsCard("Local CA", systemImage: "checkmark.shield") {
                if let cert = model.certInfo {
                    StatusLine(
                        title: cert.statusLabel,
                        subtitle: cert.statusMessage,
                        color: cert.trusted ? .green : (cert.installed ? .orange : .red)
                    )
                    SettingsValueGrid(rows: [
                        ("Available", cert.available ? "Yes" : "No"),
                        ("Installed", cert.installed ? "Yes" : "No"),
                        ("Trusted", cert.trusted ? "Yes" : "No"),
                        ("SHA256", cert.sha256Fingerprint ?? "-"),
                    ])
                    HStack {
                        Button {
                            confirmLocalCAInstall = true
                        } label: {
                            Label("Install Local CA", systemImage: "square.and.arrow.down")
                        }
                        .disabled(model.isMutating)

                        if let url = model.publicURL(path: "/cert") {
                            OpenURLButton(title: "Download CA", url: url)
                        }
                        if let url = model.publicURL(path: "/cert/qrcode") {
                            OpenURLButton(title: "Open QR Code", url: url)
                        }
                    }
                } else {
                    LoadingRow()
                }
            }
            .confirmationDialog(
                "Install Bifrost local CA?",
                isPresented: $confirmLocalCAInstall,
                titleVisibility: .visible
            ) {
                Button("Install Local CA") {
                    Task { await model.installLocalCA() }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This changes local certificate trust state for Bifrost HTTPS inspection.")
            }

            SettingsCard("Mobile Devices", systemImage: "iphone") {
                HStack {
                    Text("Connected Android/iOS devices")
                        .font(.headline)
                    Spacer()
                    Button {
                        Task { await model.refreshMobileDevices() }
                    } label: {
                        Label("Scan Devices", systemImage: "dot.radiowaves.left.and.right")
                    }
                    .disabled(model.isMutating)
                }

                if let devices = model.mobileDevices {
                    MobileDiscoverySection(title: "Android", discovery: devices.android)
                    Divider()
                    MobileDiscoverySection(title: "iOS", discovery: devices.ios)

                    if let notice = devices.ordinaryDeviceNotice, !notice.isEmpty {
                        Divider()
                        Text(notice)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    HStack {
                        if let url = model.publicURL(path: "/mobile/ios-profile.mobileconfig") {
                            OpenURLButton(title: "iOS CA Profile", url: url)
                        }
                        if let url = model.publicURL(path: "/mobile/ios-profile.mobileconfig/qrcode") {
                            OpenURLButton(title: "iOS QR", url: url)
                        }
                    }
                } else {
                    LoadingRow()
                }
            }
        }
    }
}

private struct SyncSettingsPage: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsPageHeader(
                title: "Sync",
                subtitle: "Remote sync remains optional. Local rules and traffic capture continue to work without login."
            )

            SettingsCard("Remote Sync", systemImage: "arrow.triangle.2.circlepath") {
                if let status = model.syncStatus {
                    StatusLine(
                        title: syncStatusTitle(status),
                        subtitle: status.lastError ?? syncStatusSubtitle(status),
                        color: syncStatusColor(status)
                    )

                    SettingsToggleRow(
                        title: "Enable Sync",
                        detail: "Use a remote Bifrost service for synchronized rules and settings.",
                        isOn: status.enabled,
                        isDisabled: model.isMutating
                    ) { enabled in
                        await model.setSyncEnabled(enabled)
                    }
                    SettingsToggleRow(
                        title: "Auto Sync",
                        detail: "Automatically resync after reconnection.",
                        isOn: status.autoSync,
                        isDisabled: model.isMutating || !status.enabled
                    ) { enabled in
                        await model.setAutoSync(enabled)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Remote Base URL")
                            .font(.headline)
                        HStack {
                            TextField("https://bifrost.example.com", text: $model.remoteBaseURLDraft)
                                .textFieldStyle(.roundedBorder)
                            Button("Save") {
                                Task { await model.saveRemoteBaseURL() }
                            }
                            .disabled(model.isMutating)
                        }
                    }

                    SettingsValueGrid(rows: [
                        ("Connectivity", status.reachable ? "Reachable" : "Local only"),
                        ("Session", status.hasSession ? (status.user?.userID ?? "Signed in") : "Not signed in"),
                        ("Authorized", status.authorized ? "Yes" : "No"),
                        ("Last Sync", status.lastSyncAt ?? "Never"),
                        ("Last Result", formatSyncAction(status.lastSyncAction)),
                    ])

                    HStack {
                        Button {
                            Task { await model.openSyncLogin() }
                        } label: {
                            Label("Sign In", systemImage: "person.crop.circle.badge.checkmark")
                        }
                        .disabled(model.isMutating || !status.enabled)

                        Button {
                            Task { await model.logoutSync() }
                        } label: {
                            Label("Sign Out", systemImage: "person.crop.circle.badge.xmark")
                        }
                        .disabled(model.isMutating || !status.hasSession)

                        Button {
                            Task { await model.runSyncNow() }
                        } label: {
                            Label("Sync Now", systemImage: "arrow.clockwise")
                        }
                        .disabled(model.isMutating || !status.authorized)
                    }
                } else {
                    LoadingRow()
                }
            }
        }
    }
}

private struct RemoteInvokeSettingsPage: View {
    @ObservedObject var model: SettingsViewModel
    @State private var grantPendingRevoke: Grant?
    @State private var confirmClearCalls = false
    @State private var confirmResetSSH = false
    @State private var confirmRevokeSSH = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header
            statusCard
            discoveryCard
            pendingPairingsCard
            sshKeyCard
            grantsCard
            callsCard
        }
        .remoteInvokeConfirmations(
            grantPendingRevoke: $grantPendingRevoke,
            confirmClearCalls: $confirmClearCalls,
            confirmResetSSH: $confirmResetSSH,
            confirmRevokeSSH: $confirmRevokeSSH,
            model: model
        )
    }

    private var header: some View {
        SettingsPageHeader(
            title: "Remote Invoke",
            subtitle: "Pair callers, approve access, manage SSH key authorization and inspect recent remote calls."
        )
    }

    private var statusCard: some View {
        SettingsCard("Status", systemImage: "antenna.radiowaves.left.and.right") {
            if let status = model.remoteInvokeStatus {
                StatusLine(
                    title: status.state.capitalized,
                    subtitle: "Pending pairings: \(status.pendingPairingsCount) · Active calls: \(status.activeCallIDs.count)",
                    color: status.state.lowercased() == "connected" ? .green : .orange
                )
                SettingsValueGrid(rows: [
                    ("Instance ID", model.clientIdentity?.instanceID ?? "-"),
                    ("Device", "\(model.clientIdentity?.deviceName ?? "-") (\(model.clientIdentity?.platform ?? "-"))"),
                ])
            } else {
                LoadingRow()
            }
        }
    }

    private var discoveryCard: some View {
        SettingsCard("Discovery Mode", systemImage: "qrcode.viewfinder") {
            if let session = model.remoteInvokeStatus?.discoverySession {
                DiscoverySessionView(session: session, model: model)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Not in discovery mode.")
                        .foregroundStyle(.secondary)
                    Button {
                        Task { await model.enterDiscovery() }
                    } label: {
                        Label("Enter Discovery Mode", systemImage: "viewfinder")
                    }
                    .disabled(model.isMutating || model.remoteInvokeStatus?.state.lowercased() != "connected")
                }
            }
        }
    }

    private var pendingPairingsCard: some View {
        SettingsCard("Pending Pairings", systemImage: "person.badge.plus") {
            if model.pendingPairings.isEmpty {
                Text("No pending pairing requests.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(model.pendingPairings) { pairing in
                    PairingRow(pairing: pairing, model: model)
                    Divider()
                }
            }
        }
    }

    private var sshKeyCard: some View {
        SettingsCard("SSH Key", systemImage: "key") {
            if let key = model.sshKey {
                SSHKeyDetail(
                    key: key,
                    model: model,
                    confirmResetSSH: $confirmResetSSH,
                    confirmRevokeSSH: $confirmRevokeSSH
                )
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    Text("No active SSH key.")
                        .foregroundStyle(.secondary)
                    Button {
                        Task { await model.createSSHKey() }
                    } label: {
                        Label("Create SSH Key", systemImage: "key")
                    }
                    .disabled(model.isMutating)
                }
            }
        }
    }

    private var grantsCard: some View {
        SettingsCard("Grants", systemImage: "checklist.checked") {
            if model.grants.isEmpty {
                Text("No active grants.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(model.grants) { grant in
                    GrantRow(grant: grant, model: model) {
                        grantPendingRevoke = grant
                    }
                    Divider()
                }
            }
        }
    }

    private var callsCard: some View {
        SettingsCard("Recent Calls", systemImage: "clock") {
            HStack {
                Text("\(model.calls.count) recent calls")
                    .foregroundStyle(.secondary)
                Spacer()
                Button(role: .destructive) {
                    confirmClearCalls = true
                } label: {
                    Label("Clear Calls", systemImage: "trash")
                }
                .disabled(model.isMutating || model.calls.isEmpty)
            }
            ForEach(model.calls) { call in
                RemoteCallRow(call: call)
                Divider()
            }
        }
    }
}

private struct DiscoverySessionView: View {
    let session: DiscoverySession
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(session.pairCode)
                .font(.system(size: 36, weight: .semibold, design: .monospaced))
                .textSelection(.enabled)
            Text("Expires at \(formatEpoch(session.expiresAt))")
                .foregroundStyle(.secondary)
            HStack {
                CopyButton(text: session.pairCode, title: "Copy Code")
                Button {
                    Task { await model.refreshPairCode() }
                } label: {
                    Label("Regenerate Code", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(model.isMutating)
                Button(role: .destructive) {
                    Task { await model.exitDiscovery() }
                } label: {
                    Label("Exit Discovery", systemImage: "stop.circle")
                }
                .disabled(model.isMutating)
            }
        }
    }
}

private struct PairingRow: View {
    let pairing: PairingRequest
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(pairing.callerInfo.displayName ?? pairing.callerInfo.fingerprint)
                .font(.headline)
            Text(pairing.commandSummary.commandPreview)
                .font(.system(.body, design: .monospaced))
                .lineLimit(2)
            Text(pairing.callerInfo.sourceIP ?? pairing.callerInfo.platform ?? "")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Button {
                    Task { await model.approvePairing(pairing) }
                } label: {
                    Label("Approve 1h Query", systemImage: "checkmark.circle")
                }
                .disabled(model.isMutating)
                Button(role: .destructive) {
                    Task { await model.rejectPairing(pairing) }
                } label: {
                    Label("Reject", systemImage: "xmark.circle")
                }
                .disabled(model.isMutating)
            }
        }
    }
}

private struct SSHKeyDetail: View {
    let key: RemoteInvokeSshKeyRecord
    @ObservedObject var model: SettingsViewModel
    @Binding var confirmResetSSH: Bool
    @Binding var confirmRevokeSSH: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SettingsValueGrid(rows: [
                ("Label", key.label ?? "SSH key"),
                ("Device Code", key.deviceCode),
                ("Fingerprint", key.sshKeyFingerprint ?? "-"),
                ("Status", key.status ?? "-"),
                ("Grant Mode", key.grantMode ?? "-"),
            ])
            HStack {
                Button {
                    Task { await model.copySSHPrivateKey() }
                } label: {
                    Label("Copy Key File", systemImage: "doc.on.doc")
                }
                .disabled(model.isMutating)
                Button {
                    confirmResetSSH = true
                } label: {
                    Label("Reset Key", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(model.isMutating)
                Button(role: .destructive) {
                    confirmRevokeSSH = true
                } label: {
                    Label("Revoke Key", systemImage: "trash")
                }
                .disabled(model.isMutating)
            }
        }
    }
}

private struct GrantRow: View {
    let grant: Grant
    @ObservedObject var model: SettingsViewModel
    let onRevoke: () -> Void

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text(grant.callerDisplayName ?? grant.callerFingerprint)
                    .font(.headline)
                Text("\(grant.grantScope) · \(grant.grantMode) · \(grant.status)")
                    .foregroundStyle(.secondary)
                Text("Created \(formatEpoch(grant.createdAt)) · Used \(grant.useCount ?? 0)x")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button(role: .destructive) {
                onRevoke()
            } label: {
                Label("Revoke", systemImage: "trash")
            }
            .disabled(model.isMutating)
        }
    }
}

private struct RemoteCallRow: View {
    let call: RemoteCall

    var body: some View {
        HStack(alignment: .top) {
            Circle()
                .fill(call.status == "completed" ? Color.green : Color.orange)
                .frame(width: 8, height: 8)
                .padding(.top, 7)
            VStack(alignment: .leading, spacing: 3) {
                Text(call.commandSummary?.commandPreview ?? call.commandKind ?? call.callID)
                    .font(.system(.body, design: .monospaced))
                    .lineLimit(2)
                Text("\(call.status) · \(formatEpoch(call.createdAt)) · \(call.durationMs.map { "\($0)ms" } ?? "-")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

private extension View {
    func remoteInvokeConfirmations(
        grantPendingRevoke: Binding<Grant?>,
        confirmClearCalls: Binding<Bool>,
        confirmResetSSH: Binding<Bool>,
        confirmRevokeSSH: Binding<Bool>,
        model: SettingsViewModel
    ) -> some View {
        self
        .confirmationDialog(
            "Revoke this grant?",
            isPresented: Binding(
                get: { grantPendingRevoke.wrappedValue != nil },
                set: { if !$0 { grantPendingRevoke.wrappedValue = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Revoke Grant", role: .destructive) {
                if let grant = grantPendingRevoke.wrappedValue {
                    Task { await model.revokeGrant(grant) }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            if let grant = grantPendingRevoke.wrappedValue {
                Text(grant.callerDisplayName ?? grant.callerFingerprint)
            }
        }
        .confirmationDialog(
            "Clear recent calls?",
            isPresented: confirmClearCalls,
            titleVisibility: .visible
        ) {
            Button("Clear Calls", role: .destructive) {
                Task { await model.clearCalls() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This removes Remote Invoke call history from this Bifrost instance.")
        }
        .confirmationDialog(
            "Reset SSH key?",
            isPresented: confirmResetSSH,
            titleVisibility: .visible
        ) {
            Button("Reset SSH Key", role: .destructive) {
                Task { await model.resetSSHKey() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Existing SSH callers must use the new key after reset.")
        }
        .confirmationDialog(
            "Revoke SSH key?",
            isPresented: confirmRevokeSSH,
            titleVisibility: .visible
        ) {
            Button("Revoke SSH Key", role: .destructive) {
                Task { await model.revokeSSHKey() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("SSH callers will lose access until a new key is created.")
        }
    }
}

private struct SettingsPageHeader<Trailing: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.largeTitle.weight(.semibold))
                Text(subtitle)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            trailing
        }
    }
}

private extension SettingsPageHeader where Trailing == EmptyView {
    init(title: String, subtitle: String) {
        self.title = title
        self.subtitle = subtitle
        self.trailing = EmptyView()
    }
}

private struct SettingsCard<Content: View>: View {
    let title: String
    let systemImage: String
    @ViewBuilder var content: Content

    init(_ title: String, systemImage: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.systemImage = systemImage
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label(title, systemImage: systemImage)
                .font(.title3.weight(.semibold))
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

private struct SettingsToggleRow: View {
    let title: String
    let detail: String
    let isOn: Bool
    let isDisabled: Bool
    let onChange: (Bool) async -> Void

    var body: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Toggle("", isOn: Binding(
                get: { isOn },
                set: { next in Task { await onChange(next) } }
            ))
            .toggleStyle(.switch)
            .labelsHidden()
            .disabled(isDisabled)
        }
    }
}

private struct SettingsValueGrid: View {
    let rows: [(String, String)]

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 18, verticalSpacing: 8) {
            ForEach(rows, id: \.0) { row in
                GridRow {
                    Text(row.0)
                        .foregroundStyle(.secondary)
                    Text(row.1)
                        .textSelection(.enabled)
                        .lineLimit(4)
                }
            }
        }
    }
}

private struct StatusLine: View {
    let title: String
    let subtitle: String
    let color: Color

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Circle()
                .fill(color)
                .frame(width: 9, height: 9)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                Text(subtitle)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

private struct LoadingRow: View {
    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            Text("Loading...")
                .foregroundStyle(.secondary)
        }
    }
}

private struct CopyButton: View {
    let text: String
    var title: String = "Copy"

    var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        } label: {
            Label(title, systemImage: "doc.on.doc")
        }
    }
}

private struct OpenURLButton: View {
    let title: String
    let url: URL

    var body: some View {
        Button {
            NSWorkspace.shared.open(url)
        } label: {
            Label(title, systemImage: "arrow.up.right.square")
        }
    }
}

private struct MobileDiscoverySection: View {
    let title: String
    let discovery: MobileDiscovery?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.headline)
            if let discovery {
                Text(discovery.message)
                    .foregroundStyle(.secondary)
                if discovery.devices.isEmpty {
                    Text("No devices detected.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(discovery.devices) { device in
                        HStack {
                            Image(systemName: device.platform == "ios" ? "iphone" : "smartphone")
                            VStack(alignment: .leading) {
                                Text(device.name ?? device.id)
                                Text("\(device.status) · \(device.capability) · \(device.statusMessage)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            } else {
                Text("Unavailable")
                    .foregroundStyle(.secondary)
            }
        }
    }
}

private struct SshSecret: Identifiable {
    let id = UUID()
    let deviceCode: String
    let fingerprint: String
    let keyFile: String

    init(payload: RemoteInvokeSshKeySecretPayload) {
        deviceCode = payload.deviceCode
        fingerprint = payload.sshKeyFingerprint
        keyFile = payload.bifrostKeyFile
    }
}

private struct SshSecretSheet: View {
    let secret: SshSecret
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("SSH Key Created")
                .font(.title2.weight(.semibold))
            Text("This key file is shown once. Copy it before closing.")
                .foregroundStyle(.secondary)
            SettingsValueGrid(rows: [
                ("Device Code", secret.deviceCode),
                ("Fingerprint", secret.fingerprint),
            ])
            TextEditor(text: .constant(secret.keyFile))
                .font(.system(.body, design: .monospaced))
                .frame(width: 640, height: 240)
            HStack {
                Spacer()
                CopyButton(text: secret.keyFile, title: "Copy Key File")
                Button("Done") {
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
    }
}

private func syncStatusTitle(_ status: SyncStatus) -> String {
    switch status.reason {
    case "ready":
        return "Ready"
    case "syncing":
        return "Syncing"
    case "unreachable":
        return "Offline"
    case "unauthorized":
        return "Sign in required"
    case "error":
        return "Error"
    default:
        return status.reason.capitalized
    }
}

private func syncStatusSubtitle(_ status: SyncStatus) -> String {
    if status.authorized {
        return "Signed in and authorized."
    }
    if !status.reachable {
        return "Remote service is not reachable."
    }
    if !status.hasSession {
        return "No login session on this device."
    }
    return "Local mode."
}

private func syncStatusColor(_ status: SyncStatus) -> Color {
    switch status.reason {
    case "ready":
        return .green
    case "syncing":
        return .blue
    case "unreachable", "unauthorized":
        return .orange
    case "error":
        return .red
    default:
        return .secondary
    }
}

private func formatSyncAction(_ action: String?) -> String {
    switch action {
    case "local_pushed":
        return "Local changes pushed to remote"
    case "remote_pulled":
        return "Remote changes pulled into local"
    case "bidirectional":
        return "Local and remote changes exchanged"
    case "no_change":
        return "No changes detected"
    default:
        return "No sync result yet"
    }
}

private func formatEpoch(_ epoch: Double) -> String {
    guard epoch > 0 else {
        return "-"
    }
    let date = Date(timeIntervalSince1970: epoch)
    return date.formatted(date: .numeric, time: .standard)
}

private func formatEpoch(_ epoch: Double?) -> String {
    guard let epoch else {
        return "-"
    }
    return formatEpoch(epoch)
}
