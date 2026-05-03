#!/bin/bash

set -uo pipefail

# 避免环境中的代理变量干扰本地 curl（特别是 http_proxy/https_proxy）。
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""
CURL_ERROR=""

http_request() {
    local url="$1"
    local method="${2:-GET}"
    local data="${3:-}"
    local extra_headers="${4:-}"
    local proxy="${5:-}"

    local headers_file
    local body_file
    local err_file
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

    if [[ -n "$proxy" ]]; then
        curl_args+=(--proxy "$proxy")
    fi

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
    http_request "$url" "GET" "" "$extra_headers" ""
}

http_post_json() {
    local url="$1"
    local data="$2"
    local extra_headers="${3:-}"
    http_request "$url" "POST" "$data" "$extra_headers" ""
}

proxy_get() {
    local proxy_url="$1"
    local url="$2"
    http_request "$url" "GET" "" "" "$proxy_url"
}

pick_free_port() {
    # 用 python 绑定 127.0.0.1:0 获取空闲端口，避免 CI 并行任务端口冲突。
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
export ADMIN_BASE_URL="${ADMIN_BASE_URL:-http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}}"

BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$SCRIPT_DIR/../../.bifrost-e2e-remote-auth-$RANDOM}"
export BIFROST_DATA_DIR

MOCK_SERVERS_SCRIPT="$SCRIPT_DIR/../mock_servers/start_servers.sh"

# remote-auth E2E 仅依赖 HTTP echo server；限制 mock server 集合 + 使用动态端口，避免与其他 E2E 并发抢占固定端口（如 3000/3443/3020 等）。
export MOCK_SERVERS="${MOCK_SERVERS_LIST:-http}"
HTTP_PORT="${HTTP_PORT:-}"
if [[ -z "$HTTP_PORT" ]]; then
    HTTP_PORT="$(pick_free_port)"
fi
export HTTP_PORT

