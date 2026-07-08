# CI Shell E2E 测试分片优化

## 功能模块说明

对 CI 中 shell E2E 测试进行性能优化，通过测试分片（sharding）控制单个 CI job 的执行预算，同时避免过度占用 GitHub Actions runner 排队资源。当前 Linux shell E2E 合并为单个 job 执行，macOS shell E2E 合并为 2 个分片执行。CI 模式不执行会修改宿主系统代理设置的 `test_system_proxy_e2e.sh`，该用例仅在本地 full-shell 场景验证。

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

### TC-CS-06: 单分片执行在 CI 预算内完成

**操作步骤**：
1. 运行 `time BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=4 bash scripts/ci/run-e2e-shell.sh`

**预期结果**：
- 总耗时在 GitHub Actions 60 分钟 job timeout 内完成
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
3. 运行 `BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=4 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`。

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
- 如果 `HTTPS_MOCK_PORT` 已被非 `https_echo_server` 服务占用，脚本输出 `Selected alternate HTTPS mock port ... because requested port was occupied by a non-mock service`，并使用新端口启动自己的 mock。
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
   BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=4 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh
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
2. 检查 SSE replay 回归用例使用 5s timeout 边界和 post-timeout 事件断言：
   ```bash
   rg -n 'sse/custom\\?count=20&interval=0\\.5|\"timeout_ms\":5000|id>=12|post-timeout event|kept alive beyond timeout_ms' e2e-tests/tests/test_replay_rules.sh
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
- 第 2 步能定位到 `sse/custom?count=20&interval=0.5`、`"timeout_ms":5000`、`id>=12`、`post-timeout event` 与 `kept alive beyond timeout_ms`。
- `SSE Replay with Rules` 用例输出 `SSE Replay: connection event received and stream kept alive beyond timeout_ms`，或在 macOS CI 边界噪声下输出 `SSE Replay: received post-timeout event before client disconnect`。
- `test_replay_rules.sh` 全部 21 个用例通过，退出码为 0。
- 测试端口不使用 9900，测试数据写入临时目录。

### TC-CS-15: unsafe_ssl 管理端端口碰撞回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/test_utils/admin_client.sh e2e-tests/tests/test_unsafe_ssl_e2e.sh
   ```
2. 检查管理端 helper 会校验 `/api/auth/status` 的 Bifrost JSON 结构：
   ```bash
   rg -n 'admin_probe_existing_bifrost|admin_is_bifrost_admin_response|/api/auth/status|Bifrost admin API not available' e2e-tests/test_utils/admin_client.sh e2e-tests/tests/test_unsafe_ssl_e2e.sh
   ```
3. 启动一个非 Bifrost HTTP 服务占用端口，并验证 helper 不会把它误判为 Bifrost：
   ```bash
   TEST_ROOT="$(mktemp -d /tmp/bifrost-admin-probe-human.XXXXXX)"
   python3 - "$TEST_ROOT/not-bifrost.log" <<'PY' &
   import json
   import sys
   from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

   log_path = sys.argv[1]

   class Handler(BaseHTTPRequestHandler):
       def log_message(self, fmt, *args):
           return

       def _write(self, payload):
           body = json.dumps(payload, ensure_ascii=False).encode()
           self.send_response(200)
           self.send_header("Content-Type", "application/json")
           self.send_header("Content-Length", str(len(body)))
           self.end_headers()
           self.wfile.write(body)

       def do_GET(self):
           with open(log_path, "a", encoding="utf-8") as fh:
               fh.write(f"GET {self.path}\n")
           self._write({"choices": [{"message": {"content": "not bifrost"}}]})

   ThreadingHTTPServer(("127.0.0.1", 18885), Handler).serve_forever()
   PY
   MOCK_PID=$!
   sleep 0.5
   ADMIN_PORT=18885 ADMIN_HOST=127.0.0.1 ADMIN_PATH_PREFIX=/_bifrost \
     bash -c 'source e2e-tests/test_utils/admin_client.sh; if admin_probe_existing_bifrost; then exit 1; else exit 0; fi'
   kill "$MOCK_PID" 2>/dev/null || true
   wait "$MOCK_PID" 2>/dev/null || true
   rm -rf "$TEST_ROOT"
   ```

**预期结果**：
- 脚本语法检查通过。
- 第 2 步能定位到管理端响应结构校验逻辑和 unsafe_ssl 的明确错误输出。
- 第 3 步退出码为 0，说明即使本机端口上有其它服务返回 200，helper 也不会复用该服务。
- 测试端口不使用 9900，临时文件写入 `/tmp/bifrost-admin-probe-human.*`。

### TC-CS-16: long-term memory human API 构建不触发 frontend build

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_long_term_memory_human_api.sh
   ```
2. 检查该脚本构建 Bifrost 时显式跳过 frontend build：
   ```bash
   rg -n 'SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost' e2e-tests/tests/test_long_term_memory_human_api.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- 第 2 步能定位到 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost`。
- 该 shell E2E 在 CI 并行执行时不会触发 `pnpm build` 重写 `web/dist`，避免 `rust_embed` proc-macro 在 frontend 产物临时缺失时 panic。

### TC-CS-17: remote relay fallback 预构建 binary 复用回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh
   ```
2. 检查脚本尊重外层 `SKIP_BUILD=true` 和已有 `BIFROST_BIN`：
   ```bash
   rg -n 'SKIP_BUILD|BIFROST_BIN|Using existing bifrost binary|SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost' e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh
   ```
3. 在已存在 release binary 的前提下执行 remote relay fallback E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- 第 2 步能定位到复用已有 binary 的分支和 fallback build 命令。
- 第 3 步输出 `Using existing bifrost binary`，不会输出 `Build bifrost (release)...`。
- 三段 relay 选择断言全部通过，输出 `All remote relay URL fallback assertions passed.`。
- 测试端口动态分配，不使用 9900，测试数据写入临时目录并在退出时清理。

### TC-CS-18: macOS CI SSE replay post-timeout 连接噪声回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_replay_rules.sh
   ```
2. 检查 `test_replay_rules.sh` 在 curl 进程提前退出时会先识别 post-timeout SSE 事件：
   ```bash
   rg -n 'received post-timeout event before client disconnect|\"id\":\"\\(1\\[2-9\\]\\|\\[2-9\\]\\[0-9\\]\\+\\)\"|missing connection/applied_rules/post-timeout event' e2e-tests/tests/test_replay_rules.sh
   ```
3. 使用隔离端口与临时数据目录执行 replay rules E2E：
   ```bash
   TEST_ROOT="$(mktemp -d /tmp/bifrost-replay-ci-noise-human.XXXXXX)"
   PROXY_PORT=18891 MOCK_HTTP_PORT=18892 MOCK_SSE_PORT=18893 MOCK_WS_PORT=18894 \
     BIFROST_DATA_DIR="$TEST_ROOT/data" \
     SERVER_LOG_DIR="$TEST_ROOT/logs" \
     SKIP_BUILD=true \
     BIFROST_E2E_REPORT_DIR="$TEST_ROOT/reports" \
     bash e2e-tests/tests/test_replay_rules.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- 第 2 步能定位到 post-timeout 事件兜底断言和失败提示。
- `SSE Replay with Rules` 不要求连接固定存活到 8s；只要已经收到 `id>=12` 的 timeout 边界后事件，就证明 replay 没有在 `timeout_ms=5000` 截断 SSE body。
- `test_replay_rules.sh` 全部 21 个用例通过，退出码为 0。
- 测试端口不使用 9900，测试数据写入临时目录。

### TC-CS-19: Linux/macOS shell E2E timeout 预算回归

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`，确认 Linux `e2e-shell` 与 macOS `e2e-macos-shell` job 的 timeout：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); raise "linux timeout mismatch" unless workflow["jobs"]["e2e-shell"]["timeout-minutes"] == 60; raise "mac timeout mismatch" unless workflow["jobs"]["e2e-macos-shell"]["timeout-minutes"] == 60; puts "linux and macOS e2e-shell timeouts are 60"'
   ```
2. 检查 Linux/macOS shell E2E job 都使用 60 分钟 timeout，且 Linux shell E2E 仍安装 Playwright chromium headless shell 与 Linux 依赖：
   ```bash
   rg -n 'e2e-shell:|e2e-macos-shell:|timeout-minutes: 60|playwright install --with-deps chromium-headless-shell' .github/workflows/ci.yml
   ```

**预期结果**：
- YAML 解析通过，Linux `e2e-shell` 与 macOS `e2e-macos-shell` timeout 均为 60 分钟。
- 第 2 步能定位到两个 shell E2E job、60 分钟 timeout 和 Linux shell E2E 的 Playwright `--with-deps` 安装步骤，证明 timeout 预算覆盖 Linux 依赖安装和 macOS shard 真实运行/归档成本。
- 该回归不启动 Bifrost，不使用 9900 端口，不修改系统代理。

### TC-CS-20: CLI offline 输出断言与失败日志 dump pipefail 回归

**操作步骤**：
1. 运行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh
   ```
2. 检查 CLI offline 脚本不再使用 `echo "$result" | grep -q` 断言命令输出：
   ```bash
   rg -n 'echo "\$[A-Za-z_][A-Za-z0-9_]*" \| grep -[A-Za-z]+' e2e-tests/tests/test_cli_offline_commands_e2e.sh
   ```
3. 使用预构建二进制执行 CLI offline E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_cli_offline_commands_e2e.sh
   ```
4. 检查 GitHub Actions 失败日志 dump 不再让 `find | head` 的 SIGPIPE 使诊断 step 失败：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); count = File.read(".github/workflows/ci.yml").scan(/find "\$BIFROST_(?:E2E_REPORT_DIR|DATA_DIR)".*\| head -(?:10|20) \|\| true/).length; raise "dump pipefail guard mismatch: #{count}" unless count == 24; puts "dump pipefail guards: #{count}"'
   ```

**预期结果**：
- 第 1 步退出码为 0。
- 第 2 步没有输出，表示所有变量输出断言都改为 here-string 或等效非管道输入，避免 `grep -q` 早退触发 Broken pipe。
- 第 3 步全部通过；`system-proxy enable --help` 显示为通过，不再出现 `echo: write error: Broken pipe`。
- 第 4 步输出 `dump pipefail guards: 24`，覆盖 8 个 `Dump failed suite logs` 步骤中的 report/data 目录枚举和 tail 文件列表枚举。
- 该回归只读取 CLI help 与 workflow YAML，不启动 Bifrost，不使用 9900 端口，不修改系统代理。

### TC-CS-21: Agent/IM human-api 并行端口隔离回归

**操作步骤**：
1. 执行相关 shell 用例语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_agent_builtin_status_runtime.sh \
     e2e-tests/tests/test_im_guide_queue_human_api.sh \
     e2e-tests/tests/test_long_term_memory_human_api.sh \
     e2e-tests/tests/test_update_plan_human_api.sh \
     e2e-tests/tests/test_agent_loop_runtime_limits.sh
   ```
2. 检查这些会自启动 Bifrost 与 mock model 的脚本优先消费并行调度器端口：
   ```bash
   rg -n 'BIFROST_PORT="\$\{BIFROST_PORT:-\$\{ADMIN_PORT:-|MOCK_PORT="\$\{MOCK_PORT:-\$\{MOCK_HTTP_PORT:-' \
     e2e-tests/tests/test_agent_builtin_status_runtime.sh \
     e2e-tests/tests/test_im_guide_queue_human_api.sh \
     e2e-tests/tests/test_long_term_memory_human_api.sh \
     e2e-tests/tests/test_update_plan_human_api.sh \
     e2e-tests/tests/test_agent_loop_runtime_limits.sh
   ```
3. 检查这些脚本尊重外层预构建 binary：
   ```bash
   rg -n 'SKIP_BUILD|BIFROST_BIN|skipping build, using' \
     e2e-tests/tests/test_agent_builtin_status_runtime.sh \
     e2e-tests/tests/test_im_guide_queue_human_api.sh \
     e2e-tests/tests/test_long_term_memory_human_api.sh \
     e2e-tests/tests/test_update_plan_human_api.sh \
     e2e-tests/tests/test_agent_loop_runtime_limits.sh
   ```
4. 用 CI 调度器风格端口执行 guide/queue 黑盒真实链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
   ADMIN_PORT=18111 MOCK_HTTP_PORT=18112 \
     bash e2e-tests/tests/test_im_guide_queue_human_api.sh
   ```
5. 用另一组 CI 调度器风格端口执行 `/status` 运行中指标黑盒真实链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
   ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 \
     bash e2e-tests/tests/test_agent_builtin_status_runtime.sh
   ```

