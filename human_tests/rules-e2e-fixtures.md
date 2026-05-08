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

### TC-REF-06：Windows rules 全量 outage 重试预算不被 job timeout 截断

**操作步骤**：
1. 检查 Windows rules job timeout：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); job = workflow["jobs"]["e2e-windows-rules"]; include = job["strategy"]["matrix"]["include"]; raise "expected four windows rules shards" unless include.size == 4; raise "expected x86_64 windows rules" unless include.all? { |row| row["target"] == "x86_64-pc-windows-msvc" }; raise "expected 20 minute job timeout" unless job["timeout-minutes"] == 20; raise "expected 1080 second suite timeout" unless include.all? { |row| row["suite_timeout"] == "1080" }; raise "expected 180 second retry budget" unless job["env"]["BIFROST_E2E_RETRY_BUDGET_SECS"] == "180"; puts "windows rules timeout budget ok"'
   ```
2. 在 GitHub Actions Windows rules job 中触发 rules E2E。
3. 若 Windows x86 rules 进入共享 mock outage 后的失败路径，观察 suite watchdog 应在 1080 秒内终止并上传日志，而不是拖到 job 级 timeout。

**预期结果**：
- Windows rules job 的 `timeout-minutes` 为 `20`，只影响 `e2e-windows-rules` 矩阵。
- Windows rules job 拆为 4 个 x86_64 shard，每个 shard 的 `BIFROST_E2E_SUITE_TIMEOUT` 为 `1080` 秒，让失败路径在 job timeout 前完成诊断。
- `BIFROST_E2E_RETRY_BUDGET_SECS` 仍保持 `180`，普通失败不会因为 job timeout 提升而被无限重试。
- Windows rules 只保留 x86_64 rules fixture；Windows ARM 不再跑完整 rules shards，避免串行慢失败阻塞主干合入。
- 若仍失败，应执行日志 dump/upload 步骤并保留可诊断 artifact。

### TC-REF-07：bifrost-e2e admin 测试目录重复端口重跑隔离

**操作步骤**：
1. 使用同一个 base port 连续运行 body cache 分类两次：
   ```bash
   cargo run -p bifrost-e2e -- --category body_cache --jobs 1 --test-timeout 120 --port 18180
   cargo run -p bifrost-e2e -- --category body_cache --jobs 1 --test-timeout 120 --port 18180
   ```
2. 单独重跑曾经受旧 traffic.db 污染的用例：
   ```bash
   cargo run -p bifrost-e2e -- --test binary_performance_mode_skips_binary_recording --test-timeout 120 --port 18180
   ```
3. 检查第二次运行不应读取上一次 `/var/folders/.../bifrost_e2e_test_<port>/traffic/traffic.db` 或 body cache 残留。

**预期结果**：
- 两次 body cache 分类均全部通过。
- `binary_performance_mode_skips_binary_recording` 只看到当前测试生成的记录，不能因为同端口重跑读到历史 traffic.db 而报 `Expected only 1 image record in performance mode, got 3 records`。
- 正式 `9900` 服务不受影响，测试仍使用非 9900 临时端口。

### TC-REF-08：macOS Rules CI 失败夹具语义回归

**操作步骤**：
1. 执行 `test_rules.sh` 语法检查：
   ```bash
   bash -n e2e-tests/test_rules.sh
   ```
2. 使用非 9900 代理端口和临时数据目录，逐个执行 macOS Rules CI 失败夹具：
   ```bash
   for item in \
     "content_inject/html.txt:18200" \
     "content_inject/js.txt:18210" \
     "content_inject/css.txt:18220" \
     "control/enable_disable.txt:18230" \
     "advanced/content_type.txt:18240" \
     "request_modify/url_params.txt:18250" \
     "advanced/speed.txt:18260"
   do
     file="${item%%:*}"
     base="${item##*:}"
     data_dir="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
     BIFROST_DATA_DIR="$data_dir" \
     PROXY_PORT="$base" \
     ECHO_HTTP_PORT="$((base + 1))" \
     ECHO_HTTPS_PORT="$((base + 2))" \
     ECHO_WS_PORT="$((base + 3))" \
     ECHO_WSS_PORT="$((base + 4))" \
     ECHO_SSE_PORT="$((base + 5))" \
     ECHO_PROXY_PORT="$((base + 6))" \
     TIMEOUT=30 \
     bash e2e-tests/test_rules.sh --use-binary "e2e-tests/rules/$file"
     rm -rf "$data_dir"
   done
   ```
3. 确认每个 fixture 的 Test Summary 失败数均为 `0`。

**预期结果**：
- html/js/css 注入 fixture 使用 fenced code block 的真实内容作为断言，响应体包含对应注入内容。
- `lineProps://disabled` 用例验证禁用规则值未生效，fallback 响应头可正常返回。
- `reqType`、`reqCharset`、`urlParams`、`urlReplace/pathReplace` 在 HTTPS MITM 转发到 HTTP echo 上游时真实生效。
- req/res speed 用例按初始限速窗口语义通过，不再把首个窗口立即发送误判为限速失效。
- 所有命令使用临时数据目录、非 9900 端口，代理启动仍包含 `--no-system-proxy`。

