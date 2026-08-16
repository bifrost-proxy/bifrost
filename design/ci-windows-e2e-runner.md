# CI Windows E2E Runner 稳定化设计方案

## 背景

Windows CI 的 E2E 分三条路径：

- `e2e-windows-runner` (`ci.yml` L1259–1339)：matrix 覆盖 `x86_64-pc-windows-msvc` (`windows-latest`) 与 `aarch64-pc-windows-msvc` (`windows-11-arm`)，通过 `scripts/ci/run-e2e-runner.sh` 最终 `cargo run -p bifrost-e2e` 编译并跑自定义 E2E runner。
- `e2e-windows-rules` (`ci.yml` L1150–1257)：只覆盖 `x86_64-pc-windows-msvc`，`rule_shard: 1/4..4/4` matrix，通过 `scripts/ci/run-e2e-rules.sh` 调用 `scripts/run_all_e2e.sh --ci --skip-shell --skip-runner --skip-ui --skip-build`，最终跑 `e2e-tests/run_all_tests_parallel.sh` 的规则 fixture shard；`BIFROST_E2E_RULE_JOBS: "2"`，`BIFROST_E2E_RETRY_FAILED_ONCE: "1"`。
- `e2e-windows-shell` (`ci.yml` L1098–1148)：只在 `windows-latest` 上跑 `test_upgrade_local_restart_e2e.sh`（Windows self-update replacement 专项）。

历史红灯集中在两处：

1. `E2E Runner (x86_64-pc-windows-msvc)` 的 `cargo run -p bifrost-e2e` 编译早期，rustup 报 `failed to install component: 'rust-src', detected conflict: 'lib\rustlib\src\rust\library\Cargo.toml'`——多个并行 cargo 进程同时触发 `rust-src` 下载。
2. `E2E Rules (x86_64-pc-windows-msvc, shard 2/4)` 的 retry 阶段失败摘要 `Mock 服务器未运行，但指定了 --skip-mock-servers`——共享 mock server 在补跑一批 fixture 前已经掉线，`test_rules.sh --skip-mock-servers` 直接把无关 fixture 全部误判失败。

同时 Windows Git Bash 的 `PATH` 解析与 rustup shim fallback 可能把 Cargo 1.95 与 Rustc 1.65 混用，让 `--check-cfg` 被旧 rustc 当作 unstable flag，直接编译失败。

## 用户目标验证清单

### 必须实现

**rust-src 预安装**

- `e2e-windows-runner` job 的 `dtolnay/rust-toolchain@stable` step 显式声明 `components: rust-src`（`ci.yml` L1295）。这样 `rust-src` 由单个 Actions step 在 cargo 并行编译前完成安装，避免多个 Cargo 同时触发下载的 component conflict。
- `x86_64-pc-windows-msvc` 与 `aarch64-pc-windows-msvc` 两个 matrix entry 都命中同一 toolchain step。

**RUSTC 绑定**

- `scripts/run_all_e2e.sh` 解析 `CARGO_BIN` 后，若调用方未显式设置 `RUSTC`，通过 `rustup which rustc` 绑定同一当前工具链的真实 `rustc` 路径，并在 E2E Runtime Context 打印 `Rustc bin`。
- 这避免 Windows Git Bash 上 Cargo 1.95 / Rustc 1.65 混用导致 `--check-cfg` 被旧 rustc 当作 unstable flag。

**Rules retry 阶段 mock 存活检查**

- Windows rules E2E 失败重试逻辑在每个失败 fixture 补跑前调用 `ensure_mock_servers_alive`，确认共享 mock server 仍存活。
- 若补跑仍然命中 mock outage，立即重启 mock servers，然后对同一 fixture 做一次有界补跑。
- 这样避免 `test_rules.sh --skip-mock-servers` 在重试阶段因共享 mock 掉线把多个无关 fixture 误判失败。

**Matrix 收敛**

