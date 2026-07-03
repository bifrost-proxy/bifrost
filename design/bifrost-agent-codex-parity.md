# Bifrost Agent Codex Parity

## 背景

Bifrost 内置 IM Agent 与 OpenAI Codex CLI 有一段相似的产品语义（turn loop、tool calling、reasoning、slash commands、pending input），但两者的实现来源不同：Codex 是从零基于 Responses API 的 native agent runtime；Bifrost Agent 起初围绕 Chat Completions + IM 消息队列 + WebUI/History 集成搭起，逐渐叠加了 hosted tool、reasoning、MCP、Skills、file ops 等能力，把 turn loop、session store、tool dispatch、compaction、slash、progress、pending input 都堆在一个 `session.rs` 文件里。

这种堆叠让新增能力（例如 Responses streaming、MCP、路径切换）越来越难加，也难以维持与 Codex 上游能力的语义对齐。本设计描述 Bifrost Agent 的 **Codex Parity** 计划：**在保留 Bifrost IM Agent 产品边界、消息通道、会话存储、IM guide/queue、WebUI/History 集成与 MCP/Skills 能力** 的前提下，把 Codex runtime 里已经验证有效的四条契约引入：统一 turn loop、统一 ToolOrchestrator、Responses streaming、Session 模块拆分；再加上 Codex 风格的绝对路径快切换。

唯一例外：Bifrost 内置 Agent **不做 Codex 的限制性安全沙箱**。默认授权是完全开放（Open / NeverAsk / Sandbox Disabled），因为 Bifrost 面向的是内部工程师本机代理调试场景，用户已经在本机安装了 Bifrost，再叠加一层文件系统/进程/网络沙箱既扰乱工作流，也与「本地代理调试助手」定位冲突。策略类型仍然保留，作为工具调用统一契约和后续审计点，但当前实现必须 always allow，不弹审批、不限制文件系统/进程/网络。

## 用户目标验证清单

### 必须实现

- 统一 turn loop 契约：model response、tool calls、tool results、pending input、progress event、compaction 都按可观察状态推进，不再散落在 session.rs 各处 if 分支。
- 统一 ToolOrchestrator：`exec_command`、`write_stdin`、`apply_patch`、file ops、`tool_search` 和 MCP call 走同一个入口，policy decision 与 tracing 集中。
- Responses streaming：OpenAI builtin provider 默认 `wire_api = responses`；其他 OpenAI-compatible provider（内部 AIDP、第三方兼容 endpoint）继续默认 `chat_completions`，避免破坏现有集成。
- Session 模块按职责拆分成 `session_store`、`tool_dispatch`、`compaction`、`slash_commands`、`progress`、`pending_input`、`turn_loop`、`turn_context`、`turn_timing`、`hooks`、`steer`、`task`、`tests`、`path_switch_tests`。
- Codex 风格绝对路径切换：用户消息若是真实存在的绝对目录路径，识别为 work_dir 切换，不进入 slash router、不调用模型、不执行工具。
- `ToolAuthorizationConfig` / `PermissionProfile` / `ApprovalPolicy` / `SandboxPolicy` 类型保留但默认全部开放；`ToolOrchestrator::open()` / `Default` 都产生开放配置。

### 必须不破坏

- Bifrost IM Agent 的消息通道、Session 存储、Feishu/Lark 群消息队列、WebUI Chat/History 展示、`/status` 面板、`bifrost agent` CLI 命令。
- MCP 已接入的所有工具：MCP call 只是走进新的 ToolOrchestrator，行为语义不变。
- Skills 加载、slash router、system prompt、reasoning delta 展示。
- 非 OpenAI 的兼容 provider（内部 AIDP + 各类第三方）：继续走 Chat Completions，不因 Responses 支持而回归失败。
- 现有 `agent-*.md` 设计文档（chat-history-pagination、long-task-suspension、loop-process-isolation、runtime-limits、session-context-restore、token-usage、context-status-compaction 等）：本设计只对齐 Codex Parity 相关部分，其他能力保持独立文档权威。

### 必须真实验证

