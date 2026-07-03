import Foundation
import SwiftUI

@main
struct BifrostApp: App {
    @StateObject private var appModel = AppModel()

    init() {
        let icon = AppIconInstaller.install()
        if CommandLine.arguments.contains("--check-icon") {
            guard let icon, icon.size.width > 0, icon.size.height > 0 else {
                fputs("Bifrost icon check failed: app icon resource was not loaded\n", stderr)
                Foundation.exit(1)
            }

            print("Bifrost icon check passed: \(Int(icon.size.width))x\(Int(icon.size.height))")
            Foundation.exit(0)
        }
        if CommandLine.arguments.contains("--check-admin-data") {
            AdminDataSmokeCheck.run()
        }
        if CommandLine.arguments.contains("--check-settings-data") {
            SettingsDataSmokeCheck.run()
        }
        if CommandLine.arguments.contains("--check-traffic-table-performance") {
            TrafficTablePerformanceSmoke.run()
        }
        if CommandLine.arguments.contains("--check-release-scope") {
            let items = SidebarItem.releaseScopeItems.map(\.rawValue)
            let allItems = SidebarItem.allCases.map(\.rawValue)
            let expected = ["活动", "概览", "规则", "网络"]
            guard items == expected, allItems == items else {
                fputs("Bifrost release scope check failed: visible=\(items.joined(separator: ",")) all=\(allItems.joined(separator: ","))\n", stderr)
                Foundation.exit(1)
            }
            print("Bifrost release scope check passed: \(items.joined(separator: ","))")
            Foundation.exit(0)
        }
    }

    var body: some Scene {
        WindowGroup {
            MainWindowScene()
                .environmentObject(appModel)
        }
        .commands {
            CommandGroup(after: .appInfo) {
                Button("Open Web UI") {
                    appModel.openWebUI()
                }
                .keyboardShortcut("w", modifiers: [.command, .shift])
            }
        }

        Settings {
            SettingsView()
                .environmentObject(appModel)
        }
    }
}
