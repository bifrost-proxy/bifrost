# 流量详情页 SSE / WS 订阅在桌面端命中错误端口修复

## 背景

Bifrost 桌面端（Tauri）使用 WebView 装载前端资源，前端资源本身通过一个
本机静态服务器（例如 `tauri://localhost` 或 devtools 场景下的
`http://localhost:1420`）加载。真正的 Bifrost core 管理端口另开一个
`http://127.0.0.1:<proxy_port>`，路径前缀为 `/_bifrost/api`。

流量详情页的实时消息面板（WebSocket frames 流、SSE 事件流）需要通过
`EventSource` 订阅管理端接口：

- `/_bifrost/api/traffic/{id}/frames/stream`
- `/_bifrost/api/traffic/{id}/sse/stream`

历史实现直接把这两个路径当作**相对路径**传入 `new EventSource(path)`。在
浏览器 dev / 生产虚拟主机 (`bifrost.local`) 场景下这没问题；但在桌面端，
WebView 会把它拼到 WebView 自身的 origin 上，得到 `tauri://localhost/_bifrost/api/...`
之类的地址。这个地址不指向任何真实服务，`EventSource` 会立即失败或
永远 pending，用户看不到任何实时事件；关闭详情页再打开一次时，普通
`fetch` 走的是绝对 URL（`buildApiUrl` 已经正确拼接），因此“打开一次能
看到快照，但没有增量”是最常见的用户症状。

本次修复统一让所有 `EventSource` / WebSocket URL 都经过 `web/src/runtime.ts`
中的 `buildApiUrl` / `buildBackendUrl` 构造。

## 用户目标验证清单

### 必须实现

- 详情页 SSE 事件流订阅在桌面端命中 Bifrost core 端口，而不是 WebView
  自身端口。
- 详情页 WebSocket frames 流订阅在桌面端命中 Bifrost core 端口。
- 桌面 runtime 切换 proxy port 后，新的 `EventSource` 使用新的端口
  （`setDesktopProxyPort` 生效后立即体现在下次 URL 构造）。
- Web / 虚拟主机 / dev proxy 场景下 URL 保持原来的语义（相对 origin 或
  `bifrost.local`），不因修复变差。

### 必须不破坏

- `x_client_id` query 参数继续附加，用于 push 会话追踪。
- SSE / WS 订阅时机：`isConnectionOpen` 变 true 时打开，卸载 / recordId
  变化 / 强制关闭时 close，不改变生命周期。
- 消息解析、`sseSessionKeyRef` / `sseClosedByUsRef` guard 逻辑保持不变。
- Web 场景（非桌面）URL 不发生跨域，避免触发 CSRF / cookie 边界问题。

### 必须真实验证

- 桌面构建打开一个活跃 SSE 请求 → 详情面板事件持续增长。
- 桌面构建打开一个活跃 WebSocket 请求 → 详情面板 frames 持续新增。
- 桌面构建 devtools Network 面板中 `EventSource` 请求 URL 是
  `http://127.0.0.1:<proxy_port>/_bifrost/api/traffic/{id}/sse/stream`。
- Web dev 环境（`pnpm --dir web dev`）功能不回归。

## 产品语义

### 桌面端 URL 构造统一走 `runtime.ts`

`web/src/runtime.ts` 是唯一负责“把逻辑 API 路径转成运行时可访问 URL”
的模块。任何 `fetch` / `EventSource` / `WebSocket` 都必须使用它构造出
的绝对 URL，禁止把 `/_bifrost/api/...` 直接传给 `new EventSource`。

### 桌面端 proxy port 可能运行时切换

`setDesktopProxyPort(port)` 在桌面 runtime 端口切换（例如原端口被占用
自动 fallback）时会被调用；`buildBackendUrl` 每次都读取
`desktopRuntime.proxyPort`，因此新建 `EventSource` 时自动指向新端口。
已经打开的 `EventSource` 会因为端口切换 fail，前端下一轮 `useEffect`
会重连（当前实现依赖 `recordId` / `isConnectionOpen` / `sessionKey` 变化
触发重建；实际的端口切换会伴随 admin 页面重载，因此不需要额外的
observer）。

