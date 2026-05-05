# IM Gateway Agent 设计文档

## 架构概览

Agent 功能集成在 Bifrost 的 IM Gateway 模块中，通过 Azure 兼容的 Chat Completions API 提供 LLM 驱动的对话 AI 能力。当用户通过飞书发送消息时，Agent 会将其处理并通过模型生成回复。

```
┌─────────────┐      ┌──────────────┐      ┌─────────────────┐
│   Feishu    │─────▶│  IM Gateway  │─────▶│  ImAgentClient  │
│  WebSocket  │      │   Router     │      │  (Chat API)     │
└─────────────┘      └──────────────┘      └─────────────────┘
                            │                       │
                            ▼                       ▼
                     ┌──────────────┐      ┌─────────────────┐
                     │   Session    │      │  LLM Provider   │
                     │   Manager    │      │  (aidp_crawl)   │
                     └──────────────┘      └─────────────────┘
```

## Agent Loop tool message 序列稳定性

### 问题根因

2026-05-02 发现生产默认数据目录中的 IM Agent 会话在恢复后可能向模型发送非法 Chat Completions 消息序列：

```text
API error (status 400 Bad Request): messages with role 'tool' must be a response to a preceeding message with 'tool_calls'
```

根因在 `crates/agent/src/persistence.rs::load_conversation()`：JSONL 中记录的是事件流，工具轮次以 `tool_call` 和 `tool_result` 两类事件落盘；旧恢复逻辑跳过 `tool_call`，却把 `tool_result` 直接恢复成 `role=tool` 的 `ChatMessage::tool_result("recovered", ...)`。恢复后的历史缺少前置 `assistant(tool_calls)`，下一轮 `build_messages()` 会把 orphan `tool` 发给模型。

默认数据目录中的实际证据位于 `~/.bifrost/agent/sessions/2026/05/02/session-ou_64f88363f262c64aba91f0b9e1aaed81-*.jsonl`：同一轮存在连续的 `tool_call` / `tool_result` 事件，但旧 `load_conversation()` 只恢复 `tool_result`，足以构造出 `messages.[2].role=tool`。

### 修复原则

Chat Completions tool calling 的历史不再把 `tool_result` 当作独立可恢复消息。合法片段必须是：

1. `assistant` 消息包含非空 `tool_calls`
2. 随后紧邻每个 `tool_call.id` 对应的 `role=tool` 消息
3. 不能出现无 `tool_call_id`、未知 `tool_call_id`、重复 `tool_call_id` 或不完整的 tool-call suffix

### 根修复

- `ConversationRecorder` 新增带 `call_id` 的记录方法，正常 turn loop 会把模型返回的真实 tool call id 写入 `tool_call` 和 `tool_result` 事件。
- `load_conversation()` 恢复时读取 `tool_call` 事件，重建 `ToolCallMessage`，再在对应 `tool_result` 到达时生成合法的 `assistant_with_tool_calls([tool_call])` + `tool_result(call_id, result)` 消息对。
- 对历史旧 JSONL 中缺失 `call_id` 的 `tool_call`，恢复时生成稳定的 `recovered-tool-call-N` synthetic id，保证旧会话也不会恢复出 orphan `tool`。
- 恢复层维护 pending tool-call 集合；如果同一轮先连续落盘多个 `tool_call`、再依次落盘 `tool_result`，优先按 `call_id` 精确匹配，旧记录缺少 `call_id` 时按记录顺序匹配，避免单个 pending 被后续工具调用覆盖后把结果错配到错误的 tool call。
- 无前置 `tool_call` 的孤立 `tool_result` 会被跳过，不再进入模型上下文。

### 防御机制

新增 `crates/agent/src/history.rs` 作为统一 history invariant 层：

- `sanitize_chat_history()` 在发送模型请求前检查完整 messages。
- 孤立 `role=tool` 会被删除。
- 不完整的 `assistant(tool_calls)` 片段会被删除，避免残留非法 suffix。
- `build_messages()` 在 max history 裁剪之后统一 sanitize，防止裁剪刚好切掉 assistant tool_calls 后只保留 tool results。
- context overflow trim 每次删除旧消息后统一 sanitize，防止 trim 正好切断 `assistant(tool_calls)` 与 `tool` 结果之间的配对关系。
- compaction 输出历史、context overflow trim 后的历史也会 sanitize。
- 发现修复时写入 warn 日志，包含丢弃的 orphan tool 数和不完整 tool-call 片段数。

### E2E mock 稳定性

