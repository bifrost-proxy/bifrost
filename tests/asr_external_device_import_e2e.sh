#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${ZSH_VERSION:-}" ]]; then
  source ~/.zshrc
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_ASR_E2E_PORT:-18880}"
API="http://127.0.0.1:${PORT}/_bifrost/api/asr"
DEVICES="${BIFROST_ASR_E2E_DEVICES:-LEFT,RIGHT}"
IFS=',' read -r DEVICE_LEFT DEVICE_RIGHT <<<"$DEVICES"

if [[ -z "${DEVICE_LEFT:-}" || -z "${DEVICE_RIGHT:-}" ]]; then
  echo "BIFROST_ASR_E2E_DEVICES must contain two comma-separated mounted volume names" >&2
  exit 1
fi

if [[ ! -d "/Volumes/$DEVICE_LEFT" || ! -d "/Volumes/$DEVICE_RIGHT" ]]; then
  if [[ "${BIFROST_ASR_E2E_REQUIRE_DEVICES:-0}" == "1" ]]; then
    echo "Required test volumes /Volumes/$DEVICE_LEFT and /Volumes/$DEVICE_RIGHT are not mounted" >&2
    exit 1
  fi
  echo "Skipping ASR external device E2E: /Volumes/$DEVICE_LEFT and /Volumes/$DEVICE_RIGHT are not both mounted"
  exit 0
fi

cargo build --bin bifrost

DATA_DIR="$(mktemp -d /tmp/bifrost-asr-device-e2e.XXXXXX)"
TARGET_PARENT="$(mktemp -d /tmp/bifrost-asr-target-parent.XXXXXX)"
TARGET_DIR="$TARGET_PARENT/missing-target-dir"
RUN_ID="codex-aedi-$(date +%Y%m%d%H%M%S)"
LEFT_DIR="/Volumes/$DEVICE_LEFT/$RUN_ID/2026-05-21"
RIGHT_DIR="/Volumes/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub"
SERVER_LOG="$DATA_DIR/server.log"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "/Volumes/$DEVICE_LEFT/$RUN_ID" "/Volumes/$DEVICE_RIGHT/$RUN_ID" "$TARGET_PARENT" "$DATA_DIR"
}
trap cleanup EXIT

mkdir -p "$LEFT_DIR" "$RIGHT_DIR"
printf 'left device audio payload %s\n' "$RUN_ID" > "$LEFT_DIR/A.wav"
printf 'right device audio payload %s\n' "$RUN_ID" > "$RIGHT_DIR/B.m4a"
printf 'duplicate audio payload %s\n' "$RUN_ID" > "$LEFT_DIR/duplicate.wav"
printf 'duplicate audio payload %s\n' "$RUN_ID" > "$RIGHT_DIR/duplicate-copy.wav"

BIFROST_DATA_DIR="$DATA_DIR" ./target/debug/bifrost start \
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

node - "$API" "$TARGET_DIR" "$RUN_ID" "$DEVICE_LEFT" "$DEVICE_RIGHT" <<'NODE'
const [api, target, runId, deviceLeft, deviceRight] = process.argv.slice(2);
const must = (cond, msg) => { if (!cond) throw new Error(msg); };
const request = async (path, options = {}) => {
  const res = await fetch(api + path, {
    ...options,
    headers: { "content-type": "application/json", ...(options.headers || {}) },
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : null;
  return { res, json };
};
(async () => {
  const volumes = (await request("/external-volumes")).json.volumes;
  must(volumes.some((v) => v.name === deviceLeft && v.kind === "external"), `${deviceLeft} external volume not detected`);
  must(volumes.some((v) => v.name === deviceRight && v.kind === "external"), `${deviceRight} external volume not detected`);
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
      external_devices: [{ name: deviceLeft, enabled: true }, { name: deviceRight, enabled: true }],
      import_policy: {
        enabled: true,
        file_stable_secs: 0,
        min_free_bytes: 1,
        max_file_bytes: 104857600,
        auto_run_after_import: false,
        content_hash_dedupe_enabled: true,
        content_hash_algorithm: "sha256",
        delete_source_after_import: false,
      },
    }),
  });
  must(create.res.ok, `create failed ${create.res.status}`);
  const task = create.json;
  const run = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(run.res.ok, `import failed ${run.res.status}`);
  must(run.json.imported >= 4, `expected at least 4 imports, got ${run.json.imported}`);
  const repeat = await request(`/tasks/${encodeURIComponent(task.id)}/external-import/run`, { method: "POST", body: "{}" });
  must(repeat.res.ok, `repeat import failed ${repeat.res.status}`);
  must(repeat.json.imported === 0, `repeat import should be differential, got ${repeat.json.imported}`);
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
        content_hash_algorithm: "sha256",
        delete_source_after_import: false,
      },
    }),
  });
  must(edit.res.ok, `edit failed ${edit.res.status}`);
  const badDelete = await request(`/tasks/${encodeURIComponent(task.id)}?confirm_name=wrong`, { method: "DELETE" });
  must(badDelete.res.status === 400, `wrong-name delete should fail 400, got ${badDelete.res.status}`);
  const goodDelete = await request(`/tasks/${encodeURIComponent(task.id)}?confirm_name=${encodeURIComponent(edit.json.name)}`, { method: "DELETE" });
  must(goodDelete.res.ok, `confirmed delete failed ${goodDelete.res.status}`);
  console.log(JSON.stringify({ taskId: task.id, runId, imported: run.json.imported, repeatImported: repeat.json.imported }));
})().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
NODE

test -f "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/A.wav"
test -f "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/duplicate.wav"
test -f "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/B.m4a"
test -f "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/duplicate-copy.wav"
if find "$TARGET_DIR" -name '._*' -print -quit | grep -q .; then
  echo "AppleDouble metadata files should not be imported into $TARGET_DIR" >&2
  find "$TARGET_DIR" -type f | sort >&2
  exit 1
fi
cmp "$LEFT_DIR/A.wav" "$TARGET_DIR/$DEVICE_LEFT/$RUN_ID/2026-05-21/A.wav"
cmp "$RIGHT_DIR/B.m4a" "$TARGET_DIR/$DEVICE_RIGHT/$RUN_ID/2026-05-21/sub/B.m4a"

echo "ASR external device E2E passed for $RUN_ID"
