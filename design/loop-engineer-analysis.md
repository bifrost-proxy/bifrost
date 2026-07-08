# Bifrost Loop Engineer 体系分析

> 本文把用户口语里的 "loop engineer" 具体化为 Bifrost 当前落地的 Agent 工程闭环体系。它不是单个类、单个命令或单个 runner，而是由开发流程规则、Agent turn loop、工具调度、状态可观测、长期任务控制、记忆/技能和 CI/human_tests 门禁共同组成的一套能力。

## 结论摘要

Bifrost 的 loop engineer 做得比较成熟，最突出的价值是把“模型自己努力”改造成“系统约束模型按工程闭环工作”。它通过 `AGENTS.md`、`update_plan`、turn runtime event、tool registry、session status、human_tests、CI watch 等多层结构，把目标拆解、实现、复核、测试、提交、远端验证串成可追踪流程。

亮点：

- 闭环意识强：`design/agent-development-review-loop.md` 把开发任务定义成至少两轮 `目标复核 -> 代码 review -> 修复 -> 测试 -> 复盘`。
- 运行时有状态：`crates/agent/src/session/turn_loop.rs` 记录模型请求、工具调用、长任务、完成/停止等事件。
- 工具边界清楚：`crates/agent/src/tools/mod.rs` 收敛核心工具，终端统一为 `exec_command` / `write_stdin`。
- 可观测扎实：`/status`、progress card、JSONL history、turn events、token/context/compaction 指标共同解释 loop。
- 长任务方向正确：Cooperative Long Task Loop 把等待命令完成的责任从模型轮询转移给 runtime monitor。
- 知识复用成体系：Memory、Skills、MCP resources、`tool_search` 让 Agent 既能用长期经验，也能渐进式加载能力。

问题：

- 规则层很重：适合高质量交付，但对小型文档/低风险任务成本偏高。
- 文档里现状、目标方案、阶段计划混在一起，学习者需要自行区分。
- 闭环证据依赖 Agent 认真维护，规则本身不能自动证明测试真的执行。
- 工具体系复杂：内置工具、MCP、skills、external runner、IM/Web/API 多入口并存，学习门槛高。
- 长任务和 worker 隔离是高复杂域，进程、session、SSE、IM guide queue、JSONL 恢复、stop/kill 语义都要同时正确。

## Loop Engineer 的真实组成

### 1. 规则层：把工程行为写成状态机

核心文件：

- `AGENTS.md`
- `design/agent-development-review-loop.md`
- `human_tests/agent-development-review-loop.md`

这一层解决“Agent 应该怎么做工程”的问题：

- 任务先分类：开发模式、检查模式、CI 闭环模式、文档/流程变更模式。
- 开始前必须 `git status --short --branch`，脏工作区默认 worktree 隔离。
- 开发前拆用户目标验证清单：必须实现、必须不破坏、必须真实验证、必须交付。
- 实现后至少两轮 Review/Fix/Test。
- 默认提交、推送、创建或更新 PR，并看护远端 CI。
- 所有开发场景都要更新并执行 `human_tests/`。

这让 Agent 不只是“回答问题”，而是被塑造成一个遵守交付流程的工程执行体。

### 2. Turn Loop 层：让模型、工具、历史形成循环

核心文件：

- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/turn_runtime.rs`
- `crates/agent/src/session_status.rs`

运行时主干：

```text
用户输入
  -> slash / config preflight / memory recall / compaction
  -> build model request
  -> model response
  -> tool calls
  -> tool result 回填 history
  -> 必要时继续下一次 model request
  -> final answer / stopped / max iteration / compact
```

关键工程点：

- `max_iterations` 限制避免无限 loop。
- stop signal 和 cancellation 处理 `/stop`。
- pre-turn 与 mid-turn compaction 管理 context。
- `CodexTurnEvent` 记录 turn 事件。
- tool result 按调用顺序写回，避免 Chat Completions tool message 序列损坏。
- turn 结束时处理 goal accounting、memory extraction、progress final。

这一层是 loop engineer 的发动机。

### 3. 工具层：把能力做成可控接口

核心文件：

- `crates/agent/src/tools/mod.rs`
- `crates/agent/src/tools/exec_command.rs`
- `crates/agent/src/tools/update_plan.rs`
- `crates/agent/src/tools/goal.rs`
- `crates/agent/src/tools/tool_search.rs`
- `crates/agent/src/session/tool_dispatch.rs`

| 类型 | 代表能力 | 作用 |
| --- | --- | --- |
| 终端工具 | `exec_command` / `write_stdin` | 真实执行命令、长期 session、交互输入 |
| 文件工具 | read/write/list/apply_patch/view_image | 代码读写和视觉检查 |
| 状态工具 | `update_plan` / `set_title` / goal tools | 让 loop 有计划、标题、目标和预算 |
| 扩展工具 | MCP resources、MCP tools、skills、`tool_search` | 延迟发现和加载外部能力 |

值得学习的一点是：并不是所有工具都并发。`turn_runtime.rs` 只把 `read_file`、`list_directory`、`view_image` 这类读操作标记为 parallel；`exec_command`、`apply_patch`、`update_plan`、goal、`write_stdin` 等有副作用的工具保持 ordered。这是可靠性优先的选择。

### 4. 可观测层：让 loop 能被解释和恢复

核心文件：

- `crates/agent/src/session_status.rs`
- `crates/agent/src/persistence.rs`
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`
- `web/src/pages/AI/AgentChatSection.*`

