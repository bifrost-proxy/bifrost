import SwiftUI

@main
struct BifrostMacApp: App {
    @StateObject private var appModel = AppModel()

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
