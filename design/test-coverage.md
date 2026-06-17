# 测试覆盖率检测方案

## 概述

为 Bifrost 项目引入基于 LLVM source-based instrumentation 的测试覆盖率检测，覆盖单元测试和端到端测试两个维度。

## 工具选型

| 工具 | 方案 | 选择原因 |
|------|------|----------|
| **cargo-llvm-cov** | LLVM source-based | ✅ 精度最高（Region 级别），稳定 Rust 即可运行，支持 `cargo test` / `cargo run` / nextest，原生支持多次运行合并 |
| cargo-tarpaulin | ptrace-based | ❌ 仅 Linux x86_64，macOS/Windows 不支持 |
| grcov | GCNO/GCDA | ❌ 依赖 nightly，配置复杂 |

**最终选择：cargo-llvm-cov v0.8+**

## 架构设计

### 两层覆盖率体系

```
┌─────────────────────────────────────────────────────┐
│                  Coverage Pipeline                   │
├─────────────────────┬───────────────────────────────┤
│  单元测试覆盖率      │  E2E 测试覆盖率               │
│  coverage.sh         │  coverage-e2e.sh              │
├─────────────────────┼───────────────────────────────┤
│  cargo llvm-cov      │  RUSTFLAGS=-C instrument-     │
│  --workspace         │    coverage                   │
│  --all-features      │  + llvm-profdata merge        │
│                      │  + llvm-cov report            │
├─────────────────────┼───────────────────────────────┤
│  验证：函数/模块逻辑  │  验证：运行时代码路径          │
│  覆盖粒度：Region     │  覆盖粒度：Region             │
└─────────────────────┴───────────────────────────────┘
```

### 单元测试覆盖率

直接使用 `cargo llvm-cov` 对 workspace 内所有 `#[test]` 函数运行并统计覆盖率。

**关键特性：**
- 支持 text / HTML / LCOV / JSON 多种输出格式
- 支持 `--fail-under-lines` 门禁，覆盖率低于阈值则失败
- 支持 `-p` 参数对单个 crate 做精细分析
- LCOV 格式可对接 Codecov / Coveralls / IDE 插件

### E2E 测试覆盖率

E2E 测试覆盖率需要对 **被测二进制**（bifrost server）进行插桩，而非测试框架本身。

**核心流程：**

1. **插桩编译**：使用 `RUSTFLAGS="-C instrument-coverage"` 编译 bifrost 和 bifrost-e2e 二进制
2. **收集 profraw**：通过 `LLVM_PROFILE_FILE` 环境变量，运行期间每个进程生成 `.profraw` 文件
3. **合并数据**：使用 `llvm-profdata merge -sparse` 合并所有 `.profraw` 文件
4. **生成报告**：使用 `llvm-cov report/show/export` 生成覆盖率报告

**忽略规则：**
- `.cargo/registry` — 第三方依赖
- `rustc/` — 标准库
- `crates/bifrost-e2e/` — 测试框架本身的代码（在 E2E 覆盖率中只关注被测代码）

## 文件清单

| 文件 | 用途 |
|------|------|
| `scripts/ci/coverage.sh` | 单 crate / 自定义参数的单元测试覆盖率脚本 |
| `scripts/ci/coverage-all.sh` | 统一脚本：合并单元 + 集成 (+ 可选 E2E) 覆盖率到单一报告 |
| `scripts/ci/coverage-crate.sh` | 单 crate 覆盖率便捷封装 |
| `scripts/ci/coverage-e2e.sh` | E2E 测试覆盖率脚本（插桩 `bifrost` / `bifrost-e2e` 二进制） |
| `scripts/ci/coverage-gate.py` | 读取 `coverage.json` 并按 `coverage-thresholds.toml` 执行门禁 / 输出 gaps / Markdown summary |
| `scripts/ci/coverage-thresholds.toml` | 全局目标（default = 90.0）、workspace 聚合下限、每个 crate 的 ratchet 下限 |
| `.github/workflows/ci.yml` (coverage job) | CI 自动覆盖率检测（push / PR 触发，执行 `coverage-all.sh --json --lcov --gate --gaps`） |
| `scripts/ci/local-ci.sh` (--coverage 选项) | 本地 CI 集成覆盖率（planned, not yet shipped as of 2026-06-17：当前 `local-ci.sh` 未实现 `--coverage` 开关，本地请直接调用 `coverage-all.sh`） |

## 使用方式

### 本地快速检查

