# Admin Cross-Site And Rule Share Security

## 背景

Bifrost 同时暴露本机 Admin API 和 HTTP/HTTPS 代理入口。未知网页可以利用浏览器自动发请求的能力，尝试：

- 直接访问 `http://127.0.0.1:<port>/_bifrost/api/...` 修改规则。
- 通过 DNS rebinding 让 peer 看起来是 loopback，但 `Host` 仍是攻击者域名。
- 通过 Bifrost 代理发送 absolute-form 请求，伪造成 Admin API 写操作。
- 通过 Bifrost 代理伪造 `/_bifrost/api/push` 等 Admin WebSocket 请求。
- 在目标 URL 中植入 `__bifrost_rule`，借代理流量静默启用或替换规则。

这些路径都不能依赖前端防护，必须在 server/router/proxy 三层封堵。

## 方案

### P0 Admin API 写请求防护

1. Server 层所有私有 `/_bifrost` 请求必须先通过 `is_valid_admin_request`，不再存在 loopback 直通。
2. loopback peer 始终要求 `Host` 是本机白名单 host，即使 remote access 已开启，也拒绝 `Host: evil.example` 这类 DNS rebinding 请求。
3. absolute-form Admin API 请求包含 URI scheme，会被视为代理伪造请求并拒绝。
4. Browser unsafe method 请求统一执行 Origin / Sec-Fetch / CSRF 校验：
   - `Sec-Fetch-Site: cross-site` 直接拒绝。
   - `Origin` 既不是本机允许 origin，也不与 Host 同源时拒绝。
   - `POST` / `PUT` / `PATCH` / `DELETE` 必须携带 `X-Bifrost-CSRF`。
5. CLI、curl 和内部自动化没有浏览器上下文时保留原有 Admin API 能力，仍需先通过 server 层 Host / absolute-form 校验。
6. 仅保留明确 token 化的注入页面入口作为窄例外：
   - `POST /_bifrost/api/rules/share-env/exit` 允许业务页面跨站调用，但必须携带正确 JSON body token。
   - `/_bifrost/api/devtools/bridge/<page_id>/...` 允许命中 `devtools://` 规则的被代理页面连接，后续仍由 page bridge token 校验。
   - `/_bifrost/api/push`、普通规则 API 和 rule share confirm 不属于例外，不能通过 HTTP proxy 或跨站浏览器请求访问。

### P1 Rule Share 确认页

1. 代理层识别 `__bifrost_rule` 后只生成本机确认 URL：
   `http://127.0.0.1:<port>/_bifrost/share/rule?payload=...&target=<clean_url>`。
2. 代理层不再调用 `import_rule_share_payload`，未知网页只能触发确认页，不能静默写规则。
3. 确认页展示规则名称、内容 hash、模式、独占范围、返回目标和完整规则内容。
4. 用户点击 Apply Rule 后，页面以同源 `POST /_bifrost/api/rules/share-confirm` 应用规则，再跳回 clean URL。
5. `share-confirm` 属于 unsafe Admin API，浏览器上下文必须通过 CSRF 校验。

## 测试计划

- 单元测试：
  - `security::tests` 覆盖 Host、absolute-form、loopback 和 remote access 组合。
  - `router::tests::test_browser_write_guard_*` 覆盖跨站、缺 CSRF、有效 CSRF 和 CLI 非浏览器上下文。
  - `rule_share_confirm` 覆盖确认页转义和嵌套 payload 拒绝。
  - `bifrost-proxy rule_share_query` 覆盖代理入口只跳确认页。
  - `server::tests::test_devtools_bridge_admin_path_is_narrow_exception` 覆盖 DevTools bridge 例外不扩散到 `api/push` 或规则 API。
- E2E：
  - `e2e-tests/tests/test_admin_cross_site_security.sh` 使用真实 Bifrost 服务验证 DNS rebinding Host、absolute-form 代理伪造、Origin/CSRF、防静默 rule-share。
  - `e2e-tests/tests/test_rule_share_query.sh` 验证确认页导入、重复确认复用、同名不同内容后缀和 API 生成链接。
  - `e2e-tests/tests/test_traffic_push_e2e.sh` 验证通过 HTTP proxy 伪造 Admin Push WebSocket 返回 403，直连 Admin Push 仍可用。
  - `e2e-tests/tests/test_devtools_page_bridge_api.sh` 验证 token 化 DevTools page bridge 仍能在被代理页面内建立连接并完成调试能力回归。
- human_tests：
  - `human_tests/admin-cross-site-security.md` 逐条执行真实 CLI/API/代理链路，记录 P0/P1 攻击路径不可复现。

## Review/Fix/Test 闭环

- 第 1 轮：复核 server 入口、AdminRouter guard、proxy rule-share 两条入口和前端 API wrapper，运行 admin/proxy 相关单测与新增安全 E2E。
- 第 2 轮：复查文档、human_tests、旧 rule-share E2E 语义和整体 diff，复跑受影响 E2E 与格式/编译检查。
