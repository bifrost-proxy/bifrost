# Web Admin Settings Tab Pull

## 背景

Settings 页面早期把所有配置项（Proxy、TLS、Certificate、Metrics、Access、Performance、System Proxy、CLI Proxy、桌面运行时）统一挂在 `settings_update` push 通道上。各类配置的更新节奏、服务端生效时机与前端展示时机并不一致：

- 用户提交后接口先返回真实值，随后 push 广播可能推来一个尚未完全收敛的旧快照。
- 页面局部 store 直接消费 push 快照，导致刚保存的字段被“刷回旧值”。
- 多 tab 场景下，即便当前只看某个 tab，也订阅了全量 settings scopes，造成不必要的耦合和噪音。

为降低耦合与问题面，`Settings` 数据流简化为「进入 tab 时主动拉取一次 + 保存后直接消费接口返回值」。全局实时通道保留给非 Settings 场景（例如 `StatusBar` 的 TLS/流量指示、`Traffic` 页协议信息）。

## 用户目标验证清单

### 必须实现

- 打开 `Settings -> Proxy` 时主动拉取 proxy 配置、TLS 配置、代理地址、system proxy、cli proxy、桌面运行时。
- 打开 `Settings -> Certificate` 时主动拉取证书信息与代理地址；`mobile_devices` scope 追加到全局 push 订阅以驱动手机配对状态。
- 打开 `Settings -> Metrics` 时主动拉取 history、app metrics、host metrics。
- 打开 `Settings -> Access` 时主动拉取 whitelist 状态与 pending authorizations（改为主动拉取，不再消费 settings push）。
- 打开 `Settings -> Performance` 时主动拉取性能配置。
- 各 tab 内保存操作使用接口返回值直接更新 store，成功后不需要等待 push 收敛。
- `Settings` 页面不再订阅 `settings_scopes` 中与自身直接相关的 scope，避免快照回刷。
- 桌面端改端口后仅负责重连全局 push，不再向 Settings tab 推送配置。
- Sync 登录成功后强制把 `auto_sync` 恢复为 `true`，避免历史本地配置把“登录即同步”的默认体验保留为 false。

### 必须不破坏

- `useProxyStore` / `useTlsConfigStore` / `useWhitelistStore` / `usePerformanceStore` / `usePendingAuthStore` 的对外接口。
- 全局 `pushService` 通道：`traffic`、`traffic_index`、`certificate`（mobile_devices）等仍按需订阅。
- `StatusBar`、`Traffic` 工具栏共享的 `useProxyStore.systemProxy` 状态。
- CLI Proxy、System Proxy、Certificate 的独立 API 与生效链路。
- 桌面端运行时端口切换后的重连语义。

### 必须真实验证

- 每个 Settings tab 打开时通过 DevTools/网络断言都能看到对应的拉取请求。
- 修改 Proxy / TLS / Access / Performance 配置后切走再切回，`Settings` 能拉到最新状态。
- 刷新页面（F5）后，不依赖 settings push 也能正确展示。
- 先手动关闭 `auto_sync`，完成一次 Sync 登录后自动恢复为 `true`（回归用例 `save_token_reenables_auto_sync_after_login`）。

## 产品语义

### Settings 是「配置视图」，不是「实时快照消费者」

- Settings 的每个 tab 都是一个“进入即校验一次真实态”的视图；push 不再作为主要收敛机制。
- 保存动作以接口返回值为准；返回值即为真实态，前端不再等待 push 二次确认。
- 全局实时能力（例如 StatusBar 的 TLS/流量图标、mobile pairing 状态、trust probe）仍走 push，因为这些是跨页面的共享视图。

### 独立通道的例外

- `systemProxy` 状态改由 `PUT /api/proxy/system` 服务端确认模型返回；参考同批设计文档 `web_admin_system_proxy_pull_reconcile.md`。
- `mobile_devices` scope 由 `CertificateTab` 与 `MobileDeviceTrustPrompt` 通过 `withSettingsScope` 追加到全局 push 订阅。
- `trust_probe` scope 由 `AvailabilityCheckPanel` / `AvailabilityCheckNotificationCenter` 追加到全局 push 订阅。
- `tls_config` scope 由 `StatusBar` 追加到全局 push 订阅，用于状态栏 TLS 图标；`Settings -> Proxy` 自身不再消费此 scope。

## 影响范围

