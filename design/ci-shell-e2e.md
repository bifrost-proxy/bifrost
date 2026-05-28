# CI Shell E2E

## 功能模块说明

CI shell E2E 通过 `scripts/ci/run-e2e-shell.sh` 调用 `scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build`，并由 `BIFROST_E2E_SHARD_INDEX` / `BIFROST_E2E_SHARD_TOTAL` 在 CI runner 间分片执行。

系统代理用例 `test_system_proxy_e2e.sh` 会修改宿主机网络代理设置，在 macOS CI 的临时 runner 上存在系统设置收敛不稳定问题。该用例不再纳入 CI shell 集合，仅保留本地 `--full-shell` 场景执行。

## 实现逻辑

- `scripts/run_all_e2e.sh` 的 `SKIP_IN_CI_TESTS` 维护 CI 禁跑脚本列表。
- `collect_shell_tests` 在 `MODE=ci` 时过滤 `SKIP_IN_CI_TESTS`，过滤后再应用分片，避免被跳过用例占用 shard 槽位。
- `test_system_proxy_e2e.sh` 加入 `SKIP_IN_CI_TESTS` 后，`scripts/ci/run-e2e-shell.sh` 在 macOS/Linux/Windows CI 中均不会收集该脚本。
- 本地运行 `bash scripts/run_all_e2e.sh --full-shell ...` 时 `MODE=local`，不会应用 CI skip 列表，仍可手动验证系统代理功能。
- `--list-shell-tests` 只打印当前 mode/shard 下会被收集的 shell 脚本并退出，用于验证调度结果，不会构建、启动 Bifrost 或修改系统代理配置。
- GitHub Actions E2E 日志路径使用隐藏目录 `.e2e-reports/` 与 `.bifrost-e2e-ci/`；上传失败日志 artifact 时必须设置 `include-hidden-files: true`，否则 action 会跳过这些路径并导致失败后无 artifact 可查。
- `scripts/run_all_e2e.sh` 的失败原因提取优先匹配真实断言、Playwright/JS 错误和 panic；cleanup 尾巴（例如 `Preserving failed test root`）只作为日志上下文，不能作为最终失败原因。
- `test_cli_offline_commands_e2e.sh` 的 help 文案断言必须使用扩展正则（`grep -E`）或多个 `-e` 模式表达 alternation；禁止在默认 BRE 模式下使用 `\|`，否则 Linux shard 可能把实际存在的 `rename` / `NEW_NAME` 文案误判为缺失。
- `test_cli_offline_commands_e2e.sh` 的命令输出断言不能使用 `echo "$result" | grep -q`。在 `pipefail` 下，`grep -q` 匹配后提前退出会让 `echo` 收到 SIGPIPE，导致 Linux shard 把实际存在的 help 文案误判为失败。应使用 `grep ... <<<"$result"` 等非管道输入。
- GitHub Actions 的 `Dump failed suite logs` 诊断步骤在 `bash -e -o pipefail` 下不能让 `find | head` 管道状态传播为 step 失败；日志枚举只用于诊断，必须对 head 早退导致的 SIGPIPE 容错，保证失败日志 artifact 上传步骤继续执行。
- `test_unsafe_ssl_e2e.sh` 不依赖外部共享 HTTPS mock fixture。脚本在当前 `HTTPS_MOCK_PORT` 不可用时自行启动 `e2e-tests/mock_servers/https_echo_server.py`，创建 `unsafe-ssl-fixture.test https://127.0.0.1:<HTTPS_MOCK_PORT>` 转发规则，等待端口就绪后执行 unsafe_ssl false/true/false 三段真实代理请求，并在 EXIT trap 中保留原始退出码后清理自有 mock、规则与 Bifrost。该脚本通过 `ADMIN_CLIENT_START_UNSAFE_SSL=0` 让通用 admin helper 以安全默认启动，避免 CLI 启动参数 `--unsafe-ssl` 掩盖动态配置切换。
- `admin_client.sh` 复用已有管理端前必须请求 `/api/auth/status` 并校验返回体是 Bifrost 管理端鉴权状态 JSON。CI 并行端口碰撞时，不能把其它本机服务的 200/404 响应误判为已启动的 Bifrost，否则后续规则 API 会命中错误服务。
- `run_shell_tests_parallel` / `run_shell_batch_parallel` 在所有并行 shell 子用例完成后必须显式 `return 0`。Bash 函数默认返回最后一条命令的状态；如果最后一次循环中 `[[ $running -gt 0 ]]` 为 false，函数会返回 1，在 `set -e` 下导致 CI step 在所有子用例 PASS 后仍失败。
- `scripts/run_all_e2e.sh` 顶层 final status 检查在没有失败 suite 时必须显式 `exit 0`。否则最后一次 `[[ "$status" == "failed" ]]` 判断会在所有 suite 都是 `passed` / `skipped` 时返回 1，导致 CI step 在业务日志全 PASS 后仍进入失败日志 dump。
- `test_replay_rules.sh` 的 SSE replay 长连接回归使用 5s `timeout_ms`，并断言收到 `id>=12` 的超时边界后事件。该用例验证 replay 不会把 `timeout_ms` 当作 SSE body 总时长限制，同时允许 GitHub Actions runner 在边界之后提前关闭客户端连接，避免把外部连接噪声误判为功能失败。
- `test_long_term_memory_human_api.sh` 构建 Bifrost 时设置 `SKIP_FRONTEND_BUILD=1`，避免多个 shell E2E 并行触发 `pnpm build` 重写 `web/dist`，导致 `rust_embed` 在编译时读到临时缺失的 frontend 产物。
- 会自启动 Bifrost 与 Chat Completions mock 的 Agent/IM human-api shell 用例必须优先消费并行调度器注入的 `ADMIN_PORT` 与 `MOCK_HTTP_PORT`，再回退到单脚本本地默认端口。`run_shell_batch_parallel` 只按子用例 index 分配通用端口环境变量，不会额外注入 `BIFROST_PORT` / `MOCK_PORT`；如果脚本继续硬编码 `18897/18898` 等默认端口，Linux shard 1 在 `BIFROST_E2E_SHELL_JOBS=16` 并行时会让 `test_agent_builtin_status_runtime.sh` 与 `test_im_guide_queue_human_api.sh` 抢同一 Bifrost/mock 端口，表现为 PATCH `/_bifrost/api/im-gateway/agent` 时 `curl: (52) Empty reply from server`。
- `test_remote_relay_url_fallback_e2e.sh` 以及会自启动 Bifrost 与 Chat Completions mock 的 Agent/IM human-api shell 用例必须尊重外层 `SKIP_BUILD=true` 与已有 `BIFROST_BIN`。CI shell 入口已经预构建 release binary，单个用例不能再次无条件 build，否则会在并行 CI 中长时间卡住；本地 CI 复用 release binary 时也不能被脚本内旧 Cargo toolchain 的重新构建阻塞。
- `test_agent_chat_history_continue.sh` 与 `test_agent_direct_path_switch.sh` 在 `SKIP_BUILD=true` 时必须默认使用 `$REPO_DIR/target/release/bifrost`，不能继续默认 `$REPO_DIR/target/debug/bifrost`。CI shell shard 使用 release artifact 并不会生成 debug binary；用例应在复用预构建 binary 时输出 `skipping build, using .../target/release/bifrost`，并在 binary 不可执行时给出明确错误。
- `test_agent_send_msg_feishu_card.sh` 同样属于会自启动 Bifrost 与 Chat Completions/Feishu mock 的 Agent/IM human-api shell 用例。在 `SKIP_BUILD=true` 时必须默认复用 `$REPO_DIR/target/release/bifrost`，并优先消费调度器注入的 `ADMIN_PORT` 与 `MOCK_HTTP_PORT`。CI shell shard 3 只下载 release artifact，不会生成 debug binary；脚本如果继续默认 `target/debug/bifrost` 会在 Linux 与 macOS shard 3 同时失败。
- `scripts/run_all_e2e.sh` 的 `CARGO_BIN` 默认值必须从当前 `PATH` 解析，而不是固定使用 `$HOME/.cargo/bin/cargo`。本地开发环境可能同时存在 rustup 旧工具链与 Homebrew/系统新工具链；shell E2E 子脚本里的 `cargo test/run` 需要继承入口选定的 Cargo，否则会在 `SKIP_BUILD=true` 的本地准入路径中被旧 Cargo 解析 2024 edition 依赖失败。CI 或调用方仍可通过显式 `CARGO_BIN=...` 覆盖。
- Linux 与 macOS `E2E Shell` job 的 GitHub Actions `timeout-minutes` 与 rules/runner 对齐为 60 分钟。Linux shard 需要安装 Playwright `chromium-headless-shell` 及系统依赖；macOS shard 需要等待 release artifact、安装前端依赖并运行较重的 DevTools/remote shell 用例。任一平台短时变慢时，30 分钟预算会导致 shell 用例已经通过但 job 在归档/清理阶段被 workflow timeout 标记为失败。
- GitHub Actions concurrency 对同一 ref 只保留最新 CI run。旧的 `main` push run 如果已经失败但长尾 Windows/macOS E2E 仍在运行，会阻塞后续修复 commit 的 run；`cancel-in-progress: true` 让最新 commit 立即获得执行权，避免用已过期的红灯 run 占用主干合入验证窗口。
- Linux 与 macOS shell shard 内部并发设为 4。CI run `25469654203` 的 Linux shard 2 artifact 显示多个 Bifrost 子进程被系统 `Killed`，说明 16 路 shell 内部并发会在 hosted runner 上放大内存压力；CI run `25470391707` 的 Linux shard 3 artifact 又显示所有业务断言通过但仍有 Bifrost 子进程被系统 `Killed`，说明 8 路在 shard 3 的资源峰值下仍可能触发 OOM。3 个 shard 仍保持横向并行，单 shard 内改为 4 路降低 OOM 风险。
- Shell shard 中会执行 `cargo check` / `cargo test` / `cargo run`，或在 `SKIP_BUILD=true` 下仍可能触发 Cargo 的用例，必须作为 lock-sensitive 用例串行执行。CI run `26451521064` 的 Linux shard 1 与 macOS shard 1 artifact 显示 `test_agent_codex_parity_contracts.sh`、`test_chatgpt_web_behavior_artifacts.sh`、`test_im_agent_streaming_progress_card.sh`、`test_asr_task_pause_resume.sh`、`test_voice_input_runtime.sh` 等用例在并行 shell batch 中长时间输出 `Blocking waiting for file lock on artifact directory`，部分业务断言本身已通过但最终被 900s per-test timeout 杀掉。该类用例不应靠放大 timeout 掩盖锁竞争，而应从并行批次移入串行队列，使超时预算用于真实测试执行。
- Shell E2E 不应依赖公网 `httpbin.org`、`echo.websocket.events` 等外部域名作为断言前提。CI run `26566668834` 显示 `test_http3_e2e.sh` 的 host forwarding / body append 请求偶发返回 503，`test_replay_body_decode.sh` 的 replay gzip 请求也偶发返回 503；这些路径应改为 `e2e-tests/mock_servers/http_echo_server.py` 提供的本地 httpbin-compatible mock。HTTP3 规则 fixture 使用占位符把 `bifrost-httpbin.test`、`h3-forward-test.local`、`h3-body-test.local`、`h3-websocket-test.local` 全部转发到当前分片分配的本地 HTTP mock 端口；Replay gzip 用例直接 replay 本地 `/gzip`，避免公网、DNS、外部 503 和代理环境影响。
- `scripts/run_all_e2e.sh` 在进入 shell 子用例时必须默认注入预构建 release `BIFROST_BIN`，同时保留调用者显式传入的 `BIFROST_BIN`。单个 shell 用例即使支持 `SKIP_BUILD=true`，如果默认仍指向 `target/debug/bifrost` 或忽略外层 `BIFROST_BIN`，也会在 macOS shard 中重新触发 `cargo build` 并等待 artifact lock。`test_chatgpt_web_startup_auth_preflight.sh` 等 startup 类脚本必须尊重 `SKIP_BUILD=true` 与已有 `BIFROST_BIN`。

