# 桌面端自动更新真实场景测试

## 功能模块说明

验证 Bifrost 桌面端自动更新在 macOS 和 Windows 的产品语义：桌面端与 CLI 更新 channel 分离，桌面端最多 6 小时检查一次新版本，发现新版本后显示右下角通知并自动打开更新窗口，用户点击更新后复用 Web UI 的下载/安装/重启进度，安装完成后重启桌面端；如果独立 CLI 已安装，桌面端更新时同时更新 CLI。

同时验证两种安装路径：

- 先安装 CLI：用户可通过 `bifrost app install / upgrade / uninstall` 管理桌面 App。
- 先安装 App：桌面 Settings 在 CLI 缺失时提供 `Install CLI` 按钮；CLI 已存在后才展示独立的 `Install AI Skills` / `Reinstall AI Skills` 按钮。

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
- 浏览器打开的 CLI Web UI 必须按 `isDesktopShell() === false` 处理，不展示桌面 CLI / AI Skills 安装按钮，不使用 desktop channel。

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

### TC-DAU-04D 桌面更新后仍是旧版本时必须失败

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
2. 在 macOS 上，脚本创建一个 `CFBundleShortVersionString=0.0.140` 的临时 `Bifrost.app`，但以目标版本 `0.0.141` 执行：
   ```bash
   "$BIFROST_BIN" app upgrade --package "<stale.app>" --app-dir "<tmp>" --source desktop --no-cli --version 0.0.141 -y
   ```
3. 检查命令返回非 0，输出包含 `reports version v0.0.140 instead of target v0.0.141`。
4. 检查临时 `BIFROST_DATA_DIR/upgrade-progress.json` 中 `phase=failed`。

预期结果：

- 如果安装动作结束后目标 app 仍报告旧版本，更新流程必须失败，不允许写入 `completed`。
- UI 应展示明确失败，用户重启后不应被误导为“已完成更新”。

### TC-DAU-04E desktop source 从当前运行的 .app 位置安装并重启

操作步骤：

1. 从非默认目录启动 Bifrost Desktop，例如 `~/Applications/Bifrost.app`。
2. 触发桌面端更新，或在同等环境下执行：
   ```bash
   "$BIFROST_BIN" app upgrade --source desktop --version <目标版本> -y
   ```
3. 检查更新目标路径为当前运行的 `Bifrost.app` 所在目录，而不是无条件使用 `/Applications/Bifrost.app`。
4. 更新完成后，桌面壳通过 LaunchServices 重新打开当前 `Bifrost.app` bundle。

预期结果：

- 从 `~/Applications`、下载目录或自定义目录运行的桌面 app，会更新当前实际运行的 bundle。
- 重启后版本显示为目标版本，不再继续弹出同一个更新提示。

### TC-DAU-04F Finder 启动时也能发现常见独立 CLI 安装路径

操作步骤：

1. 在 macOS 上从 Finder 启动 Bifrost Desktop，确保进程 PATH 不依赖交互 shell。
2. 准备独立 CLI 位于 `~/.local/bin/bifrost`、`~/.bifrost/bin/bifrost` 或 `~/.cargo/bin/bifrost` 之一。
3. 触发 desktop channel 更新。
4. 更新结束后执行：
   ```bash
   which -a bifrost
   bifrost --version
   ```

预期结果：

- 桌面更新会检查常见 CLI 安装位置，即使 Finder 启动时 PATH 缺少用户 shell 路径。
- 终端中的独立 CLI 更新到目标版本，或日志明确说明未发现独立 CLI。

### TC-DAU-04G Windows 普通用户静默 MSI 安装回归

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

### TC-DAU-04G2 Windows 干净安装后由上一版本真实更新

操作步骤：

1. 每一轮都从干净系统状态开始。先以管理员/SYSTEM 上下文终止所有 Bifrost 相关进程，枚举并静默卸载 HKLM 中所有 `Bifrost` MSI 产品；再以桌面登录用户上下文卸载 HKCU legacy NSIS 项，清理 `%LOCALAPPDATA%\Bifrost`、`%USERPROFILE%\.bifrost`、CLI 安装目录，以及 `.bifrost-upgrade-*`、`.bifrost.exe.pending.*`、`*.upgrade-backup`、deferred status/ready/args/log 等升级残留。
2. 断言清理完成：Bifrost 进程数为 0，HKLM/HKCU 卸载注册数为 0，安装目录和升级残留数为 0。任一项非 0，本轮作废，不得继续计为通过。
3. 安装“上一版本”的真实 ARM64 Windows MSI，并在当前桌面用户下安装同版本 CLI。断言 MSI 注册只有一个，CLI 与 Desktop 均报告上一版本。
4. 由上一版本 CLI 自身执行默认 `bifrost upgrade -y`，不得追加隐藏 channel 参数，不得用新版本二进制直接覆盖，也不得预先安装目标版本。稳定版只允许发现稳定版；`alpha.N` 只允许发现同名 `alpha` channel 的更新，不能复用稳定版或 beta/rc 的缓存结果。
5. 等待更新完成并检查：CLI、Desktop 与运行中 core 都为目标版本；MSI 注册仍只有一个；无 `.pending`、`.upgrade-backup`、helper `.ps1/.args/.ok/.status/.log` 临时文件；无旧版进程或终端闪窗。
6. 旧 alpha 本身若没有 prerelease discovery 能力，只允许把首个包含通道修复与 staged-target handoff 的 alpha 作为手工安装基线；随后必须再发布相邻的下一个 alpha，完整重复步骤 1–5，从首个修复 alpha 通过默认命令更新到下一个 alpha，证明 alpha discovery 与 staged 新二进制 helper 同时生效。

预期结果：

- “上一版本 → 下一版本”在完全清理、重新安装后的真实用户路径中成功，不依赖上一轮残留状态。
- 首个旧版本兼容升级与“修复版 → 下一版”两段都通过；后者必须在 helper log 中显示等待的 PID 是原 updater PID。
- `alpha.8` 能发现 `alpha.9`，且 `alpha.10` 的排序高于 `alpha.9`；alpha 不会被稳定版 `/releases/latest` 或其它 prerelease channel 误判为“已是最新”。正式版仍只跟随正式版。
- 更新后只保留一个有效 MSI 注册和一套当前版本文件，升级临时资产为 0。

### TC-DAU-04G3 Windows 锁文件失败回滚也必须零残留

操作步骤：

