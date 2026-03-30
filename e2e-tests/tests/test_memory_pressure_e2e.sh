#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$E2E_DIR/.." && pwd)"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

source "$E2E_DIR/test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-8080}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}"
REQUESTS="${REQUESTS:-10000}"
CONCURRENCY="${CONCURRENCY:-20}"
WS_CONCURRENCY="${WS_CONCURRENCY:-10}"
REQ_SIZE="${REQ_SIZE:-262144}"
RES_SIZE="${RES_SIZE:-524288}"
TIMEOUT="${TIMEOUT:-60}"
START_TIMEOUT="${START_TIMEOUT:-600}"
RUN_HTTP="${RUN_HTTP:-true}"
RUN_STREAMING="${RUN_STREAMING:-true}"
SSE_CONNECTIONS="${SSE_CONNECTIONS:-100}"
SSE_EVENTS="${SSE_EVENTS:-1000}"
WS_CONNECTIONS="${WS_CONNECTIONS:-100}"
WS_MESSAGES="${WS_MESSAGES:-1000}"

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-memory-pressure"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
PAYLOAD_FILE="$TEST_DATA_DIR/payload.bin"
PROXY_PID=""

log() { echo "[$(date +%H:%M:%S)] $*"; }

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"
    safe_cleanup_proxy "$PROXY_PID"
    "$E2E_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
}

trap cleanup EXIT

check_dependencies() {
    local missing=()
    command -v curl >/dev/null 2>&1 || missing+=("curl")
    command -v python3 >/dev/null 2>&1 || missing+=("python3")
    command -v nc >/dev/null 2>&1 || missing+=("nc")
    if [[ ${#missing[@]} -gt 0 ]]; then
        log "缺少依赖: ${missing[*]}"
        exit 1
    fi
}

start_mock_servers() {
    mkdir -p "$TEST_DATA_DIR"
    "$E2E_DIR/mock_servers/start_servers.sh" start > "$MOCK_LOG_FILE" 2>&1 &
    local count=0
    while ! nc -z 127.0.0.1 "$ECHO_HTTP_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge 30 ]]; then
            log "Mock 服务器启动超时"
            cat "$MOCK_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
}

start_proxy() {
    mkdir -p "$TEST_DATA_DIR"
    cat > "$TEST_DATA_DIR/config.toml" <<EOF
[traffic]
max_body_memory_size = 0
max_body_buffer_size = 10485760
max_records = 2000
EOF
    local rules_file="$E2E_DIR/test_data/memory_pressure_load_rules.txt"
    RUST_LOG=info,bifrost_proxy=info \
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        start \
        --intercept \
        --unsafe-ssl \
        --rules-file "$rules_file" \
        > "$PROXY_LOG_FILE" 2>&1 &
    PROXY_PID=$!
    local count=0
    while ! nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge "$START_TIMEOUT" ]]; then
            log "代理服务器启动超时"
            cat "$PROXY_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
}

generate_payload() {
    python3 - "$PAYLOAD_FILE" "$REQ_SIZE" <<'PY'
import sys
path = sys.argv[1]
size = int(sys.argv[2])
marker = b"PAYLOAD_MARKER"
padding = size - len(marker) * 2
if padding < 0:
    padding = 0
data = marker + (b"A" * padding) + marker
with open(path, "wb") as f:
    f.write(data)
PY
}

get_rss_kb() {
    local metrics_url="http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/metrics"
    python3 - "$metrics_url" <<'PY'
import json, sys, urllib.request
url = sys.argv[1]
try:
    with urllib.request.urlopen(url, timeout=2) as resp:
        data = json.loads(resp.read().decode("utf-8"))
        print(int(data.get("memory_used", 0)))
except Exception:
    print(0)
PY
}

run_one() {
    local status
    local args=(
        -s
        -o /dev/null
        -w "%{http_code}"
        --max-time "$TIMEOUT"
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}"
        -X POST
        --data-binary "@${PAYLOAD_FILE}"
    )
    if [[ "$RUN_HTTPS" == "true" ]]; then
        args+=(-k)
    fi
    if [[ -n "${RUN_RESOLVE}" ]]; then
        args+=(--resolve "${RUN_RESOLVE}")
    fi
    args+=("${RUN_URL}")
    status=$(curl "${args[@]}")
    echo "$status"
}

