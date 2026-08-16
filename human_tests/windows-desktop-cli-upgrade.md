# Windows Desktop 与独立 CLI 自动升级

验证 Windows Desktop 发起自动更新时，已安装的独立 CLI 能在自身进程退出后由 deferred helper 安全替换，Desktop 不会通过高频版本探测重新锁住目标文件，也不会弹出终端窗口。

## 环境

- Windows 11 ARM64 Parallels VM
- Desktop 与 `%LOCALAPPDATA%\bifrost\bin\bifrost.exe` 均安装旧版本
- 默认使用 Windows VM 当前源码编译出的本地 ARM64 Desktop/CLI 资产；只有本地矩阵全部通过后才允许验证公开 Release
- 测试前保存 VM 快照，测试后清理升级脚本、pending、backup 与状态文件

## TC-WDU-01：旧版本通过 Desktop 自动升级到 alpha

1. 恢复旧版本 VM 快照，确认 Desktop 和独立 CLI 都报告旧版本。
2. 启动 Desktop，触发其自动更新到目标 alpha。
3. 监控 `bifrost.exe`、`powershell.exe`、`conhost.exe` 进程及升级 helper 日志，直到更新进入终态。
4. 重新读取 Desktop 版本与 `%LOCALAPPDATA%\bifrost\bin\bifrost.exe --version`。

预期：

- Desktop 等待 Windows deferred helper 完成后才验证 CLI，不在 helper 替换期间循环启动 `bifrost.exe --version`。
- 升级期间没有可见终端闪窗，且不会高频创建 `conhost.exe`。
- helper 日志包含替换成功与 `done`，UI/升级进度报告成功。
- Desktop 和独立 CLI 均为目标 alpha 版本。
- 不残留 `.pending.*`、`.upgrade-backup` 或 `.desktop-cli-upgrade-*.status`；helper 自清理脚本和 args 文件。

## TC-WDU-02：替换阶段发生短暂 sharing violation

1. 在 helper 即将替换旧 CLI 时，短暂用不共享写权限的句柄占用目标 exe，然后释放。
2. 等待升级终态并检查 helper 日志。

预期：

- helper 对 Win32 5、32、33 做有界重试，释放占用后继续替换，不立即进入失败回滚。
- 最终 CLI 为目标 alpha，备份被清理，状态文件为成功终态后由 Desktop 清理。

## TC-WDU-03：helper 失败信息透传

1. 在隔离目录运行测试 helper，使替换操作持续失败直至重试预算耗尽。
2. 检查 Desktop 管理的升级命令输出/进度。

预期：

- 父进程读取 `error: ...` 终态并返回 helper 的实际错误。
- 不继续执行 60 秒高频版本探测，不误报 `installed CLI version verification timed out`。
- 可恢复时旧 CLI 从 backup 恢复，日志记录恢复结果。

## TC-WDU-04：GitHub API 限流时保持 alpha 相邻版本发现

1. 完整卸载 Windows 上的 CLI、Desktop 与 MSI 注册，清理升级残留后安装
   `v0.0.181-alpha.10` ARM64 CLI/MSI。
2. 确认 GitHub Releases API 返回 403 rate limit，同时公开 Releases HTML 可访问。
3. 不设置任何测试版覆盖环境变量，直接由 alpha.10 执行 `bifrost upgrade -y`。
4. 等待更新到相邻发布的 `v0.0.181-alpha.11`，检查 CLI、Desktop、运行中 core、
   MSI 注册和升级临时文件。

预期：

- prerelease API 失败时通过公开 Releases HTML 发现同名 alpha channel 的下一版本。
- `alpha.10` 的语义版本排序高于 `alpha.9`，不会被稳定版或 beta/rc 串台。
- CLI、Desktop 与运行中 core 均为 alpha.11；MSI 注册恰好一个。
- 不残留 pending、backup、helper 脚本、args、ready、status 或 log 临时文件。
- helper 等待的是原 updater PID，更新期间没有版本探测风暴或可见终端窗口。

## TC-WDU-05：本地目录资产保持机器级 MSI 作用域并修复 AppSearch 目录漂移

1. 完整卸载 Windows 上的 CLI、Desktop 与 MSI 注册，并清理 pending、backup、helper、
   status、ready 与安装日志等升级残留。
