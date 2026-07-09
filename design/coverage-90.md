# 单元 + 集成 + 可选 E2E 代码覆盖率机制（90% 门禁）设计方案

## 背景

Bifrost 早期覆盖率工具链分散：

- `scripts/ci/coverage.sh` 只跑 unit + integration，默认 `fail-under=0`，没有 Makefile 入口，也没有失败硬阻断能力。
- `scripts/ci/coverage-e2e.sh` 单独跑 E2E instrumented 二进制，输出独立报告，与 unit 报告无法合并。
- Makefile 没有 coverage 目标，用户只能手动记住脚本路径。
- AGENTS.md 中缺“90% 门禁”约束，导致执行心法层面没有覆盖率红线。

结果是：任何一个测试拿到的“覆盖率数字”都只反映测试金字塔的某一层，无法回答“加了这堆测试后，业务代码到底有多少被真实覆盖”这个问题；90% 目标也没有 CI 能强制到的门禁。

本方案统一 `crates/*` workspace 的单元 + 集成 + E2E 覆盖率入口，用 `coverage-thresholds.toml` 里的 per-crate 棘轮下限 + 工作区聚合下限做 CI 门禁，并在 Makefile 与 AGENTS.md 里把 90% 目标固化下来。

## 用户目标验证清单

### 必须实现

- 提供 `scripts/ci/coverage-all.sh`：一次性跑 unit + integration，`--with-e2e` 时合并 E2E instrumented profraw，输出 text / html / json / lcov。
- 提供 `scripts/ci/coverage-crate.sh`：按 crate 度量，本地迭代补测时快速反馈。
- 提供 `scripts/ci/coverage-gate.py`：解析 `coverage.json`，按棘轮下限 + 工作区聚合下限校验，支持 `--gaps` 输出最需要补测的文件列表。
- 提供 `scripts/ci/coverage-thresholds.toml`：per-crate 棘轮下限、工作区聚合下限、`default` 目标 90%、`enforce_ratchet_up` 与 `ratchet_slack` 配置。
- Makefile 增加 `coverage`（= `coverage-gate`）、`coverage-unit`、`coverage-e2e`、`coverage-html`、`coverage-json`、`coverage-crate CRATE=<name>`、`coverage-gate` 六个入口。
- AGENTS.md 增加 90% 门禁执行心法：任何改业务代码的任务必须通过 CI 覆盖率门禁；本地默认不跑全量 coverage，除非用户明确要求或专项排查覆盖率失败。
- `human_tests/coverage-mechanism.md` 覆盖机制本身的真实场景验证用例。

### 必须不破坏

- `scripts/ci/coverage.sh`、`coverage-e2e.sh` 保留原文件、原语义，作为底层拼装单元。
- `cargo llvm-cov` 命令行契约不改，`--all-features`、`--workspace` 等参数保持兼容。
- 本地 `make test`、`cargo test --workspace` 等既有入口不受影响。
- CI workflow 中原来的单元测试步骤保持存在；新增的 coverage 步骤单独一段，失败可以清楚定位。
- 冻结的 per-crate min 数字仅可上调、不可下调（棘轮）。

### 必须真实验证

- `bash scripts/ci/coverage-all.sh --json --gate --gaps` 在真实 workspace 上跑通并产出 `target/coverage/coverage.json`。
- 故意让某个 crate 覆盖率跌破 min → gate 报错、exit 非 0、GitHub Actions 阻断合入。
- `--with-e2e` 时 E2E profraw 与 unit profraw 合并成一份报告，text/html/json 里能看到 E2E 覆盖到的独占行。
- `make coverage-crate CRATE=bifrost-command` 本地能只跑目标 crate 并输出 text 报告。

## 产品语义

### 覆盖率来源三层

一次 `coverage-all.sh` 执行覆盖三个来源：

1. **单元测试**：`#[cfg(test)]` 与 `#[test]` 函数。
2. **集成测试**：`crates/*/tests/*.rs`、`tests/*.rs`（通过 `bifrost-tests` 汇聚）。
3. **E2E 测试（可选）**：`bifrost` / `bifrost-e2e` 插桩二进制在真实进程里跑 e2e 场景。

一行代码只要 **任意一层** 覆盖到就算 covered。合并后的报告才是“真实覆盖率”，与 90% 目标一致。

### 棘轮 vs 目标

- `[settings].default = 90.0` 是最终目标，所有 crate 逐步向 90 收敛。
- 每个 crate 有独立 `min` 作为棘轮下限，reflects 当前基线，不许回退。
- 补测 → 重跑 coverage → 上调 `min`；只能上调，PR 里下调 `min` 需要显式理由。
- `enforce_ratchet_up = false` 表示当前不强制“测得高于棘轮就必须同步上调”；开启后会防止大家忘了上调棘轮。
- `workspace_min = 65.0` 是工作区聚合下限，防止某些 crate 高、某些 crate 低时聚合值悄悄跌破。
- `macos_min` 是特定 crate 在 macOS 本地覆盖率的独立下限，用于承认平台差异。

