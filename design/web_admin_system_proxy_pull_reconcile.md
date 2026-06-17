# Web Admin System Proxy Pull Reconcile

## 背景

- Settings 页面此前将 `system_proxy` 纳入 `settings_scopes`，依赖 push 首次快照和后续广播同步状态。
- `systemProxy` 的真实状态来自操作系统代理配置，启停过程中可能存在异步收敛窗口。
- 当前服务端 push 构建 `system_proxy` 快照时，会读取 `SystemProxyManager::get_current()` 的系统真实状态，而不是前端刚提交的目标状态。

## 问题

- 用户在管理端切换 `systemProxy` 后，前端先收到 `PUT /api/proxy/system` 的成功响应。
- 随后配置变更触发 push 广播；若操作系统尚未完成切换，push 会把旧状态再次推到前端。
- 前端把旧快照直接写回 store，导致开关被“刷回去”，表现为操作失败或状态抖动。

## 方案

- 将 `system_proxy` 从 Settings 页的 `settings_scopes` 订阅中移除，不再通过 settings push 收敛。
- `PUT /api/proxy/system` 改为服务端确认模型：
  - 执行启用/关闭系统代理；
  - 在服务端短暂等待并回读系统真实代理状态；
  - 仅将真实状态返回给前端，而不是直接回显请求目标值。
- `useProxyStore.toggleSystemProxy` 保持轻量：
  - 请求 `PUT /api/proxy/system`；
  - 直接消费服务端返回的真实状态；
  - 若返回值仍未达到目标态，则提示未完成收敛，但不发起高频补拉。

## 影响范围

- Settings 页不再消费 `system_proxy` 的 push 快照。
- Traffic 页工具栏、StatusBar 仍复用同一个 `useProxyStore`，因此会共享切换后的拉取收敛结果。
- 其他 settings scope 继续保持 push 模型，不受本次调整影响。

## 测试方案

- 打开 Settings -> Proxy，切换 System Proxy，确认开关不会被旧 push 快照刷回。
- 切换时确认浏览器不会高频请求 `GET /api/proxy/system`。
- 确认 `PUT /api/proxy/system` 会等待真实状态收敛后再返回，返回值与系统实际状态一致。
- 验证 Traffic 页工具栏与 StatusBar 能同步看到收敛后的 `systemProxy` 状态。
- 执行相关 UI E2E 场景与项目校验流程。

## 校验要求

- 先执行与管理端 Settings 相关的 E2E / UI 验证。
- 再执行 `rust-project-validate` 规定的格式、lint、测试与构建校验。

## 文档更新

- 当前仅为前端状态同步策略调整，不涉及对外 API/README 变更。

## 实现现状（截至 2026-06-17）

- 后端 `crates/bifrost-admin/src/handlers/proxy.rs` 中 `set_system_proxy` 在 enable/disable 后通过 `wait_for_system_proxy_status` 以 `SYSTEM_PROXY_VERIFY_DELAYS_MS = [200, 400, 800, 1600]` ms 的递增退避读取真实状态，再返回给前端。
- `read_system_proxy_status` 使用 `target_matches` 与 `SystemProxyManager::any_service_proxy_matches` 判定 `managed_by_bifrost`；`matches_expected_system_proxy` 把 disable 收敛条件放宽为「未启用 或 非 Bifrost 管理」，允许外部代理保留。
- 持久化时仅写入 `enabled_by_bifrost = status.enabled && status.managed_by_bifrost`，避免用户手动接管外部代理时被错误标记为 Bifrost 拥有。
- 前端 `web/src/stores/useProxyStore.ts` 的 `toggleSystemProxy` 直接消费 `PUT /api/proxy/system` 返回的真实状态，未匹配时仅在 store 中写入 `error` 文案，不发起额外重拉。
- `web/src/services/pushService.ts` 的 `SettingsScope` 联合类型仍保留 `'system_proxy'`（兼容服务端 push schema），但 `web/src/pages/Settings/index.tsx` 已不再将其加入 `settings_scopes` 订阅；Settings 页通过 `fetchSystemProxy` 主动拉取，StatusBar / Traffic 页共享同一 `useProxyStore`。