1. 完整执行 TC-DAU-04G2 的步骤 1–3，从干净系统重新安装上一版本，不复用成功升级后的系统状态。
2. 在上一版本触发升级前，使用独立进程以拒绝共享的方式锁住 deferred status 或目标替换路径，锁定时间超过旧 helper 的短清理窗口。
3. 由上一版本 CLI 自身执行 `bifrost upgrade -y`，记录退出码、耗时、helper log、最终 CLI/Desktop/core 版本和升级残留清单。
4. 释放文件锁后再次扫描进程、MSI 注册、安装目录与升级临时资产。

预期结果：

- 升级明确失败且不会假报成功；可验证的旧版本恢复并仍可启动。
- helper 按锁定预算等待/重试，失败后恢复旧 `bifrost.exe`，不留下 pending、backup、PowerShell、args、ready 或临时 status 文件。
- MSI 注册仍唯一，没有同时注册上一版本与目标版本；无版本探测风暴和可见 PowerShell/Terminal 窗口。

### TC-DAU-04H 普通 bifrost upgrade 自动联动已安装桌面 App

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_upgrade_cli.sh
   ```
2. 在 macOS 上，脚本创建临时 `Bifrost.app`，并设置 `BIFROST_APP_INSTALL_DIR=<mktemp>/app-dir`。
3. 脚本设置 `BIFROST_UPGRADE_TEST_LATEST_VERSION` 为当前 CLI 版本，模拟 CLI 已经是最新版本。
4. 脚本执行：
   ```bash
   "$BIFROST_BIN" upgrade
   ```
5. 检查输出包含 `Detected installed Bifrost desktop app` 和 `Bifrost desktop app updated successfully`。

预期结果：

- 即使 CLI 已经是最新版本，只要检测到已安装桌面 App，`bifrost upgrade` 仍会触发桌面 App 后置更新检查。
- 后置命令使用 `app upgrade --no-cli`，不会递归执行 CLI upgrade。
- 测试只使用临时 app 目录，不触碰真实 `/Applications/Bifrost.app`。

### TC-DAU-04I 普通 bifrost upgrade 中桌面 App 更新失败不阻断主流程

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_upgrade_cli.sh
   ```
2. 在 macOS 上，脚本创建临时已安装 `Bifrost.app`，版本为 `0.0.1`。
3. 脚本通过 `BIFROST_APP_UPGRADE_TEST_PACKAGE` 注入同样是 `0.0.1` 的 stale app 包，并设置目标版本为当前 CLI 版本。
4. 脚本执行：
   ```bash
   "$BIFROST_BIN" upgrade
   ```
5. 检查命令退出码为 0，输出包含 `Bifrost desktop app update failed; continuing CLI upgrade` 和 `reports version v0.0.1 instead of target`。

预期结果：

- 桌面 App 更新失败时，`bifrost upgrade` 只打印 warning 和失败原因。
- CLI upgrade 主流程不因 App 更新失败变成失败退出。
- 输出中包含可操作的手动重试提示 `bifrost app upgrade --no-cli -y`。

### TC-DAU-04J 旧桌面 App 复用新 CLI 时仍提示桌面更新

操作步骤：

1. 确认现场或临时环境中桌面 App 版本低于最新 release，例如：
   ```bash
   /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' /Applications/Bifrost.app/Contents/Info.plist
   ```
2. 确认运行中的 core/CLI 已是最新版本，例如：
   ```bash
   bifrost --version
   curl -fsS 'http://127.0.0.1:9900/_bifrost/api/system/version-check?refresh=true' | python3 -m json.tool
   ```
3. 调用 desktop channel：
   ```bash
   curl -fsS 'http://127.0.0.1:9900/_bifrost/api/system/version-check?refresh=true&channel=desktop' | python3 -m json.tool
   ```
4. 检查 desktop channel 返回的 `current_version` 等于桌面 App bundle 版本，而不是 CLI/core 版本。
5. 如果 latest release 高于桌面 App bundle 版本，检查返回 `has_update=true`。

预期结果：

- 旧 App 复用新 CLI/core 时，桌面端版本检查仍以已安装 App bundle 版本作为当前版本。
- 桌面状态栏的启动检查能看到桌面 App 更新，不会因为 CLI/core 已经最新而静默。
- 如果无法读取 App bundle 版本，接口才回退到 CLI/core 版本并保持原有兼容行为。

### TC-DAU-05 Admin API 以真实 runtime owner 决定桌面/CLI 编排

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-admin handlers::system::tests::runtime_owner_overrides_the_request_channel --lib
   cargo test -p bifrost-admin handlers::system::tests::upgrade_process_args_separate_cli_and_desktop_channels --lib
   ```
2. 检查测试通过。

预期结果：

- CLI-owned core 即使收到 desktop query 也派发 `self-update --source admin`，并携带精确 PID/port。
- App-owned core 即使收到 CLI query 也派发 `app upgrade --source desktop -y`，不携带 CLI restart hint。
- desktop orchestrator 调用独立 CLI 时注入 skip-app/skip-restart，避免递归安装与双重重启。

### TC-DAU-06 Web UI 标记所在 shell，服务端仍以 runtime owner 为准

操作步骤：

1. 执行：
   ```bash
   pnpm --dir web run test:unit -- src/stores/useVersionStore.test.ts
   ```
2. 检查测试通过，并确认 CLI mode 仍调用 `checkVersion(true, "cli")`。
3. 代码 review `web/src/stores/useVersionStore.ts`，确认 `isDesktopShell()` 为真时 `checkVersion` 与 `startUpgrade` 使用 `desktop` channel，且桌面缓存窗口为 `6 * 60 * 60 * 1000`。
4. 代码 review `web/src/pages/Settings/tabs/ProxyTab.tsx`，确认 CLI / AI Skills 安装按钮位于 `desktopMode ? (...) : null` 分支内。

预期结果：

- 普通 Web UI 仍发送 CLI 标记，桌面 shell 仍发送 desktop 标记。
- 服务端不把客户端标记当作重启所有权；最终以实际 core 是 CLI-owned 还是 App-owned 为准。
- 浏览器打开的 CLI Web UI 不展示 App -> CLI / AI Skills 按钮。

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

### TC-DAU-06C App 弹窗默认安装 CLI 与 AI skills 不应超过前端超时

操作步骤：

1. 执行：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
2. 脚本启动临时 Bifrost 服务，并通过 `BIFROST_INSTALL_SKILL_DIR=<mktemp>/api-skills/bifrost` 隔离 AI skill 目标目录。
3. 脚本调用与桌面弹窗一致的安装路径：
   ```bash
   POST /_bifrost/api/system/cli-install
   {"install_dir":"<mktemp>/api-cli-bin","install_skills":true}
   ```
   该请求必须使用 30 秒上限执行。
4. 检查响应 `installed=true`、`skills_installed=true`，`skills_message` 包含 `embedded desktop bundle`。
5. 检查 `<mktemp>/api-skills/bifrost/SKILL.md` 与 `<mktemp>/api-skills/bifrost-remote/SKILL.md` 均已写入。

预期结果：

- 桌面 App 弹窗点击 `Install CLI` 时，不再先访问 GitHub raw 下载 skill 后才回退，避免前端 30 秒超时。
- 后端使用随桌面包内置的 Bifrost skills 安装，网络 429、DNS 慢或离线时不影响 CLI 安装。
- 即使 AI skills 后续失败，CLI 复制成功也应返回 `installed=true`，并通过 `skills_installed=false` 与 `skills_message` 提示用户手动重试。

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

### TC-DAU-08B 桌面 app 感知外部 CLI 停止并手动启动内置 core

操作步骤：

1. 构建当前源码的 CLI 与桌面壳，并准备临时数据目录：
   ```bash
   pnpm --dir web run build:desktop
   SKIP_FRONTEND_BUILD=1 RUSTC_WRAPPER= cargo build -p bifrost-cli
   node scripts/prepare-tauri-sidecar.mjs debug
   SKIP_FRONTEND_BUILD=1 RUSTC_WRAPPER= CARGO_TARGET_DIR=target/desktop-formal CARGO_BUILD_JOBS=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
   TEST_DIR=/tmp/bifrost-desktop-formal-19900
   rm -rf "$TEST_DIR"
   mkdir -p "$TEST_DIR"
   printf '{"proxy_port":19900}\n' > "$TEST_DIR/desktop-config.json"
   ```
2. 先启动外部 CLI core：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start --host 127.0.0.1 --port 19900 --skip-cert-check --no-system-proxy --no-tray
   ```