2. 以 `ALLUSERS=1` 安装一个已有的旧版 ARM64 Desktop MSI，并把同版本独立 CLI
   安装到 `%LOCALAPPDATA%\bifrost\bin\bifrost.exe`；确认只有一条 HKLM MSI 注册。为覆盖
   已复现的目录漂移回归，在 `HKCU\Software\bifrost\Bifrost` 的默认值和 `InstallDir`
   中显式写入一个旧的 `%LOCALAPPDATA%\Bifrost` 路径作为测试夹具。
3. 在 Windows VM 的隔离源码 worktree 执行
   `scripts/windows/build-local-upgrade-assets.ps1 -Version 0.0.181-local.1`，确认同一目录生成
   release 命名格式的 CLI ZIP 与 Desktop MSI。该步骤不创建 tag，不访问 Release 下载。
4. 旧版本尚不支持 `--local-assets` 时，执行
   `scripts/windows/invoke-local-upgrade.ps1 -AssetsDir <DIR>` 兼容入口；已经包含该参数的
   CLI 直接执行 `bifrost upgrade -y --local-assets <DIR>`。在 machine-wide MSI 安装需要
   提权时批准 Windows UAC。
5. 等待升级终态，检查 CLI、Desktop、运行中 core、HKLM MSI 注册、handoff 父 PID、
   升级残留和进程事件。

预期：

- CLI 与 Desktop 只读取指定目录中的配对资产，输出明确的本地目录和目标版本；网络或
  GitHub Release 状态不影响本次更新。
- 升级识别旧 Desktop 为 machine-wide，不传 `MSIINSTALLPERUSER=1`，通过 UAC 以
  `ALLUSERS=1` 安装；新 MSI 把 Tauri `AppSearch` 的 HKCU 查询结果写入独立
  `PREVINSTALLDIR`，并把公开属性 `INSTALLDIR` 标记为 `Secure` 以穿过提权后的 MSI
  client/server 边界；只有调用方未显式提供目录时才复制旧值。因此即使发起更新的
  仍是未包含新 CLI 逻辑的旧版本，MSI 也不会覆盖显式的 `C:\Program Files\Bifrost`。
  MSI 返回真实退出码且不再出现 1639、HKLM `Error 1406`、1603 或 LocalAppData 目录漂移。
- CLI 为 `0.0.181-local.1`，Desktop ProductVersion 为 `0.0.181-40001`，MSI DisplayVersion
  为 `0.0.181.40001`，运行中 core/status 也报告 `0.0.181-local.1`。
- MSI 注册仍恰好一条且位于 HKLM，不新增 HKCU per-user 注册。
- helper 等待原 updater PID，升级完成后无 pending、backup、helper、args、ready、status
  或 log 残留，也不启动 Windows Terminal。

## TC-WDU-06：本地失败回滚与相邻本地版本锁冲突重试

1. 以 TC-WDU-05 已通过的 `0.0.181-local.1` 为基线，再构建 `0.0.181-local.2` 本地资产。
2. 复制出只用于失败注入的 Desktop 包，把 MSI 替换为非 MSI 内容；直接执行
   `bifrost app upgrade --no-cli --package <BAD_MSI> --version 0.0.181-local.2
   --app-dir "C:\Program Files\Bifrost" -y`，隔离验证 Desktop 安装事务回滚，不让 CLI 的
   “Desktop 失败仍继续更新 CLI”兼容策略混淆断言。
3. 检查失败终态、CLI/Desktop 版本、HKLM/HKCU MSI 注册、安装目录哈希与升级残留。
4. 使用有效 `local.2` 资产重试；在 pending CLI 出现后短暂持有允许读取但阻止删除/写入的
   `bifrost.exe` 文件句柄，模拟杀毒软件或版本探针，确认 helper 进入 sharing violation
   重试后释放句柄。
5. 检查最终版本、进程事件、安装路径、注册作用域和残留。

预期：

- 无效 MSI 明确失败；若 MSI 未修改安装树，事务报告 previous desktop app unchanged，不在
  未提权的父 CLI 中无意义地回写 Program Files。Desktop、CLI、HKLM 注册和 Program Files
  文件均保持 `local.1`，不留下 pending/backup/helper/status/log。
