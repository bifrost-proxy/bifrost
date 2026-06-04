#!/usr/bin/env bash
# Quick dev build & run - skip frontend, use mold linker, sccache
# Usage: ./scripts/dev-run.sh [--daemon] [extra args...]
set -euo pipefail

export SKIP_FRONTEND_BUILD=1

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

exec cargo run "${CARGO_ARGS[@]}" --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy "$@"
