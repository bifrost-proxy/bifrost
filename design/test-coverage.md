# Test Coverage

## 背景

Bifrost 是一个多 crate workspace（bifrost-core / bifrost-proxy / bifrost-admin / bifrost-cli / bifrost-tls / bifrost-storage / bifrost-sync / bifrost-e2e / bifrost-asr / bifrost-script / bifrost-power / bifrost-device / bifrost-command / skills / agent 等）。为持续把测试覆盖率驱动到 90%，需要一套跨平台、可门禁、可 ratchet 的覆盖率体系：单元测试和集成测试合并统计；E2E 覆盖 被测二进制而非测试框架；CI 每次 push/PR 强制阈值，防止回退；本地一条命令跑同一路径便于开发者补齐 gap。

## 用户目标验证清单

### 必须实现

- `cargo-llvm-cov`（LLVM source-based instrumentation, v0.8+）作为唯一采集工具，跨 Linux / macOS / Windows 稳定 Rust 均能运行。
- `scripts/ci/coverage-all.sh` 统一采集单元 + 集成覆盖率，输出 `text` / `html` / `json` / `lcov`；同时支持 `--gate`（调 `coverage-gate.py`）、`--gaps`（打印 gap 榜）、`--fail-under PCT`（llvm-cov 直硬门）。
- `scripts/ci/coverage-gate.py` 按 `coverage-thresholds.toml` 强制：`[settings].default = 90.0` 项目目标、`workspace_min = 65.0` 聚合下限、每个 `[crates.<name>].min` ratchet floor（任一 crate 跌破即失败）。
- `scripts/ci/coverage-e2e.sh` 使用 `RUSTFLAGS=-C instrument-coverage` 插桩 `bifrost` / `bifrost-e2e` 二进制、`LLVM_PROFILE_FILE=<dir>/<name>-%p-%m.profraw` 收集 profraw、`llvm-profdata merge -sparse` 合并、`llvm-cov report/show/export` 出报告。
- `scripts/ci/coverage.sh` 提供本地便捷入口：全 workspace / 单 crate / html open / lcov / `--fail-under`。
- `scripts/ci/coverage-crate.sh` 单 crate 一次性封装（`text|json|html` + `--fail-under`）。
- CI `.github/workflows/ci.yml` `coverage` job：`taiki-e/install-action@cargo-llvm-cov` 装工具 → 跑 `coverage-all.sh --json --lcov --gate --gaps` → `coverage-gate.py … --markdown --no-gate` 写 `$GITHUB_STEP_SUMMARY` → 上传 `coverage-report-linux` artifact（`coverage.json` + `lcov.info`，7 天）。
- `scripts/ci/local-ci.sh --coverage` 已落地：`RUN_COVERAGE=1`；`--coverage-gate` 走 `coverage-all.sh --text --fail-under $COVERAGE_FAIL_UNDER`，`--coverage-html` 走 `coverage.sh --html --open`。
- 忽略规则：`.cargo/registry`、`rustc/`、`crates/bifrost-e2e/`（E2E 覆盖率中只关注被测代码，测试框架自身不算）。
- Ratchet 支持 `macos_min` 覆盖平台差异（bifrost-tls / bifrost-core / bifrost-power / bifrost-asr 均已配置），Linux CI 用 `min`，macOS 本地用 `macos_min`。

### 必须不破坏

- `cargo test --workspace --all-features` 结果不受插桩影响；覆盖率脚本失败时不能污染 `target/` 使普通 `cargo build` 变慢。
- 插桩产生的 `.profraw` 全部落在 `target/coverage/` 或 `--output-dir` 指定目录；不污染 `target/debug`。
- `coverage-gate.py` 只读取 `coverage.json`，不重跑 `cargo test`，不修改任何源码或 `Cargo.lock`。
- 覆盖率 gate 失败必须给出可操作输出（哪个 crate、当前 % vs floor、gaps 榜），不能只报 exit code。
- 现有 `cargo-tarpaulin` / `grcov` 未曾采用；不引入 nightly 依赖。

