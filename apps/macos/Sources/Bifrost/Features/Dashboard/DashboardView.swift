import AppKit
import BifrostNativeCore
import CoreImage
import SwiftUI

struct ActivityView: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var contentWidth: CGFloat = 0

    private var metrics: SystemOverview.Metrics? {
        appModel.overview?.metrics
    }

    var body: some View {
        NativePageScaffold(title: "活动") {
            activityMetricGrid

            ActiveRulesSummaryCard(summary: appModel.activeRulesSummary)
                .equatable()

            NativeCard {
                VStack(alignment: .leading, spacing: 14) {
                    HStack {
                        Text("流量分布")
                            .font(.system(size: 15, weight: .semibold))
                        Spacer()
                        Text("按应用统计")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.secondary)
                    }
                    ActivityBars(rows: appModel.activityClientAppCounts.map { ($0.name, $0.count) })
                        .equatable()
                }
            }
        }
        .background(ActivityWidthReader(width: $contentWidth))
    }

    private var activityMetricGrid: some View {
        activityMetricGrid(columnCount: activityMetricColumnCount)
    }

    private var activityMetricColumnCount: Int {
        guard contentWidth > 0 else {
            return 6
        }
        let minimumCardWidth: CGFloat = 150
        let spacing: CGFloat = 18
        let fit = Int((contentWidth + spacing) / (minimumCardWidth + spacing))
        return min(6, max(1, fit))
    }

    private func activityMetricGrid(columnCount: Int) -> some View {
        LazyVGrid(
            columns: Array(
                repeating: GridItem(.flexible(minimum: 150), spacing: 18, alignment: .topLeading),
                count: columnCount
            ),
            alignment: .leading,
            spacing: 18
        ) {
            activityMetricCards
        }
    }

    @ViewBuilder
    private var activityMetricCards: some View {
        NativeMetricCard(
            title: "活动连接",
            value: "\(metrics?.activeConnections ?? 0)",
            caption: "\(appModel.activityClientAppCounts.count) 个应用",
            tint: .orange
        )
        NativeMetricCard(
            title: "上传",
            value: formatRate(metrics?.bytesSentRate),
            caption: formatBytes(metrics?.bytesSent),
            tint: .indigo
        )
        NativeMetricCard(
            title: "下载",
            value: formatRate(metrics?.bytesReceivedRate),
            caption: formatBytes(metrics?.bytesReceived),
            tint: .cyan
        )
        NativeMetricCard(
            title: "请求",
            value: "\(metrics?.totalRequests ?? 0)",
            caption: qpsText,
            tint: .green
        )
        NativeMetricCard(
            title: "规则",
            value: rulesSummary,
            caption: "当前规则集",
            tint: .purple
        )
        NativeMetricCard(
            title: "服务",
            value: sidecarStatusText,
            caption: appModel.adminHostPortLabel,
            tint: .blue
        )
    }

    private var qpsText: String {
        guard let qps = metrics?.qps else {
            return "实时速率"
        }
        return String(format: "%.2f QPS", qps)
    }

    private var rulesSummary: String {
        let enabled = appModel.overview?.rules?.enabled ?? appModel.rules.filter(\.enabled).count
        let total = appModel.overview?.rules?.total ?? appModel.rules.count
        return "\(enabled)/\(total)"
    }

    private var sidecarStatusText: String {
        switch appModel.sidecarState {
        case .running(_, let origin):
            switch origin {
            case .existingDefaultDataDirectory:
                return "运行中 · CLI"
            case .launchedBundledSidecar:
                return "运行中 · App"
            }
        case .starting:
            return "启动中"
        case .recovering:
            return "恢复中"
        case .failed:
            return "异常"
        case .stopped:
            return "未启动"
        }
    }
}

private struct ActivityWidthReader: View {
    @Binding var width: CGFloat

    var body: some View {
        GeometryReader { proxy in
            Color.clear
                .preference(key: ActivityWidthPreferenceKey.self, value: proxy.size.width)
        }
        .onPreferenceChange(ActivityWidthPreferenceKey.self) { nextWidth in
            guard abs(width - nextWidth) > 1 else {
                return
            }
            width = nextWidth
        }
    }
}

