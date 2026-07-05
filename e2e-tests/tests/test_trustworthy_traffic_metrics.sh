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
TEMP_PROXY_PORT="${TEMP_PROXY_PORT:-}"
WS_PORT="${WS_PORT:-}"
SOCKS5_PORT="${SOCKS5_PORT:-}"
HTTPS_PORT="${HTTPS_PORT:-}"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"

if [[ ! -x "$BIFROST_BIN" ]]; then
    BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
fi

if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
if [[ -z "$PROXY_PORT" ]]; then
    PROXY_PORT="$ADMIN_PORT"
fi
if [[ -z "$MOCK_HTTP_PORT" ]]; then
    MOCK_HTTP_PORT="$(allocate_free_port)"
fi
if [[ -z "$TEMP_PROXY_PORT" ]]; then
    TEMP_PROXY_PORT="$(allocate_free_port)"
fi
if [[ -z "$WS_PORT" ]]; then
    WS_PORT="$(allocate_free_port)"
fi
if [[ -z "$SOCKS5_PORT" ]]; then
    SOCKS5_PORT="$(allocate_free_port)"
fi
if [[ -z "$HTTPS_PORT" ]]; then
    HTTPS_PORT="$(allocate_free_port)"
fi

export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
export ADMIN_BASE_URL

TEST_ROOT=""
MOCK_PID=""
MOCK_LOG=""
WS_PID=""
WS_LOG=""
HTTPS_PID=""
HTTPS_LOG=""
BIFROST_PID=""
BIFROST_LOG=""

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*" >&2; }