3. 用同一个 `BIFROST_DATA_DIR` 启动当前源码构建的桌面 app：
   ```bash
   BIFROST_DATA_DIR="$TEST_DIR" \
   BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1 \
   BIFROST_DESKTOP_NO_SYSTEM_PROXY=1 \
   ./target/desktop-formal/debug/bifrost-desktop
   ```
4. 确认桌面 app 正常打开，状态栏或 System Proxy 卡片显示 `http://127.0.0.1:19900`。
5. 停止步骤 2 的 CLI core，例如在前台进程按 `Ctrl+C`。
6. 等待 watchdog 轮询，观察桌面 app 出现全屏 `Start Bifrost Service` 浮层，中央只有启动服务入口。
7. 点击 `Start Bifrost Service`。
8. 观察浮层关闭，页面回到 Activity，底部状态栏显示 `Proxy: Running`，System Proxy 卡片继续显示 `http://127.0.0.1:19900`。
9. 检查进程与日志：
   ```bash
   ps -axo pid,ppid,stat,comm,args | rg 'target/debug/bifrost start|target/desktop-formal|19900'
   tail -80 "$TEST_DIR/logs/desktop-bootstrap.log"
   ```

预期结果：

- 启动阶段桌面 app 复用外部 CLI core，不额外启动第二个内置 sidecar。
- 外部 CLI 停止后，桌面 app 不静默自动恢复，而是显示全屏浮层提示启动 Bifrost 服务。
- 用户点击按钮后，桌面 app 启动内置 sidecar，日志包含 `desktop backend start requested; reason=frontend request` 和 `desktop backend start succeeded; active_port=19900 reason=frontend request`。
- 页面恢复运行态并持续刷新 core 状态；watchdog 不因单次瞬时健康探针失败反复重启 core。

### TC-DAU-08C 桌面 app 未检测到 CLI 时提示安装 CLI

操作步骤：

1. 使用临时 `PATH` 或未安装 CLI 的 macOS 用户会话启动桌面 app，确保 core 已经 ready。
2. 观察桌面 app 启动后的全屏浮层。
3. 如果执行真实安装，点击 `Install CLI`；如果避免污染用户命令路径，则只验证浮层展示，并用 TC-DAU-06B 的临时目录 API 覆盖安装动作。
4. 真实安装成功后点击文档按钮，确认浏览器打开 CLI/桌面文档页。

预期结果：

- core ready 后，如果 `GET /api/system/cli-install` 返回 `installed=false`，桌面 app 显示 `Install Bifrost CLI` 浮层。
- 浮层包含安装按钮；安装中按钮显示 loading；安装成功后显示成功状态和文档入口。
- 安装按钮不修改系统代理、不安装 CA，只安装 CLI 与 AI skills。

### TC-DAU-08D CLI/core 升级重启恢复后自动关闭 Start Service 浮层

操作步骤：

1. 执行 Rust 回归测试，模拟桌面 runtime 已进入手动启动错误态后，同一端口的外部 core 恢复健康：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml healthy_external_backend_clears_manual_start_gate
   ```
2. 执行负向回归测试，模拟端口仍不健康时错误态不能被误清除：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml unhealthy_external_backend_keeps_manual_start_gate
   ```
3. 代码 review `desktop/src-tauri/src/main.rs`，确认 `monitor_desktop_backend` 的 healthy 分支会调用 `clear_backend_unavailable_if_healthy(...)`，并且 `desktop_runtime_snapshot(...)` 在返回给前端前也执行同一健康恢复对账。
4. 代码 review `web/src/App.tsx`，确认 `DesktopStartupGate` 每 3 秒轮询 `getDesktopRuntime()`，当 `startupError=null` 且 `startupReady=true` 后 `coreNeedsAttention=false`，全屏 `Start Bifrost Service` 浮层不再渲染。

预期结果：

- CLI 升级或外部 core 自行重启导致的短暂连接断开可以临时展示 `Start Bifrost Service`。
- core 在同一端口恢复健康后，下一轮 watchdog 或 `get_desktop_runtime` 轮询会清空 `startup_error`、置 `startup_ready=true`。
- 前端无需用户点击按钮即可自动关闭 `Start Bifrost Service` 浮层，恢复原页面。
- 端口仍不健康时不会误关浮层，用户仍可手动点击 `Start Bifrost Service`。

### TC-DAU-08E Settings 中 CLI 与 AI Skills 安装按钮分离

操作步骤：

1. 执行前端单测，覆盖 `GET /api/system/cli-install` 返回 `installed=false`、`installed=true, skills_installed=false`、`installed=true, skills_installed=true` 三类状态：
   ```bash
   pnpm --dir web run test:unit -- src/pages/Settings/tabs/ProxyTab.test.ts
   ```