`crates/bifrost-e2e/src/tests/im_gateway_agent.rs` 中的 Chat Completions mock 必须按请求消息状态决定是否返回 `tool_calls`：当请求包含 tools 且最后一条消息不是 `role=tool` 时返回工具调用；当最后一条消息是工具结果时返回普通 stop 响应。

禁止用全局请求奇偶数决定返回类型。长期记忆自动抽取、重试或其它后台模型调用会共享同一个 mock 服务并消耗请求序号，导致恢复后的第二个用户 turn 错误拿到 stop 响应，CI 中表现为 `im_gateway_agent_tool_history_resume_regression` 未执行恢复后的工具调用。

### 覆盖场景

该设计覆盖：

- 正常 tool call loop
- retry 后继续 loop
- manual `/compact`
- auto/mid-turn compaction
- `/resume`
- session persistence + history reload
- 多 tool-call pending 队列恢复
- `switch_workdir` 后 clear
- `/undo` / clear / reset 后续请求
- MCP tool 与本地 tool 共用同一 ChatMessage invariant
- 多轮对话后的 max history 裁剪

## `/status` 运行中可观测指标

### 背景

旧实现中，Agent turn 执行时 session 会从 `AgentSessionManager.sessions` 中取出，`/status` 无法读取真实 session，只能返回“Agent 正在处理中”。这会让用户在长工具循环、长模型请求或自动压缩期间无法判断任务是否仍在推进，也看不到 token 与 context 的消耗趋势。

### 方案

`AgentSessionManager` 在 `take_session*` / `try_take_session*` 成功时创建 `ActiveTurnStatus` 共享快照，并把同一个 handle 注入到被取出的 `AgentSession.active_turn_status`。执行中的 turn loop 不需要重新持有 manager，只在关键阶段更新 session 内的 handle；manager 通过 `get_active_turn_status(session_key)` 暴露只读 clone。

快照字段：

- `current_loop_iteration`：当前正在执行的 Agent loop 序号，从 1 开始。
- `completed_loop_iterations`：已收到模型响应并完成 accounting 的 loop 次数。
- `max_loop_iterations`：本次 turn 的迭代上限。
- `last_response_tokens` / `total_tokens_used`：最近一次 API 响应 token 与 session 级 API 累计 token，包含 compaction 模型调用。
- `estimated_context_tokens` / `context_window_tokens` / `context_usage_percent`：基于当前 history 的粗略 token 估算、配置中的 context window 和占比；未显式配置时默认 context window 为 250,000 tokens。
- `compaction_count`：当前 session 累计压缩次数。
- `work_dir`：当前 session 工作路径；用于确认 Agent 实际在哪个项目上下文中执行。
- `message_count` / `history_version` / `local_tool_count` / `mcp_tool_count`：辅助定位当前上下文与工具规模。

更新时机：

1. turn 开始后立即写入 `starting` 快照。
2. 每次构造 messages 并发起模型请求前写入 `model_request`，此时可看到当前 loop。
3. 每次模型响应后写入 `model_response`，同步最新 token usage。
4. 进入工具调用批次和每个工具结果入 history 后写入 `tool_calls`，同步 context 估算增长。
5. 自动或手动压缩成功后由已有 session 字段反映 `compaction_count` 与 token 累计。

### 接入面

- API `POST /_bifrost/api/im-gateway/agent/chat`：当同 session 忙碌且请求消息为 `/status` 时，不再返回通用忙碌提示，而是返回 `response` 文本与结构化 `active_status`。
- IM guide/queue 忙碌路径：`/status` 优先展示 `ActiveTurnStatus`，并附加当前排队消息数量。
- 空闲 `/status` 保持原有会话状态输出，同时补充 `工作路径` 与 `Context 用量` 字段。

### 测试方案

- 单元测试：验证 `AgentConfig::default()` 的 `model_context_window` 为 250,000，默认 auto-compact threshold 为 225,000；验证 context 占比计算、运行中 status 文本包含 loop、实时 token、Context 用量和压缩次数。
- E2E 测试：使用真实 Bifrost + mock Chat Completions 服务，构造一次阻塞模型请求；同 session 并发发送 `/status`，不在 PATCH 中显式配置 `model_context_window`，断言返回运行中指标和结构化 `active_status.context_window_tokens == 250000`，不再只是通用忙碌提示。
- 真实场景测试：更新 `human_tests/agent-builtin-commands.md` 的 `/status` 运行中指标用例，增加默认 context window 250,000 的断言；按文档使用临时数据目录、`--no-system-proxy` 和真实 API 请求逐条执行关键用例。

### 校验要求

