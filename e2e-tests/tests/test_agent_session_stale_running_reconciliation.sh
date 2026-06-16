#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

pick_port() {
  python3 - <<'PY'
import socket
with socket.socket() as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

BIFROST_PORT="${BIFROST_PORT:-$(pick_port)}"
TEST_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$TEST_DIR/data"
BIFROST_LOG="$TEST_DIR/bifrost.log"
SESSIONS_RESPONSE="$TEST_DIR/sessions.json"
STOP_RESPONSE="$TEST_DIR/stop.json"
FINAL_SESSIONS_RESPONSE="$TEST_DIR/final-sessions.json"
BIFROST_BIN="${BIFROST_BIN:-}"
STALE_SESSION_KEY="stale-running-session"
CODEX_SESSION_KEY="external-codex-running-session"
CHATGPT_WEB_SESSION_KEY="external-chatgpt-web-running-session"
CUSTOM_CLI_SESSION_KEY="external-custom-cli-running-session"

cleanup() {
  local exit_code="$?"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-session-stale-running-reconciliation] DEBUG temp dir: $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 160 "$BIFROST_LOG" >&2 || true
    [[ -f "$SESSIONS_RESPONSE" ]] && cat "$SESSIONS_RESPONSE" >&2 || true
    [[ -f "$STOP_RESPONSE" ]] && cat "$STOP_RESPONSE" >&2 || true
    [[ -f "$FINAL_SESSIONS_RESPONSE" ]] && cat "$FINAL_SESSIONS_RESPONSE" >&2 || true
    [[ -f "$BIFROST_DATA_DIR/agent/im_gateway/session_state.json" ]] \
      && cat "$BIFROST_DATA_DIR/agent/im_gateway/session_state.json" >&2 || true
  fi
  [[ -n "${BIFROST_PID:-}" ]] && kill "$BIFROST_PID" >/dev/null 2>&1 || true
  [[ -n "${BIFROST_PID:-}" ]] && wait "$BIFROST_PID" >/dev/null 2>&1 || true
  rm -rf "$TEST_DIR"
  return "$exit_code"
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
  echo "[agent-session-stale-running-reconciliation] $label did not become ready" >&2
  return 1
}

