#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$PROJECT_DIR/e2e-tests/test_utils/assert.sh"
source "$PROJECT_DIR/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-$PROJECT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_ROOT="${BIFROST_DATA_DIR}/process-cleanup-isolation"
OWNED_ROOT="$TEST_ROOT/owned"
PROTECTED_ROOT="$TEST_ROOT/protected"
OWNED_PID=""
PROTECTED_PID=""
OWNED_PORT=""
PROTECTED_PORT=""

cleanup() {
    safe_cleanup_proxy "$OWNED_PID"
    safe_cleanup_proxy "$PROTECTED_PID"
    rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

wait_ready() {
    local pid="$1"
    local port="$2"
    local log_file="$3"
    local waited=0
    while [[ $waited -lt 150 ]]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            cat "$log_file" >&2 || true
            return 1
        fi
        if curl -fsS "http://127.0.0.1:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    cat "$log_file" >&2 || true
    return 1
}

hash_file() {
    local python_bin
    python_bin="$(python3_cmd)" || return 1
    "$python_bin" - "$1" <<'PY'
import hashlib
import pathlib
import sys

print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

start_instance() {
    local data_dir="$1"
    local port="$2"
    local log_file="$3"
    mkdir -p "$data_dir"
    BIFROST_DATA_DIR="$data_dir" \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
        "$BIFROST_BIN" -H 127.0.0.1 -p "$port" start \
        --yes --skip-cert-check --unsafe-ssl --no-system-proxy --no-tray \
        >"$log_file" 2>&1 &
    echo $!
}

if [[ ! -x "$BIFROST_BIN" ]]; then
    _log_fail "release bifrost binary exists" "$BIFROST_BIN" "missing"
    print_test_summary
    exit 1
fi

if is_production_bifrost_path "$BIFROST_DATA_DIR"; then
    _log_fail "inherited production data directory is redirected" "isolated path" "$BIFROST_DATA_DIR"
else
    _log_pass "inherited production data directory is redirected"
fi

if kill_bifrost_in_data_root "$HOME/.bifrost" >/dev/null 2>&1; then
    _log_fail "production data root cleanup is rejected" "non-zero refusal" "cleanup allowed"
else
    _log_pass "production data root cleanup is rejected"
fi

mkdir -p "$OWNED_ROOT" "$PROTECTED_ROOT/admin"
mark_e2e_data_root "$OWNED_ROOT"
printf '%s\n' '{"version":1,"providers":[{"id":"protected-main","provider_type":"feishu","display_name":"Protected Main","enabled":false,"event_connection_enabled":false,"event_types":[],"created_at":0,"updated_at":0}]}' \
    >"$PROTECTED_ROOT/admin/im_gateway_providers.json"
PROVIDER_HASH_BEFORE="$(hash_file "$PROTECTED_ROOT/admin/im_gateway_providers.json")"

OWNED_PORT="$(allocate_free_port)"
PROTECTED_PORT="$(allocate_free_port)"
while [[ "$PROTECTED_PORT" == "$OWNED_PORT" ]]; do
    PROTECTED_PORT="$(allocate_free_port)"
done

PROTECTED_PID="$(start_instance "$PROTECTED_ROOT" "$PROTECTED_PORT" "$PROTECTED_ROOT/bifrost-test.log")"
if wait_ready "$PROTECTED_PID" "$PROTECTED_PORT" "$PROTECTED_ROOT/bifrost-test.log"; then
    _log_pass "protected main-like instance started"
else
    _log_fail "protected main-like instance started" "ready" "not ready"
fi

OWNED_PID="$(start_instance "$OWNED_ROOT" "$OWNED_PORT" "$OWNED_ROOT/bifrost-test.log")"
if wait_ready "$OWNED_PID" "$OWNED_PORT" "$OWNED_ROOT/bifrost-test.log"; then
    _log_pass "sandbox-owned test instance started"
else
    _log_fail "sandbox-owned test instance started" "ready" "not ready"
fi

BIFROST_E2E_SANDBOX_DIR="$OWNED_ROOT"
export BIFROST_E2E_SANDBOX_DIR
if kill_all_bifrost; then
    _log_pass "sandbox cleanup completed"
else
    _log_fail "sandbox cleanup completed" "success" "failed"
fi

if kill -0 "$OWNED_PID" 2>/dev/null; then
    _log_fail "sandbox-owned instance stopped" "stopped" "PID $OWNED_PID still alive"
else
    _log_pass "sandbox-owned instance stopped"
fi

if kill -0 "$PROTECTED_PID" 2>/dev/null \
    && curl -fsS "http://127.0.0.1:${PROTECTED_PORT}/_bifrost/api/proxy/address" >/dev/null; then
    _log_pass "outside protected instance remains healthy"
else
    _log_fail "outside protected instance remains healthy" "running" "stopped or unreachable"
fi

BIFROST_E2E_PROTECTED_PORTS="$PROTECTED_PORT"
export BIFROST_E2E_PROTECTED_PORTS
if kill_bifrost_on_port "$PROTECTED_PORT" >/dev/null 2>&1; then
    _log_fail "protected port cleanup is rejected" "non-zero refusal" "cleanup allowed"
else
    _log_pass "protected port cleanup is rejected"
fi
if kill -0 "$PROTECTED_PID" 2>/dev/null; then
    _log_pass "protected-port refusal preserves instance"
else
    _log_fail "protected-port refusal preserves instance" "running" "stopped"
fi

PROVIDER_HASH_AFTER="$(hash_file "$PROTECTED_ROOT/admin/im_gateway_providers.json")"
assert_equals "$PROVIDER_HASH_BEFORE" "$PROVIDER_HASH_AFTER" \
    "outside IM provider configuration remains byte-identical"

print_test_summary
