#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

log() { echo "[remote-invoke-args-preview-e2e] $*"; }

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
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-$(pick_free_port)}"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
CLIENT_ADMIN_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$REPO_DIR/.bifrost-e2e-remote-invoke-args-preview-$RANDOM}"
CALLER_DATA_DIR="$(mktemp -d)"
RELAY_LOG="$(mktemp)"
RELAY_DATA_DIR="$(mktemp -d)"
CALLER_CONNECT_LOG="$(mktemp)"
SEARCH_LOG="$(mktemp)"
LONG_SEARCH_LOG="$(mktemp)"
GRANT_LIST_LOG="$(mktemp)"
MOCK_SERVER_LOG="$(mktemp)"
RELAY_PID=""
CALLER_CONNECT_PID=""
MOCK_SERVER_PID=""

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
    if [[ -n "$MOCK_SERVER_PID" ]] && kill -0 "$MOCK_SERVER_PID" 2>/dev/null; then
        kill "$MOCK_SERVER_PID" 2>/dev/null || true
        wait "$MOCK_SERVER_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost || true
    if [[ -n "$RELAY_PID" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    rm -rf "$BIFROST_DATA_DIR" "$CALLER_DATA_DIR" "$RELAY_DATA_DIR"
    rm -f "$RELAY_LOG" "$CALLER_CONNECT_LOG" "$SEARCH_LOG" "$LONG_SEARCH_LOG" "$GRANT_LIST_LOG" "$MOCK_SERVER_LOG"
}
trap cleanup EXIT

start_mock_server() {
    local requested_port="$MOCK_HTTP_PORT"
    log "Starting local HTTP echo fixture on port $requested_port"
    python3 -u "$REPO_DIR/e2e-tests/mock_servers/http_echo_server.py" --port "$MOCK_HTTP_PORT" --retries 5 >"$MOCK_SERVER_LOG" 2>&1 &
    MOCK_SERVER_PID=$!

    for _ in $(seq 1 120); do
        if ! kill -0 "$MOCK_SERVER_PID" 2>/dev/null; then
            _log_fail "本地 echo fixture 提前退出" "server keeps running" "$(cat "$MOCK_SERVER_LOG")"
            exit 1
        fi

        local actual_port
        actual_port="$(sed -nE 's/^Starting HTTP Echo Server on [^:]+:([0-9]+)\.\.\.$/\1/p' "$MOCK_SERVER_LOG" 2>/dev/null | tail -n 1)"
        if [[ -n "$actual_port" ]] && curl -fsS --max-time 2 "http://127.0.0.1:${actual_port}/health" >/dev/null 2>&1; then
            MOCK_HTTP_PORT="$actual_port"
            if [[ "$MOCK_HTTP_PORT" != "$requested_port" ]]; then
                log "Local HTTP echo fixture fell back from port $requested_port to $MOCK_HTTP_PORT"
            fi
            return 0
        fi
        sleep 0.5
    done

    _log_fail "本地 echo fixture 未在超时内就绪" "GET /health 返回 200" "$(cat "$MOCK_SERVER_LOG")"
    exit 1
}

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
    NEED_BUILD=0
    if [[ ! -x "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.toml" -nt "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.lock" -nt "$REPO_DIR/target/release/bifrost" ]]; then
        NEED_BUILD=1
    elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$REPO_DIR/target/release/bifrost" -print -quit | grep -q .; then
        NEED_BUILD=1
    fi

    if [[ "$NEED_BUILD" -eq 1 ]]; then
        log "Building release bifrost"
        (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null)
    fi
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
start_mock_server

SYNC_USER_ID="remote_invoke_args_preview_${RANDOM}"
SYNC_PASSWORD="remote_invoke_args_preview_123"

http_post_json "${RELAY_URL}/v4/sso/register" "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"${SYNC_PASSWORD}\",\"nickname\":\"Args Preview E2E\"}"
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
    _log_fail "remote conn up 失败" "exit 0" "$(cat "$CALLER_CONNECT_LOG")"
    exit 1
fi

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
assert_status "200" "$HTTP_STATUS" "执行命令前读取 Grants API 应返回 200"
GRANT_ID="$(echo "$HTTP_BODY" | jq -r '.grants[0].grant_id // ""')"
FIRST_CONNECTED_AT="$(echo "$HTTP_BODY" | jq -r '.grants[0].first_connected_at // .grants[0].first_authorized_at // empty')"
PRE_COMMAND_AT="$(echo "$HTTP_BODY" | jq -r '.grants[0].last_command_at // ""')"
assert_not_empty "$GRANT_ID" "grant_id 不应为空"
assert_not_empty "$FIRST_CONNECTED_AT" "first_connected_at 不应为空"
if [[ -n "$PRE_COMMAND_AT" ]]; then
    _log_fail "执行首个命令前 last_command_at 应为空" "empty" "$PRE_COMMAND_AT"
    exit 1
fi

REMOTE_MARKER="recent-calls-preview-${RANDOM}"
TRAFFIC_URL="http://127.0.0.1:${MOCK_HTTP_PORT}/anything/${REMOTE_MARKER}"
TRAFFIC_READY=0
for _ in $(seq 1 5); do
    if curl -fsS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "$TRAFFIC_URL" >/dev/null 2>&1; then
        TRAFFIC_READY=1
        break
    fi
    sleep 0.5
done
if [[ "$TRAFFIC_READY" -ne 1 ]]; then
    _log_fail "通过本地 echo fixture 生成 Recent Calls 流量失败" "$TRAFFIC_URL 返回 2xx" "$(cat "$MOCK_SERVER_LOG")"
    exit 1
fi
sleep 2

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "$REMOTE_MARKER" \
    --relay-url "$RELAY_URL" \
    --client-id "${CLIENT_INSTANCE_ID:0:12}" \
    --max-results 5 \
    --max-scan 50 >"$SEARCH_LOG" 2>&1 || {
    log "remote traffic search exited non-zero (acceptable — we only need the call recorded in Recent Calls)"
    log "search log: $(cat "$SEARCH_LOG")"
}
sleep 1

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "读取 Recent Calls API 应返回 200"

LATEST_SEARCH_MASKED_ARGS_JSON="$(echo "$HTTP_BODY" | jq -r --arg marker "$REMOTE_MARKER" '
    (.calls // [])
    | map(select(
        (.command_summary.command_preview // "") == "search.stream"
        and (
            (.command_summary.masked_args_json // "") | contains($marker)
        )
    ))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].command_summary.masked_args_json // ""
')"

assert_not_empty "$LATEST_SEARCH_MASKED_ARGS_JSON" "search.stream 的 masked_args_json 不应为空"

LATEST_SEARCH_CALL_ID="$(echo "$HTTP_BODY" | jq -r --arg marker "$REMOTE_MARKER" '
    (.calls // [])
    | map(select(
        (.command_summary.command_preview // "") == "search.stream"
        and (
            (.command_summary.masked_args_json // "") | contains($marker)
        )
    ))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].call_id // ""
')"
assert_not_empty "$LATEST_SEARCH_CALL_ID" "search.stream 的 call_id 不应为空"

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/grants"
assert_status "200" "$HTTP_STATUS" "执行命令后读取 Grants API 应返回 200"
POST_COMMAND_FIRST_CONNECTED_AT="$(echo "$HTTP_BODY" | jq -r --arg grant_id "$GRANT_ID" '
    (.grants // [])
    | map(select(.grant_id == $grant_id))
    | .[0].first_connected_at // .[0].first_authorized_at // empty
')"
POST_COMMAND_LAST_COMMAND_AT="$(echo "$HTTP_BODY" | jq -r --arg grant_id "$GRANT_ID" '
    (.grants // [])
    | map(select(.grant_id == $grant_id))
    | .[0].last_command_at // empty
')"
assert_not_empty "$POST_COMMAND_FIRST_CONNECTED_AT" "执行后 first_connected_at 不应为空"
assert_not_empty "$POST_COMMAND_LAST_COMMAND_AT" "执行后 last_command_at 不应为空"
if [[ "$POST_COMMAND_FIRST_CONNECTED_AT" != "$FIRST_CONNECTED_AT" ]]; then
    _log_fail "first_connected_at 执行命令后不应变化" "$FIRST_CONNECTED_AT" "$POST_COMMAND_FIRST_CONNECTED_AT"
    exit 1
fi
if [[ "$POST_COMMAND_LAST_COMMAND_AT" -lt "$FIRST_CONNECTED_AT" ]]; then
    _log_fail "last_command_at 不应早于 first_connected_at" ">= $FIRST_CONNECTED_AT" "$POST_COMMAND_LAST_COMMAND_AT"
    exit 1
fi
_log_pass "TC-RI-GRANTS-TIME-01: Grants API 展示首次连接时间与最近一次执行命令时间"

"$BIFROST_BIN" -p "$ADMIN_PORT" setting grant list >"$GRANT_LIST_LOG" 2>&1
if grep -Fq "first connected:" "$GRANT_LIST_LOG" \
    && grep -Fq "last command:" "$GRANT_LIST_LOG"; then
    _log_pass "TC-RI-GRANTS-TIME-02: CLI grant list 展示首次连接与最近命令时间"
else
    _log_fail "CLI grant list 缺少时间字段" "first connected + last command" "$(cat "$GRANT_LIST_LOG")"
    exit 1
fi

if echo "$LATEST_SEARCH_MASKED_ARGS_JSON" | jq -e --arg marker "$REMOTE_MARKER" '
    .keyword == $marker
    and .max_results == 5
    and .max_scan == 50
' >/dev/null 2>&1; then
    _log_pass "TC-RI-ARGS-01: Recent Calls 会展示 search.stream 的参数摘要"
else
    _log_fail "TC-RI-ARGS-01: Recent Calls 参数摘要缺少 query/max_results/max_scan" \
        'keyword=<marker>, max_results=5, max_scan=50' \
        "$LATEST_SEARCH_MASKED_ARGS_JSON"
    exit 1
fi

LONG_REMOTE_MARKER="recent-calls-long-$(python3 - <<'PY'
print("x" * 240)
PY
)"
LONG_REMOTE_MARKER_PREFIX="${LONG_REMOTE_MARKER:0:100}"
LONG_TRAFFIC_URL="http://127.0.0.1:${MOCK_HTTP_PORT}/anything/${LONG_REMOTE_MARKER}"
if ! curl -fsS --max-time 10 --proxy "http://127.0.0.1:${ADMIN_PORT}" "$LONG_TRAFFIC_URL" >/dev/null 2>&1; then
    _log_fail "通过本地 echo fixture 生成长参数 Recent Calls 流量失败" "$LONG_TRAFFIC_URL 返回 2xx" "$(cat "$MOCK_SERVER_LOG")"
    exit 1
fi

BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "$LONG_REMOTE_MARKER" \
    --relay-url "$RELAY_URL" \
    --client-id "${CLIENT_INSTANCE_ID:0:12}" \
    --max-results 5 \
    --max-scan 50 >"$LONG_SEARCH_LOG" 2>&1 || {
    log "long remote traffic search exited non-zero (acceptable — we only need the call recorded in Recent Calls)"
    log "long search log: $(cat "$LONG_SEARCH_LOG")"
}
sleep 1

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "读取长参数 Recent Calls API 应返回 200"
LATEST_LONG_SEARCH_CALL_ID="$(echo "$HTTP_BODY" | jq -r --arg prefix "$LONG_REMOTE_MARKER_PREFIX" '
    (.calls // [])
    | map(select(
        (.command_summary.command_preview // "") == "search.stream"
        and ((.command_summary.masked_args_json // "") | contains($prefix))
    ))
    | sort_by(.started_at // 0)
    | reverse
    | .[0].call_id // ""
')"
assert_not_empty "$LATEST_LONG_SEARCH_CALL_ID" "长参数 search.stream 的 call_id 不应为空"
LATEST_LONG_SEARCH_MASKED_ARGS_JSON="$(echo "$HTTP_BODY" | jq -r --arg call_id "$LATEST_LONG_SEARCH_CALL_ID" '
    (.calls // [])
    | map(select(.call_id == $call_id))
    | .[0].command_summary.masked_args_json // ""
')"
assert_not_empty "$LATEST_LONG_SEARCH_MASKED_ARGS_JSON" "长参数 search.stream 的 masked_args_json 不应为空"
LONG_MASKED_KEYWORD="$(echo "$LATEST_LONG_SEARCH_MASKED_ARGS_JSON" | jq -r '.keyword // ""')"
if [[ "${#LONG_MASKED_KEYWORD}" -le 120 ]] \
    && [[ "$LONG_MASKED_KEYWORD" == *"$LONG_REMOTE_MARKER_PREFIX"* ]]; then
    _log_pass "TC-RI-ARGS-02: Recent Calls 命令参数超过 120 字符会直接截断"
else
    _log_fail "TC-RI-ARGS-02: Recent Calls 长命令参数未按 120 字符截断" \
        "keyword length<=120 and contains prefix" \
        "keyword_length=${#LONG_MASKED_KEYWORD}; value=$LATEST_LONG_SEARCH_MASKED_ARGS_JSON"
    exit 1
fi

LEGACY_STORE_FILE="$BIFROST_DATA_DIR/admin/remote_invoke_call_history.json"
if [[ -e "$LEGACY_STORE_FILE" ]]; then
    _log_fail "旧 Recent Calls 整文件不应继续存在" "missing" "$LEGACY_STORE_FILE"
    exit 1
fi

STORE_DIR="$BIFROST_DATA_DIR/admin/remote_invoke_call_history"
if [[ ! -d "$STORE_DIR" ]]; then
    _log_fail "Recent Calls JSONL 目录不存在" "$STORE_DIR exists" "missing"
    exit 1
fi
STORE_MATCH="$(find "$STORE_DIR" -name '*.jsonl' -type f -print0 \
    | xargs -0 grep -l "\"call_id\":\"$LATEST_SEARCH_CALL_ID\"" 2>/dev/null \
    | head -n 1 || true)"
if [[ -z "$STORE_MATCH" ]]; then
    _log_fail "Recent Calls JSONL 缺少目标 call_id" "$LATEST_SEARCH_CALL_ID" "$(find "$STORE_DIR" -maxdepth 1 -type f -print -exec sh -c 'echo --- "$1"; sed -n "1,5p" "$1"' sh {} \; 2>/dev/null)"
    exit 1
fi
if grep -R -Fq "$LONG_REMOTE_MARKER" "$STORE_DIR"; then
    _log_fail "Recent Calls JSONL 不应保存超过 120 字符的完整参数" "full long marker absent" "$(find "$STORE_DIR" -maxdepth 1 -type f -print -exec sh -c 'echo --- "$1"; sed -n "1,5p" "$1"' sh {} \; 2>/dev/null)"
    exit 1
fi
_log_pass "TC-RI-ARGS-03: Recent Calls 已写入本地 JSONL 且未保存完整长参数"

log "Restarting bifrost admin with the same data dir"
stop_bifrost_preserve_data
admin_start_bifrost

http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "重启后读取 Recent Calls API 应返回 200"
RESTORED_PREVIEW="$(echo "$HTTP_BODY" | jq -r --arg call_id "$LATEST_SEARCH_CALL_ID" '
    (.calls // [])
    | map(select(.call_id == $call_id))
    | .[0].command_summary.command_preview // ""
')"
if [[ "$RESTORED_PREVIEW" == "search.stream" ]]; then
    _log_pass "TC-RI-ARGS-04: 重启 Bifrost 后 Recent Calls 仍保留 search.stream 调用"
else
    _log_fail "TC-RI-ARGS-04: 重启后 Recent Calls 丢失或摘要错误" "search.stream" "${RESTORED_PREVIEW:-<empty>}"
    exit 1
fi
RESTORED_LONG_SEARCH_MASKED_ARGS_JSON="$(echo "$HTTP_BODY" | jq -r --arg call_id "$LATEST_LONG_SEARCH_CALL_ID" '
    (.calls // [])
    | map(select(.call_id == $call_id))
    | .[0].command_summary.masked_args_json // ""
')"
assert_not_empty "$RESTORED_LONG_SEARCH_MASKED_ARGS_JSON" "重启后长参数 search.stream 的 masked_args_json 不应为空"
RESTORED_LONG_MASKED_KEYWORD="$(echo "$RESTORED_LONG_SEARCH_MASKED_ARGS_JSON" | jq -r '.keyword // ""')"
if [[ "${#RESTORED_LONG_MASKED_KEYWORD}" -le 120 ]] \
    && [[ "$RESTORED_LONG_MASKED_KEYWORD" == *"$LONG_REMOTE_MARKER_PREFIX"* ]]; then
    _log_pass "TC-RI-ARGS-04B: 重启后长参数 Recent Calls 仍保留且继续按 120 字符截断"
else
    _log_fail "TC-RI-ARGS-04B: 重启后长参数 Recent Calls 未保持截断" \
        "keyword length<=120 and contains prefix" \
        "keyword_length=${#RESTORED_LONG_MASKED_KEYWORD}; value=$RESTORED_LONG_SEARCH_MASKED_ARGS_JSON"
    exit 1
fi

http_request "${CLIENT_ADMIN_URL}/api/remote-invoke/calls" "DELETE"
assert_status "200" "$HTTP_STATUS" "清理 Recent Calls API 应返回 200"
http_get "${CLIENT_ADMIN_URL}/api/remote-invoke/calls"
assert_status "200" "$HTTP_STATUS" "清理后读取 Recent Calls API 应返回 200"
REMAINING_CALLS="$(echo "$HTTP_BODY" | jq '.calls | length')"
if [[ "$REMAINING_CALLS" -eq 0 ]]; then
    _log_pass "TC-RI-ARGS-05: Recent Calls 支持清理全部记录"
else
    _log_fail "TC-RI-ARGS-05: 清理后仍存在 Recent Calls" "0" "$REMAINING_CALLS"
    exit 1
fi

log "Recent Calls args preview and persistence E2E passed"
