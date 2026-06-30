import AppKit
import BifrostNativeCore
import SwiftUI

struct ValuesView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var searchText = ""
    @State private var createSheetVisible = false
    @State private var renameSheetVisible = false
    @State private var deleteAlertVisible = false
    @State private var copyFeedback = ""

    private var filteredValues: [ValueItem] {
        let keyword = searchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        let sortedValues = appModel.values.sorted {
            let leftTime = Date.parseISO8601($0.createdAt) ?? .distantPast
            let rightTime = Date.parseISO8601($1.createdAt) ?? .distantPast
            if leftTime != rightTime {
                return leftTime > rightTime
            }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
        guard !keyword.isEmpty else {
            return sortedValues
        }
        return sortedValues.filter {
            $0.name.localizedLowercase.contains(keyword)
                || $0.value.localizedLowercase.contains(keyword)
        }
    }

    private var selectedValue: ValueItem? {
        guard let selectedValueName = appModel.selectedValueName else {
            return nil
        }
        return appModel.values.first { $0.name == selectedValueName }
    }

    private var hasUnsavedChanges: Bool {
        guard let selectedValue else {
            return false
        }
        return appModel.selectedValueDraft != selectedValue.value
    }

    private var canFormat: Bool {
        detectedContentKind(appModel.selectedValueDraft) != .plain
    }

    var body: some View {
        HStack(spacing: 0) {
            listPane
                .frame(width: 300)
                .background(.background)

            Divider()

            detailPane
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .sheet(isPresented: $createSheetVisible) {
            NameEntrySheet(
                title: "New Value",
                prompt: "Value name",
                initialValue: "",
                confirmTitle: "Create"
            ) { name in
                Task { await appModel.createValue(name: name) }
            }
        }
        .sheet(isPresented: $renameSheetVisible) {
            NameEntrySheet(
                title: "Rename Value",
                prompt: "Value name",
                initialValue: appModel.selectedValueName ?? "",
                confirmTitle: "Rename"
            ) { name in
                Task { await appModel.renameSelectedValue(to: name) }
            }
        }
        .alert("Delete Value", isPresented: $deleteAlertVisible) {
            Button("Delete", role: .destructive) {
                Task { await appModel.deleteSelectedValue() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Delete \"\(appModel.selectedValueName ?? "")\"? This cannot be undone.")
        }
    }

    private var listPane: some View {
        VStack(spacing: 0) {
            listHeader
            Divider()
            if filteredValues.isEmpty {
                ValuesEmptyStateView(title: appModel.values.isEmpty ? "No values" : "No matching values")
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(filteredValues) { value in
                            ValueRow(
                                value: value,
                                isSelected: appModel.selectedValueName == value.name
                            ) {
                                appModel.selectValue(value.name)
                            }
                        }
                    }
                }
            }
        }
    }

    private var detailPane: some View {
        VStack(spacing: 0) {
            detailHeader
            Divider()
            if selectedValue != nil {
                CodeEditorView(text: $appModel.selectedValueDraft)
            } else {
                ValuesEmptyStateView(title: "Select a value to edit")
            }
        }
    }

    private var listHeader: some View {
        VStack(spacing: 8) {
            HStack {
                Text("Values")
                    .font(.system(size: 14, weight: .semibold))
                Spacer()
                Text("\(appModel.values.count)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(.quaternary, in: Capsule())
                Button {
                    createSheetVisible = true
                } label: {
                    Image(systemName: "plus")
                }
                .buttonStyle(.borderless)
                .help("New Value")
                Button {
                    Task { await appModel.refreshData() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .help("Refresh Values")
            }

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search values...", text: $searchText)
                    .textFieldStyle(.plain)
            }
            .font(.system(size: 12))
        }
        .padding(10)
        .frame(height: 74)
        .background(.bar)
    }

    private var detailHeader: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(selectedValue?.name ?? "Value Detail")
                        .font(.system(size: 13, weight: .semibold))
                    if hasUnsavedChanges {
                        Circle()
                            .fill(Color.orange)
                            .frame(width: 7, height: 7)
                            .help("Unsaved changes")
                    }
                }
                Text(valueSubtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if !copyFeedback.isEmpty {
                Text(copyFeedback)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Button {
                appModel.selectedValueDraft = formattedContent(appModel.selectedValueDraft)
            } label: {
                Label("Format", systemImage: "wand.and.stars")
            }
            .buttonStyle(.borderless)
            .disabled(!canFormat || appModel.isSavingValue)

            Button {
                copyToPasteboard(appModel.selectedValueDraft)
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
            .buttonStyle(.borderless)
            .disabled(selectedValue == nil)

            Button {
                appModel.selectedValueDraft = selectedValue?.value ?? ""
            } label: {
                Label("Revert", systemImage: "arrow.uturn.backward")
            }
            .buttonStyle(.borderless)
            .disabled(!hasUnsavedChanges || appModel.isSavingValue)

            Button {
                Task { await appModel.saveSelectedValue(content: appModel.selectedValueDraft) }
            } label: {
                Label("Save", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .disabled(!hasUnsavedChanges || appModel.isSavingValue)

            Menu {
                Button("Rename") { renameSheetVisible = true }
                Button("Delete", role: .destructive) { deleteAlertVisible = true }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .disabled(selectedValue == nil || appModel.isSavingValue)
        }
        .padding(.horizontal, 12)
        .frame(height: 52)
        .background(.bar)
    }

    private var valueSubtitle: String {
        guard let selectedValue else {
            return "Loaded from Admin API"
        }
        return selectedValue.updatedAt ?? "updated time unavailable"
    }

    private func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
        copyFeedback = "Copied"
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
            copyFeedback = ""
        }
    }
}

private struct ValueRow: View {
    let value: ValueItem
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                Text(value.name)
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                Text(value.value.isEmpty ? "Empty value" : value.value)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(isSelected ? Color.accentColor.opacity(0.12) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct ValuesEmptyStateView: View {
    let title: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "server.rack")
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private enum ValueContentKind {
    case json
    case xml
    case plain
}

private func detectedContentKind(_ content: String) -> ValueContentKind {
    let trimmed = content.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.hasPrefix("{") || trimmed.hasPrefix("[") {
        if JSONSerialization.isValidJSONObjectFromString(trimmed) {
            return .json
        }
    }
    if trimmed.hasPrefix("<"), trimmed.hasSuffix(">") {
        return .xml
    }
    return .plain
}

private func formattedContent(_ content: String) -> String {
    switch detectedContentKind(content) {
    case .json:
        guard let data = content.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let formattedData = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]),
              let formatted = String(data: formattedData, encoding: .utf8) else {
            return content
        }
        return formatted
    case .xml:
        return formatXML(content)
    case .plain:
        return content
    }
}

private func formatXML(_ content: String) -> String {
    let fragments = content.split(separator: ">", omittingEmptySubsequences: false)
    var indent = 0
    var lines: [String] = []
    for fragment in fragments {
        let node = fragment.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !node.isEmpty else {
            continue
        }
        let restored = node.hasPrefix("<") ? "\(node)>" : "<\(node)>"
        if restored.hasPrefix("</") {
            indent = max(0, indent - 1)
        }
        lines.append(String(repeating: "  ", count: indent) + restored)
        if restored.hasPrefix("<"),
           !restored.hasPrefix("</"),
           !restored.hasPrefix("<?"),
           !restored.hasPrefix("<!"),
           !restored.hasSuffix("/>") {
            indent += 1
        }
    }
    return lines.isEmpty ? content : lines.joined(separator: "\n")
}

private extension JSONSerialization {
    static func isValidJSONObjectFromString(_ string: String) -> Bool {
        guard let data = string.data(using: .utf8) else {
            return false
        }
        return (try? jsonObject(with: data)) != nil
    }
}

private extension Date {
    static func parseISO8601(_ value: String?) -> Date? {
        guard let value else {
            return nil
        }
        return ISO8601DateFormatter().date(from: value)
    }
}
