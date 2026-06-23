#!/usr/bin/env bash

set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

ADMIN_PORT="${BIFROST_ASR_DAILY_SYNC_HASH_E2E_PORT:-${ADMIN_PORT:-18994}}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-sync-hash.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-sync-hash-audio.XXXXXX")"
SYNC_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-sync-hash-target.XXXXXX")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR" "$SYNC_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
  echo "[asr-daily-sync-hash-e2e] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-daily-sync-hash-e2e] build current bifrost binary"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[asr-daily-sync-hash-e2e] error: Bifrost binary not executable: $BIFROST_BIN" >&2
  exit 1
fi

echo "[asr-daily-sync-hash-e2e] start temporary Bifrost on ${ADMIN_PORT}"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" start \
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
  echo "[asr-daily-sync-hash-e2e] error: Bifrost admin did not start" >&2
  exit 1
}

TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json
import sys

print(json.dumps({
    "name": "ASR Daily Sync Hash E2E",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
)"

echo "[asr-daily-sync-hash-e2e] create task and report fixture"
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

REPORT_DIR="$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}/.daily/report"
mkdir -p "$REPORT_DIR"
cat > "$REPORT_DIR/2026-05-17-report.md" <<'EOF'
# ASR Daily Sync Hash E2E

报告同步目录验证内容。
EOF

echo "[asr-daily-sync-hash-e2e] configure sync dir"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily set-sync-dir "$TASK_ID" --dir "$SYNC_DIR" \
  >"$ADMIN_DATA_DIR/daily-set-sync-dir.out"
grep -q "$SYNC_DIR" "$ADMIN_DATA_DIR/daily-set-sync-dir.out"

echo "[asr-daily-sync-hash-e2e] first sync copies report"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily sync "$TASK_ID" --json \
  >"$ADMIN_DATA_DIR/daily-sync-first.json"
python3 - "$ADMIN_DATA_DIR/daily-sync-first.json" "$SYNC_DIR" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    result = json.load(f)
sync = result["sync"]
assert result["ok"] is True, result
assert sync["target_dir"] == sys.argv[2], sync
assert sync["total_files"] == 1, sync
assert sync["copied_files"] == 1, sync
assert sync["skipped_files"] == 0, sync
assert sync["failed_files"] == 0, sync
PY
grep -q "报告同步目录验证内容" "$SYNC_DIR/daily_report/2026-05-17-report.md"

echo "[asr-daily-sync-hash-e2e] second sync repairs hash mismatch"
printf 'stale-short\n' > "$SYNC_DIR/daily_report/2026-05-17-report.md"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily sync "$TASK_ID" --json \
  >"$ADMIN_DATA_DIR/daily-sync-second.json"
python3 - "$ADMIN_DATA_DIR/daily-sync-second.json" "$SYNC_DIR" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    result = json.load(f)
sync = result["sync"]
assert result["ok"] is True, result
assert sync["target_dir"] == sys.argv[2], sync
assert sync["total_files"] == 1, sync
assert sync["copied_files"] == 1, sync
assert sync["skipped_files"] == 0, sync
assert sync["failed_files"] == 0, sync
PY
grep -q "报告同步目录验证内容" "$SYNC_DIR/daily_report/2026-05-17-report.md"

echo "[asr-daily-sync-hash-e2e] third sync skips matching hash"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily sync "$TASK_ID" --json \
  >"$ADMIN_DATA_DIR/daily-sync-third.json"
python3 - "$ADMIN_DATA_DIR/daily-sync-third.json" "$SYNC_DIR" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    result = json.load(f)
sync = result["sync"]
assert result["ok"] is True, result
assert sync["target_dir"] == sys.argv[2], sync
assert sync["total_files"] == 1, sync
assert sync["copied_files"] == 0, sync
assert sync["skipped_files"] == 1, sync
assert sync["failed_files"] == 0, sync
PY

echo "[asr-daily-sync-hash-e2e] PASS"
