#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fail() {
  echo "[qwen3-asr-e2e] error: $*" >&2
  exit 1
}

echo "[qwen3-asr-e2e] syntax checks"
bash -n "$0"

echo "[qwen3-asr-e2e] bifrost ai asr CLI structure"
ASR_HELP="$(cargo run --quiet --bin bifrost -- ai asr --help)"
if [[ "$(uname -s)-$(uname -m)" == "Darwin-arm64" ]]; then
  grep -q "stream-file" <<<"$ASR_HELP"
else
  if grep -q "stream-file" <<<"$ASR_HELP"; then
    fail "bifrost ai asr help unexpectedly exposed stream-file on unsupported platform"
  fi
fi

echo "[qwen3-asr-e2e] ASR runtime guard unit assertions"
cargo test -p bifrost-admin asr_runtime_timeouts_are_bounded_for_short_chunks --lib
cargo test -p bifrost-admin server_failure_breaker --lib
cargo test -p bifrost-admin reuse_server_failure_threshold --lib

if [[ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]]; then
  echo "[qwen3-asr-e2e] unsupported platform CLI guard"
  if cargo run --quiet --bin bifrost -- ai asr status --json >/tmp/bifrost-qwen3-asr-unsupported.out 2>&1; then
    fail "bifrost ai asr status unexpectedly succeeded on unsupported platform"
  fi
  grep -q "only supported on Apple Silicon macOS" /tmp/bifrost-qwen3-asr-unsupported.out
  rm -f /tmp/bifrost-qwen3-asr-unsupported.out
  echo "[qwen3-asr-e2e] online model verification skipped on unsupported platform"
  exit 0
fi

CLI_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-qwen3-asr-cli.XXXXXX")"
BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai asr status --json \
  | grep -q '"ready"'
if BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai asr stream-file /tmp/bifrost-qwen3-asr-missing.wav >/tmp/bifrost-qwen3-asr-cli-missing.out 2>&1; then
  fail "bifrost ai asr stream-file unexpectedly succeeded with missing audio"
fi
grep -q "audio file does not exist" /tmp/bifrost-qwen3-asr-cli-missing.out
rm -rf "$CLI_DATA_DIR" /tmp/bifrost-qwen3-asr-cli-missing.out

if [[ "${CI:-}" == "true" || "${GITHUB_ACTIONS:-}" == "true" || "${BUILDKITE:-}" == "true" || "${JENKINS_URL:-}" != "" ]]; then
  echo "[qwen3-asr-e2e] CI environment detected; online model verification skipped to avoid downloading or starting Qwen3-ASR"
  exit 0
fi

if [[ "${BIFROST_QWEN3_ASR_E2E_ONLINE:-0}" != "1" ]]; then
  echo "[qwen3-asr-e2e] online model verification skipped; set BIFROST_QWEN3_ASR_E2E_ONLINE=1 to run it"
  exit 0
fi

ASR_HOME="$HOME/.bifrost/asr"
ASR_DIR="$ASR_HOME/qwen3_asr_rs"

cleanup() {
  if [[ -n "${ADMIN_PID:-}" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${ADMIN_DATA_DIR:-}" ]]; then
    rm -rf "$ADMIN_DATA_DIR"
  fi
  if [[ -n "${TASK_AUDIO_DIR:-}" ]]; then
    rm -rf "$TASK_AUDIO_DIR"
  fi
  if [[ -n "${SCHED_AUDIO_DIR1:-}" ]]; then
    rm -rf "$SCHED_AUDIO_DIR1"
  fi
  if [[ -n "${SCHED_AUDIO_DIR2:-}" ]]; then
    rm -rf "$SCHED_AUDIO_DIR2"
  fi
}
trap cleanup EXIT

ADMIN_PORT="${BIFROST_QWEN3_ASR_E2E_ADMIN_PORT:-18880}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-qwen3-asr-admin.XXXXXX")"

echo "[qwen3-asr-e2e] start Bifrost admin on ${ADMIN_PORT}"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" cargo run --bin bifrost -- start \
  -p "$ADMIN_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy >"$ADMIN_DATA_DIR/bifrost.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null || {
  tail -100 "$ADMIN_DATA_DIR/bifrost.log" >&2 || true
  fail "Bifrost admin did not start"
}

