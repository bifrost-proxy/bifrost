# 桌面端自动更新真实场景测试

## 功能模块说明

验证 Bifrost 桌面端自动更新在 macOS 和 Windows 的产品语义：桌面端与 CLI 更新 channel 分离，桌面端最多 6 小时检查一次新版本，发现新版本后显示右下角通知并自动打开更新窗口，用户点击更新后复用 Web UI 的下载/安装/重启进度，安装完成后重启桌面端；如果独立 CLI 已安装，桌面端更新时同时更新 CLI。

同时验证两种安装路径：

- 先安装 CLI：用户可通过 `bifrost app install / upgrade / uninstall` 管理桌面 App。
- 先安装 App：桌面 Settings 提供 `Install CLI & Skills` 按钮，把内置 CLI 安装到用户命令行路径，并安装 Bifrost AI skills。

本用例中的 CLI dry-run 不修改系统 app、不下载 release、不启动系统代理。临时真实安装用例只写入 `mktemp` 目录，并用 `BIFROST_APP_SKIP_RESTART=1` 避免打开假 app；发布包级验证仍需要 macOS/Windows 桌面环境和真实 `.dmg/.msi`。Windows MSI 默认安装路径应与 Tauri 产物一致：`%LOCALAPPDATA%\Bifrost\bifrost-desktop.exe`。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 执行命令前使用当前构建或源码构建的 `bifrost`：
  ```bash
  cargo build -p bifrost-cli
  BIFROST_BIN="$PWD/target/debug/bifrost"
  ```
- 所有服务启动测试必须设置临时 `BIFROST_DATA_DIR`，并设置：
  ```bash
  export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  export BIFROST_DISABLE_TRAY=1
  ```
- 除明确验证系统代理的用例外，启动 Bifrost 必须加 `--no-system-proxy`。
- 真实桌面安装验证需要在 macOS 或 Windows 桌面会话中执行；无法访问桌面 GUI 时记录为环境阻塞。
- 浏览器打开的 CLI Web UI 必须按 `isDesktopShell() === false` 处理，不展示 `Install CLI & Skills`，不使用 desktop channel。

## 测试用例列表

### TC-DAU-01 app install dry-run 不修改系统

操作步骤：

1. 执行：
   ```bash
   "$BIFROST_BIN" app install --dry-run --version 0.0.139 --app-dir "$PWD/.tmp-desktop-app"
   ```
2. 检查输出包含 `Desktop app install target:`、`Target version: v0.0.139` 或 `Target version: 0.0.139`、`Dry run: no files will be changed.`。
3. 检查 `.tmp-desktop-app` 不存在或没有新增 `Bifrost.app` / `bifrost-desktop.exe`。

预期结果：

- 命令仅展示桌面端安装计划，不写系统 Applications / Program Files，不修改 CLI。

### TC-DAU-02 app upgrade desktop channel 会联动独立 CLI

操作步骤：

1. 执行：
   ```bash
   "$BIFROST_BIN" app upgrade --dry-run --source desktop --version 0.0.139 --app-dir "$PWD/.tmp-desktop-app"
   ```
2. 检查输出包含 `Desktop app upgrade target:`、`Would upgrade CLI with`、`Would install desktop package from:`、`Would let the current desktop shell restart`。

预期结果：

- 桌面端更新计划明确包含 CLI 联动更新。
- 该 dry-run 不执行真实 CLI self-update，也不安装桌面包。

### TC-DAU-03 app upgrade --no-cli 只更新桌面端

操作步骤：

1. 执行：
   ```bash
   "$BIFROST_BIN" app upgrade --dry-run --source desktop --no-cli --version 0.0.139 --app-dir "$PWD/.tmp-desktop-app"
   ```
2. 检查输出包含 `Would install desktop package from:`。
3. 检查输出不包含 `Would upgrade CLI with`。

预期结果：

- `--no-cli` 可显式跳过 CLI 联动更新。

### TC-DAU-04 app uninstall dry-run 只规划桌面端卸载

操作步骤：

