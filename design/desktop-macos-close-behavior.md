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

- Desktop 启动 app-bound backend sidecar 时必须清除父环境中可能继承的
  `BIFROST_DETACHED_DAEMON_CHILD`，再设置 `BIFROST_DESKTOP_CORE=1`：
  - sidecar 的 `runtime.json` 必须记录 `runtime_start_mode=desktop`；
  - Desktop Quit 必须停止该 app-bound Service；
  - 不能因 Desktop 自身由 CLI daemon 环境或升级 helper 拉起，就把 sidecar 错记为 daemon。
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
- Tray：
  - Service 的 `runtime_start_mode=desktop` 时，主操作显示 `Quit Bifrost`，不显示会被
    watchdog 恢复的 `Stop Bifrost`；
  - `Quit Bifrost` 通过 Desktop 单实例参数 `--bifrost-upgrade-shutdown` 复用
    `request_desktop_shutdown`，先关闭 watchdog，再停止 Desktop 持有的 child Service；
  - Service 由 CLI 启动（`daemon` / `foreground` / `unknown`）时，Tray 继续显示
    `Stop Bifrost`，停止后显示 `Start Bifrost`，不退出 Desktop。
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
- 真正由 CLI `start --daemon` 启动且记录 `runtime_start_mode=daemon` 的 Service
  在 Desktop 复用后仍归 CLI 所有；Desktop Quit 不得停止它。

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
| Tray `Quit Bifrost`（Desktop-owned Service） | `bifrost-desktop --bifrost-upgrade-shutdown` 的单实例回调 | 先停 watchdog，再停 owned backend 并退出 | Tray `QuitDesktop` → Desktop `request_desktop_shutdown` |
| Tray `Stop Bifrost`（CLI-owned Service） | CLI `bifrost stop` | 只停止 CLI Service | Tray `StopService` → CLI stop |
| Dock 图标点击（无窗口） | `RunEvent::Reopen { has_visible_windows: false }` | 恢复 host 窗口 | `restore_host_window` |

### “隐藏”不是“最小化”

- `window.hide()` 让 host 窗口从屏幕消失但仍存在，Dock 图标保留。
- 不使用 `set_minimized(true)`，因为 minimize 会在 Dock 生成一个缩略图，与预期语义（后台代理常驻）不同。
- Reopen 时 `reveal_host_window`：`show + unminimize + set_focus`，兼容用户可能在隐藏之前恰好 minimize 的场景。

### 单一 helper 收敛显示

`reveal_host_window` 被 `restore_host_window` 与 `start_main_window_handoff` 复用，避免 handoff / reopen 行为漂移。若未来新增“菜单栏点击 Show Window”等入口，必须通过同一 helper 恢复窗口。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `configure_desktop_backend_environment(command, data_dir, startup_session_id)`：
    清除 daemon-only child marker 后注入 Desktop sidecar ownership 环境。
  - `handle_host_window_close_request(window)`：分流 CloseRequested。
  - `host_window_close_behavior() -> HostWindowCloseBehavior`：仅根据 `cfg!(target_os = "macos")` 决定。
  - `host_window_close_behavior_for_platform(is_macos: bool)`：纯函数，便于单元测试。
  - `should_intercept_exit(app)`：读 `state.force_exit`，用于避免 `app.exit()` 时被自己拦截。
  - `request_desktop_shutdown(app)` / `complete_desktop_shutdown(app)`：异步停 backend + `app.exit(0)`。
  - `restore_host_window(app)` / `reveal_host_window(window)`：Reopen 恢复入口。
  - `on_window_event(|window, event|)`：仅处理 host label 的 CloseRequested，调用 `api.prevent_close()` 后走分流。
  - `run(|app_handle, event|)`：`RunEvent::ExitRequested` + `RunEvent::Reopen`（macOS）分流。
- Menu 中 `PredefinedMenuItem::quit` / `PredefinedMenuItem::close_window` 是与用户交互的入口。
- `crates/bifrost-cli/src/commands/tray/runtime.rs`
  - 兼容解析 `runtime_start_mode` / `start_mode`，供 Tray 判断 Service ownership。
- `crates/bifrost-cli/src/commands/tray/menu.rs`
  - Desktop owner 映射为 `Quit Bifrost` / `QuitDesktop`；CLI owner 保持
    `Stop Bifrost` / `StopService`。
- `crates/bifrost-cli/src/commands/tray/tray.rs`
  - 从 Service PID 的直系父进程解析可信的 `bifrost-desktop(.exe)`，只向该可执行文件发送
    `--bifrost-upgrade-shutdown`；执行前同时校验 runtime 记录的 Service 启动时间，
    防止陈旧菜单 PID 被复用；不直接向 Desktop 或 Service 发 kill signal。

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

