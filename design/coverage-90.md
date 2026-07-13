# 分层代码覆盖率与全面测试能力（90% 门禁）设计方案

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
- 覆盖报告必须区分 `unit + integration`、`E2E` 与 union，不能用未插桩
  不同 codegen profile 的二进制产生“看似合并、实际只有单测”的报告。
- 覆盖 E2E 必须使用仓库 worktree 内的隔离数据目录、隔离 HOME/XDG 目录和
  动态端口，且把正在运行的主服务端口 9900 设为禁止清理端口。
- 任一插桩 E2E 套件失败时，允许生成诊断报告，但覆盖命令最终必须失败。

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

一次 `coverage-all.sh --with-e2e` 执行覆盖三个来源，并保存三个可独立审计的
报告：

1. **单元测试**：`#[cfg(test)]` 与 `#[test]` 函数。
2. **集成测试**：`crates/*/tests/*.rs`、`tests/*.rs`（通过 `bifrost-tests` 汇聚）。
3. **E2E 测试（可选）**：`bifrost` / `bifrost-e2e` 插桩二进制在真实进程里跑 e2e 场景。

一行代码只要 **任意一层** 覆盖到就算 covered。合并后的报告才是“真实覆盖率”，与 90% 目标一致。

- `unit-integration.json`：只包含单元与进程内集成测试。
- `e2e.json`：只包含与单元测试使用同一 debug codegen profile 的显式插桩
  `bifrost` / `bifrost-e2e` 执行结果。
- `coverage.json`：上述 profile 的 union，也是 coverage gate 的输入。

这三份报告必须满足 `unit <= union`、`e2e <= union`。如果 E2E 没有产生
profraw、使用的二进制不可执行、数据目录落在 `~/.bifrost` 下，命令必须拒绝继续。

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
3. 保存 unit/integration profraw + `unit-integration.json`
4. (--with-e2e) `cargo llvm-cov show-env --sh` 后在同一 debug profile 构建 binaries
      → 显式导出 BIFROST_BIN / BIFROST_E2E_BIN
5. 在隔离 BIFROST_DATA_DIR + HOME/XDG + 动态端口中执行 E2E
6. 保存 E2E profraw + `e2e.json`，再恢复 unit profile
7. cargo llvm-cov report --text|--json|--lcov|--html
      → 输出 union `coverage.json`
