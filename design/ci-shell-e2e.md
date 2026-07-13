# CI Shell E2E 稳定化设计方案

## 背景

Bifrost 的 shell E2E 通过 `scripts/ci/run-e2e-shell.sh` 调用 `scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build`，由 `BIFROST_E2E_SHARD_INDEX` / `BIFROST_E2E_SHARD_TOTAL` 在 CI runner 之间横向分片。三平台分布：

- Linux `E2E Shell (Linux)`：`ubuntu-latest`，单 shard，`BIFROST_E2E_SHELL_JOBS: "4"`，`timeout-minutes: 60`。
- macOS `E2E Shell (aarch64-apple-darwin, shard N/2)`：`macos-15`，`shard: [1, 2]`，`BIFROST_E2E_SHELL_JOBS: "2"`，`timeout-minutes: 60`。
- Windows `E2E Shell (x86_64-pc-windows-msvc)`：`windows-latest`，`timeout-minutes: 30`，只跑 `test_upgrade_local_restart_e2e.sh`（Windows self-update replacement 专项）。

历史红灯来源覆盖多类问题：系统代理设置在 hosted runner 上不稳定；ASR / voice runtime 用例依赖本地模型；`echo | grep -q` 在 `pipefail` 下 Broken pipe；unsafe_ssl 用例复用错误 HTTPS mock 端口；`test_temporary_port_bindings.sh` 端口抢占；shell shard 内部并发过高触发 OOM kill；Cargo artifact lock 竞争让 heavy 用例超时；`heartbeat` 每秒 fork 让 Windows MSYS 出现 `fork: Resource temporarily unavailable`；HTTP3/Replay 用例依赖公网 `httpbin.org` 偶发 503。

本设计把这些点整体收敛为一套稳定化规则：CI-only skip list、串行 heavy 用例、隐藏日志目录 artifact 上传、失败原因抓取、并行度分层、Windows rules 内层预算。

## Go 工具链移除

- 仓库不再跟踪 `.go`、`go.mod`、`go.sum`、`go.work` 或 Go 编译产物。历史
  `e2e-tests/tests/quic_socks5_client/` 没有入口脚本、CI 调用或断言，是未接入测试体系的
  孤立实验代码，删除它不会减少实际执行的 E2E 场景。
- HTTP/3 能力继续由 Rust integration test
  `crates/bifrost-proxy/tests/upstream_http3_e2e.rs` 验证真实本地 QUIC/H3 origin；SOCKS5
  UDP ASSOCIATE、UDP 转发与 QUIC-like 数据包继续由 Rust 单测和现有 Shell E2E 验证。
- 删除未接入任何测试入口、且未实现真实 QUIC-over-SOCKS5 transport 的历史
  `quic_socks5_test.py`。这条能力只保留可执行、可断言的 Rust 与 Shell 证据，避免把演示脚本
  误认为有效回归用例。
- CI 不再使用 `actions/setup-go`。`shfmt` 从官方 v3.12.0 release 下载 Linux amd64
  预编译二进制，并在安装前校验固定 SHA-256，避免为了 Shell 格式检查引入 Go 工具链，
  同时避免未经校验的可执行文件进入 runner。
- `test_coverage_pipeline_contract.sh` 同时门禁“无 tracked Go 文件”“旧客户端目录无任何
  tracked artifact”“旧 Python 演示客户端不回流”“无 Go setup/install”和 `shfmt` 版本/哈希，
  防止后续无效依赖或伪测试悄悄回流。

## 用户目标验证清单

### 必须实现

**Skip 列表 & CI 模式**

- `scripts/run_all_e2e.sh` 维护 `SKIP_IN_CI_TESTS`，`collect_shell_tests` 在 `MODE=ci` 时先过滤再分片，避免被跳过用例占用 shard 槽位。
- `SKIP_IN_CI_TESTS` 至少包含：
  - `test_system_proxy_e2e.sh`（修改宿主机代理，hosted runner 不稳）。
  - ASR/voice runtime 类：`test_asr_*.sh`、`test_qwen3_asr_*.sh`、`test_voice_input_runtime.sh`、`test_voice_wake_actions.sh`。
  - 重型低频专项：`test_asr_admin_csrf.sh`（~583s wall time）、`test_chatgpt_web_shared_profile.sh`（~879s）、`test_security_hardening.sh`（聚合 wrapper）。