Desktop 的同步 restart-stop 与异步 quit-stop 共用
`configure_backend_stop_command`。该 helper 同时注入 `BIFROST_DATA_DIR` 与私有
`BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL=1`；后者只授权 Desktop 自己的 shutdown
helper 越过 CLI 对 live Desktop-owned runtime 的 fail-closed 保护。普通用户 CLI
`stop/restart` 不设置该变量，仍会被拒绝。

### Sidecar ownership 环境边界

Desktop shell 可能由 CLI daemon、升级 helper 或其他继承了
`BIFROST_DETACHED_DAEMON_CHILD=1` 的进程启动。CLI core 内部有意规定 detached daemon
标记优先于 Desktop 标记，因此 Desktop 不能只追加 `BIFROST_DESKTOP_CORE=1`；必须在
创建 sidecar `Command` 时显式 `env_remove(BIFROST_DETACHED_DAEMON_CHILD)`。

该清理只作用于 Desktop 创建的 child command，不修改父进程环境，也不改变 CLI
`start --daemon` 创建真正 daemon child 时的优先级。由此保证：

- Desktop 新建 sidecar → `runtime_start_mode=desktop` → Quit 时 stop。
- Desktop 复用既有 CLI daemon → `runtime_start_mode=daemon` → Quit 时 preserve。

## 依赖项

- Tauri 2 runtime：`WindowEvent::CloseRequested { api }`、`RunEvent::ExitRequested { api }`、`RunEvent::Reopen { has_visible_windows }`（macOS）。
- objc2 / objc2_app_kit：`NSWindow.setDocumentEdited`（`set_document_edited` invoke）；`NSPasteboard`（clipboard）。
- BackendState：`shutdown_started` / `force_exit` / `launcher_only` / `child` / `data_dir` / `binary_path`。
- `bifrost stop`：sidecar 用相同 binary 走 `Command::new(binary_path).arg("stop").env("BIFROST_DATA_DIR", data_dir)` 触发本地 daemon 停止流程。

## CLI / 环境变量表面

无新 CLI。相关环境变量：

- `BIFROST_DESKTOP_LAUNCHER_ONLY=1`：关闭时直接 `app.exit(0)`，跳过 sidecar stop。
- `BIFROST_DATA_DIR`：影响 `bifrost stop` 找到的 daemon 数据目录。
- `BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL=1`：Desktop 内部 stop helper 私有授权；
  不作为用户 CLI 表面公开。

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
- `desktop_sidecar_clears_inherited_detached_daemon_marker`：直接检查 child `Command`
  同时包含 daemon marker 的 `env_remove` 与 Desktop marker 的 `env=1`。
- `desktop_backend_stop_command_is_authorized_for_owned_runtime`：同步/异步 stop 共用的
  command helper 同时带 data dir 与 Desktop 私有 stop 授权。
- 现有 `backend_recovery_guard_prevents_parallel_recovery` / `poll_managed_backend_exit_reports_exited_child` 与 shutdown 路径耦合，间接保护 `request_desktop_shutdown` 的 child detach。

### E2E / 真实场景（`e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`、`human_tests/desktop-core-daemon-registration.md`）

- TC-DMC-01：macOS 冷启动 → 关闭 host 窗口 → `pgrep -af bifrost` 仍能看到桌面与 sidecar；`curl` admin API 仍 200。
- TC-DMC-02：TC-DMC-01 后点 Dock 图标 → host 窗口恢复；`get_desktop_runtime` 返回相同 pid（通过 log 佐证）。
- TC-DMC-03：Cmd+Q → 桌面进程与 sidecar 都退出；`desktop-bootstrap.log` 有 shutdown 记录。
- TC-DMC-04：App 菜单 → Quit Bifrost：同 TC-DMC-03。
- TC-DMC-05：Cmd+W 与 Close 按钮语义一致：只隐藏窗口。
- TC-DMC-06：非 macOS（Windows/Linux）关闭窗口 → 进程与 sidecar 都退出。
- TC-DMC-07：`BIFROST_DESKTOP_LAUNCHER_ONLY=1` 场景 Cmd+Q 立即退出，无 sidecar 停止流程。
- TC-DMC-08：在父环境显式设置 `BIFROST_DETACHED_DAEMON_CHILD=1` 后启动 Desktop，
  断言 runtime owner 仍为 `desktop`，触发 graceful shutdown 后 App 与 Service 都退出。
- TC-DMC-09：先用 CLI `start --daemon` 启动 Service，再打开 Desktop 复用，
  触发 graceful shutdown 后 App 退出但 CLI Service 保持健康。
- TC-DMC-10：Desktop-owned Service 运行时 Tray 主操作为 `Quit Bifrost`；点击后 Desktop
  与 Service 都退出，等待超过一次 watchdog poll 后 Service 仍未恢复。
- TC-DMC-11：CLI-owned daemon 运行时 Tray 主操作仍为 `Stop Bifrost`；点击后只停止
  Service，Tray 随后提供 `Start Bifrost`。

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
