import AppKit
import SwiftUI

struct RequestTableView: NSViewRepresentable {
    var records: [TrafficRecord]

    func makeCoordinator() -> Coordinator {
        Coordinator(records: records)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let tableView = NSTableView()
        tableView.usesAlternatingRowBackgroundColors = true
        tableView.headerView = NSTableHeaderView()
        tableView.delegate = context.coordinator
        tableView.dataSource = context.coordinator
        tableView.columnAutoresizingStyle = .uniformColumnAutoresizingStyle

        for column in Column.allCases {
            let tableColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(column.rawValue))
            tableColumn.title = column.title
            tableColumn.width = column.width
            tableView.addTableColumn(tableColumn)
        }

        let scrollView = NSScrollView()
        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        return scrollView
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        context.coordinator.records = records
        (nsView.documentView as? NSTableView)?.reloadData()
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var records: [TrafficRecord]

        init(records: [TrafficRecord]) {
            self.records = records
        }

        func numberOfRows(in tableView: NSTableView) -> Int {
            records.count
        }

        func tableView(
            _ tableView: NSTableView,
            viewFor tableColumn: NSTableColumn?,
            row: Int
        ) -> NSView? {
            guard row < records.count else { return nil }
            let record = records[row]
            let identifier = tableColumn?.identifier.rawValue ?? ""
            let text = Column(rawValue: identifier)?.value(for: record) ?? ""
            let cell = NSTextField(labelWithString: text)
            cell.lineBreakMode = .byTruncatingTail
            cell.font = .monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
            return cell
        }
    }

    enum Column: String, CaseIterable {
        case method
        case status
        case host
        case path
        case duration

        var title: String {
            switch self {
            case .method: return "Method"
            case .status: return "Status"
            case .host: return "Host"
            case .path: return "Path"
            case .duration: return "Time"
            }
        }

        var width: CGFloat {
            switch self {
            case .method, .status, .duration: return 90
            case .host: return 240
            case .path: return 520
            }
        }

        func value(for record: TrafficRecord) -> String {
            switch self {
            case .method: return record.method
            case .status: return record.status
            case .host: return record.host
            case .path: return record.path
            case .duration: return record.duration
            }
        }
    }
}
