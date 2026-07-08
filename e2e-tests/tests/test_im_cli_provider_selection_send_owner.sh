#!/usr/bin/env bash
set -eo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_PORT:-18891}"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
  BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DIR="$(mktemp -d)"
DATA_DIR="$TEST_DIR/data"
SERVER_LOG="$TEST_DIR/bifrost.log"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$DATA_DIR" "$TEST_DIR"
}
trap cleanup EXIT

rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

if [[ ! -x "$BIFROST_BIN" ]]; then
  cargo build --bin bifrost
  BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
fi

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  --skip-cert-check \
  >"$SERVER_LOG" 2>&1 &
BIFROST_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

BASE_URL_ERR="$(
  "$BIFROST_BIN" -p "$PORT" im provider add invalid-feishu \
    --type feishu \
    --app-id cli_app \
    --secret cli_secret \
    --base-url "http://127.0.0.1:1" \
    --runner Traex \
    2>&1 || true
)"
grep -q "base_url is managed by system and cannot be set via CLI" <<<"$BASE_URL_ERR"

NO_RUNNER_ERR="$(
  "$BIFROST_BIN" -p "$PORT" im provider add missing-runner \
    --type feishu \
    --app-id cli_app \
    --secret cli_secret \
    2>&1 || true
)"
grep -q -- "--runner is required when stdin is not interactive" <<<"$NO_RUNNER_ERR"
grep -q "Default built-in runners include" <<<"$NO_RUNNER_ERR"

"$BIFROST_BIN" -p "$PORT" im provider add feishu-main \
  --type feishu \
  --app-id cli_app \
  --secret cli_secret \
  --display-name "Feishu Main" \
  --owner-open-id owner-open-id \
  --enabled true \
  --runner trae

PROVIDERS_JSON="$(curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers")"
python3 - "$PROVIDERS_JSON" <<'PY'
import json
import sys

providers = json.loads(sys.argv[1])
provider = next((item for item in providers if item.get("id") == "feishu-main"), None)
assert provider is not None, providers
assert provider["provider_type"] == "feishu", provider
assert provider["display_name"] == "Feishu Main", provider
assert provider["enabled"] is True, provider
assert provider["base_url"] == "https://open.feishu.cn/open-apis", provider
assert provider["app_id"] == "cli_app", provider
assert provider["secret_configured"] is True, provider
assert provider["owner_open_id"] == "owner-open-id", provider
assert provider["agent_config"]["runner"] == "Traex", provider
PY

"$BIFROST_BIN" -p "$PORT" im target add owner-alias \
  --receive-id-type open_id \
  --receive-id owner-open-id \
  --display-name "Owner Alias"

TARGETS_JSON="$(curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/targets")"
python3 - "$TARGETS_JSON" <<'PY'
import json
import sys

targets = json.loads(sys.argv[1])
target = next((item for item in targets if item.get("id") == "owner-alias"), None)
assert target is not None, targets
assert target["provider_id"] == "feishu-main", target
assert target["receive_id_type"] == "open_id", target
assert target["receive_id"] == "owner-open-id", target
assert target["display_name"] == "Owner Alias", target
PY

echo "[im-cli-provider-selection] passed"