cleanup() {
    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" && -x "$BIFROST_BIN" ]]; then
        BIFROST_DATA_DIR="$TEST_ROOT/data" "$BIFROST_BIN" port destroy "$TEMP_PROXY_PORT" >/dev/null 2>&1 || true
    fi
    if [[ -n "$MOCK_PID" ]]; then
        kill_pid "$MOCK_PID"
        wait_pid "$MOCK_PID"
        MOCK_PID=""
    fi
    if [[ -n "$WS_PID" ]]; then
        kill_pid "$WS_PID"
        wait_pid "$WS_PID"
        WS_PID=""
    fi
    if [[ -n "$HTTPS_PID" ]]; then
        kill_pid "$HTTPS_PID"
        wait_pid "$HTTPS_PID"
        HTTPS_PID=""
    fi
    if [[ -n "$BIFROST_PID" ]]; then
        kill_pid "$BIFROST_PID"
        wait_pid "$BIFROST_PID"
        BIFROST_PID=""
    fi
    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" && -x "$BIFROST_BIN" ]]; then
        BIFROST_DATA_DIR="$TEST_ROOT/data" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi
    kill_bifrost_on_port "$PROXY_PORT" || true
    kill_bifrost_on_port "$SOCKS5_PORT" || true
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

start_ws_server() {
    WS_LOG="$TEST_ROOT/ws-echo.log"
    PYTHONUNBUFFERED=1 python3 \
        "$SCRIPT_DIR/../mock_servers/ws_echo_server.py" \
        --port "$WS_PORT" >"$WS_LOG" 2>&1 &
    WS_PID=$!

    for _ in $(seq 1 60); do
        if ! kill -0 "$WS_PID" 2>/dev/null; then
            log_fail "WebSocket server exited early"
            cat "$WS_LOG" >&2 || true
            return 1
        fi
        if python3 -c "import socket; s=socket.create_connection(('127.0.0.1', ${WS_PORT}), 2); s.close()" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    log_fail "WebSocket server did not become ready"
    cat "$WS_LOG" >&2 || true
    return 1
}

start_https_server() {
    HTTPS_LOG="$TEST_ROOT/https-echo.log"
    PYTHONUNBUFFERED=1 python3 \
        "$SCRIPT_DIR/../mock_servers/https_echo_server.py" \
        "$HTTPS_PORT" >"$HTTPS_LOG" 2>&1 &
    HTTPS_PID=$!

    for _ in $(seq 1 80); do
        if ! kill -0 "$HTTPS_PID" 2>/dev/null; then
            log_fail "HTTPS server exited early"
            cat "$HTTPS_LOG" >&2 || true
            return 1
        fi
        if grep -q '^READY$' "$HTTPS_LOG" 2>/dev/null \
            && curl -kfsS --connect-timeout 2 --max-time 5 \
                "https://127.0.0.1:${HTTPS_PORT}/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    log_fail "HTTPS server did not become ready"
    cat "$HTTPS_LOG" >&2 || true
    return 1
}

start_bifrost_with_socks() {
    BIFROST_LOG="$TEST_ROOT/bifrost.log"
    local home_dir xdg_config_dir xdg_data_dir
    home_dir="$TEST_ROOT/home"
    xdg_config_dir="$TEST_ROOT/xdg-config"
    xdg_data_dir="$TEST_ROOT/xdg-data"
    mkdir -p "$home_dir" "$xdg_config_dir" "$xdg_data_dir"

    SKIP_FRONTEND_BUILD=1 \
        HOME="$home_dir" \
        XDG_CONFIG_HOME="$xdg_config_dir" \
        XDG_DATA_HOME="$xdg_data_dir" \
        BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
        "$BIFROST_BIN" \
        -H "$ADMIN_HOST" \
        -p "$PROXY_PORT" \
        --socks5-port "$SOCKS5_PORT" \
        start \
        -y \
        --access-mode allow_all \
        --skip-cert-check \
        --no-system-proxy \
        --unsafe-ssl >"$BIFROST_LOG" 2>&1 &
    BIFROST_PID=$!

    for _ in $(seq 1 120); do
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            log_fail "Bifrost exited during startup"
            cat "$BIFROST_LOG" >&2 || true
            return 1
        fi
        if admin_get "/api/proxy/address" >/dev/null 2>&1 \
            && ! port_is_available "$SOCKS5_PORT"; then
            return 0
        fi
        sleep 0.2
    done
    log_fail "Bifrost did not become ready"
    cat "$BIFROST_LOG" >&2 || true
    return 1
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

    payload="$(python3 - <<'PY'
import json
print(json.dumps({
    "name": "trusted-metrics-temp",
    "content": "trusted-metrics-temp.local statusCode://211 resBody://trusted-temp-download-body",
    "enabled": True,
}))
PY
)"
    admin_post_json "/api/rules" "$payload" >/dev/null
}

bind_temp_port() {
    local output
    output="$TEST_ROOT/temp-port-bind.log"
    for _ in $(seq 1 5); do
        if BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" port bind \
            --port "$TEMP_PROXY_PORT" \
            --rule trusted-metrics-temp >"$output" 2>&1; then
            for _ in $(seq 1 50); do
                if port_is_available "$TEMP_PROXY_PORT"; then
                    sleep 0.1
                    continue
                fi
                return 0
            done
        fi
        if grep -qiE "address already in use|addrinuse|another process is already listening|os error 98|os error 48|os error 10048" "$output" 2>/dev/null; then
            TEMP_PROXY_PORT="$(allocate_free_port)"
            continue
        fi
        log_fail "failed to bind temp proxy port"
        cat "$output" >&2 || true
        return 1
    done
    log_fail "failed to bind temp proxy port after retries"
    cat "$output" >&2 || true
    return 1
}

proxy_curl() {
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" \
        --connect-timeout 5 --max-time 15 "$@"
}

proxy_curl_on_port() {
    local port="$1"
    shift
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sS --proxy "http://127.0.0.1:${port}" --noproxy "" \
        --connect-timeout 5 --max-time 15 "$@"
}

