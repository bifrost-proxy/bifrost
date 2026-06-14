# CLI 原生托盘 Helper 方案

> 状态：可实施方案，待 Review
> 更新时间：2026-06-11

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
- 托盘 helper 是独立进程，由 `bifrost start` 在服务就绪后通过当前 `bifrost __tray` 拉起；最终分发保持单一 `bifrost` 二进制，不引入第二个 helper artifact。
- helper 崩溃、缺失或启动失败时，不影响 Bifrost 主服务运行。
- 默认菜单提供管理端入口、代理地址复制、系统代理切换、重启、停止、打开日志/数据目录等能力。
- 支持 `<data_dir>/tray.json` 受控自定义菜单。
- 全链路可测试：核心逻辑单元测试、CLI 启动 E2E、macOS/Windows 真实托盘 human_tests、包体积和内存实测。

## 非目标

- 不引入 Tauri、Wry、WebView 等内嵌浏览器内核的重型桌面框架。允许使用 `tray-icon`、`muda`、`tao` 等体积可控的轻量托盘/菜单库。
- 不实现 Linux AppIndicator / StatusNotifierItem。
- 不支持任意 shell 菜单项。
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
   - `<data_dir>/config.toml` 中 `tray.enabled` 未被设置为 `false`；
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

Windows 约束：`bifrost.exe __tray ...` 必须在进程主线程进入原生事件循环。Windows CLI 为避免深栈初始化问题会把普通 CLI 命令放到 `bifrost-cli-main` worker 线程执行，但隐藏托盘入口必须在 `main()` 最早阶段先派发，不能等到 worker 线程里再调用 `run_if_tray_process()`。否则 Tao/tray event loop 会在非主线程初始化，helper 可能只写出 `bifrost-tray starting` 后立即退出，表现为没有 `tray.pid`、没有 notification area 图标。

### Windows daemon 侧

Windows 不支持 Unix `fork` daemon。`bifrost start -d` 在 Windows 上使用当前 exe 重新启动一个 detached child：

