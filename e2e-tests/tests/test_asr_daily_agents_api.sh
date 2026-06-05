#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-daily-agents-e2e.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-daily-agents-audio.XXXXXX")"
SYNC_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-daily-agents-sync.XXXXXX")"
E2E_DIR="$DATA_DIR/e2e"
PORT="${BIFROST_E2E_PORT:-18997}"
MOCK_PORT="${BIFROST_DAILY_AGENT_MOCK_PORT:-18998}"
BIN="${BIFROST_BIN:-target/debug/bifrost}"
PID=""
MOCK_PID=""

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
  if [[ -n "$MOCK_PID" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
  if [[ $status -eq 0 ]]; then
    rm -rf "$DATA_DIR" "$AUDIO_DIR" "$SYNC_DIR"
  else
    echo "Keeping failed E2E data dir: $DATA_DIR" >&2
    echo "Keeping failed E2E audio dir: $AUDIO_DIR" >&2
    echo "Keeping failed E2E sync dir: $SYNC_DIR" >&2
  fi
}
trap cleanup EXIT

mkdir -p "$E2E_DIR"

MODEL_LOG="$E2E_DIR/mock-model.jsonl"
cat > "$DATA_DIR/mock_model.py" <<'PY'
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
model_log = sys.argv[2]

def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": json.dumps(arguments, ensure_ascii=False),
        },
    }

def stringify(value):
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "\n".join(stringify(item) for item in value)
    if isinstance(value, dict):
        if isinstance(value.get("text"), str):
            return value["text"]
        if isinstance(value.get("content"), str):
            return value["content"]
        return json.dumps(value, ensure_ascii=False)
    return str(value)