## 依赖项

- `scripts/run_all_e2e.sh`
- `scripts/ci/run-e2e-shell.sh`
- `e2e-tests/tests/test_system_proxy_e2e.sh`
- `e2e-tests/tests/test_unsafe_ssl_e2e.sh`
- `e2e-tests/tests/test_long_term_memory_human_api.sh`
- `e2e-tests/tests/test_agent_builtin_status_runtime.sh`
- `e2e-tests/tests/test_im_guide_queue_human_api.sh`
- `e2e-tests/tests/test_update_plan_human_api.sh`
- `e2e-tests/tests/test_agent_loop_runtime_limits.sh`
- `e2e-tests/tests/test_agent_send_msg_feishu_card.sh`
- `e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `e2e-tests/tests/test_http3_e2e.sh`
- `e2e-tests/tests/test_replay_body_decode.sh`
- `e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh`
- `e2e-tests/mock_servers/https_echo_server.py`
- `e2e-tests/mock_servers/http_echo_server.py`
- `e2e-tests/test_utils/admin_client.sh`

## 测试方案

### 单元测试

本次修改为 Bash 调度逻辑，无 Rust 公共函数变化，不新增 Rust 单元测试。通过脚本级命令验证 `collect_shell_tests` 的 CI 过滤行为。

### E2E 测试

- 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --list-shell-tests --shard 3/3`，断言输出中没有 `test_system_proxy_e2e.sh`。
- 运行 `bash scripts/run_all_e2e.sh --full-shell --list-shell-tests`，断言本地 full-shell 仍可收集 `test_system_proxy_e2e.sh`。
- 运行 `BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=4 bash scripts/ci/run-e2e-shell.sh`，覆盖 shard 3 并行 shell 包装与 DevTools page bridge 用例。
- 静态检查 `.github/workflows/ci.yml` 中所有上传 `.e2e-reports/` / `.bifrost-e2e-ci/` 的 E2E artifact 步骤均包含 `include-hidden-files: true`。
- 运行 `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`，断言 `rule rename --help`、`rule reorder --help`、`script rename --help` 与其它 help 关键字匹配全部通过，最终汇总为 `105/105` PASS。
- 运行 `rg -n 'echo "\$[A-Za-z_][A-Za-z0-9_]*" \| grep -[A-Za-z]+' e2e-tests/tests/test_cli_offline_commands_e2e.sh`，断言脚本中不再存在 `echo | grep -q` 输出断言；运行 `SKIP_BUILD=true BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_cli_offline_commands_e2e.sh` 验证 `system-proxy enable --help` 不再触发 Broken pipe 误判。
- 静态检查 `.github/workflows/ci.yml` 的 `Dump failed suite logs` 步骤，断言所有 `find "$BIFROST_E2E_REPORT_DIR" ... | head` 与 `find "$BIFROST_DATA_DIR" ... | head` 管道均带有 `|| true` 容错。
- 运行 `HTTPS_MOCK_PORT=<空闲端口> PROXY_PORT=<空闲端口> ADMIN_PORT=<同代理端口> BIFROST_DATA_DIR=<临时目录> bash e2e-tests/tests/test_unsafe_ssl_e2e.sh`，断言脚本自行启动 HTTPS mock，unsafe_ssl 动态切换相关 5 个用例全部通过。
- 运行 `bash -n e2e-tests/test_utils/admin_client.sh e2e-tests/tests/test_unsafe_ssl_e2e.sh` 并通过本机非 Bifrost HTTP 服务占用目标端口，断言 `admin_ensure_bifrost` 不会复用错误服务。
- 运行 `bash -n scripts/run_all_e2e.sh` 并检查 `run_shell_tests_parallel` / `run_shell_batch_parallel` 末尾存在显式 `return 0`，断言并行 shell 调度器不会在全部子用例通过后把空闲轮询条件的 false 状态传播为 suite 失败。
- 运行 `PROXY_PORT=<空闲端口> MOCK_HTTP_PORT=<空闲端口> MOCK_SSE_PORT=<空闲端口> MOCK_WS_PORT=<空闲端口> BIFROST_DATA_DIR=<临时目录> SERVER_LOG_DIR=<临时目录> SKIP_BUILD=true bash e2e-tests/tests/test_replay_rules.sh`，断言 `SSE Replay with Rules` 收到 connection/applied_rules，并收到 `id>=12` 的 post-timeout SSE 事件。
- 运行 `bash -n e2e-tests/tests/test_long_term_memory_human_api.sh` 并检查构建命令包含 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost`，断言该用例不再参与并行 frontend build。
- 运行 `bash -n` 检查 `test_agent_builtin_status_runtime.sh`、`test_im_guide_queue_human_api.sh`、`test_long_term_memory_human_api.sh`、`test_update_plan_human_api.sh`、`test_agent_loop_runtime_limits.sh`；再用 `rg -n 'BIFROST_PORT=.*ADMIN_PORT|MOCK_PORT=.*MOCK_HTTP_PORT'` 断言这些 Agent/IM human-api 脚本消费调度器端口，并用 `rg -n 'SKIP_BUILD|BIFROST_BIN|skipping build, using'` 断言脚本尊重预构建 binary。执行 `ADMIN_PORT=18111 MOCK_HTTP_PORT=18112 bash e2e-tests/tests/test_im_guide_queue_human_api.sh` 和 `ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，确认真实 Bifrost + mock provider 在注入端口下可通过。
- 运行 `bash -n scripts/run_all_e2e.sh` 并检查 `CARGO_BIN` 默认值来自 `resolve_non_shim_command cargo`；使用 `CARGO_BIN="$(which cargo)" bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests`，断言入口仍允许调用方显式覆盖 Cargo，且列表模式不会构建或启动服务。
- 运行 `bash -n e2e-tests/tests/test_agent_chat_history_continue.sh e2e-tests/tests/test_agent_direct_path_switch.sh`，并用 `rg -n 'SKIP_BUILD|target/release/bifrost|target/debug/bifrost|skipping build, using'` 断言两个脚本在 `SKIP_BUILD=true` 时默认 release binary、在本地构建时默认 debug binary；再运行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_chat_history_continue.sh` 与 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_agent_direct_path_switch.sh`，确认二者不会查找 `target/debug/bifrost`，且真实 Bifrost + mock provider 链路通过。
- 运行 `bash -n e2e-tests/tests/test_agent_send_msg_feishu_card.sh`，并用 `rg -n 'BIFROST_PORT=.*ADMIN_PORT|MOCK_PORT=.*MOCK_HTTP_PORT|SKIP_BUILD|target/release/bifrost|target/debug/bifrost' e2e-tests/tests/test_agent_send_msg_feishu_card.sh` 断言脚本消费调度器端口，且在 `SKIP_BUILD=true` 时默认 release binary、在本地构建时默认 debug binary。运行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" ADMIN_PORT=18945 MOCK_HTTP_PORT=18946 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh`，确认 CI 预构建 release binary 路径下真实 Bifrost + mock 模型 + fake Feishu interactive card 链路通过。
- 运行 `SKIP_BUILD=true BIFROST_BIN=<已有 release bifrost> bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`，断言脚本输出 `Using existing bifrost binary` 且三段 relay fallback 断言全部通过。
- 静态解析 `.github/workflows/ci.yml`，断言 Linux `e2e-shell` 与 macOS `e2e-macos-shell` job 的 `timeout-minutes` 都为 60，避免真实 shell shard 已完成但 job 收尾阶段被 workflow timeout 拉失败。
- 静态解析 `.github/workflows/ci.yml`，断言 workflow 顶层 `concurrency` 使用 `group: ${{ github.workflow }}-${{ github.ref }}` 且 `cancel-in-progress: true`，确保主干连续 push 时旧 run 不再阻塞最新修复 commit 的 CI。
- 静态解析 `.github/workflows/ci.yml`，断言 Linux `e2e-shell` 与 macOS `e2e-macos-shell` job 的 `BIFROST_E2E_SHELL_JOBS` 都为 4，避免 hosted runner 内部 8 路或 16 路并发触发 OOM kill。
- 静态解析 `scripts/run_all_e2e.sh`，断言所有 shell E2E 中会调用 `cargo check/test/run` 的用例在 `CARGO_HEAVY_TESTS` 中登记，并且 `run_shell_tests_parallel` 将 `is_cargo_heavy` 用例加入 `serial_tests`。使用 `BIFROST_E2E_SHARD_INDEX=1 BIFROST_E2E_SHARD_TOTAL=3 scripts/run_all_e2e.sh --ci --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests` 确认当前失败 shard 仍收集这些用例，真实调度时会由 serial 队列避免 Cargo artifact lock 竞争。
- 运行 `BIFROST_BIN=<已有 bifrost> SKIP_BUILD=true SKIP_CARGO_TEST=true PROXY_PORT=<空闲端口> ECHO_HTTP_PORT=<空闲端口> ECHO_HTTPS_PORT=<空闲端口> bash e2e-tests/tests/test_http3_e2e.sh`，断言 HTTP3 host forwarding、response body append、gzip、SSE、POST、PUT/PATCH/DELETE 等请求全部命中本地 mock，最终 `Failed: 0`，日志不再出现公网 `httpbin.org` 或 `echo.websocket.events`。
- 运行 `BIFROST_BIN=<已有 bifrost> SKIP_BUILD=true PROXY_PORT=<空闲端口> MOCK_HTTP_PORT=<空闲端口> BIFROST_DATA_DIR=<临时目录> SERVER_LOG_DIR=<临时目录> bash e2e-tests/tests/test_replay_body_decode.sh`，断言 replay gzip body decode 对本地 `/gzip` 返回 `status=200` 且 body 包含 `"gzipped": true`。
- 运行 `BIFROST_BIN=<已有 bifrost> SKIP_BUILD=true BIFROST_CHATGPT_WEB_STARTUP_E2E_PORT=<空闲端口> bash e2e-tests/tests/test_chatgpt_web_startup_auth_preflight.sh`，断言脚本输出 `using existing bifrost binary` 且不执行 `cargo build`。

