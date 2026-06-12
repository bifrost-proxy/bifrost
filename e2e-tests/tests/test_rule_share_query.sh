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

TARGET_URL="http://127.0.0.1:${TARGET_PORT}/hello?site=1"
SHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-e2e "$TARGET_URL" \
    --content "share.test bp://127.0.0.1:3000"
)"
[[ "$SHARE_URL" == *"__bifrost_rule="* ]]

BARE_DOMAIN_SHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-e2e-bare a.com \
    --content "bare.test direct"
)"
[[ "$BARE_DOMAIN_SHARE_URL" == https://a.com/* ]]
[[ "$BARE_DOMAIN_SHARE_URL" == *"__bifrost_rule="* ]]

HEADERS="$(
  curl -sS -o /tmp/bifrost-rule-share-body.out -D - -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL"
)"
echo "$HEADERS" | grep -Eiq '^HTTP/.* 302'
echo "$HEADERS" | grep -Eiq "^location: http://127\\.0\\.0\\.1:${TARGET_PORT}/hello\\?site=1"

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-1.txt
grep -F 'share/rsq-e2e [enabled]' /tmp/bifrost-rule-share-list-1.txt

curl -sS -o /tmp/bifrost-rule-share-repeat.out -D /tmp/bifrost-rule-share-repeat.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-2.txt
COUNT="$(grep -Ec '^  share/rsq-e2e( [0-9]+)? \[' /tmp/bifrost-rule-share-list-2.txt || true)"
[[ "$COUNT" == "1" ]]

SHARE_URL_2="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share rsq-e2e "$TARGET_URL" \
    --content "other.test host://127.0.0.1:3000"
)"
curl -sS -o /tmp/bifrost-rule-share-second.out -D /tmp/bifrost-rule-share-second.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL_2" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-3.txt
grep -F 'share/rsq-e2e [disabled]' /tmp/bifrost-rule-share-list-3.txt
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-3.txt

curl -sS -o /tmp/bifrost-rule-share-second-repeat.out \
  -D /tmp/bifrost-rule-share-second-repeat.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$SHARE_URL_2" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-4.txt
COUNT_AFTER_SUFFIX="$(grep -Ec '^  share/rsq-e2e( [0-9]+)? \[' /tmp/bifrost-rule-share-list-4.txt || true)"
[[ "$COUNT_AFTER_SUFFIX" == "2" ]]
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-4.txt

RESHARE_URL="$(
  BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule share "share/rsq-e2e 2" "$TARGET_URL"
)"
[[ "$RESHARE_URL" == *"__bifrost_rule="* ]]
curl -sS -o /tmp/bifrost-rule-share-reshare.out \
  -D /tmp/bifrost-rule-share-reshare.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$RESHARE_URL" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-5.txt
COUNT_AFTER_RESHARE="$(grep -Ec '^  share/rsq-e2e( [0-9]+)? \[' /tmp/bifrost-rule-share-list-5.txt || true)"
[[ "$COUNT_AFTER_RESHARE" == "2" ]]
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-5.txt

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
assert resp["url"].startswith("https://a.com/")
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

API_URL="$(python3 - "$API_RESP" <<'PY'
import json
import sys
print(json.loads(sys.argv[1])["url"])
PY
)"
curl -sS -o /tmp/bifrost-rule-share-api-import.out \
  -D /tmp/bifrost-rule-share-api-import.headers \
  -x "http://127.0.0.1:${PROXY_PORT}" "$API_URL" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" rule list > /tmp/bifrost-rule-share-list-6.txt
COUNT_AFTER_API_RESHARE="$(grep -Ec '^  share/rsq-e2e( [0-9]+)? \[' /tmp/bifrost-rule-share-list-6.txt || true)"
[[ "$COUNT_AFTER_API_RESHARE" == "2" ]]
grep -F 'share/rsq-e2e 2 [enabled]' /tmp/bifrost-rule-share-list-6.txt

echo "rule share query e2e passed"
