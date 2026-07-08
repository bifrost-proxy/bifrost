# IM Gateway Agent 设计方案

## 背景

Bifrost 的 IM Gateway 需要在真实 IM 通道（飞书、微信）上接入模型驱动的对话 Agent。除了走 Chat Completions API 单轮回复，还必须支撑：多轮对话、工具循环、`update_plan`、`switch_workdir`、`/clear`/`/reset`/`/status`/`/stop`、多模态图片输入、外部 CLI Runner（Codex / Claude Code / ChatGPT Web / Traex）等能力。Agent 结果既要回给 IM，也要回给 Web Agent Chat 页面，二者共享同一后端会话与 JSONL 历史。历史上出现过 tool message 序列非法、`/clear` 无效、`/stop` 卡死、`/status` 无可见运行指标、Provider agent_config 不生效、外部 Runner 状态漂移等问题，需要通过统一 invariant 收口。

## 用户目标验证清单

### 必须实现

- IM Gateway inbound → Agent loop 使用统一 `AgentSession` + `AgentSessionManager` + JSONL 持久化；Web Agent Chat、`/agent/chat` API、IM 通道共享 session key 语义。
- `AgentConfig` 提供 `enabled`/`runner`/`model`/`model_provider`/`model_providers`/`base_instructions`/`developer_instructions`/`user_instructions`/`model_reasoning_effort`/`model_reasoning_summary`/`model_context_window`(默认 250_000) / `model_auto_compact_token_limit`(默认 225_000) / `max_completion_tokens`(默认 16384) / `max_turn_iterations`(默认 1000) / `session_ttl_secs`(默认 3600) / `default_message_channel`。
- IM Provider 支持 `agent_config` 覆盖：`work_dir` / `base_instructions` / `developer_instructions` / `user_instructions`；Route `AgentChat.system_prompt` 优先级最高。
- Provider-level 覆盖热更新：`PATCH /_bifrost/api/im-gateway/providers/{id}` 立即生效；空字符串/null 清除单字段。
- `sanitize_chat_history()` 在 `build_messages`、context overflow trim、compaction 输出与 client boundary 兜底运行，永远发不出 orphan `role=tool` 或不完整 `assistant(tool_calls)`。
- `/clear` / `/reset` 在 IM built-in Agent 路径必须由主进程短路处理，同步清理 `AgentSessionManager`、guide/queue、active worker stop handle、`session_state.json` 与 JSONL；外部 Runner 按 `sessionKey + adapter + runnerId` 精确清理。
- `/status` 运行中返回 `ActiveTurnStatus`：`current_loop_iteration` / `completed_loop_iterations` / `max_loop_iterations` / `last_response_tokens` / `total_tokens_used` / `estimated_context_tokens` / `context_window_tokens`(默认 250K) / `context_usage_percent` / `compaction_count` / `work_dir` / `message_count` / `local_tool_count` / `mcp_tool_count`。
- `/stop` 作为 session-free 控制命令：设置 stop signal 立即返回，turn loop 在模型请求 / 重试 / 工具执行前后检查 signal，被中止时补齐取消 tool result 后释放 session；空闲 session 返回“当前没有正在执行的 Agent loop”。
- 多模态图片：IM 单条消息最多 6 张，超出截断记录 warn；下载后作为 OpenAI content parts `image_url` 发给模型；持久化到 JSONL `user_message.content.images`，`load_conversation()` 恢复多模态 `ChatMessage`；`/agent/chat` API 与 WebUI 粘贴共用同一 `{mime_type,data}` 结构。
- `send_msg` 工具按 provider capability 动态注入：只暴露 provider 支持的 `format=text|markdown|card`；飞书 `markdown`/`image_key`/`image` 自动构造 JSON 2.0 interactive card；tool result 只返回 safe summary（`success`/`provider_id`/`target_mode`/`msg_type`/`message_id`/`content_preview`）。
- 消息通道用 provider-neutral `ImMessageChannelBinding{provider_id,target_id,target_mode}` + `MessageTargetMode{SourceThread|SourceUser|Owner|ConfiguredTarget}` 表达，不向模型泄漏 open_id / secret。
- Agent 模型请求默认经当前 Bifrost 代理端口出站，并使用当前 Bifrost CA 信任内置 TLS intercept；worker 从 `BIFROST_ADMIN_PORT` 或 `runtime.json` 恢复端口。

