# Desktop Launcher Startup Flow

## 背景

历史上桌面端使用“独立 launcher 窗口 + 主窗口”的双窗口方案：先弹一个小尺寸 launcher 窗口显示 loading，backend 就绪后再创建 main window，最后关掉 launcher。这个模型带来几个问题：

- 两个窗口在切换时出现明显的窗口闪烁 / 位置跳变。
- macOS 上双窗口会同时出现在 Dock、Mission Control、Cmd+Tab 里，用户看到“两个 Bifrost”。
- launcher 是 webview，同样要拉起前端 bundle，实际“Loading”很慢，反而失去了预热效果。
- 主 webview 只能等 launcher 消失后再挂载，backend 就绪与前端渲染无法并行。

当前实现改成“单个 host window + 原生启动遮罩（macOS 原生 view overlay） + 内嵌 webview handoff”：

- 只创建一个 Tauri `host` 窗口。
- macOS 上启动阶段在 host 窗口内容区安装 `NSView` overlay 作为启动器视觉层。
- 主业务 webview 通过 `create_main_webview()` 预先创建，并 “停放” 在 host 窗口外的不可见位置继续加载。
- backend 在后台线程并行启动。
- 三条流水线（overlay 显示 / webview 加载 / backend 就绪）全部就位后，`start_main_window_handoff` 保持 host window 在最终尺寸，恢复主界面背景与特效，显示 webview，并淡出 overlay。
- 非 macOS 平台没有原生 overlay 能力，直接以完整尺寸展示 host 窗口 + webview。

本文覆盖 launcher/handoff 语义、状态机、代码入口、错误路径和测试。日志、数据目录、失败上报见 `desktop-startup-observability.md`；关闭/退出/重开语义见 `desktop-macos-close-behavior.md`；端口切换见 `desktop-runtime-port-switch.md`。

## 用户目标验证清单

### 必须实现

- 桌面端只创建一个 host window，Dock / Cmd+Tab / Mission Control 只出现一个 Bifrost 图标。
- macOS 上启动阶段展示原生 launcher overlay，无独立第二个窗口。
- 非 macOS 平台没有 overlay 能力时，host 窗口直接以最终尺寸显示，不阻塞用户使用。
- 主 webview 与 backend 启动可以并行进行。
- Overlay、主 webview、backend 三者就绪后自动 handoff；如果 backend 启动失败，则在主 webview 就绪后进入同一个 handoff，让用户看到可重试的错误界面而不是永久停留在 launcher：
  - host 窗口从启动一开始就是 `TARGET_WINDOW_WIDTH×TARGET_WINDOW_HEIGHT`，避免主 Web UI 在中间尺寸下响应式重排；
  - 恢复背景色、装饰、阴影、macOS Sidebar/UnderWindow 特效；
  - 主 webview 从 park 位置移动到 `(0,0)` 并 resize 到 host 内部尺寸；
  - overlay 分帧淡出并从 NSView 树中移除。
- handoff 完成后向前端发送 `desktop://handoff-complete` 事件，供前端把 splash / loading 状态收尾。
- 在 backend 尚未 ready、webview 尚未 loaded 时，handoff 不会被误触发。
- 允许通过 `BIFROST_DESKTOP_LAUNCHER_ONLY=1` 进入“仅显示 launcher，不启动 backend、不加载 webview”的开发/调试模式。
- macOS launcher 采用接近系统启动页的虚拟水平进度条：首次绘制即显示 21%，约 1 秒推进到 80%，约 1.5 秒推进到 99%，真正 handoff 时补到 100% 并淡出；该进度不代表真实 backend/WebView 进度。

### 必须不破坏

- 现有 Tauri menu（App / File / Edit / View / Window）继续工作。
- `set_document_edited` / `write_clipboard` / `get_desktop_runtime` / `update_desktop_proxy_port` / `notify_main_window_ready` 五个 invoke handler 全部可用。
- `desktop-bootstrap.log`、`desktop-sidecar.out.log`、`desktop-sidecar.err.log` 三条日志继续按 `desktop-startup-observability.md` 描述落盘。
- macOS 关闭按钮走 `HideWindow`，Dock reopen 走 `restore_host_window`，Cmd+Q 走 `request_desktop_shutdown`。
- 端口切换、backend watchdog、cert bootstrap 保留现语义。

### 必须真实验证

