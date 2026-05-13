#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-18921}"
MODEL_PORT="${MODEL_PORT:-18922}"
RESEARCH_PORT="${RESEARCH_PORT:-18923}"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MODEL_LOG="$TEST_DIR/model.jsonl"
RESPONSE_FILE="$TEST_DIR/chat-response.json"
TOOLS_FILE="$TEST_DIR/tools.json"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
FINAL_TEXT="VOICE_MODEL_RESEARCH_ARTICLE_READY"

cleanup() {
  local exit_code="$?"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-research-pack-chat-loop] DEBUG: temp dir $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 160 "$BIFROST_LOG" >&2 || true
    [[ -f "$MODEL_LOG" ]] && tail -n 20 "$MODEL_LOG" >&2 || true
    [[ -f "$RESPONSE_FILE" ]] && cat "$RESPONSE_FILE" >&2 || true
  fi
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MODEL_PID:-}" ]]; then
    kill "$MODEL_PID" >/dev/null 2>&1 || true
    wait "$MODEL_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${RESEARCH_PID:-}" ]]; then
    kill "$RESEARCH_PID" >/dev/null 2>&1 || true
    wait "$RESEARCH_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
  return "$exit_code"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  for _ in $(seq 1 120); do
    curl -fsS --noproxy '*' "$url" >/dev/null 2>&1 && return 0
    sleep 0.25
  done
  echo "[agent-research-pack-chat-loop] endpoint not ready: $url" >&2
  return 1
}

python3 - "$RESEARCH_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

