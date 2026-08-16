# Bifrost 项目开发规则

## 执行心法

**任何人都会犯错，但我们可以通过反复检查、持续改进，终将达到巅峰水平。** Agent 必须把 review、修复和复测视为高质量交付的常态，而不是额外负担；任何没有证据支撑的“已完成”“应该可以”“看起来没问题”都不能作为完成依据。

## 执行模式与完成定义

### 任务模式判定

1. **开发模式（默认）**：用户要求实现、修复、优化、改造、补齐、提交、推送，或要求 Agent 自主完成时，必须执行本文的通用交付闭环，并按“变更类型与验证路由”只选择适用的专项流程；除非用户明确说“不要提交 / 不要推送 / 不用建 MR / 不用盯 CI”，否则开发完成后必须默认提交、推送、创建或更新 MR/PR，并跟进适用的远端 CI 到全绿或给出明确阻塞归因。
2. **检查模式（只读）**：用户明确说“仅检查 / 不要修改代码 / 只 review / 先分析”时，只能读取、运行安全检查并给出 findings；未经用户明确同意不得编辑文件、提交或推送。
3. **CI 闭环模式**：用户要求分析 CI、修到 CI 绿、推送后盯 CI、进入 fix-push-watch，或任一开发任务完成后已 push 需要默认远端 CI 看护时，必须加载 `.agents/skills/github-actions-pat/`，并按 GitHub Actions CI 分析与 fix-push-watch 闭环执行。
4. **文档/流程变更模式**：仅修改文档、流程或测试说明时，只执行格式、链接、结构和内容一致性等针对性检查；默认不触发 Rust 编译/测试、覆盖率、E2E 或 `human_tests/`。

### 变更类型与验证路由（先判定，后执行）

验证范围必须由实际影响面决定，不能因为“修改了仓库文件”就运行所有测试。任务开始时先根据 diff、依赖关系和运行时影响选择下表中的一类或多类；混合变更取各类适用验证的并集，影响面无法确认时才升级验证范围。

| 变更类型 | 典型范围 | 默认必做验证 | 默认不适用 |
| --- | --- | --- | --- |
| Rust 后端/代理/协议生产代码 | `crates/*/src/**/*.rs`、服务端行为、共享协议 | 受影响 crate 单元测试、Rust fmt/clippy、`make coverage-changed`、按风险选择相关 E2E；大范围或跨 crate 变更再执行 workspace/local-ci | 无关前端测试 |
| 前端行为/状态/API 集成 | WebUI/desktop 的 TS/TSX/JS 逻辑、路由、状态管理、接口调用 | 前端格式化、类型检查、受影响单元/组件测试；用户链路变化时做相关 UI/E2E 或真实场景验证 | Rust coverage、Cargo workspace、Rust E2E（共享协议或后端同时变化除外） |
| 纯 UI 视觉 | 样式、布局、图标、主题、纯展示文案 | 前端格式化/类型检查、受影响页面的亮暗主题和视觉检查；有现成快照时更新并验证 | Rust coverage/build、仓库级 E2E；无需为纯视觉改动强制新增 `human_tests/` 文档 |
| Skill/脚本/工具链 | `.agents/skills/`、skill 自带脚本、独立工具脚本 | 如提供则运行 `quick_validate.py`，并执行自带单元/脚本测试、shell/语法检查；涉及外部真实流程时只验证该流程 | Rust coverage/build、仓库级 E2E、`human_tests/`（除非修改 Bifrost 产品运行时行为） |
| 文档/流程/配置说明 | Markdown、设计文档、`AGENTS.md`、说明性配置 | Markdown/格式、链接、结构、一致性检查和两轮 review | Rust coverage/build、前后端 E2E、`human_tests/` |

补充原则：

- 文件后缀不是唯一判断依据；脚本若改变产品启动、发布或协议行为，应按受影响的运行时表面追加验证。
- `human_tests/` 只用于自动测试不足以证明的用户可感知运行时行为、真实账号/设备/浏览器链路或高风险回归，不是所有开发任务的默认门禁。
- `rust-project-validate`、Cargo workspace 测试和 Rust 覆盖率只在 Rust/后端影响范围内启用；前端、Skill、文档或流程变更不得为了“流程完整”空跑这些检查。
- 最终验证矩阵只列适用项；不适用项可按变更类型统一说明，无需逐项制造占位任务。

### 默认提交、MR 与 CI 看护策略

所有会修改仓库文件的开发任务都默认包含“本地验证 → 提交 → 推送 → 创建/更新 MR/PR → 远端 CI 看护”交付闭环，Agent 不得等待用户二次提醒后才执行。仅当用户明确要求只做本地修改、不要提交、不要推送、不要创建 MR/PR、不要跟进 CI，或当前任务被判定为检查模式时，才允许跳过对应动作；跳过原因必须进入最终交付。

- **提交与推送**：在适用的本地关键验证通过后，必须提交本次改动并推送当前任务分支。若本地完整验证耗时很长，可先完成格式、clippy 或相关最小测试后提前推送，让远端 CI 与剩余本地验证并行；最终完成前仍必须补齐本地验证结果或明确阻塞。
- **MR/PR 创建或更新**：如果当前分支尚无 MR/PR，必须创建 draft MR/PR 并填写目标、验证和风险；如果已有 MR/PR，必须推送到同一分支并在最终交付中给出链接。除非用户明确要求，否则不得把可交付开发停留在“本地已改好”状态。
- **远端 CI 看护**：推送后检查是否有对应 workflow run；若触发 CI，必须获取 run id，并使用 `.agents/skills/github-actions-pat/` 的 fail-fast 看护流程跟进到全绿。若路径过滤未触发 CI，记录“未产生 run”及查询证据即可，不得为了生成 run 扩大改动范围。
- **最终交付要求**：最终回复必须包含分支名、commit、MR/PR 链接，以及已触发 CI 的 run id 与状态；若未产生 run，说明路径过滤或 workflow 条件。若 CI 未能全绿，必须列出最后一次失败 job、根因判断、已做修复和剩余阻塞。

### 任务启动与工作区隔离

每个任务开始前，必须先执行 `git status --short --branch` 检查当前分支和工作区状态，并把结果纳入证据台账。

- **工作区干净**：可在当前 worktree 继续执行任务。
- **工作区存在既有修改、暂存内容或未跟踪文件**：默认优先为本次任务创建或切换到独立 `git worktree` 后再开发，避免并行任务互相污染。只有当用户明确要求在当前工作区继续，或本次任务就是处理这些既有修改时，才允许不创建 worktree；最终交付必须说明原因。
- **并行开发场景**：多个 Agent、多个功能分支、CI 复现或长任务同时进行时，必须使用独立 worktree。禁止在脏主工作区里直接做不相关开发、提交或 CI 归因。
- **worktree 命名建议**：使用可识别任务名，例如 `../bifrost-<task-slug>`；进入 worktree 后重新执行 `git status --short --branch` 确认隔离环境。

### 主站与文档站部署边界（强制）

Bifrost 的公开入口站点与文档站涉及两个仓库。任何首页、文档站、SEO、Open Graph、多语言、导航、站点资源或 GitHub Pages 部署相关任务，都必须先按以下边界执行，禁止凭感觉在错误仓库里手写一套：