1. 父进程完成参数解析和启动前门禁。
2. 父进程移除 child argv 中的 `-d` / `--daemon`，避免递归启动 daemon parent。
3. 父进程设置 `BIFROST_WINDOWS_DAEMON_CHILD=1`，使用 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` spawn child，并把 stdout/stderr 写入 `<data_dir>/logs/bifrost.log` / `bifrost.err`。
4. child 复用前台启动路径，但写入 `runtime.json` 时记录 `runtime_start_mode=daemon`。
5. 父进程等待 proxy listener ready；child 提前退出或超时则父进程返回非零，不打印成功 PID。

### 单实例

同一个 `data_dir` 只允许一个 helper：

- 使用 `<data_dir>/tray.lock` 文件锁。
- 获取锁失败时，新 helper 直接退出 0，并记录已有实例。
- stale `tray.pid` 不作为唯一判断，文件锁才是真正单实例依据。

### 服务停止后

v1 语义：

- `Stop Bifrost` 停止主服务后，helper 会进入空转宽限期；如果 10 分钟内没有新的服务启动并恢复 Running，helper 自动退出并清理 `tray.pid`，避免菜单栏长期残留失效图标。
- 如果宽限期内用户点击 `Start Bifrost`，helper 不会在启动中途退出；服务恢复 Running 后计时清零。如果启动失败，空转计时从失败后的 Stopped 状态重新开始。
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
- `config.toml` 的 `[tray] enabled = false` 持久禁用托盘，CLI start 必须跳过启动；运行中的 tray helper 轮询到该配置后必须主动退出。
- WebUI Settings > Proxy 在 System Proxy 配置前提供 Tray Icon 开关，写入同一份 `[tray]` 配置。
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

平台原生 API 仍可按需直接调用（如 macOS 模板图标、Windows PID 检测等），轻量库与原生 API 可以混用，以体积和可维护性取最优解。打开 URL/目录继续使用跨平台 `open` crate，复制文本继续使用跨平台 `arboard` crate；实测手写 `/usr/bin/open`、`ShellExecuteW`、`pbcopy`、Win32 Clipboard 对 macOS RSS 基本无收益，不采用。

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
- 菜单动作完成后立即刷新；规则切换动作完成后必须主动重新读取 Admin API 快照并提升菜单数据 generation，再触发下一轮菜单刷新，避免用户刚切换后再次打开菜单仍看到旧状态。
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

第一版支持单选与取消语义：菜单构建时只把当前已启用规则目标放入 action。用户点击未启用规则后，tray 先通过 Admin API 禁用当次菜单快照中除目标外的已启用规则，再启用目标规则；禁止把菜单中的所有候选规则都当成待禁用对象，否则一次点击会串行触发大量个人/组规则 disable 请求，导致切换耗时、失败概率升高，并让用户再次打开菜单时误以为没有切成功。用户再次点击当前已启用规则时，tray 只调用该规则的 disable API，将规则状态清理为无启用规则。这样代理运行态、WebUI、Badge、规则热更新和同步缓存仍由主服务统一处理。

数据来源：

- 个人规则候选：`GET /_bifrost/api/rules/reference-candidates`，只取 `group_name=null` 的本地个人规则；个人规则必须以本机数据为准。
- 组权限列表：`GET /_bifrost/api/group`，以远端返回的用户权限为准，`level >= 1`（Owner/Master）才进入 tray 组菜单。
- 组规则列表：对每个可展示组调用 `GET /_bifrost/api/group-rules/{group_id}`，以远端接口同步后的组规则为准；本地组目录不能作为组权限或组列表来源。
- 当前启用规则：`GET /_bifrost/api/rules/active-summary`，用于标记勾选状态和顶层 `Rules: <当前启用规则>` 文案。
- `active-summary` 必须在没有 Sync session 或远端 group cache 解析失败时保留本地组规则 fallback；否则 tray 点击本地组规则后会把顶层错误刷新成 `Rules: None`。
- 个人规则切换：`PUT /_bifrost/api/rules/{rule_name}/enable|disable`。
- 组规则切换：`PUT /_bifrost/api/group-rules/{group_name_or_id}/{rule_name}/enable|disable`。
- 最近规则快捷区：helper 在数据目录维护 `tray_recent_rules.json`，只记录最近成功切换到的规则目标，去重后最多保留 5 个。渲染时 Rules 子菜单顶部先展示这些快捷项；个人规则显示规则名，组规则显示 `组名/规则名`，保证同名规则有区分度。已删除或当前菜单不可见的规则不展示。

展示层级：

- 只有个人规则时：`Rules: <当前启用规则>` 作为第一级，悬浮展开后第二级直接展示个人规则列表。
- 存在组规则时：第一级仍是 `Rules: <当前启用规则>`；第二级展示 `My Rules` 与 Web UI `Groups` 页 `Managed` 区域一致的组名；第三级展示对应规则列表。
- 组权限判断来自 `GET /_bifrost/api/group`，与 Web UI 保持同一语义：`level >= 1`（Owner/Master）才属于 `Managed`。本地 `rules/` 目录存在但不在 Managed 列表里的组、普通 Member 组（`level=0`）以及 Discover/Public 组（`level=null`）都不直接展开到 tray。
- 本地旧组候选若存在，只作为 `More...` 的触发 marker，不作为可展开组名；例如远端返回 `next-agent` Master 时必须展示 `next-agent`，而不是本地残留目录 `nextoncall`。
- 若存在未展示的组规则，Rules 子菜单底部展示 `More...`，点击后打开 Admin Rules 页面，由管理端承载完整组规则浏览与测试。
- 当前启用规则显示原生 check mark；无启用规则时顶层显示 `Rules: None`；多条规则同时启用时顶层显示 `Rules: Multiple`，点击未启用规则后收敛为单选并刷新最近规则快捷区；再次点击当前已启用规则后取消选中并回到 `Rules: None`。
- 规则切换成功后必须立即调用同一套 `load_menu_data_snapshot` 刷新当前 helper 内存快照，而不是只等待 1 秒后台轮询；否则原生菜单关闭后用户马上再次打开，容易看到切换前的 `Rules: ...` 与勾选状态。

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
- 启动 helper 后同时记录 `ps RSS` 与 `vmmap -summary` 的 `Physical footprint`。
- `ps RSS` 会包含 AppKit、Objective-C runtime、CoreFoundation 等共享 framework resident 页，适合用于发现异常增长，但不作为 30 MB 独占内存硬门禁。
- `Physical footprint` 更接近 helper 对系统的独占物理占用，作为 macOS idle memory 的主要验收口径。

Windows：

- 记录 release exe size。
- 记录 symbols 分离后的 exe size。
- 启动 helper 后记录 working set/private bytes。

初始目标：

| 指标 | 目标 |
| --- | --- |
| macOS release binary 增量 | 超过需分析依赖来源 |
| Windows release exe 增量 | 超过需分析依赖来源 |
| macOS idle memory | `Physical footprint` <= 30 MB；`ps RSS` 超过 30 MB 时必须记录共享 framework 分析 |
| Windows idle memory | <= 20 MB，超过需分析 Win32/event loop 开销 |
| 冷启动到图标可见 | <= 1 秒 |

如果指标不达标：

- 优先去掉 `clap`、重型日志 appender、非必要 JSON/URL 依赖。
- tray helper 必须复用内部 Admin API HTTP agent，常驻后台线程使用显式小栈，tray 文件日志 non-blocking 队列使用小缓冲；远端组权限接口失败后短暂退避，避免未登录或远端不可用时每秒重复请求和写 warning。
- 如果业务要求 `ps RSS` 也稳定低于 30 MB，当前跨平台 tray 基础库 + macOS AppKit 基线下没有低风险可行方案；专用 helper binary 与替换 `open`/`arboard` 已试验但不采用，原生 AppKit/Win32 也不能承诺达到 30 MB。
- 保持不用 Tauri/WebView 的原则不变。

### macOS 内存实测与进一步优化空间

本地 release 实测结论：

- `target/release/bifrost`：约 110 MB；`strip` 后临时副本约 92 MB。
- release helper 真实启动并连接临时 Bifrost 服务：`ps RSS` 启动后约 38 MB，12 秒后约 56 MB。
- 仅启动 `bifrost __tray`、不连接有效 Admin API：5 秒后 `ps RSS` 约 55 MB，说明规则菜单、缓存和 Admin API 不是 RSS 主因。
- `strip` 后以同样方式启动 helper：5 秒后 `ps RSS` 仍约 55 MB，说明符号段不是 RSS 主因。
- `vmmap -summary <tray_pid>`：`Physical footprint` 约 17.8 MB，dirty heap 约 11.9 MB；`ps RSS` 的高值主要来自 AppKit/Objective-C runtime/CoreFoundation/QuartzCore 等系统共享 framework resident 页和通用 CLI 二进制映射。
- `otool -L target/release/bifrost` 显示当前二进制链接 AppKit、Foundation、ApplicationServices、CoreGraphics、Carbon、QuartzCore、Metal、CoreData、CoreText、CoreImage、CloudKit、Security、SystemConfiguration 等系统库。
- `cargo tree -p bifrost-cli` 显示通用 CLI 二进制仍包含 `bifrost-admin`、`bifrost-asr`、`reqwest`/`tokio`、`clap`、`tao`/`muda`/`tray-icon` 等大依赖；虽然 `__tray` 入口在 clap、全局日志和 crypto provider 前短路，但同一大二进制的代码和系统 framework 映射仍会进入 RSS 口径。
- 试验过专用瘦 helper binary：release helper 体积可降到约 5.2 MB；进一步移除 `arboard`/`open` 后约 4.9 MB。真实接入 `BIFROST_TRAY_BIN` 并连接临时 Bifrost 服务后，12 秒 `ps RSS` 仍约 50.6 MB，`Physical footprint` 约 14.9 MB。该路线需要分发第二个二进制，且没有达成 `ps RSS < 30 MB`，因此不采用。
- 试验过把 `arboard` 替换为 `/usr/bin/pbcopy`/Win32 Clipboard，以及把 `open` 替换为 `/usr/bin/open`/`ShellExecuteW`。替换后 helper 依赖树中不再包含 `arboard`/`open`，但 12 秒 `ps RSS` 仍约 50.6 MB，`Physical footprint` 约 14.8-14.9 MB。由于收益不明显且增加平台分支，恢复跨平台 `arboard` 与 `open`。
- 用 Objective-C 编写的最小原生 AppKit `NSStatusItem + NSMenu` 原型，不连接 Bifrost、不使用 Rust tray 库，8 秒后 `ps RSS` 约 40.4 MB，`Physical footprint` 约 13.1 MB。该基线说明 macOS AppKit 菜单栏程序的 `ps RSS` 本身已经高于 30 MB。

代码级定位：

| 位置 | 当前行为 | 内存影响判断 | 可实施动作 |
| --- | --- | --- | --- |
| `crates/bifrost-cli/src/main.rs` | `commands::tray::run_if_tray_process()` 已在 panic hook、crypto provider、clap parse、主日志初始化前执行 | 启动初始化路径已经足够早，继续挪入口收益很小 | 保持早返回；后续瘦身不应回退这个入口顺序 |
| `crates/bifrost-cli/Cargo.toml` | `bifrost-cli` 同时直接链接 `bifrost-admin`、`bifrost-proxy`、`bifrost-sync`、`bifrost-asr`、`reqwest`、`tokio`、TUI 等主 CLI 依赖 | 即使 `__tray` 不执行这些代码，单二进制仍带来较大的代码段、链接段和系统库映射；但专用 helper binary 试验没有达成 RSS 目标且违背单二进制分发 | 保持单二进制分发，不采用拆 helper |
| `crates/bifrost-cli/src/commands/tray/tray.rs` | Tray 直接使用 `tao` 事件循环、`tray-icon` 图标、`muda` 菜单、`image` 解码、`open` 打开 URL/目录、`arboard` 剪贴板、`ureq` Admin API | `tao`/`tray-icon`/`muda` 是原生 UI 常驻开销；`open`/`arboard` 替换试验对 macOS RSS 基本无收益；`ureq` 是合适的轻量 HTTP client | 保持当前跨平台基础库实现，不再做无收益的平台分支替换 |
| `crates/bifrost-cli/src/commands/tray/menu.rs` | 只构造平台无关菜单模型和 action | 逻辑轻，但抽 crate 对 RSS 目标无直接帮助 | 不为内存目标拆 crate |
| `crates/bifrost-cli/src/commands/tray/config.rs` 与 `runtime.rs` | 只依赖 `serde`/`serde_json` 和少量平台 PID 检测 | 逻辑轻，保持当前模块即可 | 不为内存目标拆 crate |
| `crates/bifrost-cli/src/commands/tray_launcher.rs` | `tray_enabled_by_config` 通过 `toml::from_str::<bifrost_storage::UnifiedConfig>` 读取 `[tray].enabled` | 单二进制下这不是额外依赖来源 | 保持当前实现 |
| `crates/bifrost-cli/src/commands/tray/tray.rs::http_agent` | 仅为直连 localhost Admin API 使用 `bifrost_core::direct_ureq_agent_builder()` | 单二进制下 `bifrost-core` 已是 CLI 依赖；替换无 RSS 收益 | 保持当前实现 |
| `assets/trayTemplate@2x.png` / `assets/bifrost.ico` | 运行时用 `image::load_from_memory` 解码为 RGBA，再交给 `tray-icon::Icon` | `image` 预计主要影响二进制体积，对 macOS RSS 收益有限 | 不作为当前优化方向 |

不损害功能前提下的优化分层：

1. **已落地的运行时瘦身**：复用 `ureq::Agent`，缩小 tray 日志 non-blocking 队列，常驻/动作线程使用 512 KiB 小栈，远端 group 接口失败后 5 秒退避。这些改动降低长期分配和异常场景日志压力，但不能把 macOS `ps RSS` 从 55 MB 降到 30 MB 以下。
2. **低风险继续优化**：减少菜单快照中的临时 `String` clone、把远端 group 成功结果缓存到下一轮、对 rule/system proxy polling 做按需或低频刷新。预期改善 dirty heap 和 CPU/网络抖动，RSS 口径收益有限。
3. **不采用：专用瘦 helper binary**：实测能把 helper 二进制降到约 4.9 MB、`Physical footprint` 降到约 15 MB，但 macOS `ps RSS` 仍约 50 MB；同时违背单二进制分发目标，因此不采用。
4. **不采用：外围依赖裁剪**：移除 `arboard` 与 `open` 后，macOS RSS 基本无变化，且平台分支增加维护成本，因此恢复跨平台库。
5. **暂不采用：原生平台 API helper**：macOS 直接使用 AppKit `NSStatusItem`/`NSMenu`，Windows 直接使用 Win32 `Shell_NotifyIconW`/popup menu，移除 `tao`/`tray-icon`/`muda` 通用事件循环。Objective-C 最小原型显示 macOS 原生 AppKit status item 本身 `ps RSS` 约 40 MB，因此该路线可能再降低约 10 MB，但不能承诺达到 30 MB；会显著增加 unsafe/platform glue、菜单刷新一致性和 Windows Explorer 重启恢复的维护成本。当前继续使用跨平台 tray 基础库。
6. **度量口径方案**：如果产品目标是“真实独占内存小于 30 MB”，当前 release helper 已满足 `Physical footprint < 30 MB`；如果产品目标是“活动监视器/ps RSS 显示小于 30 MB”，macOS AppKit 基线显示 30 MB 目标不具备现实可达性，应改为 `Physical footprint` 或 platform private bytes 作为验收口径。

当前建议：

- v1 保持单二进制 `bifrost __tray`，继续使用跨平台 tray 基础库（`tray-icon` / `muda` / `tao`）以及 `open` / `arboard`。
- 不建议继续为了 `ps RSS < 30 MB` 在当前方案上做小修小补；实测已经证明外围依赖替换和独立 helper 都不能达成该目标。
- macOS 验收口径建议使用 `Physical footprint < 30 MB`，`ps RSS` 只作为诊断指标；如果产品仍坚持用户可见 RSS 口径，需要先接受 AppKit 原生基线约 40 MB 的现实约束。

### 发布包体积与 Rust 编译优化评估

当前需要同时区分三个口径：

1. **本地/CI 原始 release 二进制**：未压缩的 `target/release/bifrost`。开启 `[profile.release] strip = "symbols"` 后，本地 macOS arm64 实测约 95.8 MB；开启前约 110 MB。
2. **打包前二进制**：release profile 已默认 strip，该口径影响 artifact 上传和后续压缩输入。
3. **用户最终下载包**：本地 macOS arm64 release 复测 `.tar.gz` 约 42.7 MB，`.tar.xz` 约 27.5 MB；用户感知下载体积应优先看这个口径。

已采用的低风险优化：

- 根 `Cargo.toml` 设置 `[profile.release] strip = "symbols"`，让本地 release 构建和 CI release 构建默认去除符号，降低原始 artifact 与压缩输入体积。
- Release workflow 在 Unix/macOS package 阶段继续 best-effort 执行 `strip`，作为旧 toolchain 或 profile 未生效时的兜底。
- Unix/macOS CLI 发布包同时产出 `.tar.xz` 与 `.tar.gz`；安装脚本在本机 `tar` 支持 xz 时优先尝试 `.tar.xz`，但把它视为可选优化包：只有快速探测确认资产存在时才下载，缺失时立即回退 `.tar.gz`。Windows 继续使用 `.zip`。
- 内置 `bifrost upgrade` 使用相同候选策略：Unix/macOS 优先 `.tar.xz`，下载失败、镜像不可用或本机禁用 xz 时回退 `.tar.gz`；Windows 仍只使用 `.zip`。
- npm 发布链路继续优先读取 `.tar.gz` 生成平台 npm 包，同时支持 `.tar.xz` 作为 artifact 兜底来源；release job 在发布 npm 前强制校验 Unix/macOS 同时存在 `.tar.gz` 和 `.tar.xz`，Windows 存在 `.zip`。
- 校验文件覆盖 `.tar.gz`、`.tar.xz` 和 `.zip`，不改变既有 Homebrew/npm 仍依赖 `.tar.gz` 的兼容路径。
- 合入主干后 `install-binary.sh` 会立即从 main 生效，但 latest release 可能仍是旧包集合；因此 `.tar.xz` 缺失必须被视为正常兼容场景，脚本必须快速回退旧 `.tar.gz`，并继续使用旧 checksum 校验，不能让线上用户在新 release 发布前无法安装或更新。
- 安装脚本的镜像探测和全镜像下载竞速必须有超时兜底，并且只等待自身启动的下载子进程；否则 `.tar.xz` 缺失或坏包这类优化路径失败，可能卡住而无法进入 `.tar.gz` 稳定路径。

已评估但不默认启用的 Rust 高阶优化：

| 手段 | 预期收益 | 本轮结论 | 后续门槛 |
| --- | --- | --- | --- |
| `lto = "thin"` + `codegen-units = 1` | 可能降低少量代码段体积，并改善跨 crate 优化 | 本地实验编译到 `bifrost-proxy` / `bifrost-admin` 时生成大量 `.rcgu.bc`，在仅剩约 3.7 GiB 可用空间的工作区触发 `No space left on device`；临时磁盘成本明显高于普通 release | 需要在专用 CI runner 或更大磁盘环境单独做 A/B，并同时记录构建耗时、峰值磁盘和最终包体积 |
| `panic = "abort"` | 通常可减少 unwind 相关代码 | 会改变 panic 语义和析构路径，Bifrost 有终端 raw mode、后台任务、日志 flush、测试 panic 断言等路径，不能在未做全量验证前默认开启 | 需要全 workspace 测试、E2E、human_tests 和异常路径验证后再评估 |
| `opt-level = "z"` / `"s"` | 可能进一步缩小二进制 | 可能牺牲代理热路径、脚本执行、ASR/ML 栈性能；当前用户体验更依赖代理吞吐和启动稳定性 | 需要压测代理吞吐、启动耗时、ASR 性能，并比较 `.tar.xz` 最终收益 |
| `split-debuginfo` / 符号分离 | 对 macOS 调试符号管理有价值 | 当前最直接收益已由 `strip = "symbols"` 覆盖；发布包没有携带独立 dSYM/符号包需求 | 如后续需要 crash symbolication，可单独发布符号包而不是放入用户安装包 |

更大体积来源判断：

- macOS arm64 `bifrost-cli` 目前会编译 ASR full-local 能力，`qwen3-asr`、`sherpa-onnx`、`candle-*`、`tokenizers` 等 ML/ASR 依赖是显著体积来源。
- Windows/Linux 已通过 target-gated dependency 和 CI/release workflow 注释约束，不准备也不打包 ASR native runtime。
- 如果未来必须继续显著压缩 macOS arm64 包体积，真正有效的方向不是拆 tray 二进制，而是产品层面决定 ASR/ML 能力是否继续内置在同一个 CLI release 中，或是否改为首次使用时下载/安装本地 ASR runtime。该方向会影响功能可用性，不能作为本次无损优化默认落地。

当前发布策略：

- 保持单 `bifrost` 二进制分发，不拆托盘 helper。
- 默认打开 release symbol strip，并发布 `.tar.xz` 优先下载路径；同时保留 `.tar.gz` 作为 Homebrew、npm、旧安装器和人工下载的兼容包。
- Thin LTO、`panic=abort`、`opt-level=z/s` 暂作为实验项，不进入默认 release profile，避免用构建稳定性和运行时语义换取未经量化的体积收益。

可落地路线：

1. **保持现状：单二进制 + 跨平台 tray 基础库**
   - `bifrost __tray` 继续作为唯一分发形态。
   - 保留 `tray-icon` / `muda` / `tao`、`open`、`arboard`，不引入手写平台分支。
   - 验收重点是行为稳定、菜单交互正确、macOS `Physical footprint < 30 MB`。
2. **已评估但不采用：新增瘦 helper binary**
   - 原因：违背单二进制分发目标，且实测无法达成 macOS `ps RSS < 30 MB`。
3. **已评估但不采用：替换 `open`/`arboard`**
   - 原因：对 macOS RSS 基本无收益，且增加平台分支维护成本。
4. **可选远期方向：原生 AppKit/Win32 helper**
   - 只有产品明确接受平台分支维护成本，且把 macOS 目标调整为接近 40 MB RSS 时再进入。
   - macOS 直接用 `NSStatusItem`、`NSMenu`、`NSWorkspace`、`NSPasteboard`；Windows 直接用 `Shell_NotifyIconW`、hidden HWND、popup menu、`ShellExecuteW`、Win32 Clipboard。
   - 移除 `tao`、`tray-icon`、`muda`，保留 `bifrost-tray-core` 菜单模型和 action 分发。
   - 验收除内存外必须覆盖菜单展开不闪退、后台刷新不关闭菜单、Explorer 重启图标恢复、Settings 开关重新拉起 helper、Rules 单选/取消与最近 5 条快捷项。

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
  - Rules 子菜单顶部展示最近 5 次成功切换到的规则快捷项，个人规则不带组名前缀，组规则显示 `组名/规则名`。
  - 当前启用规则显示在顶层 `Rules: ...` 且对应子项勾选。
  - 再次点击当前已启用规则会取消选中，顶层回到 `Rules: None`。
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

- `e2e-tests/tests/test_cli_tray_startup_ci.sh`
  - macOS/Windows。
  - 用临时 `BIFROST_DATA_DIR` 启动 `bifrost start`，显式携带 `--no-system-proxy` 和 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
  - 自行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost`，避免依赖 CI runner 后续 E2E 的构建顺序，并与现有 shell E2E 的 release binary 约定一致。
  - 通过 Admin API ready、`runtime.json` 端口/PID、`tray.pid` 进程存活、`logs/tray.log*` 启动标记交叉验证；Windows 默认也必须要求 `tray.pid` 存在且 helper 进程存活。仅在明确设置 `BIFROST_TRAY_STARTUP_ALLOW_LOG_ONLY=1` 的已知无交互 runner 上，才允许把 `bifrost-tray starting` 作为诊断性降级信号，不能作为常规 CI 通过条件。
  - 在 `.github/workflows/ci.yml` 的 `e2e-macos-runner` 与 `e2e-windows-runner` 中执行，覆盖 macOS arm64、Windows x64 和 Windows arm64。
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