- macOS 上冷启动能看到原生 overlay 且不出现第二个窗口。
- macOS 上 handoff 完成后 host 窗口尺寸、装饰、特效、可拖拽调整最小尺寸都正确。
- 非 macOS 平台冷启动直接进入完整 host 窗口，无 overlay 相关闪烁。
- `BIFROST_DESKTOP_LAUNCHER_ONLY=1` 场景下 backend 不启动、webview 不创建，但 overlay 正确显示。
- 前端 App 层能收到 `desktop://handoff-complete` 事件，能移除自身 splash。
- 关闭 host 窗口后重新从 Dock 打开，overlay 不会重复出现，直接恢复主界面。

## 产品语义

### Launcher = 原生 overlay，不是独立窗口

- Overlay 由 `desktop/src-tauri/src/native_launcher.rs` 通过 objc2 直接创建一个 `NSView`，添加到 host window 的 `contentView` 顶层。
- Overlay 有自己的动画时钟 (`start_animation`) 和进度值 (`set_overlay_progress`)、alpha (`set_overlay_alpha`)。
- Overlay 只出现在 macOS。`supports_native_launcher()` 返回 `cfg!(target_os = "macos")`。
- Launcher 视觉为全尺寸 `NSVisualEffectView` 毛玻璃层 + 居中水平进度条；背景由 macOS `UnderWindowBackground` 材质承载，并只叠加低透明度 Bifrost surface tint，让桌面背景在启动页后方若隐若现。亮色环境下材质会呈现柔和浅灰玻璃，暗色环境下保持接近主窗口深色背景，标题和进度条使用低饱和中性色，避免固定纯黑或纯白造成 handoff 跳变。

### Host window 是唯一顶级窗口

- Tauri label = `HOST_WINDOW_LABEL = "host"`。
- macOS 启动初始状态：1440×920，最小 1180×760，透明 host window + 全尺寸原生 launcher overlay。主 webview 仍在窗口外 park，不会在 overlay 淡出前露出响应式布局内容。
- 其他平台启动状态：1440×920，最小 1180×760，普通装饰 + 阴影，深色背景 `(8,17,23,255)`。
- Handoff 后统一变为 1440×920 + 最小 1180×760 + 装饰 + 阴影 + `apply_window_effects`（macOS Sidebar + UnderWindowBackground + radius 18；Windows Mica）。

### Main webview 是 host window 内的子 webview

- Tauri webview label = `MAIN_WINDOW_LABEL = "main"`。
- 由 `create_main_webview()` 通过 `WebviewBuilder::new(MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))` 创建。
- 初始位置：`(WEBVIEW_PARK_OFFSET=2000.0, 0.0)`（park 在 host window 视觉之外），初始尺寸为最终窗口尺寸。
- Handoff 时通过 `reveal_main_webview` 移动到 `(0,0)` 并 resize 到当前 host inner size。
- 页面加载事件通过 `on_page_load` 回调追踪；`PageLoadEvent::Finished` 时把 `main_webview_loaded` 置 true 并触发一次 handoff 尝试。

### handoff 的触发点

`try_start_native_handoff(app, reason)` 是唯一的 handoff 触发入口。它满足三个前置条件才会调用真正的 `start_main_window_handoff`：

1. `supports_native_launcher()` 为 true（macOS）。
2. `state.startup_ready == true`（backend 就绪或已复用现有 backend），或 `state.startup_error.is_some()`（backend 已明确失败，需要展示恢复 UI）。
3. `state.main_webview_loaded == true`（webview 完成首屏渲染）。

三条流水线都会调 `try_start_native_handoff`：

- backend bootstrap 完成后：`try_start_native_handoff(app, "backend ready")`。
- backend watchdog 恢复后：`try_start_native_handoff(app, "backend watchdog recovery")`。
- webview `PageLoadEvent::Finished` 后：`try_start_native_handoff(webview.app_handle(), "webview finished loading")`。
- backend bootstrap 失败后：记录 `startup_error`，再调用 `try_start_native_handoff(app, "backend startup failed")`；若 WebView 尚未 loaded，由后续 page-load 回调完成 handoff。
- 前端主动握手：`notify_main_window_ready` 调 `start_main_window_handoff(app, "frontend ready handshake")`（跳过 webview_loaded 检查，前端明确表示自己 ready）。