cleanup() {
    admin_cleanup_bifrost || true
    "$MOCK_SERVERS_SCRIPT" stop >/dev/null 2>&1 || true
    rm -rf "$BIFROST_DATA_DIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log() { echo "[remote-auth-e2e] $*"; }

assert_equals() {
    local expected="$1"
    local actual="$2"
    local msg="${3:-Values should be equal}"

    if [[ "$expected" == "$actual" ]]; then
        _log_pass "$msg"
        return 0
    fi
    _log_fail "$msg" "$expected" "$actual"
    return 1
}

assert_json_field() {
    local json="$1"
    local field="$2"
    local expected="$3"
    local msg="${4:-JSON field should match}"

    local actual
    # 注意：不能用 `jq -e` 直接取字段，因为布尔 false 会导致 jq 以非 0 退出码返回。
    if ! echo "$json" | jq -e . >/dev/null 2>&1; then
        _log_fail "$msg（响应体不是合法 JSON）" "$expected" "<invalid-json>"
        echo "[debug] HTTP_STATUS=$HTTP_STATUS" >&2
        echo "[debug] curl_error=${CURL_ERROR:-<empty>}" >&2
        echo "[debug] HTTP_HEADERS:" >&2
        echo "$HTTP_HEADERS" >&2
        echo "[debug] HTTP_BODY(head 600):" >&2
        printf '%s' "$HTTP_BODY" | head -c 600 >&2 || true
        echo >&2
        return 1
    fi

    if ! actual=$(echo "$json" | jq -r "$field" 2>/dev/null); then
        _log_fail "$msg（jq 取字段失败）" "$expected" "<jq-error>"
        echo "[debug] HTTP_STATUS=$HTTP_STATUS" >&2
        echo "[debug] curl_error=${CURL_ERROR:-<empty>}" >&2
        echo "[debug] HTTP_HEADERS:" >&2
        echo "$HTTP_HEADERS" >&2
        echo "[debug] HTTP_BODY(head 600):" >&2
        printf '%s' "$HTTP_BODY" | head -c 600 >&2 || true
        echo >&2
        return 1
    fi

    if [[ "$actual" == "null" ]]; then
        _log_fail "$msg（字段不存在或为 null）" "$expected" "null"
        return 1
    fi
    assert_equals "$expected" "$actual" "$msg"
}

require_cmd() {
    local cmd="$1"
    command -v "$cmd" >/dev/null 2>&1 || {
        echo "Missing required command: $cmd" >&2
        exit 1
    }
}

require_cmd curl
require_cmd jq
require_cmd cargo

BIFROST_BIN="$SCRIPT_DIR/../../target/release/bifrost"
if [[ -x "$BIFROST_BIN" && "${SKIP_BUILD:-}" == "true" ]]; then
    log "Reusing pre-built bifrost binary at $BIFROST_BIN"
else
    log "Build bifrost (release)..."
    (cd "$SCRIPT_DIR/../.." && cargo build --release --bin bifrost >/dev/null)
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
fi

log "Start mock servers..."
"$MOCK_SERVERS_SCRIPT" start-bg >/dev/null

log "Start bifrost (admin+proxy)..."
export ADMIN_HOST
export ADMIN_PORT
admin_start_bifrost

ADMIN_BASE_URL_EFFECTIVE="${ADMIN_BASE_URL}"
ADMIN_URL_127="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

get_non_loopback_ip() {
    local ip
    ip=$(ip route get 1.1.1.1 2>/dev/null | awk '{for (i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}') || true
    if [[ -z "${ip:-}" ]]; then
        ip=$(hostname -I 2>/dev/null | awk '{print $1}') || true
    fi
    echo "${ip:-}"
}

NON_LOOPBACK_IP="$(get_non_loopback_ip)"

log "Case: remote access disabled -> local admin should work"
http_get "${ADMIN_URL_127}/api/auth/status"
assert_status "200" "$HTTP_STATUS" "本地访问 /api/auth/status 应返回 200"
assert_json_field "$HTTP_BODY" ".remote_access_enabled" "false" "remote_access_enabled 应为 false"
assert_json_field "$HTTP_BODY" ".auth_required" "false" "auth_required 应为 false"

if [[ -n "${NON_LOOPBACK_IP:-}" ]]; then
    log "Case: remote access disabled -> non-loopback admin should be rejected"
    http_get "http://${NON_LOOPBACK_IP}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/auth/status"
    if [[ "$HTTP_STATUS" == "000" ]]; then
        _log_warning "非 loopback 地址不可达（curl 连接失败），跳过 403 断言"
    else
        assert_status "403" "$HTTP_STATUS" "非 loopback 访问管理端应返回 403"
    fi
else
    _log_warning "无法获取非 loopback IP，跳过非本地访问门禁断言"
fi

log "Ensure data plane forwarding works before enabling remote auth"
RULE_NAME="remote_auth_forward_${RANDOM}"
create_rule "$RULE_NAME" "example.com http://127.0.0.1:${HTTP_PORT}" "true" >/dev/null

proxy_get "http://127.0.0.1:${ADMIN_PORT}" "http://example.com/remote-auth-pre"
assert_status "200" "$HTTP_STATUS" "代理转发在鉴权开启前应正常工作"
assert_json_field "$HTTP_BODY" ".server.port" "${HTTP_PORT}" "代理应转发到 mock http_echo_server(${HTTP_PORT})"

log "Enable remote access and set admin password via CLI"
ADMIN_PASSWORD="test-pass-${RANDOM}-${RANDOM}"
printf '%s\n' "$ADMIN_PASSWORD" | BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" admin passwd --username admin --password-stdin >/dev/null
BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" admin remote enable >/dev/null

log "Case: loopback should bypass auth even after remote access enabled"
http_get "${ADMIN_URL_127}/api/rules"
assert_status "200" "$HTTP_STATUS" "loopback 无 Token 访问受保护 API 应返回 200（loopback 免鉴权）"

log "Login -> get token"
LOGIN_PAYLOAD=$(jq -cn --arg u "admin" --arg p "$ADMIN_PASSWORD" '{username:$u,password:$p}')
http_post_json "${ADMIN_URL_127}/api/auth/login" "$LOGIN_PAYLOAD"
assert_status "200" "$HTTP_STATUS" "登录接口应返回 200"
TOKEN=$(echo "$HTTP_BODY" | jq -r '.token')
if [[ -z "${TOKEN:-}" || "$TOKEN" == "null" ]]; then
    echo "Failed to get token from login response: $HTTP_BODY" >&2
    exit 1
fi

log "Case: loopback with valid token -> 200"
http_get "${ADMIN_URL_127}/api/rules" "Authorization: Bearer ${TOKEN}"
assert_status "200" "$HTTP_STATUS" "loopback 携带有效 Token 访问受保护 API 应返回 200"

if [[ -n "${NON_LOOPBACK_IP:-}" ]]; then
    ADMIN_URL_NON_LB="http://${NON_LOOPBACK_IP}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

    log "Case: non-loopback without token -> 401"
    http_get "${ADMIN_URL_NON_LB}/api/rules"
    if [[ "$HTTP_STATUS" == "000" ]]; then
        _log_warning "非 loopback 地址不可达（curl 连接失败），跳过 401 断言"
    else
        assert_status "401" "$HTTP_STATUS" "非 loopback 无 Token 访问受保护 API 应返回 401"
    fi

    log "Case: non-loopback with valid token -> 200"
    http_get "${ADMIN_URL_NON_LB}/api/rules" "Authorization: Bearer ${TOKEN}"
    if [[ "$HTTP_STATUS" == "000" ]]; then
        _log_warning "非 loopback 地址不可达（curl 连接失败），跳过 200 断言"
    else
        assert_status "200" "$HTTP_STATUS" "非 loopback 携带有效 Token 应返回 200"
    fi
else
    _log_warning "无法获取非 loopback IP，跳过非本地鉴权断言"
fi

log "Revoke all sessions via CLI"
BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" admin revoke-all >/dev/null

log "Case: loopback after revoke-all -> still 200 (loopback bypass)"
http_get "${ADMIN_URL_127}/api/rules" "Authorization: Bearer ${TOKEN}"
assert_status "200" "$HTTP_STATUS" "revoke-all 后 loopback 仍应返回 200（loopback 免鉴权）"

if [[ -n "${NON_LOOPBACK_IP:-}" ]]; then
    log "Case: non-loopback old token after revoke-all -> 401"
    http_get "${ADMIN_URL_NON_LB}/api/rules" "Authorization: Bearer ${TOKEN}"
    if [[ "$HTTP_STATUS" == "000" ]]; then
        _log_warning "非 loopback 地址不可达（curl 连接失败），跳过 401 断言"
    else
        assert_status "401" "$HTTP_STATUS" "revoke-all 后非 loopback 旧 Token 应失效返回 401"
    fi
fi

log "Ensure data plane forwarding still works after enabling/revoking admin auth"
proxy_get "http://127.0.0.1:${ADMIN_PORT}" "http://example.com/remote-auth-post"
assert_status "200" "$HTTP_STATUS" "代理转发不应受管理端鉴权影响"
assert_json_field "$HTTP_BODY" ".server.port" "${HTTP_PORT}" "代理应持续转发到 mock http_echo_server(${HTTP_PORT})"

delete_rule "$RULE_NAME" >/dev/null 2>&1 || true

log "All assertions: total=$TOTAL_ASSERTIONS passed=$PASSED_ASSERTIONS failed=$FAILED_ASSERTIONS"
if [[ "$FAILED_ASSERTIONS" -ne 0 ]]; then
    exit 1
fi
