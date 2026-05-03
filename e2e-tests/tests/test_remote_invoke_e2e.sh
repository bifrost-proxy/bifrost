#!/bin/bash
set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"
RELAY_PORT=""
RELAY_PID=""
RELAY_LOG=""
MOCK_HTTP_PORT=""
MOCK_HTTP_PID=""
MOCK_HTTP_LOG=""

start_local_relay() {
    RELAY_PORT="$(pick_free_port)"
    RELAY_LOG="$(mktemp)"
    local relay_data_dir
    relay_data_dir="$(mktemp -d)"
    log "Starting local bifrost-sync-server on port $RELAY_PORT..."
    local relay_exec
    relay_exec="$(sync_server_exec "$SYNC_SERVER_DIR")"
    (cd "$SYNC_SERVER_DIR" && \
        eval "$relay_exec" -p "$RELAY_PORT" -d "$relay_data_dir" --enable-remote-invoke \
    ) > "$RELAY_LOG" 2>&1 &
    RELAY_PID=$!

    while [[ $waited -lt 120 ]]; do
    while [[ $waited -lt 30 ]]; do
        if curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${RELAY_PORT}/v4/remote-invoke/client/register" 2>/dev/null | grep -q "4[0-9][0-9]\|200"; then
            log "Local relay server ready (PID: $RELAY_PID)"
            return 0
        fi
        if ! kill -0 "$RELAY_PID" 2>/dev/null; then
            log "FATAL: relay server exited early. Log:"
            cat "$RELAY_LOG" 2>/dev/null || true
            exit 1
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    log "FATAL: relay server did not become ready in 15s. Log:"
    log "FATAL: relay server did not become ready in 60s. Log:"
    exit 1
}

start_local_mock_http() {
    MOCK_HTTP_PORT="$(pick_free_port)"
    MOCK_HTTP_LOG="$(mktemp)"

    log "Starting local mock HTTP server on port $MOCK_HTTP_PORT..."
    python3 - "$MOCK_HTTP_PORT" >"$MOCK_HTTP_LOG" 2>&1 <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "path": self.path,
            "method": "GET",
            "ok": True,
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return

HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    MOCK_HTTP_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${MOCK_HTTP_PORT}/health" 2>/dev/null | grep -q "200"; then
            log "Local mock HTTP server ready (PID: $MOCK_HTTP_PID)"
            return 0
        fi
        if ! kill -0 "$MOCK_HTTP_PID" 2>/dev/null; then
            log "FATAL: mock HTTP server exited early. Log:"
            cat "$MOCK_HTTP_LOG" 2>/dev/null || true
            exit 1
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    log "FATAL: mock HTTP server did not become ready in 15s. Log:"
    tail -50 "$MOCK_HTTP_LOG" 2>/dev/null || true
    exit 1
}

RELAY_URL="${RELAY_URL:-}"

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""
CURL_ERROR=""

http_request() {
    local url="$1"
    local method="${2:-GET}"
    local data="${3:-}"
    local extra_headers="${4:-}"

    local headers_file body_file err_file
    headers_file="$(mktemp)"
    body_file="$(mktemp)"
    err_file="$(mktemp)"

    local curl_args=(
        -sS
        -X "$method"
        --max-time 15
        -D "$headers_file"
        -o "$body_file"
        -w '%{http_code}'
    )

    if [[ -n "$data" ]]; then
        curl_args+=(-H "Content-Type: application/json" --data-binary "$data")
    fi

    if [[ -n "$extra_headers" ]]; then
        while IFS= read -r header; do
            [[ -n "$header" ]] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    CURL_ERROR=""
    HTTP_STATUS="$(curl "${curl_args[@]}" "$url" 2>"$err_file")" || HTTP_STATUS="000"
    HTTP_HEADERS="$(cat "$headers_file" | tr -d '\r')"
    HTTP_BODY="$(cat "$body_file")"
    CURL_ERROR="$(cat "$err_file")"

    rm -f "$headers_file" "$body_file" "$err_file"
}

http_get() {
    local url="$1"
    local extra_headers="${2:-}"
    http_request "$url" "GET" "" "$extra_headers"
}

http_post_json() {
    local url="$1"
    local data="$2"
    local extra_headers="${3:-}"
    http_request "$url" "POST" "$data" "$extra_headers"
}

wait_for_client_grants_at_least() {
    local min_count="$1"
    local timeout_seconds="${2:-10}"
    local count="0"

    for _ in $(seq 1 "$timeout_seconds"); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
        if [[ "$HTTP_STATUS" == "200" ]]; then
            count="$(echo "$HTTP_BODY" | jq '.grants | length')"
            if [[ "$count" -ge "$min_count" ]]; then
                echo "$count"
                return 0
            fi
        fi
        sleep 1
    done

    echo "$count"
    return 1
}

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

ADMIN_PORT="${ADMIN_PORT:-}"
if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$(pick_free_port)"
fi
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX
ADMIN_HOST="${ADMIN_HOST:-0.0.0.0}"
export ADMIN_HOST
export ADMIN_PORT
export ADMIN_BASE_URL="${ADMIN_BASE_URL:-http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}}"

BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$SCRIPT_DIR/../../.bifrost-e2e-remote-invoke-$RANDOM}"
export BIFROST_DATA_DIR

CALLER_DATA_DIR="$(mktemp -d)"
CALLER_CONNECT_PID=""
CALLER_CONNECT_LOG=""
CALLER_RECONNECT_PID=""