### 虚拟主机 (`bifrost.local`)

`isVirtualHostAccess()` 判定为 true 时 `getAdminPrefix()` 返回空字符串，
`buildApiUrl('/traffic/x/sse/stream')` → `http://bifrost.local/api/traffic/x/sse/stream`。
本修复不改这条路径的行为。

## 技术细节

### 关键代码入口

- `web/src/runtime.ts`
  - `buildApiUrl(path)`：核心 URL 构造。desktop shell 下返回
    `http://127.0.0.1:${desktopRuntime.proxyPort}${adminPrefix}/api${suffix}`。
  - `buildBackendUrl(path)`：更底层的 origin + prefix 拼接。
  - `getBackendOrigin()`：desktop 下 `http://127.0.0.1:<port>`，web 下
    `window.location.origin`。
  - `setDesktopProxyPort(port)`：外部（Tauri init 回调）通知 runtime 端口
    变化。
- `web/src/components/TrafficDetail/panes/Messages/index.tsx`
  - `import { buildApiUrl } from "../../../../runtime";`（line ~36）
  - WebSocket frames 流：`new EventSource(
    \`${buildApiUrl(\`/traffic/${recordId}/frames/stream\`)}?x_client_id=${encodeURIComponent(getClientId())}\`)`
    （line ~650）
  - SSE 事件流：`new EventSource(
    \`${buildApiUrl(\`/traffic/${recordId}/sse/stream\`)}?from=begin&batch=1&x_client_id=${encodeURIComponent(getClientId())}\`)`
    （line ~716）
- 相关同类修复模式已存在于：
  - `web/src/stores/usePendingIpTlsStore.ts`（`config/ip-tls/pending/stream`）
  - `web/src/stores/usePendingAuthStore.ts`（`whitelist/pending/stream`）
  - `web/src/pages/AI/AgentChatSection.tsx`（`im-gateway/agent/sessions/events`）
  - `web/src/stores/useReplayStore.ts`（`replay/execute/ws`，同时使用 `buildWsUrl`）

### 修复要点

- 不再在 `Messages/index.tsx` 中出现裸的 `/_bifrost/api/...` 相对路径给
  `EventSource`。
- 保留 query 参数 (`from=begin`, `batch=1`, `x_client_id`) 与原始
  parsing 逻辑。
- `sseEventSourceRef` / `sseSessionKeyRef` / `sseClosedByUsRef` 生命周期
  控制不动，只替换 URL 构造。
- 现有 `flushPending` / `MAX_SSE_EVENTS` 截断 / `lastSseSeqRef` 增量索引
  不动。

### 桌面 runtime 状态源

`desktopRuntime.proxyPort` 由 `loadDesktopRuntimeSnapshot()`（Tauri
`getDesktopRuntime` command）初始化，回落值为 `DEFAULT_BACKEND_PORT = 9900`。
`isDesktopShell()` 通过 `import.meta.env.MODE === 'desktop'` 判定，避免
web 构建误判。

## CLI + Web + Admin API

### CLI

- 本修复只影响前端 WebView，无 CLI 变更。

### Web

- 详情面板的 “Messages” 标签在活跃 WebSocket / SSE 请求上现在实时更新，
  无需手动关闭再打开详情页。
- 手动关闭 SSE 订阅（面板顶部 “Stop” 按钮 → `sseForceClosed`）行为不变。

### Admin API

- 无接口变更。相关端点：
  - `GET /_bifrost/api/traffic/{id}/frames/stream`
    （`crates/bifrost-admin/src/handlers/traffic.rs`）
  - `GET /_bifrost/api/traffic/{id}/sse/stream`
    （同上）

## Sync 边界

