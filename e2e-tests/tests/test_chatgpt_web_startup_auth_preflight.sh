#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${BIFROST_CHATGPT_WEB_STARTUP_E2E_PORT:-18947}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-chatgpt-web-startup.XXXXXX")"
LOG_FILE="$DATA_DIR/bifrost.log"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

fail() {
  echo "[chatgpt-web-startup-auth] error: $*" >&2
  if [[ -f "$LOG_FILE" ]]; then
    tail -n 160 "$LOG_FILE" >&2 || true
  fi
  exit 1
}

wait_http() {
  local url="$1"
  for _ in {1..80}; do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  fail "timed out waiting for $url"
}

wait_log() {
  local pattern="$1"
  for _ in {1..80}; do
    if grep -q "$pattern" "$LOG_FILE" 2>/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  fail "timed out waiting for log pattern: $pattern"
}

mkdir -p "$DATA_DIR/admin"
cat >"$DATA_DIR/admin/im_gateway_external_cli_agent.json" <<JSON
{
  "version": 1,
  "defaultRunnerId": "codex",
  "runners": {
    "codex": {
      "enabled": false,
      "adapter": "codex"
    },
    "web": {
      "enabled": true,
      "adapter": "chatgpt_web",
      "adapterConfig": {
        "auth": {
          "statePath": "startup-auth-e2e.json"
        },
        "browser": {
          "profileDir": "startup-auth-profile"
        }
      }
    }
  },
  "channels": {}
}
JSON

echo "[chatgpt-web-startup-auth] build bifrost"
cargo build --bin bifrost >/dev/null

echo "[chatgpt-web-startup-auth] start bifrost on ${PORT}"
BIFROST_DATA_DIR="$DATA_DIR" BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN=1 \
  "$ROOT_DIR/target/debug/bifrost" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$LOG_FILE" 2>&1 &
PID="$!"

wait_http "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address"
wait_log "would open login browser"
wait_log "chatgpt_web startup auth: login still required"

AUTH_JSON="$DATA_DIR/auth-status.json"
curl -fsS \
  "http://127.0.0.1:${PORT}/_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/status?runnerId=web" \
  >"$AUTH_JSON"
python3 - "$AUTH_JSON" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
assert data["state"] == "logged_out", data
assert data["loggedIn"] is False, data
assert data["statePath"].endswith("startup-auth-e2e.json"), data
PY

echo "[chatgpt-web-startup-auth] PASS"
