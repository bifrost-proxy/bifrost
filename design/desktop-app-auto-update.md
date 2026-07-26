# 桌面端自动更新与 app 命令

## 背景

Bifrost 桌面端基于 Tauri 打包，但桌面 WebView 与普通 Web UI 共享同一套 CLI/Admin 后端服务。此前 `bifrost upgrade`、Tray、Web UI 更新只面向 CLI 二进制；桌面端安装包需要用户手动下载 `.dmg` 或 `.msi`。这会造成两个问题：

- 桌面端已经提示有新版本，但点击更新时只能更新 CLI，不能更新 `.app` / `.msi` 桌面壳。
- 桌面端内置 sidecar 与用户单独安装的 CLI 是两个安装轨道，不能把二者混成一次 self-update。

本方案新增 `bifrost app install / uninstall / upgrade`，并让桌面 WebView 使用 `channel=desktop` 触发桌面更新。

用户安装路径有两种：

1. **先安装 CLI**：用户在终端安装 `bifrost` 后，可以执行 `bifrost app install` 安装桌面端。
2. **先安装 App**：用户只安装桌面端后，可以在桌面 Settings 中点击 `Install CLI & Skills`，把桌面端内置的 CLI sidecar 安装到用户 CLI 路径，并安装 Bifrost AI skills，方便 Codex/Claude/Trae/Cursor 等 AI 工具调用 `bifrost`。

## 用户目标验证清单

### 必须实现

- 桌面端最多每 6 小时强制检查一次新版本；启动时也会主动检查。
- 桌面端检测到新版本时，在右下角显示通知，并自动打开版本更新窗口。
- 桌面端更新窗口复用 CLI Web UI 的交互：显示下载进度、安装进度和重启进度。
- 桌面端点击更新后调用 `POST /api/system/upgrade?channel=desktop`，后端执行 `bifrost app upgrade --source desktop -y`。
- `bifrost app install` 可安装 macOS `.app` / `.dmg` 与 Windows `.msi` / `.exe` / `.zip`。
- `bifrost app uninstall` 可移除桌面端，Windows 优先调用静默卸载器。
- `bifrost app upgrade` 默认下载 release 中的 `bifrost-desktop-v<version>-<target>.dmg/.msi` 并安装。
- 桌面端 Settings 提供一键安装 CLI 和 AI skills 按钮。
- 桌面端安装完成后自动重启桌面应用。
- 如果存在独立 CLI 安装，桌面端升级时也会更新该 CLI；桌面内置 sidecar 由桌面安装包更新。
- Windows 桌面快捷方式只显示桌面 UI；桌面壳启动内置 `bifrost.exe` sidecar 时使用隐藏控制台进程，不弹出 Windows Terminal / shell 窗口。
- Windows 桌面壳使用 Web UI 自定义窗口 chrome，不挂载 Tauri 原生 menu，也不显示系统标题栏，避免 `Bifrost / File / Edit / View / Window` 菜单栏挤占 WebView 高度。

### 必须不破坏

- 普通 Web UI 和 CLI 继续走 `channel=cli`，仍只更新 CLI 服务端。
- 浏览器打开的 CLI Web UI 不展示桌面安装 CLI 按钮，不走 `channel=desktop`，不执行桌面 app 重启。
- 只有 Tauri desktop mode (`isDesktopShell() === true`) 展示桌面专属能力：`Install CLI & Skills`、6 小时桌面检查、右下角桌面通知、desktop channel、Tauri 重启。
- 桌面端更新进度写入同一个 `upgrade-progress.json`，但 `source=desktop`，避免与 `source=admin/tray/cli` 混淆。
- release workflow 既有桌面资产命名不变：macOS 为 `.dmg`，Windows 为 `.msi`。
- 桌面端不是运行环境时，Web UI 仍沿用 1 小时 version cache，不弹桌面通知。

### 必须真实验证

- `bifrost app install --dry-run --version <v>` 输出目标版本、桌面包 URL 和安装路径，不修改系统。
- `bifrost app upgrade --dry-run --source desktop --version <v>` 输出桌面更新计划，并显示会更新独立 CLI。
- `bifrost app uninstall --dry-run` 只展示桌面端卸载目标，不影响 CLI。
- Admin channel 解析覆盖 `channel=desktop`、`target=desktop`、`source=desktop`。
- Web unit 测试覆盖 desktop/cli channel 参数，避免桌面更新误调用 CLI 更新。
- `POST /api/system/cli-install` 覆盖 App -> CLI 安装路径：临时目录安装、PATH 状态提示、AI skills 安装可跳过/可执行。

