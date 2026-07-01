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
