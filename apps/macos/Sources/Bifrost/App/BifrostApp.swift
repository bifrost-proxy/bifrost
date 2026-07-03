import AppKit
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
            let items = SidebarItem.visibleItems(canShowGroups: false).map(\.rawValue)
            let itemsWithGroups = SidebarItem.visibleItems(canShowGroups: true).map(\.rawValue)
            let allItems = SidebarItem.allCases.map(\.rawValue)
            let expected = ["活动", "概览", "规则", "抓包"]
            let expectedWithGroups = expected + ["小组管理"]
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
                  textWidth >= clipWidth - 1,
                  editor.textView.frame.height >= editor.scrollView.contentView.bounds.height - 1,
                  editor.scrollView.contentView.isFlipped,
                  editor.scrollView.contentView.bounds.origin == .zero,
                  editor.scrollView.documentView === editor.textView,
                  editor.scrollView.verticalRulerView != nil else {
                fputs("Bifrost rule editor layout check failed: editable=\(editor.textView.isEditable) textWidth=\(textWidth) clipWidth=\(clipWidth) textLength=\(editor.textView.string.count)\n", stderr)
                Foundation.exit(1)
            }
            print("Bifrost rule editor layout check passed")
            Foundation.exit(0)
        }

        BifrostApp.activateExistingInstanceIfNeeded()
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
