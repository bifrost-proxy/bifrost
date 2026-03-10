#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${SCRIPT_DIR}/../test_utils/assert.sh"

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy no_proxy NO_PROXY

pick_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(('127.0.0.1', 0))
print(s.getsockname()[1])
s.close()
PY
}

ADMIN_PORT="${ADMIN_PORT:-$(pick_port)}"
ECHO_PORT="${ECHO_PORT:-$(pick_port)}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_BASE_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

DATA_DIR="${BIFROST_DATA_DIR:-${ROOT_DIR}/.bifrost-e2e-test/search-admin-api-${ADMIN_PORT}}"

PIDS=()

cleanup() {
  for pid in "${PIDS[@]}"; do
    if kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
    fi
  done
}

trap cleanup EXIT

admin_curl() {
  # 强制直连，避免环境代理导致 403/转发错误
  curl -sS --noproxy "*" \
    -H "Host: 127.0.0.1:${ADMIN_PORT}" \
    "$@"
}

proxy_curl() {
  # 通过本地代理发起真实流量（不要覆盖 Host，避免影响上游与落库字段）
  curl -sS --noproxy "*" \
    --proxy "http://127.0.0.1:${ADMIN_PORT}" \
    "$@"
}

wait_for_admin() {
  local timeout="${1:-180}"
  local i=0
  while [[ $i -lt $timeout ]]; do
    if admin_curl "${ADMIN_BASE_URL}/api/system/status" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    i=$((i + 1))
  done
  echo "Timeout waiting for admin at ${ADMIN_BASE_URL}" >&2
  if [[ -f "${DATA_DIR}/bifrost.log" ]]; then
    echo "---- bifrost.log (tail) ----" >&2
    tail -n 50 "${DATA_DIR}/bifrost.log" >&2 || true
    echo "----------------------------" >&2
  fi
  return 1
}

assert_search_has_field() {
  local expected_field="$1"
  local json="$2"
  local msg="$3"
  local ok
  ok=$(echo "$json" | jq -r --arg f "$expected_field" '[.results[].matches[].field] | any(. == $f)')
  if [[ "$ok" == "true" ]]; then
    _log_pass "$msg"
  else
    _log_fail "$msg" "field=$expected_field" "$(echo "$json" | jq -r '.results[0].matches[0].field // "<none>"')"
  fi
}

wait_for_search_any() {
  local keyword="$1"
  local timeout_secs="${2:-10}"
  local start
  start=$(date +%s)
  while true; do
    local resp
    resp=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
      -H "Content-Type: application/json" \
      -d "{\"keyword\":\"${keyword}\",\"filters\":{},\"cursor\":null,\"limit\":50}")
    local cnt
    cnt=$(echo "$resp" | jq -r '.results | length')
    if [[ "$cnt" != "0" ]]; then
      return 0
    fi
    local now
    now=$(date +%s)
    if [[ $((now - start)) -ge "$timeout_secs" ]]; then
      echo "[WARN] 等待搜索结果超时（keyword=${keyword}）" >&2
      echo "$resp" >&2
      return 1
    fi
    sleep 0.2
  done
}

echo "[INFO] 启动 HTTP Echo Server: 127.0.0.1:${ECHO_PORT}"
python3 "${ROOT_DIR}/e2e-tests/mock_servers/http_echo_server.py" "${ECHO_PORT}" >/dev/null 2>&1 &
PIDS+=("$!")

echo "[INFO] 启动 Bifrost 代理（包含 Admin API）: 127.0.0.1:${ADMIN_PORT}"
mkdir -p "${DATA_DIR}"

LOG_FILE="${DATA_DIR}/bifrost.log"

echo "[INFO] 尝试停止已有 bifrost（如果之前残留进程/PID 文件）"
(cd "${ROOT_DIR}" && \
  BIFROST_DATA_DIR="${DATA_DIR}" \
  cargo run --bin bifrost -- stop \
    >/dev/null 2>&1) || true
sleep 1

(cd "${ROOT_DIR}" && \
  BIFROST_DATA_DIR="${DATA_DIR}" \
  RUST_LOG="info" \
  cargo run --bin bifrost -- start -p "${ADMIN_PORT}" --no-intercept --skip-cert-check \
    >"${LOG_FILE}" 2>&1) &
PIDS+=("$!")

wait_for_admin 180

TOKEN="search_token_${ADMIN_PORT}"

echo "[INFO] 生成流量（URL/请求头/请求体/响应头/响应体都包含 token）"