### 90% 门禁的执行心法

写入 AGENTS.md：

- 任何改业务代码的任务，最终必须通过 CI 中的 `coverage-all.sh --json --gate` 门禁，否则任务不能视为完成。
- 默认情况下不要在本机运行 `make coverage` / `coverage-all.sh --gate`，因为全量覆盖率成本很高且 CI 已有明确阈值；只有用户明确要求、本地专项排查 coverage 失败、或需要提前确认某个高风险覆盖率缺口时才运行。
- 某个 crate 因为客观原因（macOS API、桌面 API、依赖硬件、依赖网络）达不到 90%，必须在本文档的 **不适用清单** 里写明理由，并在 `coverage-thresholds.toml` 维护对应 crate 的 min 例外。
- 本地专项排查时如 E2E 无法在当前环境跑通（无网络 / 无 Tauri / 无 macOS keychain），允许退化为 `make coverage-unit` 或单 crate coverage，但必须在交付报告里说明 E2E 跳过原因。

## 技术细节

### 目录结构

```
scripts/ci/
├── coverage.sh                 既有：unit + integration
├── coverage-e2e.sh             既有：E2E instrumented
├── coverage-all.sh             新增：合并入口 + --with-e2e + --gate
├── coverage-crate.sh           新增：单 crate 度量
├── coverage-gate.py            新增：棘轮 + 工作区聚合下限校验
└── coverage-thresholds.toml    新增：per-crate 棘轮下限

Makefile
├── coverage             = coverage-gate
├── coverage-unit        = coverage.sh
├── coverage-e2e         = coverage-e2e.sh
├── coverage-gate        = coverage-all.sh --json --gate --gaps
├── coverage-html        = coverage-all.sh --html --fail-under 0
├── coverage-json        = coverage-all.sh --json --fail-under 0
└── coverage-crate CRATE = coverage-crate.sh <CRATE> --text --fail-under 90

human_tests/
└── coverage-mechanism.md   新增：机制真实场景验证

design/
└── coverage-90.md          本文档
```

### coverage-all.sh 执行管线

```
1. cargo llvm-cov clean --workspace
2. cargo llvm-cov --workspace --all-features --no-report
      → 生成 unit + integration profraw
3. (--with-e2e) bash scripts/ci/coverage-e2e.sh
      → 生成 E2E instrumented profraw，落到共享 target
4. cargo llvm-cov report --text|--json|--lcov|--html
      → 输出合并报告
5. (--gate) python3 scripts/ci/coverage-gate.py target/coverage/coverage.json
```

关键约束：

- 用 `cargo llvm-cov run` 复用 workspace 的 target 目录，让 E2E profraw 直接落到 unit 报告目录下，`llvm-cov report` 能一次性合并。
- 通过 `raise_fd_limit` 提升 `ulimit -n`，避免 llvm profile 写入时因 FD 不足失败。
- 通过 `CARGO_BUILD_JOBS` / `RAYON_NUM_THREADS` 限制并发，避免 128 核机器 spawn 过多 rustc/link 进程把链接器 OOM。
- `--fail-under PCT` 直接透传给 `cargo llvm-cov`；90% 棘轮由 `coverage-gate.py` 单独执行，避免 llvm-cov 自身粗粒度阈值把棘轮语义搞混。

### coverage-gate.py

- 输入：`target/coverage/coverage.json`（llvm-cov 生成）。
- 解析：按 crate 汇总 line coverage；按目标 crate 与 `[crates.<name>]` 中的 `min` 比较；聚合值与 `[settings].workspace_min` 比较。
- 输出：文字报告 + 违规列表 + 建议命令（例如：跑 `make coverage-crate CRATE=<name>` 补测哪些文件）。
- `--gaps`：列出各 crate 内覆盖率最低的文件（含未覆盖行数），指导 test 补写方向。
- `enforce_ratchet_up=true`（未来）：如果测得值超出 `min + ratchet_slack`，同时报告未上调 `min`，gate 失败，强制大家把棘轮维持“紧贴”实际值。

### coverage-thresholds.toml

- `[settings]`：`default`（目标）、`workspace_min`（聚合下限）、`enforce_ratchet_up`、`ratchet_slack`。
- `[crates.<name>]`：`min`（Linux CI 棘轮）与可选 `macos_min`（macOS 本地棘轮）。
- 注释里保留“当前基线是多少 / 差距在哪 / 为什么达不到 90”，让下一次上调 min 时有背景。
- 例：`bifrost-tls` min=90 但注释里说明剩余 gap 是 macOS/Windows keychain 代码在 Linux CI 天然 cfg 剪枝。

