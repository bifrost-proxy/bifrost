# CLI 升级进度流式展示真实场景测试

## 功能模块说明

验证 `bifrost upgrade` 在 CLI 与 Desktop 两个更新阶段都持续展示下载、安装和重启进度，
避免长时间没有终端输出。测试同时确认父升级进程会在 Desktop 子安装仍运行时实时转发其
stdout/stderr，而不是等子进程退出后一次性显示。

所有用例使用本地 HTTP fixture、临时 `.app`、临时安装目录和当前源码二进制；不修改
`/Applications/Bifrost.app`，不启动或停止 9900 代理，不修改系统代理。

## 前置条件

1. 当前目录为 Bifrost 仓库根目录。
2. 当前平台为 macOS；非 macOS 只执行 TC-CUP-01，Desktop 用例由对应平台 CI 兜底。
3. 构建当前源码二进制：

   ```bash
   cargo build --bin bifrost
   export BIFROST_BIN="$PWD/target/debug/bifrost"
   ```

## 测试用例列表

### TC-CUP-01 CLI 下载显示百分比、字节和速度

操作步骤：

1. 执行本地 TCP fixture 下载回归并保留控制台输出：

   ```bash
   cargo test -p bifrost-cli \
     commands::upgrade::tests::download_selection_success_and_free_restart_port_are_exercised \
     --lib -- --exact --nocapture
   ```

2. 观察输出中的 `Downloading…` 行。

预期结果：

- 本地 fixture 下载成功，测试返回 `ok`。
- `Downloading…` 行包含百分比、已下载/总字节和 `/s` 速度，例如
  `100.0% (7 B/7 B, .../s)`。
- 不访问真实 release，不替换任何已安装 CLI。

### TC-CUP-02 Desktop 子安装进度在父 upgrade 退出前可见

操作步骤：

1. 执行：

   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_cli.sh --only-progress-streaming
   ```

2. 测试会创建旧版本临时 `Bifrost.app`，并用新版临时 App + 512 KiB 随机 payload 构建
   真实 DMG；本地 HTTP 服务按 4 KiB 分块并每块等待 20ms 后发送该 DMG。
3. PATH-scoped `ditto` 包装器先输出 `BIFROST_TEST_INSTALLER_PROGRESS 10%`，等待 2 秒，
   再执行系统 `ditto`。
4. 测试分别在 HTTP 响应尚未发送完成、Desktop 子安装尚未退出时轮询父日志，最后读取
   临时安装目标的 `Info.plist` 验证版本替换。

预期结果：

- HTTP 响应尚未发送完成时，父日志已包含带百分比、字节和 `/s` 速度的
  `Downloading…` 行。
- 父进程仍运行时，父日志已经包含 `BIFROST_TEST_INSTALLER_PROGRESS 10%`。
- 完整输出包含 `Installing desktop app...`、`Desktop app installed successfully` 和
  `Bifrost desktop app updated successfully`。
- 临时安装目标的 `CFBundleShortVersionString` 从 `0.0.1` 变为当前 CLI 版本。
- E2E 汇总为 1 passed、0 failed、0 skipped。

### TC-CUP-03 子进程错误实时可见且失败摘要仍保留 stderr

操作步骤：

1. 执行升级模块测试并保留控制台输出：

   ```bash
   cargo test -p bifrost-cli \
     commands::upgrade::tests::upgrade_behavior_executes_companion_and_runtime_ownership_branches \
     --lib -- --exact --nocapture
   ```

2. 观察模拟 Desktop 子命令输出的 `app-failed` 以及父命令 warning。

预期结果：

- `app-failed` 在控制台可见。
- 父 warning 包含 `desktop app update command failed: app-failed`，证明流式转发没有丢失
  用于错误摘要的 stderr。
- 测试返回 `ok`，且 best-effort CLI 主流程语义保持不变。

## 清理步骤

1. E2E 脚本通过 trap/显式清理移除临时 app、package、包装器和日志目录。
2. 确认没有测试代理进程：

   ```bash
   pgrep -fal "$PWD/target/debug/bifrost.*start" || true
   ```

3. 不删除 `target/` 编译缓存；它会供后续项目验证复用。

## 最近执行记录

| 日期 | 用例 | 结果 | 实际观察 |
| --- | --- | --- | --- |
| 2026-07-22 | TC-CUP-01 | PASS | 本地 7-byte HTTP fixture 输出 `Downloading… 100.0% (7 B/7 B, 6.8 KiB/s)`，测试 1 passed；未访问 release、未替换 CLI。 |
| 2026-07-22 | TC-CUP-02 | PASS | 本地 HTTP 分块下载真实 528.5 KiB DMG；响应未结束时父终端已显示下载进度，最终为 `Downloading… 100.0% (528.5 KiB/528.5 KiB, 145.7 KiB/s)`；安装未结束时已显示 installer marker，临时 App 从 0.0.1 更新到 0.0.161；E2E 汇总 1 passed、0 failed、0 skipped。 |
| 2026-07-22 | TC-CUP-03 | PASS | 控制台实时出现 `app-failed`，父 warning 保留 `desktop app update command failed: app-failed`；目标测试 1 passed。 |
