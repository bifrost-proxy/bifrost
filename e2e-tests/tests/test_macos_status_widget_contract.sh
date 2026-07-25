#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
WIDGET_ROOT="${REPO_ROOT}/desktop/macos-widget"
SWIFT_SOURCE="${WIDGET_ROOT}/Sources/BifrostStatusWidget.swift"
SNAPSHOT_SOURCE="${WIDGET_ROOT}/Sources/StatusSnapshot.swift"
RELOADER_SOURCE="${WIDGET_ROOT}/Sources/WidgetReloader.swift"
BRIDGE_SOURCE="${WIDGET_ROOT}/Sources/WidgetBridge.swift"
MACOS_CONFIG="${REPO_ROOT}/desktop/src-tauri/tauri.macos.conf.json"
APP_ENTITLEMENTS="${REPO_ROOT}/desktop/src-tauri/Entitlements.plist"
WIDGET_ENTITLEMENTS="${WIDGET_ROOT}/BifrostStatusWidget.entitlements"
LOCAL_WIDGET_ENTITLEMENTS="${WIDGET_ROOT}/BifrostStatusWidget.local.entitlements"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  rg -q --fixed-strings -- "${pattern}" "${file}" \
    || fail "${file} is missing required contract: ${pattern}"
}

for file in \
  "${SWIFT_SOURCE}" \
  "${SNAPSHOT_SOURCE}" \
  "${RELOADER_SOURCE}" \
  "${BRIDGE_SOURCE}" \
  "${WIDGET_ROOT}/Info.plist" \
  "${APP_ENTITLEMENTS}" \
  "${WIDGET_ENTITLEMENTS}" \
  "${LOCAL_WIDGET_ENTITLEMENTS}" \
  "${MACOS_CONFIG}" \
  "${REPO_ROOT}/scripts/build-macos-widget.sh"; do
  [[ -f "${file}" ]] || fail "missing widget file ${file}"
done

assert_contains "${SWIFT_SOURCE}" ".supportedFamilies([.systemMedium])"
assert_contains "${SWIFT_SOURCE}" "kind: .cpu"
assert_contains "${SWIFT_SOURCE}" "kind: .memory"
assert_contains "${SWIFT_SOURCE}" "kind: .disk"
assert_contains "${SWIFT_SOURCE}" ".containerBackground(for: .widget)"
assert_contains "${SWIFT_SOURCE}" "Color.clear"
if rg -q --fixed-strings -- ".glassEffect(" "${SWIFT_SOURCE}"; then
  fail "${SWIFT_SOURCE} must let WidgetKit provide Liquid Glass instead of snapshotting a glassEffect"
fi
if rg -q --fixed-strings -- ".containerBackgroundRemovable(false)" "${SWIFT_SOURCE}"; then
  fail "${SWIFT_SOURCE} must allow WidgetKit to remove and replace the background in clear or tinted appearances"
fi
if rg -q --fixed-strings -- ".fill(.thinMaterial)" "${SWIFT_SOURCE}"; then
  fail "${SWIFT_SOURCE} must not stack thinMaterial over the system Liquid Glass container"
fi
assert_contains "${SWIFT_SOURCE}" ".widgetAccentable("
assert_contains "${SWIFT_SOURCE}" "Text(sampledAt, style: .relative)"
assert_contains "${SWIFT_SOURCE}" "bifrost://settings"
assert_contains "${SWIFT_SOURCE}" 'Bundle.main.url(forResource: "BifrostLogo", withExtension: "png")'
assert_contains "${SWIFT_SOURCE}" "Image(nsImage: image)"
assert_contains "${SNAPSHOT_SOURCE}" "bifrostWidgetReloadInterval: TimeInterval = 5"
assert_contains "${SNAPSHOT_SOURCE}" "bifrostWidgetStaleInterval: TimeInterval = 30 * 60"
assert_contains "${SNAPSHOT_SOURCE}" 'bifrostWidgetTimelineDiagnosticFileName = "timeline.log"'
assert_contains "${SWIFT_SOURCE}" 'WidgetTimelineDiagnostics.record(event: "getTimeline"'
assert_contains "${RELOADER_SOURCE}" 'let bifrostWidgetReloadURL = URL(string: "bifrost://widget-reload")!'
assert_contains "${BRIDGE_SOURCE}" '@_cdecl("bifrost_reload_status_widget")'
assert_contains "${BRIDGE_SOURCE}" 'WidgetCenter.shared.reloadTimelines(ofKind: "com.bifrost.desktop.status")'
assert_contains "${REPO_ROOT}/desktop/src-tauri/src/main.rs" \
  "widget_reload::reload_status_widget()"
