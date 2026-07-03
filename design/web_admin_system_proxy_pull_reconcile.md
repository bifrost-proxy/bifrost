# Web Admin System Proxy Pull Reconcile

## 背景

Settings 页面早期把 `system_proxy` 纳入 `settings_scopes` push 订阅，依赖首次快照和后续广播同步状态。但 `systemProxy` 的真实状态来自操作系统代理配置，启停过程中存在异步收敛窗口：

- 用户在管理端切换 `systemProxy` 后，前端先收到 `PUT /api/proxy/system` 的成功响应。
- 随后配置变更触发服务端 push 广播；服务端构建 `system_proxy` 快照时读取的是 `SystemProxyManager::get_current()` 的系统真实状态。
- 若操作系统尚未完成切换，push 会把旧状态再次推到前端；前端把旧快照直接写回 `useProxyStore`，导致开关被“刷回去”，表现为操作失败或状态抖动。

方案：拆掉 push 通道，改为**服务端确认模型**：`PUT /api/proxy/system` 由服务端在启用/关闭后短暂等待并回读系统真实状态，仅将真实状态返回给前端；前端不再依赖 push 收敛。

## 用户目标验证清单

### 必须实现

- 用户在 Settings -> Proxy 或 StatusBar 切换 System Proxy 后，UI 状态不再被 push 快照刷回。
- `PUT /api/proxy/system` 服务端等待真实态收敛后再返回，返回值即真实态。
- 前端 `useProxyStore.toggleSystemProxy` 直接消费返回值，未匹配目标时仅提示未收敛，不发起高频补拉。
- Settings 页 `SettingsScope` 订阅集合中不再自动包含 `system_proxy`。
- StatusBar、Traffic 工具栏共享 `useProxyStore.systemProxy` 状态；任一处触发切换后全局一致。
- 关闭系统代理的收敛条件放宽为「未启用 或 非 Bifrost 管理」，允许外部代理保留。
- `enabled_by_bifrost = status.enabled && status.managed_by_bifrost`；用户手动接管外部代理时不会被错误标记为 Bifrost 拥有。

### 必须不破坏

- 系统代理启停 API 的行为、异常处理与回滚：切换失败时恢复 `system_proxy_runtime_desired_enabled` 到 previous。
- launchd 守护安装/卸载：`start_system_proxy_lifecycle_helper_after_runtime_enable` 与 `stop_system_proxy_lifecycle_helper_after_runtime_disable` 正确触发。
- SystemProxyConfig 持久化：`config_manager.update_system_proxy_config` 只有在真实收敛后才写入。
- SystemProxyManager 的 macOS / Windows 平台差异实现。

### 必须真实验证

- 打开 Settings -> Proxy 切换 System Proxy，开关不会被旧 push 快照刷回。
- 切换时浏览器不会高频请求 `GET /api/proxy/system`。
- `PUT /api/proxy/system` 等待真实状态收敛后再返回，返回值与系统实际状态一致。
- Traffic 页工具栏与 StatusBar 能同步看到收敛后的 `systemProxy` 状态。
- 手动在系统偏好里改代理后，前端下一次 `fetchSystemProxy` 能拉到真实态。

## 产品语义

### System Proxy 是「设备级 OS 状态」

- 与 rules/values/scripts 等 Bifrost 内部数据不同，system proxy 是操作系统级设置，Bifrost 只是一个「意图 + 执行 + 回读校验」的编排层。
- 状态权威来源永远是 OS；服务端读回后返回给前端。
- 单一入口：`PUT /api/proxy/system` 请求 → 后端调用 `SystemProxyManager` 修改 → 退避回读 → 返回真实态。

### 服务端确认模型

`crates/bifrost-admin/src/handlers/proxy.rs` 中：

