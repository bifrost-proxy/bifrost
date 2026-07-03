# CLI 原生托盘 Helper 方案

> 状态：v1 已在 macOS / Windows 落地；Linux 明确不支持。

## 背景

Bifrost 有两类启动形态：

- **Desktop**：桌面壳负责拉起内嵌 `bifrost` 后端，并承载窗口生命周期，有自己的 tray/window。
- **CLI/脚本**：用户直接跑 `bifrost start` 或脚本启动服务；历史上没有系统托盘 / 菜单栏入口。

CLI 形态用户实际最想要的操作（打开 Traffic/Rules/Settings、复制代理地址、快速切换启用规则、Stop/Start、看日志）都在服务运行中，但没有 always-visible 入口。本方案在 CLI 形态里加一个“菜单栏/notification area”常驻 helper：

- **平台**：Windows + macOS；Linux v1 明确不支持。
- **形态**：`bifrost` 二进制内置隐藏子命令 `bifrost __tray`，`start` 在服务 ready 后以独立子进程 spawn。
- **依赖红线**：明确禁止 Tauri / Wry / WebView / 大型 GUI 运行时；允许 `tray-icon` / `muda` / `tao` / `image` / `open` / `arboard` / `ureq` 这些体积可控的轻量库。
- **分发**：保持单一 `bifrost` 二进制。

## 用户目标验证清单

### 必须实现

- macOS 上 `bifrost start` 后菜单栏出现 Bifrost 图标；Windows 上 notification area 出现图标。
- 默认菜单：Bifrost 状态标题、Open Traffic / Rules / Settings、Copy HTTP Proxy、Rules 快速切换、Start / Stop、System Proxy 开关、Open Logs、Update to vX、Quit Tray。
- Rules 子菜单支持个人规则单选、组规则（Owner/Master 权限）单选与取消，最近 5 条快捷区。
- `bifrost start --no-tray` 或 `BIFROST_DISABLE_TRAY=1` 明确禁用本次托盘。
- `config.toml` 的 `[tray] enabled = false` 持久禁用，Settings > Proxy 提供开关，运行中的 helper 探测到禁用后主动退出；开关重新打开后不重启服务也能重新拉起 helper。
- 同一数据目录只允许一个 helper：`start` spawn 前先探测 `tray.lock`；helper 内部 `TrayLock::acquire` 兜底。
- helper 缺失或启动失败时打 warning，主服务继续运行。
- `Stop Bifrost` 走可信 `bifrost stop`；`Start Bifrost` 走 `bifrost start --daemon --no-tray --no-system-proxy` 并保留必要参数（`--host`、`--socks5-port`、`--log-level`、`--skip-cert-check`、`--unsafe-ssl`、`--yes`），daemon child 承接长期 runtime。
- Linux 上 CLI help 不暴露 `--no-tray`；`start` 不尝试 spawn helper；安装脚本不打包 helper。
- 菜单渲染完全走 `MenuDataSnapshot` 快照，UI 事件循环禁止同步等待 Admin API。
- Windows helper 使用 `#![cfg_attr(windows, windows_subsystem = "windows")]`，不弹 console window；Explorer 重启后图标能恢复。
- macOS 使用 template image（`assets/trayTemplate.png` / `@2x.png`），无 Dock 图标。

### 必须不破坏

- 主服务代理业务语义、启动路径其余部分零改动；helper 缺失时代理仍能被使用。
- `runtime.json` 契约不变；helper 只读 runtime，写自己的 `tray.pid` / `tray.log` / `tray.lock` / `tray_recent_rules.json`。
- Admin API 契约不变；helper 走的都是已存在的 `/api/rules/*`、`/api/group*`、`/api/proxy/system`、`/api/config/tls`、`/api/system/overview` 等。
- Desktop 形态的托盘 / window 不受影响。
- Windows daemon 模式（`bifrost start -d`）仍在 detached child 里承接 runtime，并在 child 内按配置拉起 tray helper。