- `cargo test -p bifrost-agent session::tests::test_active_turn_status`
- `bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

## 关键组件

### 1. ImAgentConfig - 全局配置

```rust
pub struct ImAgentConfig {
    /// LLM API 基础 URL
    pub base_url: String,
    /// API 密钥（支持 $ENV_VAR 环境变量引用）
    pub api_key: String,
    /// 模型名称
    pub model: String,
    /// 最大生成 token 数
    pub max_completion_tokens: u32,
    /// 推理强度（low/medium/high）
    pub reasoning_effort: Option<String>,
    /// 推理摘要模式（auto/concise/detailed）
    pub reasoning_summary: Option<String>,
    /// 会话 TTL（秒）
    pub session_ttl_secs: u64,
    /// 单会话最大历史消息数
    pub max_history: usize,
    /// 是否使用 Azure 认证方式
    pub by_azure: bool,
    /// 是否启用 Agent 功能
    pub enabled: bool,
}
```

**配置持久化**：通过 `ImAgentConfigStore` 存储为 JSON 文件（`im_agent_config.json`），支持热更新。

### 1.1 Provider 级 Agent 基础配置覆盖

IM Provider 支持可选 `agent_config`，用于给不同 IM 通道绑定不同的 Agent 基础运行上下文：

```json
{
  "agent_config": {
    "work_dir": "/Users/eden/work/github/bifrost",
    "base_instructions": "Provider-specific base system prompt",
    "developer_instructions": "Provider-specific developer policy",
    "user_instructions": "Provider-specific AGENTS-style user notes"
  }
}
```

字段语义：

- `work_dir`：Provider 默认工作目录。来自该 Provider 的新 Agent session 会以该目录初始化；未配置时回退到全局 Agent `work_dir`。
- `base_instructions`：Codex-style base/system instructions。配置后覆盖内置默认 Agent prompt；旧字段 `instructions` / `default_system_prompt` 仅作为兼容别名写入该字段。
- `developer_instructions`：Codex-style developer instructions。不会覆盖 base prompt，而是作为独立 `<developer_instructions>` section 追加到模型可见系统上下文。
- `user_instructions`：Codex-style user/AGENTS instructions。会与全局 home AGENTS.md、项目 AGENTS.md 合并后放入 `<user_instructions>`；不会再复用 `base_instructions`，避免同一 prompt 重复注入。

Base/system instructions 优先级：

1. Route `AgentChat.system_prompt`
2. Provider `agent_config.base_instructions`（兼容 `agent_config.instructions`）
3. 全局 Agent `base_instructions`（兼容全局 `instructions` / `default_system_prompt`）
4. 内置默认 Agent prompt

Developer/user instructions 优先级：

- Provider `agent_config.developer_instructions` / `agent_config.user_instructions` 非空时覆盖同名全局字段。
- 全局 `developer_instructions` / `user_instructions` 为空时对应 section 不注入。
- AGENTS.md 始终按最终 `work_dir` 发现并追加到 user instructions。

工作目录优先级：

1. 已存在且仍有历史上下文的 session 自己的 `work_dir`
2. Provider `agent_config.work_dir`（包括 IM 对话中通过 `switch_workdir` 成功切换后回写的值）
3. 全局 Agent `work_dir`
4. 进程当前目录

动态修改：

- `PATCH /_bifrost/api/im-gateway/providers/{id}` 支持热更新 `agent_config`，无需重启 Bifrost 或重新连接 Provider。
- 空字符串或 `null` 会清除对应字段；`agent_config: null` 会清除整个 Provider 级覆盖。
- WebUI Edit Provider 保存时必须对被清空的单字段发送 `null`，不能省略字段；省略字段表示“保持当前 Provider 覆盖值不变”。
- Instructions 在后续 turn 进入 Agent 时按最新 Provider 配置合成；已有且仍有历史上下文的 session 的显式工作目录保持不变，避免运行中任务被静默切换目录。
- `/clear` 或 `/reset` 后的空 session 会重新按当前 Provider `agent_config.work_dir` 初始化，确保用户在 WebUI 修改 Provider 配置后重开 IM 对话立即生效。
- Agent 初始化必须从最终 `work_dir` 创建 session，使 AGENTS.md 与 repo-local skills 都从该目录加载。
- Agent 通过 `switch_workdir` 明确切换目录时，运行时会清空旧会话、重新挂载 skills/AGENTS.md 上下文，将最新目录持久化到当前 Provider `agent_config.work_dir`，并在 IM 回复中通知最新工作路径。
- IM 长连接事件循环每次处理消息时从 Provider store 重新读取最新 Provider 配置，避免连接启动时的旧 provider snapshot 导致 WebUI 修改后不生效。

WebUI：

- Settings → Agent 提供 Base Instructions、Developer Instructions、User Instructions 三个明确入口。
- Settings → Agent 的三段 instruction 不做行内 textarea 编辑；页面只展示短预览与 Edit 按钮，点击后在大尺寸弹窗中编辑长文本，保存时采用本地草稿优先：自动保存响应返回时不能覆盖用户仍在编辑的最新输入；清空内容会 PATCH 空字符串并清除覆盖值。
- Base Instructions / System Prompt 为空并继承默认值时，编辑弹窗必须提供将默认值复制到编辑草稿的按钮，支持用户以默认 prompt 为基础继续修改。
- Settings → Agent 不再单独展示 `Default Base Instructions (read-only)` 块；默认 Base Prompt 只作为 Base Instructions 编辑弹窗中的可复制草稿来源出现。
- Settings → Agent 左侧提供二级卡片导航，覆盖 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions；点击导航项只在右侧独立渲染当前编辑卡片，并用 `aria-current` 标记当前卡片。
- Agent 设置页导航必须使用 URL 查询参数 `agentSection` 记录当前二级卡片，刷新或复制链接后恢复到同一卡片；进入 Session 详情时继续使用现有 `session/view/historyPath` 参数。
- Agent 设置页导航必须使用主题 token / CSS 变量兼容亮色与暗色主题；桌面端左侧导航固定在自身列，只有右侧当前卡片内容区允许滚动，窄屏退化为顶部横向滚动导航，不遮挡编辑卡片内容。
- Settings → IM Gateway → Add/Edit IM Provider 支持手动填写 Agent Working Directory、Base Instructions、Developer Instructions、User Instructions。
- Settings → IM Gateway → Add/Edit IM Provider 的三段 Provider 级 instruction 同样使用短预览 + Edit 按钮 + 大尺寸弹窗编辑，避免在 Provider 表单里嵌入大段 textarea。
- Provider 级 Base Instructions 继承全局默认值时，编辑弹窗必须提供将继承值复制到编辑草稿的按钮，支持按 Provider 定制后保存覆盖值。
- Provider 卡片展示当前 Provider 是否配置了 Agent Work Dir / Base / Developer / User instructions。
- Provider 卡片展示连接状态、连接配置摘要、Owner、启用状态和 Agent 基础配置摘要。
- Provider 卡片提供 Edit 入口，可动态修改非连接配置（Display Name、Enabled、Owner Open ID、Agent Working Directory、Base/Developer/User Instructions）。
- Add/Edit Provider 表单会展示数据目录默认 Agent `work_dir` 与三层 instructions 作为继承值；字段留空表示继承默认值，用户填写后才在单个 Provider 上形成覆盖。
- Edit 入口只读展示 Provider ID、Type、App ID、Secret 状态和连接模式；连接凭据与连接模式只能在 Add IM Provider 创建时填写，避免误改已经建立的 IM 连接。

### 2. ImAgentConfigStore - 配置存储

- 文件路径：`{data_dir}/im_agent_config.json`
- 支持环境变量替换（`$MODELHUB_AK` → 实际值）
- 提供 `load()` / `save()` / `get_resolved_api_key()` 方法

### 3. ImAgentClient - HTTP 客户端

- 调用端点：`{base_url}/chat/completions`
- 认证方式：
  - Azure 模式：`api-key: {api_key}` header
  - 标准模式：`Authorization: Bearer {api_key}` header
- 非流式请求（`stream: false`）

### 4. ImAgentSessionManager - 会话管理器

```rust
pub struct ImAgentSessionManager {
    /// 用户会话映射（open_id → Session）
    sessions: DashMap<String, Session>,
    /// 会话 TTL
    ttl: Duration,
    /// 最大历史消息数
    max_history: usize,
}