- 单元测试覆盖 authorization 默认开放、Responses SSE 解析、config resolve、session 拆分模块契约、路径切换判定。
- E2E 覆盖：真实 Bifrost 启动 + OpenAI-compatible mock provider，走 `/agent/chat`，`/v1/responses` request/response 校验；绝对路径切换真实链路。
- human_tests `human_tests/agent-codex-parity.md` 逐条真实跑通。

## 产品语义

### Codex Parity 的四层契约

1. **turn loop**：一次「用户输入 → 模型响应 → 若干轮工具调用 → 最终答复 → 状态收尾」。turn loop 是 session 的主循环，必须能被 mock、能被单元测试、能承载 progress/pending 事件。
2. **tool orchestration**：所有工具（本地 exec、write_stdin、apply_patch、file ops、tool_search、MCP tools）都走 `ToolOrchestrator`。ToolOrchestrator 只做 policy decision + tracing，不改变工具执行结果。
3. **model wire protocol**：OpenAI Responses API 是新一代协议（`input` + `stream` + `reasoning` + Responses function tools shape），能承载 hosted tool 与 reasoning summary；Chat Completions 是回退。`ModelWireApi::Responses` 和 `ChatCompletions` 通过 `EffectiveModelConfig.wire_api` 表达。
4. **session module boundaries**：session.rs 只保留 turn loop 主体和公共入口；session store、tool dispatch、compaction、slash commands、progress、pending input 各自独立模块，测试可独立跑。

### 开放授权底座

`ToolAuthorizationConfig`：

```rust
pub struct ToolAuthorizationConfig {
    pub permission_profile: PermissionProfile, // Open (default)
    pub approval_policy: ApprovalPolicy,       // NeverAsk (default)
    pub sandbox_policy: SandboxPolicy,         // Disabled (default)
}

impl ToolAuthorizationConfig {
    pub const fn open() -> Self { /* all defaults */ }
    pub fn is_fully_open(&self) -> bool {
        matches!(self.permission_profile, PermissionProfile::Open)
            && matches!(self.approval_policy, ApprovalPolicy::NeverAsk)
            && matches!(self.sandbox_policy, SandboxPolicy::Disabled)
    }
}
```

- 类型保留是为了后续如果 Bifrost Agent 部署到共享/托管环境（例如企业内网托管），可以复用同一 orchestrator 只切策略；当前默认必须 always allow。
- `PermissionProfile::Open` / `ApprovalPolicy::NeverAsk` / `SandboxPolicy::Disabled` 是当前唯一被单元测试断言的实际行为。

### Responses streaming vs Chat Completions

OpenAI builtin provider（`provider_id = "openai"` 或类似 vendor tag）resolve 到 `EffectiveModelConfig.wire_api = Responses`；其他 provider 默认 `ChatCompletions`。

Responses 请求 shape 关键字段：

- `input`：Codex 风格的多角色消息（不是 Chat Completions 的 `messages`）。
- `max_output_tokens`
- `stream: true`
- `reasoning: { effort, summary }`
- Responses function tools shape（`tools: [{ type: "function", name, description, parameters }]`）

SSE parser 需要能处理：

- `response.output_text.delta`：文本流。
- `response.output_text_reasoning.delta`（或 reasoning summary delta）：reasoning 展示。
- `response.output_item.done` 且 `type = function_call`：工具调用。
- `response.completed`：usage + status。

Chat Completions 路径保留原始实现，但只在 `wire_api = ChatCompletions` 时走；不再继续在 Chat Completions body 上堆 reasoning/hosted-tool 兼容逻辑。

### Session 拆分边界

- `session.rs`：turn loop 主体、公共 `AgentSession` 入口、生命周期。
- `session/session_store.rs`：`AgentSessionManager`、session list/detail DTO、持久化 helper。
- `session/tool_dispatch.rs`：local registry、local handler、MCP call 统一调度 helper。
- `session/compaction.rs`：session-local compaction / progress helper。
- `session/slash_commands.rs`：session slash dispatch helper（与 crate 级 slash.rs 分层）。
- `session/progress.rs`：assistant delta / final progress event helper。
- `session/pending_input.rs`：pending queue drain helper。
- `session/turn_loop.rs`：turn loop 主体，包含 `direct_workdir_switch_path` 快路径。
- `session/turn_context.rs` / `turn_timing.rs`：turn-level 上下文与计时。
- `session/hooks.rs`：turn 钩子（before/after model, after tool 等）。
- `session/steer.rs`：steer 请求 / cancel 处理。
- `session/task.rs`：异步任务驱动 helper。
- `session/tests.rs`：原 session.rs 内联测试外置。
- `session/path_switch_tests.rs`：path-switch 行为独立测试文件，避免与主 session 测试互相污染 mock 状态。