### 必须真实验证

- CI `coverage` job 在 push/PR 上真正跑通，阈值失败会红。
- `bash scripts/ci/coverage-all.sh --json --lcov --gate --gaps` 在本地 Linux 与 macOS 上都能出 `target/coverage/{coverage.json,lcov.info,html/}`。
- `coverage-e2e.sh --suite rules --html --fail-under 50` 真的会跑 bifrost-e2e 并生成 html 报告。
- `local-ci.sh --skip-e2e --coverage` 真实调用 `coverage.sh`；`--coverage-gate` 真实调用 `coverage-all.sh --text --fail-under`。

## 产品语义

### 两层覆盖率体系

- **单元 + 集成**：`cargo llvm-cov` 直接跑 workspace 的 `#[test]` + `tests/` 集成测试，输出 Region 级覆盖率。`coverage-all.sh` 是唯一合并入口。
- **E2E**：被测二进制自身要插桩（`RUSTFLAGS=-C instrument-coverage`），运行期通过 `LLVM_PROFILE_FILE` 落 `.profraw`，`llvm-profdata merge -sparse` → `llvm-cov` 出报告。`coverage-e2e.sh` 只关注被测代码，`crates/bifrost-e2e/` 本身在 ignore 列表里。

### Ratchet floors 只升不降

- `default = 90.0` 是项目目标；`workspace_min = 65.0` 是聚合工作区最低值。
- 每个 crate 的 `min` 是当前基线的 ratchet floor：加测试 → 重跑 → 上调，永不下调。
- `macos_min` 覆盖 macOS 上独有的平台代码路径（如 `system_proxy.rs` `#[cfg(target_os="macos")]`、IOKit acquire、Apple Configurator 检测），Linux CI 仍用严格 `min`。
- `enforce_ratchet_up`（默认 false）+ `ratchet_slack = 5.0`：一旦启用，实测超过 floor 超过 slack 而 floor 未更新即失败，强制"保持诚实"。

### Gap 分析驱动补测

`coverage-gate.py --gaps` 打印覆盖率最低的文件（按 uncovered 行数排序），告诉开发者最需要补测的地方；`--markdown` 生成 CI job summary 表格。

### 硬门 vs 软门

- `--fail-under PCT` 传给 `llvm-cov`，是"llvm-cov 层"硬门，只看聚合值。
- `--gate` 走 `coverage-gate.py`，读 `coverage-thresholds.toml`，逐 crate + workspace 双维度检查，是"业务层"门禁。CI 用 `--gate --gaps` 组合。

## 技术细节

### 关键源码

| 文件 | 责任 |
| --- | --- |
| `scripts/ci/coverage.sh` | 单 crate / 全 workspace 单元测试覆盖率便捷入口；支持 `--text/--html/--json/--lcov`、`--open`、`-p <crate>`、`--fail-under` |
| `scripts/ci/coverage-all.sh` | 统一入口：合并单元 + 集成覆盖率，输出多格式，可挂 `--gate/--gaps/--fail-under` |
| `scripts/ci/coverage-crate.sh` | 单 crate 覆盖率封装 |
| `scripts/ci/coverage-e2e.sh` | E2E 覆盖率：插桩二进制 + profraw 收集 + merge + report |
| `scripts/ci/coverage-gate.py` | 读 `coverage.json` + `coverage-thresholds.toml` 执行门禁；`--gaps` gap 榜；`--markdown` job summary |
| `scripts/ci/coverage-thresholds.toml` | `default = 90.0`、`workspace_min = 65.0`、每 crate `min` + `macos_min` |
| `scripts/ci/local-ci.sh` | `--coverage` / `--coverage-gate` / `--coverage-html` 开关（已上线） |
| `.github/workflows/ci.yml` | `coverage` job 定义（第 117 行起） |

### `coverage-thresholds.toml`（真实基线）

聚合 workspace 下限：**65.0%**；项目总目标：**90.0%**。以下为每个 crate 当前 ratchet floor（`min` 为 Linux CI 门槛，`macos_min` 为 macOS 本地门槛）：

