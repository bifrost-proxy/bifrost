# CI Shell E2E 测试分片优化

## 功能模块说明

对 CI 中 shell E2E 测试进行性能优化，通过测试分片（sharding）将 ~70 个测试分配到 3 个并行 CI runner 上执行，将总耗时从 ~30 分钟降至 ~3-5 分钟。

## 前置条件

- 已构建 release binary：`cargo build --release --bin bifrost`
- 工作目录：项目根目录

## 测试用例

### TC-CS-01: 分片参数解析 --shard N/M

**操作步骤**：
1. 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/3 2>&1 | head -20`

**预期结果**：
- 输出包含 `Shard        : 1/3`
- 测试数量约 23-24（总 70 的 1/3）

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
- 三个分片测试数量之和等于总测试数（约 69-70，取决于 CI skip 列表）
- 每个测试只出现在一个分片中，无重复

### TC-CS-04: 无分片时行为不变

**操作步骤**：
1. 不设置任何分片参数，运行 `bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build 2>&1 | head -20`

**预期结果**：
- 输出不包含 `Shard` 行
- 测试数量为全量（约 69-70）

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

## 清理步骤

- 无特殊清理需求，测试使用临时数据目录
