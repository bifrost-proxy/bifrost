# CLI 原生托盘 Helper 方案

> 状态：可实施方案，待 Review
> 更新时间：2026-06-10

## 结论

CLI 托盘能力采用 `bifrost` 二进制内置的隐藏 `__tray` 子命令实现：`bifrost start` 在服务 ready 后以独立子进程重入当前二进制运行托盘 helper。实现明确不引入 Tauri、Wry、WebView 等内嵌浏览器内核的重型桌面框架；可以使用 `tray-icon`、`muda`、`tao` 等体积可控的轻量托盘/菜单库。判定标准是“体积与资源开销”，不是“是否第三方”。

v1 通过轻量托盘库（`tray-icon` / `muda` / `tao`）封装操作系统原生能力：

- macOS：AppKit `NSStatusItem`、`NSMenu` 体系，菜单栏图标 + 原生菜单，底层仍是 `NSWorkspace` / `NSPasteboard`。
- Windows：Win32 `Shell_NotifyIconW` 体系，notification area 图标 + 原生 popup 菜单，底层仍是 `ShellExecuteW` / Win32 Clipboard。

这样可以把托盘 helper 控制在“CLI companion”形态：包体积小、依赖少、启动快、空闲资源低，并且不把 GUI 事件循环塞进 Bifrost 代理主进程。

## 背景

Bifrost 目前有两类启动形态：

- Desktop：桌面壳负责拉起内嵌 `bifrost` 后端，并承载窗口生命周期。
- CLI/脚本：用户直接通过 `bifrost start` 或脚本启动服务，当前没有系统托盘/菜单栏入口。

本方案解决 CLI/脚本场景：用户不启动 Desktop，只通过 CLI 启动服务时，也能在 Windows notification area 或 macOS menu bar 看到 Bifrost 图标，并通过菜单完成常用操作。

## 目标

- Windows 和 macOS 支持 CLI 启动后的托盘图标。
- Linux v1 不支持：不打包 helper、不自动启动、不在 Linux CLI help 暴露托盘开关。
- 托盘 helper 是独立进程，由 `bifrost start` 在服务就绪后通过 `bifrost __tray` 拉起；不再分发第二个独立 helper 二进制。
- helper 崩溃、缺失或启动失败时，不影响 Bifrost 主服务运行。
- 默认菜单提供管理端入口、代理地址复制、系统代理切换、重启、停止、打开日志/数据目录等能力。
- 支持 `<data_dir>/tray.json` 受控自定义菜单。
- 全链路可测试：核心逻辑单元测试、CLI 启动 E2E、macOS/Windows 真实托盘 human_tests、包体积和内存实测。

## 非目标

- 不引入 Tauri、Wry、WebView 等内嵌浏览器内核的重型桌面框架。允许使用 `tray-icon`、`muda`、`tao` 等体积可控的轻量托盘/菜单库。
- 不实现 Linux AppIndicator / StatusNotifierItem。
- 不支持任意 shell 菜单项。
- 不补齐 Windows `--daemon`。Windows 脚本启动仍可以是前台服务进程 + 独立 tray helper。
- 不要求 Desktop 复用此 helper。Desktop 托盘体验可以后续单独设计。
- 不提供公开 `bifrost tray` 子命令；托盘入口是内部 `bifrost __tray`，用户入口仍是 `bifrost start`。

## 现状依据

- CLI 运行态已写入 `<data_dir>/runtime.json`，包含 `pid`、`port`、`socks5_port`、`host`、`started_at_ms`、`start_mode`、`binary_path` 等字段。
- `bifrost status`、`stop`、`restart` 已共享运行态与进程检测逻辑。
- Admin API system overview 已返回 `server.admin_url`，默认管理端地址形如 `http://127.0.0.1:<port>/_bifrost/`。
- Web 管理端已有稳定路由：`/traffic`、`/rules`、`/values`、`/scripts`、`/settings` 等。
- 现有资产包含 `assets/bifrost.ico`、`assets/bifrost.icns`、`assets/bifrost.png`、`assets/trayTemplate.png`、`assets/trayTemplate@2x.png`。
- Windows service / Session 0 不适合承载可交互托盘 UI，托盘必须运行在当前交互用户 session。

参考资料：

- Windows `Shell_NotifyIcon`: https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shell_notifyicona
- Windows `NOTIFYICONDATA`: https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataa
- Rust for Windows/windows crate: https://learn.microsoft.com/en-us/windows/dev-environment/rust/rust-for-windows
- macOS `NSStatusItem`: https://developer.apple.com/documentation/AppKit/NSStatusItem
- macOS `NSStatusBar`: https://developer.apple.com/documentation/appkit/nsstatusbar
- Windows Session 0 Isolation: https://techcommunity.microsoft.com/blog/askperf/application-compatibility---session-0-isolation/372361

## 总体架构