1. 执行：
   ```bash
   "$BIFROST_BIN" app uninstall --dry-run --app-dir "$PWD/.tmp-desktop-app"
   ```
2. 检查输出包含 `Desktop app path:` 和 `Dry run: would remove the desktop app only.`。

预期结果：

- 卸载命令只作用于桌面端路径，不卸载 CLI。

### TC-DAU-04B 临时目录真实安装、升级和卸载

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
2. 在 macOS 上，脚本会创建临时 `Bifrost.app`，执行真实 `app install --package <fake.app> --app-dir <tmp> -y`，断言 bundle 被复制到临时目录。
3. 在 Windows 上，脚本会创建临时 zip 内的 `bifrost-desktop.exe`，执行真实 `app install --package <fake.zip> --app-dir <tmp> -y`，断言 exe 被复制到临时目录。
4. 脚本继续执行 `app upgrade --source desktop --no-cli --package <fixture> --app-dir <tmp> -y`，读取临时 `BIFROST_DATA_DIR/upgrade-progress.json`。
5. 脚本最后执行 `app uninstall --app-dir <tmp> -y`，断言临时 app 被删除。

预期结果：

- 安装机制真实复制桌面包内容。
- 更新机制真实覆盖临时安装目标，并写入 `phase=completed`、`source=desktop`。
- 卸载机制真实移除临时桌面端目标。
- 测试不触碰 `/Applications`、Windows 开始菜单或真实系统安装目录。

### TC-DAU-04C 已安装桌面端等于目标版本时跳过下载和重装

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
2. 在 macOS 上，脚本创建带 `CFBundleShortVersionString=0.0.139` 的临时 `Bifrost.app` 并安装到临时目录。
3. 脚本继续执行不带 `--package` 的：
   ```bash
   "$BIFROST_BIN" app upgrade --app-dir "<tmp>" --source desktop --no-cli --version 0.0.139 -y
   ```
4. 检查输出包含 `Desktop app is already on target version`，且不包含 `Downloading desktop app:`。

预期结果：

- 当已安装桌面端版本已经等于目标版本时，`bifrost app upgrade` 不下载 release，不覆盖安装，不重启桌面端。
- 该判断只在能明确读到已安装桌面端版本时生效；版本读不到时仍继续原升级流程，避免漏装。

### TC-DAU-04D Windows 普通用户静默 MSI 安装回归

操作步骤：

1. 在 Windows 11 普通用户会话中准备真实桌面 MSI，例如：
   ```powershell
   $msi="$env:TEMP\bifrost-desktop-v0.0.139-aarch64-pc-windows-msvc.msi"
   curl.exe -L -o $msi https://github.com/bifrost-proxy/bifrost/releases/download/v0.0.139/bifrost-desktop-v0.0.139-aarch64-pc-windows-msvc.msi
   ```
2. 用当前源码构建的 CLI 执行：
   ```bash
   BIFROST_APP_SKIP_RESTART=1 "$BIFROST_BIN" app install --package "$msi" -y
   ```
3. 检查输出包含 `Desktop app install target:`，且目标为 `%LOCALAPPDATA%\Bifrost\bifrost-desktop.exe`。
4. 检查 `%LOCALAPPDATA%\Bifrost\bifrost-desktop.exe` 存在。
5. 执行：
   ```bash
   "$BIFROST_BIN" app uninstall -y
   ```
6. 检查 `%LOCALAPPDATA%\Bifrost\bifrost-desktop.exe` 已删除，命令退出码为 0。

预期结果：

- 普通用户静默安装不会再因为 MSI `ALLUSERS=1` 报 1603 / Error 1925。
- CLI 调用 MSI 时使用 per-user 安装属性，并在失败时输出 MSI 日志路径。
- CLI 显示、重启和卸载使用真实 Tauri MSI 目标路径，不再指向 `%LOCALAPPDATA%\Programs\Bifrost\Bifrost.exe`。

