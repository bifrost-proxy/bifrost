# CI Windows E2E Runner

## 功能模块说明

验证 GitHub Actions Windows `E2E Runner` job 在运行 `cargo run -p bifrost-e2e` 前已经预安装 `rust-src`，且 E2E 入口显式绑定当前工具链的 `rustc`，避免并行编译早期触发 rustup component 安装竞争，或 Cargo 1.95 混用 Rustc 1.65 导致 `--check-cfg` 失败。

## 前置条件

- 工作目录：项目根目录 `<REPO_ROOT>`。
- 本用例只做 workflow 静态验证和远端 CI 观察，不启动本地 Bifrost，不修改系统代理。
- 远端 CI 观察需要 `GITHUB_TOKEN` 环境变量可用，并将 `GH_REPO` 设为 `bifrost-proxy/bifrost`。

## 测试用例

### TC-CWER-01: Windows runner toolchain 预安装 rust-src

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 定位 `jobs.e2e-windows-runner.steps`。
3. 找到 `uses: dtolnay/rust-toolchain@stable` 的 step。
4. 检查该 step 的 `with.components`。

**预期结果**：
- `.github/workflows/ci.yml` 可被 YAML 解析器读取。
- `e2e-windows-runner` job 存在。
- Windows E2E runner 的 `dtolnay/rust-toolchain@stable` step 包含 `components: rust-src`。

### TC-CWER-02: x86_64 Windows E2E runner 不再卡在 rust-src conflict

**操作步骤**：
1. 推送当前分支。
2. 查询当前分支最新 `CI` workflow run。
3. 使用 fail-fast watcher 观察该 run。
4. 若 `E2E Runner (x86_64-pc-windows-msvc)` 失败，拉取 job log 并检查是否仍出现 `failed to install component: 'rust-src', detected conflict`。

**预期结果**：
- 最新 `CI` run 不再因 `rust-src` component conflict 失败。
- 如果 CI 出现其他失败，失败日志应指向新的独立根因，而不是本用例覆盖的 rustup component conflict。

### TC-CWER-03: E2E 入口绑定当前 rustc

**操作步骤**：
1. 检查 `scripts/run_all_e2e.sh`。
2. 确认脚本在 `RUSTC` 未显式设置时调用 `rustup which rustc`。
3. 确认 Runtime Context 输出 `Rustc bin`。
4. 运行无 suite 模式：
   ```bash
   bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build
   ```

**预期结果**：
- `scripts/run_all_e2e.sh` 默认通过 `rustup which rustc` 设置 `RUSTC`。
- Runtime Context 包含 `Rustc bin`，便于 CI 日志诊断 Cargo/Rustc 是否匹配。
- 命令可正常退出，不启动 Bifrost 服务、不运行 runner。

### TC-CWER-04: Windows rules E2E retry 阶段重拉 mock servers

**操作步骤**：
1. 检查 `e2e-tests/run_all_tests_parallel.sh`。
2. 定位 `retry_failed_suites_once` 函数中的失败 fixture 串行重试 loop。
3. 确认 Windows 分支在每个失败 fixture 补跑前调用 `ensure_mock_servers_alive`。
4. 确认如果补跑后 `result_failure_mentions_mock_outage "$idx"` 为真，脚本会再次调用 `ensure_mock_servers_alive`，清理该 fixture 的临时结果后对同一 fixture 再补跑一次。
5. 执行 shell 语法检查：
   ```bash
   bash -n e2e-tests/run_all_tests_parallel.sh
   ```

**预期结果**：
- Windows rules E2E 不会只在 retry loop 开始前检查一次共享 mock servers。
- 如果 retry 阶段仍因 mock outage 失败，脚本会重启 mock servers 并对同一 fixture 做一次有界补跑。
- 该保护不改变普通规则断言失败的语义：非 mock outage 失败仍保留失败结果。
- `bash -n` 语法检查通过。

### TC-CWER-05: Windows E2E Runner tray smoke 复用 CLI artifact

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 检查 `e2e-windows-runner.needs`。
3. 检查 `e2e-windows-runner.steps` 中存在 `Download release binary`，artifact 名称为 `bifrost-release-${{ matrix.target }}`，下载路径为 `target/release`。
4. 检查 `Tray startup smoke test` step 设置：
   - `timeout-minutes=10`
   - `BIFROST_BIN=${{ github.workspace }}/target/release/bifrost.exe`
   - `SKIP_BUILD=true`

**预期结果**：
- Windows E2E Runner 等待并复用 `build-cli-windows` 已构建的 CLI artifact。
- `test_cli_tray_startup_ci.sh` 不再进入 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 分支。
- 该 job 后续如有编译，只应来自 `scripts/run_all_e2e.sh` 中 `cargo run -p bifrost-e2e` 对 runner harness 的编译，不应来自 tray smoke。

