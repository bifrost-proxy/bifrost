import AppKit
import BifrostNativeCore
import SwiftUI

struct RequestTableView: NSViewRepresentable {
    var records: [TrafficRecordSummary]
    var selectedId: String?
    var onSelect: (TrafficRecordSummary) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(records: records, selectedId: selectedId, onSelect: onSelect)
    }

    func makeNSView(context: Context) -> TrafficTableContainerView {
        let container = TrafficTableContainerView()
        let tableView = container.tableView
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.headerView = NSTableHeaderView()
        tableView.rowHeight = 28
        tableView.allowsMultipleSelection = false
        tableView.delegate = context.coordinator
        tableView.dataSource = context.coordinator
        tableView.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        tableView.gridStyleMask = [.solidHorizontalGridLineMask]
        tableView.gridColor = NSColor.separatorColor.withAlphaComponent(0.16)
        tableView.backgroundColor = .controlBackgroundColor
        tableView.intercellSpacing = NSSize(width: 0, height: 0)

        for column in Column.allCases {
            let tableColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(column.rawValue))
            tableColumn.title = column.title
            tableColumn.width = column.width
            tableColumn.minWidth = column.minWidth
            tableView.addTableColumn(tableColumn)
        }

        context.coordinator.attach(container: container)
        return container
    }

    func updateNSView(_ nsView: TrafficTableContainerView, context: Context) {
        context.coordinator.update(records: records, selectedId: selectedId, onSelect: onSelect)
        nsView.tableView.reloadData()
        if let selectedId,
           let row = records.firstIndex(where: { $0.id == selectedId }) {
            nsView.tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        } else {
            nsView.tableView.deselectAll(nil)
        }
        context.coordinator.refreshScrollState()
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var records: [TrafficRecordSummary]
        var selectedId: String?
        var onSelect: (TrafficRecordSummary) -> Void

        private weak var container: TrafficTableContainerView?
        private var previousTailId: String?
        private var newRecordsCount = 0
        private var isAtBottom = true

        init(
            records: [TrafficRecordSummary],
            selectedId: String?,
            onSelect: @escaping (TrafficRecordSummary) -> Void
        ) {
            self.records = records
            self.selectedId = selectedId
            self.onSelect = onSelect
            self.previousTailId = records.last?.id
        }

        func attach(container: TrafficTableContainerView) {
            self.container = container
            container.floatingButton.target = self
            container.floatingButton.action = #selector(scrollToBottom)
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(boundsDidChange(_:)),
                name: NSView.boundsDidChangeNotification,
                object: container.scrollView.contentView
            )
            refreshScrollState()
        }

        func update(
            records: [TrafficRecordSummary],
            selectedId: String?,
            onSelect: @escaping (TrafficRecordSummary) -> Void
        ) {
            let tailChanged = previousTailId != nil && previousTailId != records.last?.id
            if tailChanged && !isAtBottom {
                newRecordsCount += max(1, records.count - self.records.count)
            }
            self.records = records
            self.selectedId = selectedId
            self.onSelect = onSelect
            self.previousTailId = records.last?.id
        }

        func numberOfRows(in tableView: NSTableView) -> Int {
            records.count
        }

        func tableView(
            _ tableView: NSTableView,
            viewFor tableColumn: NSTableColumn?,
            row: Int
        ) -> NSView? {
            guard row < records.count,
                  let identifier = tableColumn?.identifier.rawValue,
                  let column = Column(rawValue: identifier) else {
                return nil
            }
            return TrafficTableCellFactory.view(column: column, record: records[row])
        }

        func tableViewSelectionDidChange(_ notification: Notification) {
            guard let tableView = notification.object as? NSTableView else {
                return
            }
            let row = tableView.selectedRow
            guard row >= 0, row < records.count else {
                return
            }
            let record = records[row]
            if record.id != selectedId {
                selectedId = record.id
                onSelect(record)
            }
        }

        @objc private func boundsDidChange(_ notification: Notification) {
            refreshScrollState()
        }

        func refreshScrollState() {
            guard let container else {
                return
            }
            let contentView = container.scrollView.contentView
            let documentHeight = container.tableView.bounds.height
            let visibleMaxY = contentView.bounds.maxY
            isAtBottom = documentHeight <= contentView.bounds.height + 1 || visibleMaxY >= documentHeight - 32
            if isAtBottom {
                newRecordsCount = 0
            }
            container.updateFloatingButton(isVisible: !isAtBottom && !records.isEmpty, newRecordsCount: newRecordsCount)
        }

        @objc private func scrollToBottom() {
            guard let container else {
                return
            }
            let row = max(0, records.count - 1)
            container.tableView.scrollRowToVisible(row)
            newRecordsCount = 0
            refreshScrollState()
        }
    }

    enum Column: String, CaseIterable {
        case seq
        case dot
        case proto
        case method
        case status
        case client
        case port
        case rules
        case host
        case path
        case type
        case size
        case time

        var title: String {
            switch self {
            case .seq: return "#"
            case .dot: return ""
            case .proto: return "Protocol"
            case .method: return "Method"
            case .status: return "Status"
            case .client: return "Client"
            case .port: return "Port"
            case .rules: return "Rules"
            case .host: return "Host"
            case .path: return "Path"
            case .type: return "Type"
            case .size: return "Size"
            case .time: return "Time"
            }
        }

        var width: CGFloat {
            switch self {
            case .seq: return 52
            case .dot: return 24
            case .proto, .method: return 70
            case .status: return 55
            case .port, .rules, .size, .time: return 64
            case .client: return 160
            case .host: return 175
            case .path: return 300
            case .type: return 72
            }
        }

        var minWidth: CGFloat {
            switch self {
            case .path: return 180
            case .client, .host: return 110
            default: return width
            }
        }
    }
}

