import AppKit
import BifrostNativeCore
import SwiftUI

struct RulesView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var searchText = ""
    @State private var createSheetVisible = false
    @State private var deleteAlertVisible = false
    @State private var copyFeedback = ""
    @State private var autoSaveTask: Task<Void, Never>?
    @State private var autoSaveState = RuleAutoSaveState.saved
    @State private var inlineRenameRuleName: String?
    @State private var inlineRenameDraft = ""
    @State private var isCommittingInlineRename = false
    @FocusState private var inlineRenameFocused: Bool

    private let ruleListWidth: CGFloat = 300

    private var filteredRules: [RuleSummary] {
        let keyword = searchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        let sortedRules = appModel.sortedRules
        guard !keyword.isEmpty else {
            return sortedRules
        }
        return sortedRules.filter { $0.name.localizedLowercase.contains(keyword) }
    }

    private var hasUnsavedChanges: Bool {
        guard let detail = appModel.selectedRuleDetail else {
            return false
        }
        return appModel.ruleDraftContent != detail.content
    }

    private var isFiltering: Bool {
        !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var selectedRuleIsProtected: Bool {
        appModel.isDefaultRule(appModel.selectedRuleDetail?.name ?? appModel.selectedRuleName)
    }

    var body: some View {
        NativePageScaffold(title: "规则", contentFillsAvailableHeight: true) {
            Text("\(appModel.rules.filter(\.enabled).count)/\(appModel.rules.count) enabled")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 4)
                .background(AppSurface.subtleFill, in: Capsule())
            Spacer()
            Button {
                createSheetVisible = true
            } label: {
                Label("New Rule", systemImage: "plus")
            }
            .buttonStyle(.borderless)
            .font(.system(size: 13, weight: .medium))
        } content: {
            HStack(alignment: .top, spacing: 5) {
                NativePanel(scaleOnHover: 1.002, allowsHoverEffect: false) {
                    listPane
                }
                .frame(width: ruleListWidth)
                .frame(maxHeight: .infinity)

                NativePanel(scaleOnHover: 1.002, allowsHoverEffect: false) {
                    detailPane
                }
                .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
                .layoutPriority(1)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .sheet(isPresented: $createSheetVisible) {
            NameEntrySheet(
                title: "New Rule",
                prompt: "Rule name",
                initialValue: "",
                confirmTitle: "Create"
            ) { name in
                Task { await appModel.createRule(name: name) }
            }
        }
        .alert("Delete Rule", isPresented: $deleteAlertVisible) {
            Button("Delete", role: .destructive) {
                Task { await appModel.deleteSelectedRule() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Delete \"\(appModel.selectedRuleName ?? "")\"? This cannot be undone.")
        }
        .onChange(of: appModel.selectedRuleName) { _ in
            autoSaveTask?.cancel()
            autoSaveState = .saved
            cancelInlineRename()
        }
        .onChange(of: inlineRenameFocused) { focused in
            if !focused, inlineRenameRuleName != nil {
                commitInlineRename()
            }
        }
        .onDisappear {
            autoSaveTask?.cancel()
        }
    }

    private var listPane: some View {
        VStack(spacing: 0) {
            listHeader
            if filteredRules.isEmpty {
                RulesEmptyStateView(title: appModel.rules.isEmpty ? "No rules" : "No matching rules")
            } else if isFiltering {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ruleRows
                    }
                }
            } else {
                List {
                    ruleRows
                        .onMove { source, destination in
                            appModel.moveRules(from: source, to: destination)
                        }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
        }
    }

    private var ruleRows: some DynamicViewContent {
        ForEach(filteredRules) { rule in
            RuleRow(
                rule: rule,
                isSelected: appModel.selectedRuleName == rule.name,
                isBusy: appModel.isSavingRule,
                isProtected: appModel.isDefaultRule(rule.name),
                canReorder: !isFiltering && !appModel.isDefaultRule(rule.name)
            ) {
                Task { await appModel.selectRule(rule.name) }
            } toggle: { enabled in
                Task {
                    await appModel.selectRule(rule.name)
                    await appModel.setSelectedRuleEnabled(enabled)
                }
            }
            .listRowInsets(EdgeInsets())
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
        }
    }

    private var detailPane: some View {
        VStack(spacing: 0) {
            detailHeader
            if appModel.selectedRuleDetail != nil {
                CodeEditorView(
                    text: $appModel.ruleDraftContent,
                    isReadOnly: appModel.isSavingRule,
                    onSave: {
                        saveDraftImmediately()
                    },
                    onTextChanged: { text in
                        scheduleAutoSave(text)
                    }
                )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                RulesEmptyStateView(title: "Select a rule to edit")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var listHeader: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search rules...", text: $searchText)
                    .textFieldStyle(.plain)
            }
            .font(.system(size: 12))
            .padding(.horizontal, 12)
            .frame(height: 42)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
            .padding(12)
        }
    }

    private var detailHeader: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    inlineRuleTitle
                    if hasUnsavedChanges {
                        Circle()
                            .fill(Color.orange)
                            .frame(width: 7, height: 7)
                            .help("Unsaved changes")
                    }
                }
                Text(detailSubtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if !copyFeedback.isEmpty {
                Text(copyFeedback)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            } else if appModel.selectedRuleDetail != nil {
                Text(autoSaveStatusText)
                    .font(.system(size: 11))
                    .foregroundStyle(autoSaveStatusColor)
            }
            Toggle("Enabled", isOn: Binding(
                get: { appModel.selectedRuleDetail?.enabled ?? false },
                set: { enabled in Task { await appModel.setSelectedRuleEnabled(enabled) } }
            ))
            .toggleStyle(.switch)
            .font(.system(size: 11))
            .disabled(appModel.selectedRuleDetail == nil || appModel.isSavingRule || selectedRuleIsProtected)
            .help(selectedRuleIsProtected ? "Default rule is always enabled" : "Toggle rule")

            Button {
                copyToPasteboard(appModel.ruleDraftContent)
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
            .buttonStyle(.borderless)
            .disabled(appModel.selectedRuleDetail == nil)

            Menu {
                Button("Rename") { beginInlineRename() }
                    .disabled(selectedRuleIsProtected)
                Button("Delete", role: .destructive) { deleteAlertVisible = true }
                    .disabled(selectedRuleIsProtected)
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .disabled(appModel.selectedRuleDetail == nil || appModel.isSavingRule)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    @ViewBuilder
    private var inlineRuleTitle: some View {
        if inlineRenameRuleName != nil {
            TextField("Rule name", text: $inlineRenameDraft)
                .textFieldStyle(.plain)
                .font(.system(size: 14, weight: .semibold))
                .focused($inlineRenameFocused)
                .disabled(isCommittingInlineRename)
                .frame(minWidth: 180, idealWidth: 280, maxWidth: 420, alignment: .leading)
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
                .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(AppSurface.cardBorder)
                )
                .onSubmit {
                    commitInlineRename()
                }
                .onExitCommand {
                    cancelInlineRename()
                }
        } else {
            Text(appModel.selectedRuleDetail?.name ?? "Rule Detail")
                .font(.system(size: 14, weight: .semibold))
                .lineLimit(1)
                .contentShape(Rectangle())
                .onTapGesture(count: 2) {
                    beginInlineRename()
                }
                .help(selectedRuleIsProtected ? "Default rule cannot be renamed" : "Double-click to rename")
        }
    }

    private var detailSubtitle: String {
        guard let detail = appModel.selectedRuleDetail else {
            return "Loaded from Admin API"
        }
        let state = detail.enabled ? "Enabled" : "Disabled"
        return "\(state) · \(detail.updatedAt ?? "updated time unavailable")"
    }

    private func beginInlineRename() {
        guard let name = appModel.selectedRuleDetail?.name,
              !appModel.isDefaultRule(name),
              !appModel.isSavingRule else {
            return
        }
        inlineRenameRuleName = name
        inlineRenameDraft = name
        DispatchQueue.main.async {
            inlineRenameFocused = true
        }
    }

    private func cancelInlineRename() {
        inlineRenameRuleName = nil
        inlineRenameDraft = ""
        isCommittingInlineRename = false
        inlineRenameFocused = false
    }

    private func commitInlineRename() {
        guard let originalName = inlineRenameRuleName,
              !isCommittingInlineRename else {
            return
        }
        let trimmed = inlineRenameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != originalName else {
            cancelInlineRename()
            return
        }
        isCommittingInlineRename = true
        Task { @MainActor in
            await appModel.renameSelectedRule(to: trimmed)
            cancelInlineRename()
        }
    }

    private func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
        copyFeedback = "Copied"
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
            copyFeedback = ""
        }
    }

    private var autoSaveStatusText: String {
        if appModel.isAutoSavingRule || autoSaveState == .saving {
            return "Saving..."
        }
        switch autoSaveState {
        case .saved:
            return hasUnsavedChanges ? "Pending" : "Saved"
        case .pending:
            return "Pending"
        case .saving:
            return "Saving..."
        case .failed:
            return "Save failed"
        }
    }

    private var autoSaveStatusColor: Color {
        switch autoSaveState {
        case .failed:
            return .red
        case .pending:
            return .orange
        case .saving:
            return .secondary
        case .saved:
            return .secondary
        }
    }

    private func scheduleAutoSave(_ content: String) {
        guard let name = appModel.selectedRuleName else {
            return
        }
        autoSaveTask?.cancel()
        guard appModel.selectedRuleDetail?.content != content else {
            autoSaveState = .saved
            return
        }
        autoSaveState = .pending
        autoSaveTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 800_000_000)
            guard !Task.isCancelled, appModel.selectedRuleName == name else {
                return
            }
            await saveDraft(name: name, content: content)
        }
    }

    private func saveDraftImmediately() {
        guard let name = appModel.selectedRuleName else {
            return
        }
        autoSaveTask?.cancel()
        let content = appModel.ruleDraftContent
        Task { @MainActor in
            await saveDraft(name: name, content: content)
        }
    }

    private func saveDraft(name: String, content: String) async {
        guard appModel.selectedRuleName == name else {
            return
        }
        guard appModel.selectedRuleDetail?.content != content else {
            autoSaveState = .saved
            return
        }
        autoSaveState = .saving
        await appModel.autosaveSelectedRule(name: name, content: content)
        if appModel.selectedRuleDetail?.content == content {
            autoSaveState = .saved
        } else {
            autoSaveState = .failed
        }
    }
}

