# Admin 公开二维码接口 CORS 修复方案

## 功能模块描述

修复 Bifrost 管理端公开二维码接口的跨域访问行为，覆盖以下公开资源：

- `GET /_bifrost/public/cert`
- `GET /_bifrost/public/cert/qrcode`
- `GET /_bifrost/public/proxy/qrcode`

目标是保证这些公开资源在浏览器跨域加载或通过前端 `fetch` 访问时，稳定返回完整的 CORS 响应头，并对 `OPTIONS` 预检保持一致行为。

补充目标：`/_bifrost/public/cert` 作为证书分发入口，需要对直连请求和 absolute-form 请求一视同仁，不能因为请求 URI 带 scheme 被误判为管理端伪造访问而返回 `403 Forbidden`。

## 实现逻辑

1. 在 `crates/bifrost-admin/src/handlers/mod.rs` 中新增公开资源响应 builder `public_response_builder`，统一注入：
   - `Access-Control-Allow-Methods: GET, HEAD, OPTIONS`（常量 `PUBLIC_CORS_ALLOW_METHODS`）
   - `Access-Control-Allow-Headers: Content-Type, Authorization, X-Client-Id`（常量 `ADMIN_CORS_ALLOW_HEADERS`）
   - `Access-Control-Allow-Origin` 由 builder 留空，统一在 `router::AdminRouter::handle` 出口处通过 `apply_cors_headers` 根据请求 `Origin` 回填（受 `crates/bifrost-admin/src/cors.rs` 的 allowlist 约束），不再硬编码为 `*`。
2. 将 cert 下载、cert 二维码、proxy 二维码的成功响应切换为统一 builder，避免后续新增公开资源时遗漏 CORS 头。
3. 在公开 cert/proxy handler 中显式支持 `OPTIONS`，即使未来 router 层调整，也能保持接口级别的跨域能力清晰可见。
4. 公开证书路径的放行仅基于 `/_bifrost/public/cert` 前缀判断，不再额外拒绝 absolute-form URI；这样通过代理格式或某些客户端构造的请求目标访问时，仍然可以正常下载证书。

## 依赖项

- `crates/bifrost-admin` 现有 hyper handler 框架
- `crates/bifrost-e2e` 端到端测试框架

## 测试方案

- 新增 admin 类 E2E 回归测试，校验 `/_bifrost/public/proxy/qrcode?ip=127.0.0.1`：
  - `GET` 返回 `200`
  - 响应包含公开资源 CORS 头
  - `Content-Type` 为 `image/svg+xml`
  - `OPTIONS` 预检返回 `204`
- 新增 cert download 回归测试，校验 absolute-form 请求目标访问 `/_bifrost/public/cert` 不会返回 `403`
- 新增 handler 单测，校验公开资源 builder 始终包含预期 CORS 头。

## 校验要求

- 先执行本次相关 E2E 测试
- 再执行 `rust-project-validate` 要求的格式、lint、测试与构建校验

## 文档更新要求

- 本次不涉及外部 API 语义变化，无需更新 `README.md`
- `crates/bifrost-admin/ADMIN_API.md` 无新增字段或路径，可不变
