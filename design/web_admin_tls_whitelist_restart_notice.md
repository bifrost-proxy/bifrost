# 管理端 TLS 白名单变更后的重连提示

## 背景

Bifrost 的 TLS 拦截白名单支持三类：

- 域名白名单：`intercept_include`
- 应用白名单：`app_intercept_include`
- 客户端 IP 白名单：`ip_intercept_include`

当用户在管理端把域名 / 应用 / 客户端 IP 加入白名单后，只有**新建的连接**才能被拦截；对目标应用当前已经通过 CONNECT 建立的旧连接（例如 keep-alive、SSE、WebSocket、gRPC 长连接、系统级持久连接）不会被立即重新协商 TLS，因此“已加白但看不到解密流量”是新用户常见的 confusion。

原设计只覆盖前两类，`ip_intercept_include` 是同期落地的扩展。所有加白入口都需要在成功 toast 中额外提醒用户「重启目标应用并重新打开目标域名」，避免用户误以为拦截失败。

## 用户目标验证清单

### 必须实现

- Settings -> Proxy -> TLS Interception Patterns 中新增或移除域名/应用白名单成功后，toast 追加统一的重连提醒。
- Network -> 详情 `TunnelInterceptActions` 的三类快捷加白按钮（`Intercept this domain` / `Intercept this app` / `Intercept this client`）成功后追加同一提醒。
- Network -> 流量表格右键 `TrafficContextMenu` 的三类快捷加白操作成功后追加同一提醒。
- 三类白名单（域名/应用/客户端 IP）共用同一段文案：`Restart the target app and reopen the target domain to establish a new connection.`
- 提示语从统一的 `tlsInterceptionNotice.ts` 常量导出，避免文案漂移。
- 提示 duration 与其他成功提示协调（当前 5s）。
- 失败提示保持与其他 TLS 设置项一致，不改变错误分支的语义。

### 必须不破坏

- `updateTlsConfig` 接口调用与状态更新链路。
- Settings 页 TLS 配置的现有保存逻辑与 store 收敛。
- TunnelInterceptActions 和 TrafficContextMenu 的原有按钮布局、快捷键与业务流程。
- Reconnect notice 只是 UX 层增强，不改变白名单命中语义、匹配优先级或拦截行为。

### 必须真实验证

- Playwright：Settings 域名白名单新增触发 toast 包含重连提示。
- Playwright：TunnelInterceptActions 的 `Intercept this app` / `Intercept this client` 触发 toast 包含重连提示；配置接口写入 `app_intercept_include` / `ip_intercept_include`。
- 手工验证：TunnelInterceptActions 的 `Intercept this domain`、TrafficContextMenu 三类操作、Settings 应用 / IP 白名单也能出现重连提示。

## 产品语义

### 白名单变更 = 意图变更，不等于连接变更

- 加入白名单：新建 CONNECT 请求会被拦截解密；已建立的旧连接维持原状。
- 移出白名单：新建 CONNECT 请求不再被解密；已建立的解密连接沿用旧握手。
- Bifrost 不主动主动 kill 已有连接（避免用户不可控的中断风险）。
- 因此「加白后需要重启 target 或重新打开 URL」是必须显式告知用户的产品事实。

### 文案统一

Reconnect notice 使用同一段英文文案，避免各入口出现同义不同表述：

```
Restart the target app and reopen the target domain to establish a new connection.
```

所有 toast 使用 `antd` 的 `message.success`，格式为：

```
${content}. ${TLS_RECONNECT_NOTICE}
```

## 实现

### 统一 helper

`web/src/utils/tlsInterceptionNotice.ts`（11 行）：

```ts
import { message } from "antd";

export const TLS_RECONNECT_NOTICE =
  "Restart the target app and reopen the target domain to establish a new connection.";

export function showTlsWhitelistChangeSuccess(content: string) {
  message.success({
    content: `${content}. ${TLS_RECONNECT_NOTICE}`,
    duration: 5,
  });
}
```