private struct RuleRow: View {
    let rule: RuleSummary
    let isSelected: Bool
    let isBusy: Bool
    let isProtected: Bool
    let canReorder: Bool
    let action: () -> Void
    let toggle: (Bool) -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Circle()
                    .fill(rule.enabled ? Color.green : Color.secondary.opacity(0.35))
                    .frame(width: 7, height: 7)
                VStack(alignment: .leading, spacing: 3) {
                    Text(rule.name)
                        .font(.system(size: 12, weight: .semibold))
                        .lineLimit(1)
                    Text("\(rule.ruleCount ?? 0) entries")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if isProtected {
                    Image(systemName: "lock.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .help("Default rule is fixed and always enabled")
                } else {
                    Toggle("", isOn: Binding(
                        get: { rule.enabled },
                        set: toggle
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .scaleEffect(0.72)
                    .disabled(isBusy)
                }
                if canReorder {
                    Image(systemName: "line.3.horizontal")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.tertiary)
                        .help("Drag to reorder")
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 50)
            .background(
                isSelected ? AppSurface.sidebarSelection : Color.clear,
                in: RoundedRectangle(cornerRadius: 7, style: .continuous)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private enum RuleAutoSaveState {
    case saved
    case pending
    case saving
    case failed
}

private struct RulesEmptyStateView: View {
    let title: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "doc.text.magnifyingglass")
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
