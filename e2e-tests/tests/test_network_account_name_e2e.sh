#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
export ADMIN_PORT

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
UPSTREAM_PORT=""
UPSTREAM_URL=""
UPSTREAM_PID=""
UPSTREAM_LOG=""
RUN_MARKER="acct_e2e_$$_$(date +%s)"

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }

start_http_fixture() {
    UPSTREAM_PORT="$(allocate_free_port)"
    UPSTREAM_URL="http://127.0.0.1:${UPSTREAM_PORT}"
    UPSTREAM_LOG="$(mktemp)"

    python3 -u - "$UPSTREAM_PORT" >"$UPSTREAM_LOG" 2>&1 <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "ok": True,
            "path": self.path,
            "via": "bifrost-network-account-name-e2e",
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    UPSTREAM_PID=$!

    for _ in $(seq 1 80); do
        if curl --noproxy "*" -fsS --max-time 2 "$UPSTREAM_URL/health" >/dev/null 2>&1; then
            log_info "Started local upstream fixture on ${UPSTREAM_URL}"
            return 0
        fi
        if ! kill -0 "$UPSTREAM_PID" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done

    log_fail "Timed out waiting for local upstream fixture on ${UPSTREAM_URL}"
    [[ -n "$UPSTREAM_LOG" ]] && tail -80 "$UPSTREAM_LOG" >&2 || true
    return 1
}

stop_http_fixture() {
    if [[ -n "$UPSTREAM_PID" ]] && kill -0 "$UPSTREAM_PID" 2>/dev/null; then
        kill "$UPSTREAM_PID" 2>/dev/null || true
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
    [[ -n "$UPSTREAM_LOG" ]] && rm -f "$UPSTREAM_LOG" 2>/dev/null || true
    UPSTREAM_PID=""
    UPSTREAM_LOG=""
}

run_test() {
    local test_name="$1"
    local test_func="$2"

    TESTS_RUN=$((TESTS_RUN + 1))
    log_info "Running test: $test_name"

    if $test_func; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_pass "$test_name"
        return 0
    fi

    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_fail "$test_name"
    return 1
}

configure_accounts() {
    set_userpass_config '{
        "enabled": true,
        "accounts": [
            {"username": "alice-account", "password": "alice-secret-123", "enabled": true},
            {"username": "bob-account", "password": "bob-secret-123", "enabled": true}
        ],
        "loopback_requires_auth": true
    }' >/dev/null
    sleep 0.3
}

proxy_get_with_account() {
    local account="$1"
    local password="$2"
    local marker="$3"
    curl -sS -o /dev/null -w '%{http_code}' \
        --noproxy "" \
        --proxy "http://127.0.0.1:${ADMIN_PORT}" \
        --proxy-user "${account}:${password}" \
        --max-time 10 \
        "${UPSTREAM_URL}/get/${marker}" 2>/dev/null
}

traffic_list_for_marker() {
    local marker="$1"
    admin_get "/api/traffic?path_contains=${marker}&limit=20"
}

wait_for_account_record() {
    local marker="$1"
    local expected_account="$2"

    for _ in $(seq 1 60); do
        local response
        response="$(traffic_list_for_marker "$marker")"
        if echo "$response" | jq -e --arg account "$expected_account" \
            '.records[]? | select(.acct == $account)' >/dev/null; then
            printf '%s' "$response"
            return 0
        fi
        sleep 0.2
    done

    log_fail "Timed out waiting for traffic record marker=${marker} account=${expected_account}"
    traffic_list_for_marker "$marker" >&2 || true
    return 1
}

record_id_for_account_marker() {
    local marker="$1"
    local account="$2"
    traffic_list_for_marker "$marker" | jq -r --arg account "$account" \
        '.records[]? | select(.acct == $account) | .id' | head -1
}