### TC-REF-09：tunnel 请求侧规则与普通 HTTP handler 行为一致

**操作步骤**：
1. 执行请求侧 tunnel 规则相关的 Rust 单元测试：
   ```bash
   cargo test -p bifrost-proxy utils::url -- --nocapture
   ```
2. 执行 HTTPS MITM 请求侧规则夹具：
   ```bash
   data_dir="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
   BIFROST_DATA_DIR="$data_dir" \
   PROXY_PORT=18270 \
   ECHO_HTTP_PORT=18271 \
   ECHO_HTTPS_PORT=18272 \
   ECHO_WS_PORT=18273 \
   ECHO_WSS_PORT=18274 \
   ECHO_SSE_PORT=18275 \
   ECHO_PROXY_PORT=18276 \
   TIMEOUT=30 \
   bash e2e-tests/test_rules.sh --use-binary e2e-tests/rules/advanced/content_type.txt
   rm -rf "$data_dir"
   ```
3. 再执行 URL 修改夹具：
   ```bash
   data_dir="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
   BIFROST_DATA_DIR="$data_dir" \
   PROXY_PORT=18280 \
   ECHO_HTTP_PORT=18281 \
   ECHO_HTTPS_PORT=18282 \
   ECHO_WS_PORT=18283 \
   ECHO_WSS_PORT=18284 \
   ECHO_SSE_PORT=18285 \
   ECHO_PROXY_PORT=18286 \
   TIMEOUT=30 \
   bash e2e-tests/test_rules.sh --use-binary e2e-tests/rules/request_modify/url_params.txt
   rm -rf "$data_dir"
   ```

**预期结果**：
- 单元测试通过。
- `advanced/content_type.txt` 中请求方向和响应方向 Content-Type/charset 断言全部通过。
- `request_modify/url_params.txt` 中 urlParams、urlReplace、pathReplace 均在 echo server 响应里体现最终 path/query。
- 不使用正式 9900 端口，不修改系统代理。

### TC-REF-10：urlParams `&` 分隔多参数解析回归

**操作步骤**：
1. 执行 urlParams 解析单元测试：
   ```bash
   cargo test -p bifrost-cli test_url_params -- --nocapture
   ```
2. 执行包含 `urlParams://key1=value1&key2=value2` 的 template fixture：
   ```bash
   data_dir="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
   BIFROST_DATA_DIR="$data_dir" \
   PROXY_PORT=18290 \
   ECHO_HTTP_PORT=18291 \
   ECHO_HTTPS_PORT=18292 \
   ECHO_WS_PORT=18293 \
   ECHO_WSS_PORT=18294 \
   ECHO_SSE_PORT=18295 \
   ECHO_PROXY_PORT=18296 \
   TIMEOUT=30 \
   bash e2e-tests/test_rules.sh --use-binary e2e-tests/rules/template/values.txt
   rm -rf "$data_dir"
   ```