- **源站仓库**：`bifrost` 本仓库的 `site/` 是首页、文档站、样式、多语言、SEO metadata、Open Graph、分享图、文档同步逻辑的唯一源码来源。
- **入口部署仓库**：`bifrost-proxy/bifrost-proxy.github.io` 只承载 `https://bifrost-proxy.github.io/` 的 GitHub Pages 发布产物。该仓库不是第二套设计稿来源，不得手写独立首页、独立语言切换、独立导航、独立 SEO 文案或独立文档入口。
- **入口 URL**：`https://bifrost-proxy.github.io/` 是公开主入口；文档入口是同一发布产物内的 `/docs/`、`/getting-started/...`、`/reference/...`、`/en/...` 等路径。不要再把根站只当作跳转壳处理。
- **修改方式**：需要改首页、文档站、导航、多语言、SEO、OG 图、文档内容或站点行为时，先在本仓库 `site/`、`docs/`、`docs-en/` 中修改并验证，再构建产物同步到部署仓库。禁止直接在 `bifrost-proxy.github.io` 仓库改 `index.html` 来绕过源站。
- **根站构建命令**：部署到入口站前，必须在本仓库执行 `SITE_URL=https://bifrost-proxy.github.io/ BASE_PATH=/ pnpm run site:build`，确保所有资源路径、canonical、OG URL、多语言入口和文档链接按根路径生成。
- **产物同步命令**：将本仓库 `site/dist/` 原样同步到 `bifrost-proxy.github.io` checkout，例如：`rsync -a --delete --exclude='.git/' --exclude='.github/' --exclude='README.md' <bifrost>/site/dist/ <bifrost-proxy.github.io>/`。如果部署仓库未来有必须保留的额外管理文件，必须明确列入 exclude，并在最终交付说明原因。
- **部署动作**：在 `bifrost-proxy.github.io` 仓库提交并推送 `main`，触发该仓库 GitHub Pages workflow。源站源码改动也必须在本仓库对应分支提交/推送，不能只提交部署产物。
- **主仓库项目站**：本仓库 `.github/workflows/site.yml` 只负责构建校验 `site/dist`，不得再使用 `actions/configure-pages`、`actions/upload-pages-artifact` 或 `actions/deploy-pages` 部署 `https://bifrost-proxy.github.io/bifrost/` 项目站；该项目站应在 GitHub Pages 设置中下线，避免遮蔽入口部署仓库中的 `/bifrost/` 兼容 redirect。
- **验证要求**：同步后必须验证本地和线上至少以下路径：`/`、`/docs/`、`/getting-started/overview` 或对应 `.html` 文件、`/en/getting-started/overview` 或对应 `.html` 文件、`/reference/rules/`、`/og-image.png`。本地用 `python3 -m http.server` 验证时要注意它不支持 VitePress clean URL fallback，必要时用 `.html` 路径验证文件存在；线上必须验证 clean URL。
- **CI/Pages 看护**：推送部署仓库后必须跟进 `bifrost-proxy/bifrost-proxy.github.io` 的 `Pages` 与 `pages-build-deployment` runs 到 success，并用 `curl -L https://bifrost-proxy.github.io/` 验证线上根站已经是源站同步后的首页，不是旧跳转页或手写分叉页。

### 证据台账

每个开发任务必须维护并在最终交付中摘要呈现以下证据：

- **目标证据**：用户目标验证清单，逐条标记已完成、阻塞或不适用。
- **变更证据**：任务启动时的 `git status --short --branch`、worktree 使用或豁免原因、`git status --short`、`git diff`、必要的 `git diff --cached`、新增文件清单和未触碰的用户既有改动。
- **测试证据**：记录变更路由判定、实际执行的针对性检查和结果；仅在适用时记录单元测试、E2E、human_tests、Cargo workspace、local-ci 和远端 CI。
- **review 证据**：至少两轮 Review/Fix/Test 的发现、修复、复测结果。
- **交付证据**：提交 hash、推送分支、MR/PR 链接；有远端 CI run 时记录 run id、看护结果和失败归因，无 run 时记录查询结果。
- **边界证据**：不适用项原因、未执行项风险、临时目录清理和残余风险。

### 完成定义（Definition of Done）

只有同时满足以下条件，任务才允许标记为 completed：

- 用户目标验证清单已逐项关闭，或明确列出阻塞原因和风险。
- 代码、适用测试、文档和配置默认值相互一致；启用 `human_tests/` 时其用例与索引也必须一致。
- 至少两轮 Review/Fix/Test 闭环完成；如发现问题，已继续追加轮次直到无阻塞问题。
- 所有适用测试已真实执行并通过；不适用或未执行项有明确原因。
- 所有按变更路由选中的验证均已完成；需要 `human_tests/` 时，对应用例已创建或更新并逐条真实执行。
- 除非用户明确豁免，仓库修改已提交并推送，MR/PR 已创建或更新；远端 CI 有 run 时已全绿，无 run 时已核实触发条件。若未全绿，必须有可核验的外部阻塞、无关失败或用户叫停证据。
- 最终交付摘要包含目标对齐、闭环记录、验证矩阵、提交/MR/CI 状态、变更范围和残余风险。

## Rust 生产代码门禁：单元 + E2E 代码覆盖率 90%

**本节只适用于修改 `crates/*/src/**/*.rs` 生产 Rust 代码或明确影响 Rust 运行时的任务。** 适用任务最终必须通过 CI 中的 `bash scripts/ci/coverage-all.sh --json --gate` 覆盖率门禁，并通过 `scripts/ci/coverage-thresholds.toml` 中各 crate 的覆盖率下限（棘轮门禁，目标 90%）与工作区聚合下限校验。本地不默认执行高成本的全 workspace `make coverage` / `coverage-all.sh --gate`；推送前必须执行低成本 `make coverage-changed`，自动仅插桩变更 crate，并按 CI 同源算法检查当前工作区 changed-lines 90% 门禁。详细机制见 `design/coverage-90.md`。

强制规则：

1. 仅当存在生产 Rust 变更时，任务规划中才必须出现“coverage 90% CI 门禁”项；其他类型明确标记 Rust coverage 不适用即可，不得执行覆盖率命令。
2. 修改生产 Rust 后先补对应测试，再运行 `make coverage-changed`。该命令默认以 `origin/main` 为基线，包含已提交、暂存、未暂存和未跟踪生产 Rust 文件；自动从 `cargo metadata` 找到变更 crate，只清理独立目录中的旧 `.profraw`，保留插桩编译缓存。没有生产 Rust 变更时本规则不适用，不执行该命令，更不得为了“完成 coverage”去跑全 workspace。
3. 增量预检返回 changed-lines `FAIL` 时，必须在本地补测试并重跑，禁止先推到 CI 碰运气。局部迭代可用 `COVERAGE_CHANGED_ARGS="--test-filter <name>" make coverage-changed`；最终推送前必须再执行不带 test filter 的完整变更 crate 预检。
4. 增量预检报告 `unmeasured_files` / `INCOMPLETE` 时，禁止把“0 个可插桩行”误报为通过，也不得无说明使用 `--allow-unmeasured`。应优先用 `--include-package <test-host-crate>` 纳入跨 crate 测试；只有平台 cfg 或仅真实 E2E 可达且无法低成本插桩的路径，才可带具体文件、原因和对应专项测试证据继续，并在交付报告中标注 CI coverage 风险。
5. 默认仍不在本机运行全 workspace `make coverage`；它只用于用户明确要求、专项排查绝对 crate/workspace 门禁，或本地复现 CI coverage 失败。`make coverage-changed` 不属于此禁令，不能以“no-local-coverage”为由跳过。
6. 当某个 crate 因为外部依赖（硬件、桌面 API、平台特定能力等）天然无法达到 90%，必须在 `design/coverage-90.md` 「不适用清单」中写明原因并维护例外冻结值，禁止悄悄绕过门禁。
7. CI 的 Rust 代码路径必须把 `bash scripts/ci/coverage-all.sh --json --gate` 作为必跑步骤，任一 crate 跌破其在 `coverage-thresholds.toml` 中的棘轮下限、或工作区聚合低于下限，直接阻断合入；需要合并 E2E 覆盖率时显式追加 `--with-e2e`。本地 changed-lines 通过是提前反馈，不替代远端 unit + proxy E2E union、per-crate 和 workspace 门禁。

**适用本节却违反门禁的任务视为未完成；非 Rust 变更不受此门禁约束。**