```text
用户脚本 / Shell
      |
      v
bifrost start
      |
      | 1. 启动代理主服务
      | 2. 写入 runtime.json
      | 3. Admin API / 监听端口 ready
      | 4. 平台和配置允许托盘
      v
spawn bifrost __tray
      |
      | data-dir / runtime.json / tray.lock / tray.log
      v
轻量原生托盘 helper
      |
      +-- macOS: AppKit NSStatusItem + NSMenu
      |
      +-- Windows: Shell_NotifyIconW + hidden HWND + popup menu
      |
      v
菜单动作
      |
      +-- 打开 URL / 目录
      +-- 复制代理地址
      +-- localhost Admin API
      +-- bifrost stop / restart
```

原则：

- 主服务是控制面真相源，helper 只是 user-session UI adapter。
- helper 不持有代理监听 socket，不参与流量转发。
- helper 不嵌入 WebView，不启动浏览器内核。
- helper 优先通过 `runtime.json`、localhost Admin API、当前 CLI binary 完成操作。

## 进程模型

### `bifrost start` 侧

Windows/macOS 上，`bifrost start` 在服务 ready 后尝试启动 helper：

1. 完成现有 start 流程。
2. 写入 `runtime.json`。
3. 确认 Admin API 可访问，或至少确认代理监听端口 ready。
4. 计算 tray launch plan：
   - 平台是 Windows 或 macOS；
   - 未设置 `--no-tray`；
   - 未设置 `BIFROST_DISABLE_TRAY=1`；
   - 当前 data dir 未存在健康 helper；
   - 能找到当前 `bifrost` 二进制，或 `BIFROST_TRAY_BIN` 指向的兼容 `bifrost __tray` 二进制。
5. 以 detached child process 启动 helper：
   - `__tray`
   - `--data-dir <path>`
   - `--runtime-file <path>`
   - `--parent-pid <pid>`
   - `--bifrost-bin <path>`
   - `--admin-url <url>`，可选

启动失败只打 warning，不让 `bifrost start` 失败。

### `bifrost __tray` 侧

helper 启动后：

1. 初始化文件日志到 `<data_dir>/logs/tray.log`，启动时清理超过 30 天的 `tray.log*` 历史文件。
2. 获取 `<data_dir>/tray.lock`，写入 `<data_dir>/tray.pid`。
3. 读取 runtime，构造初始 `TrayState`；如果 `runtime.json` 暂时缺失但 tray 的父 Bifrost 进程仍存活，则使用启动参数中的 `parent_pid`、`admin_url`、`port` 合成运行态 runtime，避免菜单显示 `Bifrost: Unknown`。
4. 初始化平台托盘图标和菜单。
5. 启动状态刷新定时器：
   - 读取 runtime；
   - 校验主进程 PID；当 runtime 文件缺失但父进程仍存活时，状态保持 Running，并使用启动参数提供的 Admin URL 与端口渲染菜单；
   - 探测 Admin API；
   - 重新渲染菜单启用状态。
6. 进入平台原生事件循环。
7. 退出时删除 tray icon、释放 lock、删除 `tray.pid`。

### 单实例

同一个 `data_dir` 只允许一个 helper：

- 使用 `<data_dir>/tray.lock` 文件锁。
- 获取锁失败时，新 helper 直接退出 0，并记录已有实例。
- stale `tray.pid` 不作为唯一判断，文件锁才是真正单实例依据。

### 服务停止后

v1 语义：

- `Stop Bifrost` 停止主服务，但不退出 helper。
- `Start Bifrost`、`Stop Bifrost` 必须有用户可见的进行中状态：
  - 点击后下一次展开菜单立即显示 `Bifrost: Starting...` / `Stopping...`。
  - 进行中时禁用 Start/Stop，避免重复点击造成并发操作。
  - 成功后回到 Running/Stopped 状态。
  - 失败或超时后显示 `Bifrost: Start failed - open logs` 等错误状态，并允许用户重试或打开日志。
- helper 进入 `Stopped/Disconnected` 状态：
  - 管理端、代理地址、系统代理切换置灰；
  - `Quit Tray` 可用；
  - `Status` 显示最近一次停止或断连原因。
- `Quit Tray` 只退出 helper，不停止主服务。

## CLI 交互面

Windows/macOS：

- `bifrost start` 默认尝试启动托盘。
- `bifrost start --no-tray` 禁用本次托盘。
- `BIFROST_DISABLE_TRAY=1 bifrost start` 禁用托盘，便于 CI、E2E、无头环境。
- `BIFROST_TRAY_BIN=<path>` 指定兼容 `bifrost __tray` 的二进制路径，便于开发测试。

Linux：

- 不在 help 中暴露 `--no-tray`。
- 不自动启动 helper。
- 安装脚本不分发第二个托盘 helper artifact。

不提供公开 `bifrost tray` 子命令。`__tray` 是内部 companion 模式，不是用户主入口。

## 工程结构

CLI 集成：

```text
crates/bifrost-cli/src/
  commands/start.rs          # start ready 后调用 tray_launcher
  tray_launcher.rs           # 平台 gating、helper 查找、spawn、错误降级
  commands/tray/             # 内置 __tray helper：cli/runtime/menu/config/lock/tray
```

