import Foundation
import WidgetKit

@main
enum BifrostWidgetReloader {
    static func main() {
        WidgetCenter.shared.reloadTimelines(ofKind: "com.bifrost.desktop.status")

        // Give WidgetCenter's XPC request a brief opportunity to leave this
        // short-lived helper before the process exits. This does not activate
        // Bifrost or open a URL through LaunchServices.
        RunLoop.current.run(until: Date().addingTimeInterval(0.25))
    }
}
