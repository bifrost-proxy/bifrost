import AppKit
import SwiftUI
import WidgetKit

struct BifrostStatusEntry: TimelineEntry {
    let date: Date
    let snapshot: StatusSnapshot?
    let isStale: Bool
}

struct BifrostStatusProvider: TimelineProvider {
    func placeholder(in context: Context) -> BifrostStatusEntry {
        BifrostStatusEntry(date: .now, snapshot: .placeholder, isStale: false)
    }

    func getSnapshot(
        in context: Context,
        completion: @escaping (BifrostStatusEntry) -> Void
    ) {
        let snapshot = context.isPreview ? .placeholder : StatusSnapshotStore.load()
        WidgetTimelineDiagnostics.record(event: "getSnapshot", snapshot: snapshot)
        completion(entry(for: snapshot, at: .now))
    }

    func getTimeline(
        in context: Context,
        completion: @escaping (Timeline<BifrostStatusEntry>) -> Void
    ) {
        let now = Date()
        let snapshot = StatusSnapshotStore.load()
        WidgetTimelineDiagnostics.record(event: "getTimeline", snapshot: snapshot)
        var entries = [entry(for: snapshot, at: now)]

        if let snapshot, !snapshot.isStale(at: now) {
            let staleDate = snapshot.sampledAt.addingTimeInterval(bifrostWidgetStaleInterval)
            entries.append(BifrostStatusEntry(date: staleDate, snapshot: snapshot, isStale: true))
        }

        completion(
            Timeline(
                entries: entries,
                policy: .after(now.addingTimeInterval(bifrostWidgetReloadInterval))
            )
        )
    }

    private func entry(for snapshot: StatusSnapshot?, at date: Date) -> BifrostStatusEntry {
        BifrostStatusEntry(
            date: date,
            snapshot: snapshot,
            isStale: snapshot?.isStale(at: date) ?? true
        )
    }
}

private enum MetricKind {
    case cpu
    case memory
    case disk

    var label: String {
        switch self {
        case .cpu: "CPU"
        case .memory: "Memory"
        case .disk: "Disk"
        }
    }

    var symbol: String {
        switch self {
        case .cpu: "cpu"
        case .memory: "memorychip"
        case .disk: "internaldrive"
        }
    }

    func color(for percent: Double?) -> Color {
        guard let percent else {
            return .secondary
        }
        let warningThreshold: Double
        let criticalThreshold: Double
        switch self {
        case .cpu:
            warningThreshold = 60
            criticalThreshold = 85
        case .memory:
            warningThreshold = 60
            criticalThreshold = 80
        case .disk:
            warningThreshold = 75
            criticalThreshold = 90
        }
        if percent >= criticalThreshold {
            return .red
        }
        if percent >= warningThreshold {
            return .orange
        }
        return Color(red: 0.075, green: 0.647, blue: 0.561)
    }
}

private struct MetricRing: View {
    let kind: MetricKind
    let percent: Double?
    let isStale: Bool

    private var normalizedPercent: Double {
        min(max(percent ?? 0, 0), 100)
    }

    private var formattedPercent: String {
        percent.map { "\(Int($0.rounded()))%" } ?? "--"
    }

    var body: some View {
        VStack(spacing: 5) {
            ZStack {
                Circle()
                    .stroke(.secondary.opacity(0.18), lineWidth: 6)
                Circle()
                    .trim(from: 0, to: normalizedPercent / 100)
                    .stroke(
                        kind.color(for: percent),
                        style: StrokeStyle(lineWidth: 6, lineCap: .round)
                    )
                    .rotationEffect(.degrees(-90))
                    .widgetAccentable(true)
                Image(systemName: kind.symbol)
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(isStale ? .secondary : .primary)
            }
            .frame(width: 50, height: 50)

            Text(formattedPercent)
                .font(.system(size: 15, weight: .bold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(isStale ? .secondary : .primary)

            Text(kind.label)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(kind.label)
        .accessibilityValue(percent.map { "\(Int($0.rounded())) percent" } ?? "Unavailable")
        .accessibilityHint(isStale ? "Last Bifrost sample is out of date" : "")
    }
}

private struct ProxyBadge: View {
    let status: ProxyStatus?
    let isStale: Bool

    private var label: String {
        if isStale {
            return "Proxy status stale"
        }
        return switch status {
        case .on: "Global proxy on"
        case .off: "Global proxy off"
        case .checking: "Checking proxy"
        case .unsupported: "Proxy unsupported"
        case nil: "Waiting for Bifrost"
        }
    }

    private var symbol: String {
        if isStale {
            return "clock.badge.exclamationmark"
        }
        return switch status {
        case .on: "network"
        case .off: "network.slash"
        case .checking: "ellipsis"
        case .unsupported: "exclamationmark.triangle"
        case nil: "bolt.horizontal"
        }
    }

    private var color: Color {
        status == .on && !isStale
            ? Color(red: 0.075, green: 0.647, blue: 0.561)
            : .secondary
    }

    var body: some View {
        Label(label, systemImage: symbol)
            .font(.caption.weight(.semibold))
            .foregroundStyle(color)
            .lineLimit(1)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(.secondary.opacity(0.12), in: Capsule())
            .widgetAccentable(status == .on && !isStale)
            .accessibilityLabel(label)
    }
}

private struct BifrostLogo: View {
    private var image: NSImage? {
        guard
            let url = Bundle.main.url(forResource: "BifrostLogo", withExtension: "png")
        else {
            return nil
        }
        return NSImage(contentsOf: url)
    }

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
            } else {
                Image(systemName: "b.circle.fill")
                    .resizable()
                    .scaledToFit()
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: 16, height: 16)
        .accessibilityHidden(true)
    }
}

struct BifrostStatusWidgetView: View {
    let entry: BifrostStatusEntry

    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 12) {
                MetricRing(
                    kind: .cpu,
                    percent: entry.snapshot?.cpuPercent,
                    isStale: entry.isStale
                )
                MetricRing(
                    kind: .memory,
                    percent: entry.snapshot?.memoryPercent,
                    isStale: entry.isStale
                )
                MetricRing(
                    kind: .disk,
                    percent: entry.snapshot?.diskPercent,
                    isStale: entry.isStale
                )
            }

            HStack(spacing: 8) {
                ProxyBadge(status: entry.snapshot?.proxyStatus, isStale: entry.isStale)
                Spacer(minLength: 4)
                if let sampledAt = entry.snapshot?.sampledAt {
                    Label {
                        Text(sampledAt, style: .relative)
                    } icon: {
                        Image(systemName: "clock")
                    }
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .accessibilityLabel("Updated \(sampledAt.formatted())")
                } else {
                    Text("Open Bifrost to collect data")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                BifrostLogo()
            }
        }
        .containerBackground(for: .widget) {
            Color.clear
        }
        .widgetURL(URL(string: "bifrost://settings"))
    }
}

struct BifrostStatusWidget: Widget {
    let kind = "com.bifrost.desktop.status"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: BifrostStatusProvider()) { entry in
            BifrostStatusWidgetView(entry: entry)
        }
        .configurationDisplayName("Bifrost Status")
        .description("CPU, memory, disk, and global proxy status at a glance.")
        .supportedFamilies([.systemMedium])
    }
}

#if !BIFROST_WIDGET_PREVIEW_HOST
@main
struct BifrostWidgetBundle: WidgetBundle {
    var body: some Widget {
        BifrostStatusWidget()
    }
}
#endif
