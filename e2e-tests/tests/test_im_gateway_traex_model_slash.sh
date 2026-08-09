#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-traex-models.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

if [[ -z "${BIFROST_PORT:-}" ]]; then
  BIFROST_PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 180); do
    if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
      echo "[im-gateway-traex-model-slash] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-traex-model-slash] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

MOCK_TRAECLI="$TEST_DIR/mock-traecli"
MOCK_ARGV_LOG="$TEST_DIR/mock-traecli-argv.log"
MOCK_CODEX="$TEST_DIR/mock-codex"
MOCK_CODEX_ARGV_LOG="$TEST_DIR/mock-codex-argv.log"
MOCK_CLAUDE="$TEST_DIR/mock-claude"
MOCK_CLAUDE_ARGV_LOG="$TEST_DIR/mock-claude-argv.log"
cat >"$MOCK_CODEX" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$BIFROST_MOCK_CODEX_ARGV_LOG"
if [ "${1:-}" = "debug" ] && [ "${2:-}" = "models" ]; then
  cat <<'JSON'
{
  "models": [
    {
      "slug": "hidden-codex-unit",
      "visibility": "hidden",
      "base_instructions": "CODEX_SHOULD_NOT_LEAK"
    },
    {
      "slug": "gpt-unit",
      "description": "Codex unit model",
      "default_reasoning_level": "medium",
      "supported_reasoning_levels": [{"effort": "minimal"}, {"effort": "low"}, {"effort": "medium"}, {"effort": "high"}],
      "visibility": "list",
      "supported_in_api": true,
      "priority": 1,
      "additional_speed_tiers": ["fast"],
      "base_instructions": "CODEX_SHOULD_NOT_LEAK"
    }
  ]
}
JSON
  exit 0
fi
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread-codex-model-slash"}'
printf '%s\n' '{"type":"assistant_final","content":"BIFROST_CODEX_MODEL_SLASH_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_CODEX"

cat >"$MOCK_TRAECLI" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$BIFROST_MOCK_TRAECLI_ARGV_LOG"
if [ "${1:-}" = "debug" ] && [ "${2:-}" = "models" ]; then
  cat <<'JSON'
{
  "models": [
    {
      "slug": "hidden-unit",
      "visibility": "hidden",
      "base_instructions": "SHOULD_NOT_LEAK"
    },
    {
      "slug": "Doubao-Unit",
      "description": "Unit test model",
      "default_reasoning_level": "medium",
      "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}],
      "visibility": "list",
      "supported_in_api": true,
      "model_load": 93,
      "priority": 1,
      "additional_speed_tiers": ["fast"],
      "base_instructions": "SHOULD_NOT_LEAK"
    }
  ]
}
JSON
  exit 0
fi
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread-traex-model-slash"}'
printf '%s\n' '{"type":"assistant_final","content":"BIFROST_TRAEX_MODEL_SLASH_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_TRAECLI"

cat >"$MOCK_CLAUDE" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$BIFROST_MOCK_CLAUDE_ARGV_LOG"
# Claude stream-json keeps stdin open so live guide frames can arrive during the
# active turn. Consume the initial user frame instead of waiting for EOF.
if ! IFS= read -r initial_frame; then
  printf '%s\n' 'missing initial stream-json user frame' >&2
  exit 1
fi
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"BIFROST_CLAUDE_MODEL_SLASH_OK"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"BIFROST_CLAUDE_MODEL_SLASH_OK","session_id":"thread-claude-model-slash","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_CLAUDE"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-traex-model-slash] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-traex-model-slash] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-traex-model-slash] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" "$TEST_DIR" "$MOCK_CODEX" "$MOCK_CODEX_ARGV_LOG" "$MOCK_TRAECLI" "$MOCK_ARGV_LOG" <<'PY'
import json
import pathlib
import sys
import urllib.error
import urllib.request

port, test_dir, mock_codex, codex_argv_log, mock_traecli, traex_argv_log = sys.argv[1:7]
mock_claude = pathlib.Path(test_dir) / "mock-claude"
claude_argv_log = pathlib.Path(test_dir) / "mock-claude-argv.log"
base_url = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"
test_path = pathlib.Path(test_dir)