## 产品语义

### CLI 更新和桌面更新是两个 channel

`channel=cli` 表示更新提供 Admin 服务的 CLI 二进制。`channel=desktop` 表示更新桌面安装包；桌面包中包含新的 sidecar，因此不对当前 app 内置 sidecar 单独执行 self-update。

当桌面端触发 `bifrost app upgrade --source desktop` 时：

1. 搜索 PATH、`BIFROST_INSTALL_DIR` 和默认安装目录里的独立 `bifrost`。
2. 排除当前 `current_exe()`，避免把 app sidecar 当作独立 CLI。
3. 找到独立 CLI 时执行 `bifrost upgrade -y`。
4. 下载并安装桌面 `.dmg` / `.msi`。
5. 写入 completed 进度，由当前 Tauri WebView 调用 `restart_desktop_after_update` 退出并重新启动桌面 app，避免安装子进程和 WebView 同时拉起两个桌面实例。

普通终端执行 `bifrost app upgrade` 时，当前进程就是 CLI 安装本身，因此先复用现有 `bifrost upgrade` 更新当前 CLI，再安装桌面端，并主动打开新的桌面 app。

普通终端或普通浏览器触发的 caller-managed 桌面更新，在覆盖安装包前必须先处理正在
运行的旧桌面壳：

1. 按精确安装路径发现当前运行的 Desktop 进程。
2. 优先调用桌面壳内部 `--bifrost-upgrade-shutdown` 请求，让 App 按 ownership 规则退出。
3. 内部请求失败时使用平台退出机制，并等待进程真正释放安装目录。
4. 旧 App 未在超时内退出则拒绝覆盖，不能安装后仅用 `open` 激活仍在运行的旧版本。
5. 安装失败时重新打开已退出的旧 App，避免 caller-managed 更新把用户留在无桌面壳状态。

`source=desktop` 且带内部 handoff 标记的 WebView 更新不走上述提前退出路径；它仍由当前
Tauri shell 在安装完成后写 marker、退出并让 relaunch helper 接管。Windows deferred
installer 也继续属于该 handoff，而不是由安装子进程提前终止宿主。

### 桌面 WebUI 和浏览器 WebUI 必须区分

两者共享同一套 React Web UI 和 Admin API，但运行时语义不同：

- `isDesktopShell() === true`：Tauri 桌面壳。可展示桌面专属卡片、安装 CLI、检查桌面包更新、调用 `restart_desktop_after_update`。
- `isDesktopShell() === false`：浏览器打开的 CLI Web UI。只能做 CLI 更新和普通代理设置，不展示 App -> CLI 按钮，不调用 desktop channel。

所有桌面专属入口都必须以 `isDesktopShell()` 或上层传入的 `desktopMode` 为唯一门禁，不能只凭访问地址或后端能力判断。

### App -> CLI 一键安装

`/api/system/cli-install` 提供桌面端安装 CLI 的本机能力：

- `GET /api/system/cli-install`：返回当前推荐安装路径、是否已安装、安装目录是否在 PATH 中，以及 PATH 提示。
- `POST /api/system/cli-install`：把当前运行的桌面 sidecar (`current_exe`) 原子复制到 CLI 安装路径。
- 默认安装路径：macOS/Linux `~/.local/bin/bifrost`，Windows `%LOCALAPPDATA%\bifrost\bin\bifrost.exe`。
- 请求体可传 `install_dir` 供 E2E 或高级用户覆盖；可传 `install_skills=false` 跳过 AI skill 安装。
- 默认安装成功后执行 `bifrost install-skill --tool all -y`，让 AI coding tools 能轻松发现 Bifrost 能力。
- 该按钮不启动代理、不改系统代理、不安装 CA；桌面端本身已经有内置后端服务。

### 桌面端检查频率

桌面 shell 中：