### TC-DAU-05 Admin API 桌面 channel 与 CLI channel 分离

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-admin handlers::system::tests::parse_upgrade_channel_defaults_to_cli_and_accepts_desktop_aliases --lib
   ```
2. 检查测试通过。

预期结果：

- 默认 query 使用 CLI channel。
- `channel=desktop`、`target=desktop`、`source=desktop` 均解析为桌面 channel。
- desktop channel 派发 `app upgrade --version <v> --source desktop -y`，CLI channel 派发 `self-update --target <v> --source admin`。

### TC-DAU-06 Web UI 桌面 channel 参数不回退到 CLI，CLI Web UI 不展示桌面按钮

操作步骤：

1. 执行：
   ```bash
   pnpm --dir web run test:unit -- src/stores/useVersionStore.test.ts
   ```
2. 检查测试通过，并确认 CLI mode 仍调用 `checkVersion(true, "cli")`。
3. 代码 review `web/src/stores/useVersionStore.ts`，确认 `isDesktopShell()` 为真时 `checkVersion` 与 `startUpgrade` 使用 `desktop` channel，且桌面缓存窗口为 `6 * 60 * 60 * 1000`。
4. 代码 review `web/src/pages/Settings/tabs/ProxyTab.tsx`，确认 `Install CLI & Skills` 位于 `desktopMode ? (...) : null` 分支内。

预期结果：

- 普通 Web UI 不误触发桌面更新。
- 桌面 shell 会把 version-check/start-upgrade 请求标记为 desktop channel。
- 浏览器打开的 CLI Web UI 不展示 App -> CLI 按钮。

### TC-DAU-06B App 一键安装 CLI 与 AI skills

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
2. 脚本启动临时 Bifrost 服务，调用：
   ```bash
   POST /_bifrost/api/system/cli-install
   {"install_dir":"<mktemp>/api-cli-bin","install_skills":false}
   ```
3. 检查响应 `installed=true`，临时安装目录下存在 `bifrost` 或 `bifrost.exe`。
4. 检查 `GET /_bifrost/api/system/cli-install` 返回 `install_path` 等安装状态。

预期结果：

- App -> CLI 安装机制能把当前 sidecar 原子复制到用户指定 CLI 目录。
- 测试可跳过 AI skills 以避免污染真实工具目录；真实 UI 默认 `install_skills=true`。
- 返回中包含 PATH 提示，用户知道是否需要重启 shell 或手工加入 PATH。

### TC-DAU-07 桌面通知和自动弹窗

操作步骤：

1. 在桌面 shell 环境启动 Bifrost Desktop。
2. 准备 `version_cache.json` 或真实新 release，使 `has_update=true`。
3. 等待启动后的强制检查完成。
4. 观察右下角通知和版本更新窗口。

预期结果：

- 桌面端右下角出现更新通知。
- 版本更新窗口自动打开。
- 弹窗文案显示桌面端更新，并展示 `bifrost app upgrade` 作为手动命令。

### TC-DAU-08 桌面更新完成后重启桌面 app

操作步骤：

1. 在 macOS 或 Windows 桌面环境准备本地桌面安装包。
2. 执行：
   ```bash
   "$BIFROST_BIN" app upgrade --package <本地安装包> --source desktop --version <版本号> -y
   ```
3. 在更新窗口中观察下载/安装/重启阶段。
4. 安装完成后确认桌面 app 重新启动。

预期结果：

- 进度依次进入 installing / restarting / completed。
- 桌面 app 安装完成后自动重新打开。
- 若存在独立 CLI 安装，CLI 版本也更新到最新；若不存在，日志明确跳过独立 CLI。

### TC-DAU-09 Windows 桌面快捷方式不弹出 shell 窗口

操作步骤：

1. 在 Windows 11 桌面环境中通过桌面快捷方式 `Bifrost.lnk` 启动应用。
2. 观察启动期间的窗口列表和任务栏。
3. 使用 PowerShell 检查进程：
   ```powershell
   Get-CimInstance Win32_Process |
     Where-Object { $_.Name -match 'bifrost|WindowsTerminal|cmd.exe|powershell.exe' -or $_.CommandLine -match 'Bifrost|bifrost' } |
     Select-Object ProcessId,ParentProcessId,Name,CommandLine
   ```
4. 关闭 Bifrost 桌面窗口。

预期结果：

- 桌面快捷方式只显示 Bifrost 桌面 UI，不出现 Windows Terminal、cmd 或 PowerShell shell 窗口。
- 内置 `bifrost.exe` sidecar 作为后台子进程运行，stdout/stderr 写入桌面日志文件。
- 关闭桌面 UI 后，sidecar 按桌面壳生命周期正常停止；不存在“关闭 shell 窗口导致 app 一起退出”的用户路径。

### TC-DAU-10 Windows 桌面壳不显示原生标题栏和菜单栏

操作步骤：

1. 在 Windows 11 桌面环境中启动当前构建的 Bifrost Desktop。
2. 打开 Settings -> Proxy，观察窗口顶部和底部状态栏。
3. 截图保存启动后的完整桌面窗口。
4. 检查进程只存在 Bifrost 桌面窗口，不依赖额外原生 menu bar 提供关闭入口。

预期结果：

- 窗口顶部不显示 Windows 原生标题栏，也不显示 `Bifrost / File / Edit / View / Window` 菜单栏。
- Web UI 从窗口顶部开始使用自定义桌面 chrome，不被系统标题栏向下挤压。
- 底部状态栏完整可见，Settings 页面内容不会因为顶部系统栏占位而挤出窗口。
- 右上角自定义最小化、最大化和关闭按钮可见并可点击。

## 清理步骤

```bash
rm -rf .tmp-desktop-app
```

真实桌面安装测试后，如需回滚，请使用系统应用卸载方式或执行：

```bash
bifrost app uninstall
```

## 执行记录

| 日期 | 用例 | 执行命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-05 | TC-DAU-01 / 02 / 03 / 04 / 04B / 04C / 06B | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh` | 待复测：本轮新增 04C，验证已安装桌面端等于目标版本时跳过下载和重装 |
| 2026-07-05 | TC-DAU-05 | `cargo test -p bifrost-admin handlers::system::tests --lib` | PASS：7/7 通过，覆盖 desktop alias、spawn args、CLI install 临时目录与 skip skills |
| 2026-07-05 | TC-DAU-06 | `pnpm --dir web run test:unit -- src/stores/useVersionStore.test.ts` + 代码 review | PASS：Vitest 22 files / 93 tests 通过；新增 desktop shell 单测确认 `checkVersion/startUpgrade` 使用 `desktop` channel，代码确认桌面缓存窗口为 6 小时，非桌面仍使用 `cli` channel；`Install CLI & Skills` 位于 `desktopMode` 分支 |
| 2026-07-05 | TC-DAU-07 / 08 | 需要真实 macOS/Windows 桌面安装包与 GUI 会话 | 未执行：当前本机未进行真实桌面包安装/GUI 通知验证；需在发布包或本地 `.dmg/.msi` 准备后补跑 |
| 2026-07-06 | TC-DAU-04C | Parallels Windows 11 ARM64 VM：先用 v0.0.139 MSI 复现 `msiexec /i <msi> /qn /norestart`，再用 `ALLUSERS=2 MSIINSTALLPERUSER=1` 验证普通用户安装 | PASS：原命令稳定复现 1603，日志显示 `Error 1925`；加 per-user MSI 属性后同一 MSI 安装成功并写入 `%LOCALAPPDATA%\Bifrost` |
| 2026-07-06 | TC-DAU-09 | Parallels Windows 11 ARM64 VM：桌面快捷方式指向 `%LOCALAPPDATA%\Bifrost\bifrost-desktop.exe`，进程树显示 `bifrost-desktop.exe` 启动内置 `bifrost.exe` 后出现 `WindowsTerminal.exe -Embedding` | FAIL 复现：桌面 UI 之外会出现 Windows Terminal 窗口；关闭 shell 窗口会影响桌面 app，需要桌面壳对子进程设置 `CREATE_NO_WINDOW` 后复测 |
