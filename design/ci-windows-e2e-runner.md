# CI Windows E2E Runner

## 功能模块说明

Windows `E2E Runner` job 负责在 `x86_64-pc-windows-msvc` 和 `aarch64-pc-windows-msvc` hosted runner 上执行 `scripts/ci/run-e2e-runner.sh`，该入口最终通过 `cargo run -p bifrost-e2e` 编译并运行自定义 E2E runner。

Windows `E2E Rules` job 负责执行 `e2e-tests/run_all_tests_parallel.sh` 的规则 fixture shard。该脚本在 Windows 上使用共享 mock servers，并把并行度强制降为 1，以避免多个 fixture 同时启动/停止 mock servers。

## 实现逻辑

- 历史 GitHub Actions 失败曾发生在 `E2E Runner (x86_64-pc-windows-msvc)` 的 `cargo run -p bifrost-e2e` 编译阶段。
- 该历史失败日志显示多个 Rust 工具链路径在并行编译早期同时触发 `rust-src` 下载，随后 rustup 报 `failed to install component: 'rust-src', detected conflict: 'lib\rustlib\src\rust\library\Cargo.toml'`。
- Windows E2E runner 的 `dtolnay/rust-toolchain@stable` 步骤显式声明 `components: rust-src`，让组件安装在 cargo 并行编译前由单个 Actions step 完成。
- `scripts/run_all_e2e.sh` 在解析 `CARGO_BIN` 后，如果调用方未显式设置 `RUSTC`，会通过 `rustup which rustc` 绑定同一当前工具链的真实 `rustc` 路径，并在 E2E Runtime Context 打印 `Rustc bin`。这避免 Windows Git Bash `PATH` 解析或 rustup shim fallback 把 Cargo 1.95 和 Rustc 1.65 混用，导致 `--check-cfg` 被旧 rustc 当作 unstable flag。
- PR #200 本轮失败发生在 `E2E Rules (x86_64-pc-windows-msvc, shard 2/4)` 的 retry 阶段，失败摘要为 `Mock 服务器未运行，但指定了 --skip-mock-servers`。
- Windows rules E2E 的失败重试会在每个失败 fixture 补跑前调用 `ensure_mock_servers_alive`，确认共享 mock servers 仍存活；如果补跑仍然命中 mock outage，会立即重启 mock servers 并对同一 fixture 做一次有界补跑。这样避免 `test_rules.sh --skip-mock-servers` 在重试阶段因共享 mock 掉线而把多个无关 fixture 误判失败。
- PR #249 的 CI 压测在 run `27580795070` attempt `2` 暴露新的 Windows Rules 不稳定因素：`E2E Rules (x86_64-pc-windows-msvc, shard 4/4)` 运行约 47 分钟后 GitHub 标记 `The hosted runner lost communication with the server`，且 job log blob 返回 404，导致 PAT 看护脚本 traceback。
- Windows rules 外层 `run_and_capture` 和内层 rules runner 都依赖 `e2e-tests/test_utils/process.sh` 回收超时子进程。Git Bash/MSYS 的 `$!` 可能是 POSIX PID，而 `taskkill.exe /PID` 需要 native Windows PID；直接把 `$!` 传给 taskkill 会让 timeout watchdog 无法可靠杀掉真实进程树，最终表现为 runner 长时间失联且后置日志 dump/upload 不执行。
- `process.sh` 在 Windows 下通过 `ps -p <pid> -o winpid=` / `ps -W` 提取 native PID 候选，再对候选执行 `taskkill /T /F`，同时保留原 PID fallback。这样内外层 1200/1260 秒预算能真正收敛超时路径并保留诊断日志。
- Windows Rules 从 4 个 shard 拆为 6 个 shard，单 shard 从约 29-30 个 fixture 降到约 19-20 个 fixture，降低单个 hosted runner 长时间串行执行 rules fixture 后失联的概率。
- GitHub Actions PAT 脚本在 job log API 或 signed blob 返回 404 时返回 `[github-actions-log-unavailable]` 诊断文本，并分类为 `github actions runner/log unavailable`，避免 fail-fast 看护在最需要诊断时 traceback。
- Linux/macOS runner 当前没有同类失败；本次仅收敛 Windows runner matrix，避免扩大 CI 变更面。

## 依赖项

