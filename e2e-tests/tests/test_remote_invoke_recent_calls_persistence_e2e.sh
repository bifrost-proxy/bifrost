#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

log() { echo "[remote-invoke-calls-persistence-e2e] $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd curl
require_cmd jq
require_cmd cargo
require_cmd npx
require_cmd python3

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

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
        --max-time 20
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
    http_request "$1" "GET" "" "${2:-}"
}

http_post_json() {
    http_request "$1" "POST" "$2" "${3:-}"
}

RELAY_PORT="$(pick_free_port)"
ADMIN_PORT="$(pick_free_port)"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$REPO_DIR/.bifrost-e2e-remote-invoke-calls-persistence-$RANDOM}"
CALLER_DATA_DIR="$(mktemp -d)"
RELAY_DATA_DIR="$(mktemp -d)"
RELAY_LOG="$(mktemp)"
CALLER_CONNECT_LOG="$(mktemp)"
STATUS_LOG="$(mktemp)"
RELAY_PID=""
CALLER_CONNECT_PID=""

stop_bifrost_preserve_data() {
    if [[ -n "${ADMIN_CLIENT_BIFROST_PID:-}" ]] && kill -0 "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null; then
        safe_cleanup_proxy "$ADMIN_CLIENT_BIFROST_PID"
        wait "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null || true
    fi
    if [[ -n "${ADMIN_CLIENT_BIFROST_LOG_FILE:-}" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        rm -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null || true
    fi
    ADMIN_CLIENT_BIFROST_PID=""
    ADMIN_CLIENT_BIFROST_LOG_FILE=""
    ADMIN_CLIENT_BIFROST_DATA_DIR=""
    ADMIN_CLIENT_HOME_DIR=""
    ADMIN_CLIENT_XDG_CONFIG_HOME=""
    ADMIN_CLIENT_XDG_DATA_HOME=""
    ADMIN_CLIENT_STARTED_BIFROST=0
}

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
    rm -rf "$BIFROST_DATA_DIR" "$CALLER_DATA_DIR" "$RELAY_DATA_DIR"
    rm -f "$RELAY_LOG" "$CALLER_CONNECT_LOG" "$STATUS_LOG"
}
trap cleanup EXIT

log "Starting local relay on port $RELAY_PORT"
RELAY_EXEC="$(sync_server_exec "$SYNC_SERVER_DIR")"
(cd "$SYNC_SERVER_DIR" && eval "$RELAY_EXEC" -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke) >"$RELAY_LOG" 2>&1 &
RELAY_PID=$!

for _ in $(seq 1 40); do
    code="$(curl -s -o /dev/null -w '%{http_code}' "${RELAY_URL}/v4/remote-invoke/client/register" 2>/dev/null || true)"
    if [[ "$code" =~ ^(200|4..) ]]; then
        break
    fi
    sleep 0.5
done
if ! kill -0 "$RELAY_PID" 2>/dev/null; then
    cat "$RELAY_LOG" >&2
    exit 1
fi

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
    log "Building release bifrost"
    (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null)
fi
BIFROST_BIN="$REPO_DIR/target/release/bifrost"

mkdir -p "$BIFROST_DATA_DIR"
cat > "$BIFROST_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF

export ADMIN_PORT
export ADMIN_HOST="0.0.0.0"
export ADMIN_PATH_PREFIX
export BIFROST_DATA_DIR

log "Starting bifrost admin on port $ADMIN_PORT"
admin_start_bifrost

SYNC_USER_ID="remote_invoke_calls_persist_${RANDOM}"
SYNC_PASSWORD="remote_invoke_calls_persist_123"

http_post_json "${RELAY_URL}/v4/sso/register" "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"${SYNC_PASSWORD}\",\"nickname\":\"Calls Persistence E2E\"}"
assert_status "200" "$HTTP_STATUS" "relay 注册测试用户应返回 200"
RELAY_SYNC_TOKEN="$(echo "$HTTP_BODY" | jq -r '.data.token // ""')"
assert_not_empty "$RELAY_SYNC_TOKEN" "relay token 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/sync/session" "{\"token\":\"${RELAY_SYNC_TOKEN}\"}"
assert_status "200" "$HTTP_STATUS" "保存 sync session 应返回 200"

WORKER_READY=0
for _ in $(seq 1 40); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/status"
    if [[ "$HTTP_STATUS" == "200" ]] && [[ "$(echo "$HTTP_BODY" | jq -r '.state // ""')" == "Connected" ]]; then
        WORKER_READY=1
        break
    fi
    sleep 1
done
if [[ "$WORKER_READY" -ne 1 ]]; then
    _log_fail "remote invoke worker 未连接 relay" "state=Connected" "${HTTP_BODY:-<empty>}"
    exit 1
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/identity"
assert_status "200" "$HTTP_STATUS" "读取 client identity 应返回 200"
CLIENT_INSTANCE_ID="$(echo "$HTTP_BODY" | jq -r '.instance_id // ""')"
assert_not_empty "$CLIENT_INSTANCE_ID" "client instance_id 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/discovery/enter" "{}"
assert_status "200" "$HTTP_STATUS" "进入 discovery 应返回 200"
PAIR_CODE="$(echo "$HTTP_BODY" | jq -r '.session.pair_code // ""')"
assert_not_empty "$PAIR_CODE" "pair_code 不应为空"

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up "$PAIR_CODE" --relay-url "$RELAY_URL" >"$CALLER_CONNECT_LOG" 2>&1 &
CALLER_CONNECT_PID=$!

PAIRING_ID=""
for _ in $(seq 1 30); do
    http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/pending"
    PAIRING_ID="$(echo "$HTTP_BODY" | jq -r '.pairings[0].pairing_id // ""')"
    [[ -n "$PAIRING_ID" ]] && break
    sleep 1
done
assert_not_empty "$PAIRING_ID" "pairing_id 不应为空"

http_post_json "${CLIENT_ADMIN_URL}/api/remote-invoke/pairings/${PAIRING_ID}/approve" '{"grant_mode":"permanent"}'
assert_status "200" "$HTTP_STATUS" "批准配对应返回 200"

CONNECT_OK=0
for _ in $(seq 1 30); do
    if ! kill -0 "$CALLER_CONNECT_PID" 2>/dev/null; then
        wait "$CALLER_CONNECT_PID" 2>/dev/null || CONNECT_EXIT=$?
        CONNECT_EXIT="${CONNECT_EXIT:-0}"
        if [[ "$CONNECT_EXIT" -eq 0 ]]; then
            CONNECT_OK=1
        fi
        break
    fi
    sleep 1
done
CALLER_CONNECT_PID=""
if [[ "$CONNECT_OK" -ne 1 ]]; then
    _log_fail "remote connect 失败" "exit 0" "$(cat "$CALLER_CONNECT_LOG")"
    exit 1
fi

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status \
    --relay-url "$RELAY_URL" \
    --client-id "${CLIENT_INSTANCE_ID:0:12}" >"$STATUS_LOG" 2>&1

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "读取 Recent Calls API 应返回 200"
CALL_ID="$(echo "$HTTP_BODY" | jq -r '
    (.calls // [])
    | map(select((.command.command // .command) == "status"))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].call_id // ""
')"
assert_not_empty "$CALL_ID" "重启前 Recent Calls 应包含 status 调用"

STORE_FILE="$BIFROST_DATA_DIR/admin/remote_invoke_call_history.json"
if [[ ! -s "$STORE_FILE" ]]; then
    _log_fail "Recent Calls 落盘文件不存在或为空" "$STORE_FILE exists" "missing"
    exit 1
fi

if ! jq -e --arg call_id "$CALL_ID" '.entries[] | select(.call_id == $call_id)' "$STORE_FILE" >/dev/null; then
    _log_fail "Recent Calls 落盘文件缺少目标 call_id" "$CALL_ID" "$(cat "$STORE_FILE")"
    exit 1
fi
_log_pass "TC-RI-PERSIST-01A: Recent Calls 已写入本地落盘文件"

log "Restarting bifrost admin with the same data dir"
stop_bifrost_preserve_data
admin_start_bifrost

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "重启后读取 Recent Calls API 应返回 200"
RESTORED_PREVIEW="$(echo "$HTTP_BODY" | jq -r --arg call_id "$CALL_ID" '
    (.calls // [])
    | map(select(.call_id == $call_id))
    | .[0].command_summary.command_preview // ""
')"

if [[ "$RESTORED_PREVIEW" == "status" ]]; then
    _log_pass "TC-RI-PERSIST-01B: 重启 Bifrost 后 Recent Calls 仍保留 status 调用"
else
    _log_fail "TC-RI-PERSIST-01B: 重启后 Recent Calls 丢失或摘要错误" "status" "${RESTORED_PREVIEW:-<empty>}"
    exit 1
fi

log "Recent Calls persistence E2E passed"
