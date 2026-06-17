# Desktop Window Chrome Strategy

## 现状结论

旧文档描述的平台分配置与前端自定义标题栏方案目前并未落地。仓库中只有一个 [`desktop/src-tauri/tauri.conf.json`](../desktop/src-tauri/tauri.conf.json)，没有 `tauri.macos.conf.json` / `tauri.windows.conf.json`。

## 当前实现

- 窗口装饰策略主要由 [`desktop/src-tauri/src/main.rs`](../desktop/src-tauri/src/main.rs) 在运行时控制，平台分支统一通过 `supports_native_launcher()`（当前等价于 `cfg!(target_os = "macos")`) 判断。
- macOS 启动阶段（`supports_native_launcher()` 为 true）：
  - `create_host_window` 以 `decorations(false)`、`transparent(true)`、`shadow(false)` 创建小尺寸 `host` 窗口（`INITIAL_WINDOW_WIDTH/HEIGHT`），承载透明启动态与 `native_launcher` overlay。
  - 进入 `start_main_window_handoff` 时调用 `host_window.set_decorations(true)` 并通过 `animate_host_window_to_main_size` 扩展到 `TARGET_WINDOW_*` 尺寸，恢复标准窗口外观。
- 非 macOS 平台（`supports_native_launcher()` 为 false）：
  - `create_host_window` 直接以 `TARGET_WINDOW_*` 尺寸、`decorations(true)`、`transparent(false)`、不透明背景启动，跳过启动态 overlay 与 handoff 期间的装饰切换。
  - Windows 上 `apply_window_effects` 会额外施加 `Effect::Mica`，但仓库里没有已启用的自绘标题栏实现，也没有前端标题栏组件接管窗口控制。

## 与旧方案的差异

- 没有平台级 Tauri 配置拆分。
- 没有统一的前端标题栏抽象层。
- `startDragging()` / `toggleMaximize()` 类型定义存在于前端 runtime bridge 中，但当前仓库没有对应的桌面标题栏 UI 作为主路径使用。

## 建议

- 如果后续真的要做跨平台窗口 chrome 统一，建议重新立项，单独定义：
  - 启动态和正常态的装饰切换规则；
  - macOS / Windows / Linux 的平台差异；
  - 前端是否需要接管标题栏交互。