**预期结果**：
- 第 1 步所有脚本语法检查通过。
- 第 2 步每个脚本均能匹配到 `ADMIN_PORT` 与 `MOCK_HTTP_PORT` 回退表达式，证明并行 shell 调度器分配的端口会覆盖固定本地默认端口。
- 第 3 步每个脚本均能匹配到 `SKIP_BUILD` / `BIFROST_BIN` / `skipping build, using`，证明外层传入 `SKIP_BUILD=true` 时会复用预构建 binary，不再强制 `cargo build`。
- 第 4 步输出 `skipping build, using`、`starting bifrost on 18111`、`configuring agent mock provider`、`[im-guide-queue-human-api] PASS`，不再因为与其它并行用例争抢 `18897/18898` 出现 `curl: (52) Empty reply from server`。
- 第 5 步输出 `skipping build, using`、`starting bifrost on 18121`、`configuring agent mock provider`、`[agent-builtin-status-runtime] PASS`，运行中 `/status` 指标仍通过。
- 两个真实链路均使用临时数据目录、`--no-system-proxy` 和非 9900 端口。

### TC-CS-22: main push CI concurrency 取消旧 run 回归

**操作步骤**：
1. 静态检查 GitHub Actions workflow 顶层 concurrency：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); raise "group mismatch" unless workflow["concurrency"]["group"] == "${{ github.workflow }}-${{ github.ref }}"; raise "cancel-in-progress mismatch" unless workflow["concurrency"]["cancel-in-progress"] == true; puts "ci concurrency cancels stale runs"'
   ```
2. 连续向 `main` 推送修复 commit 后，检查 GitHub Actions `CI` run 列表。

**预期结果**：
- 第 1 步输出 `ci concurrency cancels stale runs`。
- 新 push 到 `main` 时，同一 `CI-refs/heads/main` concurrency group 下旧的 pending/in-progress run 被取消，最新 commit 的 run 获得执行权。
- 该回归只读取 workflow YAML；不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-23: Linux/macOS shell shard 内部并发预算回归

**操作步骤**：
1. 静态检查 GitHub Actions 中 Linux 与 macOS shell shard 的内部并发：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); linux = workflow["jobs"]["e2e-shell"]["env"]["BIFROST_E2E_SHELL_JOBS"]; mac = workflow["jobs"]["e2e-macos-shell"]["env"]["BIFROST_E2E_SHELL_JOBS"]; raise "linux jobs mismatch: #{linux.inspect}" unless linux == "4"; raise "mac jobs mismatch: #{mac.inspect}" unless mac == "2"; puts "shell shard jobs budget ok"'
   ```
2. 推送后检查 GitHub Actions `CI` run 中 `E2E Shell (Linux)` 与 macOS shell shards。

**预期结果**：
- 第 1 步输出 `shell shard jobs budget ok`。
- Linux shell job 内部并发为 4；macOS 两个 shell shard 的内部并发为 2，避免 hosted runner 在 8 路或 16 路内部并发下将多个 Bifrost 子进程 OOM kill。
- 该静态回归只读取 workflow YAML；不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-24: 顶层 shell E2E 全 PASS 退出码回归

**操作步骤**：
1. 检查 `scripts/run_all_e2e.sh` 的 final status 检查在失败 suite 才 `exit 1`，并在无失败 suite 时显式 `exit 0`：
   ```bash
   tail -n 16 scripts/run_all_e2e.sh
   ```
2. 运行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
3. 使用 shell E2E 入口执行一个最小 shard，验证只有通过用例时外层退出码为 0：
   ```bash
   BIFROST_UI_TEST_RUNNER_PORT=18080 \
     BIFROST_E2E_SHARD_INDEX=3 \
     BIFROST_E2E_SHARD_TOTAL=999 \
     BIFROST_E2E_SHELL_JOBS=4 \
     TIMEOUT=90 \
     bash scripts/ci/run-e2e-shell.sh
   ```

**预期结果**：
- 第 1 步能看到 final status 循环之后存在显式 `exit 0`。
- 第 2 步语法检查通过。
- 第 3 步至少选中并执行一个 shell 用例，最终报告 `Failed : 0`，外层命令退出码为 0；不再出现所有 suite 日志均 PASS 但 CI step 仍进入 `Dump failed suite logs` 的情况。
- 该回归使用临时数据目录、`--no-system-proxy` 和非 9900 端口。

### TC-CS-25: shell E2E 默认 Cargo 解析回归

**操作步骤**：
1. 检查 `scripts/run_all_e2e.sh` 不再把默认 Cargo 固定到 `$HOME/.cargo/bin/cargo`：
   ```bash
   rg -n 'CARGO_BIN="\$\{CARGO_BIN:-\$\(resolve_non_shim_command cargo\)\}"' scripts/run_all_e2e.sh
   ```
2. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
3. 使用当前 shell 选中的 Cargo 运行 shell E2E 列表模式：
   ```bash
   CARGO_BIN="$(which cargo)" \
     bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests
   ```

**预期结果**：
- 第 1 步能定位到 `resolve_non_shim_command cargo` 默认解析逻辑。
- 第 2 步语法检查通过。
- 第 3 步只列出 shell tests，不构建、不启动 Bifrost、不修改系统代理；显式 `CARGO_BIN="$(which cargo)"` 证明入口仍保留调用方覆盖能力。
- 在本机同时存在旧 rustup Cargo 与新版 Homebrew/系统 Cargo 时，shell E2E 子脚本内部的 `cargo test/run` 会继承入口选定的 Cargo，不再因 `$HOME/.cargo/bin/cargo` 旧版本解析 2024 edition 依赖失败。

### TC-CS-26: site docs sync E2E 缺失 site 依赖自举回归

**操作步骤**：
1. 删除 `site/node_modules`，模拟 GitHub Actions shell shard 只安装 `web/` 依赖、未安装 `site/` 依赖的环境：
   ```bash
   rm -rf site/node_modules
   ```
2. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_site_docs_sync.sh
   ```
3. 执行真实 site docs sync E2E：
   ```bash
   bash e2e-tests/tests/test_site_docs_sync.sh
   ```

**预期结果**：
- 第 2 步语法检查通过。
- 第 3 步先输出 `Installing site dependencies for docs sync E2E...`，随后 `docs:sync`、`docs:verify`、Astro build 与 `site:verify-links` 全部通过。
- 脚本结束后清理临时 `docs/future-docs-sync-probe.md` 与 `site/dist`，不会遗留探针文档。
- 该回归不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-27: Linux shell E2E 非交互 CA gate 回归

**操作步骤**：
1. 对本轮涉及的 shell E2E 脚本执行语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_asr_task_cli.sh \
     e2e-tests/tests/test_agent_builtin_status_runtime.sh \
     e2e-tests/tests/test_agent_codex_alignment_chat_api.sh \
     e2e-tests/tests/test_agent_send_msg_default_channel.sh \
     e2e-tests/tests/test_body_replace.sh \
     e2e-tests/tests/test_im_guide_queue_human_api.sh \
     e2e-tests/tests/test_long_term_memory_human_api.sh \
     e2e-tests/tests/test_req_res_script_e2e.sh \
     e2e-tests/tests/test_res_body_override_large.sh \
     e2e-tests/tests/test_rule_match_logging_noise.sh \
     e2e-tests/tests/test_rules_hot_reload.sh \
     e2e-tests/tests/test_values_hot_reload.sh \
     e2e-tests/tests/test_weixin_provider_e2e.sh
   ```
2. 静态检查这些脚本中非交互 `start --no-system-proxy` 的 Bifrost 启动路径均显式包含 `--skip-cert-check`，且 `test_asr_task_cli.sh` 支持 `SKIP_BUILD=true` 复用 CI 预构建 release binary。
3. 使用已构建的 release binary 真实执行 ASR task CLI 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_asr_task_cli.sh
   ```
4. 使用已构建的 release binary 真实执行一个普通规则启动回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_values_hot_reload.sh
   ```
5. 使用已构建的 release binary 真实执行 proxy chain 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh
   ```

**预期结果**：
- 所有脚本语法检查通过。
- 静态检查不会发现 CI 非交互启动路径缺少 `--skip-cert-check`。
- ASR task CLI 输出 `[asr-task-cli-e2e] skipping build, using .../target/release/bifrost` 与 `[asr-task-cli-e2e] PASS`，不会在 shell shard 中重新 debug build。
- Values hot reload 汇总 `Total: 5 / Passed: 5 / Failed: 0`。
- Proxy chain auth 汇总 `Total: 11 / Passed: 11 / Failed: 0`，包含 absolute-form URL 与 `Proxy-Authorization` 断言。
- 所有真实执行均使用临时数据目录、`--no-system-proxy` 与非 9900 端口。

### TC-CS-28: Agent history/direct-path shell E2E 复用 CI release binary 回归

**操作步骤**：
1. 对两个 Agent shell E2E 脚本执行语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_agent_chat_history_continue.sh \
     e2e-tests/tests/test_agent_direct_path_switch.sh
   ```
2. 静态检查两个脚本的 binary 选择逻辑：
   ```bash
   rg -n 'SKIP_BUILD|target/release/bifrost|target/debug/bifrost|skipping build, using|bifrost binary not found' \
     e2e-tests/tests/test_agent_chat_history_continue.sh \
     e2e-tests/tests/test_agent_direct_path_switch.sh
   ```
3. 使用已构建的 release binary 真实执行 history continue 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     bash e2e-tests/tests/test_agent_chat_history_continue.sh
   ```
4. 使用已构建的 release binary 真实执行 direct path switch 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     bash e2e-tests/tests/test_agent_direct_path_switch.sh
   ```

**预期结果**：
- 两个脚本语法检查通过。
- 静态检查显示 `SKIP_BUILD=true` 分支默认 `$REPO_DIR/target/release/bifrost`，本地构建分支默认 `$REPO_DIR/target/debug/bifrost`，且 binary 不存在时有明确错误。
- 两个真实执行均输出 `skipping build, using .../target/release/bifrost`，不会查找 `target/debug/bifrost`。
- `test_agent_chat_history_continue.sh` 输出 `[agent-chat-history-continue] PASS`，验证压缩后的历史恢复、计划恢复、续聊写回与外部 history path 拒绝。
- `test_agent_direct_path_switch.sh` 输出 `[agent-direct-path-switch] PASS`，验证绝对路径消息直接切换工作目录，不调用模型，并且 `/status` 展示新工作路径。
- 两个真实执行均使用临时数据目录、`--no-system-proxy` 与动态非 9900 端口。

### TC-CS-29: Cargo-heavy shell E2E 串行调度避免 artifact lock 超时

**操作步骤**：
1. 执行 runner 语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
2. 静态检查所有本轮观察到 Cargo artifact lock 等待的 shell E2E 用例都登记在 `CARGO_HEAVY_TESTS`：
   ```bash
   rg -n 'CARGO_HEAVY_TESTS|test_agent_builtin_status_runtime.sh|test_agent_codex_parity_contracts.sh|test_agent_loop_runtime_limits.sh|test_asr_model_autonomy.sh|test_asr_task_pause_resume.sh|test_chatgpt_web_behavior_artifacts.sh|test_client_process_transport_attribution.sh|test_http3_e2e.sh|test_im_agent_markdown_image_reply.sh|test_im_agent_streaming_progress_card.sh|test_im_gateway_long_reply_delivery_regression.sh|test_long_term_memory_remember_recall.sh|test_qwen3_asr_local_server.sh|test_qwen3_asr_runtime_guards.sh|test_skill_creator_flow.sh|test_sync_login_direct_e2e.sh|test_utf8_safe_preview_e2e.sh|test_voice_input_runtime.sh|is_cargo_heavy|serial_tests' scripts/run_all_e2e.sh
   ```
3. 列出 CI shard 1 的 shell 用例，确认本次失败相关的 cargo-heavy 用例仍属于 shard 1：
   ```bash
   BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 \
     scripts/run_all_e2e.sh --ci --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests
   ```
4. 用本轮失败 artifact 复核原始根因：
   ```bash
   grep -R -n 'Blocking.*file lock on artifact directory' \
     /tmp/bifrost-ci-26451521064/linux-shard-1/.e2e-reports \
     /tmp/bifrost-ci-26451521064/macos-shard-1/.e2e-reports | head -20
   ```

