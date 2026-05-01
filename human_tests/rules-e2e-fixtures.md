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

### TC-REF-03：Windows 共享 mock outage 后重试全部失败 rules 套件

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/run_all_tests_parallel.sh
   ```
2. 执行缩小分类的 rules runner，显式开启失败重试并限制本地并行度：
   ```bash
   BIFROST_E2E_RULE_JOBS=2 \
   BIFROST_E2E_RULE_JOBS_CAP=2 \
   BIFROST_E2E_RETRY_FAILED_ONCE=1 \
   BIFROST_E2E_MAX_RETRY_SUITES=6 \
   BIFROST_E2E_RETRY_BUDGET_SECS=180 \
   bash e2e-tests/run_all_tests_parallel.sh -c advanced --no-build --retry-failed-once
   ```
3. 在 Windows GitHub Actions rules job 中触发同一 runner。
4. 若失败日志全部指向共享 mock 掉线或连接拒绝，检查 runner 输出是否包含 `失败套件均指向共享 Mock 服务器掉线`，并确认不再因 `BIFROST_E2E_MAX_RETRY_SUITES=6` 只重试前 6 个。

**预期结果**：
- 本地脚本语法检查通过。
- 本地缩小分类 runner 能正常启动共享 mock servers，使用临时数据目录和非 9900 动态端口，代理启动仍包含 `--no-system-proxy`。
- Windows rules job 中 `JOBS>1` 时会降级为串行，降低共享 mock server 被并发套件打爆的概率。
- 如果 Windows rules job 的失败全是共享 mock outage，runner 会重启 mock servers，并串行重试全部失败套件，重试预算不少于 900 秒。
- 普通真实规则失败仍受 `BIFROST_E2E_MAX_RETRY_SUITES` 限制，避免掩盖大面积功能回归。

### TC-REF-04：mock outage 统计读取真实 suite 日志路径回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/run_all_tests_parallel.sh
   ```
2. 检查失败日志识别函数读取 runner 实际生成的 suite 日志路径：
   ```bash
   rg -n 'log_\$\{idx\}\.txt|test_\$\{idx\}\.log|result_failure_mentions_mock_outage' e2e-tests/run_all_tests_parallel.sh
   ```
3. 执行缩小分类的 rules runner，显式开启失败重试并限制本地并行度：
   ```bash
   BIFROST_E2E_RULE_JOBS=2 \
   BIFROST_E2E_RULE_JOBS_CAP=2 \
   BIFROST_E2E_RETRY_FAILED_ONCE=1 \
   BIFROST_E2E_MAX_RETRY_SUITES=6 \
   BIFROST_E2E_RETRY_BUDGET_SECS=180 \
   bash e2e-tests/run_all_tests_parallel.sh -c advanced --no-build --retry-failed-once
   ```
4. 在 GitHub Actions Windows rules job 中确认若所有失败日志都包含 `Mock 服务器未运行`、`REQUEST_CONNECT_REFUSED`、`Connection refused` 或 `os error 10061`，runner 输出会进入 `失败套件均指向共享 Mock 服务器掉线` 分支。

**预期结果**：
- 脚本语法检查通过。
- `result_failure_mentions_mock_outage` 优先读取 `log_${idx}.txt`，该路径与 `run_single_test` 写入的 `log_${test_index}.txt` 保持一致。
- 兼容读取历史 `test_${idx}.log` fallback，但不能只读取该旧路径。
- 本地缩小分类 runner 通过，且未使用 9900 端口。
- GitHub Actions Windows rules job 中，全量 mock outage 失败会被计入 `count_mock_outage_failures`，从而绕过普通失败数量上限并重试全部失败套件。

### TC-REF-05：Windows rules suite timeout 终止真实命令并保留日志

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
2. 检查 Windows 分支是否后台运行真实 suite 命令并记录真实 PID：
   ```bash
   rg -n 'is_windows|tail -n \\+1 -f|command_pid=\\$!|kill_process_tree "\\$command_pid"' scripts/run_all_e2e.sh
   ```
3. 在 GitHub Actions Windows rules job 中触发 rules E2E。
4. 若 `E2E rules tests` 超过 `BIFROST_E2E_SUITE_TIMEOUT`，检查 job 不再无日志地耗尽 timeout，而是终止真实 rules runner、登记 `rules:parallel-fixtures` 失败原因，并继续执行 `Dump failed suite logs` 与 `Upload E2E logs`。

