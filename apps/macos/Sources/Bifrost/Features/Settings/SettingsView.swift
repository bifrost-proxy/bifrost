import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        Form {
            Section("Core") {
                LabeledContent("Default Admin URL", value: appModel.adminURL.absoluteString)
                LabeledContent("Data Directory", value: "~/.bifrost")
            }
            Section("Development Safety") {
                SafetyStatusRow(title: "System proxy is not changed by smoke tests")
                SafetyStatusRow(title: "Tray helper is disabled for native smoke runs")
                SafetyStatusRow(title: "Sync auto-login prompt is disabled for native smoke runs")
            }
        }
        .formStyle(.grouped)
        .padding(20)
    }
}

private struct SafetyStatusRow: View {
    let title: String

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "checkmark.circle.fill")
                .foregroundStyle(.green)
            Text(title)
            Spacer()
        }
    }
}
