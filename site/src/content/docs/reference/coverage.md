---
title: "测试覆盖率机制"
description: "从 docs/coverage.md 自动同步生成的 测试覆盖率机制 文档。"
editUrl: false
sidebar:
  label: "测试覆盖率机制"
  order: 905
---

> 此页面由 `docs/coverage.md` 自动同步生成。
# 测试覆盖率机制

本文档说明 Bifrost 如何在**完整测试金字塔**（单元测试、集成测试、端到端
（E2E）测试）范围内度量并强制约束测试覆盖率，以及推动每个 crate 达到
**90% 行覆盖率**目标的工作流。

## TL;DR

```bash
# 1. 度量全部（单元 + 集成），合并、输出 JSON、强制下限，
#    并精确打印需要补测的位置：
bash scripts/ci/coverage-all.sh --json --gate --gaps

# 2. 将 E2E 套件纳入合并后的覆盖率：
bash scripts/ci/coverage-all.sh --with-e2e --json --gate

# 3. 针对单个 crate 快速迭代：
bash scripts/ci/coverage-all.sh -p bifrost-core --json --gaps

# 4. 在本地打开 HTML 报告：
bash scripts/ci/coverage-all.sh -p bifrost-core --html
open target/coverage/html/index.html
```

## 度量哪些内容

| 层级 | 来源 | 如何采集 |
|---|---|---|
| **单元测试** | 各 crate 内部的 `#[cfg(test)]` 模块 | `cargo llvm-cov` 对 crate 插桩并运行 `cargo test` |
| **集成测试** | `crates/bifrost-tests` → 仓库根目录 `tests/*.rs` | 同一次 `cargo llvm-cov` 运行（它们是普通的 test target） |
| **E2E 测试** | `scripts/ci/run-e2e-*.sh` 驱动 `bifrost` + `bifrost-e2e` 二进制 | 二进制以**插桩**方式构建，其 `.profraw` 被合并进来（`--with-e2e`） |

所有层级都将 LLVM 性能剖析数据写入**同一个** target 目录，然后由一次
`cargo llvm-cov report` 合并。**只要*任一*层级覆盖到某一行，该行即记为已覆盖**
——因此合并后的数值反映了整个测试套件的真实覆盖范围，这正是 90% 目标的
度量基准。

## 机制——相关文件

| 文件 | 作用 |
|---|---|
| `scripts/ci/coverage-all.sh` | **入口。** 采集单元 + 集成（+ 可选 E2E）覆盖率，合并，输出 text/JSON/LCOV/HTML，可选执行门禁。 |
| `scripts/ci/coverage-gate.py` | **门禁与缺口分析。** 读取合并后的 JSON，将文件映射到 crate，强制每个 crate 与整个 workspace 的下限，并打印按优先级排序的缺口。 |
| `scripts/ci/coverage-thresholds.toml` | **契约。** 每个 crate 的行覆盖率*下限*（棘轮）+ workspace 目标。若覆盖率低于下限，CI 失败。 |
| `scripts/ci/coverage.sh` | 旧版仅单元测试的辅助脚本（仍可用）。 |
| `scripts/ci/coverage-e2e.sh` | 旧版仅 E2E 的辅助脚本（仍可用）。 |

> 前置条件：`rustup component add llvm-tools-preview` 与
> `cargo install cargo-llvm-cov`（脚本会在缺失时自动安装）。在多核机器上，
> 若插桩链接器针对较小的 `max locked memory` ulimit 为每个 CPU 各生成一个
> 线程，可能会崩溃——因此脚本通过 `--jobs`（默认 4）约束构建/链接并行度。
> 可用 `--jobs N` 或环境变量 `COVERAGE_JOBS` 覆盖。

## 90% 门禁与棘轮

`coverage-thresholds.toml` 声明：

* `settings.default`——项目目标（**90%**），用于任何没有显式下限的 crate
  以及缺口报告的目标行。
* `settings.workspace_min`——整个 workspace 的聚合下限。
* `[crates.<name>].min`——每个 crate 的**下限**。若该 crate 低于此值，门禁失败。

这些下限充当**棘轮**：它们以当前基线为初始值，使 CI 今天即可通过，并随着
测试的补充而调高。下限在没有明确理由的情况下绝不能下调——这正是 workspace
在不发生回退的前提下收敛到 90% 的方式。

## 工作流：将某个 crate 推到 90%

1. **找出缺口。**
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --json --gaps
   ```
   缺口报告会列出距目标最远的文件，按未覆盖行数排序（缺口最大者优先），
   并给出每个 crate 的“投入优先级”总分。

2. **看清哪些行是红色。** 生成 HTML 并打开对应文件：
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --html
   open target/coverage/html/index.html
   ```

3. **针对未覆盖的分支/函数补测。** 优先编写行为驱动的测试，而非单纯追行；
   合并后的报告同样会奖励真实的集成/E2E 覆盖。

4. **重新度量并调高下限。**
   ```bash
   bash scripts/ci/coverage-all.sh -p <crate> --json --gate
   ```
   一旦该 crate ≥ 90%，在 `coverage-thresholds.toml` 中设置
   `[crates.<crate>].min = 90.0`，使其永不回退。

5. **重复**，直到每个 crate 的下限都为 90——此时下限等于目标，workspace
   也就自然达到了 90%。

## CI 集成

`.github/workflows/ci.yml` 中的 `coverage` job：

1. 安装 `llvm-tools-preview` + `cargo-llvm-cov`，
2. 运行 `scripts/ci/coverage-all.sh --json --lcov --gate --gaps`，
3. 将 Markdown 摘要写入 GitHub job summary，并
4. 上传 `coverage.json` + `lcov.info` 作为 artifact（可供 Codecov、
   Coveralls 或本地查看使用）。

若任何 crate 或 workspace 聚合值低于其配置的下限，该 job 会**使构建失败**。
要提高标准，请补测并调高下限——绝不要为了让它通过而削弱门禁。