### 当前基线快照（节选）

真实值以 `coverage-thresholds.toml` 为准；下表仅用于设计沟通：

| 范围 | 当前 min | 备注 |
|------|---------:|------|
| workspace 聚合 (`workspace_min`) | 65.0 | 保护聚合值不静默下滑 |
| default 目标 (`default`) | 90.0 | 所有 crate 逐步收敛 |
| `bifrost-command` | 90.0 | 已达 98.3% |
| `bifrost-tls` | 90.0 | Linux CI 91.5%；macOS 本地 `macos_min=78.0` |
| `bifrost-core` | 89.0 | Linux 89.4%；网络/`macOS` 代码降低聚合 |
| `bifrost-proxy` | 64.0 | wave 3-5 后基线 |
| `bifrost-admin` | 56.0 | wave 3-5 后基线 |
| `bifrost-storage` | 90.0 | Linux CI 94.5% |
| `bifrost-sync` | 90.0 | Linux CI 94.0% |
| `bifrost-power` | 84.0 | 剩余是 macOS-only IOKit |
| `bifrost-device` | 90.0 | Linux CI 92.4% |
| `bifrost-asr` | 94.0 | 当前基线 94.52% |
| `bifrost-script` | 91.0 | 当前基线 91.42% |
| `skills` | 90.0 | Linux CI 95.4% |
| `bifrost-e2e` | 50.0 | 测试运行器自身，棘轮随 e2e 扩容 |
| `agent` | 78.0 | 当前基线 78.78% |
| `bifrost-cli` | 55.0 | wave 3-5 后基线 |

### 不适用清单（客观阻塞）

| Crate | 现状 | 阻塞原因 |
|-------|------|----------|
| `bifrost-tests` | placeholder | workspace integration test 容器；测试落在其他 crate |
| `bifrost-e2e` | 测试运行器自身 | 不自测自己；`coverage-e2e.sh` 已通过 `--ignore-filename-regex=crates/bifrost-e2e/` 排除 |
| `bifrost-power` | 平台 hooks | macOS / Windows IOKit 分支 Linux 不可达 |
| `bifrost-device` | 平台特性 | macOS-only ioreg / Apple Configurator |
| `bifrost-asr` | 依赖 sherpa-onnx | 部分模型文件在 CI 缺失，用 mock 走通核心路径 |
| `bifrost-core` | 平台 + 网络 | `system_proxy_launchd.rs` macOS-only；`version_check.rs` 需要真实网络；`logging.rs` 全局 subscriber 单例 |

上述 crate 的覆盖率冻结值仍走同一个 gate，只是 min 值经过 justified 调低。

## CLI / Makefile / CI 触点

### 本地开发

- 迭代：`make coverage-crate CRATE=<changed-crate>` 快速看单 crate。
- 提交前默认不跑全量 `make coverage`；覆盖率棘轮由远端 CI `coverage-all.sh --json --gate` 兜底。
- 用户明确要求或专项排查 coverage 失败时：`make coverage`（= `coverage-gate`）确认棘轮不破。
- 需要浏览：`make coverage-html` 生成 `target/coverage/html/index.html`。
- 需要给 CI 上报：`make coverage-json`。

### CI（GitHub Actions / local-ci.sh）

- GitHub Actions workflow 必须运行 `bash scripts/ci/coverage-all.sh --json --gate` 并作为合入门禁。
- `local-ci.sh` 默认不运行全量 coverage；仅在用户明确要求或专项排查 coverage 失败时提供显式 coverage 入口，避免本机默认校验成本过高。
- 需要合并 E2E 覆盖率时显式追加 `--with-e2e`。
- 失败时 GitHub Actions 会在 job log 里打印违规 crate 与 gap 分析。

## 数据 / Sync 边界

- 覆盖率数据全部落在 `target/coverage/`，不进入 `data_dir`，不写入 sync。
- 阈值文件 `coverage-thresholds.toml` 是 repo 内的源代码，走普通 git 流程，不参与 rule sync / group sync。

## 实现切分

### Phase 1：机制脚本落地

- 新增 `coverage-all.sh`、`coverage-crate.sh`、`coverage-gate.py`、`coverage-thresholds.toml`。
- 保留 `coverage.sh` / `coverage-e2e.sh` 作为底层拼装。
- Makefile 增加 6 个 target。
- 用真实 workspace 跑通一次并把当前基线写进 thresholds。

### Phase 2：CI 集成

- GitHub Actions workflow 增加 `coverage-all.sh --json --gate` 步骤，并作为 PR / main 合入门禁。
- `scripts/ci/local-ci.sh` 保留显式 coverage 入口，但不把全量 coverage 放进默认本地校验路径。
- 覆盖率结果作为 artifact 上传（`coverage.json` / html 目录）。
- 失败时在 PR 上留链接。

