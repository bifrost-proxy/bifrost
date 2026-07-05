#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_PORT="${ADMIN_PORT:-}"
PROXY_PORT="${PROXY_PORT:-}"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-}"

if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
if [[ -z "$PROXY_PORT" ]]; then
    PROXY_PORT="$ADMIN_PORT"
fi
if [[ -z "$MOCK_HTTP_PORT" ]]; then
    MOCK_HTTP_PORT="$(allocate_free_port)"
fi

export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
export ADMIN_BASE_URL

TEST_ROOT=""
MOCK_PID=""
MOCK_LOG=""

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*" >&2; }

cleanup() {
    if [[ -n "$MOCK_PID" ]]; then
        kill_pid "$MOCK_PID"
        wait_pid "$MOCK_PID"
        MOCK_PID=""
    fi
    admin_cleanup_bifrost || true
    kill_bifrost_on_port "$PROXY_PORT" || true
    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" ]]; then
        rm -rf "$TEST_ROOT" || true
    fi
}

trap cleanup EXIT

require_tools() {
    command -v jq >/dev/null 2>&1 || {
        log_fail "jq is required"
        exit 1
    }
    command -v python3 >/dev/null 2>&1 || {
        log_fail "python3 is required"
        exit 1
    }
}

start_mock_http() {
    MOCK_LOG="$TEST_ROOT/mock-http.log"
    PYTHONUNBUFFERED=1 python3 \
        "$SCRIPT_DIR/../mock_servers/http_echo_server.py" \
        --port "$MOCK_HTTP_PORT" --retries 5 >"$MOCK_LOG" 2>&1 &
    MOCK_PID=$!

    local start
    start="$(date +%s)"
    while true; do
        local actual_port
        actual_port="$(sed -nE \
            -e 's/^Starting HTTP Echo Server on [^:]+:([0-9]+)\.\.\.$/\1/p' \
            -e 's/^NOTE: Requested port [0-9]+ was busy; bound to ([0-9]+) instead$/\1/p' \
            "$MOCK_LOG" 2>/dev/null | tail -n 1)"
        if [[ -n "$actual_port" ]]; then
            MOCK_HTTP_PORT="$actual_port"
        fi

        if grep -q '^READY$' "$MOCK_LOG" 2>/dev/null \
            && curl -fsS --connect-timeout 2 --max-time 5 \
                "http://127.0.0.1:${MOCK_HTTP_PORT}/get" >/dev/null 2>&1; then
            return 0
        fi

        if ! kill -0 "$MOCK_PID" 2>/dev/null; then
            log_fail "mock HTTP server exited early"
            cat "$MOCK_LOG" >&2 || true
            return 1
        fi

        if (( $(date +%s) - start > 60 )); then
            log_fail "mock HTTP server did not become ready"
            cat "$MOCK_LOG" >&2 || true
            return 1
        fi
        sleep 0.2
    done
}

admin_get() {
    env NO_PROXY="*" no_proxy="*" curl -fsS "$ADMIN_BASE_URL$1"
}

admin_post_json() {
    local path="$1"
    local payload="$2"
    env NO_PROXY="*" no_proxy="*" curl -fsS \
        -X POST \
        -H "Content-Type: application/json" \
        --data-binary "$payload" \
        "$ADMIN_BASE_URL$path"
}

create_mock_rule() {
    local payload
    payload="$(python3 - <<'PY'
import json
print(json.dumps({
    "name": "trusted-metrics-mock",
    "content": "trusted-metrics-mock.local statusCode://209 resBody://trusted-mock-download-body",
    "enabled": True,
}))
PY
)"
    admin_post_json "/api/rules" "$payload" >/dev/null
}

proxy_curl() {
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" \
        --connect-timeout 5 --max-time 15 "$@"
}

