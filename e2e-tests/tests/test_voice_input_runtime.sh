#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fail() {
  echo "[voice-input-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_VOICE_E2E_PORT:-18887}"
ADMIN_DATA_DIR=""
CLI_DATA_DIR=""
ADMIN_PID=""

cleanup() {
  if [[ -n "${ADMIN_PID:-}" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${ADMIN_DATA_DIR:-}" ]]; then
    rm -rf "$ADMIN_DATA_DIR"
  fi
  if [[ -n "${CLI_DATA_DIR:-}" ]]; then
    rm -rf "$CLI_DATA_DIR"
  fi
  rm -f /tmp/bifrost-voice-help.out /tmp/bifrost-voice-listen-help.out /tmp/bifrost-voice-vocabulary-help.out
  rm -f /tmp/bifrost-voice-stream-test.aiff /tmp/bifrost-voice-stream-test.wav
}
trap cleanup EXIT

echo "[voice-input-e2e] syntax checks"
bash -n "$0"

cd "$ROOT_DIR"

echo "[voice-input-e2e] CLI help exposes voice commands"
cargo run --quiet --bin bifrost -- ai voice --help > /tmp/bifrost-voice-help.out
grep -q "sources" /tmp/bifrost-voice-help.out
cargo run --quiet --bin bifrost -- ai voice listen --help > /tmp/bifrost-voice-listen-help.out
grep -q -- "--chunk-ms" /tmp/bifrost-voice-listen-help.out
if grep -Eq -- "--window-ms|--overlap-ms|--allow-asr-autostart|--warmup-asr" /tmp/bifrost-voice-listen-help.out; then
  fail "voice listen help must not expose legacy realtime window/asr-server flags"
fi
cargo run --quiet --bin bifrost -- ai voice vocabulary --help > /tmp/bifrost-voice-vocabulary-help.out
grep -q "import" /tmp/bifrost-voice-vocabulary-help.out

CLI_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-voice-cli.XXXXXX")"
TERMS_FILE="$CLI_DATA_DIR/terms.txt"
cat >"$TERMS_FILE" <<'EOF'
Bifrost=宽增,白 Frost
EOF

echo "[voice-input-e2e] CLI vocabulary import/list and dry-run transcript"
BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai voice vocabulary import "$TERMS_FILE" \
  >"$CLI_DATA_DIR/import.out"
grep -q "Imported 1 voice vocabulary terms" "$CLI_DATA_DIR/import.out"
BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai voice vocabulary list --json \
  >"$CLI_DATA_DIR/vocabulary.json"
grep -q '"canonical": "Bifrost"' "$CLI_DATA_DIR/vocabulary.json"
BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai voice sources --json \
  >"$CLI_DATA_DIR/sources.json"
grep -q '"sources"' "$CLI_DATA_DIR/sources.json"
grep -q '"file:realtime"' "$CLI_DATA_DIR/sources.json"
BIFROST_DATA_DIR="$CLI_DATA_DIR" cargo run --quiet --bin bifrost -- ai voice listen \
  --source mic \
  --dry-run \
  --text "请打开宽增" \
  --format jsonl >"$CLI_DATA_DIR/dry-run.jsonl"
grep -q '"type":"source_ready"' "$CLI_DATA_DIR/dry-run.jsonl"
grep -q '"type":"asr_final_utterance"' "$CLI_DATA_DIR/dry-run.jsonl"
grep -q '"text":"请打开Bifrost"' "$CLI_DATA_DIR/dry-run.jsonl"

echo "[voice-input-e2e] start Bifrost admin on ${ADMIN_PORT}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-voice-admin.XXXXXX")"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" BIFROST_VOICE_ENABLE_FAKE_STATEFUL=1 cargo run --bin bifrost -- start \
  -p "$ADMIN_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  --skip-cert-check >"$ADMIN_DATA_DIR/bifrost.log" 2>&1 &
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

if [[ "${BIFROST_VOICE_E2E_REAL_ASR:-0}" == "1" ]]; then
  command -v say >/dev/null 2>&1 || fail "BIFROST_VOICE_E2E_REAL_ASR=1 requires macOS say"
  command -v ffmpeg >/dev/null 2>&1 || fail "BIFROST_VOICE_E2E_REAL_ASR=1 requires ffmpeg"

  echo "[voice-input-e2e] CLI pushes realtime audio to Voice service and streams stdout events"
  say -r 120 -o /tmp/bifrost-voice-stream-test.aiff \
    "hello bifrost streaming voice input test. this is the first sentence. now we continue speaking for a little longer. the command must print transcript events before this audio stream has finished. this is the final sentence."
  ffmpeg -hide_banner -loglevel error -y \
    -i /tmp/bifrost-voice-stream-test.aiff \
    -ar 16000 -ac 1 /tmp/bifrost-voice-stream-test.wav
  AUDIO_DURATION_MS="$(ffprobe -v error -show_entries format=duration -of csv=p=0 /tmp/bifrost-voice-stream-test.wav \
    | awk '{ printf "%d", ($1 * 1000) }')"
  BIFROST_DATA_DIR="$CLI_DATA_DIR/real-asr" cargo run --quiet --bin bifrost -- -p "$ADMIN_PORT" ai voice listen \
    --source file \
    --input-file /tmp/bifrost-voice-stream-test.wav \
    --duration 7 \
    --chunk-ms 1000 \
    --format jsonl >"$CLI_DATA_DIR/stream.jsonl"
  node - "$CLI_DATA_DIR/stream.jsonl" "$AUDIO_DURATION_MS" <<'NODE'
const fs = require("fs");
const [path, durationMsRaw] = process.argv.slice(2);
const durationMs = Number(durationMsRaw);
const events = fs.readFileSync(path, "utf8")
  .trim()
  .split(/\n+/)
  .filter(Boolean)
  .map((line) => JSON.parse(line));
const partials = events.filter((event) => event.type === "asr_partial");
if (partials.length < 2) {
  throw new Error(`expected at least 2 streaming partial events, got ${partials.length}`);
}
const first = partials[0];
if (!(first.captured_at_ms < durationMs)) {
  throw new Error(`first partial was captured after full input: captured=${first.captured_at_ms}, duration=${durationMs}`);
}
if (!(first.emitted_at_ms < durationMs)) {
  throw new Error(`first partial was emitted after full input: emitted=${first.emitted_at_ms}, duration=${durationMs}`);
}
if (!events.some((event) => event.type === "asr_final_utterance")) {
  throw new Error("missing final utterance event");
}
if (!events.some((event) => event.type === "done")) {
  throw new Error("missing done event");
}
NODE
else
  echo "[voice-input-e2e] skipping real ASR stream proof; set BIFROST_VOICE_E2E_REAL_ASR=1 to run it"
fi

echo "[voice-input-e2e] Admin Voice API sources/status"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/voice/sources" \
  | tee "$ADMIN_DATA_DIR/sources.json" \
  | grep -q '"web_mic"'
grep -q '"file:realtime"' "$ADMIN_DATA_DIR/sources.json"
grep -q '"system"' "$ADMIN_DATA_DIR/sources.json"
grep -q '"app"' "$ADMIN_DATA_DIR/sources.json"

curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/voice/status" \
  | tee "$ADMIN_DATA_DIR/status.json" \
  | grep -q '"local_only":true'
grep -q '"audio_leaves_device":false' "$ADMIN_DATA_DIR/status.json"
grep -q '"qwen3_stateful_streaming"' "$ADMIN_DATA_DIR/status.json"
if grep -q '"qwen3_rs_http_chunked"' "$ADMIN_DATA_DIR/status.json"; then
  fail "Voice realtime status must not expose legacy qwen3_rs_http_chunked provider"
fi

echo "[voice-input-e2e] Admin Voice API vocabulary"
curl -fsS -X PUT "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/voice/vocabulary" \
  -H 'Content-Type: application/json' \
  --data '{"terms":[{"canonical":"Bifrost","aliases":["宽增","白 Frost"],"category":"project"}]}' \
  | tee "$ADMIN_DATA_DIR/vocabulary-put.json" \
  | grep -q '"canonical":"Bifrost"\|"canonical": "Bifrost"'
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/voice/vocabulary" \
  | tee "$ADMIN_DATA_DIR/vocabulary-get.json" \
  | grep -q '"宽增"'
test -f "$ADMIN_DATA_DIR/voice/vocabulary.json"

echo "[voice-input-e2e] Admin Voice WebSocket protocol"
node - "$ADMIN_PORT" <<'NODE'
const port = process.argv[2];
const url = `ws://127.0.0.1:${port}/_bifrost/api/voice/listen-ws?source=web_mic&mock_text=${encodeURIComponent("请打开宽增")}`;
const ws = new WebSocket(url);
const seen = [];
const fail = (message) => {
  console.error(`[voice-input-e2e] ${message}`);
  process.exit(1);
};
const timeout = setTimeout(() => fail(`timeout waiting for Voice WebSocket events: ${seen.join(",")}`), 10000);
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "start", source: "web_mic", sample_rate: 16000, channels: 1, format: "pcm_s16le" }));
  ws.send(JSON.stringify({ type: "audio", sequence: 1, duration_ms: 1000, data: Buffer.from([0, 1, 2, 3]).toString("base64") }));
  ws.send(JSON.stringify({ type: "finish" }));
};
ws.onerror = (event) => fail(`websocket error: ${event.message || "unknown"}`);
ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  seen.push(payload.type);
  if (payload.type === "asr_final_utterance" && payload.text !== "请打开Bifrost") {
    fail(`unexpected final text: ${payload.text}`);
  }
  if (payload.type === "done") {
    clearTimeout(timeout);
    for (const type of ["source_ready", "asr_partial", "asr_final_utterance", "done"]) {
      if (!seen.includes(type)) fail(`missing websocket event ${type}; seen=${seen.join(",")}`);
    }
    ws.close();
    process.exit(0);
  }
};
NODE