可观测信息包括：

- 当前状态：model_response、tool_calls、waiting 等。
- Loop 进度：当前第几次 / 最多几次 / 已完成几次。
- Token 与 context：累计 token、最近响应 token、context window 使用率。
- Compaction：显式压缩次数和上下文管理说明。
- 工具数：MCP 工具、本地工具数量。
- Guide queue：运行中新增引导消息。
- JSONL history：session_start、user_message、assistant_message、tool_call、tool_result、compaction、plan_updated、goal_updated、run_state_changed 等事件。

这层的意义是：出了问题可以回答“Agent 卡在哪、做过哪些工具调用、是否压缩、是否超上下文、是否仍在运行”。

### 5. 长任务层：减少模型轮询浪费

核心文件：

- `design/agent-long-task-suspension.md`
- `crates/agent/src/tools/exec_command.rs`
- `crates/agent/src/session/turn_loop.rs`

传统模式是：

```text
model -> exec_command
tool returns session_id
model -> write_stdin poll
model -> write_stdin poll
model -> write_stdin poll
```

每次 poll 都要消耗一次完整模型请求。Cooperative Long Task Loop 的目标是把等待交给 runtime：

```text
exec_command starts long task
runtime monitor watches process/output/exit
no semantic change -> no model request
new output / exit / timeout / user message -> resume model
```

这是非常正确的方向，因为工程任务大量时间消耗在测试、构建、CI、外部 runner、浏览器自动化。模型不应该为了“确认还在跑”反复醒来。

### 6. 进程隔离层：把执行域和控制域分开

核心文件：

- `design/agent-loop-process-isolation.md`
- `crates/bifrost-admin/src/im_gateway/agent_worker.rs`
- `crates/bifrost-admin/src/im_gateway/external_cli/*`

这个项目已经认识到：Agent loop 不能无边界地跑在主进程 Tokio runtime 里。浏览器/CDP 卡住、CPU 密集任务、外部 CLI runner 阻塞，都可能影响代理流量、Admin API 和 IM Gateway。

设计上把 worker 当作执行域，主进程当作控制域：

- 主进程维护 busy gate、SSE、IM 卡片、stop/reset/clear、session preview。
- Worker 执行 turn loop、MCP、CLI runner、浏览器自动化。
- stop 先 cooperative，再 kill process group。
- 生产环境 worker 启动失败 fail closed，不静默回退主进程执行。

### 7. 记忆和技能层：把经验复用纳入 loop

核心文件：

- `design/memory-system-analysis.md`
- `design/agent-skill.md`
- `crates/agent/src/memory/*`
- `crates/agent/src/skills/*`

Memory 系统让 Agent 跨 session 复用用户偏好、任务结论和长期事实：

- `raw_memories.md` 收集原子事实。
- `MEMORY.md` 保存巩固后的长期记忆。
- `memory_summary.md` 作为 read-path developer message 注入。
- `/remember`、`/forget` 提供显式写删。
- MCP 工具 `memory/list|read|search` 支持按需读取。

Skills 系统采用渐进式披露：

- prompt 只注入 skill 名称、描述和路径。
- 真正需要时才读取 `SKILL.md`。
- 避免把所有能力正文一次性塞进上下文。

这两层让 loop engineer 不只是当前 turn 的执行器，而能变成有项目经验和工具记忆的工程伙伴。

## 做得好的地方

### 目标拆解很工程化

Bifrost 不让 Agent 直接从“用户一句话”跳到“写代码”。它强制建立目标验证清单，把交付目标拆成可检查项。

学习点：不要只问“做完了吗”，要问“哪些目标被验证了，哪些旧行为被证明没破坏”。

### 双轮 Review/Fix/Test 是好习惯

至少两轮闭环的设计很重，但有效。第一轮容易发现实现遗漏；第二轮能发现“修复本身引入的问题”和“测试/文档是否同步”。

学习点：review 不是最后看一眼 diff，而是一个持续逼近完成定义的循环。

