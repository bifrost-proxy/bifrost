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
- `test_unsafe_ssl_e2e.sh` 不依赖外部共享 HTTPS mock fixture。脚本在当前 `HTTPS_MOCK_PORT` 不可用时自行启动 `e2e-tests/mock_servers/https_echo_server.py`，创建 `unsafe-ssl-fixture.test https://127.0.0.1:<HTTPS_MOCK_PORT>` 转发规则，等待端口就绪后执行 unsafe_ssl false/true/false 三段真实代理请求，并在 EXIT trap 中保留原始退出码后清理自有 mock、规则与 Bifrost。该脚本通过 `ADMIN_CLIENT_START_UNSAFE_SSL=0` 让通用 admin helper 以安全默认启动，避免 CLI 启动参数 `--unsafe-ssl` 掩盖动态配置切换。
- `run_shell_tests_parallel` / `run_shell_batch_parallel` 在所有并行 shell 子用例完成后必须显式 `return 0`。Bash 函数默认返回最后一条命令的状态；如果最后一次循环中 `[[ $running -gt 0 ]]` 为 false，函数会返回 1，在 `set -e` 下导致 CI step 在所有子用例 PASS 后仍失败。
- `test_replay_rules.sh` 的 SSE replay 长连接回归使用 5s `timeout_ms` 与 8s 存活断言。该用例验证 replay 不会把 `timeout_ms` 当作 SSE body 总时长限制，同时避开 GitHub Actions Linux runner 上曾出现的 10s 客户端连接边界噪声。

## 依赖项

- `scripts/run_all_e2e.sh`
- `scripts/ci/run-e2e-shell.sh`
- `e2e-tests/tests/test_system_proxy_e2e.sh`
- `e2e-tests/tests/test_unsafe_ssl_e2e.sh`
- `e2e-tests/mock_servers/https_echo_server.py`
- `e2e-tests/test_utils/admin_client.sh`

## 测试方案

### 单元测试

本次修改为 Bash 调度逻辑，无 Rust 公共函数变化，不新增 Rust 单元测试。通过脚本级命令验证 `collect_shell_tests` 的 CI 过滤行为。

### E2E 测试

- 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --list-shell-tests --shard 3/3`，断言输出中没有 `test_system_proxy_e2e.sh`。
- 运行 `bash scripts/run_all_e2e.sh --full-shell --list-shell-tests`，断言本地 full-shell 仍可收集 `test_system_proxy_e2e.sh`。
- 运行 `BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_SHELL_JOBS=16 bash scripts/ci/run-e2e-shell.sh`，覆盖 shard 3 并行 shell 包装与 DevTools page bridge 用例。
- 静态检查 `.github/workflows/ci.yml` 中所有上传 `.e2e-reports/` / `.bifrost-e2e-ci/` 的 E2E artifact 步骤均包含 `include-hidden-files: true`。
- 运行 `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`，断言 `rule rename --help`、`rule reorder --help`、`script rename --help` 与其它 help 关键字匹配全部通过，最终汇总为 `105/105` PASS。
- 运行 `HTTPS_MOCK_PORT=<空闲端口> PROXY_PORT=<空闲端口> ADMIN_PORT=<同代理端口> BIFROST_DATA_DIR=<临时目录> bash e2e-tests/tests/test_unsafe_ssl_e2e.sh`，断言脚本自行启动 HTTPS mock，unsafe_ssl 动态切换相关 5 个用例全部通过。
- 运行 `bash -n scripts/run_all_e2e.sh` 并检查 `run_shell_tests_parallel` / `run_shell_batch_parallel` 末尾存在显式 `return 0`，断言并行 shell 调度器不会在全部子用例通过后把空闲轮询条件的 false 状态传播为 suite 失败。
- 运行 `PROXY_PORT=<空闲端口> MOCK_HTTP_PORT=<空闲端口> MOCK_SSE_PORT=<空闲端口> MOCK_WS_PORT=<空闲端口> BIFROST_DATA_DIR=<临时目录> SERVER_LOG_DIR=<临时目录> SKIP_BUILD=true bash e2e-tests/tests/test_replay_rules.sh`，断言 `SSE Replay with Rules` 在 `timeout_ms=5000` 后仍保持连接并收到 connection/applied_rules。

### 真实场景测试

- 更新 `human_tests/ci-shell-e2e-sharding.md`，覆盖 CI 不执行系统代理用例、隐藏日志 artifact 上传配置、失败原因摘要提取、shard 3 shell 包装回归、CLI offline help alternation 回归，以及 SSE replay timeout 边界回归。
- 按新增用例逐条执行，确认 CI 模式过滤、本地模式保留，失败日志可上传且摘要不会被 cleanup 尾巴覆盖，CLI offline help 断言不再误判，unsafe_ssl 用例不再依赖外部 HTTPS mock fixture，并行 shell 调度器全 PASS 后返回 0，SSE replay 在超过 `timeout_ms` 后仍保持连接。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 更新 `human_tests/ci-shell-e2e-sharding.md`
- 更新 `human_tests/readme.md`
