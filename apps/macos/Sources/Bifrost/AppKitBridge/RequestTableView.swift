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
        tableView.headerView = NSTableHeaderView()
        tableView.rowHeight = 28
        tableView.allowsMultipleSelection = false
        tableView.delegate = context.coordinator
        tableView.dataSource = context.coordinator
        tableView.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.gridStyleMask = [.solidHorizontalGridLineMask]
        tableView.gridColor = NSColor.separatorColor.withAlphaComponent(0.16)
        tableView.backgroundColor = .controlBackgroundColor
        tableView.intercellSpacing = .zero

        for column in Column.allCases {
            let tableColumn = NSTableColumn(identifier: column.identifier)
            tableColumn.title = column.title
            tableColumn.width = column.width
            tableColumn.minWidth = column.minWidth
            tableView.addTableColumn(tableColumn)
        }

        context.coordinator.attach(container: container)
        return container
    }

    func updateNSView(_ nsView: TrafficTableContainerView, context: Context) {
        context.coordinator.apply(records: records, selectedId: selectedId, onSelect: onSelect)
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        private var records: [TrafficRecordSummary]
        private var rows: [TrafficRowViewModel]
        private var rowById: [String: Int]
        private var selectedId: String?
        private var onSelect: (TrafficRecordSummary) -> Void

        private weak var container: TrafficTableContainerView?
        private var previousTailId: String?
        private var newRecordsCount = 0
        private var isAtBottom = true
        private var iconObserver: NSObjectProtocol?

        init(records: [TrafficRecordSummary], selectedId: String?, onSelect: @escaping (TrafficRecordSummary) -> Void) {
            let records = Self.deduplicatedRecords(records)
            self.records = records
            self.rows = records.map(TrafficRowViewModel.init)
            self.rowById = Self.indexRows(records)
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
            iconObserver = NotificationCenter.default.addObserver(
                forName: AppIconCache.didUpdateNotification,
                object: nil,
                queue: .main
            ) { [weak self] notification in
                self?.reloadVisibleClientRows(for: notification.object as? String)
            }
            refreshScrollState()
        }

        deinit {
            if let iconObserver {
                NotificationCenter.default.removeObserver(iconObserver)
            }
            if let container {
                NotificationCenter.default.removeObserver(
                    self,
                    name: NSView.boundsDidChangeNotification,
                    object: container.scrollView.contentView
                )
            }
        }

        func apply(records newRecords: [TrafficRecordSummary], selectedId: String?, onSelect: @escaping (TrafficRecordSummary) -> Void) {
            let newRecords = Self.deduplicatedRecords(newRecords)
            guard let tableView = container?.tableView else {
                records = newRecords
                rows = newRecords.map(TrafficRowViewModel.init)
                rowById = Self.indexRows(newRecords)
                self.selectedId = selectedId
                self.onSelect = onSelect
                return
            }

            self.selectedId = selectedId
            self.onSelect = onSelect
            let tailChanged = previousTailId != nil && previousTailId != newRecords.last?.id
            if tailChanged && !isAtBottom {
                newRecordsCount += max(1, newRecords.count - records.count)
            }

            applyTablePatch(tableView: tableView, newRecords: newRecords)
            previousTailId = records.last?.id
            syncSelection(in: tableView)
            refreshScrollState()
        }

        func numberOfRows(in tableView: NSTableView) -> Int {
            rows.count
        }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard row < rows.count,
                  let identifier = tableColumn?.identifier,
                  let column = RequestTableView.Column(identifier: identifier) else {
                return nil
            }
            let view = tableView.makeView(withIdentifier: identifier, owner: self) as? TrafficCellView
                ?? TrafficCellView(column: column, identifier: identifier)
            view.configure(row: rows[row])
            return view
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

        private func applyTablePatch(
            tableView: NSTableView,
            newRecords: [TrafficRecordSummary]
        ) {
            if records.isEmpty || newRecords.isEmpty {
                records = newRecords
                rows = newRecords.map(TrafficRowViewModel.init)
                rowById = Self.indexRows(newRecords)
                tableView.reloadData()
                return
            }

            if isTailAppend(newRecords) {
                let oldCount = records.count
                let insertedRange = oldCount..<newRecords.count
                records = newRecords
                if !insertedRange.isEmpty {
                    rows.append(contentsOf: newRecords[insertedRange].map(TrafficRowViewModel.init))
                    for index in insertedRange {
                        rowById[newRecords[index].id] = index
                    }
                    tableView.insertRows(at: IndexSet(integersIn: insertedRange), withAnimation: [])
                }
                return
            }

            if isTailRemoval(newRecords) {
                let removedRange = newRecords.count..<records.count
                records = newRecords
                rows.removeSubrange(removedRange)
                rowById = Self.indexRows(newRecords)
                if !removedRange.isEmpty {
                    tableView.removeRows(at: IndexSet(integersIn: removedRange), withAnimation: [])
                }
                return
            }

            if hasSameIdentityOrder(newRecords) {
                let oldRecords = records
                records = newRecords
                var changedRows = IndexSet()
                for (index, record) in newRecords.enumerated() where oldRecords[index] != record {
                    let next = TrafficRowViewModel(record: record)
                    if rows[index] != next {
                        rows[index] = next
                        changedRows.insert(index)
                    }
                }
                reload(rows: changedRows, in: tableView)
                return
            }

            let oldIds = records.map(\.id)
            let newIds = newRecords.map(\.id)
            let oldIdSet = Set(oldIds)
            let newIdSet = Set(newIds)
            let removed = oldIds.enumerated().compactMap { newIdSet.contains($0.element) ? nil : $0.offset }
            let inserted = newIds.enumerated().compactMap { oldIdSet.contains($0.element) ? nil : $0.offset }

            if removed.count + inserted.count <= max(12, oldIds.count / 20), newIds.count < 20_000 {
                let oldRowsById = Self.indexRowsById(rows)
                records = newRecords
                rows = newRecords.map(TrafficRowViewModel.init)
                rowById = Self.indexRows(newRecords)
                tableView.removeRows(at: IndexSet(removed), withAnimation: [])
                tableView.insertRows(at: IndexSet(inserted), withAnimation: [])
                let changed = rows.enumerated().reduce(into: IndexSet()) { result, item in
                    if oldRowsById[item.element.id] != item.element {
                        result.insert(item.offset)
                    }
                }
                reload(rows: changed, in: tableView)
            } else {
                records = newRecords
                rows = newRecords.map(TrafficRowViewModel.init)
                rowById = Self.indexRows(newRecords)
                tableView.reloadData()
            }
        }

        private func isTailAppend(_ newRecords: [TrafficRecordSummary]) -> Bool {
            guard newRecords.count > records.count else {
                return false
            }
            guard let oldLast = records.last else {
                return true
            }
            return newRecords[records.count - 1].id == oldLast.id
        }

        private func isTailRemoval(_ newRecords: [TrafficRecordSummary]) -> Bool {
            guard records.count > newRecords.count else {
                return false
            }
            guard let newLast = newRecords.last else {
                return true
            }
            return records[newRecords.count - 1].id == newLast.id
        }

        private func hasSameIdentityOrder(_ newRecords: [TrafficRecordSummary]) -> Bool {
            guard newRecords.count == records.count else {
                return false
            }
            guard newRecords.first?.id == records.first?.id,
                  newRecords.last?.id == records.last?.id else {
                return false
            }
            for index in newRecords.indices where newRecords[index].id != records[index].id {
                return false
            }
            return true
        }

        private func reload(rows rowIndexes: IndexSet, in tableView: NSTableView) {
            guard !rowIndexes.isEmpty else {
                return
            }
            tableView.reloadData(forRowIndexes: rowIndexes, columnIndexes: IndexSet(integersIn: 0..<RequestTableView.Column.allCases.count))
        }

        private func reloadVisibleClientRows(for appName: String?) {
            guard let appName,
                  let tableView = container?.tableView,
                  let clientColumn = RequestTableView.Column.allCases.firstIndex(of: .client) else {
                return
            }

            let visibleRows = tableView.rows(in: tableView.visibleRect)
            guard visibleRows.location != NSNotFound, visibleRows.length > 0 else {
                return
            }

            var rowsToReload = IndexSet()
            let upperBound = min(rows.count, visibleRows.location + visibleRows.length)
            for index in visibleRows.location..<upperBound where rows[index].clientApp == appName {
                rowsToReload.insert(index)
            }
            guard !rowsToReload.isEmpty else {
                return
            }
            tableView.reloadData(forRowIndexes: rowsToReload, columnIndexes: IndexSet(integer: clientColumn))
        }

        private func syncSelection(in tableView: NSTableView) {
            if let selectedId, let row = rowById[selectedId] {
                if tableView.selectedRow != row {
                    tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
                }
            } else if tableView.selectedRow >= 0 {
                tableView.deselectAll(nil)
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
            container.updateFloatingButton(isVisible: !isAtBottom && !rows.isEmpty, newRecordsCount: newRecordsCount)
        }

        @objc private func scrollToBottom() {
            guard let container else {
                return
            }
            container.tableView.scrollRowToVisible(max(0, rows.count - 1))
            newRecordsCount = 0
            refreshScrollState()
        }

        private static func indexRows(_ records: [TrafficRecordSummary]) -> [String: Int] {
            var result: [String: Int] = [:]
            result.reserveCapacity(records.count)
            for (offset, record) in records.enumerated() {
                result[record.id] = offset
            }
            return result
        }

        private static func indexRowsById(_ rows: [TrafficRowViewModel]) -> [String: TrafficRowViewModel] {
            var result: [String: TrafficRowViewModel] = [:]
            result.reserveCapacity(rows.count)
            for row in rows {
                result[row.id] = row
            }
            return result
        }

        fileprivate static func deduplicatedRecords(_ records: [TrafficRecordSummary]) -> [TrafficRecordSummary] {
            var indexById: [String: Int] = [:]
            var result: [TrafficRecordSummary] = []
            result.reserveCapacity(records.count)
            for record in records {
                if let index = indexById[record.id] {
                    result[index] = record
                } else {
                    indexById[record.id] = result.count
                    result.append(record)
                }
            }
            return result
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

        init?(identifier: NSUserInterfaceItemIdentifier) {
            self.init(rawValue: identifier.rawValue)
        }

        var identifier: NSUserInterfaceItemIdentifier {
            NSUserInterfaceItemIdentifier(rawValue)
        }

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

private struct TrafficRowViewModel: Equatable {
    let id: String
    let sequenceText: String
    let dotColor: NSColor
    let protocolText: String
    let protocolPalette: TagPalette
    let methodText: String
    let methodPalette: TagPalette
    let statusText: String
    let statusPalette: TagPalette?
    let clientText: String
    let clientTooltip: String
    let clientApp: String?
    let portText: String
    let hasRuleHit: Bool
    let ruleCountText: String
    let ruleTooltip: String
    let hostText: String
    let pathText: String
    let typeText: String
    let sizeText: String
    let timeText: String
    let timeColor: NSColor

    init(record: TrafficRecordSummary) {
        id = record.id
        sequenceText = Self.formatSequence(record.seq)
        dotColor = Self.statusDotColor(for: record)
        protocolText = Self.displayProtocol(for: record)
        protocolPalette = record.isH3 ? .purple : .default
        methodText = record.method ?? "-"
        methodPalette = Self.methodPalette(record.method)
        if let status = record.status, status > 0 {
            statusText = String(status)
            statusPalette = Self.statusPalette(status)
        } else {
            statusText = "-"
            statusPalette = nil
        }
        clientText = record.clientDisplay
        clientTooltip = record.clientTooltip
        clientApp = record.clientApp?.isEmpty == false ? record.clientApp : nil
        portText = record.listenerPort.map(String.init) ?? "-"
        hasRuleHit = record.hasRuleHit
        ruleCountText = String(max(1, record.matchedRuleCount ?? 1))
        ruleTooltip = Self.ruleTooltip(record)
        hostText = record.host ?? "-"
        pathText = record.path?.isEmpty == false ? record.path ?? "" : "/"
        typeText = Self.contentTypeShort(for: record)
        sizeText = Self.formatBytes(record.responseSize ?? 0)
        timeText = Self.formatDuration(record.durationMs)
        timeColor = (record.durationMs ?? 0) > 1000 ? NSColor(hex: 0xfaad14) : .secondaryLabelColor
    }

    private static func formatSequence(_ sequence: Int?) -> String {
        guard let sequence else {
            return "-"
        }
        let raw = String(sequence)
        let suffix = raw.count > 5 ? String(raw.suffix(5)) : raw
        return String(repeating: "0", count: max(0, 5 - suffix.count)) + suffix
    }

    private static func displayProtocol(for record: TrafficRecordSummary) -> String {
        if record.isH3 {
            return "H3"
        }
        guard let protocolName = record.protocolName, !protocolName.isEmpty else {
            return "-"
        }
        return protocolName.replacingOccurrences(of: "HTTP/", with: "").uppercased()
    }

    private static func contentTypeShort(for record: TrafficRecordSummary) -> String {
        guard let contentType = record.contentType, !contentType.isEmpty else {
            return "-"
        }
        let primary = contentType.split(separator: ";", maxSplits: 1).first.map(String.init) ?? contentType
        return primary.split(separator: "/").last.map(String.init) ?? primary
    }

    private static func formatBytes(_ value: Int) -> String {
        if value <= 0 { return "-" }
        if value < 1024 { return "\(value) B" }
        if value < 1024 * 1024 { return String(format: "%.1f KB", Double(value) / 1024) }
        return String(format: "%.1f MB", Double(value) / 1024 / 1024)
    }

    private static func formatDuration(_ value: Int?) -> String {
        guard let value, value > 0 else { return "-" }
        if value < 1000 { return "\(value)ms" }
        if value < 60_000 { return String(format: "%.1fs", Double(value) / 1000) }
        return String(format: "%.1fm", Double(value) / 60_000)
    }

    private static func methodPalette(_ method: String?) -> TagPalette {
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

    private static func statusPalette(_ status: Int) -> TagPalette {
        if status >= 500 { return .red }
        if status >= 400 { return .orange }
        if status >= 300 { return .blue }
        if status >= 200 { return .green }
        return .default
    }

    private static func statusDotColor(for record: TrafficRecordSummary) -> NSColor {
        if record.method?.uppercased() == "CONNECT", (record.status ?? 0) == 0 {
            return .systemGreen
        }
        guard let status = record.status else { return NSColor(hex: 0xd9d9d9) }
        if status == 0 { return NSColor(hex: 0xd9d9d9) }
        if status >= 100 && status < 200 { return NSColor(hex: 0x73d13d) }
        if status >= 200 && status < 300 { return NSColor(hex: 0x52c41a) }
        if status >= 300 && status < 400 { return NSColor(hex: 0xfaad14) }
        if status >= 400 && status < 500 { return NSColor(hex: 0xfa8c16) }
        if status >= 500 { return NSColor(hex: 0xf5222d) }
        return NSColor(hex: 0xd9d9d9)
    }

    private static func ruleTooltip(_ record: TrafficRecordSummary) -> String {
        let count = max(1, record.matchedRuleCount ?? 1)
        guard !record.matchedProtocols.isEmpty else {
            return "\(count) rule(s) matched"
        }
        return "\(count) rule(s) matched: \(record.matchedProtocols.joined(separator: ", "))"
    }
}

private final class TrafficCellView: NSTableCellView {
    private let column: RequestTableView.Column
    private var row: TrafficRowViewModel?

    init(column: RequestTableView.Column, identifier: NSUserInterfaceItemIdentifier) {
        self.column = column
        super.init(frame: .zero)
        self.identifier = identifier
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override var isFlipped: Bool {
        true
    }

    func configure(row: TrafficRowViewModel) {
        guard self.row != row else {
            return
        }
        self.row = row
        toolTip = tooltip(for: row)
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let row else {
            return
        }
        switch column {
        case .seq:
            drawText(row.sequenceText, in: bounds.insetBy(dx: 8, dy: 0), font: .monospacedSystemFont(ofSize: 11, weight: .regular), color: .secondaryLabelColor, alignment: .right)
        case .dot:
            drawDot(color: row.dotColor)
        case .proto:
            drawTag(row.protocolText, palette: row.protocolPalette)
        case .method:
            drawTag(row.methodText, palette: row.methodPalette)
        case .status:
            if let palette = row.statusPalette {
                drawTag(row.statusText, palette: palette)
            } else {
                drawText(row.statusText, in: bounds, color: .secondaryLabelColor, alignment: .center)
            }
        case .client:
            drawClient(row)
        case .port:
            if row.portText == "-" {
                drawText("-", in: bounds, color: .secondaryLabelColor, alignment: .center)
            } else {
                drawTag(row.portText, palette: .default)
            }
        case .rules:
            drawRules(row)
        case .host:
            drawText(row.hostText, in: bounds.insetBy(dx: 8, dy: 0), color: .secondaryLabelColor)
        case .path:
            drawText(row.pathText, in: bounds.insetBy(dx: 8, dy: 0), color: .secondaryLabelColor)
        case .type:
            drawText(row.typeText, in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: .secondaryLabelColor)
        case .size:
            drawText(row.sizeText, in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: .secondaryLabelColor, alignment: .right)
        case .time:
            drawText(row.timeText, in: bounds.insetBy(dx: 8, dy: 0), font: .systemFont(ofSize: 11), color: row.timeColor, alignment: .right)
        }
    }

    private func tooltip(for row: TrafficRowViewModel) -> String? {
        switch column {
        case .client:
            return row.clientTooltip
        case .rules:
            return row.hasRuleHit ? row.ruleTooltip : nil
        case .host:
            return row.hostText
        case .path:
            return row.pathText
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

    private func drawClient(_ row: TrafficRowViewModel) {
        var x: CGFloat = 8
        if let app = row.clientApp {
            let iconRect = NSRect(x: x, y: bounds.midY - 7, width: 14, height: 14)
            let image = AppIconCache.shared.image(for: app) ?? AppIconCache.placeholder
            image.draw(in: iconRect, from: .zero, operation: .sourceOver, fraction: 1)
            x += 18
        }
        drawText(row.clientText, in: NSRect(x: x, y: 0, width: max(0, bounds.width - x - 8), height: bounds.height), font: .systemFont(ofSize: 11), color: .secondaryLabelColor)
    }

    private func drawRules(_ row: TrafficRowViewModel) {
        guard row.hasRuleHit else {
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

        let bubbleRect = NSRect(
            x: bounds.midX - 1,
            y: bounds.midY - 8,
            width: max(14, CGFloat(row.ruleCountText.count * 7 + 8)),
            height: 14
        )
        blue.setFill()
        NSBezierPath(roundedRect: bubbleRect, xRadius: 7, yRadius: 7).fill()
        drawText(row.ruleCountText, in: bubbleRect.offsetBy(dx: 0, dy: -1), font: .systemFont(ofSize: 9, weight: .semibold), color: .white, alignment: .center)
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

private struct TagPalette: Equatable {
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
        background = NSColor(hex: hexBackground)
        border = NSColor(hex: hexBorder)
        text = NSColor(hex: hexText)
    }
}

private final class AppIconCache {
    static let shared = AppIconCache()
    static let didUpdateNotification = Notification.Name("BifrostAppIconCacheDidUpdate")
    static let placeholder: NSImage = NSImage(systemSymbolName: "app", accessibilityDescription: nil) ?? NSImage(size: NSSize(width: 14, height: 14))

    private let images = NSCache<NSString, NSImage>()
    private let lock = NSLock()
    private var misses = Set<String>()
    private var pending = Set<String>()

    func image(for appName: String) -> NSImage? {
        if let cached = images.object(forKey: appName as NSString) {
            return cached
        }
        if shouldScheduleLoad(appName: appName) {
            resolveAsync(appName: appName)
        }
        return nil
    }

    private func shouldScheduleLoad(appName: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if misses.contains(appName) {
            return false
        }
        if pending.contains(appName) {
            return false
        }
        pending.insert(appName)
        return true
    }

    private func resolveAsync(appName: String) {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else {
                return
            }
            let appPath = self.candidateAppPaths(appName: appName).first { FileManager.default.fileExists(atPath: $0) }
            DispatchQueue.main.async {
                let icon = appPath.map { path -> NSImage in
                    let image = NSWorkspace.shared.icon(forFile: path)
                    image.isTemplate = false
                    return image
                }
                self.lock.lock()
                self.pending.remove(appName)
                if let icon {
                    self.images.setObject(icon, forKey: appName as NSString)
                } else {
                    self.misses.insert(appName)
                }
                self.lock.unlock()
                NotificationCenter.default.post(name: Self.didUpdateNotification, object: appName)
            }
        }
    }

    private func candidateAppPaths(appName: String) -> [String] {
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
            guard let entries = try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else {
                continue
            }
            for entry in entries where entry.pathExtension == "app" {
                let stem = normalized(entry.deletingPathExtension().lastPathComponent)
                if names.map(normalized).contains(where: { stem == $0 || stem.contains($0) || $0.contains(stem) }) {
                    paths.append(entry.path)
                }
            }
        }
        return Array(NSOrderedSet(array: paths)) as? [String] ?? paths
    }

    private func normalizedAppNameCandidates(_ name: String) -> [String] {
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

    private func normalized(_ value: String) -> String {
        value.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
    }
}

private extension TrafficRecordSummary {
    var isH3: Bool {
        protocolName == "h3" || protocolName == "h3s" || ((flags ?? 0) & (1 << 3)) != 0
    }

    var clientDisplay: String {
        if let app = clientApp, !app.isEmpty { return app }
        if let ip = clientIp, !ip.isEmpty { return ip }
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

enum TrafficTablePerformanceSmoke {
    static func run() {
        let baseCount = 100_000
        let appendCount = 1_000
        let updateStride = 97

        let baseRecords = makeRecords(count: baseCount, start: 0)
        let buildStart = DispatchTime.now()
        var rows = baseRecords.map(TrafficRowViewModel.init)
        let buildMs = elapsedMs(since: buildStart)

        let appendRecords = makeRecords(count: appendCount, start: baseCount)
        let appendStart = DispatchTime.now()
        rows.append(contentsOf: appendRecords.map(TrafficRowViewModel.init))
        let appendMs = elapsedMs(since: appendStart)

        var updatedRecords = baseRecords
        let updateStart = DispatchTime.now()
        var changedRows = IndexSet()
        for index in stride(from: 0, to: baseCount, by: updateStride) {
            updatedRecords[index] = TrafficRecordSummary(
                id: updatedRecords[index].id,
                seq: updatedRecords[index].seq,
                method: "POST",
                host: updatedRecords[index].host,
                path: updatedRecords[index].path,
                status: 204,
                contentType: "application/json",
                responseSize: 2048,
                durationMs: 84,
                listenerPort: 9900,
                protocolName: "https",
                clientApp: "Synthetic",
                clientIp: "127.0.0.1",
                startTime: updatedRecords[index].startTime,
                endTime: updatedRecords[index].endTime,
                flags: updatedRecords[index].flags,
                matchedRuleCount: 1,
                matchedProtocols: ["bp"]
            )
            let next = TrafficRowViewModel(record: updatedRecords[index])
            if rows[index] != next {
                rows[index] = next
                changedRows.insert(index)
            }
        }
        let updateMs = elapsedMs(since: updateStart)
        let duplicateRecords = [
            makeRecord(index: 1, method: "GET", status: 200),
            makeRecord(index: 2, method: "POST", status: 201),
            makeRecord(index: 1, method: "PATCH", status: 204),
        ]
        let deduplicatedRecords = RequestTableView.Coordinator.deduplicatedRecords(duplicateRecords)

        guard rows.count == baseCount + appendCount,
              changedRows.count == Int(ceil(Double(baseCount) / Double(updateStride))),
              deduplicatedRecords.count == 2,
              deduplicatedRecords.first?.id == "synthetic-1",
              deduplicatedRecords.first?.method == "PATCH" else {
            fputs("Traffic table performance smoke failed: row bookkeeping mismatch\n", stderr)
            Foundation.exit(1)
        }

        print(
            "Traffic table performance smoke passed: " +
            "base_rows=\(baseCount) append_rows=\(appendCount) " +
            "changed_rows=\(changedRows.count) " +
            "build_ms=\(String(format: "%.2f", buildMs)) " +
            "append_ms=\(String(format: "%.2f", appendMs)) " +
            "update_ms=\(String(format: "%.2f", updateMs))"
        )
        Foundation.exit(0)
    }

    private static func makeRecords(count: Int, start: Int) -> [TrafficRecordSummary] {
        (start..<(start + count)).map { index in
            makeRecord(index: index)
        }
    }

    private static func makeRecord(
        index: Int,
        method: String? = nil,
        status: Int? = nil
    ) -> TrafficRecordSummary {
        TrafficRecordSummary(
            id: "synthetic-\(index)",
            seq: index,
            method: method ?? (index % 5 == 0 ? "POST" : "GET"),
            host: "api\(index % 64).example.test",
            path: "/v1/items/\(index)",
            status: status ?? (index % 17 == 0 ? 204 : 200),
            contentType: "application/json; charset=utf-8",
            responseSize: 512 + index % 4096,
            durationMs: 30 + index % 1200,
            listenerPort: 9900,
            protocolName: index % 11 == 0 ? "tunnel" : "https",
            clientApp: index % 7 == 0 ? "Synthetic Helper" : "Microsoft Edge Helper",
            clientIp: "127.0.0.1",
            startTime: "2026-06-30T00:00:00Z",
            endTime: "2026-06-30T00:00:01Z",
            flags: 1 << 4,
            matchedRuleCount: index % 3 + 1,
            matchedProtocols: ["bp"]
        )
    }

    private static func elapsedMs(since start: DispatchTime) -> Double {
        let end = DispatchTime.now()
        return Double(end.uptimeNanoseconds - start.uptimeNanoseconds) / 1_000_000
    }
}
