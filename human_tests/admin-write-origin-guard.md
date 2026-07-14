# Admin 写接口 Origin Guard 回归

## 功能模块说明

管理端会对浏览器来源的写接口执行 Origin / Fetch Metadata / CSRF 防护。Mac 桌面壳从本地页面访问 `http://127.0.0.1:9900` 后端时，WebView 可能把请求标记为 `Sec-Fetch-Site: cross-site`，但 `Origin` 仍是可信的本机桌面 Origin（如 `tauri://localhost` 或 `http://tauri.localhost`）。本用例验证后端不会把可信桌面 Origin 的写接口误判为跨站攻击，同时仍然拦截外部网页来源，并继续要求 `X-Bifrost-CSRF`。

## 前置条件

- 当前仓库位于本次修复分支。
- 已安装 Rust 与 `web/` 依赖。
- 执行命令前先运行 `source ~/.zshrc`。

## 测试用例列表

### TC-AOG-01：外部网页 cross-site 写请求仍被拒绝

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin test_browser_write_guard_rejects_cross_site_fetch -- --nocapture
   ```

**预期结果**：

- 测试通过。
- `Origin: http://evil.example` 且 `Sec-Fetch-Site: cross-site` 的写请求返回 403。

### TC-AOG-02：可信桌面 Origin 即使被标记 cross-site 也可通过 Origin Guard

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin test_browser_write_guard_accepts_trusted_desktop_origin_when_sec_fetch_is_cross_site -- --nocapture
   ```

**预期结果**：

- 测试通过。
- `Origin: tauri://localhost`、`Host: 127.0.0.1:9900`、`Sec-Fetch-Site: cross-site` 且带有效 `X-Bifrost-CSRF` 的写请求不再返回 `Cross-site admin write request rejected`。

### TC-AOG-03：可信桌面 Origin 仍必须携带 CSRF token

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin test_browser_write_guard_still_requires_csrf_for_trusted_cross_site_desktop_origin -- --nocapture
   ```

**预期结果**：

- 测试通过。
- 可信桌面 Origin 如果缺少 `X-Bifrost-CSRF`，仍返回 403。

### TC-AOG-04：WebSocket Origin Guard 与写接口保持一致

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin ws_guard -- --nocapture
   ```

**预期结果**：

- `ws_guard_allows_trusted_desktop_origin_even_when_sec_fetch_is_cross_site` 通过。
- `ws_guard_rejects_untrusted_cross_site_via_sec_fetch` 通过。
- 既有本机 Origin、远程同 Host Origin、外部 Origin 用例继续通过。

### TC-AOG-05：真实 9900 服务端写接口不再误报 cross-site

**操作步骤**：

1. 用修复后的二进制重启 9900 服务端。
2. 获取 CSRF token 并发送模拟桌面壳的写请求：
   ```bash
   source ~/.zshrc && TOKEN=$(curl -s http://127.0.0.1:9900/_bifrost/api/security/csrf | jq -r '.csrf_token') && curl -i -X DELETE \
     -H "Origin: tauri://localhost" \
     -H "Sec-Fetch-Site: cross-site" \
     -H "X-Bifrost-CSRF: $TOKEN" \
     http://127.0.0.1:9900/_bifrost/api/whitelist/pending
   ```

**预期结果**：

- 响应不是 `403 Cross-site admin write request rejected`。
- 正常情况下返回 `200 OK` 与 `{"success":true,...}`。

### TC-AOG-06：Pending Authorization 弹窗关闭不依赖服务端成功

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/pending-auth-modal.spec.ts
   ```

**预期结果**：

- `全局弹窗 Clear All 失败时也会关闭，不阻塞页面` 通过。
- `全局弹窗 Settings 按钮可以导航到 Access Control 设置页` 通过，点击 Settings 后弹窗关闭。
- `Clear All` 即使后端返回 `Cross-site admin write request rejected` 也不会继续阻塞页面。

### TC-AOG-07：过期 CSRF token 仍会自动刷新并重试

**操作步骤**：

1. 执行：
   ```bash
   source ~/.zshrc && pnpm --dir web exec vitest run src/api/asr.test.ts
   ```
2. 执行：
   ```bash
   source ~/.zshrc && BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 pnpm --dir web exec playwright test tests/ui/pending-auth-modal.spec.ts
   ```

**预期结果**：

- `refreshes a stale admin CSRF token and retries unsafe fetch requests once` 通过。
- `全局弹窗操作遇到过期 CSRF token 时会刷新 token 并重试` 通过。
- `Approve`、`Reject`、`Clear All` 既有 pending auth 用例继续通过。

### TC-AOG-08：桌面 App Runner 与 IM Channel 保存的 PATCH CSRF 预检与真实写入

**操作步骤**：

1. 执行聚焦 CORS 契约单元测试：
   ```bash
   source ~/.zshrc && cargo test -p bifrost-admin cors_preflight_allows_desktop_client_header --lib -- --nocapture
   ```
2. 构建当前源码二进制后执行真实 Admin security E2E：
   ```bash
   source ~/.zshrc && cargo build --bin bifrost && \
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_admin_cross_site_security.sh
   ```

**预期结果**：

- 单元测试确认 `Access-Control-Allow-Methods` 包含独立的 `PATCH` method，并继续允许 `X-Bifrost-CSRF` 与 `X-Client-Id` 请求头。
- E2E 启动隔离临时数据目录的真实 Bifrost；模拟 `Origin: tauri://localhost` 的 Runner config OPTIONS 预检返回该 Origin、`PATCH` 和 `X-Bifrost-CSRF`。
- E2E 获取真实 CSRF token 后，对 `/_bifrost/api/im-gateway/chat/config` 执行模拟桌面 WebView 的 PATCH，写入 IM Channel 的 disabled、Runner Override 和 `no_im` Delivery Override，返回 HTTP 200。
- 随后的 GET 回读出完全一致的 channel override，覆盖截图红框中的自动保存与 `Save Channel` 共用链路。
- 同一脚本中的外部 cross-site、缺 CSRF、DNS rebinding、Rule Share 与 WebSocket 安全回归全部继续通过。

## 清理步骤

- Playwright 场景使用独立测试代理和数据目录，测试结束自动清理。
- 本用例不要求修改用户默认 `~/.bifrost` 数据。

## 执行记录

- 2026-07-14：TC-AOG-08 PASS。`cargo test -p bifrost-admin cors_preflight_allows_desktop_client_header --lib -- --nocapture` 为 1 passed；`cargo build --bin bifrost` 成功；`BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_admin_cross_site_security.sh` 连续通过。真实临时服务的 Tauri OPTIONS 响应允许 `PATCH` 与 `X-Bifrost-CSRF`，Runner/IM Channel config PATCH 返回 200，GET 回读 disabled、Runner Override 与 `no_im` Delivery Override 一致；同脚本的跨站、缺 CSRF、DNS rebinding、Rule Share 与 WebSocket 安全回归全部通过。
