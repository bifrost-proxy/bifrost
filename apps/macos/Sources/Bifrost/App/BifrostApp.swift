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
        if CommandLine.arguments.contains("--check-traffic-table-performance") {
            TrafficTablePerformanceSmoke.run()
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