### 状态型工具和副作用工具没有乱并发

只并发读工具，状态变更工具保持顺序。这避免了 `update_plan` 顺序错乱、`write_stdin` 写错状态、`switch_workdir` 和文件操作交错、`apply_patch` 与读写并发导致 diff 不可解释。

学习点：工具并发的前提不是“能并发”，而是“语义可交换”。

### 可观测性覆盖了用户和开发者两面

用户能看到 progress card、Web timeline、status；开发者能看 JSONL、turn events、test/human_tests。这个组合使得问题可以被复盘，而不是只能重新跑一次。

学习点：Agent loop 的质量，取决于失败时能不能解释。

### Human tests 是项目知识库

`human_tests/readme.md` 的索引非常大，但它不是摆设。它把真实用户感知、历史 bug、CI 回归、UI/API/CLI 验收写成可执行步骤。对复杂产品来说，这是比单测更接近“交付是否真的可用”的证据。

学习点：自动化测试证明局部逻辑，human_tests 证明用户体验和真实链路。

### Codex 对齐思路清晰

`design/agent-codex-alignment.md` 没有盲目照搬 Responses API，而是在保留 Chat Completions 的情况下对齐 prompt 分层、canonical MCP resource tool、统一终端协议、Codex-style turn event 和工具调度顺序。

学习点：对齐先进系统时，不一定要复制 wire format，可以复制关键语义。

## 主要问题和风险

### AGENTS 门禁可能压垮小任务

当前规则要求文档/流程变更也要 human_tests；业务代码还要 coverage gate、E2E、两轮闭环、提交、推送、PR、CI watch。质量很高，但成本也高。

建议：

- 保留严格默认，但增加更明确的轻量分析豁免路径。
- 区分“学习文档”“用户文档”“流程文档”“行为变更文档”的验证矩阵。

### 现状、方案、目标混在一起

多个 design 文档同时包含已落地能力、目标行为、Phase 计划、历史里程碑。对学习者来说，容易误以为所有描述都已经实现。

建议：

- 每篇 design 文档增加固定字段：`实现状态：已落地 / 部分落地 / 设计中`。
- 对关键能力加代码证据链接或测试证据链接。
- 将“现状问题”和“目标方案”视觉上分区。

### 复杂度集中在 session/turn loop

`turn_loop.rs` 同时处理 slash、preflight、memory、compaction、model request、tool loop、goal、stop、progress、long task、history。虽然已有拆分注释，但文件仍承担大量职责。

建议：

- 继续把 slash/preflight、compaction、tool execution、goal lifecycle、memory post-turn 分离成更薄的 orchestrator。
- 保持 public `run_turn*` contract 稳定，但内部按状态机拆模块。

### CI/human_tests 强依赖执行者诚实记录

规则要求真实执行，但文档本身不能证明是否真的执行。最终仍要依赖 Agent 把命令、结果、失败归因写清楚。

建议：

- 对 human_tests 增加轻量机器校验：每个测试文档的执行记录格式、日期、PASS/FAIL、命令块可被脚本扫描。
- 对最终交付摘要建立结构化模板，便于机器抽取验证矩阵。

### 长任务最终形态还需要持续验证

Cooperative Long Task Loop 涉及 runtime monitor、append-only transcript、恢复摘要、用户插话、stdin、取消、持久化 waiting state 等多个难点。

建议：

- 长任务 E2E 按时长分层持续加回归：短任务、分钟级、半小时级、交互式、无输出、巨量输出、失败退出、用户打断。
- 对 waiting state 增加统一诊断命令或 API。

### 多入口一致性难维护

同一个 Agent loop 可以从 Web、IM Gateway、Admin API、external runner、schedule、daily agent 触发。入口越多，状态一致性越难。

建议：

- 所有入口尽量收敛到 canonical timeline + channel projection。
- UI/IM 只做 projection，不重新定义 loop 语义。
- E2E 保持跨通道用例，例如 Web 触发、IM 观察，IM 触发、Web 恢复。

## 用到了哪些工具、系统、能力

### 本地工程工具

- `git status` / `git diff` / `git worktree`：工作区隔离和变更证据。
- `rg` / `sed` / shell：代码和文档检索。
- `cargo test` / `cargo check` / `cargo clippy` / `cargo fmt`：Rust 验证。
- `pnpm --dir web ...` / Playwright：Web UI 构建与真实浏览器验证。
- `scripts/ci/local-ci.sh`：本地 CI wrapper。
- `make coverage` / `scripts/ci/coverage-all.sh --json --gate`：覆盖率门禁。

### Agent 内置工具