private struct ActivityWidthPreferenceKey: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private struct ActiveRulesSummaryCard: View, Equatable {
    let summary: ActiveRulesSummary?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("生效规则解析")
                            .font(.system(size: 15, weight: .semibold))
                        Text("当前代理端口正在使用的规则集合")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusPill(title: summaryStatusText, color: summaryStatusColor)
                }

                if let summary {
                    if summary.rules.isEmpty && summary.variableConflicts.isEmpty {
                        EmptyNativeState(title: "暂无生效规则")
                            .frame(maxWidth: .infinity, alignment: .center)
                            .padding(.vertical, 18)
                    } else {
                        VStack(alignment: .leading, spacing: 14) {
                            if !summary.variableConflicts.isEmpty {
                                variableConflicts(summary.variableConflicts)
                            }
                            activeRules(summary.rules)
                            if !summary.mergedContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                mergedRules(summary.mergedContent)
                            }
                        }
                    }
                } else {
                    Text("正在读取生效规则解析信息...")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 12)
                }
            }
        }
    }

    private var summaryStatusText: String {
        guard let summary else {
            return "读取中"
        }
        return "\(summary.total) active"
    }

    private var summaryStatusColor: Color {
        guard let summary else {
            return .secondary
        }
        return summary.total > 0 ? .green : .secondary
    }

    private func activeRules(_ rules: [ActiveRuleItem]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Active Rules")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)

            if localRules(in: rules).isEmpty && groupedRules(in: rules).isEmpty {
                Text("No active rules resolved")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    if !localRules(in: rules).isEmpty {
                        RuleTokenFlow {
                            ForEach(localRules(in: rules)) { rule in
                                ActiveRuleToken(rule: rule)
                            }
                        }
                    }

                    ForEach(groupedRules(in: rules), id: \.id) { group in
                        VStack(alignment: .leading, spacing: 7) {
                            HStack(spacing: 5) {
                                Image(systemName: "person.2")
                                    .font(.system(size: 11, weight: .semibold))
                                Text(group.name)
                                    .font(.system(size: 11, weight: .semibold))
                                    .lineLimit(1)
                            }
                            .foregroundStyle(.secondary)

                            RuleTokenFlow {
                                ForEach(group.rules) { rule in
                                    ActiveRuleToken(rule: rule)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func variableConflicts(_ conflicts: [ActiveRuleVariableConflict]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(Color.orange)
                Text("Variable Conflicts")
                    .font(.system(size: 12, weight: .semibold))
            }

            ForEach(conflicts) { conflict in
                VStack(alignment: .leading, spacing: 5) {
                    Text("{\(conflict.variableName)}")
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    ForEach(conflict.definitions) { definition in
                        Text("\(definition.ruleName): \(definition.valuePreview)")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
            }
        }
        .padding(12)
        .background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .stroke(Color.orange.opacity(0.20))
        )
    }

    private func mergedRules(_ content: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Merged Rules")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .textCase(.uppercase)
                Spacer()
                Text("\(content.split(whereSeparator: \.isNewline).count) lines")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                Text(content.trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.primary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
    }

    private func localRules(in rules: [ActiveRuleItem]) -> [ActiveRuleItem] {
        rules.filter { $0.groupID == nil }
    }

    private func groupedRules(in rules: [ActiveRuleItem]) -> [ActiveRuleGroup] {
        let grouped = Dictionary(grouping: rules.filter { $0.groupID != nil }) { rule in
            rule.groupID ?? ""
        }
        return grouped
            .map { groupID, rules in
                ActiveRuleGroup(
                    id: groupID,
                    name: rules.first?.groupName ?? groupID,
                    rules: rules.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
                )
            }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    private struct ActiveRuleGroup {
        let id: String
        let name: String
        let rules: [ActiveRuleItem]
    }
}

private struct RuleTokenFlow<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        LazyVGrid(
            columns: [
                GridItem(.adaptive(minimum: 150, maximum: 260), spacing: 8, alignment: .topLeading)
            ],
            alignment: .leading,
            spacing: 8
        ) {
            content
        }
    }
}

private struct ActiveRuleToken: View {
    let rule: ActiveRuleItem

    var body: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(rule.groupID == nil ? Color.blue : Color.purple)
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(rule.name)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text("\(rule.ruleCount) entries")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 4)
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 10)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}

struct DashboardView: View {
    @EnvironmentObject private var appModel: AppModel
    @StateObject private var model = OverviewControlModel()

    var body: some View {
        NativePageScaffold(title: "概览") {
            overviewControlGrid

            RemoteInvokeCard(model: model)

            MobileConnectionCheckCard(model: model)
        }
        .task(id: appModel.adminURL) {
            model.setTrustProbePollingActive(appModel.selectedSidebarItem == .overview)
            await model.configure(baseURL: appModel.adminURL)
        }
        .onAppear {
            model.setTrustProbePollingActive(appModel.selectedSidebarItem == .overview)
        }
        .onDisappear {
            model.suspendBackgroundWork()
        }
        .onChange(of: appModel.selectedSidebarItem) { item in
            model.setTrustProbePollingActive(item == .overview)
        }
    }

    private var overviewControlGrid: some View {
        ViewThatFits(in: .horizontal) {
            overviewControlGrid(columnCount: 4)
            overviewControlGrid(columnCount: 3)
            overviewControlGrid(columnCount: 2)
            overviewControlGrid(columnCount: 1)
        }
    }

    private func overviewControlGrid(columnCount: Int) -> some View {
        LazyVGrid(
            columns: Array(
                repeating: GridItem(.flexible(minimum: 220), spacing: 18, alignment: .topLeading),
                count: columnCount
            ),
            alignment: .leading,
            spacing: 18
        ) {
            overviewControlCards
        }
    }

    @ViewBuilder
    private var overviewControlCards: some View {
        SystemProxyCard()
        TlsInterceptionCard()
        SyncControlCard(model: model)
        CertificateManagementCard(model: model)
    }
}

struct NetworkWebView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        NativePageScaffold(title: "抓包") {
            NativeCard {
                VStack(alignment: .leading, spacing: 18) {
                    ViewThatFits(in: .horizontal) {
                        networkHeader
                        VStack(alignment: .leading, spacing: 12) {
                            networkHeader
                        }
                    }
                    Divider()
                    AdaptiveFactGrid(minimum: 118) {
                        CompactFact(title: "当前记录", value: "\(appModel.overview?.traffic?.recorded ?? appModel.trafficRecords.count)")
                        CompactFact(title: "活动连接", value: "\(appModel.overview?.metrics?.activeConnections ?? 0)")
                        CompactFact(title: "规则命中", value: "\(appModel.activityRuleHitCount)")
                    }
                }
            }
        }
    }

    private var networkHeader: some View {
        HStack(spacing: 14) {
            Image(systemName: "globe")
                .font(.system(size: 28, weight: .medium))
                .foregroundStyle(.blue)
                .frame(width: 42, height: 42)
            VStack(alignment: .leading, spacing: 4) {
                Text("抓包详情")
                    .font(.system(size: 18, weight: .semibold))
                Text(appModel.adminHostPortLabel)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Button {
                appModel.openWebUI()
            } label: {
                Label("在浏览器打开", systemImage: "arrow.up.right.square")
            }
            .buttonStyle(.borderedProminent)
        }
    }
}

struct GroupsView: View {
    @EnvironmentObject private var appModel: AppModel
    @StateObject private var model = GroupsViewModel()
    @State private var detailMode = GroupDetailMode.detail
    @State private var draftName = ""
    @State private var draftDescription = ""
    @State private var draftVisibility: RuleGroupVisibility = .private
    @State private var pendingDeleteGroup: RuleGroup?
    @State private var pendingRemoveMember: GroupMember?
    @State private var isAddingMember = false
    @State private var memberSearchText = ""
    @State private var newMemberLevel = 0

    private let groupListWidth: CGFloat = 300

    var body: some View {
        NativePageScaffold(title: "小组", contentFillsAvailableHeight: true) {
            HStack(alignment: .top, spacing: 5) {
                NativePanel(scaleOnHover: 1.002, allowsHoverEffect: false) {
                    groupListPane
                }
                .frame(width: groupListWidth)
                .frame(maxHeight: .infinity)

                NativePanel(scaleOnHover: 1.002, allowsHoverEffect: false) {
                    groupDetailPane
                }
                .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
                .layoutPriority(1)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .task(id: appModel.adminURL) {
            await model.configure(baseURL: appModel.adminURL)
        }
        .alert(
            "删除小组",
            isPresented: Binding(
                get: { pendingDeleteGroup != nil },
                set: { if !$0 { pendingDeleteGroup = nil } }
            )
        ) {
            Button("取消", role: .cancel) {
                pendingDeleteGroup = nil
            }
            Button("删除", role: .destructive) {
                guard let group = pendingDeleteGroup else { return }
                pendingDeleteGroup = nil
                Task { await model.deleteGroup(id: group.id) }
            }
        } message: {
            Text("删除后，小组规则和成员关系会从服务端移除。")
        }
        .alert(
            "移除成员",
            isPresented: Binding(
                get: { pendingRemoveMember != nil },
                set: { if !$0 { pendingRemoveMember = nil } }
            )
        ) {
            Button("取消", role: .cancel) {
                pendingRemoveMember = nil
            }
            Button("移除", role: .destructive) {
                guard let member = pendingRemoveMember else { return }
                pendingRemoveMember = nil
                Task { await model.removeMember(groupID: member.groupID, userID: member.userID) }
            }
        } message: {
            Text("成员会从这个小组中移除，无法继续管理或同步该小组的规则。")
        }
    }

    private var groupListPane: some View {
        VStack(spacing: 0) {
            groupListHeader

            if model.isLoading {
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity, minHeight: 36)
            }

            if let errorMessage = model.errorMessage {
                Text(errorMessage)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
            }

            if model.visibleGroups.isEmpty && !model.isLoading {
                EmptyNativeState(title: "暂无可用小组")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        groupSection("我管理的", groups: model.managedGroups)
                        groupSection("我加入的", groups: model.joinedGroups)
                        groupSection("公开可读", groups: model.publicGroups)
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                }
            }
        }
    }

    private var groupListHeader: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField("搜索小组", text: $model.searchText)
                .textFieldStyle(.plain)
                .onChange(of: model.searchText) { _ in
                    model.scheduleSearch()
                }
                .onSubmit {
                    Task { await model.refresh() }
                }
            Button {
                Task { await model.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help("刷新小组")

            Button {
                openCreateEditor()
            } label: {
                Image(systemName: "plus")
            }
            .buttonStyle(.borderless)
            .help("新建小组")
        }
        .font(.system(size: 12))
        .padding(.horizontal, 12)
        .frame(height: 42)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
        .padding(12)
    }

    @ViewBuilder
    private func groupSection(_ title: String, groups: [RuleGroup]) -> some View {
        if !groups.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text(title.uppercased())
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)
                ForEach(groups) { group in
                    GroupListRow(
                        group: group,
                        isSelected: model.selectedGroupID == group.id,
                        onSelect: {
                            detailMode = .detail
                            isAddingMember = false
                            clearMemberInvite()
                            Task { await model.select(groupID: group.id) }
                        }
                    )
                }
            }
        }
    }

    @ViewBuilder
    private var groupDetailPane: some View {
        switch detailMode {
        case .create:
            ScrollView {
                GroupEditorPane(
                    title: "新建小组",
                    actionTitle: "创建",
                    name: $draftName,
                    description: $draftDescription,
                    visibility: $draftVisibility,
                    isSaving: model.isSaving,
                    onCancel: { detailMode = .detail },
                    onSave: { saveEditor(GroupEditorContext(groupID: nil)) }
                )
                .padding(18)
            }
        case .edit(let groupID):
            ScrollView {
                GroupEditorPane(
                    title: "编辑小组",
                    actionTitle: "保存",
                    name: $draftName,
                    description: $draftDescription,
                    visibility: $draftVisibility,
                    isSaving: model.isSaving,
                    onCancel: { detailMode = .detail },
                    onSave: { saveEditor(GroupEditorContext(groupID: groupID)) }
                )
                .padding(18)
            }
        case .detail:
            ScrollView {
                groupDetailContent
                    .padding(18)
            }
        }
    }

    @ViewBuilder
    private var groupDetailContent: some View {
        if let group = model.selectedGroup {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top, spacing: 12) {
                    ZStack {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(.blue.opacity(0.10))
                        Image(systemName: "person.2")
                            .font(.system(size: 22, weight: .medium))
                            .foregroundStyle(.blue)
                    }
                    .frame(width: 46, height: 46)

                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 8) {
                            Text(group.name)
                                .font(.system(size: 20, weight: .semibold))
                                .lineLimit(1)
                            StatusPill(title: group.permissionLabel, color: group.permissionColor)
                        }
                        Text(group.description.isEmpty ? "暂无小组说明" : group.description)
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(group.visibility.displayName)
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(group.visibility == .public ? .blue : .secondary)
                    }
                    Spacer(minLength: 12)
                    if group.canManageMembers {
                        Button("编辑") {
                            openEditEditor(group)
                        }
                        .buttonStyle(.bordered)
                    }
                    if group.level == 2 {
                        Button("删除", role: .destructive) {
                            pendingDeleteGroup = group
                        }
                        .buttonStyle(.bordered)
                    }
                }

                Divider()

                HStack(spacing: 10) {
                    CompactFact(title: "规则数量", value: "\(model.selectedGroupRules?.rules.count ?? 0)")
                    CompactFact(title: "成员", value: "\(model.selectedGroupMembers?.total ?? 0)")
                    CompactFact(title: "权限", value: model.selectedGroupRules?.writable == true ? "可修改" : "只读")
                    Spacer()
                    Button {
                        appModel.selectedSidebarItem = .rules
                        Task { await appModel.selectRuleScope(groupID: group.id) }
                    } label: {
                        Label("在规则页管理", systemImage: "arrow.right")
                    }
                    .buttonStyle(.borderedProminent)
                }

                Divider()

                HStack {
                    Text("成员")
                        .font(.system(size: 15, weight: .semibold))
                    if let total = model.selectedGroupMembers?.total {
                        Text("\(total)")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.quaternary.opacity(0.7), in: Capsule())
                    }
                    Spacer()
                    if group.canManageMembers {
                        Button {
                            isAddingMember.toggle()
                            if !isAddingMember {
                                clearMemberInvite()
                            }
                        } label: {
                            Label(isAddingMember ? "收起" : "新增成员", systemImage: "person.badge.plus")
                        }
                        .buttonStyle(.bordered)
                        .disabled(model.isSaving)
                    }
                }

                if isAddingMember && group.canManageMembers {
                    memberInvitePane(groupID: group.id)
                }

                if model.isLoadingMembers {
                    ProgressView()
                        .controlSize(.small)
                } else if let members = model.selectedGroupMembers?.list, !members.isEmpty {
                    LazyVGrid(
                        columns: [GridItem(.adaptive(minimum: 260), spacing: 8, alignment: .topLeading)],
                        alignment: .leading,
                        spacing: 8
                    ) {
                        ForEach(members) { member in
                            GroupMemberRow(
                                member: member,
                                currentUserID: appModel.syncStatus?.user?.userID,
                                currentUserLevel: group.level,
                                isSaving: model.isSaving,
                                onChangeLevel: { level in
                                    Task {
                                        await model.updateMemberLevel(groupID: group.id, userID: member.userID, level: level)
                                    }
                                },
                                onRemove: {
                                    pendingRemoveMember = member
                                }
                            )
                        }
                    }
                    GroupMemberPaginationControl(
                        page: model.membersPage,
                        totalPages: model.membersTotalPages,
                        total: model.membersTotal,
                        pageSize: model.membersPageSize,
                        isLoading: model.isLoadingMembers,
                        onPrevious: {
                            Task { await model.goToMembersPage(model.membersPage - 1) }
                        },
                        onNext: {
                            Task { await model.goToMembersPage(model.membersPage + 1) }
                        }
                    )
                } else {
                    Text("暂无成员信息")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
        } else {
            EmptyNativeState(title: "选择一个小组")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func memberInvitePane(groupID: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("搜索用户邮箱、ID 或昵称", text: $memberSearchText)
                    .textFieldStyle(.plain)
                    .onChange(of: memberSearchText) { value in
                        model.scheduleUserSearch(keyword: value)
                    }
                    .onSubmit {
                        Task { await model.searchUsers(keyword: memberSearchText) }
                    }
                Picker("角色", selection: $newMemberLevel) {
                    Text("Member").tag(0)
                    Text("Master").tag(1)
                }
                .labelsHidden()
                .frame(width: 110)
            }
            .font(.system(size: 12))
            .padding(.horizontal, 10)
            .frame(height: 34)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))

            if model.isSearchingUsers {
                ProgressView()
                    .controlSize(.small)
            } else if !memberSearchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                if model.userSearchResults.isEmpty {
                    Text("没有匹配用户")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(model.userSearchResults.prefix(8)) { user in
                            GroupUserSearchRow(user: user, isSaving: model.isSaving) {
                                Task {
                                    await model.inviteMember(groupID: groupID, userID: user.userID, level: newMemberLevel)
                                    if model.errorMessage == nil {
                                        clearMemberInvite()
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        .padding(10)
        .background(AppSurface.card.opacity(0.72), in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(AppSurface.cardBorder)
        )
    }

    private func clearMemberInvite() {
        memberSearchText = ""
        newMemberLevel = 0
        model.clearUserSearch()
    }

    private func openCreateEditor() {
        draftName = ""
        draftDescription = ""
        draftVisibility = .private
        detailMode = .create
    }

    private func openEditEditor(_ group: RuleGroup) {
        draftName = group.name
        draftDescription = group.description
        draftVisibility = group.visibility
        detailMode = .edit(group.id)
    }

    private func saveEditor(_ context: GroupEditorContext) {
        Task {
            if let groupID = context.groupID {
                await model.updateGroup(
                    id: groupID,
                    name: draftName,
                    description: draftDescription,
                    visibility: draftVisibility
                )
            } else {
                await model.createGroup(
                    name: draftName,
                    description: draftDescription,
                    visibility: draftVisibility
                )
            }
            if model.errorMessage == nil {
                detailMode = .detail
            }
        }
    }
}

private enum GroupDetailMode: Equatable {
    case detail
    case create
    case edit(String)
}

private struct GroupEditorContext {
    let groupID: String?
}

@MainActor
private final class GroupsViewModel: ObservableObject {
    @Published var groups: [RuleGroup] = []
    @Published var selectedGroupID: String?
    @Published var selectedGroupRules: GroupRulesResponse?
    @Published var selectedGroupMembers: GroupMemberListResponse?
    @Published var userSearchResults: [GroupUser] = []
    @Published var searchText = ""
    @Published var isLoading = false
    @Published var isLoadingRules = false
    @Published var isLoadingMembers = false
    @Published var isSearchingUsers = false
    @Published var isSaving = false
    @Published var errorMessage: String?

    let membersPageSize = 12
    @Published private(set) var membersPage = 1

    private var baseURL: URL?
    private var client: BifrostClient?
    private var searchTask: Task<Void, Never>?
    private var userSearchTask: Task<Void, Never>?

    var visibleGroups: [RuleGroup] {
        groups.sorted {
            ($0.permissionRank, $0.name.localizedLowercase) < ($1.permissionRank, $1.name.localizedLowercase)
        }
    }

    var managedGroups: [RuleGroup] {
        visibleGroups.filter { ($0.level ?? -1) >= 1 }
    }

    var joinedGroups: [RuleGroup] {
        visibleGroups.filter { $0.level == 0 }
    }

    var publicGroups: [RuleGroup] {
        visibleGroups.filter { ($0.level ?? -1) < 0 }
    }

    var selectedGroup: RuleGroup? {
        guard let selectedGroupID else {
            return visibleGroups.first
        }
        return visibleGroups.first { $0.id == selectedGroupID }
    }

    var membersTotal: Int {
        selectedGroupMembers?.total ?? 0
    }

    var membersTotalPages: Int {
        max(1, Int(ceil(Double(membersTotal) / Double(membersPageSize))))
    }

    func configure(baseURL: URL) async {
        guard self.baseURL != baseURL else {
            return
        }
        self.baseURL = baseURL
        do {
            client = try BifrostClient(baseURL: baseURL)
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func refresh() async {
        guard let client else {
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            let response = try await client.fetchRuleGroups(keyword: searchText, limit: 120)
            groups = response.list
            if let selectedGroupID, !groups.contains(where: { $0.id == selectedGroupID }) {
                self.selectedGroupID = nil
                selectedGroupRules = nil
                selectedGroupMembers = nil
            }
            if let nextSelection = selectedGroupID ?? visibleGroups.first?.id {
                await select(groupID: nextSelection)
            }
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func scheduleSearch() {
        searchTask?.cancel()
        searchTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 300_000_000)
            guard !Task.isCancelled else {
                return
            }
            await self?.refresh()
        }
    }

    func scheduleUserSearch(keyword: String) {
        userSearchTask?.cancel()
        let trimmedKeyword = keyword.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedKeyword.isEmpty else {
            userSearchResults = []
            isSearchingUsers = false
            return
        }
        userSearchTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 350_000_000)
            guard !Task.isCancelled else {
                return
            }
            await self?.searchUsers(keyword: trimmedKeyword)
        }
    }

    func searchUsers(keyword: String) async {
        guard let client else {
            return
        }
        let trimmedKeyword = keyword.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedKeyword.isEmpty else {
            userSearchResults = []
            return
        }
        isSearchingUsers = true
        defer { isSearchingUsers = false }
        do {
            let response = try await client.searchUsers(keyword: trimmedKeyword)
            userSearchResults = response.list
            errorMessage = nil
        } catch {
            userSearchResults = []
            errorMessage = error.localizedDescription
        }
    }

    func clearUserSearch() {
        userSearchTask?.cancel()
        userSearchResults = []
        isSearchingUsers = false
    }

    func select(groupID: String) async {
        guard selectedGroupID != groupID || selectedGroupRules == nil || selectedGroupMembers == nil else {
            return
        }
        if selectedGroupID != groupID {
            membersPage = 1
        }
        selectedGroupID = groupID
        await loadGroupDetail(groupID: groupID)
    }

    func createGroup(name: String, description: String, visibility: RuleGroupVisibility) async {
        guard let client else {
            return
        }
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty else {
            errorMessage = "小组名称不能为空"
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            let group = try await client.createRuleGroup(
                name: trimmedName,
                description: description.trimmingCharacters(in: .whitespacesAndNewlines),
                visibility: visibility
            )
            selectedGroupID = group.id
            await refresh()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func updateGroup(id: String, name: String, description: String, visibility: RuleGroupVisibility) async {
        guard let client else {
            return
        }
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty else {
            errorMessage = "小组名称不能为空"
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await client.updateRuleGroup(
                id: id,
                name: trimmedName,
                description: description.trimmingCharacters(in: .whitespacesAndNewlines),
                visibility: visibility
            )
            selectedGroupID = id
            await refresh()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func deleteGroup(id: String) async {
        guard let client else {
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await client.deleteRuleGroup(id: id)
            if selectedGroupID == id {
                selectedGroupID = nil
                selectedGroupRules = nil
                selectedGroupMembers = nil
            }
            await refresh()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func inviteMember(groupID: String, userID: String, level: Int) async {
        guard let client else {
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await client.inviteGroupMember(groupID: groupID, userID: userID, level: level)
            await loadMembers(groupID: groupID, page: membersPage)
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func updateMemberLevel(groupID: String, userID: String, level: Int) async {
        guard let client else {
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await client.updateGroupMemberLevel(groupID: groupID, userID: userID, level: level)
            await loadMembers(groupID: groupID, page: membersPage)
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func removeMember(groupID: String, userID: String) async {
        guard let client else {
            return
        }
        isSaving = true
        defer { isSaving = false }
        do {
            try await client.removeGroupMember(groupID: groupID, userID: userID)
            await loadMembers(groupID: groupID, page: membersPage)
            if selectedGroupMembers?.list.isEmpty == true, membersPage > 1 {
                await loadMembers(groupID: groupID, page: membersPage - 1)
            }
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func goToMembersPage(_ page: Int) async {
        guard let groupID = selectedGroupID else {
            return
        }
        let clampedPage = min(max(1, page), membersTotalPages)
        guard clampedPage != membersPage else {
            return
        }
        await loadMembers(groupID: groupID, page: clampedPage)
    }

    private func loadGroupDetail(groupID: String) async {
        guard let client else {
            return
        }
        isLoadingRules = true
        isLoadingMembers = true
        defer {
            isLoadingRules = false
            isLoadingMembers = false
        }
        do {
            async let rulesResult = client.fetchGroupRules(groupID: groupID)
            async let membersResult = client.fetchGroupMembers(id: groupID, limit: membersPageSize)
            selectedGroupRules = try await rulesResult
            selectedGroupMembers = try await membersResult
            errorMessage = nil
        } catch {
            selectedGroupRules = nil
            selectedGroupMembers = nil
            errorMessage = error.localizedDescription
        }
    }

    private func loadMembers(groupID: String, page: Int) async {
        guard let client else {
            return
        }
        let nextPage = max(1, page)
        isLoadingMembers = true
        defer { isLoadingMembers = false }
        do {
            let response = try await client.fetchGroupMembers(
                id: groupID,
                offset: (nextPage - 1) * membersPageSize,
                limit: membersPageSize
            )
            selectedGroupMembers = response
            membersPage = min(nextPage, max(1, Int(ceil(Double(response.total) / Double(membersPageSize)))))
            errorMessage = nil
        } catch {
            selectedGroupMembers = nil
            errorMessage = error.localizedDescription
        }
    }
}

private struct GroupMemberPaginationControl: View {
    let page: Int
    let totalPages: Int
    let total: Int
    let pageSize: Int
    let isLoading: Bool
    let onPrevious: () -> Void
    let onNext: () -> Void

    private var pageStart: Int {
        total == 0 ? 0 : (page - 1) * pageSize + 1
    }

    private var pageEnd: Int {
        min(total, page * pageSize)
    }

    var body: some View {
        HStack(spacing: 10) {
            Text("第 \(page) / \(totalPages) 页")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
            Text("\(pageStart)-\(pageEnd) / \(total) 位")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
            Spacer(minLength: 12)
            HStack(spacing: 4) {
                Button(action: onPrevious) {
                    Image(systemName: "chevron.left")
                        .frame(width: 20, height: 20)
                }
                .buttonStyle(.borderless)
                .help("上一页")
                .disabled(isLoading || page <= 1)

                Button(action: onNext) {
                    Image(systemName: "chevron.right")
                        .frame(width: 20, height: 20)
                }
                .buttonStyle(.borderless)
                .help("下一页")
                .disabled(isLoading || page >= totalPages)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(AppSurface.subtleFill.opacity(0.75), in: RoundedRectangle(cornerRadius: 7))
    }
}

private struct GroupListRow: View {
    let group: RuleGroup
    let isSelected: Bool
    let onSelect: () -> Void

    var body: some View {
        Button(action: onSelect) {
            HStack(spacing: 9) {
                Circle()
                    .fill(group.permissionColor)
                    .frame(width: 7, height: 7)
                VStack(alignment: .leading, spacing: 3) {
                    Text(group.name)
                        .font(.system(size: 13, weight: .semibold))
                        .lineLimit(1)
                    Text(group.description.isEmpty ? group.visibility.displayName : group.description)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(group.permissionLabel)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(group.permissionColor)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 3)
                    .background(group.permissionColor.opacity(0.10), in: RoundedRectangle(cornerRadius: 6))
            }
            .contentShape(Rectangle())
            .padding(.horizontal, 9)
            .padding(.vertical, 8)
            .background(isSelected ? AppSurface.subtleFill : .clear, in: RoundedRectangle(cornerRadius: 7))
        }
        .buttonStyle(.plain)
    }
}

private struct GroupMemberRow: View {
    let member: GroupMember
    let currentUserID: String?
    let currentUserLevel: Int?
    let isSaving: Bool
    let onChangeLevel: (Int) -> Void
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(member.permissionColor)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 3) {
                Text(displayName)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(member.email.isEmpty ? member.userID : member.email)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if canManageThisMember {
                Picker("角色", selection: levelBinding) {
                    Text("Member").tag(0)
                    Text("Master").tag(1)
                }
                .labelsHidden()
                .frame(width: 94)
                .disabled(isSaving)

                Button(role: .destructive, action: onRemove) {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .help("移除成员")
                .disabled(isSaving)
            } else {
                Text(member.permissionLabel)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(member.permissionColor)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(member.permissionColor.opacity(0.10), in: RoundedRectangle(cornerRadius: 5))
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
    }

    private var levelBinding: Binding<Int> {
        Binding(
            get: { member.level > 0 ? 1 : 0 },
            set: { newValue in
                guard newValue != member.level else {
                    return
                }
                onChangeLevel(newValue)
            }
        )
    }

    private var canManageThisMember: Bool {
        guard let currentUserLevel, currentUserLevel >= 1, member.userID != currentUserID else {
            return false
        }
        if currentUserLevel == 2 {
            return member.level < 2
        }
        return member.level == 0
    }

    private var displayName: String {
        if !member.nickname.isEmpty {
            return member.nickname
        }
        if !member.email.isEmpty {
            return member.email
        }
        return member.userID.isEmpty ? member.id : member.userID
    }
}

private struct GroupUserSearchRow: View {
    let user: GroupUser
    let isSaving: Bool
    let onInvite: () -> Void

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(.blue)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 3) {
                Text(displayName)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(user.email.isEmpty ? user.userID : user.email)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Button(action: onInvite) {
                Label("添加", systemImage: "plus")
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .disabled(isSaving)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
    }

    private var displayName: String {
        if !user.nickname.isEmpty {
            return user.nickname
        }
        if !user.email.isEmpty {
            return user.email
        }
        return user.userID.isEmpty ? user.id : user.userID
    }
}

private struct GroupEditorPane: View {
    let title: String
    let actionTitle: String
    @Binding var name: String
    @Binding var description: String
    @Binding var visibility: RuleGroupVisibility
    let isSaving: Bool
    let onCancel: () -> Void
    let onSave: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text(title)
                    .font(.system(size: 20, weight: .semibold))
                Spacer()
                Button("取消", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button(actionTitle, action: onSave)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(isSaving)
            }

            Divider()

            TextField("小组名称", text: $name)
                .textFieldStyle(.roundedBorder)
            TextEditor(text: $description)
                .font(.system(size: 13))
                .frame(minHeight: 140)
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(AppSurface.cardBorder)
                )
            Picker("可见性", selection: $visibility) {
                ForEach(RuleGroupVisibility.allCases, id: \.self) { item in
                    Text(item.displayName).tag(item)
                }
            }
            .pickerStyle(.segmented)

            Text("公开小组可被其他用户查看；私有小组仅对成员可见。")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
}

private extension RuleGroupVisibility {
    var displayName: String {
        switch self {
        case .public:
            return "公开可读"
        case .private:
            return "私有"
        }
    }
}

private extension RuleGroup {
    var permissionColor: Color {
        switch level {
        case 2:
            return .orange
        case 1:
            return .blue
        case 0:
            return .green
        default:
            return .secondary
        }
    }

    var canManageMembers: Bool {
        (level ?? -1) >= 1
    }
}

private extension GroupMember {
    var permissionColor: Color {
        switch level {
        case 2:
            return .orange
        case 1:
            return .blue
        default:
            return .green
        }
    }
}

@MainActor
private final class OverviewControlModel: ObservableObject {
    @Published var certInfo: CertInfo?
    @Published var mobileDevices: MobileDevicesResponse?
    @Published var proxyAddressInfo: ProxyAddressInfo?
    @Published var trustProbeSession: TrustProbeSession?
    @Published var syncStatus: SyncStatus?
    @Published var remoteInvokeStatus: RemoteInvokeStatus?
    @Published var remoteInvokeGrants: GrantsListResponse?
    @Published var remoteInvokeCalls: CallsListResponse?
    @Published var remoteInvokeSshKey: RemoteInvokeSshKeyRecord?
    @Published var copiedSshKeyAt: Date?
    @Published var copiedPairCodeAt: Date?
    @Published var copiedProbeURLAt: Date?
    @Published var syncRemoteBaseURLDraft = ""
    @Published var isEditingSyncRemoteBaseURL = false
    @Published var isLoading = false
    @Published var isMutating = false
    @Published var errorMessage: String?

    private var baseURL = URL(string: "http://127.0.0.1:9900")!
    private var trustProbePollingTask: Task<Void, Never>?
    private var pollingTrustProbeSessionID: String?
    private var trustProbePollingActive = false

    deinit {
        trustProbePollingTask?.cancel()
    }

    func configure(baseURL: URL) async {
        self.baseURL = baseURL
        await refresh()
    }

    func suspendBackgroundWork() {
        trustProbePollingActive = false
        stopTrustProbePolling()
    }

    func setTrustProbePollingActive(_ active: Bool) {
        guard trustProbePollingActive != active else {
            return
        }
        trustProbePollingActive = active
        if active,
           let session = trustProbeSession,
           session.status != "expired" {
            startTrustProbePolling(sessionID: session.sessionID)
        } else if !active {
            stopTrustProbePolling()
        }
    }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let client = try BifrostClient(baseURL: baseURL)
            async let cert = client.fetchCertInfo()
            async let mobile = client.fetchMobileDevices()
            async let proxyAddress = client.fetchProxyAddress()
            async let sync = client.fetchSyncStatus()
            async let remote = client.fetchRemoteInvokeStatus()
            async let grants = client.fetchRemoteInvokeGrants()
            async let calls = client.fetchRemoteInvokeCalls(limit: 12)
            async let sshKey = client.fetchRemoteInvokeSshKey()
            certInfo = try await cert
            mobileDevices = try await mobile
            proxyAddressInfo = try await proxyAddress
            applySyncStatus(try await sync)
            remoteInvokeStatus = try await remote
            remoteInvokeGrants = try await grants
            remoteInvokeCalls = try await calls
            remoteInvokeSshKey = try await sshKey
            try await refreshTrustProbeSession(client: client, forceCreate: false)
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func setRemoteDiscoveryEnabled(_ enabled: Bool) async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            if enabled {
                _ = try await client.enterDiscoveryMode()
            } else {
                try await client.exitDiscoveryMode()
            }
            self.remoteInvokeStatus = try await client.fetchRemoteInvokeStatus()
            self.remoteInvokeGrants = try await client.fetchRemoteInvokeGrants()
            self.remoteInvokeCalls = try await client.fetchRemoteInvokeCalls(limit: 12)
        }
    }

    func refreshRemotePairCode() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            _ = try await client.refreshPairCode()
            self.remoteInvokeStatus = try await client.fetchRemoteInvokeStatus()
        }
    }

    func copyRemotePairCode() {
        guard let pairCode = remoteInvokeStatus?.discoverySession?.pairCode else {
            return
        }
        copyToPasteboard(pairCode)
        copiedPairCodeAt = Date()
    }

    func createRemoteInvokeSshKey() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            let label = "Bifrost Mac Native"
            let secret = if self.remoteInvokeSshKey == nil {
                try await client.createRemoteInvokeSshKey(label: label)
            } else {
                try await client.resetRemoteInvokeSshKey()
            }
            self.copyToPasteboard(secret.bifrostKeyFile)
            self.copiedSshKeyAt = Date()
            self.remoteInvokeSshKey = try await client.fetchRemoteInvokeSshKey()
            self.remoteInvokeGrants = try await client.fetchRemoteInvokeGrants()
        }
    }

    func copyRemoteInvokeSshKey() async {
        await mutate {
            let secret = try await BifrostClient(baseURL: self.baseURL).fetchRemoteInvokeSshPrivateKey()
            self.copyToPasteboard(secret.bifrostKeyFile)
            self.copiedSshKeyAt = Date()
        }
    }

    func setSyncEnabled(_ enabled: Bool) async {
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(enabled: enabled))
            self.applySyncStatus(status)
        }
    }

    func setAutoSyncEnabled(_ enabled: Bool) async {
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(autoSync: enabled))
            self.applySyncStatus(status)
        }
    }

    func openSyncLogin() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).openSyncLogin())
        }
    }

    func logoutSync() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).logoutSyncSession())
        }
    }

    func runSyncNow() async {
        await mutate {
            self.applySyncStatus(try await BifrostClient(baseURL: self.baseURL).runSyncNow())
        }
    }

    func handleSyncRemoteBaseURLClick() async {
        guard syncStatus != nil else {
            return
        }
        if syncStatus?.hasSession == true {
            beginSyncRemoteBaseURLEdit()
        } else {
            await openSyncLogin()
        }
    }

    func beginSyncRemoteBaseURLEdit() {
        syncRemoteBaseURLDraft = syncStatus?.remoteBaseURL ?? syncRemoteBaseURLDraft
        isEditingSyncRemoteBaseURL = true
    }

    func cancelSyncRemoteBaseURLEdit() {
        syncRemoteBaseURLDraft = syncStatus?.remoteBaseURL ?? ""
        isEditingSyncRemoteBaseURL = false
    }

    func saveSyncRemoteBaseURL() async {
        let trimmed = syncRemoteBaseURLDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        await mutate {
            let status = try await BifrostClient(baseURL: self.baseURL)
                .updateSyncConfig(UpdateSyncConfigRequest(remoteBaseURL: trimmed))
            self.applySyncStatus(status, forceDraft: true)
            self.isEditingSyncRemoteBaseURL = false
        }
    }

    func installCertificate() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            self.certInfo = try await client.installLocalCA()
            try await self.refreshTrustProbeSession(client: client, forceCreate: false)
        }
    }

    func refreshMobileDevices() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            self.mobileDevices = try await client.refreshMobileDevices()
            try await self.refreshTrustProbeSession(client: client, forceCreate: false)
        }
    }

    func regenerateTrustProbe() async {
        await mutate {
            let client = try BifrostClient(baseURL: self.baseURL)
            try await self.refreshTrustProbeSession(client: client, forceCreate: true)
        }
    }

    func copyTrustProbeURL() {
        guard let url = trustProbeSession?.landingURL else {
            return
        }
        copyToPasteboard(url)
        copiedProbeURLAt = Date()
    }

    func openTrustProbeURL() {
        guard let value = trustProbeSession?.landingURL,
              let url = URL(string: value) else {
            return
        }
        NSWorkspace.shared.open(url)
    }

    var preferredTrustProbeHost: String? {
        proxyAddressInfo?.addresses.first(where: \.isPreferred)?.ip
            ?? proxyAddressInfo?.localIPs.first
            ?? certInfo?.localIPs.first
    }

    var detectedMobileDevices: [MobileDevice] {
        (mobileDevices?.ios?.devices ?? []) + (mobileDevices?.android?.devices ?? [])
    }

    private func refreshTrustProbeSession(client: BifrostClient, forceCreate: Bool) async throws {
        guard let host = preferredTrustProbeHost else {
            trustProbeSession = nil
            stopTrustProbePolling()
            return
        }
        if !forceCreate,
           let session = trustProbeSession,
           session.host == host {
            if let refreshed = try? await client.fetchTrustProbeSession(sessionID: session.sessionID) {
                applyTrustProbeSession(refreshed)
                return
            }
        }
        applyTrustProbeSession(try await client.createTrustProbeSession(host: host, ttlSeconds: 600))
    }

    private func applyTrustProbeSession(_ session: TrustProbeSession) {
        trustProbeSession = session
        if session.status == "expired" || !trustProbePollingActive {
            stopTrustProbePolling()
        } else {
            startTrustProbePolling(sessionID: session.sessionID)
        }
    }

    private func startTrustProbePolling(sessionID: String) {
        guard trustProbePollingActive else {
            return
        }
        if pollingTrustProbeSessionID == sessionID,
           trustProbePollingTask?.isCancelled == false {
            return
        }
        stopTrustProbePolling()
        pollingTrustProbeSessionID = sessionID
        let baseURL = baseURL
        trustProbePollingTask = Task { [weak self] in
            let client = try? BifrostClient(baseURL: baseURL)
            while !Task.isCancelled {
                do {
                    guard let client else {
                        return
                    }
                    let session = try await client.fetchTrustProbeSession(sessionID: sessionID)
                    await MainActor.run {
                        guard self?.trustProbeSession?.sessionID == sessionID else {
                            return
                        }
                        self?.trustProbeSession = session
                        if session.status == "expired" {
                            self?.stopTrustProbePolling()
                        }
                    }
                    try await Task.sleep(nanoseconds: 5_000_000_000)
                } catch is CancellationError {
                    return
                } catch {
                    await MainActor.run {
                        guard self?.trustProbeSession?.sessionID == sessionID else {
                            return
                        }
                        self?.errorMessage = error.localizedDescription
                    }
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                }
            }
        }
    }

    private func stopTrustProbePolling() {
        trustProbePollingTask?.cancel()
        trustProbePollingTask = nil
        pollingTrustProbeSessionID = nil
    }

    private func applySyncStatus(_ status: SyncStatus, forceDraft: Bool = false) {
        syncStatus = status
        if forceDraft || !isEditingSyncRemoteBaseURL {
            syncRemoteBaseURLDraft = status.remoteBaseURL
        }
    }

    private func copyToPasteboard(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }

    func formatRemoteTime(_ flexible: FlexibleString?) -> String {
        guard let value = flexible?.value else {
            return "-"
        }
        return value
    }

    func formatRemoteTime(_ value: Double?) -> String {
        guard let value else {
            return "-"
        }
        return Date(timeIntervalSince1970: value).formatted(date: .omitted, time: .shortened)
    }

    var remoteInvokeCallCountText: String {
        "\(remoteInvokeCalls?.calls.count ?? 0)"
    }

    var remoteInvokeClientCountText: String {
        "\(remoteInvokeGrants?.grants.count ?? 0)"
    }

    var remoteInvokeRecentActivity: String {
        let grantTimes = remoteInvokeGrants?.grants.compactMap(\.lastUsedAt) ?? []
        let callTimes = remoteInvokeCalls?.calls.compactMap { $0.finishedAt ?? $0.createdAt } ?? []
        guard let latest = (grantTimes + callTimes).max() else {
            return "暂无"
        }
        return Date(timeIntervalSince1970: latest).formatted(date: .abbreviated, time: .shortened)
    }

    private func mutate(_ operation: @escaping () async throws -> Void) async {
        isMutating = true
        defer { isMutating = false }
        do {
            try await operation()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

private struct RemoteInvokeCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "Remote Invoke",
                        subtitle: "SSH Key 授权、客户端与调用观测"
                    )
                    Spacer()
                    StatusPill(title: remoteStatusTitle, color: remoteStatusColor)
                    Toggle("", isOn: Binding(
                        get: { model.remoteInvokeStatus?.discoverySession != nil },
                        set: { enabled in Task { await model.setRemoteDiscoveryEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.remoteInvokeStatus == nil || model.isMutating)
                }

                if let session = model.remoteInvokeStatus?.discoverySession {
                    RemoteDiscoveryCodeStrip(session: session, model: model)
                }

                AdaptiveFactGrid(minimum: 126) {
                    CompactFact(title: "已授权客户端", value: model.remoteInvokeClientCountText)
                    CompactFact(title: "活动调用", value: "\(model.remoteInvokeStatus?.activeCallIDs.count ?? 0)")
                    CompactFact(title: "最近调用", value: model.remoteInvokeCallCountText)
                    CompactFact(title: "最近活跃", value: model.remoteInvokeRecentActivity)
                }

                ViewThatFits(in: .horizontal) {
                    remoteInvokeBody
                    VStack(alignment: .leading, spacing: 14) {
                        sshKeySection
                        clientSection
                    }
                }

                if let calls = model.remoteInvokeCalls?.calls, !calls.isEmpty {
                    Divider()
                    AdaptiveFactGrid(minimum: 126) {
                        ForEach(calls.prefix(3)) { call in
                            CompactFact(
                                title: call.callerDisplayName ?? call.commandKind ?? "调用",
                                value: call.status
                            )
                        }
                    }
                }
            }
        }
    }

    private var remoteInvokeBody: some View {
        HStack(alignment: .top, spacing: 14) {
            sshKeySection
                .frame(maxWidth: .infinity, alignment: .leading)
            clientSection
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var sshKeySection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("SSH Key")
                .font(.system(size: 13, weight: .semibold))
            HStack(spacing: 8) {
                StatusPill(
                    title: model.remoteInvokeSshKey?.status ?? "未生成",
                    color: model.remoteInvokeSshKey == nil ? .orange : .green
                )
                Text(shortFingerprint)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
            }
            ViewThatFits(in: .horizontal) {
                sshKeyButtons
                VStack(alignment: .leading, spacing: 8) {
                    sshKeyButtons
                }
            }
        }
    }

    private var sshKeyButtons: some View {
        HStack(spacing: 8) {
            Button(model.remoteInvokeSshKey == nil ? "生成 SSH Key" : "重新生成") {
                Task { await model.createRemoteInvokeSshKey() }
            }
            .buttonStyle(.bordered)
            .disabled(model.isMutating)
            Button {
                Task { await model.copyRemoteInvokeSshKey() }
            } label: {
                Label("复制 SSH Key", systemImage: "doc.on.doc")
                    .labelStyle(.titleAndIcon)
            }
            .buttonStyle(.bordered)
            .disabled(model.remoteInvokeSshKey == nil || model.isMutating)
            if sshKeyRecentlyCopied {
                Text("已复制")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var clientSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("客户端")
                .font(.system(size: 13, weight: .semibold))
            if let grants = model.remoteInvokeGrants?.grants, !grants.isEmpty {
                ForEach(grants.prefix(3)) { grant in
                    RemoteInvokeGrantRow(grant: grant)
                }
            } else {
                Text("暂无已授权客户端")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .frame(height: 38, alignment: .center)
            }
        }
    }

    private var remoteStatusTitle: String {
        guard let status = model.remoteInvokeStatus else {
            return "读取中"
        }
        if status.discoverySession != nil {
            return "发现中"
        }
        return status.state
    }

    private var remoteStatusColor: Color {
        guard let status = model.remoteInvokeStatus else {
            return .secondary
        }
        if status.discoverySession != nil || status.state.lowercased().contains("connected") {
            return .green
        }
        return .secondary
    }

    private var shortFingerprint: String {
        guard let value = model.remoteInvokeSshKey?.sshKeyFingerprint, !value.isEmpty else {
            return "尚未生成"
        }
        return String(value.prefix(22))
    }

    private var sshKeyRecentlyCopied: Bool {
        guard let copiedAt = model.copiedSshKeyAt,
              Date().timeIntervalSince(copiedAt) < 3 else {
            return false
        }
        return true
    }
}