设计与验证：

```text
design/
  cli-tray-helper.md

human_tests/
  cli-tray-helper.md
```

## 依赖策略

目标是 helper 体积小、资源开销低、可审计、可裁剪。判定红线是“是否引入内嵌浏览器内核 / 重型 GUI 运行时”，而不是“是否第三方依赖”。体积可控、用途单一的轻量库可以使用。

允许的公共依赖：

- `serde`、`serde_json`：解析 runtime 和 `tray.json`。
- `clap`：解析 helper 内部参数；也可后续改为手写解析进一步瘦身。
- `tracing`、`tracing-subscriber`、`tracing-appender`：沿用项目日志体系。
- `fs2`：跨平台文件锁。
- `tray-icon`、`muda`、`tao`：轻量托盘图标 / 菜单 / 事件循环封装，避免直接手写大量 AppKit / Win32 unsafe 代码。
- `image`：图标 PNG/ICO 解码，体积可控。
- `open`：跨平台打开 URL / 目录。
- `arboard`：跨平台剪贴板。
- `ureq`：极简同步 HTTP client，调用 localhost Admin API（不引入 `reqwest` 这类重型异步栈）。

平台原生 API 仍可按需直接调用（如 macOS 模板图标、Windows PID 检测等），轻量库与原生 API 可以混用，以体积和可维护性取最优解。

明确禁止引入：

- `tauri`
- `wry`
- 任意 WebView / 内嵌浏览器内核依赖
- 任意带完整 GUI 运行时的重型桌面框架

说明：

- 红线是包体积与运行时开销，凡是会拉入浏览器内核（Wry/WebView）或大型 GUI 运行时（Tauri）的依赖一律禁止。
- 上述允许的轻量库（`tray-icon` / `muda` / `tao` / `image` / `open` / `arboard` / `ureq`）经过体积与传递依赖核对，符合“CLI companion”形态要求，可以使用。
- 若后续某个依赖在升级后显著膨胀传递依赖或拉入 WebView/GUI 运行时，需重新评估并替换。
- URL 校验采用受控前缀、host allowlist 和控制字符过滤。

## 平台抽象接口

核心逻辑不直接调用 AppKit 或 Win32，而是通过内部 trait 隔离：

```rust
trait NativeTray {
    fn run(self, initial: TrayState, dispatcher: ActionDispatcher) -> Result<(), TrayError>;
}

trait NativeMenu {
    fn rebuild(&mut self, model: MenuModel) -> Result<(), TrayError>;
    fn set_tooltip(&mut self, text: &str) -> Result<(), TrayError>;
}

trait NativeShell {
    fn open_url(&self, url: &str) -> Result<(), ActionError>;
    fn open_dir(&self, path: &Path) -> Result<(), ActionError>;
    fn copy_text(&self, text: &str) -> Result<(), ActionError>;
}
```

分层：

- `menu_model` 只产出平台无关菜单树。
- `actions` 只处理 action 语义和安全校验。
- `platform/macos.rs` 和 `platform/windows.rs` 只负责原生 UI、事件循环、OS shell/clipboard。
- 测试中可用 `FakeNativeShell` 和 `FakeNativeMenu` 覆盖 action 与菜单状态，不需要真实托盘。

## 平台实现

### macOS

实现文件：`crates/bifrost-cli/src/commands/tray/tray.rs` 与 `crates/bifrost-cli/src/commands/tray/menu.rs`

核心 API：

- `NSApplication::sharedApplication`
- `setActivationPolicy(NSApplicationActivationPolicyAccessory)`
- `NSStatusBar::systemStatusBar`
- `statusItemWithLength(NSVariableStatusItemLength)`
- `NSMenu`
- `NSMenuItem`
- `NSImage`
- `NSWorkspace`
- `NSPasteboard`

实现要点：

- AppKit 必须在主线程初始化并运行事件循环。
- helper 不显示 Dock 图标，不创建窗口。
- 使用 `assets/trayTemplate.png` / `assets/trayTemplate@2x.png`，通过 `NSImage` 设置 template image，适配深浅色菜单栏。
- 菜单点击通过 Objective-C target/action 分发到 Rust action dispatcher。
- 状态刷新通过 AppKit timer 或后台线程向主线程投递刷新事件，所有 UI 更新在主线程执行。
- 打开 URL 使用 `NSWorkspace`。
- 打开目录使用 `NSWorkspace`。
- 复制文本使用 `NSPasteboard`。

macOS 最小验收：

- 直接运行 `bifrost __tray ...` 能创建 menu bar status item。
- 无 Dock 图标。
- 点击图标展示菜单。
- 退出 helper 后图标消失。

风险：

- 如果 bare executable 在某些 macOS 版本上无法稳定隐藏 Dock 或加载资源，再评估最小 `.app` wrapper。但 v1 目标仍是普通 helper binary。

### Windows