ASR_QUERY="host=127.0.0.1&language=chinese&model=Qwen3-ASR-1.7B"
mkdir -p "$ASR_HOME"
echo "[qwen3-asr-e2e] admin ASR rust initializer"
curl -fsSN "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/init-stream?${ASR_QUERY}" \
  > "$ASR_HOME/e2e-admin-init-download.sse"
grep -q 'event: done' "$ASR_HOME/e2e-admin-init-download.sse"
grep -q '"phase":"installed"' "$ASR_HOME/e2e-admin-init-download.sse"

echo "[qwen3-asr-e2e] admin ASR status"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/status?${ASR_QUERY}" \
  | tee "$ASR_DIR/e2e-admin-status.json" \
  | grep -q '"installed":true'

echo "[qwen3-asr-e2e] admin ASR directory task API"
TASK_AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-qwen3-asr-task-audio.XXXXXX")"
cp "${ASR_DIR}/sample3.wav" "${TASK_AUDIO_DIR}/sample3.wav"
printf 'ignore me\n' > "${TASK_AUDIO_DIR}/note.txt"
TASK_JSON="$(python3 - "$TASK_AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
    "name": "ASR E2E task",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
)"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$TASK_JSON" \
  | tee "$ASR_DIR/e2e-admin-task-create.json" \
  | grep -q '"pending":1'
grep -q '"runtime_strategy":"reuse_per_file"' "$ASR_DIR/e2e-admin-task-create.json"
TASK_ID="$(python3 - "$ASR_DIR/e2e-admin-task-create.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  | tee "$ASR_DIR/e2e-admin-task-list.json" \
  | grep -q "$TASK_ID"

echo "[qwen3-asr-e2e] admin ASR managed service start"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/service/start?${ASR_QUERY}" \
  | tee "$ASR_DIR/e2e-admin-start.json" \
  | grep -q '"managed":true'
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/status?${ASR_QUERY}" \
  | tee "$ASR_DIR/e2e-admin-status-running.json" \
  | grep -q '"ready":true'
grep -q '"server_url":"http://127.0.0.1:' "$ASR_DIR/e2e-admin-status-running.json"
MANAGED_ASR_PORT="$(python3 - "$ASR_DIR/e2e-admin-status-running.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    server_url = json.load(f)["server_url"]
print(server_url.rsplit(":", 1)[1])
PY
)"

echo "[qwen3-asr-e2e] admin ASR init stream already-ready path"
curl -fsSN "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/init-stream?${ASR_QUERY}" \
  > "$ASR_DIR/e2e-admin-init.sse"
grep -q 'event: progress' "$ASR_DIR/e2e-admin-init.sse"
grep -q '"phase":"installed"' "$ASR_DIR/e2e-admin-init.sse"
grep -q 'event: done' "$ASR_DIR/e2e-admin-init.sse"

echo "[qwen3-asr-e2e] admin ASR transcription stream"
curl -fsSN -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/transcribe-stream?${ASR_QUERY}" \
  -F "file=@${ASR_DIR}/sample3.wav" \
  -F "language=chinese" \
  -F "response_format=text" \
  > "$ASR_DIR/e2e-admin-transcribe.sse"
grep -q 'event: partial' "$ASR_DIR/e2e-admin-transcribe.sse"
grep -q 'event: final' "$ASR_DIR/e2e-admin-transcribe.sse"
grep -q 'event: text' "$ASR_DIR/e2e-admin-transcribe.sse"
partial_count="$(grep -c 'event: partial' "$ASR_DIR/e2e-admin-transcribe.sse")"
final_count="$(grep -c 'event: final' "$ASR_DIR/e2e-admin-transcribe.sse")"
if (( partial_count < 2 || final_count < 2 )); then
  fail "expected multiple streaming windows, got partial=${partial_count}, final=${final_count}"
fi
grep -E "Qwen3|语音|测试" "$ASR_DIR/e2e-admin-transcribe.sse" >/dev/null