final class TrafficTableContainerView: NSView {
    let scrollView = NSScrollView()
    let tableView = NSTableView()
    let floatingButton = NSButton(title: "", target: nil, action: nil)

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true

        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.contentView.postsBoundsChangedNotifications = true
        addSubview(scrollView)

        floatingButton.translatesAutoresizingMaskIntoConstraints = false
        floatingButton.isBordered = false
        floatingButton.wantsLayer = true
        floatingButton.layer?.cornerRadius = 14
        floatingButton.layer?.backgroundColor = NSColor.controlAccentColor.cgColor
        floatingButton.contentTintColor = .white
        floatingButton.bezelStyle = .regularSquare
        floatingButton.font = .systemFont(ofSize: 12, weight: .medium)
        floatingButton.isHidden = true
        addSubview(floatingButton)

        NSLayoutConstraint.activate([
            scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: topAnchor),
            scrollView.bottomAnchor.constraint(equalTo: bottomAnchor),

            floatingButton.centerXAnchor.constraint(equalTo: centerXAnchor),
            floatingButton.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -16),
            floatingButton.heightAnchor.constraint(equalToConstant: 28),
            floatingButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 112),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    func updateFloatingButton(isVisible: Bool, newRecordsCount: Int) {
        floatingButton.isHidden = !isVisible
        let title = newRecordsCount > 0 ? "New Traffic \(min(newRecordsCount, 99))  ↓" : "↓"
        floatingButton.attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .foregroundColor: NSColor.white,
                .font: NSFont.systemFont(ofSize: 12, weight: .medium),
            ]
        )
    }
}

private enum TrafficTableCellFactory {
    static func view(column: RequestTableView.Column, record: TrafficRecordSummary) -> NSView {
        TrafficCellDrawingView(column: column, record: record)
    }

    fileprivate static func contentTypeShort(for record: TrafficRecordSummary) -> String {
        guard let contentType = record.contentType, !contentType.isEmpty else {
            return "-"
        }
        let primary = contentType.split(separator: ";", maxSplits: 1).first.map(String.init) ?? contentType
        return primary.split(separator: "/").last.map(String.init) ?? primary
    }

    fileprivate static func formatBytes(_ value: Int) -> String {
        if value <= 0 {
            return "-"
        }
        if value < 1024 {
            return "\(value) B"
        }
        if value < 1024 * 1024 {
            return String(format: "%.1f KB", Double(value) / 1024)
        }
        return String(format: "%.1f MB", Double(value) / 1024 / 1024)
    }

    fileprivate static func formatDuration(_ value: Int?) -> String {
        guard let value, value > 0 else {
            return "-"
        }
        if value < 1000 {
            return "\(value)ms"
        }
        if value < 60_000 {
            return String(format: "%.1fs", Double(value) / 1000)
        }
        return String(format: "%.1fm", Double(value) / 60_000)
    }