2. 代码 review `web/src/pages/Settings/tabs/ProxyTab.tsx`，确认 CLI 缺失时 `Install CLI` 调用 `installCliFromDesktop({ install_skills: false })`。
3. 代码 review 同一文件，确认 CLI 已安装时不渲染 `settings-install-cli`，只渲染 `settings-install-skills`；Skills 已安装时按钮文案为 `Reinstall AI Skills`。

预期结果：

- CLI 未安装或状态未知时，只展示 CLI 安装按钮，不展示 AI Skills 安装按钮。
- CLI 已安装后，不再展示 `Install CLI` 或 `Install CLI & Skills`。
- AI Skills 安装按钮仅在 CLI 已安装后出现，且执行时只表达 Skills 安装/修复语义。

### TC-DAU-08F Settings Desktop Proxy Core 端口按钮与输入框底边对齐

操作步骤：

1. 代码 review `web/src/pages/Settings/tabs/ProxyTab.tsx`，确认端口输入行使用 `Row align="bottom"`，并保留输入框与 `Apply & Restart` 按钮在同一 `settings-desktop-port-row` 内。
2. 检查 `settings-desktop-port-input` 和 `settings-desktop-port-apply` 的 DOM test id 存在，便于后续桌面 UI 像素/坐标回归。
3. 对照截图中的 Settings -> Proxy -> Desktop Proxy Core 区域，确认按钮底边应与输入框底边对齐，而不是相对包含 label 的整组垂直居中。

预期结果：

- `Apply & Restart` 按钮底边与 Proxy Port 输入框底边对齐。
- Proxy Port label 仍位于输入框上方，状态文案仍独立位于控件行下方。
- 调整仅影响 Desktop Proxy Core 端口行，不改变 Command Line & AI Tools 行和其它 Settings 卡片布局。

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
2. 等待前端 ready handoff 完成，打开 Settings -> Proxy，观察窗口顶部和底部状态栏。
3. 截图保存启动后的完整桌面窗口。
4. 检查进程只存在 Bifrost 桌面窗口，不依赖额外原生 menu bar 提供关闭入口。

预期结果：

- 窗口顶部不显示 Windows 原生标题栏，也不显示 `Bifrost / File / Edit / View / Window` 菜单栏。
- Web UI 从窗口顶部开始使用自定义桌面 chrome，不被系统标题栏向下挤压。
- 底部状态栏完整可见，Settings 页面内容不会因为顶部系统栏占位而挤出窗口。
- 右上角自定义最小化、最大化和关闭按钮可见并可点击。

### TC-DAU-10B Windows handoff 后保持无边框自定义 chrome

操作步骤：

1. 在 Windows 11 桌面环境中启动当前构建的 Bifrost Desktop。
2. 观察启动初始窗口、Bifrost core ready、前端 ready handoff 后的主界面。
3. 在左侧侧栏导航项、OpenAPI、主题切换按钮或顶部 35px 空白区域按住鼠标拖动窗口。
4. 切换暗色/亮色主题，分别观察顶部自定义 chrome 与页面内容间距。
5. 截图保存 handoff 后的完整桌面窗口。

预期结果：

- handoff 后仍然不显示 Windows 原生标题栏，也不显示 `Bifrost / File / Edit / View / Window` 原生菜单栏。
- 窗口不因 handoff 被系统标题栏向下挤压；底部状态栏持续完整可见。
- 左侧侧栏和顶部 35px 区域均可作为拖拽起点，窗口移动跟随鼠标，不触发文本选择。
- 暗色和亮色主题下顶部空白区、左侧侧栏和右上角自定义窗口按钮风格一致。

### TC-DAU-11 App 更新重启 handoff 禁止复用旧 core

操作步骤：

1. 执行桌面 handoff contract 测试：
   ```bash
   bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh
   ```
2. 检查测试覆盖 `upgrade_relaunch_marker_activity_requires_fresh_supported_marker`、`upgrade_relaunch_marker_round_trips_and_stale_marker_is_removed`、`active_upgrade_relaunch_marker_disables_existing_backend_reuse`。
3. 代码复核 `desktop/src-tauri/src/main.rs` 中 `restart_desktop_after_update` 会写入 `desktop-upgrade-relaunch.json` 并启动 helper。
4. 代码复核 `ensure_backend_running` 在 active marker 存在时不会调用 `find_existing_backend_port` 复用旧 core，而是等待旧端口释放后再执行 stale marker cleanup 与新 sidecar 启动。

预期结果：

- App 更新完成后，新 App 不会复用旧 App shutdown helper 即将停止的 core。
- fresh marker 可被读取并触发 handoff；stale/unsupported marker 会被删除，避免长期阻塞普通启动。
- 新 sidecar 启动成功后 marker 被清理。
- Linux runner 如未安装 Tauri GTK/glib 系统依赖，或任一 runner 未准备 `desktop/src-tauri/resources/bin/*` sidecar 资源，脚本应明确输出 `SKIP` 并返回成功；该跳过只代表 runner 不具备桌面 crate 编译前置条件，不代表 handoff 合约失败。

### TC-DAU-12 App 更新重启日志可追踪 handoff 生命周期

操作步骤：

1. 在 macOS 桌面会话中触发桌面端更新，或在本地构建中调用 `restart_desktop_after_update` 对应前端路径。
2. 查看共享数据目录中的 `logs/desktop-bootstrap.log`。
3. 检查日志按顺序包含：
   - `desktop upgrade relaunch marker written`
   - `desktop upgrade relaunch helper started`
   - `desktop upgrade handoff is active; skipping existing backend reuse`
   - `desktop upgrade relaunch marker cleared after managed backend start`
4. 检查 `desktop-upgrade-relaunch.json` 在新 core ready 后不存在。
5. 检查 App 不再显示 `Start Bifrost Service` 浮层；如 CLI 缺失，进入 `Install Bifrost CLI` 流程。

预期结果：

- 更新重启生命周期可以通过日志完整追踪，便于现场诊断。
- 新 App ready 后 core 由新 App 管理，不再出现旧 stop helper 事后杀掉新 core 的竞态。

### TC-DAU-13 CLI install 遇到 core 重连时自动复查状态

操作步骤：

