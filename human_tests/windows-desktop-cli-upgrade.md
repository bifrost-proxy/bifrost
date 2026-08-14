# Windows Desktop 与独立 CLI 自动升级

验证 Windows Desktop 发起自动更新时，已安装的独立 CLI 能在自身进程退出后由 deferred helper 安全替换，Desktop 不会通过高频版本探测重新锁住目标文件，也不会弹出终端窗口。

## 环境

- Windows 11 ARM64 Parallels VM
- Desktop 与 `%LOCALAPPDATA%\bifrost\bin\bifrost.exe` 均安装旧版本
- GitHub Releases 中存在高于旧版本的 ARM64 alpha Desktop/CLI 资产
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

## 执行记录

- 2026-08-14：已在 Windows 11 ARM64 VM 使用 0.0.179 复现 issue #494。Desktop 在 deferred helper 替换 CLI 前约 60 秒内启动约 312 次 `bifrost.exe --version` 并高频创建 `conhost.exe`，helper 删除目标 exe 时返回 Access Denied，留下 pending 与 backup。
- 2026-08-15：`alpha.8` 已验证 stable/alpha channel 隔离；Windows VM 上 GitHub Releases
  API 返回 403，暴露 prerelease discovery 缺少公开 HTML fallback。`alpha.10` 已包含 fallback
  与 Desktop 防降级，待其 ARM64 CLI/MSI 发布后以干净安装为基线，通过默认命令更新到
  相邻 `alpha.11`，补录 TC-WDU-04 的版本、helper 日志、进程统计及残留检查结果。
