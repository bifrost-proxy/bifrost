#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

if [[ -n "${ZSH_VERSION:-}" ]]; then
  source ~/.zshrc
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_ASR_E2E_PORT:-18880}"
API="http://127.0.0.1:${PORT}/_bifrost/api/asr"
if [[ -z "${BIFROST_ASR_E2E_DEVICES:-}" ]]; then
  echo "Skipping ASR external device E2E: set BIFROST_ASR_E2E_DEVICES to explicit isolated test volume names"
  exit 0
fi
DEVICES="$BIFROST_ASR_E2E_DEVICES"
IFS=',' read -r DEVICE_LEFT DEVICE_RIGHT <<<"$DEVICES"

if [[ -z "${DEVICE_LEFT:-}" ]]; then
  echo "BIFROST_ASR_E2E_DEVICES must contain at least one mounted volume name" >&2
  exit 1
fi

if [[ ! -d "/Volumes/$DEVICE_LEFT" || ( -n "${DEVICE_RIGHT:-}" && ! -d "/Volumes/$DEVICE_RIGHT" ) ]]; then
  if [[ "${BIFROST_ASR_E2E_REQUIRE_DEVICES:-0}" == "1" ]]; then
    echo "Required test volumes from BIFROST_ASR_E2E_DEVICES=$DEVICES are not mounted" >&2
    exit 1
  fi
  echo "Skipping ASR external device E2E: required test volumes from BIFROST_ASR_E2E_DEVICES=$DEVICES are not mounted"
  exit 0
fi

cargo build --bin bifrost

DATA_DIR="$(mktemp -d /tmp/bifrost-asr-device-e2e.XXXXXX)"
TARGET_PARENT="$(mktemp -d /tmp/bifrost-asr-target-parent.XXXXXX)"
TARGET_DIR="$TARGET_PARENT/missing-target-dir"
RUN_ID="codex-aedi-$(date +%Y%m%d%H%M%S)"
LEFT_DIR="/Volumes/$DEVICE_LEFT/$RUN_ID/2026-05-21"
RIGHT_DIR=""
if [[ -n "${DEVICE_RIGHT:-}" ]]; then
  RIGHT_DIR="/Volumes/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub"
fi
SERVER_LOG="$DATA_DIR/server.log"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "/Volumes/$DEVICE_LEFT/$RUN_ID"
  if [[ -n "${DEVICE_RIGHT:-}" ]]; then
    rm -rf "/Volumes/$DEVICE_RIGHT/$RUN_ID"
  fi
  rm -rf "$TARGET_PARENT" "$DATA_DIR"
}
trap cleanup EXIT

mkdir -p "$LEFT_DIR"
printf 'left device audio payload %s\n' "$RUN_ID" > "$LEFT_DIR/A.wav"
printf 'duplicate audio payload %s\n' "$RUN_ID" > "$LEFT_DIR/duplicate.wav"
if [[ -n "${DEVICE_RIGHT:-}" ]]; then
  mkdir -p "$RIGHT_DIR"
  printf 'right device audio payload %s\n' "$RUN_ID" > "$RIGHT_DIR/B.m4a"
  printf 'duplicate audio payload %s\n' "$RUN_ID" > "$RIGHT_DIR/duplicate-copy.wav"
fi

BIFROST_DATA_DIR="$DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 ./target/debug/bifrost start \
  -p "$PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  --skip-cert-check \
  --access-mode allow_all \
  >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 60); do
  if curl -fsS "$API/external-volumes" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "$API/external-volumes" >/dev/null

