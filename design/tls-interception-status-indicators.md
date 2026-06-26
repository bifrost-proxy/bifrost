# HTTPS Interception 状态可见性

## 功能模块说明

全局 HTTPS Interception 是高影响开关。启用后，Web UI 底部状态栏和 tray 必须给出持续可见的状态提示，避免用户忘记当前 TLS 解包处于全面打开状态。

## 实现逻辑

- Web UI 状态栏从 `/api/config/tls` 读取 `enable_tls_interception`，并订阅 `settings_update: tls_config` 实时更新。
- 当全局 HTTPS Interception 启用时，状态栏展示 `TLS: Full On`，圆点和文字启用跳动/脉冲动画；关闭但存在 domain/app/IP allow list 时展示 `TLS: Scoped`；完全关闭时展示 `TLS: Off`。
- 状态栏 TLS 状态支持键盘和鼠标进入 `Settings ? tab=tls`，便于用户立即关闭或调整范围。
- Tray helper 轮询 Admin API 的 `/api/config/tls`，只在下拉菜单中展示 `TLS Interception: On/Off` 状态和切换项；tray 顶部图标和 macOS 系统状态标题不展示 TLS 角标，避免刷新延迟造成误导。
- Tray 下拉菜单中的 `System Proxy` 保持原有单行勾选项；`TLS Interception: On/Off` 作为独立顶层行展示在 `System Proxy` 下方，并可直接切换。

## 依赖项

- Web UI: `web/src/components/StatusBar/`、`web/src/stores/useTlsConfigStore.ts`、`web/src/services/pushService.ts`
- Tray: `crates/bifrost-cli/src/commands/tray/tray.rs`、`crates/bifrost-cli/src/commands/tray/menu.rs`
- Admin API: `/api/config/tls`、`/api/proxy/system`

## 测试方案

- 单元测试：`web/src/components/StatusBar/statusIndicators.test.ts` 覆盖 TLS full/scoped/unknown 状态派生；`crates/bifrost-cli/src/commands/tray/menu.rs` 与 `tray_tests.rs` 覆盖 System Proxy 单项交互、TLS 菜单状态读取和 tray 顶部标题不展示 TLS 状态。
- E2E 测试：`web/tests/ui/admin-settings.spec.ts` mock TLS 全局启用，断言状态栏 `Full On`、动画 class 和跳转入口。
- 真实场景测试：`human_tests/tls-interception-status-indicators.md` 覆盖 Web 状态栏亮/暗主题、tray 下拉菜单 TLS 状态与切换、tray 顶部不展示 TLS 角标。

## Review/Fix/Test 闭环方案

- 第 1 轮复核 Web 状态派生、push 订阅、tray snapshot、tray 顶部无 TLS 角标和 System Proxy/TLS 菜单结构，运行前端单元测试、tray 相关 Rust 单元测试和 Web UI E2E。
- 第 2 轮基于最新 diff 复查主题颜色、动画降级、菜单状态文案、human_tests/readme 索引和未触碰的既有 ASR 改动，复跑受影响测试。

## 校验要求

- 执行 E2E 后再执行 rust-project-validate。
- 收尾运行 `make coverage`；如 E2E 覆盖环境不可用，按项目规则降级为 `make coverage-unit` 并记录原因。

## 文档更新要求

- 同步更新 `human_tests/readme.md` 的相关索引行。
- 本功能不新增 CLI 参数或协议，不需要更新 README 协议/Hook 表。