traffic_record_json() {
    local needle="$1"
    local id=""
    for _ in $(seq 1 30); do
        id="$(admin_get "/api/traffic?limit=100" | jq -r --arg needle "$needle" '
            [.records[]? | select(((.url // .p // .path // "") | contains($needle)))] |
            sort_by(.seq // 0) |
            last |
            .id // empty
        ')"
        if [[ -n "$id" ]]; then
            admin_get "/api/traffic/${id}"
            return 0
        fi
        sleep 0.2
    done
    log_fail "traffic record containing '${needle}' was not found"
    admin_get "/api/traffic?limit=20" >&2 || true
    return 1
}

assert_num_ge() {
    local actual="$1"
    local expected="$2"
    local message="$3"
    if awk "BEGIN { exit !($actual >= $expected) }"; then
        log_pass "$message (${actual} >= ${expected})"
    else
        log_fail "$message (${actual} < ${expected})"
        return 1
    fi
}

assert_num_eq() {
    local actual="$1"
    local expected="$2"
    local message="$3"
    if [[ "$actual" == "$expected" ]]; then
        log_pass "$message (${actual})"
    else
        log_fail "$message (expected ${expected}, got ${actual})"
        return 1
    fi
}

main() {
    require_tools
    TEST_ROOT="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_ROOT/data"
    mkdir -p "$BIFROST_DATA_DIR"

    log_info "Starting mock HTTP server on ${MOCK_HTTP_PORT}"
    start_mock_http

    log_info "Starting Bifrost on ${PROXY_PORT}"
    ADMIN_PORT="$PROXY_PORT" PROXY_PORT="$PROXY_PORT" admin_ensure_bifrost
    create_mock_rule

    local before
    before="$(admin_get "/api/metrics")"
    local before_requests before_upload before_download
    before_requests="$(echo "$before" | jq -r '.total_requests')"
    before_upload="$(echo "$before" | jq -r '.bytes_sent')"
    before_download="$(echo "$before" | jq -r '.bytes_received')"

    local upload_body
    upload_body="trusted-upload-payload-1234567890"
    local upload_len="${#upload_body}"

    log_info "Sending one real POST request through proxy"
    proxy_curl \
        -X POST \
        -H "Content-Type: text/plain" \
        --data-binary "$upload_body" \
        "http://127.0.0.1:${MOCK_HTTP_PORT}/trusted_metrics_post" >/dev/null

    log_info "Sending three burst GET requests through proxy"
    for i in 1 2 3; do
        proxy_curl "http://127.0.0.1:${MOCK_HTTP_PORT}/trusted_metrics_burst_${i}.json" >/dev/null
    done

    log_info "Sending one Mock direct response request through proxy"
    local mock_body
    mock_body="$(proxy_curl "http://trusted-metrics-mock.local/trusted_metrics_mock")"
    if [[ "$mock_body" != "trusted-mock-download-body" ]]; then
        log_fail "mock body mismatch: ${mock_body}"
        exit 1
    fi

    local after
    after="$(admin_get "/api/metrics")"
    local after_requests after_upload after_download qps upload_rate download_rate
    after_requests="$(echo "$after" | jq -r '.total_requests')"
    after_upload="$(echo "$after" | jq -r '.bytes_sent')"
    after_download="$(echo "$after" | jq -r '.bytes_received')"
    qps="$(echo "$after" | jq -r '.qps')"
    upload_rate="$(echo "$after" | jq -r '.bytes_sent_rate')"
    download_rate="$(echo "$after" | jq -r '.bytes_received_rate')"

    assert_num_eq "$((after_requests - before_requests))" "5" "proxy request counter is not double counted" || exit 1
    assert_num_ge "$((after_upload - before_upload))" "$upload_len" "upload bytes include real POST body" || exit 1
    assert_num_ge "$((after_download - before_download))" "${#mock_body}" "download bytes include Mock body" || exit 1
    assert_num_ge "$qps" "5" "QPS reflects the recent burst" || exit 1
    assert_num_ge "$upload_rate" "$upload_len" "upload rate reflects recent bytes" || exit 1
    assert_num_ge "$download_rate" "${#mock_body}" "download rate reflects recent bytes" || exit 1

    local post_record mock_record
    post_record="$(traffic_record_json "trusted_metrics_post")"
    mock_record="$(traffic_record_json "trusted_metrics_mock")"

    local post_upload post_download mock_upload mock_download
    post_upload="$(echo "$post_record" | jq -r '.upload_bytes // 0')"
    post_download="$(echo "$post_record" | jq -r '.download_bytes // 0')"
    mock_upload="$(echo "$mock_record" | jq -r '.upload_bytes // 0')"
    mock_download="$(echo "$mock_record" | jq -r '.download_bytes // 0')"

    assert_num_ge "$post_upload" "$upload_len" "traffic detail stores trusted POST upload bytes" || exit 1
    assert_num_ge "$post_download" "1" "traffic detail stores trusted POST download bytes" || exit 1
    assert_num_eq "$mock_upload" "0" "Mock GET upload bytes remain zero" || exit 1
    assert_num_eq "$mock_download" "${#mock_body}" "Mock response body is stored as trusted download bytes" || exit 1

    local hosts apps
    hosts="$(admin_get "/api/metrics/hosts")"
    apps="$(admin_get "/api/metrics/apps")"
    local host_download app_download
    host_download="$(echo "$hosts" | jq -r '[.[] | select(.host == "trusted-metrics-mock.local") | .bytes_received] | add // 0')"
    app_download="$(echo "$apps" | jq -r '[.[] | .bytes_received] | add // 0')"
    assert_num_ge "$host_download" "${#mock_body}" "host distribution uses trusted download bytes" || exit 1
    assert_num_ge "$app_download" "${#mock_body}" "app distribution uses trusted download bytes" || exit 1

    log_pass "trustworthy traffic metrics E2E passed"
}

main "$@"