### 必须真实验证

- macOS release 二进制启动 `bifrost start`，menu bar 出现图标、点击菜单展开、`Open Traffic` 打开 admin URL、`Copy HTTP Proxy` 剪贴板正确、`Stop Bifrost` 停服务、`Quit Tray` 不停服务。
- Windows 前台启动：`tray.pid` 存在、helper 进程存活、notification area 图标可见、无控制台窗口。
- Explorer 重启：Windows 图标能恢复（收到 `TaskbarCreated` 广播）。
- Settings > Proxy 关掉 Tray Icon：helper 主动退出；开回来不重启服务也能拉起 helper。
- CLI 重复启动：`tray.lock` 被持有时跳过 spawn，stale `tray.pid` 不误挡启动。
- 主服务 Admin API 卡顿：菜单仍能从快照快速展开。
- macOS `Physical footprint < 30 MB`（release helper）；`ps RSS` 约 50-56 MB（AppKit 基线）；冷启动到图标可见 ≤ 1 秒。

## 产品语义

### 总体架构

```text
用户脚本 / Shell
      │
      ▼
bifrost start
      │  1. 启动代理主服务
      │  2. 写入 runtime.json
      │  3. Admin API / 监听端口 ready
      │  4. 平台+配置允许托盘
      ▼
spawn bifrost __tray  (detached child, 单二进制)
      │  data-dir / runtime.json / tray.lock / tray.log
      ▼
轻量原生托盘 helper
      │  ├─ macOS: AppKit NSStatusItem + NSMenu
      │  └─ Windows: Shell_NotifyIconW + hidden HWND + popup menu
      ▼
菜单动作
      ├─ 打开 URL / 目录
      ├─ 复制代理地址
      ├─ localhost Admin API
      └─ bifrost stop / restart / start
```

原则：

- 主服务是控制面真相源，helper 只是 user-session UI adapter。
- helper 不持有代理监听 socket，不参与流量转发。
- helper 不嵌入 WebView，不启动浏览器内核。
- helper 走 `runtime.json` + localhost Admin API + 当前 CLI binary。

### 平台矩阵

| 平台 | 是否分发 helper | 自动启动 | CLI help 暴露 `--no-tray` |
|---|---|---|---|
| macOS | 是（同一 `bifrost` 二进制） | 是 | 是 |
| Windows | 是（同一 `bifrost.exe` 二进制） | 是 | 是 |
| Linux | 否 | 否 | 否 |

### 关闭途径

- 一次性：`bifrost start --no-tray` 或 `BIFROST_DISABLE_TRAY=1 bifrost start`。
- 持久：`config.toml` 的 `[tray] enabled = false`；Settings > Proxy 的 Tray Icon 开关；`PUT /_bifrost/api/config/tray`。
- 恢复：`PUT /_bifrost/api/config/tray { enabled: true }` → Admin API 调用 CLI 注入的 tray launch 回调，复用 `bifrost start` 的启动路径。

## 技术细节

### 工程结构

```text
crates/bifrost-cli/src/
  commands/start.rs               # start ready 后调用 tray_launcher
  commands/tray_launcher.rs       # 平台 gating、helper 查找、spawn、错误降级
  commands/tray/
    mod.rs                        # run_if_tray_process 入口（在 clap 前短路）
    cli.rs                        # TrayArgs
    tray.rs                       # NativeTray 事件循环
    menu.rs                       # build_menu
    runtime.rs                    # RuntimeInfo + ServiceState
    config.rs                     # tray.json 加载/校验
    lock.rs                       # tray.lock
    dashboard.rs / system_stats.rs / tray_tests.rs
```

### 平台抽象

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

- `menu_model` 只产出平台无关菜单树；`actions` 只处理语义 + 安全校验；平台文件只负责原生 UI + 事件循环 + shell/clipboard。
- 测试可用 `FakeNativeShell` / `FakeNativeMenu` 覆盖 action，不需要真实托盘。

