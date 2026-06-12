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
├── coverage.sh           # 既有：unit + integration（保留）
├── coverage-e2e.sh       # 既有：E2E instrumented（保留）
├── coverage-all.sh       # 新增：unit + integration，--with-e2e 时合并 E2E，--gate 走棘轮门禁
└── coverage-crate.sh     # 新增：按 crate 度量，迭代用

Makefile
├── coverage              # = coverage-gate
├── coverage-unit         # = coverage.sh
├── coverage-e2e          # = coverage-e2e.sh
├── coverage-gate         # = coverage-all.sh --json --gate
├── coverage-html         # = coverage-all.sh --html
├── coverage-json         # = coverage-all.sh --json
└── coverage-crate CRATE= # = coverage-all.sh -p <CRATE> --json --gaps

human_tests/
└── coverage-mechanism.md # 新增：机制本身的真实场景验证用例

design/
└── coverage-90.md        # 本文档
```

## 工作流程

`coverage-all.sh` 的执行管线：

```
┌──────────────────────────────┐
│ 1. cargo llvm-cov clean -W   │
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
│ 4. cp E2E .profraw → llvm-cov │
│    target dir                 │
│ 5. cargo llvm-cov report      │
│      --no-run                 │
│      --fail-under-lines 90    │
└──────────────────────────────┘
           │
           ▼
   text / html / json / lcov
```

## 90% 门禁约束（写入 AGENTS.md）

- 任何会改动业务代码的开发任务，最终必须运行 `make coverage` 并保证全工作区
  行覆盖率 ≥ 90%，否则任务不能视为完成；
- 当某个 crate 因为外部依赖（硬件、桌面 API、平台特定能力等）天然无法达到
  90% 时，必须在 `design/coverage-90.md` 的「不适用清单」里写明原因，并把当
  次该 crate 的覆盖率冻结值写入 `--fail-under` 例外；
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

> 由 `make coverage-json` 输出 `target/coverage/coverage.json` 后，运行
> `python3 scripts/ci/coverage_freeze.py`（如需）回填。当前手工记录如下：

| 范围 | 行数 | 已覆盖 | 行覆盖率 |
| ---- | ---: | ---: | ---: |
| 待补测后由 CI 自动填充 | - | - | - |