- 本地 `bash scripts/run_all_e2e.sh --full-shell ...`（`MODE=local`）不应用 skip 列表，可手动跑系统代理与 ASR 用例。
- `--list-shell-tests` 只打印当前 mode/shard 下会被收集的 shell 脚本并退出，不启动 Bifrost。

**日志与失败诊断**

- GitHub Actions E2E 日志路径使用 `.e2e-reports/` 与 `.bifrost-e2e-ci/` 两个隐藏目录，所有 `Upload E2E logs` step 必须设 `include-hidden-files: true`。
- `Dump failed suite logs` 步骤内的 `find ... | head` 管道对 SIGPIPE 容错（`|| true`），不能因 head 早退让 pipefail 传播为 step 失败。
- `scripts/run_all_e2e.sh` 的失败原因抽取优先匹配真实断言 / Playwright/JS 错误 / panic；cleanup 尾巴（如 `Preserving failed test root`）只当日志上下文，不做最终原因。

**并行度与超时预算**

- Linux shell shard `BIFROST_E2E_SHELL_JOBS: "4"`；macOS shard `BIFROST_E2E_SHELL_JOBS: "2"`。历史 8 路 / 16 路会触发 hosted runner OOM，让 Bifrost 子进程被系统 `Killed`。
- Linux 与 macOS `E2E Shell` job `timeout-minutes: 60`，与 rules/runner 对齐。
- Workflow 顶层 `concurrency: { group: ${{ github.workflow }}-${{ github.ref }}, cancel-in-progress: true }`，避免旧 push 长尾阻塞新 commit。

**Cargo-heavy 用例串行**

- `scripts/run_all_e2e.sh` 定义 `CARGO_HEAVY_TESTS`，包含 `test_chatgpt_web_behavior_artifacts.sh`、`test_asr_task_pause_resume.sh`、`test_voice_input_runtime.sh` 等触发 `cargo check/test/run` 的用例。
- `run_shell_tests_parallel` 把 `is_cargo_heavy` 用例加入 `serial_tests`，串行执行，避免 Cargo artifact lock 竞争让业务已通过但被 900s per-test timeout 杀掉。

**macOS 双分片负载均衡**

- macOS `E2E Shell (aarch64-apple-darwin, shard N/2)` 使用 `shell_test_weight` 的实测秒级权重分片。权重来自近期 GitHub Actions job 日志里的 `[PASS] shell:<script> (<seconds>s)` 记录，而不是脚本数量。
- shard 内执行模型必须纳入分片计算：safe shell tests 先按 `BIFROST_E2E_SHELL_JOBS=2` 并发执行，lock-sensitive / cargo-heavy tests 再串行执行。分片算法按“串行预计耗时 + 并发 lane 最长预计耗时”估算墙钟，避免一个大的并发脚本抵消另一个 shard 的串行长尾。
- 2026-07-07 复核的异常样本包含 `CI` run `28803571034`（shard 1: 19.85 min，shard 2: 37.70 min）和 `28778932181`（shard 1: 22.85 min，shard 2: 32.18 min）。主要长尾来自 `test_chatgpt_web_behavior_artifacts.sh`、`test_im_gateway_long_reply_delivery_regression.sh`，以及此前按默认 8s 估计但实际可达数百秒的 `test_desktop_open_requests_contract.sh`、`test_skill_creator_flow.sh`。
- `CI` run `28881027276` 通过后，实测 shard 1 为 1168s、shard 2 为 1847s；该结果证明单纯平衡总权重仍会被串行段拖长。后续权重与验收口径改为 estimated wall clock。
- `scripts/run_all_e2e.sh --check-shell-shard-balance` 会打印每个 shard 的 `estimated_wall`、串行耗时、并发 lane 耗时和测试数量，并在最大/最小预计墙钟差超过平均预计墙钟的 `BIFROST_E2E_SHARD_BALANCE_MAX_DIFF_PCT` 时返回非 0。默认门槛为 20%，对应用户目标的“两边耗时误差不超过 20%”。
- 调整权重后必须运行：
  `BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/2 --check-shell-shard-balance`。
  当前期望输出为两个 shard `estimated_wall` 差距低于 20%，且不减少任一 shell 测试覆盖。