## 真实场景验证（human_tests）适用门禁

`human_tests/` 用于验证用户可感知的 Bifrost 运行时行为和自动化难以覆盖的真实链路。只有变更路由判定为适用时，才必须创建或更新用例并逐条真实执行。Skill、独立脚本、纯文档/流程、纯内部重构和已有自动化足以证明的纯视觉改动，默认不要求 `human_tests/`。

以下场景在影响用户可感知运行时行为时启用 human_tests：

| 开发场景 | human_tests 要求 |
| --- | --- |
| 🆕 新增需求 | 创建新的测试用例文档，覆盖所有新增功能点 |
| 🐛 修复 Bug | 在对应模块文档中新增回归用例，验证 Bug 已修复且不影响原有功能 |
| 🔒 修复安全漏洞 | 在对应模块文档中新增安全验证用例，验证漏洞已封堵且攻击路径不可复现 |
| ✏️ 功能修改 | 更新已有用例的预期结果，并新增用例覆盖修改点 |
| ♻️ 代码重构 | 仅当存在用户行为回归风险时执行已有用例；否则用单元/集成测试证明行为不变 |

**判定适用后不得用单元测试或 E2E 替代真实场景验证；判定不适用时必须说明变更类型与依据。**

---

## 开发需求标准流程

**核心约束：每次开发任务启动时，必须在规划阶段就产出明确的验证任务清单。禁止"先写代码后补测试"，验证计划必须与实现计划同步规划。**

**所有开发任务在实现后、收尾前，必须至少执行两轮 `目标复核 -> 变更 review -> 修复问题 -> 适用验证 -> 结果复盘`。** 两轮是最低要求，不是上限；只要任一轮发现问题或测试失败，必须继续追加新一轮直到目标、实现、适用测试和文档全部对齐。

### 第一阶段：分析与规划

1. **任务启动检查**：执行 `git status --short --branch`，判断是否需要独立 worktree。若工作区存在既有修改或并行任务，优先使用独立 worktree；若不使用，必须记录豁免原因。
2. **分析现状**：阅读相关代码与 `design/` 目录文件，明确需求范围与影响面
3. **设计技术方案**：涉及新功能、跨模块行为或重要架构决策时，在 `design/` 下新增或更新对应模块文档；局部 Skill、纯 UI、文档或流程调整可直接在任务计划中说明方案，不强制新增设计文档。
4. **拆解用户目标验证清单**：把用户需求拆成可验证条目，至少包含以下分类：
   - **必须实现**：用户明确要求的新能力、修复点、行为变化
   - **必须不破坏**：相关旧行为、兼容性、数据安全、权限边界、性能边界
   - **必须真实验证**：用户要求的真实链路、真实 CLI/WebUI/API、真实 CI 或远端环境
   - **必须交付**：文档、测试、提交、推送、CI 看护等交付动作
5. **制定验证计划**：先写明变更类型、影响面和验证路由，再规划适用的验证任务；不适用的测试不得为了凑齐流程而执行：
   - 单元/组件/脚本测试：列出受影响函数、模块或脚本及场景；没有可执行逻辑时可不规划
   - E2E/UI/真实流程：只有运行时跨组件链路或用户要求真实链路时，列出具体用例和断言
   - **真实场景测试（human_tests）【条件必填】**：仅在满足 human_tests 适用条件时，列出要创建或更新的用例文档及核心验证点
   - **Rust coverage 90% CI 门禁【条件必填】**：仅在修改生产 Rust 时规划 `make coverage-changed` 和远端 coverage
   - **Review/Fix/Test 第 1 轮【必填·不可跳过】**：列出本轮要复核的用户目标、修改文件、风险点、需要运行的验证命令
   - **Review/Fix/Test 第 2 轮【必填·不可跳过】**：列出修复后复查范围、覆盖缺口检查、需要复跑的测试命令
   - **提交/MR/CI 看护【必填·默认执行】**：列出计划提交的变更范围、推送分支、MR/PR 创建或更新动作、远端 CI 看护方式；若用户明确豁免，列出豁免原文和风险
   - **最终交付自检【必填·不可跳过】**：列出最终需要汇报的目标对齐、两轮 review、验证矩阵、提交/MR/CI 状态、未执行项及原因

> **强制要求**：TodoWrite 中的验证任务必须具体到可执行层面，禁止出现"编写测试"这类模糊描述。正确示例：
> - ✅ `任务启动检查：执行 git status --short --branch，发现当前工作区已有并行修改，创建 ../bifrost-remote-status-fix worktree 后继续`
> - ✅ `变更路由：仅修改 .agents/skills/zhihu-publish；执行 skill validator 与 Node 脚本测试，Rust/仓库 E2E/human_tests 不适用`
> - ✅ `用户目标验证清单：必须实现端口冲突提前失败；必须不修改系统代理；必须用真实 CLI 输出验证；必须更新 human_tests/cli-start-stop-status.md`
> - ✅ `单元测试：test_parse_rule_with_empty_target 验证空目标地址返回错误`
> - ✅ `E2E 测试：验证 HTTPS 代理规则创建后请求正确转发（断言状态码 200 + 响应体包含目标内容）`
> - ✅ `真实场景测试：在 human_tests/ 创建 remote-access-web-ui.md，包含远程登录、本地免登录、会话撤销等用例，然后按用例逐条执行验证`
> - ✅ `真实场景测试：在 human_tests/proxy-auth.md 中新增 TC-PA-05 用例，验证修复后的密码哈希比对逻辑`
> - ✅ `Review/Fix/Test 第 1 轮：复读用户目标，执行 git status --short 与 git diff，review crates/bifrost-cli/src/start.rs 与新增测试，修复遗漏后运行 cargo test -p bifrost-cli port_in_use`
> - ✅ `Review/Fix/Test 第 2 轮：复查第 1 轮修复后的 git diff、human_tests 索引和 CLI 真实输出，再复跑 cargo test -p bifrost-cli port_in_use`
> - ✅ `提交/MR/CI 看护：本地关键验证通过后提交并推送 codex/xxx，创建或更新 PR，使用 github-actions-pat fail-fast 看护远端 CI 到全绿`
> - ✅ `最终交付自检：按目标对齐、Review/Fix/Test 两轮结果、验证矩阵、提交/MR/CI 状态、未执行项原因汇总 final`
> - ❌ `编写单元测试`
> - ❌ `测试新功能`
> - ❌ `执行真实场景测试`（未指明 human_tests 用例文档）
> - ❌ `最终 review 一下`
> - ❌ `跑一下测试`
> - ❌ `未修改 Rust 但仍运行 cargo workspace、coverage 或 rust-project-validate`

> **规划阶段自检**：TodoWrite 必须出现 `任务启动检查`、`变更类型与验证路由`、`用户目标验证清单`、两条独立的 `Review/Fix/Test 第 1 轮` / `Review/Fix/Test 第 2 轮`、`提交/MR/CI 看护` 和 `最终交付自检`。只有适用时才加入 Rust coverage、E2E 或 `human_tests/` 条目；缺少适用项或加入明显无关的高成本验证都视为规划不合格。

### 第二阶段：实现

6. **编码实现**：按技术方案实现功能，开发过程中持续更新 todo 状态

### 第三阶段：执行适用验证

7. **最小验证集**：按变更路由执行最接近修改表面的测试、validator、lint、类型检查或静态一致性检查。
8. **扩展验证**：只有跨组件、高风险或真实链路需求时，追加 E2E、UI、集成测试或部署验证。
9. **真实场景测试（human_tests 驱动）【仅适用时执行】**：
   - **第一步：编写测试用例文档** — 在 `human_tests/` 目录下创建或更新对应功能模块的测试用例文档（详见"测试覆盖要求 - 真实场景测试"）
   - **⛔ 强制时序约束：第一步完成后，必须立即、无条件地执行第二步。禁止在创建/更新用例文档后跳转到其他任务、进入收尾阶段或标记任务完成。违反此约束的任务视为未完成。**
   - **第二步：按用例执行测试** — 严格按照文档中的每个测试用例逐条执行，记录实际结果并与预期结果对比
   - **第三步：确认结果** — 所有用例必须通过，未通过的必须修复代码后重新执行，直到全部通过

