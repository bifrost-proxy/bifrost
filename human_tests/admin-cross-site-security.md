# Admin Cross-Site Security

## 功能模块说明

验证 Bifrost Admin API 和 Rule Share 在真实代理服务下不会被未知网页或代理流量伪造写操作。覆盖规则创建、启用/停用、修改、Rule Share 确认应用，以及 Bifrost 作为代理时收到 absolute-form Admin API 请求或 Admin Push WebSocket 伪造请求的拒绝行为；同时验证注入到被代理页面内的 token 化 bridge 入口仍可正常工作。

## 前置条件

1. 在仓库根目录构建当前二进制：
   ```bash
   cargo build --bin bifrost
   ```
2. 使用临时数据目录启动测试服务，必须禁用系统代理、托盘和 Sync 自动登录弹窗：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 \
   BIFROST_DATA_DIR="$(mktemp -d)" target/debug/bifrost start \
     -p <free-port> --host 127.0.0.1 --access-mode allow_all \
     --skip-cert-check --no-system-proxy --no-intercept -y
   ```
3. 推荐直接执行自动化真实场景脚本：
   ```bash
   e2e-tests/tests/test_admin_cross_site_security.sh
   ```

## 测试用例列表

### TC-ACS-01 跨站 Origin 伪造规则创建被拒绝

操作步骤：
1. 请求 `GET http://127.0.0.1:<port>/_bifrost/api/security/csrf` 获取 CSRF token。
2. 使用 `curl` 对 `POST /_bifrost/api/rules` 发送规则创建请求，带上：
   - `Origin: http://evil.example`
   - `Sec-Fetch-Site: cross-site`
   - `X-Bifrost-CSRF: <token>`
3. 查询规则列表。

预期结果：
- 写请求返回 HTTP 403。
- 规则列表中不存在攻击请求里的规则名。

### TC-ACS-02 同源浏览器写请求缺少 CSRF 被拒绝

操作步骤：
1. 预先创建一条 `safe-rule`。
2. 使用 `curl -X PUT /_bifrost/api/rules/safe-rule/enable`，带上：
   - `Origin: http://127.0.0.1:<port>`
   - `Sec-Fetch-Site: same-origin`
   - 不带 `X-Bifrost-CSRF`

预期结果：
- 请求返回 HTTP 403。
- `safe-rule` 状态不因该请求改变。

### TC-ACS-03 同源浏览器写请求携带 CSRF 后允许

操作步骤：
1. 请求 `GET /_bifrost/api/security/csrf` 获取 CSRF token。
2. 使用 `curl -X PUT /_bifrost/api/rules/safe-rule/enable`，带上：
   - `Origin: http://127.0.0.1:<port>`
   - `Sec-Fetch-Site: same-origin`
   - `X-Bifrost-CSRF: <token>`
3. 查询规则列表。

预期结果：
- 请求返回 HTTP 200。
- `safe-rule` 状态变为 enabled。

### TC-ACS-04 DNS rebinding Host 伪造被拒绝

操作步骤：
1. 使用 loopback 地址访问 `PUT http://127.0.0.1:<port>/_bifrost/api/rules/safe-rule/disable`。
2. 显式设置 `Host: evil.example:<port>`。

预期结果：
- 请求返回 HTTP 403。
- 请求不会进入 Admin API 规则写逻辑。

### TC-ACS-05 通过 Bifrost 代理发送 absolute-form Admin API 被拒绝

操作步骤：
1. 使用 `curl -x http://127.0.0.1:<port>`。
2. 目标 URL 使用 absolute-form：
   `http://127.0.0.1:<port>/_bifrost/api/rules/safe-rule/disable`。

预期结果：
- 请求返回 HTTP 403。
- 该请求被视为代理伪造 Admin API，不会修改规则。

### TC-ACS-06 Rule Share 访问只进入确认页且未确认不落库

操作步骤：
1. 使用 `bifrost rule share shared-security http://example.com/app --content "shared-security.test bp://127.0.0.1:3000"` 生成分享链接。
2. 通过 `curl -x http://127.0.0.1:<port> <share-url>` 访问分享链接。
3. 查询规则列表。

预期结果：
- 代理响应 HTTP 302。
- `Location` 指向 `http://127.0.0.1:<port>/_bifrost/share/rule?...`。
- 规则列表中不存在 `share/shared-security`。

### TC-ACS-07 Rule Share 确认 API 缺 CSRF 被拒绝

操作步骤：
1. 从 TC-ACS-06 的确认页 URL 提取 `payload` 和 `target`。
2. 使用 `POST /_bifrost/api/rules/share-confirm`，带上：
   - `Origin: http://127.0.0.1:<port>`
   - `Sec-Fetch-Site: same-origin`
   - 不带 `X-Bifrost-CSRF`

预期结果：
- 请求返回 HTTP 403。
- 规则列表中仍不存在 `share/shared-security`。

### TC-ACS-08 Rule Share 用户确认后导入并跳回 clean URL

操作步骤：
1. 使用 TC-ACS-06 的确认页 URL 提取 `payload` 和 `target`。
2. 使用 `POST /_bifrost/api/rules/share-confirm`，带上：
   - `Origin: http://127.0.0.1:<port>`
   - `Sec-Fetch-Site: same-origin`
   - `X-Bifrost-CSRF: <token>`
