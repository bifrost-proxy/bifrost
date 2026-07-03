# Admin Cross-Site And Rule Share Security

## 背景

Bifrost 同时暴露本机 Admin API（`/_bifrost/api/...`、`/_bifrost/api/push` WebSocket、`/_bifrost/share/rule` 确认页等）和 HTTP/HTTPS 代理入口（默认 `127.0.0.1:9900`）。这两条链路共享同一个 TCP 监听器，浏览器有多种方式尝试跨越信任边界：

- 未知网页通过 `fetch('http://127.0.0.1:<port>/_bifrost/api/rules/foo')` 直接读写规则。
- DNS rebinding：`evil.example` 先解析到公网 IP 建立连接，再解析到 `127.0.0.1`，`Host: evil.example` 但 peer 是 loopback，让服务器误以为“同源”。
- 通过 Bifrost 代理向自身发 absolute-form 请求（`GET http://127.0.0.1:9900/_bifrost/api/... HTTP/1.1`），把代理入口伪装成 Admin API。
- 通过 Bifrost 代理向 `/_bifrost/api/push` 打 WebSocket，把 Admin Push 通道伪装成上游 WebSocket。
- 在被代理页面 URL 中植入 `__bifrost_rule=<payload>` query，让代理静默启用/替换规则。

这些路径必须在 server、AdminRouter、bifrost-proxy 三层同时封堵，前端 CSP 和用户教育都只是纵深防御的一层，不能作为唯一保护。

## 用户目标验证清单

### 必须实现

- Admin API 所有 unsafe 方法（POST/PUT/PATCH/DELETE）在浏览器上下文中必须先过 Origin / Sec-Fetch-Site / CSRF 校验，缺任一即拒绝。
- Loopback peer 也必须校验 `Host` header 是本机白名单值（`localhost`、`127.0.0.1`、`::1`、`bifrost.local`、`<self_port>` 变体），阻止 DNS rebinding。
- absolute-form 请求（`GET http://... HTTP/1.1`）无论目标是不是本机端口，都视为代理伪造入口，拒绝进入 Admin API。
- `__bifrost_rule` payload 在代理入口只能重定向到本机确认页 `/_bifrost/share/rule`，不允许静默调用 `import_rule_share_payload`。
- 确认页上的 Apply Rule 使用同源 `POST /_bifrost/api/rules/share-confirm`，走完整 CSRF 校验后落地。
- CLI、curl 等非浏览器上下文（无 `Sec-Fetch-Site`、无 `Origin`）保留原有 Admin API 能力，只需通过 server 层 Host / absolute-form 校验。

### 必须不破坏

- `/_bifrost/public/cert`、`/_bifrost/public/cert/qrcode`、`/_bifrost/public/proxy/qrcode` 三条公开资源继续跨域可读，OPTIONS 预检 204。
- `POST /_bifrost/api/rules/share-env/exit` 保留窄例外，允许被代理业务页面跨站调用（用于结束 Share Env 模式），但仍要求 body token。
- `/_bifrost/api/devtools/bridge/<page_id>/...` 保留 token 化的 DevTools bridge 例外，供被代理页面在 `devtools://` 规则命中时接入 Chrome DevTools。
- 主端口普通代理转发不受影响，`http://bifrost.local/` 虚拟 Host 请求仍能落到 Admin UI。
- Admin Push WebSocket 直连（浏览器打开 `http://127.0.0.1:9900/_bifrost/...` 页面后建立的 `/_bifrost/api/push`）继续正常工作。

### 必须真实验证

- 用真实 Bifrost 服务和 `curl` 手动模拟 DNS rebinding、absolute-form、跨站 POST、缺 CSRF、有 CSRF 四种情况。
- 通过 HTTP 代理向 `/_bifrost/api/push` 打 WebSocket 必须返回 403 或断开，直连 `/_bifrost/api/push` 仍可正常升级。
- 在被代理页面 URL 中植入 `__bifrost_rule=<payload>`，浏览器落地必须是确认页，不能是隐式导入。
- `bifrost rule ...` CLI 与 curl 在同一台机器上继续可用，不因 Origin/CSRF 校验被误拦。

## 产品语义

Bifrost Admin API 的信任模型分三层：