private struct RemoteDiscoveryCodeStrip: View {
    let session: DiscoverySession
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("授权码")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                    Button {
                        model.copyRemotePairCode()
                    } label: {
                        HStack(spacing: 8) {
                            Text(session.pairCode)
                                .font(.system(size: 28, weight: .semibold, design: .monospaced))
                            Image(systemName: "doc.on.doc")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("复制授权码")
                }

                if pairCodeRecentlyCopied {
                    Text("已复制")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.secondary)
                }

                Spacer(minLength: 12)

                VStack(alignment: .trailing, spacing: 6) {
                    Text(remainingText(now: context.date))
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(remainingSeconds(now: context.date) > 0 ? Color.secondary : Color.orange)
                    Button {
                        Task { await model.refreshRemotePairCode() }
                    } label: {
                        Label("重置", systemImage: "arrow.triangle.2.circlepath")
                    }
                    .buttonStyle(.bordered)
                    .disabled(model.isMutating)
                }
            }
            .padding(.vertical, 12)
            .padding(.horizontal, 14)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
        }
    }

    private var pairCodeRecentlyCopied: Bool {
        guard let copiedAt = model.copiedPairCodeAt else {
            return false
        }
        return Date().timeIntervalSince(copiedAt) < 3
    }

    private func remainingText(now: Date) -> String {
        let seconds = remainingSeconds(now: now)
        guard seconds > 0 else {
            return "已过期"
        }
        return String(format: "剩余 %02d:%02d", seconds / 60, seconds % 60)
    }

    private func remainingSeconds(now: Date) -> Int {
        let expiresAt = normalizedDate(from: session.expiresAt)
        return max(0, Int(ceil(expiresAt.timeIntervalSince(now))))
    }

    private func normalizedDate(from epoch: Double) -> Date {
        if epoch > 1_000_000_000_000 {
            return Date(timeIntervalSince1970: epoch / 1000)
        }
        return Date(timeIntervalSince1970: epoch)
    }
}

