# Web Dev 模式管理端 Public 路由转发方案

## 背景

`web/` 前端在 Vite dev 模式下通过 `web/vite.config.ts` 的 `server.proxy` 把请求转发到 Bifrost 后端（默认 `http://127.0.0.1:9900`）。第一版只代理了 `/_bifrost/api` 和 `/_bifrost/ws`，但 Bifrost 管理端还暴露了一组无需鉴权的公开资源路由 `/_bifrost/public/*`（CA 证书、CA 证书二维码、代理配置二维码），本地前端访问它们时会被 Vite dev server 直接当成前端路由，返回 `index.html`。

本模块补齐 Vite dev proxy，让开发者在 `pnpm dev` 打开的 `http://127.0.0.1:3000` 上，也能通过统一的 `buildPublicUrl()` 路径拿到后端 SVG/PEM 内容，不需要在前端为 dev 模式单独分支。

真实实现位于 `web/vite.config.ts` 与 `web/src/runtime.ts` 中的 `buildPublicUrl()`；后端提供方是 `crates/bifrost-admin` 的 public handler。

## 用户目标验证清单

### 必须实现

- Vite dev server 在 `/_bifrost/public` 前缀下把请求 HTTP 代理到 `http://127.0.0.1:${backendPort}`。
- 代理同时支持 `changeOrigin: true`，避免后端按 Origin 校验时被拒绝。
- `backendPort` 支持三种来源，优先级：命令行 `--backend-port` / `--proxy-port` > 环境变量 `BACKEND_PORT` / `PROXY_TARGET_PORT` > 默认 `9900`。
- 前端继续使用统一的 `buildPublicUrl()` 生成 URL，不允许在组件层为 dev 模式硬编码 host。
- `/_bifrost/api`、`/_bifrost/ws`、`/_bifrost/swagger`、`/_bifrost/public` 四个前缀在 dev/prod 表现一致。

### 必须不破坏

- Desktop 打包模式（`mode === 'desktop'`）仍走相对路径 `base: './'` 和 `dist-desktop` 输出目录。
- 生产模式下 Vite 只负责静态资源；`/_bifrost/*` 由 Bifrost admin server 原生托管，不涉及 Vite proxy 配置。
- `/_bifrost/api` 的 WebSocket upgrade（`ws: true`）行为保留。

### 必须真实验证

- 启动 Bifrost 后端 + `pnpm --dir web dev`，访问三条 URL：
  - `http://127.0.0.1:3000/_bifrost/public/cert`（返回 CA 证书 PEM）
  - `http://127.0.0.1:3000/_bifrost/public/cert/qrcode`（返回 SVG 二维码）
  - `http://127.0.0.1:3000/_bifrost/public/proxy/qrcode?ip=127.0.0.1`（返回代理配置 SVG）
- 全部返回 200 + 正确 Content-Type（`application/x-x509-ca-cert` 或 `image/svg+xml`）。

## 产品语义

### 为什么必须在 dev 走后端

`buildPublicUrl()` 生成的路径既被 App 内部 QR 展示复用，也被移动设备扫码打开使用。产品语义要求：

- 桌面预览时二维码内容必须与生产环境一致，才能保证扫码后手机端能正确拿到 CA 证书。
- 开发时不允许前端把 QR 内容内嵌成 data-url 或者本地静态；必须让 dev server 反代后端，才能反映最新证书 / 代理端口变化。
- Swagger UI (`/_bifrost/swagger`) 同理：调试 API 时必须看到当前 backend 端口的实时 Swagger 文档。

### 端口来源

`backendPort` 决策链：

1. 命令行：`pnpm dev --backend-port=9901` 或 `pnpm dev --proxy-port 9902`。
2. 环境变量：`BACKEND_PORT=9901 pnpm dev` 或 `PROXY_TARGET_PORT=9902 pnpm dev`。
3. 默认 `9900`。

无效数字（非有限、`<= 0`）会 fallback 到默认，避免 dev 因误传端口而完全无法启动。

## 技术细节

### `web/vite.config.ts`（真实实现）

```ts
server: {
  host: '127.0.0.1',
  port: webPort,
  proxy: {
    '/_bifrost/api': {
      target: backendHttpTarget,
      changeOrigin: true,
      ws: true,
    },
    '/_bifrost/swagger': {
      target: backendHttpTarget,
      changeOrigin: true,
    },
    '/_bifrost/public': {
      target: backendHttpTarget,
      changeOrigin: true,
    },
    '/_bifrost/ws': {
      target: backendWsTarget,
      ws: true,
    },
  },
},
```

- `webPort` 来自 `process.env.WEB_PORT ?? 3000`。
- `backendHttpTarget = "http://127.0.0.1:${backendPort}"`。
- `backendWsTarget = "ws://127.0.0.1:${backendPort}"`。
- `base` 在非 desktop 模式为 `/_bifrost/`（生产环境资源路径），desktop 模式为 `./`。

### `buildPublicUrl()`

`web/src/runtime.ts` 统一负责拼装 `/_bifrost/public/...` 路径；组件层调用它时不感知 dev/prod 差异。任何后续新增的 public 路由都必须通过 `buildPublicUrl()` 拼装，避免在 dev 模式漏配 proxy。

### 后端路由清单

`crates/bifrost-admin` public handler 提供：

