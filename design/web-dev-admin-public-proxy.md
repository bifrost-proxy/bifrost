# Web Dev 模式管理端 Public 路由转发方案

## 功能模块描述

修复 `web` 项目在 Vite dev 模式下对 Bifrost 管理端公开资源路由的代理缺失问题，确保本地开发环境访问以下路径时能够正确转发到后端：

- `/_bifrost/public/cert`
- `/_bifrost/public/cert/qrcode`
- `/_bifrost/public/proxy/qrcode`

## 实现逻辑

1. 保持现有 `/_bifrost/api` 与 `/_bifrost/ws` 代理不变。
2. 在 `web/vite.config.ts` 中新增 `/_bifrost/public` 到后端 HTTP 端口的代理。
3. 前端继续通过 `buildPublicUrl()` 生成统一 URL，不为 dev 模式单独分支逻辑。

## 依赖项

- `web/vite.config.ts`
- `web/src/runtime.ts` 中的 `buildPublicUrl()`
- `crates/bifrost-admin` 提供的 `/_bifrost/public/*` 接口

## 测试方案

- 启动 Bifrost 后端服务和 `web` dev server。
- 访问 `http://127.0.0.1:3000/_bifrost/public/proxy/qrcode?ip=127.0.0.1`，确认返回 SVG。
- 访问 `http://127.0.0.1:3000/_bifrost/public/cert/qrcode` 与 `http://127.0.0.1:3000/_bifrost/public/cert`，确认可通过 dev 代理访问。

## 校验要求

- 先完成本次 dev 代理验证
- 再执行 `rust-project-validate` 要求的格式、lint、测试与构建校验

## 文档更新要求

- 本次不涉及外部 API 语义变化，无需更新 `README.md`
- 可在相关调试说明中补充 dev server 也需要代理 `/_bifrost/public`

## 路由梳理结论

当前前端直接依赖的后端前缀主要有三类：

- `/_bifrost/api/*`：已转发
- `/_bifrost/ws` 与基于 `/_bifrost/api/*` 的 WebSocket：已转发
- `/_bifrost/public/*`：本次补齐
- `/_bifrost/swagger/*`：同样已在 `web/vite.config.ts` 中作为 HTTP 代理透传，便于本地访问 Swagger UI

除以上四类外，当前 `web/src` 中没有发现其他需要在 Vite dev server 中单独透传的服务端直连路径。
