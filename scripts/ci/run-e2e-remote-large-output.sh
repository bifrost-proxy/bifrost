#!/usr/bin/env bash
# CI entry point for the remote large-output e2e matrix.
#
# Invoked from scripts/run_all_e2e.sh (or directly from .github workflows)
# with an already-running local bifrost instance and an SSH-key connection
# pre-established. Scales heavy cases down in CI via env vars.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

# CI defaults — tune down heavy cases.
export CASE_1_BYTES="${CASE_1_BYTES:-134217728}"  # 128 MiB instead of 1 GiB for CI
export CASE_2_DURATION="${CASE_2_DURATION:-60}"
export CASE_3_ITERS="${CASE_3_ITERS:-3}"

bash scripts/remote-large-output/run-matrix.sh "$@"
