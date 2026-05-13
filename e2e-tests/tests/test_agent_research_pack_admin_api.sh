#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-18961}"
MOCK_PORT="${MOCK_PORT:-18962}"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 120); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[agent-research-pack-admin-api] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        body = """<html><head><title>Mock Voice Model Article</title></head><body><article><h1>Mock Voice Model Article</h1><p>Full Markdown body for 语音大模型 research.</p><p>Metadata should include canonical URL and content hash.</p></article></body></html>""".encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        if self.path == "/volc":
            if self.headers.get("Authorization") != "Bearer e2e-token":
                self.send_response(401)
                self.end_headers()
                return
            body = {
                "Result": {
                    "ResultCount": 1,
                    "WebResults": [
                        {
                            "Id": "volc-1",
                            "SortId": 1,
                            "Title": "语音大模型火山资源",
                            "SiteName": "Mock Volc Research",
                            "Url": f"http://127.0.0.1:{port}/volc-article?from=research#fragment",
                            "Snippet": f"Top resource for {payload.get('Query', '')}",
                            "Summary": "Volc summary for 语音大模型",
                            "Content": "## 语音大模型火山解析\n\nFull Markdown body for 语音大模型 research.",
                            "PublishTime": "2026-05-13T00:00:00+08:00",
                            "RankScore": 0.99,
                        }
                    ],
                    "SearchContext": {"OriginQuery": payload.get("Query", ""), "SearchType": payload.get("SearchType", "web")},
                    "TimeCost": 1,
                    "LogId": "mock-log-id",
                }
            }
            encoded = json.dumps(body).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            return
        web_provider_by_path = {
            "/search-generic": ("mock", "语音大模型通用网页资源", "Mock Generic Research", "/generic-article"),
            "/search-tavily": ("tavily_mock", "语音大模型 Tavily 资源", "Mock Tavily Research", "/tavily-article"),
            "/search-exa": ("exa_mock", "语音大模型 Exa 资源", "Mock Exa Research", "/exa-article"),
            "/search-custom": ("custom_mock", "语音大模型自定义网页资源", "Mock Custom Research", "/custom-article"),
        }
        if self.path not in web_provider_by_path:
            self.send_error(404)
            return
        provider_id, title, site_name, article_path = web_provider_by_path[self.path]
        body = {
            "results": [
                {
                    "title": title,
                    "url": f"http://127.0.0.1:{port}{article_path}?from={provider_id}#fragment",
                    "snippet": f"Top resource for {payload.get('query', '')}",
                    "site_name": site_name,
                    "author": "Bifrost E2E",
                    "published_at": "2026-05-13",
                    "score": 0.99,
                }
            ]
        }
        encoded = json.dumps(body).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock research provider"
export ARK_TOKEN="e2e-token"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-research-pack-admin-api] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[agent-research-pack-admin-api] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[agent-research-pack-admin-api] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/research"