实现文件：`crates/bifrost-cli/src/commands/tray/tray.rs` 与 `crates/bifrost-cli/src/commands/tray/menu.rs`

核心 API：

- `RegisterClassW`
- `CreateWindowExW`
- `DefWindowProcW`
- `Shell_NotifyIconW`
- `NOTIFYICONDATAW`
- `RegisterWindowMessageW("TaskbarCreated")`
- `CreatePopupMenu`
- `AppendMenuW`
- `TrackPopupMenu`
- `DestroyMenu`
- `ShellExecuteW`
- `OpenClipboard` / `EmptyClipboard` / `SetClipboardData`

实现要点：

- 创建隐藏窗口承接 tray callback message。
- `Shell_NotifyIconW(NIM_ADD)` 添加托盘图标。
- 添加后调用 `NIM_SETVERSION`，使用 `NOTIFYICON_VERSION_4`。
- 处理左键和右键点击，弹出原生菜单。
- 菜单 command id 映射到内部 action id。
- 处理 Explorer 重启：收到 `TaskbarCreated` 广播后重新 `NIM_ADD`。
- 退出时 `NIM_DELETE` 删除图标。
- Windows helper 使用 `#![cfg_attr(windows, windows_subsystem = "windows")]`，避免弹出控制台窗口。
- 打开 URL/目录使用 `ShellExecuteW`。
- 复制文本使用 Win32 Clipboard API。

Windows 最小验收：

- 直接运行 `bifrost.exe __tray ...` 后 notification area 出现图标。
- 点击图标展示原生菜单。
- Explorer 重启后图标能恢复。
- 退出 helper 后图标消失。

### Linux

实现文件：`crates/bifrost-cli/src/commands/tray/mod.rs`

语义：

- Linux 不打包、不自动启动。
- 为了 workspace test 友好，可以保留 no-op/unsupported module，使 crate 在 Linux 上能跑纯逻辑单元测试。
- 如果开发者手动运行 helper，返回明确错误：`tray is not supported on Linux yet`。

## 图标资源

macOS：

- 编译时嵌入 `assets/trayTemplate.png` 和 `assets/trayTemplate@2x.png`。
- 通过 `NSImage::initWithData` 加载。
- 设置 `setTemplate(true)`。

Windows：

- 通过 Windows resource 嵌入 `assets/bifrost.ico`。
- `build.rs` 只在 Windows target 编译资源。
- 运行时通过 `LoadIconW` 或 `LoadImageW` 从资源加载 `HICON`。

图标解码使用轻量 `image` crate（PNG/ICO），体积可控；macOS 模板图标可在解码后做像素处理以适配深浅色菜单栏。

## 状态模型

核心状态：

```rust
enum ServiceState {
    Starting,
    Running,
    Stopped,
    Disconnected,
    Error(String),
}

struct TrayState {
    data_dir: PathBuf,
    runtime_file: PathBuf,
    bifrost_bin: PathBuf,
    runtime: Option<RuntimeInfo>,
    service_state: ServiceState,
    admin_url: Option<String>,
    http_proxy_url: Option<String>,
    socks5_proxy_url: Option<String>,
    system_proxy_state: Option<SystemProxyState>,
    last_error: Option<String>,
}
```

刷新规则：

- 每 1 秒读取 runtime 并校验 PID，后台轮询是 v1 的状态新鲜度来源。
- `tray-icon 0.19` 在 macOS/Windows 上会在原生点击回调中同步弹出菜单，没有暴露“菜单展示前”钩子；因此不再承诺点击图标时能刷新本次已弹出的菜单。
- 纯托盘图标点击事件只能被 drain，不能触发 `set_menu` 重建；否则 macOS 原生菜单会在刚弹出时被替换，表现为菜单闪烁后立即消失。
- 菜单动作完成后立即刷新；规则切换动作完成后通过内部刷新标记触发下一轮菜单重建。
- 连续失败时保留最近一次可用 runtime，用 disabled menu 表达不可用状态。

PID 校验：

- macOS 使用与 CLI 一致的进程检测逻辑，避免 zombie 误判。
- Windows 使用 Win32 `OpenProcess` / `GetExitCodeProcess`，不用 `tasklist`。
- 需要尽量复用或下沉 `bifrost-cli` 现有 process state 逻辑，避免 status/stop/tray 判断漂移。

URL 拼接：

- `RuntimeInfo` 对 `0.0.0.0`、`::`、空 host 统一映射到 `127.0.0.1`。
- 真实 IPv6 字面量必须在 URL authority 中自动加方括号，例如 `http://[::1]:8800/_bifrost/`。

## 默认菜单

默认菜单：

```text
Bifrost: Running on 127.0.0.1:8800
Open Admin UI
Open Traffic
Open Rules
Copy Admin URL
Copy HTTP Proxy
Copy SOCKS5 Proxy
Rules: <当前启用规则>
Stop Bifrost
System Proxy
Open Logs
Quit Tray
```

状态规则：