- `SYSTEM_PROXY_VERIFY_DELAYS_MS: [u64; 4] = [200, 400, 800, 1600]`（约第 139 行）：递增退避，最多 3 秒。
- `wait_for_system_proxy_status(expected_enabled, host, port)`（约 252 行）：先读一次，命中则立即返回；否则按 `SYSTEM_PROXY_VERIFY_DELAYS_MS` 依次 `sleep + read`，直到命中或用尽全部退避。
- `read_system_proxy_status(expected_host, expected_port)`（约 227 行）：使用 `target_matches` 与 `SystemProxyManager::any_service_proxy_matches` 判定 `managed_by_bifrost`。
- `matches_expected_system_proxy(latest, expected_enabled, host, port)`（约 273 行）：把 disable 收敛条件放宽为「未启用 或 非 Bifrost 管理」，允许外部代理保留。
- `set_system_proxy(req, state)`（约 286 行）：执行启停 → 等待收敛 → 返回真实态 → 持久化 `enabled_by_bifrost = status.enabled && status.managed_by_bifrost`。

### 前端消费

`web/src/stores/useProxyStore.ts` 中：

- `toggleSystemProxy(enabled)`（约第 72 行）：请求 `PUT /api/proxy/system`；直接消费返回值；若未达到目标，仅在 store 中写入 `error` 文案，不重复补拉。
- `fetchSystemProxy()`（约第 45 行）：拉取当前真实态，用于 Settings 页 tab 打开与页面刷新场景。
- `fetchSystemProxyLaunchd()`（约第 54 行）：拉取 launchd 守护状态。
- 辅助函数（约 131-139 行）：`isSystemProxyEffectivelyDisabled(status)` 和 `isSystemProxyEnabledByBifrost(status)`，前者返回 `!status.enabled || status.managed_by_bifrost === false`，后者返回 `status.enabled && status.managed_by_bifrost !== false`。

## Admin API

- `GET /api/proxy/system` → `get_system_proxy_status`（proxy.rs:203）
  - 返回 `{ enabled, managed_by_bifrost, host, port }` 等真实状态字段。
- `PUT /api/proxy/system` → `set_system_proxy`（proxy.rs:286）
  - Request：`{ enabled: bool }`（可扩展 host/port）。
  - Behavior：调用 `SystemProxyManager.enable/disable` → `wait_for_system_proxy_status` → 返回真实态。
  - 副作用：持久化 `SystemProxyConfig`（含 `enabled_by_bifrost`）、启停 launchd lifecycle helper、失败回滚 desired enabled。
- `GET /api/proxy/system/support` → `get_system_proxy_support`（proxy.rs:470）：平台能力探测。
- `GET /api/proxy/system/launchd` / `PUT /api/proxy/system/launchd` → `get_system_proxy_launchd_status` / `set_system_proxy_launchd`（proxy.rs:478 / 511）：macOS launchd 守护。

## Web UI 交互

### Settings -> Proxy 页

- 「System Proxy」开关直接由 `useProxyStore.systemProxy` 驱动。
- 切换开关 → 调用 `toggleSystemProxy(target)` → 等待接口返回真实态 → 更新 store → UI 反映最终态。
- 若返回未达到目标（例如系统被其他进程抢占），显示 `error` 文案，用户可再次尝试。
- 页面不再订阅 `system_proxy` push scope；tab 打开时通过 `fetchSystemProxy()` 主动拉一次。

### StatusBar / Traffic 页

- StatusBar 与 Traffic 工具栏共享同一个 `useProxyStore`，因此 Settings 页切换后全局同步。
- StatusBar 显示的 System Proxy 图标只反映 store 中的真实态，不再依赖 push。

## CLI

- 现有 `bifrost proxy system enable/disable/status` 命令继续走同一 Admin API 与 SystemProxyManager；不需要 CLI 侧改动。

## Sync 边界

- System Proxy 是设备级 OS 状态，不参与 Sync；本设计不改变这一点。
- 多设备场景下每台设备独立管理各自的 system proxy 状态。

## 影响范围