struct Session {
    /// 消息历史
    messages: Vec<ChatMessage>,
    /// 最后活跃时间
    last_active: Instant,
}
```

- 基于 DashMap 实现线程安全的 per-user 会话隔离
- 支持 TTL 过期清理（默认 1 小时）
- 支持内置命令：`/clear`、`/reset`

### 5. run_turn() / run_turn_with_mcp() - 主入口函数

**处理流程**：
1. 查找或创建用户会话
2. 构建消息列表（历史 + 当前消息）
3. 调用模型 API（run_turn_with_mcp 额外支持 MCP 工具调用）
4. 记录对话轮次到会话历史
5. 返回模型响应（TurnResult）

## 默认模型配置

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| Provider | aidp_crawl | 字节跳动内部模型服务 |
| BaseURL | `https://search.bytedance.net/gpt/openapi/online/multimodal/crawl` | API 端点 |
| API Key | `$MODELHUB_AK` | 从环境变量读取 |
| Model | `gpt-5.4-2026-03-05` | 模型版本 |
| ReasoningEffort | `medium` | 推理强度 |
| ReasoningSummary | `auto` | 推理摘要模式 |
| MaxCompletionTokens | `16384` | 最大生成 token |
| ByAzure | `true` | 使用 `api-key` header 认证 |
| SessionTTL | `3600` (1小时) | 会话过期时间 |
| MaxHistory | `20` | 最大历史消息数 |