### 必须不破坏

- 已有 Chat Completions API 调用与飞书 send/reply/reaction 接口原有测试。
- Feishu tenant token 缓存、message API 与 recall API 的既有单元测试。
- 已终态历史 JSONL 与 CardKit progress card；`/clear` 只清当前 session 对应文件，不影响其它 session。
- Guide/Queue 语义：内置 Bifrost Agent busy 追加消息默认 guide，外部 Runner 默认 queue；`/q` 强制 queue；turn-end 窗口消息必须 drain 到 guide/queue，不能 ACK 后丢失。
- 库级 `AgentClient::new()` 不读取 `HTTP_PROXY/HTTPS_PROXY` 等外部 proxy env；不允许被 shell/system proxy 劫持。
- `/stop` 不清空历史、不改工作目录、不影响后续普通 chat。
- MCP/Skill/AGENTS.md 现有加载路径与优先级；`switch_workdir` 后重新挂载 skills/AGENTS.md 并回写 Provider `agent_config.work_dir`。

### 必须真实验证

- 飞书真实入站消息 → Agent 回复；`/status` 显示 loop / token / context 用量；`/stop` 立即返回停止提示；`/clear` 后下一条消息不携带旧上下文。
- Provider 级 `agent_config` 修改后不需重启即生效；同 IM 通道下不同 provider agent_config 隔离。
- 多模态：飞书发送图片 + 文字，Agent 收到 `image_url` content part；WebUI 粘贴图片同样进入 content parts；Codex/ChatGPT Web/自定义 Runner 从 `attachments/images/` 与 `## Attached Images` prompt 消费本地图片。
- 长任务：turn 迭代到 1000 上限或达到 auto-compact 阈值 225K 后能自动压缩并继续。
- 外部 Runner：Codex `codex exec resume <thread_id>` 续聊、ChatGPT Web 保留 `conversationId`；Runner 完成后 `/sessions/all` 与 `/sessions/{key}` 仍能看到线程。

## 产品语义

### 一个 session key、多入口共享

`session_key` 由 provider + open_id / chat_id / web session id / runner session id 派生；同一 key 无论从 IM、Web Agent Chat 还是 `/agent/chat` API 进入，都命中同一个 `AgentSession`，共享 history、work_dir、runner state、queue/guide、compaction count 和 active turn status。JSONL 落盘位置固定 `~/.bifrost/agent/sessions/YYYY/MM/DD/session-<key>-*.jsonl`；60 分钟 TTL 后从内存回收，但 JSONL 保留供 `/resume` 与 Web 深链恢复。

### AGENTS.md、Skills、AGENTS 目录都从最终 work_dir 加载

work_dir 优先级：已存在 session 显式路径 > Provider `agent_config.work_dir`（含 `switch_workdir` 回写）> 全局 Agent `work_dir` > 进程 cwd。AGENTS.md、`.agents/skills`、home skills、词典等都跟随 work_dir；已运行 session 的 work_dir 不被 Provider 修改静默切换，避免运行中错位。

### `send_msg`：一个工具名，能力按通道裁剪

模型只看到 `send_msg` 一个工具，参数 schema 按 provider capability 动态生成；飞书暴露 `text/markdown/card/image_key/image`，微信只暴露 `text/markdown`；`target=default` 解析优先级：工具显式安全目标 > 任务绑定通道 > 当前 inbound 来源 > Agent `default_message_channel`。schedule 触发的任务始终使用 schedule 保存的通道，不会因为对话来自其它 IM 来源漂移。

### 完成状态权威来自 assistant_message，而不是 tool timeline

Chat Completions tool calling 历史必须成对：`assistant(tool_calls)` + 对应 `role=tool` 消息。`ConversationRecorder` 落盘时写入 `call_id`；`load_conversation()` 恢复时按 `call_id` 精确匹配，缺失 `call_id` 时生成 `recovered-tool-call-N`；孤立 `tool_result` 跳过。`sanitize_chat_history()` 是最后一层兜底：`build_messages()`、context trim、compaction 输出、client boundary 都执行一次；即使调用方直接构造 `system → tool → user` 也不会漏。

