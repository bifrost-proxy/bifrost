# Desktop macOS Close Behavior

## 功能模块详细描述

桌面端在 macOS 上需要遵循原生窗口语义：

- 用户点击窗口关闭按钮或执行 `Close Window` 时，仅隐藏窗口，不退出应用；
- 用户执行明确的退出动作，例如 Dock 菜单 Quit、应用菜单 Quit、`Cmd+Q` 时，才停止内嵌 backend 并退出桌面进程；
- 应用在无可见窗口时被重新激活，需要恢复主窗口，避免出现“进程仍在但没有可见窗口”的状态。

## 实现逻辑

- 在 `desktop/src-tauri/src/main.rs` 中将 host 窗口的 `CloseRequested` 分流：
  - macOS 返回 `HideWindow`，仅调用 `window.hide()`；
  - 其他平台保持现有语义，继续走 `request_desktop_shutdown()`。
- 保留 `RunEvent::ExitRequested` 对显式退出动作的拦截，继续执行已有的 backend 停止与最终 `app.exit(0)`。
- 在 macOS 增加 `RunEvent::Reopen` 处理：
- 将 host 窗口显示逻辑收敛到公共 helper `reveal_host_window`（`show` + `unminimize` + `set_focus`），由 `restore_host_window` 与 `start_main_window_handoff` 共用，避免 handoff 和 reopen 两处行为漂移。
- 将 host 窗口显示逻辑收敛到公共 helper，避免 handoff 和 reopen 两处行为漂移。

## 依赖项

- Tauri 2 runtime 的 `RunEvent::Reopen`（macOS）
- 现有桌面端 `BackendState` / `request_desktop_shutdown()` 生命周期管理

## 测试方案（含 e2e）

- 单元测试：
  - 校验 macOS 平台的 close 行为映射为 `HideWindow`
  - 校验非 macOS 平台的 close 行为映射为 `ShutdownApp`
- E2E / 手工验证：
  - 启动桌面端后在 macOS 点击关闭按钮，确认进程仍存活且 backend 未被 shutdown；
  - 点击 Dock 图标重新激活应用，确认窗口恢复；
  - 执行 `Cmd+Q` 或应用菜单 `Quit Bifrost`，确认 backend 停止且桌面进程退出。

## 校验要求（含 rust-project-validate）

- 先按 `e2e-test` 技能执行本次改动相关的端到端验证；
- 再按 `rust-project-validate` 技能顺序执行 `fmt`、`clippy`、按改动范围测试与构建；
- 若校验失败，需先修复再结束任务。

## 文档更新要求

- 本次变更不涉及 README 中的安装、API、配置项或协议说明，无需额外更新 `README.md`；
- 若后续为 macOS 增加显式的菜单或托盘交互文档，再在桌面相关设计文档中继续增量补充。