**外部依赖本地化**

- Shell E2E 不依赖公网 `httpbin.org` / `echo.websocket.events`。`test_http3_e2e.sh` 把 `bifrost-httpbin.test`、`h3-forward-test.local`、`h3-body-test.local`、`h3-websocket-test.local` 转发到 `e2e-tests/mock_servers/http_echo_server.py` 分配的本地端口；`test_replay_body_decode.sh` 直接 replay 本地 `/gzip`。
- `test_unsafe_ssl_e2e.sh` 在当前 `HTTPS_MOCK_PORT` 不可用或被非 mock HTTPS 服务占用时，自行启动 `e2e-tests/mock_servers/https_echo_server.py` 并挑新端口；通过 `ADMIN_CLIENT_START_UNSAFE_SSL=0` 让 helper 以安全默认启动。

**端口 & binary 复用**

- `admin_client.sh` 复用已有管理端前先请求 `/api/auth/status` 并校验响应是 Bifrost 鉴权 JSON，避免端口碰撞误连本机其它服务。
- `test_temporary_port_bindings.sh` 对 `port bind --port` 做有限重试；`another process is already listening` 时重新分配端口。
- `e2e-tests/test_utils/process.sh` Bifrost 清理优先 `SIGINT` + `wait`，仅当端口迟迟不释放才 force kill，避免 `Killed ...` 让 Linux shard 把 PASS 用例误判失败。
- `test_remote_relay_url_fallback_e2e.sh` 在 `SKIP_BUILD=true` 且已有 `BIFROST_BIN` 时输出 `Using existing bifrost binary`，不再无条件 `cargo build`。
- `scripts/run_all_e2e.sh` 的 `CARGO_BIN` 默认从当前 `PATH` 解析（`resolve_cargo_command`），不再硬编码 `$HOME/.cargo/bin/cargo`。
- 顶层入口默认注入预构建 release `BIFROST_BIN`，但保留外部覆盖；`test_chatgpt_web_startup_auth_preflight.sh` 等 startup 类脚本必须尊重 `SKIP_BUILD=true`。

**Windows rules 内层预算**

- Windows rules E2E 在 MSYS bash 下频繁 fork `sleep`/`grep`/`date` 会触发 `fork: Resource temporarily unavailable`。`scripts/run_all_e2e.sh` 的 heartbeat 通过 `BIFROST_E2E_HEARTBEAT_INTERVAL` 控制，默认 30 秒；`e2e-tests/run_all_tests_parallel.sh` 在 Windows 下把主循环轮询间隔提高到 1 秒（`BIFROST_E2E_WINDOWS_POLL_INTERVAL:-1`），优先用 `result_*.txt` 中的 `STATUS=` 判断 suite 完成。
- 主循环发现 result file 已写但 cleanup 子 shell 仍存活时，使用 `wait_for_pid_exit_bounded` 做有界等待，超时后回收该子树继续下一个 fixture，避免卡在 `request_modify/forwarded_for.txt` 之后。
- Windows rules shard `BIFROST_E2E_RULE_RUNNER_TIMEOUT: "1200"`（20 分钟内层预算，主动写失败结果并退出），外层 `BIFROST_E2E_SUITE_TIMEOUT: "1260"` 兜底，留出 cleanup / dump failed suite logs / artifact upload 空间。

**Shell 断言细节**

- `test_cli_offline_commands_e2e.sh` 的 help 断言使用扩展正则（`grep -E` / 多个 `-e`），禁止 BRE 下 `\|`。
- `test_cli_offline_commands_e2e.sh` 不使用 `echo "$result" | grep -q`；`pipefail` 下 grep 早退让 echo 收到 SIGPIPE 误判失败。改用 `grep ... <<<"$result"`。
- `run_shell_tests_parallel` / `run_shell_batch_parallel` 末尾必须显式 `return 0`；bash 函数默认返回最后一条命令状态，`set -e` 下会把最后一轮 `[[ $running -gt 0 ]]` 为 false 传播为 suite 失败。
- `test_replay_rules.sh` SSE replay 使用 5s `timeout_ms`，断言收到 `id>=12` 的超时边界后事件，允许 hosted runner 在边界后提前关闭客户端连接。
- `test_large_body_protection.sh`（35MB 请求 + 100MB 响应）作为 resource-heavy shell test 串行执行，并复用调度器注入的 `BIFROST_DATA_DIR`。

