#!/usr/bin/env bash
# Quick dev build & run - skip frontend, use mold linker, sccache
# Usage: ./scripts/dev-run.sh [--daemon] [extra args...]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export SKIP_FRONTEND_BUILD=1
export BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$ROOT_DIR/.bifrost-dev}"
export BIFROST_DISABLE_TRAY="${BIFROST_DISABLE_TRAY:-1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:-1}"
BIFROST_DEV_PORT="${BIFROST_DEV_PORT:-8800}"

# mold linker (if available)
if command -v mold &>/dev/null; then
  export RUSTFLAGS="-C link-arg=-fuse-ld=mold ${RUSTFLAGS:-}"
fi

# sccache (if available)
if command -v sccache &>/dev/null; then
  export RUSTC_WRAPPER=sccache
fi

# Use thin LTO for dev linking speed
CARGO_ARGS=(
  --config "profile.dev.codegen-units=256"
  --config "profile.dev.debug=\"line-tables-only\""
  --config "profile.dev.lto=false"
)

exec cargo run "${CARGO_ARGS[@]}" --bin bifrost -- start -p "$BIFROST_DEV_PORT" \
  --unsafe-ssl --no-system-proxy --skip-cert-check "$@"
