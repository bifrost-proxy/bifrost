#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIGURATION="debug"
SKIP_BUILD=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      CONFIGURATION="release"
      shift
      ;;
    --skip-cargo-build)
      SKIP_BUILD=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  if [[ "$CONFIGURATION" == "release" ]]; then
    SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost --release
  else
    SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
  fi
fi

SOURCE_BIN="$ROOT_DIR/target/$CONFIGURATION/bifrost"
DEST_DIR="$ROOT_DIR/apps/macos/.build/sidecar/bin"
DEST_BIN="$DEST_DIR/bifrost"

if [[ ! -x "$SOURCE_BIN" ]]; then
  echo "missing built bifrost binary: $SOURCE_BIN" >&2
  exit 1
fi

mkdir -p "$DEST_DIR"
install -m 755 "$SOURCE_BIN" "$DEST_BIN"
echo "$DEST_BIN"
