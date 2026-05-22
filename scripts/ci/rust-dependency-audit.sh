#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

require_command() {
  local command_name="$1"
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "error: required command not found: $command_name" >&2
    return 1
  fi
}

require_command cargo
require_command cargo-deny
require_command cargo-udeps
require_command rustup

if ! rustup toolchain list | grep -q '^nightly'; then
  echo "error: rust nightly toolchain is required for cargo-udeps" >&2
  echo "hint: rustup toolchain install nightly --profile minimal" >&2
  exit 1
fi

echo "==> Checking duplicate Rust dependencies with cargo-deny"
cargo deny check bans --hide-inclusion-graph

echo "==> Checking unused Rust dependencies with cargo-udeps"
nightly_rustc="$(rustup which --toolchain nightly rustc)"
RUSTUP_TOOLCHAIN=nightly \
  RUSTC="$nightly_rustc" \
  SKIP_FRONTEND_BUILD=1 \
  cargo udeps --workspace --all-targets --all-features --locked