echo "[voice-input-e2e] Admin Voice WebSocket max utterance boundary"
node - "$ADMIN_PORT" <<'NODE'
const port = process.argv[2];
const query = `source=web_mic&fake_stateful_worker=1&fake_stateful_text=${encodeURIComponent("hello Bifrost runtime")}&max_utterance_ms=0`;
const url = `ws://127.0.0.1:${port}/_bifrost/api/voice/listen-ws?${query}`;
const ws = new WebSocket(url);
const seen = [];
const fail = (message) => {
  console.error(`[voice-input-e2e] ${message}`);
  process.exit(1);
};
const timeout = setTimeout(() => fail(`timeout waiting for max utterance boundary: ${seen.map((e) => e.type).join(",")}`), 10000);
const speech = Buffer.alloc(3200);
for (let i = 0; i < speech.length; i += 2) speech.writeInt16LE(10000, i);
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "start", source: "web_mic", sample_rate: 16000, channels: 1, format: "pcm_s16le" }));
  ws.send(JSON.stringify({ type: "audio", sequence: 1, duration_ms: 100, data: speech.toString("base64") }));
  setTimeout(() => ws.send(JSON.stringify({ type: "finish" })), 20);
};
ws.onerror = (event) => fail(`websocket error: ${event.message || "unknown"}`);
ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  seen.push(payload);
  if (payload.type === "done") {
    clearTimeout(timeout);
    const stable = seen.find((item) => item.type === "asr_stable_delta" && String(item.detail || "").includes("reason=max_utterance_duration"));
    if (!stable) fail(`missing max_utterance_duration stable event; seen=${seen.map((item) => `${item.type}:${item.detail || ""}`).join(",")}`);
    if (stable.committed !== "hello Bifrost runtime") fail(`unexpected max utterance committed text: ${stable.committed}`);
    ws.close();
    process.exit(0);
  }
};
NODE

