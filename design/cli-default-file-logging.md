# CLI 默认文件日志

> 状态：实施中
> 更新时间：2026-06-08

## 背景

前台 `bifrost start` 默认把 tracing 标准日志输出到 Console Terminal。后续 Terminal 区域需要承载额外能力时，持续刷新的标准日志会污染终端协议和用户交互。

## 实现逻辑

- 全局 `--log-output` 默认值改为 `file`。
- 前台启动、无子命令默认启动和普通 CLI 命令默认都通过 `LogConfig` 写入 `<data_dir>/logs/bifrost*.log`。
- 只有用户显式传入全局 `--log-output console` 或 `--log-output console,file` 时，才额外启用 Console Terminal tracing 日志；文件日志始终保留。
- macOS 系统级 LaunchDaemon 隐藏命令 `system-proxy cleanup-daemon` 例外：plist 已把 `StandardOutPath` / `StandardErrorPath` 指向 `/var/log/bifrost-system-proxy-cleanup.*`，因此该命令默认保留 `console,file`，避免系统级守护进程日志丢失。
- stdout 协议型隐藏 worker 继续强制使用文件日志，避免 JSONL/stdio IPC 被日志污染。
- daemon 模式保持既有 `reinit_logging_for_daemon` 文件日志路径。
- 日志目录保留策略统一为默认 7 天；CLI 主进程、daemon、Tray 和 Desktop sidecar/bootstrap 都调用共享清理逻辑。
- 日志目录默认容量上限为 1GiB；即使 7 天内日志暴涨，清理逻辑也会按修改时间从旧到新删除已知 Bifrost 日志产物，直到目录总量回到上限内。
- 清理范围覆盖当前 `bifrost.YYYY-MM-DD.log`、daemon err 轮转、Tray 日志、历史 `log-YYYY-MM-DD.log`、desktop sidecar/bootstrap、guardian/restart/upgrade 日志和 `*-audit.json`；未知普通文件不纳入删除。

## 依赖项

- `crates/bifrost-cli/src/cli.rs`：全局 CLI 参数默认值和 help。
- `crates/bifrost-cli/src/main.rs`：根据 CLI 参数计算 `LogOutput`。
- `crates/bifrost-core/src/logging.rs`：`LogConfig` 默认输出。
- `crates/bifrost-cli/src/commands/tray/tray.rs`：Tray 启动时复用共享日志目录清理。
- `desktop/src-tauri/src/main.rs`：Desktop bootstrap/sidecar 打开日志前复用共享日志目录清理。

## 测试方案

- 单元测试：
  - `default_log_output_writes_file_only` 验证未传全局参数时输出为 `File`。
  - `explicit_log_output_can_enable_console` 验证 `--log-output console,file` 显式启用 console 且保留文件日志。
  - `explicit_console_log_output_keeps_file_logging` 验证 `--log-output console` 作为 stdout opt-in 简写时仍保留文件日志。
  - `launchd_cleanup_daemon_keeps_console_logs_for_standard_paths` 验证 macOS LaunchDaemon cleanup 隐藏命令默认保留 console 输出。
  - `voice_worker_forces_logs_away_from_stdout_protocol` 验证 stdout 协议型 worker 不被显式 console 参数污染。
  - `cleanup_bifrost_log_dir_removes_legacy_and_shared_dated_logs` 验证 7 天保留清理覆盖历史 `log-YYYY-MM-DD.log`、Tray 和 daemon err 轮转日志。
  - `cleanup_bifrost_log_dir_removes_old_fixed_log_artifacts_by_mtime` 验证 desktop sidecar/bootstrap、audit 等固定文件按 mtime 清理。
  - `cleanup_bifrost_log_dir_enforces_total_size_by_removing_oldest_logs` 验证 1GiB 容量上限路径按旧到新删除日志。
- E2E 测试：
  - `e2e-tests/tests/test_cli_start_log_output_default_file.sh` 启动真实前台服务，断言默认无 console tracing 日志且文件日志非空。
  - 同一脚本使用 `--log-output console` 断言显式开启后 console tracing 日志可见，且文件日志仍非空。
  - macOS 上同一脚本运行 `system-proxy cleanup-daemon`，断言 launchd stdio 路径需要的 console tracing 日志仍可见。
- 真实场景测试：
  - 更新并执行 `human_tests/cli-log-output-default.md` 中默认文件日志、显式 console opt-in、daemon 文件日志、默认 info 噪声，以及日志目录 7 天保留 + 1GiB 上限回归用例。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、`cli.rs`/`main.rs`/`logging.rs` diff、E2E 脚本与 human_tests 文档，运行 focused 单测和新增 E2E。
- 第 2 轮：复查第 1 轮修复后的最新 diff、stdout/stderr 与文件日志边界、human_tests 索引和验证命令，复跑受影响测试。

## 校验要求

- 本地默认执行不牵引 ASR 模块的 focused 验证：`cargo test -p bifrost-core --lib log_output`、`cargo fmt --all -- --check` 和 `git diff --check`。
- 本地可在已有当前二进制时执行 `SKIP_BUILD=true e2e-tests/tests/test_cli_start_log_output_default_file.sh`；需要重新构建完整 bifrost 二进制时，优先交给远端 CI 覆盖，避免本地默认拉起 ASR-heavy 构建链路。
- 远端 CI 负责补齐 `bifrost-cli` bin 级单测、workspace all-features、clippy 和完整构建验证。
- 若 local-ci 未执行，说明范围、成本和远端 CI 替代验证状态。

## 文档更新要求

- 更新 `human_tests/cli-log-output-default.md`。
- 更新 `human_tests/readme.md` 中 CLI 日志输出默认行为说明。
