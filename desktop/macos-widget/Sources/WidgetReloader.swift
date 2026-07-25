import AppKit
import Foundation

let bifrostWidgetReloadURL = URL(string: "bifrost://widget-reload")!

@main
enum BifrostWidgetReloader {
    static func main() {
        if !NSWorkspace.shared.open(bifrostWidgetReloadURL) {
            FileHandle.standardError.write(
                Data("failed to dispatch Bifrost widget reload URL\n".utf8)
            )
            Foundation.exit(EXIT_FAILURE)
        }
    }
}
