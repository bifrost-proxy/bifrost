#!/usr/bin/env bash
#
# Per-crate coverage helper for Bifrost.
#
# Wraps `cargo llvm-cov` with sane defaults for measuring a single crate, the
# unit form preferred during the iterative test-writing loop.  Combined with
# the workspace-wide gate in coverage-all.sh, this enables fast feedback while
# pushing each crate toward the 90% line-coverage target.
#
# Usage:
#   bash scripts/ci/coverage-crate.sh <crate-name> [--text|--json|--html]
#                                     [--fail-under PCT]
#
# Examples:
#   bash scripts/ci/coverage-crate.sh bifrost-command --json
#   bash scripts/ci/coverage-crate.sh bifrost-power --html

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

FORMAT="text"
FAIL_UNDER=0

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <crate-name> [--text|--json|--html] [--fail-under PCT]" >&2
  exit 2
fi

CRATE="$1"; shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    --html)        FORMAT="html"; shift ;;
    --json)        FORMAT="json"; shift ;;
    --lcov)        FORMAT="lcov"; shift ;;
    --text)        FORMAT="text"; shift ;;
    --fail-under)  FAIL_UNDER="$2"; shift 2 ;;
    *) echo "Unknown option $1" >&2; exit 2 ;;
  esac
done

if ! command -v cargo-llvm-cov &>/dev/null; then
  cargo install cargo-llvm-cov
fi
if ! rustup component list --installed 2>/dev/null | grep -q llvm-tools; then
  rustup component add llvm-tools-preview
fi

OUT_DIR="target/coverage-${CRATE}"
mkdir -p "$OUT_DIR"

ARGS=(--package "$CRATE" --all-features)
case "$FORMAT" in
  html) ARGS+=(--html --output-dir "$OUT_DIR/html") ;;
  json) ARGS+=(--json --summary-only --output-path "$OUT_DIR/summary.json") ;;
  lcov) ARGS+=(--lcov --output-path "$OUT_DIR/lcov.info") ;;
  text) ARGS+=(--summary-only) ;;
esac

if [[ "$FAIL_UNDER" -gt 0 ]]; then
  ARGS+=(--fail-under-lines "$FAIL_UNDER")
fi

echo "+ cargo llvm-cov ${ARGS[*]}"
cargo llvm-cov "${ARGS[@]}"
RC=$?

if [[ "$FORMAT" == "json" && -f "$OUT_DIR/summary.json" ]]; then
  cat "$OUT_DIR/summary.json"
fi

exit $RC