- 本轮修复只动 Windows runner matrix；Linux / macOS runner 无同类失败，保持不变，避免扩大 CI 变更面。
- Windows rules matrix 仍只覆盖 `x86_64-pc-windows-msvc`（`windows-11-arm` runner 上 rules mock server / Python 依赖尚未验证）。

### 必须不破坏

- `scripts/ci/run-e2e-runner.sh` 保留 `start_e2e_sync_server` 逻辑，包括 pnpm install / sync-server build / 注册 e2e_runner_user 并 export `BIFROST_E2E_SYNC_BASE_URL` / `BIFROST_E2E_SYNC_TOKEN`。
- `scripts/ci/run-e2e-rules.sh` 保持 `bash scripts/run_all_e2e.sh --ci --skip-shell --skip-runner --skip-ui --skip-build` 一行入口。
- `e2e-windows-rules` matrix 的 `BIFROST_E2E_SUITE_TIMEOUT: 1260` / `BIFROST_E2E_RULE_RUNNER_TIMEOUT: 1200` / `BIFROST_E2E_HEARTBEAT_INTERVAL: 30` 等超时与心跳配置不改。
- `e2e-windows-runner` matrix 的 `BIFROST_E2E_RUNNER_JOBS`（x86_64: `8`，aarch64: `2`）不改。

### 必须真实验证

- 静态解析 `.github/workflows/ci.yml`：`e2e-windows-runner` job 的 toolchain step 精确出现 `components: rust-src`。
- 静态检查 `scripts/run_all_e2e.sh`：默认 `RUSTC` 来自 `rustup which rustc`，Runtime Context 输出 `Rustc bin`。
- `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build`：Runtime Context 输出 Cargo/Rustc 路径且不启动任何 suite。
- 静态检查 `e2e-tests/run_all_tests_parallel.sh`：Windows rules E2E 失败重试在 fixture 补跑前 `ensure_mock_servers_alive`，mock outage 重试失败后重启 + 有界补跑一次。
- 推送分支后 GitHub Actions `CI` workflow 的 `E2E Runner (x86_64-pc-windows-msvc)` 不再在 `rust-src` component conflict 处失败；`E2E Rules (x86_64-pc-windows-msvc, shard 2/4)` retry 阶段不再因共享 mock 掉线全批误判。

## 产品语义

本设计只影响 Windows CI 的 E2E job 稳定性，不改变 Bifrost 产品行为。

- 对 CI 维护者：新增 Windows runner 相关脚本时，`rustup which rustc` 会自动绑定同一 toolchain，本地跑 `bash scripts/run_all_e2e.sh` 也能看到 `Rustc bin` 输出。
- 对 rules 用例作者：Windows rules 失败重试逻辑对 mock outage 更 tolerant；无需在每个 fixture 内自检共享 mock。

## 技术细节

### `.github/workflows/ci.yml` — `e2e-windows-runner`

```yaml
e2e-windows-runner:
  name: E2E Runner (${{ matrix.target }})
  needs: [build-cli-windows]
  runs-on: ${{ matrix.os }}
  timeout-minutes: 60
  strategy:
    fail-fast: false
    matrix:
      include:
        - os: windows-latest
          target: x86_64-pc-windows-msvc
          runner_jobs: "8"
        - os: windows-11-arm
          target: aarch64-pc-windows-msvc
          runner_jobs: "2"
  env:
    BIFROST_DATA_DIR: ${{ github.workspace }}/.bifrost-e2e-ci
    BIFROST_E2E_REPORT_DIR: ${{ github.workspace }}/.e2e-reports
    BIFROST_E2E_RUNNER_JOBS: ${{ matrix.runner_jobs }}
    BIFROST_E2E_RETRY_FAILED_ONCE: "1"
    BIFROST_E2E_HTTP_RETRIES: "2"
    TIMEOUT: "90"
    PYTHONIOENCODING: utf-8
  steps:
    - uses: actions/checkout@v4
    - uses: pnpm/action-setup@v4
    - uses: actions/setup-node@v4
    - name: Install frontend dependencies
      # ...
    - uses: dtolnay/rust-toolchain@stable
      with:
        components: rust-src           # <-- 关键：单 step 预装
    # ...
    - name: E2E runner tests
      shell: bash
      run: bash scripts/ci/run-e2e-runner.sh
```

