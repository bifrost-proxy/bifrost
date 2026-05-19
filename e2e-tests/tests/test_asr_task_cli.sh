#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-task-cli-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_TASK_CLI_E2E_PORT:-18990}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-cli.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-audio.XXXXXX")"
OUTPUT_DOC="$(mktemp "${TMPDIR:-/tmp}/bifrost-asr-task-day.XXXXXX.md")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR" "$OUTPUT_DOC"
}
trap cleanup EXIT

echo "[asr-task-cli-e2e] build current bifrost binary"
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"

echo "[asr-task-cli-e2e] start temporary Bifrost on ${ADMIN_PORT}"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" start \
  -p "$ADMIN_PORT" \
  --unsafe-ssl \
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

TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json
import sys
print(json.dumps({
    "name": "ASR CLI E2E task",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
)"

echo "[asr-task-cli-e2e] create ASR task through admin API"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$TASK_JSON" >"$ADMIN_DATA_DIR/task-create.json"
TASK_ID="$(python3 - "$ADMIN_DATA_DIR/task-create.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"

mkdir -p "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}/daily"
cat > "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}/daily/2026-05-17.md" <<'EOF'
# ASR CLI E2E task — 2026-05-17

完整内容整理的文档展示。
EOF

echo "[asr-task-cli-e2e] task list uses runtime port when -p is omitted"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task list >"$ADMIN_DATA_DIR/task-list.out"
grep -q "$TASK_ID" "$ADMIN_DATA_DIR/task-list.out"

echo "[asr-task-cli-e2e] task show exposes daily document count"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task show "$TASK_ID" >"$ADMIN_DATA_DIR/task-show.out"
grep -q "Daily documents: 1" "$ADMIN_DATA_DIR/task-show.out"

echo "[asr-task-cli-e2e] task files handles empty task"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task files "$TASK_ID" >"$ADMIN_DATA_DIR/task-files.out"
grep -q "No ASR task files matched" "$ADMIN_DATA_DIR/task-files.out"

echo "[asr-task-cli-e2e] daily list and show expose generated markdown"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily list "$TASK_ID" >"$ADMIN_DATA_DIR/daily-list.out"
grep -q "2026-05-17" "$ADMIN_DATA_DIR/daily-list.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily show "$TASK_ID" 2026-05-17 >"$ADMIN_DATA_DIR/daily-show.out"
grep -q "完整内容整理" "$ADMIN_DATA_DIR/daily-show.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily show "$TASK_ID" 2026-05-17 --output "$OUTPUT_DOC" >/dev/null
grep -q "ASR CLI E2E task" "$OUTPUT_DOC"

echo "[asr-task-cli-e2e] run --wait refreshes daily documents without requiring ASR model when no files are pending"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task run "$TASK_ID" --wait >"$ADMIN_DATA_DIR/task-run.out"
grep -q "ASR task completed" "$ADMIN_DATA_DIR/task-run.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily list "$TASK_ID" >"$ADMIN_DATA_DIR/daily-list-after-run.out"
grep -q "2026-05-17" "$ADMIN_DATA_DIR/daily-list-after-run.out"

echo "[asr-task-cli-e2e] PASS"
