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

### 2026-05-10 重试路径补洞

2026-05-10 在默认数据目录 `~/.bifrost/body_cache/REQ-69ffe7f1-007442_req`、`007505_req`、`007511_req` 中再次发现 400：

```text
Invalid parameter: messages with role 'tool' must be a response to a preceeding message with 'tool_calls'.
```

这批请求的直接证据表明：请求体 `messages[1]` 就是孤儿 `role=tool`，其前面没有 `assistant.tool_calls`。同时对应 session JSONL 中当前 turn 的 `tool_call` / `tool_result` 落盘是完整的，因此本次并不是 `load_conversation()` 老恢复链路回归，而是**turn 重试路径复用了首次失败请求前构造的旧 `messages` 快照**。

补丁原则：

- `session.rs` 在 transient retry 前重新执行 `session.build_messages(...)`，确保重试请求基于最新 history 重新裁剪并重新 sanitize，而不是复用失败前的旧快照。
- `client.rs` 在真正发送 Chat Completions 请求前再做一次 `sanitize_chat_history()` 兜底，防止未来新增调用点绕过上层 invariant。
- client 边界兜底会记录 `sanitized malformed chat history at client request boundary` warn，便于以后从日志直接定位是否有分支再次泄漏 orphan `tool`。
- client 边界必须有真实 HTTP 请求体级别回归，证明即使调用方直接传入 `system -> tool -> user`，最终发出的请求也会先移除孤儿 `tool`。

这样可以同时覆盖：

- 首次请求前 history 已被污染但上层遗漏 sanitize；
- 首次请求失败后 history 发生变化，重试若继续复用旧快照会带入过期非法片段；
- 未来新增聊天请求路径直接调用 client 而没有经过 `build_messages()`。

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

## Agent Chat JSONL 恢复与续聊

### 背景

AI Agent Chat 页面需要从 Sessions/History 列表打开历史 JSONL，并在同一个对话上下文中继续运行。历史文件路径来自 WebUI query 或 API 请求体，不能直接作为文件系统路径使用，否则会出现任意文件读取/删除风险；同时 JSONL 末尾可能因进程退出留下半行坏 JSON，恢复流程不能因此丢弃整份有效历史。

### 方案

- `persistence::validate_conversation_path()` 对调用方传入的历史路径做 canonicalize，只允许访问当前 Agent data dir 的 `sessions/` 子目录内 `.jsonl` 文件，并复用 64 MiB 大小保护。
- `persistence::load_conversation_lossy()` 用于 restore/continue 场景：跳过无法解析的 JSONL 行，保留可恢复的 user/assistant/tool-call 合法历史，并返回 `skipped_lines` 供日志告警。
- `/_bifrost/api/im-gateway/agent/sessions/history/*` 的 GET/DELETE 先走安全路径校验，再读取或删除文件，避免越权路径。
- `/_bifrost/api/im-gateway/agent/chat` 与 `/_bifrost/api/agent/chat/stream` 接受可选 `history_path`。当 session 当前为空时，后端先校验路径、校验 JSONL 内 `session_key` 与请求 `session_key` 一致，再恢复 history/runtime summary。
- 恢复成功后通过 `ConversationRecorder::from_existing_file()` 继续写回原 JSONL，让后续再次打开同一个 historyPath 时可以看到续聊内容。
- Agent Chat WebUI 在 URL 包含 `historyPath` 时先加载并渲染历史消息，发送时把 `history_path` 传给流式 API；首次续聊成功后本地清除 pending `historyPath`，避免同一运行态重复恢复。

## Agent Chat 刷新恢复与线程列表选中

### 背景

