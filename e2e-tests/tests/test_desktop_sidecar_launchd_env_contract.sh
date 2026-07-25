#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

has_pattern() {
  local pattern="$1"
  local file="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -q "$pattern" "$file"
  else
    grep -q "$pattern" "$file"
  fi
}

if ! has_pattern 'BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL' desktop/src-tauri/src/main.rs; then
  echo "Desktop sidecar launchd suppression env is missing from desktop startup code" >&2
  exit 1
fi

if ! has_pattern 'SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL_ENV' crates/bifrost-cli/src/commands/start.rs; then
  echo "CLI LaunchDaemon install guard is missing from start.rs" >&2
  exit 1
fi

if ! has_pattern 'env_remove\(DETACHED_DAEMON_CHILD_ENV\)' desktop/src-tauri/src/main.rs; then
  echo "Desktop sidecar does not clear the inherited detached daemon marker" >&2
  exit 1
fi

if [[ "$(uname -s)" == "Linux" ]] && {
  ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists gobject-2.0;
}; then
  echo "skipping desktop sidecar Rust test: Linux Tauri GTK/GObject dependencies are not installed"
else
  pnpm --dir web run build:desktop
  SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
  node scripts/prepare-tauri-sidecar.mjs debug
  cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar -- --nocapture
fi

cargo test -p bifrost-cli desktop_core --lib -- --nocapture
cargo test -p bifrost-cli live_desktop_runtime --lib -- --nocapture
cargo test -p bifrost-cli runtime_info_new_desktop_is_app_bound_not_cli_restartable --lib -- --nocapture
