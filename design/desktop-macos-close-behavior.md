# Desktop macOS Close Behavior

## 背景

Tauri 默认的窗口关闭语义在跨平台上是“关闭窗口即退出应用”。但 macOS 有很强的原生约定：

- 关闭窗口（红色 close 按钮 / File → Close Window / `Cmd+W`）应该只隐藏窗口，进程仍保留在 Dock，服务/后台任务不断开。
- 只有 `Cmd+Q` / App 菜单 Quit / Dock 菜单 Quit / Force Quit 才真正结束进程。
- 应用无可见窗口时，用户点 Dock 图标应该恢复主窗口，不是启动新实例，也不是让 Dock 图标看起来“僵尸”一样。

Bifrost 桌面端内嵌 backend sidecar，如果关闭按钮就把 backend 一起 kill 掉，会破坏“最小化到菜单栏 / 后台代理仍在运行”的预期，而且再打开时要重新走 backend bootstrap，非常慢。

本文覆盖桌面端在 macOS/非 macOS 下的窗口关闭 / 应用退出 / Reopen 语义与实现。相关：`desktop-launcher-startup.md`（handoff / overlay）、`desktop-startup-observability.md`（日志与 startup_error）、`desktop-core-watchdog-resource-guard.md`（backend watchdog）。

## 用户目标验证清单

### 必须实现

- macOS：点击 host 窗口关闭按钮 / File → Close Window / `Cmd+W`：
  - 桌面进程仍存活；
  - backend sidecar 仍在运行；
  - 系统代理设置不被清理；
  - Dock 图标继续显示 Bifrost；
  - 关闭事件在 bootstrap log 中留有记录。
- macOS：`Cmd+Q` / App 菜单 Quit / Dock 菜单 Quit：
  - 触发 `RunEvent::ExitRequested`；
  - 走 `request_desktop_shutdown`；
  - 隐藏 host 窗口 → 异步发出 `bifrost stop` helper → 释放 managed child → `app.exit(0)`。
- macOS：无可见窗口时点 Dock 图标（`RunEvent::Reopen` with `has_visible_windows = false`）：
  - 恢复 host 窗口（show + unminimize + set_focus）；
  - 不重新启动 backend；
  - 不重复走 handoff 动画（handoff_completed 已置位时 idempotent）。
- 非 macOS：关闭窗口 = 退出应用（`ShutdownApp`），保持与 CLI daemon 语义一致；不引入 macOS-only 的“隐藏”预期。

### 必须不破坏

- Handoff / launcher / cert bootstrap / watchdog / port switch 语义。
- `set_document_edited` 逻辑（macOS 关闭按钮上的“未保存修改”标记）仍能通过 `NSWindow.setDocumentEdited` 生效。
- Menu 中的 `PredefinedMenuItem::quit` 仍触发 `RunEvent::ExitRequested`。
- Launcher-only 模式（`BIFROST_DESKTOP_LAUNCHER_ONLY=1`）关闭时立即 `app.exit(0)`，不走 sidecar stop（无 sidecar）。

### 必须真实验证

- macOS：关闭窗口后 backend 仍能被外部代理请求命中。
- macOS：从 Dock 重新激活能恢复窗口且是同一个进程。
- macOS：`Cmd+Q` 后 `pgrep -af bifrost` 无 sidecar 残留（除了 detach 的 stop helper 短暂执行）。
- 非 macOS：关闭窗口进程退出，backend 一并被 stop helper 停止。
- 日志：`desktop-bootstrap.log` 记录 “host window close requested on macOS; hiding window and keeping app alive” 或 “desktop shutdown requested” 等对应事件。

## 产品语义

### 三种关闭意图对应三条路径

| 用户动作 | 触发的 Tauri 事件 | 期望语义 | 代码路径 |
| --- | --- | --- | --- |
| Close 按钮 / File → Close Window / `Cmd+W`（macOS） | `WindowEvent::CloseRequested` | 只隐藏窗口 | `handle_host_window_close_request` → `HideWindow` → `window.hide()` |
| Close 按钮（非 macOS） | `WindowEvent::CloseRequested` | 关闭并退出应用 | `handle_host_window_close_request` → `ShutdownApp` → `request_desktop_shutdown` |
| Quit（App 菜单 / `Cmd+Q` / Dock 菜单 Quit） | `RunEvent::ExitRequested` | 停 backend 后退出 | `should_intercept_exit` + `api.prevent_exit()` + `request_desktop_shutdown` |
| Dock 图标点击（无窗口） | `RunEvent::Reopen { has_visible_windows: false }` | 恢复 host 窗口 | `restore_host_window` |

### “隐藏”不是“最小化”