### 必须不破坏

- 本地 `--full-shell` 仍能收集 `test_system_proxy_e2e.sh`、ASR / voice runtime、安全聚合 wrapper 等全部脚本。
- rules / runner / ui / build 的调度不受本设计影响；`--skip-rules --skip-runner --skip-ui --skip-build` 只在 CI shell 入口使用。
- 单脚本本地默认端口（如 `18897/18898`）仍可用，调度器只在存在 `ADMIN_PORT` / `MOCK_HTTP_PORT` / `PROXY_PORT` 环境时注入。
- 已有 shell test 依赖的 `e2e-tests/test_utils/*.sh`（`admin_client.sh`、`process.sh`、`sync_server.sh` 等）接口保持向后兼容。

### 必须真实验证

- 三平台 `E2E Shell` job 在 GitHub Actions 全绿；单 shard PASS 后 job 收尾不被 30 分钟 timeout 拉红。
- 静态断言：`grep -c 'include-hidden-files: true' .github/workflows/ci.yml` 覆盖所有 E2E artifact upload。
- 静态断言：`BIFROST_E2E_SHELL_JOBS: "4"`（Linux）、`"2"`（macOS）、`BIFROST_E2E_RULE_RUNNER_TIMEOUT: "1200"` / `BIFROST_E2E_SUITE_TIMEOUT: "1260"`（Windows rules）。
- 用 `SKIP_BUILD=true BIFROST_BIN=<现成 release> bash e2e-tests/tests/<各脚本>` 真跑代表性用例，确认端口注入、release binary 复用与 mock 本地化都生效。

## 产品语义

本设计只影响 CI 与本地 E2E 调度，不改变 Bifrost 运行时行为。

- 本地开发者：`--full-shell` 行为不变；`--list-shell-tests` 可查看具体分片内容。
- CI 维护者：新用例默认按并行调度；若触发 Cargo/资源竞争，登记到 `CARGO_HEAVY_TESTS` 或 resource-heavy 名单进入 serial 队列。

## 技术细节

### workflow (`.github/workflows/ci.yml`)

- Linux `e2e-shell`（L249–324）：`needs: [build-e2e]`；下载 `bifrost-release-linux`；跑 `bash scripts/ci/run-e2e-shell.sh`；`Dump failed suite logs` + `Upload E2E logs` 带 `include-hidden-files: true`。
- macOS `e2e-macos-shell`（L707–790）：matrix `shard: [1, 2]`；下载 `bifrost-release-aarch64-apple-darwin`；跑 `bash scripts/ci/run-e2e-shell.sh`。
- Windows `e2e-windows-shell`（L1098–1148）：`timeout-minutes: 30`；仅跑 `bash e2e-tests/tests/test_upgrade_local_restart_e2e.sh`，其余 Windows shell 用例走 rules matrix 或跳过。
- Workflow 顶层 `concurrency`（L35–37）：`group: ${{ github.workflow }}-${{ github.ref }}` + `cancel-in-progress: true`。

### `scripts/ci/run-e2e-shell.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

SHARD_ARGS=""
if [[ -n "${BIFROST_E2E_SHARD_INDEX:-}" && -n "${BIFROST_E2E_SHARD_TOTAL:-}" ]]; then
  SHARD_ARGS="--shard ${BIFROST_E2E_SHARD_INDEX}/${BIFROST_E2E_SHARD_TOTAL}"
fi