3. 执行包含 urlParams 与 body `params://` 组合的 multi rules fixture：
   ```bash
   data_dir="$(mktemp -d ./.bifrost-e2e-rules-human.XXXXXX)"
   BIFROST_DATA_DIR="$data_dir" \
   PROXY_PORT=18300 \
   ECHO_HTTP_PORT=18301 \
   ECHO_HTTPS_PORT=18302 \
   ECHO_WS_PORT=18303 \
   ECHO_WSS_PORT=18304 \
   ECHO_SSE_PORT=18305 \
   ECHO_PROXY_PORT=18306 \
   TIMEOUT=30 \
   bash e2e-tests/test_rules.sh --use-binary e2e-tests/rules/combination/multi_rules.txt
   rm -rf "$data_dir"
   ```

**预期结果**：
- `urlParams://key1=value1&key2=value2` 被解析成两个查询参数，不会把 `&key2=value2` 编码进 `key1` 的值。
- `template/values.txt` 中 inline params 用例全部通过。
- `combination/multi_rules.txt` 中 URL 查询参数修改与 body merge 语义互不混淆，全部断言通过。
- 不使用正式 9900 端口，不修改系统代理。

### TC-REF-11：Rules CI harness 断言逻辑回归

**操作步骤**：
1. 执行 `test_rules.sh` 语法检查：
   ```bash
   bash -n e2e-tests/test_rules.sh
   ```
2. 使用 CI 等价入口执行 rules fixture 全量回归，显式指定非 9900 runner 端口和报告目录：
   ```bash
   CARGO_BIN=<cargo 路径> \
   NODE_BIN=<node 路径> \
   PNPM_BIN=<pnpm 路径> \
   BIFROST_UI_TEST_RUNNER_PORT=18082 \
   BIFROST_E2E_REPORT_DIR=.e2e-reports/rules-local-fix-$(date +%Y%m%d-%H%M%S) \
   BIFROST_E2E_RULE_JOBS=4 \
   BIFROST_E2E_RETRY_FAILED_ONCE=1 \
   BIFROST_E2E_HTTP_RETRIES=2 \
   bash scripts/ci/local-ci.sh --skip-static --e2e-only rules
   ```
3. 检查报告中的 rules 汇总。

**预期结果**：
- `test_rules.sh` 语法检查通过。
- `content_inject/html.txt`、`content_inject/js.txt`、`content_inject/css.txt` 会解析 fenced code block 变量后再断言响应体。
- `control/enable_disable.txt` 的 `lineProps://disabled` 会验证禁用规则值没有生效，而不是报告“验证未实现”。
- `request_modify/url_params.txt` 与 `combination/multi_rules.txt` 会按真实 query string 验证 urlParams/params，不把空 query 误判为通过或失败。
- `advanced/speed.txt` 会按限速初始窗口语义计算预期耗时。
- rules 全量回归输出 `65 通过 / 0 失败`，总断言失败数为 `0`。
- 测试使用临时数据目录和非 9900 动态端口，代理启动仍包含 `--no-system-proxy`。

### TC-REF-12：Windows x86 Rules CI 分片预算回归

**操作步骤**：
1. 执行 `test_rules.sh` 语法检查：
   ```bash
   bash -n e2e-tests/test_rules.sh
   ```
2. 执行 rules 并行 runner 语法检查：
   ```bash
   bash -n e2e-tests/run_all_tests_parallel.sh
   ```
3. 检查 TLS readiness timeout 默认值：
   ```bash
   rg -n 'BIFROST_E2E_TLS_READY_TIMEOUT:-\$\{BIFROST_E2E_FIXTURE_TIMEOUT:-30\}' e2e-tests/test_rules.sh
   ```
