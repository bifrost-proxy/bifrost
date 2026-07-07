# Desktop Window Chrome 策略设计方案

## 背景

Bifrost 桌面端基于 Tauri，同一份前端资源需要在 macOS、Windows、Linux 上呈现观感一致但符合各平台原生习惯的窗口装饰。旧版设计文档曾描述“按平台拆分 Tauri 配置 + 前端自绘标题栏”的方案，但仓库中只有单一 [`desktop/src-tauri/tauri.conf.json`](../desktop/src-tauri/tauri.conf.json)，未落地平台化配置，也没有前端 titlebar 抽象层。当前实际策略是运行时代码差异化控制窗口装饰、透明度和背景效果。

本文档描述当前生效方案、能力清单、后续升级路径以及验证方式，避免后续开发者在多个入口重复处理平台差异，或误以为旧“platform-config split + custom titlebar”方案已落地。

2026-07 Windows 回归中确认：桌面端 Windows 主界面已经采用无原生标题栏的自绘 chrome。拖拽区域必须只放在非交互空白区域，不能覆盖左侧 tab、底部主题切换、OpenAPI 按钮或右上角窗口按钮；窗口最小化、最大化、关闭必须走 Tauri 官方 `window.getCurrentWindow()` API，并在 capability 中显式授权对应 `core:window:*` 权限。桌面壳启动 core server 之前必须能写入 `desktop-config.json`，配置写入函数需要自行创建父目录，不能只依赖 setup 调用方提前创建。

## 用户目标验证清单

### 必须实现

- macOS 启动阶段展示原生 launcher overlay：小尺寸、无边框、透明背景、无投影的 `host` 窗口。
- macOS 从 launcher overlay 交接到主界面时，把窗口装饰恢复为原生 traffic light + titlebar，同时把窗口尺寸动画到 `TARGET_WINDOW_WIDTH × TARGET_WINDOW_HEIGHT`。
- Windows 平台直接以正常尺寸和不透明背景启动，不走 launcher overlay handoff；主窗口使用无原生标题栏的自绘 chrome，右上角窗口按钮由前端调用 Tauri 官方 window API。
- Linux 平台直接以标准装饰、正常尺寸和不透明背景启动，不走 launcher overlay handoff。
- Windows 平台在可用时叠加 `Effect::Mica`，其它平台不施加。
- 所有窗口装饰切换必须集中在单一入口：`desktop/src-tauri/src/main.rs`，禁止分散到前端或多份 tauri 配置。
- 平台判定统一通过 `supports_native_launcher()` 语义（当前等价于 `cfg!(target_os = "macos")`），不允许其他模块自行判定 target_os。

### 必须不破坏

- Web 管理端在浏览器中不受影响，不能因为桌面 chrome 策略引入前端自绘标题栏或客户端路由差异。
- Windows 自绘 chrome 只能把 `data-tauri-drag-region` 放在顶部空白拖拽条和非交互 spacer 上，禁止放在交互控件或其父容器上。
- `startDragging()` / `toggleMaximize()` / `minimize()` / `close()` 等 runtime bridge 必须优先使用 Tauri 官方 `window.getCurrentWindow()` 命名空间；`webviewWindow.getCurrentWebviewWindow()` 只作为兼容 fallback。
- 不改变 `INITIAL_WINDOW_WIDTH/HEIGHT`、`TARGET_WINDOW_WIDTH/HEIGHT`、`TARGET_WINDOW_MIN_WIDTH/HEIGHT` 常量语义。
- 启动阶段 native launcher overlay 与 handoff 动画时序不能改成阻塞主线程或阻塞前端首帧。

### 必须真实验证

