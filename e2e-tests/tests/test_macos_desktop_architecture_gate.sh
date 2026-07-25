#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: macOS desktop architecture gate only runs on macOS"
  exit 0
fi

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-desktop-arch-gate.XXXXXX")"
trap 'rm -rf "$TEST_DIR"' EXIT

APP_PATH="$TEST_DIR/Bifrost.app"
MAIN_BIN="$APP_PATH/Contents/MacOS/Bifrost"
SIDECAR_BIN="$APP_PATH/Contents/Resources/resources/bin/bifrost"
WIDGET_PATH="$APP_PATH/Contents/PlugIns/BifrostStatusWidget.appex"
WIDGET_BIN="$WIDGET_PATH/Contents/MacOS/BifrostStatusWidget"
mkdir -p "$(dirname "$MAIN_BIN")" "$(dirname "$SIDECAR_BIN")" "$(dirname "$WIDGET_BIN")"

printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Bifrost</string></dict></plist>' \
  >"$APP_PATH/Contents/Info.plist"
printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '<key>CFBundleExecutable</key><string>BifrostStatusWidget</string>' \
  '<key>NSExtension</key><dict><key>NSExtensionPointIdentifier</key><string>com.apple.widgetkit-extension</string></dict>' \
  '</dict></plist>' \
  >"$WIDGET_PATH/Contents/Info.plist"
printf '%s\n' 'arm64' >"$MAIN_BIN"
printf '%s\n' 'arm64' >"$SIDECAR_BIN"
printf '%s\n' 'arm64' >"$WIDGET_BIN"

FAKE_LIPO="$TEST_DIR/lipo"
printf '%s\n' \
  '#!/bin/sh' \
  'if [ "$1" != "-archs" ]; then exit 2; fi' \
  'tr -d "\n" <"$2"' \
  >"$FAKE_LIPO"
chmod +x "$FAKE_LIPO"

LIPO_BIN="$FAKE_LIPO" bash scripts/validate-macos-desktop-architectures.sh \
  "$APP_PATH" aarch64-apple-darwin

printf '%s\n' 'x86_64 arm64' >"$MAIN_BIN"
printf '%s\n' 'arm64 x86_64' >"$SIDECAR_BIN"
printf '%s\n' 'x86_64 arm64' >"$WIDGET_BIN"
LIPO_BIN="$FAKE_LIPO" bash scripts/validate-macos-desktop-architectures.sh \
  "$APP_PATH" aarch64-apple-darwin

printf '%s\n' 'arm64' >"$MAIN_BIN"
printf '%s\n' 'x86_64' >"$SIDECAR_BIN"
if LIPO_BIN="$FAKE_LIPO" bash scripts/validate-macos-desktop-architectures.sh \
  "$APP_PATH" aarch64-apple-darwin >"$TEST_DIR/mismatch.log" 2>&1
then
  echo "FAIL: architecture gate accepted an Intel sidecar in an Apple Silicon app"
  exit 1
fi

if ! grep -Fq "Architecture mismatch" "$TEST_DIR/mismatch.log"; then
  echo "FAIL: architecture mismatch did not produce an actionable error"
  cat "$TEST_DIR/mismatch.log"
  exit 1
fi

printf '%s\n' 'arm64' >"$SIDECAR_BIN"
printf '%s\n' 'x86_64' >"$WIDGET_BIN"
if LIPO_BIN="$FAKE_LIPO" bash scripts/validate-macos-desktop-architectures.sh \
  "$APP_PATH" aarch64-apple-darwin >"$TEST_DIR/widget-mismatch.log" 2>&1
then
  echo "FAIL: architecture gate accepted an Intel WidgetKit extension in an Apple Silicon app"
  exit 1
fi

if ! grep -Fq "Architecture mismatch" "$TEST_DIR/widget-mismatch.log"; then
  echo "FAIL: widget architecture mismatch did not produce an actionable error"
  cat "$TEST_DIR/widget-mismatch.log"
  exit 1
fi

echo "PASS: macOS desktop architecture gate accepts thin/universal matching binaries and rejects a missing target architecture"