### Turn-end 窗口消息不丢

`run_agent_chat_with_interleave` 在 `process_agent_chat` 结束后、清理 guide/queue 前非阻塞 drain IM channel；同 session 已到达的消息通过 busy-message handler 落入 guide/queue，作为下一轮继续。这样模型刚 flush 完最终 assistant 又收到新消息时不会被 ACK 后丢。

## 技术细节

### 组件结构

- `crates/agent/src/`: `config.rs`(AgentConfig/Store) / `client.rs`(AgentClient) / `session.rs`(AgentSession, AgentSessionManager, ActiveTurnStatus) / `session/turn_loop.rs`(run_turn, run_turn_with_mcp, run_turn_with_mcp_multimodal) / `persistence.rs`(ConversationRecorder, load_conversation_lossy, validate_conversation_path) / `history.rs`(sanitize_chat_history) / `compact.rs` / `mcp/` / `tools/`.
- `crates/bifrost-admin/src/im_gateway/`: `agent.rs`(ImAgentConfig 兼容别名 re-export) / `agent_slash.rs`(`/clear` `/reset` `/stop` `/status` 短路) / `agent_worker.rs`(独立 `bifrost agent worker` 子进程入口) / `send_msg_tool.rs`(provider-neutral send_msg 注入) / `session_state.rs`(`session_state.json` per sessionKey+adapter+runnerId).
- `crates/bifrost-admin/src/handlers/im_gateway/`: `agent_chat.rs`(run_agent_chat_with_interleave, process_agent_chat) / `event_loop.rs`(IM inbound → agent 分发) / `agent_reply.rs`(with_title/with_plan/error_card) / `messages.rs`(build_rich_card_content).

### `AgentConfig` 结构（节选）

```rust
pub struct AgentConfig {
    pub enabled: bool,
    pub runner: Option<AgentRunnerMode>,
    pub model: Option<String>,                       // 默认 gpt-5.4-2026-03-05
    pub model_provider: Option<String>,              // 默认 aidp_crawl
    pub model_providers: HashMap<String, ModelProviderConfig>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub user_instructions: Option<String>,
    pub model_reasoning_effort: Option<String>,      // medium / none 支持
    pub model_reasoning_summary: Option<String>,     // auto / none 支持
    pub model_context_window: Option<i64>,           // 默认 250_000
    pub model_auto_compact_token_limit: Option<i64>, // 默认 225_000
    pub max_completion_tokens: Option<u32>,          // 默认 16384
    pub max_turn_iterations: Option<u32>,            // 默认 1000
    pub session_ttl_secs: Option<u64>,               // 默认 3600
    pub default_message_channel: Option<ImMessageChannelBinding>,
}
```

`ModelProviderConfig` 承担 `base_url`/`wire_api`/`env_key`/`api_key`/`http_headers`/`env_http_headers`/`request_max_retries`/`stream_idle_timeout_ms`/`stream_max_retries`；旧 `by_azure: bool` 已废弃，Azure `api-key` header 通过 `env_http_headers.api-key=MODELHUB_AK` 表达。

### Tool sequence invariants

`sanitize_chat_history()`：删除孤立 `role=tool`；删除不完整 `assistant(tool_calls)` 片段；发现修复时写 warn 日志包含丢弃计数。触发点：`build_messages()` max history 裁剪后；context overflow trim 每次删除后；compaction 输出后；`AgentClient` 发送 Chat Completions 请求前。

2026-05-10 补洞：session `session.rs` 在 transient retry 前重新 `build_messages(...)`，避免复用旧快照；`client.rs` boundary 兜底 sanitize 并记录 `sanitized malformed chat history at client request boundary` warn。E2E mock 按请求消息状态决定是否返回 `tool_calls`，禁止全局奇偶数策略。

### `/status` 运行时快照

`AgentSessionManager.take_session*` 在成功取出 session 时创建 `Arc<ActiveTurnStatus>`，注入到 `AgentSession.active_turn_status`。turn loop 在 `starting` / `model_request` / `model_response` / `tool_calls` 等关键阶段更新快照；`AgentSessionManager.get_active_turn_status(session_key)` 返回只读 clone。Web/IM `/status` 使用同一格式化器：Token/Context 数字统一 K/M/B（`38634→38.6K`, `250000→250K`, `1000000→1M`）。