# shellcheck disable=SC2086
bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build $SHARD_ARGS
```

### `scripts/run_all_e2e.sh` 关键点

- `collect_shell_tests`：`MODE=ci` 时先按 `SKIP_IN_CI_TESTS` 过滤，再按 `--shard` 应用分片。
- `resolve_cargo_command`：按当前 `PATH` 解析非 shim `cargo`；调用方可用 `CARGO_BIN=...` 覆盖。
- `run_shell_tests_parallel` / `run_shell_batch_parallel`：末尾 `return 0`；serial 队列覆盖 `CARGO_HEAVY_TESTS` 与 resource-heavy 名单。
- `heartbeat_while_running`：`BIFROST_E2E_HEARTBEAT_INTERVAL` 控制间隔，默认 30 秒。
- 顶层 final status 检查：无失败 suite 时显式 `exit 0`（计划中，2026-06-16 起标注）。

### `e2e-tests/run_all_tests_parallel.sh` Windows 优化

- 纯 bash `result_has_status`：判断 `result_*.txt` 是否已写 `STATUS=`；主循环优先按此回收 suite。
- `loop_sleep`：`BIFROST_E2E_WINDOWS_POLL_INTERVAL:-1` 秒。
- cleanup 卡住的子 shell 用 `wait_for_pid_exit_bounded` 有界等待，超时强制回收。
- `BIFROST_E2E_RULE_RUNNER_TIMEOUT` 到点时写入未完成 fixture 的失败结果后退出。

## CLI / Web / Admin API / Sync 边界

- 无 CLI 命令新增。
- 无 Web / Admin API 变化。
- Sync：本设计不影响 sync API；`start_e2e_sync_server` 由 runner E2E 使用（见 `run-e2e-runner.sh`），shell E2E 不启动 sync server。

## 实现切分

### Phase 1 — Skip 列表与 CI 模式

- `SKIP_IN_CI_TESTS` 加入 system_proxy / ASR / voice / 重型低频 wrapper。
- `collect_shell_tests` 先过滤再分片。
- `--list-shell-tests` 只列不跑。

### Phase 2 — 日志 & 失败诊断

- 全部 E2E upload step 加 `include-hidden-files: true`。
- `Dump failed suite logs` 里 `find | head` 加 `|| true`。
- 失败原因抽取优先真实错误。

### Phase 3 — 并行度与超时

- Linux `BIFROST_E2E_SHELL_JOBS: "4"`，macOS `"2"`。
- 三平台 shell job `timeout-minutes: 60`（Windows shell 特例 30，只跑 upgrade restart）。
- `concurrency` + `cancel-in-progress: true`。

### Phase 4 — Cargo-heavy 串行与端口/binary 复用

- `CARGO_HEAVY_TESTS` 登记 heavy 用例。
- 修 `test_cli_offline_commands_e2e.sh` 的正则与 `echo | grep -q`。
- 修 `test_unsafe_ssl_e2e.sh` 自启 mock。
- 修 `admin_client.sh` `/api/auth/status` 检查。
- 修 `test_temporary_port_bindings.sh` 端口重试。
- 修 `process.sh` 清理策略。
- Agent/IM human-api 脚本消费调度器端口 + release binary 复用。

### Phase 5 — 外部依赖本地化与 Windows rules 内层预算

- HTTP3 / Replay 用例改本地 mock。
- Windows rules `BIFROST_E2E_RULE_RUNNER_TIMEOUT: "1200"` / `BIFROST_E2E_SUITE_TIMEOUT: "1260"`。
- `run_all_tests_parallel.sh` Windows 轮询 + result_has_status 回收。

## 测试方案

### 单元测试

Bash 调度逻辑，无 Rust 公共函数变更。

### 集成 & E2E 测试

- `bash scripts/run_all_e2e.sh --ci --full-shell --list-shell-tests --shard 2/2`：断言输出不含 `test_system_proxy_e2e.sh`。
- `bash scripts/run_all_e2e.sh --full-shell --list-shell-tests`：断言本地仍包含 `test_system_proxy_e2e.sh`。
- `BIFROST_E2E_SHARD_INDEX=2 BIFROST_E2E_SHARD_TOTAL=2 BIFROST_E2E_SHELL_JOBS=2 bash scripts/ci/run-e2e-shell.sh`：覆盖 macOS shard 2。
- 静态：`grep -c 'include-hidden-files: true' .github/workflows/ci.yml`；`BIFROST_E2E_SHELL_JOBS: "4"` / `"2"` 匹配；`BIFROST_E2E_RULE_RUNNER_TIMEOUT: "1200"` / `BIFROST_E2E_SUITE_TIMEOUT: "1260"` 匹配。
- `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`：`105/105` PASS。
- `rg -n 'echo "\$[A-Za-z_][A-Za-z0-9_]*" \| grep -[A-Za-z]+' e2e-tests/tests/test_cli_offline_commands_e2e.sh`：无匹配。
- `HTTPS_MOCK_PORT=<free> PROXY_PORT=<free> ADMIN_PORT=<same> BIFROST_DATA_DIR=<tmp> bash e2e-tests/tests/test_unsafe_ssl_e2e.sh`：5/5 用例通过；被占端口场景 alternate。
- `SKIP_BUILD=true bash e2e-tests/tests/test_temporary_port_bindings.sh`：55 个 temporary port 用例全过。
- `PROXY_PORT=<free> MOCK_HTTP_PORT=<free> MOCK_SSE_PORT=<free> MOCK_WS_PORT=<free> BIFROST_DATA_DIR=<tmp> SERVER_LOG_DIR=<tmp> SKIP_BUILD=true bash e2e-tests/tests/test_replay_rules.sh`：`SSE Replay with Rules` 收到 `id>=12` 的 post-timeout 事件。
- `SKIP_BUILD=true BIFROST_BIN=<release> bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`：输出 `Using existing bifrost binary`，三段 relay fallback 全过。
- `BIFROST_BIN=<release> SKIP_BUILD=true SKIP_CARGO_TEST=true PROXY_PORT=<free> ECHO_HTTP_PORT=<free> ECHO_HTTPS_PORT=<free> bash e2e-tests/tests/test_http3_e2e.sh`：全部命中本地 mock，`Failed: 0`。
- Linux 与 macOS Shell capability matrix 显式设置 `SKIP_CARGO_TEST=true`：HTTP/3 Rust integration test 由 Unit/Integration 与 Coverage 两个独立 job 双重执行；Shell 保留全部真实代理场景，避免冷缓存下 test-only 依赖编译超过 job 预算。
- Layered Coverage 的 Shell 阶段同样设置 `SKIP_CARGO_TEST=true`：前置 Unit/Integration coverage 已执行并采集 HTTP/3 integration test，后续 Shell 只运行可贡献 E2E profile 的真实代理场景，禁止额外构建未插桩 release test。
- Layered Coverage 在插桩构建前生成 Web 资产，并把历史 Shell 用例使用的 `target/release/{bifrost,bifrost-e2e}` 兼容路径链接到同一份 debug 插桩二进制；既保证管理端资源与旧用例路径可用，也不混入未插桩进程。
- PR 不再触发完整 Layered Coverage；主 Coverage 使用 `--e2e-suite proxy`，通过 `BIFROST_E2E_SHELL_TESTS` 精确选择 13 个 SOCKS/CONNECT/HTTP/WebSocket 核心场景并合并 Rules、Runner profile。完整 167 个 Shell 的分层报告只在每周和手动审计生成。
- `BIFROST_BIN=<release> SKIP_BUILD=true PROXY_PORT=<free> MOCK_HTTP_PORT=<free> BIFROST_DATA_DIR=<tmp> SERVER_LOG_DIR=<tmp> bash e2e-tests/tests/test_replay_body_decode.sh`：本地 `/gzip` 返回 200 + `"gzipped": true`。
- 静态：`scripts/run_all_e2e.sh` 的 `CARGO_BIN` 默认来自 `resolve_cargo_command`；`heartbeat_while_running` 用 `BIFROST_E2E_HEARTBEAT_INTERVAL`。
- 静态：`e2e-tests/run_all_tests_parallel.sh` 存在 `result_has_status`，Windows 下 `loop_sleep` 默认 `BIFROST_E2E_WINDOWS_POLL_INTERVAL:-1`。

### human_tests

- 更新 `human_tests/ci-shell-e2e-sharding.md` / `human_tests/rules-e2e-fixtures.md`，覆盖：
  1. CI 不跑系统代理 / ASR / voice / 重型 wrapper。
  2. 隐藏日志 artifact 上传。
  3. 失败摘要抽取。
  4. CLI offline help alternation + Broken pipe 回归。
  5. Dump failed suite logs `find | head` pipefail 容错。
  6. SSE replay timeout 边界。
  7. unsafe_ssl 端口碰撞 alternate。
  8. long-term memory frontend build 竞争。
  9. Agent/IM human-api 并行端口 + release binary 复用。
  10. Agent chat history mock 动态端口回传。
  11. Remote relay fallback 跳过重复 build。
  12. Linux/macOS shell timeout 60 分钟对齐。
  13. main push CI concurrency 取消旧 run。
  14. Linux/macOS shell shard 内部并发 4 / 2。
  15. Cargo 默认解析。
  16. HTTP3/Replay 外部 httpbin 本地 mock 化。
  17. Windows rules MSYS fork 压力 + 20 分钟内层预算。
  18. Windows rules result-file 后 cleanup 卡住回归。
  19. Large body resource-heavy 串行。
  20. 顶层 shell E2E 全 PASS 退出码。
- 新增 Cargo-heavy 串行调度回归用例。
- 更新 `human_tests/readme.md` 索引行。

## Review / Fix / Test 闭环

1. 第 1 轮：diff `scripts/run_all_e2e.sh` / `.github/workflows/ci.yml` / 相关 shell 用例；静态断言；针对性跑代表性脚本。
2. 第 2 轮：PR 推送后观察三平台 shell shard；若某用例仍抖，加入 skip 列表或 CARGO_HEAVY / resource-heavy 名单。
3. 第 3 轮：CI 上串跑 macOS shard 2 与 Linux shard；确认 60 分钟内完成 + 全 PASS。

## 风险与决策

- 决策：把 ASR / voice runtime 与安全聚合 wrapper 从默认 PR CI 移出，保留 release-gate 与本地 full-shell 验证——PR CI 优先"每次都跑得完"。
- 决策：Linux 不再横向分 shard，改为单 shard 内部 4 并发；`CI run 25469654203` / `25470391707` 的 shard 2/3 显示 shard 数越多反而放大 OOM。
- 决策：Windows shell 只跑 upgrade restart，其它 Windows shell 用例交给 rules matrix；Windows MSYS 上完整 shell 集合稳定性低。
- 风险：新加 shell 用例若忘记登记 `CARGO_HEAVY_TESTS` / `SKIP_IN_CI_TESTS`，会把 shard 拖到 900s per-test timeout。缓解：`--list-shell-tests` 可在合入前 dry-run 分片。

## 依赖项

- `scripts/run_all_e2e.sh`
- `scripts/ci/run-e2e-shell.sh`
- `e2e-tests/run_all_tests_parallel.sh`
- `e2e-tests/tests/test_system_proxy_e2e.sh`
- `e2e-tests/tests/test_unsafe_ssl_e2e.sh`
- `e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `e2e-tests/tests/test_http3_e2e.sh`
- `e2e-tests/tests/test_replay_body_decode.sh`
- `e2e-tests/tests/test_replay_rules.sh`
- `e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh`
- `e2e-tests/tests/test_cli_offline_commands_e2e.sh`
- `e2e-tests/tests/test_large_body_protection.sh`
- `e2e-tests/tests/test_upgrade_local_restart_e2e.sh`
- `e2e-tests/tests/test_temporary_port_bindings.sh`
- `e2e-tests/tests/test_rule_match_logging_noise.sh`
- `e2e-tests/test_utils/admin_client.sh`
- `e2e-tests/test_utils/process.sh`
- `e2e-tests/mock_servers/http_echo_server.py`
- `e2e-tests/mock_servers/https_echo_server.py`
- `.github/workflows/ci.yml`

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 静态断言 workflow 中并发度 / timeout / include-hidden-files / concurrency。

## 文档更新要求

- 更新 `human_tests/ci-shell-e2e-sharding.md`
- 更新 `human_tests/rules-e2e-fixtures.md`（Windows rules 部分）
- 更新 `human_tests/readme.md`

## CI shell dynamic ports 附录

- `test_rule_match_logging_noise.sh` 不能硬编码 `18887` / `18888`：Linux/macOS shell shard 里它与其它 shell 用例共享 hosted runner，两个 info/debug Bifrost 实例必须用 `e2e-tests/test_utils/process.sh` 的 `allocate_free_port` 挑端口，同时保留 `INFO_PORT` / `DEBUG_PORT` 覆盖用于本地复现。