# URL + Request Header
proxy_curl \
  -H "X-Search-Token: ${TOKEN}" \
  -H "X-Test-ID: ${TOKEN}" \
  "http://127.0.0.1:${ECHO_PORT}/search/${TOKEN}?q=${TOKEN}" \
  >/dev/null

# Request Body
proxy_curl \
  -H "Content-Type: application/json" \
  -H "X-Test-ID: ${TOKEN}" \
  -d "{\"token\":\"${TOKEN}\"}" \
  "http://127.0.0.1:${ECHO_PORT}/echo" \
  >/dev/null

# Response Header + Response Body
proxy_curl \
  -H "X-Test-ID: ${TOKEN}" \
  "http://127.0.0.1:${ECHO_PORT}/large-response?size=8192&marker=${TOKEN}" \
  >/dev/null

sleep 1

wait_for_search_any "${TOKEN}" 15 || true

echo "[INFO] 使用 Admin API 验证搜索效果：${ADMIN_BASE_URL}/api/search"

resp_url=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"url\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "url" "$resp_url" "scope=url 能命中"

resp_req_hdr=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"request_headers\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "request_header" "$resp_req_hdr" "scope=request_headers 能命中"

resp_res_hdr=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"response_headers\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "response_header" "$resp_res_hdr" "scope=response_headers 能命中"

resp_req_body=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"request_body\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "request_body" "$resp_req_body" "scope=request_body 能命中"

resp_res_body=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"response_body\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "response_body" "$resp_res_body" "scope=response_body 能命中"

echo "[INFO] 强制 body 落盘（max_body_memory_size=1），验证文件场景仍可搜索"
admin_curl -X PUT "${ADMIN_BASE_URL}/api/config/performance" \
  -H "Content-Type: application/json" \
  -d '{"max_body_memory_size":1}' \
  >/dev/null

BIG_BODY_FILE="${DATA_DIR}/big_body.txt"
python3 - <<PY
token = "${TOKEN}"
with open("${BIG_BODY_FILE}", "w", encoding="utf-8") as f:
    f.write("A" * 10000 + "-" + token + "-" + "B" * 10000)
PY

proxy_curl \
  -H "Content-Type: text/plain" \
  --data-binary "@${BIG_BODY_FILE}" \
  "http://127.0.0.1:${ECHO_PORT}/echo" \
  >/dev/null

sleep 1

resp_file_body=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"scope\":{\"all\":false,\"request_body\":true},\"filters\":{},\"cursor\":null,\"limit\":50}")
assert_search_has_field "request_body" "$resp_file_body" "文件落盘 request_body 仍可命中"

echo "[INFO] 验证跨 block 边界关键字不会漏（HELLO）"
BOUNDARY_FILE="${DATA_DIR}/boundary_body.txt"
python3 - <<PY
with open("${BOUNDARY_FILE}", "w", encoding="utf-8") as f:
    f.write("a" * 65534)
    f.write("HELLO")
    f.write("b" * 1024)
PY

proxy_curl \
  -H "Content-Type: text/plain" \
  --data-binary "@${BOUNDARY_FILE}" \
  "http://127.0.0.1:${ECHO_PORT}/echo" \
  >/dev/null

sleep 1

resp_boundary=$(admin_curl -X POST "${ADMIN_BASE_URL}/api/search" \
  -H "Content-Type: application/json" \
  -d '{"keyword":"HELLO","scope":{"all":false,"request_body":true},"filters":{},"cursor":null,"limit":50}')
assert_search_has_field "request_body" "$resp_boundary" "跨 64KB block 的 request_body 关键字能命中"

echo "[INFO] 验证流式搜索接口（SSE）输出 event:progress/result/done"
STREAM_OUT="${DATA_DIR}/search_stream.out"
timeout 10 curl -sS --noproxy "*" -N \
  -H "Host: 127.0.0.1:${ADMIN_PORT}" \
  -H "Content-Type: application/json" \
  -d "{\"keyword\":\"${TOKEN}\",\"filters\":{},\"cursor\":null,\"limit\":100}" \
  "${ADMIN_BASE_URL}/api/search/stream" \
  > "${STREAM_OUT}" || true

assert_body_contains "event: progress" "$(cat "${STREAM_OUT}")" "SSE 输出 progress"
assert_body_contains "event: result" "$(cat "${STREAM_OUT}")" "SSE 输出 result"
assert_body_contains "event: done" "$(cat "${STREAM_OUT}")" "SSE 输出 done"

echo ""
echo "Results: ${PASSED_ASSERTIONS} passed, ${FAILED_ASSERTIONS} failed"
if [ "${FAILED_ASSERTIONS}" -gt 0 ]; then
  exit 1
fi