`start_main_window_handoff` 内部用两个原子标记做幂等保护：

- `handoff_started`：`swap(true, SeqCst)`，防止并发触发重复动画。
- `handoff_completed`：早退检查，避免 handoff 后被误调用。

## 状态机

```
BackendState {
  startup_ready: AtomicBool,        // backend 就绪或已复用
  main_webview_loaded: AtomicBool,  // webview page load finished
  main_window_ready: AtomicBool,    // 前端 notify_main_window_ready 已到达
  handoff_started: AtomicBool,      // handoff 动画正在跑
  handoff_completed: AtomicBool,    // handoff 已完成
  launcher_overlay: Mutex<Option<usize>>, // 原生 overlay 指针
}
```

状态转移：

1. `setup()`：
   - 创建 host window（macOS 和其他平台都使用最终尺寸；macOS 启动阶段由全尺寸 overlay 覆盖）。
   - 如果 `supports_native_launcher()`：安装 overlay，`launcher_overlay = Some(ptr)`，启动动画。
   - 如果不支持原生 overlay：直接把 `handoff_started`、`handoff_completed` 置为 true（跳过 handoff）。
   - 如果 `is_launcher_only_mode()` = false：`create_main_webview` + `bootstrap_desktop_backend` + `monitor_desktop_backend`。
2. Backend bootstrap 完成 → `startup_ready = true` → `try_start_native_handoff("backend ready")`。
3. Webview 首屏 loaded → `main_webview_loaded = true` → `try_start_native_handoff("webview finished loading")`。
4. 前端 splash 结束调用 `notify_main_window_ready` → `main_window_ready = true` → `start_main_window_handoff("frontend ready handshake")`。
5. `start_main_window_handoff`：
   - Swap `handoff_started`，防止重入。
   - `animate_host_window_to_main_size`：历史命名保留，当前不再执行窗口放大；只确认 host window 为最终尺寸，并用 `set_overlay_progress(..., 1.0)` 让虚拟进度条补到 100%。
   - 恢复背景色、装饰、阴影、window effects。
   - `reveal_host_window`：show + unminimize + set_focus。
   - 设置 resizable、maximizable、min_size。
   - `prepare_main_webview`：把子 webview resize 到 host inner size。
   - 起后台线程：睡 `WEBVIEW_REVEAL_SETTLE_DELAY (90ms)`，然后 `reveal_main_webview` 移到 `(0,0)`；如果 `overlay_ptr.is_some()` 就 `fade_out_launcher_overlay`（8 帧 × 14ms，共 ~112ms），形成 launcher 淡出、正式页面淡入的稳定过渡，最后 emit `desktop://handoff-complete`，置 `handoff_completed = true`。
6. 非 macOS 平台：状态 4/5 跳过；host window 一开始就是完整尺寸。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `main()`：Tauri builder，注册 menu / handler / setup / on_window_event / RunEvent 分流。
  - `setup(|app|)`：创建 host window、装 overlay、创建 main webview、启动 bootstrap 与 watchdog 线程。
  - `create_host_window` / `create_main_webview`
  - `try_start_native_handoff` / `start_main_window_handoff`
  - `animate_host_window_to_main_size` / `prepare_main_webview` / `reveal_main_webview` / `fade_out_launcher_overlay`
  - `notify_main_window_ready`（`#[tauri::command]`）
  - `is_launcher_only_mode`、`supports_native_launcher`
- `desktop/src-tauri/src/native_launcher.rs`
  - `install(window) -> Result<Option<usize>>`：在 macOS 上把原生 view 挂到 host window 的 contentView。
  - `start_animation(window, ptr)`：启动 overlay 内部动画时钟。
  - `set_overlay_progress(window, ptr, progress)`
  - `set_overlay_alpha(window, ptr, alpha)`
  - `remove_overlay(window, ptr)`：停止动画线程继续排队，从视图树摘除 overlay；保留 native handle 到进程退出，避免已经排队的主线程 tick 触达已释放的 Objective-C 对象。
  - 非 macOS 提供同名占位实现，返回 `Ok(None)`。
- `web/src/desktop/tauri.ts`
  - `DESKTOP_HANDOFF_COMPLETE_EVENT = "desktop://handoff-complete"`。
  - `notifyMainWindowReady()` → `invokeDesktop<void>("notify_main_window_ready")`。
