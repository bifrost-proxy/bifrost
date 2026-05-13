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
CHROME_PROFILE=""
CHROME_PROFILE_OWNS=0
CHROME_PID=""
CHROME_BIN=""
HEADLESS_CDP=0
EXTERNAL_CDP=0
CDP_ENDPOINT="${BIFROST_RESEARCH_CDP_ENDPOINT:-}"
QUERY="${BIFROST_RESEARCH_SOGOU_QUERY:-AI Agent MCP}"
MANUAL_AUTH="${BIFROST_RESEARCH_MANUAL_AUTH:-0}"
FORCE_MANUAL_AUTH="${BIFROST_RESEARCH_FORCE_MANUAL_AUTH:-0}"
PASSED=0
FAILED=0

if [[ -n "${CI:-}" \
    && "${BIFROST_RESEARCH_REAL_SOGOU:-}" != "1" \
    && -z "${BIFROST_RESEARCH_CDP_ENDPOINT:-}" ]]; then
    echo "[SKIP] Real Sogou/WeChat CDP E2E requires BIFROST_RESEARCH_REAL_SOGOU=1 or an explicit BIFROST_RESEARCH_CDP_ENDPOINT in CI."
    echo "[SKIP] Run it locally to validate live Sogou data fetches without making main CI depend on a public anti-spider surface."
    exit 0
fi

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
    if [[ -n "$CHROME_PID" ]]; then
        kill "$CHROME_PID" >/dev/null 2>&1 || true
        wait "$CHROME_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
    if [[ "$CHROME_PROFILE_OWNS" == "1" && -n "$CHROME_PROFILE" && -d "$CHROME_PROFILE" ]]; then
        rm -rf "$CHROME_PROFILE"
    fi
}

trap cleanup EXIT

run_bifrost() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" "$@" 2>&1
}

find_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

find_edge_browser() {
    local candidates=(
        "${BIFROST_RESEARCH_EDGE_BIN:-}"
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
        "$HOME/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
    )
    for path in "${candidates[@]}"; do
        if [[ -x "$path" ]]; then
            printf '%s' "$path"
            return
        fi
    done
    command -v microsoft-edge >/dev/null 2>&1 && command -v microsoft-edge && return
    command -v msedge >/dev/null 2>&1 && command -v msedge && return
}

default_edge_user_data_dir() {
    printf '%s' "${BIFROST_RESEARCH_BROWSER_USER_DATA_DIR:-$HOME/.bifrost/web/edge-user-data}"
}

url_encode() {
    python3 - "$1" <<'PY'
import sys, urllib.parse
print(urllib.parse.quote(sys.argv[1], safe=''))
PY
}