1. 打开桌面端 App，在 CLI 缺失或临时隔离数据目录下进入 `Install Bifrost CLI` 浮层。
2. 点击 `Install CLI` 后，在请求过程中模拟 core 短暂重启或网络连接中断。
3. 等待桌面前端 3 秒轮询刷新 runtime 与 CLI status。
4. 如果 CLI 已复制到安装目录，检查浮层切换为 `CLI Installed` 或关闭 CLI 缺失提示。
5. 如果 core 仍未 ready，检查 UI 回到 `Start Bifrost Service`，而不是固定显示不可恢复的 network error。

预期结果：

- CLI 安装请求期间发生 transient network error 时，前端不会直接把一次连接中断判定为最终安装失败。
- core ready 后前端会重新读取 CLI install status；若已安装成功，用户看到成功态。

### TC-DAU-14 macOS 更新 helper 只能 relaunch 一次

操作步骤：

1. 构建当前源码的真实 macOS App bundle：
   ```bash
   BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 pnpm run desktop:build:app
   ```
2. 记录当前桌面 App PID，然后受控退出旧 App；使用 `BIFROST_APP_SKIP_RESTART=1`
   将构建产物安装到 `/Applications/Bifrost.app`，避免安装命令额外触发一次启动。
3. 写入一次性 `desktop-upgrade-relaunch.json`，以修复后的
   `/Applications/Bifrost.app/Contents/MacOS/bifrost-desktop` 进入 helper 模式。
4. 等待日志出现 `desktop upgrade relaunch helper started` 后删除测试 marker，再结束步骤 2
   记录的旧 App PID；测试 marker 使用未监听的临时端口，避免影响正式 `9900` core。
5. 等待 helper 通过 LaunchServices 打开新 App，记录新 PID；连续观察至少 10 秒。
6. 检查新 PID 的 launchd 环境不包含
   `BIFROST_DESKTOP_UPGRADE_RELAUNCH_HELPER`、`BIFROST_DESKTOP_UPGRADE_RELAUNCH_MARKER`、
   `BIFROST_DESKTOP_UPGRADE_RELAUNCH_TARGET`，并检查 `desktop-bootstrap.log` 完成 WebView handoff。

预期结果：

- helper 只打开一次目标 App，不产生 `helper -> open -> helper` 递归链。
- 新 App 在观察窗口内保持同一个 PID，Dock 图标不再反复弹跳、退出和重启。
- 测试 marker 不残留；正式 `9900` core 不被停止或替换。
- 新 App 正常进入桌面启动路径，日志出现 `embedded webview handoff completed`。

### TC-DAU-15 App 更新 handoff 不误认同端口外部健康服务

操作步骤：

1. 复核现场日志 `~/.bifrost/logs/desktop-bootstrap.log` 中的更新 handoff 片段，确认存在以下顺序：
   - `desktop upgrade relaunch marker written`
   - `desktop upgrade handoff is active; skipping existing backend reuse on port 9900`
   - `starting desktop backend attempt ... port=9900`
   - `desktop backend ready on 127.0.0.1:9900`
   - `managed backend child ... exited with status`
   - `watchdog reusing healthy backend on port 9900`
2. 执行 desktop handoff shell 合约：
   ```bash
   node scripts/prepare-tauri-sidecar.mjs
   bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh
   ```
3. 执行 managed child ready 身份校验单测：
   ```bash
   CARGO_TARGET_DIR="$PWD/target/desktop-upgrade-handoff-contract" \
     cargo test --manifest-path desktop/src-tauri/Cargo.toml wait_for_backend -- --nocapture
   ```
4. 代码复核 `desktop/src-tauri/src/main.rs` 中 `wait_for_backend` 先检查 child 是否已退出，再要求 `is_backend_ready(port)` 与 `runtime_marker_matches_child(data_dir, child_pid, port)` 同时为真。
5. 代码复核 `runtime_marker_matches_child` 只接受 `runtime.json` 中 `pid` 与 `port` 同时匹配新拉起 child 的情况。

预期结果：

- App 更新 handoff 期间，即使 `127.0.0.1:9900` 已有其他健康 Bifrost 进程响应，新 App 也不会把该响应当作新 sidecar ready。
- 如果新 sidecar 因端口竞争或启动失败提前退出，`wait_for_backend` 返回 child exited 错误，保留可诊断失败，而不是提前清理 handoff marker 并进入错误的 managed-ready 状态。
- 当 `runtime.json` 的 `pid` 与 `port` 属于新拉起 child 时，managed startup 正常接受 ready，避免误伤正常启动路径。

### TC-DAU-16 独立 CLI 版本探测可恢复 Unix ETXTBSY 瞬态碰撞

操作步骤：

1. 执行确定性 spawn 重试、版本输出解析与独立失败/超时 fixture 回归：
   ```bash
   source ~/.zshrc
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin cli_version_probe_ --lib -- --nocapture
   ```
2. 连续执行原 workspace 失败用例：
   ```bash
   source ~/.zshrc
   for run in {1..20}; do
     SKIP_FRONTEND_BUILD=1 cargo test -q -p bifrost-admin cli_version_probe_parses_output_and_rejects_failure_or_timeout --lib || exit 1
   done
   ```

预期结果：

- Unix `ETXTBSY` 前两次失败、第三次成功时恢复；持续 `ETXTBSY` 时恰好尝试 8 次后停止，总线性退避不超过 140ms。
- 非 `ETXTBSY` 启动错误只尝试一次；版本命令失败、超时和路径缺失仍返回不可用。
- 版本解析能从输出读取 `0.0.155` / `v0.0.156`，无关输出返回空；失败、超时使用不同的临时 fixture，连续 20 轮均通过，不再因覆写后立即执行同一路径而产生测试竞争。

### TC-DAU-17 已卡死用户升级到修复版本后自动恢复

操作步骤：

1. 执行完整 Desktop handoff contract；测试只使用临时 data dir 与随机端口：
   ```bash
   bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh
   ```
2. 单独执行旧失败状态恢复回归；fixture 同时覆盖当前 marker 和没有
   `target_version` 的历史 marker：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     failed_cli_owned_handoff_retries_without_another_thirty_second_wait -- --nocapture
   ```
3. 执行 shutdown ownership 回归，确认旧 App 退出不会再次停止 CLI updater 已经启动的
   external core：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     desktop_shutdown_stops_only_a_backend_owned_by_the_desktop -- --nocapture
   ```
4. 执行目标版本恢复与错误版本阻断回归：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     healthy_target_backend_completes_and_clears_cli_upgrade_handoff -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     healthy_wrong_version_backend_does_not_bypass_cli_upgrade_handoff -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     healthy_target_backend_on_another_port_does_not_complete_cli_upgrade_handoff -- --nocapture
   ```
5. 执行同 PID target 复用、空闲端口 fallback、同 data dir 旧 core 安全接管和无关占用拒绝
   回归：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     cli_owned_upgrade_relaunch -- --nocapture
   ```