- `web/src/App.tsx` 在 `DESKTOP_HANDOFF_COMPLETE_EVENT` 到达时移除自身 splash。

## 常量与时序

```rust
const TARGET_WINDOW_WIDTH: f64 = 1440.0;
const TARGET_WINDOW_HEIGHT: f64 = 920.0;
const TARGET_WINDOW_MIN_WIDTH: f64 = 1180.0;
const TARGET_WINDOW_MIN_HEIGHT: f64 = 760.0;
const OVERLAY_FADE_STEPS: u16 = 8;
const OVERLAY_FADE_STEP_DELAY: Duration = Duration::from_millis(14);
const WEBVIEW_PARK_OFFSET: f64 = 2000.0;
const WEBVIEW_REVEAL_SETTLE_DELAY: Duration = Duration::from_millis(90);
const DEFAULT_DESKTOP_STARTUP_DEADLINE: Duration = Duration::from_secs(30);
const HANDOFF_COMPLETE_EVENT: &str = "desktop://handoff-complete";
```

启动窗口不再放大。macOS launcher overlay 使用全尺寸毛玻璃背景遮住 parked WebView，玻璃层保留桌面背景的模糊透出感，虚拟进度条按固定节奏推进到 99%；handoff 时主 WebView 移入窗口，overlay 淡出。

## 错误与降级路径

- Overlay 安装失败（`native_launcher::install` 返回 `None` 或 `Err`）：
  - 把 `handoff_started` / `handoff_completed` 置 true（跳过 handoff），直接沿用 host window 当前状态。
  - `append_desktop_bootstrap_log`：`native launcher unsupported on this platform; entering webview directly`。
- Backend 启动失败：
  - 首次 readiness 等待同时检查 sidecar child；child 提前退出会立即带退出状态失败，不会在 65 个候选端口上重复等待。
  - `record_startup_error` 后调用 failure handoff；WebView 从 `get_desktop_runtime` 读取错误并展示 “Start Bifrost Service” 重试入口。
  - native launcher 不得继续覆盖错误界面；watchdog 或用户重试成功后恢复正常状态。
- Webview 加载失败（`PageLoadEvent::Started` 之后没有 `Finished`）：
  - `main_webview_loaded` 保持 false，`try_start_native_handoff` 不会触发。
  - 需要依赖前端的 `notify_main_window_ready` 兜底：前端在 window `load` / React root ready 时主动握手；即使 `main_webview_loaded == false`，`notify_main_window_ready` 会直接调 `start_main_window_handoff`，避免 overlay 卡死。
- Backend 或 WebView 永久阻塞：
  - 启动后同时调度 30 秒 launcher deadline；到期仍未 handoff 时写入完整状态，并在 backend 未就绪时记录 recoverable `startup_error`。
  - WebView 已 loaded 时进入正常 failure handoff；WebView 未 loaded 时不把 parked WebView 强行移入窗口，而是停止虚拟进度动画并把原生 launcher 切换成明确错误态。后续 WebView 若恢复并触发 load finished，仍可继续正常 handoff。
  - deadline 写错误前会再次读取 `startup_ready`；backend 成功路径先发布 ready 再清理旧错误，避免恰好在 30 秒边界成功时残留伪 timeout。
  - `BIFROST_DESKTOP_STARTUP_DEADLINE_MS` 只用于自动化测试缩短等待，不是面向终端用户的配置。
- Stale backend 清理失败：
  - 同步 `bifrost stop` helper 最多等待 5 秒，超时后发送 kill，并最多再等待 2 秒回收；kill 失败或回收仍超时都会返回错误，不能回到无界 `wait()`。
  - stop 任一失败必须写入 bootstrap log 并中止本次 backend 启动；禁止在同一 `BIFROST_DATA_DIR` 上启动第二个 core。
- 端口竞争：
  - 启动前已占用端口继续按候选端口顺延。
  - child 提前退出且原端口在启动后变为不可用，视为检查与 bind 之间的竞争，可尝试下一个候选端口。
  - 配置错误、child 检查错误或 readiness timeout 不盲目顺延，避免把确定性失败放大成 65 轮等待。