**预期结果**：
- 语法检查通过。
- 静态检查显示 `CARGO_HEAVY_TESTS` 包含当前 CI 失败相关和同 shard 观察到 artifact lock 等待的 Cargo 用例，且 `is_cargo_heavy` 会把它们加入 `serial_tests`。
- shard 1 列表仍包含 `test_agent_codex_parity_contracts.sh`、`test_chatgpt_web_behavior_artifacts.sh`、`test_im_agent_streaming_progress_card.sh`、`test_long_term_memory_remember_recall.sh`、`test_skill_creator_flow.sh` 等用例，说明覆盖范围不被跳过，只改变调度方式。
- artifact 复核能定位原始失败是并发 Cargo artifact lock 竞争，而不是业务断言失败。
- 该回归不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-30: HTTP3/Replay shell E2E 不依赖外部 httpbin 域名

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_http3_e2e.sh \
     e2e-tests/tests/test_replay_body_decode.sh \
     e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh \
     scripts/run_all_e2e.sh
   ```
2. 静态检查 HTTP3/Replay 失败路径不再引用公网 `httpbin.org` 或 `echo.websocket.events`：
   ```bash
   rg -n 'httpbin\.org|echo\.websocket\.events' \
     e2e-tests/tests/test_http3_e2e.sh \
     e2e-tests/tests/test_replay_body_decode.sh \
     e2e-tests/rules/http3/http3_e2e.txt
   ```
3. 使用已有 Bifrost binary 执行 HTTP3 shell 回归，避免触发本地重构建：
   ```bash
   BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost \
   SKIP_BUILD=true \
   SKIP_CARGO_TEST=true \
   PROXY_PORT=18991 \
   ECHO_HTTP_PORT=18992 \
   ECHO_HTTPS_PORT=18993 \
   SERVER_LOG_DIR=/tmp/bifrost-http3-mock-logs \
   bash e2e-tests/tests/test_http3_e2e.sh
   ```
4. 使用本地 HTTP mock 执行 replay gzip body decode 回归：
   ```bash
   BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost \
   SKIP_BUILD=true \
   PROXY_PORT=18981 \
   MOCK_HTTP_PORT=18982 \
   BIFROST_DATA_DIR=/tmp/bifrost-replay-body-decode-test \
   SERVER_LOG_DIR=/tmp/bifrost-replay-body-decode-test/mock-logs \
   bash e2e-tests/tests/test_replay_body_decode.sh
   ```
5. 使用已有 Bifrost binary 执行 startup auth preflight 回归：
   ```bash
   BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost \
   SKIP_BUILD=true \
   BIFROST_CHATGPT_WEB_STARTUP_E2E_PORT=18971 \
   bash e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh
   ```

**预期结果**：
- 语法检查通过。
- 静态检查在 HTTP3/Replay 失败路径中无公网 `httpbin.org` 或 `echo.websocket.events` 引用。
- HTTP3 shell 回归启动本地 HTTP/HTTPS mock，host forwarding、response body append、gzip、SSE、POST、PUT/PATCH/DELETE 等断言全部通过，最终 `Passed: 34`、`Failed: 0`。
- Replay gzip 回归启动本地 HTTP mock，`Replay decoded gzip response body as JSON` 通过，最终 `Results: 1 passed, 0 failed`。
- Startup auth preflight 输出 `using existing bifrost binary` 和 `[chatgpt-web-startup-auth] PASS`，不会进入 `cargo build`，避免 macOS shell shard 内 Cargo artifact lock 竞争。

### TC-CS-31: Feishu card Agent shell E2E 复用 CI release binary 回归

**操作步骤**：
1. 对 Feishu card Agent shell E2E 脚本执行语法检查：
   ```bash
   bash -n e2e-tests/tests/test_agent_send_msg_feishu_card.sh
   ```
2. 静态检查脚本的端口与 binary 选择逻辑：
   ```bash
   rg -n 'BIFROST_PORT=.*ADMIN_PORT|MOCK_PORT=.*MOCK_HTTP_PORT|SKIP_BUILD|target/release/bifrost|target/debug/bifrost|bifrost binary is not executable' \
     e2e-tests/tests/test_agent_send_msg_feishu_card.sh
   ```
3. 使用已构建的 release binary 和并行调度器端口变量真实执行 Feishu card 链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     ADMIN_PORT=18945 MOCK_HTTP_PORT=18946 \
     bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh
   ```
4. 用本轮失败 artifact 复核原始根因：
   ```bash
   grep -R -n 'target/debug/bifrost: No such file or directory' \
     /tmp/bifrost-ci-artifacts-26515240075/e2e-shell-logs-*/.e2e-reports/shell_test_agent_send_msg_feishu_card_sh.log
   ```

**预期结果**：
- 脚本语法检查通过。
- 静态检查显示脚本优先消费 `ADMIN_PORT` / `MOCK_HTTP_PORT`，`SKIP_BUILD=true` 分支默认 `$REPO_DIR/target/release/bifrost`，本地构建分支默认 `$REPO_DIR/target/debug/bifrost`，且 binary 不可执行时有明确错误。
- 真实执行输出 `skipping build, using .../target/release/bifrost` 与 `[agent-send-msg-feishu-card] PASS`，不会查找 `target/debug/bifrost`。
- fake Feishu 收到 interactive card 请求，outbound message log 记录 `trigger=agent_tool:send_msg` 且 `msg_type=interactive`。
- artifact 复核能定位原始失败是 CI release artifact 场景下脚本错误查找 debug binary，而不是业务断言失败。
- 真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。

### TC-CS-32: macOS shell shard ASR/history 脚本复用 CI release binary 回归

**操作步骤**：
1. 对三个曾在 macOS shard 3/3 中超时的 shell E2E 脚本执行语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_agent_history_pagination_api.sh \
     e2e-tests/tests/test_asr_task_append_during_run.sh \
     e2e-tests/tests/test_asr_task_startup_recovery.sh
   ```
2. 静态检查三个脚本的 `SKIP_BUILD=true` 分支：
   ```bash
   rg -n 'SKIP_BUILD|target/release/bifrost|target/debug/bifrost|bifrost binary is not executable|skipping build, using' \
     e2e-tests/tests/test_agent_history_pagination_api.sh \
     e2e-tests/tests/test_asr_task_append_during_run.sh \
     e2e-tests/tests/test_asr_task_startup_recovery.sh
   ```
3. 复核 GitHub Actions 失败日志，确认原始失败是并行脚本重复触发 Cargo build 导致 900 秒 suite timeout：
   ```bash
   rg -n 'TIMEOUT.*test_agent_history_pagination_api|TIMEOUT.*test_asr_task_append_during_run|TIMEOUT.*test_asr_task_startup_recovery' \
     /tmp/job-78509089317.log
   ```
4. 使用已构建的 release binary 分别执行三条真实链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_history_pagination_api.sh
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" BIFROST_ASR_TASK_APPEND_E2E_PORT=19131 bash e2e-tests/tests/test_asr_task_append_during_run.sh
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" BIFROST_ASR_TASK_RECOVERY_E2E_PORT=19132 bash e2e-tests/tests/test_asr_task_startup_recovery.sh
   ```

**预期结果**：
- 三个脚本语法检查通过。
- 静态检查显示 `SKIP_BUILD=true` 时默认使用 `target/release/bifrost`，本地构建分支仍使用 `target/debug/bifrost`，且 binary 不可执行时有明确错误。
- 三条真实链路输出 `skipping build, using .../target/release/bifrost`，不会进入 `cargo build`，不会等待 Cargo build directory/file lock。
- `test_agent_history_pagination_api.sh` 输出 `agent history pagination API checks passed`。
- `test_asr_task_append_during_run.sh` 输出 `[asr-task-append] PASS`。
- `test_asr_task_startup_recovery.sh` 输出 `[asr-task-startup-recovery] PASS`。
- 全程使用临时数据目录、`--no-system-proxy` 与非 9900 端口。

### TC-CS-33: ASR shell E2E 消费并行调度器 ADMIN_PORT 回归

**操作步骤**：
1. 对 ASR CLI shell E2E 脚本执行语法检查：
   ```bash
   bash -n \
     e2e-tests/tests/test_asr_task_cli.sh \
     e2e-tests/tests/test_asr_diarization_cli.sh \
     e2e-tests/tests/test_asr_model_autonomy.sh \
     e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh
   ```
2. 静态检查这些脚本的端口优先级：
   ```bash
   rg -n 'ADMIN_PORT="\$\{BIFROST_ASR_[A-Z_]+_E2E_PORT:-\$\{ADMIN_PORT:-18[0-9]+' \
     e2e-tests/tests/test_asr_task_cli.sh \
     e2e-tests/tests/test_asr_diarization_cli.sh \
     e2e-tests/tests/test_asr_model_autonomy.sh \
     e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh
   ```
3. 使用 CI 风格的 `SKIP_BUILD=true`、`BIFROST_BIN` 和外层 `ADMIN_PORT` 执行声纹录入回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=19141 \
     bash e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh
   ```
4. 复核失败 artifact 中原始症状：
   ```bash
   rg -n 'Failed to connect to Bifrost admin API|Invalid argument' \
     /tmp/bifrost-ci-26643081691-mac-shard2/.e2e-reports/shell_test_asr_voiceprint_enroll_cli_sh.log
   ```

**预期结果**：
- 语法检查通过。
- 静态检查显示四个 ASR 脚本均优先使用显式 `BIFROST_ASR_*_E2E_PORT`，其次使用外层并行调度器传入的 `ADMIN_PORT`，最后才使用本地默认固定端口。
- 声纹录入真实链路输出 `[asr-voiceprint-enroll-cli-e2e] ok`，证明 `ADMIN_PORT=19141` 生效且不再固定到 `18994`。
- 原始 artifact 复核能定位失败为固定端口场景下 CLI finish enrollment 无法连接 admin API，而不是声纹识别准确性或业务断言失败。

### TC-CS-34: ASR diarization CLI E2E 默认不依赖公网模型下载回归

**操作步骤**：
1. 对 diarization CLI shell E2E 执行语法检查：
   ```bash
   bash -n e2e-tests/tests/test_asr_diarization_cli.sh
   ```
2. 静态检查脚本默认离线分支与显式联网开关：
   ```bash
   rg -n 'BIFROST_ASR_DIARIZATION_E2E_ONLINE|skipping online model init|diarization_ready' \
     e2e-tests/tests/test_asr_diarization_cli.sh
   ```
3. 复核 GitHub Actions 失败 artifact 中原始根因：
   ```bash
   rg -n 'status code 429|download diarization model' \
     /tmp/bifrost-ci-26896054036-shard3/.e2e-reports/shell_test_asr_diarization_cli_sh.log
   ```
4. 使用 CI 风格的 `SKIP_BUILD=true`、`BIFROST_BIN` 和外层 `ADMIN_PORT` 执行默认离线路径：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=19143 \
     bash e2e-tests/tests/test_asr_diarization_cli.sh
   ```

**预期结果**：
- 语法检查通过。
- 静态检查显示脚本只有在 `BIFROST_ASR_DIARIZATION_E2E_ONLINE=1` 时才执行 `ai asr diarization init` 的公网模型下载；默认 CI 路径会输出 `skipping online model init`。
- artifact 复核能定位原始失败为 HuggingFace 模型下载 `status code 429`，不是产品断言失败。
- 默认离线路径真实启动 Bifrost、调用 CLI status、创建启用 diarization 的 ASR 任务，并断言 `summary.diarization_enabled=true`、`summary.diarization_ready=false`。
- 全程使用临时数据目录、`--no-system-proxy` 与非 9900 端口。

### TC-CS-35: macOS shell shard 资源压力与 traffic DB mock 日志回归

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`，确认 Linux shell shard 仍为 4 路并发、macOS shell shard 降为 2 路并发：
   ```bash
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); linux = workflow["jobs"]["e2e-shell"]["env"]["BIFROST_E2E_SHELL_JOBS"]; mac = workflow["jobs"]["e2e-macos-shell"]["env"]["BIFROST_E2E_SHELL_JOBS"]; raise "linux jobs mismatch: #{linux.inspect}" unless linux == "4"; raise "mac jobs mismatch: #{mac.inspect}" unless mac == "2"; puts "linux shell jobs=4, mac shell jobs=2"'
   ```
2. 对 traffic DB shell E2E 执行语法检查：
   ```bash
   bash -n e2e-tests/tests/test_traffic_db_e2e.sh
   ```
3. 静态检查 traffic DB mock server 日志会复制到 report dir：
   ```bash
   rg -n 'traffic-db-mock-|BIFROST_E2E_REPORT_DIR|MOCK_LOG_FILE' \
     e2e-tests/tests/test_traffic_db_e2e.sh
   ```
4. 复核 GitHub Actions 失败 artifact 中原始根因：
   ```bash
   rg -n 'Killed: 9|Could not start mock server' \
     /tmp/bifrost-ci-26899628938-mac-shard1/.e2e-reports
   ```

**预期结果**：
- YAML 解析通过，Linux `e2e-shell` 保持 `BIFROST_E2E_SHELL_JOBS=4`，macOS `e2e-macos-shell` 为 `2`，降低 macOS runner 同时启动 Bifrost/mock 的资源峰值。
- `test_traffic_db_e2e.sh` 语法检查通过。
- 静态检查显示 mock server 临时日志会复制到 `$BIFROST_E2E_REPORT_DIR/traffic-db-mock-<port>.log`，失败 artifact 可保留 mock 退出原因。
- artifact 复核能定位原始失败为 macOS shard 1 中多个 Bifrost 进程被系统 `Killed: 9`，且 traffic DB 用例只留下 `Could not start mock server`，缺少 mock 详细日志。
- 静态回归不启动 Bifrost、不使用 9900、不修改系统代理。

### TC-CS-36: Agent history continue mock server 动态端口回传回归

**操作步骤**：
1. 对 history continue shell E2E 脚本执行语法检查：
   ```bash
   bash -n e2e-tests/tests/test_agent_chat_history_continue.sh
   ```
2. 静态检查脚本消费 CI shell 调度器端口，并由 Python mock server 绑定后回传真实端口：
   ```bash
   rg -n 'BIFROST_PORT=.*ADMIN_PORT|PROXY_PORT|MOCK_PORT_FILE|server_address|requested_port = .* if sys.argv\[1\] else 0' \
     e2e-tests/tests/test_agent_chat_history_continue.sh
   ```
3. 使用已构建的 release binary 和外层注入端口真实执行 history continue 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" \
     ADMIN_PORT=18131 PROXY_PORT=18131 \
     bash e2e-tests/tests/test_agent_chat_history_continue.sh
   ```