proxy_curl_socks() {
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sS --socks5-hostname "127.0.0.1:${SOCKS5_PORT}" --noproxy "" \
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

traffic_record_by_protocol_json() {
    local protocol="$1"
    local id=""
    for _ in $(seq 1 30); do
        id="$(admin_get "/api/traffic?limit=100" | jq -r --arg protocol "$protocol" '
            [.records[]? | select((.proto // .protocol // "") == $protocol)] |
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
    log_fail "traffic record with protocol '${protocol}' was not found"
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

    log_info "Starting WebSocket echo server on ${WS_PORT}"
    start_ws_server

    log_info "Starting HTTPS echo server on ${HTTPS_PORT}"
    start_https_server

    log_info "Starting Bifrost on ${PROXY_PORT} with SOCKS5 on ${SOCKS5_PORT}"
    ADMIN_PORT="$PROXY_PORT" PROXY_PORT="$PROXY_PORT" start_bifrost_with_socks
    create_mock_rule
    log_info "Binding temporary proxy port ${TEMP_PROXY_PORT}"
    bind_temp_port

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

    log_info "Sending one SSE response through proxy"
    local sse_output sse_len
    sse_output="$TEST_ROOT/sse.out"
    proxy_curl "http://127.0.0.1:${MOCK_HTTP_PORT}/sse?count=4&marker=trusted_metrics_sse" >"$sse_output"
    sse_len="$(wc -c <"$sse_output" | tr -d ' ')"
    if ! grep -q 'event: message' "$sse_output"; then
        log_fail "SSE response did not contain event frames"
        exit 1
    fi

    log_info "Sending one Mock direct response request through temporary proxy port"
    local temp_body
    temp_body="$(proxy_curl_on_port "$TEMP_PROXY_PORT" "http://trusted-metrics-temp.local/trusted_metrics_temp_port")"
    if [[ "$temp_body" != "trusted-temp-download-body" ]]; then
        log_fail "temporary port mock body mismatch: ${temp_body}"
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

    assert_num_eq "$((after_requests - before_requests))" "7" "proxy request counter is not double counted across HTTP, SSE, Mock, and temp port" || exit 1
    assert_num_ge "$((after_upload - before_upload))" "$upload_len" "upload bytes include real POST body" || exit 1
    assert_num_ge "$((after_download - before_download))" "${#mock_body}" "download bytes include Mock body" || exit 1
    assert_num_ge "$qps" "7" "QPS reflects the recent mixed burst" || exit 1
    assert_num_ge "$upload_rate" "$upload_len" "upload rate reflects recent bytes" || exit 1
    assert_num_ge "$download_rate" "${#mock_body}" "download rate reflects recent bytes" || exit 1

    local post_record mock_record sse_record temp_record
    post_record="$(traffic_record_json "trusted_metrics_post")"
    mock_record="$(traffic_record_json "trusted_metrics_mock")"
    sse_record="$(traffic_record_json "/sse")"
    temp_record="$(traffic_record_json "trusted_metrics_temp_port")"

    local post_upload post_download mock_upload mock_download sse_upload sse_download sse_flag temp_upload temp_download temp_listener temp_rule_hit
    post_upload="$(echo "$post_record" | jq -r '.upload_bytes // 0')"
    post_download="$(echo "$post_record" | jq -r '.download_bytes // 0')"
    mock_upload="$(echo "$mock_record" | jq -r '.upload_bytes // 0')"
    mock_download="$(echo "$mock_record" | jq -r '.download_bytes // 0')"
    sse_upload="$(echo "$sse_record" | jq -r '.upload_bytes // 0')"
    sse_download="$(echo "$sse_record" | jq -r '.download_bytes // 0')"
    sse_flag="$(echo "$sse_record" | jq -r '.is_sse // false')"
    temp_upload="$(echo "$temp_record" | jq -r '.upload_bytes // 0')"
    temp_download="$(echo "$temp_record" | jq -r '.download_bytes // 0')"
    temp_listener="$(echo "$temp_record" | jq -r '.listener_port // 0')"
    temp_rule_hit="$(echo "$temp_record" | jq -r --arg name "trusted-metrics-temp" '
        any((.matched_rules // [])[]?; ((.name // .rule_name // "") | endswith($name)))
    ')"

    assert_num_ge "$post_upload" "$upload_len" "traffic detail stores trusted POST upload bytes" || exit 1
    assert_num_ge "$post_download" "1" "traffic detail stores trusted POST download bytes" || exit 1
    assert_num_eq "$mock_upload" "0" "Mock GET upload bytes remain zero" || exit 1
    assert_num_eq "$mock_download" "${#mock_body}" "Mock response body is stored as trusted download bytes" || exit 1
    assert_num_ge "$sse_upload" "1" "SSE GET upload bytes include real request bytes" || exit 1
    assert_num_ge "$sse_download" "$sse_len" "SSE streamed response bytes are stored as trusted download bytes" || exit 1
    if [[ "$sse_flag" != "true" ]]; then
        log_fail "SSE traffic record is not marked as is_sse=true"
        exit 1
    fi
    log_pass "SSE traffic record is marked as is_sse=true"
    assert_num_eq "$temp_upload" "0" "temporary port Mock GET upload bytes remain zero" || exit 1
    assert_num_eq "$temp_download" "${#temp_body}" "temporary port Mock response body is trusted download bytes" || exit 1
    assert_num_eq "$temp_listener" "$TEMP_PROXY_PORT" "temporary port traffic stores listener_port" || exit 1
    if [[ "$temp_rule_hit" != "true" ]]; then
        log_fail "temporary port traffic did not record trusted-metrics-temp rule hit"
        echo "$temp_record" | jq '.matched_rules // []' >&2 || true
        exit 1
    fi
    log_pass "temporary port traffic records enabled rule details"

    local hosts apps
    hosts="$(admin_get "/api/metrics/hosts")"
    apps="$(admin_get "/api/metrics/apps")"
    local host_download app_download
    host_download="$(echo "$hosts" | jq -r '[.[] | select(.host == "trusted-metrics-mock.local") | .bytes_received] | add // 0')"
    app_download="$(echo "$apps" | jq -r '[.[] | .bytes_received] | add // 0')"
    assert_num_ge "$host_download" "${#mock_body}" "host distribution uses trusted download bytes" || exit 1
    assert_num_ge "$app_download" "${#mock_body}" "app distribution uses trusted download bytes" || exit 1

    log_info "Sending WebSocket frames through proxy"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "127.0.0.1" \
        --proxy-port "$PROXY_PORT" \
        --host-header "127.0.0.1:${WS_PORT}" \
        --path "/ws?trusted_metrics_ws=1" \
        --message '{"type":"trusted-metrics-ws"}' \
        --messages 3 \
        --timeout 15

    local after_ws ws_record ws_upload ws_download ws_flag after_ws_requests
    after_ws="$(admin_get "/api/metrics")"
    after_ws_requests="$(echo "$after_ws" | jq -r '.total_requests')"
    assert_num_eq "$((after_ws_requests - before_requests))" "8" "WebSocket traffic increments request counter once" || exit 1

    ws_record="$(traffic_record_json "/ws")"
    ws_upload="$(echo "$ws_record" | jq -r '.upload_bytes // 0')"
    ws_download="$(echo "$ws_record" | jq -r '.download_bytes // 0')"
    ws_flag="$(echo "$ws_record" | jq -r '.is_websocket // false')"
    if [[ "$ws_flag" != "true" ]]; then
        log_fail "WebSocket traffic record is not marked as is_websocket=true"
        exit 1
    fi
    log_pass "WebSocket traffic record is marked as is_websocket=true"
    assert_num_ge "$ws_upload" "1" "WebSocket upload bytes include client frames" || exit 1
    assert_num_ge "$ws_download" "1" "WebSocket download bytes include server frames" || exit 1

    log_info "Sending one HTTP request through SOCKS5 proxy"
    local socks_body after_socks socks_record socks_upload socks_download socks_flag after_socks_requests
    socks_body="$(proxy_curl_socks "http://127.0.0.1:${MOCK_HTTP_PORT}/trusted_metrics_socks")"
    if [[ "$socks_body" != *"trusted_metrics_socks"* ]]; then
        log_fail "SOCKS5 HTTP body did not contain the expected path"
        exit 1
    fi
    after_socks="$(admin_get "/api/metrics")"
    after_socks_requests="$(echo "$after_socks" | jq -r '.total_requests')"
    assert_num_eq "$((after_socks_requests - before_requests))" "9" "SOCKS5 HTTP traffic increments request counter once" || exit 1

    socks_record="$(traffic_record_json "trusted_metrics_socks")"
    socks_upload="$(echo "$socks_record" | jq -r '.upload_bytes // 0')"
    socks_download="$(echo "$socks_record" | jq -r '.download_bytes // 0')"
    socks_flag="$(echo "$socks_record" | jq -r '.protocol // ""')"
    if [[ "$socks_flag" != "socks5-http" ]]; then
        log_fail "SOCKS5 HTTP traffic protocol mismatch: ${socks_flag}"
        exit 1
    fi
    log_pass "SOCKS5 HTTP traffic record uses socks5-http protocol"
    assert_num_ge "$socks_upload" "1" "SOCKS5 HTTP upload bytes include real client request bytes" || exit 1
    assert_num_ge "$socks_download" "1" "SOCKS5 HTTP download bytes include real upstream response bytes" || exit 1

    log_info "Sending one HTTPS CONNECT tunnel request through HTTP proxy"
    local tunnel_output after_tunnel after_tunnel_requests tunnel_record tunnel_upload tunnel_download tunnel_flag tunnel_protocol
    tunnel_output="$TEST_ROOT/tunnel.out"
    proxy_curl -k "https://127.0.0.1:${HTTPS_PORT}/bytes/64?trusted_metrics_tunnel=1" >"$tunnel_output"
    if [[ "$(wc -c <"$tunnel_output" | tr -d ' ')" -lt 64 ]]; then
        log_fail "CONNECT tunnel response body was smaller than expected"
        exit 1
    fi
    after_tunnel="$(admin_get "/api/metrics")"
    after_tunnel_requests="$(echo "$after_tunnel" | jq -r '.total_requests')"
    assert_num_eq "$((after_tunnel_requests - before_requests))" "10" "HTTPS CONNECT tunnel increments request counter once" || exit 1

    tunnel_record="$(traffic_record_by_protocol_json "tunnel")"
    tunnel_upload="$(echo "$tunnel_record" | jq -r '.upload_bytes // 0')"
    tunnel_download="$(echo "$tunnel_record" | jq -r '.download_bytes // 0')"
    tunnel_flag="$(echo "$tunnel_record" | jq -r '.is_tunnel // false')"
    tunnel_protocol="$(echo "$tunnel_record" | jq -r '.protocol // ""')"
    if [[ "$tunnel_flag" != "true" ]]; then
        log_fail "HTTPS CONNECT traffic record is not marked as is_tunnel=true"
        echo "$tunnel_record" | jq '{url, protocol, is_tunnel, upload_bytes, download_bytes}' >&2 || true
        exit 1
    fi
    log_pass "HTTPS CONNECT traffic record is marked as is_tunnel=true"
    if [[ "$tunnel_protocol" != "tunnel" ]]; then
        log_fail "HTTPS CONNECT traffic protocol mismatch: ${tunnel_protocol}"
        exit 1
    fi
    log_pass "HTTPS CONNECT traffic record uses tunnel protocol"
    assert_num_ge "$tunnel_upload" "1" "HTTPS CONNECT upload bytes include real tunnel client bytes" || exit 1
    assert_num_ge "$tunnel_download" "1" "HTTPS CONNECT download bytes include real tunnel server bytes" || exit 1

    log_pass "trustworthy traffic metrics E2E passed"
}

main "$@"
