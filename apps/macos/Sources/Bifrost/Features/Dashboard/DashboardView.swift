import BifrostNativeCore
import SwiftUI

struct DashboardView: View {
    @EnvironmentObject private var appModel: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Header(title: "Overview", subtitle: "Native control surface for the local Rust sidecar")

            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 16) {
                GridRow {
                    StatusTile(title: "Core", value: sidecarStatusText)
                    StatusTile(title: "Admin API", value: appModel.adminHostPortLabel)
                    StatusTile(title: "Version", value: appModel.overview?.system?.version ?? "-")
                    StatusTile(title: "PID", value: appModel.overview?.system?.pid.map(String.init) ?? "-")
                }
                GridRow {
                    StatusTile(title: "Traffic", value: appModel.overview?.traffic?.recorded.map(String.init) ?? "-")
                    StatusTile(title: "Rules", value: rulesSummaryText)
                    StatusTile(title: "Connections", value: appModel.overview?.metrics?.activeConnections.map(String.init) ?? "-")
                    StatusTile(title: "QPS", value: qpsText)
                }
            }

            HStack(spacing: 12) {
                Button("Ensure Service") {
                    Task {
                        await appModel.ensureService()
                    }
                }
                .disabled(isEnsuringService)
                Button("Open Web UI") {
                    appModel.openWebUI()
                }
                Button("Refresh") {
                    Task {
                        await appModel.refreshData()
                    }
                }
            }

            if appModel.isLoadingData {
                Text("Loading Admin API data...")
                    .foregroundStyle(.secondary)
            }

            if let dataError = appModel.dataError {
                Text(dataError)
                    .foregroundStyle(.red)
                    .frame(maxWidth: 760, alignment: .leading)
            }

            Spacer()
        }
        .padding(24)
    }

    private var isEnsuringService: Bool {
        if case .starting = appModel.sidecarState {
            return true
        }
        return false
    }

    private var rulesSummaryText: String {
        let enabled = appModel.overview?.rules?.enabled
        let total = appModel.overview?.rules?.total
        switch (enabled, total) {
        case (.some(let enabled), .some(let total)):
            return "\(enabled)/\(total)"
        case (.some(let enabled), .none):
            return "\(enabled) enabled"
        case (.none, .some(let total)):
            return "\(total) total"
        case (.none, .none):
            return "-"
        }
    }

    private var qpsText: String {
        guard let qps = appModel.overview?.metrics?.qps else {
            return "-"
        }
        return String(format: "%.2f", qps)
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