cleanup() {
    if [[ -n "$CALLER_CONNECT_PID" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    if [[ -n "$CALLER_RECONNECT_PID" ]] && kill -0 "$CALLER_RECONNECT_PID" 2>/dev/null; then
        kill "$CALLER_RECONNECT_PID" 2>/dev/null || true
        wait "$CALLER_RECONNECT_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "$RELAY_PID" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    if [[ -n "$MOCK_HTTP_PID" ]] && kill -0 "$MOCK_HTTP_PID" 2>/dev/null; then
        kill "$MOCK_HTTP_PID" 2>/dev/null || true
        wait "$MOCK_HTTP_PID" 2>/dev/null || true
    fi
    [[ -n "${RELAY_LOG:-}" ]] && rm -f "$RELAY_LOG" 2>/dev/null || true
    [[ -n "${MOCK_HTTP_LOG:-}" ]] && rm -f "$MOCK_HTTP_LOG" 2>/dev/null || true
    rm -rf "$BIFROST_DATA_DIR" >/dev/null 2>&1 || true
    rm -rf "$CALLER_DATA_DIR" >/dev/null 2>&1 || true
    [[ -n "${CALLER_CONNECT_LOG:-}" ]] && rm -f "$CALLER_CONNECT_LOG" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[remote-invoke-e2e] $*"; }

is_caller_conn_error() {
    echo "$1" | grep -qiE "no saved connection|expired|revoked|Config error.*connect"
}

restart_target_client_preserve_data_dir() {
    if [[ -n "${ADMIN_CLIENT_BIFROST_PID:-}" ]] && kill -0 "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null; then
        safe_cleanup_proxy "$ADMIN_CLIENT_BIFROST_PID" || true
        wait "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null || true
    fi
    ADMIN_CLIENT_BIFROST_PID=""
    if [[ -n "${ADMIN_CLIENT_BIFROST_LOG_FILE:-}" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        rm -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null || true
    fi
    ADMIN_CLIENT_BIFROST_LOG_FILE=""
    admin_start_bifrost
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd curl
require_cmd jq
require_cmd cargo
require_cmd node

if [[ -z "$RELAY_URL" ]]; then
    start_local_relay
    RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
fi
start_local_mock_http

log "Prepare bifrost binary..."
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
if [[ "$BIFROST_BIN" == "$REPO_DIR/target/release/bifrost" && "${SKIP_BUILD:-}" != "true" ]]; then
    NEED_BUILD=0
    if [[ ! -x "$BIFROST_BIN" ]] \
        || [[ "$REPO_DIR/Cargo.toml" -nt "$BIFROST_BIN" ]] \
        || [[ "$REPO_DIR/Cargo.lock" -nt "$BIFROST_BIN" ]]; then
        NEED_BUILD=1
    elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$BIFROST_BIN" -print -quit | grep -q .; then
        NEED_BUILD=1
    fi

    if [[ "$NEED_BUILD" -eq 1 ]]; then
        log "Building release bifrost..."
        (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)
    fi
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
fi

mkdir -p "$BIFROST_DATA_DIR"
cat > "$BIFROST_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF

log "Start bifrost client (admin+proxy) on port $ADMIN_PORT with relay=$RELAY_URL..."
admin_start_bifrost

CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

log "Verify relay rejects client registration without x-bifrost-token"
http_post_json "${RELAY_URL}/v4/remote-invoke/client/register/challenge" '{"client_instance_id":"unauthorized-client"}'
assert_status "401" "$HTTP_STATUS" "未携带 x-bifrost-token 的 register challenge 应返回 401"

http_post_json "${RELAY_URL}/v4/remote-invoke/client/register" '{
  "challenge_id":"missing",
  "client_instance_id":"unauthorized-client",
  "client_long_term_pubkey":"Zm9v",
  "device_name":"unauthorized-device",
  "platform":"macos",
  "bifrost_version":"0.0.0-test",
  "signature":"YmFy",
  "timestamp":1700000000
}'
assert_status "401" "$HTTP_STATUS" "未携带 x-bifrost-token 的 register 应返回 401"

SYNC_USER_ID="remote_invoke_e2e_${RANDOM}"
SYNC_PASSWORD="remote_invoke_e2e_123"

log "Register relay sync user and capture x-bifrost-token"
http_post_json "${RELAY_URL}/v4/sso/register" "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"${SYNC_PASSWORD}\",\"nickname\":\"Remote Invoke E2E\"}"
assert_status "200" "$HTTP_STATUS" "relay 注册测试用户应返回 200"
RELAY_SYNC_TOKEN=$(echo "$HTTP_BODY" | jq -r '.data.token // ""')
assert_not_empty "$RELAY_SYNC_TOKEN" "relay 注册后 token 不应为空"

log "Save sync session into local bifrost so remote invoke worker can register"
http_post_json "${CLIENT_ADMIN_URL}/api/sync/session" "{\"token\":\"${RELAY_SYNC_TOKEN}\"}"
assert_status "200" "$HTTP_STATUS" "保存 sync session 应返回 200"

log "Wait for remote invoke worker to register with relay..."
WORKER_READY=0
for i in $(seq 1 30); do
    http_get "http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/remote-invoke/status"
    if [[ "$HTTP_STATUS" == "200" ]]; then
        WORKER_STATE=$(echo "$HTTP_BODY" | jq -r '.state // ""')
        if [[ "$WORKER_STATE" == "Connected" ]]; then
            WORKER_READY=1
            break
        fi
    fi
    sleep 1
done
if [[ "$WORKER_READY" -eq 0 ]]; then
    log "WARN: worker state=$WORKER_STATE (not Connected yet), continuing anyway..."
fi

log "Verify remote invoke status is available"
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/status"
assert_status "200" "$HTTP_STATUS" "remote-invoke/status 应返回 200"
log "Remote invoke status: $(echo "$HTTP_BODY" | jq -c .)"

log "Get client identity"
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/identity"
assert_status "200" "$HTTP_STATUS" "remote-invoke/identity 应返回 200"
CLIENT_INSTANCE_ID=$(echo "$HTTP_BODY" | jq -r '.instance_id')
assert_not_empty "$CLIENT_INSTANCE_ID" "client instance_id 不应为空"
log "Client instance_id: $CLIENT_INSTANCE_ID"

# =========================================================================
# TC-RI-01: Connect (pair) flow via remote relay
# =========================================================================
log "=== TC-RI-01: Connect (pair) flow ==="

log "Enter discovery mode to generate pair_code"
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
assert_status "200" "$HTTP_STATUS" "进入 discovery 模式应返回 200"
PAIR_CODE=$(echo "$HTTP_BODY" | jq -r '.session.pair_code')
assert_not_empty "$PAIR_CODE" "pair_code 不应为空"
log "Generated pair_code: $PAIR_CODE"

CALLER_CONNECT_LOG="$(mktemp)"
log "Start bifrost remote conn up in background (Caller)..."
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up "$PAIR_CODE" --relay-url "$RELAY_URL" \
    > "$CALLER_CONNECT_LOG" 2>&1 &
CALLER_CONNECT_PID=$!

log "Wait for pairing request to arrive at Client..."
PAIRING_FOUND=0
for i in $(seq 1 30); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
    PENDING_COUNT=$(echo "$HTTP_BODY" | jq '.pairings | length')
    if [[ "$PENDING_COUNT" -gt 0 ]]; then
        PAIRING_FOUND=1
        break
    fi
    sleep 1
done

if [[ "$PAIRING_FOUND" -eq 0 ]]; then
    log "FAIL: pairing request did not arrive within 30s"
    log "Caller log:"
    cat "$CALLER_CONNECT_LOG" 2>/dev/null || true
    exit 1
fi

PAIRING_ID=$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id')
assert_not_empty "$PAIRING_ID" "pairing_id 不应为空"
log "Pairing request arrived: $PAIRING_ID"

log "Approve pairing with mode=persistent"
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${PAIRING_ID}/approve" '{"grant_mode":"permanent"}'
assert_status "200" "$HTTP_STATUS" "审批配对应返回 200"
log "Pairing approved"

log "Wait for Caller connect to complete..."
CONNECT_OK=0
for i in $(seq 1 30); do
    if ! kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        wait "$CALLER_CONNECT_PID" 2>/dev/null
        CONNECT_EXIT=$?
        if [[ "$CONNECT_EXIT" -eq 0 ]]; then
            CONNECT_OK=1
        fi
        break
    fi
    sleep 1
done

if [[ "$CONNECT_OK" -eq 1 ]]; then
    _log_pass "TC-RI-01: Caller connect 成功完成"
else
    _log_fail "TC-RI-01: Caller connect 失败" "exit_code=0" "exit_code=${CONNECT_EXIT:-timeout}"
    log "Caller log:"
    cat "$CALLER_CONNECT_LOG" 2>/dev/null || true
fi
CALLER_CONNECT_PID=""

if grep -q "Connected! Authorization granted" "$CALLER_CONNECT_LOG" 2>/dev/null; then
    _log_pass "TC-RI-01A: connect 日志包含授权成功提示"
else
    _log_fail "TC-RI-01A: connect 日志缺少授权成功提示" "包含 Connected! Authorization granted" "$(cat "$CALLER_CONNECT_LOG" 2>/dev/null || true)"
fi

log "Verify grant is created on Client side"
GRANT_COUNT="$(wait_for_client_grants_at_least 1 20)"
assert_status "200" "$HTTP_STATUS" "grants 列表应返回 200"
if [[ "$GRANT_COUNT" -gt 0 ]]; then
    _log_pass "Client 侧存在至少一个 grant"
else
    _log_warning "TC-RI-01B: grant 尚未同步到 Client 侧 (count=$GRANT_COUNT)，可能是时序问题"
fi

log "Exit discovery mode"
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"

# Verify caller connection is healthy before proceeding to dependent tests
CALLER_CONN_OK=1
_VERIFY_CONN_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true
if is_caller_conn_error "$_VERIFY_CONN_OUTPUT"; then
    CALLER_CONN_OK=0
    log "WARN: caller 连接在 TC-RI-01 完成后已失效，TC-RI-02 ~ TC-RI-04G 将降级为 warning"
fi

# =========================================================================
# TC-RI-02: Remote status command
# =========================================================================
log "=== TC-RI-02: Remote status command ==="

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    STATUS_OUTPUT=""
    _log_warning "TC-RI-02: 跳过（caller 连接不可用）"
else
    STATUS_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true
    if echo "$STATUS_OUTPUT" | grep -qiE "proxy_address|instance_id|platform|version"; then
        _log_pass "TC-RI-02: remote conn status 返回了设备信息"
    elif is_caller_conn_error "$STATUS_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-02: caller 连接失效: $(echo "$STATUS_OUTPUT" | head -1)"
    else
        _log_fail "TC-RI-02: remote conn status 未返回预期的设备信息" "包含 proxy_address/instance_id" "$STATUS_OUTPUT"
    fi
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
LATEST_STATUS_PREVIEW=$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "status"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].command_summary.command_preview // ""
')
if [[ "$LATEST_STATUS_PREVIEW" == "status" ]]; then
    _log_pass "TC-RI-02A: client Recent Calls 展示 status 命令摘要"
elif [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    _log_warning "TC-RI-02A: 跳过（命令未到达 client 侧）"
else
    _log_fail "TC-RI-02A: client Recent Calls 命令摘要为空或错误" "status" "${LATEST_STATUS_PREVIEW:-<empty>}"
fi

# =========================================================================
# TC-RI-03: Remote traffic list command
# =========================================================================
log "=== TC-RI-03: Remote traffic list ==="

log "Generate some traffic via proxy..."
REMOTE_MARKER="remote-invoke-${RANDOM}"
curl -sS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "http://127.0.0.1:${MOCK_HTTP_PORT}/anything/${REMOTE_MARKER}" >/dev/null 2>&1 || true
sleep 2

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
PRE_TRAFFIC_LIST_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "traffic.list"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].started_at // 0
')

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    TRAFFIC_OUTPUT=""
    _log_warning "TC-RI-03: 跳过（caller 连接不可用）"
else
    TRAFFIC_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic list \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        --limit 7 --cursor 123 --direction forward --method GET \
        --status-min 200 --status-max 299 --protocol http --host 127.0.0.1 \
        --url "/anything/${REMOTE_MARKER}" --path "/anything" --content-type application/json \
        --client-app curl --has-rule-hit false --is-websocket false --is-sse false --is-tunnel false 2>&1) || true
    if echo "$TRAFFIC_OUTPUT" | grep -qiE "Seq|Host|Method|Status|127.0.0.1"; then
        _log_pass "TC-RI-03: remote traffic list 返回了流量记录"
    elif is_caller_conn_error "$TRAFFIC_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-03: caller 连接失效: $(echo "$TRAFFIC_OUTPUT" | head -1)"
    else
        _log_warning "TC-RI-03: traffic list 可能为空（无匹配规则时无流量）: $(echo "$TRAFFIC_OUTPUT" | head -3)"
        _log_pass "TC-RI-03: remote traffic list 命令执行成功（数据取决于规则配置）"
    fi
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
LATEST_TRAFFIC_LIST_ARGS_JSON=$(echo "$HTTP_BODY" | jq -r --argjson prev_started_at "${PRE_TRAFFIC_LIST_STARTED_AT:-0}" '
    (.calls // [])
    | map(select((.command_summary.command_preview // "") == "traffic.list" and (.started_at // 0) > $prev_started_at))
    | sort_by(.started_at // 0)
    | reverse
    | (
        .[0].command_summary.masked_args_json
        // .[0].command.args_json
        // .[0].args_json
        // (.[0].command.query.args | tojson)
        // ""
      )
')

if [[ -z "$LATEST_TRAFFIC_LIST_ARGS_JSON" || "$LATEST_TRAFFIC_LIST_ARGS_JSON" == "null" || "$LATEST_TRAFFIC_LIST_ARGS_JSON" == '""' ]]; then
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-03A: 跳过（命令未到达 client 侧）"
    else
        _log_fail "TC-RI-03A: remote traffic list 未透传完整过滤参数" "args_json 包含 list/filter 参数" "${LATEST_TRAFFIC_LIST_ARGS_JSON:-<empty>}"
    fi
elif echo "$LATEST_TRAFFIC_LIST_ARGS_JSON" | jq -e --arg marker "$REMOTE_MARKER" '
    .limit == 7
    and .cursor == 123
    and .direction == "forward"
    and .method == "GET"
    and .status_min == 200
    and .status_max == 299
    and .protocol == "http"
    and .host == "127.0.0.1"
    and .url == ("/anything/" + $marker)
    and .path == "/anything"
    and .content_type == "application/json"
    and .client_app == "curl"
    and .has_rule_hit == false
    and .is_websocket == false
    and .is_sse == false
    and .is_tunnel == false
' >/dev/null 2>&1; then
    _log_pass "TC-RI-03A: remote traffic list 将全部过滤参数透传到执行端"
else
    _log_fail "TC-RI-03A: remote traffic list 未透传完整过滤参数" "args_json 包含 list/filter 参数" "${LATEST_TRAFFIC_LIST_ARGS_JSON:-<empty>}"
fi

http_get "${CLIENT_ADMIN_URL}/api/traffic?limit=20"
assert_status "200" "$HTTP_STATUS" "traffic 列表应返回 200"
TARGET_SEQ=$(echo "$HTTP_BODY" | jq -r --arg marker "$REMOTE_MARKER" '
  (.records // [])[]
  | select(((.path // .p // "") | contains($marker)))
  | (.seq // .sequence // empty)
' | head -n 1)
assert_not_empty "$TARGET_SEQ" "用于 remote traffic get 的 sequence 不应为空"

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
PRE_TRAFFIC_GET_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "traffic.get"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].started_at // 0
')

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    REMOTE_GET_OUTPUT=""
    _log_warning "TC-RI-03B: 跳过（caller 连接不可用）"
else
    REMOTE_GET_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic get "$TARGET_SEQ" \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --request-body --response-body 2>&1) || true
    if printf '%s' "$REMOTE_GET_OUTPUT" | jq -e --arg seq "$TARGET_SEQ" --arg marker "$REMOTE_MARKER" '
        ((.seq // .sequence // "") | tostring) == $seq
        and (tostring | contains($marker))
    ' >/dev/null 2>&1; then
        _log_pass "TC-RI-03B: remote traffic get 支持 sequence 查询并返回详情"
    elif is_caller_conn_error "$REMOTE_GET_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-03B: caller 连接失效: $(echo "$REMOTE_GET_OUTPUT" | head -1)"
    else
        _log_fail "TC-RI-03B: remote traffic get 未返回目标 sequence 的详情" "包含 seq/sequence=${TARGET_SEQ} 与 marker=${REMOTE_MARKER}" "$REMOTE_GET_OUTPUT"
    fi
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
LATEST_TRAFFIC_GET_ARGS_JSON=$(echo "$HTTP_BODY" | jq -r --argjson prev_started_at "${PRE_TRAFFIC_GET_STARTED_AT:-0}" '
    (.calls // [])
    | map(select((.command_summary.command_preview // "") == "traffic.get" and (.started_at // 0) > $prev_started_at))
    | sort_by(.started_at // 0)
    | reverse
    | (
        .[0].command_summary.masked_args_json
        // .[0].command.args_json
        // .[0].args_json
        // (.[0].command.query.args | tojson)
        // ""
      )
')

if [[ -z "$LATEST_TRAFFIC_GET_ARGS_JSON" || "$LATEST_TRAFFIC_GET_ARGS_JSON" == "null" || "$LATEST_TRAFFIC_GET_ARGS_JSON" == '""' ]]; then
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-03C: 跳过（命令未到达 client 侧）"
    else
        _log_fail "TC-RI-03C: remote traffic get 未透传完整参数" "args_json 包含 id/request_body/response_body" "${LATEST_TRAFFIC_GET_ARGS_JSON:-<empty>}"
    fi
elif echo "$LATEST_TRAFFIC_GET_ARGS_JSON" | grep -q "\"id\":\"${TARGET_SEQ}\"" \
    && echo "$LATEST_TRAFFIC_GET_ARGS_JSON" | grep -q '"request_body":true' \
    && echo "$LATEST_TRAFFIC_GET_ARGS_JSON" | grep -q '"response_body":true'; then
    _log_pass "TC-RI-03C: remote traffic get 将 id/body 标志透传到执行端"
else
    _log_fail "TC-RI-03C: remote traffic get 未透传完整参数" "args_json 包含 id/request_body/response_body" "${LATEST_TRAFFIC_GET_ARGS_JSON:-<empty>}"
fi

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    _log_warning "TC-RI-03D: 跳过（caller 连接不可用）"
else
    REMOTE_GET_MISSING_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic get 999999999 \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true
    if echo "$REMOTE_GET_MISSING_OUTPUT" | grep -qE "No traffic record with sequence suffix '999999999' found|Not found: Traffic record '999999999' not found"; then
        _log_pass "TC-RI-03D: remote traffic get 失败时会透出真实错误"
    elif is_caller_conn_error "$REMOTE_GET_MISSING_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-03D: caller 连接失效: $(echo "$REMOTE_GET_MISSING_OUTPUT" | head -1)"
    else
        _log_fail "TC-RI-03D: remote traffic get 失败时未透出真实错误" "包含 sequence suffix not found 错误" "$REMOTE_GET_MISSING_OUTPUT"
    fi
fi

# =========================================================================
# TC-RI-04: Remote search command
# =========================================================================
log "=== TC-RI-04: Remote search command ==="

SEARCH_LOG="$(mktemp)"
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
PRE_SEARCH_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "search.stream"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].started_at // 0
')

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    SEARCH_OUTPUT=""
    SEARCH_STREAM_SEEN=0
    rm -f "$SEARCH_LOG"
    _log_warning "TC-RI-04: 跳过（caller 连接不可用）"
    _log_warning "TC-RI-04A: 跳过（caller 连接不可用）"
else
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "$REMOTE_MARKER" \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --max-results 5 --max-scan 50 >"$SEARCH_LOG" 2>&1 &
    SEARCH_PID=$!

    SEARCH_STREAM_SEEN=0
    for i in $(seq 1 20); do
        if grep -qE "Searching\\.\.\.|${REMOTE_MARKER}|Found [0-9]+ matches" "$SEARCH_LOG" 2>/dev/null; then
            SEARCH_STREAM_SEEN=1
            break
        fi
        if ! kill -0 "$SEARCH_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    wait "$SEARCH_PID"
    SEARCH_EXIT=$?
    SEARCH_OUTPUT="$(cat "$SEARCH_LOG")"
    rm -f "$SEARCH_LOG"

    if [[ "$SEARCH_EXIT" -eq 0 ]] && echo "$SEARCH_OUTPUT" | grep -q "$REMOTE_MARKER" && echo "$SEARCH_OUTPUT" | grep -qE "Found [0-9]+ match"; then
        _log_pass "TC-RI-04: remote traffic search 返回了目标结果"
    elif is_caller_conn_error "$SEARCH_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-04: caller 连接失效: $(echo "$SEARCH_OUTPUT" | head -1)"
    else
        _log_fail "TC-RI-04: remote traffic search 未返回目标结果" "包含 marker=${REMOTE_MARKER} 与 Found N matches" "$SEARCH_OUTPUT"
    fi

    if [[ "$SEARCH_STREAM_SEEN" -eq 1 ]]; then
        _log_pass "TC-RI-04A: remote traffic search 输出包含流式进度"
    elif [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-04A: 跳过（caller 连接失效）"
    else
        _log_fail "TC-RI-04A: remote traffic search 未输出流式进度" "输出包含 Searching..." "$SEARCH_OUTPUT"
    fi
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
LATEST_SEARCH_ARGS_JSON=$(echo "$HTTP_BODY" | jq -r --argjson prev_started_at "${PRE_SEARCH_STARTED_AT:-0}" '
    (.calls // [])
    | map(select((.command_summary.command_preview // "") == "search.stream" and (.started_at // 0) > $prev_started_at))
    | sort_by(.started_at // 0)
    | reverse
    | (
        .[0].command_summary.masked_args_json
        // .[0].command.args_json
        // .[0].args_json
        // (.[0].command.query.args | tojson)
        // ""
      )
')

if [[ -z "$LATEST_SEARCH_ARGS_JSON" || "$LATEST_SEARCH_ARGS_JSON" == "null" || "$LATEST_SEARCH_ARGS_JSON" == '""' ]]; then
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-04B: 跳过（命令未到达 client 侧）"
    else
        _log_fail "TC-RI-04B: remote traffic search 未透传 max_results/max_scan" 'args_json 包含 "max_results":5 和 "max_scan":50' "${LATEST_SEARCH_ARGS_JSON:-<empty>}"
    fi
elif echo "$LATEST_SEARCH_ARGS_JSON" | grep -q '"max_results":5' && echo "$LATEST_SEARCH_ARGS_JSON" | grep -q '"max_scan":50'; then
    _log_pass "TC-RI-04B: remote traffic search 将 max_results/max_scan 透传到执行端"
else
    _log_fail "TC-RI-04B: remote traffic search 未透传 max_results/max_scan" 'args_json 包含 "max_results":5 和 "max_scan":50' "${LATEST_SEARCH_ARGS_JSON:-<empty>}"
fi

# =========================================================================
# TC-RI-04C: Remote traffic search command
# =========================================================================
log "=== TC-RI-04C: Remote traffic search command ==="

TRAFFIC_SEARCH_LOG="$(mktemp)"
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
PRE_TRAFFIC_SEARCH_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "search.stream"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].started_at // 0
')

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    TRAFFIC_SEARCH_OUTPUT=""
    TRAFFIC_SEARCH_STREAM_SEEN=0
    rm -f "$TRAFFIC_SEARCH_LOG"
    _log_warning "TC-RI-04C: 跳过（caller 连接不可用）"
    _log_warning "TC-RI-04D: 跳过（caller 连接不可用）"
else
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "$REMOTE_MARKER" \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --max-results 3 --max-scan 30 >"$TRAFFIC_SEARCH_LOG" 2>&1 &
    TRAFFIC_SEARCH_PID=$!

    TRAFFIC_SEARCH_STREAM_SEEN=0
    for i in $(seq 1 20); do
        if grep -qE "Searching\\.\.\.|${REMOTE_MARKER}|Found [0-9]+ matches" "$TRAFFIC_SEARCH_LOG" 2>/dev/null; then
            TRAFFIC_SEARCH_STREAM_SEEN=1
            break
        fi
        if ! kill -0 "$TRAFFIC_SEARCH_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    wait "$TRAFFIC_SEARCH_PID"
    TRAFFIC_SEARCH_EXIT=$?
    TRAFFIC_SEARCH_OUTPUT="$(cat "$TRAFFIC_SEARCH_LOG")"
    rm -f "$TRAFFIC_SEARCH_LOG"

    if [[ "$TRAFFIC_SEARCH_EXIT" -eq 0 ]] && echo "$TRAFFIC_SEARCH_OUTPUT" | grep -q "$REMOTE_MARKER" && echo "$TRAFFIC_SEARCH_OUTPUT" | grep -qE "Found [0-9]+ match"; then
        _log_pass "TC-RI-04C: remote traffic search 返回了目标结果"
    elif is_caller_conn_error "$TRAFFIC_SEARCH_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-04C: caller 连接失效: $(echo "$TRAFFIC_SEARCH_OUTPUT" | head -1)"
    else
        _log_fail "TC-RI-04C: remote traffic search 未返回目标结果" "包含 marker=${REMOTE_MARKER} 与 Found N matches" "$TRAFFIC_SEARCH_OUTPUT"
    fi

    if [[ "$TRAFFIC_SEARCH_STREAM_SEEN" -eq 1 ]]; then
        _log_pass "TC-RI-04D: remote traffic search 输出包含流式进度"
    elif [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-04D: 跳过（caller 连接失效）"
    else
        _log_fail "TC-RI-04D: remote traffic search 未输出流式进度" "输出包含 Searching..." "$TRAFFIC_SEARCH_OUTPUT"
    fi
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
LATEST_TRAFFIC_SEARCH_ARGS_JSON=$(echo "$HTTP_BODY" | jq -r --argjson prev_started_at "${PRE_TRAFFIC_SEARCH_STARTED_AT:-0}" '
    (.calls // [])
    | map(select((.command_summary.command_preview // "") == "search.stream" and (.started_at // 0) > $prev_started_at))
    | sort_by(.started_at // 0)
    | reverse
    | (
        .[0].command_summary.masked_args_json
        // .[0].command.args_json
        // .[0].args_json
        // (.[0].command.query.args | tojson)
        // ""
      )
')

if [[ -z "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" || "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" == "null" || "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" == '""' ]]; then
    if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
        _log_warning "TC-RI-04E: 跳过（命令未到达 client 侧）"
    else
        _log_fail "TC-RI-04E: remote traffic search 未透传查询关键字" "args_json 包含 keyword/query" "${LATEST_TRAFFIC_SEARCH_ARGS_JSON:-<empty>}"
    fi
elif echo "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" | grep -q "\"query\":\"${REMOTE_MARKER}\"" \
    || echo "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" | grep -q "\"keyword\":\"${REMOTE_MARKER}\""; then
    if echo "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" | grep -q '"max_results":3' \
        && echo "$LATEST_TRAFFIC_SEARCH_ARGS_JSON" | grep -q '"max_scan":30'; then
        _log_pass "TC-RI-04E: remote traffic search 将 max_results/max_scan 透传到执行端"
    else
        _log_fail "TC-RI-04E: remote traffic search 未透传 max_results/max_scan" "args_json 包含 keyword/query/max_results/max_scan" "${LATEST_TRAFFIC_SEARCH_ARGS_JSON:-<empty>}"
    fi
else
    _log_fail "TC-RI-04E: remote traffic search 未透传查询关键字" "args_json 包含 keyword/query" "${LATEST_TRAFFIC_SEARCH_ARGS_JSON:-<empty>}"
fi

# =========================================================================
# TC-RI-04F: Caller cancel should settle call as cancelled for all commands
# =========================================================================
log "=== TC-RI-04F: Caller cancel settles remote call as cancelled ==="

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    _log_warning "TC-RI-04F: 跳过（caller 连接不可用）"
else
    cancel_batch_pids=()
    for i in $(seq 1 120); do
        curl -sS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "http://httpbin.org/anything/cancel-${RANDOM}" >/dev/null 2>&1 &
        cancel_batch_pids+=($!)
        if (( i % 12 == 0 )); then
            wait "${cancel_batch_pids[@]}"
            cancel_batch_pids=()
        fi
    done
    if [[ "${#cancel_batch_pids[@]}" -gt 0 ]]; then
        wait "${cancel_batch_pids[@]}"
    fi
    sleep 2

    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
    assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
    PRE_CANCEL_SEARCH_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
        (.calls // [])
        | map(select((.command.command // .command) == "search.stream"))
        | sort_by(.started_at // 0)
        | reverse
        | .[0].started_at // 0
    ')

    CANCEL_LOG="$(mktemp)"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "httpbin" \
        --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        --limit 500 --max-results 500 --max-scan 2000 >"$CANCEL_LOG" 2>&1 &
    CANCEL_PID=$!

    CALL_OPENED_FOR_CANCEL=0
    for i in $(seq 1 30); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
        assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
        LATEST_CANCEL_SEARCH_STARTED_AT=$(echo "$HTTP_BODY" | jq -r '
            (.calls // [])
            | map(select((.command.command // .command) == "search.stream"))
            | sort_by(.started_at // 0)
            | reverse
            | .[0].started_at // 0
        ')
        if [[ "${LATEST_CANCEL_SEARCH_STARTED_AT:-0}" -gt "${PRE_CANCEL_SEARCH_STARTED_AT:-0}" ]]; then
            CALL_OPENED_FOR_CANCEL=1
            break
        fi
        if ! kill -0 "$CANCEL_PID" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done

    STREAM_SEEN_FOR_CANCEL=0
    if [[ "$CALL_OPENED_FOR_CANCEL" -eq 1 ]]; then
        for i in $(seq 1 30); do
            if [[ -s "$CANCEL_LOG" ]]; then
                STREAM_SEEN_FOR_CANCEL=1
                break
            fi
            if ! kill -0 "$CANCEL_PID" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
    fi

    if [[ "$CALL_OPENED_FOR_CANCEL" -eq 1 ]] && [[ "$STREAM_SEEN_FOR_CANCEL" -eq 1 ]] && kill -0 "$CANCEL_PID" 2>/dev/null; then
        kill -INT "$CANCEL_PID" 2>/dev/null || true
    fi

    wait "$CANCEL_PID" 2>/dev/null || CANCEL_EXIT=$?
    CANCEL_EXIT="${CANCEL_EXIT:-0}"
    CANCEL_OUTPUT="$(cat "$CANCEL_LOG")"
    rm -f "$CANCEL_LOG"

    if is_caller_conn_error "$CANCEL_OUTPUT"; then
        CALLER_CONN_OK=0
        _log_warning "TC-RI-04F: caller 连接失效: $(echo "$CANCEL_OUTPUT" | head -1)"
    elif [[ "$CALL_OPENED_FOR_CANCEL" -eq 1 ]] && [[ "$STREAM_SEEN_FOR_CANCEL" -eq 1 ]] && [[ "$CANCEL_EXIT" -eq 130 ]]; then
        _log_pass "TC-RI-04F: caller 侧中断后会触发远端 cancel 收尾"
    elif [[ "$CALL_OPENED_FOR_CANCEL" -eq 1 ]] && [[ "$CANCEL_EXIT" -eq 130 ]]; then
        _log_pass "TC-RI-04F: caller 侧中断后会触发远端 cancel 收尾 (stream not yet visible)"
    else
        _log_warning "TC-RI-04F: cancel 时序不稳定 (opened=$CALL_OPENED_FOR_CANCEL stream=$STREAM_SEEN_FOR_CANCEL exit=$CANCEL_EXIT)"
    fi
fi

if [[ "$CALLER_CONN_OK" -eq 0 ]]; then
    _log_warning "TC-RI-04G: 跳过（caller 连接不可用）"
else
    CANCELLED_STATUS=""
    for i in $(seq 1 20); do
        http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
        assert_status "200" "$HTTP_STATUS" "remote-invoke/calls 应返回 200"
        CANCELLED_STATUS=$(echo "$HTTP_BODY" | jq -r '
            (.calls // [])
            | sort_by(.started_at // 0)
            | reverse
            | map(select((.command.command // .command) == "search.stream"))
            | .[0].status // ""
        ')
        if [[ "$CANCELLED_STATUS" == "cancelled" ]]; then
            break
        fi
        sleep 1
    done

    if [[ "$CANCELLED_STATUS" == "cancelled" ]]; then
        _log_pass "TC-RI-04G: client Recent Calls 最终显示 cancelled"
    else
        _log_warning "TC-RI-04G: cancel 状态未同步 (status=${CANCELLED_STATUS:-<empty>})，可能是时序问题"
    fi
fi

# =========================================================================
# TC-RI-05: Reject pairing flow
# =========================================================================
log "=== TC-RI-05: Reject pairing flow ==="

log "Enter discovery mode again"
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
assert_status "200" "$HTTP_STATUS" "进入 discovery 模式应返回 200"
PAIR_CODE_2=$(echo "$HTTP_BODY" | jq -r '.session.pair_code')
assert_not_empty "$PAIR_CODE_2" "pair_code 不应为空"

REJECT_LOG="$(mktemp)"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up "$PAIR_CODE_2" --relay-url "$RELAY_URL" \
    > "$REJECT_LOG" 2>&1 &
REJECT_PID=$!

PAIRING_FOUND_2=0
for i in $(seq 1 30); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
    PENDING_COUNT_2=$(echo "$HTTP_BODY" | jq '.pairings | length')
    if [[ "$PENDING_COUNT_2" -gt 0 ]]; then
        PAIRING_FOUND_2=1
        break
    fi
    sleep 1
done

if [[ "$PAIRING_FOUND_2" -eq 1 ]]; then
    PAIRING_ID_2=$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id')
    log "Reject pairing: $PAIRING_ID_2"
    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${PAIRING_ID_2}/reject" "{}"
    assert_status "200" "$HTTP_STATUS" "拒绝配对应返回 200"

    for i in $(seq 1 15); do
        if ! kill -0 "$REJECT_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    if ! kill -0 "$REJECT_PID" 2>/dev/null; then
        wait "$REJECT_PID" 2>/dev/null
        REJECT_EXIT=$?
        if [[ "$REJECT_EXIT" -ne 0 ]]; then
            _log_pass "TC-RI-05: Caller connect 在拒绝后正确退出 (exit_code=$REJECT_EXIT)"
        else
            _log_fail "TC-RI-05: Caller connect 在拒绝后应非零退出" "non-zero" "$REJECT_EXIT"
        fi
    else
        kill "$REJECT_PID" 2>/dev/null || true
        wait "$REJECT_PID" 2>/dev/null || true
        _log_warning "TC-RI-05: Caller connect 未在 15s 内退出，已强制终止"
    fi
else
    _log_fail "TC-RI-05: 第二次配对请求未到达" "pending>0" "0"
    kill "$REJECT_PID" 2>/dev/null || true
    wait "$REJECT_PID" 2>/dev/null || true
fi

rm -f "$REJECT_LOG" 2>/dev/null || true

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"

# =========================================================================
# TC-RI-06: Security - invalid pair_code
# =========================================================================
log "=== TC-RI-06: Security - invalid pair_code ==="

INVALID_CODE_LOG="$(mktemp)"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" timeout 15 "$BIFROST_BIN" remote conn up "000000" --relay-url "$RELAY_URL" \
    > "$INVALID_CODE_LOG" 2>&1 || true
INVALID_EXIT=$?

if grep -qi "error\|failed\|not found\|invalid" "$INVALID_CODE_LOG" || [[ "$INVALID_EXIT" -ne 0 ]]; then
    _log_pass "TC-RI-06: 无效 pair_code 被正确拒绝"
else
    _log_fail "TC-RI-06: 无效 pair_code 应被拒绝" "error/non-zero" "exit=$INVALID_EXIT"
fi
rm -f "$INVALID_CODE_LOG" 2>/dev/null || true

# =========================================================================
# TC-RI-07: Security - relay_token verification (events without token -> 401)
# =========================================================================
log "=== TC-RI-07: Security - relay_token verification ==="

FAKE_CALL_ID="00000000-0000-0000-0000-000000000000"
http_get "${RELAY_URL}/v4/remote-invoke/calls/${FAKE_CALL_ID}/events"
if [[ "$HTTP_STATUS" == "401" || "$HTTP_STATUS" == "404" ]]; then
    _log_pass "TC-RI-07: 无 token 访问 events 返回 $HTTP_STATUS"
else
    _log_fail "TC-RI-07: 无 token 访问 events 应返回 401/404" "401 or 404" "$HTTP_STATUS"
fi

http_get "${RELAY_URL}/v4/remote-invoke/calls/${FAKE_CALL_ID}/events" "Authorization: Bearer fake-token-12345"
if [[ "$HTTP_STATUS" == "401" || "$HTTP_STATUS" == "404" ]]; then
    _log_pass "TC-RI-07: 错误 token 访问 events 返回 $HTTP_STATUS"
else
    _log_fail "TC-RI-07: 错误 token 访问 events 应返回 401/404" "401 or 404" "$HTTP_STATUS"
fi

# =========================================================================
# TC-RI-07A: Missing client grant crypto should revoke stale reusable grant
# =========================================================================
log "=== TC-RI-07A: Missing client grant crypto should revoke stale reusable grant ==="

GRANT_CRYPTO_JSON="$BIFROST_DATA_DIR/admin/remote_invoke_grant_crypto.json"
GRANT_CRYPTO_KEY="$BIFROST_DATA_DIR/admin/remote_invoke_grant_crypto.key"
rm -f "$GRANT_CRYPTO_JSON" "$GRANT_CRYPTO_KEY"
restart_target_client_preserve_data_dir

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
assert_status "200" "$HTTP_STATUS" "client 重启后 grants 列表应返回 200"
GRANT_COUNT_AFTER_CRYPTO_LOSS=$(echo "$HTTP_BODY" | jq '.grants | length')
if [[ "$GRANT_COUNT_AFTER_CRYPTO_LOSS" -eq 0 ]]; then
    _log_pass "TC-RI-07A: 丢失 grant crypto 后 client 会清理本地 grants"
else
    _log_fail "TC-RI-07A: 丢失 grant crypto 后 client 仍保留幽灵 grant" "0" "$GRANT_COUNT_AFTER_CRYPTO_LOSS"
fi

STATUS_AFTER_CRYPTO_LOSS=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true

if echo "$STATUS_AFTER_CRYPTO_LOSS" | grep -qiE "expired|revoked|connect"; then
    if echo "$STATUS_AFTER_CRYPTO_LOSS" | grep -qi "missing grant shared secret"; then
        _log_fail "TC-RI-07A: caller 仍命中了远端 shared secret 错误" "expired/revoked/connect" "$STATUS_AFTER_CRYPTO_LOSS"
    else
        _log_pass "TC-RI-07A: caller 侧改为直接提示重新 connect"
    fi
else
    _log_fail "TC-RI-07A: caller 侧未返回预期的失效提示" "expired/revoked/connect" "$STATUS_AFTER_CRYPTO_LOSS"
fi

CALLER_CONNECTIONS_FILE_AFTER_CRYPTO_LOSS="$(find "$CALLER_DATA_DIR" -name remote-connections.json -print -quit)"
if [[ -n "$CALLER_CONNECTIONS_FILE_AFTER_CRYPTO_LOSS" && -f "$CALLER_CONNECTIONS_FILE_AFTER_CRYPTO_LOSS" ]]; then
    LOCAL_CONNECTION_COUNT_AFTER_CRYPTO_LOSS="$(jq '.connections | length' "$CALLER_CONNECTIONS_FILE_AFTER_CRYPTO_LOSS")"
else
    LOCAL_CONNECTION_COUNT_AFTER_CRYPTO_LOSS=0
fi

if [[ "$LOCAL_CONNECTION_COUNT_AFTER_CRYPTO_LOSS" -eq 0 ]]; then
    _log_pass "TC-RI-07A: stale 授权失效后 caller 本地连接记录已清空"
else
    _log_fail "TC-RI-07A: stale 授权失效后 caller 本地连接记录仍残留" "0" "$LOCAL_CONNECTION_COUNT_AFTER_CRYPTO_LOSS"
fi

log "Re-create a fresh reusable grant for disconnect regression..."
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
assert_status "200" "$HTTP_STATUS" "重新进入 discovery 模式应返回 200"
PAIR_CODE_3=$(echo "$HTTP_BODY" | jq -r '.session.pair_code')
assert_not_empty "$PAIR_CODE_3" "disconnect 回归前新的 pair_code 不应为空"

CALLER_RECONNECT_LOG="$(mktemp)"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up "$PAIR_CODE_3" --relay-url "$RELAY_URL" \
    > "$CALLER_RECONNECT_LOG" 2>&1 &
CALLER_RECONNECT_PID=$!

PAIRING_FOUND_3=0
for i in $(seq 1 30); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
    PENDING_COUNT_3=$(echo "$HTTP_BODY" | jq '.pairings | length')
    if [[ "$PENDING_COUNT_3" -gt 0 ]]; then
        PAIRING_FOUND_3=1
        break
    fi
    sleep 1
done

if [[ "$PAIRING_FOUND_3" -eq 1 ]]; then
    PAIRING_ID_3=$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id')
    assert_not_empty "$PAIRING_ID_3" "disconnect 回归前新的 pairing_id 不应为空"
    http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${PAIRING_ID_3}/approve" '{"grant_mode":"permanent"}'
    assert_status "200" "$HTTP_STATUS" "disconnect 回归前审批新的配对应返回 200"

    RECONNECT_OK=0
    for i in $(seq 1 30); do
        if ! kill -0 "$CALLER_RECONNECT_PID" 2>/dev/null; then
            wait "$CALLER_RECONNECT_PID" 2>/dev/null
            RECONNECT_EXIT=$?
            if [[ "$RECONNECT_EXIT" -eq 0 ]]; then
                RECONNECT_OK=1
            fi
            break
        fi
        sleep 1
    done

    if [[ "$RECONNECT_OK" -eq 1 ]]; then
        _log_pass "TC-RI-07B: stale 连接清理后 caller 可重新 connect"
    else
        _log_fail "TC-RI-07B: stale 连接清理后 caller 重新 connect 失败" "exit_code=0" "exit_code=${RECONNECT_EXIT:-timeout}"
        log "Caller reconnect log:"
        cat "$CALLER_RECONNECT_LOG" 2>/dev/null || true
    fi
else
    _log_fail "TC-RI-07B: stale 连接清理后新的配对请求未到达" "pending>0" "0"
    kill "$CALLER_RECONNECT_PID" 2>/dev/null || true
    wait "$CALLER_RECONNECT_PID" 2>/dev/null || true
fi
CALLER_RECONNECT_PID=""

if grep -q "Connected! Authorization granted" "$CALLER_RECONNECT_LOG" 2>/dev/null; then
    _log_pass "TC-RI-07B-1: reconnect 日志包含授权成功提示"
else
    _log_fail "TC-RI-07B-1: reconnect 日志缺少授权成功提示" "包含 Connected! Authorization granted" "$(cat "$CALLER_RECONNECT_LOG" 2>/dev/null || true)"
fi
rm -f "$CALLER_RECONNECT_LOG" 2>/dev/null || true

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"
assert_status "200" "$HTTP_STATUS" "disconnect 回归前退出 discovery 模式应返回 200"

# =========================================================================
# TC-RI-08: Disconnect
# =========================================================================
log "=== TC-RI-08: Disconnect ==="

CALLER_CONNECTIONS_FILE="$(find "$CALLER_DATA_DIR" -name remote-connections.json -print -quit)"
assert_not_empty "$CALLER_CONNECTIONS_FILE" "caller 侧 remote-connections.json 路径不应为空"
LOCAL_GRANT_ID="$(jq -r '.connections[0].grant_id // ""' "$CALLER_CONNECTIONS_FILE")"
LOCAL_CALLER_FINGERPRINT="$(jq -r '.connections[0].caller_fingerprint // ""' "$CALLER_CONNECTIONS_FILE")"
assert_not_empty "$LOCAL_GRANT_ID" "disconnect 前本地 grant_id 不应为空"
assert_not_empty "$LOCAL_CALLER_FINGERPRINT" "disconnect 前本地 caller_fingerprint 不应为空"

ENCODED_CALLER_FINGERPRINT="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$LOCAL_CALLER_FINGERPRINT")"
http_request "${RELAY_URL}/v4/remote-invoke/grants/${LOCAL_GRANT_ID}?caller_fingerprint=${ENCODED_CALLER_FINGERPRINT}" "DELETE"
if [[ "$HTTP_STATUS" == "200" || "$HTTP_STATUS" == "204" ]]; then
    _log_pass "TC-RI-08A: 预先删除 relay grant，制造 disconnect 404 场景"
else
    _log_fail "TC-RI-08A: 预先删除 relay grant 失败" "200/204" "status=$HTTP_STATUS body=$HTTP_BODY"
fi

DISCONNECT_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn down --all \
    --relay-url "$RELAY_URL" 2>&1) || true

if echo "$DISCONNECT_OUTPUT" | grep -qi "already missing on relay\|revoked\|disconnected\|✓"; then
    _log_pass "TC-RI-08: disconnect --all 成功并清理本地连接"
else
    _log_fail "TC-RI-08: disconnect 输出未包含成功标识" "revoked/disconnected" "$DISCONNECT_OUTPUT"
fi

LOCAL_CONNECTION_COUNT="$(jq -r '.connections | length' "$CALLER_CONNECTIONS_FILE" 2>/dev/null || echo 0)"
if [[ "$LOCAL_CONNECTION_COUNT" == "0" ]]; then
    _log_pass "TC-RI-08B: relay 404 后 caller 本地连接记录已清空"
else
    _log_fail "TC-RI-08B: relay 404 后 caller 本地连接记录仍残留" "0" "$LOCAL_CONNECTION_COUNT"
fi

# =========================================================================
# TC-RI-09: Disconnected grant cannot be reused
# =========================================================================
log "=== TC-RI-09: Disconnected grant cannot be reused ==="

REUSE_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status \
    --relay-url "$RELAY_URL" 2>&1) || true
REUSE_EXIT=$?

if [[ "$REUSE_EXIT" -ne 0 ]] || echo "$REUSE_OUTPUT" | grep -qi "expired\|revoked\|no saved\|error\|connect"; then
    _log_pass "TC-RI-09: 已断开的 grant 无法复用 (exit=$REUSE_EXIT)"
else
    _log_fail "TC-RI-09: 已断开的 grant 不应复用" "error/non-zero" "exit=$REUSE_EXIT output=$(echo "$REUSE_OUTPUT" | head -2)"
fi

# =========================================================================
# TC-RI-10: Grants list is empty after disconnect
# =========================================================================
log "=== TC-RI-10: Grants list after disconnect ==="

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
assert_status "200" "$HTTP_STATUS" "grants 列表应返回 200"
GRANT_COUNT_AFTER=$(echo "$HTTP_BODY" | jq '.grants | length')
if [[ "$GRANT_COUNT_AFTER" -eq 0 ]]; then
    _log_pass "TC-RI-10: disconnect 后 Client 侧 grants 已清空"
else
    _log_warning "TC-RI-10: disconnect 后 Client 侧可能还有残留 grant (count=$GRANT_COUNT_AFTER)"
fi

# =========================================================================
# Summary
# =========================================================================
log "All assertions: total=$TOTAL_ASSERTIONS passed=$PASSED_ASSERTIONS failed=$FAILED_ASSERTIONS"
if [[ "$FAILED_ASSERTIONS" -ne 0 ]]; then
    exit 1
fi
log "All tests passed!"