ARTICLE = """<!doctype html><html><head><title>语音大模型技术观察</title></head><body>
<article>
<h1>语音大模型技术观察</h1>
<p>语音大模型正在从语音识别、语音合成和说话人理解的单点模型，演进为具备语音理解、实时交互、多模态对齐和工具调用能力的统一模型。</p>
<p>工程落地重点包括低延迟流式推理、端云协同、噪声鲁棒性、长音频上下文、隐私保护和可观测评测体系。</p>
</article>
</body></html>"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        if self.path.startswith("/article"):
            body = ARTICLE.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        if self.path == "/search":
            body = json.dumps({
                "results": [{
                    "title": "语音大模型技术观察",
                    "url": f"http://127.0.0.1:{port}/article",
                    "snippet": "一篇关于语音大模型架构、应用与工程挑战的技术资料。",
                    "site_name": "Bifrost Research Fixture",
                    "author": "Bifrost Test",
                    "published_at": "2026-05-13",
                    "score": 0.99,
                    "query": payload.get("query"),
                }]
            }).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
RESEARCH_PID=$!
wait_http "http://127.0.0.1:$RESEARCH_PORT/health"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
  if [[ -x "$REPO_DIR/target/release/bifrost" ]]; then
    BIFROST_BIN="$REPO_DIR/target/release/bifrost"
  elif [[ -x "$REPO_DIR/target/debug/bifrost" ]]; then
    BIFROST_BIN="$REPO_DIR/target/debug/bifrost"
  else
    echo "[agent-research-pack-chat-loop] missing bifrost binary: $BIFROST_BIN" >&2
    exit 1
  fi
fi

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" agent research init \
  --preset ai-tech \
  --web-provider fixture_research \
  --base-url "http://127.0.0.1:$RESEARCH_PORT/search" \
  --yes >/dev/null

python3 - "$TEST_DIR/agent/agent_config.json" <<'PY'
import json
import sys
path = sys.argv[1]
data = json.load(open(path, encoding="utf-8"))
research = data["research"]
research["fetch_policy"]["allow_localhost"] = True
research["fetch_policy"]["allow_private_ip"] = True
json.dump(data, open(path, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY

python3 - "$MODEL_PORT" "$MODEL_LOG" "$FINAL_TEXT" "$RESEARCH_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
final_text = sys.argv[3]
research_port = sys.argv[4]
state = {"n": 0}


def tool_call(call_id, name, arguments):
    return {"id": call_id, "type": "function", "function": {"name": name, "arguments": arguments}}


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    return json.dumps(content, ensure_ascii=False)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        state["n"] += 1
        n = state["n"]
        if n == 1:
            names = [(tool.get("function") or {}).get("name") for tool in payload.get("tools", [])]
            required = {"research_search", "research_fetch", "knowledge_search", "knowledge_save", "research_digest"}
            missing = sorted(required - set(names))
            if missing:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing": missing, "tools": names}, ensure_ascii=False).encode())
                return
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call("call-knowledge-search", "knowledge_search", json.dumps({"query": "语音大模型", "limit": 5}, ensure_ascii=False)),
                    tool_call("call-research-search", "research_search", json.dumps({"query": "语音大模型", "sources": ["web"], "limit": 1, "fetch_content": True}, ensure_ascii=False)),
                    tool_call("call-research-fetch", "research_fetch", json.dumps({"url": f"http://127.0.0.1:{research_port}/article", "max_bytes": 200000}, ensure_ascii=False)),
                ],
            }
            finish_reason = "tool_calls"
        elif n == 2:
            outputs = "\n".join(message_text(m) for m in payload.get("messages", []) if m.get("role") == "tool")
            if "content_markdown" not in outputs or "canonical_url" not in outputs or "retrieved_at" not in outputs:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_markdown_artifact_fields": outputs[-2000:]}, ensure_ascii=False).encode())
                return
            markdown = "# 语音大模型技术观察\n\n语音大模型正在把 ASR、TTS、说话人识别、语义理解和多模态交互统一到一个可流式推理的系统里。\n\nSource: fixture."
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call("call-save", "knowledge_save", json.dumps({"items": [{
                        "url": f"http://127.0.0.1:{research_port}/article",
                        "title": "语音大模型技术观察",
                        "source": "web",
                        "provider": "fixture_research",
                        "query": "语音大模型",
                        "author": "Bifrost Test",
                        "published_at": "2026-05-13",
                        "content_markdown": markdown,
                        "summary": "语音大模型架构、应用和工程挑战的技术观察。",
                        "tags": ["ai", "voice-model", "speech", "chat-e2e"],
                    }]}, ensure_ascii=False)),
                    tool_call("call-digest", "research_digest", json.dumps({"task_id": "chat_voice_model_article", "query": "语音大模型", "format": "markdown"}, ensure_ascii=False)),
                ],
            }
            finish_reason = "tool_calls"
        else:
            outputs = "\n".join(message_text(m) for m in payload.get("messages", []) if m.get("role") == "tool")
            if "chat_voice_model_article" not in outputs or "items_used" not in outputs:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_digest_output": outputs[-2000:]}, ensure_ascii=False).encode())
                return
            message = {
                "role": "assistant",
                "content": f"{final_text}\n\n# 语音大模型：从识别工具到实时智能入口\n\n语音大模型的关键变化，是把语音识别、语音合成、说话人特征、语义理解和对话决策放到同一个上下文里处理。它不再只是把声音转成文字，而是直接理解语气、场景、轮次和意图，并把结果交给后续工具或业务系统。\n\n## 技术主线\n\n第一条主线是流式端到端建模。实时助手、会议纪要和车载语音都要求低延迟，系统需要边听边理解、边生成，不能等整段音频结束后再处理。\n\n第二条主线是多模态对齐。语音输入常常和文本、图像、屏幕状态或业务数据同时出现，模型需要把声学信息和语义上下文对齐，才能完成复杂任务。\n\n第三条主线是可控生成。语音输出要稳定控制音色、情绪、语速和停顿，同时避免幻觉式补全、敏感内容泄露和错误指令执行。\n\n## 工程落地挑战\n\n落地难点集中在四处：低延迟推理成本、噪声和口音鲁棒性、长音频记忆压缩，以及授权边界。真正可用的语音大模型系统，需要把模型能力、检索工具、业务 API、用户确认和审计日志组合起来。\n\n## 应用判断\n\n短期最适合落地的方向是客服质检、会议助手、语音输入增强、销售陪练和多语言内容生产。中期价值会转向实时语音 Agent：它能听、能问、能查资料、能调用工具，也能在关键动作前请求用户确认。\n\n结论是，语音大模型会成为 AI 应用最自然的入口之一，但竞争点不只在模型参数，而在实时链路、工具编排、权限治理和稳定评测体系。",
            }
            finish_reason = "stop"
        body = json.dumps({"choices": [{"message": message, "finish_reason": finish_reason}], "usage": {"total_tokens": 42}}, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MODEL_PID=$!
wait_http "http://127.0.0.1:$MODEL_PORT/health"

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"
curl -fsS --noproxy '*' "$BASE/tools" >"$TOOLS_FILE"
python3 - "$TOOLS_FILE" <<'PY'
import json, sys
tools = json.load(open(sys.argv[1], encoding="utf-8"))["tools"]
names = {(tool.get("function") or {}).get("name") for tool in tools}
required = {"research_search", "research_fetch", "knowledge_search", "knowledge_save", "research_digest"}
missing = required - names
assert not missing, missing
PY

curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-research-chat\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MODEL_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 30,
    \"max_turn_iterations\": 6,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-research-pack-chat-loop","message":"请使用 Research tools 搜索并抓取“语音大模型”这个技术主题的资料，整理成一篇可阅读的中文技术文章。"}' \
  >"$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$MODEL_LOG" "$TEST_DIR" "$FINAL_TEXT" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
model_log = Path(sys.argv[2])
data_dir = Path(sys.argv[3])
final_text = sys.argv[4]
assert response.get("success") is True, response
assert final_text in response.get("response", ""), response
names = [call.get("tool_name") for call in response.get("tool_calls", [])]
expected = ["knowledge_search", "research_search", "research_fetch", "knowledge_save", "research_digest"]
assert names[: len(expected)] == expected, names
assert all(call.get("success") is True for call in response.get("tool_calls", [])[: len(expected)]), response.get("tool_calls")
report_dir = data_dir / "agent" / "reports" / "chat_voice_model_article"
reports = list(report_dir.glob("*.md"))
assert reports, f"missing digest report in {report_dir}"
report_text = reports[-1].read_text(encoding="utf-8")
assert "语音大模型技术观察" in report_text, report_text
assert "provider: `fixture_research`" in report_text, report_text
payloads = [json.loads(line) for line in model_log.read_text(encoding="utf-8").splitlines() if line.strip()]
first_tools = {(tool.get("function") or {}).get("name") for tool in payloads[0].get("tools", [])}
assert "research_search" in first_tools and "research_digest" in first_tools, first_tools
assert "语音大模型" in response.get("response", ""), response
PY

echo "[agent-research-pack-chat-loop] PASS"