private struct SystemProxyCard: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var copiedAddress: String?
    @State private var copiedAt: Date?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack(alignment: .top) {
                    NativeCardHeader(title: "系统代理", subtitle: systemProxySubtitle)
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { systemProxyEnabledByBifrost },
                        set: { enabled in Task { await appModel.setSystemProxyEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(systemProxyToggleDisabled)
                }

                StatusPill(title: systemProxyStatusTitle, color: systemProxyStatusColor)

                VStack(alignment: .leading, spacing: 8) {
                    Text("代理地址")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .textCase(.uppercase)
                    if proxyAddresses.isEmpty {
                        ProxyAddressCopyRow(
                            title: "本机",
                            address: appModel.adminHostPortLabel,
                            isPreferred: true,
                            isCopied: copiedAddress == appModel.adminHostPortLabel && recentlyCopied
                        ) {
                            copyProxyAddress(appModel.adminHostPortLabel)
                        }
                    } else {
                        ForEach(proxyAddresses) { address in
                            ProxyAddressCopyRow(
                                title: address.isPreferred ? "推荐" : address.ip,
                                address: address.address,
                                isPreferred: address.isPreferred,
                                isCopied: copiedAddress == address.address && recentlyCopied
                            ) {
                                copyProxyAddress(address.address)
                            }
                        }
                    }
                }

                Divider()
                    .opacity(0.55)

                VStack(spacing: 9) {
                    SystemProxyOptionToggleRow(
                        title: "Boot/Shutdown Cleanup",
                        detail: launchdDetail,
                        isOn: launchdEnabled,
                        isDisabled: launchdToggleDisabled
                    ) { enabled in
                        Task { await appModel.setSystemProxyLaunchdEnabled(enabled) }
                    }

                    SystemProxyOptionToggleRow(
                        title: "Inject Bifrost Badge",
                        detail: "只应用于 HTML 页面，用来标记流量正在经过 Bifrost。",
                        isOn: appModel.performanceConfig?.traffic.injectBifrostBadge ?? false,
                        isDisabled: appModel.performanceConfig == nil || appModel.isTogglingInjectBifrostBadge
                    ) { enabled in
                        Task { await appModel.setInjectBifrostBadgeEnabled(enabled) }
                    }

                    SystemProxyOptionStatusRow(
                        title: "CLI Proxy (ENV)",
                        detail: cliProxyDetail,
                        status: appModel.cliProxyStatus?.enabled == true ? "Enabled" : "Disabled",
                        color: appModel.cliProxyStatus?.enabled == true ? .green : .secondary
                    )
                }
            }
        }
    }

    private var proxyAddresses: [ProxyAddress] {
        appModel.proxyAddressInfo?.addresses ?? []
    }

    private var systemProxyToggleDisabled: Bool {
        !(appModel.systemProxyStatus?.supported ?? false) || appModel.isTogglingSystemProxy
    }

    private var systemProxyEnabledByBifrost: Bool {
        guard let status = appModel.systemProxyStatus else {
            return false
        }
        return status.enabled && status.managedByBifrost != false
    }

    private var systemProxyStatusTitle: String {
        guard let status = appModel.systemProxyStatus else {
            return "读取中"
        }
        guard status.supported else {
            return "不支持"
        }
        if status.enabled && status.managedByBifrost == false {
            return "被其他代理占用"
        }
        if status.configuredEnabled == true && !systemProxyEnabledByBifrost {
            return "已配置待接管"
        }
        return systemProxyEnabledByBifrost ? "已接管" : "未接管"
    }

    private var systemProxyStatusColor: Color {
        guard let status = appModel.systemProxyStatus else {
            return .secondary
        }
        if status.enabled && status.managedByBifrost == false {
            return .orange
        }
        if status.configuredEnabled == true && !systemProxyEnabledByBifrost {
            return .orange
        }
        return systemProxyEnabledByBifrost ? .green : .secondary
    }

    private var systemProxySubtitle: String {
        if let info = appModel.proxyAddressInfo,
           let preferred = info.addresses.first(where: \.isPreferred) ?? info.addresses.first {
            return preferred.address
        }
        if let status = appModel.systemProxyStatus,
           let host = status.host,
           let port = status.port {
            return "\(host):\(port)"
        }
        return appModel.adminHostPortLabel
    }

    private var launchdEnabled: Bool {
        guard let launchd = appModel.systemProxyLaunchdStatus else {
            return false
        }
        return launchd.installed && launchd.loaded && launchd.needsUpgrade != true
    }

    private var launchdToggleDisabled: Bool {
        !(appModel.systemProxyLaunchdStatus?.supported ?? false) || appModel.isTogglingSystemProxyLaunchd
    }

    private var launchdDetail: String {
        guard let launchd = appModel.systemProxyLaunchdStatus else {
            return "读取中"
        }
        if launchd.needsUpgrade == true {
            return launchd.needsUpgradeReason ?? launchd.message ?? "清理组件需要升级。"
        }
        return launchd.message ?? "崩溃恢复和重启后恢复 Bifrost 管理的系统代理设置。"
    }

    private var cliProxyDetail: String {
        guard let cli = appModel.cliProxyStatus else {
            return "读取中"
        }
        let shell = cli.shell ?? "-"
        let files = cli.configFiles.prefix(2).map { URL(fileURLWithPath: $0).lastPathComponent }.joined(separator: ", ")
        return "Shell: \(shell) · Files: \(files.isEmpty ? "-" : files)"
    }

    private var recentlyCopied: Bool {
        guard let copiedAt else {
            return false
        }
        return Date().timeIntervalSince(copiedAt) < 3
    }

    private func copyProxyAddress(_ address: String) {
        appModel.copyToPasteboard(address)
        copiedAddress = address
        copiedAt = Date()
    }
}

