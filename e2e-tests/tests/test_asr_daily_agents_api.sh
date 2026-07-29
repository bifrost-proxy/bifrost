#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-daily-research-e2e.XXXXXX")"
DATA_DIR="$TEST_DIR/data"
AUDIO_DIR="$TEST_DIR/audio"
MOCK_CODEX="$TEST_DIR/mock-codex"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  local status=$?
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ $status -ne 0 || "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    tail -200 "$BIFROST_LOG" >&2 || true
    echo "[asr-daily-agents] test data: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

mkdir -p "$DATA_DIR" "$AUDIO_DIR"

cat >"$MOCK_CODEX" <<'PY'
#!/usr/bin/env python3
import json
import os
import pathlib
import re
import sys

if "--version" in sys.argv:
    print("codex-cli 0.144.1")
    raise SystemExit(0)

thread_id = f"thread-daily-{os.getpid()}"
turn_index = 0

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def report_content(report_path, prompt):
    if "/research_dispatcher/" in report_path:
        if "2026-07-14-report.md" in report_path:
            return """# Research manifest

```json
{"questions":[]}
```
"""
        return """# Research manifest

```json
{"questions":[{"id":"github-question","original_question":"IBKR 仓库如何计算成交成本？","source_excerpt":"帮我记录一下并研究 IBKR 成交成本","background":"日报中的投资研究问题","runner":"web-research","github_repositories":["ibkr-portfolio-dashboard"],"research_prompt":"列出实际读取的仓库文件"},{"id":"product-question","original_question":"日报研究问题如何做到每题独立会话？","source_excerpt":"帮我记录一下自动研究流程","background":"日报 Agent 产品设计","runner":"web-research","research_prompt":"给出直接结论"}]}
```
"""
    if "/research_seed/" in report_path:
        if "2026-07-14-report.md" in report_path:
            return """# Research seeds

```json
{"research_questions":[],"non_research_items":[{"source_excerpt":"修复线上超时","classification":"internal_investigation","reason":"这是内部执行事项"}]}
```
"""
        return """# Research seeds

```json
{"research_questions":[{"id":"github-question","original_question":"IBKR 仓库如何计算成交成本？","source_excerpt":"帮我记录一下并研究 IBKR 成交成本","background":"日报中的投资研究问题","intent_evidence":"需要仓库代码和计算口径","expected_evidence":["实际仓库文件"]},{"id":"product-question","original_question":"日报研究问题如何做到每题独立会话？","source_excerpt":"帮我记录一下自动研究流程","background":"日报 Agent 产品设计","intent_evidence":"需要跨产品研究与方案比较","expected_evidence":["产品与实现资料"]}],"non_research_items":[{"source_excerpt":"帮我记录一下修复线上超时","classification":"internal_investigation","reason":"需要当前系统日志和 Trace，不是外部研究"}]}
```
"""
    if "/research_digest/" in report_path:
        if "2026-07-14-report.md" in report_path:
            return "# Research digest\n\n本日报未识别到需要外部研究的问题。\n"
        links = []
        for upstream in pathlib.Path.cwd().glob("input/upstream/research_fanout/*-report.md"):
            links.extend(re.findall(r"https://chatgpt\.com/c/[A-Za-z0-9_-]+", upstream.read_text(encoding="utf-8")))
        unique_links = sorted(set(links))
        return (
            "# Research digest\n\n"
            "## IBKR 仓库如何计算成交成本？\n\n"
            "- 核心结论：已由独立研究会话处理。\n"
            + "".join(f"- 完整研究：{link}\n" for link in unique_links)
        )
    return (
        "# Daily report\n\n"
        "- 原始问题：帮我记录一下并研究 IBKR 成交成本。\n"
        "- 决定：每题使用独立研究会话。\n"
        "- 待办：保留原始问题和完整研究链接。\n"
    )

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{"userAgent":"mock-daily-codex"}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":thread_id}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":thread_id}}})
    elif method == "turn/start":
        turn_index += 1
        turn_id = f"turn-daily-{turn_index}"
        prompt = frame["params"]["input"][0]["text"]
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        match = re.search(r"report=([^\n\r]+)", prompt)
        if not match:
            message = "mock daily runner received a prompt without a report target"
        else:
            report_path = match.group(1).strip().strip("` ")
            path = pathlib.Path(report_path)
            if not path.is_absolute():
                path = pathlib.Path.cwd() / path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(report_content(report_path, prompt), encoding="utf-8")
            message = f"wrote {path.name}"
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":f"message-{turn_index}","type":"agentMessage","text":message}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
PY
chmod +x "$MOCK_CODEX"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_DATA_DIR="$DATA_DIR" BIFROST_E2E=1 BIFROST_CHATGPT_WEB_E2E_MOCK=1 \
  BIFROST_CHATGPT_WEB_E2E_MOCK_PLANNING_FIRST=1 \
  "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

READY=false
for _ in $(seq 1 160); do
  if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
    exit 1
  fi
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    READY=true
    break
  fi
  sleep 0.25