    fileprivate static func methodPalette(_ method: String?) -> TagPalette {
        switch method?.uppercased() {
        case "GET": return .green
        case "POST": return .blue
        case "PUT": return .orange
        case "DELETE": return .red
        case "PATCH": return .purple
        case "HEAD": return .cyan
        case "CONNECT": return .magenta
        default: return .default
        }
    }

    fileprivate static func statusPalette(_ status: Int) -> TagPalette {
        if status >= 500 { return .red }
        if status >= 400 { return .orange }
        if status >= 300 { return .blue }
        if status >= 200 { return .green }
        return .default
    }

    fileprivate static func statusDotColor(for record: TrafficRecordSummary) -> NSColor {
        if record.method?.uppercased() == "CONNECT", (record.status ?? 0) == 0 {
            return NSColor.systemGreen
        }
        guard let status = record.status else {
            return NSColor(hex: 0xd9d9d9)
        }
        if status == 0 { return NSColor(hex: 0xd9d9d9) }
        if status >= 100 && status < 200 { return NSColor(hex: 0x73d13d) }
        if status >= 200 && status < 300 { return NSColor(hex: 0x52c41a) }
        if status >= 300 && status < 400 { return NSColor(hex: 0xfaad14) }
        if status >= 400 && status < 500 { return NSColor(hex: 0xfa8c16) }
        if status >= 500 { return NSColor(hex: 0xf5222d) }
        return NSColor(hex: 0xd9d9d9)
    }

    fileprivate static func displayProtocol(for record: TrafficRecordSummary) -> String {
        if record.isH3 {
            return "H3"
        }
        guard let protocolName = record.protocolName, !protocolName.isEmpty else {
            return "-"
        }
        return protocolName.replacingOccurrences(of: "HTTP/", with: "").uppercased()
    }

    fileprivate static func formatSequence(_ sequence: Int?) -> String {
        guard let sequence else {
            return "-"
        }
        let raw = String(sequence)
        let suffix = raw.count > 5 ? String(raw.suffix(5)) : raw
        return String(repeating: "0", count: max(0, 5 - suffix.count)) + suffix
    }

    fileprivate static func durationColor(_ value: Int?) -> NSColor {
        guard let value, value > 1000 else {
            return .secondaryLabelColor
        }
        return NSColor(hex: 0xfaad14)
    }

    fileprivate static func ruleTooltip(_ record: TrafficRecordSummary) -> String {
        let count = max(1, record.matchedRuleCount ?? 1)
        guard !record.matchedProtocols.isEmpty else {
            return "\(count) rule(s) matched"
        }
        return "\(count) rule(s) matched: \(record.matchedProtocols.joined(separator: ", "))"
    }

    private static func legacyView(column: RequestTableView.Column, record: TrafficRecordSummary) -> NSView {
        switch column {
        case .seq:
            return label(formatSequence(record.seq), font: .monospacedSystemFont(ofSize: 11, weight: .regular), color: .secondaryLabelColor, alignment: .right)
        case .dot:
            return centered(StatusDotView(color: statusDotColor(for: record)))
        case .proto:
            return paddedBadge(displayProtocol(for: record), palette: record.isH3 ? .purple : .default)
        case .method:
            return paddedBadge(record.method ?? "-", palette: methodPalette(record.method))
        case .status:
            if let status = record.status, status > 0 {
                return paddedBadge(String(status), palette: statusPalette(status))
            }
            return label("-", color: .secondaryLabelColor, alignment: .center)
        case .client:
            return clientView(record)
        case .port:
            if let port = record.listenerPort {
                return paddedBadge(String(port), palette: .default)
            }
            return label("-", color: .secondaryLabelColor, alignment: .center)
        case .rules:
            return rulesView(record)
        case .host:
            return label(record.host ?? "-", color: .secondaryLabelColor)
        case .path:
            return label(record.path?.isEmpty == false ? record.path ?? "" : "/", color: .secondaryLabelColor)
        case .type:
            return label(contentTypeShort(for: record), font: .systemFont(ofSize: 11), color: .secondaryLabelColor)
        case .size:
            return label(formatBytes(record.responseSize ?? 0), font: .systemFont(ofSize: 11), color: .secondaryLabelColor, alignment: .right)
        case .time:
            return label(formatDuration(record.durationMs), font: .systemFont(ofSize: 11), color: durationColor(record.durationMs), alignment: .right)
        }
    }