- `Bifrost: ...` 是 disabled title item。
- Running 时启用依赖服务的菜单项。
- Stopped/Disconnected 时置灰：
- `Open Admin UI`
- `Open Traffic`
- `Open Rules`
- `Rules: ...`
- `Copy Admin URL`
- `Copy HTTP Proxy`
- `Copy SOCKS5 Proxy`
- `System Proxy`
- `Stop Bifrost`
- `Open Logs`、`Quit Tray` 始终可用。

动作规则：

- `Open Admin UI`：打开 `admin_url`。
- `Open Traffic`：打开 `admin_url + traffic`。
- `Open Rules`：打开 `admin_url + rules`。
- `Copy Admin URL`：复制 `admin_url`。
- `Copy HTTP Proxy`：复制 `http://<host>:<port>`。
- `Copy SOCKS5 Proxy`：复制 `socks5://<host>:<socks5_port>`；统一代理模式下没有独立 `socks5_port` 时 fallback 到主代理端口，只在服务未运行或 SOCKS 未启用时置灰。
- `System Proxy`：原生 check item，读取 `GET /_bifrost/api/proxy/system`；点击后调用 `PUT /_bifrost/api/proxy/system` 写入 `{ "enabled": <next> }`，刷新后更新勾选状态。未运行或平台不支持时置灰。
- `Rules: <当前启用规则>`：原生子菜单，完全通过主服务 Admin API 读取与切换规则，不直接读写 `rules/` 或状态文件。
- `Stop Bifrost`：调用可信 `bifrost stop` 并等待子进程退出，避免 Unix zombie。
- `Start Bifrost`：调用可信 `bifrost start --no-tray --no-system-proxy`，并保留原启动必要参数（例如 `--host`、`--socks5-port`、`--log-level`、`--skip-cert-check`、`--unsafe-ssl`、`--yes`），监控 runtime PID ready；若子进程提前退出或 15 秒内未 ready，状态行显示失败并引导打开日志。
- `Open Logs`：打开 `<data_dir>/logs`。
- `Quit Tray`：退出 helper，不停止服务。

### Rules 快速切换菜单

第一版只支持单选语义：用户点击某一条规则后，tray 先通过 Admin API 禁用菜单中已知的其他规则，再启用目标规则。这样代理运行态、WebUI、Badge、规则热更新和同步缓存仍由主服务统一处理。

数据来源：

- 个人规则候选：`GET /_bifrost/api/rules/reference-candidates`，只取 `group_name=null` 的本地个人规则；个人规则必须以本机数据为准。
- 组权限列表：`GET /_bifrost/api/group`，以远端返回的用户权限为准，`level >= 1`（Owner/Master）才进入 tray 组菜单。
- 组规则列表：对每个可展示组调用 `GET /_bifrost/api/group-rules/{group_id}`，以远端接口同步后的组规则为准；本地组目录不能作为组权限或组列表来源。
- 当前启用规则：`GET /_bifrost/api/rules/active-summary`，用于标记勾选状态和顶层 `Rules: <当前启用规则>` 文案。
- `active-summary` 必须在没有 Sync session 或远端 group cache 解析失败时保留本地组规则 fallback；否则 tray 点击本地组规则后会把顶层错误刷新成 `Rules: None`。
- 个人规则切换：`PUT /_bifrost/api/rules/{rule_name}/enable|disable`。
- 组规则切换：`PUT /_bifrost/api/group-rules/{group_name_or_id}/{rule_name}/enable|disable`。

展示层级：

- 只有个人规则时：`Rules: <当前启用规则>` 作为第一级，悬浮展开后第二级直接展示个人规则列表。
- 存在组规则时：第一级仍是 `Rules: <当前启用规则>`；第二级展示 `My Rules` 与 Web UI `Groups` 页 `Managed` 区域一致的组名；第三级展示对应规则列表。
- 组权限判断来自 `GET /_bifrost/api/group`，与 Web UI 保持同一语义：`level >= 1`（Owner/Master）才属于 `Managed`。本地 `rules/` 目录存在但不在 Managed 列表里的组、普通 Member 组（`level=0`）以及 Discover/Public 组（`level=null`）都不直接展开到 tray。
- 本地旧组候选若存在，只作为 `More...` 的触发 marker，不作为可展开组名；例如远端返回 `next-agent` Master 时必须展示 `next-agent`，而不是本地残留目录 `nextoncall`。
- 若存在未展示的组规则，Rules 子菜单底部展示 `More...`，点击后打开 Admin Rules 页面，由管理端承载完整组规则浏览与测试。
- 当前启用规则显示原生 check mark；无启用规则时顶层显示 `Rules: None`；多条规则同时启用时顶层显示 `Rules: Multiple`，点击任意规则后收敛为单选。

## 自定义菜单

配置文件：`<data_dir>/tray.json`