done
[[ "$READY" == "true" ]]

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$REPO_DIR" "$DATA_DIR" "$AUDIO_DIR" <<'PY'
import json
import pathlib
import sys
import time
import urllib.error
import urllib.request

port, executable, repo_dir, data_dir, audio_dir = sys.argv[1:]
base = f"http://127.0.0.1:{port}/_bifrost/api"

def request(method, path, payload=None, expected=200):
    expected_statuses = expected if isinstance(expected, tuple) else (expected,)
    data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(
        base + path,
        data=data,
        headers={"content-type":"application/json"},
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            assert response.status in expected_statuses, response.status
            return json.loads(response.read().decode("utf-8") or "{}")
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        if error.code in expected_statuses:
            return json.loads(body or "{}")
        raise AssertionError((error.code, body)) from error

request("PATCH", "/im-gateway/chat/config", {
    "version": 1,
    "defaultRunnerId": "daily-codex",
    "runners": {
        "daily-codex": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {
                "executable": executable,
                "transport": "app_server",
                "sandbox": "read-only",
                "approvalPolicy": "never",
                "timeoutSecs": 60,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "workDir": repo_dir,
            "deliveryMode": "final_reply",
        },
        "web-research": {
            "enabled": True,
            "adapter": "chatgpt_web",
            "adapterConfig": {},
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        },
    },
    "channels": {},
})

task = request("POST", "/asr/tasks", {
    "name": "daily research external runner e2e",
    "audio_dir": audio_dir,
    "enabled": False,
    "recursive": False,
    "daily_agent": {"enabled": True},
}, expected=(200, 201))
task_id = task["id"]

def agent(agent_id, runner, output_dir, dependencies=None, fanout=None):
    value = {
        "id": agent_id,
        "name": agent_id,
        "enabled": True,
        "runner": runner,
        "timeout_ms": 60000,
        "trigger_policy": "after_asr_run",
        "instructions_source": "default",
        "im_delivery": {
            "enabled": False,
            "mode": "summary",
            "send_policy": "on_success_with_report",
        },
        "output_dir": output_dir,
        "dependencies": [
            {"agent_id": dependency, "include_output": True}
            for dependency in (dependencies or [])
        ],
        "dependency_failure_policy": "skip",
    }
    if fanout is not None:
        value["research_fanout"] = fanout
    return value

# Deliberately reverse the stored array; execution must still use the DAG.
agents = [
    agent("research_digest", "daily-codex", "research_digest", ["research_fanout"]),
    agent(
        "research_fanout",
        "web-research",
        "research_result",
        ["research_dispatcher"],
        {
            "max_questions": 8,
            "chatgpt_project_url": "https://chatgpt.com/g/g-p-daily-research/project",
            "allowed_runners": ["web-research"],
            "context_profiles": {},
        },
    ),
    agent("research_dispatcher", "daily-codex", "research_dispatcher", ["research_seed"]),
    agent("research_seed", "daily-codex", "research_seed", ["daily_report"]),
    agent("daily_report", "daily-codex", "report"),
]
updated = request("PUT", f"/asr/tasks/{task_id}/daily-agent", {
    "enabled": True,
    "agents": agents,
})
stored = {item["id"]: item for item in updated["config"]["agents"]}
assert stored["research_fanout"]["research_fanout"]["chatgpt_interface_mode"] == "chat", stored
assert stored["research_fanout"]["research_fanout"]["chatgpt_model"] == "pro", stored

invalid = list(agents)
invalid[0] = dict(invalid[0], dependencies=[{"agent_id":"missing-agent","include_output":True}])
rejected = request("PUT", f"/asr/tasks/{task_id}/daily-agent", {"agents":invalid}, expected=400)
assert "missing-agent" in json.dumps(rejected), rejected

date = "2026-07-13"
daily_dir = pathlib.Path(data_dir) / "asr" / "data" / "text" / task_id / ".daily"
daily_dir.mkdir(parents=True, exist_ok=True)
(daily_dir / f"{date}.md").write_text(
    "# 2026-07-13\n\n"
    "帮我记录一下并研究 IBKR 成交成本。\n"
    "另一个问题：日报研究问题如何做到每题独立会话？\n"
    "帮我记录一下修复线上超时并查询 Trace。\n",
    encoding="utf-8",
)
queued = request(
    "POST",
    f"/asr/tasks/{task_id}/daily-agent/run?date={date}&force=1",
    expected=(200, 202),
)
assert queued["status"] in ("queued", "already_running"), queued

runs = None
expected_agents = {
    "daily_report",
    "research_seed",
    "research_dispatcher",
    "research_fanout",
    "research_digest",
}
for _ in range(180):
    runs = request("GET", f"/asr/tasks/{task_id}/daily-agent/runs")
    docs = {
        item["agent_id"]: item
        for item in runs.get("processed_documents", [])
        if item.get("date") == date
    }
    if set(docs) == expected_agents:
        break
    time.sleep(0.5)
else:
    raise AssertionError(runs)

order = [item["agent_id"] for item in runs["processed_documents"] if item.get("date") == date]
assert set(order) == expected_agents, order
fanout_dir = daily_dir / "agents" / "research_fanout" / "output" / "research_result"
children_dir = fanout_dir / date
manifest = json.loads((children_dir / "manifest.json").read_text(encoding="utf-8"))
assert len(manifest["questions"]) == 2, manifest
assert "修复线上超时" not in json.dumps(manifest, ensure_ascii=False), manifest

seed_path = daily_dir / "agents" / "research_seed" / "output" / "research_seed" / f"{date}-report.md"
seed_report = seed_path.read_text(encoding="utf-8")
assert '"classification":"internal_investigation"' in seed_report, seed_report

github = json.loads((children_dir / "github-question.json").read_text(encoding="utf-8"))
product = json.loads((children_dir / "product-question.json").read_text(encoding="utf-8"))
github_report = (children_dir / "github-question.md").read_text(encoding="utf-8")
product_report = (children_dir / "product-question.md").read_text(encoding="utf-8")
assert github["original_question"] == "IBKR 仓库如何计算成交成本？", github
assert product["original_question"] == "日报研究问题如何做到每题独立会话？", product
assert github["conversation_id"] != product["conversation_id"], (github, product)
assert github["full_report_link"].startswith("https://chatgpt.com/c/"), github
assert product["full_report_link"].startswith("https://chatgpt.com/c/"), product
assert github["github_connector_status"] == "missing", github
for report in (github_report, product_report):
    assert "## 原始问题" in report, report
    assert "## 核心结论" in report, report
    assert "## 事实与证据" in report, report
    assert "## 推断与不确定性" in report, report
    assert "## 对原始问题的直接回答" in report, report
wait_prompts = [
    path
    for path in (pathlib.Path(data_dir) / "im_gateway" / "runs").glob("*/prompt.md")
    if not path.read_text(encoding="utf-8").strip()
]
assert len(wait_prompts) >= 2, wait_prompts
retry_prompts = [
    path
    for path in (pathlib.Path(data_dir) / "im_gateway" / "runs").glob("*/prompt.md")
    if "上一条回复不是最终研究报告"
    in path.read_text(encoding="utf-8")
]
assert len(retry_prompts) >= 2, retry_prompts

fanout_report = (fanout_dir / f"{date}-report.md").read_text(encoding="utf-8")
assert github["full_report_link"] in fanout_report, fanout_report
assert product["full_report_link"] in fanout_report, fanout_report
digest_upstream = daily_dir / "agents" / "research_digest" / "input" / "upstream" / "research_fanout" / f"{date}-report.md"
digest_input = digest_upstream.read_text(encoding="utf-8")
assert github["full_report_link"] in digest_input, digest_input
assert product["full_report_link"] in digest_input, digest_input
digest_path = daily_dir / "agents" / "research_digest" / "output" / "research_digest" / f"{date}-report.md"
digest = digest_path.read_text(encoding="utf-8")
assert github["full_report_link"] in digest, digest
assert product["full_report_link"] in digest, digest

empty_date = "2026-07-14"
(daily_dir / f"{empty_date}.md").write_text(
    "# 2026-07-14\n\n帮我记录一下修复线上超时。\n",
    encoding="utf-8",
)
queued = request(
    "POST",
    f"/asr/tasks/{task_id}/daily-agent/run?date={empty_date}&force=1",
    expected=(200, 202),
)
assert queued["status"] in ("queued", "already_running"), queued

empty_runs = None
for _ in range(180):
    empty_runs = request("GET", f"/asr/tasks/{task_id}/daily-agent/runs")
    empty_docs = {
        item["agent_id"]: item
        for item in empty_runs.get("processed_documents", [])
        if item.get("date") == empty_date
    }
    if set(empty_docs) == expected_agents:
        break
    time.sleep(0.5)
else:
    raise AssertionError(empty_runs)

empty_children_dir = fanout_dir / empty_date
empty_manifest = json.loads(
    (empty_children_dir / "manifest.json").read_text(encoding="utf-8")
)
assert empty_manifest["questions"] == [], empty_manifest
assert list(empty_children_dir.glob("*.json")) == [empty_children_dir / "manifest.json"]
empty_fanout_report = (
    fanout_dir / f"{empty_date}-report.md"
).read_text(encoding="utf-8")
assert "本日报未识别到需要外部研究的问题" in empty_fanout_report, empty_fanout_report
empty_digest_upstream = (
    daily_dir
    / "agents"
    / "research_digest"
    / "input"
    / "upstream"
    / "research_fanout"
    / f"{empty_date}-report.md"
).read_text(encoding="utf-8")
assert "本日报未识别到需要外部研究的问题" in empty_digest_upstream
empty_digest = (
    daily_dir
    / "agents"
    / "research_digest"
    / "output"
    / "research_digest"
    / f"{empty_date}-report.md"
).read_text(encoding="utf-8")
assert "本日报未识别到需要外部研究的问题" in empty_digest

print(f"[asr-daily-agents] PASS task={task_id}")
PY