echo "[voice-input-e2e] Admin Voice WebSocket idle unload without audio messages"
node - "$ADMIN_PORT" <<'NODE'
const port = process.argv[2];
const query = new URLSearchParams({
  source: "web_mic",
  fake_stateful_worker: "1",
  ws_idle_timeout_ms: "200",
}).toString();
const url = `ws://127.0.0.1:${port}/_bifrost/api/voice/listen-ws?${query}`;
const ws = new WebSocket(url);
const seen = [];
const fail = (message) => {
  console.error(`[voice-input-e2e] ${message}`);
  process.exit(1);
};
const timeout = setTimeout(() => fail(`timeout waiting for idle unload: ${seen.join(",")}`), 5000);
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "start", source: "web_mic", sample_rate: 16000, channels: 1, format: "pcm_s16le" }));
};
ws.onerror = (event) => fail(`websocket error: ${event.message || "unknown"}`);
ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  seen.push(payload.type);
  if (payload.type === "done") {
    clearTimeout(timeout);
    if (!seen.includes("worker_idle_unloaded")) fail(`missing worker_idle_unloaded; seen=${seen.join(",")}`);
    ws.close();
    process.exit(0);
  }
};
NODE

echo "[voice-input-e2e] Admin Voice WebSocket silence finish preserves committed final"
node - "$ADMIN_PORT" <<'NODE'
const port = process.argv[2];
const query = `source=web_mic&fake_stateful_worker=1&fake_stateful_text=${encodeURIComponent("hello Bifrost after silence")}&silence_commit_ms=1`;
const url = `ws://127.0.0.1:${port}/_bifrost/api/voice/listen-ws?${query}`;
const ws = new WebSocket(url);
const seen = [];
const fail = (message) => {
  console.error(`[voice-input-e2e] ${message}`);
  process.exit(1);
};
const timeout = setTimeout(() => fail(`timeout waiting for silence/final boundary: ${seen.map((e) => e.type).join(",")}`), 10000);
const speech = Buffer.alloc(3200);
for (let i = 0; i < speech.length; i += 2) speech.writeInt16LE(10000, i);
const silence = Buffer.alloc(3200);
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "start", source: "web_mic", sample_rate: 16000, channels: 1, format: "pcm_s16le" }));
  ws.send(JSON.stringify({ type: "audio", sequence: 1, duration_ms: 100, data: speech.toString("base64") }));
  ws.send(JSON.stringify({ type: "audio", sequence: 2, duration_ms: 100, data: silence.toString("base64") }));
  setTimeout(() => ws.send(JSON.stringify({ type: "finish" })), 20);
};
ws.onerror = (event) => fail(`websocket error: ${event.message || "unknown"}`);
ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  seen.push(payload);
  if (payload.type === "done") {
    clearTimeout(timeout);
    const stable = seen.find((item) => item.type === "asr_stable_delta" && String(item.detail || "").includes("reason=silence"));
    const final = seen.find((item) => item.type === "asr_final_utterance");
    if (!stable) fail(`missing silence stable event; seen=${seen.map((item) => `${item.type}:${item.detail || ""}`).join(",")}`);
    if (!final) fail("missing final event after silence");
    if (stable.committed !== "hello Bifrost after silence") fail(`unexpected stable committed: ${stable.committed}`);
    if (final.committed !== stable.committed || final.delta !== "") fail(`final should preserve committed text without duplicate delta; final=${JSON.stringify(final)}`);
    ws.close();
    process.exit(0);
  }
};
NODE