1. **Server 层（`crates/bifrost-admin/src/security.rs`）**：粗粒度拦截。所有走进 `/_bifrost/...` 的请求都必须先通过 `is_valid_admin_request(req, peer_addr, config, remote_access_enabled)`。校验点：peer 是否 loopback；`Host` 是否白名单；URI 是否 absolute-form；`Sec-Fetch-Site` 是否 same-origin/same-site/none 或非浏览器空值。
2. **AdminRouter 层（`crates/bifrost-admin/src/router.rs`）**：细粒度拦截 unsafe method。通过 `check_browser_write_guard` 判断是否浏览器上下文，如是则校验 `Origin` 与 `Host` 同源，并要求 `X-Bifrost-CSRF: <token>` 匹配 `state.csrf_token()`。CSRF token 通过 `/_bifrost/api/csrf` 首屏发放，前端 API wrapper 自动附加。
3. **bifrost-proxy 层（`crates/bifrost-proxy/src/proxy/http/handler.rs`）**：代理入口的 rule share 与 admin websocket 短路。`__bifrost_rule` 只生成确认页 URL；本机 `/_bifrost/api/push` WebSocket 转交 AdminRouter 而非代理转发。

### 窄例外白名单

只有两条路径允许跨站浏览器写：

| 路径 | 允许原因 | 二次校验 |
| --- | --- | --- |
| `POST /_bifrost/api/rules/share-env/exit` | 被代理业务页面需要跨站结束 Share Env 模式 | 请求 body 中的 exit token 必须匹配 |
| `/_bifrost/api/devtools/bridge/<page_id>/...` | `devtools://` 规则命中的被代理页面需要 DevTools bridge | URL 中的 `page_id` token 校验 |

`/_bifrost/api/push`、普通 rule API、`share-confirm` 等都不属于例外，必须走 CSRF。

### Rule Share 确认页

`__bifrost_rule` payload 处理流程：

1. bifrost-proxy `handle_http` 识别 URL 中含 `__bifrost_rule=<encoded>`。
2. 清洗 URL（剥离该 query 参数）得到 `clean_url`。
3. 生成 `http://127.0.0.1:<listen_port>/_bifrost/share/rule?payload=<encoded>&target=<clean_url>` 并 302。
4. 用户访问确认页，页面 handler `handle_rule_share_confirm_page` 解码 payload、校验 `target` scheme（`http/https`）与不含嵌套 `__bifrost_rule`，渲染规则名、mode、content hash、独占范围与完整规则内容。
5. 用户点击 Apply Rule，同源 `POST /_bifrost/api/rules/share-confirm { payload, target_url }` → `handle_rule_share_confirm_api` → `import_rule_share_payload`，成功后 redirect 到 `target_url`。
6. 确认页 CSP 必须包含 `connect-src 'self'`，否则同源 fetch 会被拦。

## 技术细节

### 关键文件

- `crates/bifrost-admin/src/security.rs`：`is_valid_admin_request`、`is_cert_public_request`、Host 白名单。
- `crates/bifrost-admin/src/router.rs`：`AdminRouter::handle`、`check_api_auth`、`check_browser_write_guard`、`should_apply_cors`。
- `crates/bifrost-admin/src/cors.rs`：`is_allowed_origin`、`apply_cors_headers`、`origin_matches_host`、`websocket_origin_rejection`。
- `crates/bifrost-admin/src/handlers/rule_share_confirm.rs`：`handle_rule_share_confirm_page`、`handle_rule_share_confirm_api`、`validate_target_url`。
- `crates/bifrost-admin/src/rule_share_import.rs`：`import_rule_share_payload`（唯一写入入口，代理层不得直接调用）。
- `crates/bifrost-core/src/rule_share.rs`：`decode_rule_share_payload`、`RULE_SHARE_QUERY_PARAM = "__bifrost_rule"`、`RuleShareMode`。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`：`handle_http`、`rule_share_query` 检测、`handle_http_websocket` + `should_route_websocket_to_local_admin`。
- `crates/bifrost-proxy/src/server.rs`：`is_admin_virtual_host_request`，Admin 虚拟 Host 处理路径。

### 关键常量与 header

```rust
pub const RULE_SHARE_QUERY_PARAM: &str = "__bifrost_rule";
pub const ADMIN_VIRTUAL_HOST: &str = "bifrost.local";
// CSRF header
"X-Bifrost-CSRF"
// browser signals
"Sec-Fetch-Site" ∈ {"cross-site", "same-origin", "same-site", "none"}
"Origin"
```

### 判定优先级

Admin API 请求进入顺序（伪代码）：

```rust
fn admin_pipeline(req, peer_addr, state) {
    if !is_valid_admin_request(&req, peer_addr, &state.config, state.remote_access) {
        return 403;
    }
    if let Some(resp) = check_api_auth(&req, &state, path, peer_addr) {
        return resp; // 401/403
    }
    if is_unsafe_method(&req) {
        if let Some(resp) = check_browser_write_guard(&req, &state, path) {
            return resp; // 403 origin, 428 csrf missing
        }
    }
    dispatch(path, req)
}
```

## Admin API

无新增字段。相关既有端点：

- `GET /_bifrost/api/csrf`：安全下发 CSRF token（只对通过 server 层校验的调用者返回）。
- `POST /_bifrost/api/rules/share-confirm { payload, target_url }`：确认导入。
- `GET /_bifrost/share/rule?payload=...&target=...`：确认页 HTML。
- `POST /_bifrost/api/rules/share-env/exit`：Share Env 退出（跨站窄例外）。
- `/_bifrost/api/devtools/bridge/<page_id>/...`：DevTools bridge（跨站窄例外）。

## CLI 交互

CLI 无 `Sec-Fetch-Site`、`Origin`、`Referer`，被识别为非浏览器上下文，`check_browser_write_guard` 返回 `None`，允许通行。这意味着：

- `bifrost rule enable/disable/update/delete` 继续可用。
- `bifrost` 自动化脚本无需感知 CSRF token。
- 用户使用 `curl -H "Host: 127.0.0.1" http://127.0.0.1:9900/_bifrost/api/rules` 仍然可读，但如果构造 `curl` 请求带 `Sec-Fetch-Site: cross-site` header 也会触发浏览器 guard——这是有意为之的模拟测试路径。