4. 复核本轮 CI 失败 artifact 原始根因：
   ```bash
   rg -n 'test_agent_chat_history_continue.sh|REQUEST_CONNECT_REFUSED|chat/completions|history-continue' \
     /tmp/bifrost-ci-79345071063.log
   ```

**预期结果**：
- 语法检查通过。
- 静态检查显示脚本优先使用 `ADMIN_PORT` / `PROXY_PORT` 作为 Bifrost 端口，不再绕过 shell 调度器端口隔离。
- 静态检查显示 mock server 在 Python 进程中以 `requested_port=0` 绑定时通过 `server.server_address[1]` 写回实际端口，脚本再用该端口配置 Agent `base_url`。
- 真实执行输出 `[agent-chat-history-continue] PASS`，验证压缩后的历史恢复、计划恢复、续聊写回与外部 history path 拒绝。
- 原始 artifact 复核能定位旧失败为 mock `/chat/completions` 连接拒绝，而不是 history 恢复业务断言失败。

### TC-CS-38: macOS shell shard mock 启动等待回归

**操作步骤**：
1. 对本轮失败相关脚本执行语法检查：
   ```bash
   bash -n e2e-tests/tests/test_weixin_provider_e2e.sh e2e-tests/tests/test_total_size_cleanup_admin_api.sh
   ```
2. 静态检查 Weixin mock server 会等待端口文件、检测子进程提前退出，并在超时时输出明确诊断：
   ```bash
   rg -n 'MOCK_READY|mock server exited before writing port file|mock server did not write port file|seq 1 200' \
     e2e-tests/tests/test_weixin_provider_e2e.sh
   ```
3. 静态检查 total size cleanup mock server ready 等待预算提升到 30 秒：
   ```bash
   rg -n 'while \[ \$waited -lt 60 \]|not ready after 30s' \
     e2e-tests/tests/test_total_size_cleanup_admin_api.sh
   ```
4. 使用临时数据目录与本地端口真实执行两个失败相关脚本：
   ```bash
   BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     bash e2e-tests/tests/test_weixin_provider_e2e.sh
   BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     bash e2e-tests/tests/test_total_size_cleanup_admin_api.sh
   ```
5. 复核 GitHub Actions 失败日志原始根因：
   ```bash
   GH_REPO=bifrost-proxy/bifrost gh run view 27004966405 --job 79697959967 --log \
     | rg -n 'test_weixin_provider_e2e.sh|weixin_mock_port|test_total_size_cleanup_admin_api.sh|Mock server on port'
   ```

**预期结果**：
- 语法检查通过。
- Weixin provider E2E 不再在端口文件缺失时直接 `cat` 失败，而是最多等待 20 秒，并在 mock 子进程退出或超时时给出可诊断错误。
- total size cleanup E2E 在 macOS CI 资源压力下给 mock server 30 秒 ready 窗口，降低 Python mock 启动慢导致的假失败。
- 两个脚本本地真实执行通过，均使用临时数据目录、`--no-system-proxy` 和非 9900 端口。
- 原始 CI 失败日志能定位旧问题为 `weixin_mock_port: No such file or directory` 和 `Mock server on port ... not ready after 10s`，不是 `statusCode` 规则功能回归。

### TC-CS-37: CI 模式不收集 ASR/voice runtime shell E2E，本地 full-shell 保留

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-shell.sh
   ```
2. 检查 CI full-shell 列表不包含 ASR/voice runtime 相关脚本：
   ```bash
   bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg -n 'test_asr_|test_qwen3_asr_|test_voice_input_runtime\.sh|test_voice_wake_actions\.sh'
   ```
3. 检查本地 full-shell 列表仍保留代表性 ASR/voice runtime 脚本：
   ```bash
   bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg -n 'test_asr_diarization_cli\.sh|test_qwen3_asr_local_server\.sh|test_voice_input_runtime\.sh|test_asr_voiceprint_enroll_cli\.sh'
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步 `rg` 无输出且退出码为 1，表示 CI 模式不会收集 ASR 解码、模型初始化、声纹/diarization、Qwen3 ASR 或 voice runtime shell E2E。
- 第 3 步能输出代表性 ASR/voice runtime 脚本，表示这些能力仍可在本地 full-shell 真实验证。
- 三个命令都只列出或检查脚本，不启动 Bifrost、不下载模型、不访问外部模型源、不使用 9900、不修改系统代理。

### TC-CS-39: temporary port listener 端口竞态重试回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_temporary_port_bindings.sh
   ```
2. 使用预构建 binary 执行真实 temporary port 绑定回归：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_temporary_port_bindings.sh
   ```

**预期结果**：
- 脚本语法检查通过。
- `port bind --port <port>` 在目标端口被并发进程抢占并返回 `another process is already listening` 时，会重新分配测试端口并有限重试。
- 成功路径下 temporary port 绑定顺序、rule-file、inline rule、update、Traffic API/CLI listener port 断言保持不变。
- 汇总为 `Passed: 55`、`Failed: 0`，退出码为 0。

### TC-CS-40: shell E2E 清理阶段不因 `Killed` 误判失败

**操作步骤**：
1. 执行共享清理 helper 与代表性 shell 用例语法检查：
   ```bash
   bash -n e2e-tests/test_utils/process.sh \
     e2e-tests/tests/test_metrics_hosts_apps_admin_api.sh \
     e2e-tests/tests/test_rule_semantics_regressions.sh \
     e2e-tests/tests/test_proxy_chain_auth_e2e.sh \
     e2e-tests/tests/test_host_rule_path_rewrite.sh \
     e2e-tests/tests/test_multiline_rule_filter_e2e.sh
   ```
2. 使用预构建 binary 分别执行上述真实 shell 用例，所有 Bifrost 启动都使用临时数据目录、非 9900 端口和 `--no-system-proxy`。

**预期结果**：
- 脚本语法检查通过。
- 代表性用例断言全部通过，退出码为 0。
- EXIT trap 清理 Bifrost 后台进程时优先 graceful stop 并 wait，不再输出 `Killed ...`，避免 Linux shell shard 把已通过用例误判为失败。

### TC-CS-41: large body shell 用例按资源敏感路径串行调度

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh e2e-tests/tests/test_large_body_protection.sh
   ```
2. 静态检查 large body 用例被登记为 resource-heavy 串行用例，且用例数据目录复用调度器注入的 `BIFROST_DATA_DIR`：
   ```bash
   rg -n 'RESOURCE_HEAVY_TESTS|test_large_body_protection\.sh|is_resource_heavy|BIFROST_DATA_DIR:-\$PROJECT_DIR/\.bifrost-test-large-body' scripts/run_all_e2e.sh e2e-tests/tests/test_large_body_protection.sh
   ```
3. 使用隔离端口、隔离数据目录和预构建 release binary 执行 large body 用例：
   ```bash
   TEST_ROOT="$(mktemp -d /tmp/bifrost-large-body-human.XXXXXX)"
   PROXY_PORT=19214 ECHO_HTTP_PORT=19215 \
     BIFROST_DATA_DIR="$TEST_ROOT/data" \
     SKIP_BUILD=true BIFROST_BIN=target/release/bifrost \
     TIMEOUT=90 BIFROST_E2E_HTTP_RETRIES=2 \
     bash e2e-tests/tests/test_large_body_protection.sh
   ```

**预期结果**：
- 语法检查退出码为 0。
- 第 2 步能定位到 `RESOURCE_HEAVY_TESTS`、`test_large_body_protection.sh`、`is_resource_heavy` 和 `BIFROST_DATA_DIR` fallback。
- `test_large_body_protection.sh` 不再固定写入仓库根目录 `.bifrost-test-large-body`，在 CI shell 调度器中会使用每个 suite 的 sandbox 数据目录。
- large body 用例作为资源敏感测试进入串行队列，不与其他 shell 用例并发竞争 macOS hosted runner 内存和代理连接资源。
- 真实用例输出所有 HTTP large body case 通过，退出码为 0。

### TC-CS-42: Linux shell 单 job 与 macOS shell 2 分片预算回归

**背景**：GitHub Actions `CI` run `27429681824` 中 `E2E Shell (Linux, shard 1/4)` 的 35 个子日志均显示 PASS 或预期 SKIP，但 job 在 shell 调度器内部预算附近被判定 timeout。后续曾将 Linux/macOS shell E2E 调整为 6 分片；但过度分片会占用大量排队资源。因此 Linux shell E2E 合并为单个 job，macOS shell E2E 合并为 2 个分片。

**操作步骤**：
1. 解析 workflow YAML，确认 Linux shell E2E 没有 matrix/shard 环境变量，macOS shell E2E 使用 2 分片：
   ```bash
   ruby -e 'require "yaml"; y=YAML.load_file(".github/workflows/ci.yml"); linux=y["jobs"]["e2e-shell"]; mac=y["jobs"]["e2e-macos-shell"]; raise "linux matrix" if linux.key?("strategy"); raise "linux shard env" if linux["env"].key?("BIFROST_E2E_SHARD_INDEX") || linux["env"].key?("BIFROST_E2E_SHARD_TOTAL"); raise "linux name" unless linux["name"] == "E2E Shell (Linux)"; raise "mac shards" unless mac["strategy"]["matrix"]["shard"] == [1,2] && mac["env"]["BIFROST_E2E_SHARD_TOTAL"] == "2"; puts "shell layout ok"'
   ```
2. 静态列出 Linux 单 job 将执行的 CI shell tests 总数，不启动 Bifrost：
   ```bash
   bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | sed '/^$/d' | wc -l | tr -d ' '
   ```
3. 静态列出 macOS 继续使用的 2 分片数量，不启动 Bifrost：
   ```bash
   for i in 1 2; do
     count=$(bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests --shard "$i/2" | sed '/^$/d' | wc -l | tr -d ' ')
     echo "$i/2 $count"
   done
   ```

**预期结果**：
- workflow YAML 解析输出 `shell layout ok`。
- Linux 单 job 列表数量等于 133 个 CI shell tests。
- macOS 2 分片数量为 `67/66`，总和等于 133 个 CI shell tests。
- 上述命令仅静态列出测试，不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-43: CI shell E2E 不收集纯 cargo contract 脚本

**背景**：Mac shell E2E job 中曾出现额外 Rust 编译，根因为部分 `e2e-tests/tests/test_*.sh` 实际只包装 `cargo check` / `cargo test` / `cargo run`，并不验证 shell/CLI/API 端到端链路。这类 contract 已由 Rust unit/integration job 覆盖，不应进入 CI shell E2E。

**操作步骤**：
1. 静态确认 CI shell 列表不包含纯 cargo contract 脚本：
   ```bash
   for script in test_agent_codex_parity_contracts.sh test_im_agent_markdown_image_reply.sh test_im_agent_streaming_progress_card.sh test_utf8_safe_preview_e2e.sh; do
     if bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -q "^${script}$"; then
       echo "present $script"
       exit 1
     else
       echo "skipped $script"
     fi
   done
   ```
2. 确认上述脚本仍保留在本地 full-shell 列表中：
   ```bash
   for script in test_agent_codex_parity_contracts.sh test_im_agent_markdown_image_reply.sh test_im_agent_streaming_progress_card.sh test_utf8_safe_preview_e2e.sh; do
     bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -q "^${script}$"
   done
   ```

**预期结果**：
- 第 1 步输出 4 行 `skipped ...`，退出码为 0。
- 第 2 步退出码为 0，说明这些本地回归脚本没有删除，只是不进入 CI shell E2E。
- 上述命令仅静态列出测试，不启动 Bifrost，不使用 9900，不修改系统代理。

### TC-CS-44: Proxy chain shell E2E 本地下游动态端口回归

**背景**：GitHub Actions macOS shell shard 曾在 `test_proxy_chain_auth_e2e.sh` 中出现 `双代理链路请求成功` 断言失败，期望 2xx 但实际返回 502。该用例不应访问公网或依赖 CI runner 的外部网络；入口 Bifrost、上游 Bifrost、HTTP echo 和 proxy echo 都必须是本机临时服务。CI 和本机差异主要来自本机端口/进程并发，而不是公网可达性。

**操作步骤**：
1. 语法检查 proxy chain shell E2E：
   ```bash
   bash -n e2e-tests/tests/test_proxy_chain_auth_e2e.sh
   ```
2. 静态确认脚本不包含公网测试地址，且使用动态端口 helper：
   ```bash
   rg -n 'httpbin|example\\.com|echo\\.websocket|pick_available_base_port|127\\.0\\.0\\.1' \
     e2e-tests/tests/test_proxy_chain_auth_e2e.sh \
     e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt \
     e2e-tests/rules/forwarding/proxy_chain_upstream_host.txt
   ```
3. 使用已构建的 release binary 真实执行 proxy chain 回归：
   ```bash
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步只显示本地 mock 地址、规则占位符和 `pick_available_base_port`；不出现公网依赖。
- 第 3 步输出 HTTP echo、proxy echo、上游 Bifrost、入口 Bifrost ready；双 Bifrost 代理链路返回 2xx；下游代理鉴权返回 2xx，并断言 absolute-form URL 与 `Proxy-Authorization: Basic dXNlcjpwYXNz`；汇总 `Total: 11 / Passed: 11 / Failed: 0`。
- 如果链路返回非 2xx，脚本输出状态码、响应头、响应体，以及 entry/upstream/mock 日志尾部，方便一次性定位本地下游不可用、端口碰撞或产品回归。