## 事件流程

```
1. Feishu WebSocket 接收消息
          ↓
2. 转换为 ImEvent
          ↓
3. Owner 安全检查（仅允许配置的 owner_id）
          ↓
4. 存储事件 + 添加 "OK" reaction
          ↓
5. ImEventRouter::match_routes() 路由匹配
          ↓
    ┌─────────────┴─────────────┐
    ▼                           ▼
6a. 匹配到 AgentChat 路由  6b. 无匹配 && agent.enabled
    → process_agent_chat()      → handle_agent_chat()
    └─────────────┬─────────────┘
                  ↓
7. 调用模型 API 获取响应
                  ↓
8. send_text() 发送回复到飞书
                  ↓
9. 记录出站消息日志
```

### 详细步骤说明

**步骤 1-3**：WebSocket 接收与安全校验
- 通过 `ImGatewayService` 接收飞书 WebSocket 消息
- 转换为统一的 `ImEvent` 结构
- 校验发送者是否在 `owner_ids` 白名单中

**步骤 4**：事件持久化与反馈
- 将原始事件存储到 SQLite（用于审计和调试）
- 添加 ✓ Reaction 告知用户消息已收到

**步骤 5**：路由匹配
- 调用 `ImEventRouter::match_routes()` 检查是否匹配任何规则
- 支持多种路由类型（关键字、正则、IM 类型等）

**步骤 6**：Agent 处理
- **匹配到 AgentChat 路由**：使用路由级别的 `system_prompt` 和 `model` 覆盖
- **无匹配且 agent 启用**：使用全局配置进行默认对话处理

**步骤 7-9**：模型调用与响应
- 构建请求（包含历史上下文）
- 调用 Chat Completions API
- 发送文本消息到飞书
- 记录完整的交互日志

## 路由集成

### 新增路由动作类型

```rust
pub enum ImRouteAction {
    // ... 已有变体 ...
    
    /// Agent 对话处理
    AgentChat {
        /// 可选的系统提示词覆盖
        system_prompt: Option<String>,
        /// 可选的模型名称覆盖
        model: Option<String>,
    },
}
```

### 路由匹配逻辑

1. **显式 AgentChat 路由**：
   - 用户配置特定触发条件（如关键字 "AI"、"助手"）
   - 可指定自定义 system_prompt 和 model
   - 适用于特定场景的专用 Agent

2. **默认 Agent 兜底**：
   - 当所有路由规则都不匹配时
   - 检查 `ImAgentConfig.enabled`
   - 如果启用，则作为默认对话处理器
   - 适用于通用对话场景

### 配置示例

```json
{
  "routes": [
    {
      "name": "技术问答助手",
      "matchers": [
        { "type": "keyword", "pattern": "技术" }
      ],
      "action": {
        "type": "AgentChat",
        "system_prompt": "你是一个技术专家，专门回答编程和技术架构问题。",
        "model": "gpt-5.4-2026-03-05"
      }
    }
  ],
  "agent": {
    "enabled": true,
    "base_url": "https://search.bytedance.net/gpt/openapi/online/multimodal/crawl",
    "api_key": "$MODELHUB_AK",
    "model": "gpt-5.4-2026-03-05"
  }
}
```

## 管理 API

### GET /api/im-gateway/agent

获取当前 Agent 配置。

**响应示例**：
```json
{
  "enabled": true,
  "base_url": "https://search.bytedance.net/gpt/openapi/online/multimodal/crawl",
  "api_key": "$MODELHUB_AK",
  "model": "gpt-5.4-2026-03-05",
  "max_completion_tokens": 16384,
  "reasoning_effort": "medium",
  "reasoning_summary": "auto",
  "session_ttl_secs": 3600,
  "max_history": 20,
  "by_azure": true
}
```

### PATCH /api/im-gateway/agent

更新 Agent 配置（部分更新）。

