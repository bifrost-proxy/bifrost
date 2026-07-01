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

resolve_bifrost_bin() {
    local candidate="${BIFROST_BIN:-}"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        echo "$candidate"
        return 0
    fi

    for candidate in \
        "${PROJECT_DIR}/target/release/bifrost" \
        "${PROJECT_DIR}/target/release/bifrost.exe" \
        "${PROJECT_DIR}/target/debug/bifrost" \
        "${PROJECT_DIR}/target/debug/bifrost.exe"; do
        if [[ -x "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done

    return 1
}

ensure_release_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        BIFROST_BIN="$(resolve_bifrost_bin || true)"
        [[ -n "$BIFROST_BIN" ]] || {
            echo "SKIP_BUILD=true but no executable bifrost binary was found" >&2
            exit 1
        }
        export BIFROST_BIN
        echo "[security-hardening-functional] using prebuilt $BIFROST_BIN"
        return
    fi

    cargo build --release --bin bifrost
    BIFROST_BIN="$(resolve_bifrost_bin)"
    export BIFROST_BIN
    export SKIP_BUILD=true
}

run_bifrost_e2e() {
    local label="$1"
    shift
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        echo "==> $label"
        echo "[security-hardening-functional] SKIP_BUILD=true: skipping cargo-runner step in shell shard; covered by the dedicated bifrost-e2e job and local full wrapper"
        return
    fi

    run_step "$label" cargo run -p bifrost-e2e -- "$@"
}

run_bifrost_e2e "C2 functional admin brute-force lockout remains recoverable" \
    --category admin --test brute_force_lockout_after_max_failures --test-timeout 80

run_bifrost_e2e "Remote Invoke functional PoP pair-claim-open-revoke flow still works" \
    --category remote_invoke --test remote_invoke_pop_pair_claim_lookup_open_revoke --test-timeout 180

ensure_release_bifrost

run_step "M3 functional sync login CLI/API rejects HTTP relay and keeps default HTTPS login working" \
    bash e2e-tests/tests/test_sync_login_direct_e2e.sh

run_step "H1 functional BP parser explicitly opts into private-network sandbox fetch" \
    bash e2e-tests/tests/test_bp_parser_e2e.sh

run_step "H2 functional DevTools bridge still connects while admin virtual-host APIs stay protected" \
    bash e2e-tests/tests/test_devtools_page_bridge_api.sh