> **适用性规则**：一旦计划中判定需要 human_tests，上述编写与执行步骤不可拆分或被其他测试替代；判定不适用时不要创建占位用例。

### 第四阶段：强制 Review/Fix/Test 闭环

10. **Review/Fix/Test 第 1 轮（实现后自查）**：
   - **目标复核**：重新读取用户需求、用户目标验证清单、相关 `design/` 文档、实现计划和验证计划，逐条列出已满足 / 未满足 / 尚未验证的目标。
   - **变更范围复核**：必须执行 `git status --short` 和 `git diff`；如果存在 staged 内容，必须执行 `git diff --cached`。新增文件必须通过 `git status --short` 兜底确认，不得只看普通 `git diff`。
   - **代码 review**：对本次修改文件做 code-review 式检查，优先找功能缺陷、安全风险、边界条件、并发/状态一致性、用户可感知回归、缺失测试和文档不一致。
   - **修复问题**：立即修复本轮发现的问题。禁止只记录问题但不处理，除非明确写出无法处理的阻塞原因。
   - **测试运行**：运行与本轮问题和修改范围直接相关的最小测试集；失败必须先归因再修复后复跑。
   - **结果复盘**：记录本轮发现的问题、修复摘要、测试命令和测试结果。

11. **Review/Fix/Test 第 2 轮（修复后复查）**：
   - **再次目标复核**：对照用户原始目标、第 1 轮问题清单和当前 diff，确认没有遗漏目标、遗漏测试或只改测试不改功能的问题。
   - **再次变更范围复核**：再次执行 `git status --short`、`git diff` 和必要的 `git diff --cached`，确认第 1 轮修复后的最新 diff、未跟踪文件、暂存文件和用户既有改动边界。
   - **再次代码 review**：复查第 1 轮修复引入的新风险，检查设计文档、README/CLI help/WebUI 文案、E2E、human_tests 是否同步。
   - **继续修复问题**：修复本轮发现的所有问题。
   - **复跑测试**：至少复跑受影响测试；如果第 1 轮有失败用例，必须复跑失败路径并确认通过。
   - **结果复盘**：记录本轮结论，明确是否需要第 3 轮。

12. **继续循环条件**：任一轮出现以下情况，必须追加 `Review/Fix/Test 第 N 轮`，直到全部关闭：
    - 发现新的 P0/P1/P2 功能缺陷、安全风险、数据损坏风险或用户可感知回归
    - 测试失败，或测试只覆盖 mock 路径而用户要求真实链路
    - 变更路由判定的适用测试、human_tests、文档或索引存在缺口
    - 代码实现与 `design/`、README、CLI help、WebUI 文案、API/协议行为不一致
    - 用户目标仍有未实现、未验证或表述含糊的条目

> **闭环门禁**：没有完成两轮独立的 Review/Fix/Test，不得进入收尾阶段；如果两轮中的任一轮修复了代码或文档，后续轮次必须基于修复后的最新 diff 重新 review 和复测。

13. **测试失败归因规则**：任何测试失败都必须先归类，再决定修复路径。禁止为了让测试通过直接削弱断言或删除用例。
    - **功能缺陷**：实现没有达到用户目标或设计预期，必须修功能并复跑失败路径。
    - **测试缺陷**：测试断言、fixture、等待条件或 mock 设计错误，必须说明为什么产品行为正确，再修测试并补充防回归断言。
    - **环境/依赖问题**：端口占用、网络、权限、外部服务、CI 资源等问题，必须记录证据、隔离环境后重跑；不能把未验证说成通过。
    - **需求/文档不一致**：实现、测试、设计或 README/CLI help 互相冲突，必须先收敛目标，再同步代码、测试和文档。

### 第五阶段：收尾

14. **更新文档**：如涉及新功能 / API / 配置变更，同步更新相关文档（见"文档更新要求"）
15. **项目校验**：按变更路由执行对应校验。修改生产 Rust 或明确影响 Rust 运行时时，执行 `rust-project-validate` 及适用的 Cargo 测试；仅前端、Skill、脚本、文档、流程或纯 UI 变更不执行 Rust 校验。
16. **本地 CI 验证（按需执行）**：只有变更路由选中仓库 local-ci 套件时，才在其他适用验证完成后执行 `scripts/ci/local-ci.sh`（详见“本地 CI 验证要求”）。前端、Skill、独立脚本、纯 UI、文档和流程变更使用各自检查，不默认运行 local-ci。
17. **提交并推送（默认交付门禁）**：除非用户明确豁免，必须在本地关键验证通过后执行 `git commit` 和 `git push`，并记录提交 hash 与推送分支。若完整本地验证耗时较长，可先完成最小关键验证后提前推送，让远端 CI 并行运行；最终完成前必须补齐剩余本地验证或说明阻塞。
18. **创建或更新 MR/PR（默认交付门禁）**：除非用户明确豁免，必须确认当前分支有对应 MR/PR；没有则创建 draft MR/PR，已有则更新同一 MR/PR，并在证据台账记录链接。
19. **远端 CI 看护（默认交付门禁）**：推送后必须加载 `.agents/skills/github-actions-pat/`，查询对应 CI；若存在 run，获取 run id 并执行 fail-fast 看护，失败时进入 fix → push → watch 循环；若未触发 run，记录查询证据即可。
20. **收尾清理**：清理临时数据目录，避免资源膨胀
21. **检查 TodoWrite**：确认所有验证任务均已标记为 completed，无遗漏
22. **检查用户目标验证清单（强制门禁）**：逐项确认所有 `必须实现`、`必须不破坏`、`必须真实验证`、`必须交付` 条目均已完成或明确阻塞原因。
23. **检查 Review/Fix/Test 闭环（强制门禁）**：逐项确认以下所有条件，任一不满足则任务不得标记为完成：
    - [ ] 已完成第 1 轮目标复核、代码 review、问题修复、测试运行、结果复盘
    - [ ] 已完成第 2 轮目标复核、代码 review、问题修复、测试运行、结果复盘
    - [ ] 两轮都已执行 `git status --short`、`git diff` 和必要的 `git diff --cached`
    - [ ] 两轮中发现的所有阻塞问题均已修复，相关测试均已复跑通过
    - [ ] 如果第 2 轮仍发现问题，已追加第 3 轮或更多轮次直到无阻塞问题
24. **检查 human_tests（条件门禁）**：仅在变更路由判定适用时，逐项确认以下条件；不适用时记录依据后跳过：
    - [ ] `human_tests/` 下对应功能的测试用例文档已创建或更新
    - [ ] 文档中每个用例都已真实执行（不是跳过、不是假设通过）
    - [ ] 每个用例的实际结果与预期结果一致
    - [ ] `human_tests/readme.md` 索引表已同步更新
    - [ ] 如果是 Bug 修复/漏洞修复，文档中包含专门的回归验证用例
25. **最终交付自检模板（强制门禁）**：最终回复前必须按以下结构准备交付摘要；未执行项必须写明原因和风险，禁止笼统写“测试通过”：
    - **目标对齐**：逐条列出用户目标验证清单的完成状态。
    - **Review/Fix/Test 闭环**：列出第 1 轮、第 2 轮和追加轮次的发现、修复、复测结果。
    - **验证矩阵**：先写变更类型和路由结论，再列实际执行的验证命令与结果；对 Rust、前端、Skill、E2E、human_tests、local-ci、远端 CI 等未选中的类别按组说明不适用依据。
    - **提交/MR/CI 状态**：列出分支、commit、MR/PR 链接、CI run id、CI 当前状态或全绿证据；若豁免或阻塞，列出原因、风险和用户原文。
    - **变更范围**：列出修改文件、未触碰的用户既有改动、临时文件清理状态。
    - **残余风险**：列出仍未覆盖或需用户决策的事项；没有则明确写“未发现阻塞残余风险”。

