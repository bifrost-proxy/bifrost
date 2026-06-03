# statusCode 直接响应真实场景测试

## 功能模块说明

验证 `statusCode://code` 命中后直接返回指定 HTTP 状态码，不向后端服务器发送请求；同时验证 `replaceStatus://code` 仍会请求后端，只替换响应状态码。

## 前置条件

1. 在仓库根目录执行。
2. 使用临时数据目录，禁止污染正式配置。
3. 启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 并携带 `--no-system-proxy`。

## 测试用例列表

### TC-SCDR-01：statusCode + host 直接返回且不请求 upstream

**操作步骤**：

1. 先构建当前代码对应的 Bifrost 二进制：
   ```bash
   cargo build --bin bifrost
   ```
2. 执行真实 CLI/代理场景脚本：
   ```bash
   DATA_DIR="$(mktemp -d)"
   MOCK_COUNT="$DATA_DIR/mock-count"
   echo 0 > "$MOCK_COUNT"
   MAIN_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
   TEMP_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
   MOCK_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
   node -e 'const http=require("http"),fs=require("fs"); const [port,countFile]=process.argv.slice(1); http.createServer((req,res)=>{const n=(Number(fs.readFileSync(countFile,"utf8"))||0)+1; fs.writeFileSync(countFile,String(n)); res.writeHead(200,{"Content-Type":"text/plain"}); res.end("should_not_reach");}).listen(Number(port),"127.0.0.1");' "$MOCK_PORT" "$MOCK_COUNT" &
   MOCK_PID="$!"
   BIFROST_DATA_DIR="$DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 ./target/debug/bifrost start -p "$MAIN_PORT" --unsafe-ssl --no-system-proxy --skip-cert-check > "$DATA_DIR/bifrost.log" 2>&1 &
   BIFROST_PID="$!"
   trap 'kill "$BIFROST_PID" "$MOCK_PID" 2>/dev/null || true; rm -rf "$DATA_DIR"' EXIT
   until curl -sS "http://127.0.0.1:$MAIN_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; do sleep 0.2; done
   BIFROST_DATA_DIR="$DATA_DIR" ./target/debug/bifrost port bind --port "$TEMP_PORT" --rule-text "test.local host://127.0.0.1:$MOCK_PORT statusCode://451 resBody://(blocked)"
   HTTP_CODE="$(curl -sS -x "http://127.0.0.1:$TEMP_PORT" -o "$DATA_DIR/body" -w "%{http_code}" "http://test.local/api")"
   BODY="$(cat "$DATA_DIR/body")"
   COUNT="$(cat "$MOCK_COUNT")"
   test "$HTTP_CODE" = "451"
   test "$BODY" = "blocked"
   test "$COUNT" = "0"
   ```

**预期结果**：

- 客户端收到 HTTP `451`。
- 响应 Body 包含 `blocked`。
- mock upstream 请求计数为 `0`，证明 `statusCode` 命中后未向后端发送请求。

### TC-SCDR-02：replaceStatus 仍请求 upstream

**操作步骤**：

1. 执行 E2E 对照场景命令：
   ```bash
   cargo run -p bifrost-e2e -- --test status_replaceStatus_200
   ```

**预期结果**：

- 客户端收到 HTTP `200`。
- 响应 Body 包含后端返回的 `server error`。
- 该结果证明 `replaceStatus` 是请求后端后的响应状态码替换，不等同于 `statusCode` 的直接响应。

## 清理步骤

1. TC-SCDR-01 的 `trap` 会清理本次脚本启动的 Bifrost 进程、mock server 和临时数据目录。
2. TC-SCDR-02 的 E2E runner 会清理本次启动的 Bifrost 进程与 mock server。

## 执行记录

### 2026-06-03

- 已执行 `SKIP_FRONTEND_BUILD=1 CARGO_TARGET_DIR=/tmp/bifrost-push-verify-target cargo test -p bifrost-proxy test_status_code_with_host_generates_direct_response -- --nocapture`，结果 1 passed。
- 已执行 `SKIP_FRONTEND_BUILD=1 CARGO_TARGET_DIR=/tmp/bifrost-push-verify-target cargo run -p bifrost-e2e -- --test status_statusCode_direct_no_upstream`，结果 1 passed。
- 已执行 `SKIP_FRONTEND_BUILD=1 CARGO_TARGET_DIR=/tmp/bifrost-push-verify-target cargo run -p bifrost-e2e -- --test status_replaceStatus_200`，结果 1 passed。
- CI `E2E Rules (Linux)` 首轮暴露 `combination/multi_rules.txt` 仍把完整链路用例写成 `statusCode://202`，已改为 `replaceStatus://202` 以匹配“请求后端后替换状态码”的断言目标。
- 已执行 `BIFROST_E2E_PROXY_READY_TIMEOUT=120 SKIP_FRONTEND_BUILD=1 ... bash e2e-tests/test_rules.sh --use-binary -p <port> e2e-tests/rules/combination/multi_rules.txt`，结果 19 passed。
- 已执行 `SKIP_FRONTEND_BUILD=1 BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_websocket_frames.sh`，结果 6 passed，确认 WebSocket header rule fixture 仍由专门脚本覆盖。