### 依赖策略

**允许**（体积可控）：`serde`、`serde_json`、`clap`、`tracing`、`tracing-appender`、`fs2`、`tray-icon`、`muda`、`tao`、`image`、`open`、`arboard`、`ureq`。

**禁止**：`tauri`、`wry`、任意 WebView / 内嵌浏览器内核、任意重型 GUI 运行时。

Rationale：红线是包体积与运行时开销；轻量库 vs. 手写平台分支实测差异小，跨平台库更利于维护。

### 平台实现要点

**macOS**：`NSApplication::sharedApplication` + `NSApplicationActivationPolicyAccessory`（无 Dock）+ `NSStatusBar::systemStatusBar` + `statusItemWithLength(NSVariableStatusItemLength)` + `NSMenu` + template image。菜单点击走 Objective-C target/action 分发。可选 native 状态项承载 CPU/Memory/Disk/Upload/Download 两行系统状态（`BIFROST_TRAY_NATIVE_STATS_VIEW=0` 兜底回退到 tray-icon bitmap）。

**Windows**：`RegisterClassW` + hidden `HWND` + `Shell_NotifyIconW(NIM_ADD)` + `NOTIFYICON_VERSION_4` + `CreatePopupMenu` + `TrackPopupMenu`。监听 `RegisterWindowMessageW("TaskbarCreated")` 广播以在 Explorer 重启后 `NIM_ADD`。打开 URL/目录用 `ShellExecuteW`，剪贴板用 `OpenClipboard`/`SetClipboardData`。**禁止**为读系统代理 spawn `reg.exe` / `powershell.exe` / `cmd.exe`，必须走无窗口 registry API，否则托盘常驻会反复闪窗。

**Linux**：no-op module，允许 workspace 单测通过；手动跑 helper 返回明确错误。

### Rules 快速切换

- 个人规则候选：`GET /_bifrost/api/rules/reference-candidates`（`group_name=null`）。
- 组权限：`GET /_bifrost/api/group`（`level >= 1`）。
- 组规则：`GET /_bifrost/api/group-rules/{group_id}`。
- 当前启用：`GET /_bifrost/api/rules/active-summary`。勾选状态必须以 active-summary 为准，`group-rules.enabled` 可能滞后。
- 切换：`PUT /_bifrost/api/rules/{name}/enable|disable`、`PUT /_bifrost/api/group-rules/{group}/{rule}/enable|disable`。
- 单选语义：点击未启用规则先禁用菜单快照中除目标外的已启用规则再启用目标；点击当前已启用则 disable，回到 `Rules: None`。**禁止**一次点击禁用所有候选。
- 最近 5 条：`tray_recent_rules.json` 只记最近成功切换目标，去重；渲染时置于 Rules 子菜单顶部；个人规则显示规则名，组规则显示 `组名/规则名`。

### 菜单响应性隔离

- UI 事件循环只读最近一次 `MenuDataSnapshot`，禁止直接调 Admin API。
- 快照由后台线程刷新：runtime、custom tray config、规则菜单、system proxy、binary 可用性。
- 首次 tray icon 使用本地快速快照，不等远端。
- 后台刷新慢或失败保留旧快照。
- 菜单 action 的网络操作放后台线程，通过下一次快照更新勾选。
- Regression：模拟只监听但不响应的 Admin API 端口，断言快速菜单快照和菜单构建不会等 HTTP read timeout。

### 单实例锁

- `start` spawn 前探测 `tray.lock`；被持有则跳过 spawn 并记录 `tray helper already running; skipping launch`。
- `tray.pid` 只作诊断，不能单独作为权威判据（防 stale pid / PID 复用）。
- helper 内部 `TrayLock::acquire` 兜底并发竞态。

### 自定义菜单 `tray.json`

