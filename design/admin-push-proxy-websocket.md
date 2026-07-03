# 管理端 Push WebSocket 代理访问修复

## 背景

Bifrost Web UI 通过 `/_bifrost/api/push` WebSocket 接收实时事件（traffic 变更、断点触发、mobile connect、rule sync、breakpoint prompt、overview 概览刷新等）。这个通道由 `crates/bifrost-admin/src/push.rs::SharedPushManager` 驱动，前端在首屏建立后长时间保持。

Bifrost 的 HTTP 与代理入口共享同一 TCP 端口（例如 `127.0.0.1:9900`）。当用户浏览器把系统代理设置为 Bifrost 自身，或者用户显式通过 `http://bifrost.local/` 打开管理端时，`/_bifrost/api/push` WebSocket 请求会先落到 bifrost-proxy 的 `handle_http_websocket`，被识别为“普通上游 WebSocket”，然后 proxy 尝试通过代理链路回拨自身端口。这会导致：

- WebSocket 握手在 proxy 层被再包一层 Upgrade，AdminRouter 收不到真正的 upgrade 请求。
- push 通道反复断开重连，前端不断刷新 overview 面板。
- traffic 面板漏事件、断点勾选后无响应、mobile pairing 状态刷不出来。

修复思路是让代理层短路本机 Admin WebSocket 请求，直接把它交给 `AdminRouter::handle` 处理，不进入代理转发链路。

## 用户目标验证清单

### 必须实现

- 通过浏览器把系统代理指向 Bifrost 后打开管理端，`/_bifrost/api/push` 稳定连接，不出现反复重连。
- 通过 `http://bifrost.local/` 虚拟 Host 打开管理端，`/api/push` 也能自动重写为 `/_bifrost/api/push` 并稳定连接。
- 非 Admin WebSocket 请求（例如上游 `wss://example.com/ws`）仍走原代理转发路径，不被误短路到本机。
- 短路判定必须准确：只对 loopback host + 当前监听端口 + `/_bifrost/...` path 组合成立，误命中会把上游 WebSocket 变成 Admin 请求。
- 短路后 `SharedPushManager` 仍照常派发 `connected`、`overview_update`、`traffic_new`、`breakpoint_prompt` 等事件。

### 必须不破坏

- 普通 HTTP 代理转发不受影响。
- Admin HTTP（非 WebSocket）请求不因为短路逻辑走两次 AdminRouter。
- `bifrost.local` 虚拟 Host 的 HTTP GET/POST/CONNECT 语义保持。
- 现有 admin CORS / Origin / CSRF 校验仍然生效（`AdminRouter::handle` 内部照旧走 pipeline）。
- 通过 HTTP proxy 伪造 `/_bifrost/api/push` 的攻击场景不能借短路绕开 Origin/Host 校验（详见 `admin-cross-site-rule-share-security.md`）。

### 必须真实验证

- 用真实浏览器同时开系统代理 + 打开 Bifrost Web UI，DevTools Network 面板确认 `push` WebSocket `101 Switching Protocols` 且长时间保持。
- 断点、traffic、mobile pairing、overview 事件均实时到达 UI，无肉眼可见的丢包或延迟。
- `curl -x http://127.0.0.1:9900 --http1.1 --include -H 'Upgrade: websocket' -H 'Connection: Upgrade' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZQ==' -H 'Sec-WebSocket-Version: 13' http://127.0.0.1:9900/_bifrost/api/push` 返回 `101`。
- 上游 `wss://echo.websocket.events/` 通过代理仍能建立连接。

## 产品语义

### 短路判定

“本机管理端 WebSocket”定义为下述任一：

1. 目标 `Host` 是 `bifrost.local`（`ADMIN_VIRTUAL_HOST`），且方法非 `CONNECT`。
2. 请求 path 命中 `/_bifrost/...`，且目标端口等于当前监听端口 `ctx.port`，且 host 是 `localhost`、`127.0.0.1`、`::1` 三者之一。

只有满足以上任一，`should_route_websocket_to_local_admin` 才返回 true，直接 `AdminRouter::handle(req, admin_state)`。否则继续走原 WebSocket 代理转发路径。

### 虚拟 Host 路径重写

`bifrost.local` 虚拟 Host 场景下，浏览器发出的路径可能是 `/api/push`（不含 `_bifrost` 前缀），因为对用户来说 `bifrost.local` 就是 Admin 根。这里必须把路径重写为 `/_bifrost/api/push` 再进入 AdminRouter。`rewrite_local_admin_websocket_request(req, host)` 在 host == `bifrost.local` 时执行 path 补齐；`localhost`/`127.0.0.1` 场景下不需要重写。