### 三大入口调用点

1. `web/src/pages/Settings/index.tsx`
   - 第 545 行：`showTlsWhitelistChangeSuccess("Added ${pattern} to include list")`（域名 include 新增）。
   - 第 561 行：`showTlsWhitelistChangeSuccess("Removed ${pattern} from include list")`（域名 include 移除）。
   - 第 629 行：`showTlsWhitelistChangeSuccess("Added ${pattern} to app include list")`（应用白名单新增）。
   - 第 645 行：`showTlsWhitelistChangeSuccess("Removed ${pattern} from app include list")`（应用白名单移除）。
2. `web/src/components/TrafficDetail/TunnelInterceptActions.tsx`
   - 第 78 行：`Intercept this domain` → `Added "${host}" to intercept list`。
   - 第 100 行：`Intercept this app` → `Added "${clientApp}" to app intercept list`。
   - 第 122 行：`Intercept this client` → `Added "${clientIp}" to IP intercept list`。
   - 第 205 行：面板内联展示 `TLS_RECONNECT_NOTICE` 文本，用于长驻提示。
3. `web/src/components/TrafficTable/TrafficContextMenu.tsx`
   - 第 219 行：右键 `Intercept this domain` → `Added "${host}" to intercept list`。
   - 第 246 行：右键 `Intercept this app` → `Added "${app}" to app intercept list`。
   - 第 273 行：右键 `Intercept this client` → `Added "${ip}" to IP intercept list`。

### 依赖项

- `antd` 的 `message.success` 能力。
- 现有 TLS 配置更新接口 `updateTlsConfig`（Settings/index.tsx 中通过 `useTlsConfigStore` 调用）。
- `TunnelInterceptActions` / `TrafficContextMenu` 现有的 fetch/更新逻辑。

## 影响范围

- 新增：`web/src/utils/tlsInterceptionNotice.ts`。
- 修改：
  - `web/src/pages/Settings/index.tsx`：4 处 toast 迁移至 helper。
  - `web/src/components/TrafficDetail/TunnelInterceptActions.tsx`：3 处 toast + 1 处面板文案。
  - `web/src/components/TrafficTable/TrafficContextMenu.tsx`：3 处 toast。
- E2E：
  - `web/tests/ui/admin-settings.spec.ts`：断言 Settings 域名白名单新增后提示包含重连文案（第 273 行）。
  - `web/tests/ui/traffic-push.spec.ts`：`Intercept this app` / `Intercept this client` 按钮触发重连提醒，并校验配置接口写入 `app_intercept_include` / `ip_intercept_include`（约第 582-609 行）。

## 非目标

- 不改变白名单命中语义、匹配优先级、拦截行为。
- 不主动 kill 已有解密/未解密连接。
- 不新增“重启 target 应用”的自动化能力（跨平台风险太高，保持提示即可）。
- 不为 TLS 黑名单单独出提示（如后续扩展，应复用同一 helper）。

## Sync 边界

- TLS 白名单配置本身是设备级配置，是否参与 Sync 由 TLS Config 设计决定；本设计只涉及 UX 提示。

## Phase 1：抽取统一 helper

- 新增 `tlsInterceptionNotice.ts`，导出 `TLS_RECONNECT_NOTICE` 常量和 `showTlsWhitelistChangeSuccess(content)`。
- 迁移 Settings 页 4 处 toast 到 helper。

## Phase 2：Traffic 详情与右键菜单接入

- `TunnelInterceptActions` 三类按钮成功后调用 helper；面板内联 `TLS_RECONNECT_NOTICE` 展示长驻提示。
- `TrafficContextMenu` 三类右键操作成功后调用 helper。

## Phase 3：E2E 覆盖

- 断言 Settings 与 Traffic 场景 toast 文案包含重连提示。
- 校验配置接口正确写入 `intercept_include` / `app_intercept_include` / `ip_intercept_include`。

## Phase 4：补齐剩余场景