echo "[qwen3-asr-e2e] bifrost ai asr stream-file emits CLI JSON lines"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" cargo run --quiet --bin bifrost -- ai asr stream-file "${ASR_DIR}/sample3.wav" \
  --language chinese \
  > "$ASR_DIR/e2e-cli-stream-file.jsonl"
grep -q '"type":"partial"' "$ASR_DIR/e2e-cli-stream-file.jsonl"
grep -q '"type":"final"' "$ASR_DIR/e2e-cli-stream-file.jsonl"

echo "[qwen3-asr-e2e] admin ASR directory task run writes text metadata"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/run" \
  | tee "$ASR_DIR/e2e-admin-task-run.json" \
  | grep -q '"processed_now":1'
find "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}" -name '*.txt' -type f | grep -q .
find "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}" -name '*.json' -type f | grep -q .
rm -f "${TASK_AUDIO_DIR}/sample3.wav"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  | tee "$ASR_DIR/e2e-admin-task-after-delete.json" \
  | grep -q '"deleted_after_processing":1'
python3 - "$ASR_DIR/e2e-admin-task-after-delete.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    task = json.load(f)
files = task.get("files") or []
if len(files) != 1:
    raise SystemExit(f"expected one task detail file record, got {len(files)}")
record = files[0]
if record.get("status") != "success" or not record.get("output_text_path") or not record.get("output_timeline_path"):
    raise SystemExit(f"unexpected task detail file record: {record}")
with open(record["output_timeline_path"], "r", encoding="utf-8") as f:
    timeline = json.load(f)
if "segments" not in timeline or timeline.get("media_duration_ms") is None:
    raise SystemExit(f"timeline missing segments or duration: {timeline}")
if timeline.get("source_created_at_ms") is not None and timeline.get("source_created_at_source") is None:
    raise SystemExit(f"timeline missing creation source: {timeline}")
PY

echo "[qwen3-asr-e2e] admin ASR browser microphone webm normalization"
ffmpeg -y -hide_banner -loglevel error -i "${ASR_DIR}/sample3.wav" \
  -c:a libopus "$ASR_DIR/e2e-microphone.webm"
curl -fsSN -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/transcribe-stream?${ASR_QUERY}" \
  -F "file=@${ASR_DIR}/e2e-microphone.webm;type=audio/webm" \
  -F "language=chinese" \
  -F "response_format=text" \
  > "$ASR_DIR/e2e-admin-transcribe-webm.sse"
grep -q 'event: partial' "$ASR_DIR/e2e-admin-transcribe-webm.sse"
grep -q 'event: final' "$ASR_DIR/e2e-admin-transcribe-webm.sse"
grep -q 'event: text' "$ASR_DIR/e2e-admin-transcribe-webm.sse"
grep -q '"phase":"preprocess"' "$ASR_DIR/e2e-admin-transcribe-webm.sse"
grep -E "Qwen3|语音|测试" "$ASR_DIR/e2e-admin-transcribe-webm.sse" >/dev/null

echo "[qwen3-asr-e2e] admin ASR realtime WebSocket transcription"
python3 - "${ADMIN_PORT}" "${ASR_QUERY}" "$ASR_DIR/e2e-microphone.webm" "$ASR_DIR/e2e-admin-transcribe-ws.jsonl" <<'PY'
import base64
import json
import os
import socket
import struct
import sys
import time

admin_port, asr_query, audio_path, out_path = sys.argv[1:]
path = f"/_bifrost/api/asr/transcribe-ws?{asr_query}&flush_interval_ms=500"
key = base64.b64encode(os.urandom(16)).decode()
sock = socket.create_connection(("127.0.0.1", int(admin_port)), timeout=20)
request = (
    f"GET {path} HTTP/1.1\r\n"
    f"Host: 127.0.0.1:{admin_port}\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    "Sec-WebSocket-Version: 13\r\n\r\n"
)
sock.sendall(request.encode())
response = sock.recv(4096)
if b" 101 " not in response.split(b"\r\n", 1)[0]:
    raise SystemExit(f"websocket upgrade failed: {response!r}")

