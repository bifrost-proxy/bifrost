import AppKit
import Foundation
import SwiftUI

@MainActor
private enum SharedAppModel {
    static let instance = AppModel()
}

private func traceNativeStartup(_ message: String) {
    guard ProcessInfo.processInfo.environment["BIFROST_NATIVE_STARTUP_TRACE"] == "1" else {
        return
    }
    FileHandle.standardError.write(Data("[native-startup] \(message)\n".utf8))
}

@main
struct BifrostApp: App {
    @NSApplicationDelegateAdaptor(BifrostAppDelegate.self) private var appDelegate
    @StateObject private var appModel: AppModel

    init() {
        traceNativeStartup("BifrostApp.init")
        let model = SharedAppModel.instance
        _appModel = StateObject(wrappedValue: model)
        appDelegate.appModel = model

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
            let items = SidebarItem.visibleItems(canShowGroups: false).map(\.rawValue)
            let itemsWithGroups = SidebarItem.visibleItems(canShowGroups: true).map(\.rawValue)
            let allItems = SidebarItem.allCases.map(\.rawValue)
            let expected = ["活动", "概览", "规则", "抓包"]
            let expectedWithGroups = expected + ["小组"]
            guard items == expected, itemsWithGroups == expectedWithGroups, allItems == itemsWithGroups else {
                fputs("Bifrost release scope check failed: visible=\(items.joined(separator: ",")) groups=\(itemsWithGroups.joined(separator: ",")) all=\(allItems.joined(separator: ","))\n", stderr)
                Foundation.exit(1)
            }
            print("Bifrost release scope check passed: \(items.joined(separator: ",")); groups=\(itemsWithGroups.joined(separator: ","))")
            Foundation.exit(0)
        }
        if CommandLine.arguments.contains("--check-theme-contract") {
            guard ColorSchemeMode.system.colorScheme == nil,
                  ColorSchemeMode.system.next == .light,
                  ColorSchemeMode.light.next == .dark,
                  ColorSchemeMode.dark.next == .system else {
                fputs("Bifrost theme contract check failed: color scheme cycle is not system -> light -> dark\n", stderr)
                Foundation.exit(1)
            }
            guard AppSurface.resolvedContentColor(for: .aqua) != AppSurface.resolvedContentColor(for: .darkAqua) else {
                fputs("Bifrost theme contract check failed: app surfaces are not appearance-adaptive\n", stderr)
                Foundation.exit(1)
            }
            let darkEditorTheme = BifrostRuleEditorTheme(appearance: NSAppearance(named: .darkAqua) ?? NSApp.effectiveAppearance)
            guard darkEditorTheme.background != NSColor.white,
                  darkEditorTheme.rulerBackground != NSColor(calibratedWhite: 0.98, alpha: 1) else {
                fputs("Bifrost theme contract check failed: rule editor still uses light-only backgrounds\n", stderr)
                Foundation.exit(1)
            }
            print("Bifrost theme contract check passed")
            Foundation.exit(0)
        }
        if CommandLine.arguments.contains("--check-rule-editor-layout") {
            let editor = BifrostRuleEditorContainerView(frame: NSRect(x: 0, y: 0, width: 900, height: 560))
            let sampleRule = "example.com host://127.0.0.1:3000\n// comment keeps syntax highlighting visible\n"
            editor.update(text: sampleRule, editorContext: .empty, isReadOnly: false)
            editor.layoutSubtreeIfNeeded()

            let clipWidth = editor.scrollView.contentView.bounds.width
            let textWidth = editor.textView.frame.width
            guard editor.textView.isEditable,
                  editor.textView.string == sampleRule,
                  editor.textView.textContainer?.widthTracksTextView == true,
                  !editor.textView.isHorizontallyResizable,
                  editor.textView.textContainerInset.width >= 16,
                  editor.textView.textContainerInset.height >= 14,
                  textWidth >= clipWidth - 1,
                  editor.textView.frame.height >= editor.scrollView.contentView.bounds.height - 1,
                  editor.scrollView.contentView.isFlipped,
                  editor.scrollView.contentView.bounds.origin == .zero,
                  editor.scrollView.documentView === editor.textView,
                  editor.scrollView.hasVerticalRuler,
                  editor.scrollView.rulersVisible,
                  editor.scrollView.verticalRulerView === editor.textView.lineNumberRuler,
                  editor.textView.lineNumberRuler?.ruleThickness ?? 0 >= 52 else {
                fputs("Bifrost rule editor layout check failed: editable=\(editor.textView.isEditable) textWidth=\(textWidth) clipWidth=\(clipWidth) textLength=\(editor.textView.string.count)\n", stderr)
                Foundation.exit(1)
            }
            print("Bifrost rule editor layout check passed")
            Foundation.exit(0)
        }