- `open_admin_route`：只允许 `/` 开头相对路径。
- `open_url`：只允许 `http://` / `https://`；可选企业 allowlist。
- `copy_text`：模板变量 `{admin_url}` / `{http_proxy}` / `{socks5_proxy}` / `{data_dir}`。
- `admin_api`：只允许 localhost + allowlist path + `GET`/`POST`。
- **禁止**：`shell` / `exec` / `powershell` / `osascript` / 任意外部二进制执行 / 任意文件读取 / 非 localhost API。
- 文件读取上限 1 MiB；非法或超限保留默认菜单 + 记 `tray.log`。

### Local Admin Client

用 `ureq`（不引入 `reqwest` 重型异步栈）：只允许 `127.0.0.1` / `localhost` / `[::1]`，只支持 `http://`；连接超时 1 秒、读取超时 3 秒；响应体上限 1 MiB；JSON 只解析需要字段。

### 打包与门禁

单二进制分发（macOS `bifrost`、Windows `bifrost.exe`）。helper 查找顺序：`BIFROST_TRAY_BIN` → 当前 `bifrost` 可执行文件。

体积/内存门禁：

| 指标 | 目标 |
|---|---|
| macOS release binary 增量 | 超过需分析依赖来源 |
| Windows release exe 增量 | 超过需分析依赖来源 |
| macOS idle memory | `Physical footprint <= 30 MB`；`ps RSS` 超 30 MB 需记录共享 framework 分析 |
| Windows idle memory | `<= 20 MB`，超过需分析 |
| 冷启动到图标可见 | `<= 1 秒` |

真实实测（macOS arm64 release）：`Physical footprint ~17.8 MB`；`ps RSS ~50-56 MB`（含 AppKit / CoreFoundation / QuartzCore 等系统共享 framework resident 页）。专用瘦 helper binary / 替换 `arboard`+`open` / 手写 AppKit 都无法在 30 MB `ps RSS` 目标下达标——最小原生 AppKit `NSStatusItem` 原型的 `ps RSS` 已经约 40 MB。结论：`Physical footprint` 作为 macOS 内存主验收口径。

## CLI / Web / Admin API 呈现

### CLI

- `bifrost start`：默认尝试拉起 tray（macOS/Windows）；`--no-tray` 禁用本次。
- `bifrost __tray`：隐藏内部子命令；`bifrost start --help` 不推荐用户直接使用。
- `bifrost status`：可选加 `tray: unavailable (helper not found)`。

### Web

- Settings > Proxy > Tray Icon 开关：写入 `[tray].enabled`。
- Web 感知 System Proxy 变化时同步刷新，菜单勾选状态与 UI 保持一致。

### Admin API

- `GET /_bifrost/api/config/tray`：返回 `{ enabled, supported, system_stats_supported, show_system_stats }`。
- `PUT /_bifrost/api/config/tray`：持久化 `enabled`；true→拉起 helper（复用 start 路径），false→helper 主动退出。
- 其它均复用已有 API。

## Sync 边界

- Tray 是本机 UI helper，不参与 sync。
- `[tray].enabled` 是本机配置；`tray.json` / `tray_recent_rules.json` 是本机文件。
- 规则切换调用的是主服务 Admin API；主服务再走它自己的 sync 语义。

## 实现切分

### Phase 1：纯逻辑核心

- `runtime` / `config` / `menu_model` / `actions` / `local_admin` / `lock`。
- 不接 OS tray；`--self-test menu-model` 输出菜单 JSON。
- 单测覆盖。

### Phase 2：macOS 托盘接入

- `tray-icon` / `muda` / `tao` 接入 menu bar；template image；`open` / `arboard`。
- macOS self-test + human_tests。

### Phase 3：Windows 托盘接入

- `tray-icon` / `muda` / `tao` 接入 notification area；处理 Explorer 重启。
- `ShellExecuteW` 打开 URL/目录；Win32 Clipboard 复制。
- Windows self-test + human_tests。