private struct ProxyAddressCopyRow: View {
    let title: String
    let address: String
    let isPreferred: Bool
    let isCopied: Bool
    let onCopy: () -> Void

    var body: some View {
        Button(action: onCopy) {
            HStack(spacing: 8) {
                Image(systemName: isPreferred ? "star.fill" : "network")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(isPreferred ? Color.yellow : Color.secondary)
                    .frame(width: 14)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Text(address)
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Label(isCopied ? "已复制" : "复制", systemImage: isCopied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(isCopied ? Color.green : Color.secondary)
                    .labelStyle(.titleAndIcon)
            }
            .padding(.vertical, 7)
            .padding(.horizontal, 9)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
    }
}

private struct SystemProxyOptionToggleRow: View {
    let title: String
    let detail: String
    let isOn: Bool
    let isDisabled: Bool
    let onToggle: (Bool) -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Toggle("", isOn: Binding(get: { isOn }, set: onToggle))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(isDisabled)
        }
    }
}

private struct SystemProxyOptionStatusRow: View {
    let title: String
    let detail: String
    let status: String
    let color: Color

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(detail)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            StatusPill(title: status, color: color)
        }
    }
}

private struct TlsInterceptionCard: View {
    @EnvironmentObject private var appModel: AppModel
    @State private var editingKind: TlsListKind?

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "TLS 解密",
                        subtitle: "打开为默认解包，关闭为白名单解包"
                    )
                    Spacer()
                    StatusPill(
                        title: tlsModeTitle,
                        color: tlsModeColor
                    )
                    Toggle("", isOn: Binding(
                        get: { appModel.tlsConfig?.enableTlsInterception ?? false },
                        set: { enabled in Task { await appModel.setTlsInterceptionEnabled(enabled) } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(appModel.tlsConfig == nil || appModel.isTogglingTls)
                }

                LazyVGrid(columns: [
                    GridItem(.adaptive(minimum: 112, maximum: 180), spacing: 10, alignment: .topLeading)
                ], alignment: .leading, spacing: 10) {
                    ForEach(TlsListKind.allCases) { kind in
                        Button {
                            editingKind = kind
                        } label: {
                            TlsListCountTile(
                                title: kind.title,
                                value: "\(kind.values(in: appModel.tlsConfig).count)",
                                tint: kind.isInclude ? .green : .orange
                            )
                        }
                        .buttonStyle(.plain)
                        .disabled(appModel.tlsConfig == nil || appModel.isTogglingTls)
                    }
                }
            }
        }
        .sheet(item: $editingKind) { kind in
            TlsListEditorSheet(
                kind: kind,
                values: kind.values(in: appModel.tlsConfig),
                isSaving: appModel.isTogglingTls
            ) { values in
                guard var config = appModel.tlsConfig else {
                    return
                }
                kind.update(&config, values: values)
                Task {
                    await appModel.updateTlsConfig(config)
                }
            }
        }
    }

    private var tlsModeTitle: String {
        guard let config = appModel.tlsConfig else {
            return "读取中"
        }
        return config.enableTlsInterception ? "默认解包" : "白名单解包"
    }

    private var tlsModeColor: Color {
        guard let config = appModel.tlsConfig else {
            return .secondary
        }
        return config.enableTlsInterception ? .green : .blue
    }
}