- macOS 真实桌面观察：从 launcher overlay 到主窗口的尺寸动画、装饰切换、traffic light 出现与否。
- Windows 真实桌面观察：启动即为无原生标题栏的自绘 chrome，右上角窗口按钮可点击，左侧 tab、OpenAPI、底部主题切换可点击，顶部空白区域可拖拽/双击最大化。
- Windows 真实进程/API 验证：桌面壳启动后自动拉起 core server，`/_bifrost/api/proxy/address` 可返回实际代理地址；启动过程不弹 PowerShell 或 console 黑框。
- Linux 真实桌面观察：启动即为标准装饰，无 Mica，无异常透明背景。
- 前端观察 `web/src/desktop/tauri.ts` 中的 window control 桥接接口在没有自绘标题栏时不会误触发。

## 产品语义

### 平台策略统一入口

所有窗口装饰、透明度、阴影、Mica、handoff 动画都集中在 `desktop/src-tauri/src/main.rs`。派生条件：

```rust
fn supports_native_launcher() -> bool {
    cfg!(target_os = "macos")
}
```

其它模块或前端只能通过 IPC/事件调用主进程能力，禁止自行做 `cfg!(target_os = ...)` 分支决定窗口 chrome。

### macOS：Launcher Overlay + Handoff

在 `create_host_window` 中，`supports_native_launcher()` 为 true 时：

- 尺寸 `INITIAL_WINDOW_WIDTH × INITIAL_WINDOW_HEIGHT`（当前 360×260）。
- `decorations(false)`、`transparent(true)`、`shadow(false)`。
- 前端渲染透明 launcher UI（`native_launcher` overlay），承担启动可视化。

当前端 handshake 或超时条件触发 `start_main_window_handoff` 时：

- 调用 `host_window.set_decorations(true)` 恢复原生装饰。
- 调用 `apply_window_effects` 施加平台级视觉效果（macOS 无 Mica，Windows 有 Mica）。
- 通过 `animate_host_window_to_main_size` 逐帧插值动画到 `TARGET_WINDOW_WIDTH × TARGET_WINDOW_HEIGHT`。
- 动画结束固定 `set_size(LogicalSize::new(TARGET_WINDOW_WIDTH, TARGET_WINDOW_HEIGHT))` 兜底，避免舍入误差。

### Windows：直启动自绘 chrome

`supports_native_launcher()` 为 false 时：

- 尺寸直接 `TARGET_WINDOW_WIDTH × TARGET_WINDOW_HEIGHT`，最小尺寸 `TARGET_WINDOW_MIN_*`。
- Windows 通过 `uses_borderless_desktop_chrome()` 返回 true，主窗口 `decorations(false)`，前端渲染右上角窗口按钮。
- 顶部拖拽条高度由 `DESKTOP_TOP_DRAG_HEIGHT` 控制，右侧保留 `WINDOWS_WINDOW_CONTROLS_HIT_TEST_INSET`，避免拖拽区域覆盖窗口按钮。
- `data-tauri-drag-region` 只允许由 `getDesktopDragRegionAttributes(enabled)` 写入非交互区域；交互控件必须使用 `interactive: true` 或不设置 drag region。
- `core:window:allow-minimize`、`core:window:allow-toggle-maximize`、`core:window:allow-close`、`core:window:allow-start-dragging` 必须存在于 desktop capability 中。
- 跳过 launcher overlay 与 handoff 期间的装饰切换。
- Windows 通过 `apply_window_effects` 施加 `Effect::Mica`，不可用时降级为普通背景但不能阻塞启动。

### Linux：直启动标准窗口

`supports_native_launcher()` 为 false 且 `uses_borderless_desktop_chrome()` 为 false 时：

- 尺寸直接 `TARGET_WINDOW_WIDTH × TARGET_WINDOW_HEIGHT`，最小尺寸 `TARGET_WINDOW_MIN_*`。
- `decorations(true)`、`transparent(false)`、正常背景。
- 跳过 launcher overlay 与 handoff 期间的装饰切换。
- Linux 无额外 Mica 效果。

### Windows 前端标题栏约束

`web/src/desktop/tauri.ts` 中的 window control 桥接是 Windows 自绘 chrome 主路径。调用顺序必须优先选择 `window.__TAURI__.window.getCurrentWindow()`，因为这是 Tauri v2 官方 window API；`webviewWindow.getCurrentWebviewWindow()` 只用于兼容旧注入形态。

