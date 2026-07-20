#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

CLI_ROOT="$TEMP_DIR/bifrost-v0.0.0-aarch64-apple-darwin"
APP_ROOT="$TEMP_DIR/Bifrost.app"
mkdir -p "$CLI_ROOT" "$APP_ROOT/Contents/MacOS" "$APP_ROOT/Contents/Resources"
printf 'fixture binary\n' >"$CLI_ROOT/bifrost"
printf 'fixture readme\n' >"$CLI_ROOT/README.md"
printf 'fixture binary\n' >"$APP_ROOT/Contents/MacOS/bifrost"

bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$CLI_ROOT/bifrost"
tar -C "$TEMP_DIR" -czf "$TEMP_DIR/bifrost.tar.gz" "$(basename "$CLI_ROOT")"
tar -C "$TEMP_DIR" -cJf "$TEMP_DIR/bifrost.tar.xz" "$(basename "$CLI_ROOT")"
bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$TEMP_DIR/bifrost.tar.gz"
bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$TEMP_DIR/bifrost.tar.xz"
bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$APP_ROOT"
hdiutil create -quiet -fs HFS+ -srcfolder "$APP_ROOT" "$TEMP_DIR/bifrost.dmg"
bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$TEMP_DIR/bifrost.dmg"

mkdir -p "$CLI_ROOT/moss-joint-runtime/runtime/python"
printf 'dynamic runtime\n' >"$CLI_ROOT/moss-joint-runtime/runtime/python/python3.12"
tar -C "$TEMP_DIR" -czf "$TEMP_DIR/forbidden-runtime.tar.gz" "$(basename "$CLI_ROOT")"
if bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" \
  "$TEMP_DIR/forbidden-runtime.tar.gz"; then
  echo "Core package guard accepted a bundled MOSS runtime" >&2
  exit 1
fi

printf 'dynamic weight\n' >"$APP_ROOT/Contents/Resources/model.safetensors"
if bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" "$APP_ROOT"; then
  echo "Core package guard accepted a bundled model weight" >&2
  exit 1
fi

if BIFROST_MACOS_CORE_MAX_BYTES=1 \
  bash "$ROOT_DIR/scripts/ci/check-macos-release-core-payload.sh" \
  "$TEMP_DIR/bifrost.tar.gz"; then
  echo "Core package guard accepted an oversized payload" >&2
  exit 1
fi

echo "PASS: macOS core release payload guard fixture"
