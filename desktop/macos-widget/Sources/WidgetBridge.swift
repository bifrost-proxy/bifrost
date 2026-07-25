import WidgetKit

@_cdecl("bifrost_reload_status_widget")
public func bifrostReloadStatusWidget() {
    WidgetCenter.shared.reloadTimelines(ofKind: "com.bifrost.desktop.status")
}