8. (--gate) python3 scripts/ci/coverage-gate.py target/coverage/coverage.json
```

关键约束：

- 用 `cargo llvm-cov show-env --sh` 在与单元测试完全相同的 target/debug profile
  构建真正插桩的二进制；禁止混用 release/debug profile，因为两套 LLVM counter
  映射不兼容，也禁止让 E2E 回退到普通 `target/release/bifrost`。
- E2E runner 支持 `BIFROST_E2E_BIN`，覆盖任务不再通过 `cargo run` 隐式重编译
  一个来源不明的 runner。
- unit/integration 与 E2E profraw 分目录快照，生成各层报告后恢复到同一 target，
  由 `llvm-cov report` 生成 union。
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
| `bifrost-proxy` | 90.0 | production Rust；unit + integration + full E2E 合并后 90.01% |
| `bifrost-admin` | 56.0 | wave 3-5 后基线 |
| `bifrost-storage` | 90.0 | Linux CI 94.5% |
| `bifrost-sync` | 90.0 | Linux CI 94.0% |
| `bifrost-power` | 84.0 | 剩余是 macOS-only IOKit |
| `bifrost-device` | 90.0 | Linux CI 92.4% |
| `bifrost-asr` | 94.0 | 当前基线 94.52% |
| `bifrost-script` | 91.0 | 当前基线 91.42% |
| `skills` | 90.0 | Linux CI 95.4% |
| `bifrost-e2e` | exempt | 测试运行器自身；质量由可执行 Rules/Shell/Runner 契约约束，不统计自覆盖率 |
| `agent` | 78.0 | 当前基线 78.78% |
| `bifrost-cli` | 55.0 | wave 3-5 后基线 |

### 不适用清单（客观阻塞）

| Crate | 现状 | 阻塞原因 |
|-------|------|----------|
| `bifrost-tests` | placeholder | workspace integration test 容器；测试落在其他 crate |
| `bifrost-e2e` | 测试运行器自身 | `metric="exempt"` 显式排除 crate 与 workspace 百分比门禁；`coverage-e2e.sh` 也通过 `--ignore-filename-regex=crates/bifrost-e2e/` 排除，质量由 Rules/Shell/Runner 实际执行结果保证 |
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

### Phase 0：测量可信度（当前推进）

- 修复同 profile 插桩 binary 注入和 E2E failure propagation。
- 产出 `unit-integration.json`、`e2e.json`、`coverage.json` 三层证据。
- 新增全仓 Shell `bash -n` 门禁与覆盖管线契约 E2E。
- 增加生产数据目录拒绝、隔离 HOME/XDG 与 9900 protected-port 断言。
- macOS Shell E2E 按 `proxy-core`、`remote`、`agent-extensions` 三个稳定能力域分组；
  组内继续使用历史耗时权重平衡串行/并行 lane，CI 对三组估算 wall time 执行 15%
  最大偏差门禁，防止后续新增用例把某一能力 job 再次推到总超时边缘。
- CI 权重必须以成功 job 的单用例耗时日志定期重校，不能长期依赖初始估算。纯桌面
  Rust/Tauri 编译契约 `test_desktop_sidecar_launchd_env_contract.sh` 与
  `test_desktop_open_requests_contract.sh` 保留为本地桌面发布验证，不进入通用 macOS
  Shell CI；CI 继续通过桌面 bundle 编译生产路径，并把这两个例外放入显式跳过清单，
  防止它们在 Shell job 内重复编译 18-19 分钟。
- Rust cache 的 `save-if` 必须使用有效的常量布尔表达式 `${{ true }}`。禁止写成普通
  字符串 `always()`（不会保存），也不能把仅限条件判断的状态函数 `${{ always() }}`
  用在 action input 中（workflow 无法创建 job）；Windows release CLI 冷构建实测约 31 分钟。

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

- 定期把未达标 crate min 上调（`bifrost-admin` / `agent` / `bifrost-cli`）。
- 对客观阻塞 crate 在 “不适用清单” 里做完 justification 后维持不变。
- 观察 `enforce_ratchet_up=true` 打开的时机（默认关闭以避免噪音）。

### Phase 5：从行覆盖到功能完备

- 为 HTTP tunnel/handler、SOCKS TCP/UDP、WebSocket upgrade/capture 建立可注入的
  connector、DNS、clock、traffic store 与 fault injector，先把核心代理从 67%
  分波推进到 75% / 82% / 90%。
- 建立机器可读 capability matrix：协议、正常/异常/边界场景、平台、对应
  unit/integration/E2E/human test 和 CI job 必须一一映射。
- 对规则解析、URL/header/body 变换、WebSocket/SOCKS frame 增加 property/fuzz；
  对 shutdown、取消、session cleanup 增加受控并发测试。
- 下一波引入 ShellCheck 和 Bats；先对安装/升级与 coverage/CI 编排脚本建立函数级
  测试，再评估 kcov 的 Linux Shell 行覆盖门禁，避免一次引入全部存量告警。
- 每周/手动审计执行插桩全量 E2E、fuzz、mutation 与明确跳过项；release gate 执行真实
  安装、升级、证书、系统代理和外部 relay 场景。

### Phase 6：PR 增量门禁与持续分层覆盖（当前推进）

- PR coverage job 生成 LCOV 后执行 `coverage-diff.py`，只统计
  `crates/*/src/**/*.rs` 中发生变化且被 LLVM 标记为可插桩的生产行；内联
  `#[cfg(test)] mod tests` 必须从分子和分母中排除。
- changed-lines 最低门禁为 95%，高于 workspace 和 crate 历史棘轮，避免大型 crate
  依靠既有已覆盖代码吸收未测试新增逻辑。
- `e2e-tests/capabilities.json` 维护 P0/P1 代理能力的 owner、测试层、平台、失败模式和
  证据文件。P0 必须同时具备 unit、integration、E2E 与 Linux/macOS/Windows 证据。
- `scripts/run_all_e2e.sh` 每次结束生成 `summary.json`，固定记录 selected suite 的
  passed/failed/skipped、耗时、日志和跳过原因；Linux E2E artifact 无论成功失败都上传。
- 主 CI 增加阻断式 Playwright 关键能力矩阵，覆盖 Rules、Values、Scripts、Traffic、
  Breakpoint 和 Agent 新会话；完整 211 条历史套件进入每周审计并保留 artifact。在历史
  旧页面契约清零前，禁止把完整审计伪装成合入绿灯，也禁止让已知存量失败阻断所有 PR。
