#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

# PR #8 — End-to-end test matrix for bifrost remote exec streaming.
#
# Covers:
#   1. 1 GiB binary stdout byte-exact verification (SHA-256)
#   2. Long-running task with multiple reconnects (≥4)
#   3. 5× forced kill + resume with SHA-256 verification
#   4. --output-file local file write with SHA-256 digest
#   5. Legacy envelope + stream_frame dual-track validation
#
# Prereqs:
#   - bifrost binary on PATH (build: SKIP_FRONTEND_BUILD=1 cargo build --release -p bifrost)
#   - 127.0.0.1:9900 Bifrost instance running with a Shell Access policy that
#     permits bash commands (the suite creates a local instance in $BIFROST_DATA).
#
# Usage:
#   bash scripts/remote-large-output/run-matrix.sh                 # run all
#   bash scripts/remote-large-output/run-matrix.sh case-1 case-4   # run selected
#
# Env overrides:
#   BIFROST_BIN      path to bifrost binary (default: bifrost on PATH)
#   BIFROST_E2E_DATA scratch data dir (default: mktemp -d)
#   RELAY_URL        override relay URL (default: env BIFROST_RELAY_URL)
#   CASE_1_BYTES     size of case 1 payload (default: 1073741824 = 1 GiB)
#   CASE_2_DURATION  seconds for case 2 long-run (default: 120)
#   CASE_3_ITERS     kill+resume iterations for case 3 (default: 5)
#
# Notes on byte-exact verification:
#   - `bifrost remote exec --stream --output-file <path>` writes the
#     exact bytes emitted by stdout into <path>. SHA-256 is computed on that
#     file and compared against the producer's SHA-256.
#
# Exit code: 0 on success, non-zero on any failed case.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

: "${BIFROST_BIN:=bifrost}"
: "${CASE_1_BYTES:=1073741824}"
: "${CASE_2_DURATION:=120}"
: "${CASE_3_ITERS:=5}"

CASES=("$@")
if [[ ${#CASES[@]} -eq 0 ]]; then
  CASES=(case-1 case-2 case-3 case-4 case-5)
fi

PASS=()
FAIL=()

BLUE='\033[1;34m'; GREEN='\033[1;32m'; RED='\033[1;31m'; YELLOW='\033[1;33m'; NC='\033[0m'

log() { echo -e "${BLUE}[rlo-e2e]${NC} $*"; }
ok()  { echo -e "${GREEN}[ok]${NC} $*"; }
err() { echo -e "${RED}[err]${NC} $*" >&2; }
skip() { echo -e "${YELLOW}[skip]${NC} $*"; }

run_case() {
  local name="$1"
  local script="$SCRIPT_DIR/${name}.sh"
  if [[ ! -f "$script" ]]; then
    skip "$name (no script)"
    return 0
  fi
  log "==> $name"
  if BIFROST_BIN="$BIFROST_BIN" \
     CASE_1_BYTES="$CASE_1_BYTES" \
     CASE_2_DURATION="$CASE_2_DURATION" \
     CASE_3_ITERS="$CASE_3_ITERS" \
     bash "$script"; then
    ok "$name"
    PASS+=("$name")
  else
    err "$name"
    FAIL+=("$name")
  fi
}

for c in "${CASES[@]}"; do
  run_case "$c"
done

echo
log "Summary: ${#PASS[@]} passed, ${#FAIL[@]} failed"
for p in "${PASS[@]}"; do ok "  $p"; done
for f in "${FAIL[@]}"; do err "  $f"; done

[[ ${#FAIL[@]} -eq 0 ]]
