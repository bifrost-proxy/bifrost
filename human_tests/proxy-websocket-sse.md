# WebSocket 与 SSE 代理功能测试用例

## 功能模块说明

验证 Bifrost 对 WebSocket（ws://）和 Server-Sent Events（SSE）流量的代理转发能力，包括基本代理转发、帧/事件捕获，以及管理端 UI 中的消息面板展示。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录，启用 TLS 拦截以便捕获帧数据）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept
   ```
2. 确保端口 8800 未被占用
3. 需要一个可用的 WebSocket 测试服务（如 `wss://echo.websocket.events` 或本地搭建的 WebSocket 服务器）
4. 需要一个可用的 SSE 测试端点（如 `httpbin.org/sse` 或本地搭建的 SSE 服务器）
5. 部分 UI 相关测试需要通过浏览器（Chrome DevTools MCP）进行操作

---

## 测试用例

### TC-PWS-01：WebSocket 代理转发（ws:// 通过代理）

**操作步骤**：
1. 使用 `websocat`（或类似 WebSocket 客户端工具）通过代理连接 WebSocket 服务器：
   ```bash
   # 方法一：使用 curl 验证 WebSocket 升级握手
   curl -x http://127.0.0.1:8800 \
     -H "Connection: Upgrade" \
     -H "Upgrade: websocket" \
     -H "Sec-WebSocket-Version: 13" \
     -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
     -sD - http://echo.websocket.events/
   ```
   ```bash
   # 方法二：使用 websocat 通过 HTTP 代理
   websocat --ws-c-uri=ws://echo.websocket.events/ - ws-c:tcp:127.0.0.1:8800
   ```

**预期结果**：
- 方法一：收到 `101 Switching Protocols` 响应，包含 `Upgrade: websocket` 头
- 方法二：WebSocket 连接建立成功，发送消息后收到回显
- 代理正确处理了 HTTP Upgrade 请求并建立 WebSocket 隧道

---

### TC-PWS-02：WebSocket 帧捕获

**前置条件**：服务已启用 `--intercept` 模式

**操作步骤**：
1. 通过代理建立 WebSocket 连接到 `ws://echo.websocket.events/`
2. 发送文本消息：`hello bifrost`
3. 等待收到回显消息
4. 在浏览器中打开管理端 `http://127.0.0.1:8800/_bifrost/traffic`

**预期结果**：
- Traffic 页面中可以看到该 WebSocket 连接的记录
- 连接类型显示为 WebSocket
- 可以查看到 WebSocket 帧数据（包含发送和接收的消息内容）

---

### TC-PWS-03：SSE 代理转发（Server-Sent Events 通过代理）

**操作步骤**：
1. 执行命令，通过代理请求 SSE 端点：
   ```bash
   curl -x http://127.0.0.1:8800 -N http://httpbin.org/events/5
   ```
   > 注意：如果 httpbin.org 不支持 SSE 端点，可使用其他公共 SSE 测试服务，或在本地启动一个简单的 SSE 服务器：
   > ```bash
   > python3 -c "
   > from http.server import HTTPServer, BaseHTTPRequestHandler
   > import time
   > class SSEHandler(BaseHTTPRequestHandler):
   >     def do_GET(self):
   >         self.send_response(200)
   >         self.send_header('Content-Type', 'text/event-stream')
   >         self.send_header('Cache-Control', 'no-cache')
   >         self.end_headers()
   >         for i in range(5):
   >             self.wfile.write(f'data: event {i}\n\n'.encode())
   >             self.wfile.flush()
   >             time.sleep(1)
   > HTTPServer(('127.0.0.1', 3999), SSEHandler).serve_forever()
   > " &
   > ```
   > 然后通过代理请求本地 SSE 服务：
   > ```bash
   > curl -x http://127.0.0.1:8800 -N http://127.0.0.1:3999/
   > ```

**预期结果**：
- 客户端实时接收到 SSE 事件流
- 每个事件格式为 `data: event N`，依次输出
- 代理正确处理了流式响应，未缓冲整个响应后再返回

---

### TC-PWS-04：SSE 事件捕获

**前置条件**：服务已启用 `--intercept` 模式；本地已启动 SSE 测试服务（参考 TC-PWS-03）

**操作步骤**：
1. 通过代理请求 SSE 端点：
   ```bash
   curl -x http://127.0.0.1:8800 -N http://127.0.0.1:3999/
   ```