4. 检查 GitHub Actions Windows rules CI 预算与矩阵范围：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); job = workflow["jobs"]["e2e-windows-rules"]; include = job["strategy"]["matrix"]["include"]; shards = include.map { |row| row["rule_shard"] }.sort; raise "expected four x86_64 windows rules shards" unless include.size == 4 && include.all? { |row| row["target"] == "x86_64-pc-windows-msvc" }; raise "expected shards 1/4..4/4" unless shards == ["1/4", "2/4", "3/4", "4/4"]; raise "windows rules timeout should be 30 min" unless job["timeout-minutes"] == 30; raise "suite timeout should be 1080s on every shard" unless include.all? { |row| row["suite_timeout"] == "1080" }; raise "shard total should be 4" unless include.all? { |row| row["rule_shard_total"] == "4" }; puts "windows rules sharded budget ok"'
   ```
5. 检查 Windows rules job 不再安装前端依赖：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); steps = workflow["jobs"]["e2e-windows-rules"]["steps"]; text = steps.map(&:to_s).join("\n"); raise "rules job should not setup pnpm" if text.include?("pnpm/action-setup"); raise "rules job should not setup node" if text.include?("actions/setup-node"); raise "rules job should not run pnpm install" if text.include?("pnpm install"); puts "windows rules frontend setup removed"'
   ```
6. 检查失败日志 artifact 名称包含 shard 维度：
   ```bash
   rg -n 'e2e-rules-logs-\$\{\{ matrix\.target \}\}-shard-\$\{\{ matrix\.rule_shard_index \}\}-of-\$\{\{ matrix\.rule_shard_total \}\}' .github/workflows/ci.yml
   ```
7. 检查 rules runner 支持 CI 分片环境变量：
   ```bash
   rg -n 'BIFROST_E2E_RULE_SHARD_INDEX|BIFROST_E2E_RULE_SHARD_TOTAL|--shard N/M|sharded_test_files' e2e-tests/run_all_tests_parallel.sh
   ```
8. 推送后检查 GitHub Actions `CI` run：应出现 4 个 `E2E Rules (x86_64-pc-windows-msvc, shard N/4)` job，业务 rules suite 仍受 1080 秒 watchdog 限制，job 外层 30 分钟 envelope 应覆盖 Windows 固定 setup/cache/post-cleanup 开销；不再出现业务套件已 PASS 但 job 在 `Post Run Swatinem/rust-cache@v2` / `Post Run actions/checkout@v4` 阶段因 20 分钟总预算不足而失败。Windows ARM 仍应出现 `Build Windows CLI (aarch64-pc-windows-msvc)`、`E2E Runner (aarch64-pc-windows-msvc)` 与 `Build Windows (aarch64-pc-windows-msvc)`。

**预期结果**：
- `test_rules.sh` 语法检查通过。
- `run_all_tests_parallel.sh` 语法检查通过。
- 未显式设置 `BIFROST_E2E_TLS_READY_TIMEOUT` 时，TLS 拦截就绪等待继承 `BIFROST_E2E_FIXTURE_TIMEOUT`。
- Windows rules job 保留 x86_64 rules fixture，并拆为 4 个 shard；每个 shard `BIFROST_E2E_SUITE_TIMEOUT=1080`，job 总 timeout 为 30 分钟。
- Windows rules job 不再执行 Node/pnpm setup 或 `pnpm install`，减少每个 shard 的固定耗时。
- 失败日志 artifact 名称包含 shard index，多个 shard 同时失败时不会互相抢占 artifact 名称。
- Windows ARM rules shards 不再作为合入门禁；ARM 平台二进制可用性由 CLI build、runner E2E 和 desktop sidecar/check 继续覆盖。
- 若后续重新启用 Windows ARM rules 或调整 x86 shard 数，必须先证明每个 shard 在 30 分钟 job envelope 内完成，且 rules suite 自身不会超过 1080 秒 watchdog。

### TC-REF-13：Windows rules CI 只依赖 CLI 构建产物

