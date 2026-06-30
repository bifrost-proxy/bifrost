import BifrostMacCore
import SwiftUI

struct DashboardView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Header(title: "Overview", subtitle: "Native control surface for the local Rust sidecar")

            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 16) {
                GridRow {
                    StatusTile(title: "Core", value: sidecarStatusText)
                    StatusTile(title: "Admin API", value: "127.0.0.1:9900")
                    StatusTile(title: "System Proxy", value: "Off by default")
                    StatusTile(title: "TLS CA", value: "Use Admin API")
                }
            }

            HStack(spacing: 12) {
                Button("Start Sidecar") {}
                    .disabled(true)
                Button("Open Web UI") {
                    appModel.openWebUI()
                }
            }

            Text("The first native preview keeps proxy connections, TLS interception, rules, storage, and scripts in the existing Rust daemon. This app owns the Mac user experience and sidecar control plane.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: 760, alignment: .leading)

            Spacer()
        }
        .padding(24)
    }

    private var sidecarStatusText: String {
        switch appModel.sidecarState {
        case .stopped:
            return "Stopped"
        case .starting:
            return "Starting"
        case .running(let port):
            return "Running on \(port)"
        case .failed:
            return "Failed"
        case .recovering:
            return "Recovering"
        }
    }
}

private struct StatusTile: View {
    let title: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.headline)
        }
        .frame(width: 180, alignment: .leading)
        .padding(12)
        .background(.quaternary.opacity(0.45), in: RoundedRectangle(cornerRadius: 8))
    }
}
