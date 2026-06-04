# CLI 进程状态检测

> 状态：实施中
> 更新时间：2026-06-04

## 背景

`bifrost stop`、`restart`、`status` 等 CLI 路径都依赖 `is_process_running(pid)` 判断 daemon 是否仍存活。Unix 上 `kill(pid, 0)` 只能证明 PID 存在且有权限访问，不能区分真正运行中的进程和已经退出但尚未被父进程回收的 zombie。

Linux 路径已有 `/proc/<pid>/stat` zombie 识别；macOS 等非 Linux Unix 没有 `/proc`，本地 `cargo test --workspace --all-features` 在 daemon shutdown 压力场景下会把已经收到 SIGTERM 并退出的 daemon 误判为仍在运行，导致 `stop` 等到超时后升级为 `SIGKILL`。

## 实现逻辑

- Unix 仍先使用 `kill(pid, None)` 判断 PID 是否存在。
- Linux 继续通过 `/proc/<pid>/stat` 排除 `Z` 状态。
- 非 Linux Unix 通过 `ps -o stat= -p <pid>` 读取状态；当状态以 `Z` 开头时视为已退出。
- 如果 `ps` 不可用或返回异常，保持既有保守行为，避免误删仍在运行的 PID。

## 测试方案

- 单元/集成测试：执行 `cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture`，验证 daemon 模式 `stop` 不再误升级到 `SIGKILL`。
- 工作区测试：执行 `cargo test --workspace --all-features`，覆盖 stop/restart/status 共享进程状态判断。
- 真实场景测试：更新并执行 `human_tests/cli-start-stop-status.md` 的 daemon 优雅停止回归用例，确认临时数据目录下启动/停止不会修改系统代理。

## Review/Fix/Test 闭环

- 第 1 轮：复核 `is_process_running` 与 `stop` 等待逻辑，运行 daemon shutdown focused test。
- 第 2 轮：复查 diff、human_tests 索引和全量 workspace test，确认未引入 Linux/Windows 路径回归。