`components: rust-src` 让 rustup 在这一步串行完成组件安装；后续 `cargo run -p bifrost-e2e` 就不再触发下载。

### `.github/workflows/ci.yml` — `e2e-windows-rules`

matrix 4 shard，均 `runs-on: windows-latest`：

```yaml
env:
  BIFROST_E2E_RULE_SHARD_INDEX: ${{ matrix.rule_shard_index }}
  BIFROST_E2E_RULE_SHARD_TOTAL: ${{ matrix.rule_shard_total }}
  BIFROST_E2E_FIXTURE_TIMEOUT: ${{ matrix.fixture_timeout }}    # 90
  BIFROST_E2E_TLS_READY_TIMEOUT: ${{ matrix.tls_ready_timeout }}
  BIFROST_E2E_RETRY_FIXTURE_TIMEOUT: "60"
  BIFROST_E2E_RETRY_BUDGET_SECS: "180"
  BIFROST_E2E_MAX_RETRY_SUITES: "6"
  BIFROST_E2E_SUITE_TIMEOUT: ${{ matrix.suite_timeout }}         # 1260
  BIFROST_E2E_RULE_RUNNER_TIMEOUT: "1200"
  BIFROST_E2E_HEARTBEAT_INTERVAL: "30"
steps:
  - uses: actions/checkout@v4
  - uses: dtolnay/rust-toolchain@stable
  - uses: Swatinem/rust-cache@v2
    with: { key: e2e-${{ matrix.target }} }
  - name: Install jq
    shell: powershell
    run: choco install jq -y
  - name: Download release binary
    uses: actions/download-artifact@v4
    with: { name: bifrost-release-${{ matrix.target }}, path: target/release }
  - name: E2E rules tests
    shell: bash
    run: bash scripts/ci/run-e2e-rules.sh
```

失败重试与 mock 存活检查逻辑落在 `e2e-tests/run_all_tests_parallel.sh` 内。

### `scripts/ci/run-e2e-runner.sh`

关键片段（`start_e2e_sync_server`）：

```bash
sync_port="$(pick_unused_port)"
sync_url="http://127.0.0.1:${sync_port}"
SYNC_SERVER_DATA_DIR="$(mktemp -d)"
sync_log="${SYNC_SERVER_DATA_DIR}/sync-server.log"
sync_exec="$(sync_server_exec "$sync_server_dir")"

(
  cd "$sync_server_dir"
  eval "$sync_exec" -H 127.0.0.1 -p "$sync_port" -d "$SYNC_SERVER_DATA_DIR" --enable-remote-invoke
) >"$sync_log" 2>&1 &
SYNC_SERVER_PID=$!
```

后续注册 e2e_runner_user 并 export sync token 给 Rust E2E group-rule 用例。