- 在 Windows runner 上执行 `test_cli_tray_startup_ci.sh` 的平台 smoke 验证。
- 验证 Admin API ready、`runtime.json`、`tray.pid` 和 `tray.log`。
- 验证 `bifrost start` 前台模式能创建可见托盘 helper，且不依赖 daemon mode。
- 执行 `test_windows_daemon_start_e2e.sh`，验证 `bifrost start -d` 会启动 detached child、Admin API ready、`runtime.json` 记录 `runtime_start_mode=daemon`，并且 `stop` 能清理 child。
- 验证 helper 不弹出 console window。

### 菜单响应性隔离

托盘 helper 虽然是独立进程，但菜单渲染线程不能同步等待主进程 Admin API。主进程在高 CPU、规则热重载、Sync 请求或系统代理检查期间可能暂时无法及时响应；如果 helper 在 UI event loop 中同步请求 `/_bifrost/api/*`，用户点击托盘图标时会出现菜单卡住、转圈或长时间无响应。

实现约束：

- 原生 UI event loop 只读取最近一次 `MenuDataSnapshot`，不得直接调用规则、组、active-summary 或 system proxy Admin API。
- `MenuDataSnapshot` 由后台线程刷新，包含 runtime、custom tray config、规则菜单、system proxy 状态和服务控制 binary 可用性。
- 首次创建 tray icon 时使用本地快速快照：读取 runtime/启动参数、tray.json 和 binary 状态，不等待远端规则/组/system proxy API。
- 后台刷新慢或失败时保留旧快照；菜单可以暂时显示旧规则状态或缺省状态，但必须保持可展开、可点击。
- 菜单 action 的网络操作继续放在后台线程，完成后通过下一次快照刷新更新勾选状态。
- 回归测试必须模拟一个只监听但不响应的 Admin API 端口，断言快速菜单快照和菜单构建不会等待 HTTP read timeout。