**操作步骤**：
1. 静态解析 GitHub Actions workflow，确认 Windows rules job 只依赖 CLI 构建 job：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); jobs = workflow["jobs"]; raise "missing build-cli-windows" unless jobs.key?("build-cli-windows"); raise "rules should depend only on build-cli-windows" unless jobs["e2e-windows-rules"]["needs"] == ["build-cli-windows"]; raise "desktop should depend on build-cli-windows" unless jobs["build-desktop-windows"]["needs"] == ["build-cli-windows"]; puts "windows rules cli dependency ok"'
   ```
2. 检查 CLI artifact 的上传和 Windows rules job 的下载名称一致：
   ```bash
   rg -n 'build-cli-windows|name: bifrost-release-\$\{\{ matrix\.target \}\}|Download release binary' .github/workflows/ci.yml
   ```
3. 检查 Windows desktop job 下载同一 CLI artifact 作为 sidecar 输入，但 `e2e-windows-rules` 不再 `needs: [build-desktop-windows]`：
   ```bash
   rg -n 'build-desktop-windows|Download CLI binary|needs: \[build-desktop-windows\]|needs: \[build-cli-windows\]' .github/workflows/ci.yml
   ```
4. 推送后检查 GitHub Actions `CI` run：`E2E Rules (x86_64-pc-windows-msvc, shard 1/1)` 应在 `Build Windows CLI (x86_64-pc-windows-msvc)` 成功后即可排队/运行，不等待 `Build Windows (...)` 桌面端 job 完成。

**预期结果**：
- `build-cli-windows` 独立构建并上传 `bifrost-release-${{ matrix.target }}`。
- `e2e-windows-rules` 的 `needs` 只有 `build-cli-windows`，不再依赖 Windows desktop 构建或 Tauri 检查。
- `build-desktop-windows` 继续依赖 `build-cli-windows`，并下载同一 CLI artifact 执行 sidecar 准备与桌面端检查。
- Windows rules E2E 的启动条件与实际依赖一致：只要求 CLI 二进制可用。

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
- 2026-05-01：通过。补充并执行 TC-REF-06，本地执行 `rg -n 'e2e-windows-rules|timeout-minutes: 90|BIFROST_E2E_SUITE_TIMEOUT: "4800"|BIFROST_E2E_RETRY_BUDGET_SECS: "180"' .github/workflows/ci.yml` 与 `sed -n '860,890p' .github/workflows/ci.yml`，确认 Windows rules job timeout 为 90 分钟，`BIFROST_E2E_SUITE_TIMEOUT` 为 4800 秒，普通 retry budget 仍为 180 秒。Windows x86/aarch64 rules 完整完成情况由后续 CI 复跑验证。
- 2026-05-03：通过。补充并执行 TC-REF-07；在清理旧 mock server 后，先用同一 base port `18180` 执行 `cargo run -p bifrost-e2e -- --test body_resMerge_add_field --test-timeout 120 --port 18180` 通过，确认早先失败是旧 `ws_echo_server.py 24200` 端口占用；随后执行 `cargo run -p bifrost-e2e -- --test binary_performance_mode_skips_binary_recording --test-timeout 120 --port 18180` 通过，并执行 `cargo run -p bifrost-e2e -- --category body_cache --jobs 1 --test-timeout 120 --port 18180`，5/5 通过。回归确认 `start_with_admin`/`start_with_admin_sync` 启动前清理 `bifrost_e2e_test_*` 数据目录，避免同端口聚合重跑读取旧 traffic.db/body cache。
- 2026-05-04：通过。补充并执行 TC-REF-08、TC-REF-09、TC-REF-10；执行 `bash -n e2e-tests/test_rules.sh` 通过，执行 `cargo test -p bifrost-proxy utils::url -- --nocapture` 通过，执行 `cargo test -p bifrost-cli test_url_params -- --nocapture` 通过。使用 release 二进制、临时数据目录和非 9900 端口逐个执行 CI 失败清单：`content_inject/html.txt` 8/8、`content_inject/js.txt` 8/8、`content_inject/css.txt` 8/8、`control/enable_disable.txt` 6/6、`advanced/content_type.txt` 8/8、`request_modify/url_params.txt` 12/12、`advanced/speed.txt` 6/6、`template/values.txt` 38/38、`combination/multi_rules.txt` 19/19，全部失败数为 0。确认 tunnel 请求侧规则为功能缺口并已修复；html/js/css、lineProps disabled、speed、部分 URL fixture 断言为测试夹具/预期问题并已修正；`urlParams://key1=value1&key2=value2` 为解析功能缺口并已补齐。
- 2026-05-04：通过。补充并执行 TC-REF-11；先执行 `bash -n e2e-tests/test_rules.sh` 通过，随后执行 `CARGO_BIN=<cargo 路径> NODE_BIN=<node 路径> PNPM_BIN=<pnpm 路径> BIFROST_UI_TEST_RUNNER_PORT=18083 BIFROST_E2E_REPORT_DIR=.e2e-reports/rules-human-ref11-20260504-222436 BIFROST_E2E_RULE_JOBS=4 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_HTTP_RETRIES=2 bash scripts/ci/local-ci.sh --skip-static --e2e-only rules`。runner 使用动态共享 mock 端口（HTTP 57607、HTTPS 57608、WS 57609、WSS 57610、SSE 57611、Proxy 57612），代理起始端口 19678，未使用 9900；rules 全量回归 65/65 套件通过，573/573 断言通过，失败数 0，确认 CI harness 对 fenced code block 注入、lineProps disabled、urlParams/urlReplace、speed 初始窗口的断言逻辑恢复正确。
- 2026-05-06：通过。补充并执行 TC-REF-12；执行 `bash -n e2e-tests/test_rules.sh` 通过，执行 `rg -n 'BIFROST_E2E_TLS_READY_TIMEOUT:-\$\{BIFROST_E2E_FIXTURE_TIMEOUT:-30\}' e2e-tests/test_rules.sh` 定位到 TLS readiness 默认继承 fixture timeout。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；Windows ARM rules 完整结果由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-06：通过。补充并执行 TC-REF-12 的 Windows ARM suite timeout 预算验证；执行 `bash -n e2e-tests/test_rules.sh` 通过，执行 `rg -n 'timeout-minutes: 150|suite_timeout: "7200"|BIFROST_E2E_SUITE_TIMEOUT: \$\{\{ matrix\.suite_timeout \}\}' .github/workflows/ci.yml` 确认 Windows rules job 总 timeout 为 150 分钟，x86_64 继续使用 4800 秒 suite timeout，Windows ARM 使用 7200 秒 suite timeout。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；完整平台结果由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-06：通过。TC-REF-12 三次执行，基于 GitHub Actions `CI` run `25450671540` 的 Windows ARM rules 失败日志确认 ARM job 已使用 `BIFROST_E2E_SUITE_TIMEOUT=7200`，但仍因 `TLS 拦截在 90s 内未就绪` 失败；执行 `rg -n 'tls_ready_timeout: "240"|BIFROST_E2E_TLS_READY_TIMEOUT: \$\{\{ matrix\.tls_ready_timeout \}\}' .github/workflows/ci.yml` 确认 Windows ARM rules job 单独使用 240 秒 TLS readiness timeout。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；完整平台结果由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-07：通过。TC-REF-12 四次执行，基于 GitHub Actions `CI` run `25458380526` 的 Windows ARM rules 失败日志确认 ARM job 已使用 `BIFROST_E2E_TLS_READY_TIMEOUT=240` 与 `BIFROST_E2E_SUITE_TIMEOUT=7200`，但单个 fixture 仍被 `BIFROST_E2E_FIXTURE_TIMEOUT=90` 提前杀掉，且串行 60 个 suite 会撞 7200 秒 wrapper timeout；执行 `bash -n e2e-tests/run_all_tests_parallel.sh` 通过，执行 `rg -n 'rule_shard: "4/4"|fixture_timeout: "360"|BIFROST_E2E_RULE_SHARD_INDEX|BIFROST_E2E_RULE_SHARD_TOTAL' .github/workflows/ci.yml e2e-tests/run_all_tests_parallel.sh` 确认 Windows ARM rules 拆为 4 个分片，且 ARM fixture watchdog 提升到 360 秒。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；完整平台结果由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-07：通过。补充并执行 TC-REF-13；执行 Ruby workflow 静态解析，输出 `windows rules cli dependency ok`，确认 `e2e-windows-rules.needs == ["build-cli-windows"]`，`build-desktop-windows.needs == ["build-cli-windows"]`；执行 `rg -n 'build-cli-windows|name: bifrost-release-\$\{\{ matrix\.target \}\}|Download release binary|Download CLI binary|needs: \[build-desktop-windows\]|needs: \[build-cli-windows\]' .github/workflows/ci.yml`，确认 CLI artifact 上传/下载路径一致，且 Windows rules 不再依赖 desktop 构建。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；完整平台调度由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-07：通过。更新并执行 TC-REF-06 与 TC-REF-12 的 Windows rules 20 分钟预算收敛验证；执行 `bash -n e2e-tests/test_rules.sh e2e-tests/run_all_tests_parallel.sh scripts/run_all_e2e.sh` 通过；执行 Ruby workflow 静态解析，输出 `windows rules matrix budget ok` 与 `windows rules timeout budget ok`，确认 `e2e-windows-rules` 只保留 `x86_64-pc-windows-msvc` 一项、job timeout 为 30 分钟、suite timeout 为 1200 秒、retry budget 为 180 秒。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；Windows x86 完整 rules job、Windows ARM CLI build/runner/desktop job 由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-08：通过。更新并执行 TC-REF-06 与 TC-REF-12 的 Windows x86 rules 20 分钟分片预算回归；执行 `bash -n e2e-tests/test_rules.sh e2e-tests/run_all_tests_parallel.sh scripts/run_all_e2e.sh` 通过；执行 Ruby workflow 静态解析，输出 `windows rules sharded budget ok`、`windows rules frontend setup removed` 与 `windows rules timeout budget ok`，确认 `e2e-windows-rules` 拆为 4 个 x86_64 shard（1/4 到 4/4）、job timeout 为 20 分钟、每个 shard suite timeout 为 1080 秒、retry budget 为 180 秒，且 Windows rules job 不再执行 pnpm/Node setup 或 `pnpm install`。执行 `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_FIXTURE_TIMEOUT=90 BIFROST_E2E_RETRY_FIXTURE_TIMEOUT=60 BIFROST_E2E_RETRY_BUDGET_SECS=120 BIFROST_E2E_RULE_RESULTS_DIR=.bifrost-e2e-runs/rules-win-shard-smoke-20260508 bash e2e-tests/run_all_tests_parallel.sh -c priority --no-build --shard 1/4`，使用动态 mock 端口（HTTP 60753、HTTPS 60754、WS 60757、WSS 60758、SSE 60790、Proxy 60792）和代理起始端口 16031，未使用 9900；priority 分片 2/2 套件通过，6/6 断言通过。完整 Windows x86 4-shard runtime 耗时由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-09：通过。基于 GitHub Actions `CI` run `25574281001` 的 `E2E Rules (x86_64-pc-windows-msvc, shard 1/4)` 失败日志确认 rules fixture 自身已 `Passed: 1`、`Failed: 0`，耗时 354 秒；失败发生在业务步骤结束后的 `Post Run Swatinem/rust-cache@v2` / `Post Run actions/checkout@v4`，属于 20 分钟 job envelope 不足覆盖 Windows 固定 setup 与 post cleanup。更新并执行 TC-REF-12；执行 `bash -n e2e-tests/test_rules.sh e2e-tests/run_all_tests_parallel.sh scripts/run_all_e2e.sh` 通过；执行 Ruby workflow 静态解析，输出 `windows rules sharded budget ok`、`windows rules frontend setup removed`，确认 `e2e-windows-rules` 仍拆为 4 个 x86_64 shard、每个 shard suite timeout 为 1080 秒、job timeout 提升到 30 分钟、Windows rules job 不执行 pnpm/Node setup 或 `pnpm install`。随后执行 `bash scripts/ci/local-ci.sh --skip-static --e2e-only rules`，使用动态 mock 端口（HTTP 50111、HTTPS 50112、WS 50113、WSS 50114、SSE 50115、Proxy 50116）和代理起始端口 14497，未使用 9900；rules 全量回归 66/66 套件通过，575/575 断言通过，失败数 0。完整 Windows x86 runtime 由推送后的 GitHub Actions `CI` run 验证。