wait_for_cdp() {
    local endpoint="$1"
    for _ in {1..80}; do
        if curl -fsS "${endpoint%/}/json/version" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

open_cdp_target() {
    local endpoint="$1"
    local target_url="$2"
    local encoded
    encoded="$(url_encode "$target_url")"
    curl -fsS -X PUT "${endpoint%/}/json/new?${encoded}" >/dev/null 2>&1 \
        || curl -fsS "${endpoint%/}/json/new?${encoded}" >/dev/null 2>&1
}

wait_for_weixin_detail_target() {
    local endpoint="$1"
    local target_url="$2"
    local targets_file
    targets_file="${TEST_DATA_DIR}/cdp-targets.json"
    for _ in {1..80}; do
        if curl -fsS "${endpoint%/}/json/list" >"$targets_file" 2>/dev/null \
            && python3 - "$targets_file" "$target_url" >"${TEST_DATA_DIR}/cdp-opened-url.txt" 2>/dev/null <<'PY'
import json, sys
pages = json.load(open(sys.argv[1]))
target = sys.argv[2]
urls = [page.get("url", "") for page in pages]
for url in urls:
    if url == target or "weixin.sogou.com/link" in url or "mp.weixin.qq.com" in url:
        print(url)
        sys.exit(0)
print(urls[0] if urls else "")
sys.exit(1)
PY
        then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

setup() {
    header "准备真实 Sogou WeChat CDP E2E 环境"
    if [[ ! -x "$BIFROST_BIN" ]]; then
        fail "找不到 bifrost 二进制: $BIFROST_BIN"
        return
    fi
    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    info "测试数据目录: $TEST_DATA_DIR"
    pass "隔离数据目录已准备"
}

start_browser_if_needed() {
    header "准备 CDP 浏览器"
    if [[ -n "$CDP_ENDPOINT" ]]; then
        if wait_for_cdp "$CDP_ENDPOINT"; then
            EXTERNAL_CDP=1
            pass "复用外部 CDP endpoint: $CDP_ENDPOINT"
            return
        fi
        fail "外部 CDP endpoint 不可用: $CDP_ENDPOINT"
        return
    fi

    local edge_bin edge_profile port
    edge_bin="$(find_edge_browser)"
    if [[ -z "$edge_bin" || ! -x "$edge_bin" ]]; then
        fail "找不到 Microsoft Edge，请设置 BIFROST_RESEARCH_EDGE_BIN 或 BIFROST_RESEARCH_CDP_ENDPOINT"
        return
    fi
    CHROME_BIN="$edge_bin"

    port="$(find_free_port)"
    edge_profile="$(default_edge_user_data_dir)"
    mkdir -p "$edge_profile"
    CHROME_PROFILE="$edge_profile"
    CHROME_PROFILE_OWNS=0
    CDP_ENDPOINT="http://127.0.0.1:${port}"
    info "使用固定 Edge 操作目录启动无头 CDP: $edge_profile"
    "$edge_bin" \
        --headless \
        --remote-debugging-address=127.0.0.1 \
        "--remote-debugging-port=${port}" \
        "--user-data-dir=${CHROME_PROFILE}" \
        --disable-gpu \
        --disable-dev-shm-usage \
        --no-first-run \
        --no-default-browser-check \
        about:blank >/dev/null 2>&1 &
    CHROME_PID="$!"
    if wait_for_cdp "$CDP_ENDPOINT"; then
        HEADLESS_CDP=1
        pass "固定 Edge 操作目录的无头 CDP 已启动: $CDP_ENDPOINT"
    else
        fail "Edge 无头 CDP 未能启动"
    fi
}


configure_cdp_provider() {
    run_bifrost agent research init \
        --preset personal-cn \
        --wechat-cdp-endpoint "$CDP_ENDPOINT" \
        --yes >/dev/null
}

restart_headless_cdp_after_manual() {
    if [[ -z "$CHROME_BIN" || ! -x "$CHROME_BIN" || -z "$CHROME_PROFILE" ]]; then
        return 1
    fi
    local port
    port="$(find_free_port)"
    CDP_ENDPOINT="http://127.0.0.1:${port}"
    "$CHROME_BIN" \
        --headless \
        --remote-debugging-address=127.0.0.1 \
        "--remote-debugging-port=${port}" \
        "--user-data-dir=${CHROME_PROFILE}" \
        --disable-gpu \
        --disable-dev-shm-usage \
        --no-first-run \
        --no-default-browser-check \
        about:blank >/dev/null 2>&1 &
    CHROME_PID="$!"
    if ! wait_for_cdp "$CDP_ENDPOINT"; then
        return 1
    fi
    HEADLESS_CDP=1
    configure_cdp_provider
}

open_manual_challenge_page() {
    local target_url="$1"
    local visible_bin visible_pid port old_pid opened_url
    old_pid="$CHROME_PID"
    if [[ "$HEADLESS_CDP" == "1" && "$EXTERNAL_CDP" != "1" ]]; then
        visible_bin="$CHROME_BIN"
        if [[ -z "$visible_bin" || ! -x "$visible_bin" ]]; then
            visible_bin="$(find_edge_browser)"
        fi
        if [[ -z "$visible_bin" || ! -x "$visible_bin" ]]; then
            fail "找不到可弹出的 Microsoft Edge 浏览器"
            restart_headless_cdp_after_manual || true
            return
        fi
        if [[ -n "$old_pid" ]]; then
            kill "$old_pid" >/dev/null 2>&1 || true
            wait "$old_pid" >/dev/null 2>&1 || true
            CHROME_PID=""
        fi
        port="$(find_free_port)"
        CDP_ENDPOINT="http://127.0.0.1:${port}"
        "$visible_bin" \
            --remote-debugging-address=127.0.0.1 \
            "--remote-debugging-port=${port}" \
            "--user-data-dir=${CHROME_PROFILE}" \
            --no-first-run \
            --no-default-browser-check \
            --new-window \
            "$target_url" >/dev/null 2>&1 &
        visible_pid="$!"
        CHROME_PID="$visible_pid"
        if ! wait_for_cdp "$CDP_ENDPOINT" >/dev/null 2>&1; then
            fail "可见浏览器已启动但 CDP 未就绪"
        fi
        open_cdp_target "$CDP_ENDPOINT" "$target_url" || fail "可见浏览器未能通过 CDP 打开问题链接"
    else
        open_cdp_target "$CDP_ENDPOINT" "$target_url" || fail "浏览器未能通过 CDP 打开问题链接"
    fi
    if wait_for_weixin_detail_target "$CDP_ENDPOINT" "$target_url"; then
        opened_url="$(cat "${TEST_DATA_DIR}/cdp-opened-url.txt" 2>/dev/null || true)"
        pass "Edge 可见浏览器已打开 Sogou/微信详情页: ${opened_url:-$target_url}"
    else
        opened_url="$(cat "${TEST_DATA_DIR}/cdp-opened-url.txt" 2>/dev/null || true)"
        fail "Edge 可见浏览器未打开 Sogou/微信详情页，当前页面: ${opened_url:-unknown}"
    fi
    echo ""
    info "已将问题链接弹出到 Microsoft Edge，请在浏览器中完成验证码/登录/授权后回到终端按 Enter 继续。"
    info "URL: $target_url"
    IFS= read -r _ || true
    if [[ "$HEADLESS_CDP" == "1" && "$EXTERNAL_CDP" != "1" ]]; then
        kill "$CHROME_PID" >/dev/null 2>&1 || true
        wait "$CHROME_PID" >/dev/null 2>&1 || true
        CHROME_PID=""
        restart_headless_cdp_after_manual || fail "人工处理后未能回到无头 Edge CDP"
    fi
}

test_init_cdp_provider() {
    header "初始化 Sogou WeChat CDP provider"
    local output
    output=$(run_bifrost agent research init --preset personal-cn --wechat-cdp-endpoint "$CDP_ENDPOINT" --yes)
    if grep -q "Research Pack initialized" <<<"$output" \
        && grep -q "sogou_wechat_cdp" "${TEST_DATA_DIR}/agent/agent_config.json"; then
        pass "research init 写入 sogou_wechat_cdp provider"
    else
        fail "research init 未写入 CDP provider: $output"
    fi
}

test_real_sogou_search() {
    header "通过 CDP 抓取 Sogou 微信公众号搜索结果"
    local output result_file
    result_file="${TEST_DATA_DIR}/sogou-search.json"
    output=$(run_bifrost agent research provider test sogou_wechat_cdp --query "$QUERY")
    printf '%s\n' "$output" > "$result_file"
    if python3 - "$result_file" <<'PY'
import json, sys
data=json.load(open(sys.argv[1]))
items=data.get("results") or []
assert len(items) > 0, "no search results"
first=items[0]
assert first.get("provider") == "sogou_wechat_cdp", first
assert first.get("source") == "wechat", first
assert first.get("title"), first
assert first.get("url", "").startswith("https://weixin.sogou.com/link"), first
print(first["url"])
PY
    then
        FIRST_URL="$(python3 - "$result_file" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["results"][0]["url"])
PY
)"
        pass "Sogou 真实搜索返回微信公众号结果: $FIRST_URL"
    else
        fail "Sogou 真实搜索未返回可用结果: $output"
    fi
}

test_real_detail_fetch() {
    header "通过 CDP 抓取 Sogou/微信公众号详情页"
    if [[ -z "${FIRST_URL:-}" ]]; then
        fail "缺少搜索结果 URL，无法抓取详情"
        return
    fi
    local output status detail_file
    detail_file="${TEST_DATA_DIR}/sogou-detail.json"
    if [[ "$MANUAL_AUTH" == "1" && "$FORCE_MANUAL_AUTH" == "1" && -z "${CI:-}" ]]; then
        open_manual_challenge_page "$FIRST_URL"
    fi
    set +e
    output=$(run_bifrost agent research fetch "$FIRST_URL" --max-bytes 500000)
    status=$?
    set -e
    printf '%s\n' "$output" > "$detail_file"
    if [[ "$status" -eq 0 ]] && python3 - "$detail_file" <<'PY'
import json, sys
data=json.load(open(sys.argv[1]))
content=data.get("content_markdown") or ""
assert len(content) > 120, "content too short"
assert data.get("url"), "missing final url"
assert data.get("title"), "missing title"
PY
    then
        pass "详情页抓取成功，正文长度大于 120 字符"
        return
    fi

    if grep -qiE "blocked|challenge|验证码|antispider" "$detail_file"; then
        if [[ "$MANUAL_AUTH" == "1" && -z "${CI:-}" ]]; then
            open_manual_challenge_page "$FIRST_URL"
            set +e
            output=$(run_bifrost agent research fetch "$FIRST_URL" --max-bytes 500000)
            status=$?
            set -e
            printf '%s\n' "$output" > "$detail_file"
            if [[ "$status" -eq 0 ]] && python3 - "$detail_file" <<'PY'
import json, sys
data=json.load(open(sys.argv[1]))
content=data.get("content_markdown") or ""
assert len(content) > 120, "content too short"
assert data.get("url"), "missing final url"
assert data.get("title"), "missing title"
PY
            then
                pass "人工验证后详情页抓取成功，正文长度大于 120 字符"
                return
            fi
        fi
        fail "详情页被站点挑战/验证码阻断；可设置 BIFROST_RESEARCH_MANUAL_AUTH=1 弹出浏览器人工验证后继续，或用 BIFROST_RESEARCH_CDP_ENDPOINT 指向已验证浏览器后重跑"
    else
        fail "详情页抓取失败: status=${status}, output=${output}"
    fi
}

setup
start_browser_if_needed
test_init_cdp_provider
test_real_sogou_search
test_real_detail_fetch

echo ""
echo "通过: $PASSED, 失败: $FAILED"
if [[ "$FAILED" -ne 0 ]]; then
    exit 1
fi