### TC-CS-45: SOCKS5 TLS routing exceptions 下游代理端口不撞 SOCKS5 端口

**背景**：GitHub Actions Linux shell E2E run `27924364360` 中 `test_socks5_tls_routing_exceptions.sh` 失败，runner 分配 `PROXY_PORT=19373`、`SOCKS5_PORT=19379`，脚本内旧的 `DOWNSTREAM_PROXY_PORT=18890 + ($$ % 500)` 在 PID `36489` 下同样算成 `19379`，导致下游代理与独立 SOCKS5 listener 端口碰撞并报 `Address already in use`。脚本必须优先消费外层 runner 注入的 `ECHO_PROXY_PORT` / `MOCK_ECHO_PROXY_PORT`，而不是用 PID 派生端口。

**操作步骤**：
1. 语法检查脚本：
   ```bash
   bash -n e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```
2. 静态确认 `DOWNSTREAM_PROXY_PORT` 默认值优先使用 runner 注入端口：
   ```bash
   rg -n 'DOWNSTREAM_PROXY_PORT=.*ECHO_PROXY_PORT.*MOCK_ECHO_PROXY_PORT.*PROXY_PORT \\+ 7' \
     e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```
3. 使用 CI 失败时的相邻端口形态真实执行脚本：
   ```bash
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   SKIP_BUILD=true \
   BIFROST_BIN="$PWD/target/release/bifrost" \
   PROXY_PORT=19373 \
   SOCKS5_PORT=19379 \
   ECHO_PROXY_PORT=19380 \
   MOCK_ECHO_PROXY_PORT=19380 \
   ECHO_HTTP_PORT=19374 \
   ECHO_HTTPS_PORT=19375 \
   bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步能定位到 `DOWNSTREAM_PROXY_PORT` 的默认值链路。
- 第 3 步上游 Bifrost 使用 `PROXY_PORT=19373` 和 `SOCKS5_PORT=19379`；下游 Bifrost 使用 `ECHO_PROXY_PORT=19380`，不再尝试绑定 `19379`；脚本所有断言通过并清理临时进程。

### TC-CS-46: 重型低频 shell 用例跳出默认 PR CI 且本地 full-shell 保留

**背景**：GitHub Actions macOS shell shard 近期日志显示 `test_asr_admin_csrf.sh` 耗时约 583s，`test_chatgpt_web_shared_profile.sh` 耗时约 879s。前者在 shell 脚本内执行 Web unit test、重新构建 debug bifrost 并跑 Admin cross-site 安全链路；后者是 shell 包装的 Rust 单测。二者都属于低频模块专项回归，不应占用每次 PR 的默认 shell shard 预算。

**操作步骤**：
1. 语法检查 shell E2E 调度器：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
2. 验证默认 CI shell 列表不再收集这两个重型用例：
   ```bash
   bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg '^(test_asr_admin_csrf|test_chatgpt_web_shared_profile)\.sh$'
   ```
3. 验证本地 full-shell 仍保留这两个用例，供 ASR/Admin CSRF 或 ChatGPT Web profile 相关修改专项执行：
   ```bash
   for script in test_asr_admin_csrf.sh test_chatgpt_web_shared_profile.sh; do
     bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
       | rg -q "^${script}$"
   done
   ```
4. 验证 shell CI coverage guard 仍通过，确认被跳出的脚本已显式登记而不是遗漏：
   ```bash
   bash scripts/ci/check-e2e-shell-ci-coverage.sh
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步 `rg` 无匹配并返回 1，表示默认 PR CI shell shard 不再运行这两个重型低频脚本。
- 第 3 步两个脚本均能在本地 full-shell 列表中找到，说明专项回归入口没有删除。
- 第 4 步输出 `OK: every test_*.sh shell E2E script is selected by CI or explicitly skipped.`。
- 上述命令均为列表或静态校验，不启动 Bifrost、不使用 9900、不修改系统代理。

### TC-CS-47: 安全聚合 wrapper 跳出默认 PR shell CI 且功能子路径仍保留

**背景**：GitHub Actions run `28520282947` 的 macOS shell shard 1 显示 `test_security_hardening.sh` 在 902s 被 per-test timeout 杀掉。该脚本是聚合 wrapper，会重复执行多个 Cargo unit filter、installer shell、sync relay Jest、Web build 和功能 wrapper；默认 PR CI 已通过专门 unit/integration、coverage、Web build、E2E runner 与 `test_security_hardening_functional.sh` 覆盖这些组成路径。聚合 wrapper 应保留为本地 full-shell / release-gate，不占用每次 PR shell shard 预算。

**操作步骤**：
1. 语法检查 shell E2E 调度器：
   ```bash
   bash -n scripts/run_all_e2e.sh
   ```
2. 验证默认 CI shell 列表不再收集聚合 wrapper：
   ```bash
   bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg '^test_security_hardening\.sh$'
   ```
3. 验证安全功能 shell 子路径仍在默认 CI shell 列表中：
   ```bash
   bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg -q '^test_security_hardening_functional\.sh$'
   ```
4. 验证本地 full-shell 仍保留聚合 wrapper：
   ```bash
   bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
     | rg -q '^test_security_hardening\.sh$'
   ```
5. 验证 shell CI coverage guard 仍通过：
   ```bash
   bash scripts/ci/check-e2e-shell-ci-coverage.sh
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步 `rg` 无匹配并返回 1，表示默认 PR CI shell shard 不再运行安全聚合 wrapper。
- 第 3 步退出码为 0，表示功能子路径 `test_security_hardening_functional.sh` 仍在默认 CI shell 覆盖中。
- 第 4 步退出码为 0，表示本地 full-shell / release-gate 仍可运行聚合 wrapper。
- 第 5 步输出 `OK: every test_*.sh shell E2E script is selected by CI or explicitly skipped.`。
- 上述命令均为列表或静态校验，不启动 Bifrost、不使用 9900、不修改系统代理。

### TC-CS-48: macOS shell 用例清理阶段不因临时目录短暂非空变红

**背景**：GitHub Actions run `28751216421` 的 `E2E Shell (aarch64-apple-darwin, shard 2/2)` 中，`test_stop_restart_shutdown_marker.sh` 的 14 个业务断言全部通过，但退出 trap 执行 `rm -rf <tmp>` 时 macOS 短暂返回 `Directory not empty`，在 `set -e` 下把 suite 标记为失败。清理失败不应覆盖已经通过的 stop/restart/system proxy handoff 回归结论。

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n e2e-tests/tests/test_stop_restart_shutdown_marker.sh
   ```
