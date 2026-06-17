# Web Admin Settings Tab Pull

## 背景

- `Settings` 页面此前把多类配置统一挂在 `settings_update` push 通道上。
- 各配置项的更新节奏、服务端生效时机和前端展示时机并不一致，导致 push 快照容易把页面局部状态刷回旧值。
- 为了降低耦合和问题面，需要把 `Settings` 数据流简化为“进入 tab 时主动拉取一次”。

## 方案

- 移除 `Settings` 页面对 `settings_update` 的订阅，不再通过 `settings_scopes` 接收配置快照。
- 每次打开对应 tab 时主动发起一次拉取：
  - `proxy`：代理配置、TLS 配置、代理地址、system proxy、cli proxy、桌面运行时
  - `certificate`：证书信息、代理地址
  - `metrics`：history、app metrics、host metrics
  - `access`：whitelist 状态、pending authorizations
  - `performance`：性能配置
- 各 tab 内的保存/切换仍沿用现有写接口，成功后直接使用接口返回值或显式刷新，不依赖 push 收敛。
- 全局实时通道仍保留给非 Settings 场景；桌面端改端口后只负责重连全局 push，不再给 Settings 配置做同步。

## 影响范围

- `Settings` 页面内的配置类展示统一变为 pull-on-tab-open（见 `web/src/pages/Settings/index.tsx` 中各 `fetch*` 回调与挂载 `useEffect`，以及 `tabs/ProxyTab.tsx` / `AccessControlTab.tsx` / `CertificateTab.tsx` / `PerformanceTab.tsx` 等）。
- `systemProxy` 不再受 settings push 影响，继续走独立接口（`fetchSystemProxy` / `fetchSystemProxyLaunchd`）。
- Access 页顶部 pending 列表改为主动拉取，不再消费 settings push。
- Sync 登录成功后会强制把 `auto_sync` 恢复为开启，避免历史本地配置把“登录即同步”的默认体验保留下来为关闭状态（实现位于 `crates/bifrost-sync/src/manager.rs`，回归用例 `save_token_reenables_auto_sync_after_login`）。
- `CertificateTab` 仍会按需把 `mobile_devices` scope 追加进全局 `pushService` 订阅以驱动手机配对状态，但 Settings 自身的证书/代理配置数据均走 pull，不再依赖 `settings_update` 快照。

## 测试方案

- 打开每个 Settings tab，确认都会触发对应的拉取请求并能展示最新数据。
- 修改 Proxy / TLS / Access / Performance 配置后，切走再切回对应 tab，确认能重新拉到最新状态。
- 刷新 Settings 页面，确认不需要 settings push 也能正常恢复各 tab 数据。
- 先把 `auto_sync` 手动关闭，再完成一次 Sync 登录，确认登录完成后 `auto_sync` 会自动恢复为开启。

## 校验要求

- 先执行 Settings 相关 UI / E2E 验证。
- 再执行项目格式、lint、测试与构建校验。

## 文档更新

- 当前为前端内部数据流调整，不涉及对外 API 文档或 README 变更。

## 状态（截至 2026-06-17）

- Settings 各 tab 的 pull-on-open、`systemProxy` 独立通道、Access pending 主动拉取、登录回填 `auto_sync=true` 均已落地。
- 暂无标注为“(planned, not yet shipped as of 2026-06-17)”的子项。
