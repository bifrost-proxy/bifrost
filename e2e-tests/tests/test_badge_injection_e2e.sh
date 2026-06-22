#!/bin/bash

set -uo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$PROJECT_DIR/e2e-tests/test_utils/assert.sh"
source "$PROJECT_DIR/e2e-tests/test_utils/http_client.sh"

pick_free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-}"
HTML_PORT="${HTML_PORT:-}"

if [[ -z "$PROXY_PORT" ]]; then
  PROXY_PORT="$(pick_free_port)"
fi

if [[ -z "$HTML_PORT" ]]; then
  while true; do
    HTML_PORT="$(pick_free_port)"
    if [[ "$HTML_PORT" != "$PROXY_PORT" ]]; then
      break
    fi
  done
fi

export PROXY_HOST PROXY_PORT HTML_PORT
export TEST_ID="${TEST_ID:-}"


BIFROST_DATA_DIR_BASE="${PROJECT_DIR}/.bifrost-e2e-badge"
HTML_DIR="${PROJECT_DIR}/e2e-tests/test_data/badge_injection"

BIFROST_TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_DIR}/target}"
BIFROST_BIN="${BIFROST_TARGET_DIR}/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PID=""
HTML_PID=""

terminate_pid() {
  local pid="$1"
  local label="$2"

  if [[ -z "$pid" ]]; then
    return 0
  fi

  if ! kill -0 "$pid" >/dev/null 2>&1; then
    wait "$pid" >/dev/null 2>&1 || true
    return 0
  fi

  kill "$pid" >/dev/null 2>&1 || true
  for _ in $(seq 1 50); do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      wait "$pid" >/dev/null 2>&1 || true
      return 0
    fi
    sleep 0.1
  done

  echo "[WARN] ${label} did not stop after SIGTERM; sending SIGKILL" >&2
  kill -9 "$pid" >/dev/null 2>&1 || true
  wait "$pid" >/dev/null 2>&1 || true
}

cleanup() {
  if [[ -n "${PROXY_PID}" ]]; then
    terminate_pid "${PROXY_PID}" "bifrost proxy"
  fi
  if [[ -n "${HTML_PID}" ]]; then
    terminate_pid "${HTML_PID}" "html server"
  fi
}
trap cleanup EXIT