```json
{
  "version": 1,
  "items": [
    {
      "id": "settings",
      "label": "Open Settings",
      "action": {
        "type": "open_admin_route",
        "route": "/settings"
      }
    },
    {
      "id": "docs",
      "label": "Open Bifrost Docs",
      "action": {
        "type": "open_url",
        "url": "https://github.com/bytedance/bifrost"
      }
    },
    {
      "id": "copy-admin",
      "label": "Copy Admin URL",
      "action": {
        "type": "copy_text",
        "text": "{admin_url}"
      }
    }
  ]
}
```

v1 action：

- `open_admin_route`
  - 只允许 `/` 开头的相对路径。
  - 禁止 `http://`、`https://`、`file://`。
- `open_url`
  - 只允许 `http://` 和 `https://`。
  - 可选支持企业 allowlist。
- `copy_text`
  - 支持模板变量 `{admin_url}`、`{http_proxy}`、`{socks5_proxy}`、`{data_dir}`。
- `admin_api`
  - 只允许 localhost Admin API。
  - 只允许 allowlist path。
  - 只允许 `GET` 或 `POST`。

禁止：

- `shell`
- `exec`
- `powershell`
- `osascript`
- 任意外部二进制执行
- 任意文件读取
- 非 localhost API

配置加载失败：

- 保留默认菜单。
- 记录 `tray.log`。
- 菜单中可选显示 disabled item：`Custom menu failed to load`。

## Local Admin Client

使用极简同步 HTTP client（`ureq`），不引入 `reqwest` 这类重型异步栈。

约束：

- 只允许 `127.0.0.1`、`localhost`、`[::1]`。
- 只支持 `http://`，不支持 TLS。
- 只支持固定路径、固定 method（`GET` / `POST`），连接超时 1 秒、读取超时 3 秒。
- 响应体限制大小，例如 1 MiB。
- JSON 解析只解析需要字段。

用途：

- 获取 system overview。
- 查询系统代理状态。
- 执行 allowlist 内的系统代理开关或 refresh。

如果 Admin API 不可达：

- 不阻塞事件循环。
- 菜单置灰并记录最近错误。
- 下个刷新周期重试。

## 安全边界

- helper 不执行用户自定义 shell。
- `tray.json` 只允许受控 action。
- `tray.json` 读取上限为 1 MiB，超限时 fail closed 并保留默认菜单。
- Admin API action 只允许 localhost。
- helper 使用 `--bifrost-bin` 参数调用 stop/restart，不盲目信任可被篡改的 runtime `binary_path`。
- 如果 `--bifrost-bin` 不存在或不是文件，Start/Stop 菜单置灰。
- helper 不提升权限，不请求管理员权限。
- helper 不修改系统代理底层实现，只调用现有 API/CLI。
- data dir 权限异常时 fail closed：不加载自定义菜单，不执行危险动作。

## 包体积与内存门禁

实现阶段必须实测并记录单二进制引入托盘能力后的增量影响：

```bash
cargo build --release --bin bifrost
ls -lh target/release/bifrost*
```

macOS：

- 记录 release binary size。
- 记录 `strip` 后 size。
- 启动 helper 后记录 idle RSS/private memory。

Windows：

- 记录 release exe size。
- 记录 symbols 分离后的 exe size。
- 启动 helper 后记录 working set/private bytes。

初始目标：

| 指标 | 目标 |
| --- | --- |
| macOS release binary 增量 | 超过需分析依赖来源 |
| Windows release exe 增量 | 超过需分析依赖来源 |
| macOS idle memory | <= 30 MB，超过需分析 AppKit/依赖开销 |
| Windows idle memory | <= 20 MB，超过需分析 Win32/event loop 开销 |
| 冷启动到图标可见 | <= 1 秒 |

如果指标不达标：

- 优先去掉 `clap`、重型日志 appender、非必要 JSON/URL 依赖。
- 保持不用 Tauri/WebView 的原则不变。

## 打包与安装

构建产物：

- macOS: `bifrost`
- Windows: `bifrost.exe`

安装布局：

```text
<install_dir>/
  bifrost
```

Windows：

```text
<install_dir>/
  bifrost.exe
```

helper 查找顺序：

1. `BIFROST_TRAY_BIN` 指向的兼容 `bifrost __tray` 二进制
2. 当前 `bifrost` 可执行文件

找不到 helper：

- `bifrost start` 打 warning。
- 服务继续运行。
- `status` 可选显示：`tray: unavailable (helper not found)`。

## 失败模式

| 场景 | 预期行为 | 测试方式 |
| --- | --- | --- |
| helper binary 缺失 | start warning，服务继续运行 | E2E 将 `BIFROST_TRAY_BIN` 指向缺失路径 |
| helper 启动失败 | start warning，服务继续运行 | `BIFROST_TRAY_BIN` 指向失败脚本 |
| runtime.json 不存在 | helper 显示 Disconnected | helper self-test |
| 主服务退出 | helper 菜单进入 Stopped/Disconnected | human_tests + E2E |
| Admin API 不可达 | API 菜单置灰，周期重试 | fake runtime + closed port |
| `tray.json` 非法 | 默认菜单保留，日志记录 | unit + human_tests |
| 重复启动 helper | 后启动者退出 0 | lock unit + E2E |
| Windows Explorer 重启 | tray icon 恢复 | Windows human_tests |
| macOS 无 GUI session | helper 启动失败且主服务不受影响 | CI/headless E2E |

