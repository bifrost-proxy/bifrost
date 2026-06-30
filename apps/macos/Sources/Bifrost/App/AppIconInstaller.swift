import AppKit
import Foundation

enum AppIconInstaller {
    @discardableResult
    static func install() -> NSImage? {
        let icon = Bundle.main.url(forResource: "bifrost", withExtension: "icns")
            .flatMap(NSImage.init(contentsOf:))
            ?? Bundle.module.image(forResource: "bifrost")

        guard let icon else {
            assertionFailure("Missing Bifrost app icon resource")
            return nil
        }

        NSApplication.shared.applicationIconImage = icon
        return icon
    }
}