- `useVersionStore` 对桌面端使用 6 小时缓存窗口。
- `StatusBar` 启动后立即 `forceRefresh` 检查一次，并每 6 小时重复。
- 如果 `shouldShowAutoModal()` 为真，显示右下角通知并打开 `VersionModal`。

非桌面 Web UI 保持原 1 小时缓存窗口和原有弹窗行为。

## 技术细节

### CLI 命令

`crates/bifrost-cli/src/cli.rs` 新增：

```text
bifrost app install [--package <path>] [--app-dir <dir>] [--version <v>] [--dry-run] [-y]
bifrost app uninstall [--app-dir <dir>] [--dry-run] [-y]
bifrost app upgrade [--package <path>] [--app-dir <dir>] [--version <v>] [--no-cli] [--source <label>] [--dry-run] [-y]
```

实现位于 `crates/bifrost-cli/src/commands/app.rs`：

- macOS 默认安装目录 `/Applications`，目标 `/Applications/Bifrost.app`。
- Windows MSI 默认安装目录 `%LOCALAPPDATA%\Bifrost`，目标 `bifrost-desktop.exe`；CLI 调用 MSI 时强制 `ALLUSERS=2 MSIINSTALLPERUSER=1`，避免普通用户静默安装被当作全用户安装并以 1603 / Error 1925 失败。
- release 下载 URL 使用 `https://github.com/bifrost-proxy/bifrost/releases/download/v<version>/bifrost-desktop-v<version>-<target>.<ext>`。
- release 资产后缀：macOS `.dmg`，Windows `.msi`。
- `BIFROST_APP_UPGRADE_TEST_PACKAGE` 可让 E2E/测试注入本地包，避免联网。
- `BIFROST_APP_SKIP_RESTART=1` 仅用于 E2E/自动化临时安装验证，跳过主动打开桌面 app；真实桌面端更新仍由 Tauri WebView 执行 `restart_desktop_after_update`。
- `BIFROST_DESKTOP_BIN` 仅用于 debug/VM 验证时覆盖 sidecar 路径，避免本地默认 `target/debug/bifrost.exe` 被旧进程锁住；发布包仍使用内置 `resources/bin/bifrost.exe`。
- Windows 桌面壳内部启动 `resources/bin/bifrost.exe start/stop` 时设置 `CREATE_NO_WINDOW`，避免从桌面快捷方式启动后额外弹出 terminal 窗口；命令行用户直接运行 `bifrost.exe` 不受影响。
- Tauri 菜单只在 macOS 注册；Windows host window 创建时关闭 decorations，由 Web UI 提供右上角最小化、最大化、关闭按钮和自定义拖拽区域。

### Admin API

`POST /api/system/upgrade` 支持 query channel：

- 默认或 `channel=cli`：spawn `bifrost self-update --target <v> --source admin`。
- `channel=desktop`：spawn `bifrost app upgrade --version <v> --source desktop -y`。

进度初始记录的 source 分别为 `admin` 和 `desktop`。

### Web UI

- `web/src/api/version.ts` 为 `checkVersion` / `startUpgrade` 增加 `UpgradeChannel`。
- `web/src/stores/useVersionStore.ts` 通过 `isDesktopShell()` 选择 `desktop` 或 `cli` channel。
- `VersionModal` 在桌面端显示 `bifrost app upgrade`，普通 Web UI 显示 `bifrost upgrade`。
- `ProxyTab` 只在 `desktopMode` 为真时显示 `Install CLI & Skills`。
- 桌面端升级完成后调用 Tauri command `restart_desktop_after_update`，失败时 fallback 到页面 reload。
- `StatusBar` 在桌面端负责 6 小时检查、右下角通知和自动打开更新窗口。

### Tauri 边界

Tauri 官方 updater plugin 支持静态 JSON endpoint、签名和 install mode。当前实现没有直接启用插件，而是沿用 Bifrost 现有 Admin/CLI 更新进度通道，原因是桌面端 UI 复用 CLI Admin 服务，用户要求的下载/安装进度也已经由 `upgrade-progress.json` 提供。后续若启用 Tauri plugin，需要继续保持 `channel=desktop` 与 CLI channel 分离。

参考：

- https://v2.tauri.app/plugin/updater/
- https://v2.tauri.app/develop/configuration-files/

## 2026-07-06 更新循环修复

### 问题