private enum TlsListKind: String, CaseIterable, Identifiable {
    case appInclude
    case appExclude
    case domainInclude
    case domainExclude
    case ipInclude
    case ipExclude

    var id: String { rawValue }

    var title: String {
        switch self {
        case .appInclude:
            return "应用白名单"
        case .appExclude:
            return "应用黑名单"
        case .domainInclude:
            return "域名白名单"
        case .domainExclude:
            return "域名黑名单"
        case .ipInclude:
            return "IP 白名单"
        case .ipExclude:
            return "IP 黑名单"
        }
    }

    var editorTitle: String {
        "\(title)编辑"
    }

    var placeholder: String {
        switch self {
        case .appInclude, .appExclude:
            return "Safari\nGoogle Chrome\ncom.apple.Safari"
        case .domainInclude, .domainExclude:
            return "*.example.com\napi.example.com"
        case .ipInclude, .ipExclude:
            return "10.0.0.0/8\n192.168.1.20"
        }
    }

    var isInclude: Bool {
        switch self {
        case .appInclude, .domainInclude, .ipInclude:
            return true
        case .appExclude, .domainExclude, .ipExclude:
            return false
        }
    }

    func values(in config: TlsConfig?) -> [String] {
        guard let config else {
            return []
        }
        switch self {
        case .appInclude:
            return config.appInterceptInclude
        case .appExclude:
            return config.appInterceptExclude
        case .domainInclude:
            return config.interceptInclude
        case .domainExclude:
            return config.interceptExclude
        case .ipInclude:
            return config.ipInterceptInclude
        case .ipExclude:
            return config.ipInterceptExclude
        }
    }