### `scripts/ci/run-e2e-rules.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"
bash scripts/run_all_e2e.sh --ci --skip-shell --skip-runner --skip-ui --skip-build
```

### `scripts/run_all_e2e.sh` RUSTC 绑定

- `resolve_cargo_command` 挑出非 shim 的 `cargo`（尊重 `CARGO_BIN`）。
- 之后若 `RUSTC` 未设置，`RUSTC="$(rustup which rustc)"` 强制绑定同一 toolchain 下的 `rustc`。
- Runtime Context 打印 `Cargo bin:` / `Rustc bin:` / `Cargo version:` / `Rustc version:` 四行，便于 CI 日志排查。

### `e2e-tests/run_all_tests_parallel.sh` — Windows rules retry

- 每次 retry 前 `ensure_mock_servers_alive`：对共享 mock server 端口发 `TCP connect` + `HTTP GET /health`。
- 检查失败：`restart_shared_mock_servers`，再对失败 fixture 做一次有界补跑（预算 `BIFROST_E2E_RETRY_FIXTURE_TIMEOUT=60` 秒）。
- 若仍失败，写入真实 fixture 错误结果，不再全批标红。

## CLI / Web / Admin API / Sync 边界

- CLI：无新增。
- Web / Admin API：无变化。
- Sync：`scripts/ci/run-e2e-runner.sh` 内 `start_e2e_sync_server` 启动本地 sync-server，注册 `e2e_runner_user`，仅用于 E2E group-rule 测试。Sync payload / API 契约不变。

## 实现切分

### Phase 1 — rust-src 预安装

- 在 `e2e-windows-runner` 的 `dtolnay/rust-toolchain@stable` step 加 `components: rust-src`。

### Phase 2 — RUSTC 绑定

- 修改 `scripts/run_all_e2e.sh`：`RUSTC` 缺省时 `rustup which rustc`；Runtime Context 打印 `Rustc bin`。

### Phase 3 — Rules retry mock 存活

- 修改 `e2e-tests/run_all_tests_parallel.sh`：retry loop 前 `ensure_mock_servers_alive`，mock outage 场景重启后有界补跑。

### Phase 4 — human_tests

- 新增 / 更新 `human_tests/ci-windows-e2e-runner.md`，覆盖 rust-src 预安装、RUSTC 绑定、rules retry mock 存活。
- 更新 `human_tests/readme.md` 索引行。

## 测试方案

### 单元测试

CI workflow YAML + bash 脚本，无 Rust 逻辑变更。

### 集成 & E2E 测试

- 静态解析：`ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'`。
- 静态断言：`grep -A5 'e2e-windows-runner' -A80 .github/workflows/ci.yml | grep 'components: rust-src'`。
- 静态断言：`grep -n 'rustup which rustc' scripts/run_all_e2e.sh`。
- 静态断言：`grep -n 'ensure_mock_servers_alive' e2e-tests/run_all_tests_parallel.sh`。
- `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build`：Runtime Context 输出 `Cargo bin` / `Rustc bin`，不启动 suite。
- `bash -n scripts/ci/run-e2e-runner.sh scripts/ci/run-e2e-rules.sh scripts/run_all_e2e.sh e2e-tests/run_all_tests_parallel.sh`。
- 推送后使用 `github-actions-pat` fail-fast watcher 观察 `E2E Runner (x86_64-pc-windows-msvc)` 与 `E2E Rules (x86_64-pc-windows-msvc, shard 2/4)`。

### human_tests

- `human_tests/ci-windows-e2e-runner.md`：
  1. Windows runner toolchain step 预装 `rust-src`。
  2. `scripts/run_all_e2e.sh` 通过 `rustup which rustc` 绑定 RUSTC。
  3. Windows rules retry 阶段 mock outage 处理。
  4. 覆盖矩阵：x86_64 runner + aarch64 runner + rules 4 shard + shell upgrade restart。

## Review / Fix / Test 闭环

1. 第 1 轮：复核失败日志、workflow diff、`scripts/run_all_e2e.sh` diff、`e2e-tests/run_all_tests_parallel.sh` diff；跑 YAML 解析与静态断言。
2. 第 2 轮：PR 推送后 CI watch；若 aarch64 runner 出现新失败，追加针对性修复。
3. 第 3 轮：若 rules shard 仍偶发全批误判，扩展 `ensure_mock_servers_alive` 的健康检查或增加 mock 重启频率上限。

## 风险与决策

- 决策：只在 Windows E2E runner job 加 `components: rust-src`，不动 macOS/Linux——两平台历史无 component conflict，加进去反而增加 rustup 网络依赖。
- 决策：RUSTC 绑定通过 `rustup which rustc` 而非硬编码路径——尊重 rustup toolchain override，本地 rustup override 场景一致。
- 决策：rules retry mock 存活策略只在失败重试路径运行，正常首跑不加检查——首跑失败率极低，加检查反而拖慢平均耗时。
- 风险：`ensure_mock_servers_alive` 若误判 mock 存活（例如端口 accept 但 handler 卡住），仍会把 fixture 判错。缓解：健康检查带 HTTP 层探测而非只 TCP connect。
- 风险：Windows arm64 runner (`windows-11-arm`) 上 `dtolnay/rust-toolchain@stable` + `rust-src` 组合暂无长期数据；需在 PR 上验证。