任何新增拖拽区域都必须先确认其内部没有按钮、tab、开关、输入框或链接。若需要让某一块既能拖拽又包含交互元素，必须把拖拽层与交互层分开，而不是把 `data-tauri-drag-region` 放到父容器。

### Desktop config 与 core server 启动

桌面壳在 setup 阶段按顺序解析 sidecar binary、共享数据目录和 `desktop-config.json`，随后启动托管 core server。首次启动或数据目录缺失时：

- `resolve_desktop_data_dir()` 使用 CLI 共享数据目录，支持 `BIFROST_DATA_DIR` 覆盖。
- `resolve_desktop_config_path(data_dir)` 固定返回 `data_dir/desktop-config.json`。
- `save_desktop_config()` 必须在写入前创建 `config_path.parent()`，避免 setup 在首次启动、临时数据目录或目录被清理后因为 `os error 3` 中断。
- core server 没有启动时要先看 setup panic 和 bootstrap log，不能只看端口监听。

### Install CLI 与 AI skills

桌面浮层和 Settings 的 `Install CLI` / `Install CLI & Skills` 入口都调用 `POST /api/system/cli-install`。该入口的产品语义是“先保证 CLI 可用，再尽力完成 AI skills”：

- CLI 复制成功后必须返回 `installed=true`；AI skills 失败、超时或被用户环境拦截时，只能通过 `skills_installed=false` 与 `skills_message` 告知用户重试，不能让整个请求 500。
- 桌面触发的 skills 安装必须使用随二进制编译进来的 embedded bundle，即传递 `BIFROST_INSTALL_SKILL_SOURCE=embedded`，避免 GitHub raw 429、DNS 慢或离线环境把弹窗请求拖过前端 30 秒超时。
- skills 子进程必须有显式超时；超时后杀掉子进程并保留已安装 CLI。
- Windows 上如果当前运行的 `bifrost.exe` 已经是目标安装路径，复制步骤应视为 no-op，避免对正在运行的 exe 做自覆盖而触发文件锁错误。

## 技术细节

### 常量与尺寸

```rust
const INITIAL_WINDOW_WIDTH: f64 = 360.0;
const INITIAL_WINDOW_HEIGHT: f64 = 260.0;
const TARGET_WINDOW_WIDTH: f64 = 1440.0;
const TARGET_WINDOW_HEIGHT: f64 = 920.0;
const TARGET_WINDOW_MIN_WIDTH: f64 = 1180.0;
const TARGET_WINDOW_MIN_HEIGHT: f64 = 760.0;
```

调整任一常量必须同步：

- macOS launcher 视觉设计稿。
- 首帧前端布局对 `INITIAL_WINDOW_*` 的依赖（避免溢出）。
- 主窗口 `TARGET_WINDOW_*` 与 web layout 断点匹配。

### 关键函数

- `create_host_window(app: &AppHandle) -> tauri::Result<Window>`：唯一窗口创建入口。
- `supports_native_launcher() -> bool`：唯一平台判定入口。
- `apply_window_effects(window: &Window) -> tauri::Result<()>`：平台视觉效果统一入口，内部处理 Mica。
- `start_main_window_handoff(app: &AppHandle, reason: &str)`：从 launcher overlay 切换到主窗口的入口，负责恢复装饰、施加效果、启动尺寸动画。
- `animate_host_window_to_main_size`：使用 easing 逐帧插值，避免瞬时跳变造成视觉断层。

### Handoff 触发时机

- 前端 ready 事件 handshake 完成。
- 启动超时兜底：即使前端未就绪也必须切换到主窗口，避免用户卡在 launcher overlay。
- 显式 handoff：`start_main_window_handoff` 允许携带 `reason: &str` 便于日志追踪。

### 前端交互约束