### 真实场景测试

- 更新 `human_tests/ci-shell-e2e-sharding.md`，覆盖 CI 不执行系统代理用例、隐藏日志 artifact 上传配置、失败原因摘要提取、shard 3 shell 包装回归、CLI offline help alternation 回归、CLI offline `echo | grep -q` Broken pipe 回归、失败日志 dump `find | head` pipefail 回归、SSE replay timeout 边界回归、macOS CI post-timeout 连接噪声回归、unsafe_ssl 管理端端口碰撞回归、long-term memory frontend build 竞争回归、Agent/IM human-api 并行端口隔离回归、Agent history/direct-path 与 Feishu card 用例预构建 release binary 复用回归、remote relay fallback 跳过重复 release build 回归、Linux/macOS shell E2E timeout 预算回归、main push CI concurrency 取消旧 run 回归、Linux/macOS shell shard 内部并发预算回归、shell E2E Cargo 默认解析回归、HTTP3/Replay 外部 httpbin 依赖本地 mock 化回归，以及顶层 shell E2E 全 PASS 退出码回归。
- 新增 `human_tests/ci-shell-e2e-sharding.md` 的 Cargo-heavy shell 用例串行调度回归，覆盖 `test_agent_codex_parity_contracts.sh`、`test_chatgpt_web_behavior_artifacts.sh`、`test_im_agent_streaming_progress_card.sh` 等当前 CI 失败路径。
- 按新增用例逐条执行，确认 CI 模式过滤、本地模式保留，失败日志可上传且摘要不会被 cleanup 尾巴覆盖，CLI offline help 断言不再误判，unsafe_ssl 用例不再依赖外部 HTTPS mock fixture且不会复用错误本机服务，并行 shell 调度器和顶层 final status 检查在全 PASS 后返回 0，SSE replay 在超过 `timeout_ms` 后收到 post-timeout 事件，long-term memory 用例跳过 frontend build，Agent/IM human-api 用例消费调度器端口并可在不同端口真实启动，shell E2E 入口默认继承当前 PATH 的 Cargo，remote relay fallback 在预构建 binary 存在时不再重复 build，HTTP3/Replay 不再依赖公网域名，Linux/macOS shell E2E timeout 与真实 CI 运行成本匹配。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 更新 `human_tests/ci-shell-e2e-sharding.md`
- 更新 `human_tests/readme.md`