build_bifrost() {
  if [[ -f "$BIFROST_BIN" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
    echo "[INFO] Skipping build (SKIP_BUILD=true), using existing binary: $BIFROST_BIN" >&2
    echo "$BIFROST_BIN"
    return 0
  fi

  echo "[INFO] Building bifrost binary..." >&2
  cargo build --release --bin bifrost >/dev/null

  if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "[FAIL] bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
  fi

  echo "$BIFROST_BIN"
}

start_html_server() {
  echo "[INFO] Starting html server on :${HTML_PORT}"
  python3 - "$HTML_PORT" "$HTML_DIR" <<'PY' >/dev/null 2>&1 &
import http.server
import pathlib
import sys

port = int(sys.argv[1])
html_dir = pathlib.Path(sys.argv[2])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/mislabeled-json":
            body = b'{"code":200,"data":{"mode":"slide","challenge_code":99999}}'
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        if self.path in ("/", "/index.html"):
            body = (html_dir / "index.html").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        self.send_error(404)

    def log_message(self, format, *args):
        return

server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
  HTML_PID=$!

  for _ in $(seq 1 30); do
    if env NO_PROXY="*" no_proxy="*" curl -sS "http://127.0.0.1:${HTML_PORT}/index.html" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done

  echo "[FAIL] html server not ready" >&2
  exit 1
}

start_proxy() {
  local data_dir="$1"
  shift

  rm -rf "$data_dir" || true
  mkdir -p "$data_dir"

  local log_file="$data_dir/bifrost.log"

  echo "[INFO] Starting bifrost proxy on ${PROXY_HOST}:${PROXY_PORT} (data_dir=${data_dir})"
  BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" start -p "$PROXY_PORT" --skip-cert-check --unsafe-ssl --no-system-proxy "$@" >"$log_file" 2>&1 &
  PROXY_PID=$!

  for _ in $(seq 1 120); do
    if env NO_PROXY="*" no_proxy="*" curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$PROXY_PID" >/dev/null 2>&1; then
      echo "[FAIL] bifrost process exited early" >&2
      tail -n 80 "$log_file" >&2 || true
      exit 1
    fi
    sleep 0.2
  done

  echo "[FAIL] bifrost proxy not ready" >&2
  tail -n 80 "$log_file" >&2 || true
  exit 1
}

stop_proxy() {
  if [[ -n "${PROXY_PID}" ]]; then
    terminate_pid "${PROXY_PID}" "bifrost proxy"
    PROXY_PID=""
  fi
}

create_vconsole_rule() {
  local payload_file
  payload_file="$(mktemp)"
  python3 - "$payload_file" <<'PY'
import json
import sys

payload = {
    "name": "badge-vconsole-escaping-regression",
    "content": "\n".join([
        "not-current-test.local htmlAppend://{vconsole-inject}",
        "``` vconsole-inject",
        '<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>',
        "<script>new VConsole();</script>",
        "```",
    ]),
    "enabled": True,
}

with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(payload, f)
PY

  local status
  status="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    -X POST "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules" \
    -H "Content-Type: application/json" \
    --data-binary "@${payload_file}")"
  rm -f "$payload_file"

  assert_status "200" "$status" "vConsole regression rule should be created"
}

create_rule_api() {
  local name="$1"
  local content="$2"
  local enabled="$3"
  local payload_file
  payload_file="$(mktemp)"
  python3 - "$payload_file" "$name" "$content" "$enabled" <<'PY'
import json
import sys

_, path, name, content, enabled = sys.argv
payload = {
    "name": name,
    "content": content,
    "enabled": enabled.lower() == "true",
}
with open(path, "w", encoding="utf-8") as f:
    json.dump(payload, f)
PY

  local status
  status="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    -X POST "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules" \
    -H "Content-Type: application/json" \
    --data-binary "@${payload_file}")"
  rm -f "$payload_file"

  assert_status "200" "$status" "Rule '${name}' should be created"
}

rule_enabled_state() {
  local name="$1"
  env NO_PROXY="*" no_proxy="*" curl -sS \
    "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules/${name}" \
    | python3 -c 'import json,sys; print(str(json.load(sys.stdin)["enabled"]).lower())'
}

create_share_link_for_rule() {
  local name="$1"
  local target_url="$2"
  local payload_file
  payload_file="$(mktemp)"
  python3 - "$payload_file" "$name" "$target_url" <<'PY'
import json
import sys

_, path, name, target_url = sys.argv
with open(path, "w", encoding="utf-8") as f:
    json.dump({"name": name, "target_url": target_url}, f)
PY

  local response
  response="$(env NO_PROXY="*" no_proxy="*" curl -sS \
    -X POST "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules/share-link" \
    -H "Content-Type: application/json" \
    --data-binary "@${payload_file}")"
  rm -f "$payload_file"

  python3 -c 'import json,sys; print(json.load(sys.stdin)["url"])' <<<"$response"
}

fetch_via_proxy_follow_headers() {
  local url="$1"
  local headers_file
  headers_file="$(mktemp)"
  local body_file
  body_file="$(mktemp)"

  HTTP_STATUS="$(NO_PROXY="" no_proxy="" curl -sS --max-time "$(http_timeout)" --proxy "http://${PROXY_HOST}:${PROXY_PORT}" -D "$headers_file" -o "$body_file" -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
  HTTP_HEADERS="$(cat "$headers_file" | tr -d '\r')"
  HTTP_BODY="$(cat "$body_file")"

  rm -f "$headers_file" "$body_file"
}

fetch_via_proxy() {
  local url="$1"
  local headers_file
  headers_file="$(mktemp)"
  local body_file
  body_file="$(mktemp)"

  HTTP_STATUS="$(NO_PROXY="" no_proxy="" curl -sS --max-time "$(http_timeout)" --proxy "http://${PROXY_HOST}:${PROXY_PORT}" -D "$headers_file" -o "$body_file" -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
  HTTP_HEADERS="$(cat "$headers_file" | tr -d '\r')"
  HTTP_BODY="$(cat "$body_file")"

  rm -f "$headers_file" "$body_file"
}

assert_badge_injection() {
  local expected="$1"
  local url="http://127.0.0.1:${HTML_PORT}/index.html"

  fetch_via_proxy "$url"
  assert_status "200" "$HTTP_STATUS" "HTML request should succeed"

  if [[ "$expected" == "present" ]]; then
    assert_body_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should be injected"
    assert_body_contains "__bb_copy" "$HTTP_BODY" "Merged rules copy button should be injected"
    assert_body_contains "Copy merged rules" "$HTTP_BODY" "Copy button should expose accessible label"
    assert_body_contains "navigator.clipboard" "$HTTP_BODY" "Copy button should use Clipboard API when available"
    assert_body_contains "document.execCommand('copy')" "$HTTP_BODY" "Copy button should include execCommand fallback"
    assert_body_contains "clipboardData.setData('text/plain',text)" "$HTTP_BODY" "Fallback copy should write text via copy event clipboardData"
    assert_body_contains "ok&&copied?resolve()" "$HTTP_BODY" "Fallback copy should only report success after copy event writes data"
    assert_body_contains "z-index:2147483647!important" "$HTTP_BODY" "Badge panel should use top z-index"
  else
    assert_body_not_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should not be injected"
    assert_body_not_contains "__bb_copy" "$HTTP_BODY" "Merged rules copy button should not be injected"
  fi
}

assert_mislabeled_json_not_injected() {
  local url="http://127.0.0.1:${HTML_PORT}/mislabeled-json"

  fetch_via_proxy "$url"
  assert_status "200" "$HTTP_STATUS" "Mislabeled JSON request should succeed"
  assert_body_contains '"code":200' "$HTTP_BODY" "Mislabeled JSON body should be preserved"
  assert_body_not_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should not be injected into JSON body"
  assert_body_not_contains "__bb_copy" "$HTTP_BODY" "Badge panel should not be injected into JSON body"
}

assert_vconsole_rule_is_not_promoted_to_page_script() {
  local url="http://127.0.0.1:${HTML_PORT}/index.html"

  create_vconsole_rule || return 1
  fetch_via_proxy "$url"
  assert_status "200" "$HTTP_STATUS" "HTML request with vConsole rule in badge data should succeed"

  assert_body_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should still be injected"
  assert_body_contains "not-current-test.local htmlAppend://{vconsole-inject}" "$HTTP_BODY" "Merged rules should still display the vConsole rule text"
  assert_body_contains "\\u003C/script\\u003E" "$HTTP_BODY" "Inline badge data should escape HTML tag delimiters"
  assert_body_not_contains "<script src=\\\"https://unpkg.com/vconsole/dist/vconsole.min.js\\\"" "$HTTP_BODY" "vConsole opening script tag should not appear as raw HTML in badge data"
  assert_body_not_contains "</script>\n<script>new VConsole();</script>" "$HTTP_BODY" "vConsole script should not escape from badge inline data"
}

assert_share_env_exit_restores_rules() {
  local clean_url="http://127.0.0.1:${HTML_PORT}/index.html"

  create_rule_api "before-enabled" "before-enabled.test statusCode://201" "true" || return 1
  create_rule_api "before-disabled" "before-disabled.test statusCode://202" "false" || return 1
  create_rule_api "share-source" "share-source.test statusCode://204" "false" || return 1

  local share_url
  share_url="$(create_share_link_for_rule "share-source" "$clean_url")"
  assert_body_contains "__bifrost_rule" "$share_url" "Share link should include rule share query" || return 1

  fetch_via_proxy_follow_headers "$share_url"
  assert_status "302" "$HTTP_STATUS" "Share link GET should redirect to clean URL" || return 1
  assert_body_not_contains "__bifrost_rule" "$HTTP_HEADERS" "Redirect location should remove share query" || return 1

  local status_json
  status_json="$(env NO_PROXY="*" no_proxy="*" curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules/share-env/status")"
  assert_body_contains '"active":true' "$status_json" "Share env status should be active after import" || return 1
  assert_body_contains '"requested_name":"share-source"' "$status_json" "Share env should record requested rule name" || return 1

  fetch_via_proxy "$clean_url"
  assert_status "200" "$HTTP_STATUS" "Clean page request should succeed after share import" || return 1
  assert_body_contains "__bb_share_dot" "$HTTP_BODY" "Injected badge should include Share environment red dot" || return 1
  assert_body_contains "__bb_share_pulse" "$HTTP_BODY" "Injected badge should include Share environment pulse glow" || return 1
  assert_body_contains "syncShareState();" "$HTTP_BODY" "Injected badge should apply Share state before hover" || return 1
  assert_body_contains "__bb_share_status_dot" "$HTTP_BODY" "Injected badge panel should include Share status dot" || return 1
  assert_body_contains "Share preview active" "$HTTP_BODY" "Injected badge panel should show concise Share preview text" || return 1
  assert_body_contains '"requested_name":"share-source"' "$HTTP_BODY" "Badge inline data should identify active share env" || return 1
  assert_body_contains "/rules/share-env/exit" "$HTTP_BODY" "Badge panel should include share exit API" || return 1
  assert_body_contains "mode:'no-cors'" "$HTTP_BODY" "Badge exit should include no-cors fallback for cross-origin pages" || return 1

  local before_enabled
  before_enabled="$(rule_enabled_state "before-enabled")"
  assert_body_equals "false" "$before_enabled" "Before-enabled rule should be disabled inside share env" || return 1
  local imported_enabled
  imported_enabled="$(rule_enabled_state "share/share-source")"
  assert_body_equals "true" "$imported_enabled" "Imported share rule should be enabled inside share env" || return 1

  local exit_response
  exit_response="$(env NO_PROXY="*" no_proxy="*" curl -sS \
    -X POST "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules/share-env/exit")"
  assert_body_contains '"was_active":true' "$exit_response" "Exit API should report active share env" || return 1
  assert_body_contains '"before-enabled"' "$exit_response" "Exit API should report restored rule" || return 1

  before_enabled="$(rule_enabled_state "before-enabled")"
  assert_body_equals "true" "$before_enabled" "Before-enabled rule should be restored after exit" || return 1
  imported_enabled="$(rule_enabled_state "share/share-source")"
  assert_body_equals "false" "$imported_enabled" "Imported share rule should be disabled after exit" || return 1
  local before_disabled
  before_disabled="$(rule_enabled_state "before-disabled")"
  assert_body_equals "false" "$before_disabled" "Previously disabled rule should remain disabled after exit" || return 1

  status_json="$(env NO_PROXY="*" no_proxy="*" curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/rules/share-env/status")"
  assert_body_contains '"active":false' "$status_json" "Share env status should be inactive after exit" || return 1
}