- Windows 前端使用自绘 titlebar，且只能在顶部非交互区域使用 `data-tauri-drag-region`。
- 浏览器 Web 管理端不使用自绘 titlebar，不应设置 `data-tauri-drag-region`。
- `web/src/desktop/tauri.ts` 中的 window control API 是桌面 shell 专用路径，浏览器环境只能保持不可用 fallback。

## Tauri 配置

保留单一 `desktop/src-tauri/tauri.conf.json`，不拆分 `tauri.macos.conf.json` / `tauri.windows.conf.json`。配置中：

- 默认窗口条目不预设 `TARGET_WINDOW_*`，因为窗口通过 `create_host_window` 运行时创建。
- 保留 window feature、shell allowlist、fs allowlist 等平台无关的能力开关。

如果未来必须拆平台配置，需要同步更新本文档并把 `main.rs` 中相关判定改为 runtime `WindowConfig` 读取，禁止两套真值来源。

## CLI 与 Admin API

窗口 chrome 策略不暴露 CLI 或 Admin API。所有变更通过桌面进程 IPC 完成：

- `set_document_edited(edited: bool)`（详见 `desktop_monaco_edit_commands.md`）复用于 macOS 关闭按钮黄点。
- 未来若增加“隐藏/显示 traffic light”能力，应作为独立 Tauri command 并写入本文档。

## 实现切分

### Phase 1：现状固化

- 集中所有平台判定到 `supports_native_launcher()`。
- `create_host_window` 单入口创建窗口。
- `apply_window_effects` 单入口施加 Mica/无。
- 明确 `INITIAL_WINDOW_*` 与 `TARGET_WINDOW_*` 常量。

### Phase 2：Launcher Overlay handoff 稳定

- Handoff 由前端 ready 或超时兜底触发。
- `animate_host_window_to_main_size` 提供平滑动画。
- macOS/Linux 装饰恢复顺序：先 `set_decorations(true)` 再 `apply_window_effects`。
- Windows handoff 后继续保持 `decorations(false)`，避免原生 caption 与自绘 chrome 同时出现。

### Phase 3：Windows 自绘 chrome 稳定化

- Windows caption 按钮实现必须继续使用 Tauri 官方 window API 与 capability 授权。
- 任何布局改动都必须复查 drag region 与交互控件的 hit-test 边界。
- macOS traffic light 与 Linux WM 装饰保持原生，不复用 Windows 自绘 chrome。

### Phase 4：跨平台观察与遥测

- 记录启动到 handoff 完成耗时，便于回归 launcher overlay 性能。
- 记录 handoff 失败或超时次数。

## 测试方案

### 单元测试

- Rust 侧对 `supports_native_launcher()`、`apply_window_effects` 平台分支的编译期覆盖：在 macOS/Windows/Linux 三个 target 上 `cargo check` 不报警。
- `desktop/src-tauri` 现有测试保持通过：`cargo test -p bifrost-desktop`。

### E2E 测试

- 桌面窗口 chrome 主要依赖 human_tests 真实桌面观察，辅以前端单测覆盖 drag region 属性和 Tauri window bridge 选择。
- 若未来接入 Playwright/自研 desktop harness，需要覆盖：macOS launcher overlay → 主窗口 handoff、Windows 自绘 chrome hit-test、Windows core server 自动启动、Linux 直启动。

### 真实场景测试 human_tests

新增 `human_tests/desktop-window-chrome.md`：

- TC-DWC-01：macOS 启动展示 launcher overlay（透明、无投影、小尺寸）。
- TC-DWC-02：macOS handoff 恢复 traffic light 与装饰，动画平滑到 1440×920。
- TC-DWC-03：Windows 启动直接为无原生标题栏的自绘 chrome，右上角窗口按钮可点击且无 console 黑框。
- TC-DWC-04：Windows 左侧 tab、OpenAPI、底部主题切换可点击，顶部空白区域可拖拽。
- TC-DWC-05：Windows 桌面壳自动启动 core server，Admin API 可返回代理地址。
- TC-DWC-06：Linux 启动直接为标准装饰，无 Mica，无异常透明。
- TC-DWC-07：`INITIAL_WINDOW_*` / `TARGET_WINDOW_*` 常量与真实窗口尺寸一致。