## 测试覆盖要求

**核心原则：所有可执行逻辑变更都必须有与影响面匹配的验证。** 测试深度随风险和运行时表面增加；纯文档/流程改动以静态一致性检查为主，不强制代码测试。

### 单元测试

- **覆盖范围**：新增或修改的公共函数、核心逻辑模块原则上必须有对应的单元测试；Skill/脚本使用其语言和目录内约定的测试框架，不限定为 Cargo test
- **测试内容**：
  - 正常路径：验证主要功能在标准输入下的正确性
  - 边界条件：空值、极端值、非法输入等边界情况
  - 错误处理：验证错误返回值和异常分支的行为
- **执行方式**：Rust 使用 `cargo test` 或 `cargo test -p <crate_name>`；前端、Skill 和脚本使用项目现有的最小相关测试命令
- **要求**：
  - 测试函数命名清晰，体现测试意图（如 `test_parse_empty_input_returns_error`）
  - 每个测试用例独立，不依赖执行顺序或外部状态
  - 新增测试必须通过后才能进入下一步

### 端到端测试（E2E）

- **覆盖范围**：跨组件运行时链路、高风险回归或单元/集成测试无法证明的功能才要求 E2E；局部 Skill、纯 UI、文档和流程变更默认不新增仓库 E2E
- **详细要求**：判定适用时，按照 `.agents/skills/e2e-test/` 中的规范执行（使用技能 e2e-test）

### 真实场景测试（human_tests 驱动）

真实场景测试通过 `human_tests/` 目录下的测试用例文档驱动 Agent 自主执行。它只在用户可感知的 Bifrost 运行时行为、真实浏览器/账号/设备链路、高风险回归或用户明确要求时启用；不用于替代 Skill 自测、前端静态检查或文档 review。

**适用范围**：
- 用户可感知的新功能或行为修改，且自动测试不能充分证明 → 创建或更新测试用例文档
- 用户可复现的 Bug 或安全漏洞 → 新增回归/安全用例（命名建议：`TC-XX-回归-01`）
- 涉及真实账号、外部服务、浏览器、设备或权限的链路 → 编写并执行对应真实场景
- 纯内部重构 → 仅在存在真实行为回归风险时复跑相关已有用例

#### 第一步：编写测试用例文档

- **文档位置**：`human_tests/` 目录下，文件名：`功能模块名.md`（如 `remote-access-web-ui.md`）
- **文档结构**：每个文件必须包含以下部分：
  - 功能模块说明
  - 前置条件（环境准备、服务启动命令等）
  - 测试用例列表（每个用例包含：用例编号、用例名称、操作步骤、预期结果）
  - 清理步骤
- **用例编写要求**：
  - 每个用例必须有唯一编号（如 `TC-RA-01`），方便追踪
  - 操作步骤必须具体到可直接执行（如"在浏览器中打开 `http://localhost:8800/_bifrost/`"），禁止模糊描述
  - 预期结果必须包含可验证的具体断言（如"页面显示 Settings 页面，无登录弹窗"）
  - 覆盖正常路径、异常路径、边界条件
  - Bug 修复用例必须包含：原 Bug 复现步骤 → 修复后预期行为
  - 安全漏洞用例必须包含：原攻击路径模拟 → 验证已被拦截/拒绝
- **文档同步更新**：新增用例后必须同步更新 `human_tests/readme.md` 索引表
- **索引冲突约束**：`human_tests/readme.md` 只维护按模块分组的索引行，禁止维护全局测试文件总数、全局测试用例总数或其他汇总数字；新增或更新用例时只调整相关模块的索引行，避免改动无关行。这类汇总在并行开发中频繁冲突，不能作为验收依据。

#### ⛔ 强制时序约束（第一步 → 第二步）

**第一步完成后（用例文档已创建或更新），必须立即进入第二步执行测试。以下行为均属违规：**
- 创建用例文档后跳转到其他开发任务
- 创建用例文档后直接进入收尾阶段
- 创建用例文档后标记 human_tests 任务为 completed
- 以"已创建文档，后续执行"为由延迟执行

**只有当第二步和第三步全部完成（所有用例真实执行且通过），才允许离开 human_tests 验证流程。**

#### 第二步：按用例执行测试

- **执行方式**：Agent 读取 `human_tests/` 下对应的测试用例文档，按用例逐条自主执行
  - 涉及 Web UI 的用例：必须使用 Chrome（通过 Chrome DevTools MCP）进行真实浏览器操作
  - 涉及 CLI 的用例：直接执行命令并验证输出
  - 涉及 API 的用例：通过 curl 或等效方式发起请求并验证响应
- **结果验证**：每个用例执行后，必须将实际结果与预期结果逐条对比
- **失败处理**：如发现实际行为与预期不符，必须在提交前修复代码或更新用例文档说明差异原因
- **不可替代性**：真实场景测试（human_tests）不可被单元测试或 E2E 测试替代——它验证的是端到端的用户感知体验

#### 常见逃避模式（严禁出现）

以下行为均视为违规，必须纠正：

1. **"这个改动很小，所以一定不需要 human_tests"** → 大小不是判断依据；应根据用户可感知行为、风险和自动测试充分性判定
2. **"单元测试已经覆盖了"** → 单元测试和 human_tests 验证维度不同，不可替代
3. **"E2E 测试已经验证了"** → E2E 是自动化脚本验证，human_tests 是真实场景端到端验证
4. **适用但没有对应的 human_tests 文档** → 必须创建新文档或在最相关的已有文档中新增用例
5. **"用例文档已经存在，不需要更新"** → 如果本次修改了相关功能，必须检查已有用例是否需要更新，且必须重新执行
6. **"已经假设通过/应该通过"** → 禁止假设，必须真实执行并记录结果
7. **"用例文档已创建，稍后执行"** → 创建文档后必须立即执行，禁止延迟。第一步和第二步是原子操作，不可拆分到不同阶段

### 测试完备性检查清单

开发完成后，提交前必须确认所有适用条件满足：

- [ ] 已记录变更类型、影响面和验证路由
- [ ] 新增/修改的可执行核心逻辑已有对应语言/模块的最小相关测试并通过
- [ ] 如 E2E 适用，已编写或更新对应脚本、包含断言并执行通过
- [ ] **如 human_tests 适用，已创建或更新用例文档、同步索引并逐条真实执行通过**
- [ ] **已完成至少两轮 Review/Fix/Test 闭环，且每轮都有目标复核、代码 review、修复动作、测试命令和结果记录**
- [ ] **如果第 2 轮仍发现问题，已继续追加第 3 轮或更多轮次，直到无阻塞问题**
- [ ] 如生产 Rust/跨 crate 变更需要 workspace 验证，`cargo test --workspace --all-features` 全部通过
- [ ] **本地 CI 验证已按修改范围执行对应检查项并通过（详见"本地 CI 验证要求"）**
- [ ] **除非用户明确豁免，修改已提交并推送到任务分支**
- [ ] **除非用户明确豁免，MR/PR 已创建或更新，并已记录链接**
- [ ] **除非用户明确豁免，已查询远端 CI；有 run 时看护到全绿，无 run 时已记录 workflow 路径/触发条件证据**

> **最终门禁**：如果变更路由选中的适用验证、Review/Fix/Test 闭环、提交/MR/CI 看护任一未完成且没有用户明确豁免或可核验阻塞，整个任务不得标记为 completed。不得把不适用项反向升级为强制门禁。

## 技术方案（design 目录）