**请求示例**：
```json
{
  "enabled": true,
  "model": "gpt-5.5-2026-04-01",
  "max_completion_tokens": 32768
}
```

**行为**：
- 合并现有配置
- 支持热更新（无需重启服务）
- 持久化到 `im_agent_config.json`

### GET /api/im-gateway/agent/sessions

列出当前活跃的会话列表。

**响应示例**：
```json
{
  "sessions": [
    {
      "open_id": "ou_xxxxx",
      "message_count": 5,
      "last_active": "2026-05-01T10:30:00Z",
      "created_at": "2026-05-01T10:00:00Z"
    }
  ],
  "total": 1
}
```

## 会话管理

### 设计原则

1. **Per-User 隔离**：每个 `open_id` 独立会话，互不干扰
2. **内存存储**：不持久化会话，重启后清空（适合对话场景）
3. **TTL 过期**：默认 1 小时无活动自动清理
4. **历史限制**：默认保留最近 20 条消息，避免上下文过长

### 会话结构

```rust
struct Session {
    /// 对话历史（包含 user 和 assistant 消息）
    messages: Vec<ChatMessage>,
    /// 最后活跃时间（用于 TTL 检查）
    last_active: Instant,
    /// 会话创建时间
    created_at: Instant,
}

struct ChatMessage {
    role: String,  // "user" | "assistant" | "system"
    content: String,
}
```

### 内置命令

| 命令 | 功能 | 实现 |
|------|------|------|
| `/clear` | 清空当前会话历史 | `session.messages.clear()` |
| `/reset` | 重置会话（等同 /clear） | `session.messages.clear()` |

**命令处理流程**：
1. 检测消息是否以 `/clear` 或 `/reset` 开头
2. 执行清空操作
3. 返回确认消息（不调用模型）

### 清理机制

**惰性清理**：
- 每次访问会话时检查 `last_active`
- 如果超过 TTL，删除会话

**主动清理**（可选）：
- 后台任务定期扫描过期会话
- 避免长时间无访问导致的内存泄漏

## 设计决策

### 1. 为什么使用 Chat Completions API

**选择 Chat Completions API 而非 Responses API 的原因**：

- **简单性**：Chat Completions API 接口更简洁，适合 IM 单轮对话场景
- **兼容性**：Azure/OpenAI 广泛支持，迁移成本低
- **流式支持**：虽然当前未启用，但 Chat API 原生支持 SSE 流式
- **成熟度**：文档完善，社区实践丰富

### 2. 为什么使用非流式模式

**选择 `stream: false` 的原因**：

- **飞书限制**：飞书消息 API 不支持流式编辑
- **用户体验**：单次发送完整消息比多次编辑更稳定
- **错误处理**：非流式模式下错误处理更简单
- **性能可控**：避免长时间占用连接

### 3. 为什么使用 Azure 认证方式

**选择 `api-key` header 而非 `Authorization: Bearer` 的原因**：

- **MODELHUB 要求**：字节跳动内部模型服务使用 Azure 认证格式
- **兼容性**：`by_azure: true` 可配置，同时支持标准 OpenAI API
- **安全性**：避免 Bearer token 被误用

### 4. 为什么会话不持久化

**选择内存存储的原因**：

- **隐私性**：对话历史敏感，不落盘更安全
- **时效性**：对话上下文有时效性，持久化意义不大
- **简单性**：避免引入数据库依赖和迁移逻辑
- **重启清空**：服务重启后从新对话开始，符合用户预期

### 5. 为什么限制历史消息数

**限制 `max_history` 的原因**：

- **成本控制**：减少 API token 消耗
- **性能优化**：避免请求体过大
- **上下文聚焦**：保留最近对话更相关
- **模型限制**：避免超出模型上下文窗口

## 文件结构

```
crates/bifrost-admin/
├── src/
│   ├── im_gateway/
│   │   ├── agent.rs          # Agent 类型 re-exports (from bifrost_agent)
│   │   │   ├── ImAgentConfig
│   │   │   ├── ImAgentConfigStore
│   │   │   ├── ImAgentClient
│   │   │   ├── ImAgentSessionManager
│   │   │   ├── run_turn()
│   │   │   └── run_turn_with_mcp()
│   │   ├── types.rs          # ImRouteAction::AgentChat 变体
│   │   └── mod.rs
│   └── handlers/
│       └── im_gateway.rs     # HTTP Handler 统一入口
│           └── handle_im_gateway()
```

## 依赖项

### 外部依赖

```toml
[dependencies]
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dashmap = "5.5"
tokio = { version = "1", features = ["time"] }
tracing = "0.1"
```

### 内部依赖

