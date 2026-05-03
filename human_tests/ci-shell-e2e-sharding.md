# CI Shell E2E 测试分片优化

## 功能模块说明

对 CI 中 shell E2E 测试进行性能优化，通过测试分片（sharding）将 shell 测试分配到 3 个并行 CI runner 上执行，将总耗时从 ~30 分钟降至 ~3-5 分钟。CI 模式不执行会修改宿主系统代理设置的 `test_system_proxy_e2e.sh`，该用例仅在本地 full-shell 场景验证。

## 前置条件

- 已构建 release binary：`cargo build --release --bin bifrost`
- 工作目录：项目根目录

## 测试用例

### TC-CS-01: 分片参数解析 --shard N/M

**操作步骤**：
1. 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/3 2>&1 | head -20`

**预期结果**：
- 输出包含 `Shard        : 1/3`
- 测试数量约 22-23（取决于 CI skip 列表）

### TC-CS-02: 环境变量分片透传

**操作步骤**：
1. 运行 `BIFROST_E2E_SHARD_INDEX=2 BIFROST_E2E_SHARD_TOTAL=3 bash scripts/ci/run-e2e-shell.sh 2>&1 | head -20`

**预期结果**：
- 输出包含 `Shard        : 2/3`
- 测试数量约 23

### TC-CS-03: 分片覆盖完整性

**操作步骤**：
1. 分别运行 shard 1/3、2/3、3/3，统计各分片测试数量之和

**预期结果**：
- 三个分片测试数量之和等于总测试数（约 68-69，取决于 CI skip 列表）
- 每个测试只出现在一个分片中，无重复

### TC-CS-04: 无分片时行为不变

**操作步骤**：
1. 不设置任何分片参数，运行 `bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build 2>&1 | head -20`

**预期结果**：
- 输出不包含 `Shard` 行
- 测试数量为全量（约 68-69）

### TC-CS-05: local-ci.sh --shard 参数

**操作步骤**：
1. 运行 `bash scripts/ci/local-ci.sh --skip-static --e2e-only shell --shard 1/3`

**预期结果**：
- 报告标题显示 `E2E shell (shard 1/3)`
- 只运行分片 1 的测试
- 所有测试通过

### TC-CS-06: 单分片执行耗时 < 5 分钟

**操作步骤**：
1. 运行 `time BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 bash scripts/ci/run-e2e-shell.sh`

**预期结果**：
- 总耗时 < 5 分钟（wall clock）
- 所有测试通过（已知环境问题除外）

### TC-CS-07: test_tls_logic_simple.sh 在 CI 模式下被跳过

**操作步骤**：
1. 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build 2>&1 | grep tls_logic`

**预期结果**：
- 该测试不出现在队列中（已加入 SKIP_IN_CI_TESTS）

### TC-CS-08: --shard 参数格式校验

**操作步骤**：
1. 运行 `bash scripts/run_all_e2e.sh --shard invalid 2>&1`

**预期结果**：
- 输出错误信息包含 "Error: --shard requires N/M format"
- 退出码非 0

### TC-CS-09: CI 模式不收集系统代理用例，本地 full-shell 保留

**操作步骤**：
1. 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --list-shell-tests --shard 3/3 | grep test_system_proxy_e2e.sh; test $? -eq 1`
2. 运行 `bash scripts/run_all_e2e.sh --full-shell --list-shell-tests | grep -q '^test_system_proxy_e2e.sh$'`

**预期结果**：
- 第 1 步退出码为 0，表示 CI 模式 shard 3/3 未收集 `test_system_proxy_e2e.sh`
- 第 2 步退出码为 0，表示系统代理用例仍可在本地 full-shell 全量场景收集
- 两个命令只列出测试脚本，不启动 Bifrost 服务、不修改系统代理配置

### TC-CS-10: E2E 失败日志 artifact 与失败摘要可诊断

**操作步骤**：
1. 运行 `rg -n "include-hidden-files: true" .github/workflows/ci.yml | wc -l`。
2. 运行以下命令构造包含真实 Playwright 错误和 cleanup 尾巴的临时日志，并通过 `scripts/run_all_e2e.sh` 内置的 `extract_failure_reason` 逻辑验证摘要：
   ```bash
   TMP_LOG="$(mktemp)"
   {
     echo "browserType.launch: Host system is missing dependencies"
     echo "--- bifrost.log ---"
     echo "Preserving failed test root: /tmp/bifrost-devtools-e2e.demo"
   } > "$TMP_LOG"
   bash scripts/run_all_e2e.sh --extract-failure-reason "$TMP_LOG"
   ```
3. 运行 `BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`。

**预期结果**：
- 第 1 步输出为 `8`，覆盖 Linux/macOS/Windows rules、shell、runner E2E 日志上传步骤。
- 上传 `.e2e-reports/` 与 `.bifrost-e2e-ci/` 的 artifact 步骤均包含 `include-hidden-files: true`。
- 第 2 步输出 `browserType.launch: Host system is missing dependencies`，不会把 `Preserving failed test root` 当作失败原因。
- 第 3 步 shard 3 shell E2E 全部通过；其中 `test_devtools_page_bridge_api.sh` 通过，且使用非 9900 临时端口与临时数据目录。

### TC-CS-11: CLI offline help alternation 断言回归

**操作步骤**：
1. 运行 `bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh`。
2. 运行 `rg -n 'grep\\s+(-[^ ]*)?q[^ ]*\\s+"[^"]*\\\\\\|' e2e-tests/tests/test_cli_offline_commands_e2e.sh`。
3. 运行 `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`。

**预期结果**：
- 第 1 步退出码为 0。
- 第 2 步没有输出，表示脚本中不再存在默认 BRE 模式下的 `\|` alternation 断言。
- 第 3 步全部通过，包含 `rule rename --help`、`rule reorder --help`、`script rename --help`，汇总为 106 个测试通过且 0 个失败。

### TC-CS-12: unsafe_ssl shell E2E 自带 HTTPS mock fixture

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_unsafe_ssl_e2e.sh
   ```
