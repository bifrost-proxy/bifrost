#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_DIR="$ROOT_DIR/apps/macos"
CONFIGURATION="debug"
RUN_TESTS=0
SKIP_SIDECAR=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      CONFIGURATION="release"
      shift
      ;;
    --test)
      RUN_TESTS=1
      shift
      ;;
    --skip-sidecar)
      SKIP_SIDECAR=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

SWIFT_CONFIGURATION_FLAG="debug"
if [[ "$CONFIGURATION" == "release" ]]; then
  SWIFT_CONFIGURATION_FLAG="release"
fi

create_dev_app_bundle() {
  local bin_dir
  bin_dir="$(swift build --package-path "$PACKAGE_DIR" -c "$SWIFT_CONFIGURATION_FLAG" --show-bin-path | tail -n 1)"

  local executable="$bin_dir/Bifrost"
  local resource_bundle="$bin_dir/Bifrost_Bifrost.bundle"
  local app_dir="$PACKAGE_DIR/.build/Bifrost.app"
  local contents_dir="$app_dir/Contents"

  if [[ ! -x "$executable" ]]; then
    echo "missing built Bifrost executable: $executable" >&2
    exit 1
  fi

  rm -rf "$app_dir"
  mkdir -p "$contents_dir/MacOS" "$contents_dir/Resources"
  install -m 755 "$executable" "$contents_dir/MacOS/Bifrost"
  install -m 644 "$ROOT_DIR/assets/bifrost.icns" "$contents_dir/Resources/bifrost.icns"

  local sidecar_bin="$PACKAGE_DIR/.build/sidecar/bin/bifrost"
  if [[ -x "$sidecar_bin" ]]; then
    mkdir -p "$contents_dir/Resources/bin"
    install -m 755 "$sidecar_bin" "$contents_dir/Resources/bin/bifrost"
  fi

  if [[ -d "$resource_bundle" ]]; then
    cp -R "$resource_bundle" "$app_dir/Bifrost_Bifrost.bundle"
    cp -R "$resource_bundle" "$contents_dir/Resources/Bifrost_Bifrost.bundle"
  fi

  cat >"$contents_dir/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Bifrost</string>
  <key>CFBundleExecutable</key>
  <string>Bifrost</string>
  <key>CFBundleIconFile</key>
  <string>bifrost</string>
  <key>CFBundleIdentifier</key>
  <string>com.bifrost.native.mac</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Bifrost</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.129</string>
  <key>CFBundleVersion</key>
  <string>0.0.129</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

  echo "$app_dir"
}

if [[ "$SKIP_SIDECAR" -eq 0 ]]; then
  if [[ "$CONFIGURATION" == "release" ]]; then
    "$ROOT_DIR/scripts/prepare-macos-native-sidecar.sh" --release
  else
    "$ROOT_DIR/scripts/prepare-macos-native-sidecar.sh"
  fi
fi

swift build --package-path "$PACKAGE_DIR" -c "$SWIFT_CONFIGURATION_FLAG"
create_dev_app_bundle

if [[ "$RUN_TESTS" -eq 1 ]]; then
  swift run --package-path "$PACKAGE_DIR" BifrostNativeCoreChecks
fi