### Phase 4：CLI 启动集成

- `tray_launcher.rs`；start ready 后 spawn helper。
- `--no-tray` + `BIFROST_DISABLE_TRAY` + `BIFROST_TRAY_BIN` + `[tray].enabled`。
- 单实例锁探测。
- helper 缺失 non-fatal warning。

### Phase 5：Rules 快切 + 系统状态 + 配置化启停

- Rules 子菜单 + 最近 5 条快捷区。
- macOS native `NSStatusItem` 承载 CPU/Memory/Disk/Upload/Download。
- `[tray].enabled` API + Settings 开关 + 运行时探测退出。

### Phase 6：Review / Fix / Test 闭环

- 复核依赖红线、菜单 action 安全边界、平台实现。
- 运行 unit + focused CLI tests + macOS/Windows self-test。
- 运行 E2E + workspace all-features。
- 执行 human_tests；记录包体积和内存。

## 测试方案

### 单元

- `runtime.rs`：runtime 正常解析；`0.0.0.0`/`::` 归一化 loopback；IPv6 host URL 加方括号；缺 socks5 时菜单变量为空。
- `config.rs`：合法/非法 `tray.json`；`open_admin_route` 拒绝绝对 URL；`open_url` 拒绝 `file://`；`admin_api` 拒绝非 allowlist 与非 GET/POST；超大文件拒绝；重复 id 拒绝。
- `menu_model.rs`：Running/Stopped 启用/置灰；自定义插入顺序；Rules 两级/三级；勾选与顶层文案；点击已启用规则取消。
- `actions.rs`：Copy 模板展开；Start/Stop 参数构造；Admin API only localhost。
- `local_admin.rs`：请求拼接；响应大小限制；超时与错误归类。
- `lock.rs`：单实例 lock；drop 后释放。
- `tray_launcher.rs`：macOS/Windows 构造 launch plan；Linux 不构造；`--no-tray` / `BIFROST_DISABLE_TRAY` 禁用；helper 缺失 non-fatal warning；`quick_menu_snapshot` 断言慢 Admin API 不阻塞；`existing_tray_helper_pid` 断言 stale pid 不误判、lock 被持有才跳过；`should_launch_tray_disabled_by_config`。

### E2E

- `test_cli_tray_startup_ci.sh`（已落地）：macOS/Windows 起服务后验证 Admin API ready、`runtime.json`、`tray.pid`、`logs/tray.log*`。
- `test_cli_tray_config_reenable.sh`（已落地）：Settings 开关关掉 → helper 退出；打开 → 不重启服务也能重新拉起。
- `test_cli_tray_menu_click_regression.sh`（已落地）：菜单点击后勾选与 recent 快捷区正确刷新。
- Planned：`test_cli_tray_launch_macos.sh` / `test_cli_tray_disable.sh` / `test_cli_tray_missing_helper.sh` / `test_cli_tray_custom_menu.sh`。
- Windows daemon 路径由 `test_windows_daemon_start_e2e.sh` 覆盖。

### 真实场景（human_tests/cli-tray-helper.md，planned）

- **TC-TRAY-MAC-01/02/02A-D/03/04/05/06**：图标出现、Open Traffic、菜单展开不闪烁、Admin API 无响应快速展开、单实例、Settings 开关持久化、复制、Quit Tray 不停服务、Stop Bifrost、非法 tray.json。
- **TC-TRAY-WIN-01/01A/02/02B-D/03/04/05/06**：图标出现、tray.pid 存在、菜单展开、快照快速展开、单实例、Settings 开关、Open Traffic、`--no-tray`、Explorer 重启恢复、无控制台窗口。
- **TC-TRAY-LINUX-01/02**：CLI help 不出现 tray 参数；`bifrost start` 不 spawn helper。

## Review / Fix / Test 闭环

- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- `cargo test -p bifrost-cli tray`、`cargo test -p bifrost-cli tray_launcher`。
- `cargo tree -p bifrost-cli` 搜索禁用依赖：不得包含 Tauri/Wry/WebView。
- macOS/Windows smoke：`bifrost __tray ...` 图标可见 + 菜单可操作。
- E2E：`test_cli_tray_startup_ci.sh` + `test_cli_tray_config_reenable.sh` + `test_cli_tray_menu_click_regression.sh`。
- Human tests：按 `human_tests/cli-tray-helper.md` 逐条执行。
- 包体积：`ls -lh target/release/bifrost*`；内存：macOS `vmmap -summary` / Windows Process Explorer。

## 失败模式

| 场景 | 预期行为 | 测试方式 |
|---|---|---|
| helper binary 缺失 | start warning，服务继续 | E2E `BIFROST_TRAY_BIN` 指向缺失路径 |
| helper 启动失败 | start warning，服务继续 | `BIFROST_TRAY_BIN` 指向失败脚本 |
| runtime.json 不存在 | helper 显示 Disconnected | helper self-test |
| 主服务退出 | helper 菜单进入 Stopped/Disconnected | human_tests + E2E |
| Admin API 不可达 | API 菜单置灰，周期重试 | fake runtime + closed port |
| `tray.json` 非法 | 默认菜单保留，日志记录 | unit + human_tests |
| 重复启动 helper | 后启动者退出 0 | lock unit + E2E |
| Windows Explorer 重启 | tray icon 恢复 | Windows human_tests |
| macOS 无 GUI session | helper 启动失败，主服务不受影响 | CI/headless E2E |

## 风险与决策

- **决策 1**：单二进制分发，不拆瘦 helper。理由：专用瘦 helper 实测 `Physical footprint ~15 MB`、`ps RSS ~50 MB`——没达成 30 MB `ps RSS`；违背单二进制目标。
- **决策 2**：跨平台库 (`tray-icon`/`muda`/`tao`/`open`/`arboard`) 优先于手写平台分支。理由：替换实验对 macOS RSS 基本无收益；平台分支维护成本明显。
- **决策 3**：macOS 内存主口径是 `Physical footprint`。理由：`ps RSS` 含系统共享 framework resident 页；最小原生 AppKit 原型都 40 MB，30 MB `ps RSS` 不具备现实可达性。
- **决策 4**：Windows helper 禁止 spawn `reg.exe` / `powershell.exe` / `cmd.exe` / `cmd /c start`；一律走无窗口 API（registry / `ShellExecuteW`）。理由：Windows Terminal / OpenConsole 会为短命 console 程序创建可见窗口，托盘常驻会反复闪窗。
- **决策 5**：Rules 单选切换禁止一次禁用所有候选。理由：串行触发大量 disable 请求→切换耗时+失败概率高+用户误判没切成。
- **决策 6**：UI 事件循环禁止同步调 Admin API。理由：主服务在高 CPU/规则热重载/Sync 期间可能延迟；同步调用会让菜单卡住甚至崩溃。
- **风险**：macOS `.app` wrapper 差异。macOS bare executable 在极端版本可能有 Dock/activation policy 差异。v1 以 bare binary 为目标；实测后再评估最小 `.app` wrapper。

## 开放问题

1. Windows/macOS 是否默认启动托盘？→ **建议默认启动**，提供 `--no-tray` + `BIFROST_DISABLE_TRAY=1` + Settings 开关三层退出。
2. v1 是否允许企业策略禁用 `open_url` 或限制域名？→ 预留 allowlist，后续实现。
3. Stop 后 helper 是保留 stopped 状态还是延迟自动退出？→ **保留**，因为用户可能立刻想 Start Bifrost。
4. macOS bare executable 是否需要 `.app` wrapper fallback？→ v1 先以普通二进制为目标，实测后再决策。