### 与安全模型的关系

短路不绕过安全校验：

- 进入 AdminRouter 后仍会走 `is_valid_admin_request`、`check_api_auth`、Origin 校验。
- 浏览器伪造场景（Origin 是外部页面）会在 `websocket_origin_rejection` 或 `check_api_auth` 阶段被拒绝，返回 403 或断开。

## 技术细节

### 关键文件与函数

- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - `handle_http_websocket(req, rules, ctx, admin_state, push_manager, unsafe_ssl)`（约行 4158）
  - `should_route_websocket_to_local_admin(host: &str, port: u16, path: &str, self_port: u16) -> bool`（约行 4575）
  - `rewrite_local_admin_websocket_request<T>(req: Request<T>, host: &str) -> Request<T>`（约行 4593）
- `crates/bifrost-proxy/src/server.rs`
  - `ADMIN_VIRTUAL_HOST: &str = "bifrost.local"`
  - `is_admin_virtual_host_request<B>(req: &Request<B>) -> bool`
- `crates/bifrost-admin/src/lib.rs`
  - `AdminRouter::handle`
  - `SharedPushManager`
- `crates/bifrost-admin/src/push.rs`：push 通道核心，`SharedPushManager::subscribe`、`push_event`、`overview_snapshot`。
- `crates/bifrost-admin/src/query_service.rs`：为 push 提供 overview 数据。

### 短路判定伪代码

```rust
fn should_route_websocket_to_local_admin(host: &str, port: u16, path: &str, self_port: u16) -> bool {
    if host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
        return true;
    }
    if !path.starts_with("/_bifrost/") {
        return false;
    }
    if port != self_port {
        return false;
    }
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}
```

### `handle_http_websocket` 入口

```rust
async fn handle_http_websocket(req, rules, ctx, admin_state, push_manager, unsafe_ssl) -> Result<Response> {
    let host = extract_host(&req);
    let port = extract_port(&req, ctx.default_port);
    let uri = req.uri().clone();

    if should_route_websocket_to_local_admin(&host, port, uri.path(), ctx.port) {
        let req = rewrite_local_admin_websocket_request(req, &host);
        return AdminRouter::handle(req, admin_state, push_manager).await;
    }
    // ... 原有代理转发链路
}
```

## CLI 交互

无 CLI 变化。开发者调试可用：

- `websocat 'ws://127.0.0.1:9900/_bifrost/api/push'` 直连 push。
- `websocat --proxy 'http://127.0.0.1:9900' 'ws://127.0.0.1:9900/_bifrost/api/push'` 模拟走代理场景。
- `bifrost status --format json` 输出 push 连接数 / 最近事件（可选，若已实装）。

## Web UI 交互

- 首屏 `PushClient` 建立 `new WebSocket('/_bifrost/api/push?need_overview=true')`。
- 断线后 backoff 重连；本次修复消除“反复重连”症状。
- Overview / Traffic / Breakpoint 面板都订阅 push 事件；连接掉线时 UI 显示灰色气泡“realtime disconnected”。
- 前端不感知短路：路径永远是 `/_bifrost/api/push`，`?need_overview=true` 让服务端首帧回发一次 overview 快照。

## Admin API

不变。相关端点：

- `GET /_bifrost/api/push?need_overview=<bool>` — WebSocket upgrade。首帧 `connected` + 可选 `overview_update`；后续持续推送。
- 事件类型（`crates/bifrost-admin/src/push.rs` 中枚举）：`connected`、`overview_update`、`traffic_new`、`traffic_update`、`breakpoint_prompt`、`rule_change`、`mobile_pair`、`sync_status` 等。

## Sync / 导入导出 / 分享边界

不涉及。

## 实现切分

### Phase 1：短路函数与重写函数

- 在 `handle_http_websocket` 之前实现 `should_route_websocket_to_local_admin` 与 `rewrite_local_admin_websocket_request`，并加纯函数单元测试。

### Phase 2：接入 `handle_http_websocket`

- 在 WebSocket upgrade 分支的开头调用短路判定，命中即转交 AdminRouter。
- 保留原有代理转发路径作为默认分支。

### Phase 3：真实回归

- 扩展 E2E 与 human_tests。
- 更新 `human_tests/readme.md` 索引。

### Phase 4：文档与安全边界

- 明确本次改动不放宽 Origin/CSRF 校验，链接到 `admin-cross-site-rule-share-security.md`。
- 确认 `bifrost.local` 虚拟 Host 场景下 path 重写行为。

## 测试方案

### 单元测试

