import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case activity = "活动"
    case overview = "概览"
    case rules = "规则"
    case network = "抓包"
    case groups = "小组"

    var id: String { rawValue }

    static var releaseScopeItems: [SidebarItem] {
        [.activity, .overview, .rules, .network]
    }

    static func visibleItems(canShowGroups: Bool) -> [SidebarItem] {
        releaseScopeItems + (canShowGroups ? [.groups] : [])
    }

    var systemImage: String {
        switch self {
        case .activity: return "waveform.path.ecg"
        case .overview: return "square.grid.2x2"
        case .network: return "globe"
        case .rules: return "doc.text"
        case .groups: return "person.2"
        }
    }

    var needsTrafficRecords: Bool {
        switch self {
        case .network:
            return true
        case .activity, .overview, .rules, .groups:
            return false
        }
    }
}

struct PrimarySidebar: View, Equatable {
    @Binding var selection: SidebarItem
    var items: [SidebarItem]
    var colorSchemeMode: ColorSchemeMode
    var canShowGroupManagement: Bool
    var toggleColorScheme: () -> Void
    var ensureSelectionVisible: () -> Void

    static func == (lhs: PrimarySidebar, rhs: PrimarySidebar) -> Bool {
        lhs.selection == rhs.selection
            && lhs.items == rhs.items
            && lhs.colorSchemeMode == rhs.colorSchemeMode
            && lhs.canShowGroupManagement == rhs.canShowGroupManagement
    }

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
                .frame(height: 54)

            List(items) { item in
                Button {
                    selection = item
                } label: {
                    Label(item.rawValue, systemImage: item.systemImage)
                        .font(.system(size: 13, weight: .semibold))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(selection == item ? Color.primary : Color.secondary)
                .listRowBackground(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(selection == item ? AppSurface.sidebarSelection : Color.clear)
                )
                .help(item.rawValue)
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .animation(.easeInOut(duration: 0.12), value: selection)
            .animation(.easeInOut(duration: 0.16), value: canShowGroupManagement)

            Spacer(minLength: 12)

            Button {
                toggleColorScheme()
            } label: {
                Label(colorSchemeMode.rawValue, systemImage: colorSchemeMode.systemImage)
                    .font(.system(size: 12, weight: .medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
            .help("Toggle \(colorSchemeMode.next.rawValue) Theme")
        }
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(AppSurface.sidebar)
        .onAppear {
            ensureSelectionVisible()
        }
        .onChange(of: canShowGroupManagement) { _ in
            ensureSelectionVisible()
        }
    }
}
