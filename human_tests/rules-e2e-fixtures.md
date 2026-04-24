# Rules E2E Fixtures 真实场景测试

## 功能模块说明

验证 `e2e-tests/test_rules.sh` 对规则夹具运行时占位符的处理，重点覆盖 replay 历史夹具中 `__MOCK_HTTP_PORT__` 与并行 runner 动态 HTTP echo 端口的兼容。

## 前置条件

1. 当前正式 Bifrost 可继续运行在 `9900`，但本测试禁止使用 `9900` 作为测试代理端口。
2. 测试命令必须使用临时数据目录，避免写入默认数据目录。
3. 测试代理启动必须使用 `--no-system-proxy`；`test_rules.sh` 默认启动参数已包含该选项。
4. 若需要访问外网或 GitHub，命令环境使用：
   ```bash
   HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900
   ```
5. 执行前准备 release 二进制，或允许脚本自行编译：
   ```bash
   cargo build --release --bin bifrost
   ```

## 测试用例列表

### TC-REF-01：replay localhost legacy mock 端口占位符回归

**操作步骤**：
1. 选择非 9900 的测试代理端口和临时数据目录：
   ```bash
   TEST_PORT=18880
   TEST_DATA_DIR="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
   ```
2. 执行单个 replay fixture：
   ```bash
   BIFROST_DATA_DIR="$TEST_DATA_DIR" \
   ECHO_HTTP_PORT=18881 \
   ECHO_HTTPS_PORT=18882 \
   ECHO_WS_PORT=18883 \
   ECHO_WSS_PORT=18884 \
   ECHO_SSE_PORT=18885 \
   ECHO_PROXY_PORT=18886 \
   bash e2e-tests/test_rules.sh --no-build --use-binary -p "$TEST_PORT" -d "$TEST_DATA_DIR" e2e-tests/rules/replay/forward_localhost_api.txt
   ```
3. 检查处理后的规则文件：
   ```bash
   grep -n "__MOCK_HTTP_PORT__" "$TEST_DATA_DIR/processed_rules.txt" || true
   grep -n "bifrost.local http://127.0.0.1:18881/" "$TEST_DATA_DIR/processed_rules.txt"
   ```

**预期结果**：
- `test_rules.sh` 输出 `代理应成功转发请求` 通过。
- 测试总结中失败数为 `0`。
- `processed_rules.txt` 中不包含 `__MOCK_HTTP_PORT__`。
- `processed_rules.txt` 中包含 `bifrost.local http://127.0.0.1:18881/`。

### TC-REF-02：并行 runner fixture-only replay 分类回归

**操作步骤**：
1. 执行 replay 分类并行规则测试，限制并行度减少本地资源波动：
   ```bash
   BIFROST_E2E_RULE_JOBS=2 \
   BIFROST_E2E_RULE_JOBS_CAP=2 \
   bash e2e-tests/run_all_tests_parallel.sh -c replay --no-build --retry-failed-once
   ```
2. 观察最终测试结果和失败套件列表。

**预期结果**：
- 当前 replay 历史夹具已归类为 fixture-only 时，runner 输出 `没有找到测试文件` 并以 0 退出。
- runner 启动和停止共享 mock server 正常完成，不残留测试端口。
- `replay/forward_localhost_api.txt` 的直接转发能力由 TC-REF-01 覆盖，避免因分类跳过导致误判。

## 清理步骤

1. 删除本测试创建的临时数据目录：
   ```bash
   rm -rf ./.bifrost-e2e-rules-human.*
   ```
2. 确认测试端口没有残留进程：
   ```bash
   lsof -nP -iTCP:18880 -sTCP:LISTEN || true
   lsof -nP -iTCP:18881 -sTCP:LISTEN || true
   ```
