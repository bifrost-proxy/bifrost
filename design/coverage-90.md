# 单元 + 集成 + 可选 E2E 代码覆盖率机制（90% 门禁）

## 目标

1. 统一 `crates/*` 工作区的 **单元测试 + 集成测试** 覆盖率统计入口；
2. 把 E2E 套件运行时通过插桩 `bifrost` / `bifrost-e2e` 二进制采集到的
   覆盖率数据合并进同一份报告；
3. 用 `--gate`（基于 `coverage-thresholds.toml` 的 per-crate 棘轮下限 + 工作区聚合下限，目标 90%）在 CI / 本地校验阶段作为强制门禁；
4. 提供按 crate 单独度量的快速反馈通道，方便迭代式补测过程使用。

## 现有机制（修改前）

| 入口 | 范围 | 缺点 |
| ---- | ---- | ---- |
| `scripts/ci/coverage.sh` | unit + integration | 没有 E2E；默认 fail-under=0；没有 Makefile 目标 |
| `scripts/ci/coverage-e2e.sh` | E2E（instrumented binary） | 单独跑、单独出报告；和 unit 报告无法合并 |
| `Makefile` | 无 coverage 目标 | 用户需要手动调用脚本 |
| `AGENTS.md` | 没有 90% 门禁规则 | 无法在执行心法中强制 |

## 本次新增

```
scripts/ci/
├── coverage.sh                 # 既有：unit + integration（保留）
├── coverage-e2e.sh             # 既有：E2E instrumented（保留）
├── coverage-all.sh             # 新增：unit + integration，--with-e2e 时合并 E2E，--gate 走棘轮门禁
├── coverage-crate.sh           # 新增：按 crate 度量，迭代用
├── coverage-gate.py            # 新增：解析 coverage.json，按棘轮下限 + 工作区聚合下限校验，可输出 gap 分析
└── coverage-thresholds.toml    # 新增：per-crate 棘轮下限 + workspace_min 等门禁配置

Makefile
├── coverage              # = coverage-gate
├── coverage-unit         # = coverage.sh
├── coverage-e2e          # = coverage-e2e.sh
├── coverage-gate         # = coverage-all.sh --json --gate --gaps
├── coverage-html         # = coverage-all.sh --html --fail-under 0
├── coverage-json         # = coverage-all.sh --json --fail-under 0
└── coverage-crate CRATE= # = coverage-crate.sh <CRATE> --text --fail-under 90

human_tests/
└── coverage-mechanism.md # 新增：机制本身的真实场景验证用例

design/
└── coverage-90.md        # 本文档
```

## 工作流程

`coverage-all.sh` 的执行管线：

```
┌──────────────────────────────┐
│ 1. cargo llvm-cov clean      │
│      --workspace             │
│ 2. cargo llvm-cov            │
│      --workspace             │
│      --all-features          │
│      --no-report             │   → 生成 .profraw（unit + integration）
└──────────────────────────────┘
           │
           ▼
┌──────────────────────────────┐
│ 3. coverage-e2e.sh           │
│    （--with-e2e 时启用）      │   → 生成 .profraw（E2E instrumented）
└──────────────────────────────┘
           │
           ▼
┌──────────────────────────────┐
│ 4. 通过 cargo llvm-cov run    │
│    复用共享 target，使 E2E    │
│    profraw 直接落在合并目录   │
│ 5. cargo llvm-cov report      │
│      （--fail-under PCT 可选；│
│       90% 棘轮门禁由           │
│       coverage-gate.py 执行） │
└──────────────────────────────┘
           │
           ▼
   text / html / json / lcov
```

## 90% 门禁约束（写入 AGENTS.md）

- 任何会改动业务代码的开发任务，最终必须运行 `make coverage` 并保证全工作区
  行覆盖率 ≥ 90%，否则任务不能视为完成；
- 当某个 crate 因为外部依赖（硬件、桌面 API、平台特定能力等）天然无法达到
  90% 时，必须在 `design/coverage-90.md` 的「不适用清单」里写明原因，并在
  `scripts/ci/coverage-thresholds.toml` 中维护该 crate 的棘轮下限例外；