- 有效重试仅从本地目录取资产；短暂文件锁触发有界重试而不是失败，最终 CLI/Desktop/core
  均为 `local.2`。
- 整个失败与成功矩阵不创建 tag、Release 或 CI run，且 `WindowsTerminal.exe` 启动数为 0。

## TC-WDU-07：本地矩阵通过后才执行公开 Release 验收

1. 确认 TC-WDU-05 与 TC-WDU-06 的所有断言及两轮复测均通过。
2. 仅在准备正式交付时创建一个包含同一已验证 commit 的目标 tag/Release，并等待 ARM64
   CLI/MSI 资产可用。
3. 从干净的上一公开版本执行默认 `bifrost upgrade -y`，不得设置本地资产或测试覆盖变量。
4. 复用 TC-WDU-05/06 的版本、安装路径、MSI 作用域、UAC、handoff、进程与零残留断言。

预期：远端下载只替换资产来源，已在本地证明的解压、替换、MSI、验证和回滚路径保持一致。
若本地矩阵未通过，不得通过继续发 tag 来试错。

## 执行记录

- 2026-08-14：已在 Windows 11 ARM64 VM 使用 0.0.179 复现 issue #494。Desktop 在 deferred helper 替换 CLI 前约 60 秒内启动约 312 次 `bifrost.exe --version` 并高频创建 `conhost.exe`，helper 删除目标 exe 时返回 Access Denied，留下 pending 与 backup。
- 2026-08-15：`alpha.8` 已验证 stable/alpha channel 隔离；Windows VM 上 GitHub Releases
  API 返回 403，暴露 prerelease discovery 缺少公开 HTML fallback。`alpha.10` 已包含 fallback
  与 Desktop 防降级，待其 ARM64 CLI/MSI 发布后以干净安装为基线，通过默认命令更新到
  相邻 `alpha.11`，补录 TC-WDU-04 的版本、helper 日志、进程统计及残留检查结果。
- 2026-08-15：已完整清理 VM，并安装唯一的 machine-wide alpha.13 MSI
  (`DisplayVersion=0.0.181.10013`) 与独立 alpha.13 CLI；待 alpha.14 ARM64 Desktop 资产公开后
  按 TC-WDU-05 执行真实相邻版本升级。
- 2026-08-15：真实执行 alpha.13 → alpha.14。alpha 通道发现、CLI/MSI 下载与 CLI 替换均成功，
  但 Desktop MSI 返回 1603；日志确认错误参数仍为 `ALLUSERS=2 MSIINSTALLPERUSER=1`，并出现
  HKLM `Error 1406`。根因是真实 MSI 注册的 `UninstallString` 为 `REG_EXPAND_SZ`，而 alpha.14
  作用域识别只接受 `REG_SZ`，导致已匹配的 HKLM `InstallLocation` 无法解析 ProductCode；升级
  事务已回滚至 alpha.13。修复进入 alpha.15，TC-WDU-05 调整为干净 alpha.14 → alpha.15。
- 2026-08-15：后续相邻版本验证继续暴露 Windows 更新链的时序问题。alpha.14 → alpha.15
  修复了 `REG_EXPAND_SZ` 解析；alpha.15 → alpha.16 的真实 `msiexec` 命令却把 `/i`、`/qn`、
  `/norestart`、`ALLUSERS=1` 和 `/l*v` 全部包在双引号中，Windows Installer 显示帮助并返回
  1639。Alpha.17 修正 Desktop ProductVersion 探针路径，Alpha.18 修正 MSI 参数转义。
- 2026-08-15：真实执行 alpha.17 → alpha.18 时确认 CLI 自替换是 deferred 的：父 updater
  退出前启动的 Desktop 子升级仍加载 alpha.17 的旧二进制，所以命令行仍为 `"/i" ...
  "/qn"`，不能用这一跳证明 alpha.18 中的新逻辑。按照相邻版本原则，最终回归改为完整清理后
  安装 alpha.18，再默认升级到 alpha.19。