def send_frame(opcode, payload):
    if isinstance(payload, str):
        payload = payload.encode()
    mask = os.urandom(4)
    header = bytearray([0x80 | opcode])
    length = len(payload)
    if length < 126:
        header.append(0x80 | length)
    elif length < 65536:
        header.append(0x80 | 126)
        header.extend(struct.pack("!H", length))
    else:
        header.append(0x80 | 127)
        header.extend(struct.pack("!Q", length))
    masked = bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
    sock.sendall(bytes(header) + mask + masked)

def recv_exact(length):
    chunks = []
    remaining = length
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            raise EOFError("websocket closed")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)

def recv_frame():
    first = recv_exact(2)
    opcode = first[0] & 0x0F
    masked = first[1] & 0x80
    length = first[1] & 0x7F
    if length == 126:
        length = struct.unpack("!H", recv_exact(2))[0]
    elif length == 127:
        length = struct.unpack("!Q", recv_exact(8))[0]
    mask = recv_exact(4) if masked else b""
    payload = recv_exact(length)
    if masked:
        payload = bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
    return opcode, payload

send_frame(0x1, json.dumps({"type": "start", "mime_type": "audio/webm", "file_name": "e2e-microphone.webm"}))
with open(audio_path, "rb") as audio:
    media_bytes = audio.read()