BIFROST_BIN="$(build_bifrost)"
start_html_server

echo "[INFO] Case 1: --disable-badge-injection"
start_proxy "${BIFROST_DATA_DIR_BASE}-disabled" --disable-badge-injection
assert_badge_injection "absent" || { print_test_summary || true; exit 1; }
stop_proxy

echo "[INFO] Case 2: --enable-badge-injection"
start_proxy "${BIFROST_DATA_DIR_BASE}-enabled" --enable-badge-injection
assert_badge_injection "present" || { print_test_summary || true; exit 1; }
stop_proxy

echo "[INFO] Case 3: --enable-badge-injection skips mislabeled JSON"
start_proxy "${BIFROST_DATA_DIR_BASE}-mislabeled-json" --enable-badge-injection
assert_mislabeled_json_not_injected || { print_test_summary || true; exit 1; }
stop_proxy

echo "[INFO] Case 4: badge merged rules escape </script> in vConsole htmlAppend values"
start_proxy "${BIFROST_DATA_DIR_BASE}-escape" --enable-badge-injection
assert_vconsole_rule_is_not_promoted_to_page_script || { print_test_summary || true; exit 1; }
stop_proxy

echo "[INFO] Case 5: share env badge exit restores previous enabled rules"
start_proxy "${BIFROST_DATA_DIR_BASE}-share-env" --enable-badge-injection
assert_share_env_exit_restores_rules || { print_test_summary || true; exit 1; }
stop_proxy

print_test_summary || exit 1
echo "[PASS] Badge injection E2E finished"