- **前端**：
  - `web/src/pages/Settings/index.tsx`：`SettingsScope` 集合中不再包含 `system_proxy`；tab 打开时调用 `fetchSystemProxy`。
  - `web/src/stores/useProxyStore.ts`：`toggleSystemProxy` 直接消费接口返回值；`fetchSystemProxy` / `fetchSystemProxyLaunchd` 提供主动拉取入口。
  - `web/src/services/pushService.ts`：`SettingsScope` 联合类型仍保留 `'system_proxy'` 以兼容服务端 push schema，但 Settings 页不再默认订阅。
  - `web/src/components/StatusBar/index.tsx`、`web/src/pages/Traffic` 工具栏：共享 store，不需改动。
- **后端**：
  - `crates/bifrost-admin/src/handlers/proxy.rs`：`SYSTEM_PROXY_VERIFY_DELAYS_MS`、`wait_for_system_proxy_status`、`read_system_proxy_status`、`matches_expected_system_proxy`、`set_system_proxy`。
- **配置持久化**：`ConfigManager::update_system_proxy_config` 保持接口不变；`enabled_by_bifrost` 语义按新规则写入。

## Phase 1：后端服务端确认模型

- 引入 `SYSTEM_PROXY_VERIFY_DELAYS_MS` 常量与 `wait_for_system_proxy_status`。
- `set_system_proxy` 在启停后调用 `wait_for_system_proxy_status` 收敛真实态。
- `matches_expected_system_proxy` 放宽 disable 收敛条件。
- 持久化 `enabled_by_bifrost = status.enabled && status.managed_by_bifrost`。

## Phase 2：前端消费真实态

- `toggleSystemProxy` 直接消费接口返回值；未匹配时仅写入 error，不高频补拉。
- `fetchSystemProxy` / `fetchSystemProxyLaunchd` 提供 tab 打开与页面刷新场景的主动拉取。

## Phase 3：拆除 push 通道

- Settings 页 `SettingsScope` 集合中删除 `system_proxy`。
- `pushService.SettingsScope` 联合类型仍保留 `'system_proxy'` 以兼容服务端 push schema，但客户端不订阅。
- StatusBar / Traffic 工具栏保持共享 store，因此全局同步。

## Phase 4：观测与手工验证

- 手工验证 macOS / Windows 下切换后 UI 不被刷回。
- 观察 `wait_for_system_proxy_status` 的最长等待时间是否可接受（≤ 3 秒）。
- 若发现某些环境需要更长收敛时间，可扩展 `SYSTEM_PROXY_VERIFY_DELAYS_MS`。

## 测试方案

### 后端单元测试

- `crates/bifrost-admin/src/handlers/proxy.rs`：
  - `matches_expected_system_proxy_disable_allows_external_proxy`：断言 disable 收敛条件放宽为「未启用 或 非 Bifrost 管理」。
  - `wait_for_system_proxy_status_converges_within_backoff`：mock `read_system_proxy_status` 在第 3 次返回目标态，验证不会超时。
  - `set_system_proxy_persists_enabled_by_bifrost_correctly`：断言 `enabled_by_bifrost` 计算正确。

### 前端 E2E

- `web/tests/ui/admin-settings.spec.ts`：
  - System Proxy toggle 场景（Host Integration 套件，默认不进 CI）：切换后 UI 不被 push 刷回。
  - 通过 `fetchSystemProxy` 拉到的初始态与 UI 一致。

### 真实场景手工验证

- macOS：Settings -> Proxy 切 System Proxy on/off，观察 UI 无抖动；`launchd list | grep bifrost` 验证守护安装/卸载。
- Windows：使用 `netsh winhttp show proxy` 或注册表验证。
- 手动在系统偏好里改代理后回到 UI，`fetchSystemProxy` 能拉到真实态。

