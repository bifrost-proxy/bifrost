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

### TC-CWER-06: sync-server native 依赖安装可重试

**操作步骤**：
1. 检查 `scripts/ci/run-e2e-runner.sh`。
2. 确认脚本包含 `install_sync_server_dependencies` 函数，并在 sync-server `node_modules` 不存在时调用该函数。
3. 确认该函数设置 pnpm/npm fetch retry 与 timeout 环境变量。
4. 确认安装失败且仍有剩余尝试次数时，脚本会清理 `better-sqlite3` / `node-gyp` native package 半成品后重试。
5. 检查 `e2e-tests/test_utils/sync_server.sh`，确认 `sync_server_exec` 在 hardcoded mise Node fallback 前优先使用当前 PATH 中的 `node`。
6. 执行 shell 语法检查：
   ```bash
   bash -n scripts/ci/run-e2e-runner.sh e2e-tests/test_utils/sync_server.sh
   ```
7. 执行 runner E2E 入口：
   ```bash
   bash scripts/ci/run-e2e-runner.sh
   ```

**预期结果**：
- Windows E2E Runner 中 sync-server 依赖安装不再因一次 `better-sqlite3` prebuild 下载超时直接进入不可恢复失败。
- 本地或 CI 运行 sync-server 时，依赖安装使用的 Node 与运行 sync-server 的 Node 保持一致，避免 `NODE_MODULE_VERSION` ABI 不匹配。
- 脚本保留失败退出语义：多次重试仍失败时返回非 0，不隐藏真实依赖安装问题。
- `bash -n` 语法检查通过，runner E2E 入口可启动本地 sync-server 并完成 runner suite。

## 清理步骤

- 无本地清理需求；本测试不创建临时服务实例、不写入数据目录、不修改系统代理。

## 执行记录

- 2026-05-19：TC-CWER-01 通过 `ruby -e 'require "yaml"; ...'` 静态检查；TC-CWER-03 通过 `bash -n scripts/run_all_e2e.sh scripts/ci/run-e2e-runner.sh`、`rg -n 'rustup which rustc|Rustc bin|export RUSTC' scripts/run_all_e2e.sh` 和 `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build` 验证，Runtime Context 输出当前 Cargo/Rustc 真实路径且未启动任何 suite；TC-CWER-02 首次推送已越过 `rust-src` component conflict，但暴露 Cargo 1.95 / Rustc 1.65 混用导致的 `--check-cfg` 失败，最终结果由后续 GitHub Actions `CI` run 观察确认。
- 2026-06-08：TC-CWER-04 通过。执行 `bash -n e2e-tests/run_all_tests_parallel.sh` 通过；执行 `rg -n 'is_windows && ! ensure_mock_servers_alive|result_failure_mentions_mock_outage|重启 Mock 后补跑一次' e2e-tests/run_all_tests_parallel.sh` 命中 retry loop，确认 Windows rules E2E 会在每个失败 fixture 补跑前确认 mock servers 存活，并在 mock outage 重试失败后重启 mock servers 对同一 fixture 再补跑一次。远端验证由 PR #200 下一次 GitHub Actions `CI` run 的 Windows `E2E Rules (x86_64-pc-windows-msvc, shard 2/4)` 结果确认。
- 2026-06-13：TC-CWER-05 通过。执行 YAML 静态检查确认 `e2e-windows-runner.needs` 包含 `build-cli-windows`，并且 Windows Runner 在 tray smoke 前下载 `bifrost-release-${{ matrix.target }}` 到 `target/release`，`Tray startup smoke test` 通过 `BIFROST_BIN` 指向 `target/release/bifrost.exe`、设置 `SKIP_BUILD=true`，且 step-level timeout 为 10 分钟。
- 2026-07-05：TC-CWER-06 通过。执行 `bash -n scripts/ci/run-e2e-runner.sh e2e-tests/test_utils/sync_server.sh` 通过；执行 `rg -n 'install_sync_server_dependencies|npm_config_fetch_retries|better-sqlite3|node-gyp' scripts/ci/run-e2e-runner.sh` 命中安装重试函数、fetch retry/timeout 环境变量和 native package 半成品清理逻辑；执行 `rg -n 'command -v node|mise/installs/node/22' e2e-tests/test_utils/sync_server.sh` 确认 sync-server 启动优先使用当前 PATH node，再回退到 hardcoded mise Node。执行 `bash scripts/ci/run-e2e-runner.sh` 通过，启动本地 sync-server 并完成 runner suite。远端 Windows E2E Runner 最终结果由 PR #309 后续 GitHub Actions `CI` run 观察确认。
