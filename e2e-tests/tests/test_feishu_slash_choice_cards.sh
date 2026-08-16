#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-feishu-choice.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
FEISHU_DRY_RUN="$TEST_DIR/feishu-dry-run.jsonl"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[feishu-slash-choice] kept test directory: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

wait_http() {
  for _ in $(seq 1 180); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_capture_count() {
  local expected="$1"
  for _ in $(seq 1 240); do
    local actual=0
    if [[ -f "$FEISHU_DRY_RUN" ]]; then
      actual="$(wc -l <"$FEISHU_DRY_RUN" | tr -d ' ')"
    fi
    if [[ "$actual" -ge "$expected" ]]; then
      return 0
    fi
    sleep 0.05
  done
  echo "[feishu-slash-choice] expected at least $expected dry-run rows" >&2
  [[ -f "$FEISHU_DRY_RUN" ]] && sed -n '1,160p' "$FEISHU_DRY_RUN" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_capture_source() {
  local source_message_id="$1"
  for _ in $(seq 1 240); do
    if [[ -f "$FEISHU_DRY_RUN" ]] && python3 - "$FEISHU_DRY_RUN" "$source_message_id" <<'PY'
import json
import pathlib
import sys

path, source_message_id = pathlib.Path(sys.argv[1]), sys.argv[2]
rows = [
    json.loads(line)
    for line in path.read_text(encoding="utf-8").splitlines()
]
raise SystemExit(0 if any(
    row.get("kind") == "card"
    and row.get("sourceMessageId") == source_message_id
    for row in rows
) else 1)
PY
    then
      return 0
    fi
    sleep 0.05
  done
  echo "[feishu-slash-choice] missing capture for $source_message_id" >&2
  [[ -f "$FEISHU_DRY_RUN" ]] && sed -n '1,240p' "$FEISHU_DRY_RUN" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_state_value() {
  local field="$1"
  local expected="$2"
  for _ in $(seq 1 240); do
    if python3 - "$TEST_DIR/agent/im_gateway/session_state.json" "$field" "$expected" <<'PY'
import json
import pathlib
import sys

path, field, expected = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
if not path.exists():
    raise SystemExit(1)
state = json.loads(path.read_text(encoding="utf-8"))
matching = [
    session for session in state.get("sessions", {}).values()
    if session.get("runnerId") == "choice-codex"
    and session.get(field) == expected
]
raise SystemExit(0 if matching else 1)
PY
    then
      return 0
    fi
    sleep 0.05
  done
  echo "[feishu-slash-choice] missing state $field=$expected" >&2
  [[ -f "$TEST_DIR/agent/im_gateway/session_state.json" ]] \
    && cat "$TEST_DIR/agent/im_gateway/session_state.json" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_session_field() {
  local session_key="$1"
  local field="$2"
  local expected="$3"
  for _ in $(seq 1 240); do
    if python3 - "$TEST_DIR/agent/im_gateway/session_state.json" \
      "$session_key" "$field" "$expected" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
session_key, field, expected = sys.argv[2:5]
if not path.exists():
    raise SystemExit(1)
state = json.loads(path.read_text(encoding="utf-8"))
matches = [
    session
    for session in state.get("sessions", {}).values()
    if session.get("sessionKey") == session_key
    and session.get("runnerId") == "choice-codex"
]
if len(matches) != 1:
    raise SystemExit(1)
session = matches[0]
if expected == "__ABSENT__":
    raise SystemExit(0 if field not in session else 1)
raise SystemExit(0 if session.get(field) == expected else 1)
PY
    then
      return 0
    fi
    sleep 0.05
  done
  echo "[feishu-slash-choice] missing session field $session_key $field=$expected" >&2
  [[ -f "$TEST_DIR/agent/im_gateway/session_state.json" ]] \
    && cat "$TEST_DIR/agent/im_gateway/session_state.json" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

MOCK_CODEX="$TEST_DIR/mock-codex"
cat >"$MOCK_CODEX" <<'SH'
#!/usr/bin/env sh
if [ "${1:-}" = "debug" ] && [ "${2:-}" = "models" ]; then
  cat <<'JSON'
{
  "models": [
    {
      "slug": "gpt-unit",
      "display_name": "GPT Unit",
      "default_reasoning_level": "medium",
      "supported_reasoning_levels": [
        {"effort": "low"},
        {"effort": "medium"},
        {"effort": "high"}
      ],
      "visibility": "list",
      "priority": 1
    }
  ]
}
JSON
  exit 0
fi
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread-choice"}'
printf '%s\n' '{"type":"assistant_final","content":"CHOICE_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_CODEX"

mkdir -p "$TEST_DIR/codex/sessions/2026/08/17"
cat >"$TEST_DIR/codex/sessions/2026/08/17/session.jsonl" <<'JSONL'
{"timestamp":"2026-08-17T01:00:00Z","type":"session_meta","payload":{"id":"11111111-1111-1111-1111-111111111111","timestamp":"2026-08-17T01:00:00Z"}}
JSONL
cat >"$TEST_DIR/codex/session_index.jsonl" <<'JSONL'
{"id":"11111111-1111-1111-1111-111111111111","thread_name":"Feishu choice session","updated_at":"2026-08-17T01:02:00Z"}
JSONL

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

CODEX_HOME="$TEST_DIR/codex" \
BIFROST_DATA_DIR="$TEST_DIR" \
BIFROST_FEISHU_DRY_RUN_FILE="$FEISHU_DRY_RUN" \
  "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http

python3 - "$BIFROST_PORT" "$MOCK_CODEX" <<'PY'
import json
import sys
import urllib.request

port, mock_codex = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"


def request(path, payload, method="POST"):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")


request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "choice-codex",
    "runners": {
        "choice-codex": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {
                "executable": mock_codex,
                "timeoutSecs": 30
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply"
        }
    },
    "channels": {}
}, "PATCH")
request("/providers", {
    "id": "feishu-choice-e2e",
    "provider_type": "feishu",
    "display_name": "Feishu Choice E2E",
    "enabled": True,
    "app_id": "cli_choice_e2e",
    "app_secret": "choice-secret",
    "owner_open_id": "ou_owner",
    "event_connection_enabled": False,
    "agent_config": {"runner": "choice-codex"}
})
PY

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug"


def inject(message_id, text, chat_id="oc_direct", chat_type="p2p"):
    payload = {
        "providerId": "feishu-choice-e2e",
        "chatId": chat_id,
        "chatType": chat_type,
        "userId": "ou_owner",
        "messageId": message_id,
        "eventId": "evt-" + message_id,
        "text": text,
        "mentionBot": False
    }
    request = urllib.request.Request(
        base + "/mock-inbound",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")


inject("om_resume", "/resume")
PY
wait_capture_count 2

python3 - "$BIFROST_PORT" "$FEISHU_DRY_RUN" "om_resume" "/resume 11111111-1111-1111-1111-111111111111" "evt_click_resume" <<'PY'
import json
import pathlib
import sys
import urllib.request

port, capture_path, source_message_id, command, event_id = sys.argv[1:6]
rows = [
    json.loads(line)
    for line in pathlib.Path(capture_path).read_text(encoding="utf-8").splitlines()
]
row = next(
    item for item in rows
    if item.get("kind") == "card"
    and item.get("sourceMessageId") == source_message_id
)
card = row["card"]
assert card["schema"] == "2.0", card
assert "header" not in card, card
summary = card["body"]["elements"][0]["content"]
assert "点击下方按钮" in summary, summary
assert "发送 `/resume <id>`" not in summary, summary
buttons = []
for element in card["body"]["elements"]:
    for column in element.get("columns", []):
        buttons.extend(
            child for child in column.get("elements", [])
            if child.get("tag") == "button"
        )
assert buttons, card
element_ids = [button["element_id"] for button in buttons]
assert len(element_ids) == len(set(element_ids)), element_ids
button = next(
    item for item in buttons
    if item["behaviors"][0]["value"]["command"] == command
)
value = button["behaviors"][0]["value"]
assert button["behaviors"][0]["type"] == "callback", button
payload = {
    "providerId": "feishu-choice-e2e",
    "payload": {
        "header": {
            "event_id": event_id,
            "event_type": "card.action.trigger"
        },
        "event": {
            "operator": {"open_id": "ou_owner"},
            "action": {"tag": "button", "value": value},
            "context": {
                "open_message_id": row["messageId"],
                "open_chat_id": value["chatId"]
            }
        }
    }
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-card-action",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode("utf-8")
PY
wait_state_value externalThreadId "11111111-1111-1111-1111-111111111111"

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound"


def inject(message_id, text, chat_id="oc_direct", chat_type="p2p"):
    request = urllib.request.Request(
        base,
        data=json.dumps({
            "providerId": "feishu-choice-e2e",
            "chatId": chat_id,
            "chatType": chat_type,
            "userId": "ou_owner",
            "messageId": message_id,
            "eventId": "evt-" + message_id,
            "text": text
        }).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")


inject("om_model", "/model")
inject("om_effort", "/effort")
inject("om_group_model", "/model", "oc_group", "group")
PY
wait_capture_count 8

python3 - "$BIFROST_PORT" "$FEISHU_DRY_RUN" <<'PY'
import json
import pathlib
import sys
import urllib.error
import urllib.request

port, capture_path = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-card-action"
rows = [
    json.loads(line)
    for line in pathlib.Path(capture_path).read_text(encoding="utf-8").splitlines()
]


def choice(source_message_id, command):
    row = next(
        item for item in rows
        if item.get("kind") == "card"
        and item.get("sourceMessageId") == source_message_id
    )
    card = row["card"]
    buttons = [
        child
        for element in card["body"]["elements"]
        for column in element.get("columns", [])
        for child in column.get("elements", [])
        if child.get("tag") == "button"
    ]
    button = next(
        item for item in buttons
        if item["behaviors"][0]["value"]["command"] == command
    )
    return row, button["behaviors"][0]["value"]


def callback(row, value, event_id, operator="ou_owner", chat_id=None, now_ms=None, expected=200):
    payload = {
        "providerId": "feishu-choice-e2e",
        "payload": {
            "header": {
                "event_id": event_id,
                "event_type": "card.action.trigger"
            },
            "event": {
                "operator": {"open_id": operator},
                "action": {"tag": "button", "value": value},
                "context": {
                    "open_message_id": row["messageId"],
                    "open_chat_id": chat_id or value["chatId"]
                }
            }
        }
    }
    if now_ms is not None:
        payload["nowMs"] = now_ms
    request = urllib.request.Request(
        base,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        response = urllib.request.urlopen(request, timeout=30)
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        assert error.code == expected, body
        return
    with response:
        body = response.read().decode("utf-8")
        assert response.status == expected, body


model_row, model_value = choice("om_model", "/model gpt-unit")
effort_row, effort_value = choice("om_effort", "/effort high")
group_row, group_value = choice("om_group_model", "/model gpt-unit")
assert group_value["chatType"] == "group", group_value
assert group_value["chatId"] == "oc_group", group_value

callback(model_row, model_value, "evt_click_model")
callback(effort_row, effort_value, "evt_click_effort")
callback(group_row, group_value, "evt_click_group_model")

for index, (row, value, overrides) in enumerate([
    (model_row, model_value, {"operator": "ou_intruder"}),
    (model_row, model_value, {"chat_id": "oc_other"}),
    (model_row, model_value, {"now_ms": model_value["expiresAtMs"]}),
]):
    callback(
        row,
        value,
        f"evt_rejected_{index}",
        operator=overrides.get("operator", "ou_owner"),
        chat_id=overrides.get("chat_id"),
        now_ms=overrides.get("now_ms"),
        expected=400,
    )

forbidden = dict(model_value)
forbidden["command"] = "/stop now"
callback(model_row, forbidden, "evt_rejected_command", expected=400)
PY
wait_session_field "feishu-choice-e2e:ou_owner" modelOverride "gpt-unit"
wait_session_field "feishu-choice-e2e:ou_owner" reasoningEffortOverride "high"
wait_session_field "im:feishu-choice-e2e:group:oc_group" modelOverride "gpt-unit"

python3 - "$BIFROST_PORT" "$FEISHU_DRY_RUN" <<'PY'
import json
import pathlib
import sys
import urllib.request

port, capture_path = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-card-action"
rows = [
    json.loads(line)
    for line in pathlib.Path(capture_path).read_text(encoding="utf-8").splitlines()
]


def choice(source_message_id, command):
    row = next(
        item for item in rows
        if item.get("kind") == "card"
        and item.get("sourceMessageId") == source_message_id
    )
    button = next(
        child
        for element in row["card"]["body"]["elements"]
        for column in element.get("columns", [])
        for child in column.get("elements", [])
        if child.get("tag") == "button"
        and child["behaviors"][0]["value"]["command"] == command
    )
    return row, button["behaviors"][0]["value"]


def callback(row, value, event_id):
    payload = {
        "providerId": "feishu-choice-e2e",
        "payload": {
            "header": {
                "event_id": event_id,
                "event_type": "card.action.trigger"
            },
            "event": {
                "operator": {"open_id": "ou_owner"},
                "action": {"tag": "button", "value": value},
                "context": {
                    "open_message_id": row["messageId"],
                    "open_chat_id": value["chatId"]
                }
            }
        }
    }
    request = urllib.request.Request(
        base,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")


model_row, model_clear = choice("om_model", "/model clear")
effort_row, effort_clear = choice("om_effort", "/effort clear")
callback(model_row, model_clear, "evt_click_model_clear")
callback(effort_row, effort_clear, "evt_click_effort_clear")
PY
wait_session_field "feishu-choice-e2e:ou_owner" modelOverride "__ABSENT__"
wait_session_field "feishu-choice-e2e:ou_owner" reasoningEffortOverride "__ABSENT__"

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound"


def inject(message_id, text):
    request = urllib.request.Request(
        base,
        data=json.dumps({
            "providerId": "feishu-choice-e2e",
            "chatId": "oc_direct",
            "chatType": "p2p",
            "userId": "ou_owner",
            "messageId": message_id,
            "eventId": "evt-" + message_id,
            "text": text
        }).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")


inject("om_model_text", "/model gpt-unit")
inject("om_effort_text", "/effort high")
inject("om_models_text", "/models")
inject("om_efforts_text", "/efforts")
PY
wait_session_field "feishu-choice-e2e:ou_owner" modelOverride "gpt-unit"
wait_session_field "feishu-choice-e2e:ou_owner" reasoningEffortOverride "high"
wait_capture_source "om_model_text"
wait_capture_source "om_effort_text"
wait_capture_source "om_models_text"
wait_capture_source "om_efforts_text"

python3 - "$TEST_DIR" "$FEISHU_DRY_RUN" <<'PY'
import json
import pathlib
import sys

test_dir, capture_path = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
state = json.loads(
    (test_dir / "agent/im_gateway/session_state.json").read_text(encoding="utf-8")
)
sessions = state["sessions"]
direct = next(
    session
    for session in sessions.values()
    if session.get("sessionKey") == "feishu-choice-e2e:ou_owner"
    and session.get("runnerId") == "choice-codex"
)
group = next(
    session
    for session in sessions.values()
    if session.get("sessionKey") == "im:feishu-choice-e2e:group:oc_group"
    and session.get("runnerId") == "choice-codex"
)
assert direct["externalThreadId"] == "11111111-1111-1111-1111-111111111111", direct
assert direct["modelOverride"] == "gpt-unit", direct
assert direct["reasoningEffortOverride"] == "high", direct
assert group["modelOverride"] == "gpt-unit", group

rows = [
    json.loads(line)
    for line in capture_path.read_text(encoding="utf-8").splitlines()
]
choice_cards = [
    row for row in rows
    if row.get("kind") == "card"
    and any(
        child.get("tag") == "button"
        for element in row.get("card", {}).get("body", {}).get("elements", [])
        for column in element.get("columns", [])
        for child in column.get("elements", [])
    )
]
assert len(choice_cards) == 4, choice_cards
commands = {
    child["behaviors"][0]["value"]["command"]
    for row in choice_cards
    for element in row["card"]["body"]["elements"]
    for column in element.get("columns", [])
    for child in column.get("elements", [])
    if child.get("tag") == "button"
}
assert "/resume 11111111-1111-1111-1111-111111111111" in commands, commands
assert "/model gpt-unit" in commands and "/model clear" in commands, commands
assert "/effort high" in commands and "/effort clear" in commands, commands

for source_message_id, expected_text in [
    ("om_models_text", "gpt-unit"),
    ("om_efforts_text", "high"),
]:
    row = next(
        item for item in rows
        if item.get("kind") == "card"
        and item.get("sourceMessageId") == source_message_id
    )
    serialized = json.dumps(row["card"], ensure_ascii=False)
    assert expected_text in serialized, row
    assert not any(
        child.get("tag") == "button"
        and any(
            behavior.get("type") == "callback"
            for behavior in child.get("behaviors", [])
        )
        for element in row["card"]["body"]["elements"]
        for column in element.get("columns", [])
        for child in column.get("elements", [])
    ), row
PY

echo "[feishu-slash-choice] PASS"