    func update(_ config: inout TlsConfig, values: [String]) {
        switch self {
        case .appInclude:
            config.appInterceptInclude = values
        case .appExclude:
            config.appInterceptExclude = values
        case .domainInclude:
            config.interceptInclude = values
        case .domainExclude:
            config.interceptExclude = values
        case .ipInclude:
            config.ipInterceptInclude = values
        case .ipExclude:
            config.ipInterceptExclude = values
        }
    }
}

private struct TlsListCountTile: View {
    let title: String
    let value: String
    let tint: Color

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(value)
                    .font(.system(size: 17, weight: .semibold))
            }
            Spacer()
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 11)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}

private struct TlsListEditorSheet: View {
    let kind: TlsListKind
    let isSaving: Bool
    let onSave: ([String]) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var draftText: String

    init(kind: TlsListKind, values: [String], isSaving: Bool, onSave: @escaping ([String]) -> Void) {
        self.kind = kind
        self.isSaving = isSaving
        self.onSave = onSave
        _draftText = State(initialValue: values.joined(separator: "\n"))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                NativeCardHeader(title: kind.editorTitle, subtitle: "每行一个匹配项，保存后立即影响 TLS 解包范围")
                Spacer()
                Button("取消") {
                    dismiss()
                }
                .buttonStyle(.borderless)
                Button("保存") {
                    onSave(normalizedValues)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .disabled(isSaving)
            }

            ZStack(alignment: .topLeading) {
                TextEditor(text: $draftText)
                    .font(.system(size: 13, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .padding(8)
                    .background(AppSurface.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .stroke(AppSurface.cardBorder)
                    )
                if draftText.isEmpty {
                    Text(kind.placeholder)
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .padding(.top, 16)
                        .padding(.leading, 14)
                        .allowsHitTesting(false)
                }
            }
            .frame(minWidth: 520, minHeight: 260)
        }
        .padding(22)
        .background(AppSurface.content)
    }

    private var normalizedValues: [String] {
        var seen = Set<String>()
        return draftText
            .split(whereSeparator: \.isNewline)
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .filter { seen.insert($0).inserted }
    }
}

private struct SyncControlCard: View {
    @EnvironmentObject private var appModel: AppModel
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    NativeCardHeader(
                        title: "同步",
                        subtitle: model.syncStatus?.user?.email ?? "规则与设置远端同步"
                    )
                    Spacer()
                    Toggle("", isOn: Binding(
                        get: { model.syncStatus?.enabled ?? false },
                        set: { enabled in Task { await model.setSyncEnabled(enabled); await appModel.refreshSyncStatus() } }
                    ))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .disabled(model.syncStatus == nil || model.isMutating)
                }
                SyncRemoteServiceRow(model: model)
                ViewThatFits(in: .horizontal) {
                    syncControls
                    VStack(alignment: .leading, spacing: 10) {
                        syncControls
                    }
                }
            }
        }
    }

    private var syncControls: some View {
        HStack(spacing: 10) {
            StatusPill(
                title: model.syncStatus?.authorized == true ? "已授权" : "未授权",
                color: model.syncStatus?.authorized == true ? .green : .orange
            )
            Toggle("自动同步", isOn: Binding(
                get: { model.syncStatus?.autoSync ?? false },
                set: { enabled in Task { await model.setAutoSyncEnabled(enabled); await appModel.refreshSyncStatus() } }
            ))
            .toggleStyle(.switch)
            .font(.system(size: 12))
            .disabled(model.syncStatus == nil || model.isMutating)
            Spacer(minLength: 8)
            Button(model.syncStatus?.hasSession == true ? "退出" : "登录") {
                Task {
                    if model.syncStatus?.hasSession == true {
                        await model.logoutSync()
                    } else {
                        await model.openSyncLogin()
                    }
                    await appModel.refreshSyncStatus()
                }
            }
            .buttonStyle(.borderless)
            Button("同步") {
                Task { await model.runSyncNow() }
            }
            .buttonStyle(.borderless)
            .disabled(model.syncStatus?.enabled != true || model.isMutating)
        }
    }
}

private struct SyncRemoteServiceRow: View {
    @ObservedObject var model: OverviewControlModel
    @FocusState private var isFocused: Bool

    var body: some View {
        Group {
            if model.isEditingSyncRemoteBaseURL {
                editingRow
            } else {
                readRow
            }
        }
        .animation(.snappy(duration: 0.18), value: model.isEditingSyncRemoteBaseURL)
    }

    private var readRow: some View {
        Button {
            Task { await model.handleSyncRemoteBaseURLClick() }
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("远端服务")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Text(remoteBaseURLText)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Label(isSignedIn ? "编辑" : "登录授权", systemImage: isSignedIn ? "pencil" : "person.crop.circle.badge.checkmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .labelStyle(.titleAndIcon)
            }
            .padding(.vertical, 9)
            .padding(.horizontal, 11)
            .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(model.syncStatus == nil || model.isMutating)
        .help(isSignedIn ? "编辑远端服务地址" : "登录并授权远端同步")
    }

    private var editingRow: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 5) {
                Text("远端服务")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                TextField("https://bifrost.example.com", text: $model.syncRemoteBaseURLDraft)
                    .textFieldStyle(.plain)
                    .font(.system(size: 12, design: .monospaced))
                    .focused($isFocused)
                    .onSubmit {
                        Task { await model.saveSyncRemoteBaseURL() }
                    }
            }
            Button {
                Task { await model.saveSyncRemoteBaseURL() }
            } label: {
                Image(systemName: "checkmark")
            }
            .buttonStyle(.borderless)
            .disabled(model.isMutating)
            .help("保存远端服务地址")
            Button {
                model.cancelSyncRemoteBaseURLEdit()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .disabled(model.isMutating)
            .help("取消编辑")
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 11)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onAppear {
            isFocused = true
        }
    }

    private var remoteBaseURLText: String {
        guard let value = model.syncStatus?.remoteBaseURL, !value.isEmpty else {
            return "未配置"
        }
        return value
    }

    private var isSignedIn: Bool {
        model.syncStatus?.hasSession == true
    }
}

private struct CertificateManagementCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "证书管理",
                        subtitle: model.certInfo?.statusMessage ?? "安装并验证本机 CA"
                    )
                    Spacer()
                    StatusPill(
                        title: model.certInfo?.trusted == true ? "已信任" : "未信任",
                        color: model.certInfo?.trusted == true ? .green : .orange
                    )
                }

                CertificateSummarySection(model: model, fingerprintText: fingerprintText)
            }
        }
    }

    private var fingerprintText: String {
        guard let value = model.certInfo?.sha256Fingerprint, !value.isEmpty else {
            return "SHA256: -"
        }
        return "SHA256: \(value)"
    }
}

private struct CertificateSummarySection: View {
    @ObservedObject var model: OverviewControlModel
    let fingerprintText: String

    private var factColumns: [GridItem] {
        [
            GridItem(.adaptive(minimum: 118, maximum: 180), spacing: 10, alignment: .topLeading)
        ]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            LazyVGrid(columns: factColumns, alignment: .leading, spacing: 10) {
                CompactFact(title: "本机 CA", value: model.certInfo?.statusLabel ?? "读取中")
                CompactFact(title: "代理地址", value: model.proxyAddressInfo?.addresses.first(where: \.isPreferred)?.address ?? "-")
            }

            Text(fingerprintText)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)

            if shouldShowInstallButton {
                Button("安装本机 CA") {
                    Task { await model.installCertificate() }
                }
                .buttonStyle(.bordered)
                .disabled(model.certInfo == nil || model.certInfo?.available == false || model.isMutating)
            }
        }
    }

    private var shouldShowInstallButton: Bool {
        model.certInfo?.trusted != true
    }
}

private struct RemoteInvokeGrantRow: View {
    let grant: Grant

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(grant.status == "active" ? Color.green : Color.secondary.opacity(0.45))
                .frame(width: 7, height: 7)
            VStack(alignment: .leading, spacing: 2) {
                Text(grant.callerDisplayName ?? String(grant.callerFingerprint.prefix(10)))
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text("调用 \(grant.useCount ?? 0) 次")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text(grant.grantScope)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }
}

private struct MobileDeviceRow: View {
    let device: MobileDevice

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: device.platform.lowercased().contains("ios") ? "iphone" : "smartphone")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(device.name ?? device.id)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text(device.certificateStatus?.message ?? device.statusMessage)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            StatusPill(
                title: device.certificateStatus?.trusted == true ? "已信任" : device.status,
                color: device.certificateStatus?.trusted == true ? .green : .orange
            )
        }
    }
}

