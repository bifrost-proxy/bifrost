import AppKit
import BifrostNativeCore
import SwiftUI

struct ScriptsView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var searchText = ""
    @State private var createSheetVisible = false
    @State private var renameSheetVisible = false
    @State private var deleteAlertVisible = false
    @State private var copyFeedback = ""

    private var scripts: [ScriptInfo] {
        let source = appModel.scriptsByType[appModel.selectedScriptType] ?? []
        let keyword = searchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !keyword.isEmpty else {
            return source
        }
        return source.filter {
            $0.name.localizedLowercase.contains(keyword)
                || ($0.description ?? "").localizedLowercase.contains(keyword)
        }
    }

    private var hasUnsavedChanges: Bool {
        guard let detail = appModel.selectedScriptDetail else {
            return false
        }
        return appModel.selectedScriptDraft != detail.content
    }

    var body: some View {
        HStack(spacing: 0) {
            listPane
                .frame(width: 320)
                .background(.background)

            Divider()

            detailPane
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .sheet(isPresented: $createSheetVisible) {
            NameEntrySheet(
                title: "New \(appModel.selectedScriptType.label) Script",
                prompt: "Script name",
                initialValue: "",
                confirmTitle: "Create"
            ) { name in
                Task { await appModel.createScript(type: appModel.selectedScriptType, name: name) }
            }
        }
        .sheet(isPresented: $renameSheetVisible) {
            NameEntrySheet(
                title: "Rename Script",
                prompt: "Script name",
                initialValue: appModel.selectedScriptName ?? "",
                confirmTitle: "Rename"
            ) { name in
                Task { await appModel.renameSelectedScript(to: name) }
            }
        }
        .alert("Delete Script", isPresented: $deleteAlertVisible) {
            Button("Delete", role: .destructive) {
                Task { await appModel.deleteSelectedScript() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Delete \"\(appModel.selectedScriptName ?? "")\"? This cannot be undone.")
        }
        .task(id: appModel.selectedScriptName) {
            if let name = appModel.selectedScriptName {
                await appModel.selectScript(type: appModel.selectedScriptType, name: name)
            }
        }
    }

    private var listPane: some View {
        VStack(spacing: 0) {
            listHeader
            Divider()
            if scripts.isEmpty {
                ScriptsEmptyStateView(title: emptyTitle)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(scripts) { script in
                            ScriptRow(
                                script: script,
                                isSelected: appModel.selectedScriptType == script.scriptType
                                    && appModel.selectedScriptName == script.name
                            ) {
                                Task { await appModel.selectScript(type: script.scriptType, name: script.name) }
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
            if appModel.selectedScriptDetail != nil {
                CodeEditorView(text: $appModel.selectedScriptDraft)
            } else {
                ScriptsEmptyStateView(title: "Select a script to edit")
            }
        }
    }

    private var listHeader: some View {
        VStack(spacing: 8) {
            HStack {
                Text("Scripts")
                    .font(.system(size: 14, weight: .semibold))
                Spacer()
                Text("\((appModel.scriptsByType[appModel.selectedScriptType] ?? []).count)")
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
                .help("New Script")
                Button {
                    Task { await appModel.refreshData() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .help("Refresh Scripts")
            }

            Picker("", selection: Binding(
                get: { appModel.selectedScriptType },
                set: { type in Task { await appModel.selectScriptType(type) } }
            )) {
                ForEach(ScriptType.allCases) { type in
                    Text(type.label).tag(type)
                }
            }
            .pickerStyle(.segmented)

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search scripts...", text: $searchText)
                    .textFieldStyle(.plain)
            }
            .font(.system(size: 12))
        }
        .padding(10)
        .frame(height: 108)
        .background(.bar)
    }

    private var detailHeader: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(appModel.selectedScriptDetail?.name ?? "Script Detail")
                        .font(.system(size: 13, weight: .semibold))
                    if hasUnsavedChanges {
                        Circle()
                            .fill(Color.orange)
                            .frame(width: 7, height: 7)
                            .help("Unsaved changes")
                    }
                }
                Text(scriptSubtitle)
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
                copyToPasteboard(appModel.selectedScriptDraft)
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
            .buttonStyle(.borderless)
            .disabled(appModel.selectedScriptDetail == nil)

            Button {
                appModel.selectedScriptDraft = appModel.selectedScriptDetail?.content ?? ""
            } label: {
                Label("Revert", systemImage: "arrow.uturn.backward")
            }
            .buttonStyle(.borderless)
            .disabled(!hasUnsavedChanges || appModel.isSavingScript)

            Button {
                Task { await appModel.saveSelectedScript(content: appModel.selectedScriptDraft) }
            } label: {
                Label("Save", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .disabled(!hasUnsavedChanges || appModel.isSavingScript)

            Menu {
                Button("Rename") { renameSheetVisible = true }
                Button("Delete", role: .destructive) { deleteAlertVisible = true }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .disabled(appModel.selectedScriptDetail == nil || appModel.isSavingScript)
        }
        .padding(.horizontal, 12)
        .frame(height: 52)
        .background(.bar)
    }

    private var emptyTitle: String {
        (appModel.scriptsByType[appModel.selectedScriptType] ?? []).isEmpty
            ? "No \(appModel.selectedScriptType.label.lowercased()) scripts"
            : "No matching scripts"
    }

    private var scriptSubtitle: String {
        guard let detail = appModel.selectedScriptDetail else {
            return "Loaded from Admin API"
        }
        let updated = Date(timeIntervalSince1970: detail.updatedAt / 1000)
        return "\(detail.scriptType.label) · \(updated.formatted(date: .numeric, time: .standard))"
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

private struct ScriptRow: View {
    let script: ScriptInfo
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                Text(script.name)
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                Text(script.description?.isEmpty == false ? script.description! : script.scriptType.label)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
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

private struct ScriptsEmptyStateView: View {
    let title: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "terminal")
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