- （planned, not yet shipped as of 2026-06-17）尚未对 `TunnelInterceptActions.Intercept this domain` 以及 `TrafficContextMenu` 三类操作单独补充重连提醒的 spec 断言；后续可在 `traffic-push.spec.ts` 或 `admin-settings.spec.ts` 中扩展。

## 测试方案（含 e2e）

### Playwright E2E（已落地）

- `web/tests/ui/admin-settings.spec.ts`（约第 273 行）：Settings 新增 TLS 域名白名单，断言 toast 包含 `Restart the target app and reopen the target domain to establish a new connection.`
- `web/tests/ui/traffic-push.spec.ts`（约第 582-609 行）：
  - `Intercept this app` 按钮：点击后校验 `PUT /api/tls` 写入 `app_intercept_include`，并断言 toast 包含重连文案。
  - `Intercept this client` 按钮：点击后校验写入 `ip_intercept_include`，并断言 toast 文案。

### Planned E2E

- （planned, not yet shipped as of 2026-06-17）
  - `TunnelInterceptActions.Intercept this domain` 单独断言。
  - `TrafficContextMenu` 三类操作单独断言。

### 手工验证

- Settings -> Proxy -> TLS Interception Patterns：分别新增/移除域名、应用两类，均能看到重连提示。
- Network 详情面板：三类按钮点击后成功提示都包含重连文案。
- Network 表格右键：三类操作成功提示都包含重连文案。
- 加白后再次通过代理请求 target，如果 target 是浏览器 tab，需要手动刷新才能看到解密流量，符合提示语义。

### 环境约束

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

## 校验要求（含 rust-project-validate）

- 执行与管理端设置页相关的 UI E2E，确认提示展示正确且不影响原有保存逻辑。
- 在 E2E 完成后执行：
  - `pnpm -C web lint`
  - `pnpm -C web test:ui -- admin-settings.spec.ts traffic-push.spec.ts`
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - 按改动范围执行 `cargo test` 与 `cargo build`。

本机 no-local-coverage 约定：不跑 `make coverage`。

## 文档更新要求

- 当前变更仅涉及交互提示与测试说明，无需更新 `README.md`。
- 若后续把同类提示扩展到 TLS 黑名单或其他配置项，应同步补充到管理端 UI E2E 说明。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：三类白名单入口都追加重连提示；文案统一。
- 复核 diff：`tlsInterceptionNotice.ts` 是否被所有相关 toast 消费；`updateTlsConfig` 调用未被绕过；`TunnelInterceptActions` / `TrafficContextMenu` 保持原按钮布局。
- 重点 review：错误分支是否仍走原有 `message.error`；toast duration 是否统一；文案是否国际化（当前仅英文，后续如有 i18n 需求应改为 `t('tls.reconnectNotice')`）。
- 复测：`admin-settings.spec.ts`、`traffic-push.spec.ts`。

### 第 2 轮

- 复核第 1 轮问题修复；`git status --short` 与 `git diff` 覆盖 helper、Settings、TrafficDetail、TrafficTable、tests。
- 重点 review：补齐 planned 断言的可行性；`Intercept this domain` 与 `TrafficContextMenu` 三类操作是否需要额外 mock。
- 复测：失败路径重跑，必要时补 mac 桌面端手测。

## 风险与决策

- **文案统一**：所有入口共用同一段文案；国际化在下一版可以按 key 化重构（例如 `tls.whitelistChangeReconnectNotice`）。
- **不主动 kill 连接**：保持“提示而不强制”是安全默认；跨平台 kill 长连接会带来不可控风险，短期不引入。
- **面板长驻提示**：`TunnelInterceptActions` 在面板内内联展示 `TLS_RECONNECT_NOTICE` 文本，帮助用户在没看 toast 的情况下也能看到；不影响其他布局。
- **TLS 黑名单扩展**：若后续扩展到 exclude 场景（意图相反），需要单独提示语义，不能简单复用同一文案；应新增 `showTlsBlacklistChangeSuccess` 与新常量。