- `web/src/pages/Settings/index.tsx`（1395 行）：新增 `fetchProxySettings`、`fetchTlsConfigData`、`fetchProxyAddressInfo`、`fetchPerformanceConfig`、`fetchCertInfoData`、`fetchWhitelistStatus` 等 useCallback；在挂载与 tab 切换的 `useEffect` 中触发。
- `web/src/pages/Settings/tabs/ProxyTab.tsx` / `AccessControlTab.tsx` / `CertificateTab.tsx` / `PerformanceTab.tsx`：接收父层已拉取到的 store 数据，不再自行订阅 `settings_scopes`。
- `web/src/services/pushService.ts`：`SettingsScope` 联合类型保留（`system_proxy`、`tls_config`、`mobile_devices`、`trust_probe`、`performance`、`whitelist` 等）以兼容服务端 push schema，但 `Settings` 页 default 订阅集合中不再自动加入这些 scope。
- `web/src/components/StatusBar/index.tsx`、`web/src/components/AvailabilityCheckPanel/index.tsx`、`web/src/components/AvailabilityCheckNotificationCenter/index.tsx`、`web/src/components/MobileDeviceTrustPrompt/index.tsx`、`web/src/pages/Settings/tabs/CertificateTab.tsx`：调用 `withSettingsScope(scope)` 显式补 scope 到全局订阅。
- `crates/bifrost-sync/src/manager.rs`：`save_token` 在登录成功后强制 `sync.auto_sync = true`；回归测试 `save_token_reenables_auto_sync_after_login` 与 `tick_marks_ready_without_sync_when_auto_sync_off` 分别覆盖登录回填与关闭态 tick 行为。

## Admin API 与 CLI

本设计只涉及前端消费策略，不改 Admin API/CLI，行为如下：

- `GET /api/proxy` / `PUT /api/proxy`：Proxy 基本配置。
- `GET /api/tls` / `PUT /api/tls`：TLS 拦截配置（含 include/exclude/app_include/ip_include）。
- `GET /api/proxy/addresses`：代理监听地址列表。
- `GET /api/proxy/system` / `PUT /api/proxy/system`：System Proxy（返回值即真实态，见 pull-reconcile 设计）。
- `GET /api/proxy/system/launchd` / `PUT /api/proxy/system/launchd`：System Proxy launchd 守护。
- `GET /api/cli/proxy` / `PUT /api/cli/proxy`：CLI proxy 状态。
- `GET /api/whitelist/status`、`GET /api/whitelist/pending`、`POST /api/whitelist/approve|reject|clear-all`：Access。
- `GET /api/performance` / `PUT /api/performance`：性能配置。
- `GET /api/certificate` / `POST /api/certificate/download` / `GET /api/certificate/mobile-devices`：证书。
- Sync：`POST /api/sync/token`（登录回填 auto_sync=true）。

## Sync 边界

- Settings tab 内的 pull-on-open 不触发 Sync 流量。
- Sync 登录成功后由 `bifrost-sync` 后端主动把 `auto_sync` 覆盖为 `true`，前端不需感知；下一次拉取 Settings/Sync 状态时能自然看到。
- `system_proxy`、`tls_config` 等本地设备语义仍不参与远端 Sync。

## Phase 1：识别 & 拆通道

- 梳理 Settings 页面订阅的所有 `SettingsScope`。
- 找出哪些 scope 是「Settings 自身消费」，哪些是「全局跨页共享」。
- 全局跨页共享的 scope（`mobile_devices`、`trust_probe`、`tls_config` 状态图标）保留在对应组件通过 `withSettingsScope` 显式追加。

## Phase 2：Pull-on-Open

- 每个 tab 挂载时 `useEffect` 触发对应 `fetch*` 回调。
- 保存操作直接使用接口返回值更新 store，不依赖 push 后台收敛。
- `Settings/index.tsx` 移除 `settings_scopes` 中与 Settings 自身直接相关的默认 scope。

## Phase 3：System Proxy 独立通道

- 见 `web_admin_system_proxy_pull_reconcile.md`：`PUT /api/proxy/system` 服务端等待真实态收敛再返回。
- Settings 页不再消费 `system_proxy` push 快照。

## Phase 4：Sync 登录回填 auto_sync

- `crates/bifrost-sync/src/manager.rs` `save_token` 在写入 token 后强制 `sync.auto_sync = true`。
- 单测 `save_token_reenables_auto_sync_after_login`（约第 1629 行）覆盖：预置 `auto_sync=false` → 调用 `save_token` → 断言 `auto_sync=true`。

## 测试方案

