#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18937}}"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 120); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[weixin-provider] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -100 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[weixin-provider] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[weixin-provider] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[weixin-provider] bifrost binary is not executable: $BIFROST_BIN" >&2
  exit 1
fi

echo "[weixin-provider] starting bifrost on $BIFROST_PORT with data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

IM_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway"

BASE_URL_ERR="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-bad \
  --type weixin \
  --base-url http://127.0.0.1:9 \
  --runner traex 2>&1 || true)"
grep -q "base_url is managed by system and cannot be set via CLI" <<<"$BASE_URL_ERR"

RUNNER_ERR="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-no-runner \
  --type weixin \
  --app-id mock-bot \
  --secret mock-token 2>&1 || true)"
grep -q -- "--runner is required when stdin is not interactive" <<<"$RUNNER_ERR"
grep -q "Default built-in runners include" <<<"$RUNNER_ERR"

"$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-cli \
  --type weixin \
  --app-id mock-bot@im.bot \
  --secret mock-token \
  --owner-open-id owner@im.wechat \
  --enable-long-connection false \
  --runner claude-code >/dev/null

PROVIDER_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-cli")"
python3 - "$PROVIDER_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["provider_type"] == "weixin", provider
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["app_id"] == "mock-bot@im.bot", provider
assert provider["owner_open_id"] == "owner@im.wechat", provider
assert provider["secret_configured"] is True, provider
assert "secret_ref" not in provider, provider
assert provider["event_connection_enabled"] is False, provider
assert provider["agent_config"]["runner"] == "Claude-Code", provider
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "weixin-admin",
    "provider_type": "weixin",
    "display_name": "Weixin Admin",
    "enabled": true,
    "base_url": "http://127.0.0.1:12345",
    "app_secret": "admin-token",
    "event_connection_enabled": false,
    "agent_config": {"runner": "traex"}
  }' >/dev/null

curl -fsS --noproxy '*' -X PATCH "$IM_BASE/providers/weixin-admin" \
  -H 'Content-Type: application/json' \
  -d '{"base_url":"https://evil.example","event_connection_enabled":false}' >/dev/null

ADMIN_PROVIDER_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-admin")"
python3 - "$ADMIN_PROVIDER_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["secret_configured"] is True, provider
assert "secret_ref" not in provider, provider
assert provider["event_connection_enabled"] is False, provider
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "weixin-unconfigured",
    "provider_type": "weixin",
    "display_name": "Weixin Unconfigured",
    "enabled": true,
    "base_url": "https://evil.example",
    "event_connection_enabled": true,
    "agent_config": {"runner": "traex"}
  }' >/dev/null

UNCONFIGURED_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-unconfigured")"
python3 - "$UNCONFIGURED_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["secret_configured"] is False, provider
assert "secret_ref" not in provider, provider
PY

CONNECT_BODY="$(curl -sS --noproxy '*' -w '\n%{http_code}' -X POST "$IM_BASE/providers/weixin-unconfigured/connect" || true)"
CONNECT_CODE="$(tail -n 1 <<<"$CONNECT_BODY")"
CONNECT_JSON="$(sed '$d' <<<"$CONNECT_BODY")"
if [[ "$CONNECT_CODE" == "200" ]]; then
  echo "[weixin-provider] connect unexpectedly succeeded without completed QR login" >&2
  exit 1
fi
python3 - "$CONNECT_JSON" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
message = payload.get("error") or payload.get("message") or ""
assert "QR login" in message or "bot token" in message or "secret configured" in message, payload
PY

STATUS_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-unconfigured/status")"
python3 - "$STATUS_JSON" <<'PY'
import json
import sys

status = json.loads(sys.argv[1])
assert status.get("state") in {"failed", "disconnected"}, status
if status.get("state") == "failed":
    last_error = status.get("last_error") or ""
    assert "QR login" in last_error or "bot token" in last_error or "secret configured" in last_error, status
PY

echo "[weixin-provider] PASS"
