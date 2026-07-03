import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case activity = "活动"
    case overview = "概览"
    case rules = "规则"
    case network = "网络"

    var id: String { rawValue }

    static var releaseScopeItems: [SidebarItem] {
        [.activity, .overview, .rules, .network]
    }

    var systemImage: String {
        switch self {
        case .activity: return "waveform.path.ecg"
        case .overview: return "square.grid.2x2"
        case .network: return "globe"
        case .rules: return "doc.text"
        }
    }

    var needsTrafficRecords: Bool {
        switch self {
        case .activity:
            return true
        case .overview, .rules, .network:
            return false
        }
    }
}

struct PrimarySidebar: View {
    @EnvironmentObject private var appModel: AppModel
    @Binding var selection: SidebarItem

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
                .frame(height: 54)

            List(SidebarItem.releaseScopeItems) { item in
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

            Spacer(minLength: 12)

            Button {
                appModel.colorSchemeMode = appModel.colorSchemeMode.next
            } label: {
                Label(appModel.colorSchemeMode.rawValue, systemImage: appModel.colorSchemeMode.systemImage)
                    .font(.system(size: 12, weight: .medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
            .help("Toggle \(appModel.colorSchemeMode.next.rawValue) Theme")
        }
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(AppSurface.sidebar)
    }
}