### Phase 3：执行心法与文档

- AGENTS.md 增加 90% 门禁段落。
- `docs/coverage.md` / `site` 文档更新，说明 3 层来源与棘轮语义。
- `human_tests/coverage-mechanism.md` 覆盖机制自身回归。

### Phase 4：迭代收敛

- 定期把 crate min 上调（`bifrost-proxy` / `bifrost-admin` / `agent` / `bifrost-cli`）。
- 对客观阻塞 crate 在 “不适用清单” 里做完 justification 后维持不变。
- 观察 `enforce_ratchet_up=true` 打开的时机（默认关闭以避免噪音）。

## 测试方案

### 单元 / 脚本验证

- `bash scripts/ci/coverage-all.sh --json --output-dir target/coverage`：产出 `target/coverage/coverage.json`。
- `python3 scripts/ci/coverage-gate.py target/coverage/coverage.json`：在当前基线下 exit 0。
- `python3 scripts/ci/coverage-gate.py target/coverage/coverage.json --gaps`：打印 gap 报告。
- 故意把 `[crates.bifrost-command].min` 调到 99.9 → gate 失败并给出 diff。
- `make coverage-crate CRATE=bifrost-command`：单 crate 覆盖率通过 90 目标。

### E2E

- `bash scripts/ci/coverage-all.sh --with-e2e --json`：合并 E2E profraw；HTML 报告里能看到 E2E 独占覆盖行。
- CI workflow 内 `coverage-all.sh --json --gate` 步骤：绿灯 = 门禁生效。

### 真实场景测试（human_tests/coverage-mechanism.md）

- TC-COV-01：`make coverage` 通过并输出 `target/coverage/coverage.json`。
- TC-COV-02：`make coverage-crate CRATE=bifrost-command` 只跑目标 crate，text 报告 ≥ 90。
- TC-COV-03：`make coverage-html` 生成可打开的 html 报告。
- TC-COV-04：手动把某个 crate 的 min 调高一档，验证 gate 报错、exit 非 0。
- TC-COV-05：`--with-e2e` 合并报告，验证 E2E 独占行进入 covered 集合。

### 校验清单

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 远端 CI：`bash scripts/ci/coverage-all.sh --json --gate`
- 本地专项排查（非默认）：按需运行 `bash scripts/ci/coverage-all.sh --json --gate` 或单 crate coverage
- `bash scripts/ci/local-ci.sh --skip-e2e`（本地无 e2e 环境时）
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `coverage-all.sh`：unit / integration / E2E profraw 是否真的落到共享 target；`--fail-under` 与 `--gate` 语义是否分离清楚。
- 复核 `coverage-gate.py`：workspace 聚合、per-crate min、`macos_min`、`enforce_ratchet_up` 都走同一入口。
- 复核 `coverage-thresholds.toml`：min 与当前基线一致，注释解释达不到 90 的原因。
- 跑 `make coverage`、`make coverage-crate CRATE=<changed>`。

### 第 2 轮

- 复核 CI workflow：`--json --gate` 步骤能真正阻断合入，日志可读。
- 复核 AGENTS.md 更新：90% 门禁与不适用清单表述一致。
- 抽一条 `human_tests/coverage-mechanism.md` case 真实执行，记录结果。

## 风险与决策点

- **E2E 环境限制**：某些环境（无 Tauri、无 macOS keychain、无网络）跑不动 E2E。当前策略是允许 `make coverage-unit` 降级，并在交付报告写明；未来若 CI 全量支持 E2E，可考虑把 `--with-e2e` 变默认。
- **棘轮下调**：任何 PR 下调 `min` 视为红线，需要 reviewer 显式同意并在 PR 描述里说明原因。
- **`enforce_ratchet_up`**：默认关闭，等大家习惯棘轮流程后再考虑打开。打开后写测试补覆盖率必须同步上调 `min`，否则 gate 失败。
- **workspace 聚合门禁**：`workspace_min` 保护聚合值，但如果某个大 crate 被拆分或删掉，聚合值可能跳变；维护者需要在 refactor 时同步调整。
- **CI 时间成本**：`coverage-all.sh --with-e2e` 比普通 unit CI 慢很多，需要平衡；建议 PR 阶段跑 unit gate，主分支合并后跑 `--with-e2e`。
- **平台特异 min**：`macos_min` 只在 macOS 本地跑覆盖时校验，避免让 macOS-only 代码在 Linux CI 上被错误计入；对应 crate 的注释必须解释清楚。
- **thresholds 文件的变更节奏**：min 只允许 PR 内上调；下调需 justified；`workspace_min` 每次上调都要跑一次 `--gate` 确认聚合值稳定。
