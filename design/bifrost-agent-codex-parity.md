# Bifrost Agent Codex Parity

## 目标边界

Bifrost 内置 Agent 不完全复刻 Codex runtime。目标是保留 Bifrost IM Agent 的产品边界、消息通道、会话存储、IM guide/queue、WebUI/History 集成与现有 MCP/Skills 能力，同时引入 Codex runtime 的关键契约：

- 统一的 turn loop 契约：model response、tool calls、tool results、pending input、progress event 和 compaction 都按可观察状态推进。
- 统一的 tool orchestration 契约：`exec_command`、`write_stdin`、`apply_patch`、file ops、`tool_search`、MCP call 进入同一个 ToolOrchestrator。
- Responses streaming 模型协议：OpenAI 官方 provider 使用 Responses API；Chat Completions 只作为非 Responses 兼容 provider 的 fallback，不再继续在 Chat Completions body 上堆 reasoning/hosted-tool 兼容逻辑。
- Session 模块按职责拆分，避免继续把 turn loop、session store、tool dispatch、compaction、slash commands、progress 和 pending input 堆在单文件。

唯一例外：Bifrost 内置 Agent 不做 Codex 的限制性安全沙箱。默认授权是完全开放的：

- `PermissionProfile::Open`
- `ApprovalPolicy::NeverAsk`
- `SandboxPolicy::Disabled`

这些策略类型仍然保留，因为它们是工具调用的统一契约和后续审计点；当前实现必须 always allow，不弹审批、不限制文件系统/进程/网络。

## 第一阶段：开放授权底座与 ToolOrchestrator

实现文件：

- `crates/agent/src/authorization.rs`
- `crates/agent/src/session/tool_dispatch.rs`
- `crates/agent/src/session.rs`

设计要求：

- `ToolAuthorizationConfig` 表达 permission profile、approval policy、sandbox policy，但默认值全部开放。
- `ToolOrchestrator` 是本地工具和 MCP 工具的统一入口。
- 本地工具路径覆盖 `exec_command`、`write_stdin`、file ops、`apply_patch`、`tool_search` 和 registry 内其他工具。
- MCP 路径覆盖 `McpManager::call_tool`。
- ToolOrchestrator 不改变工具执行结果，只在调用前做统一 policy decision 和 tracing。

测试要求：

- 单元测试验证默认 policy 是 open/never ask/sandbox disabled。
- 单元测试验证 local/MCP 都被开放策略放行。
- Session turn loop 至少通过编译测试确认 tool dispatch 类型边界正确。

## 第二阶段：Responses Streaming Client

实现文件：

- `crates/agent/src/responses.rs`
- `crates/agent/src/client.rs`
- `crates/agent/src/config.rs`

设计要求：

- `ModelWireApi` 支持 `chat_completions` 与 `responses`。
- `EffectiveModelConfig` 携带 resolved `wire_api`。
- OpenAI builtin provider 默认 `responses`。
- 其他 OpenAI-compatible provider 继续默认 `chat_completions`，避免破坏内部 AIDP 与第三方兼容路径。
- Responses request 使用 `input`、`max_output_tokens`、`stream: true`、`reasoning: { effort, summary }` 和 Responses function tools shape。
- Chat Completions request 仍保留旧路径，但只有 wire_api 为 `chat_completions` 时使用。
- SSE parser 支持 `response.output_text.delta`、reasoning delta、`response.output_item.done` 的 `function_call`、`response.completed` usage/status。

测试要求：

- URL 转换：`/v1/chat/completions` 转 `/v1/responses`。
- SSE 解析：文本 delta、function_call、usage。
- Mock HTTP streaming client：断言 request path 为 `/v1/responses`，body 包含 `input`/`stream`/`reasoning`，且不包含 Chat Completions 的 `messages`/`reasoning_effort`。
- Config 测试：OpenAI provider resolves to `ModelWireApi::Responses`。

## 第三阶段：Session 拆分

实现文件：

- `crates/agent/src/session.rs`
- `crates/agent/src/session/session_store.rs`
- `crates/agent/src/session/tool_dispatch.rs`
- `crates/agent/src/session/compaction.rs`
- `crates/agent/src/session/slash_commands.rs`
- `crates/agent/src/session/progress.rs`
- `crates/agent/src/session/pending_input.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/session/tests.rs`

拆分边界：

- `session_store.rs`：`AgentSessionManager`、session list/detail DTO。
- `tool_dispatch.rs`：local registry、local handler、MCP call 统一调度 helper。
- `compaction.rs`：session-local compaction/progress helper。
- `slash_commands.rs`：session slash dispatch helper。
- `progress.rs`：assistant delta/final progress event helper。
- `pending_input.rs`：pending queue drain helper。
- `turn_loop.rs`：turn loop 小型公共 primitive。
- `tests.rs`：原 `session.rs` 内联测试外置。

后续收敛要求：

- `session.rs` 当前仍是 turn loop 主体，后续新增功能不得再塞回主文件；优先落到上面的职责模块。
- 新增 session store、pending input、progress、tool dispatch 行为时必须优先在对应模块加测试。

## 第四阶段：Codex 风格绝对路径切换

实现文件：

- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/session/tests.rs`
- `e2e-tests/tests/test_agent_direct_path_switch.sh`

设计要求：

- 用户消息如果是一个真实存在的绝对目录路径（例如 `/Users/eden/work/github/bifrost`），应被识别为工作目录切换请求，而不是 slash router 的未知 `/Users/...` 命令。
- 该路径在 slash router 分发前检测，且仅当完整输入同时满足 `Path::is_absolute()` 与 `Path::is_dir()` 时触发，避免把普通文本、相对路径、带参数的 `/foo bar`、不存在路径或文件路径误判为切换目录；这也避免了真实目录第一段刚好与已注册 slash/skill 命令同名时被 router 截获。
- 切换时调用 `AgentSession::reinitialize_work_dir`，清空旧会话历史、重新加载目标目录配置，并返回 `work_dir_switched`，保持 `/status`、IM runtime 与 WebUI session 状态一致。
- 切换路径是 session-local 快路径，不调用模型、不执行工具，避免将本地路径误发给外部模型。

测试要求：

- 单元测试验证绝对目录输入直接切换 work_dir、不返回“未知命令”、清空旧 history 且不消费 mock 模型响应。
- E2E 脚本启动真实 Bifrost 服务和 OpenAI-compatible mock provider，经 `/agent/chat` 先建立 session，再发送第二个绝对目录路径，断言响应为切换成功、mock provider 只收到第一次业务 turn，请求 `/status` 后工作路径已变为新目录。
- human_tests 在 `human_tests/agent-codex-parity.md` 增加回归用例，并按用例真实执行。

## 验证计划

- 单元测试：`authorization`、`responses`、`config`、session 原有测试。
- E2E：新增 `e2e-tests/tests/test_agent_codex_parity_contracts.sh`，覆盖开放授权、Responses streaming、session 拆分编译契约；新增 `e2e-tests/tests/test_agent_direct_path_switch.sh`，覆盖真实 `/agent/chat` 绝对目录切换链路。
- human_tests：新增/更新 `human_tests/agent-codex-parity.md`，逐条执行开放授权、Responses streaming、session 拆分、Codex 风格路径切换与 E2E 脚本。
- 收尾：执行 `rust-project-validate`，并至少执行一次 `cargo test --workspace --all-features`。