test_proxy_traffic_records_account_name() {
    configure_accounts

    local alice_marker="${RUN_MARKER}_alice"
    local status
    status="$(proxy_get_with_account "alice-account" "alice-secret-123" "$alice_marker")"
    if [[ "$status" != "200" ]]; then
        log_fail "alice proxy request should return 200, got $status"
        return 1
    fi

    local list_response
    list_response="$(wait_for_account_record "$alice_marker" "alice-account")" || return 1

    if ! echo "$list_response" | jq -e --arg account "alice-account" \
        '.records[]? | select(.acct == $account and .h == "127.0.0.1")' >/dev/null; then
        log_fail "traffic list should expose compact acct=alice-account"
        echo "$list_response" >&2
        return 1
    fi

    local record_id
    record_id="$(record_id_for_account_marker "$alice_marker" "alice-account")"
    if [[ -z "$record_id" || "$record_id" == "null" ]]; then
        log_fail "could not find alice traffic id"
        return 1
    fi

    local detail
    detail="$(admin_get "/api/traffic/${record_id}")"
    if [[ "$(echo "$detail" | jq -r '.account_name // empty')" != "alice-account" ]]; then
        log_fail "traffic detail should persist account_name=alice-account"
        echo "$detail" >&2
        return 1
    fi

    return 0
}

test_account_name_filter_is_exact() {
    configure_accounts

    local alice_marker="${RUN_MARKER}_filter_alice"
    local bob_marker="${RUN_MARKER}_filter_bob"
    local status

    status="$(proxy_get_with_account "alice-account" "alice-secret-123" "$alice_marker")"
    if [[ "$status" != "200" ]]; then
        log_fail "alice proxy request should return 200, got $status"
        return 1
    fi
    wait_for_account_record "$alice_marker" "alice-account" >/dev/null || return 1

    status="$(proxy_get_with_account "bob-account" "bob-secret-123" "$bob_marker")"
    if [[ "$status" != "200" ]]; then
        log_fail "bob proxy request should return 200, got $status"
        return 1
    fi
    wait_for_account_record "$bob_marker" "bob-account" >/dev/null || return 1

    local alice_filter
    alice_filter="$(admin_get "/api/traffic?account_name=alice-account&account_name_match=equals&limit=100")"
    if ! echo "$alice_filter" | jq -e --arg marker "$alice_marker" \
        '.records[]? | select(.acct == "alice-account" and (.p | contains($marker)))' >/dev/null; then
        log_fail "exact account_name filter should include alice marker"
        echo "$alice_filter" >&2
        return 1
    fi
    if echo "$alice_filter" | jq -e --arg marker "$bob_marker" \
        '.records[]? | select(.p | contains($marker))' >/dev/null; then
        log_fail "exact account_name filter should exclude bob marker"
        echo "$alice_filter" >&2
        return 1
    fi

    local query_filter
    query_filter="$(env NO_PROXY="*" no_proxy="*" curl -sS \
        -X POST -H "Content-Type: application/json" \
        -d '{"account_name":"bob-account","account_name_match":"equals","limit":100}' \
        "$(admin_base_url)/api/traffic/query")"
    if ! echo "$query_filter" | jq -e --arg marker "$bob_marker" \
        '.records[]? | select(.acct == "bob-account" and (.p | contains($marker)))' >/dev/null; then
        log_fail "POST traffic query should support account_name filter for bob"
        echo "$query_filter" >&2
        return 1
    fi
    if echo "$query_filter" | jq -e --arg marker "$alice_marker" \
        '.records[]? | select(.p | contains($marker))' >/dev/null; then
        log_fail "POST account_name filter should exclude alice marker"
        echo "$query_filter" >&2
        return 1
    fi

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Network Account Name E2E Test Summary"
    echo "======================================"
    echo "Tests Run:    $TESTS_RUN"
    echo "Tests Passed: $TESTS_PASSED"
    echo "Tests Failed: $TESTS_FAILED"
    echo "======================================"

    if [[ $TESTS_FAILED -eq 0 ]]; then
        echo "All tests passed!"
        return 0
    fi

    echo "Some tests failed!"
    return 1
}

main() {
    trap 'stop_http_fixture; admin_cleanup_bifrost' EXIT

    if ! admin_ensure_bifrost; then
        log_fail "Admin server is not reachable and failed to start"
        exit 1
    fi
    if ! start_http_fixture; then
        log_fail "Local upstream fixture failed to start"
        exit 1
    fi

    log_info "Starting Network Account Name E2E Tests"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    log_info "Upstream: $UPSTREAM_URL"
    echo ""

    clear_traffic >/dev/null 2>&1 || true
    run_test "Proxy traffic persists authenticated account name" test_proxy_traffic_records_account_name
    run_test "Traffic account_name filters are exact and isolate accounts" test_account_name_filter_is_exact

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