- `bifrost-core`：基础工具、错误处理
- `bifrost-feishu`：飞书 API 客户端（send_text）
- `bifrost-protocol`：IM 事件类型定义

## 测试方案

### 单元测试

| 测试项 | 测试内容 |
|--------|----------|
| `test_im_agent_config_env_var` | 验证 `$ENV_VAR` 环境变量替换 |
| `test_session_manager_ttl` | 验证会话 TTL 过期清理 |
| `test_session_manager_max_history` | 验证历史消息数限制 |
| `test_agent_client_request_build` | 验证 HTTP 请求构建正确性 |
| `test_agent_client_azure_auth` | 验证 Azure 认证 header |
| `test_builtin_commands` | 验证 /clear、/reset 命令 |

### E2E 测试

| 测试场景 | 验证点 |
|----------|--------|
| Agent 默认对话 | 发送消息 → 收到模型回复 |
| AgentChat 路由 | 触发关键字 → 使用自定义 system_prompt |
| 会话持久性 | 多轮对话 → 上下文保持 |
| 会话清空 | /clear 命令 → 历史清空 |
| 配置热更新 | PATCH 配置 → 立即生效 |
| E2E 启动器服务注入回归 | `ProxyInstance::start_with_admin` 启动后 `/api/im-gateway/agent` 与 `/api/im-gateway/routes` 返回 200，确保测试启动路径与真实 CLI 一样注入 `ImGatewayService` |
| Agent tool history 恢复回归 | `im_gateway_agent_tool_history_resume_regression` 在长期记忆后台调用存在时仍完成首次工具调用、JSONL 恢复和恢复后再次工具调用 |
| Chat API 长期记忆真实链路 | 运行 `e2e-tests/tests/test_long_term_memory_human_api.sh`，验证 `POST /_bifrost/api/im-gateway/agent/chat` 在真实 Bifrost + mock Chat Completions 下触发自动记忆、Phase 2 consolidation、跨独立 session 消费 |
| Chat API runtime gate 回归 | 运行 `e2e-tests/tests/test_update_plan_human_api.sh`，验证 `/agent/chat` 路径下 update_plan runtime 收口提醒仍会强制模型在结束前补齐最终 plan 状态 |
| Chat API runtime limits 回归 | 运行 `e2e-tests/tests/test_agent_loop_runtime_limits.sh`，验证默认 1000 次 turn 上限与 600 秒超时配置在 `/agent/chat` 黑盒链路中生效 |
| Chat API 引导/排队注入回归 | 通过 `/api/im-gateway/agent/chat` 的测试专用字段 `guide_message` / `queue_messages`，验证 turn-end guide drain、queued FIFO drain、guide 优先于 queue，以及空白注入被忽略 |
| Agent 模型请求默认代理回归 | `im_gateway_agent_model_request_uses_bifrost_proxy` 使用 `AgentClient::new_with_bifrost_proxy(port)` 调用 mock Chat Completions，断言请求经当前 Bifrost 端口转发并在 `/api/traffic` 中出现可查询记录 |
| WebUI instruction 大窗口编辑回归 | `Settings Agent 三层 instructions 使用大窗口编辑` 验证全局 Agent instruction 页面无行内 textarea、点击 Edit 打开大弹窗并 PATCH；`Settings IM Provider instructions 使用大窗口编辑后保存覆盖值` 验证 Provider Edit 弹窗中 instruction 通过嵌套大弹窗编辑并保存到 `agent_config` |
| Provider agent_config 进入 IM 事件链路 | `im_event_loop_uses_provider_agent_config_for_agent_chat` 创建带 Provider 级 base/developer/user instructions 的新 Provider，注入 IM inbound event，断言 Chat Completions 请求使用 Provider 配置且不泄漏全局 fallback marker |

### 真实场景测试（human_tests）

**测试用例文档**：`human_tests/im-gateway-agent.md`、`human_tests/im-guide-queue-mode.md`、`human_tests/long-term-memory.md`