chunk_size = max(1, len(media_bytes) // 4)
open(out_path, "w", encoding="utf-8").close()
seen_before_finish = []
sock.settimeout(2)
try:
    while True:
        opcode, payload = recv_frame()
        if opcode == 0x9:
            send_frame(0xA, payload)
            continue
        if opcode != 0x1:
            continue
        event = json.loads(payload.decode())
        seen_before_finish.append(event.get("type"))
        with open(out_path, "a", encoding="utf-8") as out:
            out.write(json.dumps({**event, "before_finish": True}, ensure_ascii=False) + "\n")
        if event.get("type") == "stream":
            break
except socket.timeout:
    pass
sock.settimeout(30)
for offset in range(0, len(media_bytes), chunk_size):
    send_frame(0x2, media_bytes[offset:offset + chunk_size])
    time.sleep(0.7)
    if offset + chunk_size < len(media_bytes):
        try:
            while True:
                opcode, payload = recv_frame()
                if opcode == 0x9:
                    send_frame(0xA, payload)
                    continue
                if opcode != 0x1:
                    continue
                event = json.loads(payload.decode())
                event_type = event.get("type")
                seen_before_finish.append(event_type)
                with open(out_path, "a", encoding="utf-8") as out:
                    out.write(json.dumps({**event, "before_finish": True}, ensure_ascii=False) + "\n")
                if event_type in {"partial", "final"}:
                    break
        except socket.timeout:
            pass
if not {"partial", "final"}.intersection(seen_before_finish):
    raise SystemExit(f"missing realtime partial/final before finish; seen_before_finish={seen_before_finish}")
send_frame(0x1, json.dumps({"type": "finish"}))

seen = []
deadline = 80
sock.settimeout(deadline)
with open(out_path, "a", encoding="utf-8") as out:
    while True:
        opcode, payload = recv_frame()
        if opcode == 0x9:
            send_frame(0xA, payload)
            continue
        if opcode == 0x8:
            break
        if opcode != 0x1:
            continue
        event = json.loads(payload.decode())
        out.write(json.dumps(event, ensure_ascii=False) + "\n")
        seen.append(event.get("type"))
        if event.get("type") == "done":
            break

sock.close()
required = {"connected", "stream", "partial", "final", "text", "done"}
missing = required.difference([*seen_before_finish, *seen])
if missing:
    raise SystemExit(f"missing websocket event types: {sorted(missing)}; seen_before_finish={seen_before_finish}; seen={seen}")
PY
grep -q '"before_finish": true' "$ASR_DIR/e2e-admin-transcribe-ws.jsonl"
grep -q '"type": "partial"' "$ASR_DIR/e2e-admin-transcribe-ws.jsonl"
grep -q '"type": "final"' "$ASR_DIR/e2e-admin-transcribe-ws.jsonl"
grep -q '"type": "text"' "$ASR_DIR/e2e-admin-transcribe-ws.jsonl"
grep -q 'processed_ms' "$ASR_DIR/e2e-admin-transcribe-ws.jsonl"
grep -E "Qwen3|语音|测试" "$ASR_DIR/e2e-admin-transcribe-ws.jsonl" >/dev/null

echo "[qwen3-asr-e2e] admin ASR error stream for unavailable server"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/service/stop?${ASR_QUERY}" \
  | tee "$ASR_DIR/e2e-admin-stop.json" \
  | grep -q '"managed":false'
curl -sN -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/transcribe-stream?${ASR_QUERY}" \
  -F "file=@${ASR_DIR}/sample3.wav" \
  -F "language=chinese" \
  -F "response_format=text" \
  > "$ASR_DIR/e2e-admin-error.sse"
grep -q 'event: error' "$ASR_DIR/e2e-admin-error.sse"
grep -E 'not ready|not running' "$ASR_DIR/e2e-admin-error.sse" >/dev/null

echo "[qwen3-asr-e2e] admin ASR scheduled directory tasks serialize and restore stopped service"
SCHED_AUDIO_DIR1="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-qwen3-asr-sched-a.XXXXXX")"
SCHED_AUDIO_DIR2="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-qwen3-asr-sched-b.XXXXXX")"
cp "${ASR_DIR}/sample1.wav" "${SCHED_AUDIO_DIR1}/sample1.wav"
cp "${ASR_DIR}/sample2.wav" "${SCHED_AUDIO_DIR2}/sample2.wav"
create_scheduled_task() {
  local name="$1"
  local dir="$2"
  python3 - "$name" "$dir" <<'PY'
import json, sys
from datetime import datetime
now = datetime.now()
print(json.dumps({
    "name": sys.argv[1],
    "audio_dir": sys.argv[2],
    "recursive": True,
    "enabled": True,
    "schedule": {"kind": "daily", "hour": now.hour, "minute": now.minute},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
}
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$(create_scheduled_task "ASR scheduled task A" "$SCHED_AUDIO_DIR1")" \
  > "$ASR_DIR/e2e-admin-scheduled-create-a.json"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$(create_scheduled_task "ASR scheduled task B" "$SCHED_AUDIO_DIR2")" \
  > "$ASR_DIR/e2e-admin-scheduled-create-b.json"
SCHED_TASK_ID1="$(python3 - "$ASR_DIR/e2e-admin-scheduled-create-a.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"
SCHED_TASK_ID2="$(python3 - "$ASR_DIR/e2e-admin-scheduled-create-b.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"
for _ in $(seq 1 36); do
  curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${SCHED_TASK_ID1}" \
    > "$ASR_DIR/e2e-admin-scheduled-a.json"
  curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${SCHED_TASK_ID2}" \
    > "$ASR_DIR/e2e-admin-scheduled-b.json"
  if python3 - "$ASR_DIR/e2e-admin-scheduled-a.json" "$ASR_DIR/e2e-admin-scheduled-b.json" <<'PY'
import json, sys
ok = True
for path in sys.argv[1:]:
    with open(path, "r", encoding="utf-8") as f:
        task = json.load(f)
    summary = task["summary"]
    ok = ok and summary["processed"] == 1 and summary["failed"] == 0 and task.get("last_run_at_ms") is not None
raise SystemExit(0 if ok else 1)
PY
  then
    break
  fi
  sleep 5
done
python3 - "$ASR_DIR/e2e-admin-scheduled-a.json" "$ASR_DIR/e2e-admin-scheduled-b.json" <<'PY'
import json, sys
for path in sys.argv[1:]:
    with open(path, "r", encoding="utf-8") as f:
        task = json.load(f)
    summary = task["summary"]
    if summary["processed"] != 1 or summary["failed"] != 0 or task.get("last_run_at_ms") is None:
        raise SystemExit(f"scheduled task did not finish successfully: {path}: {task}")
PY
find "$ADMIN_DATA_DIR/asr/data/text/${SCHED_TASK_ID1}" -name '*.txt' -type f | grep -q .
find "$ADMIN_DATA_DIR/asr/data/text/${SCHED_TASK_ID2}" -name '*.txt' -type f | grep -q .
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/status?${ASR_QUERY}" \
  | tee "$ASR_DIR/e2e-admin-scheduled-status-after.json" \
  | grep -q '"ready":false'

echo "[qwen3-asr-e2e] PASS"