## Web UI 交互

- 首屏加载前先 `GET /_bifrost/api/csrf` 拿 token，缓存到内存并注入到全局 fetch wrapper。
- 所有 unsafe 请求自动附加 `X-Bifrost-CSRF: <token>`。
- Rule Share 确认页由后端渲染，前端只负责一个 form + fetch 提交 Apply Rule。
- 前端不得暴露“直接导入”按钮跳过确认页。

## Sync / 导入导出 / 分享边界

- Rule Share URL 只是把规则序列化到 query，最终写入靠确认页 + `import_rule_share_payload`。代理层不参与写入。
- Rule Sync（远端同步）走 `/_bifrost/api/sync/...`，与本方案安全边界无关，但同样受 server + AdminRouter 保护。
- 导入导出仍走 CLI 或 Web UI 内主动上传，无跨站入口。

## 实现切分

### Phase 1：Server 与 AdminRouter 加固

- 完成 `is_valid_admin_request` Host/absolute-form 校验。
- 完成 `check_browser_write_guard` Origin/Sec-Fetch/CSRF 三重校验。
- 完成 `share-env/exit` 与 `devtools/bridge/*` 白名单窄例外。
- 单元测试覆盖 12+ 情况。

### Phase 2：Rule Share 确认页

- 代理入口 `handle_http` 检测 `__bifrost_rule` 并 302 到确认页。
- 确认页 handler + 前端表单。
- `share-confirm` API 走 CSRF。
- 移除代理层旧 `import_rule_share_payload` 调用点。

### Phase 3：Admin Push WebSocket 加固

- `handle_http_websocket` 中 `should_route_websocket_to_local_admin` 短路本机 Admin 路径。
- 通过代理伪造的 push WebSocket 走普通 WebSocket 转发路径（Origin 校验拒绝或目标不存在拒绝）。

### Phase 4：文档与真实回归

- 更新 `human_tests/admin-cross-site-security.md`。
- E2E 脚本覆盖攻击路径。
- 前端 CSP `connect-src 'self'` 检查。

## 测试方案

### 单元测试

- `security::tests::test_loopback_peer_with_bad_host_is_rejected`：DNS rebinding host 拒绝。
- `security::tests::test_absolute_form_admin_uri_is_rejected`：absolute-form 拒绝。
- `security::tests::test_remote_access_toggle_paths`：remote access on/off 覆盖 loopback/remote 组合。
- `router::tests::test_browser_write_guard_rejects_cross_site_fetch`：`Sec-Fetch-Site: cross-site` 拒绝。
- `router::tests::test_browser_write_guard_requires_csrf_for_local_origin`：同源缺 CSRF 拒绝。
- `router::tests::test_browser_write_guard_accepts_local_origin_with_csrf`：同源含正确 CSRF 通过。
- `router::tests::test_browser_write_guard_allows_share_env_exit_cross_site_bridge`：`share-env/exit` 窄例外通过。
- `router::tests::test_check_api_auth_allows_devtools_bridge_page_id`：`devtools/bridge/<page_id>` 通过。
- `rule_share_confirm::tests::test_confirm_page_rejects_nested_share_payload`：`target` 中含 `__bifrost_rule` 拒绝。
- `rule_share_confirm::tests::test_confirm_page_escapes_html`：payload 中 HTML 特殊字符转义。
- `bifrost-proxy` `test_rule_share_query_redirects_to_confirm_page`：代理入口只跳确认页。
- `server::tests::test_devtools_bridge_admin_path_is_narrow_exception`：例外不扩散到 `api/push` 或规则 API。