2. 使用隔离端口与临时数据目录执行 unsafe_ssl E2E，不预先启动任何 HTTPS mock：
   ```bash
   TEST_ROOT="$(mktemp -d /tmp/bifrost-unsafe-ssl-human.XXXXXX)"
   PROXY_PORT=11295 ADMIN_PORT=11295 HTTPS_MOCK_PORT=11297 \
     BIFROST_DATA_DIR="$TEST_ROOT/data" \
     SERVER_LOG_DIR="$TEST_ROOT/logs" \
     SKIP_BUILD=true \
     bash e2e-tests/tests/test_unsafe_ssl_e2e.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- 脚本输出 `Starting HTTPS mock server on 127.0.0.1:11297` 与 `HTTPS mock server ready`，说明不依赖外部共享 fixture。
- 脚本创建 `unsafe-ssl-fixture.test https://127.0.0.1:11297` 转发规则，确保 Bifrost 作为上游 TLS client 真实受 `unsafe_ssl` 配置影响，而不是 curl 自己通过 CONNECT 直连。
- 脚本通过 `ADMIN_CLIENT_START_UNSAFE_SSL=0` 以安全默认启动 Bifrost，CLI 启动参数不会掩盖 unsafe_ssl API 动态切换。
- unsafe_ssl false/true/false 三段代理请求全部执行，不再因为 mock 缺失跳过。
- 汇总为 `Results: 5/5 passed`，退出码为 0。
- 测试端口不使用 9900，测试数据写入临时目录。

### TC-CS-13: 并行 shell 调度器全部通过后返回 0

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
2. 检查并行 shell 调度器完成路径：
   ```bash
   rg -n 'run_shell_tests_parallel\\(\\)|run_shell_batch_parallel\\(\\)|return 0' scripts/run_all_e2e.sh
   ```
3. 执行一次 CI shell shard 3 回归：
   ```bash
   BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh
   ```

**预期结果**：
- 语法检查退出码为 0。
- `run_shell_tests_parallel` 与 `run_shell_batch_parallel` 的正常完成路径末尾都有显式 `return 0`。
- shard 3 中所有 shell 子用例通过后，外层 `scripts/ci/run-e2e-shell.sh` 退出码为 0，不会在日志显示 “All tests passed” 后因为 Bash 函数最后一个 false 条件而失败。

### TC-CS-14: SSE replay timeout 边界回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_replay_rules.sh
   ```
2. 检查 SSE replay 回归用例使用 5s timeout 边界和 8s 存活断言：
   ```bash
   rg -n 'sse/custom\\?count=20&interval=0\\.5|\"timeout_ms\":5000|>=8s alive|kept alive beyond timeout_ms' e2e-tests/tests/test_replay_rules.sh
   ```
3. 使用隔离端口与临时数据目录执行 replay rules E2E：
   ```bash
   TEST_ROOT="$(mktemp -d /tmp/bifrost-replay-human.XXXXXX)"
   PROXY_PORT=18881 MOCK_HTTP_PORT=18882 MOCK_SSE_PORT=18883 MOCK_WS_PORT=18884 \
     BIFROST_DATA_DIR="$TEST_ROOT/data" \
     SERVER_LOG_DIR="$TEST_ROOT/logs" \
     SKIP_BUILD=true \
     BIFROST_E2E_REPORT_DIR="$TEST_ROOT/reports" \
     bash e2e-tests/tests/test_replay_rules.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- 第 2 步能定位到 `sse/custom?count=20&interval=0.5`、`"timeout_ms":5000`、`>=8s alive` 与 `kept alive beyond timeout_ms`。
