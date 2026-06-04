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
   ruby -ryaml -e 'workflow = YAML.load_file(".github/workflows/ci.yml"); %w[e2e-shell e2e-macos-shell].each { |name| jobs = workflow["jobs"][name]["env"]["BIFROST_E2E_SHELL_JOBS"]; raise "#{name} jobs mismatch: #{jobs.inspect}" unless jobs == "4" }; puts "shell shard jobs budget ok"'
   ```
2. 推送后检查 GitHub Actions `CI` run 中 `E2E Shell (Linux, shard 2/3)` 与 macOS shell shards。

**预期结果**：
- 第 1 步输出 `shell shard jobs budget ok`。
- Linux 与 macOS shell shard 仍按 3 shard 横向执行，但每个 shard 内部只并发 4 个 shell suite，避免 hosted runner 在 8 路或 16 路内部并发下将多个 Bifrost 子进程 OOM kill。
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

## 本轮执行记录

测试日期：2026-05-09

| 用例 | 结果 | 实际结果 |
|------|------|----------|
| TC-CS-10 | 通过 | `rg -n "include-hidden-files: true" .github/workflows/ci.yml \| wc -l` 输出 `8`；`bash scripts/run_all_e2e.sh --extract-failure-reason "$TMP_LOG"` 输出 `browserType.launch: Host system is missing dependencies`；本地执行 `source ~/.zshrc && BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 TIMEOUT=90 bash scripts/ci/run-e2e-shell.sh`，24/24 通过，`test_devtools_page_bridge_api.sh` 在并行 shard 3 中 44s 通过。 |
| TC-CS-11 | 通过 | 2026-04-30 本轮执行：`bash -n e2e-tests/tests/test_cli_offline_commands_e2e.sh` 通过；`rg -n 'grep\s+(-[^ ]*)?q[^ ]*\s+"[^"]*\\\|' e2e-tests/tests/test_cli_offline_commands_e2e.sh` 无输出；`bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 汇总 `通过: 106`、`失败: 0`，其中 `rule rename --help`、`rule reorder --help`、`script rename --help` 均通过。 |
| TC-CS-12 | 通过 | 2026-05-01 本轮执行：`bash -n e2e-tests/tests/test_unsafe_ssl_e2e.sh e2e-tests/test_utils/admin_client.sh` 通过；随后使用 `TEST_ROOT="$(mktemp -d /tmp/bifrost-unsafe-ssl-human.XXXXXX)" PROXY_PORT=11295 ADMIN_PORT=11295 HTTPS_MOCK_PORT=11297 BIFROST_DATA_DIR="$TEST_ROOT/data" SERVER_LOG_DIR="$TEST_ROOT/logs" SKIP_BUILD=true bash e2e-tests/tests/test_unsafe_ssl_e2e.sh` 执行真实场景。脚本输出 `Starting HTTPS mock server on 127.0.0.1:11297`、`HTTPS mock server ready`、`Created unsafe_ssl forwarding rule to https://127.0.0.1:11297`，并完成 unsafe_ssl false/true/false 三段代理请求，汇总 `Results: 5/5 passed`，退出码 0；全程使用临时目录和 11295/11297，未使用 9900。 |
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
| TC-CS-23 | 通过 | 2026-05-07 本轮执行：基于 GitHub Actions `CI` run `25469654203` 的 `E2E Shell (Linux, shard 2/3)` artifact，定位到多个 Bifrost 子进程被系统 `Killed`，符合 hosted runner 内存压力症状；随后 run `25470391707` 的 `E2E Shell (Linux, shard 3/3)` artifact 显示所有业务断言通过但仍有 Bifrost 子进程在 cleanup 中被系统 `Killed`，说明 8 路并发仍有资源峰值风险；随后 Ruby YAML 标准库解析 `.github/workflows/ci.yml`，确认 `e2e-shell` 与 `e2e-macos-shell` 的 `BIFROST_E2E_SHELL_JOBS == "4"`。该静态回归不启动 Bifrost，不使用 9900，不修改系统代理。完整云端结果由推送后的 GitHub Actions `CI` run 验证。 |
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

## 清理步骤

- 无特殊清理需求，测试使用临时数据目录
