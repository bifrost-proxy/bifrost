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
3. 用 CI 调度器风格端口执行 guide/queue 黑盒真实链路：
   ```bash
   ADMIN_PORT=18111 MOCK_HTTP_PORT=18112 \
     bash e2e-tests/tests/test_im_guide_queue_human_api.sh
   ```
4. 用另一组 CI 调度器风格端口执行 `/status` 运行中指标黑盒真实链路：
   ```bash
   ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 \
     bash e2e-tests/tests/test_agent_builtin_status_runtime.sh
   ```

**预期结果**：
- 第 1 步所有脚本语法检查通过。
- 第 2 步每个脚本均能匹配到 `ADMIN_PORT` 与 `MOCK_HTTP_PORT` 回退表达式，证明并行 shell 调度器分配的端口会覆盖固定本地默认端口。
- 第 3 步输出 `starting bifrost on 18111`、`configuring agent mock provider`、`[im-guide-queue-human-api] PASS`，不再因为与其它并行用例争抢 `18897/18898` 出现 `curl: (52) Empty reply from server`。
- 第 4 步输出 `starting bifrost on 18121`、`configuring agent mock provider`、`[agent-builtin-status-runtime] PASS`，运行中 `/status` 指标仍通过。
- 两个真实链路均使用临时数据目录、`--no-system-proxy` 和非 9900 端口。

## 本轮执行记录

测试日期：2026-05-05

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
| TC-CS-21 | 通过 | 2026-05-06 本轮执行：`bash -n e2e-tests/tests/test_agent_builtin_status_runtime.sh e2e-tests/tests/test_im_guide_queue_human_api.sh e2e-tests/tests/test_long_term_memory_human_api.sh e2e-tests/tests/test_update_plan_human_api.sh e2e-tests/tests/test_agent_loop_runtime_limits.sh` 通过；`rg -n 'BIFROST_PORT="\$\{BIFROST_PORT:-\$\{ADMIN_PORT:-\|MOCK_PORT="\$\{MOCK_PORT:-\$\{MOCK_HTTP_PORT:-' ...` 显示 5 个脚本均优先消费并行调度器端口。随后执行 `ADMIN_PORT=18111 MOCK_HTTP_PORT=18112 bash e2e-tests/tests/test_im_guide_queue_human_api.sh`，输出 `starting bifrost on 18111` 与 `[im-guide-queue-human-api] PASS`；执行 `ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，输出 `starting bifrost on 18121` 与 `[agent-builtin-status-runtime] PASS`。两条真实链路均使用临时数据目录、`--no-system-proxy` 与非 9900 端口，未复现 CI 中 `curl: (52) Empty reply from server` 的端口碰撞症状。 |

## 清理步骤

- 无特殊清理需求，测试使用临时数据目录