## 依赖项

- `.github/workflows/ci.yml`
- `scripts/ci/run-e2e-runner.sh`
- `scripts/ci/run-e2e-rules.sh`
- `scripts/run_all_e2e.sh`
- `e2e-tests/run_all_tests_parallel.sh`
- `e2e-tests/test_utils/sync_server.sh`
- `crates/bifrost-e2e`（`cargo run -p bifrost-e2e`）
- Artifact：`bifrost-release-x86_64-pc-windows-msvc` / `bifrost-release-aarch64-pc-windows-msvc`

## 校验要求

- `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'`
- 静态断言 `e2e-windows-runner` 的 toolchain step 包含 `components: rust-src`
- 静态断言 `scripts/run_all_e2e.sh` 通过 `rustup which rustc` 设置默认 `RUSTC`
- `bash -n e2e-tests/run_all_tests_parallel.sh`
- 静态断言 Windows rules E2E retry loop 补跑前调用 `ensure_mock_servers_alive`，mock outage 重试失败后重启 mock servers 再补跑一次
- GitHub Actions 最新 `CI` run fail-fast watch

## 文档更新要求

- 更新 `human_tests/ci-windows-e2e-runner.md`
- 更新 `human_tests/readme.md`

## 2026-07-14 retry worker 栈稳定性补充

### 失败证据与根因

- PR #386 在 rebase 最新主干后的 CI run `29304684646` 中，
  `E2E Runner (x86_64-pc-windows-msvc)` 首轮执行失败用例后进入
  `BIFROST_E2E_RETRY_FAILED_ONCE`，日志在重试
  `remote_shell_exec_unix_shell_path_fallback` 时出现 `STATUS_STACK_OVERFLOW`。
- 首轮并发用例通过 `tokio::spawn` 在 runtime worker task 中执行；旧 retry loop 却直接在
  `Runtime::block_on(run_e2e())` 的调用线程上 await `run_single_test()`。Windows 主线程栈更小，
  因而同一用例可能首轮正常、重试才栈溢出。

### 修复约束

- retry attempt 必须通过独立 Tokio task 执行，保持与首轮并发执行相同的 worker 栈模型。
- spawned retry task panic 必须转换成具名 `TestResult::Failed`，不能丢失用例名或让 runner
  静默漏记结果。
- retry 仍然逐个执行、仍然只补跑一次，不扩大并发度，不改变原失败判定和端口分配规则。

### 验证

- `runner_tests::retry_test_runs_on_runtime_worker_instead_of_block_on_thread`：断言 retry 的线程
  与 `block_on` 调用线程不同。
- `runner_tests::retry_test_converts_spawned_task_panic_to_failed_result`：断言 task panic 被收敛为
  带原用例名的失败结果。
- `runner_tests::runner_retries_failed_test_once_and_replaces_the_result`：通过完整 `run_all()`
  制造首轮失败、重试通过，断言执行两次且最终结果被重试结果替换；同时覆盖 retry loop 的
  TestCase clone、端口等待和 worker helper 调用。
- runner 单测使用 synthetic standalone 用例验证 worker 隔离与 panic 收敛，确认 retry helper
  不在 `block_on` 线程内联执行；Windows x86_64 的完整 retry 路径由 GitHub Actions 最终补验。

CI run `29311063590` 的 changed-lines coverage gate 报告 retry loop 变更行仅覆盖 13/15
（86.67%），缺少第 256、260 行的完整路径覆盖。上述 full-run retry 回归用例专门关闭该缺口，
该次修复没有降低当时的 95% 变更行门禁，也没有对生产代码增加覆盖率排除。本地使用单 crate instrumentation 报告
和 CI 同款 `coverage-diff.py` 复核后，`runner.rs` 变更生产行覆盖率为 15/15（100%）。