def patch_config():
    payload = {
        "version": 1,
        "defaultRunnerId": "mock-codex-models",
        "runners": {
            "mock-codex-models": {
                "enabled": True,
                "adapter": "codex",
                "adapterConfig": {
                    "executable": mock_codex,
                    "env": {"BIFROST_MOCK_CODEX_ARGV_LOG": codex_argv_log},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            },
            "mock-traex-models": {
                "enabled": True,
                "adapter": "traex",
                "adapterConfig": {
                    "executable": mock_traecli,
                    "env": {"BIFROST_MOCK_TRAECLI_ARGV_LOG": traex_argv_log},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            },
            "mock-claude-code-models": {
                "enabled": True,
                "adapter": "claude_code",
                "adapterConfig": {
                    "executable": str(mock_claude),
                    "env": {"BIFROST_MOCK_CLAUDE_ARGV_LOG": str(claude_argv_log)},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            }
        },
        "channels": {},
    }
    req = urllib.request.Request(
        f"{base_url}/config",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="PATCH",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        assert resp.status == 200, resp.read().decode("utf-8")


def stream(runner_id, session_key, message):
    payload = {
        "message": message,
        "providerId": "web-e2e",
        "runnerId": runner_id,
        "sessionKey": session_key,
    }
    req = urllib.request.Request(
        f"{base_url}/stream",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    events = []
    with urllib.request.urlopen(req, timeout=90) as resp:
        assert resp.status == 200, resp.status
        for raw_line in resp:
            line = raw_line.decode("utf-8").strip()
            if line:
                events.append(json.loads(line))
    return events


def stream_bad_request(runner_id, session_key, message):
    payload = {
        "message": message,
        "providerId": "web-e2e",
        "runnerId": runner_id,
        "sessionKey": session_key,
    }
    req = urllib.request.Request(
        f"{base_url}/stream",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=90) as resp:
            body = resp.read().decode("utf-8")
            raise AssertionError(f"expected HTTP 400, got {resp.status}: {body}")
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        assert error.code == 400, body
        return body


def final_response(events):
    finished = [event for event in events if event.get("eventType") == "run_finished"]
    assert len(finished) == 1, events
    assert finished[0].get("status") == "succeeded", finished[0]
    return finished[0].get("response") or ""


patch_config()

def assert_runner_model_flow(runner_id, session_key, model, expected_response, leaked_marker, hidden_model, argv_log):
    models_response = final_response(stream(runner_id, session_key, "/models"))
    assert model in models_response, models_response
    assert "fast" in models_response, models_response
    assert leaked_marker not in models_response, models_response
    assert hidden_model not in models_response, models_response

    set_response = final_response(stream(runner_id, session_key, f"/model {model}"))
    assert model in set_response, set_response

    run_events = stream(runner_id, session_key, "hello after model switch")
    run_response = final_response(run_events)
    assert expected_response in run_response, run_response
    run_id = [event for event in run_events if event.get("eventType") == "run_finished"][0]["runId"]
    snapshot = json.loads((test_path / "agent" / "im_gateway" / "chat_runs" / run_id / "runtime_snapshot.json").read_text(encoding="utf-8"))
    assert snapshot["model"] == model, snapshot
    argv = pathlib.Path(argv_log).read_text(encoding="utf-8")
    assert "debug models" in argv, argv
    assert "exec --json" in argv, argv
    return run_id

def assert_runner_effort_flow(runner_id, session_key, effort, expected_response, expected_arg_fragment, argv_log):
    efforts_response = final_response(stream(runner_id, session_key, "/efforts"))
    assert effort in efforts_response, efforts_response

    set_response = final_response(stream(runner_id, session_key, f"/effort {effort}"))
    assert effort in set_response, set_response

    run_events = stream(runner_id, session_key, "hello after effort switch")
    run_response = final_response(run_events)
    assert expected_response in run_response, run_response
    run_id = [event for event in run_events if event.get("eventType") == "run_finished"][0]["runId"]
    snapshot = json.loads((test_path / "agent" / "im_gateway" / "chat_runs" / run_id / "runtime_snapshot.json").read_text(encoding="utf-8"))
    assert snapshot["reasoningEffort"] == effort, snapshot
    argv = pathlib.Path(argv_log).read_text(encoding="utf-8")
    assert expected_arg_fragment in argv, argv
    return run_id


codex_run_id = assert_runner_model_flow(
    "mock-codex-models",
    "codex-model-slash-e2e",
    "gpt-unit",
    "BIFROST_CODEX_MODEL_SLASH_OK",
    "CODEX_SHOULD_NOT_LEAK",
    "hidden-codex-unit",
    codex_argv_log,
)
traex_run_id = assert_runner_model_flow(
    "mock-traex-models",
    "traex-model-slash-e2e",
    "Doubao-Unit",
    "BIFROST_TRAEX_MODEL_SLASH_OK",
    "SHOULD_NOT_LEAK",
    "hidden-unit",
    traex_argv_log,
)
traex_models_response = final_response(stream(
    "mock-traex-models",
    "traex-model-slash-e2e",
    "/models",
))
assert "Model load: 93%" in traex_models_response, traex_models_response
traex_bad_effort = final_response(stream(
    "mock-traex-models",
    "traex-model-slash-e2e",
    "/effort high",
))
assert "不在 Traex 当前模型 `Doubao-Unit` 支持列表中" in traex_bad_effort, traex_bad_effort

codex_effort_run_id = assert_runner_effort_flow(
    "mock-codex-models",
    "codex-model-slash-e2e",
    "minimal",
    "BIFROST_CODEX_MODEL_SLASH_OK",
    'model_reasoning_effort="minimal"',
    codex_argv_log,
)
traex_effort_run_id = assert_runner_effort_flow(
    "mock-traex-models",
    "traex-model-slash-e2e",
    "low",
    "BIFROST_TRAEX_MODEL_SLASH_OK",
    'model_reasoning_effort="low"',
    traex_argv_log,
)

claude_models_response = final_response(stream(
    "mock-claude-code-models",
    "claude-code-model-slash-e2e",
    "/models",
))
assert "sonnet" in claude_models_response, claude_models_response
assert "opus" in claude_models_response, claude_models_response
assert "haiku" in claude_models_response, claude_models_response
assert "fable" in claude_models_response, claude_models_response
assert "Sonnet 4.6" in claude_models_response, claude_models_response
assert "Opus 4.8" in claude_models_response, claude_models_response
assert "base_instructions" not in claude_models_response, claude_models_response

claude_bad_model = stream_bad_request(
    "mock-claude-code-models",
    "claude-code-model-slash-e2e",
    "/model bad model",
)
assert "模型名称只能包含" in claude_bad_model, claude_bad_model

claude_set_response = final_response(stream(
    "mock-claude-code-models",
    "claude-code-model-slash-e2e",
    "/model sonnet",
))
assert "sonnet" in claude_set_response, claude_set_response

claude_events = stream(
    "mock-claude-code-models",
    "claude-code-model-slash-e2e",
    "hello after claude code model switch",
)
claude_response = final_response(claude_events)
assert "BIFROST_CLAUDE_MODEL_SLASH_OK" in claude_response, claude_response
claude_run_id = [event for event in claude_events if event.get("eventType") == "run_finished"][0]["runId"]
claude_snapshot = json.loads((test_path / "agent" / "im_gateway" / "chat_runs" / claude_run_id / "runtime_snapshot.json").read_text(encoding="utf-8"))
assert claude_snapshot["model"] == "sonnet", claude_snapshot
claude_argv = pathlib.Path(claude_argv_log).read_text(encoding="utf-8")
assert "debug models" not in claude_argv, claude_argv
assert "--model sonnet" in claude_argv, claude_argv
claude_effort_run_id = assert_runner_effort_flow(
    "mock-claude-code-models",
    "claude-code-model-slash-e2e",
    "xhigh",
    "BIFROST_CLAUDE_MODEL_SLASH_OK",
    "--effort xhigh",
    claude_argv_log,
)

state = json.loads((test_path / "agent" / "im_gateway" / "session_state.json").read_text(encoding="utf-8"))
codex_session = next(value for value in state["sessions"].values() if value.get("runnerId") == "mock-codex-models")
traex_session = next(value for value in state["sessions"].values() if value.get("runnerId") == "mock-traex-models")
claude_session = next(value for value in state["sessions"].values() if value.get("runnerId") == "mock-claude-code-models")
assert codex_session["modelOverride"] == "gpt-unit", codex_session
assert traex_session["modelOverride"] == "Doubao-Unit", traex_session
assert claude_session["modelOverride"] == "sonnet", claude_session
assert codex_session["reasoningEffortOverride"] == "minimal", codex_session
assert traex_session["reasoningEffortOverride"] == "low", traex_session
assert claude_session["reasoningEffortOverride"] == "xhigh", claude_session
assert codex_session["modelOverrideSource"] == "session slash command", codex_session
assert traex_session["modelOverrideSource"] == "session slash command", traex_session
assert claude_session["modelOverrideSource"] == "session slash command", claude_session
assert codex_session["reasoningEffortOverrideSource"] == "session slash command", codex_session
assert traex_session["reasoningEffortOverrideSource"] == "session slash command", traex_session
assert claude_session["reasoningEffortOverrideSource"] == "session slash command", claude_session

print("[im-gateway-model-slash] PASS")
print(f"codex_run_id={codex_run_id}")
print(f"traex_run_id={traex_run_id}")
print(f"claude_run_id={claude_run_id}")
print(f"codex_effort_run_id={codex_effort_run_id}")
print(f"traex_effort_run_id={traex_effort_run_id}")
print(f"claude_effort_run_id={claude_effort_run_id}")
PY
