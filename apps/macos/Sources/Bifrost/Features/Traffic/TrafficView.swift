import AppKit
import BifrostNativeCore
import SwiftUI

struct TrafficView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        VStack(spacing: 0) {
            if let error = appModel.dataError {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                    Text(error)
                        .lineLimit(1)
                    Spacer()
                    Button("Retry") {
                        Task {
                            await appModel.refreshData()
                        }
                    }
                    .buttonStyle(.borderless)
                }
                .font(.system(size: 12))
                .foregroundStyle(.orange)
                .frame(height: 30)
                .padding(.horizontal, 10)

                Divider()
            }

            HStack(spacing: 0) {
                if !appModel.isFilterPanelCollapsed {
                    NetworkFilterPanel()
                        .frame(width: 205)

                    Divider()
                }

                VStack(spacing: 0) {
                    if appModel.displayedTrafficRecords.isEmpty {
                        EmptyStateView(title: emptyTitle, systemImage: "list.bullet.rectangle")
                            .frame(minHeight: 360)
                    } else {
                        RequestTableView(
                            records: appModel.displayedTrafficRecords,
                            selectedId: appModel.selectedTrafficId
                        ) { record in
                            Task {
                                await appModel.selectTrafficRecord(record)
                            }
                        }
                            .frame(minHeight: 360)
                    }
                }
                .frame(minWidth: 560, idealWidth: 720, maxWidth: 900)

                if !appModel.isDetailPanelCollapsed {
                    Divider()

                    RequestDetailView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
    }

    private var emptyTitle: String {
        if appModel.trafficRecords.isEmpty {
            return "No traffic records"
        }
        return "No matching traffic records"
    }
}

private struct NetworkFilterPanel: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var searchText = ""

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Filters")
                    .font(.system(size: 13, weight: .semibold))
                Spacer()
                Image(systemName: "trash")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            .frame(height: 34)
            .padding(.horizontal, 10)
            .background(.quaternary.opacity(0.28))

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                TextField("Search filters...", text: $searchText)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12))
            }
            .padding(.horizontal, 8)
            .frame(height: 32)

            Divider()

            ScrollView {
                VStack(spacing: 0) {
                    FilterSectionView(
                        title: "Client IP",
                        count: appModel.clientIpCounts.count,
                        rows: filteredRows(appModel.clientIpCounts),
                        onSelect: selectFilter
                    )
                    FilterSectionView(
                        title: "Applications",
                        count: appModel.clientAppCounts.count,
                        rows: filteredRows(appModel.clientAppCounts),
                        onSelect: selectFilter
                    )
                    FilterSectionView(
                        title: "Domains",
                        count: appModel.domainCounts.count,
                        rows: filteredRows(appModel.domainCounts),
                        onSelect: selectFilter
                    )
                }
            }
        }
        .background(.background)
    }

    private func filteredRows(_ rows: [(name: String, count: Int)]) -> [FilterRow] {
        let keyword = searchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        return rows
            .filter { keyword.isEmpty || $0.name.localizedLowercase.contains(keyword) }
            .map { FilterRow(name: $0.name, count: $0.count) }
    }

    private func selectFilter(_ row: FilterRow) {
        appModel.isNetworkSearchVisible = true
        appModel.networkSearchText = row.name
    }
}

private struct FilterRow: Identifiable {
    var id: String { name }
    let name: String
    let count: Int
}

private struct FilterSectionView: View {
    let title: String
    let count: Int
    let rows: [FilterRow]
    let onSelect: (FilterRow) -> Void
    @State private var isCollapsed = false