用户现场表现为：桌面端收到 `v0.0.141` 更新推送，点击更新后 UI 进度先停在下载 0%，最后直接跳到 100%；退出并重新打开后仍显示 `v0.0.140`，继续弹出同一个更新提示；独立 CLI 也没有更新成功。

### 根因判断

- 桌面包下载使用一次性 `response.bytes()` 读取完整 body，只在下载前写 0%、下载后写 100%，中间没有持续更新 `upgrade-progress.json`。
- `bifrost app upgrade --source desktop` 默认安装目录固定回落到 `/Applications`，无法覆盖“用户实际从 `~/Applications`、下载目录或其他自定义位置启动 Bifrost.app”的场景。安装写到另一个 bundle 后，Tauri 再按当前旧 executable 路径重启，就会继续打开旧版本。
- Finder 启动的桌面 app 不继承交互 shell PATH。独立 CLI 发现逻辑虽然检查了 PATH、`~/.local/bin`、`~/.bifrost/bin`、Homebrew 路径，但漏掉常见的 `~/.cargo/bin/bifrost`，可能导致终端里实际使用的 CLI 没被联动更新。
- 安装完成后没有重新读取目标 app 版本做门禁；即使复制了错误版本、错误路径或 release 包内容仍旧，也可能写入 `completed`，让 UI 误判更新成功。

### 修复方案

- desktop source 且未显式传 `--app-dir` 时，从当前 sidecar executable 路径向上查找 `Bifrost.app`，并把该 bundle 的父目录作为安装目录；找不到时才回退默认安装目录。
- macOS `restart_desktop_after_update` 从当前 executable 反推 `Bifrost.app` bundle，并用 `open -n <bundle>` 通过 LaunchServices 重启；找不到 bundle 时保留原可执行文件启动 fallback。
- 桌面包下载改为流式读取 response，每 250ms 写入 `phase=downloading`、下载字节和百分比，避免 UI 0% 假死。
- 独立 CLI 搜索增加 `~/.cargo/bin/bifrost`，覆盖 Finder 启动时缺少用户 PATH 的常见安装方式。
- 独立 CLI 版本探测启动刚写入的可执行文件时，Unix `ETXTBSY` 仅做最多 8 次、总退避不超过 140ms 的有界重试；持续占用或其他启动错误继续按探测失败处理，避免并行检查把可用 CLI 瞬态误判为缺失。
- 桌面包安装后，如果能读取目标 app 版本，必须与目标版本一致；否则写 `phase=failed` 和可操作错误，不允许写 `completed`。

### 回归覆盖

- `cargo test -p bifrost-cli app::tests --lib` 覆盖当前 `.app` 目录反推、版本比较和 macOS stale bundle 失败门禁。
- `bash e2e-tests/tests/test_desktop_app_update_cli.sh` 覆盖临时 app 真实安装/更新/卸载、同版本跳过下载，以及“目标 0.0.141 但安装后仍报告 0.0.140”时非零退出并写 failed 进度。
- `human_tests/desktop-app-auto-update.md` 增加 TC-DAU-04D/04E/04F，覆盖旧版本假成功、非默认运行路径和 Finder 启动 CLI 路径发现。

## 2026-07-06 桌面 core 启动门禁

### 问题

桌面 app 与 CLI 共用同一个本机 core。用户启动桌面 app 时有两类状态需要明确区分：

- 没有任何 Bifrost core 在运行：桌面 app 应自动启动内置 sidecar，避免打开后空白或停留在离线状态。
- 桌面 app 启动时复用了外部 CLI core，但该 CLI 之后被用户停止：桌面 app 应感知到 core 不可用，显示全屏阻塞浮层，提示用户启动 Bifrost 服务；用户点击按钮后再由桌面 app 启动内置 sidecar，成功后关闭浮层并刷新页面状态。

同时，如果桌面 app 已经能连接 core，但用户命令路径中没有安装独立 CLI，app 启动后应提示安装 CLI。安装成功后提示完成，并提供文档入口。

### 修复方案