        BifrostApp.activateExistingInstanceIfNeeded()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            traceNativeStartup("BifrostApp.init async ensure visible")
            MainWindowFallback.ensureVisible(appModel: SharedAppModel.instance)
        }
    }

    var body: some Scene {
        WindowGroup {
            MainWindowScene()
                .environmentObject(appModel)
        }
        .windowStyle(.hiddenTitleBar)
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

    private static func activateExistingInstanceIfNeeded() {
        guard !CommandLine.arguments.contains("--allow-multiple-instances"),
              let bundleIdentifier = Bundle.main.bundleIdentifier else {
            return
        }
        let currentProcessIdentifier = ProcessInfo.processInfo.processIdentifier
        let existingApplications = NSRunningApplication
            .runningApplications(withBundleIdentifier: bundleIdentifier)
            .filter { application in
                application.processIdentifier != currentProcessIdentifier && !application.isTerminated
            }
        guard let existingApplication = existingApplications.first else {
            return
        }
        existingApplication.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
        Foundation.exit(0)
    }
}

@MainActor
final class BifrostAppDelegate: NSObject, NSApplicationDelegate {
    weak var appModel: AppModel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        traceNativeStartup("applicationDidFinishLaunching")
        NSApp.setActivationPolicy(.regular)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
            self?.ensureMainWindowVisible()
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        ensureMainWindowVisible()
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    private func ensureMainWindowVisible() {
        traceNativeStartup("delegate ensureMainWindowVisible")
        MainWindowFallback.ensureVisible(appModel: appModel ?? SharedAppModel.instance)
    }
}

@MainActor
private enum MainWindowFallback {
    private static weak var fallbackWindow: NSWindow?

    static func ensureVisible(appModel: AppModel) {
        traceNativeStartup("fallback ensureVisible windows=\(NSApp.windows.count)")

        if let window = NSApp.windows.first(where: isUsableMainWindow) {
            placeWindowOnVisibleScreenIfNeeded(window)
            window.makeKeyAndOrderFront(nil)
            closeExtraMainWindows(keeping: window)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        if let fallbackWindow, fallbackWindow.isVisible {
            placeWindowOnVisibleScreenIfNeeded(fallbackWindow)
            fallbackWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        traceNativeStartup("fallback creating single NSWindow")
        let controller = NSHostingController(
            rootView: MainWindowScene()
                .environmentObject(appModel)
        )
        let window = NSWindow(contentViewController: controller)
        window.title = ""
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isOpaque = true
        window.backgroundColor = AppSurface.windowBackground
        window.styleMask.insert(.fullSizeContentView)
        window.isMovable = true
        window.isMovableByWindowBackground = false
        window.isReleasedWhenClosed = false
        window.setContentSize(NSSize(width: 1280, height: 820))
        window.minSize = NSSize(width: 960, height: 720)
        placeWindowOnVisibleScreenIfNeeded(window, force: true)
        window.makeKeyAndOrderFront(nil)
        fallbackWindow = window
        NSApp.activate(ignoringOtherApps: true)
    }

    private static func closeExtraMainWindows(keeping keptWindow: NSWindow) {
        for window in NSApp.windows where window !== keptWindow && isUsableMainWindow(window) {
            traceNativeStartup("fallback closing duplicate window frame=\(window.frame)")
            window.orderOut(nil)
            window.close()
        }
    }

    private static func isUsableMainWindow(_ window: NSWindow) -> Bool {
        traceNativeStartup("window candidate visible=\(window.isVisible) mini=\(window.isMiniaturized) canMain=\(window.canBecomeMain) frame=\(window.frame)")
        guard window.canBecomeMain,
              !window.isMiniaturized,
              window.frame.width > 100,
              window.frame.height > 100
        else {
            return false
        }
        return true
    }

    private static func placeWindowOnVisibleScreenIfNeeded(_ window: NSWindow, force: Bool = false) {
        let screenFrame = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1280, height: 820)
        if !force, screenFrame.insetBy(dx: 12, dy: 12).contains(NSPoint(x: window.frame.midX, y: window.frame.midY)) {
            return
        }
        let width = min(max(window.frame.width, 960), max(screenFrame.width - 80, 960))
        let height = min(max(window.frame.height, 720), max(screenFrame.height - 80, 720))
        let frame = NSRect(
            x: screenFrame.midX - width / 2,
            y: screenFrame.midY - height / 2,
            width: width,
            height: height
        )
        window.setFrame(frame, display: true)
    }
}