```bash
# 全 workspace 单元测试覆盖率（终端输出）
bash scripts/ci/coverage.sh

# 生成 HTML 报告并在浏览器打开
bash scripts/ci/coverage.sh --open

# 单个 crate 覆盖率
bash scripts/ci/coverage.sh -p bifrost-core

# 设置门禁：低于 70% 则失败
bash scripts/ci/coverage.sh --fail-under 70

# 生成 LCOV 格式（可导入 IDE）
bash scripts/ci/coverage.sh --lcov

# 统一脚本：合并单元 + 集成测试覆盖率，输出 JSON / LCOV / HTML 并执行门禁
bash scripts/ci/coverage-all.sh --json --lcov --gate --gaps
bash scripts/ci/coverage-all.sh --html --text

# 通过 local-ci 运行（包含 fmt/clippy/test + 覆盖率）
# planned, not yet shipped as of 2026-06-17：local-ci.sh 暂无 --coverage 开关
bash scripts/ci/local-ci.sh --skip-e2e --coverage
bash scripts/ci/local-ci.sh --skip-e2e --coverage-html
```

### E2E 覆盖率

```bash
# 全量 E2E 覆盖率
bash scripts/ci/coverage-e2e.sh --html

# 指定 suite
bash scripts/ci/coverage-e2e.sh --suite rules --open

# 设置门禁
bash scripts/ci/coverage-e2e.sh --fail-under 50
```

### CI 集成

`push` (main) / `pull_request` 自动触发 `coverage` Job（`Coverage (Unit + Integration) & 90% Gate`）：
1. 使用 `taiki-e/install-action@cargo-llvm-cov` 安装工具，并附加 `llvm-tools-preview` 组件
2. 运行 `scripts/ci/coverage-all.sh --json --lcov --gate --gaps` 收集合并的单元 + 集成覆盖率，并按 `coverage-thresholds.toml` 中的 ratchet 下限执行门禁
3. 通过 `scripts/ci/coverage-gate.py … --markdown --no-gate` 写入 `$GITHUB_STEP_SUMMARY` 展示覆盖率摘要
4. 上传 `coverage.json` + `lcov.info` 作为 `coverage-report-linux` artifact（7 天有效期）

## 当前覆盖率基线 (ratchet floors, source: `scripts/ci/coverage-thresholds.toml`, as of 2026-06-17)

聚合 workspace 下限：**55.0%**；项目总目标：**90.0%**。下表为每个 crate 的 ratchet floor（实际测量值通常略高）：

| Crate | Floor (line%) | 备注 |
|-------|--------------:|------|
| bifrost-command | 90.0 | measured 98.3% |
| bifrost-tls | 90.0 | reached 91.5% on Linux CI |
| bifrost-core | 89.0 | reached 89.4% (reachable 95.9%) |
| bifrost-proxy | 59.0 | baseline 59.13% |
| bifrost-admin | 49.0 | baseline 49.22% |
| agent | 78.0 | baseline 78.78% |
| bifrost-cli | 45.0 | baseline 45.67% |
| bifrost-storage | 90.0 | reached 94.5% |
| bifrost-sync | 90.0 | reached 94.0% |
| bifrost-power | 84.0 | Linux CI baseline 84.5% |
| bifrost-device | 90.0 | reached 92.4% |
| bifrost-asr | 94.0 | baseline 94.52% |
| bifrost-script | 91.0 | baseline 91.42% |
| skills | 90.0 | reached 95.4% |
| bifrost-e2e | 50.0 | Linux CI baseline 50.68% |

## 渐进式门禁策略

采用"只升不降"的 ratchet 策略，由 `coverage-thresholds.toml` + `coverage-gate.py` 强制执行：
1. `[settings].default = 90.0`：项目总体目标。
2. `[settings].workspace_min = 55.0`：聚合 workspace 必须达到的下限。
3. `[crates.<name>].min`：每个 crate 的 ratchet floor；任一 crate 跌破即 fail。
4. 添加测试 → 重新跑 `coverage-all.sh --json` → `coverage-gate.py … --gaps` 找出最缺测试的文件 → 上调 floor。
5. 目标：所有 crate floors 最终达到 90%。
6. （planned, not yet shipped as of 2026-06-17）`enforce_ratchet_up`：若实测高出 floor 超过 `ratchet_slack` 但 floor 未更新即失败，默认关闭。

## 依赖项

- `cargo-llvm-cov` >= 0.8（`cargo install cargo-llvm-cov`）
- `llvm-tools-preview` rustup component（`rustup component add llvm-tools-preview`）
- 脚本自动检测并安装缺失依赖