2. 使用当前 release/debug binary 执行该 focused shell E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_stop_restart_shutdown_marker.sh
   ```
   如果本地没有 release binary，可先执行 `cargo build -p bifrost-cli` 并改用 `BIFROST_BIN="$PWD/target/debug/bifrost"`。
3. 检查输出的 `Test Summary`。

**预期结果**：
- 脚本语法检查通过。
- 业务断言保持 `Total: 14 / Passed: 14 / Failed: 0`。
- 即使 macOS daemon/log writer 在退出瞬间仍持有临时目录，cleanup 会 stop daemon、重试删除并 best-effort 收尾，不再因 `rm: ... Directory not empty` 把脚本退出码改为 1。
- 全程使用临时数据目录和随机端口，不使用 9900，不修改真实系统代理。

### TC-CS-49: macOS 两个 shell shard 权重均衡回归

**操作步骤**：
1. 执行脚本语法检查：
   ```bash
   bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-shell.sh
   ```
2. 检查 CI full-shell 总列表与两个 macOS shard 的覆盖关系：
   ```bash
   all="$(mktemp)"
   s1="$(mktemp)"
   s2="$(mktemp)"
   BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | sort > "$all"
   BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/2 --list-shell-tests | sort > "$s1"
   BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 2/2 --list-shell-tests | sort > "$s2"
   comm -12 "$s1" "$s2"
   cat "$s1" "$s2" | sort > "$all.sharded"
   diff -u "$all" "$all.sharded"
   rm -f "$all" "$s1" "$s2" "$all.sharded"
   ```
3. 执行 20% 预计墙钟误差门禁：
   ```bash
   BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/2 --check-shell-shard-balance
   ```
4. 检查实测长尾脚本已有非默认权重：
   ```bash
   rg -n 'test_long_term_memory_remember_recall\.sh\) echo 529|test_desktop_open_requests_contract\.sh\) echo 486|test_chatgpt_web_behavior_artifacts\.sh\) echo 243|test_im_gateway_long_reply_delivery_regression\.sh\) echo 142|test_skill_creator_flow\.sh\) echo 102' scripts/run_all_e2e.sh
   ```

**预期结果**：
- 第 1 步语法检查通过。
- 第 2 步 `comm -12` 无输出，说明两个 shard 没有重复执行同一个 shell 脚本；`diff -u` 无输出，说明 shard 1/2 + shard 2/2 完整覆盖 CI full-shell 列表。
- 第 3 步输出两个 shard 的 `estimated_wall`、串行耗时、并发 lane 耗时和测试数量，`pct_of_avg` 小于等于 `20.0%`，命令退出码为 0；当前期望为 `shard 1/2 estimated_wall=1398s serial=941s parallel_lanes=457,456 tests=76`、`shard 2/2 estimated_wall=1397s serial=526s parallel_lanes=871,866 tests=84`、`pct_of_avg=0.1%`。
- 第 4 步能定位到近期 GitHub Actions 实测长尾脚本的非默认权重，避免它们继续按默认 8 秒分配到同一个慢 shard。
- 该回归只列出和静态校验 shell 测试，不启动 Bifrost、不使用 9900、不修改系统代理。

### TC-CS-51: Windows unit external runner stdin timeout 回归

**操作步骤**：
1. 运行 `cargo test -p bifrost-admin schedule_agent_adapter_config_overrides_runner_without_dropping_command -- --nocapture`。
2. 推送修复分支后查看 GitHub Actions `CI` 的 `Windows Unit Tests (x86_64)` job。

**预期结果**：
- 第 1 步通过，`schedule_agent_adapter_config_overrides_runner_without_dropping_command` 返回 `TaskRunStatus::Success`，`agent_final_response` 为 `OVERRIDE_OK`，不会触发 schedule 外层 10s timeout。
- 第 2 步 Windows 单测 job 通过，不再出现 `timeout after 10000ms` 或该测试的 panic。
- 该回归不启动 Bifrost，不使用 9900，不修改系统代理。

## 本轮执行记录

测试日期：2026-05-09

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-CS-10 | 通过 | `rg -n "include-hidden-files: true" .github/workflows/ci.yml \| wc -l` 输出 `8`；`bash scripts/run_all_e2e.sh --extract-failure-reason "$TMP_LOG"` 输出 `browserType.launch: Host system is missing dependencies`；本地执行 `source ~/.zshrc && BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`，24/24 通过，`test_devtools_page_bridge_api.sh` 在并行 shard 3 中 44s 通过。 |
| TC-CS-11 | 通过 | 2026-04-30 本轮执行：`bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；`rg -n 'grep\s+(-[^ ]*)?q[^ ]*\s+"[^"]*\\\|' e2e-tests/tests/test_cli_offline_commands_e2e.sh` 无输出；`bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 汇总 `通过: 106`、`失败: 0`，其中 `rule rename --help`、`rule reorder --help`、`script rename --help` 均通过。 |
| TC-CS-12 | 通过 | 2026-05-01 本轮执行：`bash -n e2e-tests/tests/test_unsafe_ssl_e2e.sh e2e-tests/test_utils/admin_client.sh` 通过；随后使用 `TEST_ROOT="$(mktemp -d /tmp/bifrost-unsafe-ssl-human.XXXXXX)" PROXY_PORT=11295 ADMIN_PORT=11295 HTTPS_MOCK_PORT=11297 BIFROST_DATA_DIR="$TEST_ROOT/data" SERVER_LOG_DIR="$TEST_ROOT/logs" SKIP_BUILD=true bash e2e-tests/tests/test_unsafe_ssl_e2e.sh` 执行真实场景。脚本输出 `Starting HTTPS mock server on 127.0.0.1:11297`、`HTTPS mock server ready`、`Created unsafe_ssl forwarding rule to https://127.0.0.1:11297`，并完成 unsafe_ssl false/true/false 三段代理请求，汇总 `Results: 5/5 passed`，退出码 0；全程使用临时目录和 11295/11297，未使用 9900。 |
| TC-CS-12 | 通过 | 2026-06-08 追加执行：`SKIP_BUILD=true PROXY_PORT=20291 ADMIN_PORT=20291 HTTPS_MOCK_PORT=20293 BIFROST_DATA_DIR=<临时目录> SERVER_LOG_DIR=<临时目录> bash e2e-tests/tests/test_unsafe_ssl_e2e.sh`，本机 20293 被非 mock trust-probe 服务占用时，脚本输出 `Selected alternate HTTPS mock port 60671 because requested port was occupied by a non-mock service`，随后启动自有 HTTPS echo mock，5/5 通过。 |
| TC-CS-13 | 通过 | 2026-05-03 本轮执行：`bash -n scripts/run_all_e2e.sh` 通过；`rg -n 'run_shell_tests_parallel\(\)\|run_shell_batch_parallel\(\)\|return 0' scripts/run_all_e2e.sh` 显示两个调度函数及其显式 `return 0`。完整 shard 3 本机执行卡在大端口段扫描前置探针，随后改用同一入口的最小 shard 验证返回码路径：`BIFROST_UI_TEST_RUNNER_PORT=18080 BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=999 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh` 选中 `test_body_cache_sync_cleanup_admin_api.sh`，输出 `[PASS] shell:test_body_cache_sync_cleanup_admin_api.sh`，最终 `Total suites : 1 / Passed : 1 / Failed : 0`，外层退出码 0。完整 macOS shard 3 由推送后的 GitHub Actions 继续验证。 |
| TC-CS-14 | 通过 | 2026-05-03 本轮执行：`bash -n e2e-tests/tests/test_replay_rules.sh` 通过；`rg -n 'sse/custom\?count=20&interval=0\.5\|"timeout_ms":5000\|>=8s alive\|kept alive beyond timeout_ms' e2e-tests/tests/test_replay_rules.sh` 显示 4 个预期匹配；随后使用 `TEST_ROOT="$(mktemp -d /tmp/bifrost-replay-human.XXXXXX)" PROXY_PORT=18881 MOCK_HTTP_PORT=18882 MOCK_SSE_PORT=18883 MOCK_WS_PORT=18884 BIFROST_DATA_DIR="$TEST_ROOT/data" SERVER_LOG_DIR="$TEST_ROOT/logs" SKIP_BUILD=true BIFROST_E2E_REPORT_DIR="$TEST_ROOT/reports" bash e2e-tests/tests/test_replay_rules.sh` 执行真实场景。`SSE Replay with Rules` 输出 `SSE Replay: connection event received and stream kept alive beyond timeout_ms`，全脚本汇总 `Passed: 21`、`Failed: 0`，退出码 0；全程使用临时目录和 18881-18884，未使用 9900。 |
| TC-CS-15 | 通过 | 2026-05-03 本轮执行：`bash -n e2e-tests/test_utils/admin_client.sh e2e-tests/tests/test_unsafe_ssl_e2e.sh` 通过；`rg -n 'admin_probe_existing_bifrost\|admin_is_bifrost_admin_response\|/api/auth/status\|Bifrost admin API not available' e2e-tests/test_utils/admin_client.sh e2e-tests/tests/test_unsafe_ssl_e2e.sh` 显示管理端响应结构校验逻辑。随后启动非 Bifrost HTTP 服务占用 `127.0.0.1:18885`，服务对 `/_bifrost/api/auth/status` 返回 OpenAI-like JSON，执行 `ADMIN_PORT=18885 ADMIN_HOST=127.0.0.1 ADMIN_PATH_PREFIX=/_bifrost bash -c 'source e2e-tests/test_utils/admin_client.sh; if admin_probe_existing_bifrost; then exit 1; else exit 0; fi'` 退出码 0，确认 helper 不会误复用错误服务。最后使用 `PROXY_PORT=18886 ADMIN_PORT=18886 HTTPS_MOCK_PORT=18887 BIFROST_DATA_DIR=<临时目录>/data SERVER_LOG_DIR=<临时目录>/logs SKIP_BUILD=true bash e2e-tests/tests/test_unsafe_ssl_e2e.sh` 执行完整 unsafe_ssl 场景，输出 `Created unsafe_ssl forwarding rule to https://127.0.0.1:18887` 和 `Results: 5/5 passed`，退出码 0；全程未使用 9900。 |
| TC-CS-16 | 通过 | 2026-05-03 本轮执行：`bash -n e2e-tests/tests/test_long_term_memory_human_api.sh` 通过；`rg -n 'SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost' e2e-tests/tests/test_long_term_memory_human_api.sh` 定位到构建命令。随后执行 `BIFROST_PORT=18888 MOCK_PORT=18889 bash e2e-tests/tests/test_long_term_memory_human_api.sh`，构建日志显示 `Skipping frontend build (SKIP_FRONTEND_BUILD is set)`，脚本完成三段独立 session 写入/读取长期记忆并输出 `[long-term-memory-human-api] PASS`，退出码 0；确认该用例在 CI 并行执行时不触发 frontend build。 |
| TC-CS-17 | 通过 | 2026-05-03 本轮执行：`bash -n e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh` 通过；`rg -n 'SKIP_BUILD\|BIFROST_BIN\|Using existing bifrost binary\|SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost' e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh` 显示复用已有 binary 的分支和 fallback build 命令。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`，输出 `Using existing bifrost binary: <REPO_ROOT>/target/release/bifrost`，未输出 `Build bifrost (release)...`，三段 relay 选择断言全部通过并输出 `All remote relay URL fallback assertions passed.`；测试端口动态分配，未使用 9900。 |
| TC-CS-18 | 通过 | 2026-05-04 本轮执行：`bash -n e2e-tests/tests/test_replay_rules.sh` 通过；`rg -n 'received post-timeout event before client disconnect\|"id":"\(1\[2-9\]\|\[2-9\]\[0-9\]\+\)"\|missing connection/applied_rules/post-timeout event' e2e-tests/tests/test_replay_rules.sh` 定位到 post-timeout 事件兜底断言和失败提示。随后执行 `PROXY_PORT=18891 MOCK_HTTP_PORT=18892 MOCK_SSE_PORT=18893 MOCK_WS_PORT=18894 BIFROST_DATA_DIR=/tmp/bifrost-replay-ci-noise-human.pV12r4/data SERVER_LOG_DIR=/tmp/bifrost-replay-ci-noise-human.pV12r4/logs SKIP_BUILD=true BIFROST_E2E_REPORT_DIR=/tmp/bifrost-replay-ci-noise-human.pV12r4/reports bash e2e-tests/tests/test_replay_rules.sh`，输出 `SSE Replay: connection event received and stream kept alive beyond timeout_ms`，全脚本汇总 `Passed: 21`、`Failed: 0`，退出码 0；测试端口 18891-18894，未使用 9900。 |
| TC-CS-19 | 通过 | 2026-05-06 本轮执行：Ruby YAML 标准库解析 `.github/workflows/ci.yml`，确认 `jobs.e2e-shell.timeout-minutes == 60` 且 `jobs.e2e-macos-shell.timeout-minutes == 60`；`rg -n 'e2e-shell:\|e2e-macos-shell:\|timeout-minutes: 60\|playwright install --with-deps chromium-headless-shell' .github/workflows/ci.yml` 定位到 Linux/macOS shell E2E job、60 分钟 timeout 和 Playwright `--with-deps` 安装步骤。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理。 |
| TC-CS-20 | 通过 | 2026-05-05 本轮执行：`bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；`rg -n 'echo "\$[A-Za-z_][A-Za-z0-9_]*" \| grep -[A-Za-z]+' e2e-tests/tests/test_cli_offline_commands_e2e.sh` 无输出；`SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 汇总 `通过: 106`、`失败: 0`，其中 `system-proxy enable --help` 正确显示且无 Broken pipe；Ruby 静态检查 `.github/workflows/ci.yml` 输出 `dump pipefail guards: 24`，确认 8 个失败日志 dump 步骤均对 `find \| head` 管道做容错。该回归未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-21 | 通过 | 2026-05-09 本轮执行：`bash -n e2e-tests/tests/test_agent_builtin_status_runtime.sh e2e-tests/tests/test_im_guide_queue_human_api.sh e2e-tests/tests/test_long_term_memory_human_api.sh e2e-tests/tests/test_update_plan_human_api.sh e2e-tests/tests/test_agent_loop_runtime_limits.sh` 通过；`rg -n 'BIFROST_PORT="\$\{BIFROST_PORT:-\$\{ADMIN_PORT:-\|MOCK_PORT="\$\{MOCK_PORT:-\$\{MOCK_HTTP_PORT:-' ...` 显示 5 个脚本均优先消费并行调度器端口；`rg -n 'SKIP_BUILD\|BIFROST_BIN\|skipping build, using' ...` 显示 5 个脚本均支持外层预构建 binary。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18111 MOCK_HTTP_PORT=18112 bash e2e-tests/tests/test_im_guide_queue_human_api.sh`，输出 `skipping build, using`、`starting bifrost on 18111` 与 `[im-guide-queue-human-api] PASS`；执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，输出 `skipping build, using`、`starting bifrost on 18121` 与 `[agent-builtin-status-runtime] PASS`。两条真实链路均使用临时数据目录、`--no-system-proxy` 与非 9900 端口，未复现端口碰撞或旧 Cargo 重新构建阻塞。2026-05-29 追加回归 CI `26646452401` Linux shard 1 暴露的隔离 worker active status 竞态：先执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost`，再执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" BIFROST_PORT=18897 MOCK_PORT=18898 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，输出 `[agent-builtin-status-runtime] PASS`，确认主进程预热 active status 后 `/status` 不再错过运行中窗口。 |
| TC-CS-22 | 通过 | 2026-05-07 本轮执行：Ruby YAML 标准库解析 `.github/workflows/ci.yml`，确认 `concurrency.group == "${{ github.workflow }}-${{ github.ref }}"` 且 `cancel-in-progress == true`；该静态回归不启动 Bifrost，不使用 9900，不修改系统代理。旧 run 取消和最新 run 获得执行权由推送后的 GitHub Actions `CI` run 验证。 |
| TC-CS-23 | 通过 | 2026-05-07 本轮执行：基于 GitHub Actions `CI` run `25469654203` 的 `E2E Shell (Linux, shard 2/3)` artifact，定位到多个 Bifrost 子进程被系统 `Killed`，符合 hosted runner 内存压力症状；随后 run `25470391707` 的 `E2E Shell (Linux, shard 3/3)` artifact 显示所有业务断言通过但仍有 Bifrost 子进程在 cleanup 中被系统 `Killed`，说明 8 路并发仍有资源峰值风险。2026-07-07 复核 Ruby YAML 解析 `.github/workflows/ci.yml`，确认 `e2e-shell` 的 `BIFROST_E2E_SHELL_JOBS == "4"`，`e2e-macos-shell` 的 `BIFROST_E2E_SHELL_JOBS == "2"`。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理。完整云端结果由推送后的 GitHub Actions `CI` run 验证。 |
| TC-CS-24 | 通过 | 2026-05-07 本轮执行：`tail -n 16 scripts/run_all_e2e.sh` 显示 final status 循环后存在显式 `exit 0`；`bash -n scripts/run_all_e2e.sh` 通过；随后使用 `BIFROST_UI_TEST_RUNNER_PORT=18080 BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=999 BIFROST_E2E_SHELL_JOBS=4 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh` 执行最小 shard，选中 `test_badge_injection_e2e.sh`，最终报告 `Total suites : 1`、`Passed : 1`、`Failed : 0`，外层退出码 0。该回归使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-25 | 通过 | 2026-05-09 本轮执行：`rg -n 'CARGO_BIN="\$\{CARGO_BIN:-\$\(resolve_non_shim_command cargo\)\}"' scripts/run_all_e2e.sh` 定位到默认 Cargo 解析逻辑；`bash -n scripts/run_all_e2e.sh` 通过；`which cargo` 输出 `/opt/homebrew/bin/cargo`；`CARGO_BIN="$(which cargo)" bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests` 只列出 shell tests，未构建、未启动 Bifrost、未使用 9900、未修改系统代理。随后完整本地 shell CI 由 `bash scripts/ci/local-ci.sh --skip-static --e2e-only shell` 验证。 |
| TC-CS-26 | 通过 | 2026-05-12 本轮执行：CI run `25725290679` 的 `E2E Shell (Linux, shard 3/3)` 失败日志显示 `sh: 1: astro: not found` 与 `Local package.json exists, but node_modules missing`，`E2E Shell (aarch64-apple-darwin, shard 3/3)` 同 shard 上传失败 artifact；修复后执行 `rm -rf site/node_modules` 模拟缺失依赖，`bash -n e2e-tests/tests/test_site_docs_sync.sh` 通过，随后使用本机可用新版 Node 执行 `PATH="/opt/homebrew/bin:$PATH" bash e2e-tests/tests/test_site_docs_sync.sh`，输出 `Installing site dependencies for docs sync E2E...`、`Docs sync verification passed for 27 docs pages.`、探针文档加入后的 `Docs sync verification passed for 28 docs pages.`、`Site link verification passed.`、`Site docs sync E2E passed.`，确认缺少 site 依赖时会自举安装并完成 Astro build。该回归未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-27 | 通过 | 2026-05-20 本轮执行：`bash -n` 覆盖 13 个本轮涉及的 shell E2E 脚本并通过；静态检查确认这些脚本中 `start --no-system-proxy` 的 Bifrost 启动窗口均包含 `--skip-cert-check`，且 `test_asr_task_cli.sh` 包含 `SKIP_BUILD=true` 复用 `target/release/bifrost` 的路径。随后使用 release binary 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_asr_task_cli.sh`，输出 `skipping build, using .../target/release/bifrost` 与 `[asr-task-cli-e2e] PASS`；执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_values_hot_reload.sh`，汇总 `Total: 5 / Passed: 5 / Failed: 0`；执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh`，汇总 `Total: 11 / Passed: 11 / Failed: 0`，包含 absolute-form 与 `Proxy-Authorization` 断言。全部真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-28 | 通过 | 2026-05-23 本轮执行：`bash -n e2e-tests/tests/test_agent_chat_history_continue.sh e2e-tests/tests/test_agent_direct_path_switch.sh` 通过；`rg -n 'SKIP_BUILD\|target/release/bifrost\|target/debug/bifrost\|skipping build, using\|bifrost binary not found' ...` 显示两个脚本均在 `SKIP_BUILD=true` 分支默认 release binary、本地构建分支默认 debug binary，并在 binary 不可执行时明确失败。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_chat_history_continue.sh`，输出 `skipping build, using .../target/release/bifrost` 与 `[agent-chat-history-continue] PASS`；执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_direct_path_switch.sh`，输出 `skipping build, using .../target/release/bifrost` 与 `[agent-direct-path-switch] PASS`。两条真实链路均使用临时数据目录、`--no-system-proxy` 与动态非 9900 端口，未再查找 `target/debug/bifrost`。 |
| TC-CS-29 | 通过 | 2026-05-26 本轮执行：`bash -n scripts/run_all_e2e.sh` 退出码 0；`rg -n 'CARGO_HEAVY_TESTS\|... \|serial_tests' scripts/run_all_e2e.sh` 显示 `test_agent_builtin_status_runtime.sh`、`test_agent_codex_parity_contracts.sh`、`test_agent_loop_runtime_limits.sh`、`test_asr_model_autonomy.sh`、`test_asr_task_pause_resume.sh`、`test_chatgpt_web_behavior_artifacts.sh`、`test_client_process_transport_attribution.sh`、`test_http3_e2e.sh`、`test_im_agent_markdown_image_reply.sh`、`test_im_agent_streaming_progress_card.sh`、`test_im_gateway_long_reply_delivery_regression.sh`、`test_long_term_memory_remember_recall.sh`、`test_qwen3_asr_local_server.sh`、`test_qwen3_asr_runtime_guards.sh`、`test_skill_creator_flow.sh`、`test_sync_login_direct_e2e.sh`、`test_utf8_safe_preview_e2e.sh`、`test_voice_input_runtime.sh` 等 Cargo-heavy 用例，以及 `is_cargo_heavy` 加入 `serial_tests` 的调度逻辑；`BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 scripts/run_all_e2e.sh --ci --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests` 显示失败相关用例仍在 shard 1 覆盖范围内；`grep -R -n 'Blocking.*file lock on artifact directory' /tmp/bifrost-ci-26451521064/... | head -20` 定位到 Linux/macOS shard 1 artifact 中多个 Cargo-heavy 用例等待 Cargo artifact lock 的原始症状。该回归未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-30 | 通过 | 2026-05-28 本轮执行：`bash -n e2e-tests/tests/test_http3_e2e.sh e2e-tests/tests/test_replay_body_decode.sh e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh scripts/run_all_e2e.sh` 通过；`rg -n 'httpbin\.org|echo\.websocket\.events' e2e-tests/tests/test_http3_e2e.sh e2e-tests/tests/test_replay_body_decode.sh e2e-tests/rules/http3/http3_e2e.txt` 无公网域名匹配。随后使用已有 debug binary 执行 `BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost SKIP_BUILD=true SKIP_CARGO_TEST=true PROXY_PORT=18991 ECHO_HTTP_PORT=18992 ECHO_HTTPS_PORT=18993 SERVER_LOG_DIR=/tmp/bifrost-http3-mock-logs bash e2e-tests/tests/test_http3_e2e.sh`，脚本启动本地 mock，host forwarding 和 body append 均返回 200，最终 `Passed: 34`、`Failed: 0`。执行 `BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost SKIP_BUILD=true PROXY_PORT=18981 MOCK_HTTP_PORT=18982 BIFROST_DATA_DIR=/tmp/bifrost-replay-body-decode-test SERVER_LOG_DIR=/tmp/bifrost-replay-body-decode-test/mock-logs bash e2e-tests/tests/test_replay_body_decode.sh`，输出 `Replay decoded gzip response body as JSON`，`Results: 1 passed, 0 failed`。执行 `BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost SKIP_BUILD=true BIFROST_CHATGPT_WEB_STARTUP_E2E_PORT=18971 bash e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh`，输出 `using existing bifrost binary` 与 `[chatgpt-web-startup-auth] PASS`，未触发 cargo build。三条真实执行均使用临时端口、`--no-system-proxy`，未使用 9900。 |
| TC-CS-31 | 通过 | 2026-05-27 本轮执行：`bash -n e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 通过；`rg -n 'BIFROST_PORT=.*ADMIN_PORT\|MOCK_PORT=.*MOCK_HTTP_PORT\|SKIP_BUILD\|target/release/bifrost\|target/debug/bifrost\|bifrost binary is not executable' e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 显示脚本优先消费调度器端口、`SKIP_BUILD=true` 默认 release binary、本地构建默认 debug binary，且 binary 不可执行时明确失败。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18945 MOCK_HTTP_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh`，输出 `skipping build, using .../target/release/bifrost` 与 `[agent-send-msg-feishu-card] PASS`。CI run `26515240075` 的 Linux/macOS shard 3 artifact 中原始日志均显示 `target/debug/bifrost: No such file or directory`，确认根因为 release artifact 场景错误查找 debug binary；真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-32 | 通过 | 2026-05-29 本轮执行：`bash -n e2e-tests/tests/test_agent_history_pagination_api.sh e2e-tests/tests/test_asr_task_append_during_run.sh e2e-tests/tests/test_asr_task_startup_recovery.sh` 通过；`rg -n 'SKIP_BUILD\|target/release/bifrost\|target/debug/bifrost\|bifrost binary is not executable\|skipping build, using' ...` 显示三个脚本均在 `SKIP_BUILD=true` 分支默认 release binary、本地构建分支默认 debug binary，且 binary 不可执行时明确失败；`/tmp/job-78509089317.log` 复核确认原始失败为三个脚本并行重复触发 Cargo build 后超过 900 秒 suite timeout。随后执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 生成 release binary，并分别运行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_history_pagination_api.sh`、`SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" BIFROST_ASR_TASK_APPEND_E2E_PORT=19131 bash e2e-tests/tests/test_asr_task_append_during_run.sh`、`SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" BIFROST_ASR_TASK_RECOVERY_E2E_PORT=19132 bash e2e-tests/tests/test_asr_task_startup_recovery.sh`，三者分别输出 `agent history pagination API checks passed`、`[asr-task-append] PASS`、`[asr-task-startup-recovery] PASS`，均未进入 `cargo build`；真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-33 | 通过 | 2026-05-29 本轮执行：`bash -n e2e-tests/tests/test_asr_task_cli.sh e2e-tests/tests/test_asr_diarization_cli.sh e2e-tests/tests/test_asr_model_autonomy.sh e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh` 通过；`rg -n 'ADMIN_PORT="\$\{BIFROST_ASR_[A-Z_]+_E2E_PORT:-\$\{ADMIN_PORT:-18[0-9]+' ...` 显示四个 ASR CLI shell E2E 均优先使用显式 `BIFROST_ASR_*_E2E_PORT`，其次使用外层 `ADMIN_PORT`，最后使用本地默认固定端口；`rg -n 'Failed to connect to Bifrost admin API\|Invalid argument' /tmp/bifrost-ci-26643081691-mac-shard2/.e2e-reports/shell_test_asr_voiceprint_enroll_cli_sh.log` 复核原始失败为固定端口场景下 CLI finish enrollment 无法连接 admin API。执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 时发现并修复 `web/dist-gzip` stale 目录删除 `Directory not empty` 构建竞态；修复后 release 构建通过。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=19141 bash e2e-tests/tests/test_asr_voiceprint_enroll_cli.sh`，输出 `[asr-voiceprint-enroll-cli-e2e] ok`；真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-34 | 通过 | 2026-06-04 本轮执行：`bash -n e2e-tests/tests/test_asr_diarization_cli.sh` 通过；`rg -n 'BIFROST_ASR_DIARIZATION_E2E_ONLINE\|skipping online model init\|diarization_ready' e2e-tests/tests/test_asr_diarization_cli.sh` 显示默认离线分支、显式联网开关和 ready 断言；`rg -n 'status code 429\|download diarization model' /tmp/bifrost-ci-26896054036-shard3/.e2e-reports/shell_test_asr_diarization_cli_sh.log` 复核原始 CI 失败为 HuggingFace 429。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=19143 bash e2e-tests/tests/test_asr_diarization_cli.sh`，输出 `skipping online model init` 与 `[asr-diarization-cli-e2e] ok`；真实链路启动 Bifrost、调用 CLI status、创建启用 diarization 的 ASR 任务，并在默认无模型资产时断言 `diarization_ready=false`。全程使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-35 | 通过 | 2026-06-04 本轮执行：Ruby YAML 解析 `.github/workflows/ci.yml` 通过，确认 Linux `e2e-shell` 仍为 `BIFROST_E2E_SHELL_JOBS=4`，macOS `e2e-macos-shell` 降为 `2`；`bash -n e2e-tests/tests/test_traffic_db_e2e.sh` 通过；`rg -n 'traffic-db-mock-\|BIFROST_E2E_REPORT_DIR\|MOCK_LOG_FILE' e2e-tests/tests/test_traffic_db_e2e.sh` 显示 traffic DB mock 临时日志会复制到 report dir；`rg -n 'Killed: 9\|Could not start mock server' /tmp/bifrost-ci-26899628938-mac-shard1/.e2e-reports` 复核原始 macOS shard 1 失败包含多个 Bifrost `Killed: 9` 与 traffic DB `Could not start mock server`。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理；完整 macOS shard 稳定性由推送后的 GitHub Actions run 验证。 |
| TC-CS-36 | 通过 | 2026-06-03 本轮执行：CI run `26896844438` 的 `E2E Shell (Linux, shard 2/3)` 失败日志显示 `test_agent_chat_history_continue.sh` 命中 `REQUEST_CONNECT_REFUSED`，Bifrost 请求 `http://127.0.0.1:34663/chat/completions` 失败，根因为脚本绕过调度器端口且 mock server 先挑端口再绑定存在并发抢占窗口。修复后执行 `bash -n e2e-tests/tests/test_agent_chat_history_continue.sh` 通过；`rg -n 'BIFROST_PORT=.*ADMIN_PORT\|PROXY_PORT\|MOCK_PORT_FILE\|server_address\|requested_port = .* if sys.argv\\[1\\] else 0' e2e-tests/tests/test_agent_chat_history_continue.sh` 显示脚本消费 `ADMIN_PORT` / `PROXY_PORT`，并由 Python mock server 绑定后写回真实端口。随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18131 PROXY_PORT=18131 bash e2e-tests/tests/test_agent_chat_history_continue.sh`，输出 `skipping build, using .../target/release/bifrost` 与 `[agent-chat-history-continue] PASS`；真实执行使用临时数据目录、`--no-system-proxy` 与非 9900 端口。 |
| TC-CS-37 | 通过 | 2026-06-04 本轮执行：`bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-shell.sh` 通过；`bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \| rg -n 'test_asr_\|test_qwen3_asr_\|test_voice_input_runtime\.sh\|test_voice_wake_actions\.sh'` 无输出且退出码为 1，确认 CI 模式不收集 ASR/voice runtime shell E2E；`bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \| rg -n 'test_asr_diarization_cli\.sh\|test_qwen3_asr_local_server\.sh\|test_voice_input_runtime\.sh\|test_asr_voiceprint_enroll_cli\.sh'` 输出代表性脚本，确认本地 full-shell 仍可验证。列表命令不启动 Bifrost、不下载模型、不访问外部模型源、不使用 9900、不修改系统代理。 |
| TC-CS-39 | 通过 | 2026-06-08 本轮执行：`SKIP_BUILD=true e2e-tests/tests/test_temporary_port_bindings.sh` 通过，输出 `Passed: 55`、`Failed: 0`；验证成功 listener 绑定在端口竞态时可重试，且 temporary port 绑定顺序、rule-file、inline rule、update、Traffic API/CLI listener port 等断言保持通过。 |
| TC-CS-40 | 通过 | 2026-06-08 本轮执行：`bash -n e2e-tests/test_utils/process.sh e2e-tests/tests/test_metrics_hosts_apps_admin_api.sh e2e-tests/tests/test_rule_semantics_regressions.sh e2e-tests/tests/test_proxy_chain_auth_e2e.sh e2e-tests/tests/test_host_rule_path_rewrite.sh e2e-tests/tests/test_multiline_rule_filter_e2e.sh` 通过；随后使用预构建 `target/release/bifrost` 分别执行 `test_metrics_hosts_apps_admin_api.sh`、`test_multiline_rule_filter_e2e.sh`、`test_rule_semantics_regressions.sh`、`test_host_rule_path_rewrite.sh`、`test_proxy_chain_auth_e2e.sh`，五个脚本均退出码 0，输出中不再出现清理阶段 `Killed ...`。 |
| TC-CS-41 | 通过 | 2026-06-09 本轮执行：`bash -n scripts/run_all_e2e.sh e2e-tests/tests/test_large_body_protection.sh` 通过；`rg -n 'RESOURCE_HEAVY_TESTS\|test_large_body_protection\\.sh\|is_resource_heavy\|BIFROST_DATA_DIR:-\\$PROJECT_DIR/\\.bifrost-test-large-body' scripts/run_all_e2e.sh e2e-tests/tests/test_large_body_protection.sh` 定位到 resource-heavy 串行队列、调度判断和 `BIFROST_DATA_DIR` fallback；使用 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 构建 release binary 后，以临时目录 `/tmp/bifrost-large-body-human.*`、端口 `19214/19215`、`SKIP_BUILD=true BIFROST_BIN=target/release/bifrost` 真实执行 `test_large_body_protection.sh`，输出 5 个 large body HTTP 用例通过、0 失败，且清理阶段停止代理和 mock 服务。 |
| TC-CS-42 | 通过 | 2026-06-13 本轮执行：Ruby YAML 解析 `.github/workflows/ci.yml` 输出 `shell layout ok`，确认 Linux shell E2E 没有 matrix/shard 环境变量且 job 名称为 `E2E Shell (Linux)`，macOS shell E2E 为 2 分片；静态执行 Linux 单 job `--list-shell-tests` 得到 133 个 CI shell tests；静态执行 `--list-shell-tests --shard N/2` 得到 `1/2 67`、`2/2 66`，总计 133 个 CI shell tests。全部命令只列测试或解析 YAML，未启动 Bifrost、未使用 9900、未修改系统代理。 |
| TC-CS-43 | 通过 | 2026-06-13 本轮执行：静态执行 CI shell 列表排除检查，输出 `skipped test_agent_codex_parity_contracts.sh`、`skipped test_im_agent_markdown_image_reply.sh`、`skipped test_im_agent_streaming_progress_card.sh`、`skipped test_utf8_safe_preview_e2e.sh`；确认这些纯 cargo contract 脚本不再进入 CI shell E2E。该回归只列测试，不启动 Bifrost、未使用 9900、未修改系统代理。 |
| TC-CS-44 | 通过 | 2026-06-17 本轮执行：GitHub Actions `CI` run `27676908356` 仅失败 `E2E Shell (aarch64-apple-darwin, shard 1/2)`，失败套件为 `shell:test_proxy_chain_auth_e2e.sh`，断言 `双代理链路请求成功` 期望 2xx 实际 502；同 run 中 `test_sync_login_direct_e2e.sh`、coverage 90% gate、Unit & Integration、Linux E2E、Windows/macOS/Linux build 与其它 E2E rules/runner 均通过。确认该 proxy chain 用例使用本机 `127.0.0.1` mock，不访问公网；随后把端口从 `$$ % 200` 固定小窗口改为 `pick_available_base_port` 动态连续端口段，并在非 2xx 时输出响应与 entry/upstream/mock 日志。执行 `bash -n e2e-tests/tests/test_proxy_chain_auth_e2e.sh` 通过；执行 `BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_proxy_chain_auth_e2e.sh`，输出 HTTP echo、proxy echo、Bifrost ready，双代理链路与下游代理鉴权全部通过，汇总 `Total: 11 / Passed: 11 / Failed: 0`，实际端口为动态非 9900 端口。 |
| TC-CS-45 | 通过 | 2026-06-22 本轮执行：GitHub Actions `CI` run `27924364360` 的 `E2E Shell (Linux)` 失败套件为 `shell:test_socks5_tls_routing_exceptions.sh`，失败原因为 `Network error: Failed to bind to 0.0.0.0:19379: Address already in use`。完整 job log 显示 runner 分配 `PROXY_PORT=19373`、`SOCKS5_PORT=19379`，脚本旧的 `DOWNSTREAM_PROXY_PORT=18890 + ($$ % 500)` 在 PID `36489` 下也算成 `19379`，下游代理和独立 SOCKS5 listener 端口碰撞。修复为 `DOWNSTREAM_PROXY_PORT` 优先使用 runner 注入的 `ECHO_PROXY_PORT` / `MOCK_ECHO_PROXY_PORT`，再 fallback 到 `PROXY_PORT+7`。执行 `bash -n e2e-tests/tests/test_socks5_tls_routing_exceptions.sh` 通过；执行静态 `rg` 确认默认值链路存在；随后使用 `PROXY_PORT=19373 SOCKS5_PORT=19379 ECHO_PROXY_PORT=19380 MOCK_ECHO_PROXY_PORT=19380 ECHO_HTTP_PORT=19374 ECHO_HTTPS_PORT=19375 SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh` 真实复现 CI 端口形态，脚本完成所有 routing exception 断言并退出 0，全程未使用 9900，未修改系统代理。 |
| TC-CS-46 | 通过 | 2026-07-01 本轮执行：`bash -n scripts/run_all_e2e.sh` 通过；`bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg '^(test_asr_admin_csrf|test_chatgpt_web_shared_profile)\\.sh$'` 无输出且退出码为 1，确认默认 PR CI shell shard 不再收集两个重型低频脚本；随后分别确认 `bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -q '^test_asr_admin_csrf\\.sh$'` 和同命令匹配 `test_chatgpt_web_shared_profile.sh` 均通过，说明本地 full-shell 专项入口仍保留；`bash scripts/ci/check-e2e-shell-ci-coverage.sh` 输出 selected/skipped 统计并以 `OK: every test_*.sh shell E2E script is selected by CI or explicitly skipped.` 结束。全部命令只列测试或做静态校验，未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-47 | 通过 | 2026-07-01 本轮执行：`bash -n scripts/run_all_e2e.sh` 通过；`bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg '^test_security_hardening\\.sh$'` 无输出且退出码为 1，确认默认 PR CI shell shard 不再收集安全聚合 wrapper；`bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -q '^test_security_hardening_functional\\.sh$'` 通过，确认安全功能子路径仍在默认 CI shell 覆盖中；`bash scripts/run_all_e2e.sh --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests | rg -q '^test_security_hardening\\.sh$'` 通过，确认本地 full-shell / release-gate 仍保留聚合入口；`bash scripts/ci/check-e2e-shell-ci-coverage.sh` 输出 selected/skipped 统计并以 `OK: every test_*.sh shell E2E script is selected by CI or explicitly skipped.` 结束。全部命令只列测试或做静态校验，未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-48 | 通过 | 2026-07-06 新增回归：先用 GitHub Actions run `28751216421` artifact 确认原失败为 `test_stop_restart_shutdown_marker.sh` 14/14 业务断言通过后 cleanup `rm: ... Directory not empty`；本轮修复 cleanup retry/best-effort 后执行 `bash -n e2e-tests/tests/test_stop_restart_shutdown_marker.sh` 通过，并执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_stop_restart_shutdown_marker.sh`，输出 `Total: 14 / Passed: 14 / Failed: 0`，退出码 0；全程使用随机端口和临时数据目录，未使用 9900，未修改真实系统代理。 |
| TC-CS-49 | 通过 | 2026-07-08 本轮执行：`bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-shell.sh` 通过；`BIFROST_E2E_SHELL_JOBS=2` 下 CI full-shell 全量列表与 `--shard 1/2`、`--shard 2/2` 合并列表 `diff -u` 无输出，输出 `all=160 shard1=76 shard2=84 overlap=0`；`BIFROST_E2E_SHELL_JOBS=2 bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --shard 1/2 --check-shell-shard-balance` 输出 `shard 1/2 estimated_wall=1398s serial=941s parallel_lanes=457,456 tests=76`、`shard 2/2 estimated_wall=1397s serial=526s parallel_lanes=871,866 tests=84`、`pct_of_avg=0.1%`；`rg` 定位到 `test_long_term_memory_remember_recall.sh`、`test_desktop_open_requests_contract.sh`、`test_chatgpt_web_behavior_artifacts.sh`、`test_skill_creator_flow.sh`、`test_im_gateway_long_reply_delivery_regression.sh` 的实测非默认权重。GitHub Actions `CI` run `28881027276` attempt 2 已全绿，调优前实测 macOS shell shard 为 1168s / 1847s，验证了继续按 estimated wall clock 优化的必要性。全部本地命令只列出或静态校验 shell 测试，未启动 Bifrost，未使用 9900，未修改系统代理。 |
| TC-CS-50 | 通过 | 2026-07-08 本轮执行：GitHub Actions `CI` run `28925523375` 的 `E2E Shell (Linux)` 失败套件为 `shell:test_cli_offline_commands_e2e.sh`，失败原因为 `CLI 快速开始缺少场景化说明: 场景 12：和 Agent 协作开发业务 Skill`。复核 `docs/cli-quick-start.md` 后确认新增 IM Gateway 快速开始后，`场景 12` 已变为 `添加飞书或微信 IM 通道`，Agent Skill 场景顺延为 `场景 13`，因此修复为同步 E2E 文档断言，并新增 IM provider 关键文案断言。执行 `bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；执行 `rg -n '场景 12：添加飞书或微信 IM 通道|场景 13：和 Agent 协作开发业务 Skill|bifrost im provider add feishu-main --type feishu --runner traex|Feishu 会在终端显示授权 URL 和二维码' e2e-tests/tests/test_cli_offline_commands_e2e.sh docs/cli-quick-start.md` 均命中；随后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`，确认 CLI offline help 与 quick-start 文档同步回归通过。该回归不启动 Bifrost，不使用 9900，不修改系统代理。 |
| TC-CS-51 | 通过 | 2026-07-08 本轮执行：GitHub Actions main push `CI` run `28931733950` 的 `Windows Unit Tests (x86_64)` 失败，失败测试为 `schedule_agent_adapter_config_overrides_runner_without_dropping_command`，日志显示 `timeout after 10000ms`，`TaskRunStatus` 实际为 `Timeout`。第一版修复后 PR `CI` run `28933280470` 确认 timeout 消失，但 Windows `cmd.exe echo` 输出转义 JSON，导致 `agent_final_response` 为原始 JSON 字符串而不是 `OVERRIDE_OK`。第二版修复改为 PowerShell 不读取 stdin，直接用 `[char]34` 拼接合法 JSON 行。本机执行 `cargo test -p bifrost-admin schedule_agent_adapter_config_overrides_runner_without_dropping_command -- --nocapture` 多次通过，测试输出 `1 passed; 0 failed`。Windows 真实 job 由推送后的 GitHub Actions `CI` run 继续验证。该本地回归不启动 Bifrost，不使用 9900，不修改系统代理。 |

## 清理步骤

- 删除测试中创建的临时目录（如 `/tmp/bifrost-large-body-human.*`）。
- 真实运行 shard 用例后确认临时 Bifrost 进程已退出：`pgrep -fl "bifrost.*start"`。