    private static func centered(_ view: NSView) -> NSView {
        let container = NSView()
        view.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            view.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        return container
    }

    private static func clientView(_ record: TrafficRecordSummary) -> NSView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 4
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 8, bottom: 0, right: 8)

        if let app = record.clientApp, !app.isEmpty {
            let imageView = NSImageView()
            imageView.imageScaling = .scaleProportionallyUpOrDown
            imageView.image = AppIconResolver.image(for: app) ?? NSImage(systemSymbolName: "app", accessibilityDescription: nil)
            imageView.symbolConfiguration = .init(pointSize: 12, weight: .regular)
            imageView.setContentHuggingPriority(.required, for: .horizontal)
            imageView.widthAnchor.constraint(equalToConstant: 16).isActive = true
            imageView.heightAnchor.constraint(equalToConstant: 16).isActive = true
            stack.addArrangedSubview(imageView)
        }

        let text = NSTextField(labelWithString: record.clientDisplay)
        text.lineBreakMode = .byTruncatingTail
        text.font = .systemFont(ofSize: 11)
        text.textColor = .secondaryLabelColor
        text.toolTip = record.clientTooltip
        stack.addArrangedSubview(text)
        return stack
    }

    private static func rulesView(_ record: TrafficRecordSummary) -> NSView {
        guard record.hasRuleHit else {
            return label("-", color: .secondaryLabelColor, alignment: .center)
        }

        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 0
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 8, bottom: 0, right: 8)

        let imageView = NSImageView()
        imageView.image = NSImage(systemSymbolName: "bolt.fill", accessibilityDescription: nil)
        imageView.symbolConfiguration = .init(pointSize: 13, weight: .semibold)
        imageView.contentTintColor = NSColor.systemBlue
        imageView.widthAnchor.constraint(equalToConstant: 16).isActive = true
        imageView.heightAnchor.constraint(equalToConstant: 16).isActive = true
        stack.addArrangedSubview(imageView)

        let count = max(1, record.matchedRuleCount ?? 1)
        let bubble = badgeLabel(String(count), palette: .blue)
        bubble.font = .systemFont(ofSize: 9, weight: .semibold)
        bubble.toolTip = ruleTooltip(record)
        stack.toolTip = ruleTooltip(record)
        stack.addArrangedSubview(bubble)
        return stack
    }

    private static func paddedBadge(_ text: String, palette: TagPalette) -> NSView {
        let container = NSView()
        let badge = badgeLabel(text, palette: palette)
        badge.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(badge)
        NSLayoutConstraint.activate([
            badge.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 8),
            badge.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        return container
    }

    private static func badgeLabel(_ text: String, palette: TagPalette) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: 11, weight: .medium)
        label.textColor = palette.text
        label.alignment = .center
        label.wantsLayer = true
        label.layer?.backgroundColor = palette.background.cgColor
        label.layer?.borderColor = palette.border.cgColor
        label.layer?.borderWidth = 1
        label.layer?.cornerRadius = 4
        label.lineBreakMode = .byTruncatingTail
        label.setContentCompressionResistancePriority(.required, for: .horizontal)
        label.translatesAutoresizingMaskIntoConstraints = false
        label.heightAnchor.constraint(equalToConstant: 18).isActive = true
        label.widthAnchor.constraint(greaterThanOrEqualToConstant: 34).isActive = true
        return label
    }

    private static func label(
        _ text: String,
        font: NSFont = .systemFont(ofSize: 12),
        color: NSColor = .labelColor,
        alignment: NSTextAlignment = .left
    ) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = font
        field.textColor = color
        field.alignment = alignment
        field.lineBreakMode = .byTruncatingMiddle
        return field
    }

    private static func legacyFormatSequence(_ sequence: Int?) -> String {
        guard let sequence else {
            return "-"
        }
        let raw = String(sequence)
        let suffix = raw.count > 5 ? String(raw.suffix(5)) : raw
        return String(repeating: "0", count: max(0, 5 - suffix.count)) + suffix
    }

    private static func legacyDisplayProtocol(for record: TrafficRecordSummary) -> String {
        if record.isH3 {
            return "H3"
        }
        guard let protocolName = record.protocolName, !protocolName.isEmpty else {
            return "-"
        }
        return protocolName.replacingOccurrences(of: "HTTP/", with: "").uppercased()
    }

    private static func legacyContentTypeShort(for record: TrafficRecordSummary) -> String {
        guard let contentType = record.contentType, !contentType.isEmpty else {
            return "-"
        }
        let primary = contentType.split(separator: ";", maxSplits: 1).first.map(String.init) ?? contentType
        return primary.split(separator: "/").last.map(String.init) ?? primary
    }

    private static func legacyFormatBytes(_ value: Int) -> String {
        if value <= 0 {
            return "-"
        }
        if value < 1024 {
            return "\(value) B"
        }
        if value < 1024 * 1024 {
            return String(format: "%.1f KB", Double(value) / 1024)
        }
        return String(format: "%.1f MB", Double(value) / 1024 / 1024)
    }

    private static func legacyFormatDuration(_ value: Int?) -> String {
        guard let value, value > 0 else {
            return "-"
        }
        if value < 1000 {
            return "\(value)ms"
        }
        if value < 60_000 {
            return String(format: "%.1fs", Double(value) / 1000)
        }
        return String(format: "%.1fm", Double(value) / 60_000)
    }

    private static func legacyDurationColor(_ value: Int?) -> NSColor {
        guard let value, value > 1000 else {
            return .secondaryLabelColor
        }
        return NSColor.systemOrange
    }

    private static func legacyMethodPalette(_ method: String?) -> TagPalette {
        switch method?.uppercased() {
        case "GET": return .green
        case "POST": return .blue
        case "PUT": return .orange
        case "DELETE": return .red
        case "PATCH": return .purple
        case "HEAD": return .cyan
        case "CONNECT": return .magenta
        default: return .default
        }
    }

    private static func legacyStatusPalette(_ status: Int) -> TagPalette {
        if status >= 500 { return .red }
        if status >= 400 { return .orange }
        if status >= 300 { return .blue }
        if status >= 200 { return .green }
        return .default
    }

    private static func legacyStatusDotColor(for record: TrafficRecordSummary) -> NSColor {
        if record.method?.uppercased() == "CONNECT", (record.status ?? 0) == 0 {
            return NSColor.systemGreen
        }
        guard let status = record.status else {
            return NSColor(calibratedRed: 0.85, green: 0.85, blue: 0.85, alpha: 1)
        }
        if status == 0 { return NSColor(calibratedRed: 0.85, green: 0.85, blue: 0.85, alpha: 1) }
        if status >= 100 && status < 200 { return NSColor(calibratedRed: 0.45, green: 0.82, blue: 0.24, alpha: 1) }
        if status >= 200 && status < 300 { return NSColor(calibratedRed: 0.32, green: 0.77, blue: 0.10, alpha: 1) }
        if status >= 300 && status < 400 { return NSColor.systemOrange }
        if status >= 400 && status < 500 { return NSColor.systemOrange }
        if status >= 500 { return NSColor.systemRed }
        return NSColor(calibratedRed: 0.85, green: 0.85, blue: 0.85, alpha: 1)
    }

    private static func legacyRuleTooltip(_ record: TrafficRecordSummary) -> String {
        let count = max(1, record.matchedRuleCount ?? 1)
        guard !record.matchedProtocols.isEmpty else {
            return "\(count) rule(s) matched"
        }
        return "\(count) rule(s) matched: \(record.matchedProtocols.joined(separator: ", "))"
    }
}

