import SwiftUI

struct MainWindowScene: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        NavigationSplitView {
            Sidebar(selection: $appModel.selectedSidebarItem)
        } detail: {
            switch appModel.selectedSidebarItem ?? .overview {
            case .overview:
                DashboardView()
            case .traffic:
                TrafficView()
            case .rules:
                RulesView()
            case .scripts:
                PlaceholderFeatureView(title: "Scripts")
            case .replay:
                PlaceholderFeatureView(title: "Replay")
            case .certificates:
                PlaceholderFeatureView(title: "Certificates")
            case .devices:
                PlaceholderFeatureView(title: "Devices")
            case .metrics:
                PlaceholderFeatureView(title: "Metrics")
            case .settings:
                SettingsView()
            }
        }
        .frame(minWidth: 1180, minHeight: 760)
    }
}