## 测试设计

### 单元测试

`crates/bifrost-cli/src/commands/tray`：

- `runtime.rs`
  - runtime 正常解析。
  - `0.0.0.0` / `::` 归一化为 loopback admin URL。
  - IPv6 host URL 自动加方括号。
  - 缺少 socks5 时菜单变量为空。
- `config.rs`
  - 合法 `tray.json`。
  - 非法 JSON。
  - 不支持 version。
  - `open_admin_route` 拒绝绝对 URL。
  - `open_url` 拒绝 `file://`。
  - `admin_api` 拒绝非 allowlist path。
  - `admin_api` 拒绝非 `GET` / `POST` method。
  - 超大 `tray.json` 被拒绝。
  - 重复 id 拒绝。
- `menu_model.rs`
  - Running 菜单启用状态。
  - Stopped 菜单置灰状态。
  - 自定义菜单插入顺序。
  - 只有个人规则时 Rules 菜单为两级。
  - 存在组规则时 Rules 菜单为三级，`My Rules` 与组名平级。
  - 当前启用规则显示在顶层 `Rules: ...` 且对应子项勾选。
  - action id 到 platform command id 映射稳定。
- `actions.rs`
  - Copy 模板展开。
  - Start/Stop 参数构造。
  - Admin API action 只允许 localhost。
- `local_admin.rs`
  - HTTP 请求拼接。
  - 响应大小限制。
  - 超时和错误归类。
  - 自定义 `admin_api` 基于 `/_bifrost/` admin base 拼接 URL。
- `lock.rs`
  - 单实例 lock。
  - drop 后释放。
- `tray_launcher.rs`
  - macOS/Windows 构造 launch plan。
  - Linux 不构造 launch plan。
  - `--no-tray` 禁用。
  - `BIFROST_DISABLE_TRAY=1` 禁用。
  - helper 缺失返回 non-fatal warning。

### 平台适配测试钩子

为了让原生 UI 可测，helper 增加内部测试参数：

- `--self-test platform`
  - 初始化平台 tray。
  - 成功添加图标后写 ready file。
  - 2 秒后删除图标退出。
- `--self-test menu-model`
  - 根据 fake runtime 输出菜单 JSON，不进入事件循环。
- `--test-ready-file <path>`
  - 托盘图标添加成功后写入该文件。
- `--test-command-log <path>`
  - 菜单 action 分发时写入 action id，便于自动化断言。

这些参数只用于测试，不在用户文档中主推。

### E2E 测试

新增脚本建议：

- `e2e-tests/tests/test_cli_tray_launch_macos.sh`
  - macOS only。
  - 使用临时 `BIFROST_DATA_DIR`。
  - 设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
  - `cargo run --bin bifrost -- start --no-system-proxy`。
  - 验证 helper ready file、`tray.pid`、`tray.log`。
  - 验证 `bifrost stop` 后 helper 进入 stopped 状态。
- `e2e-tests/tests/test_cli_tray_disable.sh`
  - macOS/Windows。
  - `--no-tray` 不启动 helper。
  - `BIFROST_DISABLE_TRAY=1` 不启动 helper。
- `e2e-tests/tests/test_cli_tray_missing_helper.sh`
  - helper 不存在时 start 成功但 warning。
- `e2e-tests/tests/test_cli_tray_custom_menu.sh`
  - 写入合法和非法 `tray.json`。
  - 使用 `--self-test menu-model` 断言菜单模型。

Windows E2E：

- 在 Windows runner 上执行 `bifrost.exe __tray ...` 的平台 smoke 验证。
- 验证 ready file。
- 验证 `bifrost start` 不依赖 daemon mode。
- 验证 helper 不弹出 console window。

### 真实场景测试

实现阶段必须新增 `human_tests/cli-tray-helper.md` 并执行。

macOS 用例：

- `TC-TRAY-MAC-01`：CLI 启动后 menu bar 出现 Bifrost 图标。
- `TC-TRAY-MAC-02`：点击 `Open Admin UI` 打开管理端。
- `TC-TRAY-MAC-02A`：点击 menu bar 图标后默认菜单保持展开，不闪烁消失。
- `TC-TRAY-MAC-03`：`Copy HTTP Proxy` 后剪贴板内容正确。
- `TC-TRAY-MAC-04`：`Quit Tray` 不停止主服务。
- `TC-TRAY-MAC-05`：`Stop Bifrost` 停止主服务，helper 进入 stopped 状态。
- `TC-TRAY-MAC-06`：非法 `tray.json` 不破坏默认菜单。

Windows 用例：

- `TC-TRAY-WIN-01`：CLI 启动后 notification area 出现 Bifrost 图标。
- `TC-TRAY-WIN-02`：点击托盘图标展示菜单。
- `TC-TRAY-WIN-03`：`Open Admin UI` 打开管理端。
- `TC-TRAY-WIN-04`：`--no-tray` 不出现托盘图标。
- `TC-TRAY-WIN-05`：Explorer 重启后图标恢复。
- `TC-TRAY-WIN-06`：helper 不弹出 console window。