### 单实例启动保护

同一个 `BIFROST_DATA_DIR` 只能有一个 tray helper。helper 进程内部继续使用 `tray.lock` 做最终互斥；CLI `start` 在 spawn helper 之前也必须先探测同一数据目录下的 `tray.lock` 是否已被持有：

- 如果 `tray.lock` 已被持有，说明已有 helper 正在运行，CLI 直接跳过 helper spawn，并记录 `tray helper already running; skipping launch`。
- `tray.pid` 只作为日志与诊断信息，不能单独作为是否已有 helper 的权威依据，避免 crash 后 stale pid 或 PID 复用误挡启动。
- 如果 `tray.lock` 可获取，说明没有活动 helper；CLI 释放探测锁后正常 spawn helper。
- helper 内部 `TrayLock::acquire` 仍保留，覆盖并发启动竞态。
- 回归测试必须覆盖：仅有 stale `tray.pid` 时不跳过；`tray.lock` 被持有且 `tray.pid` 存在时返回已有 pid 并跳过 spawn。

### 配置化启停

托盘启停必须由统一配置驱动，不能只依赖启动参数：

- `UnifiedConfig` 增加 `[tray] enabled = true`，默认保持现有自动启动体验。
- `GET /_bifrost/api/config` 与 `GET /_bifrost/api/config/tray` 返回 `{ enabled, supported }`。
- `PUT /_bifrost/api/config/tray` 持久化 `enabled`，Settings > Proxy 的 Tray Icon 开关使用该接口。
- `bifrost start` 在 spawn helper 前读取 `config.toml`，若 `tray.enabled = false` 则不创建托盘。
- `bifrost __tray` 启动后和后台快照轮询中都检查配置，发现禁用时退出，保证 WebUI 关闭开关会收敛已有托盘。
- `PUT /_bifrost/api/config/tray` 将 `enabled` 改回 `true` 后，Admin API 必须调用 CLI 注入的 tray launch 回调，复用 `bifrost start` 的启动路径重新创建 helper；该路径继续遵守 Linux 不支持、`--no-tray`、`BIFROST_DISABLE_TRAY=1` 和单实例锁。