- `test_should_route_websocket_to_local_admin_for_loopback_admin_path`：`("127.0.0.1", 9900, "/_bifrost/api/push", 9900) => true`；同结构 `localhost` 也为 true。
- `test_should_not_route_websocket_to_local_admin_for_non_admin_path_or_port`：非 `_bifrost` path、非 self port、非 loopback host 各自 false。
- `test_rewrite_local_admin_websocket_request_rewrites_virtual_host_path`：`Host: bifrost.local` + path `/api/push?need_overview=true` 被重写为 `/_bifrost/api/push?need_overview=true`；`Host: 127.0.0.1` 保持原 path。
- `test_should_route_websocket_to_local_admin_matches_virtual_host_regardless_of_port`：`bifrost.local` 命中无关端口。
- 已有 `router::tests` 中 push 相关 auth 用例保持通过，覆盖短路后仍走 AdminRouter 校验。

### E2E 测试

- 扩展 `e2e-tests/tests/test_traffic_push_e2e.sh`：
  - 场景 A：直连 push，断言 `connected` + `overview_update`。
  - 场景 B：通过 HTTP 代理访问同一端口 push，断言 `connected` + `overview_update`，不出现 4 秒内多次重连。
  - 场景 C：通过 HTTP 代理伪造 push（不同 Origin）→ 断开或 403。
- 关联脚本 `e2e-tests/tests/test_admin_virtual_host_proxy.sh` 中新增 `bifrost.local` push 路径重写用例。

### 真实场景测试 human_tests

- 更新 `human_tests/api-push.md`：
  - TC-APW-01：浏览器不开系统代理，`push` 稳定。
  - TC-APW-02：浏览器把系统代理指向 Bifrost 自己，`push` 稳定。
  - TC-APW-03：通过 `http://bifrost.local/` 打开管理端，`push` 稳定。
  - TC-APW-04：断点触发时 UI 弹窗延迟 < 500ms。
  - TC-APW-05：mobile pairing 二维码扫码后状态实时刷新。
- 更新 `human_tests/readme.md` 索引条目。

### 覆盖率与项目校验

- `cargo test -p bifrost-proxy admin_virtual_host_ws -- --nocapture`
- `cargo test -p bifrost-proxy should_route_websocket_to_local_admin`
- `cargo test -p bifrost-proxy rewrite_local_admin_websocket_request`
- `cargo test -p bifrost-proxy --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 目标 E2E 脚本 `BIFROST_BIN=... e2e-tests/tests/test_traffic_push_e2e.sh`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核短路判定的 host/port/path 三条件是否严格，防止误短路上游 WebSocket。
- 复核 `bifrost.local` 场景下 path 重写只在 `handle_http_websocket` 一处进行，避免与 `rewrite_virtual_host_request` 重复重写导致双前缀 `/_bifrost/_bifrost/api/push`。
- 复核 push 通道事件类型未减少，overview 快照仍下发。
- 复测：受影响单测 + E2E。

### 第 2 轮

- 复核 admin cross-site security guardrail 未被短路绕过（Origin 拒绝、伪造 push 拒绝）。
- 复测浏览器真实场景（TC-APW-02、TC-APW-03）。
- 全量 `cargo test --workspace --all-features`。

## 风险与决策点

- **判定误命中上游 WebSocket**：如果上游服务恰好在 `127.0.0.1:9900/_bifrost/...`（例如用户自建 Admin mock 端），会被误短路。生产 Bifrost 用户很难同时把 `9900` 又跑上游服务，风险可接受；作为安全兜底，短路后仍走 AdminRouter，若 path 不存在会返回 404，不会造成数据泄漏。
- **虚拟 Host path 重写与 `rewrite_virtual_host_request` 一致性**：HTTP 与 WebSocket 两条重写路径必须只做一次前缀补齐。单测覆盖 `bifrost.local` 已经带 `/_bifrost/...` path 时不重复补齐。
- **端口漂移**：Bifrost 支持多监听端口（temporary port）。短路判定必须使用当前监听端口 `ctx.port`，而不是全局 default port。否则 temporary port 上的 push 请求会被判为“非 self”而走代理链路。
- **CONNECT 隧道**：CONNECT 请求不会进入 `handle_http_websocket`，短路逻辑与 HTTPS CONNECT 隧道解耦；HTTPS 场景下 admin push 走 CONNECT + intercept + `rewrite_intercepted_virtual_host_request` 后再进入 AdminRouter，与本方案独立。
- **前端 backoff 语义**：修复后若仍见到反复重连，多半是后端 push 主动踢或 keepalive 超时，需要进一步检查 `SharedPushManager` heartbeat 而非本方案。