assert_contains "${REPO_ROOT}/desktop/src-tauri/src/main.rs" \
  "start_periodic_widget_reload(app.handle())"
assert_contains "${REPO_ROOT}/desktop/src-tauri/src/main.rs" \
  "const WIDGET_PERIODIC_RELOAD_INTERVAL: Duration = Duration::from_secs(60);"
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/widget_snapshot.rs" \
  "const SNAPSHOT_PUBLISH_INTERVAL: Duration = Duration::from_secs(5);"
assert_contains "${REPO_ROOT}/desktop/src-tauri/src/main.rs" \
  "periodic macOS status widget reload requested"
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/widget_snapshot.rs" \
  "const WIDGET_RELOAD_INTERVAL: Duration = Duration::from_secs(60);"
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/widget_snapshot.rs" \
  "last_proxy_status != Some(proxy_status)"
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/start.rs" \
  "crate::commands::tray::start_widget_snapshot_publisher("
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/tray.rs" \
  'spawn_tray_thread("bifrost-widget-snapshot"'
assert_contains "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/tray.rs" \
  "thread::sleep(Duration::from_secs(1));"
publisher_count="$(rg -c --fixed-strings -- "WidgetSnapshotPublisher::new()" \
  "${REPO_ROOT}/crates/bifrost-cli/src/commands/tray/tray.rs")"
[[ "${publisher_count}" == "1" ]] \
  || fail "WidgetSnapshotPublisher must be owned only by the core worker, found ${publisher_count} constructors"

python3 - "${MACOS_CONFIG}" "${APP_ENTITLEMENTS}" "${WIDGET_ENTITLEMENTS}" "${LOCAL_WIDGET_ENTITLEMENTS}" <<'PY'
import json
import plistlib
import sys

config_path, app_entitlements_path, widget_entitlements_path, local_widget_entitlements_path = sys.argv[1:]
config = json.load(open(config_path, encoding="utf-8"))
if config["build"]["beforeBundleCommand"] != "bash scripts/build-macos-widget.sh":
    raise SystemExit("Tauri macOS bundle must build the WidgetKit extension before bundling")
mac = config["bundle"]["macOS"]
expected_source = "../macos-widget/build/BifrostStatusWidget.appex"
actual_source = mac["files"].get("PlugIns/BifrostStatusWidget.appex")
if actual_source != expected_source:
    raise SystemExit(
        f"widget bundle mapping mismatch: expected {expected_source!r}, got {actual_source!r}"
    )
if mac["entitlements"] != "Entitlements.plist":
    raise SystemExit("Tauri macOS config must apply the host app entitlements")

with open(app_entitlements_path, "rb") as handle:
    app_entitlements = plistlib.load(handle)
with open(widget_entitlements_path, "rb") as handle:
    widget_entitlements = plistlib.load(handle)
with open(local_widget_entitlements_path, "rb") as handle:
    local_widget_entitlements = plistlib.load(handle)

expected_group = ["group.com.bifrost.desktop"]
if app_entitlements.get("com.apple.security.application-groups") != expected_group:
    raise SystemExit("host app App Group entitlement mismatch")
if widget_entitlements.get("com.apple.security.application-groups") != expected_group:
    raise SystemExit("widget App Group entitlement mismatch")
if widget_entitlements.get("com.apple.security.app-sandbox") is not True:
    raise SystemExit("widget extension must enable App Sandbox")
if "com.apple.security.temporary-exception.files.home-relative-path.read-only" in widget_entitlements:
    raise SystemExit("production widget entitlements must not contain temporary file exceptions")
if local_widget_entitlements.get("com.apple.security.application-groups") != expected_group:
    raise SystemExit("local widget App Group entitlement mismatch")
if local_widget_entitlements.get("com.apple.security.app-sandbox") is not True:
    raise SystemExit("local widget extension must keep App Sandbox enabled")
if any(key.startswith("com.apple.security.temporary-exception") for key in local_widget_entitlements):
    raise SystemExit("local widget must not rely on temporary sandbox exceptions")
PY

bash -n "${REPO_ROOT}/scripts/build-macos-widget.sh"
bash -n "${REPO_ROOT}/scripts/resign-macos-app.sh"
assert_contains "${REPO_ROOT}/scripts/resign-macos-app.sh" '--entitlements "${WIDGET_ENTITLEMENTS}"'
assert_contains "${REPO_ROOT}/scripts/resign-macos-app.sh" 'WIDGET_ENTITLEMENTS="${LOCAL_WIDGET_ENTITLEMENTS}"'
assert_contains "${REPO_ROOT}/scripts/resign-macos-app.sh" '--entitlements "${APP_ENTITLEMENTS}"'
assert_contains "${REPO_ROOT}/scripts/resign-macos-app.sh" 'sign_executables_in_dir "${APP_PATH}/Contents/Resources/resources/bin" "${APP_ENTITLEMENTS}"'

if [[ "${BIFROST_WIDGET_SKIP_RUST_TESTS:-0}" != "1" ]]; then
  (
    cd "${REPO_ROOT}"
    cargo test -p bifrost-cli widget_snapshot --lib
  )
fi

if [[ "$(uname -s)" == "Darwin" ]] && command -v swiftc >/dev/null 2>&1; then
  TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-widget-contract.XXXXXX")"
  trap 'rm -rf "${TEMP_ROOT}"' EXIT

  swiftc \
    "${SNAPSHOT_SOURCE}" \
    "${WIDGET_ROOT}/Tests/StatusSnapshotTests.swift" \
    -o "${TEMP_ROOT}/StatusSnapshotTests"
  "${TEMP_ROOT}/StatusSnapshotTests"

  (
    cd "${REPO_ROOT}"
    TAURI_ENV_ARCH="$(uname -m)" APPLE_SIGNING_IDENTITY=- \
      bash scripts/build-macos-widget.sh
  )
  APPEX_PATH="${WIDGET_ROOT}/build/BifrostStatusWidget.appex"
  RELOADER_PATH="${REPO_ROOT}/desktop/src-tauri/resources/bin/bifrost-widget-reloader"
  BRIDGE_PATH="${REPO_ROOT}/desktop/src-tauri/resources/bin/libBifrostWidgetBridge.dylib"
  [[ -f "${APPEX_PATH}/Contents/Resources/BifrostLogo.png" ]] \
    || fail "compiled widget is missing the Bifrost logo resource"
  [[ -x "${APPEX_PATH}/Contents/MacOS/BifrostStatusWidget" ]] \
    || fail "compiled widget executable is missing"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionPointIdentifier' "${APPEX_PATH}/Contents/Info.plist")" == "com.apple.widgetkit-extension" ]] \
    || fail "compiled extension point is not WidgetKit"
  nm -u "${APPEX_PATH}/Contents/MacOS/BifrostStatusWidget" | grep -Fq "_NSExtensionMain" \
    || fail "compiled widget is missing the macOS extension process entry point"
  codesign --verify --strict --verbose=2 "${APPEX_PATH}"
  [[ -x "${RELOADER_PATH}" ]] || fail "compiled WidgetKit reload helper is missing"
  codesign --verify --strict --verbose=2 "${RELOADER_PATH}"
  [[ -f "${BRIDGE_PATH}" ]] || fail "compiled WidgetKit host bridge is missing"
  codesign --verify --strict --verbose=2 "${BRIDGE_PATH}"
  nm -gU "${BRIDGE_PATH}" | grep -Fq "_bifrost_reload_status_widget" \
    || fail "compiled WidgetKit host bridge is missing its C entry point"
else
  echo "SKIP: Swift WidgetKit binary build requires macOS with a Swift SDK"
fi

echo "PASS: macOS status widget source, sharing, Liquid Glass, and bundle contracts"
