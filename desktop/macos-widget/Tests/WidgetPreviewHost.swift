#if BIFROST_WIDGET_PREVIEW_HOST
import AppKit
import SwiftUI

@main
enum WidgetPreviewHost {
    static func main() {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)

        let snapshot = StatusSnapshot(
            schemaVersion: bifrostWidgetSnapshotSchemaVersion,
            sampledAtMs: UInt64(Date().timeIntervalSince1970 * 1_000),
            cpuPercent: 42,
            memoryPercent: 68,
            diskPercent: 53,
            proxyStatus: .on
        )
        #if BIFROST_WIDGET_STALE_PREVIEW
        let isStale = true
        #else
        let isStale = false
        #endif
        let entry = BifrostStatusEntry(date: .now, snapshot: snapshot, isStale: isStale)
        let content = BifrostStatusWidgetView(entry: entry)
            .frame(width: 338, height: 158)
            .padding(16)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 28))
            #if BIFROST_WIDGET_DARK_PREVIEW
            .preferredColorScheme(.dark)
            #else
            .preferredColorScheme(.light)
            #endif

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 370, height: 190),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "Bifrost Widget Visual Test"
        window.isReleasedWhenClosed = false
        window.center()
        window.contentView = NSHostingView(rootView: content)
        window.makeKeyAndOrderFront(nil)

        application.activate(ignoringOtherApps: true)
        application.run()
    }
}
#endif