- 需要方案文档时必须位于 `design/`，每个功能模块维护一个独立文件：`功能模块名.md`
- 同模块方案持续增量更新，保持与代码实现同步
- 技术方案必须包含：
  - 功能模块详细描述
  - 实现逻辑
  - 依赖项
  - 验证方案（明确变更路由和适用的单元/组件/脚本、E2E、UI 或真实场景验证；不适用层级无需强行规划）
  - Review/Fix/Test 闭环方案（至少两轮，明确每轮 review 范围、修复策略和复测命令）
  - 校验要求（仅 Rust/后端影响范围包含 `rust-project-validate`）
  - 文档更新要求（明确需更新的文件）

## 文档更新要求

- **文档语言要求**：所有新增或修改的文档、测试说明、设计文档、OpenAPI/接口描述、README/CLI help/WebUI 文案，必须使用中文或英文撰写，禁止使用其他语言。如果在本次涉及范围内发现既有文档包含其他语言，必须同步改为中文或英文后再交付。
- 如果修改涉及新功能、API 变更或配置变更，需要同步更新 `README.md`
- 如果添加了新的协议，需要更新 README 中的协议列表
- 如果添加了新的 Hook，需要更新 README 中的 Hook 表格
- 如果修改了命令行参数或配置选项，需要更新相关文档说明

## 启动服务的要求

- 启动服务时必须配置临时数据目录，避免覆盖正在运行的服务数据
- 必须采用立即编译运行的方式，避免使用已编译的二进制文件
- **⛔ 必须加上 `--no-system-proxy`**：除非测试目标明确涉及系统代理功能（如 `test_system_proxy_e2e.sh`），否则启动 Bifrost 时必须携带 `--no-system-proxy` 参数。原因：Bifrost 默认会修改操作系统的系统代理配置，在开发/测试环境中这会导致网络中断，影响其他进程和开发体验。
- **⛔ 必须禁用 Sync 自动登录弹窗**：除非测试目标明确涉及 Sync 启动自动登录引导，否则开发、调试、E2E、human_tests 启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。原因：Sync 默认启用后会在远端可达且本地未登录时自动打开登录页；多数测试不需要这个交互，意外弹窗会干扰自动化、污染用户体验并造成误判。
- **⛔ 必须禁用托盘（Tray）**：除非测试目标明确涉及托盘行为（如 `test_cli_tray_*`），否则开发、调试、E2E、human_tests、任何复现脚本启动 Bifrost 时必须设置 `BIFROST_DISABLE_TRAY=1`。原因：在 macOS / Windows 上启动完整 `bifrost` 会派生托盘 helper 进程；并发跑测试矩阵时会瞬间拉起几十个托盘并打开上百个登录页，直接打爆机器。这是已经发生过的真实事故。
- 示例：

```bash
BIFROST_DATA_DIR=./.bifrost-test BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

> **唯一豁免场景**：当测试用例的验证目标本身就是系统代理功能时（如需要验证 `--system-proxy` 开启/关闭行为），可以省略 `--no-system-proxy` 或显式使用 `--system-proxy`。此类场景必须在测试用例文档中明确标注。
> **Sync 弹窗豁免场景**：当测试用例的验证目标本身就是启动登录预检、自动打开登录页或登录弹窗去重时，可以省略 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。此类场景必须在测试用例文档中明确标注，并优先使用 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE` 记录登录 URL，避免真实打开浏览器。

## ⛔ 测试启动红线（禁止裸调 bifrost start）

**任何测试、调试、复现脚本启动完整 `bifrost`（`start` / `run`）之前，必须先具备托盘 + 登录页双重护栏。** 这是事故复盘后的硬约束，违反即视为交付不合格。

1. **禁止裸调**：禁止在没有护栏的情况下直接执行 `bifrost start` / `$BIFROST_BIN start` / `cargo run --bin bifrost -- start`。护栏二选一：
   - 在脚本顶部 `source e2e-tests/test_utils/process.sh`（共享 helper 已默认 `BIFROST_DISABLE_TRAY=1` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`）；或
   - 显式 `export BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 后再启动。
2. **禁止仓库根目录裸脚本**：临时复现脚本不得放在仓库根目录绕过 `scripts/run_all_e2e.sh` 这把"伞"。复现脚本应放进 `e2e-tests/` 并自带护栏；用完即删，禁止提交进仓库。
3. **禁止并发起多个完整实例**：除非测试目标就是多实例并发，否则不得在同一脚本里并行拉起多个完整 `bifrost` 进程；并发场景必须用独立端口 + 独立数据目录，并设置实例数上限。
4. **收尾必须清理派生进程**：脚本退出（含失败路径，用 `trap` 兜底）必须杀掉本次派生的 `bifrost`、托盘 helper 与 mock server 进程，并释放占用端口，禁止残留进程污染后续运行。
5. **提交前自检（不入 CI）**：`scripts/ci/check-e2e-launch-guard.sh` 会扫描所有启动 `bifrost start|run` 的脚本是否携带护栏。为节省 CI 资源，该校验**不作为独立 CI job**，而是要求人/Agent 在新增或修改测试脚本后、提交前本地执行一次（`bash scripts/ci/check-e2e-launch-guard.sh`，退出码非 0 即不合规）。新增不合规脚本必须先修复护栏再提交。
6. **禁止占用或清理正式端口**：自动化测试默认保护 `9900`。不得把它作为测试实例端口，也不得调用按端口清理 helper 终止其监听进程；测试必须使用动态端口或明确的非正式端口。
7. **禁止全机进程名清理**：测试和 runner 禁止使用 `pkill -f bifrost`、`killall bifrost`、`taskkill /IM bifrost.exe`。残留进程只能按本次测试记录的 PID、进程组、端口或带 `.bifrost-e2e-owned` 标记的数据目录回收。
8. **真实数据只读复用**：确需真实账号/IM 数据的 human test 必须优先复用已经运行的正式服务，通过 API/CLI 验证；禁止为了“真实”而用当前 checkout 二进制和 `--yes` 重启 `~/.bifrost`。若验证目标本身是正式服务升级/重启，必须在用例中明确列为破坏性步骤并先备份配置。

## 工作区测试要求

- 修改生产 Rust、共享协议或跨 crate 行为时，提交前必须至少执行一次 `cargo test --workspace --all-features`
- 目的：提前发现仅在工作区聚合、全 feature 组合或 CI 路径下出现的失败，避免代码提交后才在 CI 暴露问题
- 如果该命令失败，需要先定位并处理，或在提交说明中明确标注阻塞原因与影响范围
- 前端、Skill、脚本、纯 UI、文档和流程变更默认不执行该命令，除非依赖分析证明其影响 Rust workspace

## 代码质量检查要求（Clippy）

- **Rust 代码提交前必须通过 clippy 检查**，执行命令：
  ```bash
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  ```
- CI 使用 `-D warnings` 标志，任何 clippy 警告都会导致构建失败
- 常见问题及修复：
  - `useless_format`: 不带插值参数的 `format!()` 改用 `.to_string()`
  - `unused_imports`: 删除未使用的 import
  - `dead_code`: 删除未使用的代码或添加 `#[allow(dead_code)]` 并注释原因
- 本地开发时建议在 IDE 中启用 clippy 实时检查
- **修改 Rust 代码时绝对不要在未通过** **`cargo clippy -- -D warnings`** **的情况下推送代码到远端**
- 项目提供了 `.githooks/pre-commit` 钩子脚本，自动执行 workspace fmt、desktop fmt 与 clippy 检查。初始化方式：
  ```bash
  bash scripts/setup-git-hooks.sh
  # 或
  make setup
  ```

## 代码格式化要求（Cargo Fmt）

**所有 Rust 代码在提交或推送至远端前，必须执行 `cargo fmt --all -- --check` 检查。如果不符合规范，必须执行 `cargo fmt --all` 进行格式化后再提交，以保证通过 CI 流水线的代码风格检查。**

## 架构设计要求

