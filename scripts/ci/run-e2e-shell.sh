#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cd "$ROOT_DIR"

SHARD_ARGS=""
if [[ -n "${BIFROST_E2E_SHARD_INDEX:-}" && -n "${BIFROST_E2E_SHARD_TOTAL:-}" ]]; then
  SHARD_ARGS="--shard ${BIFROST_E2E_SHARD_INDEX}/${BIFROST_E2E_SHARD_TOTAL}"
fi

# shellcheck disable=SC2086
bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build $SHARD_ARGS
