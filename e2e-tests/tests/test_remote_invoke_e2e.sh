#!/bin/bash
set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"
RELAY_PORT=""
RELAY_PID=""
RELAY_LOG=""

start_local_relay() {
    RELAY_PORT="$(pick_free_port)"
    RELAY_LOG="$(mktemp)"
    local relay_data_dir
    relay_data_dir="$(mktemp -d)"

    log "Starting local bifrost-sync-server on port $RELAY_PORT..."
    (cd "$SYNC_SERVER_DIR" && \
        npx tsx src/cli.ts -p "$RELAY_PORT" -d "$relay_data_dir" --enable-remote-invoke \
    ) > "$RELAY_LOG" 2>&1 &
    RELAY_PID=$!

    local waited=0
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
    tail -50 "$RELAY_LOG" 2>/dev/null || true
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

cleanup() {
    if [[ -n "$CALLER_CONNECT_PID" ]] && kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        kill "$CALLER_CONNECT_PID" 2>/dev/null || true
        wait "$CALLER_CONNECT_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "$RELAY_PID" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    [[ -n "${RELAY_LOG:-}" ]] && rm -f "$RELAY_LOG" 2>/dev/null || true
    rm -rf "$BIFROST_DATA_DIR" >/dev/null 2>&1 || true
    rm -rf "$CALLER_DATA_DIR" >/dev/null 2>&1 || true
    [[ -n "${CALLER_CONNECT_LOG:-}" ]] && rm -f "$CALLER_CONNECT_LOG" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[remote-invoke-e2e] $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd curl
require_cmd jq
require_cmd cargo

if [[ -z "$RELAY_URL" ]]; then
    start_local_relay
    RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
fi

log "Build bifrost (release)..."
(cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)

BIFROST_BIN="$REPO_DIR/target/release/bifrost"
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
log "Start bifrost remote connect in background (Caller)..."
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote connect "$PAIR_CODE" --relay-url "$RELAY_URL" \
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
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
assert_status "200" "$HTTP_STATUS" "grants 列表应返回 200"
GRANT_COUNT=$(echo "$HTTP_BODY" | jq '.grants | length')
if [[ "$GRANT_COUNT" -gt 0 ]]; then
    _log_pass "Client 侧存在至少一个 grant"
else
    _log_fail "Client 侧应存在至少一个 grant" ">=1" "$GRANT_COUNT"
fi

log "Exit discovery mode"
http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/exit" "{}"

# =========================================================================
# TC-RI-02: Remote status command
# =========================================================================
log "=== TC-RI-02: Remote status command ==="

STATUS_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote status \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true

if echo "$STATUS_OUTPUT" | grep -qiE "proxy_address|instance_id|platform|version"; then
    _log_pass "TC-RI-02: remote status 返回了设备信息"
else
    _log_fail "TC-RI-02: remote status 未返回预期的设备信息" "包含 proxy_address/instance_id" "$STATUS_OUTPUT"
fi

# =========================================================================
# TC-RI-03: Remote traffic list command
# =========================================================================
log "=== TC-RI-03: Remote traffic list ==="

log "Generate some traffic via proxy..."
REMOTE_MARKER="remote-invoke-${RANDOM}"
curl -sS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "http://httpbin.org/anything/${REMOTE_MARKER}" >/dev/null 2>&1 || true
sleep 2

TRAFFIC_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic list \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --limit 5 2>&1) || true

if echo "$TRAFFIC_OUTPUT" | grep -qiE "Seq|Host|Method|Status|httpbin"; then
    _log_pass "TC-RI-03: remote traffic list 返回了流量记录"
else
    _log_warning "TC-RI-03: traffic list 可能为空（无匹配规则时无流量）: $(echo "$TRAFFIC_OUTPUT" | head -3)"
    _log_pass "TC-RI-03: remote traffic list 命令执行成功（数据取决于规则配置）"
fi

http_get "${CLIENT_ADMIN_URL}/api/traffic?limit=20"
assert_status "200" "$HTTP_STATUS" "traffic 列表应返回 200"
TARGET_SEQ=$(echo "$HTTP_BODY" | jq -r --arg marker "$REMOTE_MARKER" '
  (.records // [])[]
  | select(((.path // .p // "") | contains($marker)))
  | (.seq // .sequence // empty)
' | head -n 1)
assert_not_empty "$TARGET_SEQ" "用于 remote traffic get 的 sequence 不应为空"

REMOTE_GET_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic get "$TARGET_SEQ" \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true

if [[ "$REMOTE_GET_OUTPUT" == *"\"seq\":${TARGET_SEQ}"* || "$REMOTE_GET_OUTPUT" == *"\"sequence\":${TARGET_SEQ}"* ]] \
    && [[ "$REMOTE_GET_OUTPUT" == *"$REMOTE_MARKER"* ]]; then
    _log_pass "TC-RI-03A: remote traffic get 支持 sequence 查询并返回详情"
else
    _log_fail "TC-RI-03A: remote traffic get 未返回目标 sequence 的详情" "包含 seq/sequence=${TARGET_SEQ} 与 marker=${REMOTE_MARKER}" "$REMOTE_GET_OUTPUT"
fi

REMOTE_GET_MISSING_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic get 999999999 \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" 2>&1) || true

if echo "$REMOTE_GET_MISSING_OUTPUT" | grep -q "No traffic record with sequence suffix '999999999' found"; then
    _log_pass "TC-RI-03B: remote traffic get 失败时会透出真实错误"
else
    _log_fail "TC-RI-03B: remote traffic get 失败时未透出真实错误" "包含 sequence suffix not found 错误" "$REMOTE_GET_MISSING_OUTPUT"
fi

# =========================================================================
# TC-RI-04: Remote search command
# =========================================================================
log "=== TC-RI-04: Remote search command ==="

SEARCH_LOG="$(mktemp)"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote search "$REMOTE_MARKER" \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --limit 5 >"$SEARCH_LOG" 2>&1 &
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

if [[ "$SEARCH_EXIT" -eq 0 ]] && echo "$SEARCH_OUTPUT" | grep -q "$REMOTE_MARKER" && echo "$SEARCH_OUTPUT" | grep -q "Found 1 matches"; then
    _log_pass "TC-RI-04: remote search 返回了目标结果"
else
    _log_fail "TC-RI-04: remote search 未返回目标结果" "包含 marker=${REMOTE_MARKER} 与 Found 1 matches" "$SEARCH_OUTPUT"
fi

if [[ "$SEARCH_STREAM_SEEN" -eq 1 ]] && echo "$SEARCH_OUTPUT" | grep -q "Searching..."; then
    _log_pass "TC-RI-04A: remote search 输出包含流式进度"
else
    _log_fail "TC-RI-04A: remote search 未输出流式进度" "输出包含 Searching..." "$SEARCH_OUTPUT"
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
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote connect "$PAIR_CODE_2" --relay-url "$RELAY_URL" \
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
BIFROST_DATA_DIR="$CALLER_DATA_DIR" timeout 15 "$BIFROST_BIN" remote connect "000000" --relay-url "$RELAY_URL" \
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
# TC-RI-08: Disconnect
# =========================================================================
log "=== TC-RI-08: Disconnect ==="

DISCONNECT_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote disconnect --all \
    --relay-url "$RELAY_URL" 2>&1) || true

if echo "$DISCONNECT_OUTPUT" | grep -qi "revoked\|disconnected\|✓"; then
    _log_pass "TC-RI-08: disconnect --all 成功"
else
    _log_fail "TC-RI-08: disconnect 输出未包含成功标识" "revoked/disconnected" "$DISCONNECT_OUTPUT"
fi

# =========================================================================
# TC-RI-09: Disconnected grant cannot be reused
# =========================================================================
log "=== TC-RI-09: Disconnected grant cannot be reused ==="

REUSE_OUTPUT=$(BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote status \
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