### TC-CWER-06: 失败重试保持 runtime worker 栈隔离

**操作步骤**：
1. 执行 runner retry 专项单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-e2e runner_tests -- --nocapture
   ```
2. 检查 `crates/bifrost-e2e/src/runner.rs` 的 retry loop，确认失败用例通过独立 Tokio task
   调用 `run_single_test()`，不在 `Runtime::block_on` 调用线程直接 await。
3. 重复执行 `retry_test_runs_on_runtime_worker_instead_of_block_on_thread` 至少 20 次。
4. 推送后观察 `E2E Runner (x86_64-pc-windows-msvc)`，确认
   `BIFROST_E2E_RETRY_FAILED_ONCE=1` 触发 retry 时不再出现 `STATUS_STACK_OVERFLOW`。

**预期结果**：
- retry attempt 所在线程与 `block_on` 调用线程不同。
- retry task panic 被转换成带原测试名的 failed result，runner 不丢失失败证据。
- Windows runner 即使首轮失败并补跑，也不会因主线程栈较小导致进程级 stack overflow。
- retry 次数、串行补跑顺序和 retry port 计算保持不变。

### TC-CWER-07: 完整 runner 失败重试路径覆盖

**操作步骤**：
1. 执行 `runner_retries_failed_test_once_and_replaces_the_result` 精确单测。
2. 确认 synthetic 用例首轮返回失败、第二次返回成功，并且 attempt counter 最终为 2。
3. 确认 `run_all()` 返回的最终结果为成功，而不是保留首轮失败结果。
4. 推送后检查 changed production Rust line coverage，确认 `runner.rs` 新增 retry 生产代码达到
   90% 变更行门禁。

**预期结果**：
- 完整 retry loop 实际执行 `TestCase` clone、retry port 等待和 worker task helper。
- 失败用例只补跑一次，最终结果槽位被补跑结果替换。
- 不通过降低 coverage 阈值或添加 coverage ignore 绕过门禁。

## 清理步骤

- 无本地清理需求；本测试不创建临时服务实例、不写入数据目录、不修改系统代理。

## 执行记录

- 2026-05-19：TC-CWER-01 通过 `ruby -e 'require "yaml"; ...'` 静态检查；TC-CWER-03 通过 `bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-runner.sh`、`rg -n 'rustup which rustc|Rustc bin|export RUSTC' scripts/run_all_e2e.sh` 和 `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build` 验证，Runtime Context 输出当前 Cargo/Rustc 真实路径且未启动任何 suite；TC-CWER-02 首次推送已越过 `rust-src` component conflict，但暴露 Cargo 1.95 / Rustc 1.65 混用导致的 `--check-cfg` 失败，最终结果由后续 GitHub Actions `CI` run 观察确认。
- 2026-06-08：TC-CWER-04 通过。执行 `bash -n e2e-tests/run_all_tests_parallel.sh` 通过；执行 `rg -n 'is_windows && ! ensure_mock_servers_alive|result_failure_mentions_mock_outage|重启 Mock 后补跑一次' e2e-tests/run_all_tests_parallel.sh` 命中 retry loop，确认 Windows rules E2E 会在每个失败 fixture 补跑前确认 mock servers 存活，并在 mock outage 重试失败后重启 mock servers 对同一 fixture 再补跑一次。远端验证由 PR #200 下一次 GitHub Actions `CI` run 的 Windows `E2E Rules (x86_64-pc-windows-msvc, shard 2/4)` 结果确认。
- 2026-06-13：TC-CWER-05 通过。执行 YAML 静态检查确认 `e2e-windows-runner.needs` 包含 `build-cli-windows`，并且 Windows Runner 在 tray smoke 前下载 `bifrost-release-${{ matrix.target }}` 到 `target/release`，`Tray startup smoke test` 通过 `BIFROST_BIN` 指向 `target/release/bifrost.exe`、设置 `SKIP_BUILD=true`，且 step-level timeout 为 10 分钟。
- 2026-07-14：TC-CWER-06 本地通过。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-e2e runner_tests -- --nocapture` 为 3/3 通过；worker 隔离专项随后连续执行 20 次均通过，panic 收敛专项确认返回具名 failed result。Windows x86_64 真实 retry 由 PR #386 下一次 CI run 补验。
- 2026-07-14：TC-CWER-07 本地通过。精确单测实际运行 1/1，制造首轮失败并由完整 `run_all()` 进入 retry，attempt counter 为 2，最终结果为 Passed；`coverage-all.sh -p bifrost-e2e` 生成 LCOV 后使用 CI 同款 `coverage-diff.py --threshold 95` 验证变更生产行 15/15（100%），门禁通过；远端由 PR #386 下一次 CI run 再次补验。