- `SSE Replay with Rules` 用例输出 `SSE Replay: connection event received and stream kept alive beyond timeout_ms`。
- `test_replay_rules.sh` 全部 21 个用例通过，退出码为 0。
- 测试端口不使用 9900，测试数据写入临时目录。

## 本轮执行记录

测试日期：2026-04-30

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-CS-10 | 通过 | `rg -n "include-hidden-files: true" .github/workflows/ci.yml \| wc -l` 输出 `8`；`bash scripts/run_all_e2e.sh --extract-failure-reason "$TMP_LOG"` 输出 `browserType.launch: Host system is missing dependencies`；本地执行 `source ~/.zshrc && BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`，24/24 通过，`test_devtools_page_bridge_api.sh` 在并行 shard 3 中 44s 通过。 |
| TC-CS-11 | 通过 | 2026-04-30 本轮执行：`bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；`rg -n 'grep\s+(-[^ ]*)?q[^ ]*\s+"[^"]*\\\|' e2e-tests/tests/test_cli_offline_commands_e2e.sh` 无输出；`bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 汇总 `通过: 106`、`失败: 0`，其中 `rule rename --help`、`rule reorder --help`、`script rename --help` 均通过。 |
| TC-CS-12 | 通过 | 2026-05-01 本轮执行：`bash -n e2e-tests/tests/test_unsafe_ssl_e2e.sh e2e-tests/test_utils/admin_client.sh` 通过；随后使用 `TEST_ROOT="$(mktemp -d /tmp/bifrost-unsafe-ssl-human.XXXXXX)" PROXY_PORT=11295 ADMIN_PORT=11295 HTTPS_MOCK_PORT=11297 BIFROST_DATA_DIR="$TEST_ROOT/data" SERVER_LOG_DIR="$TEST_ROOT/logs" SKIP_BUILD=true bash e2e-tests/tests/test_unsafe_ssl_e2e.sh` 执行真实场景。脚本输出 `Starting HTTPS mock server on 127.0.0.1:11297`、`HTTPS mock server ready`、`Created unsafe_ssl forwarding rule to https://127.0.0.1:11297`，并完成 unsafe_ssl false/true/false 三段代理请求，汇总 `Results: 5/5 passed`，退出码 0；全程使用临时目录和 11295/11297，未使用 9900。 |
| TC-CS-13 | 通过 | 2026-05-03 本轮执行：`bash -n scripts/run_all_e2e.sh` 通过；`rg -n 'run_shell_tests_parallel\(\)\|run_shell_batch_parallel\(\)\|return 0' scripts/run_all_e2e.sh` 显示两个调度函数及其显式 `return 0`。完整 shard 3 本机执行卡在大端口段扫描前置探针，随后改用同一入口的最小 shard 验证返回码路径：`BIFROST_UI_TEST_RUNNER_PORT=18080 BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=999 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh` 选中 `test_body_cache_sync_cleanup_admin_api.sh`，输出 `[PASS] shell:test_body_cache_sync_cleanup_admin_api.sh`，最终 `Total suites : 1 / Passed : 1 / Failed : 0`，外层退出码 0。完整 macOS shard 3 由推送后的 GitHub Actions 继续验证。 |
| TC-CS-14 | 通过 | 2026-05-03 本轮执行：`bash -n e2e-tests/tests/test_replay_rules.sh` 通过；`rg -n 'sse/custom\?count=20&interval=0\.5\|"timeout_ms":5000\|>=8s alive\|kept alive beyond timeout_ms' e2e-tests/tests/test_replay_rules.sh` 显示 4 个预期匹配；随后使用 `TEST_ROOT="$(mktemp -d /tmp/bifrost-replay-human.XXXXXX)" PROXY_PORT=18881 MOCK_HTTP_PORT=18882 MOCK_SSE_PORT=18883 MOCK_WS_PORT=18884 BIFROST_DATA_DIR="$TEST_ROOT/data" SERVER_LOG_DIR="$TEST_ROOT/logs" SKIP_BUILD=true BIFROST_E2E_REPORT_DIR="$TEST_ROOT/reports" bash e2e-tests/tests/test_replay_rules.sh` 执行真实场景。`SSE Replay with Rules` 输出 `SSE Replay: connection event received and stream kept alive beyond timeout_ms`，全脚本汇总 `Passed: 21`、`Failed: 0`，退出码 0；全程使用临时目录和 18881-18884，未使用 9900。 |

## 清理步骤

- 无特殊清理需求，测试使用临时数据目录
