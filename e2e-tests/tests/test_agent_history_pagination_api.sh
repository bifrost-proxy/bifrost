#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-agent-history-pagination.XXXXXX")"
DATA_DIR="$TEST_DIR/data"
LOG_FILE="$TEST_DIR/bifrost.log"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/.bifrost-e2e-target}"
export CARGO_TARGET_DIR
export BIFROST_DATA_DIR="$DATA_DIR"

resolve_bifrost_bin() {
  local candidate="${BIFROST_BIN:-}"
  if [[ -z "$candidate" ]]; then
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
      candidate="$ROOT_DIR/target/release/bifrost"
    else
      candidate="$CARGO_TARGET_DIR/debug/bifrost"
    fi
  fi
  if [[ ! -x "$candidate" ]]; then
    echo "bifrost binary is not executable: $candidate" >&2
    return 1
  fi
  printf '%s\n' "$candidate"
}

cleanup() {
  status=$?
  if [[ "$status" -ne 0 && -f "$LOG_FILE" ]]; then
    echo "---- bifrost test server log ----" >&2
    cat "$LOG_FILE" >&2 || true
    echo "---- end bifrost test server log ----" >&2
  fi
  if [[ -n "${BIFROST_PID:-}" ]] && kill -0 "$BIFROST_PID" 2>/dev/null; then
    kill "$BIFROST_PID" 2>/dev/null || true
    wait "$BIFROST_PID" 2>/dev/null || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

SESSION_KEY="pagination-session"
HISTORY_DIR="$DATA_DIR/agent/sessions/2026/05/29"
HISTORY_FILE="$HISTORY_DIR/session-pagination-session-1780017999.jsonl"
mkdir -p "$HISTORY_DIR"
cat > "$HISTORY_FILE" <<'JSONL'
{"timestamp":1780017900,"event_type":"session_start","session_key":"pagination-session","content":{"source":"admin-api","work_dir":"/tmp/pagination"}}
{"timestamp":1780017901,"event_type":"user_message","session_key":"pagination-session","content":{"message":"first user"}}
{"timestamp":1780017902,"event_type":"assistant_delta","session_key":"pagination-session","content":{"message":"thinking"}}
{"timestamp":1780017903,"event_type":"tool_call","session_key":"pagination-session","content":{"tool_name":"exec_command","arguments":"{\"cmd\":\"pwd\"}","call_id":"call-1"}}
{"timestamp":1780017904,"event_type":"tool_result","session_key":"pagination-session","content":{"tool_name":"exec_command","result":"/tmp/pagination","success":true,"call_id":"call-1"}}
{"timestamp":1780017905,"event_type":"user_message","session_key":"pagination-session","content":{"message":"second user"}}
{"timestamp":1780017906,"event_type":"assistant_message","session_key":"pagination-session","content":{"message":"done","tokens":12}}
JSONL

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  echo "[agent-history-pagination] skipping build, using ${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
else
  cargo build --bin bifrost
fi
BIFROST_BIN="$(resolve_bifrost_bin)"
"$BIFROST_BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check --yes >"$LOG_FILE" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 80); do
  if curl --noproxy '*' -fsS "http://127.0.0.1:$PORT/_bifrost/api/system/overview" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

curl --noproxy '*' -fsS "http://127.0.0.1:$PORT/_bifrost/api/system/overview" >/dev/null

BASE="http://127.0.0.1:$PORT/_bifrost/api/im-gateway/agent"
HISTORY_PATH_ENCODED="$(python3 - <<PY
import urllib.parse
print(urllib.parse.quote("$HISTORY_FILE", safe=""))
PY
)"

SESSIONS_JSON="$TEST_DIR/sessions.json"
TAIL_JSON="$TEST_DIR/tail.json"
OLDER_JSON="$TEST_DIR/older.json"
DELTA_JSON="$TEST_DIR/delta.json"

curl --noproxy '*' -fsS "$BASE/sessions/all" -o "$SESSIONS_JSON"
python3 - "$SESSIONS_JSON" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
sessions = payload.get("sessions", [])
match = next((item for item in sessions if item.get("session_key") == "pagination-session"), None)
assert match, "pagination-session summary not found"
assert "events" not in match, "session list must not include full events"
assert match.get("timeline_event_count") == 7, match
assert match.get("history_path"), match
PY

curl --noproxy '*' -fsS "$BASE/sessions/history/$HISTORY_PATH_ENCODED?tail=true&limit=2" -o "$TAIL_JSON"
python3 - "$TAIL_JSON" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
assert payload["count"] == 2, payload
assert payload["total_count"] == 7, payload
assert payload["start_index"] == 5, payload
assert payload["end_index"] == 7, payload
assert payload["has_more"] is True, payload
events = payload["events"]
assert [event["event_type"] for event in events] == ["user_message", "assistant_message"], events
PY

curl --noproxy '*' -fsS "$BASE/sessions/history/$HISTORY_PATH_ENCODED?cursor=5&limit=2" -o "$OLDER_JSON"
python3 - "$OLDER_JSON" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
assert payload["count"] == 2, payload
assert payload["start_index"] == 3, payload
assert payload["end_index"] == 5, payload
assert payload["has_more"] is True, payload
events = payload["events"]
assert [event["event_type"] for event in events] == ["tool_call", "tool_result"], events
PY

curl --noproxy '*' -fsS "$BASE/sessions/history/$HISTORY_PATH_ENCODED?since=6" -o "$DELTA_JSON"
python3 - "$DELTA_JSON" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
assert payload["count"] == 1, payload
assert payload["start_index"] == 6, payload
assert payload["end_index"] == 7, payload
assert payload["has_more"] is False, payload
events = payload["events"]
assert [event["event_type"] for event in events] == ["assistant_message"], events
PY

echo "agent history pagination API checks passed"