- 无。本修复仅影响前端本地 URL 构造。

## Phase 1-4

已落地为单点修复，无多 Phase。

### Phase 1（历史，已完成）

- Messages 面板 SSE / WS 订阅统一走 `buildApiUrl`。
- 与 `usePendingIpTlsStore` / `usePendingAuthStore` / `useReplayStore` 保持
  同一 URL 构造模式。

### Phase 2（防御性维护）

- 建议加一条 lint / eslint rule 或 CI grep：`git grep -n 'new EventSource(\`/_bifrost'`
  出现即失败，避免回归。目前依赖 code review。

## 测试方案

### 静态检查

- `pnpm --dir web exec tsc -b --pretty false` 保证 TypeScript 编译通过。
- `pnpm --dir web lint` 覆盖前端 lint。
- 建议增加自定义脚本 `scripts/check-no-relative-eventsource.sh` 检查
  裸 `/_bifrost/api` 传入 `new EventSource`。

### E2E

- Playwright `web/tests/ui/traffic.spec.ts` 中 SSE 详情实时更新用例：
  - `SSE 详情实时更新`（若已有）验证事件持续到达。
  - `WebSocket frames 详情实时更新` 同理。
- 桌面构建 (Tauri) 需要在 e2e-verify 脚本中开一个 SSE 长请求，验证
  详情面板事件数量随时间递增。

### 手工验证

- 桌面构建启动 Bifrost，用 `curl -N http://127.0.0.1:<proxy_port>/_bifrost/api/traffic/{id}/sse/stream?from=begin&batch=1`
  确认服务端可用。
- 桌面 devtools Network → 打开一个 SSE 请求详情 → 检查
  `EventSource` 请求 URL host = `127.0.0.1:<proxy_port>`。
- 桌面切换 proxy port 后重新打开详情，确认 `EventSource` 命中新端口。

### human_tests

- `human_tests/webui-traffic.md`：SSE 详情面板用例应显式说明“桌面端
  EventSource URL 必须命中 core 端口”。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核 `web/src/components/TrafficDetail/panes/Messages/index.tsx` 中
  所有 `new EventSource(` 调用是否都被 `buildApiUrl` 包裹。
- 复核 `web/src/pages/Traffic/**` 中是否存在其他 `EventSource` / `WebSocket`
  裸路径构造。
- Grep：`git grep -n "new EventSource" web/src`；`git grep -n "new WebSocket" web/src`。

### 第 2 轮

- 桌面端手工验证 SSE + WS 两种详情实时更新场景。
- 复核桌面 proxy port 切换场景：`setDesktopProxyPort` 后重新打开详情。
- 回归 web dev 构建：确认 URL 未变化。

## 风险与决策

- **决策**：修复方式选择“在调用点显式 `buildApiUrl`”，不做全局
  `EventSource` monkey patch，避免影响第三方脚本注入的 EventSource。
- **决策**：不在 `runtime.ts` 提供 `buildEventSourceUrl` / `buildSseUrl`
  等专用 helper；`buildApiUrl` + query 拼接已经足够，且与 fetch 复用同
  一 helper 降低认知成本。
- **风险**：如果有第三方 UI 库或未来新增的详情面板复制旧代码，会再次
  引入相对路径 bug。缓解措施：code review + 可选 lint 规则。
- **风险**：桌面端 CORS / cookie。由于 `EventSource` 强制携带 cookie，
  跨 origin (WebView origin 与 core origin 不同) 时后端需要允许
  credential；`admin` 层已通过 `X-Bifrost-CSRF` + `x_client_id` 处理。
- **风险**：桌面 runtime 端口初始化尚未完成时，`buildBackendUrl` 会返回
  `DEFAULT_BACKEND_PORT = 9900`。若真实端口不是 9900，详情面板挂载时
  可能命中错误端口。前端启动流程会 await `initializeDesktopRuntime()`
  之后才 render Traffic 页面，因此正常路径无问题。