- Tauri 新增 `start_desktop_core` command，前端按钮可显式请求桌面壳启动内置 sidecar，并返回最新 `DesktopRuntimeInfo`。
- 启动阶段复用 `start_desktop_backend_now("startup")`，仍然自动拉起内置 core；如果发现同 data dir 的外部 CLI 已经健康运行，则复用该实例。
- Watchdog 对托管 sidecar 和外部 CLI 做区分：
  - 托管 sidecar 异常退出或连续健康探针失败时，仍由 watchdog 自动恢复。
  - 外部 CLI 健康探针连续失败时，不自动恢复，而是设置 `startup_ready=false` 和可展示错误，交给前端全屏浮层提示用户手动启动。
  - 启动/恢复进行中跳过 watchdog 处理，避免 sidecar 正在启动时被一次探针失败误判；健康探针需要连续失败才进入恢复或手动启动状态。
- React 新增 `DesktopStartupGate`：
  - 周期读取 `getDesktopRuntime()` 并同步 `setDesktopProxyPort()`，保证 app 状态和实际 core 端口刷新。
  - core 不可用时显示全屏 `Start Bifrost Service` 浮层，按钮调用 `startDesktopCore()`，成功后关闭浮层。
  - core 可用后检查 `GET /api/system/cli-install`；未安装 CLI 时显示 `Install Bifrost CLI` 浮层，按钮后台调用 `POST /api/system/cli-install`，成功后显示文档入口。

### 回归覆盖

- Rust 单测覆盖外部 core 掉线时 `startup_ready=false`、保留手动启动错误且不创建托管 child。
- macOS 真实链路验证：临时 `BIFROST_DATA_DIR` + 端口 `19900` 启动 CLI，启动当前源码构建的桌面 app 复用外部 CLI；停止 CLI 后，app 显示全屏 `Start Bifrost Service` 浮层；点击按钮后桌面 app 拉起内置 sidecar，页面恢复运行态并显示 `http://127.0.0.1:19900`。
- 真实链路中确认未安装 CLI 时进入 `Install Bifrost CLI` 浮层；不在人工验证中点击真实安装按钮，避免写入用户命令路径，安装机制由临时目录 API/E2E 覆盖。

## 2026-07-08 CLI upgrade 联动桌面 App 与版本检查修复

### 问题

用户现场状态为：

- `/Applications/Bifrost.app` 的 `CFBundleShortVersionString` 为 `0.0.144`。
- 正在运行的 core 是 `/Users/eden/.local/bin/bifrost 0.0.145`。
- `GET /_bifrost/api/system/version-check?refresh=true&channel=desktop` 返回 `current_version=0.0.145`、`latest_version=0.0.145`、`has_update=false`。

这说明旧桌面 App 复用了已更新的独立 CLI/core 后，desktop channel 的版本检查仍然按 core 版本比较，而不是按已安装 App bundle 版本比较，因此不会弹出新版桌面 App 更新提示。

同时，普通终端执行 `bifrost upgrade` 只更新 CLI、安装 skills 并重启 proxy；即使机器上已经安装了桌面 App，也不会顺带更新 App。

### 修复方案

- `GET /api/system/version-check?channel=desktop` 在能读取已安装桌面 App 版本时，使用 App 版本作为 `current_version` 与 latest release 比较；读取不到时才回退 CLI/core 版本。
- `POST /api/system/upgrade?channel=desktop` 复用同一 desktop version-check 结果，避免旧 App + 新 core 时被误判为 `No update available`。
- `bifrost upgrade` 在 CLI 已升级成功或 CLI 已经是最新版本时，如果检测到本机已安装桌面 App，则执行：

  ```bash
  bifrost app upgrade --no-cli --source cli-upgrade --version <latest> -y
  ```

- `--no-cli` 防止 `bifrost app upgrade` 反过来递归执行 CLI upgrade。
- 桌面 App 联动更新是 best-effort：失败、超时或无法启动子命令时只输出 warning 和原因，并提示用户手动执行 `bifrost app upgrade --no-cli -y`；不得让 App 更新失败导致 CLI upgrade、skills 安装、proxy restart 等主流程失败。

### 回归覆盖

- `cargo test -p bifrost-cli upgrade::tests --lib`
  - `bifrost upgrade` 后置 App 更新参数必须包含 `--no-cli`、`--source cli-upgrade` 和目标版本。
  - `BIFROST_APP_INSTALL_DIR` 可隔离检测已安装 App 路径。
  - App 更新失败原因优先使用 stderr/stdout 摘要。
