#!/usr/bin/env bash
# Realistic CI-like reproduction of the guide-queue race.
# Runs N concurrent copies of the default-guide flow against the same
# release binary, matching BIFROST_E2E_SHELL_JOBS=4 contention. On any
# failure it preserves the bifrost.log + mock log so we can determine
# LOST vs SLOW delivery.
set -uo pipefail

REPO_DIR="/Users/eden/work/github/bifrost"
BIFROST_BIN="$REPO_DIR/target/release/bifrost"
KEEP_ROOT="/tmp/guide_repro_keep"
JOBS="${JOBS:-4}"
ROUNDS="${ROUNDS:-6}"
rm -rf "$KEEP_ROOT"; mkdir -p "$KEEP_ROOT"

run_one() {
  local idx="$1"
  local round="$2"
  local base_port=$((19000 + idx * 4))
  local BIFROST_PORT=$base_port
  local MOCK_PORT=$((base_port + 1))
  local TEST_DIR
  TEST_DIR="$(mktemp -d)"
  local MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
  local BIFROST_LOG="$TEST_DIR/bifrost.log"

  cleanup_one() {
    [[ -n "${BIFROST_PID:-}" ]] && { kill "$BIFROST_PID" >/dev/null 2>&1; wait "$BIFROST_PID" 2>/dev/null; }
    [[ -n "${MOCK_PID:-}" ]] && { kill "$MOCK_PID" >/dev/null 2>&1; wait "$MOCK_PID" 2>/dev/null; }
  }

  python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
port=int(sys.argv[1]); log_path=sys.argv[2]
default_guide_gate=threading.Event()
class H(BaseHTTPRequestHandler):
    def log_message(self,*a): return
    def do_GET(self):
        if self.path=="/health":
            self.send_response(200); self.end_headers(); self.wfile.write(b"ok"); return
        if self.path=="/release/default-guide":
            default_guide_gate.set(); self.send_response(200); self.end_headers(); self.wfile.write(b"released"); return
        self.send_error(404)
    def do_POST(self):
        length=int(self.headers.get("Content-Length","0")); body=self.rfile.read(length)
        try: payload=json.loads(body or b"{}")
        except Exception: payload={}
        with open(log_path,"a",encoding="utf-8") as fh: fh.write(json.dumps(payload,ensure_ascii=False)+"\n")
        texts=[m.get("content") for m in payload.get("messages",[]) if isinstance(m.get("content"),str)]
        ut=[t for t in texts if t.strip()]
        if any("DEFAULT_GUIDE_INITIAL" in t for t in ut):
            default_guide_gate.wait(timeout=30); content="DEFAULT_GUIDE_INITIAL_DONE"
        elif any("默认引导消息" in t for t in ut): content="DEFAULT_GUIDE_CONSUMED"
        else: content="OK"
        resp={"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
              "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}
        enc=json.dumps(resp,ensure_ascii=False).encode("utf-8")
        self.send_response(200); self.send_header("Content-Type","application/json")
        self.send_header("Content-Length",str(len(enc))); self.end_headers(); self.wfile.write(enc)
ThreadingHTTPServer(("127.0.0.1",port),H).serve_forever()
PY
  MOCK_PID=$!

  for _ in $(seq 1 120); do curl -fsS --noproxy '*' "http://127.0.0.1:$MOCK_PORT/health" >/dev/null 2>&1 && break; sleep 0.25; done

  BIFROST_DATA_DIR="$TEST_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    RUST_LOG=bifrost_admin=debug,bifrost_agent=debug \
    "$BIFROST_BIN" start --host 127.0.0.1 -p "$BIFROST_PORT" \
    --unsafe-ssl --skip-cert-check --no-system-proxy >"$BIFROST_LOG" 2>&1 &
  BIFROST_PID=$!
  for _ in $(seq 1 120); do curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1 && break; sleep 0.25; done

  BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"
  curl -fsS --noproxy '*' -X PATCH "$BASE" -H 'Content-Type: application/json' -d "{
    \"enabled\": true, \"model_provider\": \"mock\", \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\", \"api_key\": \"k\",
    \"request_timeout_secs\": 20, \"max_turn_iterations\": 8,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}}" >/dev/null

  PROV="prov-$idx"; OWNER="owner-$idx"; SKEY="$PROV:$OWNER"
  curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers" \
    -H 'Content-Type: application/json' -d "{\"id\":\"$PROV\",\"provider_type\":\"feishu\",
    \"display_name\":\"d\",\"enabled\":true,\"app_id\":\"a$idx\",\"owner_open_id\":\"$OWNER\",
    \"event_connection_enabled\":false}" >/dev/null

  curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/debug/mock-inbound" \
    -H 'Content-Type: application/json' -d "{\"providerId\":\"$PROV\",\"userId\":\"$OWNER\",
    \"chatId\":\"c$idx\",\"text\":\"DEFAULT_GUIDE_INITIAL\"}" >/dev/null

  ok=""
  for _ in $(seq 1 200); do grep -q "DEFAULT_GUIDE_INITIAL" "$MOCK_LOG" 2>/dev/null && { ok=1; break; }; sleep 0.05; done
  [[ -z "$ok" ]] && { echo "FAIL idx=$idx r=$round: initial not reached"; cp -r "$TEST_DIR" "$KEEP_ROOT/fail-init-$idx-$round"; cleanup_one; rm -rf "$TEST_DIR"; return 1; }

  curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/debug/mock-inbound" \
    -H 'Content-Type: application/json' -d "{\"providerId\":\"$PROV\",\"userId\":\"$OWNER\",
    \"chatId\":\"c$idx\",\"text\":\"默认引导消息\"}" >/dev/null

  # wait until it becomes a pending guide
  pend=""
  for _ in $(seq 1 100); do
    cand="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" -H 'Content-Type: application/json' -d "{\"session_key\":\"$SKEY\",\"message\":\"/status\"}")"
    echo "$cand" | grep -q '默认引导消息' && { pend=1; break; }
    sleep 0.2
  done
  [[ -z "$pend" ]] && { echo "FAIL idx=$idx r=$round: never pending"; cp -r "$TEST_DIR" "$KEEP_ROOT/fail-pend-$idx-$round"; cleanup_one; rm -rf "$TEST_DIR"; return 1; }

  curl -fsS --noproxy '*' "http://127.0.0.1:$MOCK_PORT/release/default-guide" >/dev/null

  drained=""
  for _ in $(seq 1 80); do grep -q "默认引导消息" "$MOCK_LOG" 2>/dev/null && { drained=1; break; }; sleep 0.1; done
  if [[ -z "$drained" ]]; then
    echo "FAIL idx=$idx r=$round: DRAIN — guide not consumed"
    cp -r "$TEST_DIR" "$KEEP_ROOT/fail-drain-$idx-$round"
    cleanup_one; rm -rf "$TEST_DIR"; return 1
  fi
  cleanup_one; rm -rf "$TEST_DIR"
  return 0
}

total=0; fails=0
for r in $(seq 1 "$ROUNDS"); do
  pids=()
  for i in $(seq 1 "$JOBS"); do run_one "$i" "$r" & pids+=($!); done
  for p in "${pids[@]}"; do total=$((total+1)); wait "$p" || fails=$((fails+1)); done
  echo "=== round $r done: cumulative total=$total fails=$fails ==="
done
echo "RESULT: total=$total fails=$fails"
