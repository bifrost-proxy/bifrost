import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case overview = "Overview"
    case traffic = "Traffic"
    case rules = "Rules"
    case scripts = "Scripts"
    case replay = "Replay"
    case devices = "Devices"
    case certificates = "Certificates"
    case metrics = "Metrics"
    case settings = "Settings"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .overview: return "gauge.with.dots.needle.33percent"
        case .traffic: return "list.bullet.rectangle"
        case .rules: return "slider.horizontal.3"
        case .scripts: return "curlybraces"
        case .replay: return "play.rectangle"
        case .devices: return "iphone.gen3.radiowaves.left.and.right"
        case .certificates: return "checkmark.seal"
        case .metrics: return "chart.xyaxis.line"
        case .settings: return "gearshape"
        }
    }
}

struct Sidebar: View {
    @Binding var selection: SidebarItem?

    var body: some View {
        List(SidebarItem.allCases, selection: $selection) { item in
            Label(item.rawValue, systemImage: item.systemImage)
                .tag(item)
        }
        .navigationTitle("Bifrost")
        .listStyle(.sidebar)
    }
}