echo "[agent-research-pack-admin-api] configuring research provider"
curl -fsS --noproxy '*' -X PATCH "$BASE/config" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"preset\": \"custom\",
    \"providers\": {
      \"volc_mock\": {
        \"enabled\": true,
        \"type\": \"volc_web_search\",
        \"base_url\": \"http://127.0.0.1:$MOCK_PORT/volc\",
        \"env_key\": \"ARK_TOKEN\",
        \"search_type\": \"web\",
        \"count\": 1,
        \"need_content\": true,
        \"need_url\": true,
        \"need_summary\": false,
        \"content_formats\": \"markdown\",
        \"query_rewrite\": false
      },
      \"mock\": {
        \"enabled\": true,
        \"type\": \"generic_web_search\",
        \"base_url\": \"http://127.0.0.1:$MOCK_PORT/search-generic\"
      },
      \"tavily_mock\": {
        \"enabled\": true,
        \"type\": \"tavily\",
        \"base_url\": \"http://127.0.0.1:$MOCK_PORT/search-tavily\"
      },
      \"exa_mock\": {
        \"enabled\": true,
        \"type\": \"exa\",
        \"base_url\": \"http://127.0.0.1:$MOCK_PORT/search-exa\"
      },
      \"custom_mock\": {
        \"enabled\": true,
        \"type\": \"custom_http\",
        \"base_url\": \"http://127.0.0.1:$MOCK_PORT/search-custom\"
      }
    },
    \"provider_order\": [\"volc_mock\", \"mock\", \"tavily_mock\", \"exa_mock\", \"custom_mock\"],
    \"cache\": {\"enabled\": true, \"store\": \"sqlite\", \"retention_days\": 180},
    \"defaults\": {
      \"sources\": [\"web\", \"wechat\"],
    \"limit\": 8,
      \"fetch_content\": true,
      \"language\": \"zh-CN\"
    },
    \"fetch_policy\": {
      \"allow_private_ip\": true,
      \"allow_localhost\": true,
      \"max_redirects\": 5,
      \"max_response_bytes\": 500000,
      \"timeout_secs\": 20,
      \"user_agent\": \"BifrostResearchE2E/0.1\"
    },
    \"tasks\": []
  }" >/dev/null

CAPABILITIES_FILE="$TEST_DIR/capabilities.json"
curl -fsS --noproxy '*' "$BASE/capabilities" > "$CAPABILITIES_FILE"

VOLC_SEARCH_FILE="$TEST_DIR/search-volc.json"
curl -fsS --noproxy '*' -X POST "$BASE/search" \
  -H 'Content-Type: application/json' \
  -d '{
    "query": "语音大模型",
    "sources": ["web"],
    "provider_ids": ["volc_mock"],
    "freshness": null,
    "limit": 1,
    "fetch_content": true,
    "language": "zh-CN"
  }' > "$VOLC_SEARCH_FILE"

ALL_SOURCE_SEARCH_FILE="$TEST_DIR/search-all-sources.json"
curl -fsS --noproxy '*' -X POST "$BASE/search" \
  -H 'Content-Type: application/json' \
  -d '{
    "query": "语音大模型",
    "sources": ["web", "wechat"],
    "freshness": null,
    "limit": 8,
    "fetch_content": true,
    "language": "zh-CN"
  }' > "$ALL_SOURCE_SEARCH_FILE"

SELECTED_PROVIDER_SEARCH_FILE="$TEST_DIR/search-selected-provider.json"
curl -fsS --noproxy '*' -X POST "$BASE/search" \
  -H 'Content-Type: application/json' \
  -d '{
    "query": "语音大模型",
    "sources": ["web"],
    "provider_ids": ["exa_mock"],
    "freshness": null,
    "limit": 3,
    "fetch_content": true,
    "language": "zh-CN"
  }' > "$SELECTED_PROVIDER_SEARCH_FILE"

STREAM_SEARCH_FILE="$TEST_DIR/search-stream.ndjson"
curl -fsS --noproxy '*' -X POST "$BASE/search/stream" \
  -H 'Content-Type: application/json' \
  -d '{
    "query": "语音大模型",
    "sources": ["web"],
    "provider_ids": ["mock", "exa_mock"],
    "freshness": null,
    "limit": 1,
    "fetch_content": true,
    "language": "zh-CN"
  }' > "$STREAM_SEARCH_FILE"

python3 - "$CAPABILITIES_FILE" "$VOLC_SEARCH_FILE" "$ALL_SOURCE_SEARCH_FILE" "$SELECTED_PROVIDER_SEARCH_FILE" "$STREAM_SEARCH_FILE" "$MOCK_PORT" <<'PY'
import json
import sys
from pathlib import Path

capabilities = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
volc_search = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
all_search = json.loads(Path(sys.argv[3]).read_text(encoding="utf-8"))
selected_search = json.loads(Path(sys.argv[4]).read_text(encoding="utf-8"))
stream_events = [json.loads(line) for line in Path(sys.argv[5]).read_text(encoding="utf-8").splitlines() if line.strip()]
mock_port = sys.argv[6]

