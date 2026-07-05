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
- Windows 默认安装目录 `%LOCALAPPDATA%\Programs\Bifrost`，目标 `Bifrost.exe`。
- release 下载 URL 使用 `https://github.com/bifrost-proxy/bifrost/releases/download/v<version>/bifrost-desktop-v<version>-<target>.<ext>`。
- release 资产后缀：macOS `.dmg`，Windows `.msi`。
- `BIFROST_APP_UPGRADE_TEST_PACKAGE` 可让 E2E/测试注入本地包，避免联网。
- `BIFROST_APP_SKIP_RESTART=1` 仅用于 E2E/自动化临时安装验证，跳过主动打开桌面 app；真实桌面端更新仍由 Tauri WebView 执行 `restart_desktop_after_update`。

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

## 测试方案

### 单元测试

- `cargo test -p bifrost-cli app::tests --lib`
  - release 资产名区分 macOS `.dmg` 与 Windows `.msi`。
  - macOS app path 为 `/Applications/Bifrost.app`。
- `cargo test -p bifrost-admin handlers::system::tests --lib`
  - `channel=desktop` / `target=desktop` / `source=desktop` 解析为桌面 channel。
  - desktop channel 构造 `app upgrade --version <v> --source desktop -y`，CLI channel 构造 `self-update --target <v> --source admin`。
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
  - Windows 构造临时 zip 内的 `Bifrost.exe`，真实安装到临时目录、真实 desktop-source upgrade、检查进度文件，再真实卸载。
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