### 真实场景测试

实现阶段必须新增 `human_tests/cli-tray-helper.md` 并执行。

macOS 用例：

- `TC-TRAY-MAC-01`：CLI 启动后 menu bar 出现 Bifrost 图标。
- `TC-TRAY-MAC-02`：点击 `Open Admin UI` 打开管理端。
- `TC-TRAY-MAC-02A`：点击 menu bar 图标后默认菜单保持展开，不闪烁消失。
- `TC-TRAY-MAC-02B`：主进程 Admin API 繁忙或无响应时，托盘菜单仍从缓存快速展开。
- `TC-TRAY-MAC-02C`：CLI 重启时，同一数据目录已有 tray helper 则不再创建第二个托盘进程。
- `TC-TRAY-MAC-02D`：Settings > Proxy 关闭 Tray Icon 后，配置持久化且已有托盘退出；重新打开开关后不重启服务也能重新创建托盘；再次关闭后重新 `bifrost start` 不再创建托盘。
- `TC-TRAY-MAC-03`：`Copy HTTP Proxy` 后剪贴板内容正确。
- `TC-TRAY-MAC-04`：`Quit Tray` 不停止主服务。
- `TC-TRAY-MAC-05`：`Stop Bifrost` 停止主服务，helper 进入 stopped 状态。
- `TC-TRAY-MAC-06`：非法 `tray.json` 不破坏默认菜单。

