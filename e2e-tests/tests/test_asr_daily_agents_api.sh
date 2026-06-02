#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-daily-agents-e2e.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-daily-agents-audio.XXXXXX")"
PORT="${BIFROST_E2E_PORT:-18997}"
BIN="${BIFROST_BIN:-target/debug/bifrost}"
PID=""

cleanup() {
  local status=$?
  if [[ $status -ne 0 && -f "$DATA_DIR/server.log" ]]; then
    echo "---- bifrost server.log ----" >&2
    sed -n '1,240p' "$DATA_DIR/server.log" >&2 || true
    echo "---- end server.log ----" >&2
  fi
  if [[ -n "$PID" ]]; then
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
  fi
  if [[ $status -eq 0 ]]; then
    rm -rf "$DATA_DIR" "$AUDIO_DIR"
  else
    echo "Keeping failed E2E data dir: $DATA_DIR" >&2
    echo "Keeping failed E2E audio dir: $AUDIO_DIR" >&2
  fi
}
trap cleanup EXIT

SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost

BIFROST_DATA_DIR="$DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 "$BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check >"$DATA_DIR/server.log" 2>&1 &
PID="$!"

for _ in {1..120}; do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/capabilities" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/capabilities" >/dev/null

TASK_JSON="$DATA_DIR/task.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"daily agents e2e\",\"audio_dir\":\"$AUDIO_DIR\",\"enabled\":false,\"recursive\":false,\"daily_agent\":{\"enabled\":true}}" > "$TASK_JSON"
TASK_ID="$(python3 - <<'PY' "$TASK_JSON"
import json, sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"

CONFIG_JSON="$DATA_DIR/config.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" > "$CONFIG_JSON"
python3 - <<'PY' "$CONFIG_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
agents=body["config"].get("agents", [])
assert len(agents)==2, agents
assert [a["id"] for a in agents] == ["daily_report", "tomorrow_todo"], agents
assert agents[0]["output_dir"] == "report", agents
assert agents[1]["output_dir"] == "tomorrow_todo", agents
assert agents[1]["im_delivery"]["enabled"] is True, agents[1]
assert agents[1]["im_delivery"]["channel"] == "owner:feishu-main", agents[1]
PY

DAILY_DIR="$DATA_DIR/asr/data/text/$TASK_ID/daily"
mkdir -p "$DAILY_DIR/report" "$DAILY_DIR/tomorrow_todo" "$DATA_DIR/asr/tasks/$TASK_ID"
cat > "$DAILY_DIR/2026-05-22.md" <<'MD'
# 2026-05-22
今天讨论了发布计划，并明确明天需要整理上线 checklist。
MD
cat > "$DAILY_DIR/report/2026-05-22-report.md" <<'MD'
# Daily Report
MD
cat > "$DAILY_DIR/tomorrow_todo/2026-05-22-report.md" <<'MD'
# 明日 To Do List
- [ ] 整理上线 checklist
MD
cat > "$DATA_DIR/asr/tasks/$TASK_ID/daily_agent_processed.json" <<JSON
{
  "version": 1,
  "documents": {
    "daily_report:2026-05-22": {
      "agent_id": "daily_report",
      "agent_name": "daily_report",
      "output_dir": "report",
      "date": "2026-05-22",
      "source_sha256": "hash-report",
      "source_len_bytes": 100,
      "processed_at_ms": 1000,
      "runner": "bifrost_agent",
      "report_path": "$DAILY_DIR/report/2026-05-22-report.md",
      "last_run_id": "run-report"
    },
    "tomorrow_todo:2026-05-22": {
      "agent_id": "tomorrow_todo",
      "agent_name": "tomorrow_todo",
      "output_dir": "tomorrow_todo",
      "date": "2026-05-22",
      "source_sha256": "hash-todo",
      "source_len_bytes": 100,
      "processed_at_ms": 1001,
      "runner": "bifrost_agent",
      "report_path": "$DAILY_DIR/tomorrow_todo/2026-05-22-report.md",
      "last_run_id": "run-todo"
    }
  }
}
JSON

RUNS_JSON="$DATA_DIR/runs.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/runs" > "$RUNS_JSON"
python3 - <<'PY' "$RUNS_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
docs=body["processed_documents"]
assert len(docs)==2, docs
assert {d["agent_id"] for d in docs} == {"daily_report", "tomorrow_todo"}, docs
assert {d["output_dir"] for d in docs} == {"report", "tomorrow_todo"}, docs
assert len({(d["agent_id"], d["date"]) for d in docs}) == 2, docs
PY

TODO_REPORT_JSON="$DATA_DIR/todo_report.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/reports/2026-05-22?agent_id=tomorrow_todo" > "$TODO_REPORT_JSON"
python3 - <<'PY' "$TODO_REPORT_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["agent_id"] == "tomorrow_todo", body
assert body["output_dir"] == "tomorrow_todo", body
assert "明日 To Do List" in body["content"], body
PY

INSTR_JSON="$DATA_DIR/todo_instructions.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/agents?agent_id=tomorrow_todo" > "$INSTR_JSON"
python3 - <<'PY' "$INSTR_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["agent_id"] == "tomorrow_todo", body
assert "明日 To Do List" in body["content"], body
PY

RUN_JSON="$DATA_DIR/run_response.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/run?agent_id=tomorrow_todo&date=2026-05-22&force=1" > "$RUN_JSON"
python3 - <<'PY' "$RUN_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["status"] in ("queued", "already_running"), body
assert body.get("agent_id") == "tomorrow_todo", body
assert body.get("date") == "2026-05-22", body
PY

echo "ASR daily agents API E2E passed: task=$TASK_ID data_dir=$DATA_DIR"
