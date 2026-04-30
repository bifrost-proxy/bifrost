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

## 本轮执行记录

测试日期：2026-04-30

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-CS-10 | 通过 | `rg -n "include-hidden-files: true" .github/workflows/ci.yml \| wc -l` 输出 `8`；`bash scripts/run_all_e2e.sh --extract-failure-reason "$TMP_LOG"` 输出 `browserType.launch: Host system is missing dependencies`；本地执行 `source ~/.zshrc && BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`，24/24 通过，`test_devtools_page_bridge_api.sh` 在并行 shard 3 中 44s 通过。 |
| TC-CS-11 | 通过 | 2026-04-30 本轮执行：`bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；`rg -n 'grep\s+(-[^ ]*)?q[^ ]*\s+"[^"]*\\\|' e2e-tests/tests/test_cli_offline_commands_e2e.sh` 无输出；`bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 汇总 `通过: 106`、`失败: 0`，其中 `rule rename --help`、`rule reorder --help`、`script rename --help` 均通过。 |

## 清理步骤

- 无特殊清理需求，测试使用临时数据目录