**预期结果**：
- `scripts/run_all_e2e.sh` 语法检查通过。
- Windows 分支中 `command_pid` 指向真实 suite 命令，而不是 `tee`/`sed` 日志管道进程。
- Windows timeout watchdog 对 `command_pid` 调用 `kill_process_tree`，可终止真实 runner 及其子进程。
- 日志流由独立子 shell 包裹 `tail -f "$log_file" | sed ...` 提供，命令结束后会对日志流子 shell 调用 `kill_process_tree`，不留下 `tail -f` 残留进程，也不影响 suite 结果收集。
- 如果 Windows rules 后续仍失败，GitHub 会上传 `.e2e-reports/` 与 `.bifrost-e2e-ci/` artifact，避免再次出现无日志红灯。

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

## 执行记录

- 2026-05-01：通过。补充并执行 TC-REF-03，本地先执行 `bash -n e2e-tests/run_all_tests_parallel.sh`，随后执行 `BIFROST_E2E_RULE_JOBS=2 BIFROST_E2E_RULE_JOBS_CAP=2 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_MAX_RETRY_SUITES=6 BIFROST_E2E_RETRY_BUDGET_SECS=180 bash e2e-tests/run_all_tests_parallel.sh -c advanced --no-build --retry-failed-once`。runner 使用动态端口启动共享 mock servers（HTTP 49368、HTTPS 49369、WS 49371、WSS 49372、SSE 49373、Proxy 49374），选择代理起始端口 11402，未使用 9900；7 个 advanced 规则套件全部通过，总断言 54/54，结束时正常停止全部 mock servers。Windows 串行 cap 和共享 mock outage 全量重试继续由 GitHub Actions Windows rules job 验证。
- 2026-05-01：通过。补充并执行 TC-REF-04，本地执行 `bash -n e2e-tests/run_all_tests_parallel.sh` 通过；执行 `rg -n 'log_\$\{idx\}\.txt|test_\$\{idx\}\.log|result_failure_mentions_mock_outage' e2e-tests/run_all_tests_parallel.sh` 确认 outage 识别优先读取 `log_${idx}.txt`，并保留 `test_${idx}.log` fallback；随后执行 `BIFROST_E2E_RULE_JOBS=2 BIFROST_E2E_RULE_JOBS_CAP=2 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_MAX_RETRY_SUITES=6 BIFROST_E2E_RETRY_BUDGET_SECS=180 bash e2e-tests/run_all_tests_parallel.sh -c advanced --no-build --retry-failed-once`。runner 使用动态端口启动共享 mock servers（HTTP 60377、HTTPS 60378、WS 60379、WSS 60380、SSE 60381、Proxy 60382），选择代理起始端口 11362，未使用 9900；7 个 advanced 规则套件全部通过，总断言 54/54，结束时正常停止全部 mock servers。Windows 全量 mock outage 失败计数由后续 GitHub Actions Windows rules job 验证。
- 2026-05-01：通过。补充并执行 TC-REF-05，本地执行 `bash -n scripts/run_all_e2e.sh` 通过；执行 `rg -n 'is_windows|tail -n \+1 -f|command_pid=\$!|kill_process_tree "\$command_pid"' scripts/run_all_e2e.sh` 和 `sed -n '392,420p' scripts/run_all_e2e.sh`，确认 Windows 分支先后台运行真实命令并记录 `command_pid=$!`，再用子 shell 包裹 `tail -n +1 -f "$log_file"` 流式打印日志，watchdog 对真实 `command_pid` 调用 `kill_process_tree`，命令结束后会对日志流子 shell调用 `kill_process_tree` 主动停止日志流。GitHub Actions Windows rules timeout artifact 行为由后续 CI 复跑验证。
- 2026-05-01：通过。TC-REF-05 二次执行，本地执行 `bash -n scripts/run_all_e2e.sh` 通过；执行 `sed -n '396,482p' scripts/run_all_e2e.sh` 和 `rg -n 'tail -n \+1 -f|kill_process_tree "\$stream_pid"|kill_process_tree "\$command_pid"|command_pid=\$!' scripts/run_all_e2e.sh`，确认日志流已改为子 shell 后台任务，命令结束后对 `stream_pid` 调用 `kill_process_tree`，避免 `tail -f` 残留导致 Windows runner/rules wrapper 不退出。