- 2026-08-15：针对 alpha.18 的实机定向探针确认参数转义修复生效，但 machine-wide 安装
  仍出现目录漂移：MSI 写入 HKLM alpha.18 注册，`InstallLocation` 却变为当前管理员的
  `%LOCALAPPDATA%\Bifrost`，原有 `C:\Program Files\Bifrost\bifrost-desktop.exe` 被移除。
  根因是提权后的 WiX 会重新解析默认 `INSTALLDIR`；修复为安装时显式传入已发现的既有安装
  目录。由于 alpha.19 不含这段修复，最终有效相邻版本回归必须使用均已包含修复的
  alpha.20 → alpha.21。
- 2026-08-15：真实执行 alpha.22 → alpha.23。Alpha 通道发现与 CLI/MSI 下载成功，CLI
  replacement 已保存为 `.bifrost.exe.pending.*`，但 Desktop MSI 虽返回 0，最终 HKLM
  `InstallLocation` 和文件都漂移到 `%LOCALAPPDATA%\Bifrost`，随后旧 Desktop 回滚因
  Access Denied 失败，CLI 仍停留在 alpha.22。保留 MSI verbose log 后确认命令行最初已把
  `INSTALLDIR` 设为 `C:\Program Files\Bifrost`，但紧接着 `AppSearch` 从
  `HKCU\Software\bifrost\Bifrost` 将其覆盖为旧 LocalAppData 路径。相同 MSI 的交互管理员
  A/B 探针中，预先固定该键的默认值和 `InstallDir` 后安装稳定落到 Program Files。进一步用
  本地 `local.1` 暂存文件做最小探针确认，新 CLI 自身能够固定注册表，但 alpha.22 发起更新时
  Desktop 子进程仍是 alpha.22：新 CLI 修复尚未执行，无法修复第一跳。因此最终修复下沉到
  Desktop MSI 的 WiX `AppSearch` 属性设计；停止继续发布 alpha 试错，后续先按 TC-WDU-05/06
  使用 VM 本地编译资产闭环。
- 2026-08-15：在 Windows 11 ARM64 VM 用隔离 worktree 本地构建 `local.1` 至 `local.6`，全程
  未创建 tag、Release 或 CI run。构建矩阵先发现脚本会从 Tauri bundle 目录误选上一版本 MSI；
  修复为构建前只清理当前 target 的生成 MSI 目录，构建后要求唯一候选且文件名包含本次
  MSI-safe 版本。修复后本地资产 MSI 与 bundle 唯一产物哈希一致。
- 2026-08-15：从干净 machine-wide alpha.22 基线，经兼容脚本升级到 `local.2`，再用已安装
  CLI 的正式 `--local-assets` 路径执行 `local.4` → `local.5`。首次直连暴露
  `std::fs::canonicalize` 产生的 `\\?\C:\...` MSI 路径令 `msiexec` 返回 1619；改用 Windows
  普通路径规范化后复测成功。最终 CLI 为 `local.5`、Desktop 为 `0.0.181-40005`，只有一条
  HKLM 注册，安装目录保持 `C:\Program Files\Bifrost`，旧 HKCU LocalAppData 夹具未覆盖它，
  `residual_count=0`。
- 2026-08-15：在 `local.5` 上注入非 MSI 文件并直接执行 Desktop-only upgrade。`msiexec`
  返回 1620，首次测试发现未变化安装树仍被非提权父进程回写，产生 rollback Access Denied；
  加入安装树内容比较后复测输出 `previous desktop app unchanged`，旧 CLI/Desktop 哈希、
  `0.0.181.40005` HKLM 注册和 Program Files 路径全部不变，Desktop 自动重新拉起且升级残留为 0。
- 2026-08-15：把独立 CLI 还原为 `local.5` 后，用正式本地资产入口升级到 `local.6`；pending
  出现后从 `18:31:48.841+08:00` 到 `18:31:56.862+08:00` 持有删除阻断句柄。helper 在释放后
  完成替换，CLI 报告 `local.6` 且无 pending/backup/helper/status/log 残留。随后在 CLI 已最新
  的情况下用同一 `--local-assets` 补齐 Desktop，最终 Product/File 版本均为
  `0.0.181-40006`、DisplayVersion 为 `0.0.181.40006`、唯一 HKLM InstallLocation 为
  `C:\Program Files\Bifrost`、`residual_count=0`。