### 前端手工/E2E

- `pnpm -C web test:ui -- admin-settings.spec.ts` 覆盖：
  - TLS 域名白名单增删（含重连提示，参考 `web_admin_tls_whitelist_restart_notice.md`）。
  - Performance 配置调整、Whitelist 添加/删除、pending authorizations approve/reject。
- 手工验证：
  - 打开每个 Settings tab，DevTools 观察对应 GET 请求发起。
  - 切走后再切回，能重新拉取。
  - F5 刷新，页面不需要 push 也能正常渲染。

### 后端单元测试

- `crates/bifrost-sync/src/manager.rs`：
  - `save_token_reenables_auto_sync_after_login`（1629）：登录成功强制回填 `auto_sync=true`。
  - `tick_marks_ready_without_sync_when_auto_sync_off`（2468）：`auto_sync=false` 时 tick 不触发 sync，仅标记 ready。
  - 相关行：253-256（config.sync.auto_sync 读取）、299/332（默认值 Some(true)）、615/667（tick 判断）、1629-1676、2468/2629-2661。

### 环境约束

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `pnpm -C web lint` / `pnpm -C web test:ui`
- `cargo test -p bifrost-sync save_token_reenables_auto_sync_after_login`
- `cargo test -p bifrost-sync tick_marks_ready_without_sync_when_auto_sync_off`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定：不跑 `make coverage` / `make coverage-unit`。

## 校验要求

- 先执行 Settings 相关 UI / E2E 验证。
- 再执行 `rust-project-validate` 规定的格式、lint、测试与构建校验。

## 文档更新

- 当前为前端内部数据流调整，不涉及对外 API 文档或 README 变更。
- 与之相关的补充文档：`web_admin_system_proxy_pull_reconcile.md`、`web_admin_tls_whitelist_restart_notice.md`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：每个 tab 打开时都会拉取对应数据；保存操作不再依赖 push 收敛；`auto_sync` 登录回填生效。
- 复核 diff：`Settings/index.tsx` 是否遗漏某个 tab 的 fetch 挂载；`SettingsScope` 联合类型是否与服务端 schema 兼容；全局订阅 scope 是否仍能驱动 StatusBar / MobileDeviceTrustPrompt / trust_probe。
- 重点 review：切走再切回是否会造成 request storm；同时打开多 tab 是否会重复请求；错误分支是否有 fallback。
- 复测：`admin-settings.spec.ts`、`bifrost-sync` 单测。

### 第 2 轮

- 复核第 1 轮发现问题修复；`git status --short` 与 `git diff` 覆盖前端与 sync 模块。
- 重点 review：桌面端切端口后是否只重连全局 push、不推 settings；Access 页 pending 主动拉取是否覆盖 approve/reject/clear-all 的 UI 收敛。
- 复测：失败路径重跑，必要时补 mac 桌面端手测。

## 实现状态（截至 2026-06-17）

- Settings 各 tab 的 pull-on-open 已落地：`fetchProxySettings`（234）、`fetchTlsConfigData`（245）、`fetchPerformanceConfig`（277）、`fetchProxyAddressInfo`（346）等 useCallback 在 `Settings/index.tsx` 的挂载 `useEffect` 与 tab 切换 `useEffect` 中触发。
- `systemProxy` 走独立通道（`fetchSystemProxy` / `fetchSystemProxyLaunchd`），不再消费 settings push；详见 pull-reconcile 设计。
- Access 页 pending 列表通过 `usePendingAuthStore.fetchPending` 主动拉取。
- 登录回填 `auto_sync=true` 已在 `bifrost-sync` 落地，含回归测试。
- 无标注为「(planned, not yet shipped as of 2026-06-17)」的子项。

## 风险与决策

- **切走再切回带来的重复 GET**：可接受；每个 fetch 是幂等只读接口，负载低于 push 快照回刷带来的 UX 抖动。若某个 tab 出现明显性能问题，可加最短刷新间隔阈值。
- **兼容旧客户端**：`SettingsScope` 联合类型继续包含 `system_proxy` 等旧值，避免旧后端 push 反序列化失败。
- **Sync 回填 auto_sync**：若用户业务场景确实需要“登录不同步”，需通过后续 UI 显式关闭；本设计以“登录即同步”为默认体验。
- **多 tab 共享 store**：Traffic 工具栏、StatusBar 与 Settings 共享 `useProxyStore`，任何一处触发 fetch 都会广播到 store，从而实现跨页面一致。