- Overlay 淡出失败（例如 objc2 崩溃）：
  - `fade_out_launcher_overlay` 内部所有调用 `let _ =`，不会 panic；overlay 最后仍会调 `remove_overlay`。
  - `remove_overlay` 不在主线程 `join()` 动画线程，也不释放 `LauncherOverlayHandle`。动画线程可能已经通过 `run_on_main_thread` 排队了 tick 回调；如果移除时释放 handle，晚到的 tick 会解引用悬空指针并在 macOS runloop observer 中触发 Rust foreign exception / `SIGABRT`。保留 handle 的泄漏量为一次启动一个 overlay，随桌面进程退出回收。
- `BIFROST_DESKTOP_LAUNCHER_ONLY=1`：
  - `create_main_webview` 与 `bootstrap_desktop_backend` 都不执行；overlay 会一直显示，用户按 Cmd+Q 退出会走 `request_desktop_shutdown` 的 launcher-only 分支直接 `app.exit(0)`。

## CLI / 环境变量表面

Launcher 本身没有 CLI 命令；控制入口只有环境变量：

- `BIFROST_DESKTOP_LAUNCHER_ONLY=1|true|yes|on`：仅展示 launcher，不启动 backend、不加载 webview，用于开发时快速验证 overlay。
- `BIFROST_DATA_DIR`：影响 backend 数据目录与日志路径。
- `BIFROST_DESKTOP_STARTUP_DEADLINE_MS`：测试用 launcher deadline override；非法值或 0 回退到 30 秒默认值。
- `BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES=1`：仅 debug/E2E 使用，允许隔离数据目录的测试 App 与已安装正式 App 并存；release 构建忽略该开关并继续强制 single-instance。
- 桌面端可执行文件本身 (`bifrost-desktop`) 一律无子命令。

## Web 交互契约

前端与桌面 handoff 的契约：

- 前端启动后应尽快调 `notifyMainWindowReady()`，让桌面端在 backend / webview 竞争条件下也能推进 handoff。
- 前端应订阅 `DESKTOP_HANDOFF_COMPLETE_EVENT` 事件，把内部 loading / splash 状态收尾。
- 前端不应假设收到事件时 backend 一定 ready；backend 就绪状态需通过 `getDesktopRuntime()` 或对应的 store 读取。

## Admin API 表面

Launcher 不新增 admin API。相关信息通过：

- `get_desktop_runtime` invoke handler：`{ expectedProxyPort, proxyPort, platform, startupReady, startupError }`。
- `notify_main_window_ready` invoke handler：`Result<(), String>`。

## Sync 边界

- Launcher 是本地视觉层，不涉及任何 sync。
- `BIFROST_DESKTOP_LAUNCHER_ONLY` 也不写入配置，属于运行时开关。

## 实现切分

### Phase 1：单窗口 + 原生 overlay 骨架（已完成）

- 删除旧 launcher window label 与相关 Tauri config。
- 新增 `native_launcher` 模块 + macOS 实现。
- `create_host_window` 分平台初始尺寸/装饰。
- `try_start_native_handoff` 三条件闸门 + 幂等标记。

### Phase 2：Handoff 动画与前端事件（已完成）

- `animate_host_window_to_main_size`：保持最终尺寸 + overlay progress 收尾。
- `prepare_main_webview` / `reveal_main_webview`：park + reveal。
- `fade_out_launcher_overlay`：分帧 alpha。
- Emit `desktop://handoff-complete`，前端消费。

### Phase 3：降级路径（已完成）

- 非 macOS 直接跳过 handoff。
- `BIFROST_DESKTOP_LAUNCHER_ONLY` 支持仅 launcher 调试。
- 前端 `notify_main_window_ready` 强制推进兜底。
- sidecar 提前退出时以 failure handoff 暴露 `startupError`；所有其他未知阻塞由 30 秒 launcher deadline 最终兜底。

### Phase 4：文档 & 测试维护

- 保持本文与 `desktop-startup-observability.md`、`desktop-macos-close-behavior.md`、`desktop-runtime-port-switch.md` 的边界清晰。
- 单元测试覆盖 close 行为、port response 解析、recovery guard；handoff / overlay 依赖真实窗口环境，走 human_tests。

## 测试方案

### 单元测试（`desktop/src-tauri/src/main.rs` 内嵌 `#[cfg(test)] mod tests`）