6. 检查所有 fixture 的 data dir 都来自 `tempfile`，监听端口为随机端口或隔离端口
   `19900`；测试前后不停止、不替换 `127.0.0.1:9900` 的正式服务。

预期结果：

- 用户已被旧版本留下 fresh CLI-owned marker 与 `Failed` progress 时，安装并打开修复版本
  后立即进入恢复，不再每次启动或点击 `Start Bifrost Service` 都重复等待 30 秒。
- 旧 marker 即使没有 `target_version` 也能识别同类历史失败并零等待恢复。
- CLI updater 已提供目标版本 core 时直接复用；pinned target 命中时不强制要求第三个 PID。
- 外部 CLI/core 没有恢复且端口已释放时，桌面端自动接管并启动内置 core。
- 端口上的旧版本 Bifrost core 只有在 `/api/system` PID/port 与同 data dir
  `runtime.json` 双重匹配时才允许停止后接管；无关占用仍 fail-closed。
- 旧 Desktop shell 退出时只停止 managed child 或身份匹配的 desktop runtime，不停止
  daemon/unknown/stale external runtime；CLI-owned helper 也不等待该端口释放。
- 健康 target core 只有同时位于 marker 记录端口时才会写 `Completed` 并清理 marker；健康但
  错误版本或位于其它端口的 core 都不能绕过 handoff。

### TC-DAU-18（回归）：direct app upgrade 先退出旧壳再安装并启动新壳

操作步骤：

1. 构建当前 CLI：
   ```bash
   cargo build -p bifrost-cli
   ```
