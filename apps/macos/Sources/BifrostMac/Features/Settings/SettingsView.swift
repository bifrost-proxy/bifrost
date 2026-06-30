import SwiftUI

struct SettingsView: View {
    var body: some View {
        Form {
            Section("Core") {
                LabeledContent("Default Admin URL", value: "http://127.0.0.1:9900")
                LabeledContent("Data Directory", value: "~/.bifrost")
            }
            Section("Development Safety") {
                Toggle("Disable system proxy during smoke tests", isOn: .constant(true))
                Toggle("Disable tray helper during smoke tests", isOn: .constant(true))
                Toggle("Disable Sync auto-login prompt", isOn: .constant(true))
            }
        }
        .formStyle(.grouped)
        .padding(20)
    }
}