- `cargo test -p bifrost-admin version_check::tests handlers::system::tests --lib`
  - desktop version-check 可使用已安装 App bundle 版本判断更新。
  - 旧 App 版本低于 latest 时 `has_update=true`。
- `bash e2e-tests/tests/test_upgrade_cli.sh`
  - macOS 临时 `Bifrost.app` 已是目标版本时，`bifrost upgrade` 仍会发现并处理已安装 App，且不触碰真实 `/Applications`。
  - macOS 临时 `Bifrost.app` 更新失败时，`bifrost upgrade` 退出码仍为 0，并输出失败原因。
- `human_tests/desktop-app-auto-update.md`
  - 增加 CLI upgrade 联动 App 更新、App 更新失败不阻断、旧 App + 新 CLI 仍提示 desktop update 的真实场景用例。

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli app::tests --lib`
  - release 资产名区分 macOS `.dmg` 与 Windows `.msi`。
  - macOS app path 为 `/Applications/Bifrost.app`。
- `cargo test -p bifrost-cli upgrade::tests --lib`
  - 普通 `bifrost upgrade` 后置桌面 App 更新使用 `app upgrade --no-cli --source cli-upgrade --version <latest> -y`。
  - 桌面 App 更新失败只输出 warning，不阻断主升级流程。
- `cargo test -p bifrost-admin handlers::system::tests --lib`
  - `channel=desktop` / `target=desktop` / `source=desktop` 解析为桌面 channel。
  - desktop channel 构造 `app upgrade --version <v> --source desktop -y`，CLI channel 构造 `self-update --target <v> --source admin`。
  - desktop version-check 在可读取已安装 App 版本时使用 App 版本，而不是 CLI/core 版本。
  - CLI install status 使用覆盖目录、返回 PATH hint。
  - CLI install 可把当前 executable 复制到临时目录，且可跳过 AI skill 安装。
  - stale progress 归一化保持原有行为。
- `pnpm --dir web run test:unit -- src/stores/useVersionStore.test.ts`
  - CLI mode 调 `checkVersion(true, "cli")`。

### E2E 测试

- `bash e2e-tests/tests/test_desktop_app_update_cli.sh`
  - `bifrost app install --dry-run` 不改系统。
  - `bifrost app upgrade --dry-run --source desktop` 展示桌面更新计划和 CLI 联动提示。
  - `bifrost app upgrade --dry-run --no-cli` 不展示 CLI 联动提示。
  - `bifrost app uninstall --dry-run` 只规划桌面卸载。
  - macOS 构造临时 `Bifrost.app`，真实安装到临时目录、真实 desktop-source upgrade、检查 `upgrade-progress.json` 为 `completed/source=desktop`，再真实卸载。
  - Windows 构造临时 zip 内的 `bifrost-desktop.exe`，真实安装到临时目录、真实 desktop-source upgrade、检查进度文件，再真实卸载；设置 `BIFROST_DESKTOP_REAL_MSI` 时额外执行真实 MSI 普通用户安装/卸载回归。
  - Windows VM 中启动桌面包并截图确认没有原生标题栏/菜单栏，底部状态栏仍完整可见。
  - 启动临时 Bifrost 服务，调用 `POST /api/system/cli-install` 把 CLI 安装到临时目录，断言二进制存在，并通过 `GET /api/system/cli-install` 获取状态。

### human_tests

- `human_tests/desktop-app-auto-update.md`
  - 覆盖 CLI dry-run、Admin channel、Web channel、桌面通知与重启边界。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：6 小时检查、桌面通知、自动弹窗、桌面/CLI channel 分离、进度复用、安装后重启、CLI 联动更新。
- Review 文件：`commands/app.rs`、`system.rs`、`useVersionStore.ts`、`StatusBar`、`VersionModal`、Tauri command。
- 测试：CLI/Admin Rust 单测、Web unit、E2E dry-run、human_tests。

第 2 轮：

- 复核 release 资产名与 `.dmg/.msi` 对齐，检查 Windows/macOS 条件编译。
- 复跑受影响测试和 workspace 关键校验。
- 若发现 channel 串台、进度 source 错误或 desktop shell 判断缺口，继续追加 Review/Fix/Test 轮次。
