# Desktop Runner 保存 CSRF 预检修复

## 用户目标验证清单

### 必须实现

- 桌面 App 编辑 Runner 后可以通过 `PATCH /_bifrost/api/im-gateway/chat/config` 保存。
- 桌面 App 的 IM Channel Runner Override、Enabled Override 与 Delivery Override 可以通过同一 PATCH 保存并回读。
- 保存请求继续携带 `X-Bifrost-CSRF`，不得通过绕过或放松 CSRF 门禁修复。
- Admin API 的 CORS 预检明确允许 `PATCH`，与后端实际支持的 unsafe 方法一致。

### 必须不破坏

- 外部跨站 Origin 仍被拒绝；只有既有可信本机/桌面 Origin 获得 CORS 响应。
- `X-Bifrost-CSRF`、`X-Client-Id` 与 JSON Content-Type 继续列入允许请求头。
- WebUI Runner 表单、保存 payload、紧凑布局以及亮色/暗色主题均不改变。
- CLI 等没有浏览器上下文的原生调用保持现有行为。

### 必须真实验证

- 单元测试断言统一 Admin CORS methods 包含 `PATCH`。
- 真实 Bifrost 临时服务接收模拟 `tauri://localhost` 的 OPTIONS 预检，并返回允许 Origin、`PATCH` 和 `X-Bifrost-CSRF`。
- 同一临时服务使用有效 CSRF token 执行 Runner config PATCH，写入 channel override 后返回 HTTP 200，并能从 GET 回读一致配置。
- 既有跨站拒绝、缺 CSRF 拒绝与 Rule Share 安全回归继续通过。

### 必须交付

- 更新设计说明、单元测试、Shell E2E、`human_tests/` 用例和索引。
- 完成两轮 Review/Fix/Test、本地验证、提交、PR 与远端 CI（含 coverage 90% 门禁）看护。

## 根因

桌面 App 页面与本地 Bifrost core 运行在不同 Origin。Runner 保存使用 `PATCH`，共享 API client 还会自动加入 `X-Bifrost-CSRF`、`X-Client-Id` 和 JSON Content-Type，因此 WebView 必须先完成 CORS OPTIONS 预检。

后端 Runner handler 已支持 `PATCH`，CSRF guard 也认可带有效 token 的可信桌面 Origin，但统一 `cors_preflight()` 的 `Access-Control-Allow-Methods` 只包含 `GET, POST, PUT, DELETE, OPTIONS`。浏览器在真正的 PATCH 发出前即阻断请求，Axios 得到无 HTTP response 的网络错误；桌面 UI 因而同时显示 core reconnect 提示和 Runner `Save failed`。

## 方案

- 在 Admin handler 层定义唯一 `ADMIN_CORS_ALLOW_METHODS` 常量，覆盖 `GET, POST, PUT, PATCH, DELETE, OPTIONS`。
- `cors_preflight()` 使用该常量，不对 Runner endpoint 增加安全例外。
- 单元测试同时断言完整 methods 契约和 `PATCH` 的独立存在，防止未来重构再次遗漏。
- 在既有 Admin cross-site security E2E 中加入桌面 Runner/Channel 保存场景：先验证浏览器预检响应，再使用真实 CSRF token PATCH 一个 channel override，并通过 GET 回读断言 enabled、runner 与 delivery mode。

## 测试计划

### 单元测试

- `handlers::tests::cors_preflight_allows_desktop_client_header`：断言 methods 等于统一常量且包含独立的 `PATCH` token；继续断言 `X-Client-Id` 与 `X-Bifrost-CSRF`。

### E2E

- `e2e-tests/tests/test_admin_cross_site_security.sh`：启动隔离数据目录的真实 Bifrost，模拟 Tauri WebView 的 OPTIONS 与 PATCH Runner config 请求，并继续执行原有安全矩阵。

### 真实场景测试

- 更新 `human_tests/admin-write-origin-guard.md`，新增桌面 Runner 保存预检与真实 PATCH 回归用例，并在文档更新后立即执行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照截图与网络语义复核根因，检查 `git status --short`、`git diff` 与新增文件。
- Review 统一 CORS 契约是否完整、是否意外放宽 Origin/CSRF、E2E 是否真实访问 Runner endpoint。
- 运行聚焦 Rust 单测与 Admin security E2E。

### 第 2 轮

- 基于第 1 轮修复后的 diff 再次检查 methods/header token 匹配、文档和索引一致性。
- 复跑受影响单测、E2E 和 human_tests，并检查明暗主题无 UI 代码变更。

## 覆盖率与交付

本次生产代码仅修改共享 CORS methods 常量，新增 Rust 分支覆盖断言与真实 Shell E2E。默认不在本机运行高成本全量 coverage；由远端 `bash scripts/ci/coverage-all.sh --json --gate` 和 changed-lines 门禁兜底。最终按仓库规则执行 E2E、`rust-project-validate`、workspace all-features、提交推送、PR 和 CI 看护。