- `window.hide()` 让 host 窗口从屏幕消失但仍存在，Dock 图标保留。
- 不使用 `set_minimized(true)`，因为 minimize 会在 Dock 生成一个缩略图，与预期语义（后台代理常驻）不同。
- Reopen 时 `reveal_host_window`：`show + unminimize + set_focus`，兼容用户可能在隐藏之前恰好 minimize 的场景。

### 单一 helper 收敛显示

`reveal_host_window` 被 `restore_host_window` 与 `start_main_window_handoff` 复用，避免 handoff / reopen 行为漂移。若未来新增“菜单栏点击 Show Window”等入口，必须通过同一 helper 恢复窗口。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `handle_host_window_close_request(window)`：分流 CloseRequested。
  - `host_window_close_behavior() -> HostWindowCloseBehavior`：仅根据 `cfg!(target_os = "macos")` 决定。
  - `host_window_close_behavior_for_platform(is_macos: bool)`：纯函数，便于单元测试。
  - `should_intercept_exit(app)`：读 `state.force_exit`，用于避免 `app.exit()` 时被自己拦截。
  - `request_desktop_shutdown(app)` / `complete_desktop_shutdown(app)`：异步停 backend + `app.exit(0)`。
  - `restore_host_window(app)` / `reveal_host_window(window)`：Reopen 恢复入口。
  - `on_window_event(|window, event|)`：仅处理 host label 的 CloseRequested，调用 `api.prevent_close()` 后走分流。
  - `run(|app_handle, event|)`：`RunEvent::ExitRequested` + `RunEvent::Reopen`（macOS）分流。
- Menu 中 `PredefinedMenuItem::quit` / `PredefinedMenuItem::close_window` 是与用户交互的入口。

## Close / Quit / Reopen 状态机

```
CloseRequested (host window)
├── api.prevent_close()
└── handle_host_window_close_request
    ├── macOS → HideWindow
    │   ├── log "host window close requested on macOS; hiding window and keeping app alive"
    │   └── window.hide()
    └── non-macOS → ShutdownApp → request_desktop_shutdown

RunEvent::ExitRequested
├── should_intercept_exit(app) == true  → api.prevent_exit() + request_desktop_shutdown
└── should_intercept_exit(app) == false → 放行（用于 request_desktop_shutdown 内部的 app.exit(0)）

RunEvent::Reopen { has_visible_windows: false }  (macOS)
└── restore_host_window(app)
    ├── log "desktop reopen requested on macOS; restoring host window"
    └── reveal_host_window(host_window) = show + unminimize + set_focus

request_desktop_shutdown(app)
├── shutdown_started.swap(true) → 已在进行则直接 return
├── window.hide()
├── launcher_only ?
│   ├── yes → force_exit = true; app.exit(0)
│   └── no  → spawn thread → complete_desktop_shutdown
│       ├── spawn_backend_stop (async `bifrost stop`)
│       ├── detach managed child（不 kill，靠 stop helper 收尾）
│       ├── force_exit = true
│       └── app.exit(0)  → 触发 ExitRequested → should_intercept_exit == false → 放行
```

## 依赖项

- Tauri 2 runtime：`WindowEvent::CloseRequested { api }`、`RunEvent::ExitRequested { api }`、`RunEvent::Reopen { has_visible_windows }`（macOS）。
- objc2 / objc2_app_kit：`NSWindow.setDocumentEdited`（`set_document_edited` invoke）；`NSPasteboard`（clipboard）。
- BackendState：`shutdown_started` / `force_exit` / `launcher_only` / `child` / `data_dir` / `binary_path`。
- `bifrost stop`：sidecar 用相同 binary 走 `Command::new(binary_path).arg("stop").env("BIFROST_DATA_DIR", data_dir)` 触发本地 daemon 停止流程。

## CLI / 环境变量表面

无新 CLI。相关环境变量：

- `BIFROST_DESKTOP_LAUNCHER_ONLY=1`：关闭时直接 `app.exit(0)`，跳过 sidecar stop。
- `BIFROST_DATA_DIR`：影响 `bifrost stop` 找到的 daemon 数据目录。

## Web / Admin API 表面

无。前端不直接触发关闭；用户只通过 macOS 原生手势 / 菜单。若前端要主动退出，可通过 tauri `app.exit()` 或 window close，会走同一分流路径。

## Sync 边界

- 关闭 / 退出仅影响本机进程状态，不写入任何 sync 配置。
- Sync 会话在 backend 内部完成清理，桌面壳层不干预。

## 实现切分

### Phase 1：关闭分流骨架（已完成）

- 提取 `HostWindowCloseBehavior` 枚举 + `host_window_close_behavior_for_platform` 纯函数。
- `on_window_event` 只处理 host label；`api.prevent_close()` + 分流。
- 单元测试覆盖平台 → 行为映射。

### Phase 2：Quit 拦截（已完成）

