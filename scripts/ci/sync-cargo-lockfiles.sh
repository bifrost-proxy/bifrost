#!/usr/bin/env bash

set -euo pipefail

mode=sync
if [[ "${1:-}" == "--check" ]]; then
  mode=check
  shift
fi
if [[ "$#" -gt 1 || "${1:-}" == --* ]]; then
  echo "usage: sync-cargo-lockfiles.sh [--check] [repository-root]" >&2
  exit 2
fi

ROOT_DIR="$(cd "${1:-$(dirname "${BASH_SOURCE[0]}")/../..}" && pwd)"
command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required" >&2
  exit 1
}

# Unlike a blanket update, --workspace retains already locked dependencies.
# The desktop declares its own workspace and therefore has a separate lockfile.
for manifest in Cargo.toml desktop/src-tauri/Cargo.toml; do
  echo "==> $mode $manifest lockfile"
  if [[ "$mode" == check ]]; then
    cargo update --workspace --locked --manifest-path "$ROOT_DIR/$manifest"
  else
    cargo update --workspace --manifest-path "$ROOT_DIR/$manifest"
  fi
done