3. 查询规则列表。

预期结果：
- 请求返回 HTTP 200。
- 响应里的 `redirect_url` 等于 clean target URL。
- 规则列表出现 `share/shared-security [enabled]`。

### TC-ACS-09 通过 Bifrost 代理伪造 Admin Push WebSocket 被拒绝

操作步骤：
1. 启动 Bifrost 后，使用 `e2e-tests/test_utils/ws_via_http_proxy.js` 通过同一个 Bifrost HTTP proxy 连接：
   `ws://127.0.0.1:<port>/_bifrost/api/push?x_client_id=<id>&need_overview=true`。
2. 观察 WebSocket 握手结果。
3. 继续通过本地直连方式连接 `/_bifrost/api/push`，验证正常本地 WebUI push 仍可用。

预期结果：
- 经 HTTP proxy 伪造的 Admin Push WebSocket 返回 HTTP 403。
- 本地直连 WebSocket 仍可建立连接并接收 `connected` / push 消息。
- 代理伪造请求不会绕过 Admin Host / CSRF / 浏览器写入防护边界。

### TC-ACS-10 被代理页面内的 DevTools page bridge 可连接

操作步骤：
1. 启动 Bifrost 后，通过 Admin API 创建启用的 `devtools://` 规则，匹配测试页面域名。
2. 使用浏览器通过 Bifrost 代理打开该测试页面。
3. 等待页面内注入的 `window.__BIFROST_DEVTOOLS_BRIDGE__` 状态变成 `connected`。
4. 通过 WebUI/DevTools API 执行页面 bridge 会话命令，验证 DOM、Network、Storage、Console 等能力可用。
5. 同时保留 TC-ACS-09，确认通用 `/_bifrost/api/push` 不能通过 HTTP proxy 伪造连接。

预期结果：
- DevTools page bridge 的专属 `/_bifrost/api/devtools/bridge/<page_id>/ws` 入口允许被代理页面连接。
- bridge 连接仍依赖页面专属 token，不能扩展为普通 Admin API 或 Admin Push 的跨站访问。
- `/_bifrost/api/push` 经 HTTP proxy 伪造访问仍返回 HTTP 403。

### TC-ACS-11 全部 Admin WebSocket 端点的 CSWSH 跨站劫持被拒绝

操作步骤：
1. 启动 Bifrost（`allow_all` 模式，监听 127.0.0.1:<port>）。
2. 对以下每个 WebSocket 端点分别发起原始 WebSocket 升级请求（携带 `Upgrade: websocket`、`Connection: Upgrade`、`Sec-WebSocket-Version: 13`、`Sec-WebSocket-Key`）：
   - `/_bifrost/api/push`
   - `/_bifrost/api/replay/execute/ws`
   - `/_bifrost/api/devtools/sessions/<id>/ws`
   - `/_bifrost/api/devtools/bridge/<page_id>/ws`
   - `/_bifrost/api/voice/listen-ws`
   - `/_bifrost/api/asr/transcribe-ws`
3. 每个端点依次发送四类请求并记录 HTTP 状态：
   - (a) 跨站：`Origin: http://evil.example.com` + `Sec-Fetch-Site: cross-site`。
   - (b) 跨源无 Sec-Fetch：`Origin: http://attacker.example.com`。
   - (c) 原生客户端：不带任何 `Origin` / `Referer` / `Sec-Fetch-*` 头。
   - (d) 同源：`Origin: http://127.0.0.1:<port>` + `Sec-Fetch-Site: same-origin`。

预期结果：
- (a) 与 (b) 全部返回 HTTP 403（CSWSH 守卫拦截）。
- (c) 与 (d) 不返回 403（守卫放行，握手进入正常协议升级或后续鉴权流程）。
- WebSocket 升级（GET）不再绕过跨站写入防护边界。
## 清理步骤

1. 终止测试启动的 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`。

## 执行记录

- 2026-06-23：已执行 `e2e-tests/tests/test_admin_cross_site_security.sh`，TC-ACS-01 到 TC-ACS-08 全部通过。
- 2026-06-23：已执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 SKIP_BUILD=true bash e2e-tests/tests/test_traffic_push_e2e.sh`，其中 `WebSocket Admin Push Via HTTP Proxy Rejected` 断言经代理连接 `/_bifrost/api/push` 返回 `Unexpected server response: 403`，本地直连 WebSocket、traffic delta、overview、metrics、channel limit、polling fallback、pending ids 和 incremental sequence 均通过。
- 2026-06-23：已执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`，验证 DevTools page bridge 可在被代理页面内通过专属 bridge WebSocket 连接并完成页面调试能力回归。
- 2026-06-24：已执行 `cargo test -p bifrost-admin cors:: cswsh`（cors WS 守卫 18 项 + replay CSWSH 接线 4 项全部通过）以及 `bash e2e-tests/tests/test_admin_cross_site_security.sh`（含新增 TC-ACS-11，对 `/_bifrost/api/push` 与 `/_bifrost/api/replay/execute/ws` 的跨站/跨源 WebSocket 升级返回 403，原生客户端与同源放行），覆盖 P0 CSWSH 修复。