run_load() {
    local name="$1"
    local url="$2"
    local https_flag="$3"
    local resolve_arg="$4"
    local result_file="$TEST_DATA_DIR/result_${name}.txt"

    RUN_URL="$url"
    RUN_HTTPS="$https_flag"
    RUN_RESOLVE="$resolve_arg"
    export RUN_URL RUN_HTTPS RUN_RESOLVE PROXY_HOST PROXY_PORT TIMEOUT PAYLOAD_FILE
    export -f run_one

    : > "$result_file"
    local rss_before
    local rss_after
    rss_before=$(get_rss_kb)
    local start_ts
    start_ts=$(date +%s)
    seq 1 "$REQUESTS" | xargs -P "$CONCURRENCY" -I{} bash -c 'run_one' >> "$result_file"
    local end_ts
    end_ts=$(date +%s)
    rss_after=$(get_rss_kb)

    local ok_count
    ok_count=$(grep -c '^200$' "$result_file" || true)
    local fail_count=$((REQUESTS - ok_count))
    local elapsed=$((end_ts - start_ts))
    local rss_before_mb=$((rss_before / 1024 / 1024))
    local rss_after_mb=$((rss_after / 1024 / 1024))

    log "场景: $name"
    log "请求数: $REQUESTS 并发: $CONCURRENCY"
    log "成功: $ok_count 失败: $fail_count 耗时: ${elapsed}s"
    log "RSS: ${rss_before_mb}MB -> ${rss_after_mb}MB (raw=${rss_before}->${rss_after})"
}

run_sse_load() {
    local result_file="$TEST_DATA_DIR/result_sse.txt"
    local sse_url="http://stress-sse.local/sse?count=${SSE_EVENTS}&interval=0.01"
    : > "$result_file"
    local rss_before
    local rss_after
    rss_before=$(get_rss_kb)
    local start_ts
    start_ts=$(date +%s)
    seq 1 "$SSE_CONNECTIONS" | xargs -P "$CONCURRENCY" -I{} bash -c \
        "curl -s --proxy http://${PROXY_HOST}:${PROXY_PORT} '$sse_url' >/dev/null && echo 200 || echo 500" \
        >> "$result_file"
    local end_ts
    end_ts=$(date +%s)
    rss_after=$(get_rss_kb)
    local ok_count
    ok_count=$(grep -c '^200$' "$result_file" || true)
    local fail_count=$((SSE_CONNECTIONS - ok_count))
    local elapsed=$((end_ts - start_ts))
    local rss_before_mb=$((rss_before / 1024 / 1024))
    local rss_after_mb=$((rss_after / 1024 / 1024))
    log "场景: sse"
    log "连接数: $SSE_CONNECTIONS 事件数/连接: $SSE_EVENTS"
    log "成功: $ok_count 失败: $fail_count 耗时: ${elapsed}s"
    log "RSS: ${rss_before_mb}MB -> ${rss_after_mb}MB (raw=${rss_before}->${rss_after})"
}

run_ws_load() {
    local result_file="$TEST_DATA_DIR/result_ws.txt"
    : > "$result_file"
    local rss_before
    local rss_after
    rss_before=$(get_rss_kb)
    local start_ts
    start_ts=$(date +%s)
    seq 1 "$WS_CONNECTIONS" | xargs -P "$WS_CONCURRENCY" -I{} bash -c \
        "python3 '$E2E_DIR/test_utils/ws_stress_client.py' --proxy-host '${PROXY_HOST}' --proxy-port '${PROXY_PORT}' --host-header 'stress-ws.local' --path '/ws' --messages '${WS_MESSAGES}' --timeout '${TIMEOUT}' >/dev/null 2>&1 && echo 200 || echo 500" \
        >> "$result_file"
    local end_ts
    end_ts=$(date +%s)
    rss_after=$(get_rss_kb)
    local ok_count
    ok_count=$(grep -c '^200$' "$result_file" || true)
    local fail_count=$((WS_CONNECTIONS - ok_count))
    local elapsed=$((end_ts - start_ts))
    local rss_before_mb=$((rss_before / 1024 / 1024))
    local rss_after_mb=$((rss_after / 1024 / 1024))
    log "场景: websocket"
    log "连接数: $WS_CONNECTIONS 消息数/连接: $WS_MESSAGES"
    log "成功: $ok_count 失败: $fail_count 耗时: ${elapsed}s"
    log "RSS: ${rss_before_mb}MB -> ${rss_after_mb}MB (raw=${rss_before}->${rss_after})"
}

main() {
    check_dependencies
    start_mock_servers
    start_proxy
    generate_payload

    if [[ "$RUN_HTTP" == "true" ]]; then
        run_load \
            "http" \
            "http://stress-http.local/large-response?size=${RES_SIZE}&marker=HTTP" \
            "false" \
            ""
    fi

    run_load \
        "tls_intercept" \
        "https://stress-https-intercept.local/large-response?size=${RES_SIZE}&marker=MITM" \
        "true" \
        ""

    run_load \
        "tls_passthrough" \
        "https://stress-https-passthrough.local/large-response?size=${RES_SIZE}&marker=PASS" \
        "true" \
        ""

    if [[ "$RUN_STREAMING" == "true" ]]; then
        run_sse_load
        run_ws_load
    fi
}

main "$@"