caps = {item["id"]: item for item in capabilities["capabilities"]}
for builtin in [
    "volc_web_search",
    "sogou_wechat_cdp",
    "arxiv",
    "hacker_news",
    "github_repositories",
    "generic_web_search",
    "tavily",
    "exa",
    "custom_http",
    "mcp",
]:
    assert builtin in caps, caps
assert "volc_mock" in caps, caps
assert "wechat_http" not in caps, caps
for expected in ["mock", "tavily_mock", "exa_mock", "custom_mock"]:
    assert expected in caps, caps
mock = caps["mock"]
assert mock["configured"] is True and mock["enabled"] is True, mock
assert mock["authorization_status"] == "not_required", mock
assert caps["tavily_mock"]["type"] == "tavily", caps["tavily_mock"]
assert caps["tavily_mock"]["configured"] is True and caps["tavily_mock"]["enabled"] is True, caps["tavily_mock"]
assert caps["exa_mock"]["type"] == "exa", caps["exa_mock"]
assert caps["exa_mock"]["configured"] is True and caps["exa_mock"]["enabled"] is True, caps["exa_mock"]
assert caps["custom_mock"]["type"] == "custom_http", caps["custom_mock"]
assert caps["custom_mock"]["configured"] is True and caps["custom_mock"]["enabled"] is True, caps["custom_mock"]
volc_builtin = caps["volc_web_search"]
assert volc_builtin["configured"] is True, volc_builtin
assert volc_builtin["authorization_status"] == "configured", volc_builtin
volc_mock = caps["volc_mock"]
assert volc_mock["type"] == "volc_web_search", volc_mock
assert volc_mock["authorization_status"] == "configured", volc_mock
sogou = caps["sogou_wechat_cdp"]
assert sogou["supported"] is True, sogou
assert sogou["authorization_status"] == "not_required", sogou
assert sogou["search_url_template"] == "https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query={query}", sogou
assert "logged_in" in sogou and "login_status" in sogou, sogou
mcp = caps["mcp"]
assert mcp["supported"] is False, mcp
assert mcp["authorization_status"] == "reserved", mcp

assert volc_search["query"] == "语音大模型", volc_search
volc_results = volc_search["results"]
assert len(volc_results) == 1, volc_search
volc_item = volc_results[0]
assert volc_item["title"] == "语音大模型火山资源", volc_item
assert volc_item["provider"] == "volc_mock", volc_item
assert volc_item["source"] == "web", volc_item
assert volc_item["canonical_url"] == f"http://127.0.0.1:{mock_port}/volc-article?from=research", volc_item
assert "Full Markdown body for 语音大模型 research." in volc_item["content_markdown"], volc_item
assert volc_item["content_hash"], volc_item
assert volc_item["retrieved_at"], volc_item
assert volc_item["site_name"] == "Mock Volc Research", volc_item

assert all_search["query"] == "语音大模型", all_search
all_results = all_search["results"]
providers = {item["provider"] for item in all_results}
sources = {item["source"] for item in all_results}
assert {"volc_mock", "mock", "tavily_mock", "exa_mock", "custom_mock"}.issubset(providers), all_search
assert sources == {"web"}, all_search
for item in all_results:
    assert item["canonical_url"], item
    assert item["content_markdown"], item
    assert item["content_hash"], item
    assert item["retrieved_at"], item

selected_results = selected_search["results"]
assert selected_results, selected_search
assert {item["provider"] for item in selected_results} == {"exa_mock"}, selected_search
for item in selected_results:
    assert item["content_markdown"], item
    assert item["content_hash"], item

provider_events = [event for event in stream_events if event.get("type") == "provider_result"]
assert stream_events[-1]["type"] == "done", stream_events
assert {event["provider_id"] for event in provider_events} == {"mock", "exa_mock"}, stream_events
for event in provider_events:
    assert len(event["results"]) == 1, event
    assert event["error"] is None, event
    assert event["results"][0]["content_markdown"], event
PY

echo "[agent-research-pack-admin-api] passed"
