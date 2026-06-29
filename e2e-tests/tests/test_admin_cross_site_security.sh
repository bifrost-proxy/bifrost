#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "BIFROST_BIN is not executable: $BIFROST_BIN" >&2
  echo "Build it first with: cargo build --bin bifrost" >&2
  exit 1
fi

free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-admin-security.XXXXXX")"
PROXY_PORT="$(free_port)"
PROXY_PID=""

cat >"$DATA_DIR/config.toml" <<'EOF'
[sync]
enabled = false
auto_sync = false
remote_base_url = "http://127.0.0.1:9"
probe_interval_secs = 3600
connect_timeout_ms = 100
EOF

cleanup() {
  if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
    kill "$PROXY_PID" 2>/dev/null || true
    wait "$PROXY_PID" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$PROXY_PORT" \
  --host 127.0.0.1 \
  --access-mode allow_all \
  --skip-cert-check \
  --no-system-proxy \
  --no-intercept \
  -y >/tmp/bifrost-admin-security-proxy.log 2>&1 &
PROXY_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null

csrf_token() {
  curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/security/csrf" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["csrf_token"])'
}

http_status() {
  curl -sS -o /tmp/bifrost-admin-security-response.json -w '%{http_code}' "$@"
}

assert_header_matches() {
  local headers_file="$1"
  local pattern="$2"
  local message="$3"
  if ! tr -d '\r' <"$headers_file" | grep -Eiq "$pattern"; then
    echo "[admin-cross-site-security] header assertion failed: $message" >&2
    echo "[admin-cross-site-security] expected pattern: $pattern" >&2
    echo "[admin-cross-site-security] headers:" >&2
    tr -d '\r' <"$headers_file" >&2
    exit 1
  fi
}

curl -sS -o /tmp/bifrost-admin-security-index.out \
  -D /tmp/bifrost-admin-security-index.headers \
  -H 'Accept-Encoding: gzip' \
  "http://127.0.0.1:${PROXY_PORT}/_bifrost/" >/dev/null
assert_header_matches /tmp/bifrost-admin-security-index.headers '^x-frame-options: DENY$' "admin index denies framing"
assert_header_matches /tmp/bifrost-admin-security-index.headers "^content-security-policy: .*frame-ancestors 'none'" "admin index CSP denies frame ancestors"

TOKEN="$(csrf_token)"

CREATE_BODY='{"name":"safe-rule","content":"safe.test bp://127.0.0.1:3000","enabled":false}'
STATUS="$(http_status -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" \
  -H 'Content-Type: application/json' \
  --data "$CREATE_BODY")"
[[ "$STATUS" == "200" ]]

STATUS="$(http_status -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" \
  -H 'Content-Type: application/json' \
  -H 'Origin: http://evil.example' \
  -H 'Sec-Fetch-Site: cross-site' \
  -H "X-Bifrost-CSRF: $TOKEN" \
  --data '{"name":"evil-cross-site","content":"evil.test bp://127.0.0.1:6666","enabled":true}')"
[[ "$STATUS" == "403" ]]

STATUS="$(http_status -X PUT "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/safe-rule/enable" \
  -H 'Origin: http://127.0.0.1:'"${PROXY_PORT}" \
  -H 'Sec-Fetch-Site: same-origin')"
[[ "$STATUS" == "403" ]]

STATUS="$(http_status -X PUT "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/safe-rule/enable" \
  -H 'Origin: http://127.0.0.1:'"${PROXY_PORT}" \
  -H 'Sec-Fetch-Site: same-origin' \
  -H "X-Bifrost-CSRF: $TOKEN")"
[[ "$STATUS" == "200" ]]

STATUS="$(http_status -X PUT "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/safe-rule/disable" \
  -H "Host: evil.example:${PROXY_PORT}")"
[[ "$STATUS" == "403" ]]

STATUS="$(http_status -x "http://127.0.0.1:${PROXY_PORT}" \
  -X PUT "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/safe-rule/disable")"
[[ "$STATUS" == "403" ]]

SHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share shared-security http://example.com/app \
    --content "shared-security.test bp://127.0.0.1:3000"
)"
curl -sS -o /tmp/bifrost-admin-security-share.out \
  -D /tmp/bifrost-admin-security-share.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL" >/dev/null
assert_header_matches /tmp/bifrost-admin-security-share.headers '^HTTP/.* 302' "proxied share URL redirects to local confirmation page"
assert_header_matches /tmp/bifrost-admin-security-share.headers "^location: http://127\\.0\\.0\\.1:${PROXY_PORT}/_bifrost/share/rule\\?" "share redirect targets local admin confirmation page"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-admin-security-rules-before-confirm.txt
! grep -F 'share/shared-security [enabled]' /tmp/bifrost-admin-security-rules-before-confirm.txt

CONFIRM_LOCATION="$(python3 - <<'PY'
for line in open("/tmp/bifrost-admin-security-share.headers", "rb"):
    text = line.decode("latin1").strip()
    if text.lower().startswith("location:"):
        print(text.split(":", 1)[1].strip())
        break
PY
)"

curl -sS -o /tmp/bifrost-admin-security-confirm.out \
  -D /tmp/bifrost-admin-security-confirm.headers \
  "$CONFIRM_LOCATION" >/dev/null