echo "[voice-input-e2e] Admin Voice WebSocket sustained silence does not emit transcript"
node - "$ADMIN_PORT" <<'NODE'
const port = process.argv[2];
const query = new URLSearchParams({
  source: "web_mic",
  fake_stateful_worker: "1",
  worker_idle_unload_ms: "1",
}).toString();
const url = `ws://127.0.0.1:${port}/_bifrost/api/voice/listen-ws?${query}`;
const ws = new WebSocket(url);
const seen = [];
const fail = (message) => {
  console.error(`[voice-input-e2e] ${message}`);
  process.exit(1);
};
const timeout = setTimeout(() => fail(`timeout waiting for sustained silence finish: ${seen.map((e) => e.type).join(",")}`), 10000);
const silence = Buffer.alloc(3200);
ws.onopen = () => {
  ws.send(JSON.stringify({ type: "start", source: "web_mic", sample_rate: 16000, channels: 1, format: "pcm_s16le" }));
  ws.send(JSON.stringify({ type: "audio", sequence: 1, duration_ms: 100, data: silence.toString("base64") }));
  ws.send(JSON.stringify({ type: "audio", sequence: 2, duration_ms: 100, data: silence.toString("base64") }));
  ws.send(JSON.stringify({ type: "audio", sequence: 3, duration_ms: 100, data: silence.toString("base64") }));
  setTimeout(() => ws.send(JSON.stringify({ type: "finish" })), 20);
};
ws.onerror = (event) => fail(`websocket error: ${event.message || "unknown"}`);
ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  seen.push(payload);
  if (payload.type === "done") {
    clearTimeout(timeout);
    if (seen.some((item) => item.type === "asr_partial" || item.type === "asr_stable_delta")) {
      fail(`silence-only session emitted transcript events: ${seen.map((item) => item.type).join(",")}`);
    }
    const final = seen.find((item) => item.type === "asr_final_utterance");
    if (!final || final.committed !== "" || final.delta !== "") fail(`silence final should be empty; final=${JSON.stringify(final)}`);
    ws.close();
    process.exit(0);
  }
};
NODE

echo "[voice-input-e2e] Admin Voice API session creation"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/voice/sessions" \
  | tee "$ADMIN_DATA_DIR/session.json" \
  | grep -q '"provider":"qwen3_stateful_streaming"'

echo "[voice-input-e2e] PASS"
