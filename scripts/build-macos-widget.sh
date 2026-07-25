#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WIDGET_ROOT="${REPO_ROOT}/desktop/macos-widget"
BUILD_ROOT="${WIDGET_ROOT}/build"
APPEX_PATH="${BUILD_ROOT}/BifrostStatusWidget.appex"
CONTENTS_PATH="${APPEX_PATH}/Contents"
EXECUTABLE_PATH="${CONTENTS_PATH}/MacOS/BifrostStatusWidget"
RELOADER_SOURCE="${WIDGET_ROOT}/Sources/WidgetReloader.swift"
RELOADER_PATH="${REPO_ROOT}/desktop/src-tauri/resources/bin/bifrost-widget-reloader"
BRIDGE_SOURCE="${WIDGET_ROOT}/Sources/WidgetBridge.swift"
BRIDGE_PATH="${REPO_ROOT}/desktop/src-tauri/resources/bin/libBifrostWidgetBridge.dylib"
INFO_PLIST_PATH="${CONTENTS_PATH}/Info.plist"
LOGO_SOURCE="${REPO_ROOT}/assets/bifrost.png"
LOGO_PATH="${CONTENTS_PATH}/Resources/BifrostLogo.png"
ENTITLEMENTS_PATH="${WIDGET_ROOT}/BifrostStatusWidget.entitlements"
LOCAL_ENTITLEMENTS_PATH="${WIDGET_ROOT}/BifrostStatusWidget.local.entitlements"

case "${TAURI_ENV_ARCH:-$(uname -m)}" in
  aarch64 | arm64)
    SWIFT_ARCH="arm64"
    ;;
  x86_64 | x64)
    SWIFT_ARCH="x86_64"
    ;;
  *)
    echo "Unsupported macOS widget architecture: ${TAURI_ENV_ARCH:-$(uname -m)}" >&2
    exit 1
    ;;
esac

if ! SDK_PATH="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null)"; then
  echo "Building the Bifrost WidgetKit extension requires a full Xcode macOS SDK." >&2
  exit 1
fi

VERSION="${BIFROST_VERSION:-}"
if [[ -z "${VERSION}" ]]; then
  VERSION="$(python3 -c 'import json, pathlib, sys; print(json.loads(pathlib.Path(sys.argv[1]).read_text())["version"])' "${REPO_ROOT}/desktop/src-tauri/tauri.conf.json" 2>/dev/null || true)"
fi
if [[ -z "${VERSION}" ]]; then
  echo "Unable to resolve Bifrost version for the widget extension." >&2
  exit 1
fi
VERSION="${VERSION#v}"

rm -rf "${APPEX_PATH}"
mkdir -p "${CONTENTS_PATH}/MacOS"
mkdir -p "${CONTENTS_PATH}/Resources"
cp "${WIDGET_ROOT}/Info.plist" "${INFO_PLIST_PATH}"
cp "${LOGO_SOURCE}" "${LOGO_PATH}"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${VERSION}" "${INFO_PLIST_PATH}"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${VERSION}" "${INFO_PLIST_PATH}"

SWIFT_SOURCES=(
  "${WIDGET_ROOT}/Sources/BifrostStatusWidget.swift"
  "${WIDGET_ROOT}/Sources/StatusSnapshot.swift"
)
if [[ ! -e "${SWIFT_SOURCES[0]}" || ! -e "${RELOADER_SOURCE}" || ! -e "${BRIDGE_SOURCE}" ]]; then
  echo "No Swift sources found under ${WIDGET_ROOT}/Sources." >&2
  exit 1
fi

xcrun --sdk macosx swiftc \
  -sdk "${SDK_PATH}" \
  -target "${SWIFT_ARCH}-apple-macos14.0" \
  -parse-as-library \
  -application-extension \
  -O \
  -module-name BifrostStatusWidget \
  -framework SwiftUI \
  -framework WidgetKit \
  -Xlinker -e \
  -Xlinker _NSExtensionMain \
  "${SWIFT_SOURCES[@]}" \
  -o "${EXECUTABLE_PATH}"

mkdir -p "$(dirname "${RELOADER_PATH}")"
xcrun --sdk macosx swiftc \
  -sdk "${SDK_PATH}" \
  -target "${SWIFT_ARCH}-apple-macos14.0" \
  -parse-as-library \
  -O \
  -framework AppKit \
  "${RELOADER_SOURCE}" \
  -o "${RELOADER_PATH}"

xcrun --sdk macosx swiftc \
  -sdk "${SDK_PATH}" \
  -target "${SWIFT_ARCH}-apple-macos14.0" \
  -parse-as-library \
  -emit-library \
  -O \
  -module-name BifrostWidgetBridge \
  -framework WidgetKit \
  -Xlinker -install_name \
  -Xlinker "@rpath/$(basename "${BRIDGE_PATH}")" \
  "${BRIDGE_SOURCE}" \
  -o "${BRIDGE_PATH}"

SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
if [[ "${SIGNING_IDENTITY}" == "-" ]]; then
  ENTITLEMENTS_PATH="${LOCAL_ENTITLEMENTS_PATH}"
fi
SIGN_ARGS=(--force --sign "${SIGNING_IDENTITY}" --entitlements "${ENTITLEMENTS_PATH}")
if [[ "${SIGNING_IDENTITY}" != "-" ]]; then
  SIGN_ARGS+=(--options runtime)
fi
codesign "${SIGN_ARGS[@]}" "${APPEX_PATH}"
codesign --force --sign "${SIGNING_IDENTITY}" "${RELOADER_PATH}"
codesign --force --sign "${SIGNING_IDENTITY}" "${BRIDGE_PATH}"
codesign --verify --strict --verbose=2 "${APPEX_PATH}"
codesign --verify --strict --verbose=2 "${RELOADER_PATH}"
codesign --verify --strict --verbose=2 "${BRIDGE_PATH}"

echo "Built macOS WidgetKit extension: ${APPEX_PATH}"
echo "Built macOS WidgetKit reload helper: ${RELOADER_PATH}"
echo "Built macOS WidgetKit host bridge: ${BRIDGE_PATH}"
