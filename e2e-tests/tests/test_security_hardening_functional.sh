#!/bin/bash
#
# Functional regression coverage for the 2026-07 security hardening changes.
# This suite intentionally exercises real CLI/API/browser flows, complementing
# the narrower unit-level assertions in test_security_hardening.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "$PROJECT_DIR"

export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

run_step() {
    local label="$1"
    shift
    echo "==> $label"
    "$@"
}

ensure_release_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "${PROJECT_DIR}/target/release/bifrost" ]]; then
        return
    fi
    cargo build --release --bin bifrost
    export SKIP_BUILD=true
}

run_step "C2 functional admin brute-force lockout remains recoverable" \
    cargo run -p bifrost-e2e -- --category admin --test brute_force_lockout_after_max_failures --test-timeout 80

run_step "Remote Invoke functional PoP pair-claim-open-revoke flow still works" \
    cargo run -p bifrost-e2e -- --category remote_invoke --test remote_invoke_pop_pair_claim_lookup_open_revoke --test-timeout 180

ensure_release_bifrost

run_step "M3 functional sync login CLI/API rejects HTTP relay and keeps default HTTPS login working" \
    bash e2e-tests/tests/test_sync_login_direct_e2e.sh

run_step "H1 functional BP parser explicitly opts into private-network sandbox fetch" \
    bash e2e-tests/tests/test_bp_parser_e2e.sh

run_step "H2 functional DevTools bridge still connects while admin virtual-host APIs stay protected" \
    bash e2e-tests/tests/test_devtools_page_bridge_api.sh