- `desktop_config_uses_shared_data_dir`
- `desktop_data_dir_matches_shared_cli_dir`
- `parses_snake_case_port_update_response`
- `parses_camel_case_port_update_response`
- `detects_legacy_server_config_response`
- `macos_close_request_hides_window`
- `non_macos_close_request_shuts_down_app`
- `backend_recovery_guard_prevents_parallel_recovery`
- `poll_managed_backend_exit_reports_exited_child`
- `launcher_handoff_allows_ready_or_recoverable_error`
- `desktop_startup_deadline_defaults_and_accepts_test_override`
- `wait_for_child_exit_kills_process_after_timeout`
- `wait_for_backend_reports_child_exit_without_waiting_for_timeout`
- `native_launcher::imp::tests::virtual_progress_matches_startup_milestones`
- `native_launcher::imp::tests::handoff_progress_uses_only_final_one_percent`

Overlay 视觉、窗口层级和 handoff 仍以真实窗口验证为主；虚拟进度时序通过 unit test 固定关键里程碑。

### E2E / 真实场景

- `human_tests/desktop-launcher-startup.md`（新增或就地维护）：
  - TC-DLS-01：macOS 冷启动显示全尺寸启动页，只出现一个 Bifrost 窗口，主 Web UI 在 overlay 淡出前不可见。
  - TC-DLS-02：`BIFROST_DESKTOP_LAUNCHER_ONLY=1` 下截图验证虚拟进度条首次约 21%、约 1 秒 80%、约 1.5 秒 99%。
  - TC-DLS-03：验证启动页背景在当前暗色/亮色偏好下与主窗口背景风格连续，标题和进度条可读且不过度抢眼。
  - TC-DLS-06：sidecar 提前退出后快速 failure handoff，主界面显示可恢复错误。
  - TC-DLS-08：sidecar 永久阻塞时 deadline 强制移除 launcher。
  - TC-DLS-04：真实 handoff 过程中窗口尺寸和位置保持稳定，仅发生 launcher 淡出和正式页面淡入。

启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-desktop --tests`（含上述内嵌 tests）
- `rust-project-validate`
- 本机 no-local-coverage 约定生效，交付时说明本地豁免。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标：单窗口、并行启动、原生 overlay、handoff 三闸门、幂等、非 macOS 降级、launcher-only 调试。
- 复核 diff：`main.rs` handoff 逻辑、`native_launcher.rs` 平台分支、`web/src/desktop/tauri.ts` 事件常量、`web/src/App.tsx` 事件订阅。
- 重点 review：`handoff_started` / `handoff_completed` swap 是否有 race；overlay 在 backend 失败时是否正确留在屏幕上；`notify_main_window_ready` 与自动 handoff 之间是否有双触发。
- 复测：cargo tests + macOS 冷启动人工验证 + launcher-only 场景。

### 第 2 轮

- 复核第 1 轮发现的问题修复。
- 再次检查 `git status --short`、`git diff`。
- 重点 review：错误路径（overlay install 失败、webview 未 finished、backend 挂掉）下 overlay/handoff 状态是否一致。
- 复测：失败路径重跑，必要时补充真实操作。

## 风险与决策点

- macOS 使用原生 `NSView` overlay 而非 SwiftUI/Metal：兼容性最好，但视觉效果受限于纯 objc2 能力；如未来要引入更丰富动画，可以在 `native_launcher.rs` 里替换实现，无需改动 handoff 状态机。
- Windows/Linux 目前直接跳过 overlay 是权衡：这些平台窗口装饰更宽、启动更快，overlay 收益不明显；如未来要补齐，可在 `supports_native_launcher()` 与 `native_launcher` 里加平台实现。
- `notify_main_window_ready` 会绕过 `main_webview_loaded` 检查，一旦前端错误地在页面完全就绪前调用会导致 handoff 提前；权衡是前端可用作 fallback，避免 overlay 永久卡死。
- `WEBVIEW_PARK_OFFSET=2000.0` 假设显示器逻辑坐标不超过 2000，对超大显示器可能造成 park 位置仍可见；如遇到问题可改用 `webview.hide()` 或负偏移。第一版保持简单。
- `BIFROST_DESKTOP_LAUNCHER_ONLY` 主要用于开发调试，未来若成为正式产品能力应有明确 UI；当前仅通过环境变量。
- 进度条是虚拟进度，不绑定 backend/WebView 的真实加载百分比；如果未来引入真实进度，需要避免倒退，并保留短启动时的稳定视觉节奏。
