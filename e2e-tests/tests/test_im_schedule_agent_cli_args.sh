#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

if [[ -z "${BIFROST_PORT:-}" ]]; then
  BIFROST_PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi

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
    if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
      echo "[im-schedule-agent-cli-args] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -120 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-schedule-agent-cli-args] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -120 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[im-schedule-agent-cli-args] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-schedule-agent-cli-args] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-schedule-agent-cli-args] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

"$BIFROST_BIN" -p "$BIFROST_PORT" im schedule add codex-args-human \
  --target oc_human_test \
  --cron '0 0 1 1 *' \
  --agent-prompt 'Human test only' \
  --agent-runner-id codex \
  --agent-model gpt-5 \
  --agent-profile-v2 team \
  --agent-reasoning-effort high \
  --agent-reasoning-summary auto \
  --agent-approval-policy never \
  --agent-add-dir /tmp/extra \
  --agent-config 'shell_environment_policy.inherit=all' \
  --agent-enable web_search \
  --agent-ephemeral \
  --agent-timeout-secs 180 \
  --agent-danger-full-access

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
with urllib.request.urlopen(f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/schedules") as resp:
    schedules = json.loads(resp.read().decode("utf-8"))

schedule = next((item for item in schedules if item.get("name") == "codex-args-human"), None)
assert schedule is not None, schedules
assert schedule.get("task_type") == "agent", schedule
assert schedule.get("trigger", {}).get("type") == "cron", schedule
assert schedule.get("trigger", {}).get("expr") == "0 0 1 1 *", schedule
channel = schedule.get("message_channel") or {}
assert channel.get("provider_id") == "feishu-main", schedule
assert channel.get("target_id") == "oc_human_test", schedule
assert channel.get("target_mode") == "configured_target", schedule
agent = schedule.get("agent") or {}
assert agent.get("prompt") == "Human test only", schedule
assert agent.get("runner_id") == "codex", schedule
adapter = agent.get("adapter_config") or {}
assert adapter.get("model") == "gpt-5", adapter
assert adapter.get("profileV2") == "team", adapter
assert adapter.get("reasoningEffort") == "high", adapter
assert adapter.get("reasoningSummary") == "auto", adapter
assert adapter.get("approvalPolicy") == "never", adapter
assert adapter.get("dangerFullAccess") is True, adapter
assert adapter.get("ephemeral") is True, adapter
assert adapter.get("timeoutSecs") == 180, adapter
assert adapter.get("addDirs") == ["/tmp/extra"], adapter
assert adapter.get("configOverrides") == ["shell_environment_policy.inherit=all"], adapter
assert adapter.get("enableFeatures") == ["web_search"], adapter
PY

echo "[im-schedule-agent-cli-args] PASS"