### WebUI 主题设计要求

**所有 WebUI 开发必须同时支持亮色（Light）和暗色（Dark）主题，确保用户界面在不同主题下均具备良好的可读性和视觉一致性。**

#### 强制要求

1. **双主题兼容性**：所有新增或修改的 WebUI 组件、页面必须在亮色和暗色主题下进行验证，禁止仅考虑单一主题。
2. **CSS 变量优先**：颜色、阴影、边框等样式属性必须使用 CSS 变量（如 `var(--color-text)`）而非硬编码颜色值，确保主题切换时样式自动适配。
3. **图标与图片**：图标应使用 SVG 格式并通过 `currentColor` 或 CSS 变量控制颜色；图片资源需确保在两种主题下均清晰可见，必要时提供双主题版本。
4. **组件库规范**：若使用 Semi Design 等组件库，必须正确配置主题 Provider，确保组件样式与自定义样式保持一致。
5. **测试验证**：WebUI 视觉或交互变更完成后，必须在亮色和暗色主题下做针对性 UI 验证，检查布局、颜色对比度和可交互元素的可识别性；只有需要固化真实用户链路时才更新 `human_tests/`。

#### 检查清单

开发 WebUI 功能时，必须确认以下条件：

- [ ] 所有颜色使用 CSS 变量，无硬编码颜色值
- [ ] 在亮色主题下验证过所有页面和组件
- [ ] 在暗色主题下验证过所有页面和组件
- [ ] 图标、图片在两种主题下均清晰可辨
- [ ] 文本与背景的对比度符合可访问性标准（WCAG AA）

> **违反此约束的任务视为未完成，禁止标记为 completed。**

### 单文件行数限制

**禁止让单个代码文件超过 1500 行。超过此限制必须进行模块化拆分，确保长期可持续维护。**

#### 拆分原则

1. **职责单一**：每个模块/文件应只负责一个明确的功能领域
2. **高内聚低耦合**：相关功能聚合在同一模块，跨模块依赖最小化
3. **按领域拆分**：优先按业务领域划分，而非技术层面（如"handlers"、"utils"）
4. **渐进式拆分**：当文件接近 1000 行时应主动评估拆分方案，避免等到超过 1500 行

#### 常见拆分策略

| 场景 | 拆分建议 |
| --- | --- |
| 大型结构体及实现 | 按 trait 或功能组拆分到独立文件，主文件保留核心定义 |
| 冗长的 match 分支 | 使用子模块或策略模式，每个分支独立实现 |
| 多个相关类型 | 按类型拆分，通过 `mod.rs` 或 `lib.rs` 重新导出公共接口 |
| 处理逻辑复杂的模块 | 提取通用逻辑到 helper 子模块，主模块保持编排职责 |

#### 文件组织规范

```
src/
├── module/
│   ├── mod.rs          # 模块入口，导出公共接口
│   ├── core.rs         # 核心逻辑
│   ├── handlers.rs     # 请求处理
│   └── utils.rs        # 辅助函数（如需要）
```

#### 例外情况

以下情况可适当放宽限制，但需在文件头部注释说明原因：
- 由代码生成工具自动生成的文件
- 包含大量静态数据（如配置表、映射表）的文件
- 历史遗留文件正在重构过程中（需标注重构计划）

## 日志配置规范

### 日志级别优先级（从高到低）

1. `RUST_LOG` 环境变量 - 支持精细化控制，如 `RUST_LOG=bifrost_proxy=debug,info`
2. 命令行参数 `-l/--log-level` - 仅 bifrost-cli 支持
3. 默认值 `info`

### 各入口点日志初始化规范

| 入口点         | 初始化方式                          | 说明                            |
| ----------- | ------------------------------ | ----------------------------- |
| bifrost-cli | `init_logging(&cli.log_level)` | 使用 bifrost-core 统一函数          |
| bifrost-e2e | `tracing_subscriber::fmt()`    | 从 `--verbose` 和 `RUST_LOG` 读取 |

### verbose\_logging 双轨机制

项目使用两套日志控制机制：

1. **tracing 级别** - 控制全局日志输出（通过 `EnvFilter`）
2. **verbose\_logging 布尔值** - 控制详细业务日志（规则匹配、请求转发等）

规则：当日志级别为 `debug` 或 `trace` 时，`verbose_logging` 自动设为 `true`

```rust
let verbose_logging = matches!(log_level.as_str(), "debug" | "trace");
```

### 新增入口点时的要求

1. 必须支持 `RUST_LOG` 环境变量优先
2. 如果有命令行参数，作为 `RUST_LOG` 未设置时的回退
3. 如果需要传递 `verbose_logging`，必须根据日志级别正确设置
4. 使用 `bifrost_core::init_logging()` 统一初始化（已支持 RUST\_LOG 回退）

### 关键文件位置

- 日志初始化：`crates/bifrost-core/src/logging.rs`
- CLI 入口：`crates/bifrost-cli/src/main.rs`
- E2E 入口：`crates/bifrost-e2e/src/main.rs`
- ProxyConfig 定义：`crates/bifrost-proxy/src/server.rs`

### 日志要求

- 所有日志输出必须符合 tracing 标准，包含 `target`、`level`、`message` 等字段
- 所有日志级别必须根据 `RUST_LOG` 环境变量或 `--log-level` 参数进行控制
- 所有日志输出必须包含 `file`、`line`、`module` 等字段，方便定位问题
- 开发新需求时务必添加详细的日志，方便调试和问题定位

## 数据库表结构

如果涉及新的需求需要修改数据库表，请直接修改表协议，我们不考虑对旧数据兼容，当协议更新版本时，直接删除旧版本数据库，重建数据即可。

## 本地 CI 验证要求（按需执行）

**只有变更路由选中 Rust/E2E 仓库级验证时，才根据修改范围选择性执行本地 CI；它成本很高，必须在其他适用验证之后执行。** 脚本位于 `scripts/ci/local-ci.sh`。前端、Skill、独立脚本、纯 UI、文档和流程变更使用自身工具，不进入该脚本。

### 按修改范围选择执行策略

| 修改范围 | 必须执行的检查 | 命令 |
| --- | --- | --- |
| 仅修改 Rust 代码（非 E2E 测试模块） | fmt + clippy + 单元测试 | `bash scripts/ci/local-ci.sh --skip-e2e` |
| 仅修改 E2E 测试代码 | 对应的 E2E 套件 | `bash scripts/ci/local-ci.sh --skip-static --e2e-only <suite>` |
| 修改了代理核心逻辑（规则匹配/请求转发/协议处理） | fmt + clippy + 单元测试 + 相关 E2E | `bash scripts/ci/local-ci.sh --e2e-only rules` |
| 修改了 Shell/CLI 相关代码 | fmt + clippy + 单元测试 + shell E2E | `bash scripts/ci/local-ci.sh --e2e-only shell` |
| 修改了平台集成相关代码 | fmt + clippy + 单元测试 + platform E2E | `bash scripts/ci/local-ci.sh --e2e-only platform` |
| 仅修改前端行为 | 前端格式化 + 类型检查 + 受影响单元/组件测试 | 使用对应前端 package 的现有脚本 |
| 仅修改纯 UI/样式/文案 | 前端格式化/类型检查 + 亮暗主题针对性 UI 验证 | 使用对应前端 package 的现有脚本 |
| 仅修改 Skill/独立脚本 | skill validator + 对应脚本测试/语法检查 | 使用该 skill 或脚本目录内命令 |
| 仅修改文档/流程 | 格式、链接、结构和一致性检查 | 不运行 `local-ci.sh` |
| 大范围重构或不确定影响面 | 全量执行 | `bash scripts/ci/local-ci.sh` |

### Rust 变更最小必须执行项