class Handler(BaseHTTPRequestHandler):
    def _json(self, status, payload):
        data = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._json(200, {"ok": True})

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b"{}"
        payload = json.loads(body.decode("utf-8") or "{}")
        with open(model_log, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        tool_names = {
            tool.get("function", {}).get("name")
            for tool in payload.get("tools", [])
            if tool.get("type") == "function"
        }
        if "write_file" not in tool_names:
            self._json(500, {"error": f"write_file tool missing: {sorted(tool_names)}"})
            return

        messages = payload.get("messages", [])
        full_text = "\n".join(stringify(message.get("content")) for message in messages)
        has_tool_result = any(message.get("role") == "tool" for message in messages)

        if has_tool_result:
            message = {"role": "assistant", "content": "daily agent report written"}
            finish_reason = "stop"
        else:
            match = re.search(r"report=([^\n\r]+)", full_text)
            if not match:
                self._json(500, {"error": "report target missing from prompt", "text": full_text[-1000:]})
                return
            report_path = match.group(1).strip()
            agent_id = "tomorrow_todo" if "tomorrow_todo/" in report_path else "daily_report"
            if agent_id == "tomorrow_todo" and "E2E_CUSTOM_TOMORROW_AGENT_MARKER" not in full_text:
                self._json(500, {"error": "custom tomorrow AGENTS.md marker missing from model context"})
                return
            report_content = (
                f"# E2E generated report for {agent_id}\n\n"
                f"- report_path: {report_path}\n"
                "- source: 2026-05-22.md\n"
                "- marker: E2E_DAILY_AGENT_REAL_RUN\n"
            )
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        f"call-write-{agent_id}",
                        "write_file",
                        {"path": report_path, "content": report_content},
                    )
                ],
            }
            finish_reason = "tool_calls"

        self._json(200, {
            "choices": [{"message": message, "finish_reason": finish_reason}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        })

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
python3 "$DATA_DIR/mock_model.py" "$MOCK_PORT" "$MODEL_LOG" &
MOCK_PID="$!"
for _ in {1..60}; do
  if curl -fsS "http://127.0.0.1:$MOCK_PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
curl -fsS "http://127.0.0.1:$MOCK_PORT/health" >/dev/null

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

curl -fsS -X PATCH "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/agent" \
  -H 'content-type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-daily-agent\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 4,
    \"history\": {\"persistence\": \"none\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

TASK_JSON="$E2E_DIR/task.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"daily agents e2e\",\"audio_dir\":\"$AUDIO_DIR\",\"enabled\":false,\"recursive\":false,\"daily_agent\":{\"enabled\":true}}" > "$TASK_JSON"
TASK_ID="$(python3 - <<'PY' "$TASK_JSON"
import json, sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"

CONFIG_JSON="$E2E_DIR/config.json"
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

TASK_TEXT_DIR="$DATA_DIR/asr/data/text/$TASK_ID"
DAILY_DIR="$TASK_TEXT_DIR/.daily"
AGENTS_DIR="$DAILY_DIR/agents"
REPORT_DIR="$AGENTS_DIR/daily_report/output/report"
TODO_DIR="$AGENTS_DIR/tomorrow_todo/output/tomorrow_todo"
test -d "$AGENTS_DIR/daily_report"
test -d "$AGENTS_DIR/tomorrow_todo"
test -d "$AGENTS_DIR/daily_report/input"
test -d "$AGENTS_DIR/tomorrow_todo/input"
test -d "$REPORT_DIR"
test -d "$TODO_DIR"
test -f "$AGENTS_DIR/daily_report/AGENTS.md"
test -f "$AGENTS_DIR/tomorrow_todo/AGENTS.md"
grep -q "明日 To Do List" "$AGENTS_DIR/tomorrow_todo/AGENTS.md"

CONFIG_UPDATE_JSON="$E2E_DIR/config_update.json"
python3 - <<'PY' "$CONFIG_JSON" "$CONFIG_UPDATE_JSON" "$SYNC_DIR"
import json, sys
body=json.load(open(sys.argv[1]))
config=body["config"]
for agent in config["agents"]:
    agent["runner"]="bifrost_agent"
    agent["timeout_ms"]=60000
    agent.setdefault("im_delivery", {})["enabled"]=False
payload={
    "enabled": True,
    "agents": config["agents"],
    "runner": "bifrost_agent",
    "timeout_ms": 60000,
    "report_sync_dir": sys.argv[3],
}
json.dump(payload, open(sys.argv[2], "w"), ensure_ascii=False)
PY
curl -fsS -X PUT "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" \
  -H 'content-type: application/json' \
  -d @"$CONFIG_UPDATE_JSON" >/dev/null

CUSTOM_INSTRUCTIONS="$E2E_DIR/tomorrow_agents.md"
cat > "$CUSTOM_INSTRUCTIONS" <<'MD'
# E2E Custom Tomorrow Agent

E2E_CUSTOM_TOMORROW_AGENT_MARKER

Write a concise tomorrow todo report for changed daily markdown files.
MD
CUSTOM_PAYLOAD="$E2E_DIR/tomorrow_agents_payload.json"
python3 - <<'PY' "$CUSTOM_INSTRUCTIONS" "$CUSTOM_PAYLOAD"
import json, sys
json.dump({"content": open(sys.argv[1], encoding="utf-8").read()}, open(sys.argv[2], "w"), ensure_ascii=False)
PY
curl -fsS -X PUT "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/agents?agent_id=tomorrow_todo" \
  -H 'content-type: application/json' \
  -d @"$CUSTOM_PAYLOAD" >/dev/null
grep -q "E2E_CUSTOM_TOMORROW_AGENT_MARKER" "$AGENTS_DIR/tomorrow_todo/AGENTS.md"

mkdir -p "$DAILY_DIR"
cat > "$DAILY_DIR/2026-05-22.md" <<'MD'
# 2026-05-22
今天讨论了发布计划，并明确明天需要整理上线 checklist。
MD

RUN_JSON="$E2E_DIR/run_response.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/run?date=2026-05-22&force=1" > "$RUN_JSON"
python3 - <<'PY' "$RUN_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["status"] in ("queued", "already_running"), body
assert body.get("date") == "2026-05-22", body
PY

RUNS_JSON="$E2E_DIR/runs.json"
RUN_DONE=0
for _ in {1..120}; do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/runs" > "$RUNS_JSON"
  if python3 - <<'PY' "$RUNS_JSON" "$REPORT_DIR/2026-05-22-report.md" "$TODO_DIR/2026-05-22-report.md"
import json, pathlib, sys
body=json.load(open(sys.argv[1]))
docs=[
    d for d in body.get("processed_documents", [])
    if d.get("date") == "2026-05-22" and d.get("agent_id") in {"daily_report", "tomorrow_todo"}
]
if len({d.get("agent_id") for d in docs}) != 2:
    raise SystemExit(1)
for path in sys.argv[2:]:
    text=pathlib.Path(path).read_text(encoding="utf-8")
    assert "E2E_DAILY_AGENT_REAL_RUN" in text, text
PY
  then
    RUN_DONE=1
    break
  fi
  sleep 1
done
if [[ "$RUN_DONE" != "1" ]]; then
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" > "$E2E_DIR/final_config.json" || true
  cat "$RUNS_JSON" >&2 || true
  cat "$E2E_DIR/final_config.json" >&2 || true
  exit 1
fi

python3 - <<'PY' "$RUNS_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
docs=[d for d in body["processed_documents"] if d["date"]=="2026-05-22"]
assert len(docs)==2, docs
assert {d["agent_id"] for d in docs} == {"daily_report", "tomorrow_todo"}, docs
assert {d["output_dir"] for d in docs} == {"report", "tomorrow_todo"}, docs
assert {d["runner"] for d in docs} == {"bifrost_agent"}, docs
assert len({(d["agent_id"], d["date"]) for d in docs}) == 2, docs
for doc in docs:
    assert "/.daily/agents/" in doc.get("report_path", ""), doc
PY
grep -q "E2E_DAILY_AGENT_REAL_RUN" "$REPORT_DIR/2026-05-22-report.md"
grep -q "E2E_DAILY_AGENT_REAL_RUN" "$TODO_DIR/2026-05-22-report.md"
grep -q "E2E_DAILY_AGENT_REAL_RUN" "$SYNC_DIR/daily_report/2026-05-22-report.md"
grep -q "E2E_DAILY_AGENT_REAL_RUN" "$SYNC_DIR/tomorrow_todo/2026-05-22-report.md"
test ! -f "$SYNC_DIR/2026-05-22-report.md"
grep -q "整理上线 checklist" "$AGENTS_DIR/daily_report/input/2026-05-22.md"
grep -q "整理上线 checklist" "$AGENTS_DIR/tomorrow_todo/input/2026-05-22.md"
test ! -f "$DAILY_DIR/report/2026-05-22-report.md"
test ! -f "$DAILY_DIR/tomorrow_todo/2026-05-22-report.md"

TODO_REPORT_JSON="$E2E_DIR/todo_report.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/reports/2026-05-22?agent_id=tomorrow_todo" > "$TODO_REPORT_JSON"
python3 - <<'PY' "$TODO_REPORT_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["agent_id"] == "tomorrow_todo", body
assert body["output_dir"] == "tomorrow_todo", body
assert "E2E_DAILY_AGENT_REAL_RUN" in body["content"], body
assert "/.daily/agents/tomorrow_todo/output/tomorrow_todo/" in body["path"], body
PY

SYNC_STATUS_JSON="$E2E_DIR/sync_status.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" > "$SYNC_STATUS_JSON"
python3 - <<'PY' "$SYNC_STATUS_JSON" "$SYNC_DIR"
import json, os, pathlib, sys
body=json.load(open(sys.argv[1]))
sync_root=pathlib.Path(sys.argv[2])
agents={agent["id"]: agent for agent in body["config"]["agents"]}
assert os.path.normpath(agents["daily_report"]["last_report_sync"]["target_dir"]) == os.path.normpath(sync_root / "daily_report"), agents["daily_report"]
assert os.path.normpath(agents["tomorrow_todo"]["last_report_sync"]["target_dir"]) == os.path.normpath(sync_root / "tomorrow_todo"), agents["tomorrow_todo"]
assert agents["daily_report"]["last_report_sync"]["total_files"] == 1, agents["daily_report"]
assert agents["tomorrow_todo"]["last_report_sync"]["total_files"] == 1, agents["tomorrow_todo"]
PY

INSTR_JSON="$E2E_DIR/todo_instructions.json"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/agents?agent_id=tomorrow_todo" > "$INSTR_JSON"
python3 - <<'PY' "$INSTR_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["agent_id"] == "tomorrow_todo", body
assert "E2E_CUSTOM_TOMORROW_AGENT_MARKER" in body["content"], body
assert body["source"] == "file", body
PY

python3 - <<'PY' "$MODEL_LOG"
import json, sys
lines=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
assert len(lines) >= 4, len(lines)
dump="\n".join(json.dumps(line, ensure_ascii=False) for line in lines)
assert "E2E_CUSTOM_TOMORROW_AGENT_MARKER" in dump, "custom AGENTS.md marker never reached model context"
assert "output/report/2026-05-22-report.md" in dump, "daily_report target missing from model prompt"
assert "output/tomorrow_todo/2026-05-22-report.md" in dump, "tomorrow_todo target missing from model prompt"
PY

RUN_IDS_BEFORE_APPEND="$E2E_DIR/run_ids_before_append.json"
python3 - <<'PY' "$RUNS_JSON" "$RUN_IDS_BEFORE_APPEND"
import json, sys
body=json.load(open(sys.argv[1]))
docs=[
    d for d in body.get("processed_documents", [])
    if d.get("date") == "2026-05-22" and d.get("agent_id") in {"daily_report", "tomorrow_todo"}
]
assert len({d.get("agent_id") for d in docs}) == 2, docs
json.dump({d["agent_id"]: d["last_run_id"] for d in docs}, open(sys.argv[2], "w"), ensure_ascii=False)
PY

cat >> "$DAILY_DIR/2026-05-22.md" <<'MD'

新增明日跟进动作：确认追加后的 daily markdown 会在非 force 运行中被识别为 Appended。
MD

APPEND_RUN_JSON="$E2E_DIR/run_response_appended.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/run?date=2026-05-22" > "$APPEND_RUN_JSON"
python3 - <<'PY' "$APPEND_RUN_JSON"
import json, sys
body=json.load(open(sys.argv[1]))
assert body["status"] in ("queued", "already_running"), body
assert body.get("date") == "2026-05-22", body
PY

APPENDED_RUN_DONE=0
for _ in {1..120}; do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent/runs" > "$RUNS_JSON"
  if python3 - <<'PY' "$RUNS_JSON" "$RUN_IDS_BEFORE_APPEND" "$REPORT_DIR/2026-05-22-report.md" "$TODO_DIR/2026-05-22-report.md"
import json, pathlib, sys
body=json.load(open(sys.argv[1]))
before=json.load(open(sys.argv[2]))
docs=[
    d for d in body.get("processed_documents", [])
    if d.get("date") == "2026-05-22" and d.get("agent_id") in {"daily_report", "tomorrow_todo"}
]
if len({d.get("agent_id") for d in docs}) != 2:
    raise SystemExit(1)
for doc in docs:
    if doc.get("last_run_id") == before.get(doc.get("agent_id")):
        raise SystemExit(1)
for path in sys.argv[3:]:
    text=pathlib.Path(path).read_text(encoding="utf-8")
    assert "E2E_DAILY_AGENT_REAL_RUN" in text, text
PY
  then
    APPENDED_RUN_DONE=1
    break
  fi
  sleep 1
done
if [[ "$APPENDED_RUN_DONE" != "1" ]]; then
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" > "$E2E_DIR/final_config_after_append.json" || true
  cat "$RUNS_JSON" >&2 || true
  cat "$E2E_DIR/final_config_after_append.json" >&2 || true
  exit 1
fi

python3 - <<'PY' "$MODEL_LOG"
import json, sys
lines=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
dump="\n".join(json.dumps(line, ensure_ascii=False) for line in lines)
assert "change_kind=Appended" in dump, "appended daily markdown did not reach model prompt"
PY

echo "ASR daily agents API E2E passed: task=$TASK_ID data_dir=$DATA_DIR"