- PR 主 Coverage job 使用同一插桩 profile 执行 unit+integration、Rules、Runner 与 13 个
  代理核心 Shell 场景；每周/手动 `Layered E2E Coverage` 才执行完整 Rules、Shell、Runner，
  并上传 unit+integration、E2E-only、union 与 production 四份报告。PR 不重复串行 167 个
  Shell 场景，但仍以轻量代理集合执行 production 90% 绝对门禁。
  `bifrost-proxy` 使用 `metric="production"`：只排除 exact `#[cfg(test)]` item 及其外置
  module，生产分母不因测试辅助代码规模变化；当前证据为 19304/21446 = 90.01%。
- Shell 质量分为三层：全仓 `bash -n`、全仓 ShellCheck error gate、变更的
  `scripts/ci/*.sh` shfmt gate；ShellCheck 首次启用发现的 stdin redirection 和常量条件
  必须作为行为缺陷修复，禁止以全局 disable 绕过。
- WebSocket upgrade handshake 和双向 capture 使用可注入 duplex stream 做专项测试，
  覆盖 hop-by-hop header 过滤、mask 方向、双向 payload 与 handshake leftover。

## 测试方案

### 单元 / 脚本验证

- `bash scripts/ci/coverage-all.sh --json --output-dir target/coverage`：产出 `target/coverage/coverage.json`。
- `python3 scripts/ci/coverage-gate.py target/coverage/coverage.json`：在当前基线下 exit 0。
- `python3 scripts/ci/coverage-gate.py target/coverage/coverage.json --gaps`：打印 gap 报告。
- 故意把 `[crates.bifrost-command].min` 调到 99.9 → gate 失败并给出 diff。
- `make coverage-crate CRATE=bifrost-command`：单 crate 覆盖率通过 90 目标。

### E2E

- `bash scripts/ci/coverage-all.sh --with-e2e --json`：合并 E2E profraw；HTML 报告里能看到 E2E 独占覆盖行。
- `bash scripts/ci/coverage-all.sh --with-e2e --json --lcov --gate`：运行完整
  Rules / Shell / Runner，产出三层 JSON、LCOV 和 `production-coverage.json`；
  `bifrost-proxy` production 低于 90% 或任一 E2E 失败时最终退出非 0。
- `bash e2e-tests/tests/test_coverage_pipeline_contract.sh`：验证插桩二进制注入、
  分层 profile、生产目录拒绝、9900 protected port 与失败传播契约。
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
- 复核 `coverage-thresholds.toml`：`bifrost-proxy min=90` 且
  `metric="production"`，其余 min 与当前基线一致并解释例外原因。
- 跑 `make coverage`、`make coverage-crate CRATE=<changed>`。

### 第 2 轮

- 复核 CI workflow：`--json --gate` 步骤能真正阻断合入，日志可读。
- 复核 AGENTS.md 更新：90% 门禁与不适用清单表述一致。
- 抽一条 `human_tests/coverage-mechanism.md` case 真实执行，记录结果。

## 风险与决策点

- **E2E 环境限制**：某些本地环境（无 Tauri、无 macOS keychain、无网络）跑不动
  E2E；本地可用 `make coverage-unit` 迭代，但 PR 主 Coverage 必须运行 proxy E2E 子集并
  执行 production 90% 门禁，不能用本地降级结果代替。完整分层覆盖改为每周与手动审计。
- **棘轮下调**：任何 PR 下调 `min` 视为红线，需要 reviewer 显式同意并在 PR 描述里说明原因。
- **`enforce_ratchet_up`**：默认关闭，等大家习惯棘轮流程后再考虑打开。打开后写测试补覆盖率必须同步上调 `min`，否则 gate 失败。
- **workspace 聚合门禁**：`workspace_min` 保护聚合值，但如果某个大 crate 被拆分或删掉，聚合值可能跳变；维护者需要在 refactor 时同步调整。
- **CI 时间成本**：核心代理 90% 的分子包含 E2E 独占路径，因此 PR 主 Coverage 使用
  `--e2e-suite proxy` 只采集 Runner、Rules 与 13 个核心代理 Shell 场景；完整
  `coverage-all.sh --with-e2e` 仅每周/手动执行，避免每个 PR 重复约 84 分钟的全量串行审计。
- **平台特异 min**：`macos_min` 只在 macOS 本地跑覆盖时校验，避免让 macOS-only 代码在 Linux CI 上被错误计入；对应 crate 的注释必须解释清楚。
- **thresholds 文件的变更节奏**：min 只允许 PR 内上调；下调需 justified；`workspace_min` 每次上调都要跑一次 `--gate` 确认聚合值稳定。