    var body: some View {
        VStack(spacing: 0) {
            Button {
                isCollapsed.toggle()
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
                        .font(.system(size: 9, weight: .bold))
                    Text(title)
                        .font(.system(size: 12, weight: .semibold))
                    Spacer()
                    Text(count > 99 ? "99+" : "\(count)")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(.quaternary, in: Capsule())
                }
                .frame(height: 24)
                .padding(.horizontal, 8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)

            if !isCollapsed {
                ForEach(rows.prefix(40)) { row in
                    Button {
                        onSelect(row)
                    } label: {
                        HStack(spacing: 7) {
                            if title == "Applications" {
                                AppFilterIcon(appName: row.name)
                            } else {
                                Image(systemName: "circle.grid.cross")
                                    .font(.system(size: 10))
                                    .foregroundStyle(.tertiary)
                                    .frame(width: 16, height: 16)
                            }
                            Text(row.name)
                                .font(.system(size: 11))
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 4)
                            Text("\(row.count)")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                        .frame(height: 26)
                        .padding(.horizontal, 12)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

private struct AppFilterIcon: View {
    @EnvironmentObject private var appModel: AppModel
    let appName: String
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image = LocalAppIconResolver.image(for: appName) ?? image {
                NativeAppIconImage(image: image)
            } else {
                Image(systemName: "app")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            }
        }
        .frame(width: 18, height: 18)
        .onAppear {
            loadIcon()
        }
        .onChange(of: appName) { _ in
            image = nil
            loadIcon()
        }
    }

    private func loadIcon() {
        Task {
            image = await AppIconImageCache.shared.image(for: appName, baseURL: appModel.adminURL)
        }
    }
}

private struct NativeAppIconImage: NSViewRepresentable {
    let image: NSImage

    func makeNSView(context: Context) -> NSImageView {
        let imageView = NSImageView()
        imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.imageFrameStyle = .none
        imageView.wantsLayer = true
        return imageView
    }

    func updateNSView(_ imageView: NSImageView, context: Context) {
        image.isTemplate = false
        imageView.image = image
    }
}

@MainActor
private final class AppIconImageCache {
    static let shared = AppIconImageCache()

    private var images: [String: NSImage] = [:]
    private var misses: Set<String> = []
    private var tasks: [String: Task<Data?, Never>] = [:]

    func image(for appName: String, baseURL: URL) async -> NSImage? {
        guard !appName.isEmpty else {
            return nil
        }
        if let cached = images[appName] {
            return cached
        }
        if misses.contains(appName) {
            return nil
        }

        let data: Data?
        if let task = tasks[appName] {
            data = await task.value
        } else {
            let task = Task<Data?, Never> {
                do {
                    let client = try BifrostClient(baseURL: baseURL)
                    return try await client.fetchAppIcon(appName: appName)
                } catch {
                    return nil
                }
            }
            tasks[appName] = task
            data = await task.value
            tasks[appName] = nil
        }

        guard let data, let image = NSImage(data: data) else {
            misses.insert(appName)
            return nil
        }

        image.isTemplate = false
        images[appName] = image
        return image
    }
}

private enum LocalAppIconResolver {
    private static var images: [String: NSImage] = [:]
    private static var misses: Set<String> = []

    static func image(for appName: String) -> NSImage? {
        if let cached = images[appName] {
            return cached
        }
        if misses.contains(appName) {
            return nil
        }
        for appPath in candidateAppPaths(appName: appName) where FileManager.default.fileExists(atPath: appPath) {
            let icon = NSWorkspace.shared.icon(forFile: appPath)
            icon.isTemplate = false
            images[appName] = icon
            return icon
        }
        misses.insert(appName)
        return nil
    }

    private static func candidateAppPaths(appName: String) -> [String] {
        let names = normalizedAppNameCandidates(appName)
        let roots = appSearchRoots()
        var paths: [String] = []

        for root in roots {
            for name in names {
                paths.append(root.appendingPathComponent("\(name).app").path)
            }
        }

        for root in roots {
            guard let entries = try? FileManager.default.contentsOfDirectory(
                at: root,
                includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]
            ) else {
                continue
            }
            for entry in entries where entry.pathExtension == "app" {
                let stem = entry.deletingPathExtension().lastPathComponent
                if names.contains(where: { candidate in
                    let normalizedStem = stem.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
                    let normalizedCandidate = candidate.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
                    return normalizedStem == normalizedCandidate
                        || normalizedStem.contains(normalizedCandidate)
                        || normalizedCandidate.contains(normalizedStem)
                }) {
                    paths.append(entry.path)
                }
            }
        }

        return Array(NSOrderedSet(array: paths)) as? [String] ?? paths
    }

    private static func appSearchRoots() -> [URL] {
        var roots = [
            URL(fileURLWithPath: "/Applications"),
            URL(fileURLWithPath: "/System/Applications"),
            URL(fileURLWithPath: "/System/Applications/Utilities"),
        ]
        if let home = FileManager.default.homeDirectoryForCurrentUser.path.removingPercentEncoding {
            roots.append(URL(fileURLWithPath: home).appendingPathComponent("Applications"))
        }
        return roots
    }

    private static func normalizedAppNameCandidates(_ name: String) -> [String] {
        let stripped = name
            .replacingOccurrences(of: " Helper (Renderer)", with: "")
            .replacingOccurrences(of: " Helper (GPU)", with: "")
            .replacingOccurrences(of: " Helper (Plugin)", with: "")
            .replacingOccurrences(of: " Helper EH", with: "")
            .replacingOccurrences(of: " Helper NP", with: "")
            .replacingOccurrences(of: " Helper", with: "")
            .replacingOccurrences(of: " (Service)", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)

        var candidates = [stripped, name.trimmingCharacters(in: .whitespacesAndNewlines)]
        let withoutBrowser = stripped
            .replacingOccurrences(of: " Browser", with: "")
            .replacingOccurrences(of: " browser", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if !withoutBrowser.isEmpty {
            candidates.append(withoutBrowser)
        }
        if let firstWord = stripped.split(separator: " ").first {
            candidates.append(String(firstWord))
        }

        return candidates.reduce(into: []) { result, candidate in
            if !candidate.isEmpty && !result.contains(candidate) {
                result.append(candidate)
            }
        }
    }
}

private struct RequestDetailView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var side: DetailSide = .request
    @State private var pane: DetailPane = .overview

    var body: some View {
        if appModel.selectedTrafficId == nil {
            RequestDetailEmptyState()
        } else if appModel.isLoadingTrafficDetail {
            VStack(spacing: 10) {
                ProgressView()
                    .controlSize(.small)
                Text("Loading request details")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        } else {
            VStack(spacing: 0) {
                HStack(spacing: 8) {
                    Image(systemName: "doc.text.magnifyingglass")
                        .foregroundStyle(.secondary)
                    DetailTitle(payload: payload, fallbackId: appModel.selectedTrafficId ?? "")
                    Spacer()
                    Button {
                        guard let id = appModel.selectedTrafficId,
                              let record = appModel.trafficRecords.first(where: { $0.id == id }) else {
                            return
                        }
                        Task {
                            await appModel.selectTrafficRecord(record)
                        }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .buttonStyle(.borderless)
                    .help("Reload request details")
                }
                .frame(height: 34)
                .padding(.horizontal, 10)
                .background(.quaternary.opacity(0.24))

                Divider()

                VStack(spacing: 8) {
                    Picker("Side", selection: $side) {
                        ForEach(DetailSide.allCases) { item in
                            Text(item.rawValue).tag(item)
                        }
                    }
                    .pickerStyle(.segmented)

                    Picker("Pane", selection: $pane) {
                        ForEach(DetailPane.allCases) { item in
                            Text(item.rawValue).tag(item)
                        }
                    }
                    .pickerStyle(.segmented)
                }
                .labelsHidden()
                .padding(.horizontal, 10)
                .padding(.vertical, 8)

                Divider()

                ScrollView {
                    DetailPaneContent(
                        payload: payload,
                        side: side,
                        pane: pane,
                        requestBody: appModel.selectedTrafficRequestBodyText,
                        responseBody: appModel.selectedTrafficResponseBodyText,
                        rawDetail: appModel.selectedTrafficDetailText
                    )
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    private var payload: TrafficDetailPayload {
        TrafficDetailPayload(jsonText: appModel.selectedTrafficDetailText)
    }
}

private enum DetailSide: String, CaseIterable, Identifiable {
    case request = "Request"
    case response = "Response"

    var id: String { rawValue }
}

private enum DetailPane: String, CaseIterable, Identifiable {
    case overview = "Overview"
    case header = "Header"
    case body = "Body"
    case raw = "Raw"

    var id: String { rawValue }
}

private struct DetailTitle: View {
    let payload: TrafficDetailPayload
    let fallbackId: String

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(title)
                .font(.system(size: 12, weight: .semibold, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
            Text(payload.url ?? fallbackId)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    private var title: String {
        let sequence = payload.sequence.map { "#\($0)" } ?? fallbackId
        let method = payload.method ?? "-"
        let status = payload.status.map { "\($0)" } ?? "-"
        return "\(sequence)  \(method)  \(status)"
    }
}

private struct DetailPaneContent: View {
    let payload: TrafficDetailPayload
    let side: DetailSide
    let pane: DetailPane
    let requestBody: String
    let responseBody: String
    let rawDetail: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            switch pane {
            case .overview:
                DetailOverviewGrid(rows: side == .request ? payload.requestOverviewRows : payload.responseOverviewRows)
            case .header:
                DetailKeyValueList(rows: side == .request ? payload.requestHeaders : payload.responseHeaders)
            case .body:
                DetailPayloadBlock(text: side == .request ? requestBody : responseBody)
            case .raw:
                DetailPayloadBlock(text: side == .request ? rawDetail : responseBody)
            }
        }
    }
}

private struct DetailOverviewGrid: View {
    let rows: [(String, String)]

    var body: some View {
        DetailKeyValueList(rows: rows)
    }
}

private struct DetailKeyValueList: View {
    let rows: [(String, String)]

    var body: some View {
        if rows.isEmpty {
            Text("No data")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        } else {
            VStack(spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    HStack(alignment: .top, spacing: 0) {
                        Text(row.0)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .frame(width: 118, alignment: .leading)
                            .padding(.vertical, 6)
                            .padding(.horizontal, 8)
                            .background(.quaternary.opacity(0.28))
                        Text(row.1.isEmpty ? "-" : row.1)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, 6)
                            .padding(.horizontal, 8)
                    }
                    Divider()
                }
            }
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(Color.secondary.opacity(0.18), lineWidth: 1)
            )
        }
    }
}

private struct DetailPayloadBlock: View {
    let text: String

    var body: some View {
        Text(text.isEmpty ? "No data" : text)
            .font(.system(size: 11, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(8)
            .background(.quaternary.opacity(0.32), in: RoundedRectangle(cornerRadius: 6))
    }
}

private struct TrafficDetailPayload {
    let values: [String: Any]

    init(jsonText: String) {
        guard let data = jsonText.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let values = object as? [String: Any] else {
            self.values = [:]
            return
        }
        self.values = values
    }

    var id: String? { string("id") }
    var sequence: Int? { int("sequence") }
    var method: String? { string("method") }
    var status: Int? { int("status") }
    var url: String? { string("url") }
    var protocolName: String? { string("protocol") }
    var listenerPort: Int? { int("listener_port") }
    var host: String? { string("host") }
    var path: String? { string("path") }
    var contentType: String? { string("content_type") }
    var clientApp: String? { string("client_app") }
    var clientIp: String? { string("client_ip") }
    var durationMs: Int? { int("duration_ms") }
    var requestSize: Int? { int("request_size") }
    var responseSize: Int? { int("response_size") }
    var frameCount: Int? { int("frame_count") }

    var requestOverviewRows: [(String, String)] {
        compactRows([
            ("URL", url),
            ("Method", method),
            ("Status", status.map(String.init)),
            ("Protocol", protocolName),
            ("Proxy Port", listenerPort.map(String.init)),
            ("Host", host),
            ("Path", path),
            ("Content Type", contentType),
            ("Client", clientLabel),
        ])
    }

    var responseOverviewRows: [(String, String)] {
        compactRows([
            ("Status", status.map(String.init)),
            ("Duration", durationMs.map { "\($0) ms" }),
            ("Request Size", requestSize.map(formatBytes)),
            ("Response Size", responseSize.map(formatBytes)),
            ("Frame Count", frameCount.map(String.init)),
            ("Content Type", contentType),
            ("Rule Hit", bool("has_rule_hit").map { $0 ? "true" : "false" }),
        ])
    }

    var requestHeaders: [(String, String)] {
        headerRows("request_headers")
    }

    var responseHeaders: [(String, String)] {
        headerRows("response_headers").isEmpty ? headerRows("original_response_headers") : headerRows("response_headers")
    }

    private var clientLabel: String? {
        let values: [String?] = [clientApp, clientIp]
        let parts: [String] = values.compactMap { value in
            guard let value, !value.isEmpty else { return nil }
            return value
        }
        return parts.isEmpty ? nil : parts.joined(separator: " / ")
    }

    private func compactRows(_ rows: [(String, String?)]) -> [(String, String)] {
        rows.compactMap { key, value in
            guard let value, !value.isEmpty else {
                return nil
            }
            return (key, value)
        }
    }

    private func string(_ key: String) -> String? {
        values[key] as? String
    }

    private func int(_ key: String) -> Int? {
        if let intValue = values[key] as? Int {
            return intValue
        }
        if let doubleValue = values[key] as? Double {
            return Int(doubleValue)
        }
        return nil
    }

    private func bool(_ key: String) -> Bool? {
        values[key] as? Bool
    }

    private func headerRows(_ key: String) -> [(String, String)] {
        guard let rows = values[key] as? [[Any]] else {
            return []
        }
        return rows.compactMap { row in
            guard row.count >= 2 else {
                return nil
            }
            return ("\(row[0])", "\(row[1])")
        }
    }

    private func formatBytes(_ value: Int) -> String {
        if value < 1024 {
            return "\(value) B"
        }
        if value < 1024 * 1024 {
            return String(format: "%.1f KB", Double(value) / 1024)
        }
        return String(format: "%.1f MB", Double(value) / 1024 / 1024)
    }
}

private struct RequestDetailEmptyState: View {
    @State private var sequenceSearch = ""

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "doc.text.magnifyingglass")
                .font(.system(size: 36))
                .foregroundStyle(.tertiary)
            Text("Select a request to view details")
                .font(.callout)
                .foregroundStyle(.secondary)
            TextField("Search by sequence number...", text: $sequenceSearch)
                .textFieldStyle(.roundedBorder)
                .frame(width: 230)
        }
    }
}

private struct EmptyStateView: View {
    let title: String
    let systemImage: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