resolve_bifrost_bin() {
  local preferred="${1:-}"
  local candidates=()
  [[ -n "$preferred" ]] && candidates+=("$preferred")
  candidates+=("$REPO_DIR/target/release/bifrost" "$REPO_DIR/target/debug/bifrost")
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

prepare_fixture() {
  python3 - "$BIFROST_DATA_DIR" "$STALE_SESSION_KEY" "$CODEX_SESSION_KEY" "$CHATGPT_WEB_SESSION_KEY" "$CUSTOM_CLI_SESSION_KEY" <<'PY'
import json
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
stale_key = sys.argv[2]
codex_key = sys.argv[3]
chatgpt_web_key = sys.argv[4]
custom_cli_key = sys.argv[5]
history_dir = data_dir / "agent" / "sessions" / "2026" / "05" / "14"
state_dir = data_dir / "agent" / "im_gateway"
history_dir.mkdir(parents=True, exist_ok=True)
state_dir.mkdir(parents=True, exist_ok=True)
history_path = history_dir / "session-stale-running-session-1778759968.jsonl"
events = [
    {
        "timestamp": 100,
        "event_type": "session_start",
        "session_key": stale_key,
        "content": {"source": "admin-api", "work_dir": "/tmp/stale-running"},
    },
    {
        "timestamp": 101,
        "event_type": "user_message",
        "session_key": stale_key,
        "content": {"message": "stale running user"},
    },
    {
        "timestamp": 102,
        "event_type": "assistant_message",
        "session_key": stale_key,
        "content": {"message": "stale running answer"},
    },
    {
        "timestamp": 103,
        "event_type": "run_state_changed",
        "session_key": stale_key,
        "content": {"state": "completed", "source_channel": "api", "agent_kind": "builtin"},
    },
]
history_path.write_text(
    "\n".join(json.dumps(event, ensure_ascii=False) for event in events) + "\n",
    encoding="utf-8",
)
store = {
    "version": 1,
    "writeSeq": 2,
    "sessions": {
        f"{stale_key}::bifrost_agent::": {
            "sessionKey": stale_key,
            "adapter": "bifrost_agent",
            "historyPath": str(history_path),
            "workDir": "/tmp/stale-running",
            "title": "Stale Running",
            "lastUserMessage": "stale running user",
            "status": "running",
            "updatedAt": 1780274116096,
            "updatedSeq": 1,
        },
        f"{codex_key}::codex::codex": {
            "sessionKey": codex_key,
            "adapter": "codex",
            "runnerId": "codex",
            "title": "Codex Running",
            "lastUserMessage": "codex running user",
            "latestRunId": "run-external-running",
            "status": "running",
            "updatedAt": 1780274117000,
            "updatedSeq": 2,
        },
        f"{chatgpt_web_key}::chatgpt_web::web": {
            "sessionKey": chatgpt_web_key,
            "adapter": "chatgpt_web",
            "runnerId": "web",
            "title": "ChatGPT Web Running",
            "lastUserMessage": "chatgpt web running user",
            "latestRunId": "run-chatgpt-web-running",
            "status": "running",
            "updatedAt": 1780274118000,
            "updatedSeq": 3,
        },
        f"{custom_cli_key}::custom_cli::custom": {
            "sessionKey": custom_cli_key,
            "adapter": "custom_cli",
            "runnerId": "custom",
            "title": "Custom CLI Running",
            "lastUserMessage": "custom cli running user",
            "latestRunId": "run-custom-cli-running",
            "status": "running",
            "updatedAt": 1780274119000,
            "updatedSeq": 4,
        },
    },
}
(state_dir / "session_state.json").write_text(
    json.dumps(store, ensure_ascii=False, indent=2),
    encoding="utf-8",
)
PY
}

assert_sessions_projection() {
  local response_file="$1"
  shift
  python3 - "$response_file" "$STALE_SESSION_KEY" "$@" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
stale_key = sys.argv[2]
external_keys = sys.argv[3:]
sessions = {item.get("session_key"): item for item in payload.get("sessions", [])}
assert stale_key in sessions, f"missing stale session: {payload}"
stale = sessions[stale_key]
assert stale.get("running") is False, stale
assert stale.get("status") == "ended", stale
assert stale.get("state") == "ended", stale
assert stale.get("run_state") == "completed", stale
assert stale.get("has_timeline") is True, stale
assert stale.get("timeline_event_count", 0) >= 4, stale
assert stale.get("last_active_time") == 103, stale
for external_key in external_keys:
    assert external_key in sessions, f"missing external session {external_key}: {payload}"
    external = sessions[external_key]
    assert external.get("running") is True, external
    assert external.get("status") == "active", external
    assert external.get("state") == "running", external
    assert external.get("run_state") == "running", external
PY
}

assert_stop_repaired_state() {
  python3 - "$STOP_RESPONSE" "$BIFROST_DATA_DIR/agent/im_gateway/session_state.json" "$STALE_SESSION_KEY" "$CODEX_SESSION_KEY" "$CHATGPT_WEB_SESSION_KEY" "$CUSTOM_CLI_SESSION_KEY" <<'PY'
import json
import sys

stop_payload = json.load(open(sys.argv[1], encoding="utf-8"))
store = json.load(open(sys.argv[2], encoding="utf-8"))
stale_key = sys.argv[3]
external_keys = sys.argv[4:]
assert stop_payload.get("success") is True, stop_payload
assert stop_payload.get("stopped") is False, stop_payload
assert stop_payload.get("repaired_stale_running", 0) >= 1, stop_payload
sessions = store.get("sessions", {})
stale = sessions[f"{stale_key}::bifrost_agent::"]
assert stale.get("status") == "ended", stale
for external_key in external_keys:
    external = next(
        item for item in sessions.values()
        if item.get("sessionKey") == external_key
    )
    assert external.get("status") == "running", external
PY
}

prepare_fixture

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN" || true)"
  echo "[agent-session-stale-running-reconciliation] skipping build, using ${BIFROST_BIN:-<missing>}"
else
  echo "[agent-session-stale-running-reconciliation] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN" || true)"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-session-stale-running-reconciliation] bifrost binary not found: $BIFROST_BIN" >&2
  exit 1
fi

BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

AGENT_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"
curl -fsS --noproxy '*' "$AGENT_BASE/sessions/all" >"$SESSIONS_RESPONSE"
assert_sessions_projection \
  "$SESSIONS_RESPONSE" \
  "$CODEX_SESSION_KEY" \
  "$CHATGPT_WEB_SESSION_KEY" \
  "$CUSTOM_CLI_SESSION_KEY"

curl -fsS --noproxy '*' -X POST "$AGENT_BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"$STALE_SESSION_KEY\",\"message\":\"/stop\"}" \
  >"$STOP_RESPONSE"
assert_stop_repaired_state

curl -fsS --noproxy '*' "$AGENT_BASE/sessions/all" >"$FINAL_SESSIONS_RESPONSE"
assert_sessions_projection \
  "$FINAL_SESSIONS_RESPONSE" \
  "$CODEX_SESSION_KEY" \
  "$CHATGPT_WEB_SESSION_KEY" \
  "$CUSTOM_CLI_SESSION_KEY"

echo "[agent-session-stale-running-reconciliation] PASS"
