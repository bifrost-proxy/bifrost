import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case network = "Network"
    case rules = "Rules"
    case settings = "Settings"

    var id: String { rawValue }

    static var releaseScopeItems: [SidebarItem] {
        [.network, .rules, .settings]
    }

    var systemImage: String {
        switch self {
        case .network: return "globe"
        case .rules: return "doc.text"
        case .settings: return "gearshape"
        }
    }
}

struct PrimarySidebar: View {
    @EnvironmentObject private var appModel: AppModel
    @Binding var selection: SidebarItem

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
                .frame(height: 56)

            List(SidebarItem.releaseScopeItems) { item in
                Button {
                    selection = item
                } label: {
                    Label(item.rawValue, systemImage: item.systemImage)
                        .font(.system(size: 13, weight: .medium))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(selection == item ? Color.white : Color.primary)
                .listRowBackground(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(selection == item ? Color.accentColor : Color.clear)
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
        .background(.regularMaterial)
    }
}