### `/stop` 语义

`AgentSessionManager` 为每个 active turn 维护 stop signal；`/stop` 只设置 signal 立即返回，不进入模型 turn、不排队、不抢占 session lock。turn loop 在模型请求、重试等待、工具执行前后检查 signal；被命中时返回用户可见停止提示，标记 goal interrupted，为剩余已声明 tool call 写入取消 tool result，释放 session。IM 忙碌 session 的 `/stop` 同样调用 `request_stop(session_key)`，而不是 guide/queue。

### `/clear`/`/reset`

IM built-in Agent 必须主进程短路，不进入 worker turn loop。清理内容：`AgentSessionManager` 内存 session、guide/queue、active worker stop handle、`im_gateway/session_state.json` 中 built-in adapter 的 `historyPath`/`externalThreadId`/`externalConversationId`、`historyPath` 指向的 JSONL 文件。外部 Runner 按 `sessionKey + adapter + runnerId` 精确清理，先请求停止 runner 再清空 queue/guide。Stop 后 Clear 幂等：无 active worker 时也要清理成功。

### 多模态图片链路

1. `normalize_feishu_event()` 解析 `message.content`：`message_type=image` 读 `image_key`；富文本递归收集 `image_key`；写入 `ImEventMessage.images`。
2. IM handler `GET /im/v1/messages/:message_id/resources/:file_key?type=image`（tenant token）下载 bytes，从响应头 `Content-Type` 取 MIME，base64 编码；单条消息最多 6 张，超出截断记录 warn。
3. Agent runtime 使用 OpenAI content parts：`{"type":"text","text":"..."}` + `{"type":"image_url","image_url":{"url":"data:image/png;base64,...","detail":"auto"}}`。
4. JSONL `user_message.content.images` 保存 `{mime_type,data}`；`load_conversation()` 恢复多模态 `ChatMessage`；`/sessions/{key}` 返回 `messages[].content_parts` 供 Web 缩略图。
5. WebUI 粘贴走 `AgentChatSection.pendingImages`：`onPaste` 从 `clipboardData.items/files` 筛选 `image/*`，data URL → `{id,mimeType,data,previewUrl,name,size}`；最多 6 张；发送按钮启用条件 `draft.trim().length>0 || pendingImages.length>0 || running`；纯图片消息 title fallback `Attached N image(s)`。
6. 外部 Runner：`ExternalCliRuntime::run()` 在 run 目录创建 `attachments/images/`，写为 `image-1.png` 等稳定文件名；`build_prompt()` 追加 `## Attached Images` 段列出 path、mime、size 与使用指令；`ExternalCliRunResult.metadata.attachments.images` 记录路径。队列续跑不继承上一轮图片，但外部 Runner 忙碌时新进入队列的图片必须随对应 `QueueItem` 保存并在该队列项执行时恢复，不能只保留 `[图片消息]` 文本占位。`ExternalCliAgentChat` route 也必须允许纯图片消息进入 Runner，不能因为 route message text 为空跳过图片事件。

### `send_msg` 工具注入

不注册进全局 `ToolRegistry::with_defaults()`。IM Gateway 进 agent turn 前构造 `AgentMessageContext{inbound_source, task_message_channel, agent_default_channel, provider_capability}`，为本 turn 克隆 registry 并按 capability 生成 schema 与描述。`send_msg` 走 ordered 执行，不进入并行 batch；tool result 只返回 safe summary，不返回 token/secret/原始 provider config/真实 receive id。飞书特殊分支：`markdown`/`image_key`/`image` 自动构造 JSON 2.0 卡片；`image.data_base64` 先上传为 `image_key` 再入卡；只提供 `text` 时仍发文本。

### Agent 模型请求默认经 Bifrost 代理