| Crate | Linux `min` | macOS `macos_min` | 备注 |
| --- | ---: | ---: | --- |
| bifrost-command | 90.0 | — | measured 98.3% |
| bifrost-tls | 90.0 | 78.0 | Linux 91.5%；macOS 计入 install.rs keychain/certutil |
| bifrost-core | 89.0 | 87.0 | Linux 89.4%（可达 95.9%）；macOS 计入 system_proxy* |
| bifrost-proxy | 64.0 | — | wave 3-5 后从 59 提升 |
| bifrost-admin | 56.0 | — | wave 3-5 后从 49 提升 |
| agent | 78.0 | — | baseline 78.78% |
| bifrost-cli | 55.0 | — | wave 3-5 后从 45 提升 |
| bifrost-storage | 90.0 | — | reached 94.5% |
| bifrost-sync | 90.0 | — | reached 94.0% |
| bifrost-power | 84.0 | 74.0 | Linux 84.5%；macOS IOKit acquire |
| bifrost-device | 90.0 | — | reached 92.4%，ioreg / Configurator 走 macOS |
| bifrost-asr | 94.0 | 91.0 | baseline 94.52% |
| bifrost-script | 91.0 | — | baseline 91.42% |
| skills | 90.0 | — | reached 95.4% |
| bifrost-e2e | 50.0 | — | Linux baseline 50.68% |

### E2E 插桩细节

```bash
export RUSTFLAGS="-C instrument-coverage"
export LLVM_PROFILE_FILE="$PROFRAW_DIR/bifrost-%p-%m.profraw"
cargo build --release -p bifrost
export LLVM_PROFILE_FILE="$PROFRAW_DIR/e2e-%p-%m.profraw"
cargo run -p bifrost-e2e -- --suite "$SUITE"
unset LLVM_PROFILE_FILE
llvm-profdata merge -sparse "$PROFRAW_DIR"/*.profraw -o coverage.profdata
llvm-cov report --instr-profile=coverage.profdata --object target/release/bifrost
```

`%p` = PID、`%m` = binary id；确保多进程 / 多次运行不覆盖彼此的 profraw。

### CI Job（`.github/workflows/ci.yml` L117-156）

```yaml
coverage:
  ...
  - uses: taiki-e/install-action@cargo-llvm-cov
  - name: Collect unit + integration coverage and enforce gate
    run: bash scripts/ci/coverage-all.sh --json --lcov --gate --gaps
  - name: Write Markdown summary
    run: python3 scripts/ci/coverage-gate.py target/coverage/coverage.json \
           --markdown --no-gate >> "$GITHUB_STEP_SUMMARY"
  - name: Upload coverage reports
    uses: actions/upload-artifact@v4
    with:
      name: coverage-report-linux
      path: |
        target/coverage/coverage.json
        target/coverage/lcov.info
      retention-days: 7
```

`llvm-tools-preview` 组件由 `taiki-e/install-action` 自动附带；无需手动 `rustup component add`。

## CLI + Web + Admin API

覆盖率是 CI/开发者本地流程，不涉及 Bifrost 运行时 Admin API 与 Web UI，也不写入 `BIFROST_DATA_DIR`。相关 CLI 全部走 `bash scripts/ci/coverage*.sh`。

### 常用命令

```bash
# 本地全 workspace 单元测试覆盖率
bash scripts/ci/coverage.sh
bash scripts/ci/coverage.sh --open              # HTML 报告
bash scripts/ci/coverage.sh -p bifrost-core     # 单 crate
bash scripts/ci/coverage.sh --fail-under 70     # 硬门
bash scripts/ci/coverage.sh --lcov              # IDE 可导入

# 统一合并单元 + 集成覆盖率
bash scripts/ci/coverage-all.sh --json --lcov --gate --gaps
bash scripts/ci/coverage-all.sh --html --text
bash scripts/ci/coverage-all.sh --fail-under 65

# 单 crate 封装
bash scripts/ci/coverage-crate.sh bifrost-admin --html --fail-under 56

# E2E 覆盖率
bash scripts/ci/coverage-e2e.sh --html
bash scripts/ci/coverage-e2e.sh --suite rules --open
bash scripts/ci/coverage-e2e.sh --fail-under 50

# 通过 local-ci 一条命令跑
bash scripts/ci/local-ci.sh --skip-e2e --coverage
bash scripts/ci/local-ci.sh --skip-e2e --coverage-gate
bash scripts/ci/local-ci.sh --skip-e2e --coverage-html
```