- E2E 套件不能跑通的环境（无网络/无 Tauri/无 macOS keychain）下允许使用
  `make coverage-unit` 退化为单元覆盖率 + 90% 门禁，但必须在交付报告里写明
  E2E 已跳过的原因。

## 不适用清单（已知客观阻塞）

| Crate | LOC | 现状 | 阻塞原因 |
| ---- | ---: | ---- | ---- |
| `bifrost-tests` | 1 | 仅 placeholder | 该 crate 是 workspace integration test 的容器；测试落在其他 crate |
| `bifrost-e2e` | 27k | 测试运行器自身 | 不应自测自己；coverage-e2e.sh 已通过 `--ignore-filename-regex=crates/bifrost-e2e/` 排除 |
| `bifrost-power` | 1.2k | 包含平台 hooks | 部分 macOS / Windows 平台代码在 Linux CI 不可达，覆盖率会被 cfg 自然过滤 |
| `bifrost-device` | 1.5k | 平台特性 | 同上 |
| `bifrost-asr` | 3.3k | 依赖 sherpa-onnx | CI 环境可能缺少模型文件；用 mock 走通核心路径 |

> 上述 crate 的覆盖率冻结值由 CI 维护人员在每次 `make coverage` 后更新到本
> 文档底部的「冻结表」。

## 与开发流程的对接

1. 任务规划阶段：`TodoWrite` 中必须出现 `Review/Fix/Test` 第 1/2 轮 +
   `coverage 90% 门禁` 项；
2. 实现阶段：迭代时跑 `make coverage-crate CRATE=<changed-crate>` 取得快速
   反馈；
3. 提交前：执行 `make coverage`（或 CI 等价命令）确认 ≥ 90%；
4. CI：在 `local-ci.sh` / GitHub Actions workflow 里把
   `bash scripts/ci/coverage-all.sh --json --gate` 作为必跑步骤，任一 crate 跌破
   其棘轮下限即阻断合入；需要合并 E2E 覆盖率时显式追加 `--with-e2e`。

## 冻结表（最近一次基线）

> 各 crate 的棘轮下限（floor）记录在 `scripts/ci/coverage-thresholds.toml`，由 `coverage-gate.py` 在每次 `make coverage` 时校验。补测后请同步上调对应 `min`，不要随意下调；下方汇总为最近一次基线（节选，详见 thresholds 文件）：

| 范围 | 棘轮下限 (line %) | 备注 |
| ---- | ----: | ---- |
| 工作区聚合 (`workspace_min`) | 55.0 | 由 `coverage-gate.py` 聚合校验 |
| `bifrost-command` | 90.0 | 已达 98.3% |
| `bifrost-tls` | 90.0 | Linux CI 实测 91.5% |
| `bifrost-core` | 89.0 | Linux 可达 95.9%；macOS-only / 网络代码降低聚合值 |
| `bifrost-proxy` | 59.0 | 当前基线 59.13%，待棘轮上调 |
| `bifrost-admin` | 49.0 | 当前基线 49.22%，待棘轮上调 |
| `bifrost-storage` | 90.0 | Linux CI 94.5% |
| `bifrost-sync` | 90.0 | Linux CI 94.0% |
| `bifrost-power` | 84.0 | Linux 84.5%，残余为 macOS-only IOKit |
| `bifrost-device` | 90.0 | Linux 92.4%，残余为 macOS-only ioreg |
| `bifrost-asr` | 94.0 | 当前基线 94.52% |
| `bifrost-script` | 91.0 | 当前基线 91.42% |
| `skills` | 90.0 | Linux CI 95.4% |
| `bifrost-e2e` | 50.0 | 测试运行器自身，随 e2e 用例增长棘轮上调 |
| `agent` | 78.0 | 当前基线 78.78% |
| `bifrost-cli` | 45.0 | 当前基线 45.67% |