真实 CLI 与 E2E `ProxyInstance::start_with_admin` 都使用 `ImGatewayService::new_with_agent_proxy_port(data_dir, Some(port))`；底层 `AgentClient::new_with_bifrost_proxy_and_ca(port, data_dir/certs/ca.crt)` 把 Chat Completions 请求代理到 `http://127.0.0.1:<port>`，并只信任当前 Bifrost CA。`bifrost agent worker` 子进程从父进程或 `runtime.json` 恢复端口；库级 `AgentClient::new()` 使用 `direct_reqwest_client_builder().no_proxy()`，不读取 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY`。所有 Agent HTTP 客户端（MCP Streamable HTTP / MCP OAuth / Agent 远端附件下载 / ChatGPT Web CDP HTTP 探测）必须复用 `bifrost_core` 的 direct/proxied builder，不能裸用 `reqwest::Client::new()`。stdio MCP 子进程默认移除继承 proxy env，只有 MCP server config `env` 显式写入才生效。Bifrost 长期 worker（Agent、Runner、Voice、ASR）通过 `runtime/process-aliases/` 场景别名（`bifrost-agent` / `bifrost-runner` / `bifrost-voice` / `bifrost-asr-server` / `bifrost-asr-cli`）exec，别名失败降级到原 executable 并 warn。

### `/agent/chat` `/status` 工作目录语义

`POST /_bifrost/api/im-gateway/agent/chat` 的 `/status` 是 session-free 快速命令：session 正在运行时返回 active turn status 不改 work_dir；session 不存在且请求带非空 `work_dir` 则创建空 session 并显示该路径；session 存在且请求带新 `work_dir` 使用普通 chat 的 override 逻辑重初始化 idle session；session 不存在且未带 `work_dir` 保持“新会话”纯读输出。

### Agent Chat WebUI 信息架构

Agent Chat 页右侧只承载 Threads 列表；Workspace/Status/Context/Errors/Run Settings 移入 `Agent Chat Status` 弹窗（composer 区域 New Chat 按钮旁）。线程数据来自 `/api/im-gateway/agent/sessions/all` active + history 合并，按 `session_key` 去重（active 优先）。Runner 类型（Bifrost Agent / Codex / ChatGPT Web / Unknown）用左侧图标表达；入口渠道（Web / WeChat / Feishu / ASR Task / Scheduled）在第二行；运行中线程展示跳动绿点。线程行右键 context menu：当前 Delete，删除调用 `DELETE /api/im-gateway/agent/sessions/{session_key}` 并停止运行中 turn、清理内存 session、queue/guide、session_state.json、JSONL。

标题 fallback 统一：显式 `set_title`/`title_updated` 最高优先级；否则用户第一条消息 UTF-8 安全摘要；否则前端兜底 `session_key`；`plan_updated` title 只属于 Plan 模块，不覆盖 Conversation title。

新会话未初始化前允许 New Chat 弹窗选 workspace 与 Runner；已初始化会话 Settings 中 Workspace 只读。运行中输入框不禁用：无输入时主按钮为 Stop；有输入时内置 Bifrost Agent 展示 Guide/Queue 切换（默认 Guide），Codex/ChatGPT Web/其它外部 Runner 只支持 Queue。Queue 是本地交互态，不写入 JSONL、不作为 assistant 气泡，只有真正 drain 成下一轮才落 history。Plan 面板放输入框上方，最多 5 条 step 高度内滚，超出不抬 composer。消息区不显示全局 loading spinner；assistant 气泡 `Generating...` + Threads 跳动绿点表达运行态。

刷新页面或关闭浏览器不代表停止 loop：SSE/NDJSON client disconnect 只停止写入增量，不调用 `request_stop` 或 external CLI stop marker；只有显式点击 Stop 或发 `/stop` 才写 stop signal。切换线程使用 `AbortController` 中止旧 stream，用 `sessionKey` guard 丢弃旧会话延迟事件；`finally` 中不无条件 `setRunning(false)`。

## CLI + Web + Admin API

- CLI：`bifrost agent worker`（独立子进程）；`bifrost agent run`（Server 模式，调用 `/api/im-gateway/chat/stream`）；`bifrost install-skill`。
- Admin API：`GET/PATCH /api/im-gateway/agent`；`GET /api/im-gateway/agent/sessions`；`GET /api/im-gateway/agent/sessions/all`；`GET/DELETE /api/im-gateway/agent/sessions/{session_key}`；`GET/DELETE /api/im-gateway/agent/sessions/history/*`；`POST /api/im-gateway/agent/chat`；`POST /agent/chat/stream`；`POST /api/im-gateway/agent/chat` 的 `/status` `/stop`；`GET /api/im-gateway/agent/instructions`。
- Web：Settings → AI → Agent（General/Model/Runtime/History/Memories/Skills/Memory Records/MCP Servers/Sessions），二级卡片导航用 `agentSection` URL 参数；AI → Agent Chat 页面（Threads + Conversation + Composer + Status 弹窗）；Settings → IM Gateway → Provider Edit 弹窗支持 Agent Work Dir 与三段 instructions 大窗口编辑。

## Sync 边界

- `agent_config.json` 属于本地 Agent 配置，不进 Sync。
- Agent session JSONL 与内存 session 不进 Sync。
- Remote Invoke 只能读安全摘要；`bifrost remote im ...` 不返回 provider secret / token / 完整 provider config / raw event payload。
- `im_gateway/session_state.json` 是本地 runner state，禁止镜像。

## 实现切分

### Phase 1：session + history 稳定性

- `AgentSession` / `AgentSessionManager` / `ActiveTurnStatus` 落地。
- `ConversationRecorder` 带 `call_id`；`load_conversation()` 按 `call_id` 匹配、缺失时生成 `recovered-tool-call-N`；孤立 tool_result 跳过。
- `sanitize_chat_history()` 与四个触发点（build_messages / trim / compaction / client boundary）；retry 前重新 `build_messages`。
- 单元测试：`test_build_messages_uses_full_sanitized_history`、`test_agent_client_request_build`、`test_agent_client_azure_auth`、`test_active_turn_status`。

### Phase 2：IM Provider agent_config 与 send_msg

- Provider `agent_config`（`work_dir` / `base_instructions` / `developer_instructions` / `user_instructions`）热更新；空字符串/null 清除单字段。
- `AgentMessageContext` + `send_msg` 动态注入；飞书 markdown/image_key/image 自动构造 JSON 2.0 卡片。
- IM `/clear` / `/reset` 主进程短路 + `session_state.json` 清理；`/stop` stop signal。
- 回归：`im_event_loop_uses_provider_agent_config_for_agent_chat`、`im_gateway_agent_chat_stop_active_loop`。

### Phase 3：多模态 + WebUI 完善

- 飞书图片下载 + 6 张上限 + JSONL content parts 恢复。
- `/agent/chat` `images` 字段与 WebUI 粘贴 Composer；`ExternalCliRuntime` `attachments/images/` + `## Attached Images` prompt。
- Web Agent Chat Threads/Conversation/Composer 信息架构；Settings 三段 instructions 大窗口编辑。
- 回归：`im_gateway_agent_chat_multimodal_image_parts`、`external_cli_run_writes_image_attachments_and_injects_prompt_paths`、`session_state_persists_message_content_parts`。

### Phase 4：Runner 观测 + Web 恢复

- Runner 元信息（`threadId` / `conversationId`）进入 `/status`；Token/Context K/M/B 格式化；压缩次数 JSONL 恢复。
- Web Agent Chat active preview + `sessions/all` running 一等成员；外部 Runner 多轮消息序列保存到 `session_state.json`；ChatGPT Web DOM fallback 覆盖 `button.behavior-btn` 图片/ZIP。
- `human_tests/im-gateway-agent.md` 覆盖 TC-IMA-53D / TC-IMA-84 / TC-IMA-85 / TC-IMA-87 / TC-IMA-89 / TC-IMA-90 / TC-IMA-91 / TC-IMA-116 / TC-LTM-09 等。

## 测试方案

### 单元测试

- `test_im_agent_config_env_var`、`test_session_manager_ttl`、`test_build_messages_uses_full_sanitized_history`、`test_agent_client_request_build`、`test_agent_client_azure_auth`、`test_builtin_commands`。
- `session::tests::test_stop_request_cancels_in_flight_model_request`；`session::tests::test_active_turn_status`。
- `bifrost_agent::types::tests::user_with_images_serializes_openai_content_parts`；`im_gateway::feishu::tests::test_normalize_feishu_image_message_extracts_resource_key`；`handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat`。
- `external_cli::tests::external_cli_run_writes_image_attachments_and_injects_prompt_paths`；`session_state::tests::session_state_persists_message_content_parts`。
- `im_gateway::queue_manager::tests::test_queue_preserves_image_attachments`；`handlers::im_gateway::tests::im_event_loop_external_cli_route_processes_image_only_message`。
- `agent_api_status_detail_applies_work_dir_for_fresh_status_session`、`agent_api_status_detail_overrides_existing_idle_session_work_dir`、`agent_api_status_detail_keeps_new_session_text_when_no_work_dir_requested`。

### E2E 测试

- `im_gateway_agent_tool_history_resume_regression`、`im_gateway_agent_retry_sanitizes_orphan_tool_history`。
- `im_gateway_agent_chat_multimodal_image_parts`、`im_gateway_agent_chat_stop_active_loop`。
- `im_event_loop_uses_provider_agent_config_for_agent_chat`；`im_gateway_agent_model_request_uses_bifrost_proxy`。
- `e2e-tests/tests/test_long_term_memory_human_api.sh`、`test_update_plan_human_api.sh`、`test_agent_loop_runtime_limits.sh`、`test_im_gateway_traex_model_slash.sh`。

### 真实场景测试

`human_tests/im-gateway-agent.md`、`human_tests/im-guide-queue-mode.md`、`human_tests/long-term-memory.md`、`human_tests/agent-builtin-commands.md`：TC-AG-01～06、TC-IMA-53D / 66 / 67 / 83 / 83A / 83B / 84 / 84A / 85 / 87 / 89 / 90A / 90B / 91 / 91A / 92 / 116 / 139 / 140、TC-GQ-04～06 / 14 / 15 / 16、TC-BC-34、TC-ASP-14 / 15、TC-LTM-09。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent`
- `cargo test -p bifrost-admin im_gateway::`
- `cargo test --workspace --all-features`
- 按修改范围评估 `scripts/ci/local-ci.sh`；本机 no-local-coverage 约定下不跑 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 tool sequence invariants（四个 sanitize 触发点、retry rebuild、client boundary）。
- 复核 `/status` `/stop` `/clear` 主进程短路与幂等；`session_state.json` 清理不留 orphan。
- 复核 Provider agent_config 热更新与 work_dir 优先级；`switch_workdir` 回写；`send_msg` provider capability 裁剪。
- 复跑受影响单元与 E2E；抓真实 IM 消息与 `/agent/chat` payload 抽样。

### 第 2 轮

- 复查第 1 轮修复 diff；确认 `sessions/all` running 一等成员、外部 Runner 多轮消息序列保存与 ChatGPT Web DOM fallback。
- 复跑 WebUI Threads/Composer/Plan/图片粘贴/主题（亮暗）用例；human_tests 索引与真实浏览器表现。
- 如仍发现 orphan tool / stop 未清 / clear 未彻底 / 图片截断 / send_msg 泄漏 等缺口，追加第 3 轮。

## 风险与决策点

- **历史无固定条数上限**：Agent loop 使用完整 sanitized history，由 token/context budget、auto-compact 与 provider context-window 错误共同收口；避免因固定 N 消息裁剪破坏语义。
- **不承诺流式回复**：飞书消息 API 不支持流式编辑，最终 assistant 一次性写入；进度通过 CardKit streaming card 表达。
- **敏感信息**：`send_msg` tool result / message log / `/sessions` API / remote invoke 全部只返回 safe summary，不返回 open_id / token / secret / receive id。
- **worker 进程边界**：Agent loop 在 `bifrost-agent` 场景别名子进程执行，父进程必须传 Bifrost 端口；worker 从 env / runtime.json 恢复；库级 `AgentClient::new()` 不读外部 proxy env。
- **外部 Runner 会话续接**：Codex 用 `codex exec resume <thread_id>`；ChatGPT Web 保留 `conversationId`；Runner 完成后消息序列必须追加到同一 `sessionKey + adapter + runnerId`，避免 5 轮及以上多轮漂移成多个线程。
- **模型能力诚实**：`model_reasoning_effort=none` / `model_reasoning_summary=none` 时不发对应 Chat Completions 字段；无法读到当前模型能力时 fallback runner 兼容默认并注明来源。