后续新增 session 行为必须优先落到对应模块并附单元测试；不允许再把新逻辑塞回 `session.rs`。

### Codex 风格绝对路径切换

`turn_loop.rs::direct_workdir_switch_path(input) -> Option<String>`：

- 输入 trim 后同时满足 `Path::is_absolute()` 与 `Path::is_dir()` 才返回 `Some(dir)`。
- 排除相对路径、带参数的 `/foo bar`、以 `/` 开头但不是真实目录的字符串（例如 `/help`、`/status`）、文件路径。
- 排除方案设计上不依赖 slash router：即使 `/Users` 恰好和一个假想 slash 命令首段同名，也不会被误判——因为要求整段完全是一个真实存在的目录。

命中后：

- 调用 `session.reinitialize_work_dir(new_dir)`：清空旧会话历史、重新加载目标目录配置。
- 返回 `TurnOutcome::WorkDirSwitched { work_dir: new_dir }` 或结构化字段 `work_dir_switched: Some(new_dir)`。
- 不调用模型、不执行工具、不消费 pending input（除了这条 workdir 消息本身）。
- 保持 `/status`、IM runtime、WebUI session 状态与新 workdir 一致。

## 技术细节

### ToolOrchestrator

```rust
pub struct ToolOrchestrator {
    config: ToolAuthorizationConfig,
}

impl ToolOrchestrator {
    pub fn open() -> Self { Self { config: ToolAuthorizationConfig::open() } }
    pub fn new(config: ToolAuthorizationConfig) -> Self { Self { config } }
    pub fn config(&self) -> &ToolAuthorizationConfig;
    // dispatch 前的统一 policy decision，本地/MCP 一致
    pub fn authorize(&self, tool_kind: ToolKind) -> AuthorizationDecision;
}
```

- 本地工具走 `session::tool_dispatch::run_local_tool(&orchestrator, ...)`。
- MCP 工具走 `session::tool_dispatch::run_mcp_tool(&orchestrator, mcp_manager, ...)`。
- Orchestrator 不解析工具参数、不改结果，只在调用前打 tracing / metrics，并根据 config 决定 allow/deny/prompt（当前始终 allow）。

### ModelWireApi resolve

`ModelConfig.wire_api: Option<ModelWireApi>` → provider-specific default → `EffectiveModelConfig.wire_api: ModelWireApi`。

- OpenAI builtin provider：默认 `Responses`。
- 其他 OpenAI-compatible provider：默认 `ChatCompletions`。
- 用户显式指定 `wire_api = "responses"` 或 `"chat_completions"` 覆盖 provider default。
- config resolve 单测覆盖三条：OpenAI builtin → Responses；AIDP → ChatCompletions；third-party generic → ChatCompletions。

### Responses request 构造

URL：`{base_url}/v1/responses`（若 `base_url` 已带 `/v1` 前缀则不重复）。

Body：

```json
{
  "model": "...",
  "input": [ { "role": "user", "content": [ {"type": "input_text", "text": "..."} ] } ],
  "max_output_tokens": 4096,
  "stream": true,
  "reasoning": { "effort": "medium", "summary": "auto" },
  "tools": [ { "type": "function", "name": "exec_command", "description": "...", "parameters": {...} } ]
}
```

Chat Completions body 不出现在 Responses 请求：单测断言 body 不包含 `messages` / `reasoning_effort` 字段。

### SSE parser

`parse_responses_event(chunk)` 需识别：

- `event: response.output_text.delta` + `data: {"delta": "..."}` → 文本增量。
- reasoning delta 事件（按上游最新 event name）。
- `event: response.output_item.done` + `data: {"item": {"type": "function_call", ...}}` → 累积工具调用。
- `event: response.completed` + `data: {"response": {"usage": {...}, "status": "..."}}` → 收尾 usage。
- 未识别事件 → 忽略但不 error（未来 event 平滑演进）。

