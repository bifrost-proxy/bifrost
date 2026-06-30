import AppKit
import BifrostNativeCore
import SwiftUI

struct RulesView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var searchText = ""
    @State private var createSheetVisible = false
    @State private var renameSheetVisible = false
    @State private var deleteAlertVisible = false
    @State private var copyFeedback = ""

    private var filteredRules: [RuleSummary] {
        let keyword = searchText.trimmingCharacters(in: .whitespacesAndNewlines).localizedLowercase
        let sortedRules = appModel.rules.sorted {
            ($0.sortOrder ?? Int.max, $0.name) < ($1.sortOrder ?? Int.max, $1.name)
        }
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
                title: "New Rule",
                prompt: "Rule name",
                initialValue: "",
                confirmTitle: "Create"
            ) { name in
                Task { await appModel.createRule(name: name) }
            }
        }
        .sheet(isPresented: $renameSheetVisible) {
            NameEntrySheet(
                title: "Rename Rule",
                prompt: "Rule name",
                initialValue: appModel.selectedRuleName ?? "",
                confirmTitle: "Rename"
            ) { name in
                Task { await appModel.renameSelectedRule(to: name) }
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
        .task(id: appModel.selectedRuleName) {
            if let name = appModel.selectedRuleName {
                await appModel.selectRule(name)
            }
        }
    }

    private var listPane: some View {
        VStack(spacing: 0) {
            listHeader
            Divider()
            if filteredRules.isEmpty {
                RulesEmptyStateView(title: appModel.rules.isEmpty ? "No rules" : "No matching rules")
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(filteredRules) { rule in
                            RuleRow(
                                rule: rule,
                                isSelected: appModel.selectedRuleName == rule.name,
                                isBusy: appModel.isSavingRule
                            ) {
                                Task { await appModel.selectRule(rule.name) }
                            } toggle: { enabled in
                                Task {
                                    await appModel.selectRule(rule.name)
                                    await appModel.setSelectedRuleEnabled(enabled)
                                }
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
            if appModel.selectedRuleDetail != nil {
                CodeEditorView(text: $appModel.ruleDraftContent)
            } else {
                RulesEmptyStateView(title: "Select a rule to edit")
            }
        }
    }

    private var listHeader: some View {
        VStack(spacing: 8) {
            HStack {
                Text("Rules")
                    .font(.system(size: 14, weight: .semibold))
                Spacer()
                Text("\(appModel.rules.filter(\.enabled).count)/\(appModel.rules.count)")
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
                .help("New Rule")
                Button {
                    Task { await appModel.refreshData() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .help("Refresh Rules")
            }

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search rules...", text: $searchText)
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
                    Text(appModel.selectedRuleDetail?.name ?? "Rule Detail")
                        .font(.system(size: 13, weight: .semibold))
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
            }
            Toggle("Enabled", isOn: Binding(
                get: { appModel.selectedRuleDetail?.enabled ?? false },
                set: { enabled in Task { await appModel.setSelectedRuleEnabled(enabled) } }
            ))
            .toggleStyle(.switch)
            .font(.system(size: 11))
            .disabled(appModel.selectedRuleDetail == nil || appModel.isSavingRule)

            Button {
                copyToPasteboard(appModel.ruleDraftContent)
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
            }
            .buttonStyle(.borderless)
            .disabled(appModel.selectedRuleDetail == nil)

            Button {
                appModel.ruleDraftContent = appModel.selectedRuleDetail?.content ?? ""
            } label: {
                Label("Revert", systemImage: "arrow.uturn.backward")
            }
            .buttonStyle(.borderless)
            .disabled(!hasUnsavedChanges || appModel.isSavingRule)

            Button {
                Task { await appModel.saveSelectedRule(content: appModel.ruleDraftContent) }
            } label: {
                Label("Save", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .disabled(!hasUnsavedChanges || appModel.isSavingRule)

            Menu {
                Button("Rename") { renameSheetVisible = true }
                Button("Delete", role: .destructive) { deleteAlertVisible = true }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .disabled(appModel.selectedRuleDetail == nil || appModel.isSavingRule)
        }
        .padding(.horizontal, 12)
        .frame(height: 52)
        .background(.bar)
    }

    private var detailSubtitle: String {
        guard let detail = appModel.selectedRuleDetail else {
            return "Loaded from Admin API"
        }
        let state = detail.enabled ? "Enabled" : "Disabled"
        return "\(state) · \(detail.updatedAt ?? "updated time unavailable")"
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

private struct RuleRow: View {
    let rule: RuleSummary
    let isSelected: Bool
    let isBusy: Bool
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
                        .font(.system(size: 12, weight: .medium))
                        .lineLimit(1)
                    Text("\(rule.ruleCount ?? 0) entries")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Toggle("", isOn: Binding(
                    get: { rule.enabled },
                    set: toggle
                ))
                .labelsHidden()
                .toggleStyle(.switch)
                .scaleEffect(0.72)
                .disabled(isBusy)
            }
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(isSelected ? Color.accentColor.opacity(0.12) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
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
