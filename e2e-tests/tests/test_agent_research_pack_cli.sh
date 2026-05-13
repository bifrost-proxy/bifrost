#!/usr/bin/env bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
MOCK_PID=""
PASSED=0
FAILED=0

header() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
}

info() {
    echo -e "${CYAN}[INFO]${NC} $1"
}

pass() {
    echo -e "  ${GREEN}✓${NC} $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo -e "  ${RED}✗${NC} $1"
    FAILED=$((FAILED + 1))
}

cleanup() {
    if [[ -n "$MOCK_PID" ]]; then
        kill "$MOCK_PID" >/dev/null 2>&1 || true
        wait "$MOCK_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}

trap cleanup EXIT

run_bifrost() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" RESEARCH_TEST_KEY="secret-test-key" "$BIFROST_BIN" "$@" 2>&1
}

setup() {
    header "准备 Research Pack CLI E2E 环境"
    if [[ ! -x "$BIFROST_BIN" ]]; then
        fail "找不到 bifrost 二进制: $BIFROST_BIN"
        return
    fi
    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    info "测试数据目录: $TEST_DATA_DIR"
}

start_mock_provider() {
    header "启动本地 mock research provider"
    local port_file="${TEST_DATA_DIR}/mock-provider.port"
    local server_file="${TEST_DATA_DIR}/mock-provider.py"
    cat > "$server_file" <<'PY'
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        if self.headers.get("authorization") != "Bearer secret-test-key":
            self.send_response(401)
            self.end_headers()
            self.wfile.write(b"missing bearer token")
            return
        body = {
            "results": [
                {
                    "title": "Mock AI Agent MCP Research",
                    "url": "https://example.com/agent-mcp?utm_source=test#frag",
                    "snippet": "Normalized mock result for %s" % payload.get("query", ""),
                    "site_name": "Example",
                    "published_at": "2026-05-13",
                    "score": 0.91,
                }
            ]
        }
        encoded = json.dumps(body).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self):
        body = b"<html><head><title>Private Article</title></head><body>blocked</body></html>"
        self.send_response(200)
        self.send_header("content-type", "text/html")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(os.environ["MOCK_PORT_FILE"], "w", encoding="utf-8") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
    MOCK_PORT_FILE="$port_file" python3 "$server_file" &
    MOCK_PID="$!"
    for _ in {1..50}; do
        [[ -s "$port_file" ]] && break
        sleep 0.1
    done
    if [[ ! -s "$port_file" ]]; then
        fail "mock provider 未能启动"
        return
    fi
    MOCK_PORT="$(cat "$port_file")"
    info "mock provider: http://127.0.0.1:${MOCK_PORT}"
    pass "mock provider 已启动"
}

test_init_config() {
    header "测试 research init 写入隔离配置"
    local output
    output=$(run_bifrost agent research init \
        --preset personal-cn \
        --web-provider mock \
        --base-url "http://127.0.0.1:${MOCK_PORT}/search" \
        --api-key '$RESEARCH_TEST_KEY' \
        --yes)
    if grep -q "Research Pack initialized" <<<"$output" && [[ -f "${TEST_DATA_DIR}/agent/agent_config.json" ]]; then
        pass "research init 创建了启用的 Agent 配置"
    else
        fail "research init 输出或配置异常: $output"
    fi
}

test_provider_search() {
    header "测试 research provider test 标准化结果"
    local output
    output=$(run_bifrost agent research provider test mock --query "AI Agent MCP")
    if grep -q "Mock AI Agent MCP Research" <<<"$output" \
        && grep -q '"provider": "mock"' <<<"$output" \
        && grep -q '"source": "web"' <<<"$output"; then
        pass "provider test 返回标准化 web 结果"
    else
        fail "provider test 结果异常: $output"
    fi
}

test_search_command() {
    header "测试 research search 统一入口"
    local output
    output=$(run_bifrost agent research search "AI Agent MCP" --limit 1)
    if grep -q "https://example.com/agent-mcp?utm_source=test" <<<"$output" && ! grep -q "#frag" <<<"$output"; then
        pass "research search 返回去 fragment 后的规范 URL"
    else
        fail "research search URL 标准化异常: $output"
    fi
}

test_fetch_policy_blocks_localhost() {
    header "测试 research fetch 默认阻止 localhost"
    local output
    set +e
    output=$(run_bifrost agent research fetch "http://127.0.0.1:${MOCK_PORT}/article")
    local status=$?
    set -e
    if [[ "$status" -ne 0 ]] && grep -qiE "localhost|loopback|private" <<<"$output"; then
        pass "research fetch 默认拒绝 localhost/private IP"
    else
        fail "research fetch 未按默认策略拒绝 localhost: status=${status}, output=${output}"
    fi
}

setup
start_mock_provider
test_init_config
test_provider_search
test_search_command
test_fetch_policy_blocks_localhost

echo ""
echo "通过: $PASSED, 失败: $FAILED"
if [[ "$FAILED" -ne 0 ]]; then
    exit 1
fi