| 用例编号 | 用例名称 | 验证点 |
|----------|----------|--------|
| TC-AG-01 | 基础对话 | 飞书发送消息 → 收到回复 |
| TC-AG-02 | 多轮对话 | 连续对话 → 上下文关联 |
| TC-AG-03 | 会话清空 | /clear → 历史清空 |
| TC-AG-04 | 路由覆盖 | 触发 AgentChat 路由 → 使用自定义配置 |
| TC-AG-05 | 非_OWNER_拦截 | 非 owner 用户 → 无响应 |
| TC-AG-06 | 配置更新 | 通过 API 更新配置 → 生效 |
| TC-IMA-66 | CI E2E 启动器服务注入回归 | 运行 `bifrost-e2e --test im_gateway_agent`，验证新增 Agent API 用例不再返回 503 |
| TC-IMA-67 | Agent Loop tool message 序列回归 | 运行 `im_gateway_agent_tool_history_resume_regression`，验证恢复后的 turn 仍会执行工具调用 |
| TC-GQ-04 | turn-end guide drain 黑盒回归 | 通过 `/agent/chat` 注入 `guide_message`，验证模型 stop 后到达的 guide 不会丢失，而是继续同一 turn loop |
| TC-GQ-05 | queued FIFO drain 黑盒回归 | 通过 `/agent/chat` 注入 `queue_messages`，验证在同一次 `run_turn_with_mcp` 中按 FIFO 逐条继续处理 |
| TC-GQ-06 | guide 优先于 queue | 同时注入 `guide_message` 与 `queue_messages`，验证处理顺序为 initial → guide → queued FIFO |
| TC-LTM-09 | 长期记忆真实对话链路 | 真实 Bifrost + mock Chat API 环境下验证自动记忆、Phase 2 consolidation、跨 session 消费 |
| TC-IMA-83 | Agent 模型请求默认进入 Traffic | 真实 Bifrost 监听端口启动后，Agent 底层 Chat Completions 请求默认经 `http://127.0.0.1:<port>` 代理发出；mock 模型 host 可查询到 POST 记录，真实模型域名在 `--intercept-include` 下可解包为 HTTPS POST 明文记录 |
| TC-IMA-84 | Agent 设置页卡片导航 | Settings → Agent 左侧导航可见，点击 MCP Servers / Runtime 只渲染对应编辑卡片，URL `agentSection` 可刷新恢复，亮色与暗色主题下当前项高亮可读 |
| TC-IMA-53A | 新建 IM Provider 的 agent_config 经 IM 事件链路生效 | Provider 创建时配置 base/developer/user/work_dir 后，IM inbound event 进入 `run_event_loop` 时模型请求使用 Provider 级配置而非全局 fallback |

## Agent 模型请求代理

IM Gateway 内嵌 Agent 默认通过当前启动的 Bifrost HTTP 代理访问模型提供方：真实 CLI 启动和 E2E `ProxyInstance::start_with_admin` 都使用 `ImGatewayService::new_with_agent_proxy_port(data_dir, Some(port))` 创建服务，底层 `AgentClient::new_with_bifrost_proxy_and_ca(port, data_dir/certs/ca.crt)` 会把 Chat Completions 请求代理到 `http://127.0.0.1:<port>`，并只把当前 Bifrost CA 加入 Agent 自己的 reqwest trust store。这样模型请求、响应、状态码和耗时会落入现有 Traffic 记录；对模型域名启用 TLS intercept 时，Agent 不会因为 Bifrost 签发的拦截证书报 `UnknownIssuer`。

库级直连调用仍保留 `AgentClient::new()`，用于纯单元测试和不在 Bifrost 服务内运行的场景。需要临时绕过默认代理时，可设置 `BIFROST_AGENT_DISABLE_MODEL_PROXY=1`，服务会回退为直连模型请求。

## 扩展性考虑

### 未来可能的功能扩展

1. **流式响应**：
   - 飞书支持消息编辑后可实现
   - 需要改用 SSE 流式读取

2. **多模型支持**：
   - 通过路由级别 `model` 字段已支持
   - 可扩展为模型池配置

3. **工具调用**：
   - Chat Completions API 支持 function calling
   - 可扩展为 Agent 工具调用能力

4. **会话持久化**：
   - 如有需求可扩展为 SQLite 存储
   - 支持跨重启会话恢复

5. **上下文压缩**：
   - 当历史消息过长时自动摘要
   - 减少 token 消耗

## 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 模型 API 故障 | 用户无响应 | 超时机制 + 错误提示消息 |
| 上下文过长 | Token 消耗大 | max_history 限制 |
| 敏感信息泄露 | 隐私问题 | 会话不持久化 + TTL 清理 |
| 非 owner 滥用 | 成本失控 | owner_ids 白名单校验 |
| 并发请求过多 | API 限流 | 请求队列 + 限流机制 |

## 参考资料

- [OpenAI Chat Completions API](https://platform.openai.com/docs/api-reference/chat)
- [Azure OpenAI Authentication](https://learn.microsoft.com/en-us/azure/ai-services/openai/reference)
- [Feishu Message API](https://open.feishu.cn/document/server-docs/im-v1/messages/create)