### E2E 测试

- 新增 `e2e-tests/tests/test_admin_cross_site_security.sh`：
  - 用 `--resolve evil.example:9900:127.0.0.1` 模拟 DNS rebinding，断言 403。
  - 构造 absolute-form 请求 `printf 'GET http://127.0.0.1:9900/_bifrost/api/rules HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n' | nc 127.0.0.1 9900` 断言 403。
  - `Sec-Fetch-Site: cross-site` POST 断言 403。
  - 同源 POST 缺 `X-Bifrost-CSRF` 断言 403。
  - 同源 POST 携带正确 CSRF 断言 200。
- 复用 `e2e-tests/tests/test_asr_admin_csrf.sh` 覆盖 ASR admin 类接口 CSRF。
- 新增 `e2e-tests/tests/test_rule_share_query.sh`：代理入口 `__bifrost_rule` 只 302 到确认页；确认页 fetch `share-confirm` 落地规则；重复确认不重复导入。
- 扩展 `e2e-tests/tests/test_traffic_push_e2e.sh`：通过 HTTP 代理伪造 `/_bifrost/api/push` WebSocket → 断开或 403；直连 push → connected。
- 扩展 `e2e-tests/tests/test_devtools_page_bridge_api.sh`：token 化 DevTools bridge 仍能连接。

### 真实场景测试 human_tests

`human_tests/admin-cross-site-security.md` 已存在，用例覆盖：

- TC-ACS-01：DNS rebinding attack 模拟，Host 白名单拒绝。
- TC-ACS-02：absolute-form 代理伪造 Admin API，拒绝。
- TC-ACS-03：跨站 POST 缺 CSRF，拒绝。
- TC-ACS-04：确认页 Apply Rule 走 CSRF 后落地。
- TC-ACS-05：`share-env/exit` 跨站 POST 携带正确 token 成功。
- TC-ACS-06：DevTools bridge `page_id` 校验通过。
- TC-ACS-07：Admin Push WebSocket 直连 vs 代理伪造对比。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin security::`
- `cargo test -p bifrost-admin router::`
- `cargo test -p bifrost-admin rule_share_confirm`
- `cargo test -p bifrost-proxy rule_share_query`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 server 入口 `is_valid_admin_request` 是否有绕过分支（例如某个 handler 直连未通过 pipeline）。
- 复核 AdminRouter 的 `check_browser_write_guard` 是否漏掉 unsafe method（PATCH、DELETE）。
- 复核代理层 `__bifrost_rule` 是否只走确认页，`import_rule_share_payload` 只在 `handle_rule_share_confirm_api` 中调用。
- 复核前端 fetch wrapper 是否统一附加 CSRF。
- 复测 admin/proxy 相关单测与新增 E2E 脚本。

### 第 2 轮

- 复查文档、`human_tests/admin-cross-site-security.md`、旧 rule-share E2E 语义与整体 diff。
- 复跑受影响 E2E 与 `cargo fmt/clippy/test --workspace --all-features`。
- 验证 CSP `connect-src 'self'` 与 CORS `allowed_origin_header_value` 不冲突。

## 风险与决策点

- **CSRF token 泄漏**：token 通过 `/_bifrost/api/csrf` 返回，只能被通过 server 层校验的调用者拿到；跨站页面拿不到 token，即便脚本尝试也过不了 CORS。若未来引入 SSE/EventSource 场景，需要确保它不成为 token 泄漏通道。
- **窄例外扩散**：`share-env/exit` 与 `devtools/bridge/*` 是当前仅有的跨站写例外。任何新增例外必须走 design review，并附带专用的 token 校验，不能靠 URL prefix 单一判断。
- **Absolute-form 兼容性**：企业内部工具可能用 `curl -x http://127.0.0.1:9900 http://127.0.0.1:9900/_bifrost/api/rules` 这种带代理的调用方式访问 Admin API。这类调用会命中 absolute-form 拒绝，需要改用直连或 `bifrost` CLI；文档需明确说明。
- **DNS rebinding 白名单维护**：Host 白名单来自 `is_allowed_host`，若未来允许自定义 Admin domain（如 `https://bifrost.corp/`），需扩展白名单并保证 rebinding 校验不失效。
- **Rule Share 确认页 UX**：第一版不做 content-hash 手工输入，仅展示 hash 供人工核对。若发现用户不看内容就 Apply，可考虑增加二次确认或高危规则拦截（如 `*` matcher）。