## Sync 边界

覆盖率数据（`.profraw` / `coverage.json` / `lcov.info` / html）全部落在 `target/coverage/`，是本地 build artifact，不参与 Bifrost Sync / 导入导出 / 分享。`coverage-thresholds.toml` 是仓库源码的一部分，通过 git 提交与 CI 绑定；只有 PR 显式修改此文件才能调整 ratchet。

## Phase 1-4

### Phase 1：工具选型与基础脚本

- 确定 `cargo-llvm-cov` 为唯一采集工具，弃用 tarpaulin / grcov。
- 落 `scripts/ci/coverage.sh`（单元测试快速入口）、`scripts/ci/coverage-e2e.sh`（E2E 插桩）。

### Phase 2：统一采集 + 门禁

- 新增 `scripts/ci/coverage-all.sh` 合并 unit + integration。
- 新增 `scripts/ci/coverage-gate.py` + `coverage-thresholds.toml`，实现 workspace + 每 crate ratchet floor。
- CI `coverage` job 挂 `--gate --gaps`；`$GITHUB_STEP_SUMMARY` 输出 Markdown 表格。

### Phase 3：本地流程整合

- `scripts/ci/local-ci.sh` 新增 `--coverage` / `--coverage-gate` / `--coverage-html`；跑 `coverage-all.sh --text --fail-under $COVERAGE_FAIL_UNDER` 或 `coverage.sh --html --open`。
- `scripts/ci/coverage-crate.sh` 单 crate 便捷封装。

### Phase 4：Ratchet 推进 + 平台分线

- 每 wave 结束更新 `coverage-thresholds.toml`：wave 3-5 把 bifrost-proxy 59→64、bifrost-admin 49→56、bifrost-cli 45→55；`workspace_min` 55→65。
- macOS 与 Linux 分线：新增 `macos_min`（bifrost-tls / bifrost-core / bifrost-power / bifrost-asr），承认 IOKit / keychain / system_proxy 平台差。
- `enforce_ratchet_up` + `ratchet_slack = 5.0` 保留为未来"强制上调 floor"开关，默认关闭。

## 测试方案

### 单元 / 集成测试

- `cargo test --workspace --all-features` 走原路径，不受插桩影响。
- `bash scripts/ci/coverage-all.sh --json` → `target/coverage/coverage.json`，供 gate 消费。
- `python3 scripts/ci/coverage-gate.py target/coverage/coverage.json --gaps`：验证阈值 + 打印 gap。
- `python3 scripts/ci/coverage-gate.py --markdown --no-gate`：验证 Markdown 输出无异常。

### E2E 测试

- `bash scripts/ci/coverage-e2e.sh --suite rules --html`：验证 profraw → merge → report 全链路。
- 忽略列表验证：`crates/bifrost-e2e/` 自身不进入覆盖率统计。
- E2E 覆盖率 gate：`coverage-e2e.sh --fail-under 50`。

### CI 门禁

- `.github/workflows/ci.yml` `coverage` job：`coverage-all.sh --json --lcov --gate --gaps` 必须绿；一旦某 crate 跌破 `min` 立即失败。
- `Coverage (Unit + Integration) & 90% Gate` job name 在 branch protection 中作为 required check。

### 本地 CI 集成