2. 等待接收完所有事件
3. 在浏览器中打开管理端 `http://127.0.0.1:8800/_bifrost/traffic`

**预期结果**：
- Traffic 页面中可以看到该 SSE 请求的记录
- 响应类型标识为 SSE（`text/event-stream`）
- 可以查看到捕获的 SSE 事件数据

---

### TC-PWS-05：管理端 UI WebSocket 消息面板

**前置条件**：已通过 TC-PWS-02 建立过 WebSocket 连接并发送消息

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 在 Traffic 列表中找到 WebSocket 类型的请求记录
3. 点击该记录查看详情
4. 切换到 Messages/Frames 面板

**预期结果**：
- 详情页中有 Messages（或 Frames）面板/Tab
- 面板中展示了 WebSocket 帧列表
- 每个帧显示方向（发送/接收）、内容、时间戳等信息
- 发送的 `hello bifrost` 消息和回显消息均可见

---

### TC-PWS-06：管理端 UI SSE 消息面板

**前置条件**：已通过 TC-PWS-04 请求过 SSE 端点

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 在 Traffic 列表中找到 SSE 类型（`text/event-stream`）的请求记录
3. 点击该记录查看详情
4. 切换到 Messages/Events 面板

**预期结果**：
- 详情页中有 Messages（或 Events）面板/Tab
- 面板中展示了 SSE 事件列表
- 每个事件显示事件数据（如 `event 0`、`event 1` 等）和时间戳
- 事件按接收顺序排列

---

### TC-PWS-07：Replay WebSocket E2E 启动隔离与诊断回归

**前置条件**：
- 已执行 `bifrost status`，确认正式代理端口为 `9900`，本用例不得使用该端口作为测试代理端口。
- 已存在可执行的 `target/release/bifrost` 或先执行 `cargo build --release --bin bifrost`。

**操作步骤**：
1. 执行以下命令，使用隔离端口和临时数据目录运行 replay WebSocket shell E2E：
   ```bash
   env HTTP_PROXY=http://127.0.0.1:9900 \
     HTTPS_PROXY=http://127.0.0.1:9900 \
     NO_PROXY=127.0.0.1,localhost \
     PROXY_HOST=127.0.0.1 \
     PROXY_PORT=18881 \
     ADMIN_HOST=127.0.0.1 \
     ADMIN_PORT=18881 \
     WS_HOST=127.0.0.1 \
     WS_PORT=18882 \
     WSS_PORT=18883 \
     BIFROST_BIN=target/release/bifrost \
     bash e2e-tests/tests/test_replay_websocket_frames.sh
   ```
2. 检查脚本输出中包含 replay WS/WSS mock 服务启动端口、Bifrost 测试代理端口和临时数据目录。
3. 检查脚本最终结果行。

**预期结果**：
- 脚本不使用 `9900` 作为测试代理端口，启动命令包含 `--no-system-proxy`。
- 如果 mock 端口被占用，脚本会打印端口重分配信息，并继续使用新的空闲端口生成 replay URL。
- 如果 WS/WSS mock 或 Bifrost 启动失败，脚本会输出对应日志 tail，而不是静默退出为 unknown failure。
- 正常环境下最终输出 `Replay WebSocket E2E Results: PASSED=9 FAILED=0`。

**本轮执行记录（2026-04-27）**：
- 先执行 `bifrost status`，确认正式代理运行在 `0.0.0.0:9900`；本用例使用测试代理端口 `18881`，未修改系统代理。
- 执行上述命令后，脚本打印 `Starting replay WS server on 127.0.0.1:18882`、`Starting replay WSS server on 127.0.0.1:18883`、`Starting Bifrost replay test proxy on 127.0.0.1:18881`。
- Replay WebSocket echo、frames capture、ping/pong、long connection、permessage-deflate fragmentation、invalid control frame、oversize payload length、wss upstream、subprotocol negotiation 共 9 项均通过。
- 最终结果为 `Replay WebSocket E2E Results: PASSED=9 FAILED=0`。

---

## 清理

测试完成后清理临时数据和本地测试服务：
```bash
# 停止本地 SSE 测试服务（如果启动了）
kill %1 2>/dev/null

# 清理临时数据
rm -rf .bifrost-test
```