private struct TrafficCellContent: View {
    let column: RequestTableView.Column
    let record: TrafficRecordSummary

    var body: some View {
        Group {
            switch column {
            case .seq:
                Text(TrafficTableCellFactory.formatSequence(record.seq))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .padding(.horizontal, 8)
            case .dot:
                Circle()
                    .fill(Color(TrafficTableCellFactory.statusDotColor(for: record)))
                    .frame(width: 8, height: 8)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .proto:
                TagView(text: TrafficTableCellFactory.displayProtocol(for: record), palette: record.isH3 ? .purple : .default)
            case .method:
                TagView(text: record.method ?? "-", palette: TrafficTableCellFactory.methodPalette(record.method))
            case .status:
                if let status = record.status, status > 0 {
                    TagView(text: String(status), palette: TrafficTableCellFactory.statusPalette(status))
                } else {
                    PlainCellText("-")
                        .frame(maxWidth: .infinity, alignment: .center)
                }
            case .client:
                HStack(spacing: 4) {
                    if let app = record.clientApp, !app.isEmpty {
                        if let image = AppIconResolver.image(for: app) {
                            Image(nsImage: image)
                                .resizable()
                                .aspectRatio(contentMode: .fit)
                                .frame(width: 14, height: 14)
                        } else {
                            Image(systemName: "app")
                                .font(.system(size: 10))
                                .foregroundStyle(.tertiary)
                                .frame(width: 14, height: 14)
                        }
                    }
                    PlainCellText(record.clientDisplay)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .padding(.horizontal, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(record.clientTooltip)
            case .port:
                if let port = record.listenerPort {
                    TagView(text: String(port), palette: .default)
                } else {
                    PlainCellText("-")
                        .frame(maxWidth: .infinity, alignment: .center)
                }
            case .rules:
                if record.hasRuleHit {
                    HStack(spacing: -1) {
                        Image(systemName: "bolt.fill")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(Color.accentColor)
                        Text(String(max(1, record.matchedRuleCount ?? 1)))
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(.white)
                            .frame(minWidth: 14, minHeight: 14)
                            .background(Color.accentColor, in: Circle())
                    }
                    .frame(maxWidth: .infinity)
                    .help(TrafficTableCellFactory.ruleTooltip(record))
                } else {
                    PlainCellText("-")
                        .frame(maxWidth: .infinity, alignment: .center)
                }
            case .host:
                PlainCellText(record.host ?? "-")
                    .padding(.horizontal, 8)
            case .path:
                PlainCellText(record.path?.isEmpty == false ? record.path ?? "" : "/")
                    .padding(.horizontal, 8)
            case .type:
                PlainCellText(TrafficTableCellFactory.contentTypeShort(for: record), size: 11)
                    .padding(.horizontal, 8)
            case .size:
                PlainCellText(TrafficTableCellFactory.formatBytes(record.responseSize ?? 0), size: 11)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .padding(.horizontal, 8)
            case .time:
                Text(TrafficTableCellFactory.formatDuration(record.durationMs))
                    .font(.system(size: 11))
                    .foregroundStyle(Color(TrafficTableCellFactory.durationColor(record.durationMs)))
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .padding(.horizontal, 8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private final class TrafficCellDrawingView: NSView {
    let column: RequestTableView.Column
    let record: TrafficRecordSummary

    init(column: RequestTableView.Column, record: TrafficRecordSummary) {
        self.column = column
        self.record = record
        super.init(frame: .zero)
        wantsLayer = true
        toolTip = tooltip
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override var isFlipped: Bool {
        true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        switch column {
        case .seq:
            drawText(TrafficTableCellFactory.formatSequence(record.seq), in: bounds.insetBy(dx: 8, dy: 0), font: .monospacedSystemFont(ofSize: 11, weight: .regular), color: .secondaryLabelColor, alignment: .right)
        case .dot:
            drawDot(color: TrafficTableCellFactory.statusDotColor(for: record))
        case .proto:
            drawTag(TrafficTableCellFactory.displayProtocol(for: record), palette: record.isH3 ? .purple : .default)
        case .method:
            drawTag(record.method ?? "-", palette: TrafficTableCellFactory.methodPalette(record.method))
        case .status:
            if let status = record.status, status > 0 {
                drawTag(String(status), palette: TrafficTableCellFactory.statusPalette(status))
            } else {
                drawText("-", in: bounds, color: .secondaryLabelColor, alignment: .center)
            }
        case .client:
            drawClient()
        case .port:
            if let port = record.listenerPort {
                drawTag(String(port), palette: .default)
            } else {
                drawText("-", in: bounds, color: .secondaryLabelColor, alignment: .center)
            }
        case .rules:
            drawRules()
        case .host:
            drawText(record.host ?? "-", in: bounds.insetBy(dx: 8, dy: 0), color: .secondaryLabelColor)
        case .path:
            drawText(record.path?.isEmpty == false ? record.path ?? "" : "/", in: bounds.insetBy(dx: 8, dy: 0), color: .secondaryLabelColor)
        case .type:
            drawText(TrafficTableCellFactory.contentTypeShort(for: record), in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: .secondaryLabelColor)
        case .size:
            drawText(TrafficTableCellFactory.formatBytes(record.responseSize ?? 0), in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: .secondaryLabelColor, alignment: .right)
        case .time:
            drawText(TrafficTableCellFactory.formatDuration(record.durationMs), in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: TrafficTableCellFactory.durationColor(record.durationMs), alignment: .right)
        }
    }

    private var tooltip: String? {
        switch column {
        case .client:
            return record.clientTooltip
        case .rules:
            return record.hasRuleHit ? TrafficTableCellFactory.ruleTooltip(record) : nil
        case .host:
            return record.host
        case .path:
            return record.path
        default:
            return nil
        }
    }

    private func drawDot(color: NSColor) {
        let rect = NSRect(x: bounds.midX - 4, y: bounds.midY - 4, width: 8, height: 8)
        color.setFill()
        NSBezierPath(ovalIn: rect).fill()
    }

    private func drawTag(_ text: String, palette: TagPalette) {
        let font = NSFont.systemFont(ofSize: 11, weight: .medium)
        let attributes = textAttributes(font: font, color: palette.text, alignment: .center)
        let textSize = (text as NSString).size(withAttributes: attributes)
        let width = max(34, ceil(textSize.width) + 14)
        let rect = NSRect(x: 8, y: bounds.midY - 9, width: min(width, max(34, bounds.width - 12)), height: 18)
        let path = NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4)
        palette.background.setFill()
        path.fill()
        palette.border.setStroke()
        path.lineWidth = 1
        path.stroke()
        (text as NSString).draw(in: rect.insetBy(dx: 4, dy: 2), withAttributes: attributes)
    }

    private func drawClient() {
        var x: CGFloat = 8
        if let app = record.clientApp, !app.isEmpty {
            let iconRect = NSRect(x: x, y: bounds.midY - 7, width: 14, height: 14)
            let image = AppIconResolver.image(for: app) ?? NSImage(systemSymbolName: "app", accessibilityDescription: nil)
            image?.draw(in: iconRect, from: .zero, operation: .sourceOver, fraction: 1)
            x += 18
        }
        drawText(record.clientDisplay, in: NSRect(x: x, y: 0, width: max(0, bounds.width - x - 8), height: bounds.height), font: .systemFont(ofSize: 11), color: .secondaryLabelColor)
    }

    private func drawRules() {
        guard record.hasRuleHit else {
            drawText("-", in: bounds, color: .secondaryLabelColor, alignment: .center)
            return
        }

        let blue = NSColor.controlAccentColor
        let boltRect = NSRect(x: bounds.midX - 13, y: bounds.midY - 7, width: 14, height: 14)
        if let bolt = NSImage(systemSymbolName: "bolt.fill", accessibilityDescription: nil) {
            bolt.isTemplate = true
            blue.set()
            bolt.draw(in: boltRect, from: .zero, operation: .sourceOver, fraction: 1)
        }

        let countText = String(max(1, record.matchedRuleCount ?? 1))
        let bubbleRect = NSRect(x: bounds.midX - 1, y: bounds.midY - 8, width: max(14, CGFloat(countText.count * 7 + 8)), height: 14)
        blue.setFill()
        NSBezierPath(roundedRect: bubbleRect, xRadius: 7, yRadius: 7).fill()
        drawText(countText, in: bubbleRect.offsetBy(dx: 0, dy: -1), font: .systemFont(ofSize: 9, weight: .semibold), color: .white, alignment: .center)
    }

    private func drawText(
        _ text: String,
        in rect: NSRect,
        font: NSFont = .systemFont(ofSize: 12),
        color: NSColor = .labelColor,
        alignment: NSTextAlignment = .left
    ) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        paragraph.lineBreakMode = .byTruncatingMiddle
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: color,
            .paragraphStyle: paragraph,
        ]
        let textRect = NSRect(x: rect.minX, y: rect.midY - font.ascender / 2 - 5, width: rect.width, height: 16)
        (text as NSString).draw(in: textRect, withAttributes: attributes)
    }

    private func textAttributes(font: NSFont, color: NSColor, alignment: NSTextAlignment) -> [NSAttributedString.Key: Any] {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        paragraph.lineBreakMode = .byTruncatingTail
        return [
            .font: font,
            .foregroundColor: color,
            .paragraphStyle: paragraph,
        ]
    }
}

private struct PlainCellText: View {
    let text: String
    let size: CGFloat

    init(_ text: String, size: CGFloat = 12) {
        self.text = text
        self.size = size
    }

    var body: some View {
        Text(text)
            .font(.system(size: size))
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct TagView: View {
    let text: String
    let palette: TagPalette

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(Color(palette.text))
            .lineLimit(1)
            .padding(.horizontal, 7)
            .frame(height: 18)
            .background(Color(palette.background), in: RoundedRectangle(cornerRadius: 4))
            .overlay(
                RoundedRectangle(cornerRadius: 4)
                    .stroke(Color(palette.border), lineWidth: 1)
            )
            .padding(.horizontal, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private final class StatusDotView: NSView {
    private let color: NSColor

    init(color: NSColor) {
        self.color = color
        super.init(frame: NSRect(x: 0, y: 0, width: 8, height: 8))
        wantsLayer = true
        layer?.cornerRadius = 4
        layer?.backgroundColor = color.cgColor
        translatesAutoresizingMaskIntoConstraints = false
        widthAnchor.constraint(equalToConstant: 8).isActive = true
        heightAnchor.constraint(equalToConstant: 8).isActive = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }
}

private struct TagPalette {
    let background: NSColor
    let border: NSColor
    let text: NSColor

    static let `default` = TagPalette(hexBackground: 0xf5f5f5, hexBorder: 0xd9d9d9, hexText: 0x595959)
    static let green = TagPalette(hexBackground: 0xf6ffed, hexBorder: 0xb7eb8f, hexText: 0x52c41a)
    static let blue = TagPalette(hexBackground: 0xe6f4ff, hexBorder: 0x91caff, hexText: 0x1677ff)
    static let orange = TagPalette(hexBackground: 0xfff7e6, hexBorder: 0xffd591, hexText: 0xfa8c16)
    static let red = TagPalette(hexBackground: 0xfff1f0, hexBorder: 0xffa39e, hexText: 0xf5222d)
    static let purple = TagPalette(hexBackground: 0xf9f0ff, hexBorder: 0xd3adf7, hexText: 0x722ed1)
    static let cyan = TagPalette(hexBackground: 0xe6fffb, hexBorder: 0x87e8de, hexText: 0x13c2c2)
    static let magenta = TagPalette(hexBackground: 0xfff0f6, hexBorder: 0xffadd2, hexText: 0xeb2f96)

    private init(hexBackground: UInt32, hexBorder: UInt32, hexText: UInt32) {
        self.background = NSColor(hex: hexBackground)
        self.border = NSColor(hex: hexBorder)
        self.text = NSColor(hex: hexText)
    }
}

private enum AppIconResolver {
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
        let roots = [
            URL(fileURLWithPath: "/Applications"),
            URL(fileURLWithPath: "/System/Applications"),
            URL(fileURLWithPath: "/System/Applications/Utilities"),
            FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications"),
        ]
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
                let normalizedStem = normalized(stem)
                if names.map(normalized).contains(where: { candidate in
                    normalizedStem == candidate || normalizedStem.contains(candidate) || candidate.contains(normalizedStem)
                }) {
                    paths.append(entry.path)
                }
            }
        }

        return Array(NSOrderedSet(array: paths)) as? [String] ?? paths
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

    private static func normalized(_ value: String) -> String {
        value.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
    }
}

private extension TrafficRecordSummary {
    var isH3: Bool {
        protocolName == "h3" || protocolName == "h3s" || ((flags ?? 0) & (1 << 3)) != 0
    }

    var clientDisplay: String {
        if let app = clientApp, !app.isEmpty {
            return app
        }
        if let ip = clientIp, !ip.isEmpty {
            return ip
        }
        return "-"
    }

    var clientTooltip: String {
        if let app = clientApp, !app.isEmpty {
            return "\(app) / \(clientIp ?? "?")"
        }
        return clientIp ?? "-"
    }
}

private extension NSColor {
    convenience init(hex: UInt32) {
        self.init(
            calibratedRed: CGFloat((hex >> 16) & 0xff) / 255,
            green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255,
            alpha: 1
        )
    }
}