### direct_workdir_switch_path

```rust
fn direct_workdir_switch_path(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() { return None; }
    let p = Path::new(trimmed);
    if p.is_absolute() && p.is_dir() {
        return Some(trimmed.to_string());
    }
    None
}
```

在 `turn_loop.rs` 主循环入口、slash router 分发前检测；命中时直接构造 `TurnOutcome::WorkDirSwitched` 并返回，跳过所有下游模型/工具调用。

## CLI / Web / Admin API 接触面

- CLI：不新增专门的 codex-parity 子命令；`bifrost agent chat`、`bifrost agent send` 等既有命令语义不变。用户绝对路径切换会被 IM/CLI 通道原样透传到 turn loop。
- WebUI：Chat 面板继续显示 assistant delta / reasoning summary / tool calls；`/status` 面板已经能看到 work_dir，切换后自动刷新。
- Admin API：`/agent/chat` 输出结构新增 `work_dir_switched: Option<String>`；前端识别到该字段时提示「工作目录已切换到 <dir>」而不是渲染成 assistant 回复。

## Sync 边界

Codex Parity 相关的能力全部本地生效，不参与 Sync：

- Authorization / orchestrator / wire_api resolve 都是本机 config，不推送到 relay。
- Session 拆分模块的持久化仍走既有 `AgentSessionManager`，不改数据格式。
- Codex 风格路径切换是 session-local 快路径，绝对不把本地路径发到外部模型或 relay；单测和 E2E 都要断言 mock provider 不收到路径 payload。

## 实现切分

### Phase 1：开放授权底座与 ToolOrchestrator

- `crates/agent/src/authorization.rs`：定义 `PermissionProfile`、`ApprovalPolicy`、`SandboxPolicy`、`ToolAuthorizationConfig`、`ToolOrchestrator`。
- `crates/agent/src/session/tool_dispatch.rs`：把原 `session.rs` 里 local/MCP tool dispatch 抽出，接受 `&ToolOrchestrator`。
- `crates/agent/src/session.rs`：turn loop 里所有工具调用改走 `tool_dispatch`。
- 单元测试：默认 open、is_fully_open、local/MCP 都放行。

### Phase 2：Responses Streaming Client

- `crates/agent/src/responses.rs`：`build_responses_request`、`parse_responses_event`。
- `crates/agent/src/client.rs`：按 `EffectiveModelConfig.wire_api` 分发。
- `crates/agent/src/config.rs`：`ModelWireApi`、`EffectiveModelConfig.wire_api`、`resolve_effective_config`。
- 单元测试：URL 转换、SSE 事件解析、mock HTTP body 断言、config resolve 三条。

### Phase 3：Session 拆分

- 按上文 13 个模块把 `session.rs` 拆分（session_store / tool_dispatch / compaction / slash_commands / progress / pending_input / turn_loop / turn_context / turn_timing / hooks / steer / task / tests / path_switch_tests）。
- `session.rs` 只保留 turn loop 主体与公共入口。
- 每个子模块自带单元测试。

### Phase 4：Codex 风格绝对路径切换

- `crates/agent/src/session/turn_loop.rs::direct_workdir_switch_path` + turn loop 主循环入口集成。
- `crates/agent/src/session/tests.rs` / `path_switch_tests.rs` 单测：绝对目录切换命中、相对路径不命中、`/help` 不命中、文件路径不命中、mock 模型不被调用、旧 history 已清空。
- `e2e-tests/tests/test_agent_direct_path_switch.sh`：真实 Bifrost + mock OpenAI provider，`/agent/chat` 先业务 turn、再发绝对目录路径，断言 mock 只收到 1 次业务 turn、`/status` 显示新 work_dir。
- `human_tests/agent-codex-parity.md` 补充 TC-CDX-*用例。

## 测试方案

### 单元测试

- `authorization::tests`：默认 open；`is_fully_open` 断言 Open + NeverAsk + Disabled；`ToolOrchestrator::open()` 与 `Default` 等价。
- `responses::tests`：SSE 文本 delta / function_call / usage 三条；`build_responses_request` body shape。
- `config::tests`：OpenAI provider → Responses；AIDP → ChatCompletions；third-party generic → ChatCompletions。
- `session/tests.rs`：turn loop 基本推进、tool dispatch 契约。
- `session/path_switch_tests.rs`：`direct_workdir_switch_path` 覆盖 6 类输入。