- `.github/workflows/ci.yml`
- `scripts/ci/run-e2e-runner.sh`
- `scripts/run_all_e2e.sh`
- `e2e-tests/run_all_tests_parallel.sh`
- `crates/bifrost-e2e`

## 测试方案

### 单元测试

本次变更为 GitHub Actions workflow 配置，不修改 Rust 逻辑和公共函数，不新增 Rust 单元测试。通过 YAML 解析和 CI 实际运行验证。

### E2E 测试

- 静态解析 `.github/workflows/ci.yml`，确认 `e2e-windows-runner` job 存在。
- 检查该 job 的 `dtolnay/rust-toolchain@stable` step 显式包含 `components: rust-src`。
- 执行 `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build`，确认 Runtime Context 输出 Cargo/Rustc 路径且不启动任何 suite。
- 静态检查 `scripts/run_all_e2e.sh`，确认默认 `RUSTC` 来自 `rustup which rustc`，且 Runtime Context 输出 `Rustc bin`。
- 推送分支后观察 GitHub Actions `CI` workflow，确认 `E2E Runner (x86_64-pc-windows-msvc)` 不再在 `rust-src` component conflict 处失败。
- 静态检查 `e2e-tests/run_all_tests_parallel.sh`，确认 Windows rules E2E 的失败重试会在 fixture 补跑前确认 mock servers 存活，并对 mock outage 重试失败执行一次重启后补跑。
- 静态解析 `.github/workflows/ci.yml`，确认 `e2e-windows-rules` 拆为 6 个 x86_64 shard，内层 `BIFROST_E2E_RULE_RUNNER_TIMEOUT=1200`、外层 `suite_timeout=1260`、heartbeat 30 秒保持不变。
- 运行 `bash -n e2e-tests/test_utils/process.sh scripts/run_all_e2e.sh e2e-tests/run_all_tests_parallel.sh`，确认 Windows PID 回收 helper 的 Bash 语法有效。
- 运行 `python3 -m py_compile .trae/skills/github-actions-pat/scripts/common.py .trae/skills/github-actions-pat/scripts/gh_ci.py .trae/skills/github-actions-pat/scripts/watch_jobs.py`，确认 PAT 脚本语法有效。
- 对 run `27580795070` 执行 `gh_ci.py run 27580795070`，确认 job log blob 404 时输出 `github actions runner/log unavailable` 摘要而不是 traceback。

### 真实场景测试

- 新增 `human_tests/ci-windows-e2e-runner.md`。
- 执行静态 workflow 和脚本检查，验证 Windows E2E runner 的 toolchain step 已预安装 `rust-src`，且 E2E 入口会显式绑定 `RUSTC`。
- 更新 `human_tests/ci-windows-e2e-runner.md`，覆盖 Windows rules E2E 重试阶段 mock servers 掉线回归。
- 推送后使用 `github-actions-pat` 的 fail-fast watcher 观察最新 CI run。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核失败日志、workflow diff、设计文档和 human_tests；运行 YAML 解析与静态断言。
- 第 2 轮：复查第 1 轮修复后的最新 diff、索引同步和 CI watch 结果；必要时继续追加修复并重新推送观察。

## 校验要求

- `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'`
- 静态断言 `e2e-windows-runner` 的 toolchain step 包含 `components: rust-src`
- 静态断言 `scripts/run_all_e2e.sh` 通过 `rustup which rustc` 设置默认 `RUSTC`
- `bash -n e2e-tests/run_all_tests_parallel.sh`
- 静态断言 Windows rules E2E retry loop 在补跑前调用 `ensure_mock_servers_alive`，并在 mock outage 重试失败后重启 mock servers 再补跑一次
- 静态断言 Windows rules matrix 为 6 个 shard，且每个 shard 的 `rule_shard_total` 均为 `6`
- `python3 -m py_compile .trae/skills/github-actions-pat/scripts/common.py .trae/skills/github-actions-pat/scripts/gh_ci.py .trae/skills/github-actions-pat/scripts/watch_jobs.py`
- GitHub Actions 最新 `CI` run fail-fast watch

## 文档更新要求

- 更新 `human_tests/ci-windows-e2e-runner.md`
- 更新 `human_tests/rules-e2e-fixtures.md`
- 更新 `human_tests/ci-flake-hunt-10x.md`
- 更新 `human_tests/readme.md`
