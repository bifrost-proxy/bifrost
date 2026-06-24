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

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-rule-share.XXXXXX")"
SITE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-rule-share-site.XXXXXX")"
PROXY_PORT="$(free_port)"
TARGET_PORT="$(free_port)"
PROXY_PID=""
SITE_PID=""

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
  if [[ -n "$SITE_PID" ]] && kill -0 "$SITE_PID" 2>/dev/null; then
    kill "$SITE_PID" 2>/dev/null || true
    wait "$SITE_PID" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR" "$SITE_DIR"
}
trap cleanup EXIT

echo '<html><body>rule share target</body></html>' >"$SITE_DIR/hello"
echo '<html><body>rule share api target</body></html>' >"$SITE_DIR/from-api"

python3 -m http.server "$TARGET_PORT" --bind 127.0.0.1 --directory "$SITE_DIR" >/tmp/bifrost-rule-share-site.log 2>&1 &
SITE_PID=$!

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$PROXY_PORT" \
  --host 127.0.0.1 \
  --access-mode allow_all \
  --skip-cert-check \
  --no-system-proxy \
  --no-intercept \
  -y >/tmp/bifrost-rule-share-proxy.log 2>&1 &
PROXY_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null

header_location() {
  python3 - "$1" <<'PY'
import sys
for line in open(sys.argv[1], "rb"):
    text = line.decode("latin1").strip()
    if text.lower().startswith("location:"):
        print(text.split(":", 1)[1].strip())
        break
PY
}

assert_confirm_location() {
  local location="$1"
  [[ "$location" == "http://127.0.0.1:${PROXY_PORT}/_bifrost/share/rule?"* ]]
  [[ "$location" == *"payload="* ]]
  [[ "$location" == *"target="* ]]
  [[ "$location" != *"__bifrost_rule"* ]]
}

confirm_location() {
  local location="$1"
  python3 - "$location" "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/share-confirm" <<'PY'
import json
import base64
import sys
import urllib.parse
import urllib.request

location = sys.argv[1]
endpoint = sys.argv[2]
query = urllib.parse.parse_qs(urllib.parse.urlparse(location).query)
encoded_payload = query["payload"][0]
padding = "=" * (-len(encoded_payload) % 4)
payload = json.loads(base64.urlsafe_b64decode(encoded_payload + padding).decode())
body = json.dumps({
    "payload": encoded_payload,
    "target_url": query["target"][0],
    "confirmation": payload["content_hash"],
}).encode()
req = urllib.request.Request(
    endpoint,
    data=body,
    method="POST",
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=10) as resp:
    data = json.loads(resp.read().decode())
assert data["success"] is True
assert data["redirect_url"] == query["target"][0]
print(data["rule_name"])
PY
}

assert_rule_count() {
  local pattern="$1"
  local expected="$2"
  local file="$3"
  local count
  count="$(grep -Ec "$pattern" "$file" || true)"
  [[ "$count" == "$expected" ]]
}

TARGET_URL="http://127.0.0.1:${TARGET_PORT}/hello?site=1"
SHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-e2e "$TARGET_URL" \
    --content "share.test bp://127.0.0.1:3000"
)"
[[ "$SHARE_URL" == *"__bifrost_rule="* ]]

curl -sS -o /tmp/bifrost-rule-share-body.out \
  -D /tmp/bifrost-rule-share.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL" >/dev/null
grep -Eiq '^HTTP/.* 302' /tmp/bifrost-rule-share.headers
CONFIRM_URL="$(header_location /tmp/bifrost-rule-share.headers)"
assert_confirm_location "$CONFIRM_URL"

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-before-confirm.txt
! grep -F 'share/rsq-e2e [enabled]' /tmp/bifrost-rule-share-list-before-confirm.txt

CONFIRM_PAGE="$(curl -fsS "$CONFIRM_URL")"
[[ "$CONFIRM_PAGE" == *"Apply Shared Bifrost Rule"* ]]
[[ "$CONFIRM_PAGE" == *"rsq-e2e"* ]]
[[ "$CONFIRM_PAGE" == *"share.test bp://127.0.0.1:3000"* ]]

CONFIRMED_RULE="$(confirm_location "$CONFIRM_URL")"
[[ "$CONFIRMED_RULE" == "share/rsq-e2e" ]]
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-1.txt
grep -F 'share/rsq-e2e [enabled]' /tmp/bifrost-rule-share-list-1.txt

CONFIRMED_RULE_REPEAT="$(confirm_location "$CONFIRM_URL")"
[[ "$CONFIRMED_RULE_REPEAT" == "share/rsq-e2e" ]]
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-2.txt
assert_rule_count '^  share/rsq-e2e( [0-9]+)? \[' 1 /tmp/bifrost-rule-share-list-2.txt

SHARE_URL_2="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-e2e "$TARGET_URL" \
    --content "other.test host://127.0.0.1:3000"
)"
curl -sS -o /tmp/bifrost-rule-share-second.out \
  -D /tmp/bifrost-rule-share-second.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL_2" >/dev/null
CONFIRM_URL_2="$(header_location /tmp/bifrost-rule-share-second.headers)"
assert_confirm_location "$CONFIRM_URL_2"
CONFIRMED_RULE_2="$(confirm_location "$CONFIRM_URL_2")"
[[ "$CONFIRMED_RULE_2" == "share/rsq-e2e 2" ]]
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-3.txt
grep -F 'share/rsq-e2e [disabled]' /tmp/bifrost-rule-share-list-3.txt
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-3.txt

RESHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share "share/rsq-e2e 2" "$TARGET_URL"
)"
[[ "$RESHARE_URL" == *"__bifrost_rule="* ]]
curl -sS -o /tmp/bifrost-rule-share-reshare.out \
  -D /tmp/bifrost-rule-share-reshare.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$RESHARE_URL" >/dev/null
CONFIRM_URL_RESHARE="$(header_location /tmp/bifrost-rule-share-reshare.headers)"
assert_confirm_location "$CONFIRM_URL_RESHARE"
CONFIRMED_RESHARE="$(confirm_location "$CONFIRM_URL_RESHARE")"
[[ "$CONFIRMED_RESHARE" == "share/rsq-e2e 2" ]]
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-4.txt
assert_rule_count '^  share/rsq-e2e( [0-9]+)? \[' 2 /tmp/bifrost-rule-share-list-4.txt
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-4.txt

API_BARE_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/share-link" \
    -H 'Content-Type: application/json' \
    --data "{\"name\":\"share/rsq-e2e 2\",\"target_url\":\"a.com\"}"
)"
python3 - "$API_BARE_RESP" <<'PY'
import json
import sys
resp = json.loads(sys.argv[1])
assert resp["rule_name"] == "rsq-e2e"
assert resp["url"].startswith("http://a.com/")
assert "__bifrost_rule=" in resp["url"]
PY

API_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules/share-link" \
    -H 'Content-Type: application/json' \
    --data "{\"name\":\"share/rsq-e2e 2\",\"target_url\":\"http://127.0.0.1:${TARGET_PORT}/from-api\"}"
)"
python3 - "$API_RESP" <<'PY'
import json
import sys
resp = json.loads(sys.argv[1])
assert resp["query_param"] == "__bifrost_rule"
assert resp["payload_version"] == 1
assert resp["rule_name"] == "rsq-e2e"
assert "__bifrost_rule=" in resp["url"]
PY

echo "rule share confirmation E2E passed"