private struct MobileConnectionCheckCard: View {
    @ObservedObject var model: OverviewControlModel

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(
                        title: "移动端连接检查",
                        subtitle: "扫码检查同网、证书与代理连接"
                    )
                    Spacer()
                    StatusPill(title: probeStatusTitle, color: probeStatusColor)
                }

                HStack(alignment: .top, spacing: 18) {
                    QRPreview(urlString: model.trustProbeSession?.landingURL)
                    mobileProbeDetails
                }
            }
        }
    }

    private var mobileProbeDetails: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(model.trustProbeSession?.landingURL ?? "等待生成检查链接")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .truncationMode(.middle)

            HStack(spacing: 8) {
                Button("打开") {
                    model.openTrustProbeURL()
                }
                .buttonStyle(.borderless)
                .disabled(model.trustProbeSession == nil)
                Button(model.copiedProbeURLAt.map { Date().timeIntervalSince($0) < 3 } == true ? "已复制" : "复制链接") {
                    model.copyTrustProbeURL()
                }
                .buttonStyle(.borderless)
                .disabled(model.trustProbeSession == nil)
                Spacer()
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("已连接设备")
                    .font(.system(size: 13, weight: .semibold))
                if model.detectedMobileDevices.isEmpty {
                    Text("暂无 USB 设备。可让手机扫描二维码检查同网、证书与代理连接。")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    ForEach(model.detectedMobileDevices.prefix(3)) { device in
                        MobileDeviceRow(device: device)
                    }
                }
            }

            if let devices = model.trustProbeSession?.devices, !devices.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 6) {
                        Text("扫码设备")
                            .font(.system(size: 13, weight: .semibold))
                        Text("\(devices.count)")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.quaternary.opacity(0.7), in: Capsule())
                    }
                    ForEach(devices.prefix(4)) { device in
                        TrustProbeDeviceRow(device: device)
                    }
                }
            } else {
                Text("扫码后会在这里显示正在连接的设备。")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }

            if let session = model.trustProbeSession,
               !session.proxyConfigured,
               let message = sessionProbeMessage(for: session) {
                TrustProbeMessageRow(symbol: "exclamationmark.triangle.fill", tint: .orange, message: message)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    private func sessionProbeMessage(for session: TrustProbeSession) -> String? {
        guard let message = normalizedTrustProbeMessage(session.proxyConfigurationMessage) else {
            return nil
        }
        let deviceMessages = Set(
            session.devices.flatMap { device in
                [
                    normalizedTrustProbeMessage(device.proxyConfigurationMessage),
                    normalizedTrustProbeMessage(device.lastError),
                    normalizedTrustProbeMessage(device.proxyAccessMessage),
                ].compactMap { $0 }
            }
        )
        return deviceMessages.contains(message) ? nil : message
    }

    private var probeStatusTitle: String {
        guard let session = model.trustProbeSession else {
            return "未生成"
        }
        if session.tlsTrusted {
            return "证书可信"
        }
        if session.networkReachable {
            return "网络可达"
        }
        if session.opened {
            return "已打开"
        }
        return session.status
    }

    private var probeStatusColor: Color {
        guard let session = model.trustProbeSession else {
            return .orange
        }
        if session.tlsTrusted {
            return .green
        }
        if session.networkReachable {
            return .blue
        }
        return .orange
    }
}

private struct QRPreview: View {
    let urlString: String?
    @State private var qrImage: NSImage?
    @State private var isLoading = false
    @State private var didFail = false

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(.white)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(AppSurface.cardBorder)
                )
            if let qrImage {
                Image(nsImage: qrImage)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .padding(12)
            } else if isLoading {
                ProgressView()
            } else {
                Image(systemName: "qrcode")
                    .font(.system(size: 42, weight: .regular))
                    .foregroundStyle(didFail ? AnyShapeStyle(.orange.opacity(0.7)) : AnyShapeStyle(.tertiary))
            }
        }
        .frame(width: 136, height: 136)
        .task(id: urlString) {
            await loadQRCode()
        }
    }

    private func loadQRCode() async {
        guard let value = urlString,
              !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            await MainActor.run {
                qrImage = nil
                isLoading = false
                didFail = urlString != nil
            }
            return
        }

        await MainActor.run {
            qrImage = nil
            didFail = false
            isLoading = true
        }

        let image = makeQRCodeImage(from: value)
        await MainActor.run {
            qrImage = image
            isLoading = false
            didFail = image == nil
        }
    }

    private func makeQRCodeImage(from value: String) -> NSImage? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else {
            return nil
        }
        filter.setValue(Data(value.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let outputImage = filter.outputImage else {
            return nil
        }

        let scaledImage = outputImage.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        let representation = NSCIImageRep(ciImage: scaledImage)
        let image = NSImage(size: representation.size)
        image.addRepresentation(representation)
        return image
    }
}

private struct TrustProbeDeviceRow: View {
    let device: TrustProbeDevice

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(device.tlsTrusted ? Color.green : (device.networkReachable ? Color.blue : Color.orange))
                .frame(width: 7, height: 7)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(shortDeviceID)
                        .font(.system(size: 11, weight: .semibold, design: .monospaced))
                        .lineLimit(1)
                    if let platform = device.platformHint, !platform.isEmpty {
                        Text(platform)
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.quaternary.opacity(0.65), in: Capsule())
                    }
                    if let ip = device.clientIP, !ip.isEmpty {
                        Text(ip)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    Text(formatProbeLastSeen(device.lastSeen))
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }

                HStack(spacing: 5) {
                    TrustProbeDeviceStatusTag(title: device.opened ? "已打开" : "等待打开", color: device.opened ? .green : .gray)
                    TrustProbeDeviceStatusTag(title: device.networkReachable ? "网络可达" : "网络待检", color: device.networkReachable ? .green : .gray)
                    TrustProbeDeviceStatusTag(title: device.tlsTrusted ? "证书可信" : tlsPendingTitle, color: tlsStatusColor)
                    TrustProbeDeviceStatusTag(title: proxyAccessTitle, color: proxyAccessColor)
                    TrustProbeDeviceStatusTag(title: device.proxyConfigured ? "代理已配置" : proxyConfiguredTitle, color: device.proxyConfigured ? .green : proxyConfiguredColor)
                }

                if let message = probeMessage {
                    Text(message)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private var shortDeviceID: String {
        guard device.deviceID.count > 10 else {
            return device.deviceID
        }
        return String(device.deviceID.prefix(8))
    }

    private var probeMessage: String? {
        var seen = Set<String>()
        for candidate in [
            device.proxyConfigurationMessage,
            device.lastError,
            device.proxyAccessMessage,
        ] {
            guard let message = normalizedTrustProbeMessage(candidate),
                  !seen.contains(message) else {
                continue
            }
            seen.insert(message)
            return message
        }
        return nil
    }

    private var tlsPendingTitle: String {
        device.status == "tls_failed" ? "证书失败" : "证书待检"
    }

    private var tlsStatusColor: Color {
        if device.tlsTrusted {
            return .green
        }
        if device.status == "tls_failed" {
            return .red
        }
        return .gray
    }

    private var proxyAccessTitle: String {
        guard let status = device.proxyAccessStatus, !status.isEmpty else {
            return "授权待检"
        }
        return status == "allowed" ? "授权通过" : "授权 \(status)"
    }

    private var proxyAccessColor: Color {
        if device.proxyAccessAllowed == true {
            return .green
        }
        return device.proxyAccessStatus == "pending" ? .orange : .gray
    }

    private var proxyConfiguredTitle: String {
        device.proxyConfigurationMessage?.isEmpty == false ? "代理缺失" : "代理待检"
    }

    private var proxyConfiguredColor: Color {
        device.proxyConfigurationMessage?.isEmpty == false ? .red : .gray
    }
}

private struct TrustProbeDeviceStatusTag: View {
    let title: String
    let color: Color

    var body: some View {
        Text(title)
            .font(.system(size: 9, weight: .medium))
            .foregroundStyle(color)
            .lineLimit(1)
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(color.opacity(0.10), in: Capsule())
    }
}

private struct TrustProbeMessageRow: View {
    let symbol: String
    let tint: Color
    let message: String

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(tint)
                .padding(.top, 1)
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private func normalizedTrustProbeMessage(_ value: String?) -> String? {
    let message = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard !message.isEmpty else {
        return nil
    }
    let normalized = message.lowercased()
    if normalized.contains("typeerror: load failed") || normalized.contains("load failed") {
        return "手机浏览器请求失败，请确认 Wi-Fi 代理已指向上方代理地址后重试。"
    }
    return message
}

private func formatProbeLastSeen(_ value: String) -> String {
    if let date = ISO8601DateFormatter().date(from: value) {
        return date.formatted(date: .omitted, time: .standard)
    }
    return value
}

private struct OverviewToggleCard: View {
    let title: String
    let subtitle: String
    let status: String
    let tint: Color
    let isOn: Bool
    let isDisabled: Bool
    let onToggle: (Bool) -> Void

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top) {
                    NativeCardHeader(title: title, subtitle: subtitle)
                    Spacer()
                    Toggle("", isOn: Binding(get: { isOn }, set: onToggle))
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .disabled(isDisabled)
                }
                StatusPill(title: status, color: tint)
            }
        }
    }
}

private struct ActivityBars: View, Equatable {
    let rows: [(String, Int)]

    static func == (lhs: ActivityBars, rhs: ActivityBars) -> Bool {
        guard lhs.rows.count == rhs.rows.count else {
            return false
        }
        for index in lhs.rows.indices where lhs.rows[index].0 != rhs.rows[index].0 || lhs.rows[index].1 != rhs.rows[index].1 {
            return false
        }
        return true
    }

    var body: some View {
        let maxValue = max(rows.map(\.1).max() ?? 1, 1)
        VStack(spacing: 10) {
            if rows.isEmpty {
                EmptyNativeState(title: "暂无流量")
                    .frame(height: 180)
            } else {
                ForEach(rows, id: \.0) { row in
                    HStack(spacing: 10) {
                        Text(row.0)
                            .font(.system(size: 12))
                            .lineLimit(1)
                            .frame(width: 150, alignment: .leading)
                        GeometryReader { proxy in
                            RoundedRectangle(cornerRadius: 3)
                                .fill(Color.accentColor.opacity(0.78))
                                .frame(width: max(4, proxy.size.width * CGFloat(row.1) / CGFloat(maxValue)))
                        }
                        .frame(height: 8)
                        Text("\(row.1)")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(.secondary)
                            .frame(width: 44, alignment: .trailing)
                    }
                }
            }
        }
    }
}

private struct NativeMetricCard: View {
    let title: String
    let value: String
    let caption: String
    let tint: Color

    var body: some View {
        NativeCard {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    Text(title)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Circle()
                        .fill(tint)
                        .frame(width: 8, height: 8)
                }
                Text(value)
                    .font(.system(size: 31, weight: .bold, design: .rounded))
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                Text(caption)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(minHeight: 126, alignment: .topLeading)
        }
    }
}

private func formatRate(_ value: Double?) -> String {
    guard let value else {
        return "0 B/s"
    }
    return "\(formatBytes(Int(value)))/s"
}

private func formatBytes(_ value: Int?) -> String {
    formatBytes(value ?? 0)
}

private func formatBytes(_ value: Int) -> String {
    let bytes = max(value, 0)
    if bytes < 1024 {
        return "\(bytes) B"
    }
    let units = ["KB", "MB", "GB", "TB"]
    var scaled = Double(bytes) / 1024
    var unitIndex = 0
    while scaled >= 1024, unitIndex < units.count - 1 {
        scaled /= 1024
        unitIndex += 1
    }
    if scaled >= 100 {
        return String(format: "%.0f %@", scaled, units[unitIndex])
    }
    return String(format: "%.1f %@", scaled, units[unitIndex])
}
