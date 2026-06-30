import BifrostMacCore
import SwiftUI

@MainActor
final class AppModel: ObservableObject {
    @Published var sidecarState: SidecarState = .stopped
    @Published var selectedSidebarItem: SidebarItem? = .overview

    let defaultAdminURL = URL(string: "http://127.0.0.1:9900")!

    func openWebUI() {
        NSWorkspace.shared.open(defaultAdminURL.appendingPathComponent("_bifrost"))
    }
}