AI Agent Chat 页支持 `session` 深链，例如 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=<key>&view=active`。旧实现只有 URL 携带 `historyPath` 时才读取 JSONL 历史；普通 active session 深链刷新后会保留 session key，但消息区回到 starter messages，用户刚发出的消息和回复看起来像丢失。同时 Recent Threads 只按 `session_key` 判断选中态，同一 session 同时出现 active 与 ended 记录时会多条高亮；线程列表还被截断到 8 条，列表区域本身没有独立滚动容器。

### 方案

- WebUI 在 `session` 存在且 `historyPath` 不存在时调用 `GET /_bifrost/api/im-gateway/agent/sessions/{session}`，从 active in-memory session detail 恢复 user/assistant 消息。
- 如果当前 URL 只有 `session`，但 sessions/all 返回同 session 的 ended history 记录，则自动补齐 `view=history&historyPath=<jsonl>`，继续复用现有安全 history loader。
- 对话卡片标题展示当前会话标题：优先使用运行中 `set_title`/线程标题，其次使用第一条用户消息摘要，最后才回退到 session key。
- Recent Threads 不再截断到 8 条；线程卡片内部增加独立可滚动列表，避免右侧其他状态卡片把线程项挤出可视区。
- 选中态按当前视图区分：history 视图匹配 `history_path`，active 视图只匹配无 `history_path` 的 active 记录，避免同一 `session_key` 多条记录同时高亮。
- 发送完成后 URL 保留当前 `session` 和 `view=active`，刷新时能重新进入同一 active session。
- 切换线程、active session detail 恢复、JSONL history 恢复后的下一次消息区滚动使用 `behavior: "auto"`，直接展示底部；普通发送和流式追加仍可使用 smooth 跟随。

### 测试方案

- 单元/类型检查：执行 `pnpm --dir web exec tsc --noEmit`，覆盖新增类型与 helper 的 TypeScript 约束。
- E2E/UI：在 `web/tests/ui/agent-chat.spec.ts` 中补充 session detail 恢复、对话标题展示、恢复后即时滚到底部、historyPath 自动补齐、线程列表滚动和唯一选中态测试。
- 真实场景测试：在 `human_tests/im-gateway-agent.md` 增加 Agent Chat 刷新恢复回归用例，使用真实浏览器打开 active session 深链，确认刷新后消息保留、对话标题使用线程/消息标题、恢复后即时展示底部、线程列表可滚动、同 key active/history 不重复选中。

### Review/Fix/Test 闭环

- 第 1 轮：复核用户目标、检查 `AgentChatSection.tsx` diff、执行 TypeScript 与新增 UI 测试，修复发现的加载/选中/滚动问题。
- 第 2 轮：重新检查 URL 参数、history/active 双路径、human_tests 索引与真实浏览器表现，复跑受影响 UI 测试和项目校验命令。

### 验证

- 单元测试覆盖越权路径拒绝、坏 JSONL 行容错恢复、existing-file recorder 续写。
- UI 测试覆盖 Agent Chat 流式 API 发送，以及从 `historyPath` 渲染历史后携带 `history_path` 续聊。
- human_tests 新增 TC-IMA-116，要求真实验证合法 historyPath 恢复、续聊写回和外部路径 400。

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

## 多模态图片理解链路

### 目标

IM Gateway Agent 支持把飞书 IM 中的图片和文本一起传给模型；`POST /_bifrost/api/im-gateway/agent/chat` 也支持同一套图片输入，便于自动化和人工端到端验证。模型配置仍走现有 Agent 配置，API key 按 provider 的默认 `env_key` / 环境变量解析，不在请求里硬编码。

### 实现逻辑

1. 飞书长连接收到 `im.message.receive_v1` 后，`normalize_feishu_event()` 解析 `message.content`：
   - `message_type=image` 时读取 `image_key`
   - 富文本内容中递归收集 `image_key`
   - 统一写入 `ImEventMessage.images`
2. 进入 Agent 前，IM handler 按飞书“获取消息中的资源文件”接口下载图片：
   - `GET /im/v1/messages/:message_id/resources/:file_key?type=image`
   - 使用当前 provider 的 `tenant_access_token`
   - 从响应头 `Content-Type` 获取 MIME，二进制内容 base64 编码
   - 单条消息默认最多传入 6 张图片；超过 6 张时记录 warn，并截断为前 6 张，避免请求体和会话记录过大。
3. Agent runtime 使用 OpenAI-compatible Chat Completions content parts：
   - 文本 part：`{"type":"text","text":"..."}`
   - 图片 part：`{"type":"image_url","image_url":{"url":"data:image/png;base64,...","detail":"auto"}}`
4. 用户图片随会话事件落盘：
   - JSONL `user_message.content.images` 保存 `{mime_type,data}`，其中 `data` 为 base64 或 data URL。
   - `load_conversation()` 恢复历史时重建多模态 `ChatMessage`。
   - active session detail API 返回 `messages[].content_parts`，history event API 返回原始 `content.images`，WebUI Session 详情据此渲染图片缩略图，点击缩略图后可放大预览。
5. `/agent/chat` 新增请求字段：

```json
{
  "message": "请描述这张图片",
  "images": [
    {
      "mime_type": "image/png",
      "data": "<base64 image bytes 或 data URL>"
    }
  ]
}
```

纯文本消息继续按字符串 `content` 序列化；只有包含图片时才切换为 content parts，保持历史记录、记忆、内置命令和纯文本模型兼容。

### 失败降级

- 图片资源下载失败时记录 warn，继续把文本消息传给 Agent，不阻塞整条 IM 会话。
- 事件缺少 `message_id` 时不尝试下载图片，避免把错误 key 传给飞书。
- 图片数据进入 Agent session JSONL 以便 Session 详情可查看；IM message log preview 仍使用文本摘要，避免消息列表膨胀。
- `/agent/chat` 与飞书 IM 链路共用 6 张图片上限，超过上限时只传前 6 张并记录 warn。

### 测试方案

- 单元测试：
  - `bifrost_agent::types::tests::user_with_images_serializes_openai_content_parts` 验证 text + image 被序列化为 OpenAI content parts。
  - `im_gateway::feishu::tests::test_normalize_feishu_image_message_extracts_resource_key` 验证飞书图片消息提取 `image_key`。
  - `handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat` 验证 IM event loop 将图片附件传入模型请求，并验证单条消息超过 6 张时截断为 6 张。
- E2E 测试：
  - `im_gateway_agent_chat_multimodal_image_parts` 启动真实 Bifrost admin + mock Chat Completions，通过 `/agent/chat` 发送图片，断言模型请求包含 `image_url` content part，响应包含视觉理解确认，并验证 Session 详情返回持久化图片 content parts。
- 真实场景测试：
  - `human_tests/im-gateway-agent.md` 新增 `TC-IMA-85`，使用非 9900 端口、临时数据目录、`--no-system-proxy` 启动真实 Bifrost，配置 mock 多模态模型，通过 `/agent/chat` 发送图片并验证模型收到图片 content part。
  - `human_tests/im-gateway-agent.md` 新增 `TC-IMA-87`，验证图片数量上限为 6、超出截断并记录 warn，且 WebUI Session 详情图片缩略图可点击放大。

### 校验要求

- `cargo test -p bifrost-agent types::tests::user_with_images_serializes_openai_content_parts`
- `cargo test -p bifrost-admin im_gateway::feishu::tests::test_normalize_feishu_image_message_extracts_resource_key`
- `cargo test -p bifrost-admin handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat`
- `CARGO_TARGET_DIR=target/im-multimodal-e2e BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_multimodal_image_parts --test-timeout 120 --port 18885`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## Agent Chat WebUI 图片粘贴与 Runner 附件桥接

### 目标

Agent Chat Composer 支持直接粘贴图片，图片预览展示在输入框上部，单次最多 6 张。用户可以发送“文本 + 图片”，也可以只发送图片。内置 Bifrost Agent 继续走原生 Chat Completions 多模态 content parts；Codex/BIF/自定义外部 Runner 在没有原生图片输入能力时走文件路径桥接，让 Runner 可以通过本地图片文件消费视觉输入。

### WebUI 交互

1. `AgentChatSection` 维护 `pendingImages`，`TextArea` 的 `onPaste` 从 `clipboardData.items/files` 中筛选 `image/*` 文件。
2. 图片读取为 data URL 后保存 `{id, mimeType, data, previewUrl, name, size}`；`data` 使用去掉 data URL 前缀后的 base64，`previewUrl` 用于缩略图展示。
3. Composer 在输入框上方渲染预览条：最多 6 张，支持逐张删除；超过上限时用 toast 提示并忽略超出部分。
4. 发送按钮启用条件改为 `draft.trim().length > 0 || pendingImages.length > 0 || running`。非运行态允许纯图片发送，运行态 Guide/Queue 仍只支持文本，避免把图片注入到已运行 loop 的中途控制消息。
5. 本地消息和历史消息支持 `contentParts` 图片渲染；用户消息有图片但无文本时展示 `Attached N image(s)` 作为可读占位，同时在气泡中展示缩略图。
6. WebUI 使用 CSS 变量/Ant Design token 颜色，预览框、删除按钮和占位文字必须兼容亮色/暗色主题。

### API 与持久化

1. `/_bifrost/api/agent/chat/stream` 允许 `message` 为空但 `images` 非空；请求体 `images` 继续使用 `{mime_type,data}` 并截断到 6 张。
2. `/_bifrost/api/im-gateway/chat/stream` 与 `ExternalCliRunRequest` 新增 `images` 字段，格式同 WebUI：`{mimeType|mime_type,data,name?}`。
3. `SessionMessage.content_parts` 已从内置 Agent JSONL 恢复路径返回给 WebUI；外部 Runner 的 `session_state` 增加 `content_parts`，保证 Codex/BIF 线程刷新后仍能回显用户粘贴的图片。
4. 线程标题 fallback：若消息文本为空但有图片，使用 `Attached N image(s)`，避免纯图片会话在 Threads 中显示空标题或 session key。

### 外部 Runner 附件桥接

1. 外部 Runner 请求进入 `ExternalCliRuntime::run()` 后，在 run 目录下创建 `attachments/images/`，把图片写为 `image-1.png` / `image-2.jpg` 等稳定文件名。
2. `build_prompt()` 在用户消息前追加：
   - `## Attached Images`
   - 每张图片的 `path`、`mime_type`、`size_bytes`
   - 指令：`Use the local image file paths above when you need to inspect the user's pasted images.`
3. Codex/BIF 这类 CLI Runner 可以通过现有文件读取或 `view_image` 能力消费本地图片；自定义 Runner 即使不支持原生图片，也能从 prompt 中拿到可访问路径。
4. 附件路径写入 `ExternalCliRunResult.metadata` 的 `attachments.images` JSON，便于调试和后续 UI 展示。
5. 队列续跑时只携带当前消息文本，不继承上一轮图片，避免重复消费旧图片。

### 测试方案

- 单元测试：
  - `external_cli::tests::external_cli_run_writes_image_attachments_and_injects_prompt_paths` 验证外部 Runner 写出图片文件、prompt 引用路径、metadata 记录附件。
  - `session_state::tests::session_state_persists_message_content_parts` 验证外部 Runner 会话状态保留用户图片 content parts。
  - `/agent/chat/stream` 的纯图片请求不再被 `message must not be empty` 拒绝。
- UI/E2E 测试：
  - `web/tests/ui/agent-chat.spec.ts` 新增 Composer 图片粘贴用例：粘贴图片后预览在输入框上方，删除可生效，最多保留 6 张，纯图片发送请求体包含 `images`。
  - 断言内置 Bifrost Agent 请求发往 `/api/agent/chat/stream`，外部 Runner 请求发往 `/api/im-gateway/chat/stream` 且携带同样的 `images`。
- 真实场景测试：
  - `human_tests/im-gateway-agent.md` 新增 WebUI 图片粘贴真实用例，使用真实浏览器粘贴图片，分别验证文本+图片、纯图片、6 张上限、删除预览、亮色/暗色主题、外部 Runner 附件路径桥接。

### Review/Fix/Test 闭环

- 第 1 轮：复核用户目标、检查 WebUI paste/preview/send、后端 6 张上限、Builtin 多模态、外部 Runner 文件桥接，运行前端 UI targeted 测试和 Rust targeted 单元测试。
- 第 2 轮：复查第 1 轮修复后的 diff、历史恢复 content parts、human_tests 索引和真实浏览器表现，复跑 targeted 测试并执行项目级校验。

### 1. ImAgentConfig - 全局配置

`ImAgentConfig` 是 `bifrost_agent::AgentConfig` 的兼容别名（见 `crates/bifrost-admin/src/im_gateway/agent.rs`）。当前结构按职责分层，模型 base_url / api_key / wire format 已下沉到 `ModelProviderConfig`，而不是直接挂在 `AgentConfig` 上：

```rust
pub struct AgentConfig {
    pub enabled: bool,
    pub runner: Option<AgentRunnerMode>,

    // -- Model selection --
    pub model: Option<String>,                         // 默认 "gpt-5.4-2026-03-05"
    pub model_provider: Option<String>,                // 默认 "aidp_crawl"
    pub model_providers: HashMap<String, ModelProviderConfig>,

    // -- Prompt instructions --
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub user_instructions: Option<String>,

    // -- Model parameters --
    pub model_reasoning_effort: Option<String>,        // 默认 "medium"，可设 "none"
    pub model_reasoning_summary: Option<String>,       // 默认 "auto"，可设 "none"
    pub model_context_window: Option<i64>,             // 默认 250_000
    pub model_auto_compact_token_limit: Option<i64>,   // 默认 90% 即 225_000
    pub max_completion_tokens: Option<u32>,            // 默认 16384

    // -- Runtime --
    pub max_turn_iterations: Option<u32>,              // 默认 1000
    pub session_ttl_secs: Option<u64>,                 // 默认 3600

    // -- IM outbound --
    /// Agent 在没有 IM 入站来源时使用的默认消息发送通道
    pub default_message_channel: Option<ImMessageChannelBinding>,

    // -- MCP / Skills / AGENTS.md 等其余字段省略，见 crates/agent/src/config.rs --
}

pub struct ModelProviderConfig {
    pub name: Option<String>,
    pub base_url: Option<String>,                      // 例如 https://search.bytedance.net/...
    pub wire_api: Option<ModelWireApi>,                // chat / responses，决定认证 header
    pub env_key: Option<String>,                       // API key 环境变量名，例如 MODELHUB_AK
    pub api_key: Option<String>,                       // 直接值或 $ENV_VAR
    pub http_headers: Option<HashMap<String, String>>,
    pub env_http_headers: Option<HashMap<String, String>>,
    pub request_max_retries: Option<u64>,
    pub stream_idle_timeout_ms: Option<u64>,
    pub stream_max_retries: Option<u64>,
}
```

注意旧文档中提到的 `by_azure: bool` 已被 `ModelProviderConfig.wire_api` + `env_http_headers`（例如 `"api-key" -> "MODELHUB_AK"`）替代；内置 `aidp_crawl` provider 默认即使用 Azure `api-key` header。
**配置持久化**：通过 `AgentConfigStore` 存储为 JSON 文件（`{data_dir}/agent_config.json`），支持热更新。

### 1.1 Agent 默认消息通道与 send_msg 工具

Agent loop 需要一个直接向用户发送消息的工具，但模型不应感知飞书、微信等通道的底层差异。对模型暴露的工具名固定为 `send_msg`；实际可发送的消息类型、默认目标和参数 schema 由 IM Gateway 在每个 turn 开始前根据消息来源、任务绑定和 Agent 配置动态注入。

#### 统一消息通道绑定

消息发送目标使用 provider-neutral 的通道绑定表达：

```rust
pub enum MessageTargetMode {
    SourceThread,
    SourceUser,
    Owner,
    ConfiguredTarget,
}

pub struct ImMessageChannelBinding {
    pub provider_id: String,
    pub target_id: String,
    pub target_mode: MessageTargetMode,
}
```

字段语义：

- `provider_id`：发送消息使用的 IM Provider，例如 `feishu-main` 或 `wechat-main`。
- `target_id`：安全目标引用，可以是已配置 target id，也可以是 `__owner__` 这类后端解析的特殊目标；不向模型暴露 open_id、chat_id、secret 等底层字段。
- `target_mode`：描述 target 的解析方式。`SourceThread` / `SourceUser` 只能来自当前 inbound event；`Owner` 解析为 provider owner；`ConfiguredTarget` 解析为已保存的 `ImTarget`。

Agent 全局配置增加默认发送通道：

```json
{
  "default_message_channel": {
    "provider_id": "feishu-main",
    "target_id": "__owner__",
    "target_mode": "owner"
  }
}
```

该配置用于没有 IM 入站来源的 Agent 场景，例如 WebUI `/agent/chat`、Admin API 触发、手动执行 Agent schedule。Provider 级 `agent_config` 仍只覆盖工作目录和 instructions，不覆盖默认发送通道；默认发送通道属于 Agent 全局可见的 outbound 配置，避免同一个 Provider 同时承担 prompt 覆盖和发送策略两种职责。

#### send_msg 注入规则

`send_msg` 不注册进全局 `ToolRegistry::with_defaults()`。IM Gateway 在进入 agent turn 前构造 `AgentMessageContext`，然后为本 turn 克隆一个工具 registry 并注入 `send_msg`：

```rust
pub struct AgentMessageContext {
    pub inbound_source: Option<ImInboundSource>,
    pub task_message_channel: Option<ImMessageChannelBinding>,
    pub agent_default_channel: Option<ImMessageChannelBinding>,
    pub provider_capability: ImSendCapability,
}
```

注入策略：

1. 飞书/微信消息触发 Agent 时，`inbound_source` 自动携带当前 provider、来源群/用户和 message id。
2. schedule/route/manual run 触发 Agent 时，优先携带任务保存的 `task_message_channel`。
3. WebUI/API 触发 Agent 时，使用 `agent_default_channel`。
4. 三者都不存在时，不注入 `send_msg`，或注入后执行返回明确配置错误：需要配置默认发送通道或显式绑定任务目标。

`send_msg` 作为外部可见副作用工具，必须按 ordered 工具执行，不能进入并行 batch。它的 tool result 只返回 safe summary：`success`、`provider_id`、`target_mode`、`msg_type`、`message_id`、`content_preview` 和错误摘要；不得返回 token、secret、原始 provider config 或真实 receive id。

#### 通道能力裁剪

`send_msg` 的工具名固定，但 description 和 JSON schema 按 provider capability 动态生成：

- 飞书支持 `text`、`markdown`、`interactive card` 时，schema 才出现 `format=card`、`card`、`image_key`、`image` 与 `image_alt` 字段。飞书通道下模型只传 `markdown` 或 `image_key` 时，工具默认构造 JSON 2.0 interactive card 发送，而不是纯文本消息；这样 Markdown 与图片都走飞书卡片渲染能力。
- 微信如果仅支持 `text` / `markdown`，则 schema 只暴露这两类，不提示卡片能力。
- 不支持的消息类型在模型可见 schema 中直接不可见，而不是等执行时报错。

基础参数建议：

```json
{
  "body": "要发送的文本或 markdown",
  "format": "text|markdown|card",
  "target": "default|current_thread|current_user|owner",
  "card": {},
  "image_key": "飞书 image_key，可作为卡片图片元素发送",
  "image_alt": "图片说明"
}
```

`target=default` 的解析优先级是：tool call 显式安全目标、任务绑定通道、当前入站来源、Agent 默认发送通道。schedule 执行时必须优先使用 schedule 保存的任务绑定通道，不能因为后续对话来自另一个 IM 来源而漂移。

#### 飞书 send_msg 卡片默认值

2026-05-27 起，`send_msg` 在 `provider_type=Feishu` 时对 `markdown`、`image_key`、`image` 走卡片优先策略：

1. 如果模型提供 `card`，按原始 interactive card JSON 直通发送。
2. 如果模型提供 `markdown`、`image_key` 或 `image`，后端构造飞书 JSON 2.0 卡片：header 默认 `Bifrost AI`，body 中按顺序放置可选 `img` 元素与 `markdown` 元素。
3. `image_key` 直接作为卡片 `img_key`；`image.data_base64` 先上传为飞书 image_key 后再入卡，避免 data URL 或 Markdown 图片 URL 在飞书客户端变成不可渲染文本。
4. 如果只提供 `text`，仍发送纯文本，便于模型明确要求短文本通知；非飞书通道继续按 provider 能力降级为文本或原有 card 行为。
5. tool result 和 message log 只记录 `msg_type=interactive`、message_id 与安全摘要，不暴露 open_id、tenant token、原始 secret 或上传字节。

### 1.2 Provider 级 Agent 基础配置覆盖

IM Provider 支持可选 `agent_config`，用于给不同 IM 通道绑定不同的 Agent 基础运行上下文：

```json
{
  "agent_config": {
    "work_dir": "/path/to/your/project",
    "base_instructions": "Provider-specific base system prompt",
    "developer_instructions": "Provider-specific developer policy",
    "user_instructions": "Provider-specific AGENTS-style user notes"
  }
}
```

字段语义：

- `work_dir`：Provider 默认工作目录。来自该 Provider 的新 Agent session 会以该目录初始化；未配置时回退到全局 Agent `work_dir`。
- `base_instructions`：base/system instructions。配置后覆盖内置默认 Agent prompt；旧字段 `instructions` / `default_system_prompt` 仅作为兼容别名写入该字段。
- `developer_instructions`：developer instructions。不会覆盖 base prompt，而是作为独立 `<developer_instructions>` section 追加到模型可见系统上下文。
- `user_instructions`：user/AGENTS instructions。会与全局 home AGENTS.md、项目 AGENTS.md 合并后放入 `<user_instructions>`；不会再复用 `base_instructions`，避免同一 prompt 重复注入。

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
- IM 通道收到内置 Bifrost Agent 的 `/clear` 或 `/reset` 时必须由主进程先处理，不能把该命令当作普通消息交给 agent worker。主进程需要同步清理 `AgentSessionManager`、guide/queue 状态、active worker stop handle、`im_gateway/session_state.json` 中当前 built-in adapter 的 `historyPath` / `externalThreadId` / `externalConversationId`，以及该 state 指向的 JSONL timeline；否则 worker 内部 `session.clear()` 完成后，下一条 IM 消息仍会从旧持久化状态恢复上下文。
- 外置 Runner 的 IM `/clear` 或 `/reset` 仍按 adapter + runnerId 精确清理，避免误删同一 IM session key 下其它 runner 的状态；清理前也要请求停止当前 runner 并清空 queue/guide，保证 Stop 后 Clear 不会留下待处理消息或旧进程状态。
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
- Agent Sessions 列表的 session title 与整行都必须可点击进入详情；列表不再展示单独的查看 icon 按钮，删除按钮需要阻止行点击冒泡。
- Agent Session 详情页必须使用 `Messages` / `Settings` 两个 Tab 替代纵向平铺布局：默认选中 `Messages`，历史事件或 active messages 在右侧内容区内形成真实滚动容器；`Settings` 承载 Session Info、AGENTS.md Instructions 和 Skills，避免长设置内容把消息区推到页面下方。
- Agent 设置页导航必须使用主题 token / CSS 变量兼容亮色与暗色主题；桌面端左侧导航固定在自身列，只有右侧当前卡片内容区允许滚动，窄屏退化为顶部横向滚动导航，不遮挡编辑卡片内容。
- Settings → IM Gateway → Add/Edit IM Provider 支持手动填写 Agent Working Directory、Base Instructions、Developer Instructions、User Instructions。
- Settings → IM Gateway → Add/Edit IM Provider 的三段 Provider 级 instruction 同样使用短预览 + Edit 按钮 + 大尺寸弹窗编辑，避免在 Provider 表单里嵌入大段 textarea。
- Provider 级 Base Instructions 继承全局默认值时，编辑弹窗必须提供将继承值复制到编辑草稿的按钮，支持按 Provider 定制后保存覆盖值。
- Provider 卡片展示当前 Provider 是否配置了 Agent Work Dir / Base / Developer / User instructions。
- Provider 卡片展示连接状态、连接配置摘要、Owner、启用状态和 Agent 基础配置摘要。
- Provider 卡片的摘要详情必须使用可收缩的响应式网格：长 owner/work_dir 等代码值在单元格内省略并通过 tooltip 展示完整内容，短状态值（如 Long Connection、Global default）在桌面宽度保持单行，避免跨列遮挡或异常断行；亮色和暗色主题都需要真实浏览器验证。
- Provider 卡片提供单一连接操作入口：已连接/连接中的 Provider 展示 Disconnect provider，未连接 Provider 展示 Connect provider；卡片右侧不再重复展示 Provider Enabled 开关，避免与连接启停操作混淆。Provider Enabled 仍在摘要中只读展示，并通过 Edit 入口动态修改非连接配置（Display Name、Enabled、Owner Open ID、Agent Working Directory、Base/Developer/User Instructions）。
- Add/Edit Provider 表单会展示数据目录默认 Agent `work_dir` 与三层 instructions 作为继承值；字段留空表示继承默认值，用户填写后才在单个 Provider 上形成覆盖。
- Edit 入口只读展示 Provider ID、Type、App ID、Secret 状态和连接模式；连接凭据与连接模式只能在 Add IM Provider 创建时填写，避免误改已经建立的 IM 连接。

### 2. ImAgentConfigStore - 配置存储

- 文件路径：`{data_dir}/agent_config.json`（由 `bifrost-agent::AgentConfigStore` 管理；`ImAgentConfigStore` 仅是 `crates/bifrost-admin/src/im_gateway/agent.rs` 中的兼容别名）
- 支持环境变量替换（`$MODELHUB_AK` → 实际值）
- 提供 `load()` / `save()` / `get_resolved_api_key()` 方法

### 3. ImAgentClient - HTTP 客户端

- 调用端点：`{base_url}/chat/completions`
- 认证方式：
  - Azure 模式：`api-key: {api_key}` header
  - 标准模式：`Authorization: Bearer {api_key}` header
- 非流式请求（`stream: false`）

### 4. ImAgentSessionManager - 会话管理器

`ImAgentSessionManager` 在 `crates/bifrost-admin/src/im_gateway/agent.rs` 中作为 `bifrost_agent::AgentSessionManager` 的兼容别名 re-export。当前真实实现位于 `crates/agent/src/session.rs`：

```rust
pub struct AgentSession {
    /// 会话主键（Web/IM/Runner 通道共用，详见各 adapter）
    pub session_key: String,
    // 完整字段见 crates/agent/src/session.rs::AgentSession
}

pub struct AgentSessionManager {
    /// 按 `session_key` 索引的活跃 session 池
    sessions: DashMap<String, AgentSession>,
    // 还包含 active_turn_status、checkout 状态、TTL 等
}
```

- 基于 `DashMap` 实现线程安全的 per-session 隔离。
- 默认 TTL 由 `AgentConfig::DEFAULT_SESSION_TTL = 3600`（1 小时）控制。
- 取出 / 归还 session 时维护 `ActiveTurnStatus` 共享快照，配合 `/status` 暴露运行中指标。
- 支持内置命令：`/clear`、`/reset`、`/stop`、`/status` 等（实现见 `crates/agent/src/slash.rs` 与 `crates/bifrost-admin/src/im_gateway/agent_slash.rs`）。

### 5. run_turn() / run_turn_with_mcp() - 主入口函数

当前实现位于 `crates/agent/src/session/turn_loop.rs`：`run_turn`、`run_turn_with_mcp` 与多模态变体 `run_turn_with_mcp_multimodal`。**处理流程**：

1. 通过 `AgentSessionManager::take_session*` 查找或创建对应 `session_key` 的 `AgentSession`，并挂载 `ActiveTurnStatus`。
2. `AgentSession::build_messages()` 构建消息列表，调用 `sanitize_chat_history()` 兜底后送入模型。
3. 调用模型 API（`run_turn_with_mcp` 在内置工具基础上额外注入 MCP 工具调用，多模态变体支持图片 content parts）。
4. 把 `tool_call` / `tool_result` / `compaction` / `session_end` 事件落盘到 `~/.bifrost/agent/sessions/.../*.jsonl`。
5. 返回模型响应（`TurnResult`）。
## 默认模型配置

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| Provider | aidp_crawl | 字节跳动内部模型服务 |
| BaseURL | `https://search.bytedance.net/gpt/openapi/online/multimodal/crawl` | API 端点 |
| API Key | `$MODELHUB_AK` | 从环境变量读取 |
| Model | `gpt-5.4-2026-03-05` | 模型版本 |
| ReasoningEffort | `medium` | 推理强度；WebUI 的 Agent → Model 配置可设为 `none`，用于 GPT-5.5 等不支持 Chat Completions `reasoning_effort` 的模型，运行时会省略该请求字段 |
| ReasoningSummary | `auto` | 推理摘要模式；WebUI 的 Agent → Model 配置可设为 `none`，运行时会省略 `reasoning_summary` 字段 |
| MaxCompletionTokens | `16384` | 最大生成 token |
| Authentication | `wire_api=chat` + `env_http_headers.api-key=MODELHUB_AK`（替代旧 `by_azure: true`） | 内置 `aidp_crawl` 用 Azure `api-key` header 认证 |
| SessionTTL | `3600` (1 小时) | 会话过期时间（`AgentConfig::DEFAULT_SESSION_TTL`） |
| ContextWindow | `250_000` | `model_context_window` 默认；auto-compact 阈值默认 90% 即 225,000 |
| MaxTurnIterations | `1000` | `max_turn_iterations` 默认 |
| MaxHistory | 无固定上限 | Agent loop 已移除请求级历史条数限制，由 token/context budget 与 auto-compact 共同收口 |

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
  "model": "gpt-5.4-2026-03-05",
  "model_provider": "aidp_crawl",
  "model_providers": {
    "aidp_crawl": {
      "base_url": "https://search.bytedance.net/gpt/openapi/online/multimodal/crawl",
      "wire_api": "chat",
      "env_key": "MODELHUB_AK",
      "env_http_headers": { "api-key": "MODELHUB_AK" }
    }
  },
  "model_reasoning_effort": "medium",
  "model_reasoning_summary": "auto",
  "model_context_window": 250000,
  "max_completion_tokens": 16384,
  "session_ttl_secs": 3600,
  "max_turn_iterations": 1000
}
```
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
- 持久化到 `{data_dir}/agent_config.json`

### GET /api/im-gateway/agent/sessions

列出当前活跃的会话列表。

```json
{
  "sessions": [
    {
      "session_key": "feishu:ou_xxxxx",
      "running": false,
      "message_count": 5,
      "user_turn_count": 2,
      "compaction_count": 0,
      "work_dir": "/path/to/project",
      "created_at": 1781595586,
      "last_active_at": 1781598446
    }
  ],
  "total": 1
}
```

实际字段完整列表见 `crates/agent/src/session/session_store.rs::SessionInfo`，包含 `agent_type` / `runner_type` / `runner_id` / `external_thread_id` / `external_conversation_id` / `title` 等扩展字段。
```

## 会话管理

### 设计原则

1. **Per-Session 隔离**：每个 `session_key`（IM 通常衍生自 provider + open_id / chat_id，Web/Runner 自定义命名）独立会话，互不干扰。
2. **JSONL 持久化**：session 事件按 `~/.bifrost/agent/sessions/YYYY/MM/DD/session-<key>-*.jsonl` 落盘，重启或 `/resume` 后可恢复。
3. **TTL 过期**：默认 `AgentConfig::DEFAULT_SESSION_TTL = 3600`（1 小时）无活动后回收内存 session（JSONL 仍保留）。
4. **历史无固定条数限制**：Agent loop 使用完整 sanitized history，由 token/context budget、auto-compact 与 provider context-window 错误共同收口（详见后文 §“为什么不限制历史消息数”）。

### 会话结构

```rust
pub struct AgentSession {
    pub session_key: String,
    pub messages: Vec<ChatMessage>,
    pub last_active: Instant,
    pub created_at: Instant,
    pub active_turn_status: Option<Arc<ActiveTurnStatus>>,
    pub compaction_count: u32,
    pub work_dir: Option<String>,
    // 完整字段见 crates/agent/src/session.rs::AgentSession
}

pub struct ChatMessage {
    // 兼容 OpenAI Chat Completions：role + 可选 tool_calls / tool_call_id / content parts
    // 完整定义见 crates/agent/src/types.rs::ChatMessage
}
```

### 内置命令

| 命令 | 功能 | 实现 |
|------|------|------|
| `/clear` | 清空当前会话历史并开始新对话 | 主进程清理内存 session、queue/guide、worker stop handle、IM session_state 和对应 JSONL history |
| `/reset` | 重置会话（等同 /clear） | 与 `/clear` 相同 |

**命令处理流程**：
1. 检测消息是否以 `/clear` 或 `/reset` 开头
2. 若当前通道使用外置 Runner，按 adapter + runnerId 停止并清理该 runner 的持久状态；若使用内置 Bifrost Agent，由主进程直接清理 built-in adapter 的状态
3. 清空 guide/queue，删除旧 `historyPath` 指向的 JSONL，避免下一条消息 fallback 恢复旧上下文
4. 返回确认消息（不调用模型）

### Stop 后 Clear 回归

2026-06-02 修复 IM 通道中 `/stop` 后 `/clear` 无效的问题。旧流程里，内置 Bifrost Agent 的 IM `/clear` 进入 worker turn loop，worker 内部会清空临时 `AgentSession`，但主进程的 `im_gateway/session_state.json` 与旧 JSONL timeline 仍保留。下一条 IM 消息再次启动 worker 时，主进程先从旧 `historyPath` 恢复 history，worker 也会在缺少明确 historyPath 时按 session key fallback 到最新 JSONL，导致用户看到“Clear 成功”但旧对话被恢复。

修复后的不变量：

- IM built-in `/clear` / `/reset` 必须在主进程短路处理，不创建新的模型请求。
- 清理后 `load_session_state(session_key, BUILTIN_AGENT_ADAPTER, None)` 返回空，state 指向的 JSONL 文件不存在。
- 下一条 IM 普通消息使用同一 provider/user/session key 时，请求体只能包含新消息，不得包含 Clear 前的 user/assistant 旧上下文。
- `/stop` 后无论 worker 是否已经退出，Clear 都要尝试清理 active worker handle 和 queue/guide；无 active worker 时清理应幂等成功。
- Chat Gateway / 外置 Runner 的清理仍保持 adapter + runnerId 作用域，避免影响同一 IM 通道下其它 runner。

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

- **兼容性**：通过 `ModelProviderConfig.wire_api` + `env_http_headers` 在同一 provider 抽象内表达 Azure `api-key`、标准 OpenAI Bearer 或自定义 header；旧文档中的 `by_azure: bool` 已废弃。
- **兼容性**：`by_azure: true` 可配置，同时支持标准 OpenAI API
- **安全性**：避免 Bearer token 被误用
### 4. 为什么会话以 JSONL 持久化
### 4. 为什么会话不持久化

**选择内存存储的原因**：

- **隐私性**：对话历史敏感，不落盘更安全
- **时效性**：对话上下文有时效性，持久化意义不大
- **简单性**：避免引入数据库依赖和迁移逻辑
- **重启清空**：服务重启后从新对话开始，符合用户预期

### 5. 为什么不限制历史消息数

Agent Loop 已移除请求级历史条数限制，常规请求使用完整 sanitized history；上下文管理对齐 Codex，由 token/context budget、自动压缩和 provider context-window 错误共同驱动。

- **语义完整**：不因为固定消息条数裁掉早期需求、约束或工具结果。
- **压缩优先**：达到 token 阈值时用 compaction 生成 summary，而不是直接截断。
- **错误显式化**：provider 仍报告超窗时保留 live history 并返回错误，避免静默丢上下文。

## 文件结构

```
crates/agent/                     # bifrost-agent 主体实现
└── src/
    ├── config.rs             # AgentConfig / AgentConfigStore（agent_config.json）
    ├── client.rs             # AgentClient + Chat Completions HTTP 调用
    ├── session.rs            # AgentSession / AgentSessionManager / ActiveTurnStatus
    ├── session/turn_loop.rs  # run_turn / run_turn_with_mcp / run_turn_with_mcp_multimodal
    ├── persistence.rs        # ConversationRecorder + load_conversation(_lossy)
    ├── history.rs            # sanitize_chat_history + history invariants
    ├── compact.rs            # 自动/手动压缩、context window budget
    ├── mcp/                  # MCP tool / server 接入
    └── tools/                # 内置工具

crates/bifrost-admin/
└── src/
    ├── im_gateway/
    │   ├── agent.rs          # Agent 类型 re-exports (from bifrost_agent)：
    │   │   ├── ImAgentConfig          # = bifrost_agent::AgentConfig
    │   │   ├── ImAgentConfigStore     # = bifrost_agent::AgentConfigStore
    │   │   ├── ImAgentClient          # = bifrost_agent::AgentClient
    │   │   ├── ImAgentSessionManager  # = bifrost_agent::AgentSessionManager
    │   │   ├── run_turn               # = bifrost_agent::session::run_turn
    │   │   └── run_turn_with_mcp      # = bifrost_agent::session::run_turn_with_mcp
    │   ├── agent_slash.rs    # IM 通道 /clear /reset /stop /status 短路处理
    │   ├── agent_worker.rs   # 独立 `bifrost agent worker` 子进程入口
    │   ├── send_msg_tool.rs  # 通道无关 send_msg 工具，按 provider capability 动态注入
    │   ├── session_state.rs  # 外置 Runner / 内置 Agent IM session 持久状态
    │   ├── types.rs          # ImRouteAction::AgentChat 变体
    │   └── mod.rs
    └── handlers/
        ├── im_gateway.rs    # IM Gateway HTTP Handler 统一入口
        └── agent_chat.rs    # /_bifrost/api/im-gateway/agent/chat 等 Admin API
```
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
| `test_build_messages_uses_full_sanitized_history` | 验证常规请求使用完整 sanitized history |
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
| Agent retry orphan tool 回归 | `im_gateway_agent_retry_sanitizes_orphan_tool_history` 在首次 500 + 历史含孤儿 `tool` 时重试请求仍保持合法消息序列，并继续完成工具调用 |
| Chat API 长期记忆真实链路 | 运行 `e2e-tests/tests/test_long_term_memory_human_api.sh`，验证 `POST /_bifrost/api/im-gateway/agent/chat` 在真实 Bifrost + mock Chat Completions 下触发自动记忆、Phase 2 consolidation、跨独立 session 消费 |
| Chat API runtime gate 回归 | 运行 `e2e-tests/tests/test_update_plan_human_api.sh`，验证 `/agent/chat` 路径下 update_plan runtime 收口提醒仍会强制模型在结束前补齐最终 plan 状态 |
| Chat API runtime limits 回归 | 运行 `e2e-tests/tests/test_agent_loop_runtime_limits.sh`，验证默认 1000 次 turn 上限与 600 秒超时配置在 `/agent/chat` 黑盒链路中生效 |
| Chat API 引导/排队注入回归 | 通过 `/api/im-gateway/agent/chat` 的测试专用字段 `guide_message` / `guide_messages` / `queue_messages`，验证 turn-end guide drain、多条 guide 在进入 loop 前通过 `/status` 展示明细并合并消费、queued FIFO drain、guide 优先于 queue，以及空白注入被忽略；隔离 worker 已接收 guide 但尚未完成当前 turn 时，主进程 `/status` 必须继续展示 handed-off guide 快照 |
| IM busy runner-aware 默认策略回归 | 真实 IM/debug inbound busy 链路按 runner 能力分流：内置 Bifrost Agent 普通追加消息默认进入 guide channel，只有 `/q` 进入 queue；ChatGPT Web、Codex 和其他自定义 runner 普通追加消息默认进入 queue |
| Codex Runner 排队续聊回归 | Codex CLI 当前支持 `codex exec resume <thread_id> [PROMPT]` 进行下一轮接续，不支持运行中追加 guide。外部 runner 队列 drain 时必须继承上一轮 Codex JSONL 解析出的 `threadId`，让排队消息通过 resume 续同一个 Codex session |
| 飞书进度卡片与 `/status` 指标格式化回归 | progress card 折叠标题、展开状态区和 `/status` 中的 Token、Context 数字统一使用 K/M/B 单位，最多一位小数并去掉 `.0`，例如 `38634 -> 38.6K`、`19333 -> 19.3K`、`250000 -> 250K`、`1000000 -> 1M` |
| 飞书进度卡片冻结并新发回归 | Feishu CardKit 无法移动既有消息；运行中 guide/queue 状态变化和 queue drain 下一轮都必须先新建并发送 card entity，使最新 progress card 位于最新用户消息下方，再 best-effort 把旧 Running card 更新为结束/冻结快照并关闭 streaming；冻结失败只记录 warn，不阻断新卡发送；已 Finished/Failed 的历史卡片必须保留，后续独立新消息只能新发卡片，不得改写或撤回旧卡 |
| IM turn-end 入站消息不丢失回归 | 内置 IM Agent 在模型最后输出或 `process_agent_chat` 刚结束时若收到同 session 新消息，event loop 必须在清理 guide/queue 前 drain 已到达 channel 的事件，并把消息落入 guide/queue 后继续下一轮，不能只 ACK 后丢失 |
| `/status` runner 元信息回归 | IM `/status` 和 `/agent/chat` `/status` 展示当前 Agent 类型、Runner 类型、Runner ID、历史对话轮次、外部会话引用；Codex 展示 `threadId`，ChatGPT Web 展示 `conversationId` |
| 压缩次数恢复回归 | session JSONL 中的 `compaction` 事件会恢复为 `SessionRuntimeState.compaction_count`，`/resume` 后 `/status` 不再把已发生的压缩次数重置为 0 |
| Agent 模型请求默认代理回归 | `im_gateway_agent_model_request_uses_bifrost_proxy` 使用 `AgentClient::new_with_bifrost_proxy(port)` 调用 mock Chat Completions，断言请求经当前 Bifrost 端口转发并在 `/api/traffic` 中出现可查询记录 |
| Agent worker 代理端口恢复回归 | 独立 `bifrost agent worker` 子进程从父进程请求或 `BIFROST_DATA_DIR/runtime.json` 恢复当前 Bifrost 端口，并使用当前 Bifrost CA 信任内置 TLS intercept；`AgentClient::new()` 不读取外部 `HTTP_PROXY/HTTPS_PROXY` 环境变量 |
| Chat API `/stop` 停止运行中 loop | `im_gateway_agent_chat_stop_active_loop` 启动真实 Admin + 慢速 mock Chat Completions，先发起长请求，再用同 session 的 `/stop` 立即停止 active turn，并验证后续 chat 可继续使用 |
| Web Agent Chat 持久排队恢复回归 | WebUI 运行中输入 Queue 时，`/_bifrost/api/agent/chat/stream` busy 路径只把消息写入后端 `SessionQueueManager`；`/sessions/all` 与 `/sessions/{session_key}` 返回 `queue_items` / `queue_length`，页面刷新后从后端恢复排队面板；当前 turn 完成后由后端 drain queue，前端不得再次重发队列消息 |
| WebUI instruction 大窗口编辑回归 | `Settings Agent 三层 instructions 使用大窗口编辑` 验证全局 Agent instruction 页面无行内 textarea、点击 Edit 打开大弹窗并 PATCH；`Settings IM Provider instructions 使用大窗口编辑后保存覆盖值` 验证 Provider Edit 弹窗中 instruction 通过嵌套大弹窗编辑并保存到 `agent_config` |
| WebUI Session 详情 Tab 与滚动回归 | `AI Agent Session 详情默认展示 Messages Tab 且内容区可真实滚动` 验证 history 深链默认进入 Messages、长事件列表在 `agent-session-messages-scroll` 内滚动、Settings Tab 展示 metadata/AGENTS/Skills |
| WebUI Sessions 列表点击进入详情回归 | `AI Agent Sessions 列表支持点击 title 或整行进入详情` 验证列表不再展示查看 icon，history title 点击进入 history 详情，active session 整行点击进入 active 详情 |
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
| TC-GQ-14 | 多 guide pending status 与合并消费 | 通过 `/agent/chat` 注入多条 `guide_messages`，运行中 `/status` 展示尚未进入 loop 的具体 guide 列表，随后 loop 将多条 guide 合并为一条 user message 继续处理 |
| TC-GQ-15 | 内置 Agent busy 普通消息默认 guide | 通过 IM/debug inbound 在内置 Bifrost Agent active turn 期间发送普通消息，验证 `/status` 暴露 pending guide，且消息未进入 queue；当 guide 已从主进程 channel 转交给隔离 worker 后，`/status` 仍合并 handed-off guide 快照，避免运行中状态短暂显示“引导消息: 无” |
| TC-GQ-16 | 自定义 Runner busy 普通消息默认 queue | 通过 IM/debug inbound 在自定义 runner active run 期间发送普通消息，验证消息等待当前 run 结束后再处理；Codex runner 若返回 `threadId`，下一条排队消息使用 `codex exec resume` 接续 |
| TC-IMA-90A | 飞书流式进度卡片与 `/status` Token/Context KMB 格式化 | 构造百万级 Token 与几十万 Context 的 progress card，并调用 `/status`，验证折叠标题、展开状态区和状态文本均展示 `K/M/B` 单位，不再裸显长数字 |
| TC-IMA-90B | `/status` runner 元信息与压缩次数回归 | 构造外部 runner session 和 compaction 记录，验证 `/status` 展示 Agent 类型、Runner 类型、Runner ID、历史对话轮次、`threadId` / `conversationId`，且恢复后压缩次数保持非 0 |
| TC-IMA-91/92/139/140 | 飞书流式进度卡片冻结并新发 | running 中 guide/queue 更新必须保留当前快照、发送新卡并冻结旧卡；缺失 `message_id` 不影响 freeze，因为 freeze 基于 `card_id`；新卡发送失败时保留旧 running handle；queue drain 下一轮在旧卡仍 Running 时冻结旧卡并新发；已完成历史卡片不改写不撤回；turn-end 窗口消息会进入 guide/queue 并继续下一轮 |
| TC-IMA-91A | Web Agent Chat 后端持久排队与刷新恢复 | 同一 session 运行中在 WebUI 选择 Queue 发送追加消息；刷新页面后队列面板仍从后端 `queue_items` 恢复；上一轮结束后由后端自动处理排队消息，前端不再本地重发 |
| TC-LTM-09 | 长期记忆真实对话链路 | 真实 Bifrost + mock Chat API 环境下验证自动记忆、Phase 2 consolidation、跨 session 消费 |
| TC-IMA-83 | Agent 模型请求默认进入 Traffic | 真实 Bifrost 监听端口启动后，Agent 底层 Chat Completions 请求默认经 `http://127.0.0.1:<port>` 代理发出；mock 模型 host 可查询到 POST 记录，真实模型域名在 `--intercept-include` 下可解包为 HTTPS POST 明文记录 |
| TC-IMA-83A | Agent worker 内置代理信任与外部 proxy env 隔离 | IM/Web 入口的内置 Agent worker 使用当前 Bifrost 端口和 `data_dir/certs/ca.crt` 访问模型；CLI `agent run` 通过 Admin Server stream 执行，Server 不运行时明确失败；库级 direct client 不被 shell/system proxy 环境变量劫持 |
| TC-IMA-83B | Bifrost 异步子进程场景化命名 | 内置 Agent、external Runner、Voice、ASR 这类独立子进程通过 `bifrost-agent`、`bifrost-runner`、`bifrost-voice`、`bifrost-asr-server`、`bifrost-asr-cli` 场景别名启动，系统进程列表不再全显示为 `bifrost` |
| TC-IMA-84 | Agent 设置页卡片导航 | Settings → Agent 左侧导航可见，点击 MCP Servers / Runtime 只渲染对应编辑卡片，URL `agentSection` 可刷新恢复，亮色与暗色主题下当前项高亮可读 |
| TC-ASP-14 | WebUI Session 详情 Messages/Settings Tab 与右侧内容滚动回归 | 历史 session 深链默认显示 Messages Tab，长事件列表在右侧内容区真实滚动；Settings Tab 展示 Session Info、AGENTS.md Instructions 和 Skills |
| TC-ASP-15 | WebUI Sessions 列表 title/整行点击进入详情回归 | Sessions 列表不再显示查看 icon；点击 history session title 进入 history 详情；点击 active session 当前行进入 active 详情；删除按钮不会触发行跳转 |
| TC-IMA-84A | Agent 模型 reasoning 参数开关 | Settings → Agent → Model 可把 Reasoning Effort / Summary 设为 `None (disabled)`，API 持久化为 `model_reasoning_effort=none`、`model_reasoning_summary=none`，运行时不会把对应 Chat Completions 字段发给不支持的模型 |
| TC-IMA-89 | `/stop` 停止运行中 Agent loop | 同 session 发起长模型请求后发送 `/stop`，验证 stop 请求立即返回 stopped，原 chat 返回停止提示，session 释放后后续 chat 成功 |
| TC-IMA-53A | 新建 IM Provider 的 agent_config 经 IM 事件链路生效 | Provider 创建时配置 base/developer/user/work_dir 后，IM inbound event 进入 `run_event_loop` 时模型请求使用 Provider 级配置而非全局 fallback |

## Agent 模型请求代理

IM Gateway 内嵌 Agent 默认通过当前启动的 Bifrost HTTP 代理访问模型提供方：真实 CLI 启动和 E2E `ProxyInstance::start_with_admin` 都使用 `ImGatewayService::new_with_agent_proxy_port(data_dir, Some(port))` 创建服务，底层 `AgentClient::new_with_bifrost_proxy_and_ca(port, data_dir/certs/ca.crt)` 会把 Chat Completions 请求代理到 `http://127.0.0.1:<port>`，并只把当前 Bifrost CA 加入 Agent 自己的 reqwest trust store。这样模型请求、响应、状态码和耗时会落入现有 Traffic 记录；对模型域名启用 TLS intercept 时，Agent 不会因为 Bifrost 签发的拦截证书报 `UnknownIssuer`。

内置 Agent loop 实际在隔离的 `bifrost agent worker` 子进程里执行。父进程创建 worker 请求时必须携带当前 Bifrost 端口；worker 也必须能从 `BIFROST_ADMIN_PORT` 或 `BIFROST_DATA_DIR/runtime.json` 恢复端口，避免子进程退化成 direct client。库级 `AgentClient::new()` 使用 `direct_reqwest_client_builder().no_proxy()`，不能读取 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY` 等外部环境变量；否则日志会显示 `model_proxy_url="direct"`，但请求实际被 shell/system proxy 劫持，且没有加载 Bifrost CA，最终在 TLS intercept 场景下报 `UnknownIssuer`。

Agent 相关 HTTP 客户端必须复用 `bifrost_core` 的 direct/proxied client builder：MCP Streamable HTTP、MCP availability 检查、Agent worker 内 MCP、IM event loop 内 MCP、MCP OAuth discovery/token、Agent 回复远端图片/附件下载、ChatGPT Web native/CDP HTTP 探测都不能裸用 `reqwest::Client::new()` 或默认 builder。HTTP MCP 在 Agent 已配置内置代理时使用同一个 Bifrost proxy URL 与 `data_dir/certs/ca.crt`；stdio MCP 子进程默认移除继承自父进程的 proxy 环境变量，只有 MCP server config 的 `env` 显式写入时才会生效。

`bifrost agent run` 是 Server 模式命令：CLI 只调用 `/_bifrost/api/im-gateway/chat/stream`，由已经启动的 Bifrost Server 再拉起内置 worker 或 `external-runner-worker` 执行 Codex/ChatGPT Web/自定义 Runner。若代理服务未启动，CLI 应明确提示 `Failed to reach Bifrost ... is the proxy running?`，而不是在当前 CLI 进程中 fallback 本地执行。

库级直连调用仍保留 `AgentClient::new()`，用于纯单元测试和不在 Bifrost 服务内运行的场景。需要临时绕过默认代理时，可设置 `BIFROST_AGENT_DISABLE_MODEL_PROXY=1`，服务会回退为直连模型请求。

Agent/Runner/Voice/ASR 这类异步工作必须在独立进程中运行，主进程只保留代理、Admin 网关和调度任务。为了方便在 Activity Monitor、`ps` 等系统进程列表中辨认，所有由 Bifrost 启动的长期 worker 都必须通过 `runtime/process-aliases/` 下的场景别名入口 exec：内置 Agent worker 使用 `bifrost-agent`，外部 CLI Runner worker 使用 `bifrost-runner`，Voice worker 使用 `bifrost-voice`，托管 ASR server 使用 `bifrost-asr-server`，按 chunk fork 的 ASR CLI 使用 `bifrost-asr-cli`。别名创建失败不能阻断业务，应记录 warning 并回退到原 executable。

## `/agent/chat` `/status` 工作目录语义

`POST /_bifrost/api/im-gateway/agent/chat` 的 `/status` 是 session-free 快速命令，不应进入模型 turn，也不应抢占正在运行的 session。对于 idle session，它仍必须保留普通 chat 请求的 `work_dir` 语义：

1. 如果 session 正在运行，优先返回 active turn status，不修改运行中工作目录。
2. 如果请求携带非空 `work_dir` 且 session 不存在，创建空 session 并把 status 输出中的 `工作路径` 设置为该请求路径。
3. 如果 session 已存在且请求携带新的 `work_dir`，使用与普通 chat 相同的 work_dir override 逻辑重初始化 idle session，然后格式化 status。
4. 如果 session 不存在且请求未携带 `work_dir`，保持“新会话”纯读输出，不额外创建持久 session。

回归覆盖：

- `agent_api_status_detail_applies_work_dir_for_fresh_status_session`
- `agent_api_status_detail_overrides_existing_idle_session_work_dir`
- `agent_api_status_detail_keeps_new_session_text_when_no_work_dir_requested`
- `human_tests/agent-builtin-commands.md` 的 TC-BC-34 通过真实 Admin API 验证新 session `/status` 响应包含请求工作路径。

## `/agent/chat` `/stop` 停止语义

`POST /_bifrost/api/im-gateway/agent/chat` 收到同 session 的 `/stop` 时，必须作为 session-free 控制命令处理：不等待当前 session lock、不进入模型 turn、不排队。运行中的 `AgentSessionManager` 为每个 active turn 维护 stop signal；`/stop` 只设置该 signal 并立即返回。turn loop 在模型请求、重试等待、工具执行前后检查 signal；命中后返回用户可见的停止提示，标记 goal 为 interrupted，补齐已声明但未执行的 tool result，释放 session。

IM 事件链路也使用同一语义：busy session 收到 `/stop` 时调用 `request_stop(session_key)`，而不是进入 guide 或 queue。空闲 session 收到 `/stop` 返回“当前没有正在执行的 Agent loop”。

边界要求：

1. `/stop` 不清空历史，不修改工作目录，不影响 `/status` 的 active snapshot。
2. `/stop` 请求本身不抢占 session；正在运行的原 chat 请求会收到“已收到 /stop，正在执行的 Agent loop 已停止。”。
3. 停止发生在 tool_calls 后时，必须为剩余 tool call 写入取消 tool result，避免恢复后的 OpenAI-compatible history 出现悬空 assistant tool_calls。
4. session 释放后，同一个 session_key 的后续普通 chat 必须能继续执行。

回归覆盖：

- `session::tests::test_stop_request_cancels_in_flight_model_request`
- `im_gateway_agent_chat_stop_active_loop`
- `human_tests/im-gateway-agent.md` 的 TC-IMA-89 通过真实 Admin API 验证 active `/status`、`/stop`、原 chat 停止返回和后续 chat 恢复。

## Agent Chat 页面信息架构

Agent Chat 页面右侧侧栏只承载 Threads 列表。Workspace、Status、Context、Errors、Run Settings 等状态信息从侧栏移入 `Agent Chat Status` 弹窗，弹窗入口放在 composer 区域的 New Chat 按钮旁边，避免右侧列表被设置卡片挤压。对话标题栏展示当前会话 title、来源标签、Runner 标签和状态标签，让用户不用打开弹窗也能判断该对话来自 Web/IM/Runner/ASR、使用哪个 runner、是否 running/ready。已执行工具调用的 Args/Result 只属于消息过程步骤，不展示在 Status 弹窗中。

Threads 数据源使用 `/api/im-gateway/agent/sessions/all` 的 active + history 合并结果，后端按 `session_key` 去重，前端再做兜底去重：同一 `session_key` 只展示一条记录，active 优先于 history。线程列表使用无边框两行列表：左侧小标识只表达 Runner 类型（Bifrost Agent / Codex / ChatGPT Web / Unknown），第二行表达入口渠道（Web / WeChat / Feishu / ASR Task / Scheduled）以及创建时间、运行时长。线程列表不展示 `Active` / `Ended` 文案，避免噪音；只有运行中的线程展示跳动绿点。

线程行右键使用可扩展 context menu，而不是把操作按钮挤在线程行内。当前菜单包含 Delete；点击 Delete 后在同一个菜单位置切换为 Confirm / Cancel 原位二次确认，后续可继续追加复制 session key、导出 history、打开详情等菜单项。删除操作调用 `DELETE /api/im-gateway/agent/sessions/{session_key}`，服务端必须停止运行中 turn、清理内存 session、queue/guide、`session_state.json` 中的外部 runner 状态以及同 `session_key` 的 JSONL history，避免 UI 删除后刷新又从另一个数据源合并回来。

线程标题必须来自统一摘要语义，避免列表未选中时显示 `session_key`、选中后又因详情加载改成首条用户消息而抖动。后端 active session list、JSONL history scan、`/sessions/{session_key}` 详情合成都提供同一套 `title` fallback：显式 `set_title` / `title_updated` 持久化标题优先级最高；只有没有显式标题时，才使用第一条用户消息的 UTF-8 安全摘要；最后才由前端兜底显示 `session_key`。`plan_updated` 的标题或说明只属于 Plan 模块，不允许覆盖 Conversation title。这样 WebUI 不需要猜测多个来源的标题，Codex Runner、ChatGPT Web Runner 和内置 Bifrost Agent 只在扩展字段上差异化，公共字段保持一致。

窄屏布局仍保持 Conversation 与 Threads 平级左右布局，不把 Threads 挤到 Conversation 下方。Threads 宽度使用有上限的右栏，标题文本必须 `min-width: 0` 并单行省略，防止长中文标题撑宽整页。

首次进入 Agent Chat 且 URL 没有指定 `session` / `historyPath` 时，如果 Threads 中已有会话，默认打开第一条线程，让用户直接看到最近对话；如果没有任何线程，则消息列表保持空，不渲染 demo/starter 对话，只在空态提示用户从输入框发起问题。用户主动点击 New Chat 后进入空白草稿，这个状态不能再被“默认选中第一条线程”逻辑抢回旧会话。

对话区在宽屏上不能把 user/assistant 气泡拉到屏幕两端。Conversation 卡片保持占满主栏，但消息轨道和 composer 内容轨道限制 `max-width: 750px` 并水平居中；窄屏仍使用 `width: 100%`，右侧 Threads 继续保持与 Conversation 平级展示。

运行中的会话必须作为服务端数据源的一等成员暴露给 Threads。内置 Bifrost Agent turn 执行期间，`AgentSessionManager` 会把 session checkout 出 idle map，因此必须维护 `active_session_infos` 快照；Web 发起新 turn 时用当前消息作为临时 title fallback，并记录 workspace、source、runner 元信息。外部 Runner 的 `/api/im-gateway/chat/stream` 同样在开始运行时写入 active preview。这样用户在等待回复时刷新页面，`/sessions/all` 仍返回该 running session，线程列表不会消失。

外部 Runner 完成后的会话也必须进入同一个服务端数据源。`/api/im-gateway/chat/stream` 在 Codex/ChatGPT Web/自定义 Runner 完成后，把 `latest_run_id`、首条用户消息、最终回复、runner_id、adapter、work_dir、status 写入 `im_gateway/session_state.json`；`/sessions/all` 会把这些 session state 作为 ended thread 返回，`/sessions/{session_key}` 在没有内置 Agent in-memory detail 时从最新 run detail 或 session state 合成 user/assistant 消息。这样 Codex Runner 运行成功后不会因为 active preview 被清理而从线程列表和对话详情中消失。

外部 Runner 的 session state 不能只保存最后一轮。Codex、ChatGPT Web、自定义 Runner 每次 run 完成后，都要把本轮 user/assistant message 追加到同一个 `session_key + adapter + runner_id` 的消息序列中，并保留 external `threadId` / `conversationId` 作为续聊引用。`/sessions/all` 的 turns、start_time、duration 以及 `/sessions/{session_key}` 的 messages 必须从这条消息序列生成，确保 5 轮及以上多轮对话仍完整挂在同一线程下，不因 latest run 或 conversation id 变化漂移成多个线程。

ChatGPT Web Runner 的 DOM fallback 不能只识别传统 `<img>` 生成图。ChatGPT 可能把生成图片卡片、信息卡和 ZIP 打包下载渲染成 `button.behavior-btn` / `entity-underline` 行为按钮，按钮本身没有 `<a href>`，也不会出现在 `generatedImages`。DOM 提取需要在最终 assistant turn 内识别这些按钮，过滤复制、分享、来源、模型切换和输入文件 pill，把图片类按钮标记为 `kind=image`，把 `ZIP` / `打包下载` 上下文标记为 `kind=archive`，写入 `conversation_final.json.final.artifacts` / run raw `artifacts`。Runner 在 `ask` / `wait` 收尾时复用默认 ChatGPT Web 浏览器登录态，通过 CDP 打开 `/c/{conversationId}`：对 `kind=image` 按钮逐个点击并从图片 dialog / `estuary/content` URL 下载原图到 `attachments/chatgpt_web/<conversationId>/`，最终回复追加本地 Markdown 图片 `![label](path)`，由 IM 投递层拆出并按通道单独发图；对 `kind=archive` / ZIP / 压缩包按钮配置临时下载目录、监听下载完成事件并归档 ZIP，同时在最终回复和 run raw 中写入 `downloadedArtifacts`。如果某个行为按钮点击后没有暴露可下载图片或附件，则保留可点击下载项摘要，避免 IM 或 Agent Chat 里只看到 `打包下载：` 而不知道实际缺失了哪些产物。

对话详情恢复必须带上运行元信息，而不仅恢复消息文本：

1. active session 深链通过 `/api/im-gateway/agent/sessions/{session_key}` 恢复 messages、title、work_dir、message_count、token、compaction、runner 元信息。
2. history session 深链通过 JSONL events 恢复 messages，并从 `session_start`、`plan_updated`、`tool_call`、`tool_result`、`compaction`、`session_end` 回填 workspace、plan、tools、context 和完成状态。
3. 新建对话从 `/api/im-gateway/agent/instructions` 读取默认 `work_dir`。只有点击 New Chat 时弹出 workspace 输入框，用户可在创建新会话前选择路径；确认后该 workspace 随 stream 请求进入后端并保存到 session runtime state。
4. 已初始化过的会话不允许在 Settings 中切换 workspace。原因是会话初始化时已经加载了工作目录相关的 AGENTS.md、skills、词典和执行上下文；后续切换路径会导致 UI 展示与真实运行上下文错位。Settings 中的 Workspace 只读展示当前会话路径。
5. 如果当前已经是未输入问题、未产生历史、未初始化运行信息的新会话，再次点击 New Chat 并确认时只更新待创建会话的 workspace，不生成新的 `admin-chat-*` session id。
6. 切换对话或刷新恢复时消息区第一次直接定位到底部，使用非动画滚动；同一个 history/session 的重复详情加载、线程列表刷新、标题回填不再替换消息或抢滚动位置。用户手动向上阅读历史时，后续状态刷新不能把 scrollTop 拉回底部。
7. New Chat 弹窗允许选择待创建会话的 Runner。内置 `bifrost_agent` 与自定义 Runner registry 的 `defaultRunnerId + runners{}` 共用一个下拉框；选择 Codex/ChatGPT Web/其他 Runner 后，该选择锁定到新会话，并在发送时分别走内置 Agent SSE 或外部 Runner NDJSON stream。

刷新页面或关闭浏览器响应流不能代表用户停止 Agent Loop。`/_bifrost/api/agent/chat/stream` 和 `/_bifrost/api/im-gateway/chat/stream` 的 SSE/NDJSON client disconnect 只停止向该 HTTP 响应写入增量，不调用 `request_stop` 或 external CLI stop marker；后台 turn/run 继续执行并在完成后归还 session / 记录 runner state。只有显式点击停止当前轮次或发送 `/stop`，才允许写入 stop signal。

多线程并发运行时，WebView 切换会话必须把“旧流事件”和“旧流收尾”都隔离掉。切换线程时前端用 `AbortController` 中止当前 HTTP stream，并在 `onEvent`、`onFinal`、`catch` 和成功收尾路径中用选中 `sessionKey` 做 guard，丢弃旧会话的延迟事件。即使旧 stream 因 abort 进入 `finally`，也不能无条件 `setRunning(false)` 或清空 collaboration mode；这些状态只能在当前选中会话仍是发起 stream 的会话时更新，避免 A 线程收尾把已经切到的 B 线程按钮、状态 tag 或输入模式打乱。

运行中的输入框不能禁用。无输入时，输入框内右下角主按钮切换为 Stop；有输入时，内置 Bifrost Agent 展示 Guide / Queue 模式切换，默认 Guide 注入当前 loop，也可选择 Queue 等当前轮结束后处理；Codex、ChatGPT Web 和其他外部 Runner 不支持运行中 guide，默认只排队。Queue 状态显示在输入框上方，支持多条追加与删除；当 Runner 支持 guide 时，队列项可一键改为立即 Guide。Queue/Remove 是本地交互状态，不应插入 MessageList，也不应作为 assistant 消息持久化；只有排队项被实际 drain 成下一轮输入后，才进入消息列表和历史。

Composer 与 MessageList 共用同一个滚动容器。输入区使用 sticky/floating 样式贴在对话容器底部，短消息时仍位于容器底部，长历史时随同一滚动容器保持底部悬浮；输入区不再通过顶部硬边框与消息列表切开。

Plan 不属于 Settings 弹窗。存在 plan 时，Plan 面板展示在输入框上方；没有 plan 时整个模块隐藏。用户手动折叠或展开后，该偏好保存在当前页面状态中，不因切换会话或新建对话而重置。

Plan 面板是辅助信息，不能抢占 Agent Chat 的主要阅读空间。展开时只展示真正的 todo step，不展示 `plan_updated` title / explanation 这类二级标题；header 和每个 step 都使用紧凑字体、行高与 padding。每条 step 使用 todo 风格状态图标：completed 显示勾选，in_progress 显示旋转 loading，pending 显示空心待办圆点，不再用文字 tag 占据横向空间。step 列表最多展示 5 条的高度，超过 5 条时只在列表内部滚动，不能继续抬高 composer 或把对话区顶出可视区域。输入框默认只提供 2 行内容高度，随用户输入自动扩高，最高沿用现有 7 行上限；超过上限后由输入框内部滚动承载长文本。输入框 hint 只展示换行方式 `Shift + Enter for a new line`，不展示 session id；hint 与发送按钮的底部留白必须和顶部输入留白保持一致，避免 composer 底部出现大块空白。

消息区自身不展示全局 loading spinner。运行状态由顶部 `Running` 标签、Threads 的跳动绿点，以及 assistant 气泡中的 `Generating...` 表达；历史恢复只设置 `aria-busy`，避免左上角出现位置突兀的 loading 图标。

每条消息都必须带时间语义。JSONL history 使用事件 `timestamp`，`/sessions/{session_key}` 详情在服务端把 message timestamp 透传给前端；当前新发送消息使用发送时刻作为临时时间。时间戳展示在消息气泡外侧底部，hover 显示完整时间，不占用正文区域。assistant 消息气泡使用完整 750px 内容轨道宽度，user 消息仍右对齐并保持较窄气泡宽度。

MessageList 不渲染 user/assistant 头像，左右位置、气泡背景和顶部来源/Runner 标签已经足够区分角色；移除头像后不保留横向占位，assistant 消息直接使用完整内容轨道。Markdown 链接统一新开页面，避免点击消息里的链接覆盖当前 Agent Chat 会话页面。

Threads 列表的详情 tooltip 只属于左侧 runner/source 图标，不属于整行。用户悬浮在线程标题、meta 或空白区域时不能弹出详情；只有鼠标停在图标上超过 0.5 秒才显示 Workspace、Runner、Source、State、Created、Duration，降低扫列表时的误触干扰。

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
| 上下文过长 | Token 消耗大 | token/context budget 触发 compaction，provider 超窗时显式报错并保留 live history |
| 敏感信息泄露 | 隐私问题 | 会话不持久化 + TTL 清理 |
| 非 owner 滥用 | 成本失控 | owner_ids 白名单校验 |
| 并发请求过多 | API 限流 | 请求队列 + 限流机制 |

## 参考资料

- [OpenAI Chat Completions API](https://platform.openai.com/docs/api-reference/chat)
- [Azure OpenAI Authentication](https://learn.microsoft.com/en-us/azure/ai-services/openai/reference)
- [Feishu Message API](https://open.feishu.cn/document/server-docs/im-v1/messages/create)