同步更新 `human_tests/readme.md` 对应索引行。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-desktop --all-features`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下不跑 `make coverage`；在交付备注中说明依赖远端 CI 与 human_tests 覆盖。

## 边界与非目标

- 本方案不覆盖前端页面内容渲染，只覆盖窗口装饰。
- 本方案不覆盖多窗口管理（当前 Bifrost 桌面端只有单主窗口 + 系统托盘；托盘由独立模块处理）。
- 本方案不覆盖 macOS Full Screen 模式的特殊行为；进入/退出 full screen 由系统与 Tauri 默认处理，本方案只保证退出后装饰恢复正常。
- 本方案不覆盖 Windows 上的 Fluent Design Acrylic；仅使用 Mica 作为背景效果。
- 本方案仅在 Windows 引入前端自绘 chrome；macOS 与 Linux 不引入自绘 titlebar。
- 本方案不处理隐藏窗口/关闭到托盘的关闭行为（详见 `desktop-macos-close-behavior.md`）。

## CLI / Admin API / Sync 边界（补充）

- 桌面窗口 chrome 策略不进入 rules/values/scripts 数据同步链路。
- 桌面进程重启不改变窗口装饰策略；重启后按当前平台重新走 `create_host_window` 路径。
- 若用户在数据目录里手动修改 `tauri.conf.json` 或添加平台配置文件，`main.rs` 中的运行时策略优先级更高；这是有意为之，避免多份真值来源。

## 与其它设计文档的关系

- `desktop-launcher-startup.md`：launcher overlay 阶段的启动逻辑与本文档协同；本文档覆盖“窗口尺寸/装饰/透明度”这一维度，launcher-startup 覆盖“UI 内容与握手时序”。
- `desktop-macos-close-behavior.md`：关闭到托盘的行为与本文档解耦；关闭窗口不改变启动路径的装饰策略。
- `desktop_monaco_edit_commands.md`：`set_document_edited` 命令是本方案 handoff 完成后长期使用的原生 dirty 状态接口，与窗口装饰无直接耦合但共享桌面进程入口。
- `desktop-runtime-port-switch.md`：切换代理端口不影响窗口 chrome。

## 常见问题排查

- **macOS handoff 后主窗口尺寸不到 1440×920**：检查 `animate_host_window_to_main_size` 是否在动画结束前被取消（例如用户手动 resize），以及末尾 `set_size` 兜底是否被 skip。日志 `frontend ready handshake` 或 `startup timeout fallback` 有助定位入口。
- **launcher overlay 在多显示器上偏移**：确认窗口 `center` 调用是否在正确的 monitor scope 内，必要时改用 `Monitor::from_point` 显式选定主 monitor。
- **Windows Mica 未生效**：`apply_window_effects` 会在不支持 Mica 的驱动上返回错误，进程会记录日志并降级；用户看到的应是自绘 chrome 普通背景窗口而非启动失败。
- **Windows 按钮 hover 但点击无效，双击却最大化**：优先检查交互控件或其父容器是否带有 `data-tauri-drag-region`，以及顶部拖拽条是否给右上角窗口按钮预留 hit-test inset。
- **Windows 只有 UI 没有 core server**：先前台启动桌面 exe 观察 setup panic，再检查 `desktop-config.json` 父目录和 `save_desktop_config()`；如果 setup 已经 panic，sidecar 日志和端口监听都可能不存在。
- **Linux 上 titlebar 显示异常**：确认 `decorations(true)` 由 WM 处理；如某些 GTK/Wayland 组合下渲染异常，方案不做 client-side decoration workaround，需要用户切换 WM 或反馈。
- **`startDragging` 报错**：说明前端在浏览器或非 Tauri 注入环境误触发了桌面路径；需要回到 `isDesktopShell()` 和 window bridge 注入状态排查。

## 测试用例矩阵（补充）

| 平台     | 启动路径                    | 期望装饰             | 期望背景          | 期望尺寸               |
| -------- | --------------------------- | -------------------- | ----------------- | ---------------------- |
| macOS    | launcher overlay            | 无装饰、透明、无投影 | 透明              | 360×260                |
| macOS    | handoff 后                  | 原生 traffic light   | 系统背景          | 1440×920               |
| Windows  | 直启动                      | 自绘 chrome          | Mica/降级背景     | 1440×920               |
| Linux    | 直启动                      | WM 装饰              | 系统默认          | 1440×920               |

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：macOS launcher + handoff、Windows Mica、Linux 直启动、常量集中。
- 复核 diff：`desktop/src-tauri/src/main.rs`、`tauri.conf.json`、`web/src/desktop/tauri.ts`、human_tests。
- 重点 review：是否存在绕过 `supports_native_launcher()` 的分支；是否存在多份窗口创建入口；handoff 装饰恢复顺序是否稳定。
- 复测：桌面手工验证 macOS/Windows/Linux 三平台启动路径。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- 再次确认 `git status --short`、新增 human_tests 索引。
- 重点 review：handoff 动画在低性能设备上的兜底、window effect 施加失败的降级路径、错误日志可读性。
- 复测：真实设备重跑 macOS 与 Windows 上的 launcher overlay 与 handoff。

## 与旧方案的差异

历史版本设计文档曾描述“按平台拆分 Tauri 配置 + 前端自绘标题栏”的方案，本次明确该方案未落地：

- 仓库中只有一个 `desktop/src-tauri/tauri.conf.json`，没有 `tauri.macos.conf.json` / `tauri.windows.conf.json`。
- 没有统一的前端标题栏抽象层。
- `startDragging()` / `toggleMaximize()` 类型定义存在于前端 runtime bridge 中，但当前仓库没有对应的桌面标题栏 UI 作为主路径使用。
- 因此后续若要做跨平台窗口 chrome 统一，建议重新立项，单独定义启动态和正常态的装饰切换规则、macOS / Windows / Linux 的平台差异、以及前端是否需要接管标题栏交互。

## 观察与遥测

为便于诊断 handoff 卡住、Mica 施加失败等问题，桌面进程日志中应输出：

- launcher overlay 创建时的窗口尺寸、透明度、装饰状态。
- `start_main_window_handoff(reason)` 被调用时的 `reason`，例如 `frontend ready handshake` 或 `startup timeout fallback`。
- `apply_window_effects` 成败与降级路径。
- `animate_host_window_to_main_size` 起始与终止帧的实际窗口尺寸，便于对齐真实设备渲染。

日志走当前桌面 tracing subscriber，级别 `info` 或更高，避免污染业务日志。

## 风险与决策点

- macOS launcher overlay 依赖前端 handshake，若前端崩溃需要超时兜底切换主窗口，避免用户永久卡住。当前实现通过 `start_main_window_handoff` 的兜底调用路径完成，任何修改都必须保留超时保护。
- Windows Mica 在部分显卡驱动上可能失败；`apply_window_effects` 失败不应阻断启动，只降级为普通装饰窗口。
- Linux 上不同 WM（GNOME、KDE、Sway）对 `decorations(true)` 的实际渲染差异较大；本方案不处理 client-side decoration，交给 WM。
- 未来是否接管标题栏是产品级决策；一旦启用，本文档需要重新立项并覆盖 traffic light 与 caption button 的实现细节。
- Tauri 版本升级时需要 verify `set_decorations`/`Effect::Mica`/`transparent`/`shadow` API 兼容性，尤其是 tauri 2.x → 后续大版本。
- 若 `INITIAL_WINDOW_*` / `TARGET_WINDOW_*` 常量修改，必须回归 launcher overlay 视觉设计稿与前端布局断点，避免动画结束后主界面被裁切或大幅留白。
- 若多显示器 DPI 差异下 launcher overlay 出现偏移，需要通过 Tauri 的 `set_position` / `center` 兜底而不是引入前端补丁。