- `GET /_bifrost/public/cert` → CA 证书 PEM。
- `GET /_bifrost/public/cert/qrcode` → CA 证书下载页面二维码 SVG。
- `GET /_bifrost/public/proxy/qrcode?ip=<addr>` → 代理配置二维码 SVG，携带 IP、端口、CA 下载指引。

这些路由不需要 CSRF/token；相关 CORS 与 QRCode 语义由 `admin-public-qrcode-cors.md` 覆盖。

## CLI + Web + Admin API

- CLI：无变更；`bifrost start` 仍在 backend 端口暴露 `/_bifrost/public/*`。
- Web：只调整 `vite.config.ts`；组件层不改。
- Admin API：无新增；本方案只是补代理，不改 handler。

## Sync 边界

不涉及。此模块是 dev 环境的开发者体验配置，与规则/Group/Sync 无关。

## Phase 1-4

### Phase 1：dev proxy 补齐

- 在 `web/vite.config.ts` 的 `server.proxy` 追加 `/_bifrost/public` 项，与 `/_bifrost/api / swagger / ws` 并列。
- 保证 `changeOrigin: true`，避免后端按 Origin 校验时拒绝 dev 请求。

### Phase 2：端口参数复用

- 抽出 `readArg`/`backendPort`/`backendHttpTarget/backendWsTarget` 共用；确保四条 proxy 使用同一 target。
- 参数无效时 fallback 到 9900。

### Phase 3：文档同步

- 把「dev server 也需要代理 `/_bifrost/public`」写入 web README / debugging 文档。
- 明确所有后续新增 public 路由必须走 `buildPublicUrl()`。

### Phase 4：真实浏览器验证

- 启动后端 + `pnpm --dir web dev`；用 Chrome DevTools 校验三条 URL 状态码、Content-Type、payload。
- 手机扫 `proxy/qrcode` 检查扫码后能进入下载 CA 证书页面。

## 测试方案

### 单元测试

- 无（Vite 配置属于构建脚本，无独立可单测的业务算法）。

### E2E 测试

- 不新增 E2E 脚本；由 `pnpm --dir web build` 与手工浏览器验证覆盖类型和运行时代理。

### 真实场景测试 human_tests

`human_tests/web-dev-admin-public-proxy.md`（若已建立，则维持；否则新建）：

- **TC-WDP-01** 默认端口：启动 Bifrost 9900 + `pnpm dev`，访问 `http://127.0.0.1:3000/_bifrost/public/cert` 返回 200 PEM。
- **TC-WDP-02** 自定义端口：`BACKEND_PORT=9901 pnpm dev` + `bifrost start -p 9901`，访问同一路径仍 200。
- **TC-WDP-03** QR 内容一致：`http://127.0.0.1:3000/_bifrost/public/proxy/qrcode?ip=192.168.0.10` 与 `http://127.0.0.1:9900/_bifrost/public/proxy/qrcode?ip=192.168.0.10` 二维码扫描后内容一致。
- **TC-WDP-04** Swagger 可访问：`http://127.0.0.1:3000/_bifrost/swagger/` 显示当前后端 Swagger UI。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：三条 URL 在 dev / prod 表现一致；desktop 模式不受影响。
- Diff 复核：只应改到 `web/vite.config.ts`，不应触及 `runtime.ts` 或后端。
- 校验：`pnpm --dir web lint` + `pnpm --dir web build` + 手工浏览器打开三条 URL。

### 第 2 轮

- 复查是否遗漏未来新增的 public 路径（例如 `/api/qrcode` 是否被搬到 `/_bifrost/public`）。
- 复跑 `pnpm --dir web build` 与 `bifrost start` 联调。

## 路由梳理结论

- `/_bifrost/api/*`：REST + WebSocket，dev 已代理（`ws: true`）。
- `/_bifrost/ws`：独立 WebSocket 端点，dev 已代理（`ws: true`）。
- `/_bifrost/swagger/*`：Swagger UI HTTP，dev 已代理。
- `/_bifrost/public/*`：**本次补齐**，含 `cert` / `cert/qrcode` / `proxy/qrcode`。

除以上四类外，当前 `web/src` 中未发现其他直接连后端的路径。若未来新增 `/_bifrost/xxx` 前缀，必须同步在 `vite.config.ts` 增补代理项，并考虑更新本设计文档的路由清单。

## 风险与决策

- **端口误配**：`BACKEND_PORT=abc pnpm dev` 会 fallback 到 9900；提示信息目前较弱，未来可增加显式 warning。
- **CORS 冲突**：dev proxy `changeOrigin: true` 会把 Origin 改成 `127.0.0.1:${backendPort}`，若后端 CORS 严格按 dev host 白名单校验，需要额外允许 dev origin；`admin-public-qrcode-cors.md` 已覆盖此点。
- **HTTPS 后端**：当前假设 backend 是 http；若未来 dev 环境需要连 https backend，需要在 proxy 项里追加 `secure: false` 或 `changeOrigin` + `rejectUnauthorized`。
- **多前缀维护成本**：`/_bifrost/` 家族不断增长；建议未来考虑一个通配 `/_bifrost/(api|ws|swagger|public|...)` 白名单表，避免每次新增都改 vite.config.ts；本次不引入以免破坏现有配置。
