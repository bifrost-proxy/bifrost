import AppKit
import BifrostNativeCore
import SwiftUI
import UniformTypeIdentifiers

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
    @State private var draggingRuleName: String?
    @State private var groupPickerVisible = false
    @State private var groupSearchText = ""
    @State private var groupSearchResults: [RuleGroup]?
    @State private var groupSearchTask: Task<Void, Never>?
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
            .disabled(!appModel.canCreateRuleInCurrentScope)
            .help(appModel.canCreateRuleInCurrentScope ? "Create rule" : "Current rule list is read-only")
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
        .onChange(of: appModel.selectedRuleGroupID) { _ in
            searchText = ""
            groupSearchText = ""
            groupSearchResults = nil
            autoSaveTask?.cancel()
            autoSaveState = .saved
            cancelInlineRename()
        }
        .onChange(of: inlineRenameFocused) { focused in
            if !focused, inlineRenameRuleName != nil {
                commitInlineRename()
            }
        }
        .task {
            await appModel.refreshRuleGroups()
        }
        .onDisappear {
            autoSaveTask?.cancel()
            groupSearchTask?.cancel()
        }
    }

    private var listPane: some View {
        VStack(spacing: 0) {
            listHeader
            if filteredRules.isEmpty {
                RulesEmptyStateView(title: appModel.rules.isEmpty ? "No rules" : "No matching rules")
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ruleRows
                    }
                }
            }
        }
    }

    private var ruleRows: some View {
        ForEach(filteredRules) { rule in
            ruleRow(rule)
        }
    }

    @ViewBuilder
    private func ruleRow(_ rule: RuleSummary) -> some View {
        let ruleName = rule.name
        let isDefault = appModel.isDefaultRule(ruleName)
        let isReadOnly = !appModel.canEditCurrentRuleScope
        RuleRow(
            rule: rule,
            isSelected: appModel.selectedRuleName == ruleName,
            isBusy: appModel.isSavingRule,
            isProtected: isDefault || isReadOnly,
            protectedHelp: isDefault ? "Default rule is fixed and always enabled" : "Current rule list is read-only",
            canToggle: !isDefault && !isReadOnly && rule.canDisable != false,
            canReorder: !isFiltering && appModel.canReorderRule(rule),
            isDragging: draggingRuleName == ruleName
        ) {
            Task { await appModel.selectRule(ruleName) }
        } toggle: { enabled in
            Task {
                await appModel.selectRule(ruleName)
                await appModel.setSelectedRuleEnabled(enabled)
            }
        } onStartDrag: {
            draggingRuleName = ruleName
        }
        .onDrop(of: [.plainText], isTargeted: nil) { _, location in
            guard let draggingRuleName,
                  !isFiltering,
                  draggingRuleName != ruleName else {
                self.draggingRuleName = nil
                return false
            }
            appModel.moveRule(
                named: draggingRuleName,
                relativeTo: ruleName,
                placement: location.y > 25 ? .after : .before
            )
            self.draggingRuleName = nil
            return true
        }
    }

    private var detailPane: some View {
        VStack(spacing: 0) {
            detailHeader
            if appModel.selectedRuleDetail != nil {
                CodeEditorView(
                    text: $appModel.ruleDraftContent,
                    isReadOnly: appModel.isSavingRule || !appModel.canEditSelectedRuleContent,
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
            if appModel.canShowRuleGroupSwitcher {
                groupScopePicker
                    .padding(.horizontal, 12)
                    .padding(.top, 12)
                    .padding(.bottom, 6)
            }
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
            .padding(.horizontal, 12)
            .padding(.top, appModel.canShowRuleGroupSwitcher ? 0 : 12)
            .padding(.bottom, 12)
        }
    }

    private var groupScopePicker: some View {
        Button {
            groupPickerVisible.toggle()
        } label: {
            HStack(spacing: 8) {
                Text(appModel.ruleScopeTitle)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                Spacer()
                if appModel.isLoadingRuleGroups {
                    ProgressView()
                        .scaleEffect(0.48)
                        .frame(width: 16, height: 16)
                } else {
                    Image(systemName: "arrow.left.arrow.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 40)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .stroke(groupPickerVisible ? Color.accentColor.opacity(0.65) : AppSurface.cardBorder)
            )
        }
        .buttonStyle(.plain)
        .popover(isPresented: $groupPickerVisible, arrowEdge: .bottom) {
            ruleGroupPopover
                .frame(width: 280, height: 360)
        }
        .task {
            await appModel.refreshRuleGroups()
        }
    }

    private var filteredRuleGroups: [RuleGroup] {
        let keyword = groupSearchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        guard !keyword.isEmpty else {
            return appModel.sortedRuleGroups
        }
        if let groupSearchResults {
            return groupSearchResults
        }
        return appModel.sortedRuleGroups.filter { $0.name.localizedLowercase.contains(keyword) }
    }

    private func scheduleGroupSearch(_ text: String) {
        groupSearchTask?.cancel()
        let keyword = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !keyword.isEmpty else {
            groupSearchResults = nil
            return
        }
        groupSearchTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard !Task.isCancelled else {
                return
            }
            groupSearchResults = await appModel.searchRuleGroups(keyword: keyword)
        }
    }

    private var ruleGroupPopover: some View {
        VStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search groups...", text: $groupSearchText)
                    .textFieldStyle(.plain)
                    .onChange(of: groupSearchText) { value in
                        scheduleGroupSearch(value)
                    }
            }
            .font(.system(size: 12))
            .padding(.horizontal, 10)
            .frame(height: 34)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .padding(.horizontal, 10)
            .padding(.top, 10)

            ScrollView {
                LazyVStack(spacing: 4) {
                    RuleScopeOptionRow(
                        title: "My Rules",
                        badge: nil,
                        isSelected: appModel.selectedRuleGroupID == nil,
                        isWritable: true
                    ) {
                        groupPickerVisible = false
                        Task { await appModel.selectRuleScope(groupID: nil) }
                    }

                    ForEach(filteredRuleGroups) { group in
                        RuleScopeOptionRow(
                            title: group.name,
                            badge: group.permissionLabel,
                            isSelected: appModel.selectedRuleGroupID == group.id,
                            isWritable: group.isWritable
                        ) {
                            groupPickerVisible = false
                            Task { await appModel.selectRuleScope(groupID: group.id) }
                        }
                    }

                    if filteredRuleGroups.isEmpty {
                        Text("No matching groups")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, minHeight: 56)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 10)
            }
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
            .disabled(!appModel.canToggleSelectedRule || appModel.isSavingRule)
            .help(toggleHelp)

            Button {
                copyToPasteboard(appModel.ruleDraftContent)
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
            .buttonStyle(.borderless)
            .disabled(appModel.selectedRuleDetail == nil)

            Menu {
                Button("Rename") { beginInlineRename() }
                    .disabled(!appModel.canRenameSelectedRule)
                Button("Delete", role: .destructive) { deleteAlertVisible = true }
                    .disabled(!appModel.canDeleteSelectedRule)
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
                .help(appModel.canRenameSelectedRule ? "Double-click to rename" : "This rule cannot be renamed")
        }
    }

    private var toggleHelp: String {
        if appModel.selectedRuleDetail == nil {
            return "Select a rule"
        }
        if selectedRuleIsProtected {
            return "Default rule is always enabled"
        }
        if !appModel.canEditCurrentRuleScope {
            return "Current rule list is read-only"
        }
        return "Toggle rule"
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
              appModel.canRenameSelectedRule,
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
        guard appModel.canEditSelectedRuleContent else {
            autoSaveState = .saved
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
        guard appModel.canEditSelectedRuleContent else {
            autoSaveState = .saved
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
    let protectedHelp: String
    let canToggle: Bool
    let canReorder: Bool
    let isDragging: Bool
    let action: () -> Void
    let toggle: (Bool) -> Void
    let onStartDrag: () -> Void

    var body: some View {
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
                    .frame(width: 48, alignment: .center)
                    .help(protectedHelp)
            } else {
                Toggle("", isOn: Binding(
                    get: { rule.enabled },
                    set: toggle
                ))
                .labelsHidden()
                .toggleStyle(.switch)
                .scaleEffect(0.72)
                .disabled(isBusy || !canToggle)
            }
            dragHandle
        }
        .padding(.horizontal, 12)
        .frame(height: 50)
        .background(
            isSelected ? AppSurface.sidebarSelection : Color.clear,
            in: RoundedRectangle(cornerRadius: 7, style: .continuous)
        )
        .opacity(isDragging ? 0.55 : 1)
        .contentShape(Rectangle())
        .onTapGesture(perform: action)
    }

    @ViewBuilder
    private var dragHandle: some View {
        if canReorder {
            Image(systemName: "line.3.horizontal")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.tertiary)
                .frame(width: 24, height: 34)
                .contentShape(Rectangle())
                .help("Drag to reorder")
                .onDrag {
                    onStartDrag()
                    return NSItemProvider(object: rule.name as NSString)
                }
        } else {
            Color.clear
                .frame(width: 24, height: 34)
        }
    }
}

private struct RuleScopeOptionRow: View {
    let title: String
    let badge: String?
    let isSelected: Bool
    let isWritable: Bool
    let action: () -> Void

    private var badgeColor: Color {
        switch badge {
        case "Owner":
            return .orange
        case "Master":
            return .blue
        case "Member":
            return .cyan
        default:
            return .secondary
        }
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: 13, weight: isSelected ? .semibold : .regular))
                    .lineLimit(1)
                    .foregroundStyle(.primary)
                Spacer()
                if let badge {
                    Text(badge)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(badgeColor)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(badgeColor.opacity(0.12), in: Capsule())
                } else if isWritable {
                    Image(systemName: "person.crop.circle")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 38)
            .background(
                isSelected ? AppSurface.sidebarSelection : Color.clear,
                in: RoundedRectangle(cornerRadius: 7, style: .continuous)
            )
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