Linux 用例：

- `TC-TRAY-LINUX-01`：Linux `bifrost start --help` 不出现 tray 相关参数。
- `TC-TRAY-LINUX-02`：Linux `bifrost start` 不尝试启动 helper。

## 验证矩阵

| 类型 | 命令/方式 | 通过标准 |
| --- | --- | --- |
| fmt | `cargo fmt --all -- --check` | 无 diff |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 无 warning |
| unit | `cargo test -p bifrost-cli tray` | 全部通过 |
| CLI focused | `cargo test -p bifrost-cli tray_launcher` | 全部通过 |
| 依赖红线 | `cargo tree -p bifrost-cli` 后搜索禁用依赖 | 不包含 Tauri/Wry/WebView 等浏览器内核或重型 GUI 运行时；轻量托盘库（tray-icon/muda/tao 等）允许 |
| macOS smoke | `target/debug/bifrost __tray ...` | 菜单栏图标出现且菜单可操作 |
| Windows smoke | `bifrost.exe __tray ...` | notification area 图标出现且菜单可操作 |
| E2E | 新增 tray e2e 脚本 | 全部通过 |
| human_tests | `human_tests/cli-tray-helper.md` | 每条用例真实执行通过 |
| workspace | `cargo test --workspace --all-features` | 全部通过 |
| 包体积 | `ls -lh target/release/bifrost*` | 符合门禁或有分析 |
| 内存 | macOS/Windows 原生工具 | 符合门禁或有分析 |

## 实施拆解

### Phase 1：纯逻辑核心

- 在 `crates/bifrost-cli/src/commands/tray` 内实现托盘纯逻辑核心。
- 实现 `runtime`、`config`、`menu_model`、`actions`、`local_admin`、`lock`。
- 不接 OS tray，先通过 `--self-test menu-model` 输出菜单 JSON。
- 完成单元测试。

### Phase 2：macOS 原生托盘

- 实现 `platform/macos.rs`。
- 接入 AppKit status item。
- 接入菜单、打开 URL/目录、剪贴板。
- 完成 macOS self-test 和 human_tests。

### Phase 3：Windows 原生托盘

- 实现 `platform/windows.rs`。
- 接入 hidden HWND、`Shell_NotifyIconW`、popup menu。
- 处理 Explorer 重启。
- 接入 `ShellExecuteW` 和 Clipboard。
- 完成 Windows self-test 和 human_tests。

### Phase 4：CLI 启动集成

- 新增 `tray_launcher.rs`。
- `start` ready 后 spawn helper。
- 添加 Windows/macOS `--no-tray`。
- 添加 env gate：`BIFROST_DISABLE_TRAY=1`、`BIFROST_TRAY_BIN`。
- Linux 不暴露参数、不启动 helper。
- helper 缺失时 non-fatal warning。

### Phase 5：安装与文档

- 更新 macOS/Windows 安装脚本，分发 helper。
- 更新 README CLI 托盘说明。
- 新增 `human_tests/cli-tray-helper.md` 并更新索引。
- 记录包体积和内存实测结果。

### Phase 6：Review/Fix/Test 闭环

- 第 1 轮：
  - 复核目标、依赖禁用清单、平台实现、菜单 action 安全边界。
  - 运行 unit + focused CLI tests + macOS self-test。
  - 修复发现问题。
- 第 2 轮：
  - 复查 diff、安装脚本、README、human_tests、包体积和内存数据。
  - 运行 E2E + workspace all-features。
  - 执行 macOS/Windows human_tests。
- 若第 2 轮仍发现功能缺口、测试失败或依赖超标，继续追加第 3 轮。

## 交付定义

实现完成必须满足：

- Windows/macOS CLI start 能自动拉起原生托盘 helper。
- Linux 不暴露、不启动、不打包。
- 没有引入 Tauri/Wry/WebView 等浏览器内核或重型 GUI 运行时。
- helper 缺失或失败不影响主服务。
- 默认菜单和自定义菜单安全可控。
- Start/Stop/Quit Tray 语义清晰且通过验证。
- 包体积和内存有真实测量数据。
- 单元测试、E2E、human_tests、workspace tests 完成并记录。

## 开放问题

1. Windows/macOS 是否默认启动托盘？本方案建议默认启动，提供 `--no-tray` 和 `BIFROST_DISABLE_TRAY=1`。
2. v1 是否要允许企业策略禁用 `open_url` 或限制域名？本方案预留 allowlist，但可后续实现。
3. Stop 后 helper 是一直保留 stopped 状态，还是延迟自动退出？本方案建议保留。
4. macOS bare executable 如果在某些版本出现 Dock/activation policy 差异，是否接受最小 `.app` wrapper fallback？本方案先以普通二进制为目标，实测后再决策。