### 环境约束

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`（System Proxy 手工测试需另开非 --no-system-proxy 会话）。

### 覆盖率与项目校验

- `pnpm -C web lint`
- `pnpm -C web test:ui -- admin-settings.spec.ts`
- `cargo test -p bifrost-admin proxy`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定：不跑 `make coverage`。

## 校验要求

- 先执行与管理端 Settings 相关的 E2E / UI 验证。
- 再执行 `rust-project-validate` 规定的格式、lint、测试与构建校验。

## 文档更新

- 当前仅为前端状态同步策略调整，不涉及对外 API/README 变更。
- 与之相关：`web_admin_settings_tab_pull.md`（Settings 数据流总览）。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：切换 System Proxy UI 不被刷回；服务端等待真实态收敛；`enabled_by_bifrost` 语义正确。
- 复核 diff：`SYSTEM_PROXY_VERIFY_DELAYS_MS`、`wait_for_system_proxy_status`、`matches_expected_system_proxy`、`set_system_proxy` 的实现；前端 `toggleSystemProxy` 的返回值消费。
- 重点 review：退避总时长是否可控；异常路径回滚 `desired_enabled` 是否正确；launchd 守护启停触发点是否与 runtime enable 一致。
- 复测：后端单测、手工切换。

### 第 2 轮

- 复核第 1 轮问题修复；`git status --short` 与 `git diff` 覆盖 admin、web 与 sync。
- 重点 review：外部代理场景（用户手动接管）下 `enabled_by_bifrost` 是否正确置 false；用户在系统偏好手动改后 UI 是否能通过 `fetchSystemProxy` 恢复真实态；多 tab 场景下 store 广播是否一致。
- 复测：失败路径重跑，必要时补 macOS/Windows 手测。

## 实现现状（截至 2026-06-17）

- 后端 `crates/bifrost-admin/src/handlers/proxy.rs`：
  - `SYSTEM_PROXY_VERIFY_DELAYS_MS = [200, 400, 800, 1600]`（第 139 行）。
  - `wait_for_system_proxy_status`（第 252 行）：首读命中则直接返回，否则按退避 sleep + read。
  - `read_system_proxy_status`（第 227 行）：判定 `managed_by_bifrost`。
  - `matches_expected_system_proxy`（第 273 行）：disable 条件放宽为「未启用 或 非 Bifrost 管理」。
  - `set_system_proxy`（第 286 行）：执行 → 等待 → 持久化 `enabled_by_bifrost = status.enabled && status.managed_by_bifrost`；失败回滚 `desired_enabled`；成功后 `start_system_proxy_lifecycle_helper_after_runtime_enable` 或 `stop_system_proxy_lifecycle_helper_after_runtime_disable`。
- 前端 `web/src/stores/useProxyStore.ts`：
  - `toggleSystemProxy`（第 72 行）：请求 `PUT /api/proxy/system`；消费返回真实态；未匹配仅写 error。
  - `fetchSystemProxy`（第 45 行）与 `fetchSystemProxyLaunchd`（第 54 行）用于 tab 打开与刷新。
- `web/src/services/pushService.ts`（第 81/98/238/269-270/406-407 行）：`SettingsScope` 联合类型仍保留 `'system_proxy'`；但 `web/src/pages/Settings/index.tsx` 已不在 default 订阅中加入该 scope。
- Settings 页通过 `fetchSystemProxy` 主动拉取；StatusBar、Traffic 页共享同一 `useProxyStore`，跨页面一致。

## 风险与决策

- **退避总时长**：`[200, 400, 800, 1600]` 累计约 3 秒；对绝大多数环境足够，最坏情况仍在用户可接受范围内。若发现某些机器需要更长，可扩展常量。
- **外部代理保留**：disable 收敛条件放宽为「未启用 或 非 Bifrost 管理」，避免用户在关闭 Bifrost 代理后 UI 一直停留在 pending。
- **launchd 守护**：runtime enable/disable 后必须同步启停 helper，避免用户下次登录时 launchd 恢复旧状态。
- **兼容旧客户端**：`SettingsScope` 联合类型继续保留 `'system_proxy'`，避免旧服务端 push 反序列化失败。
- **多设备**：System Proxy 是设备级 OS 状态，不参与 Sync；本设计不改变这一点。