### E2E 测试

- `e2e-tests/tests/test_agent_codex_parity_contracts.sh`：开放授权 + Responses streaming + session 拆分编译契约；mock OpenAI provider 断言 request path 为 `/v1/responses`、body 含 `input`/`stream`/`reasoning`，不含 `messages`/`reasoning_effort`。
- `e2e-tests/tests/test_agent_direct_path_switch.sh`：绝对目录切换真实链路，见上。

### 真实场景测试 human_tests

`human_tests/agent-codex-parity.md`：

- TC-CDX-01：开放授权真实工具调用（exec_command 读写本地文件），不弹审批。
- TC-CDX-02：OpenAI builtin provider 触发 Responses streaming（观察 `/agent/chat` payload 与 mock provider 收到的 `/v1/responses` 请求）。
- TC-CDX-03：AIDP 或第三方 provider 保持 Chat Completions。
- TC-CDX-04：Session 拆分后 slash / pending input / compaction 表现无回归。
- TC-CDX-05：绝对目录路径切换后 `/status` 与 IM/WebUI 显示新 work_dir，模型未被调用。

所有 human_tests 使用临时数据目录、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-agent authorization responses config session`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 至少执行一次 `bash scripts/ci/local-ci.sh --skip-e2e`；`--include-e2e` 版本在 CI 上跑。

no-local-coverage 本地约定生效时不运行 `make coverage`，交付说明豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：四层契约 + 开放授权 + 绝对路径切换。
- 复核 diff：authorization / responses / config / session 各模块拆分是否完整；`session.rs` 是否仍是 turn loop 主体、无新增杂项。
- 重点 review：
  - Responses request body 是否 100% 移除 Chat Completions 字段；
  - `ToolOrchestrator` 是否已经拦截所有本地/MCP 调用点，没有绕过路径；
  - `direct_workdir_switch_path` 是否在 slash router 之前分发。
- 复测：全部单元 + 两条 E2E 脚本 + `cargo test --workspace --all-features`。

### 第 2 轮

- 复核第 1 轮修复。
- 再看 `git status --short` / `git diff`：新增文件、human_tests 索引、README/AGENTS 引用是否同步。
- 重点 review：
  - IM/WebUI 消息通道在 wire_api 切换后无回归；
  - session 拆分后 pending input drain 顺序仍正确；
  - workdir 切换后 `/status` 与 tool_dispatch 使用的路径变量一致。
- 复测：human_tests TC-CDX-01..05 真实跑；`test_agent_direct_path_switch.sh` 至少跑 3 次验证稳定。

## 风险与决策

- **开放授权**：本设计明确不引入沙箱；若未来 Bifrost Agent 进入托管/多租户场景，需要额外增加 `ToolAuthorizationConfig` 的落地策略与 UI，不在本设计范围。
- **Responses 上游演进**：`event: response.*` 名字与 payload shape 在上游可能演进；SSE parser 保留“未识别事件忽略”策略，避免上游演进直接击穿；单测覆盖当前 event 集合。
- **兼容 provider 回退**：不给所有 OpenAI-compatible provider 默认开 Responses，是因为许多第三方兼容 endpoint 只实现 Chat Completions；一次性打开会破坏兼容路径。
- **Session 拆分风险**：拆分是纯重构 + 单测，但 turn loop 内部状态跨模块调用需要严格保持顺序（例如 pending input drain 必须在下一次 model call 前）；review 时用测试 + 手动 walkthrough 双保险。
- **路径切换误触发**：`Path::is_dir()` 会做系统调用；如果用户输入常见文件路径（`/etc/hosts`），`is_dir()` 返回 false 不触发切换，行为正确；如果是超长路径导致系统调用慢，最坏影响是 turn loop 入口多一次 stat，属于可接受成本。
- **不改变 IM Agent 产品边界**：本设计只做 runtime 契约对齐，不改变 IM Agent 的消息通道、Session 存储、WebUI/History 集成、MCP/Skills 接入方式。