assert_header_matches /tmp/bifrost-admin-security-confirm.headers '^cache-control: no-store$' "confirmation page is not cached"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers '^referrer-policy: no-referrer$' "confirmation page suppresses referrer"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers '^x-content-type-options: nosniff$' "confirmation page disables content sniffing"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers '^x-frame-options: DENY$' "confirmation page denies framing"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers "^content-security-policy: .*frame-ancestors 'none'" "confirmation CSP denies frame ancestors"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers "^content-security-policy: .*connect-src 'self'" "confirmation CSP allows same-origin apply fetch"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers "^content-security-policy: .*base-uri 'none'" "confirmation CSP blocks base URI injection"
assert_header_matches /tmp/bifrost-admin-security-confirm.headers "^content-security-policy: .*form-action 'none'" "confirmation CSP blocks form posts"
grep -F 'Content hash' /tmp/bifrost-admin-security-confirm.out >/dev/null
grep -F '<button id="apply" type="button">Apply Rule</button>' /tmp/bifrost-admin-security-confirm.out >/dev/null
! grep -F 'Type the full content hash to apply' /tmp/bifrost-admin-security-confirm.out >/dev/null
! grep -F 'id="confirmation"' /tmp/bifrost-admin-security-confirm.out >/dev/null

CONFIRM_JSON="$(python3 - "$CONFIRM_LOCATION" <<'PY'
import json
import sys
import urllib.parse
query = urllib.parse.parse_qs(urllib.parse.urlparse(sys.argv[1]).query)
print(json.dumps({
    "payload": query["payload"][0],
    "target_url": query["target"][0],
}))
PY
)"

STATUS="$(http_status -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/share-confirm" \
  -H 'Content-Type: application/json' \
  -H 'Origin: http://127.0.0.1:'"${PROXY_PORT}" \
  -H 'Sec-Fetch-Site: same-origin' \
  --data "$CONFIRM_JSON")"
[[ "$STATUS" == "403" ]]

STATUS="$(http_status -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/share-confirm" \
  -H 'Content-Type: application/json' \
  -H 'Origin: http://127.0.0.1:'"${PROXY_PORT}" \
  -H 'Sec-Fetch-Site: same-origin' \
  -H "X-Bifrost-CSRF: $TOKEN" \
  --data "$CONFIRM_JSON")"
[[ "$STATUS" == "200" ]]

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-admin-security-rules-after-confirm.txt
grep -F 'share/shared-security [enabled]' /tmp/bifrost-admin-security-rules-after-confirm.txt
! grep -F 'evil-cross-site' /tmp/bifrost-admin-security-rules-after-confirm.txt

# --- CSWSH (Cross-Site WebSocket Hijacking) regression --------------------
# WebSocket upgrades are GET requests and bypass the CSRF write guard. The
# dedicated WS origin guard must reject cross-site / cross-origin upgrades for
# every admin WebSocket endpoint while allowing same-origin and native clients.
ws_upgrade_status() {
  # $1 = path, remaining args = extra "Header: value" lines
  local path="$1"
  shift
  python3 - "$PROXY_PORT" "$path" "$@" <<'PY'
import socket, sys
port = int(sys.argv[1])
path = sys.argv[2]
extra = sys.argv[3:]
lines = [
    f"GET {path} HTTP/1.1",
    f"Host: 127.0.0.1:{port}",
    "Upgrade: websocket",
    "Connection: Upgrade",
    "Sec-WebSocket-Version: 13",
    "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
]
lines.extend(extra)
request = "\r\n".join(lines) + "\r\n\r\n"
s = socket.create_connection(("127.0.0.1", port), timeout=5)
s.sendall(request.encode())
data = s.recv(1024).decode("latin1", "replace")
s.close()
status_line = data.splitlines()[0] if data else ""
parts = status_line.split()
print(parts[1] if len(parts) > 1 else "0")
PY
}

WS_ENDPOINTS=(
  "/_bifrost/api/push"
  "/_bifrost/api/replay/execute/ws"
)

for ws_path in "${WS_ENDPOINTS[@]}"; do
  # Cross-site upgrade => rejected (403).
  STATUS="$(ws_upgrade_status "$ws_path" \
    'Origin: http://evil.example.com' \
    'Sec-Fetch-Site: cross-site')"
  [[ "$STATUS" == "403" ]] || { echo "CSWSH cross-site not rejected for $ws_path (got $STATUS)" >&2; exit 1; }

  # Cross-origin upgrade without Sec-Fetch => rejected (403).
  STATUS="$(ws_upgrade_status "$ws_path" \
    'Origin: http://attacker.example.com')"
  [[ "$STATUS" == "403" ]] || { echo "CSWSH cross-origin not rejected for $ws_path (got $STATUS)" >&2; exit 1; }

  # Native client (no browser context) => not rejected by the guard.
  STATUS="$(ws_upgrade_status "$ws_path")"
  [[ "$STATUS" != "403" ]] || { echo "CSWSH guard wrongly rejected native client for $ws_path" >&2; exit 1; }

  # Same-origin upgrade => not rejected by the guard.
  STATUS="$(ws_upgrade_status "$ws_path" \
    "Origin: http://127.0.0.1:${PROXY_PORT}" \
    'Sec-Fetch-Site: same-origin')"
  [[ "$STATUS" != "403" ]] || { echo "CSWSH guard wrongly rejected same-origin for $ws_path" >&2; exit 1; }
done

echo "admin cross-site security E2E passed"
