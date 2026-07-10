#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_ROOT"

echo "[desktop-upgrade-handoff] running desktop handoff contract tests"

if [[ "$(uname -s)" == "Linux" ]]; then
  if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists glib-2.0 >/dev/null 2>&1; then
    echo "[desktop-upgrade-handoff] SKIP: glib-2.0 pkg-config metadata is not available on this Linux runner"
    echo "[desktop-upgrade-handoff]       desktop/src-tauri depends on Tauri's GTK stack on Linux; macOS/desktop-capable runners execute the contract"
    exit 0
  fi
fi

if ! compgen -G "$REPO_ROOT/desktop/src-tauri/resources/bin/*" >/dev/null; then
  echo "[desktop-upgrade-handoff] SKIP: desktop sidecar resources are not prepared"
  echo "[desktop-upgrade-handoff]       run this contract in a desktop-capable build environment with desktop/src-tauri/resources/bin populated"
  exit 0
fi

CARGO_TARGET_DIR="$REPO_ROOT/target/desktop-upgrade-handoff-contract" \
  cargo test --manifest-path desktop/src-tauri/Cargo.toml upgrade_relaunch -- --nocapture

echo "[desktop-upgrade-handoff] PASS"