Windows 用例：

- `TC-TRAY-WIN-01`：CLI 启动后 notification area 出现 Bifrost 图标。
- `TC-TRAY-WIN-01A`：CLI 前台启动后 `<data_dir>/tray.pid` 存在且 helper 进程存活；只有 `logs/tray.log*` 中出现 `bifrost-tray starting` 但无活 helper 时判为回归失败。
- `TC-TRAY-WIN-02`：点击托盘图标展示菜单。
- `TC-TRAY-WIN-02B`：主进程 Admin API 繁忙或无响应时，notification area 菜单仍从缓存快速展开。
- `TC-TRAY-WIN-02C`：CLI 重启时，同一数据目录已有 tray helper 则不再创建第二个 notification area helper。
- `TC-TRAY-WIN-02D`：Settings > Proxy 关闭 Tray Icon 后，配置持久化且已有 notification area helper 退出；重新打开开关后不重启服务也能重新创建托盘；再次关闭后重新 `bifrost start` 不再创建托盘。
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
| tray 响应性 | `cargo test -p bifrost-cli quick_menu_snapshot -- --nocapture` | 慢 Admin API 不阻塞快速菜单快照 |
| tray 单实例 | `cargo test -p bifrost-cli existing_tray_helper_pid -- --nocapture` | lock 被持有才跳过 helper spawn，stale pid 不误判 |
| tray 配置化启停 | `cargo test -p bifrost-cli should_launch_tray_disabled_by_config -- --nocapture` + `curl /_bifrost/api/config/tray` | 配置禁用阻止 CLI spawn，WebUI/API 可持久切换 |
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

### Phase 2：macOS 托盘接入

- 通过 `tray-icon` / `muda` / `tao` 接入 macOS menu bar status item。
- 通过 `open` 打开 URL/目录，通过 `arboard` 复制文本。
- 完成 macOS self-test 和 human_tests。

### Phase 3：Windows 托盘接入

- 通过 `tray-icon` / `muda` / `tao` 接入 Windows notification area 图标和 popup menu。
- 处理 Explorer 重启。
- 通过 `open` 打开 URL/目录，通过 `arboard` 复制文本。
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