只要修改 Rust 代码，以下两项始终必须执行：
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`

这两项可通过 `bash scripts/ci/local-ci.sh --skip-e2e` 完成（包含 fmt + clippy + 单元测试）。没有 Rust 变更时不得仅为满足本节运行 Cargo 或 `local-ci.sh`。

### E2E 套件对照表

| 套件名 | 对应脚本 | 覆盖范围 |
| --- | --- | --- |
| `rules` | `scripts/ci/run-e2e-rules.sh` | 规则匹配、请求转发、Header 修改等 |
| `shell` | `scripts/ci/run-e2e-shell.sh` | CLI 命令、Shell 交互 |
| `runner` | `scripts/ci/run-e2e-runner.sh` | 服务启停、生命周期管理 |
| `platform` | `scripts/ci/run-e2e-platform.sh` | 平台集成、跨端能力 |


## GitHub Actions CI 分析与默认远端闭环

**唯一入口：`.agents/skills/github-actions-pat/`**。所有"读 CI 运行状态 / 拉 failed job 日志 / 归因失败 / 轮询 run / 做 PR review / 推完代码后自动盯 CI"的工作必须走本 skill，不得直接手写 curl、不得走 cookie / gh CLI login / OAuth device flow。

### 什么时候启用

触发 **任一** 条件即必须加载：

1. 用户说"看看 CI / 查一下 workflow / 为什么失败了 / CI 怎么样了"等意图分析 GitHub Actions 的话
2. 任一开发任务完成后执行了 `git push`，需要默认等 CI 走完并验证绿灯
3. 需要对已 push 的 PR 做 code review（拉 diff + 分层建议，必要时 `--post`）
4. 进入 fix → push → watch → iterate 的自动化闭环
5. 需要与上一次绿跑做 regression 对比，定位是哪次提交引入的失败

### 如何加载

本 skill 只依赖 `GITHUB_TOKEN` 环境变量（PAT，推荐 fine-grained + 仓库维度限定）。**不支持也不允许**用 cookie / OAuth / `gh auth login` 等其他认证方式。

```bash
# 1) 确认 token 已经 export（由用户在自己本地 shell rc 里配好）
#    macOS / zsh 用户必须用 -ic（interactive 模式）才会加载 ~/.zshrc
zsh -ic 'echo "${GITHUB_TOKEN:+present}"'   # 应输出 "present"

# 2) 设置目标仓库（未设置时默认 bifrost-proxy/bifrost）
export GH_REPO=bifrost-proxy/bifrost

# 3) 所有调用都要带 proxy 隔离前缀，避免 bifrost 本身 MITM 掉证书
NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
python3 .agents/skills/github-actions-pat/scripts/gh_ci.py run <run_id>
```

### 典型命令

| 目的 | 命令 |
| --- | --- |
| 按 run-id 分析失败 | `python3 .agents/skills/github-actions-pat/scripts/gh_ci.py run <run_id>` |
| PR 上最近一次 failed | `... scripts/gh_ci.py pr <N>` |
| 按 commit sha 找 | `... scripts/gh_ci.py sha <sha>` |
| 分支上最近一次 failed | `... scripts/gh_ci.py branch <name>` |
| 与上次绿跑 regression 对比 | `... scripts/gh_ci.py regression <run_id>` |
| **fail-fast 看护**（推荐，任一 job 失败立即退出并打印归因） | `POLL_SEC=20 MAX_WAIT_SEC=3600 python3 ... scripts/watch_jobs.py <run_id>` |
| 等整个 run 跑完（仅用于最终全绿确认） | `POLL_SEC=45 MAX_WAIT_SEC=1800 python3 ... scripts/poll_run.py <run_id>` |
| PR code review（仅生成 markdown） | `... scripts/gh_review.py <N>` |
| PR code review（真的 POST） | `... scripts/gh_review.py <N> --post --event REQUEST_CHANGES` |

更多细节与坑点清单见 `.agents/skills/github-actions-pat/SKILL.md` 与 `references/pitfalls.md`。

### fix-push-watch 闭环（默认跑 CI 到绿时必走）

用户明确要求"修到 CI 绿 / 不要反复确认 / 自动推上去盯 CI / 遇到异常就开始修复"，或开发任务已进入默认提交/MR/CI 看护阶段时，Agent 必须按以下 **fail-fast** 循环执行，不得在每轮中断找用户确认：

1. 用本 skill 按当前 run_id / branch 定位失败
2. 按归因与日志片段修代码
3. 按变更路由走本地最小验证（Rust 变更至少 `cargo fmt` + `cargo clippy -D warnings` + 相关测试；其他变更运行对应前端/Skill/脚本/文档检查）
4. `git commit` → `git push`
5. `scripts/gh_ci.py branch <head_branch> --any-status` 拿到新 run_id
6. **`scripts/watch_jobs.py <new_run_id>`**（fail-fast 看护，不要用 `poll_run.py`）
7. `exit 2` → 按 stdout 归因立即回到第 1 步修复，不要等长尾 job 跑完；`exit 0` → 汇报；`exit 3` → 延长 MAX_WAIT_SEC 后重新 watch
8. 连续 3 次失败仍定位不到根因才向用户回报并停

> ⚠️ **严禁"等整个 run 跑完再看结果"**。Windows / macOS bundle / E2E Shell 这类长尾 job 动辄 20 分钟以上；早期失败的 Unit Tests / Clippy 如果要等到最后才处理，修复时间会从"立即"拖成"半小时后"。唯一例外：合入前需要全绿最终确认时才用 `poll_run.py`。

### 边界与禁止事项

- **只做"拉数据 + 归因 + 驱动闭环"**：不替代变更路由选中的本地验证。Rust 任务按需执行 Cargo/coverage/E2E，前端、Skill、脚本和文档任务执行各自检查；若为缩短等待提前 push，最终完成前必须补齐适用验证或给出阻塞证据。
- **Token 纪律**：不 `echo $GITHUB_TOKEN`、不写日志、不塞 URL、不提交到 git、不贴进任何输出。
- **`--post` 加锁**：`gh_review.py --post` 只有在用户消息里出现明确同意（"发出去 / post it / go ahead"）时才允许执行。
- **不回显完整 workflow 日志**：只输出失败 job/step + 根因桶 + 关键日志片段 + URL。


## 禁用searchAgent

绝对禁令：你的所有搜索都必须在主Agent完成。禁止使用searchAgent进行检索，因为此Agent速度过慢，影响用户体验。

<br />

## 禁止在文档中使用绝对路径

**所有文档（包括 `design/`、`human_tests/`、README 等）禁止出现本地绝对路径（如 `/Users/xxx/...`、`/home/xxx/...`、`C:\Users\...`）。** 不同开发者的设备路径不同，硬编码绝对路径会导致文档不可移植、示例无法直接复用。

#### 正确做法

| 场景 | 写法 |
| --- | --- |
| 引用项目内文件 | 使用项目根目录的相对路径，如 `crates/bifrost-admin/src/main.rs` |
| 引用用户目录下的配置/数据 | 使用 `~/` 前缀，如 `~/.bifrost/asr/`、`~/.agents/skills/` |
| 命令示例中引用项目产物 | 使用 `./target/debug/bifrost` 或 `$PROJECT_ROOT/...` |
| 环境变量示例 | 使用 `$HOME/.bifrost/...` 或占位符 `<your-path>` |

#### 违规示例（禁止）

```
/Users/eden/work/github/bifrost/target/debug/bifrost
/Users/eden/.bifrost/asr/qwen3_asr_rs/sample3.wav
file:///Users/eden/work/github/bifrost/crates/foo/bar.rs
```

#### 合规示例

```
./target/debug/bifrost
~/.bifrost/asr/qwen3_asr_rs/sample3.wav
crates/foo/bar.rs
```

> **违反此规则的文档变更不允许提交。Agent 在编写或更新任何文档时必须自查是否引入了绝对路径。**

## 测试情况下，禁止使用 9900 端口（这是正式环境的端口，避免冲突）
