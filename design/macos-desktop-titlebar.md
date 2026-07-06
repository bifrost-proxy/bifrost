# 桌面端标题栏融合方案

## 功能模块说明

macOS Tauri 桌面端需要去掉截图中独立的灰色系统标题栏，让原生窗口控制区与 Bifrost Web UI 视觉融为一体。同时，左侧导航栏应提供可拖拽区域，用户可从左侧菜单区域移动窗口位置。

Windows Tauri 桌面端同样需要使用 Web UI 自定义 chrome：不能显示 Windows 原生标题栏，也不能显示 `Bifrost / File / Edit / View / Window` 原生菜单栏，否则会把共享 Web UI 向下挤压并遮掉底部状态栏。

普通浏览器打开的 CLI Web UI 不参与该能力，避免把桌面窗口行为泄漏到共享 Web UI。

## 实现逻辑

- Rust 窗口层：
  - 按 Tauri 官方窗口自定义文档，macOS host window 使用 `hidden_title(true)` 和 `TitleBarStyle::Overlay`，保留原生红黄绿窗口控制与系统窗口能力，同时让 Web UI 背景延伸到标题栏下方。
  - 不能使用 `TitleBarStyle::Transparent` 作为最终方案：它只让标题栏透明，但仍保留标题栏布局高度，会形成一条占位色带。
  - 启动 handoff 完成、窗口恢复为主界面时再次设置 `TitleBarStyle::Overlay`，避免 `set_decorations(true)` 回到默认可见标题栏或透明但占位的标题栏。
  - Windows host window 从创建到前端 ready handoff 后都必须保持 `decorations(false)`。handoff 期间禁止无条件 `set_decorations(true)`，否则原生标题栏和菜单栏会重新出现。
  - 不提供前端 JS 坐标移动或自定义 `start_window_drag` command；拖拽完全交给 Tauri 的 `data-tauri-drag-region` 原生机制。
  - 本地视觉验证可设置 `BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1` 跳过证书信任预检，避免截图和拖拽验证时触发 macOS 系统授权弹窗；生产默认仍执行证书预检。
- Web UI 层：
  - 当 `isDesktopShell()` 且平台为 `macos` 或 `windows` 时启用自定义 chrome 拖拽区域。
  - 左侧侧栏顶部预留 38px 原生/自定义窗口控制区，避免菜单项压到 macOS traffic lights 或 Windows 自定义控制按钮的视觉节奏。
  - 全局顶部 35px 作为桌面拖拽保留区；内容容器统一从 35px 以下开始，避免透明拖拽层覆盖页面按钮、输入框或筛选栏。
  - 顶部 35px 放置透明 `data-tauri-drag-region` drag strip，只覆盖真实空白区域；左侧侧栏、窗口控制 spacer、导航项、OpenAPI、主题切换均标记 `data-tauri-drag-region`。
  - Tauri 的拖拽区需要接收鼠标事件，不能同时作为覆盖层点击穿透到下方控件；因此顶部区域必须布局上留空，而不是叠在页面内容上。
  - 不再使用额外的左侧全局 wash 覆盖层。真实侧栏和页面内容各自绘制背景，避免绝对定位渐变层越过 50px 主导航并污染二级菜单区域。
  - 拖拽区域 CSS 设置 `user-select: none` / `-webkit-user-select: none`，避免拖拽时进入文本选择状态。
  - 不再通过 `screenX/screenY` 手动计算并设置窗口位置，避免 Retina、外接显示器、物理/逻辑像素比例导致窗口跟鼠标不同步。
  - 暗色/亮色主题沿用现有侧栏和顶部 wash 的 token 分支，保证 overlay 标题栏下方颜色与 UI 一致。

## 依赖项

- Tauri v2 WindowBuilder `title_bar_style` / `hidden_title`。
- Tauri `data-tauri-drag-region` 原生拖拽区域。
- Web 运行时已有 `isDesktopShell()` 与 `getDesktopPlatform()`。

## 测试方案

- 单元测试：
  - `web/src/runtime.test.ts` 验证 macOS 平台归一化，确保 Rust 返回 `macos` / `darwin` 时 Web UI 都进入桌面 macOS 分支。
  - `web/src/runtime.test.ts` 验证 Windows 平台归一化，确保 Rust 返回 `windows` / JS 返回 `win32` 时 Web UI 进入桌面 Windows 分支。
  - `desktop/src-tauri/src/main.rs` 单测验证 Windows 主界面 handoff 保持无边框 decorations。
  - `pnpm --dir web exec tsc --noEmit --pretty false` 覆盖 Layout 中 `data-tauri-drag-region` 与 CSSProperties 变更。
- E2E / 真实场景：
  - 使用临时数据目录和 `BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1` 启动 macOS 桌面端，观察标题栏已不再出现灰色系统栏。
  - 在 Windows 11 VM 中构建并启动当前桌面端，等待前端 ready handoff 后截图确认没有原生标题栏/菜单栏，底部状态栏完整可见。
  - 在左侧菜单、顶部 35px 区域拖拽，确认窗口由系统接口跟随鼠标移动，不出现倍速或分辨率比例偏差。
  - 拖拽导航项、OpenAPI、主题切换等非输入元素，确认可作为拖拽起点；普通点击仍正常响应。
  - 分别切换暗色/亮色主题，并逐页截图 Network、Replay、Rules、Values、Scripts、AI、DevTools、Notify、Settings，确认顶部 35px 保留区一致，且页面控件从保留区下方开始。
- 回归：
  - 浏览器 CLI Web UI 不启用拖拽 handler，不展示 macOS window control spacer。

## Review/Fix/Test 闭环方案

第 1 轮复核 Rust 窗口样式、Web 拖拽过滤和 macOS 主题视觉；运行 targeted unit test、Tauri check 和桌面启动验证。

第 2 轮复核真实截图、拖拽行为、CLI Web UI 隔离和前一个桌面更新回归；复跑受影响测试后再提交。

## 校验要求

- `pnpm --dir web run test:unit -- src/runtime.test.ts`
- `pnpm --dir web run build:desktop`
- `cargo check --manifest-path desktop/src-tauri/Cargo.toml`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- 按修改范围执行 E2E 与 human_tests。