- `exec_command`：运行真实命令，支持 pipe/TTY、yield、长任务 session。
- `write_stdin`：对已有 terminal session 轮询、输入、Ctrl-C。
- `apply_patch`：结构化修改文件。
- `update_plan`：维护任务计划并向用户展示 progress。
- `get_goal` / `create_goal` / `update_goal`：跨 turn 目标和预算跟踪。
- `request_user_input`：计划模式下的结构化用户输入入口；当前 Bifrost runtime 主要做校验，缺交互 wait channel。
- `tool_search`：延迟发现 deferred tools，避免一次性暴露过多工具。
- `view_image`：视觉验证。

### Bifrost Runtime 系统

- `AgentSession`：会话状态、history、plan、goal、memory、progress。
- `ConversationRecorder` / JSONL：持久化事件流。
- `CodexTurnEvent`：turn 运行事件。
- `ToolRegistry` / MCP / Skills：工具注册与扩展。
- `ExecSessionManager`：终端 session、append-only transcript、退出 watcher。
- `AgentWorker`：进程隔离执行域。
- IM Gateway progress card：飞书过程展示。
- Web Agent Chat timeline：Web 过程展示和恢复。
- Memory subsystem：长期记忆抽取、巩固、召回。

### 外部协作系统

- GitHub Actions：远端 CI。
- `.agents/skills/github-actions-pat/`：fail-fast CI 诊断与 fix-push-watch。
- `.agents/skills/e2e-test/`：E2E 规范。
- `human_tests/`：真实场景验收知识库。
- MCP servers/resources：外部工具和资源能力接入。

## 如何学习这套体系

建议按以下顺序读：

1. `design/agent-development-review-loop.md`
   先理解工程闭环规则，知道为什么每次任务都要目标清单、两轮 review、human_tests、CI 看护。

2. `crates/agent/src/tools/mod.rs` 和 `crates/agent/src/tools/update_plan.rs`
   看工具是如何被注册、排序、暴露给模型，以及 plan 如何从普通 tool result 变成 runtime event。

3. `crates/agent/src/session/turn_loop.rs`
   重点看 `run_turn_with_mcp_multimodal()` 周边：输入处理、model request、tool calls、compaction、stop、goal、memory。

4. `crates/agent/src/turn_runtime.rs`
   看事件类型、取消 token、工具并发/顺序分类。

5. `crates/agent/src/session_status.rs`
   看 loop 如何被用户和开发者观察。

6. `design/agent-long-task-suspension.md`
   学习如何把长任务等待从模型轮询迁移到 runtime monitor。

7. `design/agent-loop-process-isolation.md`
   学习主进程控制域和 worker 执行域如何分离。

8. `design/memory-system-analysis.md` 与 `design/agent-skill.md`
   理解 loop 如何拥有长期记忆和渐进式技能加载。

## 可借鉴的方法论

### 把 Agent 工程化，不只提示词化

提示词可以提醒模型“要认真”，但系统设计要让认真变成默认路径。Bifrost 的做法是把目标、计划、工具、状态、测试、CI 都纳入 runtime 和文档门禁。

### 把“过程”作为一等产物

最终回答只是结果，过程事件才是可复盘证据。turn events、progress card、JSONL、human_tests 都是在保存过程。

### 把副作用边界显式化

能并发的工具必须证明无副作用；能自动恢复的长任务必须有 transcript 和 cursor；能 stop 的任务必须有 cooperative 和 kill 两层。

### 把真实场景沉淀成测试文档

Bifrost 的 `human_tests/` 保存了产品真实使用路径和历史事故。对 Agent 项目来说，这是非常宝贵的回归资产。

## 改进建议清单

- 为 design 文档增加统一 `实现状态` 字段，减少“目标方案”和“已落地事实”的混淆。
- 给 `turn_loop.rs` 继续拆模块，降低单文件认知负担。
- 为 human_tests 执行记录加结构化格式和脚本扫描。
- 为长任务 waiting state 增加统一诊断视图。
- 对轻量文档分析任务提供明确低成本验证路径，避免严格门禁过度消耗。
- 继续强化跨入口 canonical timeline，避免 Web/IM/API 各自解释 loop 状态。

## 本次分析依据

本次分析基于当前仓库静态阅读，重点参考：

- `AGENTS.md`
- `design/agent-development-review-loop.md`
- `design/agent-codex-alignment.md`
- `design/agent-long-task-suspension.md`
- `design/agent-loop-process-isolation.md`
- `design/memory-system-analysis.md`
- `design/agent-skill.md`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/turn_runtime.rs`
- `crates/agent/src/tools/mod.rs`
- `crates/agent/src/tools/exec_command.rs`
- `crates/agent/src/tools/update_plan.rs`
- `crates/agent/src/tools/goal.rs`
- `crates/agent/src/tools/tool_search.rs`
- `crates/agent/src/session_status.rs`
- `human_tests/readme.md`

