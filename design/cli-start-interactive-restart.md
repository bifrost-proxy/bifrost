# CLI start：进程冲突时交互式重启

## 背景与目标

当用户执行 `bifrost start` 时，如果本机已经存在一个正在运行的 Bifrost 进程（由 `runtime.json`/`bifrost.pid` 记录且进程存活），此前行为是直接报错退出。该方案将其改为“交互式重启”：提示用户是否停止旧进程并重新启动，以降低误操作成本。

## 预期行为

当 `bifrost start` 检测到已有进程在运行：

- 终端提示：`Detected an existing Bifrost proxy process (PID: <pid>). Restart? (y/n)`。
- 读取 stdin：
  - 输入 `y` / `yes`（忽略大小写）：执行重启（停止旧进程后继续启动）。
  - 输入 `n` / `no` 或空输入：取消本次启动，打印 `Start cancelled.` 并以 exit code 0 退出。
  - stdin EOF：视为取消启动。
  - 连续 3 次非法输入：视为取消启动。
- `--yes` 旁路：当 `bifrost start --yes` 时跳过提示直接执行重启（CI / 自动化场景）。

## 实现逻辑

- `crates/bifrost-cli/src/commands/start.rs`：在 `run_start` 的最前置阶段读取 `read_pid()` 并判断 `is_process_running(pid)`。
- 若进程存活：当 `--yes` 时直接判定为重启，否则调用 `prompt_restart_if_running(pid)` 读取 stdin。
- 若用户确认重启，则复用 `stop` 的收尾逻辑：直接调用 `commands::stop::run_stop()`（包含：发送 SIGTERM/TerminateProcess、等待退出、必要时强杀、恢复/关闭 system proxy、清理 CLI proxy、删除 pid/runtime 文件）。
- 若 PID 文件存在但进程已不在：当 `system_proxy_shutdown_mode == PreserveForRestart` 时保留 runtime 文件用于 restart handoff，否则调用 `remove_pid()` 清理陈旧 PID。
- stop 成功后，先（按需）执行 `check_and_install_certificate`，再调用 `check_and_resolve_port_conflict(&host, port, yes)` 做端口冲突检查，然后才进入配置初始化、启动摘要打印和系统代理收敛规划。
- `check_and_resolve_port_conflict` 行为：端口已占用时尝试通过 `find_process_on_port` 定位占用方；交互终端下提示 `Kill it and continue? (y/n)`，`--yes` 自动确认；非交互且未传 `--yes` 时立即返回错误（消息形如 `Port <host>:<port> is already in use and no interactive terminal is available. Use --yes to auto-resolve.`）。kill 成功后短暂等待并复检端口，仍占用则返回错误。
- 端口占用判断 (`is_port_in_use`) 优先尝试绑定目标地址（`0.0.0.0` / `::` 归一到 `127.0.0.1`），仅在 `AddrInUse` 之外的错误下回退到 200ms 超时的 TCP connect 探测，避免仅靠 TCP connect 时被 backlog、未 accept 的测试监听器或防火墙行为影响；这样非交互 CI 或脚本环境中，普通端口占用会稳定返回 `Port <host>:<port> is already in use...`，且不会在实际启动失败前打印 `System proxy: enabled` 这类易误导的摘要。

## 依赖与影响面

- 复用现有 `stop` 子命令逻辑；不新增 CLI 参数，不改变非冲突场景的启动行为。
- 为保证 Windows 下 `stop`/进程检测语义正确，补齐 `is_process_running` 的 Windows 实现。
- 端口冲突提前检查不依赖配置文件或数据目录初始化，因此不会改变已运行 Bifrost PID 冲突的重启语义，也不会提前触发系统代理修改。

## 测试方案

### 单元/集成测试

- 目前该能力主要是终端交互与进程级行为，优先使用 E2E shell 测试覆盖。

### E2E 测试

新增脚本：`e2e-tests/tests/test_cli_start_interactive_restart_e2e.sh` (planned, not yet shipped as of 2026-06-16；当前仓库内尚无该脚本)

覆盖点：

- 场景 1：检测冲突 -> stdin 输入 `y` -> 旧进程退出 -> 新进程启动成功
- 场景 2：检测冲突 -> stdin 输入 `n` -> 不终止旧进程 -> 本次 start 退出
- 回归脚本：`e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh`（已落地）
  - 用 dummy listener 占用测试端口，启动 Bifrost 后必须非 0 退出。
  - 输出必须包含 `already in use`。
  - 输出不得包含 `System proxy: enabled` 或 `System proxy enabled:`，证明端口冲突时不会进入系统代理启用阶段或打印启用摘要。

### 真实场景测试（手动）

```bash
# 第一次启动（前台/后台均可）
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 18890 --skip-cert-check --unsafe-ssl

# 另一个终端再次执行 start，观察交互提示并选择 y/n
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 18890 --skip-cert-check --unsafe-ssl
```

补充回归：在端口被普通进程占用且 stdin 非交互时，执行 `bifrost start --system-proxy` 应直接失败并报告端口占用，不应打印系统代理启用摘要。

## 校验要求

- 本次改动提交前必须执行：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、以及至少一次 `cargo test --workspace --all-features`。

## 文档更新

- 更新 `docs/cli.md` 的 `start` 章节，补充“已有进程时会提示是否重启”的说明。