- `bash scripts/ci/local-ci.sh --skip-e2e --coverage`：`RUN_COVERAGE=1` 分支跑 `coverage.sh --text`。
- `bash scripts/ci/local-ci.sh --skip-e2e --coverage-gate`：走 `coverage-all.sh --text --fail-under $COVERAGE_FAIL_UNDER`。
- `bash scripts/ci/local-ci.sh --skip-e2e --coverage-html`：走 `coverage.sh --html --open`。

### human_tests

无独立 human_tests 目录条目；覆盖率是 CI/开发者流程，不需要人工回归。重大 ratchet 上调随功能 PR 一并 review。

### 项目校验

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
bash scripts/ci/coverage-all.sh --json --lcov --gate --gaps
python3 scripts/ci/coverage-gate.py target/coverage/coverage.json --markdown --no-gate
```

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 diff：`coverage-thresholds.toml`（floor 变化必须有实测支撑）、`coverage*.sh`（新增 flag 保持向后兼容）、`coverage-gate.py`（`--gaps` / `--markdown` / `--no-gate` 行为一致）、`.github/workflows/ci.yml` `coverage` job。
- 重点 review：任何 crate 的 `min` 是否只升不降；`macos_min` 是否与 Linux `min` 语义一致（都是"低于即失败"）；E2E 忽略列表是否漏掉新增 test 目录。
- 复测：`coverage-all.sh --json --lcov --gate --gaps` 本地跑一遍；抽一个刚上调 floor 的 crate 单独 `coverage-crate.sh <name> --json --fail-under <new_min>`。

### 第 2 轮

- 复核第 1 轮修复。
- 再次 `git diff coverage*`，确保 CI job 与本地脚本调用参数一致。
- 重点 review：`workspace_min` 上调（55→65）是否有 3 个 wave 的实测支撑；`enforce_ratchet_up` 是否被意外开启导致 CI 抖动。
- 复测：CI `coverage` job 在 draft PR 上手动 rerun；`local-ci.sh --coverage-gate` 本地实测。

## 风险与决策

- **平台差异**：`system_proxy.rs` / `system_proxy_launchd.rs` / IOKit / keychain / certutil 只能在 macOS 覆盖，Linux CI 会把这些行计为 uncoverable。`macos_min` 是平台差异的合法出口；不允许为了 Linux 绿而把 `min` 下调。
- **E2E 覆盖率**：`crates/bifrost-e2e/` 是测试框架，必须在忽略列表；否则会污染被测代码覆盖率。
- **profraw 冲突**：`LLVM_PROFILE_FILE` 必须包含 `%p-%m`，多进程 / 多次 run 才不覆盖；`coverage-e2e.sh` 已固化。
- **CI job artifact 保留期**：只留 7 天，超期只能重跑；`coverage.json` + `lcov.info` 两个即可，`html/` 不上传（体积太大）。
- **ratchet 抖动**：某 crate 由于测试的浮点/时间敏感行为，测得值可能在 ±0.5% 抖动；`min` 设成实测 - 1% 是安全边距；不需要 `ratchet_slack` 缓冲。
- **`enforce_ratchet_up` 默认关闭**：一旦开启，CI 上任何超过 floor + slack 的实测都会失败提示"更新 floor"；这是"强制诚实"开关，接受一定 review 成本再开。
- **弃用工具**：cargo-tarpaulin 仅 Linux x86_64；grcov 需 nightly；两者都不满足跨平台稳定 Rust 要求，不引入。

## 依赖项

- `cargo-llvm-cov` >= 0.8：`cargo install cargo-llvm-cov` 或 `taiki-e/install-action@cargo-llvm-cov`。
- `llvm-tools-preview` rustup component：`rustup component add llvm-tools-preview`（CI 由 install-action 自动附带）。
- Python 3（`coverage-gate.py`），只依赖标准库 `tomllib`（Python 3.11+）/ 回退 `tomli`。
- `.github/workflows/ci.yml` `coverage` job 绑定 branch protection required check。
- 本地 `bash` + GNU coreutils；`scripts/ci/coverage*.sh` 未使用 macOS 独有 flag，Linux/macOS 通吃。