- `should_intercept_exit(app)` 读 `state.force_exit`。
- `RunEvent::ExitRequested` 走 `api.prevent_exit()` + `request_desktop_shutdown`。
- `request_desktop_shutdown` 幂等（`shutdown_started.swap`）。
- 异步 stop helper + detach child + `app.exit(0)`。

### Phase 3：Reopen 恢复（已完成）

- macOS `RunEvent::Reopen { has_visible_windows: false }` → `restore_host_window`。
- `reveal_host_window` helper 统一 show + unminimize + set_focus。
- 与 `start_main_window_handoff` 共用 helper，避免行为漂移。

### Phase 4：文档与人工测试

- 保持本文与 `desktop-launcher-startup.md`、`desktop-startup-observability.md` 边界清晰。
- human_tests 覆盖关键点。

## 测试方案

### 单元测试

- `macos_close_request_hides_window`：`host_window_close_behavior_for_platform(true) == HideWindow`。
- `non_macos_close_request_shuts_down_app`：`host_window_close_behavior_for_platform(false) == ShutdownApp`。
- 现有 `backend_recovery_guard_prevents_parallel_recovery` / `poll_managed_backend_exit_reports_exited_child` 与 shutdown 路径耦合，间接保护 `request_desktop_shutdown` 的 child detach。

### E2E / 真实场景（`human_tests/desktop-macos-close-behavior.md`）

- TC-DMC-01：macOS 冷启动 → 关闭 host 窗口 → `pgrep -af bifrost` 仍能看到桌面与 sidecar；`curl` admin API 仍 200。
- TC-DMC-02：TC-DMC-01 后点 Dock 图标 → host 窗口恢复；`get_desktop_runtime` 返回相同 pid（通过 log 佐证）。
- TC-DMC-03：Cmd+Q → 桌面进程与 sidecar 都退出；`desktop-bootstrap.log` 有 shutdown 记录。
- TC-DMC-04：App 菜单 → Quit Bifrost：同 TC-DMC-03。
- TC-DMC-05：Cmd+W 与 Close 按钮语义一致：只隐藏窗口。
- TC-DMC-06：非 macOS（Windows/Linux）关闭窗口 → 进程与 sidecar 都退出。
- TC-DMC-07：`BIFROST_DESKTOP_LAUNCHER_ONLY=1` 场景 Cmd+Q 立即退出，无 sidecar 停止流程。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-desktop --tests`
- `rust-project-validate`
- 本地无 coverage 依赖。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核：三种意图 → 三条路径映射是否覆盖 (Close / Quit / Reopen)。
- 复核 diff：`main.rs` 的 CloseRequested / ExitRequested / Reopen 分流是否闭环；`reveal_host_window` 是否被两处入口共用。
- 重点 review：`api.prevent_close()` 是否漏了；`api.prevent_exit()` 与 `force_exit.store(true)` 的顺序是否会陷入死循环；Reopen 分支是否只对 `!has_visible_windows` 触发（避免多窗口场景重复 focus）。
- 复测：单元测试；macOS 冷启动 close / reopen / quit 三次组合。

### 第 2 轮

- 复核第 1 轮发现的问题修复。
- 再次检查 `git status --short`、`git diff`、human_tests 索引。
- 重点 review：错误路径下 `shutdown_started` 与 `force_exit` 是否可能被重复设置导致 exit hang；Launcher-only 分支是否绕过所有 sidecar 相关逻辑。
- 复测：真实操作、非 macOS 平台冷启动关闭。

## 风险与决策点

- 是否为菜单栏图标（Tray）增加“Show Window / Quit”入口：当前主要通过 Dock 与菜单，Tray 不属于本文；如未来引入应复用 `reveal_host_window` + `request_desktop_shutdown`。
- 多窗口扩展：目前只有 host label，`on_window_event` 已通过 `window.label() != HOST_WINDOW_LABEL` 忽略其它窗口；如未来加浮窗，需要显式判断是否属于要保留的窗口列表。
- 非 macOS 平台是否也考虑“关闭 = 后台常驻”：当前策略保持与 CLI daemon 一致，避免 Linux/Windows 用户被“看不到但仍在跑”的进程困扰；若未来引入 Tray，可按平台重新评估。
- macOS `RunEvent::Reopen` 在 Cmd+Tab 唤起时不触发（`has_visible_windows` 视场景而定），因此不能作为唯一“恢复窗口”入口；若用户以 Cmd+Tab 恢复到隐藏应用，需要额外的菜单或托盘手段。当前不新增。
- `bifrost stop` 是同步/异步的选择：现在 `spawn_backend_stop` 用异步 spawn 后立刻 `app.exit(0)`，如果 sidecar 需要更强保证（例如 sync flush），可以在 stop helper 内做，桌面壳层不阻塞。