2. 在 macOS 桌面会话中执行隔离 App 更新 E2E：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_desktop_app_update_cli.sh
   ```
3. 脚本会编译两个临时 `.app` fixture。先安装并直接启动 `0.0.139` 旧壳，再从普通 CLI
   执行 caller-managed `bifrost app upgrade --package <0.0.140 fixture> --no-cli`。
4. 检查命令输出包含
   `Requesting the running desktop shell to release its installed files`，旧 PID 在安装完成前
   退出，目标 bundle 安装后由 LaunchServices 启动新的 PID。

预期结果：

- direct caller-managed App upgrade 检测到已安装旧 App 正在运行时，先走 Desktop 内部
  shutdown 通道并等待旧 PID 退出，再替换 bundle，避免旧壳继续占用或执行旧代码。
- 安装成功后启动的是新 bundle；脚本创建的新版本 marker 存在，marker 中的新 PID 保持
  运行，且不同于已退出的旧 PID。
- `source=desktop` 且 handoff 环境有效的 WebView/App-owned 更新仍由当前 Desktop shell
  管理重启，不在安装前自杀；本回归只验证 direct caller-managed 路径。
- 所有 `.app`、marker、data-dir 和进程都位于 `mktemp` 测试目录并由 trap 定向清理，不改
  `/Applications`，不停止正式 `9900/9901` Service。

### TC-DAU-19 Settings 刷新后保持 AI Skills 已安装状态

操作步骤：

1. 使用隔离的 `BIFROST_DATA_DIR`、`BIFROST_INSTALL_DIR` 和 `BIFROST_INSTALL_SKILL_DIR` 启动当前分支 Bifrost。
2. 调用 `POST /_bifrost/api/system/cli-install`，请求体设置 `install_skills=true`，确认安装主 `bifrost` 与 `bifrost-remote` skill 文件。
3. 再调用 `GET /_bifrost/api/system/cli-install`，模拟离开 Settings 后重新进入或点击 Refresh。
4. 执行前端状态单测，确认 `installed=true, skills_installed=true` 对应 `Reinstall AI Skills`，不是 `Install AI Skills`。
5. 删除任一必需 `SKILL.md` 后执行状态检测单测，确认完整性检查回落为未安装。

预期结果：

- Skills 安装成功后，后续 GET 仍返回 `skills_installed=true`，状态不会只保存在当前 React 页面内。
- Settings 重新进入或刷新后展示 `AI skills installed`，按钮文案为 `Reinstall AI Skills`。
- 默认安装完整性同时检查 Universal Agent Skills 与 Claude Code 下的 `bifrost`、`bifrost-remote` 四个文件；隔离测试 override 路径检查对应主/远程两个文件。
- 缺少任一必需文件时不误报已安装，用户仍可执行安装修复。

**回归目的**：覆盖 issue #497 报告的 Windows 11 安装 AI Skills 后切换页面再返回 Settings 又显示 `Install AI Skills` 的问题。

**执行结果（2026-08-16，本地隔离 API + 单元测试）**：
- ✅ PASS：`cargo test -p bifrost-admin skill_install --lib` 3/3 通过，覆盖完整 bundle、缺失文件、override 路径与安装结果映射。
- ✅ PASS：`BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh` 40/40 通过；使用隔离目录完成真实 POST → skill 文件落盘 → GET，刷新后的响应保持 `skills_installed=true`。
- ✅ PASS：`pnpm --dir web run test:unit -- src/pages/Settings/tabs/ProxyTab.test.ts` 通过；`installed=true, skills_installed=true` 映射为 `Reinstall AI Skills`。

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
| 2026-08-16 | TC-DAU-19 | `cargo test -p bifrost-admin skill_install --lib`；`BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh`；`pnpm --dir web run test:unit -- src/pages/Settings/tabs/ProxyTab.test.ts` | PASS：完整/缺失/override 状态与结果映射 3/3，通过真实隔离 POST→落盘→GET 40/40，刷新后保持 `skills_installed=true`，前端映射为 `Reinstall AI Skills`。 |
| 2026-08-15 | TC-DAU-04G2 alpha.8 干净安装基线 | Parallels Windows 11 ARM64 VM：SYSTEM/HKCU 双阶段清理返回 `system_msi_entries=0`、`all_uninstall_entries=0`、进程与残留目录均为 0；Release run `31815005722` 的 ARM64 CLI/MSI workflow artifacts 通过 SHA-256 校验后安装；CLI=`bifrost 0.0.181-alpha.8`，Desktop ProductVersion=`0.0.181-10008`，MSI DisplayVersion=`0.0.181.10008`，唯一 ProductCode=`{581E598F-99BD-4700-BA49-57EB820C7290}` | BASELINE PASS：已从彻底清理状态安装首个同时包含 channel discovery 与 staged-target handoff 的 alpha；等待发布 alpha.9 后执行默认 `bifrost upgrade -y` 完成相邻版本验证 |
| 2026-07-05 | TC-DAU-01 / 02 / 03 / 04 / 04B / 04C / 06B | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh` | 待复测：本轮新增 04C，验证已安装桌面端等于目标版本时跳过下载和重装 |
| 2026-07-05 | TC-DAU-05 | `cargo test -p bifrost-admin handlers::system::tests --lib` | PASS：7/7 通过，覆盖 desktop alias、spawn args、CLI install 临时目录与 skip skills |
| 2026-07-05 | TC-DAU-06 | `pnpm --dir web run test:unit -- src/stores/useVersionStore.test.ts` + 代码 review | PASS：Vitest 22 files / 93 tests 通过；新增 desktop shell 单测确认 `checkVersion/startUpgrade` 使用 `desktop` channel，代码确认桌面缓存窗口为 6 小时，非桌面仍使用 `cli` channel；桌面安装按钮位于 `desktopMode` 分支 |
| 2026-07-05 | TC-DAU-07 / 08 | 需要真实 macOS/Windows 桌面安装包与 GUI 会话 | 未执行：当前本机未进行真实桌面包安装/GUI 通知验证；需在发布包或本地 `.dmg/.msi` 准备后补跑 |
| 2026-07-06 | TC-DAU-04C | Parallels Windows 11 ARM64 VM：先用 v0.0.139 MSI 复现 `msiexec /i <msi> /qn /norestart`，再用 `ALLUSERS=2 MSIINSTALLPERUSER=1` 验证普通用户安装 | PASS：原命令稳定复现 1603，日志显示 `Error 1925`；加 per-user MSI 属性后同一 MSI 安装成功并写入 `%LOCALAPPDATA%\Bifrost` |
| 2026-07-06 | TC-DAU-09 | Parallels Windows 11 ARM64 VM：先复现桌面快捷方式启动后额外出现 shell 窗口；修复后构建 `target-desktop-verify/debug/bifrost-desktop.exe`，以 `BIFROST_DESKTOP_BIN=desktop/src-tauri/resources/bin/bifrost.exe` 启动并截图 `/tmp/bifrost-windows-custom-chrome-final.png` | PASS：桌面 UI 启动后 sidecar 由桌面壳隐藏控制台启动，截图中没有独立 shell 窗口遮挡；启动验证用 `cmd start` 包装进程已清理，不属于桌面壳子进程 |
| 2026-07-06 | TC-DAU-10 | Parallels Windows 11 ARM64 VM：`CARGO_TARGET_DIR=target-desktop-verify cargo build --manifest-path desktop/src-tauri/Cargo.toml`，启动当前构建并截图 `/tmp/bifrost-windows-custom-chrome-final.png` | PASS：窗口顶部不再显示 Windows 原生标题栏，也没有 `Bifrost / File / Edit / View / Window` 原生菜单栏；Web UI 自定义右上角最小化/最大化/关闭按钮可见，底部状态栏完整可见 |
| 2026-07-06 | TC-DAU-10B | Parallels Windows 11 ARM64 VM：将当前 diff 应用到 `C:\Users\eden_studio\work\github\bifrost`，执行 `CARGO_TARGET_DIR=target-desktop-chrome-verify cargo build --manifest-path desktop/src-tauri/Cargo.toml`；通过交互计划任务启动 `target-desktop-chrome-verify\debug\bifrost-desktop.exe`，`desktop-bootstrap.log` 显示 `embedded webview page load event Finished`、`desktop backend bootstrap finished`；截图 `/Users/eden_studio/Downloads/bifrost-windows-chrome-hidden-20260706.png` 与 `/Users/eden_studio/Downloads/bifrost-windows-chrome-resized-20260706.png` | PASS：frontend ready handoff 后仍未出现 Windows 原生标题栏和 `Bifrost / File / Edit / View / Window` 菜单栏，Web UI 未被系统 chrome 向下挤压；当前 VM 可视区高度不足以在同一张截图里完整纳入底部状态栏，但回归根因的 Windows handoff decorations 路径已由单测和真实启动截图共同覆盖 |
| 2026-07-06 | TC-DAU-01 / 02 / 03 / 04 / 04B / 04C / 04D / 06B | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh` | PASS：33/33 通过。覆盖 dry-run、App -> CLI 临时安装、临时 app 真实安装/升级/卸载、同版本跳过下载、desktop source progress 为 completed，以及目标 0.0.141 但安装后仍报告 0.0.140 时非零退出并写 `phase=failed` |
| 2026-07-06 | TC-DAU-08B / 08C | macOS 真实桌面会话：`BIFROST_DATA_DIR=/tmp/bifrost-desktop-formal-19900 ./target/debug/bifrost start --host 127.0.0.1 --port 19900 --skip-cert-check --no-system-proxy --no-tray`，随后以同 data dir 启动 `./target/desktop-formal/debug/bifrost-desktop`；停止外部 CLI 后观察 UI；通过 AX 点击 `Start Bifrost Service`；查看 `/tmp/bifrost-desktop-formal-19900/logs/desktop-bootstrap.log`、`curl http://127.0.0.1:19900/_bifrost/api/proxy/system/support` 与进程表 | PASS：桌面 app 启动时复用外部 CLI core；停止 CLI 后显示全屏 `Start Bifrost Service` 浮层；点击按钮后拉起内置 sidecar，进程表出现 `target/debug/bifrost start --host 0.0.0.0 --port 19900 --skip-cert-check --no-system-proxy`，页面恢复 Activity 并显示 `http://127.0.0.1:19900`，健康接口返回 `{"supported":true,"platform":"macOS"}`；显式禁用系统代理时状态栏显示 `Proxy: Not Applied` 属预期；10 秒稳定观察未再出现 watchdog 误恢复。随后出现 `Install Bifrost CLI` 浮层。未点击真实 `Install CLI`，避免写入用户命令路径；安装动作由 TC-DAU-06B 临时目录 API 覆盖 |
| 2026-07-07 | TC-DAU-01 / 02 / 03 / 04 / 04B / 04C / 04D / 06B / 06C | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh` | PASS：36/36 通过。新增覆盖 App 弹窗默认 `install_skills=true` 路径，请求设置 30 秒上限，后端使用 embedded desktop bundle，响应 `skills_installed=true`，并在隔离 `BIFROST_INSTALL_SKILL_DIR` 写入 `bifrost/SKILL.md` 与 `bifrost-remote/SKILL.md` |
| 2026-07-07 | TC-DAU-08D | `cargo test --manifest-path desktop/src-tauri/Cargo.toml healthy_external_backend_clears_manual_start_gate`; `cargo test --manifest-path desktop/src-tauri/Cargo.toml unhealthy_external_backend_keeps_manual_start_gate`; 代码 review `desktop/src-tauri/src/main.rs` 与 `web/src/App.tsx` | PASS：恢复健康的一次性 mock backend 会让 `clear_backend_unavailable_if_healthy` 置 `startup_ready=true` 并清空 `startup_error`；端口仍不健康时返回 false 且保留错误态；代码确认 watchdog healthy 分支和 runtime snapshot 都会对账，前端 3 秒轮询后 `coreNeedsAttention=false` 自动关闭 Start Service 浮层 |
| 2026-07-07 | TC-DAU-08E / 08F | `pnpm --dir web run test:unit -- src/pages/Settings/tabs/ProxyTab.test.ts`; `pnpm --dir web run lint`; `pnpm --dir web run build`; 代码 review `web/src/pages/Settings/tabs/ProxyTab.tsx` | PASS：CLI 缺失时只展示 `Install CLI` 且请求 `install_skills=false`；CLI 已安装后隐藏 CLI 安装按钮并展示独立 AI Skills 按钮；Skills 已安装时文案为 `Reinstall AI Skills`。端口行改为 `Row align="bottom"`，输入框与 `Apply & Restart` 位于同一 test id 行，便于后续像素/坐标回归 |
| 2026-07-10 | TC-DAU-11 | `bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh` | PASS：macOS 本地 3/3 通过，覆盖 fresh/stale/unsupported marker 判定、stale marker 自动删除、active upgrade marker 禁止复用既有 backend；Linux CI runner 缺 `glib-2.0.pc` 或通用 shell shard 缺 `desktop/src-tauri/resources/bin/*` 时预期输出 SKIP，避免桌面编译前置条件缺失误伤 shell E2E |
| 2026-07-10 | TC-DAU-12 / 13 | 代码 review `desktop/src-tauri/src/main.rs`、`web/src/App.tsx`，真实 App 更新 GUI 路径需发布包/桌面会话补跑 | 待复测：本轮新增日志可追踪 handoff 生命周期与 CLI install transient reconnect 后自动复查状态 |
| 2026-07-14 | TC-DAU-14 | 构建并 ad-hoc 签名真实 `Bifrost.app`，安装到 `/Applications`；以 one-shot marker 启动修复后的 helper，helper 读取 marker 后删除测试 marker 并释放 hold PID；检查新 App PID、launchd 环境、正式 core PID 与 `desktop-bootstrap.log` | PASS：helper 只打开一次 App；新 PID `75504` 连续 10 秒稳定；三项 `BIFROST_DESKTOP_UPGRADE_RELAUNCH_*` 环境变量均未继承；marker 不残留；正式 core PID `19574` 未变化；WebView handoff 与证书预检完成。首次安装准备因同版本跳过覆盖而失败，改为先卸载旧 bundle、签名并恢复安装后完整重跑通过 |
| 2026-07-16 | TC-DAU-15 | 现场日志复核 `desktop-bootstrap.log:14374-14410`；`node scripts/prepare-tauri-sidecar.mjs && bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh`；`CARGO_TARGET_DIR="$PWD/target/desktop-upgrade-handoff-contract" cargo test --manifest-path desktop/src-tauri/Cargo.toml wait_for_backend -- --nocapture` | PASS：日志确认 App 更新后新进程跳过复用、在 9900 上误读健康响应、随后 managed child 退出并由 watchdog 复用健康后端；handoff 合约 5/5 通过；`wait_for_backend` 3/3 通过，覆盖外部健康服务不能满足 managed child ready 与匹配 runtime marker 正常 ready |
| 2026-07-20 | TC-DAU-16 | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin cli_version_probe_ --lib -- --nocapture`；连续 20 轮版本解析与独立失败/超时 fixture | PASS：4 个版本探测回归通过，覆盖 `ETXTBSY` 第三次恢复、持续占用 8 次停止、非瞬态错误不重试、版本输出解析以及独立失败/超时/缺失路径；原 workspace 失败用例连续 20/20 轮通过。 |
| 2026-07-24 | TC-DAU-17 | 现场日志复核 `~/.bifrost/logs/desktop-bootstrap.log`，确认旧版本在 fresh `desktop-upgrade-relaunch.json` 下反复等待 `CLI-owned backend`，点击 `Start Bifrost Service` 也再次等待 30 秒；删除残留 marker 临时恢复本机；执行 `cargo test --manifest-path desktop/src-tauri/Cargo.toml cli_owned_upgrade_relaunch -- --nocapture` | PASS：3/3 通过。覆盖 CLI-owned core 已以新 PID/目标版本恢复时继续复用；无 CLI/core 重启且端口空闲时进入 `port is free, launching desktop-managed core` fallback，不再返回旧的 refusing 错误；端口仍被 `0.0.0.0` 占用时保留 fail-closed，拒绝启动第二个 desktop-managed core。 |
| 2026-07-24 | TC-DAU-17 完整恢复矩阵 | `bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh`；定向执行旧 Failed/legacy marker 零等待、shutdown ownership、target 完成清 marker、错误版本/错误端口阻断及 `cli_owned_upgrade_relaunch` 4 条恢复测试；测试前后读取 9900 listener 与 `/api/system` | PASS：handoff contract 全部通过；旧 marker 无 target 也不再重复等待 30 秒；同 PID target 可复用，端口空闲 fallback、同 data dir 旧 core 安全接管、无关端口 fail-closed 均通过；target core 仅在 marker 端口完成并清 marker，错误版本或错误端口不能绕过。正式服务前后均为 PID `85734`、版本 `0.0.163`，未被停止或替换。 |
| 2026-07-25 | TC-DAU-18 回归 | `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_desktop_app_update_cli.sh`；首次运行发现外部 Runner 继承 `BIFROST_DETACHED_DAEMON_CHILD=1` 令临时 daemon 不分离，补 `env -u` 后发现 macOS `/var` 与 `/private/var` 路径别名漏判，修复 canonical path 比较并完整复跑；第 2 轮增加新旧 PID 显式差异断言后再次复跑。 | PASS：39/39 通过。direct caller-managed upgrade 输出内部 shutdown 请求，旧 fixture PID 退出，目标 `.app` 安装后不同的新 PID 运行；临时 Admin Service、fixture 与 data-dir 均清理，正式 Desktop PID `58982` 及 `9900/9901` Service 保持运行。 |