node - "$API" "$TARGET_DIR" "$RUN_ID" "$DEVICE_LEFT" "$DEVICE_RIGHT" "$DATA_DIR" <<'NODE'
const fs = require("fs");
const path = require("path");
const [api, target, runId, deviceLeft, deviceRight, dataDir] = process.argv.slice(2);
const must = (cond, msg) => { if (!cond) throw new Error(msg); };
let csrfToken = null;
const request = async (path, options = {}) => {
  const res = await fetch(api + path, {
    ...options,
    headers: {
      "content-type": "application/json",
      ...(csrfToken ? { "x-bifrost-csrf": csrfToken } : {}),
      ...(options.headers || {}),
    },
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : null;
  return { res, json };
};
(async () => {
  const csrfRes = await fetch(`${api.replace(/\/asr$/, "")}/security/csrf`);
  const csrf = await csrfRes.json();
  must(csrfRes.ok && csrf.csrf_token, "CSRF token should be available");
  csrfToken = csrf.csrf_token;
  const volumes = (await request("/external-volumes")).json.volumes;
  must(volumes.some((v) => v.name === deviceLeft && ["external", "mounted"].includes(v.kind)), `${deviceLeft} test volume not detected`);
  if (deviceRight) {
    must(volumes.some((v) => v.name === deviceRight && ["external", "mounted"].includes(v.kind)), `${deviceRight} test volume not detected`);
  }
  const externalDevices = [{ name: deviceLeft, enabled: true }];
  if (deviceRight) externalDevices.push({ name: deviceRight, enabled: true });
  const create = await request("/tasks", {
    method: "POST",
    body: JSON.stringify({
      name: `External Device E2E ${runId}`,
      audio_dir: target,
      recursive: true,
      enabled: false,
      schedule: { kind: "hourly", minute: 0 },
      language: "chinese",
      model: "Qwen3-ASR-1.7B",
      runtime_strategy: "reuse_per_file",
      external_devices: externalDevices,
      import_policy: {
        enabled: true,
        file_stable_secs: 0,
        min_free_bytes: 1,
        max_file_bytes: 104857600,
        auto_run_after_import: false,
        content_hash_dedupe_enabled: true,
        content_hash_algorithm: "blake3",
        delete_source_after_import: false,
      },
    }),
  });
  must(create.res.ok, `create failed ${create.res.status}`);
  const task = create.json;
  const waitForImport = async () => {
    let latest = null;
    for (let attempt = 0; attempt < 120; attempt += 1) {
      const status = await request(`/tasks/${encodeURIComponent(task.id)}/external-import`);
      must(status.res.ok, `status failed ${status.res.status}`);
      latest = status.json.current_run;
      if (latest && latest.status !== "importing") return latest;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    throw new Error("external import did not finish in time");
  };
  const started = Date.now();
  const run = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  const runDurationMs = Date.now() - started;
  must(run.res.status === 202, `import should start asynchronously with 202, got ${run.res.status}`);
  must(runDurationMs < 1500, `import start should return quickly, took ${runDurationMs}ms`);
  must(run.json.running === true, "import response should report running=true");
  must(run.json.progress && run.json.progress.status === "importing", "import response should include importing progress");
  const taskListDuringImport = await request("/tasks");
  must(taskListDuringImport.res.ok, "tasks API should remain responsive while external import runs");
  const completed = await waitForImport();
  const expectedImports = deviceRight ? 4 : 2;
  must(completed.imported >= expectedImports, `expected at least ${expectedImports} imports, got ${completed.imported}`);
  must(completed.processed_files >= expectedImports, `expected progress to process at least ${expectedImports} files, got ${completed.processed_files}`);
  const repeat = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(repeat.res.status === 202, `repeat import should start asynchronously with 202, got ${repeat.res.status}`);
  const repeatCompleted = await waitForImport();
  must(repeatCompleted.imported === 0, `repeat import should be differential, got ${repeatCompleted.imported}`);
  const leftTarget = path.join(target, deviceLeft, runId, "2026-05-21", "A.wav");
  fs.unlinkSync(leftTarget);
  const pause = await request(`/tasks/${encodeURIComponent(task.id)}/pause`, { method: "POST", body: "{}" });
  must(pause.res.ok, `pause before deleted-file reimport failed ${pause.res.status}`);
  const reimport = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(reimport.res.status === 202, `paused deleted-file reimport should start with 202, got ${reimport.res.status}`);
  const reimportCompleted = await waitForImport();
  must(reimportCompleted.imported >= 1, `deleted target file should be reimported, got ${reimportCompleted.imported}`);
  must(fs.existsSync(leftTarget), "deleted target file should exist after reimport");
  const processedSize = fs.statSync(leftTarget).size;
  const now = Date.now();
  const fileStorePath = path.join(dataDir, "asr", "tasks", task.id, "files.json");
  const processedText = path.join(dataDir, "asr", "processed", "processed-left-target.txt");
  const processedMetadata = path.join(dataDir, "asr", "processed", "processed-left-target.json");
  fs.mkdirSync(path.dirname(fileStorePath), { recursive: true });
  fs.mkdirSync(path.dirname(processedText), { recursive: true });
  fs.writeFileSync(processedText, "processed");
  fs.writeFileSync(processedMetadata, "{}");
  fs.writeFileSync(fileStorePath, JSON.stringify({
    version: 1,
    files: {
      "processed-left-target": {
        task_id: task.id,
        source_path: leftTarget,
        source_size: processedSize,
        source_modified_ms: null,
        source_created_at_ms: null,
        source_created_at_source: null,
        content_hash: null,
        content_hash_algorithm: null,
        duplicate_of_source_key: null,
        transcript_alias: null,
        media_duration_ms: null,
        status: "success",
        output_text_path: processedText,
        output_metadata_path: processedMetadata,
        output_timeline_path: null,
        text_chars: 9,
        error: null,
        runtime_strategy: "reuse_per_file",
        chunk_metrics: [],
        fallback_reason: null,
        started_at_ms: now - 1000,
        finished_at_ms: now,
        progress_current: null,
        progress_total: null,
        failed_chunks: [],
        memory_limit_hints: [],
      },
    },
  }, null, 2));
  fs.unlinkSync(leftTarget);
  const skipProcessed = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(skipProcessed.res.status === 202, `processed-record import should start with 202, got ${skipProcessed.res.status}`);
  const processedSkipCompleted = await waitForImport();
  must(processedSkipCompleted.imported === 0, `already processed deleted target should not be reimported, got ${processedSkipCompleted.imported}`);
  must(processedSkipCompleted.processed_record_skipped >= 1, `already processed skip count should be visible, got ${processedSkipCompleted.processed_record_skipped}`);
  must(!fs.existsSync(leftTarget), "already processed deleted target should stay absent after import");

  // Regression: old compression state may leave the import manifest pointing
  // at the removed WAV while the reusable canonical source is now a FLAC.
  // The original WAV hash must prevent any new local copy before ASR discovery.
  const duplicateTarget = path.join(target, deviceLeft, runId, "2026-05-21", "duplicate.wav");
  const compressedTarget = duplicateTarget.replace(/\.wav$/, ".flac");
  const externalStorePath = path.join(dataDir, "asr", "tasks", task.id, "external_imports.json");
  const externalStore = JSON.parse(fs.readFileSync(externalStorePath, "utf8"));
  const importedDuplicate = Object.values(externalStore.devices)
    .flatMap((device) => Object.values(device.files))
    .find((record) => record.target_path === duplicateTarget);
  must(importedDuplicate, "duplicate import manifest record should exist");
  const originalHash = importedDuplicate.source_hashes?.blake3;
  must(originalHash, "duplicate import manifest should retain original BLAKE3");
  const originalSize = fs.statSync(duplicateTarget).size;
  fs.renameSync(duplicateTarget, compressedTarget);
  const compressedText = path.join(dataDir, "asr", "processed", "compressed-duplicate.txt");
  const compressedMetadata = path.join(dataDir, "asr", "processed", "compressed-duplicate.json");
  fs.mkdirSync(path.dirname(compressedText), { recursive: true });
  fs.writeFileSync(compressedText, "compressed transcript");
  fs.writeFileSync(compressedMetadata, "{}");
  const fileStore = JSON.parse(fs.readFileSync(fileStorePath, "utf8"));
  fileStore.files["compressed-duplicate-key"] = {
    task_id: task.id,
    source_path: compressedTarget,
    source_size: fs.statSync(compressedTarget).size,
    source_modified_ms: null,
    source_created_at_ms: null,
    source_created_at_source: null,
    content_hash: originalHash,
    content_hash_algorithm: "blake3",
    duplicate_of_source_key: null,
    transcript_alias: null,
    media_duration_ms: null,
    status: "success",
    output_text_path: compressedText,
    output_metadata_path: compressedMetadata,
    output_timeline_path: null,
    text_chars: 21,
    error: null,
    runtime_strategy: "reuse_per_file",
    chunk_metrics: [],
    fallback_reason: null,
    started_at_ms: now - 1000,
    finished_at_ms: now,
    progress_current: null,
    progress_total: null,
    failed_chunks: [],
    memory_limit_hints: [],
  };
  fs.writeFileSync(fileStorePath, JSON.stringify(fileStore, null, 2));
  const contentHashIndexPath = path.join(dataDir, "asr", "tasks", task.id, "content_hash_index.json");
  fs.writeFileSync(contentHashIndexPath, JSON.stringify({
    version: 1,
    hashes: {
      [`blake3:${originalHash}`]: {
        algorithm: "blake3",
        hash: originalHash,
        size: originalSize,
        canonical_source_key: "compressed-duplicate-key",
        canonical_source_path: compressedTarget,
        transcript_artifacts: {
          text_path: compressedText,
          metadata_path: compressedMetadata,
          timeline_path: null,
        },
        model: "Qwen3-ASR-1.7B",
        language: "chinese",
        runtime_strategy: "reuse_per_file",
        completed_at_ms: now,
        duplicate_count: 0,
      },
    },
  }, null, 2));
  const compressedSkip = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(compressedSkip.res.status === 202, `compressed-source import should start with 202, got ${compressedSkip.res.status}`);
  const compressedSkipCompleted = await waitForImport();
  must(compressedSkipCompleted.imported === 0, `compressed original WAV should not be copied again, got ${compressedSkipCompleted.imported}`);
  must(compressedSkipCompleted.processed_record_skipped >= 2, `path and hash skips should be visible, got ${compressedSkipCompleted.processed_record_skipped}`);
  must(!fs.existsSync(duplicateTarget), "compressed original WAV should remain absent after import");
  must(fs.existsSync(compressedTarget), "canonical FLAC should remain available after import");
  const edit = await request(`/tasks/${encodeURIComponent(task.id)}`, {
    method: "PATCH",
    body: JSON.stringify({
      name: `External Device E2E Edited ${runId}`,
      audio_dir: target,
      recursive: false,
      enabled: true,
      schedule: { kind: "daily", hour: 3, minute: 15 },
      language: "english",
      model: "Qwen3-ASR-1.7B",
      runtime_strategy: "fork_per_chunk",
      external_devices: [{ name: deviceLeft, enabled: true }],
      import_policy: {
        enabled: true,
        file_stable_secs: 0,
        min_free_bytes: 1,
        max_file_bytes: 104857600,
        auto_run_after_import: false,
        content_hash_dedupe_enabled: true,
        content_hash_algorithm: "blake3",
        delete_source_after_import: false,
      },
    }),
  });
  must(edit.res.ok, `edit failed ${edit.res.status}`);
  const badDelete = await request(`/tasks/${encodeURIComponent(task.id)}?confirm_name=wrong`, { method: "DELETE" });
  must(badDelete.res.status === 400, `wrong-name delete should fail 400, got ${badDelete.res.status}`);
  const goodDelete = await request(`/tasks/${encodeURIComponent(task.id)}?confirm_name=${encodeURIComponent(edit.json.name)}`, { method: "DELETE" });
  must(goodDelete.res.ok, `confirmed delete failed ${goodDelete.res.status}`);
  console.log(JSON.stringify({ taskId: task.id, runId, imported: completed.imported, repeatImported: repeatCompleted.imported, reimportedAfterDelete: reimportCompleted.imported, processedRecordSkipped: processedSkipCompleted.processed_record_skipped, compressedSourceSkipped: compressedSkipCompleted.processed_record_skipped, runDurationMs }));
})().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
NODE

test ! -e "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/A.wav"
test ! -e "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/duplicate.wav"
test -f "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/duplicate.flac"
if [[ -n "${DEVICE_RIGHT:-}" ]]; then
  test -f "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/B.m4a"
  test -f "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/duplicate-copy.wav"
fi
if find "$TARGET_DIR" -name '._*' -print -quit | grep -q .; then
  echo "AppleDouble metadata files should not be imported into $TARGET_DIR" >&2
  find "$TARGET_DIR" -type f | sort >&2
  exit 1
fi
cmp "$LEFT_DIR/duplicate.wav" "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/duplicate.flac"
if [[ -n "${DEVICE_RIGHT:-}" ]]; then
  cmp "$RIGHT_DIR/B.m4a" "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/B.m4a"
fi

echo "ASR external device E2E passed for $RUN_ID"
