#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
export BIFROST_UI_TEST_TARGET_DIR="${BIFROST_UI_TEST_TARGET_DIR:-$ROOT_DIR/.bifrost-ui-target}"
export BIFROST_UI_TEST_RUNNER_PORT="${BIFROST_UI_TEST_RUNNER_PORT:-18080}"

header() {
  echo
  echo "==> $1"
}

cd "$ROOT_DIR"

header "Running bifrost-e2e custom runner"
cargo run -p bifrost-e2e -- --port "$BIFROST_UI_TEST_RUNNER_PORT"

header "Running web Playwright E2E suite"
pnpm --dir web run test:ui
