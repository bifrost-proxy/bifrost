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

if [[ "$SKIP_SIDECAR" -eq 0 ]]; then
  if [[ "$CONFIGURATION" == "release" ]]; then
    "$ROOT_DIR/scripts/prepare-macos-native-sidecar.sh" --release
  else
    "$ROOT_DIR/scripts/prepare-macos-native-sidecar.sh"
  fi
fi

swift build --package-path "$PACKAGE_DIR" -c "$SWIFT_CONFIGURATION_FLAG"

if [[ "$RUN_TESTS" -eq 1 ]]; then
  swift run --package-path "$PACKAGE_DIR" BifrostMacCoreChecks
fi
